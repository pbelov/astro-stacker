# Backlog

Tasks, not decisions. An entry here is agreed to be worth doing and is not done
yet; the reasoning that would be expensive to reverse belongs in
[ARCHITECTURE.md](ARCHITECTURE.md) instead. Each entry says what the problem is
and how the fix would be recognised, so that picking one up later does not mean
rediscovering why it was written down.

## Defects

### Two passes can run at once, sharing one stop flag

The crash this entry was opened for is gone, and gone for a reason rather than
by chance. The window built a fresh plugin host for every command and a host
unloaded its libraries when dropped, while the decoder's work-stealing pool
parks its workers inside that library between frames — so each command mapped
the library, started threads in it, and unmapped it under them. The reported
sequence, measure then read the frames again then measure, is three of those
cycles, which is why it took that much churn to show. Fixed in 4d63e45, and
then made impossible in bae6569: a format is a crate now, and there is no
library to unload. ARCHITECTURE.md carries the full account under the plugin
reversal. The owner no longer sees it.

What the entry also asked for is still undone, and it is reachable in two
clicks: start a measurement on the quality step, switch to stacking, press the
button. Nothing refuses it. `Stack.svelte` keeps its own `running` and knows
nothing of `measurement.svelte.ts`, so the button is live; and on the Rust side
neither command checks whether another is in flight. What they share is one
`cancel: Arc<AtomicBool>` on `Running`, and each stores `false` into it as it
starts — so the second pass un-cancels the first, and Stop afterwards reaches
both. The passes also fight over the same cores, each sized for having them
all. `apps/desktop/src-tauri/src/lib.rs`.

`journal::Pass` already counts passes in flight and logs a warning when one
starts inside another, which is how this would be noticed after the fact. A
warning is not a guard.

Refusing is the answer rather than supporting it: two passes of a few hundred
frames each want every core, and running them together makes both slower than
running them in turn. What it needs is a claim taken at the start of a pass and
released when it ends however it ends, and a message naming what is already
running.

Done when starting a pass while another is running is refused with a sentence
saying which one is in the way, when the refusal comes from the Rust side
rather than from a disabled button, and when Stop still stops exactly the pass
that is running.

### A calibration frame is decoded twice and nothing says it is the same frame

`combine_streaming` in `crates/astro-core/src/calibrate/combine.rs` reads every
frame twice: pass one folds each photosite's mean and spread by Welford's
method, pass two decodes the frame again and rejects against the threshold pass
one produced. Nothing ties the second read to the first. The only check is that
the sample count matches, which a file of the same dimensions passes whatever
its pixels now say.

So a file that changes between the passes has the threshold of one version
applied to the pixels of another, and the master comes out wrong with nothing
reporting it. That is not hypothetical for the way these files are kept: a
network share, a removable card, or a transfer still in flight all allow it, and
a session is often pointed at a folder while it is still filling.

Found while unifying the combination paths at 0.18.0 and left on purpose then,
because it is a different class of defect from the ones that change was making.

Done when the second pass verifies it is reading what the first pass measured —
a checksum taken in pass one is the cheap way — and when a frame that fails that
check stops the master with a sentence naming the file, rather than being folded
in.

### The two combination paths find the centre by different arithmetic

The in-memory path takes an exact sum in `clipped_mean`; the streaming path
folds Welford. The two agree to within rounding, which is enough for the centre
itself and not enough for the keep-set: about one photosite in twenty thousand
lands on the other side of the threshold depending on which path ran, and which
path runs is decided by how much memory the session needs. The same frames can
therefore produce two slightly different masters.

The fix is to fold Welford inside `clipped_mean` too, trading a marginally more
accurate estimator for an identical one — agreement between the paths is worth
more here than the last bit of accuracy, because a result that depends on
available memory cannot be reasoned about.

Left at 0.18.0 because it moves results, and that release had already moved them
once. Worth folding into the next change that moves them anyway rather than
spending a release of its own.

Done when a session small enough for either path produces a byte-identical
master through both, and a test pins it.


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

### Stop the window behaving like a browser page

Right-click anywhere and WebView2's own menu appears — Back, Reload, Save as,
View source. Drag across a label and it selects like text on a page. Press F5
and the whole thing reloads. None of that belongs in a program that is supposed
to look like it was written for the desktop, and each one tells the user what is
under the hood at the moment they least need to know.

Nothing is handled today: there is no `contextmenu` handler, no `keydown`
handler and no `user-select` rule anywhere in `apps/desktop/src`. Page zoom is
the exception — `zoomHotkeysEnabled` defaults to false and our config does not
turn it on — but that covers the keys, not `ctrl`+wheel, so it is worth checking
rather than assuming.

The sibling star-trails has done this, and two things it learned are worth
copying rather than rediscovering — `App.svelte`, around the `contextmenu`
listener:

* **Match on `e.code`, not `e.key`.** WebView2 fires its accelerators by
  physical key whatever the layout, so on ЙЦУКЕН `ctrl`+A arrives as `ctrl`+«ф»
  by `e.key` and a handler keyed on the letter simply misses. Their blocked set
  is physical: `KeyA F G P R S O U J L`, `Equal Minus Digit0` and the numpad
  three, `F3 F5 F7`, and `alt`+arrows for history.
* **A lone `alt`, pressed and released, puts the window into menu mode.** Windows
  enters its own message loop and the webview stops delivering pointer events
  until the next click. There is no menu on this window at all, so both keydown
  and keyup for a bare `alt` are swallowed there. `alt` as a modifier is
  untouched, because the system sets `altKey` on mouse events rather than these
  handlers.

The part to get right is what stays. Turning selection off everywhere would be
worse than the disease: a user has to be able to take a path, an error message
or a line of the third-party notices with them. So `user-select: none` belongs
on the frame — labels, buttons, headings — and `user-select: text` goes back
explicitly on the content that is data, which is how the sibling does it too.
Copy has to keep working wherever selection does.

Blocking a key is also claiming it. `ctrl`+O and `ctrl`+S are on the list, and
those are the two a project file will want the moment sessions can be saved, so
this entry and that one should agree on who gets them.

Done when right-click does nothing, when no key reloads, prints, finds or
opens a browser dialog, when dragging across the window does not paint a
selection over labels, and when a path, an error and the notices can still be
selected and copied.

### Paths are shown with whichever slashes they happened to arrive with

The three result files sit in one column under "Where to save", and the column
shows two conventions at once: a path the save dialog returned keeps Windows
backslashes, while a path the window proposed carries a forward slash where it
was joined. Nothing is broken by it — Windows takes either — but three lines of
the same kind of thing should not look like they came from two programs.

The cause is a hardcoded separator in two places in `Stack.svelte`: `propose`
builds the names the user did not pick as `${folder}/${name}${suffix}`, and
`choose` builds the dialog's `defaultPath` the same way, while `folder` comes
from `folderOf`, which deliberately preserves whatever separators it was given.
So the moment one file is picked, the other two are proposed as a mixture.

Display and storage are different questions here and both want an answer. What
is kept should stay exactly as the OS gave it, because it is what gets opened;
what is shown should be one convention, chosen once. Windows writes backslashes,
so that is the one to show on Windows.

Worth doing together with the shortening in the same row: `short` cuts a path at
a fixed number of characters, so it lands mid-segment and produces
`…ain6\…`. Cutting at a separator instead would drop whole folders and read
as a path rather than as a string that got clipped.

Done when the three rows show one convention whatever order the files were named
in, when what is passed to the stacker is still the path the OS gave, and when a
shortened path begins at a folder boundary.

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

### Stack first, decide what to keep afterwards

Today the stacking step refuses to start until at least one output file has been
named. The order is backwards: naming three files is a decision about what to
keep, and it is being asked before there is anything to keep or any way to tell
whether it is worth keeping. What should happen is that the run starts on the
frames alone, and when it ends the result appears — on a step of its own — with
a button per output offering to save it.

**This reverses a refusal that was put in on purpose**, and the reason is in the
code beside it: a stack of a few hundred frames is minutes of work, and finding
out at the end that it was written nowhere is the worst moment to be told. The
new shape answers that better than the refusal did, but only if it holds to one
rule — **the result stays in memory after the run, and the buttons write from
it**. If saving re-runs anything, the refusal was right and this is worse.

The code is already most of the way there, which is worth knowing before
estimating this:

* `combine` returns `Stacked` — the planes, in memory. `write` is already a
  separate function taking that plus the chosen paths.
* `write` already builds each output only when it is asked for: the linear TIFF
  costs another pass over every pixel and a run that wants only the FITS does
  not pay for it. That is exactly the behaviour three buttons want.
* The preview is already kept on `Running` and already comes from the same
  levelled copy the stretched TIFF does, so the window and the file agree.

What has to change is where the result lives between the run and the button.
`write` takes `&Selection<'_>`, which borrows from the survey, so it cannot be
parked in `Running` as it is. What it actually uses from there is small — the
first frame's camera model, exposure, ISO and layout — so lifting those into an
owned header struct dissolves the borrow rather than fighting it.

The cost is memory, and it should be stated rather than discovered: `Stacked`
holds one `f32` plane per colour plus one coverage plane per colour, so a 45
megapixel stack is about 540 MB of planes and as much again of coverage. Worth
checking whether coverage is needed once the run has finished — if it is not,
dropping it halves what is held.

When it is dropped follows the precedent already set by the held masters of a
stopped measurement: the moment it cannot serve. A new run, or a change to what
is being stacked, and it goes. Saving must not drop it — FITS now and the view
TIFF five minutes later is an ordinary thing to want. And since what is held is
minutes of work, throwing it away deserves to be said out loud: closing the
window or starting another run with a result unsaved should ask.

The step itself is a fourth: frames, quality, stacking, result. That collides
with the layout rework, which leaves open whether two of the existing steps
merge, so the two entries want designing together rather than in sequence.

Done when a stack can be started with nothing named, when the result appears on
its own step with what it is made of, when each of the three files can be
written from it afterwards in any order and more than once, when nothing is
recomputed except the output being written, and when an unsaved result is not
lost silently.

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

Four stages, not five, if "Stack first, decide what to keep afterwards" lands
before this: writing leaves the run and becomes a button. That removes the one
stage whose cost depends on which files were named rather than on how many
frames there are, so it makes the estimate easier rather than harder. Each save
then wants its own small figure, and the pass over every pixel a linear TIFF
costs is the one worth showing.

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

### Say how much light the stack actually holds

Nothing anywhere says it. The result reports how many frames were stacked, how
many were refused, and an effective count that discounts for uneven weights —
but never the one number an astrophotographer states first about a night, which
is how long the shutter was open in total. `scan` does not say it either, so a
session's worth is not known before minutes are spent on it.

There is a trap in the way: `StackResultDto.seconds` already exists and is the
wall-clock duration of the run — `started.elapsed()` — not integration time. Two
fields called seconds, one meaning how long you waited and the other how much
light you got, will be read wrongly by someone eventually, so the existing one
wants renaming as part of this.

The reason this is not one number is the reason it is worth doing carefully.
Four are defensible and they are not equal:

* **Shot** — every light in the set, whatever became of it. What the night cost.
* **Kept** — the frames that survived selection. The headline, and what "3h 20m"
  should mean when the window says it.
* **Effective** — kept, discounted by weight. `effective` already does this for
  the frame count, with the comment calling it the honest answer to how deep the
  stack is; the same discount applied to time is the honest answer here.
* **Per pixel** — and this is the one that stops a single figure being a lie.
  Frames are deposited after alignment, so field rotation and drift leave the
  edges covered by fewer frames than the middle. `Stacked.coverage` already
  carries exactly that, per colour plane. Whatever headline is chosen must not
  imply the depth is uniform across the frame, because it is not.

Exposure per frame is `read.exposure_seconds` and it is an `Option`: a body that
recorded nothing leaves a hole. Per this project's rule the sum then is a lower
bound and has to say so, rather than quietly skipping the frame or inventing a
value for it.

It belongs in the FITS header too, not only on screen. Other programs read that
header, and a stack whose total integration has to be recovered by multiplying
two other keywords is a stack that will be quoted wrongly.

Done when the stacking result states the kept total in a form a person would say
out loud, when the effective figure sits beside it rather than replacing it,
when a frame with no recorded exposure makes the total admit it is a floor, when
the FITS carries it, and when the scan step says what a session holds before it
is stacked.

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
