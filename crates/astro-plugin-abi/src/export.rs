//! The safe side of writing a plugin.
//!
//! A plugin implements [`FormatPlugin`] in ordinary safe Rust and invokes
//! [`export_plugin!`][crate::export_plugin]. Everything `unsafe` about the
//! boundary — pointer validation, unwind containment, string lifetimes — lives
//! here and is written once.

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::abi::{
    ABI_VERSION, AsStr, FrameHandle, FrameMetadata, HostVTable, ImageLayout, LogLevel,
    PROBE_UNSUPPORTED, PluginInfo, PluginVTable, Status, TIME_UNKNOWN,
};
use crate::safe::{FrameInfo, PluginDescription, PluginError};

// ---------------------------------------------------------------------------
// The trait a plugin implements
// ---------------------------------------------------------------------------

/// One image format, implemented in safe Rust.
///
/// An instance represents one open file. The host may hold many at once and
/// may move them between threads, hence `Send + Sync`.
pub trait FormatPlugin: Send + Sync + Sized + 'static {
    /// Static identity. Called once per process and cached.
    fn description() -> PluginDescription;

    /// Decides whether this plugin should handle a file, without opening it.
    ///
    /// `header` holds the leading bytes of the file, already read by the host,
    /// and may be shorter than requested or empty for a zero-length file.
    /// Return [`PROBE_UNSUPPORTED`], or a confidence in
    /// `0..=`[`PROBE_CERTAIN`][crate::abi::PROBE_CERTAIN].
    fn probe(path: &Path, header: &[u8]) -> i32;

    /// Opens a file and parses its headers. Pixel data is decoded later, in
    /// [`read_samples`][FormatPlugin::read_samples].
    fn open(path: &Path) -> Result<Self, PluginError>;

    /// Called once, immediately after `open`, and cached by the host.
    fn layout(&self) -> Result<ImageLayout, PluginError>;

    /// Called once, immediately after `open`, and cached by the host.
    fn info(&self) -> Result<FrameInfo, PluginError>;

    /// Decodes the frame. `dst` is exactly
    /// [`ImageLayout::required_bytes`] long and its alignment is the host
    /// allocator's, so use [`samples_u16_mut`][crate::safe::samples_u16_mut]
    /// rather than casting by hand.
    fn read_samples(&self, dst: &mut [u8]) -> Result<(), PluginError>;
}

// ---------------------------------------------------------------------------
// Talking back to the host
// ---------------------------------------------------------------------------

static HOST_VTABLE: AtomicPtr<HostVTable> = AtomicPtr::new(ptr::null_mut());

/// Records the host vtable. Called by the generated entry point; a plugin
/// should not call this itself.
pub fn set_host_vtable(host: *const HostVTable) {
    HOST_VTABLE.store(host.cast_mut(), Ordering::Release);
}

/// Writes a line into the host log. A no-op before the plugin is initialised.
pub fn host_log(level: LogLevel, target: &str, message: &str) {
    let host = HOST_VTABLE.load(Ordering::Acquire);
    if host.is_null() {
        return;
    }
    // SAFETY: the host guarantees its vtable outlives `shutdown`, and the two
    // strings are only borrowed for the duration of the call.
    unsafe { ((*host).log)(level, AsStr::new(target), AsStr::new(message)) }
}

// ---------------------------------------------------------------------------
// Storage backing the borrowed strings we hand out
// ---------------------------------------------------------------------------

/// Owns the strings that [`PluginInfo`] points into.
///
/// Self-referential, and sound because every pointer targets heap data owned by
/// a `String` or `Vec`: moving this struct moves the owners, not their buffers.
/// Nothing here may be mutated after construction.
pub struct InfoStorage {
    _id: String,
    _display_name: String,
    _version: String,
    _author: String,
    _extensions: Vec<String>,
    _extensions_abi: Vec<AsStr>,
    info: PluginInfo,
}

impl InfoStorage {
    pub fn new(desc: PluginDescription) -> Self {
        let PluginDescription { id, display_name, version, author, extensions } = desc;
        let extensions_abi: Vec<AsStr> = extensions.iter().map(|e| AsStr::new(e)).collect();
        let info = PluginInfo {
            struct_size: size_of::<PluginInfo>() as u32,
            abi_version: ABI_VERSION,
            id: AsStr::new(&id),
            display_name: AsStr::new(&display_name),
            version: AsStr::new(&version),
            author: AsStr::new(&author),
            extensions: extensions_abi.as_ptr(),
            extension_count: extensions_abi.len(),
        };
        Self {
            _id: id,
            _display_name: display_name,
            _version: version,
            _author: author,
            _extensions: extensions,
            _extensions_abi: extensions_abi,
            info,
        }
    }

    pub fn as_ptr(&self) -> *const PluginInfo {
        &self.info
    }
}

/// Owns the strings that [`FrameMetadata`] points into. Same reasoning as
/// [`InfoStorage`]: built once at `open`, never mutated, so the pointers the
/// host receives stay valid for the life of the handle.
struct MetadataStorage {
    _camera_make: String,
    _camera_model: String,
    _lens_model: String,
    meta: FrameMetadata,
}

impl MetadataStorage {
    fn new(info: &FrameInfo) -> Self {
        let camera_make = info.camera_make.clone();
        let camera_model = info.camera_model.clone();
        let lens_model = info.lens_model.clone();
        let meta = FrameMetadata {
            struct_size: size_of::<FrameMetadata>() as u32,
            camera_make: AsStr::new(&camera_make),
            camera_model: AsStr::new(&camera_model),
            lens_model: AsStr::new(&lens_model),
            exposure_seconds: or_nan(info.exposure_seconds),
            iso: or_nan(info.iso),
            aperture: or_nan(info.aperture),
            focal_length_mm: or_nan(info.focal_length_mm),
            capture_time_unix: info.capture_time_unix.unwrap_or(TIME_UNKNOWN),
            sensor_temperature_c: or_nan(info.sensor_temperature_c),
        };
        Self { _camera_make: camera_make, _camera_model: camera_model, _lens_model: lens_model, meta }
    }
}

fn or_nan(v: Option<f64>) -> f64 {
    v.unwrap_or(f64::NAN)
}

/// Asserts a value is safe to share across threads.
///
/// Used only for [`InfoStorage`], which is written once during initialisation
/// and read-only thereafter. The raw pointers inside it are what deny the
/// automatic `Sync`.
pub struct SyncWrapper<T>(pub T);

// SAFETY: see the type documentation. Callers must not mutate the contents.
unsafe impl<T> Sync for SyncWrapper<T> {}
// SAFETY: as above.
unsafe impl<T> Send for SyncWrapper<T> {}

// ---------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------

/// Tags a live handle so a stale or foreign pointer is rejected instead of
/// dereferenced. Spells `ASTROFR1` in ASCII.
const HANDLE_MAGIC: u64 = 0x4153_5452_4F46_5231;

struct HandleState<P: FormatPlugin> {
    magic: u64,
    plugin: P,
    /// Captured at `open`, so repeated queries never re-parse.
    layout: ImageLayout,
    metadata: MetadataStorage,
    /// Detail for the last failed call. The host serialises calls on one
    /// handle, so a `RefCell` is enough — and turns a host contract violation
    /// into a caught panic rather than a data race.
    last_error: RefCell<String>,
}

/// # Safety
/// `handle` must be null, or a pointer returned by `ffi_open::<P>` and not yet
/// closed.
unsafe fn state<'a, P: FormatPlugin>(handle: *mut FrameHandle) -> Option<&'a HandleState<P>> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: delegated to the caller. The magic check is a best-effort guard
    // against a pointer from a different plugin, not a substitute for it.
    let state = unsafe { &*handle.cast::<HandleState<P>>() };
    (state.magic == HANDLE_MAGIC).then_some(state)
}

fn record<P: FormatPlugin>(state: &HandleState<P>, err: PluginError) -> Status {
    if let Ok(mut slot) = state.last_error.try_borrow_mut() {
        slot.clear();
        slot.push_str(&err.message);
    }
    err.status
}

thread_local! {
    /// Errors from a failed `open`, which has no handle to report through.
    static OPEN_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

// ---------------------------------------------------------------------------
// Generic FFI shims
// ---------------------------------------------------------------------------

/// Runs plugin code with unwinding contained: a panic becomes
/// [`Status::INTERNAL`] instead of undefined behaviour at the ABI boundary.
fn guard<T>(f: impl FnOnce() -> Result<T, PluginError>) -> Result<T, PluginError> {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_owned());
            Err(PluginError::internal(format!("plugin panicked: {what}")))
        }
    }
}

/// # Safety
/// `host` must be null or point at a live [`HostVTable`]. `vtable` must live
/// for the rest of the process.
pub unsafe fn ffi_entry(host: *const HostVTable, vtable: &'static PluginVTable) -> *const PluginVTable {
    if host.is_null() {
        return ptr::null();
    }
    // Reading two fields from a pointer we have not yet validated is inherent
    // to any versioned ABI; there is no way to check the version without it.
    // SAFETY: delegated to the caller.
    let host_ref = unsafe { &*host };
    if host_ref.abi_version != ABI_VERSION || host_ref.struct_size as usize != size_of::<HostVTable>() {
        return ptr::null();
    }
    set_host_vtable(host);
    vtable
}

/// # Safety
/// `path` must describe live UTF-8; `header` must be null or point at
/// `header_len` readable bytes.
pub unsafe fn ffi_probe<P: FormatPlugin>(path: AsStr, header: *const u8, header_len: usize) -> i32 {
    let result = guard(|| {
        // SAFETY: delegated to the caller.
        let path = unsafe { path.as_str() };
        let header = if header.is_null() || header_len == 0 {
            &[][..]
        } else {
            // SAFETY: delegated to the caller.
            unsafe { std::slice::from_raw_parts(header, header_len) }
        };
        Ok(P::probe(Path::new(path), header))
    });
    result.unwrap_or(PROBE_UNSUPPORTED)
}

/// # Safety
/// `path` must describe live UTF-8; `out_handle` must be null or writable.
pub unsafe fn ffi_open<P: FormatPlugin>(path: AsStr, out_handle: *mut *mut FrameHandle) -> Status {
    if out_handle.is_null() {
        return Status::INVALID_ARGUMENT;
    }
    // SAFETY: delegated to the caller. Cleared first so a failed open never
    // leaves a stale handle behind.
    unsafe { *out_handle = ptr::null_mut() };

    let built = guard(|| {
        // SAFETY: delegated to the caller.
        let path = unsafe { path.as_str() };
        if path.is_empty() {
            return Err(PluginError::invalid_argument("empty path"));
        }
        let plugin = P::open(Path::new(path))?;
        let layout = plugin.layout()?;
        let info = plugin.info()?;
        Ok(Box::new(HandleState {
            magic: HANDLE_MAGIC,
            plugin,
            layout,
            metadata: MetadataStorage::new(&info),
            last_error: RefCell::new(String::new()),
        }))
    });

    match built {
        Ok(state) => {
            // SAFETY: delegated to the caller.
            unsafe { *out_handle = Box::into_raw(state).cast::<FrameHandle>() };
            Status::OK
        }
        Err(err) => {
            OPEN_ERROR.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    slot.clear();
                    slot.push_str(&err.message);
                }
            });
            err.status
        }
    }
}

/// # Safety
/// `handle` must be null, or a live handle from `ffi_open::<P>`.
pub unsafe fn ffi_close<P: FormatPlugin>(handle: *mut FrameHandle) {
    // SAFETY: delegated to the caller.
    if unsafe { state::<P>(handle) }.is_none() {
        return;
    }
    let state = handle.cast::<HandleState<P>>();
    // Invalidate first, so a double close is caught by the magic check rather
    // than freeing twice.
    // SAFETY: `state` was just validated.
    unsafe { (*state).magic = 0 };
    // A plugin's `Drop` is still plugin code, so contain it.
    let _ = guard(|| {
        // SAFETY: the pointer came from `Box::into_raw` in `ffi_open`.
        drop(unsafe { Box::from_raw(state) });
        Ok(())
    });
}

/// # Safety
/// `handle` must be null or live; `out` must be null or point at a writable
/// [`ImageLayout`] whose `struct_size` the caller has already set.
pub unsafe fn ffi_layout<P: FormatPlugin>(handle: *mut FrameHandle, out: *mut ImageLayout) -> Status {
    // SAFETY: delegated to the caller.
    let Some(state) = (unsafe { state::<P>(handle) }) else {
        return Status::INVALID_ARGUMENT;
    };
    if out.is_null() {
        return Status::INVALID_ARGUMENT;
    }
    // SAFETY: delegated to the caller.
    if unsafe { (*out).struct_size } as usize != size_of::<ImageLayout>() {
        return Status::ABI_MISMATCH;
    }
    // SAFETY: delegated to the caller.
    unsafe { *out = state.layout };
    Status::OK
}

/// # Safety
/// As [`ffi_layout`], for [`FrameMetadata`].
pub unsafe fn ffi_metadata<P: FormatPlugin>(handle: *mut FrameHandle, out: *mut FrameMetadata) -> Status {
    // SAFETY: delegated to the caller.
    let Some(state) = (unsafe { state::<P>(handle) }) else {
        return Status::INVALID_ARGUMENT;
    };
    if out.is_null() {
        return Status::INVALID_ARGUMENT;
    }
    // SAFETY: delegated to the caller.
    if unsafe { (*out).struct_size } as usize != size_of::<FrameMetadata>() {
        return Status::ABI_MISMATCH;
    }
    // SAFETY: delegated to the caller. The strings point into storage owned by
    // the handle and stay valid until it is closed.
    unsafe { *out = state.metadata.meta };
    Status::OK
}

/// # Safety
/// `handle` must be null or live; `dst` must be null or point at `dst_len`
/// writable bytes.
pub unsafe fn ffi_read_samples<P: FormatPlugin>(handle: *mut FrameHandle, dst: *mut u8, dst_len: usize) -> Status {
    // SAFETY: delegated to the caller.
    let Some(state) = (unsafe { state::<P>(handle) }) else {
        return Status::INVALID_ARGUMENT;
    };
    if dst.is_null() {
        return Status::INVALID_ARGUMENT;
    }
    let Some(required) = state.layout.required_bytes() else {
        return record(state, PluginError::internal("layout describes no valid buffer size"));
    };
    if dst_len < required {
        return record(
            state,
            PluginError::new(Status::BUFFER_TOO_SMALL, format!("need {required} bytes, host offered {dst_len}")),
        );
    }
    // Hand the plugin exactly the buffer its layout describes, never more.
    // SAFETY: delegated to the caller; `required <= dst_len`.
    let buffer = unsafe { std::slice::from_raw_parts_mut(dst, required) };
    match guard(|| state.plugin.read_samples(buffer)) {
        Ok(()) => Status::OK,
        Err(err) => record(state, err),
    }
}

/// # Safety
/// `handle` must be null, or a live handle from `ffi_open::<P>`.
pub unsafe fn ffi_last_error<P: FormatPlugin>(handle: *mut FrameHandle) -> AsStr {
    if handle.is_null() {
        // Points into a thread-local `String`, so it stays valid until this
        // thread next calls `open`.
        return OPEN_ERROR.with(|slot| match slot.try_borrow() {
            Ok(slot) => AsStr::new(slot.as_str()),
            Err(_) => AsStr::EMPTY,
        });
    }
    // SAFETY: delegated to the caller.
    match unsafe { state::<P>(handle) } {
        // Points into the handle's own `String`, valid until the next failed
        // call on this handle overwrites it.
        Some(state) => match state.last_error.try_borrow() {
            Ok(slot) => AsStr::new(slot.as_str()),
            Err(_) => AsStr::EMPTY,
        },
        None => AsStr::EMPTY,
    }
}

// ---------------------------------------------------------------------------
// The macro
// ---------------------------------------------------------------------------

/// Turns a [`FormatPlugin`] implementation into a loadable plugin.
///
/// Emits the `astro_plugin_entry` symbol and the whole `extern "C"` surface.
/// Invoke it exactly once per plugin crate, and build that crate as a
/// `cdylib`.
///
/// ```ignore
/// struct MyFormat { /* ... */ }
/// impl FormatPlugin for MyFormat { /* ... */ }
/// astro_plugin_abi::export_plugin!(MyFormat);
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        #[doc(hidden)]
        mod __astro_plugin_exports {
            use super::*;

            use $crate::abi::{
                AsStr, FrameHandle, FrameMetadata, HostVTable, ImageLayout, PluginInfo,
                PluginVTable, Status, ABI_VERSION,
            };
            use $crate::export::{
                ffi_close, ffi_entry, ffi_last_error, ffi_layout, ffi_metadata, ffi_open,
                ffi_probe, ffi_read_samples, FormatPlugin, InfoStorage, SyncWrapper,
            };

            static INFO: ::std::sync::OnceLock<SyncWrapper<InfoStorage>> = ::std::sync::OnceLock::new();

            unsafe extern "C" fn info() -> *const PluginInfo {
                INFO.get_or_init(|| SyncWrapper(InfoStorage::new(<$plugin as FormatPlugin>::description())))
                    .0
                    .as_ptr()
            }

            unsafe extern "C" fn probe(path: AsStr, header: *const u8, header_len: usize) -> i32 {
                // SAFETY: the host upholds the contract documented on `PluginVTable::probe`.
                unsafe { ffi_probe::<$plugin>(path, header, header_len) }
            }

            unsafe extern "C" fn open(path: AsStr, out_handle: *mut *mut FrameHandle) -> Status {
                // SAFETY: as above.
                unsafe { ffi_open::<$plugin>(path, out_handle) }
            }

            unsafe extern "C" fn close(handle: *mut FrameHandle) {
                // SAFETY: as above.
                unsafe { ffi_close::<$plugin>(handle) }
            }

            unsafe extern "C" fn layout(handle: *mut FrameHandle, out: *mut ImageLayout) -> Status {
                // SAFETY: as above.
                unsafe { ffi_layout::<$plugin>(handle, out) }
            }

            unsafe extern "C" fn metadata(handle: *mut FrameHandle, out: *mut FrameMetadata) -> Status {
                // SAFETY: as above.
                unsafe { ffi_metadata::<$plugin>(handle, out) }
            }

            unsafe extern "C" fn read_samples(handle: *mut FrameHandle, dst: *mut u8, dst_len: usize) -> Status {
                // SAFETY: as above.
                unsafe { ffi_read_samples::<$plugin>(handle, dst, dst_len) }
            }

            unsafe extern "C" fn last_error(handle: *mut FrameHandle) -> AsStr {
                // SAFETY: as above.
                unsafe { ffi_last_error::<$plugin>(handle) }
            }

            unsafe extern "C" fn shutdown() {}

            static VTABLE: PluginVTable = PluginVTable {
                struct_size: size_of::<PluginVTable>() as u32,
                abi_version: ABI_VERSION,
                info,
                probe,
                open,
                close,
                layout,
                metadata,
                read_samples,
                last_error,
                shutdown,
            };

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn astro_plugin_entry(host: *const HostVTable) -> *const PluginVTable {
                // SAFETY: the host passes a live vtable or null.
                unsafe { ffi_entry(host, &VTABLE) }
            }
        }
    };
}
