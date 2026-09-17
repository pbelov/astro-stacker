//! The command line's own options for aligning a run.
//!
//! The alignment itself lives in `astro_core::pipeline::align`, because the
//! window needs the same answer and two copies of it would eventually disagree
//! about which frame the stack was built on.

use anyhow::{Context, Result};
use astro_core::pipeline::Measured;
use astro_core::pipeline::align::{AlignOptions, Alignment};
use clap::Args;

pub use astro_core::pipeline::align::{Aligned, Seed};

#[derive(Args, Debug, Clone)]
pub struct AlignArgs {
    /// Align against this frame rather than one chosen from the middle of the
    /// run. Give the file name, as printed, extension included.
    #[arg(long, value_name = "NAME")]
    pub reference: Option<String>,

    /// How many of the brightest stars to match with. Generous on purpose: the
    /// top of a brightness ranking is the least stable part of it, because
    /// which stars saturate moves with the trailing.
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub stars: usize,

    /// How far along the run to look for a frame to chain through. Larger steps
    /// over more ruined frames and costs more matching.
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub span: usize,
}

pub fn align<'a>(read: &'a [Measured], args: &AlignArgs) -> Result<Alignment<'a>> {
    let options = AlignOptions { brightest: args.stars, span: args.span };
    astro_core::pipeline::align::align(read, args.reference.as_deref(), &options).with_context(
        || match &args.reference {
            Some(name) => format!("no light called {name} was read"),
            None => "the run holds too few lights to align".to_owned(),
        },
    )
}
