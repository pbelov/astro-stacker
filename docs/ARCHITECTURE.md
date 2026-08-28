# Architecture

A record of the decisions that are expensive to reverse, and why they were
taken. Anything not written here is still open.

## Decisions

### Rust for everything

Chosen over C++/Qt and C#/.NET. The deciding factor was not language taste but
build friction: rawler decodes CR2 and CR3 in pure Rust, so the whole project
builds with `cargo build` and no C toolchain, CMake or vcpkg. Memory safety and
`rayon` are what make a hundreds-of-frames pipeline tractable without a class of
bug that is very hard to find in a numerical codebase.

### Formats are dynamic libraries, not Rust traits

A plugin is a `.dll` discovered at runtime and reached through a `#[repr(C)]`
ABI. Rust traits would have been simpler and faster to write, and would have
made every format a compile-time dependency of the application — exactly the
coupling this project exists to avoid.

The consequences are deliberate:

* Anything can implement a plugin: a different Rust version, C++, or Zig.
* No Rust types cross the boundary. No `String`, `Vec`, `Option`, `Result`, or
  data-carrying enums.
* Whoever allocates, frees. On Windows each DLL may carry its own CRT heap, so
  freeing across the boundary corrupts memory instead of failing loudly. Large
  pixel buffers are therefore allocated by the host and filled by the plugin.
* Panics must not unwind across the boundary. `export_plugin!` contains them.
* `ABI_VERSION` is checked in both directions at load. There is no forward
  compatibility: a mismatch refuses to load rather than guessing.

Layout sizes are asserted in `astro-plugin-abi`'s tests so that changing a
struct fails the build rather than silently breaking installed plugins.

### Portable by construction, built only for Windows

Nothing in the code is Windows-specific, but CI, packaging and testing target
Windows 11 alone. The cost of cross-platform support is almost entirely in
decisions taken now, not in work done now:

* Paths and strings cross the ABI as UTF-8, never UTF-16.
* GPU work will go through `wgpu`, not CUDA or D3D12 directly.
* SIMD will go through portable abstractions, not AVX intrinsics.

Deferred: installers, CI runners, and platform-specific I/O tuning.

### CPU first, GPU later

Stacking is dominated by disk and memory bandwidth more often than by
arithmetic. Correct results and honest measurements come first; acceleration
goes where the measurements point. The plugin architecture leaves room for a GPU
backend to arrive without disturbing the frame model.

### Timestamps are read as UTC

EXIF records wall-clock time with no zone. The Canon plugin interprets it as
UTC. That is not the true instant, but every frame in a session is off by the
same constant, so ordering and the intervals between frames — all stacking
actually needs — are exact.

### Evidence may propose and may veto, but may never elect

A frame's kind is assigned by the user. Folder names, file names and shooting
parameters are read as evidence and shown as proposals; nothing is ever assigned
from them, and there is no confidence score anywhere in the model.

This is an asymmetry, not a preference. A light, a flat and a bias each have a
positive signature. A dark has only a negative one — no sky, no stars, no
gradient — and those absences are equally consistent with a light shot under
thick cloud. A classifier that must label every frame is forced to guess exactly
where a wrong guess does the most damage, and the damage is silent: a light
filed as a dark subtracts a star field from every frame in the stack, a light
filed as a flat divides every frame by a picture of the sky, and neither throws.

Nor is this a gap the ABI could close. A correctly shot dark has the *same*
exposure, ISO and body as its lights — that is what makes it valid — so those
are precisely the fields that cannot separate them. FITS capture software writes
an `IMAGETYP` keyword; CR2 and CR3 have no equivalent.

### Two types for incompatibility, not two severities

`Incompatibility` is a refusal; `Mismatch` is a report. They are separate types
rather than one type with a severity field, so that a caller cannot accidentally
treat a refusal as advisory or promote a report into a refusal.

The line is drawn where the arithmetic stops being **defined**, not where it
stops being ideal. Dimensions, component count, sample format, bit depth, mosaic
phase and active area decide which photosite `samples[i]` is; if two frames
disagree about that, no later correction recovers them. Everything else —
exposure, gain, temperature, black point, orientation — changes the magnitude of
a signal that is still spatially aligned. A stacker that refuses a 295-second
dark on a 300-second light because a threshold said so is worse than one that
uses it and says what it did.

The two exceptions are deliberate and both are about gain: a dark or a bias at
the wrong ISO is a hard refusal, because gain scales read noise and the
fixed-pattern amplitude together, there is no benign reason for the mismatch,
and the result is a faintly wrong background nobody traces back to the
calibration frames.

Two lights at different ISO, by contrast, are never *refused* — normalisation
removes a linear scale factor. They still land in separate sets, because a set
is the unit calibration is matched to and a dark must match ISO exactly: one set
spanning two gains could only be given one master dark, and it would be wrong
for half of it. DeepSkyStacker draws the same line, building a separate task per
ISO and summing them into one output image. Summing is a stacking concern, and
stacking is not built yet; when it is, one integration will draw from several
light sets.

### A rule that could not run is a third state

`Mismatch::Unrecorded` and `Incompatibility::Unrecorded` exist because two
absences are not agreement. On the hardware this project targets, sensor
temperature is *never* recorded — rawler does not surface Canon's makernote
value — so a report that said "temperatures match" would be a lie the user acts
on, on every single frame. A silent pass and a rule that never fired must not
look the same.

### Frames are named by a dense id, not by a path

`Session` is an append-only arena and a frame is a `FrameId` into it. Excluding
a frame leaves its id in place. Registration will want one transform per frame,
star extraction a list of stars per frame, and integration a weight per frame;
all three arrive as side tables indexed by the same integer, so none of them
reshapes `FrameRecord`, and none of them leaves a megabyte of star positions
resident while the user is only looking at a list of sets.

`SetId`, by contrast, is a **within-partition handle only**. Partitioning is
pure and reallocates ids from zero, so excluding one frame can renumber every
set. Anything that outlives one partition — a cached master frame — must travel
by `SetKey`, which is `Hash + Eq`. `Partition::set_by_key` is the rebinding path.

### Exposure cannot key a hash map

Sets are built in two passes. The first hashes on `PartitionKey`, every field of
which is exact. The second sorts each bucket by exposure and cuts it into runs.
A tolerance is not an equivalence relation — 100, 104 and 108 seconds are each
within five per cent of a neighbour while the ends are not within five per cent
of each other — so it cannot be part of a hash key. Sorting first is what makes
the answer independent of the order files were scanned in; extending a run only
while the whole run's span stays inside the tolerance of its longest member is
what stops a night of sixty-second subs chaining into a night of six-hundred
second ones.

### Bit depth is a container width, not a scale

`ImageLayout::bits_per_sample` is whatever the plugin reports as the width of
the container, and for the Canon plugin that is rawler's `real_bps`, which
defaults to 16 for every camera whose database entry does not override it.
Measured on real frames: the 5D Mark IV, R5 and R5 Mark II all report 16 while
their converters run at 14 bits, and only the 60D reports 14 — because its
database entry happens to say so.

So the bit-depth rule cannot do the job it was written for. The saturation
point can: rawler derives it per camera and per mode, and the four bodies report
14448, 15094, 14888 and 14888 — one scale, ordinary variation. A 12-bit readout
would report about 4095. `same_scale` therefore refuses two frames whose white
levels differ by a factor of two or more, and that is the load-bearing check;
the bit-depth comparison stays as a cheap first pass.

Neither refuses on an absence. Unlike gain, where nothing else covers the
ground, these two rules back each other up, so an unrecorded white level is
reported rather than fatal.

### `read_samples` returns sensor readout order

A plugin must never permute the buffer to honour `ImageLayout::orientation`.
That field is metadata about how an image should be shown, not about how it is
stored. The host indexes frames against each other photosite by photosite, so
two frames of one sensor must always agree on what `samples[i]` is, whichever
way the camera was pointing. Orientation is therefore a note on a flat, never a
reason to refuse a frame.

### Stars are found on the raw mosaic, at full resolution

Detection thresholds each photosite against its own colour's local sky and its
own colour's local noise, on the undemosaiced frame. Nothing is binned or
reduced to green.

The worry this answers is that a small star landing on a red photosite and the
same star landing on a green one would be found in different places. The colour
modulation of a Bayer pattern is a signal at exactly the Nyquist frequency, so
when a source is centred on a photosite the modulated part is even about that
centre and contributes nothing to the first moment. Measured on 763 stars
matched between two consecutive frames of the reference session, the mosaic's
own contribution to the centroid is 0.005 to 0.018 photosites, against two to
five times that from ordinary aperture truncation, which has nothing to do with
colour. Binning two by two was measured against it and is worse: it turns a
0.015 px problem into a 0.08 to 0.18 px one, because the cross-trail width of a
star on this rig is about 1.9 photosites and binning pushes that axis below
Nyquist.

### Second moments are measured under a matched window, never over the footprint

The natural estimator — flux times displacement squared, summed over the
threshold footprint — reported the reference session's stars as 4.16 photosites
across where they are 2.07. The footprint is grown on the *filtered* plane, so
it reaches out past where the unsmoothed star has any flux left and its outer
ring is noise; the weight in a second moment is displacement squared, which is
largest exactly there; and dropping the negative half of that noise — which the
natural `value <= 0.0` guard does — leaves a one-sided positive residual at the
worst possible radius.

So each source is weighted by a Gaussian of its own covariance, iterated to a
fixed point. For a Gaussian source under a Gaussian window the measured
covariance is the harmonic combination of the two, so twice the measurement is
the window to try next and the fixed point is the source itself. A fit that does
not converge yields `Moments::NONE` and keeps its position: registration wants
positions, and the frame's shape statistics are better off without a guess.

### A position angle is spin-2 and is never averaged as an angle

An ellipse at 179 degrees and one at 1 degree point almost the same way, and
their arithmetic mean is 90 — perpendicular to both. The reference session's own
trail sits at 94 degrees, close enough to the branch cut of the obvious
implementation to matter. Frame shape is therefore the median of the moment
*matrix*, which is a tensor average and cannot cross a cut, and the consistency
of the direction is a separate number: the length of the mean of
`(cos 2t, sin 2t)`. That separation is what distinguishes a tracking rate error,
which points one way in every frame, from wind, which does not.

## Layout

```
crates/astro-plugin-abi   the contract; depends on nothing
crates/astro-core         plugin host, frame model, and the session:
    session/kind.rs         what a frame is, and what the evidence suggests
    session/compat.rs       whether two frames may be indexed against each other
    session/sets.rs         partitioning, and matching calibration to lights
    session/scan.rs         walking paths into a session
    session/mosaic.rs       which colour a photosite carries, shared by all passes
    calibrate/              masters, pedestal, combination, FITS
    stars/sky.rs            per-colour background and noise on a coarse grid
    stars/shape.rs          moments, trailing, and spin-2 direction arithmetic
    stars/mod.rs            detection, footprints, and windowed measurement
crates/astro-cli          the astro-stacker binary
plugins/astro-format-canon  CR2/CR3, via rawler
```

`astro-core` is the only place in the host that touches the C ABI. Everything
above it works with `OpenFrame` and safe Rust types.

## Plugin discovery

Libraries named `astro_format_*` (plus the platform's `lib` prefix where one
applies) are loaded from, in order:

1. `<exe dir>/plugins`
2. `<exe dir>` — which is what makes `cargo run` work, since cargo drops plugin
   cdylibs next to the binary
3. any `--plugin-dir` given on the command line

The name filter exists so the host never loads an unrelated library that happens
to share a directory. A file that looks like a plugin but fails to load is
reported to the user, never silently skipped: it would otherwise quietly remove
a supported format.

When several plugins can read a file, each returns a confidence from `probe` and
the highest bid wins. `probe` sees the first 4 KiB of the file, read once by the
host, and is expected to check magic numbers rather than trust the extension.

## What is deliberately not built yet

**Pixel statistics.** A `--measure` pass was designed and left out. Decoding 500
frames of 45 megapixels is roughly two minutes and 22 GB of reads, and the
histogram signatures that would veto a misfiled frame have not been run against
a real CR2 or CR3. Shipping an unverified decode path would break this project's
own rule about claiming things work. It is the first thing to add, and the hook
for it is `Suspicion`, which already carries findings a decode would raise.

**Session persistence.** The save-and-reload format belongs in a future
`astro-project` crate, on the line: astro-core models what was shot,
astro-project models what the user is doing about it. `FrameId` and
`FrameFingerprint` are already shaped for it — the fingerprint hashes with
FNV-1a rather than the standard library's hasher precisely because it has to
mean the same thing next year.

**Camera serial numbers.** A dark library is body-specific: the hot-pixel map
belongs to the individual sensor, so matching on make and model alone will
silently accept another body's darks. rawler parses the tag and the ABI does not
carry it. Breaking a frozen contract to add a field, in a milestone that does
not yet calibrate anything, is the wrong trade — so `BodyKey` carries
`serial: Option<String>` set to `None`, and the day the ABI is opened for a
reason that forces it, the change is one line in `BodyKey::from_info` and
nothing that consumes a `BodyKey` moves.

**A colour matrix the user can override.** One of the owner's bodies is a 60D
he modified himself for astrophotography — the IR-cut filter is gone. rawler
identifies it as a stock 60D, correctly, because that is what the firmware
writes into the file, and hands out the stock 60D `xyz_to_cam` matrix. That
matrix no longer describes the sensor: a filter-modified camera has a different
spectral response, most of all in the deep red the modification exists to let
through, and its as-shot white balance is off for the same reason.

Nothing built so far is affected, and that is by design: `xyz_to_cam` and
`wb_coeffs` are reported and never used to match frames, because white balance
and the colour matrix are applied after stacking. But the moment this project
produces a colour image, a per-body override becomes load-bearing rather than a
nicety — a modified camera is the normal case in this field, not an edge case.
The override belongs to the session, not to the plugin: a plugin reports what
the file says, and what the file says about a modified body is stale rather than
wrong.

**Everything downstream.** Calibration and master generation, star detection,
FWHM and trailing analysis, registration, stacking, and the Tauri user
interface. The session model will grow to meet them; the ABI should not have to.
