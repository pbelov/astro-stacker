# Third-party code

astro-stacker itself is MIT or Apache-2.0, at your option — see
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). It is built
from other people's libraries, and this is what they are.

## rawler — LGPL-2.1

The one dependency that is not permissively licensed, and the one this file
exists for.

* Reads the camera raw formats. Used by `crates/astro-format-canon`, and
  compiled into both `astro-stacker.exe` and `astro-stacker-desktop.exe`.
* Licence: **LGPL-2.1**.
* Source: <https://github.com/dnglab/dnglab>, crate `rawler`, version 0.7.2 —
  the exact version, which the workspace pins.

The LGPL asks that whoever receives a program built with the library be able to
replace the library with their own build of it. astro-stacker's own source is
published in full, at <https://github.com/pbelov/astro-stacker>, which is how
that is met here: anyone can modify rawler and rebuild.

## Everything else

Of the 182 packages in `Cargo.lock`, rawler is the only copyleft one. The rest
are permissive — overwhelmingly `MIT OR Apache-2.0`, with a handful of BSD,
Zlib, Unlicense and 0BSD among them, each offered alongside MIT or Apache-2.0
where it is not one of those already. One more, `r-efi`, lists LGPL as one of
three options it offers; it is a lock-file entry for another platform and is not
in a Windows build at all.

To check that rather than take this file's word for it:

```bash
cargo tree --format "{p} {l}"
```

The list is as of version 0.23.0. It is worth re-reading when a dependency is
added, and the only question worth asking each time is whether anything new is
copyleft.
