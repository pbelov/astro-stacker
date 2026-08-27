//! Rendering a scan for a terminal.
//!
//! Everything printed here comes out of typed data on the [`Partition`], never
//! out of a string built during the scan. That is what will let the same report
//! render as JSON, and later as a window, without any of it being re-parsed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use astro_core::session::{
    CalibrationMatch, FrameKind, FrameSet, MatchQuality, Partition, Rejection, ScanReport, Session,
    MAIN_GROUP, Severity, Suggestion, Suspicion,
};

use crate::format;

pub fn render(report: &ScanReport, partition: &Partition) {
    let session = &report.session;

    println!(
        "scanned {} from {} in {:.1}s, {} at a time",
        format::plural(session.len(), "frame"),
        format::plural(session.len() + report.rejected.len(), "file"),
        report.elapsed.as_secs_f64(),
        report.workers
    );

    for plan_index in 0..partition.plans.len() {
        println!();
        print_plan(partition, plan_index);
    }

    print_splits(partition);
    print_suspicions(partition);
    print_unassigned(session, partition);
    print_rejected(report);
}

fn print_plan(partition: &Partition, plan_index: usize) {
    let plan = &partition.plans[plan_index];
    let Some(lights) = partition.set(plan.lights) else { return };

    print_set_heading(lights);

    for (kind, matched) in plan.calibration() {
        let Some(set) = partition.set(matched.set) else { continue };
        println!(
            "  {:<10} {}, {}",
            kind.name(),
            format::plural(set.len(), "frame"),
            describe_quality(matched)
        );
        for finding in &matched.mismatches {
            let label = match finding.severity() {
                Severity::Warning => "warning",
                Severity::Note => "note",
            };
            println!("    {label:<8} {finding}");
        }
    }

    // Roles the user supplied frames for that could not be used. Never silent:
    // otherwise the user believes the stack was calibrated.
    for blocked in &plan.blocked {
        // Naming the set it was judged against matters for a dark flat, which is
        // measured against the flats: without it the flats' exposure would read
        // as what the lights expected.
        let against = partition.set(blocked.against).map_or_else(
            String::new,
            |set| {
                if set.id == plan.lights {
                    String::new()
                } else {
                    format!(" against set {} ({})", set.id.index(), set.kind().name())
                }
            },
        );
        println!(
            "  {:<10} refused{against}, and nothing else was available",
            blocked.kind.name()
        );
        for (set, reason) in &blocked.candidates {
            let count = partition.set(*set).map_or(0, FrameSet::len);
            println!("    {}: {reason}", format::plural(count, "frame"));
        }
    }

    let missing: Vec<&str> = [
        (FrameKind::Bias, plan.bias.is_some()),
        (FrameKind::Dark, plan.dark.is_some()),
        (FrameKind::Flat, plan.flat.is_some()),
    ]
    .into_iter()
    .filter(|(kind, present)| {
        !present && !plan.blocked.iter().any(|blocked| blocked.kind == *kind)
    })
    .map(|(kind, _)| kind.name())
    .collect();
    if !missing.is_empty() {
        println!("  {:<10} {}", "no", missing.join(", "));
    }
}

fn print_set_heading(set: &FrameSet) {
    let key = &set.key;
    let exposure = key
        .exposure
        .nominal()
        .map_or_else(|| "exposure unknown".to_owned(), format::exposure);
    let gain = match key.partition.gain {
        astro_core::session::GainBucket::Iso(iso) => format!("ISO {iso}"),
        astro_core::session::GainBucket::Unknown => "ISO unknown".to_owned(),
    };
    let group = key.partition.group.as_ref();

    println!(
        "set {}  {}  {}  {}  {}  {}  {}{}",
        set.id.index(),
        key.partition.kind.name(),
        key.partition.body.display(),
        key.partition.geometry.describe(),
        exposure,
        gain,
        format::plural(set.len(), "frame"),
        if group == MAIN_GROUP { String::new() } else { format!("  [{group}]") }
    );
}

fn describe_quality(matched: &CalibrationMatch) -> String {
    let quality = match matched.quality {
        MatchQuality::Exact => "an exact match",
        MatchQuality::OnlyCandidate => "the only candidate",
        MatchQuality::ClosestExposure => "the closest exposure",
        MatchQuality::ClosestGain => "the closest gain",
    };
    match matched.alternatives {
        0 => quality.to_owned(),
        1 => format!("{quality}, 1 other was possible"),
        others => format!("{quality}, {others} others were possible"),
    }
}

fn print_splits(partition: &Partition) {
    if partition.splits.is_empty() {
        return;
    }
    println!();
    println!("frames of one kind that could not share a set");
    for split in &partition.splits {
        println!(
            "  {:<8} set {} against set {}: {}",
            split.kind.name(),
            split.other.index(),
            split.reference.index(),
            split.reason
        );
    }
}

fn print_suspicions(partition: &Partition) {
    if partition.suspicions.is_empty() {
        return;
    }
    println!();
    println!("worth a look");
    for suspicion in &partition.suspicions {
        println!("  {}", describe_suspicion(suspicion));
    }
}

fn describe_suspicion(suspicion: &Suspicion) -> String {
    match suspicion {
        Suspicion::BiasIsNotTheShortestExposure { set, seconds, shortest } => format!(
            "set {} is filed as bias but runs to {}, while the shortest frame in the session is {}. \
             Flats through a fast optic land here.",
            set.index(),
            format::exposure(*seconds),
            format::exposure(*shortest)
        ),
        Suspicion::FlatNeedsDarkFlats { set, seconds } => format!(
            "set {} holds flats of {}, long enough to collect dark current, and no dark flats \
             matched. A bias alone will not remove it.",
            set.index(),
            format::exposure(*seconds)
        ),
        Suspicion::ProbableInCameraDarkSubtraction { set, interval_seconds, exposure_seconds } => {
            format!(
                "set {} has frames {:.0}s apart at {:.0}s exposure, which is what long-exposure \
                 noise reduction looks like. Those frames already had a dark subtracted in camera, \
                 so a master dark would remove it twice.",
                set.index(),
                interval_seconds,
                exposure_seconds
            )
        }
        Suspicion::CalibrationSpansNights { set, kind, span_seconds } => format!(
            "set {} holds {} frames shot across {:.0} hours. A {} records the optical train at a \
             moment; averaging several nights averages several dust patterns and focus positions \
             into one that matches none of them.",
            set.index(),
            kind.name(),
            *span_seconds as f64 / 3600.0,
            kind.name()
        ),
    }
}

fn print_unassigned(session: &Session, partition: &Partition) {
    if partition.unassigned.is_empty() {
        return;
    }
    println!();
    println!("unassigned  {}", format::plural(partition.unassigned.len(), "frame"));
    println!("  nothing in the path or the metadata says what these are, and guessing at a");
    println!("  calibration frame ruins a stack quietly rather than loudly");

    // Grouped by directory, because that is the unit the fix is applied in.
    let mut by_directory: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for id in &partition.unassigned {
        let Some(path) = session.path(*id) else { continue };
        let directory = path.parent().unwrap_or(&path).to_owned();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        by_directory.entry(directory).or_default().push(name);
    }

    for (directory, mut names) in by_directory {
        names.sort();
        let shown = match names.len() {
            1 => names[0].clone(),
            _ => format!("{} .. {} ({} files)", names[0], names[names.len() - 1], names.len()),
        };
        println!();
        println!("  {}", directory.display());
        println!("    {shown}");

        let proposed = proposal_for(session, partition, &directory);
        let suggestion =
            Suggestion::AssignDirectory { directory: directory.clone(), frames: names.len(), proposed };
        match (proposed, suggestion.command_fragment()) {
            (Some(kind), Some(fragment)) => {
                println!("    looks like a {}: {fragment}", kind.name());
            }
            // Nothing was proposed, so there is no line to paste — only the
            // menu. Printing one flag here would be the guess the whole model
            // refuses to make.
            _ => println!(
                "    name them with one of --lights --darks --flats --biases --dark-flats \"{}\"",
                directory.display()
            ),
        }
    }
}

/// The one kind every unassigned frame in a directory was proposed as, if they
/// all agree. A directory whose frames disagree gets no suggestion rather than
/// an arbitrary one.
fn proposal_for(
    session: &Session,
    partition: &Partition,
    directory: &std::path::Path,
) -> Option<FrameKind> {
    let mut proposed: Option<FrameKind> = None;
    for id in &partition.unassigned {
        let path = session.path(*id)?;
        if path.parent() != Some(directory) {
            continue;
        }
        let kind = session.frame(*id)?.inference()?.proposed()?;
        match proposed {
            Some(existing) if existing != kind => return None,
            _ => proposed = Some(kind),
        }
    }
    proposed
}

fn print_rejected(report: &ScanReport) {
    if report.rejected.is_empty() {
        return;
    }
    let mut not_frames = Vec::new();
    let mut duplicates = Vec::new();
    let mut problems = Vec::new();
    for (path, reason) in &report.rejected {
        match reason {
            Rejection::NotAFrame => not_frames.push(path),
            // A duplicate was read perfectly. It is set aside because its bytes
            // are already in the session, which is not a failure to report under
            // a heading that says one.
            Rejection::Duplicate { of } => duplicates.push((path, *of)),
            _ => problems.push((path, reason)),
        }
    }

    if !duplicates.is_empty() {
        println!();
        println!("already in the session  {}", format::plural(duplicates.len(), "file"));
        for (path, of) in duplicates.iter().take(10) {
            // Resolved to a path here rather than in `Rejection`'s Display,
            // which has no session to consult and can only print an index.
            let original = report
                .session
                .path(*of)
                .map_or_else(|| String::from("another file"), |p| p.display().to_string());
            println!("  {}", path.display());
            println!("    same bytes as {original}");
        }
        if duplicates.len() > 10 {
            println!("  and {} more", duplicates.len() - 10);
        }
    }

    if !problems.is_empty() {
        println!();
        println!("could not be read  {}", format::plural(problems.len(), "file"));
        for (path, reason) in problems {
            println!("  {}: {reason}", path.display());
        }
    }
    if !not_frames.is_empty() {
        println!();
        println!("not frames  {}", format::plural(not_frames.len(), "file"));
        for path in not_frames.iter().take(10) {
            println!("  {}", path.display());
        }
        if not_frames.len() > 10 {
            println!("  and {} more", not_frames.len() - 10);
        }
    }
}
