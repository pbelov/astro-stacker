# Backlog

Tasks, not decisions. An entry here is agreed to be worth doing and is not done
yet; the reasoning that would be expensive to reverse belongs in
[ARCHITECTURE.md](ARCHITECTURE.md) instead. Each entry says what the problem is
and how the fix would be recognised, so that picking one up later does not mean
rediscovering why it was written down.

## Defects

### Measuring, rescanning and measuring again crashes the window

Reported sequence: choose lights, measure quality, read the frames again, then
measure quality a second time. Not reproduced yet, and the first step is to
catch what the crash actually is rather than to guess — a webview reload, a
panic in the Rust side, or the process going away — because the three point at
different places.

Two things in the current code are worth suspecting, and both are true
independently of whether they cause this:

* `Running` holds a single `cancel: Arc<AtomicBool>` shared by every command,
  and each command stores `false` into it as it starts. Two passes overlapping
  therefore share one flag: the second un-cancels the first, and Stop hits
  whichever is listening. `apps/desktop/src-tauri/src/lib.rs`.
* The window can have a pass running that it no longer knows about, which is
  the entry below. A second pass started on top of the first is exactly the
  reported sequence.

Done when the sequence runs clean, and when starting a pass while another is
running is either refused with a reason or genuinely supported — not left to
chance. The log below is the tool for finding out, and comes first.

### The window reports a version that is not the one being built

`app_version` returns the desktop crate's own `CARGO_PKG_VERSION`, and
`tauri.conf.json` carries a third copy. The desktop is deliberately its own
workspace — Tauri's dependency tree has no business slowing `cargo test` at the
top of the project — but nothing carries the version across, so both sit at
whatever they were last edited to while the root moves on. `build.bat` names the
release folder from the root version, so a folder can say one number while the
About box inside it says another.

Harmless until something goes wrong, and then it is the first thing that lies:
a crash report or a log line naming the wrong version sends whoever reads it to
the wrong code.

Done when one edit moves all three, and when nothing can build with them
disagreeing.

### Leaving the quality step abandons the measurement

`App.svelte` switches steps with `{#if step === "frames"} … {:else if step ===
"quality"} <Quality …/>`, so stepping back to the frames destroys the component.
Its `running` flag, its `Channel` and the promise from `measure_quality` all go
with it, while the pass itself keeps running in the background with nothing left
to report to. Coming back shows a fresh, idle step.

A measurement is minutes of work, and the reason to step back is usually to look
at something the measurement just raised, so losing it is the wrong answer to a
normal thing to do.

Done when a pass survives leaving and returning to the step, still showing its
progress, and when a genuine cancel is something the user asks for rather than
something a click on another tab does silently.

## Diagnostics

### A log file that outlives the process

Nothing in the window keeps a record: there is no logging in
`apps/desktop/src-tauri` at all, so a crash leaves only what the user
remembers. A file, replaced each run, is the right shape — with two conditions,
because without them such a log is reliably empty exactly when it is needed.

**Keep one generation, do not truncate on start.** The user's next action after
a crash is to relaunch and go looking for the log, and a log truncated on start
is wiped by that very launch. Move the current file aside on startup and keep
the one before it.

**Flush every line.** A buffered writer loses its tail, and the tail is the
part describing the crash. At the rate a window logs, flushing per line costs
nothing.

Three kinds of failure need three different capture points, and only the first
is free:

* A Rust panic. `panic = "unwind"` is already set in the release profile, so a
  hook installed in `run()` can write the payload and a backtrace before the
  stack goes.
* An error in the webview. A JS exception or a rejected promise is invisible to
  the Rust side unless the frontend forwards it, so `window.onerror` and
  `unhandledrejection` need to reach the same file.
* A hard abort — access violation, stack overflow, an allocation that fails. No
  hook runs at all, and only what was already flushed survives. This is what
  makes flushing per line the load-bearing part rather than a nicety.

Prefer `tauri-plugin-log`, the official one, over writing this: it already has
the file target, the webview console and a rotation strategy, and this project
does not need its own logger. Check what its rotation actually does before
relying on it — size-based rotation is not the same as one file per run.

Content worth having, judged by whether it would answer the crash above: the
version, every command as it starts and as it ends with its outcome, how many
frames each pass was given, and every cancel. That sequence alone would likely
settle whether two passes were running at once.

The file has to be findable. A path under `%LOCALAPPDATA%` that nobody is told
about is the same as no log, so the About box should show it and offer to open
the folder.

Done when a deliberate panic, a deliberate JS error and a kill of the process
each leave a file that says what was happening, when relaunching to read it does
not destroy it, and when the user can reach it without being told a path.

## The window

### The role cards take more room than they earn, and files are the awkward way in

The five cards for lights, darks, flats, biases and dark-flats are each a
minimum of 132 px tall and sit before everything else, so the part of the window
that has something to say is pushed down. They should be compact enough that all
five and the result of a scan fit together.

Picking individual frames should be as ordinary as picking a folder. A second
button for it exists, but it reads as the exception; a session where the frames
worth stacking are a hand-picked subset is not an exception.

Done when the five cards together take about half the height they do now, and
when choosing files is not visibly the lesser of the two ways in.

### The quality step needs a better control, a way to resume, and a time estimate

Three things about the same header:

* The Measure button is plain and out of keeping with the rest of the window.
* A run stopped part way through is currently only a run thrown away. Stopping
  at a hundred frames to look, then continuing, is the natural way to use a step
  that takes minutes — and the pipeline already reports frames done out of
  total, so the state to resume from exists.
* Progress is a bar and a frame count with no time on it. The per-frame rate is
  steady enough after the first handful that a remaining-time estimate is honest
  rather than a guess, and without one the only way to know whether to wait is
  to wait.

Done when a stopped run can be continued without re-reading the frames it
already measured, and when the step says how long it has left.

### Name the result files, not the folder they land in

The window asks for an output directory — `open({ directory: true })` in
`Stack.svelte` — and the Rust side writes `stack.fits`, `stack.tif` and
`stack_view.tif` into it. Two runs of the same night with different settings
therefore overwrite each other, and naming a result means renaming files
afterwards.

Each output should be named individually, the FITS and the TIFF separately,
since they are wanted separately: the FITS is the measurement to keep, the TIFF
is what goes into an editor, and a run often wants one and not the other.

Done when each written file has a path the user chose, when declining to name
one means it is not written, and when nothing is overwritten without being
asked.

### A name, a logo and an icon

`astro-stacker` is a working title that describes the category rather than the
program. The window, the installer and the taskbar all show a default icon.

This is a decision to make rather than a task to execute, and it is written down
here so it is made deliberately and once, before a name spreads into the crate
names, the repository, the release artefacts and anything published.

## The result

### Quieten the colour grain in the view TIFF, and only there

Frames are combined by depositing photosites rather than by interpolating them.
One consequence of that is structural rather than incidental: on a Bayer sensor
the red and blue output planes are each built from a quarter of the photosites
and the green from a half, so at full zoom red and blue carry visibly coarser
grain than the same night stacked by a program that demosaiced every frame
before combining it.

That grain is not extra noise. A stacker that interpolates has spread each
measurement over its neighbours, which buys a smooth-looking pixel and no
information — average even a few pixels together and the deposited stack is the
quieter of the two, at every scale a faint object actually occupies. But 1:1 is
where the eye lands first, and a first stretch exaggerates chroma grain before
it brings up anything worth seeing, so the deposited stack reads as the noisier
one to whoever is judging it.

The fix belongs to presentation, not to the stack. `stack.fits` and `stack.tif`
are the measurement and must not move. `stack_view.tif` already exists to be
looked at, and is the place for a mild smoothing of the colour-difference
channels alone, leaving luminance untouched — which is what removes the grain
without costing any resolution.

Done when:

* the view TIFF's red-minus-green and blue-minus-green scatter at pixel scale is
  brought down to about what an interpolating stacker produces,
* star FWHM measured on the view TIFF is unchanged from the linear TIFF, and
* `stack.fits` and `stack.tif` come out bit-identical to what the same run
  produced before the change.
