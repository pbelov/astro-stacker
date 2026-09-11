//! The `scan` subcommand: point at a folder, get a plan.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use astro_core::PluginHost;
use astro_core::session::{
    FrameKind, MAIN_GROUP, RoleRule, ScanOptions, Tolerances, scan_with_progress,
};
use clap::{ArgMatches, Args};

use crate::report;

/// Below this many files a scan finishes before a progress line could be read.
const PROGRESS_THRESHOLD: usize = 20;

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Files or directories to scan. Frames found here are added without a
    /// kind unless one of the options below claims them.
    #[arg(value_name = "PATH", id = "paths")]
    pub paths: Vec<PathBuf>,

    /// Assign every frame under PATH. Repeatable. This is the only thing that
    /// assigns a kind: everything else the scan learns is a proposal.
    #[arg(long, id = "lights", value_name = "PATH")]
    pub lights: Vec<PathBuf>,

    #[arg(long, id = "darks", value_name = "PATH")]
    pub darks: Vec<PathBuf>,

    #[arg(long, id = "flats", value_name = "PATH")]
    pub flats: Vec<PathBuf>,

    #[arg(long, id = "biases", value_name = "PATH")]
    pub biases: Vec<PathBuf>,

    #[arg(long = "dark-flats", id = "dark_flats", value_name = "PATH")]
    pub dark_flats: Vec<PathBuf>,

    /// Put the paths that follow into a named group. Calibration in a group
    /// serves only that group; calibration outside every group serves all of
    /// them, which is how a bias library shot once a year gets reused.
    #[arg(long, id = "group", value_name = "NAME")]
    pub group: Vec<String>,

    /// Also read the subdirectories of every directory given. Off by default:
    /// a subdirectory inside a folder of lights is usually where the frames
    /// that should not be stacked were put.
    #[arg(long)]
    pub recurse: bool,

    /// Do not read folder and file names as evidence.
    #[arg(long = "no-infer", id = "no_infer")]
    pub no_infer: bool,

    /// How far two exposures may differ and still count as the same, in per
    /// cent of the longer.
    #[arg(long = "tolerance-exposure", id = "tolerance_exposure", value_name = "PERCENT")]
    pub tolerance_exposure: Option<f64>,

    /// Leave this frame out, by file name. Repeatable.
    ///
    /// The frame is still read and still appears in the report, marked as
    /// excluded by you, so a session that drops a frame never looks like one
    /// that never had it.
    #[arg(long, id = "exclude", value_name = "NAME")]
    pub exclude: Vec<String>,

    /// Act on this light set rather than the deepest one. The numbers are
    /// what `scan` prints.
    #[arg(long, id = "set", value_name = "N")]
    pub set: Option<usize>,

    /// Do not print progress while reading.
    #[arg(long, short, id = "quiet")]
    pub quiet: bool,
}

/// Turns the shared frame-selection options into what the session model wants.
///
/// Shared with `master` rather than duplicated: two commands that disagreed
/// about what `--exclude` or `--group` means would be worse than either being
/// wrong on its own.
pub fn options_from(args: &ScanArgs, matches: &ArgMatches) -> Result<(ScanOptions, Tolerances)> {
    // Reserved rather than merely special: a group by this name is handed the
    // id whose calibration serves every other group, so accepting it would
    // silently drop the isolation the user asked for.
    if let Some(reserved) = args.group.iter().find(|name| name.eq_ignore_ascii_case(MAIN_GROUP)) {
        bail!(
            "--group {reserved} is reserved. Calibration named before any --group already serves \
             every group, so a bias library needs no group at all; call this one something else."
        );
    }

    let rules = rules_from(args, matches);
    if rules.is_empty() {
        bail!(
            "nothing to scan: give a path, or name one with --lights, --darks, --flats or --biases"
        );
    }

    let options = ScanOptions {
        rules,
        recursive: args.recurse,
        infer_from_paths: !args.no_infer,
        excluded: args.exclude.clone(),
        ..Default::default()
    };
    let mut tolerances = Tolerances::default();
    if let Some(percent) = args.tolerance_exposure {
        tolerances.exposure_relative = percent / 100.0;
    }
    Ok((options, tolerances))
}

pub fn run(host: &PluginHost, args: &ScanArgs, matches: &ArgMatches) -> Result<()> {

    let (options, tolerances) = options_from(args, matches)?;

    // A scan of five hundred frames takes long enough that silence reads as a
    // hang. Reporting at each tenth keeps the progress from being the slow part,
    // and a handful of files finishes before a progress line would be read.
    let last = AtomicUsize::new(0);
    let report = scan_with_progress(host, &options, &|progress| {
        if args.quiet || progress.total < PROGRESS_THRESHOLD {
            return;
        }
        let step = (progress.total / 10).max(1);
        if progress.opened.is_multiple_of(step)
            && last.swap(progress.opened, Ordering::Relaxed) != progress.opened
        {
            eprint!("\r  read {} of {} files", progress.opened, progress.total);
        }
    })?;
    if !args.quiet && report.rejected.len() + report.session.len() >= PROGRESS_THRESHOLD {
        eprint!("\r                                        \r");
    }

    let partition = report.session.partition(&tolerances);
    report::render(&report, &partition);

    // An exit code a script can branch on: nothing stackable is a different
    // outcome from a scan that failed to run.
    if partition.plans.is_empty() {
        bail!("no stackable set of lights was formed");
    }
    Ok(())
}

/// Rebuilds the role rules in the order the options were typed.
///
/// The order matters because `--group` applies to the options that follow it,
/// so a per-night layout reads as
/// `--group mon --lights .. --darks .. --group tue --lights ..`. The parsed
/// struct has the values but not the ordering, so the raw match indices supply
/// it.
fn rules_from(args: &ScanArgs, matches: &ArgMatches) -> Vec<RoleRule> {
    enum Entry {
        Group(String),
        Role(Option<FrameKind>, PathBuf),
    }

    let roles: [(&str, Option<FrameKind>, &Vec<PathBuf>); 6] = [
        ("paths", None, &args.paths),
        ("lights", Some(FrameKind::Light), &args.lights),
        ("darks", Some(FrameKind::Dark), &args.darks),
        ("flats", Some(FrameKind::Flat), &args.flats),
        ("biases", Some(FrameKind::Bias), &args.biases),
        ("dark_flats", Some(FrameKind::DarkFlat), &args.dark_flats),
    ];

    let mut entries: Vec<(usize, Entry)> = Vec::new();
    for (id, kind, values) in roles {
        let Some(indices) = matches.indices_of(id) else { continue };
        for (path, index) in values.iter().zip(indices) {
            entries.push((index, Entry::Role(kind, path.clone())));
        }
    }
    if let Some(indices) = matches.indices_of("group") {
        for (name, index) in args.group.iter().zip(indices) {
            entries.push((index, Entry::Group(name.clone())));
        }
    }
    entries.sort_by_key(|(index, _)| *index);

    let mut group: Option<String> = None;
    let mut rules = Vec::new();
    for (_, entry) in entries {
        match entry {
            Entry::Group(name) => group = Some(name),
            Entry::Role(kind, path) => {
                rules.push(RoleRule::new(path, kind).in_group(group.clone()));
            }
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use clap::{CommandFactory, FromArgMatches};

    fn parse(argv: &[&str]) -> Vec<RoleRule> {
        let matches = crate::Cli::command().get_matches_from(argv);
        let scan = matches.subcommand_matches("scan").expect("a scan command");
        let args = ScanArgs::from_arg_matches(scan).expect("valid arguments");
        rules_from(&args, scan)
    }

    #[test]
    fn a_group_claims_the_paths_that_follow_it() {
        // The per-night layout, which only works if the order of the options is
        // preserved: Monday's darks must not end up serving Tuesday.
        let rules = parse(&[
            "astro-stacker",
            "scan",
            "--group",
            "mon",
            "--lights",
            "D:/mon/lights",
            "--darks",
            "D:/mon/darks",
            "--group",
            "tue",
            "--lights",
            "D:/tue/lights",
            "--biases",
            "D:/library/bias",
        ]);

        let named = |root: &str| {
            rules
                .iter()
                .find(|rule| rule.root.as_path() == Path::new(root))
                .unwrap_or_else(|| panic!("no rule for {root}"))
        };
        assert_eq!(named("D:/mon/lights").group_name.as_deref(), Some("mon"));
        assert_eq!(named("D:/mon/darks").group_name.as_deref(), Some("mon"));
        assert_eq!(named("D:/tue/lights").group_name.as_deref(), Some("tue"));
        // The bias library came after --group tue, so it is Tuesday's. Putting
        // it before every --group is what makes it serve both nights.
        assert_eq!(named("D:/library/bias").group_name.as_deref(), Some("tue"));
    }

    #[test]
    fn paths_before_any_group_belong_to_every_group() {
        let rules =
            parse(&["astro-stacker", "scan", "--biases", "D:/library", "--group", "mon", "--lights", "D:/mon"]);
        assert_eq!(rules[0].root, PathBuf::from("D:/library"));
        assert_eq!(rules[0].group_name, None);
        assert_eq!(rules[1].group_name.as_deref(), Some("mon"));
    }

    #[test]
    fn a_bare_path_is_added_without_claiming_what_it_is() {
        let rules = parse(&["astro-stacker", "scan", "D:/astro/M31"]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].kind, None);
    }

    #[test]
    fn every_role_flag_reaches_the_rules() {
        let rules = parse(&[
            "astro-stacker",
            "scan",
            "--lights",
            "l",
            "--darks",
            "d",
            "--flats",
            "f",
            "--biases",
            "b",
            "--dark-flats",
            "df",
        ]);
        let kinds: Vec<Option<FrameKind>> = rules.iter().map(|rule| rule.kind).collect();
        assert_eq!(
            kinds,
            vec![
                Some(FrameKind::Light),
                Some(FrameKind::Dark),
                Some(FrameKind::Flat),
                Some(FrameKind::Bias),
                Some(FrameKind::DarkFlat),
            ]
        );
    }
}
