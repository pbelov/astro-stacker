//! What a decoded frame is: how to interpret its samples, and what the camera
//! recorded about it.
//!
//! These describe a frame rather than any one format's idea of one, which is
//! what lets the rest of the project group, calibrate and stack frames without
//! knowing which decoder produced them.

/// Decoded sample data for one frame, still in the format the decoder produced.
///
/// Kept in the sensor's native representation rather than converted to `f32` on
/// the spot: a 45-megapixel frame is 90 MB as `u16` and 180 MB as `f32`, and a
/// stack is hundreds of frames. Conversion happens per tile, when calibration
/// actually needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl Samples {
    pub fn len(&self) -> usize {
        match self {
            Self::U16(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Smallest and largest sample. `None` for an empty frame.
    ///
    /// Cheap enough to be worth having on the raw data: a light frame whose
    /// maximum never approaches the white level is under-exposed, and one whose
    /// minimum sits at the black level everywhere is a lens cap.
    pub fn range(&self) -> Option<(f64, f64)> {
        match self {
            Self::U16(v) => v
                .iter()
                .fold(None, |acc: Option<(u16, u16)>, &s| {
                    Some(acc.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))))
                })
                .map(|(lo, hi)| (lo as f64, hi as f64)),
            Self::F32(v) => v
                .iter()
                .copied()
                .filter(|s| s.is_finite())
                .fold(None, |acc: Option<(f32, f32)>, s| {
                    Some(acc.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))))
                })
                .map(|(lo, hi)| (lo as f64, hi as f64)),
        }
    }
}

/// Returned by [`Format::probe`][crate::format::Format::probe] when a decoder
/// cannot read a file.
pub const PROBE_UNSUPPORTED: i32 = -1;
/// Certainty that the file belongs to this decoder, e.g. a magic number matched.
pub const PROBE_CERTAIN: i32 = 100;
/// The extension matches, but the contents were not verified.
pub const PROBE_LIKELY: i32 = 50;

/// How many leading bytes of a file are read before probing. Enough for every
/// container magic we care about without touching the disk twice.
pub const PROBE_HEADER_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// Pixel description
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SampleFormat(pub u32);

impl SampleFormat {
    pub const U16: SampleFormat = SampleFormat(0);
    pub const F32: SampleFormat = SampleFormat(1);

    /// Returns 0 for an unrecognised format, which callers must treat as an error.
    pub const fn bytes_per_sample(self) -> usize {
        match self.0 {
            0 => 2,
            1 => 4,
            _ => 0,
        }
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            0 => "u16",
            1 => "f32",
            _ => "unknown",
        }
    }
}

/// Colour-filter indices used in [`ImageLayout::cfa_pattern`].
pub const COLOR_RED: u8 = 0;
pub const COLOR_GREEN: u8 = 1;
pub const COLOR_BLUE: u8 = 2;
/// A fourth filter colour: emerald on some Sony sensors, yellow on others.
/// Bayer sensors never use this.
pub const COLOR_FOURTH: u8 = 3;

/// Largest CFA repeat we describe. 2x2 covers Bayer, 6x6 covers X-Trans.
pub const CFA_MAX_DIM: usize = 6;
pub const CFA_MAX_CELLS: usize = CFA_MAX_DIM * CFA_MAX_DIM;

/// Everything needed to interpret the sample buffer, and to calibrate it.
///
/// Deliberately not `PartialEq`: several fields are NaN when the camera did not
/// record them, and NaN compares unequal to itself, so a derived comparison
/// would report two copies of one layout as different. Compare the fields that
/// matter — the session does this in `GeometryKey`.
#[derive(Debug, Clone, Copy)]
pub struct ImageLayout {
    /// Full decoded frame, including any masked border.
    pub width: u32,
    pub height: u32,
    /// 1 for a CFA/mosaiced frame, 3 for already-demosaiced RGB.
    pub components: u32,
    pub bits_per_sample: u32,
    pub sample_format: SampleFormat,

    /// CFA repeat size. Both zero means the frame is not mosaiced.
    pub cfa_width: u32,
    pub cfa_height: u32,
    /// Row-major `cfa_height * cfa_width` colour indices; trailing bytes unused.
    pub cfa_pattern: [u8; CFA_MAX_CELLS],

    /// Light-sensitive region. Pixels outside it are optically black, which is
    /// what makes per-frame black-level calibration possible.
    pub active_x: u32,
    pub active_y: u32,
    pub active_width: u32,
    pub active_height: u32,

    /// Black level as a repeating grid aligned to the frame origin.
    /// `1x1` means a single level for the whole frame.
    pub black_level_width: u32,
    pub black_level_height: u32,
    pub black_level: [f32; CFA_MAX_CELLS],

    /// Saturation point per colour index. Anything at or above it is clipped
    /// and must be excluded from stacking.
    pub white_level: [f32; 4],

    /// As-shot white balance multipliers per colour index. NaN if not recorded.
    pub wb_coeffs: [f32; 4],

    /// 4x3 row-major matrix converting CIE XYZ to camera colour space.
    pub xyz_to_cam: [f32; 12],

    /// EXIF orientation (1..=8), or 0 when unknown.
    pub orientation: u32,
}

impl Default for ImageLayout {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            components: 1,
            bits_per_sample: 16,
            sample_format: SampleFormat::U16,
            cfa_width: 0,
            cfa_height: 0,
            cfa_pattern: [0; CFA_MAX_CELLS],
            active_x: 0,
            active_y: 0,
            active_width: 0,
            active_height: 0,
            black_level_width: 1,
            black_level_height: 1,
            // NaN, like every other float in this struct, so that "the file did
            // not record a black level" is representable. A zero here would be
            // a fabricated pedestal, and a fabricated pedestal is subtracted
            // from real data without anything looking wrong.
            black_level: [f32::NAN; CFA_MAX_CELLS],
            white_level: [f32::NAN; 4],
            wb_coeffs: [f32::NAN; 4],
            xyz_to_cam: [f32::NAN; 12],
            orientation: 0,
        }
    }
}

impl ImageLayout {
    /// Bytes to allocate for [`Frame::read_samples`][crate::format::Frame::read_samples].
    /// Returns `None` on overflow or an unrecognised sample format.
    pub fn required_bytes(&self) -> Option<usize> {
        let bps = self.sample_format.bytes_per_sample();
        if bps == 0 {
            return None;
        }
        (self.width as usize)
            .checked_mul(self.height as usize)?
            .checked_mul(self.components as usize)?
            .checked_mul(bps)
    }
}

// ---------------------------------------------------------------------------
// Frame metadata
// ---------------------------------------------------------------------------

/// Shooting parameters for one frame. `None` means the format did not record it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameInfo {
    pub camera_make: String,
    pub camera_model: String,
    pub lens_model: String,
    pub exposure_seconds: Option<f64>,
    pub iso: Option<f64>,
    pub aperture: Option<f64>,
    pub focal_length_mm: Option<f64>,
    /// Seconds since the Unix epoch.
    pub capture_time_unix: Option<i64>,
    pub sensor_temperature_c: Option<f64>,
}

/// A decoder's static identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatDescription {
    /// Stable machine-readable id, e.g. `canon-raw`.
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub author: String,
    /// Lowercase, no leading dot.
    pub extensions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Helpers both sides of a decoder want
// ---------------------------------------------------------------------------

/// Reinterprets a byte buffer as 16-bit samples.
///
/// Returns `None` if the buffer is misaligned or has an odd length, which a
/// decoder should report as an error rather than work around.
pub fn samples_u16_mut(dst: &mut [u8]) -> Option<&mut [u16]> {
    // SAFETY: u16 has no invalid bit patterns, and `align_to_mut` guarantees
    // the middle slice is correctly aligned and in bounds.
    let (head, body, tail) = unsafe { dst.align_to_mut::<u16>() };
    (head.is_empty() && tail.is_empty()).then_some(body)
}

/// Names a colour-filter array pattern for display, e.g. `RGGB`.
///
/// Returns `None` for a non-mosaiced frame, and falls back to a `WxH` label for
/// patterns too large to spell out (X-Trans).
pub fn cfa_pattern_name(layout: &ImageLayout) -> Option<String> {
    let (w, h) = (layout.cfa_width as usize, layout.cfa_height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let cells = w.checked_mul(h)?;
    if cells > CFA_MAX_CELLS {
        return None;
    }
    if cells > 4 {
        return Some(format!("{w}x{h} mosaic"));
    }
    Some(
        layout.cfa_pattern[..cells]
            .iter()
            .map(|&c| match c {
                COLOR_RED => 'R',
                COLOR_GREEN => 'G',
                COLOR_BLUE => 'B',
                COLOR_FOURTH => 'E',
                _ => '?',
            })
            .collect(),
    )
}
