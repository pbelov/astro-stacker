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

### The quality step needs a better control and a way to resume

Two things about the same header:

* The Measure button is plain and out of keeping with the rest of the window.
* A run stopped part way through is currently only a run thrown away. Stopping
  at a hundred frames to look, then continuing, is the natural way to use a step
  that takes minutes — and the pipeline already reports frames done out of
  total, so the state to resume from exists.

Done when a stopped run can be continued without re-reading the frames it
already measured.

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

### An honest remaining time on both long steps, and no talking around it

Neither the quality step nor the stacking step says how long it has left. Both
show a bar and a frame count, and under it a sentence explaining that this takes
minutes — `measureSlow` and `stackSlow` in `i18n.svelte.ts`. Those sentences
exist only because there is no number; a number replaces them, and they should
go with it rather than sit beside it.

Quality is the easy half: one stage, a steady per-frame rate after the first
handful, so time remaining follows from frames remaining.

Stacking is the real problem, and doing it the easy way would produce a
confident wrong answer. The run is five stages of quite different cost — masters,
measuring every light, aligning, depositing in one pass or two, writing — and
the progress bar restarts within each. A remaining time has to be over the whole
run, which means weighting the stages by what they actually cost rather than by
their frame counts. Measuring and depositing are both dominated by decoding and
are the two that matter; aligning and writing are rounding error next to them;
rejection adds a second deposit pass, which the run already knows about because
it reports the pass number.

Done when both steps show a time that is stable rather than jumping about as a
stage changes, when a run with rejection is not estimated as though it had one
pass, and when the explanatory sentences are gone.

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
