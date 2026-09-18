//! Camera raw support, whatever camera wrote it.
//!
//! Decoding is delegated to [`rawler`], which is pure Rust — no CMake, no
//! vcpkg, no C toolchain. This crate's job is to map rawler's model onto the
//! stacker's own, and to be honest about what it cannot express.
//!
//! What can be read is rawler's answer rather than a list kept here: the
//! extensions come from [`rawler::decoders::supported_extensions`] at run time,
//! and a file is identified by asking rawler for a decoder. Neither is a table
//! in this crate, so neither can drift from what the library actually does.
//!
//! That is a deliberate widening, and its cost is real: frames from six Canon
//! bodies have been decoded and stacked here, and the rest of the cameras
//! rawler names have not been tried. Offering them anyway was the owner's call
//! — see CLAUDE.md, which carries the rule this replaced.

mod datetime;

use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Mutex;

use astro_core::format::{Format, Frame};
use astro_core::frame::{
    CFA_MAX_CELLS, CFA_MAX_DIM, COLOR_BLUE, COLOR_GREEN, COLOR_RED, FormatDescription, FrameInfo,
    ImageLayout, PROBE_CERTAIN, PROBE_UNSUPPORTED, SampleFormat, samples_u16_mut,
};
use rawler::cfa::CFAColor;
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::formats::tiff::Rational;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::rawsource::RawSource;
use rawler::{RawImage, RawImageData};

const ID: &str = "camera-raw";

/// The decoder itself: no state, since everything it needs comes with the file.
pub struct Raw {
    description: FormatDescription,
}

impl Default for Raw {
    fn default() -> Self {
        Self::new()
    }
}

impl Raw {
    pub fn new() -> Self {
        Self { description: describe() }
    }
}

/// One raw file, opened and understood.
pub struct RawFrame {
    /// Kept so a failure partway through decoding can say which file it was.
    path: std::path::PathBuf,
    /// Memory-mapped file. Kept open so decoding does not re-read from disk.
    source: RawSource,
    /// `rawler`'s decoders are `Send` but not `Sync`, and a frame is read from
    /// whichever worker thread owns it.
    decoder: Mutex<Box<dyn Decoder>>,
    layout: ImageLayout,
    info: FrameInfo,
}

fn describe() -> FormatDescription {
    FormatDescription {
        id: ID.to_owned(),
        display_name: "Camera raw".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        author: "Pavel Belov".to_owned(),
        // rawler's own list rather than one kept here, so the two cannot
        // disagree. It is upper-case at the source; paths are matched in lower.
        extensions: {
            let mut extensions: Vec<String> = rawler::decoders::supported_extensions()
                .iter()
                .map(|extension| extension.to_lowercase())
                .collect();
            extensions.sort();
            extensions
        },
    }
}

impl Format for Raw {
    fn description(&self) -> &FormatDescription {
        &self.description
    }

    /// Asks rawler whether it has a decoder for the file, which means opening
    /// it rather than reading the header the registry offers.
    ///
    /// Measured rather than assumed: `get_decoder` builds the decoder, and the
    /// index structures it parses sit far past the first four kilobytes, so on
    /// a 4 KiB slice it identified none of six real Canon frames. Given the
    /// whole file it identifies all six in about 0.3 ms and refuses a JPEG and
    /// a plain TIFF. A table of container magics kept here would be a second
    /// copy of what rawler already knows, and the copy that goes stale quietly.
    fn probe(&self, path: &Path, _header: &[u8]) -> i32 {
        RawSource::new(path)
            .ok()
            .filter(|source| rawler::get_decoder(source).is_ok())
            .map_or(PROBE_UNSUPPORTED, |_| PROBE_CERTAIN)
    }

    fn open(&self, path: &Path) -> astro_core::Result<Box<dyn Frame>> {
        RawFrame::open(path)
            .map(|frame| Box::new(frame) as Box<dyn Frame>)
            .map_err(|why| why.at(path))
    }
}

impl RawFrame {
    fn open(path: &Path) -> Result<Self, Unreadable> {
        let source = RawSource::new(path).map_err(|err| Unreadable::io(err.to_string()))?;
        let decoder = rawler::get_decoder(&source)
            .map_err(|err| Unreadable::unsupported(err.to_string()))?;

        // Metadata first: the orientation in the layout has to come from EXIF,
        // because rawler does not carry it on the decoded image (see
        // `exif_orientation`).
        let metadata = contained(|| decoder.raw_metadata(&source, &RawDecodeParams::default()))?
            .map_err(|err| Unreadable::parse(err.to_string()))?;

        // `dummy` builds the full description — dimensions, CFA, levels — while
        // skipping the expensive pixel decompression. Opening a frame stays
        // cheap, which matters when a session holds hundreds of them.
        let described = contained(|| decoder.raw_image(&source, &RawDecodeParams::default(), true))?
            .map_err(|err| Unreadable::parse(err.to_string()))?;

        let layout = layout_from(&described, &metadata)?;
        let info = info_from(&metadata);

        Ok(Self { path: path.to_owned(), source, decoder: Mutex::new(decoder), layout, info })
    }

    fn decode_into(&self, dst: &mut [u8]) -> Result<(), Unreadable> {
        let samples = samples_u16_mut(dst).ok_or_else(|| {
            Unreadable::invalid_argument("destination buffer is misaligned for 16-bit samples")
        })?;

        let decoder = self
            .decoder
            .lock()
            .map_err(|_| Unreadable::internal("decoder lock was poisoned by an earlier panic"))?;
        let image = contained(|| decoder.raw_image(&self.source, &RawDecodeParams::default(), false))?
            .map_err(|err| Unreadable::parse(err.to_string()))?;

        let RawImageData::Integer(data) = image.data else {
            return Err(Unreadable::internal(
                "decoder returned floating-point data after describing an integer frame",
            ));
        };
        if data.len() != samples.len() {
            return Err(Unreadable::internal(format!(
                "decoder produced {} samples but the layout promised {}",
                data.len(),
                samples.len()
            )));
        }
        samples.copy_from_slice(&data);
        Ok(())
    }
}

impl Frame for RawFrame {
    fn layout(&self) -> &ImageLayout {
        &self.layout
    }

    fn info(&self) -> &FrameInfo {
        &self.info
    }

    fn read_samples(&self, dst: &mut [u8]) -> astro_core::Result<()> {
        self.decode_into(dst).map_err(|why| why.at(&self.path))
    }
}

/// Why a file could not be read, before it is attached to the file.
///
/// The decoder knows what went wrong; which file it was is the caller's to say,
/// so the two are joined at the boundary rather than threaded through every
/// helper.
struct Unreadable(String);

impl Unreadable {
    fn io(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    fn unsupported(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    fn parse(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    fn invalid_argument(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    fn internal(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn at(self, path: &Path) -> astro_core::Error {
        astro_core::Error::Decode {
            format: ID.to_owned(),
            action: "read",
            path: path.to_owned(),
            message: self.0,
        }
    }
}

// ---------------------------------------------------------------------------
// rawler -> ABI
// ---------------------------------------------------------------------------

fn layout_from(raw: &RawImage, metadata: &RawMetadata) -> Result<ImageLayout, Unreadable> {
    if !matches!(raw.data, RawImageData::Integer(_)) {
        return Err(Unreadable::unsupported(
            "this decoder reads integer sensor data only, and this file stores floating-point samples",
        ));
    }

    let mut layout = ImageLayout {
        width: raw.width as u32,
        height: raw.height as u32,
        components: raw.cpp as u32,
        bits_per_sample: raw.bps as u32,
        sample_format: SampleFormat::U16,
        orientation: exif_orientation(metadata),
        white_level: raw.whitelevel.as_bayer_array(),
        wb_coeffs: raw.wb_coeffs,
        ..Default::default()
    };

    for (row, values) in raw.xyz_to_cam.iter().enumerate() {
        layout.xyz_to_cam[row * 3..row * 3 + 3].copy_from_slice(values);
    }

    copy_cfa(raw, &mut layout);
    copy_black_level(raw, &mut layout);
    copy_active_area(raw, &mut layout);

    Ok(layout)
}

fn copy_cfa(raw: &RawImage, layout: &mut ImageLayout) {
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        // Already demosaiced, or plain greyscale. Leaving the CFA size at zero
        // tells the host not to demosaic.
        return;
    };
    let cfa = &config.cfa;
    if !cfa.is_valid() || cfa.width == 0 || cfa.height == 0 {
        return;
    }
    if cfa.width > CFA_MAX_DIM || cfa.height > CFA_MAX_DIM {
        return;
    }

    let mut pattern = [0u8; CFA_MAX_CELLS];
    for row in 0..cfa.height {
        for col in 0..cfa.width {
            match filter_color(cfa.cfa_color_at(row, col)) {
                Some(color) => pattern[row * cfa.width + col] = color,
                // A CYGM or otherwise unrecognised filter. Report no CFA at all
                // rather than a pattern the host would silently misread.
                None => return,
            }
        }
    }

    layout.cfa_width = cfa.width as u32;
    layout.cfa_height = cfa.height as u32;
    layout.cfa_pattern = pattern;
}

fn filter_color(color: CFAColor) -> Option<u8> {
    match color {
        CFAColor::RED => Some(COLOR_RED),
        CFAColor::GREEN | CFAColor::FUJI_GREEN => Some(COLOR_GREEN),
        CFAColor::BLUE => Some(COLOR_BLUE),
        _ => None,
    }
}

fn copy_black_level(raw: &RawImage, layout: &mut ImageLayout) {
    let black = &raw.blacklevel;
    let cells = black.width.saturating_mul(black.height);
    let expressible = black.cpp == 1
        && black.width <= CFA_MAX_DIM
        && black.height <= CFA_MAX_DIM
        && black.levels.len() == cells
        && cells > 0;

    if expressible {
        layout.black_level_width = black.width as u32;
        layout.black_level_height = black.height as u32;
        for (cell, level) in black.levels.iter().enumerate() {
            layout.black_level[cell] = level.as_f32();
        }
    } else {
        // A grid we cannot express is still better collapsed to one level than
        // dropped: calibration needs *a* black point. But when there is no level
        // at all, say so — NaN rather than zero, because a fabricated pedestal
        // gets subtracted from real data with nothing looking wrong.
        layout.black_level_width = 1;
        layout.black_level_height = 1;
        layout.black_level[0] = black.levels.first().map_or(f32::NAN, Rational::as_f32);
    }
}

fn copy_active_area(raw: &RawImage, layout: &mut ImageLayout) {
    // `crop_area` is the manufacturer's recommended crop; `active_area` is
    // everything that saw light. Astro work wants the latter, because the
    // masked border is what calibrates the black level.
    let area = raw.active_area.or(raw.crop_area).map(|area| (area.p.x, area.p.y, area.d.w, area.d.h));
    let (x, y, width, height) = active_area(area, layout.width, layout.height);
    layout.active_x = x;
    layout.active_y = y;
    layout.active_width = width;
    layout.active_height = height;
}

/// The light-sensitive region, or the whole frame when what the decoder reports
/// cannot be believed.
///
/// It has to be checked rather than trusted, and the reason is specific.
/// rawler builds these rectangles by subtracting camera-database borders from
/// the frame's own size, and the database describes the whole sensor. A body
/// shooting in a crop mode writes a smaller frame, the subtraction runs past
/// zero, and what it does then depends on the build: measured against rawler
/// 0.7.2, a debug build panics on the overflow, while a release build — the one
/// that ships — wraps and hands over a rectangle about 1.8e19 wide, with no
/// error anywhere. Arriving at the whole frame is right in both cases. We
/// deposit photosites rather than crop, so the only thing the active area
/// decides is which pixels count as optically black, and a frame shot in crop
/// mode has no masked border to find.
///
/// The checks widen to `u64` first: on a 64-bit target the wrapped value is
/// close enough to `usize::MAX` that adding the offset to it wraps a second
/// time, and the comparison would pass.
fn active_area(area: Option<(usize, usize, usize, usize)>, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let whole = (0, 0, width, height);
    let Some((x, y, area_width, area_height)) = area else {
        return whole;
    };

    let fits = |start: usize, span: usize, limit: u32| {
        span > 0 && (start as u64).checked_add(span as u64).is_some_and(|end| end <= u64::from(limit))
    };
    if !fits(x, area_width, width) || !fits(y, area_height, height) {
        return whole;
    }

    // Each is at most its limit, and the limits are `u32`.
    (x as u32, y as u32, area_width as u32, area_height as u32)
}

/// Runs a step of rawler's, turning a panic inside it into an `Unreadable`.
///
/// The registry catches decoder panics too, but as a last resort and with
/// nothing to say beyond what the panic said. Catching here, where the step is
/// known, is what makes a sentence possible.
///
/// `AssertUnwindSafe` is honest because nothing the call touched is read after
/// a panic: the value never materialises, and a half-written destination buffer
/// leaves with the error.
fn contained<T>(call: impl FnOnce() -> T) -> Result<T, Unreadable> {
    std::panic::catch_unwind(AssertUnwindSafe(call)).map_err(|panic| {
        let said = panic
            .downcast_ref::<&str>()
            .map(|said| (*said).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_owned());
        Unreadable::parse(why_it_fell(&said))
    })
}

/// What to tell the user when the decoder falls over, which for one cause is
/// more than the panic itself says.
///
/// Relaying `assertion failed: p1.x <= p2.x` asks the user to debug a library
/// they did not install. The assertion has a known cause worth naming instead,
/// and it is the same one `active_area` guards against.
fn why_it_fell(said: &str) -> String {
    let geometry = said.contains("attempt to subtract with overflow")
        || said.contains("p1.x <= p2.x")
        || said.contains("p1.y <= p2.y");
    if geometry {
        // `concat!` rather than a backslash continuation: the continuation is
        // one edit away from collapsing into a run of spaces inside the
        // sentence, which is what happened when this was first written.
        concat!(
            "the frame is smaller than the decoder's camera database expects, which is what a ",
            "body shooting in a crop mode produces; this file cannot be read yet",
        )
        .to_owned()
    } else {
        format!("the decoder fell over: {said}")
    }
}

/// Reads orientation from EXIF rather than from the decoded image.
///
/// `RawImage::orientation` looks like the obvious source and is not one:
/// rawler 0.7.2 hardcodes it to `Orientation::Normal` in both constructors
/// (`src/rawimage.rs:389` and `:478`, each marked `// TODO fixme`), so reading
/// it would report every frame as upright whichever way the camera pointed —
/// a fabricated value, which this project treats as worse than none.
///
/// Returns 0 for absent or out-of-range, per the ABI. Note that rawler never
/// permutes the sample buffer either way, so this is display metadata only.
fn exif_orientation(metadata: &RawMetadata) -> u32 {
    match metadata.exif.orientation {
        Some(value @ 1..=8) => u32::from(value),
        _ => 0,
    }
}

fn info_from(metadata: &RawMetadata) -> FrameInfo {
    let exif = &metadata.exif;
    FrameInfo {
        camera_make: metadata.make.clone(),
        camera_model: metadata.model.clone(),
        lens_model: metadata
            .lens
            .as_ref()
            .map(|lens| lens.lens_name.clone())
            .or_else(|| exif.lens_model.clone())
            .unwrap_or_default(),
        exposure_seconds: exif.exposure_time.and_then(positive_rational),
        iso: iso_speed(exif.iso_speed_ratings, exif.iso_speed),
        aperture: exif.fnumber.and_then(positive_rational),
        focal_length_mm: exif.focal_length.and_then(positive_rational),
        capture_time_unix: exif
            .date_time_original
            .as_deref()
            .or(exif.create_date.as_deref())
            .and_then(datetime::parse_exif),
        // Cameras that record sensor temperature keep it in their makernotes,
        // and rawler does not surface it in the generalised metadata. Claiming
        // a value we cannot read would be worse than admitting we have none.
        sensor_temperature_c: None,
    }
}

/// EXIF stores ISO in a 16-bit tag that saturates at 65535; cameras that shoot
/// past it record the real value in the 32-bit tag instead.
fn iso_speed(ratings: Option<u16>, extended: Option<u32>) -> Option<f64> {
    match (ratings, extended) {
        (Some(u16::MAX), Some(extended)) => Some(extended as f64),
        (Some(ratings), _) => Some(ratings as f64),
        (None, Some(extended)) => Some(extended as f64),
        (None, None) => None,
    }
}

/// Reads a rational that is only meaningful when positive.
///
/// Exposure time, aperture and focal length are each strictly positive on a
/// frame that recorded them. A body with no electronic lens — every telescope —
/// writes `0/1` for aperture and focal length, and returning `Some(0.0)` there
/// would let two telescope frames compare equal at "f/0, 0 mm" and report a
/// matching optical train having compared nothing.
fn positive_rational(value: Rational) -> Option<f64> {
    if value.d == 0 || value.n == 0 {
        return None;
    }
    let value = value.n as f64 / value.d as f64;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;


    /// One case per way the active area can be wrong, because the numbers come
    /// from arithmetic that runs past zero rather than from a file.
    #[test]
    fn the_active_area_is_the_whole_frame_whenever_it_cannot_be_believed() {
        // Nothing reported.
        assert_eq!(active_area(None, 6000, 4000), (0, 0, 6000, 4000));
        // An ordinary frame: kept as it is.
        assert_eq!(active_area(Some((144, 60, 5800, 3900)), 6000, 4000), (144, 60, 5800, 3900));
        // Exactly filling the frame is still believable.
        assert_eq!(active_area(Some((0, 0, 6000, 4000)), 6000, 4000), (0, 0, 6000, 4000));
        // An empty span says nothing about where the light fell.
        assert_eq!(active_area(Some((0, 0, 0, 4000)), 6000, 4000), (0, 0, 6000, 4000));
        // Reaching past the frame by one pixel.
        assert_eq!(active_area(Some((1, 0, 6000, 4000)), 6000, 4000), (0, 0, 6000, 4000));

        // What a crop-mode frame actually produces, measured from rawler 0.7.2
        // in a release build: 5568 - 6000 wrapped, and no error anywhere.
        let wrapped = 5568usize.wrapping_sub(6000);
        assert_eq!(active_area(Some((0, 0, wrapped, 1774)), 5568, 3712), (0, 0, 5568, 3712));
        // And with an offset large enough to carry the sum past the end and
        // back to a small number — the case that defeats a check done in
        // `usize`, because the area would then have looked as though it fitted.
        assert_eq!(500usize.wrapping_add(wrapped), 68);
        assert_eq!(active_area(Some((500, 60, wrapped, 1774)), 5568, 3712), (0, 0, 5568, 3712));
    }

    /// The assertion rawler raises is not something a user can act on, and the
    /// cause is known. This calls rawler's own arithmetic rather than a copy of
    /// its message, so the day that message changes the test says so.
    #[test]
    fn a_frame_smaller_than_the_database_expects_is_named_not_relayed() {
        use rawler::imgop::{Dim2, Rect};

        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        // Borders measured against a full sensor, applied to the smaller frame
        // a body in a crop mode writes. This shape falls over in both profiles;
        // the plain overshoot only does in a debug build.
        let fell = std::panic::catch_unwind(|| {
            Rect::new_with_borders(Dim2::new(5568, 3712), &[3000, 60, 3000, 80])
        });
        std::panic::set_hook(hook);

        let panic = fell.expect_err("rawler still refuses borders wider than the frame");
        let said = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|said| (*said).to_owned()))
            .expect("a panic carries a message");

        let told = why_it_fell(&said);
        assert!(told.contains("crop mode"), "{said} -> {told}");
        assert!(!told.contains("p1."), "the assertion must not reach the user: {told}");
        // A sentence shown to a user, so it has to read like one: the first
        // version of it carried ten spaces where a line break had collapsed.
        assert!(!told.contains("  "), "doubled spaces in a sentence: {told}");
    }

    /// Anything else is still passed on: a cause we have not met is better
    /// reported verbatim than described wrongly.
    #[test]
    fn an_unfamiliar_panic_is_repeated_rather_than_explained() {
        let told = why_it_fell("index out of bounds: the len is 3 but the index is 7");
        assert!(told.contains("index out of bounds"), "{told}");
        assert!(!told.contains("crop mode"), "{told}");
    }

    fn testdata() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
    }

    /// Raw files directly inside `dir`. Gitignored, so a test that needs them
    /// says nothing rather than failing on a machine without them.
    fn frames_in(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found: Vec<std::path::PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension().is_some_and(|extension| {
                    let extension = extension.to_string_lossy().to_lowercase();
                    extension == "cr2" || extension == "cr3"
                })
            })
            .collect();
        found.sort();
        found
    }

    /// Frames from the owner's bodies, one file per camera.
    fn real_frames() -> Vec<std::path::PathBuf> {
        frames_in(&testdata())
    }

    fn open_layout(path: &std::path::Path) -> ImageLayout {
        let frame = Raw::new().open(path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
        *frame.layout()
    }

    /// The frame this entry was written about: an R5 Mark II in its 1.6x crop
    /// mode, which the sibling project measured the same decoder falling over
    /// on. On this path it reads, and these numbers are the camera's rather
    /// than merely plausible ones — the masked border is the same width as in a
    /// full-frame file from the same body, because the optically black columns
    /// are a property of the sensor and do not shrink with the crop.
    #[test]
    fn a_crop_mode_frame_reads_with_the_cameras_own_geometry() {
        let Some(path) = frames_in(&testdata().join("crop")).pop() else {
            return;
        };
        let full_path = testdata().join("CanonR5m2.CR3");
        if !full_path.exists() {
            return;
        }

        assert_eq!(Raw::new().probe(&path, &[]), PROBE_CERTAIN, "{}", path.display());
        let crop = open_layout(&path);
        let full = open_layout(&full_path);

        assert!(crop.width < full.width && crop.height < full.height, "{crop:?}");

        // The active area is inside the frame and is not the whole of it: were
        // the numbers unbelievable, `active_area` would have fallen back to the
        // frame and this is what would say so.
        assert!(crop.active_width > 0 && crop.active_height > 0, "{crop:?}");
        assert!(crop.active_x + crop.active_width <= crop.width, "{crop:?}");
        assert!(crop.active_y + crop.active_height <= crop.height, "{crop:?}");
        assert!(crop.active_width < crop.width, "fell back to the whole frame: {crop:?}");

        // The same masked border in both, which is what makes the geometry the
        // sensor's rather than a coincidence that happens to fit.
        assert_eq!(
            crop.width - crop.active_width,
            full.width - full.active_width,
            "masked columns must not shrink with the crop: {crop:?} against {full:?}",
        );

        // And the pixels arrive: the crop factor this body crops by is 1.6, and
        // a decode that produced the wrong count would not reach here at all.
        let ratio = f64::from(full.active_width) / f64::from(crop.active_width);
        assert!((1.55..1.65).contains(&ratio), "crop factor {ratio:.3}");

        let frame = Raw::new().open(&path).expect("opens");
        let mut buffer = vec![0u8; frame.layout().required_bytes().expect("a sane layout")];
        frame.read_samples(&mut buffer).expect("a crop-mode frame must decode");
        assert_eq!(buffer.len(), crop.width as usize * crop.height as usize * 2);
    }

    /// The probe opens the file instead of reading the header it is handed, and
    /// this is the test that would catch the day that stops being necessary or
    /// stops working. Both directions matter: a frame must be claimed, and the
    /// program's own output must not be, or a stack dropped back into a session
    /// would be read as a light.
    #[test]
    fn claims_a_real_frame_and_refuses_what_is_not_one() {
        let frames = real_frames();
        if frames.is_empty() {
            return;
        }
        let format = Raw::new();
        for path in &frames {
            assert_eq!(format.probe(path, &[]), PROBE_CERTAIN, "{}", path.display());
        }

        let root = frames[0].parent().expect("testdata").to_owned();
        for name in ["2026-09-11-astro/stack-as.tif", "2026-09-11-astro/stack-as.jpg"] {
            let path = root.join(name);
            if path.exists() {
                assert_eq!(format.probe(&path, &[]), PROBE_UNSUPPORTED, "{}", path.display());
            }
        }
        assert_eq!(format.probe(&root.join("no-such-file.cr2"), &[]), PROBE_UNSUPPORTED);
    }

    /// The extensions are rawler's rather than ours; what this pins is the
    /// shape they are handed over in, since paths are matched in lower case.
    #[test]
    fn declares_rawlers_extensions_in_lower_case() {
        let description = Raw::new().description().clone();
        assert!(description.extensions.len() > 20, "{:?}", description.extensions);
        assert!(description.extensions.iter().all(|e| e == &e.to_lowercase()));
        assert!(description.extensions.windows(2).all(|pair| pair[0] < pair[1]));
        for known in ["cr2", "cr3", "nef", "arw", "dng"] {
            assert!(description.extensions.iter().any(|e| e == known), "{known} missing");
        }
    }

    #[test]
    fn iso_prefers_the_extended_tag_only_when_the_short_one_saturated() {
        assert_eq!(iso_speed(Some(1600), None), Some(1600.0));
        assert_eq!(iso_speed(Some(u16::MAX), Some(102_400)), Some(102_400.0));
        // A camera that genuinely shot at 65535 and recorded nothing else.
        assert_eq!(iso_speed(Some(u16::MAX), None), Some(65535.0));
        assert_eq!(iso_speed(None, Some(400)), Some(400.0));
        assert_eq!(iso_speed(None, None), None);
    }

    #[test]
    fn a_lens_less_body_reports_no_aperture_rather_than_f_zero() {
        assert_eq!(positive_rational(Rational { n: 1, d: 4 }), Some(0.25));
        // What a telescope writes: the tag is present and says nothing.
        assert_eq!(positive_rational(Rational { n: 0, d: 1 }), None);
        assert_eq!(positive_rational(Rational { n: 1, d: 0 }), None);
    }
}
