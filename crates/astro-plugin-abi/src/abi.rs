//! The raw C ABI. Everything here is `#[repr(C)]` and must stay
//! layout-compatible forever within a given [`ABI_VERSION`].
//!
//! Ground rules, so that a plugin built by a different compiler (or written in
//! another language entirely) keeps working:
//!
//! * No Rust types cross the boundary. No `String`, `Vec`, `Option`,
//!   `Result`, or data-carrying enums.
//! * Strings are UTF-8 byte slices ([`AsStr`]), never NUL-terminated and never
//!   platform-native wide chars. This is what keeps paths portable.
//! * Whoever allocates, frees. The host never frees plugin memory, and the
//!   plugin never frees host memory. On Windows each DLL can carry its own CRT
//!   heap, so crossing this line corrupts memory rather than failing loudly.
//! * Large pixel buffers are allocated by the host and filled by the plugin,
//!   which keeps the one big allocation on the host side.
//! * No panic may unwind across the boundary. The export macro in
//!   [`crate::export`] catches unwinds for you.

use core::ptr;

/// Bumped whenever any layout or function signature in this module changes.
/// Host and plugin must agree exactly; there is no forward compatibility yet.
pub const ABI_VERSION: u32 = 1;

/// The one symbol every plugin must export, as a NUL-terminated name.
pub const ENTRY_SYMBOL: &[u8] = b"astro_plugin_entry\0";

/// Signature of [`ENTRY_SYMBOL`].
///
/// The host passes its own vtable; the plugin returns a vtable that stays valid
/// until [`PluginVTable::shutdown`] is called. Returning null means the plugin
/// refused to load, almost always an ABI version mismatch.
pub type EntryFn = unsafe extern "C" fn(host: *const HostVTable) -> *const PluginVTable;

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// A borrowed UTF-8 string slice. Not NUL-terminated.
///
/// The lifetime is documented per field rather than encoded in the type:
/// strings handed out by a plugin stay valid until the next call on the same
/// handle.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsStr {
    pub ptr: *const u8,
    pub len: usize,
}

impl AsStr {
    pub const EMPTY: AsStr = AsStr { ptr: ptr::null(), len: 0 };

    pub const fn new(s: &str) -> Self {
        Self { ptr: s.as_ptr(), len: s.len() }
    }

    /// # Safety
    /// `ptr` must point at `len` live bytes of valid UTF-8 for `'a`.
    pub unsafe fn as_str<'a>(self) -> &'a str {
        if self.ptr.is_null() || self.len == 0 {
            return "";
        }
        // SAFETY: the caller guarantees the buffer is live and valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(self.ptr, self.len)) }
    }
}

impl Default for AsStr {
    fn default() -> Self {
        Self::EMPTY
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Result of a fallible ABI call.
///
/// A transparent newtype rather than a `#[repr(i32)]` enum: the host reads
/// these from foreign code, and an out-of-range discriminant in a real enum
/// would be undefined behaviour.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Status(pub i32);

impl Status {
    pub const OK: Status = Status(0);
    /// The plugin does not handle this file at all.
    pub const UNSUPPORTED: Status = Status(1);
    pub const IO: Status = Status(2);
    /// The file is recognised but malformed.
    pub const PARSE: Status = Status(3);
    pub const INVALID_ARGUMENT: Status = Status(4);
    /// The destination buffer is smaller than the layout requires.
    pub const BUFFER_TOO_SMALL: Status = Status(5);
    /// A bug in the plugin: a panic, or an invariant it failed to uphold.
    pub const INTERNAL: Status = Status(6);
    pub const ABI_MISMATCH: Status = Status(7);

    pub const fn is_ok(self) -> bool {
        self.0 == 0
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            0 => "ok",
            1 => "unsupported",
            2 => "io error",
            3 => "parse error",
            4 => "invalid argument",
            5 => "buffer too small",
            6 => "internal plugin error",
            7 => "ABI mismatch",
            _ => "unknown status",
        }
    }
}

// ---------------------------------------------------------------------------
// Probe confidence
// ---------------------------------------------------------------------------

/// Returned by [`PluginVTable::probe`] when the plugin cannot read a file.
pub const PROBE_UNSUPPORTED: i32 = -1;
/// Certainty that the file belongs to this plugin, e.g. a magic number matched.
pub const PROBE_CERTAIN: i32 = 100;
/// The extension matches, but the contents were not verified.
pub const PROBE_LIKELY: i32 = 50;

/// How many leading bytes of a file the host reads before calling `probe`.
/// Enough for every container magic we care about without touching the disk twice.
pub const PROBE_HEADER_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// Pixel description
// ---------------------------------------------------------------------------

#[repr(transparent)]
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
/// The host zeroes this and sets `struct_size` before calling; the plugin
/// rejects a `struct_size` it does not recognise.
///
/// Deliberately not `PartialEq`: several fields are NaN when the camera did not
/// record them, and NaN compares unequal to itself, so a derived comparison
/// would report two copies of one layout as different. Compare the fields that
/// matter — the host does this in `GeometryKey`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ImageLayout {
    pub struct_size: u32,

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
            struct_size: core::mem::size_of::<ImageLayout>() as u32,
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
    /// Bytes the host must allocate for [`PluginVTable::read_samples`].
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

/// Marks an unknown timestamp in [`FrameMetadata::capture_time_unix`].
/// Unknown floating-point fields use NaN instead.
pub const TIME_UNKNOWN: i64 = i64::MIN;

/// Shooting parameters, used to group frames and to reject mismatched ones.
///
/// Every [`AsStr`] borrows from the plugin and stays valid until the next call
/// on the same handle, or until the handle is closed.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrameMetadata {
    pub struct_size: u32,
    pub camera_make: AsStr,
    pub camera_model: AsStr,
    pub lens_model: AsStr,
    /// Shutter time in seconds. NaN if unknown.
    pub exposure_seconds: f64,
    pub iso: f64,
    pub aperture: f64,
    pub focal_length_mm: f64,
    /// Seconds since the Unix epoch, or [`TIME_UNKNOWN`].
    pub capture_time_unix: i64,
    /// Sensor temperature in Celsius, if the camera records it. NaN otherwise.
    /// Matters for dark-frame matching.
    pub sensor_temperature_c: f64,
}

impl Default for FrameMetadata {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<FrameMetadata>() as u32,
            camera_make: AsStr::EMPTY,
            camera_model: AsStr::EMPTY,
            lens_model: AsStr::EMPTY,
            exposure_seconds: f64::NAN,
            iso: f64::NAN,
            aperture: f64::NAN,
            focal_length_mm: f64::NAN,
            capture_time_unix: TIME_UNKNOWN,
            sensor_temperature_c: f64::NAN,
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin identity
// ---------------------------------------------------------------------------

/// Static description of a plugin. Every pointer here must stay valid for as
/// long as the plugin is loaded.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginInfo {
    pub struct_size: u32,
    pub abi_version: u32,
    /// Stable machine-readable id, e.g. `canon-raw`. Used to pin a plugin in a
    /// saved project, so it must not change between releases.
    pub id: AsStr,
    pub display_name: AsStr,
    pub version: AsStr,
    pub author: AsStr,
    /// Lowercase extensions without a dot, e.g. `cr2`. Used only to narrow the
    /// file dialog and to order probes; `probe` remains the authority.
    pub extensions: *const AsStr,
    pub extension_count: usize,
}

// ---------------------------------------------------------------------------
// Vtables
// ---------------------------------------------------------------------------

/// An opaque, plugin-owned open file. Only ever handled through a pointer.
#[repr(C)]
pub struct FrameHandle {
    _private: [u8; 0],
}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LogLevel(pub u32);

impl LogLevel {
    pub const ERROR: LogLevel = LogLevel(1);
    pub const WARN: LogLevel = LogLevel(2);
    pub const INFO: LogLevel = LogLevel(3);
    pub const DEBUG: LogLevel = LogLevel(4);
    pub const TRACE: LogLevel = LogLevel(5);
}

/// Services the host offers to plugins. Passed to [`EntryFn`] and valid until
/// [`PluginVTable::shutdown`] returns.
#[repr(C)]
pub struct HostVTable {
    pub struct_size: u32,
    pub abi_version: u32,
    /// Routes a log line from the plugin into the host log. Both strings are
    /// borrowed for the duration of the call only.
    pub log: unsafe extern "C" fn(level: LogLevel, target: AsStr, message: AsStr),
}

/// What a plugin offers the host.
///
/// Calls on distinct handles may run concurrently, so implementations must be
/// thread-safe. Calls on a single handle are serialised by the host.
#[repr(C)]
pub struct PluginVTable {
    pub struct_size: u32,
    pub abi_version: u32,

    /// Never null, never changes for the lifetime of the plugin.
    pub info: unsafe extern "C" fn() -> *const PluginInfo,

    /// Cheap, no-I/O test of whether this plugin should handle a file.
    ///
    /// `header` holds up to [`PROBE_HEADER_BYTES`] leading bytes, already read
    /// by the host. Returns [`PROBE_UNSUPPORTED`], or a confidence in
    /// `0..=`[`PROBE_CERTAIN`]; the host picks the highest bidder.
    pub probe: unsafe extern "C" fn(path: AsStr, header: *const u8, header_len: usize) -> i32,

    /// Opens a file and parses enough of it to answer `layout` and `metadata`.
    /// On success writes a non-null handle to `out_handle`.
    pub open: unsafe extern "C" fn(path: AsStr, out_handle: *mut *mut FrameHandle) -> Status,

    /// Releases a handle. Null is a no-op. The handle must not be used after.
    pub close: unsafe extern "C" fn(handle: *mut FrameHandle),

    pub layout: unsafe extern "C" fn(handle: *mut FrameHandle, out: *mut ImageLayout) -> Status,

    pub metadata: unsafe extern "C" fn(handle: *mut FrameHandle, out: *mut FrameMetadata) -> Status,

    /// Decodes the frame into a host-allocated buffer of exactly
    /// [`ImageLayout::required_bytes`]. Samples are row-major, top-left origin,
    /// components interleaved, native endianness.
    ///
    /// Samples are in **sensor readout order**. A plugin must never permute the
    /// buffer to honour [`ImageLayout::orientation`] — that field is metadata
    /// about how the image should be shown, not about how it is stored. The
    /// distinction is load-bearing: the host indexes frames against each other
    /// photosite by photosite, so two frames of one sensor must always agree on
    /// what `samples[i]` is, whichever way the camera was pointing.
    pub read_samples: unsafe extern "C" fn(handle: *mut FrameHandle, dst: *mut u8, dst_len: usize) -> Status,

    /// Human-readable detail for the last failed call on `handle`.
    ///
    /// Pass null to read the error from a failed `open`, which has no handle to
    /// report through. The returned string is valid until the next call on the
    /// same handle, or for null until the next `open` on the same thread.
    pub last_error: unsafe extern "C" fn(handle: *mut FrameHandle) -> AsStr,

    /// Called once before the library is unloaded. All handles are already closed.
    pub shutdown: unsafe extern "C" fn(),
}
