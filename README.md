# Ordinata

Ordinata is a local linear inverse-problem and uncertainty workbench. It turns a
bounded weighted linear calibration problem into a portable, tamper-checkable evidence
bundle without presenting fitted estimates or synthetic samples as measurements.

This repository contains the private v0.1 release candidate. Sales and public
distribution are not active.

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

## Verify the source and release

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
powershell -ExecutionPolicy Bypass -File packaging/package.ps1
```

The packaging command also runs the advisory and license gates, builds with local-path
remapping, performs a clean-room solve/verify/tamper smoke, generates SBOMs and notices,
scans the complete distribution for personal or local paths, and writes checksums.

Support: support@flintglade.com
