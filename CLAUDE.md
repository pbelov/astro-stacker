# Working on astro-stacker

A DeepSkyStacker alternative: faster and modern. Read
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) before changing anything
structural — it records which decisions are already settled and why.

## Versioning and commits

Every change ships as a commit with a version bump. The scheme is the owner's,
not plain semver:

| Change | Bump | Example |
|---|---|---|
| Small fix, refactor, docs | patch | `0.1.3` → `0.1.4` |
| New feature | minor | `0.1.4` → `0.2.0` |
| Major version | **only when the owner says so** | never bump to `1.0.0` on your own |

Bump `workspace.package.version` in the root `Cargo.toml`; every crate inherits
it. Run `cargo build` afterwards so `Cargo.lock` is updated in the same commit.
Tag releases `v<version>`.

## Before every commit

```bash
cargo test && cargo clippy --all-targets
```

Both must be clean. Clippy warnings are not tolerated in committed code.

## Formats are crates, and the core names none of them

A decoder is a crate under `crates/`, named `astro-format-<something>`,
implementing `Format` and `Frame` from `astro-core::format`. The applications
say which decoders they were built with — `formats()` in the CLI, `load_formats`
in the window — and `astro-core` asks the registry rather than naming one.

That last part is the rule worth keeping: no format may become a branch inside
the pipeline. Adding one is a crate beside the others and a line where they are
assembled.

A decoder is named for what it has been run against, not for what its library
could in principle read. `rawler` reads some 1800 cameras; `astro-format-canon`
declares `cr2` and `cr3` because those are what real frames from real bodies
have been decoded and stacked. Claiming a format nobody has tried is claiming it
works.

## Conventions

* Comments explain *why*, not *what*. Every `unsafe` block carries a `SAFETY:`
  comment naming the invariant it relies on.
* Missing data is `None` or `NaN`, never a plausible-looking default. A frame
  whose sensor temperature was not recorded must not report 20 °C.
* Tests assert behaviour that matters to stacking — that a night of subframes
  stays ordered, that a buffer size cannot overflow — not implementation detail.

## Verification needs real frames

Unit tests cover the pure logic. Anything touching a decoder has to be run
against actual CR2/CR3 files. Four are in `testdata/`, one from each of the
owner's bodies (5D Mark IV, 60Da, R5, R5 Mark II); the directory is gitignored,
so the files are on this machine only.

```bash
cargo run --release --bin astro-stacker -- info --decode <file>
```

Do not claim a decode path works without having run it on a real frame.
