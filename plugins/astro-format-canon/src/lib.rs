//! Canon CR2 and CR3 support, as a loadable plugin.
//!
//! Decoding is delegated to [`rawler`], which is pure Rust — no CMake, no
//! vcpkg, no C toolchain. This crate's job is to map rawler's model onto the
//! stacker's ABI, and to be honest about what it cannot express.

mod datetime;
mod magic;

use std::path::Path;
use std::sync::Mutex;

use astro_plugin_abi::abi::{
    CFA_MAX_CELLS, CFA_MAX_DIM, COLOR_BLUE, COLOR_GREEN, COLOR_RED, ImageLayout, PROBE_UNSUPPORTED,
    SampleFormat,
};
use astro_plugin_abi::export::FormatPlugin;
use astro_plugin_abi::safe::{FrameInfo, PluginDescription, PluginError, samples_u16_mut};
use rawler::cfa::CFAColor;
use rawler::decoders::{Decoder, Orientation, RawDecodeParams, RawMetadata};
use rawler::formats::tiff::Rational;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::rawsource::RawSource;
use rawler::{RawImage, RawImageData};

const EXTENSIONS: [&str; 2] = ["cr2", "cr3"];

pub struct CanonRaw {
    /// Memory-mapped file. Kept open so decoding does not re-read from disk.
    source: RawSource,
    /// `rawler`'s decoders are `Send` but not `Sync`, and the ABI allows the
    /// host to call into a handle from whichever worker thread owns it.
    decoder: Mutex<Box<dyn Decoder>>,
    layout: ImageLayout,
    info: FrameInfo,
}

impl FormatPlugin for CanonRaw {
    fn description() -> PluginDescription {
        PluginDescription {
            id: "canon-raw".to_owned(),
            display_name: "Canon CR2/CR3".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            author: "Pavel Belov".to_owned(),
            extensions: EXTENSIONS.iter().map(|e| (*e).to_owned()).collect(),
        }
    }

    fn probe(_path: &Path, header: &[u8]) -> i32 {
        magic::identify(header).map_or(PROBE_UNSUPPORTED, |_| astro_plugin_abi::abi::PROBE_CERTAIN)
    }

    fn open(path: &Path) -> Result<Self, PluginError> {
        let source = RawSource::new(path).map_err(|err| PluginError::io(err.to_string()))?;
        let decoder = rawler::get_decoder(&source)
            .map_err(|err| PluginError::unsupported(err.to_string()))?;

        // `dummy` builds the full description — dimensions, CFA, levels — while
        // skipping the expensive pixel decompression. Opening a frame stays
        // cheap, which matters when a session holds hundreds of them.
        let described = decoder
            .raw_image(&source, &RawDecodeParams::default(), true)
            .map_err(|err| PluginError::parse(err.to_string()))?;
        let layout = layout_from(&described)?;

        let metadata = decoder
            .raw_metadata(&source, &RawDecodeParams::default())
            .map_err(|err| PluginError::parse(err.to_string()))?;
        let info = info_from(&metadata);

        Ok(Self { source, decoder: Mutex::new(decoder), layout, info })
    }

    fn layout(&self) -> Result<ImageLayout, PluginError> {
        Ok(self.layout)
    }

    fn info(&self) -> Result<FrameInfo, PluginError> {
        Ok(self.info.clone())
    }

    fn read_samples(&self, dst: &mut [u8]) -> Result<(), PluginError> {
        let samples = samples_u16_mut(dst).ok_or_else(|| {
            PluginError::invalid_argument("destination buffer is misaligned for 16-bit samples")
        })?;

        let decoder = self
            .decoder
            .lock()
            .map_err(|_| PluginError::internal("decoder lock was poisoned by an earlier panic"))?;
        let image = decoder
            .raw_image(&self.source, &RawDecodeParams::default(), false)
            .map_err(|err| PluginError::parse(err.to_string()))?;

        let RawImageData::Integer(data) = image.data else {
            return Err(PluginError::internal(
                "decoder returned floating-point data after describing an integer frame",
            ));
        };
        if data.len() != samples.len() {
            return Err(PluginError::internal(format!(
                "decoder produced {} samples but the layout promised {}",
                data.len(),
                samples.len()
            )));
        }
        samples.copy_from_slice(&data);
        Ok(())
    }
}

astro_plugin_abi::export_plugin!(CanonRaw);

// ---------------------------------------------------------------------------
// rawler -> ABI
// ---------------------------------------------------------------------------

fn layout_from(raw: &RawImage) -> Result<ImageLayout, PluginError> {
    if !matches!(raw.data, RawImageData::Integer(_)) {
        return Err(PluginError::unsupported(
            "this plugin only reads integer sensor data; no Canon format produces floating-point raw",
        ));
    }

    let mut layout = ImageLayout {
        width: raw.width as u32,
        height: raw.height as u32,
        components: raw.cpp as u32,
        bits_per_sample: raw.bps as u32,
        sample_format: SampleFormat::U16,
        orientation: orientation_number(raw.orientation),
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
        // dropped: calibration needs *a* black point.
        layout.black_level_width = 1;
        layout.black_level_height = 1;
        layout.black_level[0] = black.levels.first().map_or(0.0, Rational::as_f32);
    }
}

fn copy_active_area(raw: &RawImage, layout: &mut ImageLayout) {
    // `crop_area` is the manufacturer's recommended crop; `active_area` is
    // everything that saw light. Astro work wants the latter, because the
    // masked border is what calibrates the black level.
    let area = raw.active_area.or(raw.crop_area);
    let Some(area) = area else {
        layout.active_width = layout.width;
        layout.active_height = layout.height;
        return;
    };

    let fits = area.p.x + area.d.w <= raw.width && area.p.y + area.d.h <= raw.height;
    if !fits || area.d.w == 0 || area.d.h == 0 {
        layout.active_width = layout.width;
        layout.active_height = layout.height;
        return;
    }

    layout.active_x = area.p.x as u32;
    layout.active_y = area.p.y as u32;
    layout.active_width = area.d.w as u32;
    layout.active_height = area.d.h as u32;
}

fn orientation_number(orientation: Orientation) -> u32 {
    match orientation {
        Orientation::Normal => 1,
        Orientation::HorizontalFlip => 2,
        Orientation::Rotate180 => 3,
        Orientation::VerticalFlip => 4,
        Orientation::Transpose => 5,
        Orientation::Rotate90 => 6,
        Orientation::Transverse => 7,
        Orientation::Rotate270 => 8,
        Orientation::Unknown => 0,
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
        exposure_seconds: exif.exposure_time.map(rational),
        iso: iso_speed(exif.iso_speed_ratings, exif.iso_speed),
        aperture: exif.fnumber.map(rational),
        focal_length_mm: exif.focal_length.map(rational),
        capture_time_unix: exif
            .date_time_original
            .as_deref()
            .or(exif.create_date.as_deref())
            .and_then(datetime::parse_exif),
        // Canon records sensor temperature in its makernotes, but rawler does
        // not surface it in the generalised metadata. Claiming a value we
        // cannot read would be worse than admitting we have none.
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

fn rational(value: Rational) -> f64 {
    if value.d == 0 {
        return f64::NAN;
    }
    value.n as f64 / value.d as f64
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_zero_denominator_does_not_divide() {
        assert_eq!(rational(Rational { n: 1, d: 4 }), 0.25);
        assert!(rational(Rational { n: 1, d: 0 }).is_nan());
    }

    #[test]
    fn orientation_maps_to_exif_numbering() {
        assert_eq!(orientation_number(Orientation::Normal), 1);
        assert_eq!(orientation_number(Orientation::Rotate270), 8);
        assert_eq!(orientation_number(Orientation::Unknown), 0);
    }
}
