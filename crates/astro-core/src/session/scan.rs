//! Turning a list of paths into a [`Session`].

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::frame::PROBE_HEADER_BYTES;
use crate::frame::FrameInfo;
use crate::frame::ImageLayout;
use rayon::prelude::*;

use super::kind::{self, FrameKind};
use super::{
    BodyKey, Exclusion, ExclusionSource, FrameFingerprint, FrameId, FrameRecord, FrameRole,
    FrameSource, GroupId, Inference, Rejection, Session,
};
use crate::error::{Error, Result};
use crate::format::Formats;

/// How a directory or file the user named should be treated.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleRule {
    pub root: PathBuf,
    /// `None` adds the frames without saying what they are, which is what a
    /// bare path on the command line does.
    pub kind: Option<FrameKind>,
    pub group_name: Option<String>,
}

impl RoleRule {
    pub fn new(root: impl Into<PathBuf>, kind: Option<FrameKind>) -> Self {
        Self { root: root.into(), kind, group_name: None }
    }

    pub fn in_group(mut self, group: Option<String>) -> Self {
        self.group_name = group;
        self
    }
}

/// Cap on transient memory during the metadata pass.
///
/// Opening a frame is not as cheap as it looks: rawler's describe path
/// allocates a full-frame buffer — ninety megabytes for a forty-five megapixel
/// CR3 — and frees it again, so an unbounded pool turns a metadata scan into
/// gigabytes of transient commit and a great deal of page-fault time.
pub const DEFAULT_OPEN_BUDGET_BYTES: u64 = 512 << 20;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub rules: Vec<RoleRule>,
    /// Whether a folder's subfolders are read as well. Off by default.
    ///
    /// A subfolder inside a folder of lights is, far more often than not,
    /// where the frames that should *not* be stacked were put — the framing
    /// shots, the ones with a satellite, the tests at other settings. Reading
    /// it assigns all of them the folder's kind, and the ones that happen to
    /// share the run's exposure and gain cannot be told apart from it by
    /// anything a frame records, so they join the stack without a word. A
    /// frame the user set aside must stay aside; one they want can be named.
    pub recursive: bool,
    pub open_budget_bytes: u64,
    /// Read folder and file names as evidence. Proposals only; nothing is
    /// assigned from them.
    pub infer_from_paths: bool,
    /// File names the user asked to leave out, compared without case.
    ///
    /// The frames are still scanned and still appear in the report, marked as
    /// excluded by the user. Dropping them silently would make a session that
    /// excludes a frame indistinguishable from one that never had it.
    pub excluded: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            recursive: false,
            open_budget_bytes: DEFAULT_OPEN_BUDGET_BYTES,
            infer_from_paths: true,
            excluded: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub opened: usize,
    pub total: usize,
}

#[derive(Debug)]
pub struct ScanReport {
    pub session: Session,
    /// Files that were looked at and are not frames in the session, each with a
    /// reason. Reported rather than skipped, for the same reason a plugin that
    /// fails to load is reported: a silently dropped file quietly shrinks the
    /// stack.
    pub rejected: Vec<(PathBuf, Rejection)>,
    pub elapsed: Duration,
    /// How many frames were opened at once. Reported because it is derived from
    /// a memory budget and the frame size, not chosen.
    pub workers: usize,
}

/// Walks the rules, opens what looks like a frame, and records what it found.
///
/// The pass order is fixed and is not negotiable: a frame is classified before
/// it is grouped. Grouping by exposure first would drop a two-second framing
/// exposure into the flat set, and every light in the stack would then be
/// divided by a picture of the sky.
pub fn scan(host: &Formats, options: &ScanOptions) -> Result<ScanReport> {
    scan_with_progress(host, options, &|_| {})
}

/// [`scan`], reporting how far it has got.
///
/// The callback is `Sync` because it is called from the worker pool. It is
/// called once per frame opened, which is often enough for a progress bar and
/// rare enough not to matter.
pub fn scan_with_progress(
    host: &Formats,
    options: &ScanOptions,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<ScanReport> {
    let started = Instant::now();
    let extensions = host.supported_extensions();

    // Canonicalised once, up front. The filesystem is case-insensitive on
    // Windows and `Path::starts_with` is not, so `--darks D:/astro/darks` would
    // otherwise fail to claim a file the walk discovered as `D:/Astro/Darks/..`,
    // and every dark in that folder would be filed as unassigned. Canonicalising
    // the roots gives every discovered path one spelling, so the rule matching,
    // the sort and the dedup all agree.
    let rules: Vec<RoleRule> = options
        .rules
        .iter()
        .map(|rule| RoleRule {
            root: std::fs::canonicalize(&rule.root).unwrap_or_else(|_| rule.root.clone()),
            ..rule.clone()
        })
        .collect();

    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for rule in &rules {
        collect(&rule.root, options.recursive, &extensions, &mut candidates, &mut rejected)?;
    }
    candidates.sort();
    candidates.dedup();
    // Two copies of one frame are found by content later, and the first one
    // inserted is the one that stays. Order so that a copy the user assigned a
    // kind to always comes first: otherwise `--lights D:/lights D:/misc` keeps
    // whichever path sorts earlier, and if that is the unassigned copy in misc,
    // the light is dropped from the stack without anything looking wrong.
    candidates.sort_by_key(|path| {
        let assigned = deepest_rule(&rules, path).and_then(|rule| rule.kind).is_some();
        (!assigned, path.clone())
    });

    // One frame opened serially, to size the pool against the frames actually
    // present rather than against a guess.
    let (workers, first) = size_the_pool(host, &candidates, options.open_budget_bytes);

    let total = candidates.len();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    let open_at = |index: usize, path: &PathBuf| {
        let outcome = match (index, &first) {
            // Reuse the frame that sized the pool rather than opening it twice.
            (0, Some(opened)) => Ok(opened.clone()),
            _ => open_one(host, path),
        };
        let done = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        progress(Progress { opened: done, total });
        (path.clone(), outcome)
    };

    // A pool that will not build is not a reason to refuse to scan; it only
    // costs speed. The fallback runs genuinely serially: falling through to
    // `par_iter` outside an `install` would run on rayon's global pool, whose
    // thread count owes nothing to the memory budget this scan was sized
    // against — and the number reported to the user would be a fiction.
    let mut workers = workers;
    type Opened = Vec<(PathBuf, std::result::Result<Scanned, Rejection>)>;
    let opened: Opened = match rayon::ThreadPoolBuilder::new().num_threads(workers).build() {
        Ok(pool) => {
            pool.install(|| candidates.par_iter().enumerate().map(|(i, p)| open_at(i, p)).collect())
        }
        Err(err) => {
            log::warn!("scanning on one thread: {err}");
            workers = 1;
            candidates.iter().enumerate().map(|(i, p)| open_at(i, p)).collect()
        }
    };

    let mut session = Session::new();
    for (path, outcome) in opened {
        match outcome {
            Ok(scanned) => {
                if let Some(existing) = session.find_by_content(scanned.fingerprint) {
                    rejected.push((presentable(&path), Rejection::Duplicate { of: existing }));
                    continue;
                }
                insert(&mut session, options, &rules, &path, scanned);
            }
            Err(reason) => rejected.push((presentable(&path), reason)),
        }
    }

    Ok(ScanReport { session, rejected, elapsed: started.elapsed(), workers })
}

/// What one worker copies out of an open frame before dropping it.
///
/// An `OpenFrame` holds a memory map, a decoder and a plugin handle; five
/// hundred held at once would be five hundred of each.
#[derive(Debug, Clone)]
struct Scanned {
    layout: ImageLayout,
    info: FrameInfo,
    plugin: String,
    fingerprint: FrameFingerprint,
}

fn open_one(host: &Formats, path: &Path) -> std::result::Result<Scanned, Rejection> {
    let fingerprint = fingerprint(path).map_err(|err| Rejection::Unreadable { detail: err.to_string() })?;
    match host.open(path) {
        Ok(frame) => Ok(Scanned {
            layout: *frame.layout(),
            info: frame.info().clone(),
            plugin: frame.format().description().id.clone(),
            fingerprint,
        }),
        Err(Error::UnsupportedFormat { .. }) => Err(Rejection::NotAFrame),
        Err(err) => Err(Rejection::Unreadable { detail: err.to_string() }),
    }
}

fn insert(
    session: &mut Session,
    options: &ScanOptions,
    rules: &[RoleRule],
    path: &Path,
    scanned: Scanned,
) -> FrameId {
    let rule = deepest_rule(rules, path);
    let group = match rule.and_then(|rule| rule.group_name.as_deref()) {
        Some(name) => session.intern_group(name),
        None => GroupId::MAIN,
    };
    let role = match rule.and_then(|rule| rule.kind) {
        Some(kind) => FrameRole::Assigned(kind),
        None if options.infer_from_paths => {
            FrameRole::Unassigned { inference: kind::infer(path, &scanned.info) }
        }
        None => FrameRole::Unassigned { inference: Inference::NoEvidence },
    };

    let directory =
        session.intern_directory(&presentable(path.parent().unwrap_or(Path::new("."))));
    let plugin = session.intern_plugin(&scanned.plugin);
    let file_name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();

    // Matched on the file name rather than the whole path: a name is what the
    // report prints and what the user can copy back out of it.
    let excluded = options
        .excluded
        .iter()
        .any(|name| name.eq_ignore_ascii_case(file_name))
        .then_some(Exclusion { reason: Rejection::Excluded, by: ExclusionSource::User });

    session.insert(FrameRecord {
        source: FrameSource {
            directory,
            file_name: file_name.into(),
            plugin,
            fingerprint: scanned.fingerprint,
        },
        layout: scanned.layout,
        body: BodyKey::from_info(&scanned.info),
        info: scanned.info,
        role,
        group,
        exclusion: excluded,
    })
}

/// The rule whose root is the longest prefix of `path`.
///
/// Longest wins so that `--lights session/ --darks session/darks/` does what it
/// reads like, rather than depending on the order the flags were given.
fn deepest_rule<'a>(rules: &'a [RoleRule], path: &Path) -> Option<&'a RoleRule> {
    rules
        .iter()
        .filter(|rule| path.starts_with(&rule.root) || path == rule.root)
        .max_by_key(|rule| rule.root.components().count())
}

/// Turns a memory budget into a worker count, using the size of a frame that is
/// actually there.
///
/// A guessed constant is wrong in both directions: too high on a 100-megapixel
/// sensor, too low on a small one. Returns the frame it opened so the caller
/// does not pay for it twice.
fn size_the_pool(
    host: &Formats,
    candidates: &[PathBuf],
    budget_bytes: u64,
) -> (usize, Option<Scanned>) {
    let ceiling = std::thread::available_parallelism().map_or(4, |n| n.get());
    let Some(first) = candidates.first() else {
        return (1, None);
    };
    let Ok(scanned) = open_one(host, first) else {
        // Whatever went wrong will be reported again in the main pass.
        return (ceiling.min(4), None);
    };
    let per_frame = scanned.layout.required_bytes().unwrap_or(1 << 26) as u64;
    let workers = (budget_bytes / per_frame.max(1)).clamp(1, ceiling as u64) as usize;
    (workers, Some(scanned))
}

fn fingerprint(path: &Path) -> std::io::Result<FrameFingerprint> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok().or_else(|| unix_before_epoch(time)))
        .and_then(|since| i64::try_from(since.as_secs()).ok());

    let mut header = vec![0u8; PROBE_HEADER_BYTES];
    let mut file = File::open(path)?;
    let mut filled = 0;
    while filled < header.len() {
        match file.read(&mut header[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    header.truncate(filled);
    Ok(FrameFingerprint::new(metadata.len(), modified, &header))
}

/// A file dated before 1970 is not a frame anyone shot, but it should not make
/// the scan fail either.
fn unix_before_epoch(_time: SystemTime) -> Option<Duration> {
    None
}

fn collect(
    root: &Path,
    recursive: bool,
    extensions: &[String],
    into: &mut Vec<PathBuf>,
    unreadable: &mut Vec<(PathBuf, Rejection)>,
) -> Result<()> {
    let metadata =
        std::fs::metadata(root).map_err(|source| Error::Io { path: root.to_owned(), source })?;
    if metadata.is_file() {
        // A file the user named is opened whatever it is called: the extension
        // filter exists to keep a directory walk cheap, not to overrule them.
        into.push(root.to_owned());
        return Ok(());
    }

    // A junction or a symlink can point back up the tree, and a walk that did
    // not remember where it had been would never finish.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        let identity = std::fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone());
        if !visited.insert(identity) {
            continue;
        }

        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            // Only the root the user named is worth failing for. A subdirectory
            // that cannot be listed - a permission, a disconnected share - is one
            // more thing to report, not a reason to throw away every frame
            // already found.
            Err(err) => {
                unreadable.push((
                    presentable(&directory),
                    Rejection::Unreadable { detail: err.to_string() },
                ));
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            // Windows reports a junction and a directory symlink as neither a
            // file nor a directory, so testing `is_dir` alone would drop a whole
            // subtree without a word.
            let is_directory = kind.is_dir()
                || (kind.is_symlink()
                    && std::fs::metadata(&path).is_ok_and(|target| target.is_dir()));
            if is_directory {
                if recursive {
                    directories.push(path);
                }
            } else if has_supported_extension(&path, extensions) {
                into.push(path);
            }
        }
    }
    Ok(())
}

/// Removes the verbatim prefix that `canonicalize` adds on Windows.
///
/// The long and the short spelling open the same file; only one of them belongs
/// in a report. A verbatim UNC path keeps its prefix, because shortening that
/// one changes which share it names.
fn presentable(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    match text.strip_prefix(VERBATIM_PREFIX) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_owned(),
    }
}

const VERBATIM_PREFIX: &str = "\\\\?\\";

fn has_supported_extension(path: &Path, extensions: &[String]) -> bool {
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    extensions.iter().any(|known| known.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_deepest_rule_claims_a_file() {
        // What `--lights session/ --darks session/darks/` has to mean, whichever
        // order the flags came in.
        let rules = vec![
            RoleRule::new("D:/astro/session", Some(FrameKind::Light)),
            RoleRule::new("D:/astro/session/darks", Some(FrameKind::Dark)),
        ];
        let light = deepest_rule(&rules, Path::new("D:/astro/session/IMG_0001.CR3"));
        assert_eq!(light.and_then(|rule| rule.kind), Some(FrameKind::Light));

        let dark = deepest_rule(&rules, Path::new("D:/astro/session/darks/IMG_0100.CR3"));
        assert_eq!(dark.and_then(|rule| rule.kind), Some(FrameKind::Dark));

        assert_eq!(deepest_rule(&rules, Path::new("D:/elsewhere/x.CR3")), None);
    }

    #[test]
    fn an_assigned_copy_outranks_an_unassigned_one() {
        // Which of two identical files survives deduplication is decided here,
        // by the order they are opened in.
        let rules = vec![
            RoleRule::new("D:/misc", None),
            RoleRule::new("D:/lights", Some(FrameKind::Light)),
        ];
        let mut candidates =
            [PathBuf::from("D:/misc/copy.CR3"), PathBuf::from("D:/lights/IMG_0001.CR3")];
        candidates.sort();
        assert_eq!(candidates[0], PathBuf::from("D:/lights/IMG_0001.CR3"));

        // Now the case that bites: the unassigned folder sorts first.
        let mut candidates =
            [PathBuf::from("D:/lights/IMG_0001.CR3"), PathBuf::from("D:/misc/copy.CR3")];
        candidates.sort_by_key(|path| {
            let assigned = deepest_rule(&rules, path).and_then(|rule| rule.kind).is_some();
            (!assigned, path.clone())
        });
        assert_eq!(
            candidates[0],
            PathBuf::from("D:/lights/IMG_0001.CR3"),
            "the frame the user named must be the one that stays"
        );
    }

    #[test]
    fn a_directory_walk_skips_what_no_plugin_reads() {
        let extensions = vec!["cr2".to_owned(), "cr3".to_owned()];
        assert!(has_supported_extension(Path::new("a/IMG_0001.CR3"), &extensions));
        assert!(has_supported_extension(Path::new("a/IMG_0001.cr2"), &extensions));
        assert!(!has_supported_extension(Path::new("a/IMG_0001.xmp"), &extensions));
        assert!(!has_supported_extension(Path::new("a/capture.log"), &extensions));
        assert!(!has_supported_extension(Path::new("a/README"), &extensions));
    }
}
