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

## Layout

```
crates/astro-plugin-abi   the contract; depends on nothing
crates/astro-core         plugin host and frame model
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

Frame classification (light/dark/flat/bias), calibration, star detection,
registration, stacking, quality analysis, and the Tauri user interface. The
frame model will grow to meet them; the ABI should not have to.
