//! What the window writes down about itself, so that a crash leaves evidence.
//!
//! Three things here are load-bearing rather than incidental, and each of them
//! is the difference between a log that answers a question and one that is
//! empty exactly when it is needed.
//!
//! **The previous run is kept.** The first thing anyone does after a crash is
//! start the program again and go looking for the log, so a file truncated on
//! startup is destroyed by the act of going to read it. Each launch moves the
//! last run's file aside under its date and opens a fresh one.
//!
//! **Every line reaches the disk as it is written.** `fern` flushes after each
//! record and the plugin's writer flushes through to the file, so an abrupt end
//! costs nothing already written. This is what makes the third case survivable:
//! a hard end — an access violation, a stack overflow, an allocation that fails
//! — runs no hook at all, and what is on disk is the whole of what is left.
//!
//! **A pass says when it starts and when it stops.** Not for its own sake: the
//! question a crash report has to answer here is whether two passes were
//! running at once, and that is only visible if both ends of each are recorded.

use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use tauri::Runtime;
use tauri::plugin::TauriPlugin;
use tauri_plugin_log::{
    FileOpenStrategy, RotationStrategy, Target, TargetKind, TimezoneStrategy,
};

/// Far more than a run of a few hundred frames writes, so the file that rotates
/// away is the previous launch's and never the first half of this one. The
/// plugin's own default is 40 KB, which a long run would pass — and passing it
/// would discard the start of the run, which is the part that says what was
/// asked for.
const MAX_LOG_BYTES: u128 = 8 << 20;

/// The log file's stem. Fixed rather than taken from the product name because
/// the dated file beside it is matched by this prefix, and a name that moved
/// with a rename would orphan the previous run's log.
const STEM: &str = "astro-stacker";

/// One file per launch, in the user's own clock, with the launch before it kept.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_log::Builder::new()
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir { file_name: Some(STEM.to_owned()) }),
        ])
        // `Rotate` is what makes a launch its own file. `KeepSome(1)` is what
        // keeps the one before it: `KeepOne` reads like the same thing and is
        // not — it keeps one file in total, which means deleting the run that
        // just crashed.
        .file_open_strategy(FileOpenStrategy::Rotate)
        .rotation_strategy(RotationStrategy::KeepSome(1))
        // Whoever reads this was sitting in front of the program when it
        // happened, so the timestamps should match the clock they were
        // watching rather than UTC.
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .max_file_size(MAX_LOG_BYTES)
        .level(log::LevelFilter::Info)
        .build()
}

/// Says whether the run before this one ended on purpose, and marks this one
/// as running.
///
/// An end with no hook - an access violation, a stack overflow, a process
/// killed - leaves a log that stops mid-sentence, and a log that stops
/// mid-sentence looks exactly like one whose writer had nothing more to say.
/// Telling the two apart is the difference between "the log says nothing" and
/// "the log says it was killed", and only the second sends anyone looking in
/// the right place. Windows keeps that place: Event Viewer, Windows Logs ->
/// Application, where an Application Error entry names the faulting module and
/// the exception code.
///
/// A marker file rather than the previous log, because the question is one bit
/// and reading a log to answer it means parsing a log.
pub fn opened(dir: &std::path::Path) {
    let marker = dir.join(format!("{STEM}.running"));
    if marker.exists() {
        log::warn!("the run before this one ended without closing");
        log::warn!(
            "if it vanished rather than being closed, Windows recorded it: Event Viewer, \
Windows Logs, Application - the entry beside that time names the module and the code"
        );
    }
    let _ = std::fs::write(&marker, "running");
}

/// Marks this run as ended on purpose. Whatever is not marked, crashed.
pub fn closed(dir: &std::path::Path) {
    log::info!("closing");
    let _ = std::fs::remove_file(dir.join(format!("{STEM}.running")));
}

/// Sends a panic to the log before the stack goes.
///
/// Chained rather than replacing: the hook already installed is what prints a
/// panic to the console during development, and trading one record for another
/// is not the point.
pub fn catch_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
        previous(info);
    }));
}

/// Passes in flight. Overlap is the thing being watched for, so it is counted
/// rather than inferred from timestamps by whoever reads the file later.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// One command, from the call to whatever ends it.
///
/// A guard rather than a pair of log lines, because the endings that matter are
/// the ones a line at the bottom of a function never reaches: an early return,
/// a `?`, a panic. Those all run `Drop`, and a pass that ended without saying
/// how says that instead of saying nothing.
struct Pass {
    name: &'static str,
    started: Instant,
    outcome: Option<String>,
}

impl Pass {
    fn start(name: &'static str) -> Pass {
        let live = LIVE.fetch_add(1, Ordering::Relaxed) + 1;
        if live > 1 {
            log::warn!("{name} starts while {} already running", live - 1);
        } else {
            log::info!("{name} starts");
        }
        Pass { name, started: Instant::now(), outcome: None }
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        let live = LIVE.fetch_sub(1, Ordering::Relaxed) - 1;
        let seconds = self.started.elapsed().as_secs_f64();
        match self.outcome.take() {
            Some(outcome) => log::info!("{} {outcome} after {seconds:.1}s", self.name),
            // Only a panic or a dropped future reaches this, and both are worth
            // seeing plainly rather than as a missing line.
            None => log::error!("{} ended without an outcome after {seconds:.1}s", self.name),
        }
        if live > 0 {
            log::info!("still running: {live}");
        }
    }
}

/// Runs a command, recording that it ran and how it went.
pub fn pass<T, E: Display>(name: &'static str, body: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    let mut entry = Pass::start(name);
    let result = body();
    entry.outcome = Some(describe(&result));
    result
}

/// [`pass`], for the commands that are `async`.
pub async fn pass_async<T, E: Display>(
    name: &'static str,
    body: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut entry = Pass::start(name);
    let result = body.await;
    entry.outcome = Some(describe(&result));
    result
}

/// The error text, because a failure that reached the window is a failure worth
/// reading here too — and because the window shows it once and then the user
/// clicks something else.
fn describe<T, E: Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "finished".to_owned(),
        Err(error) => format!("failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Everything logged during a test, so the lines can be asserted on.
    static WRITTEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            WRITTEN.lock().unwrap().push(format!("{} {}", record.level(), record.args()));
        }
        fn flush(&self) {}
    }

    fn lines() -> Vec<String> {
        WRITTEN.lock().unwrap().drain(..).collect()
    }

    fn mentions(said: &[String], text: &str) -> bool {
        said.iter().any(|line| line.contains(text))
    }

    /// One test rather than several: the logger is process-wide and can only be
    /// installed once, so tests that each wanted it would race for it.
    #[test]
    fn a_pass_says_when_it_starts_and_how_it_ended() {
        log::set_boxed_logger(Box::new(Capture)).expect("the test logger installs");
        log::set_max_level(log::LevelFilter::Trace);

        let out: Result<u8, String> = pass("reading", || Ok(7));
        assert_eq!(out, Ok(7), "the value the command returned is not the journal's to change");
        let said = lines();
        assert!(mentions(&said, "reading starts"), "{said:?}");
        assert!(mentions(&said, "reading finished"), "{said:?}");

        // A failure is worth reading here too: the window shows it once and
        // then the user clicks something else.
        let out: Result<u8, String> = pass("stacking", || Err("no lights".to_owned()));
        assert!(out.is_err());
        assert!(mentions(&lines(), "stacking failed: no lights"), "an error should reach the log");

        // Overlap is the thing this exists to catch, so a second pass starting
        // inside the first has to be visible rather than merely inferable.
        let _: Result<(), String> = pass("outer", || {
            let _: Result<(), String> = pass("inner", || Ok(()));
            Ok(())
        });
        let said = lines();
        // How many others are live is the other tests' business; that the
        // inner one found the outer one is this test's.
        assert!(
            said.iter().any(|l| l.starts_with("WARN inner starts while")),
            "a pass starting inside another must say so: {said:?}",
        );
        assert!(
            mentions(&said, "still running:"),
            "and what it left behind must be named on the way out: {said:?}",
        );

        // A panic unwinds past the outcome being recorded, which is the case a
        // log line at the end of the function would simply miss.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let fell = std::panic::catch_unwind(|| {
            let _: Result<(), String> = pass("measuring", || panic!("mid-pass"));
        });
        std::panic::set_hook(hook);
        assert!(fell.is_err(), "the panic must still reach the caller");
        let said = lines();
        assert!(mentions(&said, "measuring ended without an outcome"), "{said:?}");
    }
}
