use std::path::PathBuf;


pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Two decoders were registered under one name. A mistake in how the
    /// program was assembled, not in anything the user did — but refused
    /// rather than ignored, since reports would then name something that
    /// cannot be looked up.
    #[error("two decoders claim the id {id}")]
    DuplicateFormatId { id: String },

    #[error("no decoder can read {path}")]
    UnsupportedFormat { path: PathBuf },

    /// A decoder failed on a file. `message` is its own explanation.
    #[error("{format} failed to {action} {path}: {message}")]
    Decode { format: String, action: &'static str, path: PathBuf, message: String },

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

    /// A frame read twice in one combination did not read the same both times.
    ///
    /// The streaming path decodes every frame once to find the threshold and
    /// again to apply it, so a file that changes in between would have one
    /// version's threshold used on another version's pixels, and the master
    /// would come out wrong with nothing saying so. A network share, a card,
    /// or a copy still in flight all allow it.
    #[error("{path} changed while it was being combined: read twice, decoded differently")]
    FrameChanged { path: PathBuf },

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
