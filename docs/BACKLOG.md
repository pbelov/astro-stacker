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

Half of what made the sequence possible is gone: the window no longer loses
track of a running measurement, and a second one can no longer be started on
top of a first. What remains worth suspecting is `Running`, which holds a single
`cancel: Arc<AtomicBool>` shared by every command and stores `false` into it as
each one starts — so a measurement and a stack overlapping share one flag, the
second un-cancels the first, and Stop reaches whichever is listening.
`apps/desktop/src-tauri/src/lib.rs`.

The run is now logged: each command says when it starts and how it ended, and
passes in flight are counted, so retrying the sequence and reading
`astro-stacker.log` afterwards is the first step rather than reasoning about it.

Done when the sequence runs clean, and when starting a pass while another is
running is either refused with a reason or genuinely supported — not left to
chance.

### Confirm a crop-mode frame against a real one

The handling is written; what is missing is a file to run it against, and the
rule here is that a decode path is not claimed to work until it has been.

What was found on the way is the part worth keeping. `rawler` builds the active
and crop rectangles by subtracting camera-database borders from the frame's own
size, and a body in a crop mode writes a frame smaller than the database
describes. This entry used to say the subtraction panics and the frame lands in
the rejected list. That is true of a debug build only. A release build — the one
that ships — wraps instead, and hands over a rectangle about 1.8e19 pixels wide
with no error anywhere, which is a worse failure than the one that was written
down. ARCHITECTURE.md carries the measurement.

Both paths are now handled and tested: the absurd rectangle is refused and the
whole frame used instead, which is the right geometry here because this program
deposits photosites rather than cropping, and the panic that a debug build still
raises is translated into a sentence naming crop mode rather than relayed as
`assertion failed: p1.x <= p2.x`.

None of that has met an actual crop-mode frame. The cheapest way to close it is
one exposure from the R5 Mark II in crop mode, lens cap on, dropped into
`testdata/`; the test that exists would then be pointed at it. Failing that,
raw.pixls.us publishes sample files per body, including the compressed X-T2 RAF
that fails the same way.

Done when such a frame has been read here, its reported geometry checked against
what the camera actually wrote, and the test in `astro-format-raw` runs against
it rather than against synthetic numbers.

## The window

### Rework the layout: controls on the left, results on the right, nothing scrolling

The window is one column of stacked cards, so controls and results are mixed
down the page and everything is reached by scrolling. It should be two panes:
every button, setting and control on the left, and the right showing only what
came out.

Steps stay as they are — frames, quality, stacking — unless two of them turn out
to belong together once the controls are gathered on one side; that is a
question to answer while doing it, not before.

The no-scrolling requirement needs stating precisely, because taken literally it
cannot be met: a per-frame table of three hundred frames does not fit on a
screen. Read as: **the window itself never scrolls in either direction.**
Anything longer than its pane scrolls inside that pane, which a fixed two-pane
layout gives for free, and which is why the cards' ad-hoc `max-height` limits —
`.scroll` in `Quality.svelte`, `.paths` in `DropZone.svelte` — go away rather
than multiply. If that reading is wrong, it is the thing to correct before any
of this is built.

Done when the window has no scrollbar of its own at any size it can be opened
at, when every control is on the left and nothing on the right is a control, and
when narrowing the window rearranges rather than clips.

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

### Name the frames that are not lights, and offer to refile them

A dark or a flat filed among the lights survives the whole run today. The
measuring step finds no stars in it, so it lands under "no shape" with a count
of zero beside it, which says what failed and not what the frame is; the frame
then fails to register and is dropped without anyone learning why it was there.

The numbers that would name it are already measured. Every light is decoded,
calibrated and searched at this step, and what comes back per frame is the star
count, the sky level and its noise, the saturated and oversized counts and the
shape — `Detection` in `stars/mod.rs`, carried per frame on `Measured`. A dark
sits at the calibration floor with no stars; a flat sits high and smooth with no
stars; a light of a clouded sky sits high with no stars but is not smooth. So
this costs no extra decoding, which is the whole reason it belongs at this step
rather than at the scan: the scan would have to read five hundred frames to
learn what this pass has already read.

It may only propose. ARCHITECTURE.md settles that evidence never elects a
frame's kind, and the reasoning there is worth re-reading before starting: it
argues a dark has only a negative signature, no sky and no stars, indistinguishable
from a light under thick cloud. That argument is about metadata — exposure, ISO
and body are identical for a dark and its lights by construction. Pixels do
separate them, which is what makes this worth doing and also what it must not be
allowed to overreach into: the residual pairs that pixels still cannot split are
a dark against a light of an empty field, and a flat against a light of the
twilight sky.

Refiling is a change of role inside the session and nothing more: no file on
disk moves, then or ever — see ARCHITECTURE.md. Roles are carried by `RoleRule`,
which is per path rather than per frame, so how a single frame takes a role
different from the folder it sits in is the open design question; the window can
now be given individual files, which is most of the way there.

The reverse direction is the damaging one and is not covered by this entry: a
light among the darks poisons the master and subtracts a star field from every
frame in the stack, silently. It is not visible at this step because this step
measures lights, though it does build the masters first. Worth deciding whether
it belongs here too.

Done when a dark and a flat placed among the lights are each named as what they
are rather than as a frame with no stars, when nothing is reassigned without
being asked, and when a frame the tool is unsure about is reported as unsure
rather than guessed at.

### An honest remaining time on the stacking step too

The quality step now says how long it has left; the stacking step still shows a
bar, a frame count and `stackSlow` in `i18n.svelte.ts` — a sentence that exists
only because there is no number, and that goes when one arrives.

The estimator is `remaining.ts`, and what it does is worth knowing before
extending it: it times the **current stage only** and names that stage beside
the figure. This entry used to claim quality was the easy half because it had
one stage. That was wrong twice over. It has two — the masters are built first —
and a frame in them does not cost what a light costs: on one session a master
frame ran several times slower than a light, because masters are held in memory
all at once while lights are read on several threads. Any estimate that spends
one stage's rate on another is confidently wrong, and a confident wrong figure
is worse than none, because it is what someone decides on when they choose to
wait or to walk away.

Stacking makes that harder rather than different. Its five stages — masters,
measuring every light, aligning, depositing in one pass or two, writing — differ
in cost the same way, and rejection adds a second deposit pass, which the run
already knows about because it reports the pass number. Measuring and depositing
are dominated by decoding and are the two that matter; aligning and writing are
rounding error beside them. Whether those ratios are stable enough to carry one
figure across the whole run is a question to settle by measuring a real run, not
by reasoning.

Done when the stacking step shows a time that does not jump about, when a run
with rejection is not estimated as though it had one pass, and when `stackSlow`
is gone.

### Carry the name the rest of the way, and settle the extension

The name is decided and written down: AstroAccretion, with the reasoning in
[ARCHITECTURE.md](ARCHITECTURE.md). The visible half is done — the window, the
installer, the About box and the icons at every size, generated by
`tools/gen-icons.py`.

What is left is the half that touches everything: `astro-core`, `astro-cli`,
`astro-format-raw`, the `astro-stacker` binary and the log file it writes, the
names `build.bat` checks for, the repository and the URL printed in
`THIRD-PARTY-NOTICES.md`, and CLAUDE.md. Mechanical, but wide enough that it
wants its own commit and a working build on the other side rather than being
folded into something else.

The extension is the part still undecided, and the next entry waits on it.
`.accr` is the proposal; `.as` is the placeholder in use. Whatever is chosen,
check it against the registry on the machine first — the sibling project took
`.stx` and found Winamp already claimed it here, which is how a double-click
ends up opening the wrong program.

Done when nothing but git history says astro-stacker, when a build produced
after the rename passes the same checks as one before it, and when the project
file has an extension that nothing else on a normal Windows machine claims.

### Saving a project, and opening it again

A session is a list of frames with roles, the thresholds that were set, and what
came out. None of it survives closing the window: the next run starts from an
empty pair of panes, and a night that took an evening to arrange is arranged
again. The sibling star-trails project has this, so the shape is decided rather
than invented — `.as` for the extension for now, JSON inside, and a pair of
narrow commands rather than opening the filesystem to the window at large.

Read that project's ARCHITECTURE.md §8.4 before starting. Four things there were
learned the expensive way and are cheaper to copy than to rediscover:

* **Autosave does not write the open file.** It went straight into the project
  file once, which made the unsaved-changes marker go out after 800 ms and mean
  nothing. Autosave belongs in an invisible draft beside the application's own
  data; the file is written when the user says to.
* **An empty project is never written anywhere.** That is what stops a stray
  clear from overwriting a real file with nothing.
* **The recent list lives beside the application's data, not in the browser's
  storage.** A list of sessions would not survive the webview clearing its
  storage, and a thumbnail has no business in it.
* **The window title wants a narrow command, not a permission.** Letting the
  page set the title at will is a wider door than putting the name in a title
  the shell assembles.

Done when a session can be saved, reopened and continued — frames, roles and
thresholds — when nothing of the user's is written anywhere but the file they
named and the program's own data, and when closing with unsaved work says so.

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

The fix belongs to presentation, not to the stack. The FITS and the linear TIFF
are the measurement and must not move. The stretched TIFF is written to be
looked at and nothing else, so it is the place for a mild smoothing of the
colour-difference channels alone, leaving luminance untouched — which is what
removes the grain without costing any resolution.

Done when:

* the stretched TIFF's red-minus-green and blue-minus-green scatter at pixel
  scale is brought down to about what an interpolating stacker produces,
* star FWHM measured on the stretched TIFF is unchanged from the linear one, and
* the FITS and the linear TIFF come out bit-identical to what the same run
  produced before the change.
