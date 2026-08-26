# astro-stacker

A stacker for deep-sky astrophotography: calibrate, align and integrate light
frames into one deep image. In the spirit of DeepSkyStacker, but faster, and
built so that every image format is a plugin rather than a branch in a switch
statement.

**Status: 0.0.1 — foundation.** The plugin boundary works end to end, and
Canon CR2/CR3 frames can be described and decoded through it. There is no
stacking yet.

## What is here

| Crate | Role |
|---|---|
| [`astro-plugin-abi`](crates/astro-plugin-abi) | The frozen C ABI between host and plugins, plus a safe wrapper for writing them |
| [`astro-core`](crates/astro-core) | Plugin discovery, loading, and reading frames |
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

Describe a frame, and decode it to confirm the pixels really read:

```bash
cargo run --release --bin astro-stacker -- info --decode path/to/IMG_0001.CR3
```

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
