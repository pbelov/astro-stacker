//! Reading a frame, whatever wrote it.
//!
//! A decoder is a Rust type implementing [`Format`], and the applications say
//! which ones they have. The core never names one: it asks the registry for a
//! frame and gets back something it can decode, which is what keeps every
//! format out of the pipeline's own code.
//!
//! Which decoder gets a file is decided by bidding rather than by extension.
//! Each is shown the path and the first few kilobytes and answers with a
//! confidence; the highest bid wins. That is what lets a decoder that reads a
//! container properly outrank one that only recognises the suffix, and what
//! makes a file the user named by hand readable whatever it is called.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::frame::{
    FormatDescription, FrameInfo, ImageLayout, PROBE_CERTAIN, PROBE_HEADER_BYTES, Samples,
};

/// A decoder: what it is, what it will take, and how to open it.
pub trait Format: Send + Sync + 'static {
    fn description(&self) -> &FormatDescription;

    /// Decides whether this decoder should handle a file.
    ///
    /// `header` holds the leading bytes of the file, already read, and may be
    /// shorter than asked for or empty for a zero-length file. It is offered so
    /// that a decoder able to answer from a few kilobytes need not touch the
    /// disk again. A decoder that cannot answer from it may open the file, and
    /// the raw decoder does: identifying a raw means parsing index structures
    /// that live well past the header. Return
    /// [`PROBE_UNSUPPORTED`][crate::frame::PROBE_UNSUPPORTED], or a confidence
    /// in `0..=`[`PROBE_CERTAIN`].
    ///
    /// This runs once per candidate file, so whatever it costs is paid several
    /// hundred times over in a night's session.
    fn probe(&self, path: &Path, header: &[u8]) -> i32;

    /// Opens a file and parses its headers. Pixels are decoded later.
    fn open(&self, path: &Path) -> Result<Box<dyn Frame>>;
}

/// One file, opened and understood but not yet decoded.
pub trait Frame: Send {
    fn layout(&self) -> &ImageLayout;
    fn info(&self) -> &FrameInfo;

    /// Decodes into `dst`, which is exactly
    /// [`ImageLayout::required_bytes`] long.
    fn read_samples(&self, dst: &mut [u8]) -> Result<()>;
}

/// The decoders this program has.
#[derive(Default, Clone)]
pub struct Formats {
    formats: Vec<Arc<dyn Format>>,
}

impl Formats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn formats(&self) -> &[Arc<dyn Format>] {
        &self.formats
    }

    pub fn is_empty(&self) -> bool {
        self.formats.is_empty()
    }

    /// Adds a decoder, refusing a second one that claims an id already taken.
    ///
    /// An id collision is a mistake in how the program was assembled rather
    /// than anything the user did, but it is refused rather than ignored: two
    /// decoders under one name means the reports name something that cannot be
    /// looked up.
    pub fn add(&mut self, format: impl Format) -> Result<()> {
        let format: Arc<dyn Format> = Arc::new(format);
        let id = &format.description().id;
        if self.formats.iter().any(|held| &held.description().id == id) {
            return Err(Error::DuplicateFormatId { id: id.clone() });
        }
        self.formats.push(format);
        Ok(())
    }

    /// Every extension any decoder claims, lowercase and without a dot.
    ///
    /// Used only to narrow a directory walk before anything is opened. `probe`
    /// remains the authority on what a file is, so a file the user names
    /// directly is still opened whatever it is called.
    pub fn supported_extensions(&self) -> Vec<String> {
        let mut extensions: Vec<String> = self
            .formats
            .iter()
            .flat_map(|format| format.description().extensions.iter().cloned())
            .collect();
        extensions.sort();
        extensions.dedup();
        extensions
    }

    /// Opens a frame using whichever decoder bids highest for the file.
    pub fn open(&self, path: &Path) -> Result<OpenFrame> {
        let header = read_header(path)?;

        let mut best: Option<(i32, &Arc<dyn Format>)> = None;
        for format in &self.formats {
            let score = format.probe(path, &header);
            if score < 0 {
                continue;
            }
            let score = score.min(PROBE_CERTAIN);
            if best.is_none_or(|(best_score, _)| score > best_score) {
                best = Some((score, format));
            }
        }

        let format = best
            .map(|(_, format)| Arc::clone(format))
            .ok_or_else(|| Error::UnsupportedFormat { path: path.to_owned() })?;

        let frame = format.open(path)?;
        validate(&format, frame.layout(), path)?;
        Ok(OpenFrame { format, frame, path: path.to_owned() })
    }
}

/// A frame opened through a decoder. Headers are parsed; pixels are not decoded
/// until [`decode`][OpenFrame::decode] is called.
pub struct OpenFrame {
    format: Arc<dyn Format>,
    frame: Box<dyn Frame>,
    path: PathBuf,
}

impl OpenFrame {
    pub fn layout(&self) -> &ImageLayout {
        self.frame.layout()
    }

    pub fn info(&self) -> &FrameInfo {
        self.frame.info()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn format(&self) -> &dyn Format {
        self.format.as_ref()
    }

    /// Decodes the pixel data.
    pub fn decode(&self) -> Result<Samples> {
        // `validate` already ruled out `None`.
        let bytes = self.layout().required_bytes().expect("layout was checked at open");

        match self.layout().sample_format {
            crate::frame::SampleFormat::F32 => {
                let mut buffer = vec![0f32; bytes / size_of::<f32>()];
                self.fill(bytemuck_f32(&mut buffer))?;
                Ok(Samples::F32(buffer))
            }
            // `validate` accepted the format, so anything else is U16.
            _ => {
                let mut buffer = vec![0u16; bytes / size_of::<u16>()];
                self.fill(bytemuck_u16(&mut buffer))?;
                Ok(Samples::U16(buffer))
            }
        }
    }

    fn fill(&self, dst: &mut [u8]) -> Result<()> {
        self.frame.read_samples(dst)
    }
}

/// Refuses a layout that would make the buffer meaningless, before anything is
/// allocated against it.
fn validate(format: &Arc<dyn Format>, layout: &ImageLayout, path: &Path) -> Result<()> {
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
        Some(reason) => Err(Error::Decode {
            format: format.description().id.clone(),
            action: "describe",
            path: path.to_owned(),
            message: reason.to_owned(),
        }),
    }
}

/// The leading bytes every decoder is shown before any of them opens the file.
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

/// Hands a `u16` buffer over as bytes.
///
/// A decoder is given bytes rather than typed samples because a frame may be
/// either width, and one entry point is simpler to be right about than two.
fn bytemuck_u16(buffer: &mut [u16]) -> &mut [u8] {
    // SAFETY: every bit pattern is a valid `u8`, and the length is scaled to
    // match. The lifetime is tied to the borrow of `buffer`.
    unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), size_of_val(buffer)) }
}

fn bytemuck_f32(buffer: &mut [f32]) -> &mut [u8] {
    // SAFETY: as above.
    unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), size_of_val(buffer)) }
}
