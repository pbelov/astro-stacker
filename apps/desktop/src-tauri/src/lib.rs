//! The desktop shell: what the interface is allowed to ask the core for.
//!
//! Two rules shape everything here.
//!
//! **The bridge carries structure, never sentences.** A `Mismatch` crosses as
//! its kind and its numbers, not as the string the command line prints. The
//! command line says its piece in English; the window says the same thing in
//! whatever language the user picked, and a shared string would force one of
//! them to be wrong. The English rendering is sent alongside as a fallback, for
//! a finding the interface has no words for yet — visibly English, so it reads
//! as a gap rather than as a translation.
//!
//! **No decision is taken here.** This crate opens files, calls the core and
//! serialises the answer. Which set to stack, which frames to drop, what a
//! mismatch means — all of that already has a home, and a second copy of it
//! behind a window would be the copy that drifts.

use std::path::{Path, PathBuf};

use astro_core::session::{
    CalibrationMatch, FrameId, FrameKind, FrameRole, Incompatibility, MatchQuality, Mismatch,
    Partition, RoleRule, ScanOptions, ScanReport, Session, Severity, Suspicion, Tolerances,
};
use astro_core::{PluginHost, default_plugin_dirs};
use serde::{Deserialize, Serialize};

/// What the user pointed the window at.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Roots {
    #[serde(default)]
    pub lights: Vec<String>,
    #[serde(default)]
    pub darks: Vec<String>,
    #[serde(default)]
    pub flats: Vec<String>,
    #[serde(default)]
    pub biases: Vec<String>,
    #[serde(default)]
    pub dark_flats: Vec<String>,
}

impl Roots {
    fn rules(&self) -> Vec<RoleRule> {
        let mut rules = Vec::new();
        for (paths, kind) in [
            (&self.lights, FrameKind::Light),
            (&self.darks, FrameKind::Dark),
            (&self.flats, FrameKind::Flat),
            (&self.biases, FrameKind::Bias),
            (&self.dark_flats, FrameKind::DarkFlat),
        ] {
            for path in paths {
                rules.push(RoleRule::new(PathBuf::from(path), Some(kind)));
            }
        }
        rules
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub frames: usize,
    pub seconds: f64,
    pub sets: Vec<SetDto>,
    pub plans: Vec<PlanDto>,
    /// Frames that were read but that no rule claimed. Counted rather than
    /// hidden: a session that quietly dropped forty frames looks exactly like
    /// one that never had them.
    pub unassigned: Vec<FrameDto>,
    pub suspicions: Vec<SuspicionDto>,
    pub rejected: Vec<RejectedDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDto {
    pub id: usize,
    pub kind: String,
    pub frames: usize,
    pub active: usize,
    pub body: String,
    /// Seconds, or `null` where the frames did not record one.
    pub exposure: Option<f64>,
    pub iso: Option<f64>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDto {
    pub lights: usize,
    pub light_frames: usize,
    pub roles: Vec<RoleDto>,
    pub blocked: Vec<BlockedDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub kind: String,
    pub set: usize,
    pub frames: usize,
    pub quality: String,
    pub alternatives: usize,
    pub mismatches: Vec<MismatchDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedDto {
    pub kind: String,
    pub candidates: usize,
    /// Why each candidate was refused. Carried rather than counted: "no dark
    /// matched" tells the user nothing they can act on, and "the darks are
    /// 5202x3465 and the lights 5202x3464" tells them exactly what happened.
    pub reasons: Vec<ReasonDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonDto {
    pub kind: String,
    pub expected: Option<f64>,
    pub found: Option<f64>,
    pub expected_text: Option<String>,
    pub found_text: Option<String>,
    pub english: String,
}

/// A mismatch as its kind and its numbers.
///
/// `english` is what the command line would print, carried so the window can
/// show something rather than nothing for a kind it has no words for. It is not
/// a translation and is not meant to look like one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MismatchDto {
    pub kind: String,
    pub severity: String,
    pub expected: Option<f64>,
    pub found: Option<f64>,
    pub expected_text: Option<String>,
    pub found_text: Option<String>,
    pub english: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuspicionDto {
    pub kind: String,
    pub set: usize,
    pub numbers: Vec<f64>,
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameDto {
    pub name: String,
    pub path: String,
    pub kind: Option<String>,
    pub exposure: Option<f64>,
    pub iso: Option<f64>,
    pub active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedDto {
    pub name: String,
    pub reason: String,
}

/// Where a development run finds the format plugins.
///
/// The shell has its own cargo workspace, so its executable lands somewhere the
/// plugins do not: they are built by the workspace at the top of the project.
/// The path is compiled in rather than searched for, and only into a debug
/// build — a release is a folder with the plugins beside the binary, and
/// nothing here should teach it otherwise.
#[cfg(debug_assertions)]
fn development_dirs() -> Vec<PathBuf> {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target");
    vec![target.join("release"), target.join("debug")]
}

#[cfg(not(debug_assertions))]
fn development_dirs() -> Vec<PathBuf> {
    Vec::new()
}

fn host() -> Result<PluginHost, String> {
    let mut host = PluginHost::new();
    let mut failures = Vec::new();
    for dir in default_plugin_dirs().into_iter().chain(development_dirs()) {
        let report = host.load_dir(&dir);
        for (path, error) in report.failures {
            failures.push(format!("{}: {error:#}", path.display()));
        }
    }
    if host.is_empty() {
        // A window with no format plugin opens, looks healthy and reads nothing.
        // Saying so here is the only place it can be caught before the user
        // concludes their frames are broken.
        let mut message = String::from("no format plugin loaded, so no frame can be read");
        if !failures.is_empty() {
            message.push_str(": ");
            message.push_str(&failures.join("; "));
        }
        return Err(message);
    }
    Ok(host)
}

#[tauri::command]
fn scan_session(roots: Roots) -> Result<SessionDto, String> {
    let rules = roots.rules();
    if rules.is_empty() {
        return Err("nothing to scan".to_owned());
    }
    let host = host()?;
    let options = ScanOptions { rules, ..Default::default() };
    let report = astro_core::session::scan(&host, &options).map_err(|error| format!("{error:#}"))?;
    let partition = report.session.partition(&Tolerances::default());
    Ok(describe(&report, &partition))
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[tauri::command]
fn formats() -> Vec<String> {
    host().map(|host| host.supported_extensions()).unwrap_or_default()
}

fn describe(report: &ScanReport, partition: &Partition) -> SessionDto {
    let session = &report.session;
    let sets = partition
        .sets
        .iter()
        .map(|set| {
            let first = set.members.first().copied();
            let record = first.map(|id| &session[id]);
            SetDto {
                id: set.id.index(),
                kind: set.kind().name().to_owned(),
                frames: set.members.len(),
                active: set.members.iter().filter(|id| session[**id].is_active()).count(),
                body: record.map(|r| r.info.camera_model.clone()).unwrap_or_default(),
                exposure: record.and_then(|r| r.info.exposure_seconds),
                iso: record.and_then(|r| r.info.iso),
                width: record.map_or(0, |r| r.layout.width),
                height: record.map_or(0, |r| r.layout.height),
            }
        })
        .collect();

    // Deepest first, from the core's own ranking. Set order is arbitrary, and
    // on a night that began with framing shots the arbitrary first plan is a
    // two-frame set — which on screen would read as the session.
    let plans = partition
        .plans_by_depth(session)
        .into_iter()
        .map(|(plan, light_frames)| PlanDto {
            lights: plan.lights.index(),
            light_frames,
            roles: plan
                .calibration()
                .map(|(kind, matched)| role(kind, matched, partition))
                .collect(),
            blocked: plan
                .blocked
                .iter()
                .map(|blocked| BlockedDto {
                    kind: blocked.kind.name().to_owned(),
                    candidates: blocked.candidates.len(),
                    reasons: blocked
                        .candidates
                        .iter()
                        .map(|(_, why)| reason(why))
                        .collect(),
                })
                .collect(),
        })
        .collect();

    SessionDto {
        frames: session.len(),
        seconds: report.elapsed.as_secs_f64(),
        sets,
        plans,
        unassigned: partition.unassigned.iter().map(|id| frame(session, *id)).collect(),
        suspicions: partition.suspicions.iter().map(suspicion).collect(),
        rejected: report
            .rejected
            .iter()
            .map(|(path, why)| RejectedDto {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                reason: format!("{why}"),
            })
            .collect(),
    }
}

fn role(kind: FrameKind, matched: &CalibrationMatch, partition: &Partition) -> RoleDto {
    RoleDto {
        kind: kind.name().to_owned(),
        set: matched.set.index(),
        frames: partition.set(matched.set).map_or(0, |set| set.members.len()),
        quality: match matched.quality {
            MatchQuality::Exact => "exact",
            MatchQuality::OnlyCandidate => "only",
            MatchQuality::ClosestExposure => "exposure",
            MatchQuality::ClosestGain => "gain",
        }
        .to_owned(),
        alternatives: matched.alternatives,
        mismatches: matched.mismatches.iter().map(mismatch).collect(),
    }
}

fn mismatch(found: &Mismatch) -> MismatchDto {
    let severity = match found.severity() {
        Severity::Note => "note",
        Severity::Warning => "warning",
    }
    .to_owned();
    let english = found.to_string();

    let (kind, expected, actual, expected_text, found_text) = match found {
        Mismatch::Exposure { expected, found } => {
            ("exposure", Some(*expected), Some(*found), None, None)
        }
        Mismatch::Gain { expected, found, .. } => {
            ("gain", Some(*expected), Some(*found), None, None)
        }
        Mismatch::SensorTemperature { expected, found, .. } => {
            ("temperature", Some(*expected), Some(*found), None, None)
        }
        Mismatch::BlackLevel { expected, found } => {
            ("black", Some(f64::from(*expected)), Some(f64::from(*found)), None, None)
        }
        Mismatch::WhiteLevel { expected, found } => {
            ("white", Some(f64::from(*expected)), Some(f64::from(*found)), None, None)
        }
        Mismatch::Orientation { expected, found } => {
            ("orientation", Some(f64::from(*expected)), Some(f64::from(*found)), None, None)
        }
        Mismatch::CameraModel { expected, found } => {
            ("body", None, None, Some(expected.clone()), Some(found.clone()))
        }
        Mismatch::LensModel { expected, found } => {
            ("lens", None, None, Some(expected.clone()), Some(found.clone()))
        }
        Mismatch::OpticalTrain { expected, found, .. } => {
            ("optics", *expected, *found, None, None)
        }
        Mismatch::Elapsed { seconds } => ("elapsed", Some(*seconds as f64), None, None, None),
        Mismatch::Unrecorded { .. } => ("unrecorded", None, None, None, None),
    };

    MismatchDto {
        kind: kind.to_owned(),
        severity,
        expected,
        found: actual,
        expected_text,
        found_text,
        english,
    }
}

fn reason(why: &Incompatibility) -> ReasonDto {
    let english = why.to_string();
    let pair = |a: f64, b: f64| (Some(a), Some(b), None, None);
    let words = |a: String, b: String| (None, None, Some(a), Some(b));

    let (kind, expected, found, expected_text, found_text) = match why {
        Incompatibility::Dimensions { expected, found } => (
            "dimensions",
            None,
            None,
            Some(format!("{}x{}", expected.0, expected.1)),
            Some(format!("{}x{}", found.0, found.1)),
        ),
        Incompatibility::CfaPattern { expected, found } => {
            let (a, b, c, d) = words(
                expected.clone().unwrap_or_default(),
                found.clone().unwrap_or_default(),
            );
            ("cfa", a, b, c, d)
        }
        Incompatibility::CameraModel { expected, found } => {
            let (a, b, c, d) = words(expected.clone(), found.clone());
            ("body", a, b, c, d)
        }
        Incompatibility::Gain { expected, found } => {
            let (a, b, c, d) = pair(*expected, *found);
            ("gain", a, b, c, d)
        }
        Incompatibility::BitDepth { expected, found } => {
            let (a, b, c, d) = pair(f64::from(*expected), f64::from(*found));
            ("depth", a, b, c, d)
        }
        Incompatibility::Scale { expected, found } => {
            let (a, b, c, d) = pair(f64::from(*expected), f64::from(*found));
            ("scale", a, b, c, d)
        }
        Incompatibility::Unrecorded { .. } => ("unrecorded", None, None, None, None),
        _ => ("other", None, None, None, None),
    };

    ReasonDto { kind: kind.to_owned(), expected, found, expected_text, found_text, english }
}

fn suspicion(found: &Suspicion) -> SuspicionDto {
    match found {
        Suspicion::MinorityOfKind { set, members, dominant_members, .. } => SuspicionDto {
            kind: "minority".to_owned(),
            set: set.index(),
            numbers: vec![*members as f64, *dominant_members as f64],
            text: None,
        },
        Suspicion::BiasIsNotTheShortestExposure { set, seconds, shortest } => SuspicionDto {
            kind: "biasNotShortest".to_owned(),
            set: set.index(),
            numbers: vec![*seconds, *shortest],
            text: None,
        },
        Suspicion::FlatNeedsDarkFlats { set, seconds } => SuspicionDto {
            kind: "flatNeedsDarkFlats".to_owned(),
            set: set.index(),
            numbers: vec![*seconds],
            text: None,
        },
        Suspicion::ProbableInCameraDarkSubtraction {
            set,
            interval_seconds,
            exposure_seconds,
        } => SuspicionDto {
            kind: "inCameraDark".to_owned(),
            set: set.index(),
            numbers: vec![*interval_seconds, *exposure_seconds],
            text: None,
        },
        Suspicion::CalibrationSpansNights { set, kind, span_seconds } => SuspicionDto {
            kind: "spansNights".to_owned(),
            set: set.index(),
            numbers: vec![*span_seconds as f64],
            text: Some(kind.name().to_owned()),
        },
    }
}

fn frame(session: &Session, id: FrameId) -> FrameDto {
    let record = &session[id];
    let path = session.path(id);
    FrameDto {
        name: path
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| format!("frame {}", id.index())),
        path: path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        kind: match &record.role {
            FrameRole::Assigned(kind) => Some(kind.name().to_owned()),
            FrameRole::Unassigned { .. } => None,
        },
        exposure: record.info.exposure_seconds,
        iso: record.info.iso,
        active: record.is_active(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![scan_session, app_version, formats])
        .run(tauri::generate_context!())
        .expect("error while running astro-stacker");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's own frames, which are not in the repository. A machine
    /// without them skips rather than fails: the point of the test is to catch
    /// the bridge lying about a real session, and there is nothing to lie about
    /// when there is no session.
    fn testdata() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/real-frames-canon-60da");
        root.is_dir().then_some(root)
    }

    #[test]
    fn a_real_session_crosses_the_bridge_intact() {
        let Some(root) = testdata() else { return };
        let roots = Roots {
            lights: vec![root.join("lights").display().to_string()],
            darks: vec![root.join("darks").display().to_string()],
            flats: vec![root.join("flats56_iso100-1").display().to_string()],
            biases: vec![root.join("bias").display().to_string()],
            dark_flats: Vec::new(),
        };

        let dto = scan_session(roots).expect("the session scans");
        assert!(dto.frames > 200, "the reference session is deep: {}", dto.frames);
        assert!(!dto.sets.is_empty(), "it partitions into sets");
        assert!(!dto.plans.is_empty(), "and at least one is stackable");

        // Every set names a body and a kind. A blank here is the bridge dropping
        // a field, which on screen looks like a session with no camera.
        for set in &dto.sets {
            assert!(!set.kind.is_empty(), "set {} has no kind", set.id);
            assert!(!set.body.is_empty(), "set {} has no body", set.id);
            assert!(set.active <= set.frames);
        }

        // The plan must reach its calibration, and every difference it reports
        // must be one the interface has words for - an English fallback on
        // screen is a gap, and this is where it gets noticed.
        let plan = &dto.plans[0];
        assert!(plan.light_frames > 200, "{} lights", plan.light_frames);
        assert!(!plan.roles.is_empty(), "calibration was matched");
        const KNOWN: [&str; 11] = [
            "exposure", "gain", "temperature", "black", "white", "orientation", "body", "lens",
            "optics", "elapsed", "unrecorded",
        ];
        for role in &plan.roles {
            assert!(role.frames > 0, "{} matched an empty set", role.kind);
            for found in &role.mismatches {
                assert!(
                    KNOWN.contains(&found.kind.as_str()),
                    "no words for a {} mismatch: {}",
                    found.kind,
                    found.english
                );
            }
        }
    }

    #[test]
    fn a_scan_of_nothing_says_so_rather_than_returning_an_empty_session() {
        // An empty session on screen is indistinguishable from a session whose
        // frames were all unreadable, and the user would go looking at their
        // files rather than at what they picked.
        assert!(scan_session(Roots::default()).is_err());
    }
}
