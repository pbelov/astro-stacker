//! Loading format plugins and reading frames through them.
//!
//! This is the only place in the host that touches the C ABI. Everything above
//! it works with [`OpenFrame`] and safe Rust types.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use astro_plugin_abi::abi::{
    ABI_VERSION, AsStr, EntryFn, ENTRY_SYMBOL, FrameHandle, FrameMetadata, HostVTable, ImageLayout,
    LogLevel, PROBE_CERTAIN, PROBE_HEADER_BYTES, PluginInfo, PluginVTable, SampleFormat, Status,
};
use astro_plugin_abi::safe::{FrameInfo, PluginDescription};
use libloading::{Library, Symbol};

use crate::error::{Error, Result};
use crate::frame::Samples;

/// Only files with this prefix are considered plugins, so the host never loads
/// an unrelated library that happens to sit in the same folder.
pub const PLUGIN_FILE_PREFIX: &str = "astro_format_";

/// A plugin declaring more extensions than this is treated as malformed.
const MAX_EXTENSIONS: usize = 256;

// ---------------------------------------------------------------------------
// What the host offers plugins
// ---------------------------------------------------------------------------

unsafe extern "C" fn host_log(level: LogLevel, source: AsStr, message: AsStr) {
    // A panic here would unwind into plugin code, which the ABI forbids.
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: the plugin lends us both strings for the duration of the call.
        let (source, message) = unsafe { (source.as_str(), message.as_str()) };
        let level = match level {
            LogLevel::ERROR => log::Level::Error,
            LogLevel::WARN => log::Level::Warn,
            LogLevel::INFO => log::Level::Info,
            LogLevel::DEBUG => log::Level::Debug,
            _ => log::Level::Trace,
        };
        log::log!(target: "plugin", level, "{source}: {message}");
    });
}

static HOST_VTABLE: HostVTable = HostVTable {
    struct_size: size_of::<HostVTable>() as u32,
    abi_version: ABI_VERSION,
    log: host_log,
};

// ---------------------------------------------------------------------------
// A loaded plugin
// ---------------------------------------------------------------------------

/// One loaded plugin library, kept alive for as long as any frame it opened.
pub struct LoadedPlugin {
    description: PluginDescription,
    path: PathBuf,
    vtable: *const PluginVTable,
    /// Declared last so it unloads only after everything that points into it.
    library: Library,
}

// SAFETY: `vtable` points at immutable data inside `library`, and the ABI
// requires plugin entry points to be thread-safe across distinct handles.
unsafe impl Send for LoadedPlugin {}
// SAFETY: as above.
unsafe impl Sync for LoadedPlugin {}

impl LoadedPlugin {
    /// Loads a library and completes the ABI handshake.
    pub fn load(path: &Path) -> Result<Self> {
        // SAFETY: loading a library runs its initialisers, so this is only ever
        // pointed at files the user installed as plugins.
        let library = unsafe { Library::new(path) }
            .map_err(|source| Error::LoadLibrary { path: path.to_owned(), source })?;

        let vtable = {
            // SAFETY: the symbol is only valid while `library` is loaded, and
            // the borrow ends before `library` is moved into `Self`.
            let entry: Symbol<EntryFn> = unsafe { library.get(ENTRY_SYMBOL) }
                .map_err(|source| Error::NotAPlugin { path: path.to_owned(), source })?;
            // SAFETY: `HOST_VTABLE` is a static, so it outlives the plugin.
            unsafe { entry(&raw const HOST_VTABLE) }
        };

        if vtable.is_null() {
            return Err(Error::IncompatiblePlugin { path: path.to_owned(), abi_version: ABI_VERSION });
        }
        // SAFETY: non-null, and the plugin promises it outlives `shutdown`.
        let table = unsafe { &*vtable };
        if table.abi_version != ABI_VERSION {
            return Err(Error::AbiVersionMismatch {
                path: path.to_owned(),
                expected: ABI_VERSION,
                found: table.abi_version,
            });
        }
        if table.struct_size as usize != size_of::<PluginVTable>() {
            return Err(Error::MalformedPlugin { path: path.to_owned() });
        }

        // SAFETY: the vtable passed its version and size checks.
        let description = unsafe { read_description(table) }
            .ok_or_else(|| Error::MalformedPlugin { path: path.to_owned() })?;

        Ok(Self { description, path: path.to_owned(), vtable, library })
    }

    pub fn description(&self) -> &PluginDescription {
        &self.description
    }

    pub fn id(&self) -> &str {
        &self.description.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn vtable(&self) -> &PluginVTable {
        // SAFETY: validated in `load`, and immutable for the life of `library`.
        unsafe { &*self.vtable }
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // SAFETY: every `OpenFrame` holds an `Arc` to its plugin, so all
        // handles are closed by the time this runs. `library` is dropped after
        // this, since it is the last declared field.
        unsafe { (self.vtable().shutdown)() };
        let _ = &self.library;
    }
}

/// # Safety
/// `table` must be a validated vtable whose plugin is still loaded.
unsafe fn read_description(table: &PluginVTable) -> Option<PluginDescription> {
    // SAFETY: delegated to the caller.
    let info = unsafe { (table.info)() };
    if info.is_null() {
        return None;
    }
    // SAFETY: non-null, and the ABI requires it to live as long as the plugin.
    let info: &PluginInfo = unsafe { &*info };
    if info.abi_version != ABI_VERSION || info.struct_size as usize != size_of::<PluginInfo>() {
        return None;
    }
    if info.extension_count > MAX_EXTENSIONS {
        return None;
    }

    let extensions = if info.extensions.is_null() || info.extension_count == 0 {
        Vec::new()
    } else {
        // SAFETY: the plugin promises `extension_count` valid entries.
        unsafe { std::slice::from_raw_parts(info.extensions, info.extension_count) }
            .iter()
            // SAFETY: each entry is a live UTF-8 slice owned by the plugin.
            .map(|ext| unsafe { ext.as_str() }.trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect()
    };

    // SAFETY: each field is a live UTF-8 slice owned by the plugin.
    let (id, display_name, version, author) = unsafe {
        (
            info.id.as_str().to_owned(),
            info.display_name.as_str().to_owned(),
            info.version.as_str().to_owned(),
            info.author.as_str().to_owned(),
        )
    };
    if id.is_empty() {
        return None;
    }

    Some(PluginDescription { id, display_name, version, author, extensions })
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// What a directory scan turned up. Failures are reported rather than logged
/// away: one broken plugin should not silently shrink the list of supported
/// formats.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    pub failures: Vec<(PathBuf, Error)>,
}

#[derive(Default)]
pub struct PluginHost {
    plugins: Vec<Arc<LoadedPlugin>>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plugins(&self) -> &[Arc<LoadedPlugin>] {
        &self.plugins
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Loads one library, rejecting a second plugin that claims an id already taken.
    pub fn load_file(&mut self, path: &Path) -> Result<Arc<LoadedPlugin>> {
        let plugin = Arc::new(LoadedPlugin::load(path)?);
        if let Some(existing) = self.plugins.iter().find(|p| p.id() == plugin.id()) {
            return Err(Error::DuplicatePluginId {
                id: plugin.id().to_owned(),
                first: existing.path().to_owned(),
                second: path.to_owned(),
            });
        }
        self.plugins.push(Arc::clone(&plugin));
        Ok(plugin)
    }

    /// Loads every plugin in a directory. A missing directory is not an error;
    /// it just contributes nothing.
    pub fn load_dir(&mut self, dir: &Path) -> LoadReport {
        let mut report = LoadReport::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return report;
        };
        // Sorted so that a duplicate id is reported against the same file every
        // run, rather than whichever the filesystem happened to yield first.
        let mut candidates: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| is_plugin_file(p)).collect();
        candidates.sort();

        for path in candidates {
            match self.load_file(&path) {
                Ok(plugin) => report.loaded.push(plugin.id().to_owned()),
                Err(err) => report.failures.push((path, err)),
            }
        }
        report
    }

    /// Opens a frame using whichever plugin bids highest for the file.
    pub fn open(&self, path: &Path) -> Result<OpenFrame> {
        let path_str = utf8(path)?;
        let header = read_header(path)?;

        let mut best: Option<(i32, &Arc<LoadedPlugin>)> = None;
        for plugin in &self.plugins {
            // SAFETY: `path_str` and `header` outlive the call, and the plugin
            // contains its own panics.
            let score = unsafe {
                (plugin.vtable().probe)(AsStr::new(path_str), header.as_ptr(), header.len())
            };
            if score < 0 {
                continue;
            }
            let score = score.min(PROBE_CERTAIN);
            if best.is_none_or(|(best_score, _)| score > best_score) {
                best = Some((score, plugin));
            }
        }

        let plugin = best
            .map(|(_, plugin)| Arc::clone(plugin))
            .ok_or_else(|| Error::UnsupportedFormat { path: path.to_owned() })?;

        OpenFrame::open(plugin, path)
    }
}

/// Where the application looks for plugins, in priority order.
pub fn default_plugin_dirs() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    // The second entry is what makes `cargo run` work: cargo drops plugin
    // cdylibs next to the binary rather than in a `plugins` subdirectory.
    vec![dir.join("plugins"), dir.to_owned()]
}

fn is_plugin_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    if !extension.eq_ignore_ascii_case(std::env::consts::DLL_EXTENSION) {
        return false;
    }
    let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
        return false;
    };
    // `DLL_PREFIX` is empty on Windows and "lib" elsewhere.
    stem.strip_prefix(std::env::consts::DLL_PREFIX)
        .unwrap_or(stem)
        .starts_with(PLUGIN_FILE_PREFIX)
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| Error::NonUtf8Path { path: path.to_owned() })
}

fn read_header(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|source| Error::Io { path: path.to_owned(), source })?;
    let mut buffer = vec![0u8; PROBE_HEADER_BYTES];
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => return Err(Error::Io { path: path.to_owned(), source }),
        }
    }
    buffer.truncate(filled);
    Ok(buffer)
}

// ---------------------------------------------------------------------------
// An open frame
// ---------------------------------------------------------------------------

/// A frame opened through a plugin. Headers are parsed; pixels are not decoded
/// until [`decode`][OpenFrame::decode] is called.
pub struct OpenFrame {
    plugin: Arc<LoadedPlugin>,
    handle: *mut FrameHandle,
    layout: ImageLayout,
    info: FrameInfo,
    path: PathBuf,
}

// SAFETY: the handle may move between threads; what the ABI forbids is two
// threads using it at once, which is why `OpenFrame` is deliberately not `Sync`.
unsafe impl Send for OpenFrame {}

impl OpenFrame {
    fn open(plugin: Arc<LoadedPlugin>, path: &Path) -> Result<Self> {
        let path_str = utf8(path)?;
        let table = plugin.vtable();

        let mut handle: *mut FrameHandle = std::ptr::null_mut();
        // SAFETY: `path_str` outlives the call; `handle` is a valid out-pointer.
        let status = unsafe { (table.open)(AsStr::new(path_str), &mut handle) };
        if !status.is_ok() || handle.is_null() {
            // SAFETY: a failed `open` reports through the null handle.
            let message = unsafe { (table.last_error)(std::ptr::null_mut()).as_str() }.to_owned();
            return Err(Error::PluginCall {
                plugin: plugin.id().to_owned(),
                action: "open",
                path: path.to_owned(),
                status,
                message,
            });
        }

        // From here on the handle must be closed on every exit path, so build
        // the guard before anything else can fail.
        let mut frame = Self {
            plugin,
            handle,
            layout: ImageLayout::default(),
            info: FrameInfo::default(),
            path: path.to_owned(),
        };

        let mut layout = ImageLayout::default();
        // SAFETY: live handle, valid out-pointer with `struct_size` set.
        let status = unsafe { (frame.plugin.vtable().layout)(frame.handle, &mut layout) };
        if !status.is_ok() {
            return Err(frame.call_failed("read the layout of", status));
        }
        frame.validate_layout(&layout)?;
        frame.layout = layout;

        let mut metadata = FrameMetadata::default();
        // SAFETY: as above.
        let status = unsafe { (frame.plugin.vtable().metadata)(frame.handle, &mut metadata) };
        if !status.is_ok() {
            return Err(frame.call_failed("read the metadata of", status));
        }
        // SAFETY: the strings point into storage owned by the handle, which is
        // still open, so they are live for this call.
        frame.info = unsafe { FrameInfo::from_abi(&metadata) };

        Ok(frame)
    }

    /// Rejects a layout the host cannot act on, so that later code can treat
    /// the dimensions as trustworthy.
    fn validate_layout(&self, layout: &ImageLayout) -> Result<()> {
        let reason = if layout.width == 0 || layout.height == 0 {
            Some("frame has zero width or height")
        } else if layout.components == 0 {
            Some("frame has no components")
        } else if layout.sample_format.bytes_per_sample() == 0 {
            Some("frame uses an unknown sample format")
        } else if layout.required_bytes().is_none() {
            Some("frame dimensions overflow a buffer size")
        } else {
            None
        };
        match reason {
            None => Ok(()),
            Some(reason) => Err(Error::PluginCall {
                plugin: self.plugin.id().to_owned(),
                action: "describe",
                path: self.path.clone(),
                status: Status::INTERNAL,
                message: reason.to_owned(),
            }),
        }
    }

    pub fn layout(&self) -> &ImageLayout {
        &self.layout
    }

    pub fn info(&self) -> &FrameInfo {
        &self.info
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn plugin(&self) -> &LoadedPlugin {
        &self.plugin
    }

    /// Decodes the pixel data. The buffer is allocated here, on the host heap,
    /// which is what keeps allocation and freeing on the same side of the ABI.
    pub fn decode(&self) -> Result<Samples> {
        // `validate_layout` already ruled out `None`.
        let bytes = self.layout.required_bytes().expect("layout was validated at open");

        match self.layout.sample_format {
            SampleFormat::F32 => {
                let mut buffer = vec![0f32; bytes / size_of::<f32>()];
                self.fill(buffer.as_mut_ptr().cast::<u8>(), bytes)?;
                Ok(Samples::F32(buffer))
            }
            // `validate_layout` accepted the format, so anything else is U16.
            _ => {
                let mut buffer = vec![0u16; bytes / size_of::<u16>()];
                self.fill(buffer.as_mut_ptr().cast::<u8>(), bytes)?;
                Ok(Samples::U16(buffer))
            }
        }
    }

    fn fill(&self, dst: *mut u8, len: usize) -> Result<()> {
        // SAFETY: `dst` points at `len` writable bytes owned by the caller's
        // `Vec`, and the plugin writes no more than the layout describes.
        let status = unsafe { (self.plugin.vtable().read_samples)(self.handle, dst, len) };
        if status.is_ok() {
            Ok(())
        } else {
            Err(self.call_failed("decode", status))
        }
    }

    fn call_failed(&self, action: &'static str, status: Status) -> Error {
        // SAFETY: the handle is still open, and the returned string is valid
        // until the next call on it.
        let message = unsafe { (self.plugin.vtable().last_error)(self.handle).as_str() }.to_owned();
        Error::PluginCall {
            plugin: self.plugin.id().to_owned(),
            action,
            path: self.path.clone(),
            status,
            message,
        }
    }
}

impl std::fmt::Debug for OpenFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenFrame")
            .field("path", &self.path)
            .field("plugin", &self.plugin.id())
            .field("width", &self.layout.width)
            .field("height", &self.layout.height)
            .finish_non_exhaustive()
    }
}

impl Drop for OpenFrame {
    fn drop(&mut self) {
        // SAFETY: the handle came from this plugin and has not been closed.
        unsafe { (self.plugin.vtable().close)(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_plugin_libraries_by_name() {
        let dll = format!("{}astro_format_canon.{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_EXTENSION);
        assert!(is_plugin_file(Path::new(&dll)));

        // Anything else in the same directory must be left alone.
        assert!(!is_plugin_file(Path::new("astro-stacker.exe")));
        assert!(!is_plugin_file(Path::new("vcruntime140.dll")));
        assert!(!is_plugin_file(Path::new("astro_format_canon.pdb")));
        assert!(!is_plugin_file(Path::new("astro_format_canon")));
    }

    #[test]
    fn opening_a_missing_file_reports_the_path() {
        let host = PluginHost::new();
        let err = host.open(Path::new("no-such-frame.cr3")).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "unexpected error: {err}");
    }
}
