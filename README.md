# Ordinata

Ordinata is a local linear inverse-problem and uncertainty workbench. It turns a
bounded weighted linear calibration problem into a portable, tamper-checkable evidence
bundle without presenting fitted estimates or synthetic samples as measurements.

Ordinata is open-source software under the [Apache License 2.0](LICENSE).

## Download

Version 0.1.0, the first release, is on the
[releases page](https://github.com/draconitic-labs/ordinata/releases/tag/v0.1.0):

| System | File |
| --- | --- |
| Windows x86_64 | `ordinata-0.1.0-x86_64-pc-windows-gnullvm.zip` |
| Linux x86_64, glibc 2.35 or newer (built on Ubuntu 22.04) | `ordinata-0.1.0-x86_64-unknown-linux-gnu.tar.gz` |

`SHA256SUMS` on the same page lists the checksum of each file. On Linux, check a download
with `sha256sum -c SHA256SUMS --ignore-missing`.

Nothing is installed. On Windows, extract the ZIP and run `ordinata.exe` from the extracted
folder, which also holds the `libunwind.dll` runtime it needs. The executable is not
code-signed, so Windows SmartScreen may ask before the first run.

```powershell
cd ordinata-0.1.0-x86_64-pc-windows-gnullvm
.\ordinata.exe demo --out demo-bundle
.\ordinata.exe verify demo-bundle
```

On Linux:

```sh
tar -xzf ordinata-0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd ordinata-0.1.0-x86_64-unknown-linux-gnu
./ordinata demo --out demo-bundle
./ordinata verify demo-bundle
```

`demo` writes an evidence bundle from built-in synthetic data, and `verify` checks it. The
Windows ZIP also carries the full documentation, the reference fixtures, CycloneDX SBOMs,
and the dependency licence notices; the Linux archive carries the binary, this README,
`LICENSE`, and `THIRD-PARTY-NOTICES.html`.

## What it does

- joins design and observation rows by explicit ID;
- solves weighted linear least squares and diagonal Tikhonov/ridge problems through SVD;
- chooses a declared lambda or a deterministic GCV grid;
- reports rank, conditioning, residuals, effective degrees of freedom, and visible refusals;
- emits analytic and/or seeded residual-bootstrap intervals where scientifically defined;
- writes a self-contained static evidence bundle; and
- verifies directory or ZIP bundles offline without executing their contents.

It is not a nonlinear optimizer, Bayesian engine, PDE/ODE solver, measurement device,
safety authority, or certification tool.

## Command line

```text
ordinata validate <project.json> [--json]
ordinata solve <project.json> --out <new-directory> [--observed-at <RFC3339>] [--json]
ordinata verify <bundle-directory-or-zip> [--json]
ordinata demo --out <new-directory> [--observed-at <RFC3339>]
```

`demo` is permanently labeled synthetic. `verify` proves internal integrity, not
authenticity or scientific correctness; v0.1 bundles are not signatures.

The complete input, numerical, refusal, and bundle contracts are in
[PRODUCT-SPEC.md](PRODUCT-SPEC.md), [docs/MODEL.md](docs/MODEL.md), and
[docs/FILE-FORMATS.md](docs/FILE-FORMATS.md).

## Build and verify from source

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
powershell -ExecutionPolicy Bypass -File packaging/package.ps1
```

The packaging command also runs the advisory and license gates, builds with local-path
remapping, stages and accounts for the pinned Windows runtime, performs clean-room and
extracted-archive smokes under a toolchain-free `PATH`, model-validates reproducible
CycloneDX 1.5 SBOMs, scans the complete distribution for personal or local paths, and
writes deterministic checksums and a deterministic ZIP.

`.github/workflows/release.yml` builds both release packages. On Windows it runs
`packaging/package.ps1`; on Linux it builds on Ubuntu 22.04 with the same path remapping
and scans the package the same way. Each job then runs the packaged program from its
extracted archive outside the checkout. A `v*` tag creates a draft release with both files
and `SHA256SUMS`. See [docs/RELEASING.md](docs/RELEASING.md).

Support: support@draconitic.com
