//! What a frame is, and — kept strictly apart from it — what the evidence
//! suggests it might be.
//!
//! One policy sentence, and the rest of this module follows from it: **an
//! assignment comes from the user; evidence may propose and may veto, but may
//! never elect.**
//!
//! The reason is an asymmetry, not a preference. A light, a flat and a bias each
//! have a positive signature. A dark has only a negative one — no sky, no stars,
//! no gradient — and those absences are equally consistent with a light shot
//! under thick cloud. A classifier that must label every frame is therefore
//! forced to guess exactly where a wrong guess does the most damage, and the
//! damage is silent: a light filed as a dark subtracts a star field from every
//! frame in the stack, a light filed as a flat divides every frame by a picture
//! of the sky, and neither throws.
//!
//! Nor is this a gap the ABI could close. A correctly shot dark has the *same*
//! exposure, ISO and body as its lights — that is what makes it a valid dark —
//! so those are exactly the fields that cannot separate them. FITS capture
//! software writes an `IMAGETYP` keyword; CR2 and CR3 have no equivalent.

use std::path::{Component, Path};

use crate::frame::FrameInfo;

/// The role a frame plays in calibration.
///
/// `DarkFlat` is not a fifth physical kind: it is a dark, and no camera records
/// the difference. It is here because the difference is one of *role* — a dark
/// flat calibrates the flats and a dark calibrates the lights — and that role
/// cannot be re-derived stably. Deriving it from "the dark whose exposure
/// matches the flats" would make the label flip as flats are added and removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FrameKind {
    Light,
    Dark,
    Flat,
    Bias,
    DarkFlat,
}

impl FrameKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Flat => "flat",
            Self::Bias => "bias",
            Self::DarkFlat => "dark flat",
        }
    }

    /// The command-line flag that assigns this kind, for a suggestion the user
    /// can paste.
    pub fn assign_flag(self) -> &'static str {
        match self {
            Self::Light => "--lights",
            Self::Dark => "--darks",
            Self::Flat => "--flats",
            Self::Bias => "--biases",
            Self::DarkFlat => "--dark-flats",
        }
    }

    pub fn is_calibration(self) -> bool {
        !matches!(self, Self::Light)
    }

    /// Which kind this one calibrates. Note that a dark flat calibrates the
    /// flats, never the lights — getting this backwards subtracts a 1/60 s dark
    /// from a 300 s light and removes almost nothing.
    pub fn calibrates(self) -> Option<FrameKind> {
        match self {
            Self::Light => None,
            Self::Dark | Self::Bias | Self::Flat => Some(Self::Light),
            Self::DarkFlat => Some(Self::Flat),
        }
    }

    /// Every kind, in the order a report should list them.
    pub const ALL: [FrameKind; 5] =
        [Self::Light, Self::Dark, Self::Flat, Self::Bias, Self::DarkFlat];
}

/// What a frame is, and separately what it looks like it might be.
///
/// The two are never collapsed into one field. A calibration frame that is only
/// inferred must not calibrate anything without the user saying so.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameRole {
    Assigned(FrameKind),
    Unassigned { inference: Inference },
}

impl FrameRole {
    pub fn unassigned() -> Self {
        Self::Unassigned { inference: Inference::NoEvidence }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inference {
    Proposed { kind: FrameKind, basis: Basis },
    /// Two readings disagreed. Both are kept rather than one being picked: a
    /// file at `.../Darks/Flat_0001.CR3` is a question, not an answer.
    Ambiguous(Vec<(FrameKind, Basis)>),
    /// Nothing in the path or the metadata says. A camera on an intervalometer
    /// produces `IMG_0001.CR3`, and this is the usual outcome for it.
    NoEvidence,
}

impl Inference {
    /// The single kind this evidence points at, if it points at one.
    pub fn proposed(&self) -> Option<FrameKind> {
        match self {
            Self::Proposed { kind, .. } => Some(*kind),
            Self::Ambiguous(_) | Self::NoEvidence => None,
        }
    }
}

/// The evidence behind a proposal, named rather than scored.
///
/// A confidence of 0.7 is not something a user can act on; "the folder was
/// called FLAT" is.
#[derive(Debug, Clone, PartialEq)]
pub enum Basis {
    DirectoryComponent { component: String, token: String },
    FilenameToken { token: String },
    /// The exposure is short enough that little but a bias is ever shot there.
    MinimumShutter { seconds: f64 },
    /// The user named a whole directory when adding it.
    UserRule,
}

impl std::fmt::Display for Basis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectoryComponent { component, .. } => write!(f, "the folder {component}"),
            Self::FilenameToken { token } => write!(f, "{token} in the file name"),
            Self::MinimumShutter { seconds } => write!(f, "a {seconds:.5} s exposure"),
            Self::UserRule => write!(f, "you said so"),
        }
    }
}

/// Below this, a frame is more likely a bias than anything else.
///
/// Weak evidence, and it says so: a flat through a fast astrograph onto a bright
/// panel lands at 1/2000 s or shorter, which is the same place. It proposes
/// only, and the partition separately checks that a frame *assigned* Bias really
/// is the shortest exposure in its session.
pub const BIAS_EXPOSURE_CEILING_SECONDS: f64 = 1.0 / 2000.0;

/// Reads a frame kind out of a path.
///
/// Directory components are tested from the leaf towards the root and the first
/// match wins, because `.../WO-GT81-Flat61/Lights/IMG_0001.CR3` names an optic
/// in one component and the frame kind in another, and only the nearer one is
/// about this file. A directory and a filename that disagree yield
/// [`Inference::Ambiguous`] rather than an arbitrary winner.
pub fn infer_from_path(path: &Path) -> Inference {
    let from_directory = directory_proposal(path);
    let from_filename = filename_proposal(path);

    match (from_directory, from_filename) {
        (Some(dir), Some(file)) if dir.0 != file.0 => Inference::Ambiguous(vec![dir, file]),
        // The folder wins outright when they agree, and when only it matched:
        // a folder is a deliberate act, a file name is often the camera's.
        (Some((kind, basis)), _) => Inference::Proposed { kind, basis },
        (None, Some((kind, basis))) => Inference::Proposed { kind, basis },
        (None, None) => Inference::NoEvidence,
    }
}

/// Reads what the shooting parameters alone can prove, which is one thing.
pub fn infer_from_metadata(info: &FrameInfo) -> Inference {
    match info.exposure_seconds {
        Some(seconds) if seconds > 0.0 && seconds <= BIAS_EXPOSURE_CEILING_SECONDS => {
            Inference::Proposed { kind: FrameKind::Bias, basis: Basis::MinimumShutter { seconds } }
        }
        _ => Inference::NoEvidence,
    }
}

/// Path evidence first, then metadata. Metadata is weaker and only speaks when
/// the path said nothing at all.
pub fn infer(path: &Path, info: &FrameInfo) -> Inference {
    match infer_from_path(path) {
        Inference::NoEvidence => infer_from_metadata(info),
        found => found,
    }
}

fn directory_proposal(path: &Path) -> Option<(FrameKind, Basis)> {
    let parent = path.parent()?;
    for component in parent.components().rev() {
        let Component::Normal(text) = component else { continue };
        let Some(text) = text.to_str() else { continue };
        let tokens = tokenise(text);
        if let Some((kind, token)) = kind_from_tokens(&tokens) {
            let basis = Basis::DirectoryComponent { component: text.to_owned(), token };
            return Some((kind, basis));
        }
    }
    None
}

fn filename_proposal(path: &Path) -> Option<(FrameKind, Basis)> {
    let stem = path.file_stem()?.to_str()?;
    let tokens = tokenise(stem);
    let (kind, token) = kind_from_tokens(&tokens)?;
    Some((kind, Basis::FilenameToken { token }))
}

/// Splits into whole alphanumeric words, case-folded.
///
/// Whole words matter: a mount or optic named `Flat61` or `GT81` must not read
/// as a frame kind, and it does not, because `flat61` is one token and is not
/// `flat`.
fn tokenise(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn kind_from_tokens(tokens: &[String]) -> Option<(FrameKind, String)> {
    // Two-word forms first, or `dark flat` reads as `dark` and every dark flat
    // gets matched against the lights.
    for pair in tokens.windows(2) {
        if let Some(kind) = kind_from_pair(&pair[0], &pair[1]) {
            return Some((kind, format!("{} {}", pair[0], pair[1])));
        }
    }
    tokens.iter().find_map(|token| kind_from_token(token).map(|kind| (kind, token.clone())))
}

fn kind_from_pair(first: &str, second: &str) -> Option<FrameKind> {
    let dark = matches!(first, "dark" | "darks");
    let flat = matches!(second, "flat" | "flats");
    if dark && flat {
        return Some(FrameKind::DarkFlat);
    }
    if matches!(first, "flat" | "flats") && matches!(second, "dark" | "darks") {
        return Some(FrameKind::DarkFlat);
    }
    None
}

fn kind_from_token(token: &str) -> Option<FrameKind> {
    match token {
        // Before `dark` and `flat`, though as distinct whole tokens the order
        // does not actually matter — it is kept for the reader.
        "darkflat" | "darkflats" | "flatdark" | "flatdarks" => Some(FrameKind::DarkFlat),
        "light" | "lights" => Some(FrameKind::Light),
        "dark" | "darks" => Some(FrameKind::Dark),
        "flat" | "flats" => Some(FrameKind::Flat),
        // `offset` is what Siril and several European tools call a bias.
        "bias" | "biases" | "offset" | "offsets" => Some(FrameKind::Bias),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposed(path: &str) -> Option<FrameKind> {
        infer_from_path(Path::new(path)).proposed()
    }

    #[test]
    fn a_directory_named_flat61_does_not_make_a_flat() {
        // A real rig name: a William Optics GT81 with a field flattener. The
        // frame kind is the folder nearer the file, and `Flat61` is one token.
        assert_eq!(proposed("D:/astro/WO-GT81-Flat61/Lights/IMG_0001.CR3"), Some(FrameKind::Light));
        assert_eq!(proposed("D:/astro/WO-GT81-Flat61/IMG_0001.CR3"), None);
    }

    #[test]
    fn the_folder_nearest_the_file_wins() {
        // A darks folder inside a session folder called Lights.
        assert_eq!(proposed("D:/astro/M31/Lights/Darks/IMG_0001.CR3"), Some(FrameKind::Dark));
    }

    #[test]
    fn darkflat_is_read_before_dark() {
        // Getting this wrong files every dark flat against the lights, where it
        // is compared with a 300 s exposure and warned about rather than used.
        for path in [
            "D:/astro/DarkFlats/x.CR3",
            "D:/astro/dark flats/x.CR3",
            "D:/astro/dark_flat/x.CR3",
            "D:/astro/flatdarks/x.CR3",
            "D:/astro/flat-darks/x.CR3",
        ] {
            assert_eq!(proposed(path), Some(FrameKind::DarkFlat), "{path}");
        }
    }

    #[test]
    fn a_filename_and_a_folder_that_disagree_leave_the_frame_unassigned() {
        let inference = infer_from_path(Path::new("D:/astro/Darks/Flat_0001.CR3"));
        let Inference::Ambiguous(readings) = &inference else {
            panic!("expected an ambiguous reading, got {inference:?}");
        };
        assert_eq!(readings.len(), 2);
        assert!(readings.iter().any(|(kind, _)| *kind == FrameKind::Dark));
        assert!(readings.iter().any(|(kind, _)| *kind == FrameKind::Flat));
        // And crucially: it proposes nothing, so nothing is assigned.
        assert_eq!(inference.proposed(), None);
    }

    #[test]
    fn the_common_case_is_no_evidence_at_all() {
        // A camera on an intervalometer, everything in one folder. Honest
        // silence beats a guess.
        assert_eq!(infer_from_path(Path::new("D:/astro/M31/IMG_0001.CR3")), Inference::NoEvidence);
    }

    #[test]
    fn capture_software_conventions_are_recognised() {
        // The directory layouts N.I.N.A., Ekos, SGP and Siril produce.
        assert_eq!(proposed("D:/N.I.N.A./M31/LIGHT/2026-08-27_M31.CR3"), Some(FrameKind::Light));
        assert_eq!(proposed("D:/Ekos/M31/Dark/Dark_300_secs_001.cr3"), Some(FrameKind::Dark));
        assert_eq!(proposed("D:/siril/biases/bias_00001.cr2"), Some(FrameKind::Bias));
        assert_eq!(proposed("D:/tools/OFFSET/o_0001.cr2"), Some(FrameKind::Bias));
    }

    #[test]
    fn a_short_exposure_only_ever_proposes_a_bias() {
        let mut info = FrameInfo { exposure_seconds: Some(1.0 / 4000.0), ..Default::default() };
        assert_eq!(infer_from_metadata(&info).proposed(), Some(FrameKind::Bias));

        // A flat, and a light. Neither is guessed at.
        info.exposure_seconds = Some(1.0 / 60.0);
        assert_eq!(infer_from_metadata(&info), Inference::NoEvidence);
        info.exposure_seconds = Some(300.0);
        assert_eq!(infer_from_metadata(&info), Inference::NoEvidence);
        info.exposure_seconds = None;
        assert_eq!(infer_from_metadata(&info), Inference::NoEvidence);
    }

    #[test]
    fn path_evidence_outranks_a_short_exposure() {
        // A flat through a fast astrograph is as short as a bias. The folder
        // knows better than the shutter speed does.
        let info = FrameInfo { exposure_seconds: Some(1.0 / 4000.0), ..Default::default() };
        let inference = infer(Path::new("D:/astro/Flats/IMG_0001.CR3"), &info);
        assert_eq!(inference.proposed(), Some(FrameKind::Flat));
    }

    #[test]
    fn a_dark_flat_calibrates_the_flats_and_not_the_lights() {
        assert_eq!(FrameKind::DarkFlat.calibrates(), Some(FrameKind::Flat));
        assert_eq!(FrameKind::Dark.calibrates(), Some(FrameKind::Light));
        assert_eq!(FrameKind::Light.calibrates(), None);
        assert!(!FrameKind::Light.is_calibration());
        assert!(FrameKind::DarkFlat.is_calibration());
    }
}
