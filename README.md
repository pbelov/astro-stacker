# astro-stacker

A stacker for deep-sky astrophotography: calibrate, align and integrate light
frames into one deep image. In the spirit of DeepSkyStacker, but faster, and
built so that every image format is a plugin rather than a branch in a switch
statement.

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
| [`astro-plugin-abi`](crates/astro-plugin-abi) | The frozen C ABI between host and plugins, plus a safe wrapper for writing them |
| [`astro-core`](crates/astro-core) | Plugin discovery, frame reading, and the session model: classification, grouping and compatibility |
| [`astro-cli`](crates/astro-cli) | `astro-stacker` command line binary |
| [`astro-format-canon`](plugins/astro-format-canon) | Canon CR2/CR3 reader, built on [rawler](https://crates.io/crates/rawler) |

## Building

Requires Rust 1.88 or newer. Nothing else — no CMake, no vcpkg, no C toolchain.

```bash
cargo build --release
```

## Trying it

List the format plugins that loaded:

```bash
cargo run --bin astro-stacker -- plugins
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

Formats are dynamic libraries discovered at runtime and reached through a
`#[repr(C)]` ABI — not Rust traits — so a plugin can be built by a different
compiler, or written in another language, and keep working. Strings are UTF-8
and paths carry no platform-native encoding, which is what will make Linux and
macOS support a port rather than a rewrite. Builds and testing target Windows 11
for now. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the decisions and
why they were made.

## Licence

MIT or Apache-2.0, at your option.
