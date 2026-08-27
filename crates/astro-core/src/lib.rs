//! Host-side machinery for astro-stacker: discovering format plugins, and
//! reading frames through them.
//!
//! Stacking, calibration and frame analysis will grow here. For now this crate
//! owns the one thing everything else depends on — turning a file on disk into
//! a described, decodable frame, without the rest of the application knowing
//! which plugin did it.

pub mod calibrate;
pub mod error;
pub mod frame;
pub mod plugin;
pub mod session;

pub use error::{Error, Result};
pub use calibrate::{Master, apply, build};
pub use frame::Samples;
pub use plugin::{LoadReport, LoadedPlugin, OpenFrame, PluginHost, default_plugin_dirs};

/// Re-exported so callers do not have to depend on the ABI crate directly to
/// read a frame's description.
pub use astro_plugin_abi::abi::ImageLayout;
pub use astro_plugin_abi::safe::{FrameInfo, PluginDescription, cfa_pattern_name};

pub use session::{FrameKind, FrameRecord, Partition, ScanOptions, ScanReport, Session, Tolerances, scan};
