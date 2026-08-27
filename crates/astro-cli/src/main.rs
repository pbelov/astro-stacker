//! Command line front end.
//!
//! At this stage it exists to exercise the plugin boundary against real files:
//! which plugin claimed a frame, what it says the frame is, and whether the
//! pixels actually decode.

mod format;
mod report;
mod master_command;
mod scan_command;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use astro_core::session::{ColourStats, FrameStats, illumination_map, measure};
use astro_core::{OpenFrame, PluginHost, Samples, cfa_pattern_name, default_plugin_dirs};
use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand};
use master_command::MasterArgs;
use scan_command::ScanArgs;

#[derive(Parser)]
#[command(name = "astro-stacker", version, about = "Stacking for deep-sky astrophotography")]
pub(crate) struct Cli {
    /// Load plugins from this directory as well. May be repeated.
    #[arg(long = "plugin-dir", global = true, value_name = "DIR")]
    plugin_dirs: Vec<PathBuf>,

    /// Print more detail. Repeat for more still.
    #[arg(long, short, global = true, action = ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the format plugins that loaded, and what they read.
    Plugins,

    /// Read a session: group frames into stackable sets, match calibration to
    /// them, and say what does not fit.
    Scan(ScanArgs),

    /// Combine a session's calibration frames into masters and write them.
    Master(MasterArgs),

    /// Decode frames and report what the pixels say: where each colour sits
    /// between black and white, how much is clipped, and how evenly the frame
    /// is lit.
    Measure {
        /// Raw files to measure.
        #[arg(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Also print an N by N illumination map, normalised to its own mean so
        /// that two frames can be compared cell by cell.
        #[arg(long, value_name = "N")]
        map: Option<usize>,
    },

    /// Describe frames: sensor layout, calibration levels, shooting parameters.
    Info {
        /// Raw files to describe.
        #[arg(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Also decode the pixels, to confirm the frame really reads and to
        /// time how long that takes.
        #[arg(long)]
        decode: bool,
    },
}

fn main() -> Result<()> {
    // Parsed through `ArgMatches` rather than `Cli::parse` because `scan` needs
    // the order the options were typed in: `--group` applies to the paths that
    // follow it, and the parsed struct does not preserve that.
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches)?;

    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level)).init();

    let host = load_plugins(&cli.plugin_dirs)?;

    match &cli.command {
        Command::Plugins => list_plugins(&host),
        Command::Scan(args) => {
            let scan = matches.subcommand_matches("scan").expect("the scan subcommand was matched");
            scan_command::run(&host, args, scan)
        }
        Command::Master(args) => {
            let sub = matches
                .subcommand_matches("master")
                .expect("the master subcommand was matched");
            master_command::run(&host, args, sub)
        }
        Command::Measure { files, map } => measure_frames(&host, files, *map),
        Command::Info { files, decode } => describe_frames(&host, files, *decode),
    }
}

fn load_plugins(extra_dirs: &[PathBuf]) -> Result<PluginHost> {
    let mut host = PluginHost::new();
    let mut searched = Vec::new();

    for dir in default_plugin_dirs().iter().chain(extra_dirs) {
        if searched.contains(dir) {
            continue;
        }
        searched.push(dir.clone());

        let report = host.load_dir(dir);
        for id in &report.loaded {
            log::info!("loaded plugin {id} from {}", dir.display());
        }
        // A library that looks like a plugin but does not load is a real
        // problem for the user: it silently removes a supported format.
        for (path, error) in &report.failures {
            eprintln!("warning: {} did not load: {error:#}", path.display());
        }
    }

    if host.is_empty() {
        let list =
            searched.iter().map(|d| format!("  {}", d.display())).collect::<Vec<_>>().join("\n");
        bail!("no format plugins found. Searched:\n{list}");
    }
    Ok(host)
}

fn list_plugins(host: &PluginHost) -> Result<()> {
    for plugin in host.plugins() {
        let description = plugin.description();
        println!("{} {}", description.id, description.version);
        println!("  name       {}", description.display_name);
        println!("  author     {}", description.author);
        println!("  reads      {}", description.extensions.join(", "));
        println!("  library    {}", plugin.path().display());
    }
    Ok(())
}

/// Decodes each frame and prints the numbers, without judging them.
///
/// Thresholds — what counts as too dim, too clipped, too uneven — need arguing
/// against real frames before they are worth encoding. The measurements do not:
/// whole-frame minimum and maximum turned out to be useless on real data,
/// because hot and cold photosites pin both ends of every frame alike.
fn measure_frames(host: &PluginHost, files: &[PathBuf], map: Option<usize>) -> Result<()> {
    let mut failures = 0;

    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{}", file.display());
        match measure_frame(host, file, map) {
            Ok(()) => {}
            Err(error) => {
                failures += 1;
                println!("  error      {error:#}");
            }
        }
    }

    if failures > 0 {
        bail!("{failures} of {} frames could not be measured", files.len());
    }
    Ok(())
}

fn measure_frame(host: &PluginHost, file: &Path, map: Option<usize>) -> Result<()> {
    let frame = host.open(file).with_context(|| format!("opening {}", file.display()))?;
    let samples = frame.decode().with_context(|| format!("decoding {}", file.display()))?;
    let Some(stats) = measure(&samples, frame.layout()) else {
        bail!("this frame is not a single-component mosaic, so it cannot be measured per colour");
    };
    print_stats(&stats);

    if let Some(grid) = map {
        match illumination_map(&samples, frame.layout(), grid) {
            Some(cells) => {
                println!("  map {grid}x{grid}, normalised to its own mean");
                for row in cells.chunks(grid) {
                    let line: Vec<String> = row
                        .iter()
                        .map(|value| {
                            if value.is_finite() {
                                format!("{value:.3}")
                            } else {
                                "  -  ".to_owned()
                            }
                        })
                        .collect();
                    println!("    {}", line.join(" "));
                }
            }
            None => println!("  map        not available without a black level"),
        }
    }
    Ok(())
}

fn print_stats(stats: &FrameStats) {
    const NAMES: [&str; 4] = ["R", "G", "B", "E"];

    let present = |pick: &dyn Fn(&ColourStats) -> String| {
        stats
            .colours
            .iter()
            .enumerate()
            .filter_map(|(index, colour)| colour.map(|c| format!("{} {}", NAMES[index], pick(&c))))
            .collect::<Vec<_>>()
            .join("  ")
    };

    // The headline: where the median sits between black and white. A flat is
    // conventionally shot at a third to a half; a bias sits on the black point.
    println!("  level      {}", present(&|c| percent(c.level)));
    println!("  median     {}", present(&|c| format!("{:.0}", c.median)));
    println!("  spread     {}", present(&|c| format!("{:.0}", c.spread)));
    println!("  clipped    {}", present(&|c| percent(c.clipped)));

    if let Some(dominant) = stats.dominant() {
        println!("  levels     black {} white {}", number(dominant.black), number(dominant.white));
    }

    let blocks = stats.uniformity.blocks;
    let row = |start: usize| {
        blocks[start..start + 3]
            .iter()
            .map(|value| if value.is_finite() { format!("{value:.2}") } else { "-".to_owned() })
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Only meaningful on a flat, where it measures the light source rather than
    // the sensor: a ramp here is imprinted on every frame divided by it.
    println!("  uniformity {} / {} / {}   ramp {}", row(0), row(3), row(6), ratio(stats.uniformity.ramp()));
}

fn percent(value: f32) -> String {
    if value.is_finite() { format!("{:.1}%", value * 100.0) } else { "-".to_owned() }
}

fn number(value: f32) -> String {
    if value.is_finite() { format!("{value:.0}") } else { "not recorded".to_owned() }
}

fn ratio(value: f32) -> String {
    if value.is_finite() { format!("{value:.2}:1") } else { "-".to_owned() }
}

fn describe_frames(host: &PluginHost, files: &[PathBuf], decode: bool) -> Result<()> {
    let mut failures = 0;

    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{}", file.display());
        match describe_frame(host, file, decode) {
            Ok(()) => {}
            Err(error) => {
                failures += 1;
                println!("  error      {error:#}");
            }
        }
    }

    if failures > 0 {
        bail!("{failures} of {} frames could not be read", files.len());
    }
    Ok(())
}

fn describe_frame(host: &PluginHost, file: &Path, decode: bool) -> Result<()> {
    let frame = host.open(file).with_context(|| format!("opening {}", file.display()))?;
    print_frame(&frame);

    if decode {
        let started = Instant::now();
        let samples = frame.decode().with_context(|| format!("decoding {}", file.display()))?;
        let elapsed = started.elapsed();

        let megapixels = samples.len() as f64 / 1e6;
        println!(
            "  decoded    {} samples in {:.2}s ({:.1} Mpx/s)",
            samples.len(),
            elapsed.as_secs_f64(),
            megapixels / elapsed.as_secs_f64().max(f64::EPSILON)
        );
        if let Some((low, high)) = samples.range() {
            let kind = match samples {
                Samples::U16(_) => "u16",
                Samples::F32(_) => "f32",
            };
            println!("  values     {low} .. {high} ({kind})");
        }
    }
    Ok(())
}

fn print_frame(frame: &OpenFrame) {
    let plugin = frame.plugin().description();
    let layout = frame.layout();
    let info = frame.info();

    println!("  plugin     {} {} ({})", plugin.id, plugin.version, plugin.display_name);
    println!("  camera     {} {}", info.camera_make, info.camera_model);
    if !info.lens_model.is_empty() {
        println!("  lens       {}", info.lens_model);
    }

    let mut shot = Vec::new();
    if let Some(exposure) = info.exposure_seconds {
        shot.push(format::exposure(exposure));
    }
    if let Some(aperture) = info.aperture {
        shot.push(format!("f/{}", format::trim_zeros(aperture, 1)));
    }
    if let Some(iso) = info.iso {
        shot.push(format!("ISO {iso:.0}"));
    }
    if let Some(focal) = info.focal_length_mm {
        shot.push(format!("{} mm", format::trim_zeros(focal, 1)));
    }
    if !shot.is_empty() {
        println!("  exposure   {}", shot.join(", "));
    }
    if let Some(captured) = info.capture_time_unix {
        println!("  captured   {}", format::timestamp(captured));
    }
    if let Some(temperature) = info.sensor_temperature_c {
        println!("  sensor t   {} C", format::trim_zeros(temperature, 1));
    }

    println!(
        "  sensor     {} x {}, {} ch, {} bit {}",
        layout.width,
        layout.height,
        layout.components,
        layout.bits_per_sample,
        layout.sample_format.name()
    );
    match cfa_pattern_name(layout) {
        Some(pattern) => println!("  cfa        {pattern}"),
        None => println!("  cfa        none (not mosaiced)"),
    }

    // Display metadata, not storage: the sample buffer is always in sensor
    // readout order. Shown because a frame whose flag differs from its
    // neighbours was shot with the camera held another way round.
    println!(
        "  orientation {}",
        match layout.orientation {
            0 => "not recorded".to_owned(),
            n => n.to_string(),
        }
    );

    // Worth showing even when it equals the full frame: an unexpectedly small
    // active area is the first sign a frame was shot in a crop mode.
    println!(
        "  active     {} x {} at {},{}",
        layout.active_width, layout.active_height, layout.active_x, layout.active_y
    );

    let black_cells = (layout.black_level_width * layout.black_level_height) as usize;
    let black = layout.black_level[..black_cells.min(layout.black_level.len())]
        .iter()
        .map(|level| format::trim_zeros(*level as f64, 2))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "  black      {black} ({}x{})",
        layout.black_level_width, layout.black_level_height
    );
    println!("  white      {}", finite_list(&layout.white_level));

    let balance = finite_list(&layout.wb_coeffs);
    if !balance.is_empty() {
        println!("  as-shot wb {balance}");
    }
}

/// Joins the values a camera actually recorded, dropping the NaN placeholders
/// that mean "not present" rather than printing them.
fn finite_list(values: &[f32]) -> String {
    values
        .iter()
        .filter(|value| value.is_finite())
        .map(|value| format::trim_zeros(*value as f64, 4))
        .collect::<Vec<_>>()
        .join(", ")
}
