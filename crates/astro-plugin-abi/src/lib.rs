//! The contract between the astro-stacker host and its format plugins.
//!
//! Every image format the application understands — including the ones shipped
//! with it — is a separate dynamic library loaded at runtime. This crate is the
//! only thing the two sides share.
//!
//! * [`abi`] is the wire format: `#[repr(C)]` layouts and function signatures.
//!   Frozen within an [`abi::ABI_VERSION`].
//! * [`safe`] holds owned mirrors of those types, used by both sides.
//! * [`export`] lets a plugin be written in ordinary safe Rust.
//!
//! A minimal plugin:
//!
//! ```ignore
//! use astro_plugin_abi::export::FormatPlugin;
//!
//! struct MyFormat { /* ... */ }
//! impl FormatPlugin for MyFormat { /* ... */ }
//! astro_plugin_abi::export_plugin!(MyFormat);
//! ```

pub mod abi;
pub mod export;
pub mod safe;

pub use abi::ABI_VERSION;

#[cfg(test)]
mod tests {
    use super::abi::*;

    /// The ABI is only stable if these layouts are. A change here is a change
    /// that every existing plugin has to be rebuilt for, so it must be
    /// deliberate: bump `ABI_VERSION` alongside it.
    #[test]
    fn abi_struct_layouts_are_frozen() {
        assert_eq!(ABI_VERSION, 1);
        assert_eq!(size_of::<AsStr>(), 16);
        assert_eq!(size_of::<Status>(), 4);
        assert_eq!(size_of::<SampleFormat>(), 4);
        assert_eq!(size_of::<LogLevel>(), 4);
        assert_eq!(size_of::<ImageLayout>(), 320);
        assert_eq!(size_of::<FrameMetadata>(), 104);
        assert_eq!(size_of::<PluginInfo>(), 88);
        assert_eq!(size_of::<HostVTable>(), 16);
        assert_eq!(size_of::<PluginVTable>(), 80);
    }

    #[test]
    fn required_bytes_matches_a_full_frame() {
        let layout = ImageLayout {
            width: 8280,
            height: 5520,
            components: 1,
            sample_format: SampleFormat::U16,
            ..Default::default()
        };
        assert_eq!(layout.required_bytes(), Some(8280 * 5520 * 2));
    }

    #[test]
    fn required_bytes_refuses_an_unknown_sample_format() {
        let layout = ImageLayout { width: 4, height: 4, sample_format: SampleFormat(99), ..Default::default() };
        assert_eq!(layout.required_bytes(), None);
    }

    #[test]
    fn required_bytes_refuses_to_overflow() {
        let layout = ImageLayout {
            width: u32::MAX,
            height: u32::MAX,
            components: 3,
            sample_format: SampleFormat::F32,
            ..Default::default()
        };
        assert_eq!(layout.required_bytes(), None);
    }

    #[test]
    fn empty_strings_survive_the_round_trip() {
        // A null pointer and a zero length must both read back as "" rather
        // than dereferencing.
        // SAFETY: `EMPTY` is null with zero length, which `as_str` special-cases.
        assert_eq!(unsafe { AsStr::EMPTY.as_str() }, "");
        let owned = String::new();
        // SAFETY: points at a live (if empty) `String`.
        assert_eq!(unsafe { AsStr::new(&owned).as_str() }, "");
    }

    #[test]
    fn cfa_pattern_names_a_bayer_grid() {
        use crate::safe::cfa_pattern_name;

        let mut layout = ImageLayout { cfa_width: 2, cfa_height: 2, ..Default::default() };
        layout.cfa_pattern[..4].copy_from_slice(&[COLOR_RED, COLOR_GREEN, COLOR_GREEN, COLOR_BLUE]);
        assert_eq!(cfa_pattern_name(&layout).as_deref(), Some("RGGB"));

        let plain = ImageLayout::default();
        assert_eq!(cfa_pattern_name(&plain), None);
    }
}
