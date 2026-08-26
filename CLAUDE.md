# Working on astro-stacker

A DeepSkyStacker alternative: faster, modern, plugin-based. Read
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

## The plugin ABI is load-bearing

`crates/astro-plugin-abi/src/abi.rs` is a frozen contract. Changing a struct or
signature breaks every installed plugin, so:

* Bump `ABI_VERSION` **and** update the size assertions in
  `crates/astro-plugin-abi/src/lib.rs` in the same change.
* Never let a Rust type cross the boundary — no `String`, `Vec`, `Option`,
  `Result`, or data-carrying enums.
* Never free memory allocated on the other side.
* Never let a panic unwind across it.

New format support means a new crate under `plugins/`, named
`astro-format-<vendor>`, built as a `cdylib`, implementing `FormatPlugin` and
invoking `export_plugin!`. It must not be a branch added to an existing plugin.

## Conventions

* Comments explain *why*, not *what*. Every `unsafe` block carries a `SAFETY:`
  comment naming the invariant it relies on.
* Missing data is `None` or `NaN`, never a plausible-looking default. A frame
  whose sensor temperature was not recorded must not report 20 °C.
* Tests assert behaviour that matters to stacking — that a night of subframes
  stays ordered, that a buffer size cannot overflow — not implementation detail.

## Verification needs real frames

Unit tests cover the pure logic. Anything touching a decoder has to be run
against actual CR2/CR3 files, which are not in the repository:

```bash
cargo run --release --bin astro-stacker -- info --decode <file>
```

Do not claim a decode path works without having run it on a real frame.
