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

// Only the development plugin search and the tests reach for `Path`, and
// both are gone from a release build, which is what leaves the import unused
// there.
#[cfg(any(debug_assertions, test))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use astro_core::calibrate::{fits, tiff};
use astro_core::integrate::Rejection;
use astro_core::pipeline::stack::{StackOptions, combine, select};
use astro_core::pipeline::view;
use astro_core::pipeline::{Flow, Step};
use astro_core::session::mosaic::Mosaic;
use astro_core::stars::DetectOptions;
use astro_core::session::{
    CalibrationMatch, FrameId, FrameKind, FrameRole, Incompatibility, MatchQuality, Mismatch,
    Partition, RoleRule, ScanOptions, ScanReport, Session, Severity, Suspicion, Tolerances,
};
use astro_core::{PluginHost, default_plugin_dirs};
use tauri::State;
use tauri::ipc::Channel;
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

/// What a running pass reports back while it runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub enum Progress {
    /// Which stage the run is in. Stacking is four passes over the frames and a
    /// single bar across all of them would sit still through the slow one, so
    /// each says what it is.
    Aligning,
    Stacking { pass: usize, passes: usize, done: usize, total: usize, name: String },
    Writing,
    /// Building one of the masters. These come first and take about a third of
    /// the time, so a bar that only counted lights would sit at zero through
    /// them and read as a hang.
    Master { kind: String, done: usize, total: usize },
    Frame { done: usize, total: usize, name: String },
}

/// One light, measured.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameQualityDto {
    pub name: String,
    pub stars: usize,
    /// Photosites across the trail: the seeing and the focus.
    pub fwhm: f64,
    /// How much longer than wide the stars are: what the mount did.
    pub trail: f64,
    /// Degrees, in [0, 180). NaN where the stars were round enough to have no
    /// direction, which is not the same as pointing at zero.
    pub angle: f64,
    pub agreement: f64,
    pub sky: f64,
    pub noise: f64,
    pub saturated: usize,
    pub oversized: usize,
    pub seconds: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityDto {
    pub frames: Vec<FrameQualityDto>,
    pub failed: Vec<RejectedDto>,
    pub seconds: f64,
    /// True when the user stopped it: `frames` then holds part of a run and
    /// must not be read as the whole of one.
    pub stopped: bool,
    /// Where the run as a whole is trailed, in degrees, and how consistently.
    ///
    /// Aggregated in the core as a spin-2 quantity. An ellipse at 179 degrees
    /// and one at 1 point almost the same way and their arithmetic mean is 90,
    /// perpendicular to both, so this cannot be computed by averaging the
    /// per-frame angles the window was sent.
    pub direction: f64,
    pub direction_agreement: f64,
    /// How many stars were kept per frame, and whether that was a ceiling
    /// rather than a count.
    pub star_cap: usize,
}

/// One frame, and what the selection made of it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackedFrameDto {
    pub name: String,
    pub weight: f64,
    pub brightness: f64,
    pub trail: f64,
    pub fwhm: f64,
    pub shift: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackResultDto {
    pub frames: Vec<StackedFrameDto>,
    pub refused: Vec<RejectedDto>,
    /// How many frames of median quality the run is worth. Lower than the count
    /// whenever the weights are uneven, and the honest answer to how deep it is.
    pub effective: usize,
    pub stacked: usize,
    pub width: usize,
    pub height: usize,
    pub seconds: f64,
    /// Per colour plane: what share carries data, and how deep it is.
    pub coverage: Vec<CoverageDto>,
    pub written: Vec<String>,
    /// How many deposits the second pass threw away, out of how many, or `null`
    /// where rejection was not asked for.
    pub rejected: Option<(usize, usize)>,
    /// Frames that lost an unusual share, worst first. A frame losing a lot is
    /// not a frame full of satellites but a sign the threshold does not fit the
    /// run, and only the user can tell those apart.
    pub heavy_losses: Vec<(String, f64)>,
    /// The preview's own size, which is smaller than the canvas.
    pub preview_width: usize,
    pub preview_height: usize,
    pub note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageDto {
    pub filled: f64,
    pub median_depth: f64,
    pub thinnest: f64,
}

/// Set while a pass runs, cleared when it ends.
#[derive(Default)]
pub struct Running {
    cancel: Arc<AtomicBool>,
    /// The last preview rendered, kept so the window can fetch its pixels as
    /// bytes rather than as a JSON array of numbers.
    preview: std::sync::Mutex<Vec<u8>>,
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
fn cancel(state: State<'_, Running>) {
    state.cancel.store(true, Ordering::Relaxed);
}

/// Reads the run and measures every light: stars, width, trailing.
///
/// This is the slow one — about 140 seconds for 226 frames of 19 megapixels on
/// the reference machine — so it reports as it goes and can be stopped. A
/// stopped pass returns what it measured rather than nothing, and says that it
/// was stopped.
#[tauri::command]
async fn measure_quality(
    roots: Roots,
    sigma: f32,
    max_stars: usize,
    raw: bool,
    on: Channel<Progress>,
    state: State<'_, Running>,
) -> Result<QualityDto, String> {
    let rules = roots.rules();
    if rules.is_empty() {
        return Err("nothing to measure".to_owned());
    }
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);

    let host = host()?;
    let options = ScanOptions { rules, ..Default::default() };
    let report = astro_core::session::scan(&host, &options).map_err(|error| format!("{error:#}"))?;
    let partition = report.session.partition(&Tolerances::default());
    let session = &report.session;

    // The deepest plan, from the core's own ranking, for the same reason the
    // frames step shows it first: set order is arbitrary.
    let ranked = partition.plans_by_depth(session);
    let Some(&(plan, _)) = ranked.first() else {
        return Err("no stackable set of lights was formed, so there is nothing to measure"
            .to_owned());
    };
    let Some(lights) = partition.set(plan.lights) else {
        return Err("the deepest plan names a set that is not in the partition".to_owned());
    };

    let stopped = || cancel.load(Ordering::Relaxed);
    let masters = if raw {
        astro_core::pipeline::MasterSet::default()
    } else {
        let report = |step: Step<'_>| {
            if let Step::MasterReading { kind, done, total, .. } = step {
                let _ = on.send(Progress::Master { kind: kind.name().to_owned(), done, total });
            }
            if stopped() { Flow::Stop } else { Flow::Continue }
        };
        match astro_core::pipeline::masters(
            &host,
            session,
            &partition,
            plan,
            &Default::default(),
            astro_core::pipeline::Wanted::Applied,
            &report,
        ) {
            Ok(masters) => masters,
            Err(astro_core::Error::Cancelled) => return Err("cancelled".to_owned()),
            Err(error) => return Err(format!("{error:#}")),
        }
    };

    let ids: Vec<FrameId> =
        lights.members.iter().copied().filter(|id| session[*id].is_active()).collect();
    let detect = DetectOptions { detect_sigma: sigma, max_stars, ..Default::default() };
    let survey = astro_core::pipeline::survey(&host, session, &ids, &masters, &detect, &|step| {
        if let Step::FrameRead { done, total, name } = step {
            let _ = on.send(Progress::Frame { done, total, name: name.to_owned() });
        }
        if stopped() { Flow::Stop } else { Flow::Continue }
    });

    Ok(quality(&survey, max_stars))
}

fn quality(survey: &astro_core::pipeline::Survey, star_cap: usize) -> QualityDto {
    let frames: Vec<FrameQualityDto> = survey
        .frames
        .iter()
        .map(|measured| {
            let shape = &measured.detection.shape;
            FrameQualityDto {
                name: measured.name.clone(),
                stars: measured.detection.stars.len(),
                fwhm: shape.moments.minor_fwhm(),
                trail: shape.moments.trail(),
                angle: shape.moments.angle_degrees(),
                agreement: shape.direction_agreement,
                sky: f64::from(shape.sky),
                noise: f64::from(shape.noise),
                saturated: measured.detection.saturated,
                oversized: measured.detection.oversized,
                seconds: measured.seconds,
            }
        })
        .collect();

    // Spin-2 across the run, weighted by how trailed each frame is: a round
    // frame has no direction to contribute and must not dilute the answer
    // toward zero.
    let (mut cos, mut sin, mut weight) = (0f64, 0f64, 0f64);
    for measured in &survey.frames {
        let moments = measured.detection.shape.moments;
        if let Some((c, s)) = moments.orientation() {
            let trail = moments.trail();
            if trail.is_finite() && trail > 0.0 {
                cos += c * trail;
                sin += s * trail;
                weight += trail;
            }
        }
    }
    let (direction, direction_agreement) = if weight > 0.0 {
        let degrees = sin.atan2(cos).to_degrees() / 2.0;
        (
            if degrees < 0.0 { degrees + 180.0 } else { degrees },
            (cos * cos + sin * sin).sqrt() / weight,
        )
    } else {
        (f64::NAN, f64::NAN)
    };

    QualityDto {
        frames,
        failed: survey
            .failed
            .iter()
            .map(|(name, reason)| RejectedDto { name: name.clone(), reason: reason.clone() })
            .collect(),
        seconds: survey.seconds,
        stopped: survey.stopped,
        direction,
        direction_agreement,
        star_cap,
    }
}

/// The pixels of the last preview, as raw RGBA.
///
/// A separate command returning bytes rather than a field on the result: the
/// same image as a JSON array of numbers is an order of magnitude larger and
/// has to be parsed a byte at a time.
#[tauri::command]
fn stack_preview(state: State<'_, Running>) -> tauri::ipc::Response {
    let bytes = state.preview.lock().map(|p| p.clone()).unwrap_or_default();
    tauri::ipc::Response::new(bytes)
}

/// Reads the run, aligns it, and combines it into one image.
///
/// The long one: four passes over every light, two of them decoding. It reports
/// which stage it is in rather than one bar across all four, and can be stopped
/// at any of them.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn stack_run(
    roots: Roots,
    sigma: f32,
    max_stars: usize,
    raw: bool,
    sharpness: f64,
    max_trail: Option<f64>,
    max_fwhm: Option<f64>,
    max_shift: Option<f64>,
    pixfrac: f64,
    reject: bool,
    kappa: f32,
    out: String,
    on: Channel<Progress>,
    state: State<'_, Running>,
) -> Result<StackResultDto, String> {
    let rules = roots.rules();
    if rules.is_empty() {
        return Err("nothing to stack".to_owned());
    }
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let stopped = || cancel.load(Ordering::Relaxed);

    let host = host()?;
    let options = ScanOptions { rules, ..Default::default() };
    let report = astro_core::session::scan(&host, &options).map_err(|e| format!("{e:#}"))?;
    let partition = report.session.partition(&Tolerances::default());
    let session = &report.session;

    let ranked = partition.plans_by_depth(session);
    let Some(&(plan, _)) = ranked.first() else {
        return Err("no stackable set of lights was formed".to_owned());
    };
    let Some(lights) = partition.set(plan.lights) else {
        return Err("the deepest plan names a set that is not in the partition".to_owned());
    };

    let masters = if raw {
        astro_core::pipeline::MasterSet::default()
    } else {
        let report = |step: Step<'_>| {
            if let Step::MasterReading { kind, done, total, .. } = step {
                let _ = on.send(Progress::Master { kind: kind.name().to_owned(), done, total });
            }
            if stopped() { Flow::Stop } else { Flow::Continue }
        };
        match astro_core::pipeline::masters(
            &host,
            session,
            &partition,
            plan,
            &Default::default(),
            astro_core::pipeline::Wanted::Applied,
            &report,
        ) {
            Ok(masters) => masters,
            Err(astro_core::Error::Cancelled) => return Err("cancelled".to_owned()),
            Err(error) => return Err(format!("{error:#}")),
        }
    };

    let ids: Vec<FrameId> =
        lights.members.iter().copied().filter(|id| session[*id].is_active()).collect();
    let detect = DetectOptions { detect_sigma: sigma, max_stars, ..Default::default() };
    let survey = astro_core::pipeline::survey(&host, session, &ids, &masters, &detect, &|step| {
        if let Step::FrameRead { done, total, name } = step {
            let _ = on.send(Progress::Frame { done, total, name: name.to_owned() });
        }
        if stopped() { Flow::Stop } else { Flow::Continue }
    });
    if survey.stopped {
        return Err("cancelled".to_owned());
    }

    let _ = on.send(Progress::Aligning);
    let alignment =
        astro_core::pipeline::align::align(&survey.frames, None, &Default::default())
            .ok_or("the run holds too few lights to align")?;

    let stack_options = StackOptions {
        sharpness,
        max_trail,
        max_fwhm,
        max_shift,
        pixfrac,
        rejection: reject.then(|| Rejection { kappa, ..Default::default() }),
    };
    let selection = select(&alignment, &stack_options);
    if selection.frames.is_empty() {
        return Err("every frame was refused, so there is nothing to stack".to_owned());
    }

    let stacked = match combine(&host, &selection, &masters, &stack_options, &|step| {
        if let Step::Stacking { pass, passes, done, total, name } = step {
            let _ = on.send(Progress::Stacking {
                pass,
                passes,
                done,
                total,
                name: name.to_owned(),
            });
        }
        if stopped() { Flow::Stop } else { Flow::Continue }
    }) {
        Ok(stacked) => stacked,
        Err(astro_core::Error::Cancelled) => return Err("cancelled".to_owned()),
        Err(error) => return Err(format!("{error:#}")),
    };

    let _ = on.send(Progress::Writing);
    let first = selection.frames.first().ok_or("nothing was stacked")?;
    let mosaic = Mosaic::new(&first.read.layout).ok_or("the frames are not a mosaic")?;

    let written = write(&stacked, &selection, &mosaic, &out).map_err(|e| format!("{e:#}"))?;

    // The preview is rendered from the levelled copy, the same one the stretched
    // TIFF beside it comes from, so the window and the file agree.
    let levelled = view::for_viewing(&stacked, &first.read.layout, &mosaic, true);
    let rendered = view::preview(&levelled, &stacked, 1400, 200.0);
    if let Ok(mut slot) = state.preview.lock() {
        *slot = rendered.rgba;
    }

    let mut weights: Vec<f64> =
        selection.frames.iter().map(|f| f.weight).filter(|w| w.is_finite()).collect();
    weights.sort_by(f64::total_cmp);
    let total: f64 = weights.iter().sum();
    let squares: f64 = weights.iter().map(|w| w * w).sum();

    Ok(StackResultDto {
        frames: selection
            .frames
            .iter()
            .map(|frame| StackedFrameDto {
                name: frame.read.name.clone(),
                weight: frame.weight,
                brightness: frame.scale.unwrap_or(f64::NAN),
                trail: frame.read.detection.shape.moments.trail(),
                fwhm: frame.read.detection.shape.moments.minor_fwhm(),
                shift: frame.shift,
            })
            .collect(),
        refused: selection
            .refused
            .iter()
            .map(|(name, reason)| RejectedDto { name: name.clone(), reason: reason.clone() })
            .collect(),
        effective: if squares > 0.0 { (total * total / squares).round() as usize } else { 0 },
        stacked: stacked.frames,
        width: stacked.canvas.width,
        height: stacked.canvas.height,
        seconds: stacked.seconds,
        coverage: stacked
            .coverage
            .iter()
            .map(|plane| {
                let mut depths: Vec<f32> =
                    plane.iter().copied().filter(|w| *w > 0.0).collect();
                if depths.is_empty() {
                    return CoverageDto { filled: 0.0, median_depth: 0.0, thinnest: 0.0 };
                }
                depths.sort_by(f32::total_cmp);
                let at = |f: f64| f64::from(depths[((depths.len() - 1) as f64 * f) as usize]);
                CoverageDto {
                    filled: depths.len() as f64 / plane.len() as f64 * 100.0,
                    median_depth: at(0.5),
                    thinnest: at(0.1),
                }
            })
            .collect(),
        written,
        rejected: stacked.rejected,
        heavy_losses: stacked.heavy_losses.clone(),
        preview_width: rendered.width,
        preview_height: rendered.height,
        note: levelled.note,
    })
}

/// Writes the FITS and both TIFFs, exactly as the command line does.
fn write(
    stacked: &astro_core::pipeline::stack::Stacked,
    selection: &astro_core::pipeline::stack::Selection<'_>,
    mosaic: &Mosaic,
    out: &str,
) -> std::io::Result<Vec<String>> {
    let dir = PathBuf::from(out);
    std::fs::create_dir_all(&dir)?;
    let first = &selection.frames[0];
    let (width, height) = (stacked.canvas.width, stacked.canvas.height);

    let header = fits::Header {
        image_type: FrameKind::Light.name().to_owned(),
        instrument: first.read.camera_model.clone(),
        exposure: first.read.exposure_seconds,
        iso: first.read.iso,
        frames: stacked.frames,
        combination: "weighted mean".to_owned(),
        bayer_pattern: None,
        notes: vec!["NaN where no frame covered the pixel".to_owned()],
    };
    let borrowed: Vec<&[f32]> = stacked.planes.iter().map(|p| p.as_slice()).collect();
    let mut written = Vec::new();

    let target = dir.join("stack.fits");
    fits::write_planes(&target, &borrowed, width, height, &header)?;
    written.push(target.display().to_string());

    let linear = view::for_viewing(stacked, &first.read.layout, mosaic, false);
    let planes: Vec<&[f32]> = linear.planes.iter().map(|p| p.as_slice()).collect();
    let target = dir.join("stack.tif");
    tiff::write(
        &target,
        &tiff::Image { width, height, planes: &planes[..mosaic.colours.min(3)] },
        &tiff::Mapping::Linear { full: linear.full },
        &linear.note,
    )?;
    written.push(target.display().to_string());

    let levelled = view::for_viewing(stacked, &first.read.layout, mosaic, true);
    let planes: Vec<&[f32]> = levelled.planes.iter().map(|p| p.as_slice()).collect();
    let floor = view::percentile(&levelled.planes[mosaic.dominant], 0.02).min(levelled.common);
    let target = dir.join("stack_view.tif");
    tiff::write(
        &target,
        &tiff::Image { width, height, planes: &planes[..mosaic.colours.min(3)] },
        &tiff::Mapping::Asinh { black: floor, white: levelled.full, softening: 200.0 },
        &levelled.note,
    )?;
    written.push(target.display().to_string());

    Ok(written)
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
        .manage(Running::default())
        .invoke_handler(tauri::generate_handler![
            scan_session,
            measure_quality,
            stack_run,
            stack_preview,
            cancel,
            app_version,
            formats
        ])
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
    fn measuring_a_real_run_reports_what_the_command_line_reports() {
        // The window and the command line must not disagree about the same
        // night. These are the numbers `stars` prints for this session, and if
        // the bridge ever computes its own the two would drift apart in a way
        // only the owner would notice, and only after acting on one of them.
        let Some(root) = testdata() else { return };
        let host = host().expect("a plugin loads");
        let options = ScanOptions {
            rules: vec![
                RoleRule::new(root.join("lights"), Some(FrameKind::Light)),
                RoleRule::new(root.join("darks"), Some(FrameKind::Dark)),
                RoleRule::new(root.join("flats56_iso100-1"), Some(FrameKind::Flat)),
                RoleRule::new(root.join("bias"), Some(FrameKind::Bias)),
            ],
            ..Default::default()
        };
        let report = astro_core::session::scan(&host, &options).expect("the session scans");
        let partition = report.session.partition(&Tolerances::default());
        let session = &report.session;

        let (plan, _) = partition.plans_by_depth(session)[0];
        let lights = partition.set(plan.lights).expect("the deepest plan has its set");
        // Six frames, not the whole night: the point is that the numbers match,
        // and two and a half minutes of decoding would not make them match
        // harder.
        let ids: Vec<FrameId> = lights
            .members
            .iter()
            .copied()
            .filter(|id| session[*id].is_active())
            .take(6)
            .collect();

        let masters = astro_core::pipeline::masters(
            &host,
            session,
            &partition,
            plan,
            &Default::default(),
            astro_core::pipeline::Wanted::Applied,
            &|_| Flow::Continue,
        )
        .expect("the masters build");
        let survey = astro_core::pipeline::survey(
            &host,
            session,
            &ids,
            &masters,
            &DetectOptions::default(),
            &|_| Flow::Continue,
        );

        let dto = quality(&survey, DetectOptions::default().max_stars);
        assert_eq!(dto.frames.len(), 6, "six frames measured");
        assert!(!dto.stopped);

        // The command line reports 2.07 px across the trail at the median and
        // 92 degrees of direction over these frames.
        let mut widths: Vec<f64> = dto.frames.iter().map(|f| f.fwhm).collect();
        widths.sort_by(f64::total_cmp);
        let median = widths[widths.len() / 2];
        assert!((median - 2.07).abs() < 0.2, "width across the trail: {median}");
        assert!(
            (dto.direction - 92.0).abs() < 6.0,
            "the run trails at 92 degrees, not {}",
            dto.direction
        );
        assert!(
            dto.direction_agreement > 0.8,
            "one direction in every frame: {}",
            dto.direction_agreement
        );
        for frame in &dto.frames {
            assert!(frame.stars > 0, "{} found no stars", frame.name);
            assert!(frame.trail.is_finite(), "{} has no trail", frame.name);
        }
    }

    #[test]
    fn a_stack_comes_out_the_size_of_its_canvas_and_its_preview_matches() {
        // The preview travels as loose bytes with its size in a separate field,
        // so nothing checks the two agree except this. A mismatch draws
        // diagonal garbage into the canvas, which looks like a decoding bug
        // rather than like an off-by-one here.
        let Some(root) = testdata() else { return };
        let host = host().expect("a plugin loads");
        let options = ScanOptions {
            rules: vec![
                RoleRule::new(root.join("lights"), Some(FrameKind::Light)),
                RoleRule::new(root.join("darks"), Some(FrameKind::Dark)),
                RoleRule::new(root.join("flats56_iso100-1"), Some(FrameKind::Flat)),
                RoleRule::new(root.join("bias"), Some(FrameKind::Bias)),
            ],
            ..Default::default()
        };
        let report = astro_core::session::scan(&host, &options).expect("the session scans");
        let partition = report.session.partition(&Tolerances::default());
        let session = &report.session;
        let (plan, _) = partition.plans_by_depth(session)[0];
        let lights = partition.set(plan.lights).expect("the plan has its set");

        let ids: Vec<FrameId> = lights
            .members
            .iter()
            .copied()
            .filter(|id| session[*id].is_active())
            .skip(100)
            .take(6)
            .collect();
        let masters = astro_core::pipeline::masters(
            &host,
            session,
            &partition,
            plan,
            &Default::default(),
            astro_core::pipeline::Wanted::Applied,
            &|_| Flow::Continue,
        )
        .expect("the masters build");
        let survey = astro_core::pipeline::survey(
            &host,
            session,
            &ids,
            &masters,
            &DetectOptions::default(),
            &|_| Flow::Continue,
        );

        let alignment =
            astro_core::pipeline::align::align(&survey.frames, None, &Default::default())
                .expect("six frames align");
        let options = StackOptions::default();
        let selection = select(&alignment, &options);
        let stacked = combine(&host, &selection, &masters, &options, &|_| Flow::Continue)
            .expect("they combine");

        assert_eq!(stacked.frames, 6);
        assert_eq!(stacked.planes.len(), stacked.colours);
        for plane in &stacked.planes {
            assert_eq!(plane.len(), stacked.canvas.width * stacked.canvas.height);
        }

        let first = &selection.frames[0];
        let mosaic = Mosaic::new(&first.read.layout).expect("a mosaic");
        let levelled = view::for_viewing(&stacked, &first.read.layout, &mosaic, true);
        let rendered = view::preview(&levelled, &stacked, 1400, 200.0);

        assert_eq!(
            rendered.rgba.len(),
            rendered.width * rendered.height * 4,
            "the byte count must match the size the window is told"
        );
        assert!(rendered.width <= 1400 && rendered.height <= 1400);
        // And it is a picture rather than a black rectangle: a preview whose
        // stretch collapsed would still be the right size.
        let bright = rendered.rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 24 || p[1] > 24).count();
        assert!(
            bright > rendered.width * rendered.height / 100,
            "the preview is nearly all black: {bright} lit of {}",
            rendered.width * rendered.height
        );
    }

    #[test]
    fn a_scan_of_nothing_says_so_rather_than_returning_an_empty_session() {
        // An empty session on screen is indistinguishable from a session whose
        // frames were all unreadable, and the user would go looking at their
        // files rather than at what they picked.
        assert!(scan_session(Roots::default()).is_err());
    }
}
