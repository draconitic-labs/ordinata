# Releasing

## The one command

```powershell
packaging\package.ps1
```

It runs every gate in order and stops at the first thing that is wrong. Nothing in it is
advisory. There is no `--force`.

## The gates

| # | gate | what it is for |
|---|---|---|
| 1 | `cargo fmt --all -- --check` | the diff is about the change, not about whitespace |
| 2 | `cargo clippy --workspace --all-targets --locked -- -D warnings` | strict, and with no blanket `allow` |
| 3 | `cargo test --workspace --locked` | unit, property, reference, hostile-input, tamper, and real-binary CLI |
| 4 | `cargo audit` | the locked graph against the RustSec database |
| 5 | `cargo about` | **fails** on any licence not on the accepted list |
| 6 | `packaging/build-release.ps1` | optimized build **with path remapping**, then the artifact scan |
| 7 | `cargo cyclonedx` | an SBOM, checked to parse as JSON |
| 8 | clean-room smoke | see below |
| 9 | artifact scan over the whole distribution | nothing that ships names the machine that built it |
| 10 | checksums | `SHA256SUMS.txt` over every shipped file, then a zip and its own digest |

## Path remapping, and why the build script exists

A Rust binary embeds the source path of every panic site it can reach, because that is what
`file!()` expands to. For this workspace's own crates Cargo passes relative paths, so those
are harmless. For the **dependencies** it passes wherever the registry was unpacked — which
on a developer's machine is a directory named after that developer.

Before this was fixed, `target/release/ordinata.exe` contained **52 copies** of the
registry path under the building account's home directory — drive letter, user name, and
all. Shipping it would have told every customer who built the binary, from what account,
on what drive.

(The path is described rather than written out here on purpose. `docs/` ships inside the
distribution, and the artifact scan in gate 9 reads *every* file in it — so a document
that spelled out an example absolute path would fail the very gate it is explaining. It
did, once. Keeping the gate strict and rewording the prose was the right way round.)

`--remap-path-prefix` rewrites those paths as rustc records them. It **cannot** live in
`.cargo/config.toml`, because the values are absolute paths that differ on every machine and
Cargo does not interpolate variables into `rustflags`. So it is computed at build time, and
`packaging/build-release.ps1` is the only supported way to produce a release.

What the dependency paths become is:

```
/cargo\registry\src\index.crates.io-1949cf8c6b5b557f\nalgebra-0.34.1\src\linalg\svd.rs
```

That is deliberate rather than merely tolerable. The index hash is a public constant, and the
crate name and version are public facts, so a panic message still says exactly which line of
which released crate it came from — while naming no user, no drive, and no directory that
exists anywhere.

**The gate that proves it:** `crates/ordinata-cli/tests/artifact.rs`. It computes its markers
from the environment it runs in — this account's name, this home directory, this Cargo home,
this checkout — rather than hard-coding anybody's name into the repository, which would be
self-defeating. It also detects drive-letter absolute paths structurally.

It was verified to fail. Rebuilding without the remapping produced **58 findings** across
every marker class: the account name, the home directory, the Cargo registry path, the
structural home-and-registry markers, and the drive-letter path detector. A gate that
cannot fail is not a gate.

The release script sets `ORDINATA_SCAN_REQUIRED=1`, which turns "there was nothing to scan"
into a failure. The release path cannot take the skip.

## The clean-room smoke

Gates 1–7 all run inside the source tree, with the toolchain, against fixtures the binary has
seen. A release that passes all of them and still does not work on a customer's machine is
entirely possible. Gate 8 is what catches that.

It copies the executable out of the tree, changes to a scratch directory outside the
repository, writes inputs the binary has never seen, and then:

- `--version`;
- `demo`, and verifies the bundle it wrote;
- `validate` on the fresh project;
- `solve` on it, at a pinned `--observed-at`;
- `verify` on the result — must pass;
- checks that `parameters.csv` really carries `analytic_*` **and** `bootstrap_*` columns, since
  the project asked for both;
- **tampers with one byte** of `fit.csv` and re-verifies — must exit **5**;
- **solves over the existing bundle** — must exit **4**.

The last two are the ones that matter most. A verifier that passes a tampered bundle, or a
solve that overwrites somebody's evidence, is not a bug to be found by a customer.

## What ships

```
ordinata-0.1.0-<target-triple>/
  ordinata.exe
  LICENSE
  README.md
  PRODUCT-SPEC.md
  docs/            MODEL, LIMITS, SECURITY, PRIVACY, VALIDATION, FILE-FORMATS, RELEASING
  fixtures/        every reference case, with its hand derivation
  THIRD-PARTY-NOTICES.html
  *.cdx.json       CycloneDX SBOM
  SHA256SUMS.txt
```

plus the zip and its `.sha256`.

`<target-triple>` is read from the active pinned Rust toolchain at build time; the
directory name therefore describes the binary that was actually produced rather than
assuming a particular Windows runtime.

The fixtures ship on purpose. A buyer who wants to know whether to believe the solver can run
it against cases whose answers were worked out by hand, on paper, in `PROVENANCE.md`.

## Version discipline

- **`spec::MODEL_VERSION`** is bumped whenever any rule or threshold in `spec.rs` changes a
  number Ordinata reports. A bundle carries it, so a reader can tell whether two bundles were
  made by the same rules.
- The **golden content hash** in `tests/bundle.rs` moves when the numbers, the canonical form,
  or the bundle layout change. Moving it is fine when that is what was meant, and it must be
  moved together with `MODEL_VERSION`. What it catches is the change nobody meant: a refactor
  that quietly alters a number, a dependency that reorders a map, a formatting routine that
  starts rounding. Any of those would silently invalidate every bundle ever written.
- **The pinned bootstrap stream** in `numeric.rs` may only be updated together with
  `MODEL_VERSION`. Changing it moves every bootstrap interval Ordinata has ever published.

## This release

**Private.** GitHub stays private, nothing is published, sales are not active, and billing is
not switched on. The distribution is a review artifact.

See [RELEASE-REVIEW.md](../RELEASE-REVIEW.md) for the evidence from the most recent run, the
exact commands, the artifact hashes, and the holds that remain.
