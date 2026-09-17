//! What was shot: the frames of an imaging session, what each one is, and which
//! of them can be stacked together.
//!
//! The decision worth defending here is that a frame is named by a dense
//! [`FrameId`] rather than by its path or by an `Arc`. Registration will want
//! one transform per frame, star extraction a list of stars per frame, and
//! integration a weight per frame. All three arrive as side tables indexed by
//! the same integer, so none of them has to reshape [`FrameRecord`], and none of
//! them puts a megabyte of star positions inside a record that a five-hundred
//! frame session holds all of at once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::frame::ImageLayout;
use crate::frame::FrameInfo;

pub mod compat;
pub mod kind;
pub mod mosaic;
pub mod scan;
pub mod sets;
pub mod stats;

pub use compat::{
    GeometryKey, Incompatibility, Mismatch, Pairing, Property, Rect, Severity, Tolerances,
};
pub use kind::{Basis, FrameKind, FrameRole, Inference};
pub use mosaic::{Area, Mosaic, active_area};
pub use scan::{Progress, RoleRule, ScanOptions, ScanReport, scan, scan_with_progress};
pub use stats::{ColourStats, FrameStats, Uniformity, illumination_map, measure};
pub use sets::{
    BlockedRole, CalibrationMatch, ExposureBucket, ExposureSpan, FrameSet, GainBucket, MatchQuality,
    Partition, PartitionKey, SetId, SetKey, Split, StackPlan, Suspicion, best_match,
};

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Handle to one frame within one [`Session`].
///
/// Dense and never reused. That is what lets registration transforms, star
/// lists and quality scores live in side tables indexed by this integer instead
/// of as fields on the frame record: a table can be added, spilled to disk or
/// thrown away without any other type in this module knowing it exists.
/// Excluding a frame leaves its id in place — removing it would renumber every
/// side table written against the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameId(u32);

impl FrameId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A user-declared subdivision of a session — DeepSkyStacker calls it a file
/// group.
///
/// It exists because the properties that invalidate a flat are not recorded
/// anywhere: focus position, camera rotation, a filter swapped, a dew shield
/// nudged. Only the user can say that Tuesday's flats do not belong to Monday's
/// lights. [`GroupId::MAIN`] is the exception — calibration in it is offered to
/// every group, which is how a bias library shot once a year gets reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupId(u16);

impl GroupId {
    pub const MAIN: GroupId = GroupId(0);
}

/// The name of the group whose calibration serves every other group.
///
/// Reserved: [`Session::intern_group`] hands `GroupId::MAIN` to anyone who asks
/// for it by name, so a user group called `main` would silently become the
/// global one. Callers that take a group name from a person must refuse this
/// one rather than let the isolation the user asked for be dropped.
pub const MAIN_GROUP: &str = "main";

/// Whether frames in `target` may draw calibration from `source`.
///
/// By name rather than by [`GroupId`], because ids are assigned in the order
/// groups were first seen. A key carrying one would rebind to a different night
/// when the same session is described with the flags in another order.
pub fn may_draw_from(target: &str, source: &str) -> bool {
    source == MAIN_GROUP || source == target
}

/// Index into the session's directory table. Never exposed as a path directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DirId(u32);

/// Index into the session's plugin table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginId(u16);

/// Enough of a file's identity to notice that it changed, without reading it
/// twice.
///
/// Length and modification time catch an edited or replaced file. The hash of
/// the leading bytes — the ones the host already reads to probe a file — catches
/// a file rewritten at the same length and timestamp, and recognises the same
/// frame after it has been moved or renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameFingerprint {
    pub len: u64,
    /// `None` when the filesystem did not report one, never a stand-in value.
    pub modified_unix: Option<i64>,
    pub header_hash: u64,
}

impl FrameFingerprint {
    pub fn new(len: u64, modified_unix: Option<i64>, header: &[u8]) -> Self {
        Self { len, modified_unix, header_hash: fnv1a(header) }
    }

    /// Whether two fingerprints name the same bytes, ignoring the timestamp.
    ///
    /// A file copied to another drive keeps its length and its contents and
    /// loses its modification time, and it is still the same frame.
    pub fn same_content(self, other: Self) -> bool {
        self.len == other.len && self.header_hash == other.header_hash
    }

    fn content(self) -> (u64, u64) {
        (self.len, self.header_hash)
    }
}

/// FNV-1a rather than the standard library's hasher, which is seeded per process
/// and explicitly not stable between Rust releases. A fingerprint is meant to
/// survive into a saved session, so it has to mean the same thing next year.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x1000_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME))
}

/// Where a frame came from, and enough to recognise it again.
///
/// The directory and the plugin are ids into tables on the [`Session`] rather
/// than strings on every record: five hundred frames out of one folder read by
/// one plugin would otherwise carry five hundred copies of both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSource {
    pub directory: DirId,
    pub file_name: Box<str>,
    pub plugin: PluginId,
    pub fingerprint: FrameFingerprint,
}

/// Which physical camera shot a frame.
///
/// `serial` is `None` for every frame today: the frozen ABI carries make and
/// model but not the body serial number, so two Canon bodies of one model are
/// indistinguishable. Dark current and the hot-pixel map are properties of the
/// individual sensor, so this is the one place the model is knowingly weaker
/// than the optics. The field exists anyway, so that adding a serial to the ABI
/// later changes how this key is built and changes nothing that consumes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BodyKey {
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
}

impl BodyKey {
    pub fn from_info(info: &FrameInfo) -> Self {
        Self { make: info.camera_make.clone(), model: info.camera_model.clone(), serial: None }
    }

    /// How the body reads in a report: `Canon Canon EOS R5` is what the file
    /// says and not what anyone wants to read.
    pub fn display(&self) -> String {
        let make = self.make.trim();
        let model = self.model.trim();
        if model.is_empty() {
            return make.to_owned();
        }
        // Canon writes the make into the model as well; other vendors do not.
        if make.is_empty() || model.to_ascii_lowercase().starts_with(&make.to_ascii_lowercase()) {
            model.to_owned()
        } else {
            format!("{make} {model}")
        }
    }
}

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

/// Everything known about one frame without decoding it.
///
/// The rule for what belongs here: a field goes on the record when it is small,
/// always present, and comes either from the file or from the user saying so.
/// Anything derived, expensive or usually absent — statistics, star lists,
/// transforms, weights — goes in a side table keyed by [`FrameId`], so that
/// adding one costs nothing to a session that does not use it.
///
/// Not `PartialEq`, because [`ImageLayout`] is not: its unrecorded fields are
/// NaN, and NaN is not equal to itself.
#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub source: FrameSource,
    /// Copied out of the `OpenFrame`, which is then closed. Holding open frames
    /// would pin one memory map and one decoder per frame.
    pub layout: ImageLayout,
    pub info: FrameInfo,
    pub body: BodyKey,
    pub role: FrameRole,
    pub group: GroupId,
    /// Excluded frames stay in the session so the user can see why and put them
    /// back. Removing one would renumber every [`FrameId`] after it.
    pub exclusion: Option<Exclusion>,
}

impl FrameRecord {
    /// The kind this frame will actually be treated as, or `None` when nothing
    /// has said. An inferred kind is deliberately not returned here: a proposal
    /// is something to show the user, not something to calibrate with.
    pub fn kind(&self) -> Option<FrameKind> {
        match self.role {
            FrameRole::Assigned(kind) => Some(kind),
            FrameRole::Unassigned { .. } => None,
        }
    }

    /// What the evidence proposed, for a frame nobody has assigned a kind to.
    /// `None` once a kind is assigned: at that point the proposal is history.
    pub fn inference(&self) -> Option<&Inference> {
        match &self.role {
            FrameRole::Unassigned { inference } => Some(inference),
            FrameRole::Assigned(_) => None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.exclusion.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Exclusion {
    pub reason: Rejection,
    pub by: ExclusionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionSource {
    /// The scan or the partition decided; re-running either may change it.
    Automatic,
    /// The user decided; nothing may override it.
    User,
}

/// Why a file that was offered to the scan is not in any set.
///
/// Deliberately not an [`Error`][crate::Error] variant. `Error` holds
/// `io::Error` and `libloading::Error` and can therefore never be `Clone`, while
/// a five-hundred frame report has to be sorted, counted by cause and
/// re-rendered. The division of labour is: an `Error` aborts the scan, a
/// `Rejection` is one file's outcome within a scan that succeeded.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejection {
    /// No loaded plugin recognised the file: a sidecar, a JPEG, a note.
    NotAFrame,
    /// A plugin claimed the file and then failed on it. `detail` is the
    /// plugin's own words, kept because it is the only explanation there is.
    Unreadable { detail: String },
    /// The same bytes are already in the session under another path.
    Duplicate { of: FrameId },
    /// Geometry that cannot be reconciled with the set the frame would join.
    Incompatible { with: SetId, reason: Incompatibility },
    /// Nothing said what this frame is, and nothing could be inferred.
    Unclassified,
    /// The user named this frame and asked for it to be left out.
    Excluded,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAFrame => write!(f, "not a frame any loaded plugin reads"),
            Self::Unreadable { detail } => write!(f, "unreadable: {detail}"),
            Self::Duplicate { of } => write!(f, "the same file is already in the session as frame {}", of.index()),
            Self::Incompatible { reason, .. } => write!(f, "{reason}"),
            Self::Unclassified => write!(f, "nothing says whether this is a light, dark, flat or bias"),
            Self::Excluded => write!(f, "you asked for this one to be left out"),
        }
    }
}

/// Something the user could do about what the scan found.
///
/// Typed rather than prose so that it survives into a report, into `--format
/// json`, and eventually into a button in a user interface, from one structure.
/// Strictness is only survivable when the fix is one paste.
#[derive(Debug, Clone, PartialEq)]
pub enum Suggestion {
    /// Unassigned frames that all sit under one directory.
    AssignDirectory { directory: PathBuf, frames: usize, proposed: Option<FrameKind> },
    /// Several calibration sets of one role could serve the same lights, and
    /// only one was chosen. Almost always flats shot on more than one night.
    SeparateGroups { kind: FrameKind, candidates: usize },
}

impl Suggestion {
    /// The command line that would act on this suggestion, where one exists.
    ///
    /// `None` when nothing is known about the frames. Filling in `--lights`
    /// there would be a guess wearing the clothes of a suggestion, and the one
    /// place the tool refuses to guess is exactly the place a paste-able line is
    /// most tempting.
    pub fn command_fragment(&self) -> Option<String> {
        match self {
            Self::AssignDirectory { directory, proposed, .. } => {
                let flag = proposed.map(FrameKind::assign_flag)?;
                Some(format!("{flag} \"{}\"", directory.display()))
            }
            // There is no single right answer here; the user has to say which
            // night is which, and the shape of the answer depends on their tree.
            Self::SeparateGroups { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The arena
// ---------------------------------------------------------------------------

/// Every frame the user has put in front of the stacker, and the tables that
/// name them compactly.
///
/// Append-only: frames are indexed by [`FrameId`] and are never reordered or
/// shortened, because a side table written by a later pass indexes into them by
/// position.
#[derive(Debug, Default)]
pub struct Session {
    frames: Vec<FrameRecord>,
    directories: Vec<PathBuf>,
    plugins: Vec<String>,
    groups: Vec<String>,
    /// Content hash to the first frame that carried it, so adding a folder
    /// twice does not double every sub.
    by_content: HashMap<(u64, u64), FrameId>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn frames(&self) -> &[FrameRecord] {
        &self.frames
    }

    pub fn frame(&self, id: FrameId) -> Option<&FrameRecord> {
        self.frames.get(id.index())
    }

    pub fn frame_mut(&mut self, id: FrameId) -> Option<&mut FrameRecord> {
        self.frames.get_mut(id.index())
    }

    pub fn ids(&self) -> impl Iterator<Item = FrameId> + '_ {
        (0..self.frames.len() as u32).map(FrameId)
    }

    /// Frames that are not excluded, which is what every later pass wants.
    pub fn active(&self) -> impl Iterator<Item = (FrameId, &FrameRecord)> + '_ {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, record)| record.is_active())
            .map(|(index, record)| (FrameId(index as u32), record))
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Rebuilds the full path. Allocates, so it is a method rather than a field.
    pub fn path(&self, id: FrameId) -> Option<PathBuf> {
        let record = self.frame(id)?;
        Some(self.directory(record.source.directory).join(record.source.file_name.as_ref()))
    }

    pub fn directory(&self, dir: DirId) -> &Path {
        &self.directories[dir.0 as usize]
    }

    pub fn plugin(&self, plugin: PluginId) -> &str {
        &self.plugins[plugin.0 as usize]
    }

    /// Falls back to the main group rather than panicking: the group table is
    /// empty until someone declares a group, and every frame is in the main one
    /// until then.
    pub fn group_name(&self, group: GroupId) -> &str {
        self.groups.get(group.0 as usize).map_or("main", String::as_str)
    }

    pub fn intern_directory(&mut self, dir: &Path) -> DirId {
        if let Some(index) = self.directories.iter().position(|known| known == dir) {
            return DirId(index as u32);
        }
        self.directories.push(dir.to_owned());
        DirId((self.directories.len() - 1) as u32)
    }

    pub fn intern_plugin(&mut self, id: &str) -> PluginId {
        if let Some(index) = self.plugins.iter().position(|known| known == id) {
            return PluginId(index as u16);
        }
        self.plugins.push(id.to_owned());
        PluginId((self.plugins.len() - 1) as u16)
    }

    /// [`GroupId::MAIN`] is interned on first use, so index 0 is always the
    /// main group whatever the user called it.
    pub fn intern_group(&mut self, name: &str) -> GroupId {
        if self.groups.is_empty() {
            self.groups.push("main".to_owned());
        }
        if let Some(index) = self.groups.iter().position(|known| known == name) {
            return GroupId(index as u16);
        }
        self.groups.push(name.to_owned());
        GroupId((self.groups.len() - 1) as u16)
    }

    pub fn insert(&mut self, record: FrameRecord) -> FrameId {
        let id = FrameId(self.frames.len() as u32);
        self.by_content.entry(record.source.fingerprint.content()).or_insert(id);
        self.frames.push(record);
        id
    }

    /// Finds a frame already in the session with the same bytes.
    pub fn find_by_content(&self, fingerprint: FrameFingerprint) -> Option<FrameId> {
        self.by_content.get(&fingerprint.content()).copied()
    }

    pub fn exclude(&mut self, id: FrameId, reason: Rejection, by: ExclusionSource) {
        if let Some(record) = self.frame_mut(id) {
            // A frame the user excluded stays excluded: re-running the scan
            // must not quietly undo a decision the user made.
            if matches!(record.exclusion, Some(Exclusion { by: ExclusionSource::User, .. })) {
                return;
            }
            record.exclusion = Some(Exclusion { reason, by });
        }
    }

    pub fn include(&mut self, id: FrameId) {
        if let Some(record) = self.frame_mut(id) {
            record.exclusion = None;
        }
    }

    pub fn assign(&mut self, id: FrameId, kind: FrameKind) {
        if let Some(record) = self.frame_mut(id) {
            record.role = FrameRole::Assigned(kind);
        }
    }

    /// Partitions the active frames into sets and builds one plan per light set.
    ///
    /// Pure: it reads the session and allocates a fresh [`Partition`], so it can
    /// be re-run after any edit without the previous answer contaminating it.
    pub fn partition(&self, tolerances: &Tolerances) -> Partition {
        sets::partition(self, tolerances)
    }
}

impl std::ops::Index<FrameId> for Session {
    type Output = FrameRecord;

    fn index(&self, id: FrameId) -> &FrameRecord {
        &self.frames[id.index()]
    }
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_bytes_under_two_paths_are_one_frame() {
        let mut session = Session::new();
        let first = session.insert(testing::record("a.cr3", testing::light()));
        // A second copy of the same file, reached through a mapped drive.
        let same = session.frames()[first.index()].source.fingerprint;
        assert_eq!(session.find_by_content(same), Some(first));

        let other = testing::record("b.cr3", testing::light());
        assert_eq!(session.find_by_content(other.source.fingerprint), None);
    }

    #[test]
    fn a_moved_file_is_recognised_by_content() {
        // Copying a frame to another drive keeps its bytes and loses its
        // timestamp. It is still the same frame.
        let original = FrameFingerprint::new(1024, Some(1_700_000_000), b"CR3 header");
        let copied = FrameFingerprint::new(1024, None, b"CR3 header");
        assert_ne!(original, copied);
        assert!(original.same_content(copied));

        let edited = FrameFingerprint::new(1024, Some(1_700_000_000), b"CR3 heaDer");
        assert!(!original.same_content(edited));
    }

    #[test]
    fn a_user_exclusion_survives_a_rescan() {
        let mut session = Session::new();
        let id = session.insert(testing::record("a.cr3", testing::light()));

        session.exclude(id, Rejection::Unclassified, ExclusionSource::User);
        session.exclude(id, Rejection::NotAFrame, ExclusionSource::Automatic);

        let exclusion = session[id].exclusion.as_ref().expect("still excluded");
        assert_eq!(exclusion.by, ExclusionSource::User);
        assert_eq!(exclusion.reason, Rejection::Unclassified);
    }

    #[test]
    fn excluding_a_frame_does_not_renumber_the_others() {
        // The whole reason ids are dense and never reused: a side table written
        // by a later pass indexes into the session by position.
        let mut session = Session::new();
        let ids: Vec<_> =
            ["a.cr3", "b.cr3", "c.cr3"].iter().map(|n| session.insert(testing::record(n, testing::light()))).collect();

        session.exclude(ids[1], Rejection::Unclassified, ExclusionSource::User);

        assert_eq!(session.len(), 3);
        assert_eq!(session.active().map(|(id, _)| id).collect::<Vec<_>>(), vec![ids[0], ids[2]]);
        assert_eq!(session[ids[2]].source.file_name.as_ref(), "c.cr3");
    }

    #[test]
    fn the_main_group_is_reachable_from_every_group() {
        // A bias library in the main group serves every night...
        assert!(may_draw_from("mon", MAIN_GROUP));
        assert!(may_draw_from("tue", MAIN_GROUP));
        // ...but Monday's darks stay with Monday.
        assert!(!may_draw_from("tue", "mon"));
        assert!(may_draw_from("mon", "mon"));
    }

    #[test]
    fn the_main_group_name_is_reserved() {
        // Asking for it by name hands back the global group, which is why a
        // caller taking the name from a person has to refuse it.
        let mut session = Session::new();
        assert_eq!(session.intern_group(MAIN_GROUP), GroupId::MAIN);
        assert_eq!(session.intern_group("mon"), GroupId(1));
        assert_eq!(session.group_name(GroupId::MAIN), MAIN_GROUP);
    }

    #[test]
    fn a_canon_body_is_not_named_twice() {
        let canon = BodyKey { make: "Canon".into(), model: "Canon EOS R5".into(), serial: None };
        assert_eq!(canon.display(), "Canon EOS R5");

        let nikon = BodyKey { make: "Nikon".into(), model: "D850".into(), serial: None };
        assert_eq!(nikon.display(), "Nikon D850");
    }

    #[test]
    fn a_suggestion_renders_as_a_command_the_user_can_paste() {
        let suggestion = Suggestion::AssignDirectory {
            directory: PathBuf::from("D:/astro/M31/darks"),
            frames: 40,
            proposed: Some(FrameKind::Dark),
        };
        assert_eq!(suggestion.command_fragment().as_deref(), Some("--darks \"D:/astro/M31/darks\""));
    }

    #[test]
    fn a_suggestion_with_nothing_to_suggest_offers_no_command() {
        // The one place the tool refuses to guess is exactly where a paste-able
        // line is most tempting, and `--lights` would be a guess in disguise.
        let suggestion = Suggestion::AssignDirectory {
            directory: PathBuf::from("D:/astro/M31"),
            frames: 9,
            proposed: None,
        };
        assert_eq!(suggestion.command_fragment(), None);
    }
}
