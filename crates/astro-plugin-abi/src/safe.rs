//! Owned, ergonomic mirrors of the [`crate::abi`] types.
//!
//! Both sides of the boundary use these: plugins build them, the host consumes
//! them. Keeping the conversion in one place means the `unsafe` that reads
//! plugin-owned strings is written once.

use crate::abi::{
    CFA_MAX_CELLS, COLOR_BLUE, COLOR_FOURTH, COLOR_GREEN, COLOR_RED, FrameMetadata, ImageLayout,
    Status, TIME_UNKNOWN,
};

/// A plugin's static identity, as the plugin declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDescription {
    /// Stable machine-readable id, e.g. `canon-raw`.
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub author: String,
    /// Lowercase, no leading dot.
    pub extensions: Vec<String>,
}

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

impl FrameInfo {
    /// Reads an ABI struct into owned data.
    ///
    /// # Safety
    /// Every [`AsStr`] in `meta` must point at live, valid UTF-8.
    pub unsafe fn from_abi(meta: &FrameMetadata) -> Self {
        // SAFETY: delegated to the caller.
        unsafe {
            Self {
                camera_make: meta.camera_make.as_str().to_owned(),
                camera_model: meta.camera_model.as_str().to_owned(),
                lens_model: meta.lens_model.as_str().to_owned(),
                exposure_seconds: finite(meta.exposure_seconds),
                iso: finite(meta.iso),
                aperture: finite(meta.aperture),
                focal_length_mm: finite(meta.focal_length_mm),
                capture_time_unix: (meta.capture_time_unix != TIME_UNKNOWN)
                    .then_some(meta.capture_time_unix),
                sensor_temperature_c: finite(meta.sensor_temperature_c),
            }
        }
    }
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// An error a plugin reports across the boundary: a [`Status`] the host can
/// branch on, plus text for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    pub status: Status,
    pub message: String,
}

impl PluginError {
    pub fn new(status: Status, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(Status::UNSUPPORTED, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(Status::IO, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(Status::PARSE, message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(Status::INVALID_ARGUMENT, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Status::INTERNAL, message)
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status.name(), self.message)
    }
}

impl std::error::Error for PluginError {}

/// Reinterprets a host-provided byte buffer as 16-bit samples.
///
/// Returns `None` if the buffer is misaligned or has an odd length, which a
/// plugin should report as [`Status::INVALID_ARGUMENT`] rather than work around.
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
