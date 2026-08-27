//! Rendering a scan for a terminal.
//!
//! Everything printed here comes out of typed data on the [`Partition`], never
//! out of a string built during the scan. That is what will let the same report
//! render as JSON, and later as a window, without any of it being re-parsed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use astro_core::session::{
    CalibrationMatch, FrameKind, FrameSet, MatchQuality, Partition, Rejection, ScanReport, Session,
    Severity, Suggestion, Suspicion,
};

use crate::format;

pub fn render(report: &ScanReport, partition: &Partition) {
    let session = &report.session;

    println!(
        "scanned {} frames from {} files in {:.1}s, {} at a time",
        session.len(),
        session.len() + report.rejected.len(),
        report.elapsed.as_secs_f64(),
        report.workers
    );

    for plan_index in 0..partition.plans.len() {
        println!();
        print_plan(session, partition, plan_index);
    }

    print_splits(partition);
    print_suspicions(partition);
    print_unassigned(session, partition);
    print_rejected(report);
}

fn print_plan(session: &Session, partition: &Partition, plan_index: usize) {
    let plan = &partition.plans[plan_index];
    let Some(lights) = partition.set(plan.lights) else { return };

    print_set_heading(session, lights);

    for (kind, matched) in plan.calibration() {
        let Some(set) = partition.set(matched.set) else { continue };
        println!(
            "  {:<10} {} frames, {}",
            kind.name(),
            set.len(),
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
        println!("  {:<10} refused, and nothing else was available", blocked.kind.name());
        for (set, reason) in &blocked.candidates {
            let count = partition.set(*set).map_or(0, FrameSet::len);
            println!("    {count} frames: {reason}");
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

fn print_set_heading(session: &Session, set: &FrameSet) {
    let key = &set.key;
    let exposure = key
        .exposure
        .nominal()
        .map_or_else(|| "exposure unknown".to_owned(), format::exposure);
    let gain = match key.partition.gain {
        astro_core::session::GainBucket::Iso(iso) => format!("ISO {iso}"),
        astro_core::session::GainBucket::Unknown => "ISO unknown".to_owned(),
    };
    let group = session.group_name(key.partition.group);

    println!(
        "set {}  {}  {}  {}  {}  {}  {} frames{}",
        set.id.index(),
        key.partition.kind.name(),
        key.partition.body.display(),
        key.partition.geometry.describe(),
        exposure,
        gain,
        set.len(),
        if group == "main" { String::new() } else { format!("  [{group}]") }
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
            "set {} holds {} flats long enough to collect dark current, and no dark flats matched. \
             A bias alone will not remove it.",
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
            "set {} holds {}s shot across {:.0} hours. A {} records the optical train at a moment; \
             averaging several nights averages several dust patterns into one that matches none.",
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
    println!("unassigned  {} frames", partition.unassigned.len());
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
            Suggestion::AssignDirectory { directory, frames: names.len(), proposed };
        if let Some(fragment) = suggestion.command_fragment() {
            match proposed {
                Some(kind) => println!("    looks like {}s: {fragment}", kind.name()),
                None => println!("    name them: {fragment}"),
            }
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
    let mut problems = Vec::new();
    for (path, reason) in &report.rejected {
        match reason {
            Rejection::NotAFrame => not_frames.push(path),
            _ => problems.push((path, reason)),
        }
    }

    if !problems.is_empty() {
        println!();
        println!("could not be read  {} files", problems.len());
        for (path, reason) in problems {
            println!("  {}: {reason}", path.display());
        }
    }
    if !not_frames.is_empty() {
        println!();
        println!("not frames  {} files", not_frames.len());
        for path in not_frames.iter().take(10) {
            println!("  {}", path.display());
        }
        if not_frames.len() > 10 {
            println!("  and {} more", not_frames.len() - 10);
        }
    }
}
