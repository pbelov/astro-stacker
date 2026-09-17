//! The whole of astro-stacker that is not an interface: reading frames,
//! grouping a night, calibrating, measuring and stacking it.
//!
//! One thing everything else rests on — turning a file on disk into a
//! described, decodable frame — is deliberately not tied to any format here.
//! The applications say which decoders they were built with; this crate asks
//! the registry and never names one.

pub mod calibrate;
pub mod machine;
pub mod error;
pub mod frame;
pub mod format;
pub mod session;
pub mod integrate;
pub mod pipeline;
pub mod register;
pub mod stars;

pub use error::{Error, Result};
pub use calibrate::{Master, apply, build};
pub use frame::Samples;
pub use format::{Format, Formats, Frame, OpenFrame};

pub use frame::{FormatDescription, FrameInfo, ImageLayout, cfa_pattern_name};

pub use session::{FrameKind, FrameRecord, Partition, ScanOptions, ScanReport, Session, Tolerances, scan};
