# Ordinata agent contract

## Observable finish line

- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- `cargo test --workspace --locked` passes unit, integration, property, hostile-input, numerical-reference, bundle-tamper, and real-binary CLI tests.
- `cargo audit` reports no known affected package.
- A release build completes from a clean checkout and the packaged binary passes `demo`, `solve`, and `verify` outside the source tree.
- The release archive, checksums, dependency notices, SBOM, and verification report agree byte-for-byte.
- A fresh-context review finds no release blocker.

## Product boundaries

- Ordinata is a local linear inverse-problem instrument. It is not a universal simulator, nonlinear optimizer, Bayesian engine, PDE solver, controller, measurement device, safety authority, or certification tool.
- Simulation, fitted estimates, and bootstrap samples must never be called measurements.
- No runtime network, telemetry, account, external process, plugin execution, script evaluation, or secret-bearing configuration.
- Public identity is Ordinata, with Flintglade/support@flintglade.com only in restrained publisher/support metadata.

## Engineering constraints

- Rust stable, `#![forbid(unsafe_code)]`, workspace `core` + `cli`, small audited dependency budget.
- The numerical core, canonicalization, evidence hashing, and bundle verification are library APIs with no CLI coupling.
- Join observation rows by explicit ID; never rely on accidental file order.
- Reject NaN, infinity, duplicate IDs/headers, dimension mismatch, invalid uncertainty, unsafe output paths, oversized work, and ill-defined degrees of freedom with typed errors.
- Every solver choice and threshold is versioned and present in the output bundle.
- Determinism includes stable ordering and seeded bootstrap output. Wall-clock observation time must not affect numerical results or canonical content hashes.
