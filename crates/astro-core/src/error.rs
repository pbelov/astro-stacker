use std::path::PathBuf;

use astro_plugin_abi::abi::Status;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not load plugin {path}")]
    LoadLibrary {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },

    #[error("{path} is not an astro-stacker plugin: it exports no entry point")]
    NotAPlugin {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },

    /// The plugin loaded but refused the handshake, which in practice always
    /// means it was built against a different ABI version.
    #[error("plugin {path} rejected ABI v{abi_version}; rebuild it against this version of astro-stacker")]
    IncompatiblePlugin { path: PathBuf, abi_version: u32 },

    #[error("plugin {path} reports ABI v{found}, but this build speaks v{expected}")]
    AbiVersionMismatch { path: PathBuf, expected: u32, found: u32 },

    #[error("plugin {path} returned a malformed description")]
    MalformedPlugin { path: PathBuf },

    #[error("two plugins claim the id {id}: {first} and {second}")]
    DuplicatePluginId { id: String, first: PathBuf, second: PathBuf },

    #[error("no loaded plugin can read {path}")]
    UnsupportedFormat { path: PathBuf },

    /// A plugin call failed. `message` is the plugin's own explanation.
    #[error("plugin {plugin} failed to {action} {path}: {message} ({status})", status = status.name())]
    PluginCall {
        plugin: String,
        action: &'static str,
        path: PathBuf,
        status: Status,
        message: String,
    },

    /// The ABI carries paths as UTF-8 so they mean the same thing on every
    /// platform. Windows permits paths that are not valid UTF-8; they are
    /// vanishingly rare, and refusing them beats mangling them.
    #[error("path {path} is not valid UTF-8, which plugins require")]
    NonUtf8Path { path: PathBuf },

    /// The caller asked the pass to stop.
    #[error("cancelled")]
    Cancelled,

    /// A frame that decoded but cannot be measured, with the reason.
    ///
    /// Separate from a decode failure because it is not the plugin's fault and
    /// not the file's: a frame of one repeated value has no noise to threshold
    /// against, and the honest answer is to name it and carry on with the rest
    /// of the night.
    #[error("{name}: {reason}")]
    Unmeasurable { name: String, reason: String },

    /// A set with no active frames, or a frame whose layout describes nothing.
    #[error("there is nothing to combine")]
    NothingToCombine,

    /// A flat whose zero point could not be measured. Deliberately fatal rather
    /// than falling back to a plausible 2048: a normalisation taken on the
    /// wrong zero is a multiplicative error on every light it touches.
    #[error("the flat has no measurable pedestal: no masked border, and no bias at its gain")]
    NoPedestal,

    #[error("could not read {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
