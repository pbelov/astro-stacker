# astro-stacker

A stacker for deep-sky astrophotography: calibrate, align and integrate light
frames into one deep image. In the spirit of DeepSkyStacker, but faster, and
built so that no image format is a branch in a switch statement: each is a crate
of its own, and the pipeline asks a registry rather than naming one.

**Status: 0.6.0 — it calibrates.** Point it at a night's folder and it groups
the frames into stackable sets, matches darks, flats and biases to the lights,
names everything that does not fit, and combines the calibration frames into
master frames written as 32-bit FITS. It does not register or integrate the
lights yet.

Verified against real frames from a Canon 5D Mark IV, a modified 60D, an R5 and
an R5 Mark II, and against one complete 370-frame session: all decode at
190–200 Mpx/s, which puts a 45-megapixel CR3 at a quarter of a second, and a
whole night is read and grouped in 0.13 s.

## What is here

| Crate | Role |
|---|---|
| [`astro-core`](crates/astro-core) | The frame model, the format registry, and the session model: classification, grouping and compatibility |
| [`astro-cli`](crates/astro-cli) | `astro-stacker` command line binary |
| [`astro-format-canon`](crates/astro-format-canon) | Canon CR2/CR3 reader, built on [rawler](https://crates.io/crates/rawler) |

## Building

Requires Rust 1.88 or newer. Nothing else — no CMake, no vcpkg, no C toolchain.

```bash
cargo build --release
```

For a release you can hand to someone, `build.bat` runs the tests and clippy,
builds both the command line and the window, and stages a portable folder with
the two of them into `build/`, beside a zip of the same:

```
build.bat
```

The script runs the staged binary and checks that it lists the Canon format
before calling the build done — a binary that built but cannot read a frame
starts, prints its help and looks healthy. What it does not cover is the window
itself: it is a window, so a script cannot ask it anything, and it additionally
wants WebView2 on whatever machine opens it.

Building the window needs Node, since the interface is bundled into it before the
binary is built. The script stages a portable executable rather than an
installer; the installer is the same `tauri build` without `--no-bundle`, and is a
different kind of delivery. `build.bat quick` skips the tests and clippy when you
are only iterating.

## The window

There is a desktop shell beside the command line, for looking at a session
rather than reading it. A staged build carries it as `astro-stacker-desktop.exe`;
from source it runs like this:

```
cd apps/desktop
npm install
npm run tauri dev
```

Tauri 2 and Svelte 5, the same stack and the same palette as the two projects
next to it on disk. It reads a session and shows what the core made of it: the
sets it found, which calibration it matched to the lights and how well, what
differed, and what it refused and why. The steps past that one are not built
yet and say so rather than being absent.

Nothing in the shell decides anything. It opens files, calls the core and
renders the answer; when it needed to know which of four plans was the session,
that rule moved into the core rather than being written a second time.

## Trying it

List the formats it reads:

```bash
cargo run --bin astro-stacker -- formats
```

Describe one frame, and decode it to confirm the pixels really read:

```bash
cargo run --release --bin astro-stacker -- info --decode path/to/IMG_0001.CR3
```

Measure what the pixels say — where each colour sits between black and white,
how much is clipped, and how evenly the frame is lit:

```bash
cargo run --release --bin astro-stacker -- measure path/to/flats/*.CR2
```

Read a whole session:

```bash
cargo run --release --bin astro-stacker -- scan --lights D:/astro/M31/lights --darks D:/astro/M31/darks --flats D:/astro/M31/flats --biases D:/astro/library/bias
```

Combine the calibration frames into masters:

```bash
cargo run --release --bin astro-stacker -- master --lights D:/astro/M31/lights --darks D:/astro/M31/darks --flats D:/astro/M31/flats --biases D:/astro/library/bias -o D:/astro/M31/masters
```

Apply them to a light and look at the result:

```bash
cargo run --release --bin astro-stacker -- calibrate --lights D:/astro/M31/lights --darks D:/astro/M31/darks --flats D:/astro/M31/flats --biases D:/astro/library/bias --skip 100 -o D:/astro/M31/calibrated
```

One light by default, because a calibrated frame is 76 MB and a session is many
gigabytes; integration will calibrate on the way past and write none of them.

Masters are written as 32-bit float FITS in ADU, in sensor readout order, with
the Bayer pattern and what went into them in the header. Below six frames a set
is combined by the median and nothing is rejected; from twenty-five up, by a
mean clipped once at three sigma — a sample sigma from eleven frames is
inflated by the very outlier it would catch, and no threshold can fire through
that.

`--group` puts the paths that follow it into a named group, for a per-night
layout. Calibration named before any `--group` serves every group, which is how
a bias library shot once a year gets reused:

```bash
cargo run --release --bin astro-stacker -- scan --biases D:/astro/library/bias --group mon --lights D:/astro/mon/lights --darks D:/astro/mon/darks --group tue --lights D:/astro/tue/lights
```

## What it will and will not decide for you

A frame's kind comes from you, never from a guess. Folder and file names are
read as *evidence* and shown as proposals, but nothing is ever assigned from
them. The reason is asymmetry: a light, a flat and a bias each have a positive
signature, while a dark has only a negative one — no sky, no stars — which a
light shot under thick cloud shares. A classifier forced to label every frame
guesses exactly where a wrong guess does the most damage, and the damage is
silent. A light filed as a flat divides every frame in the stack by a picture of
the sky, and nothing throws.

What it *will* decide: which frames can be indexed against each other at all,
which calibration set fits best, and what about that fit is worth telling you.

## Design in one paragraph

Each format is a crate implementing one trait, and the applications say which
ones they were built with, so the pipeline never names a format and no format is
a branch inside it. Which decoder gets a file is settled by bidding on its first
few kilobytes rather than by its extension. Paths carry no platform-native
assumptions, which is what will make Linux and macOS support a port rather than
a rewrite. Builds and testing target Windows 11
for now. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the decisions and
why they were made, and [docs/BACKLOG.md](docs/BACKLOG.md) for what is agreed to
be worth doing next.

## Licence

MIT or Apache-2.0, at your option — [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).

The raw decoder it is built on, [rawler](https://github.com/dnglab/dnglab), is
LGPL-2.1, and is the only dependency that is not permissively licensed.
Publishing this source is what lets anyone modify it and rebuild, which is what
that licence asks for. [THIRD-PARTY.md](THIRD-PARTY.md) says so in full, and
travels with every release.
