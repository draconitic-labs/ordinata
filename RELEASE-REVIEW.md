# Ordinata v0.1 private release review

Date: 2026-07-14
Disposition: pending fresh-context review; no public release or billing activation.

## Reproduced gates

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: pass, 185 tests including real-binary CLI,
  independent numerical references, properties, hostile inputs, archive attacks,
  tamper ladder, bundle determinism, and documentation tests.
- RustSec locked-graph scan: zero known vulnerabilities. One unmaintained transitive
  package warning (`paste` through `nalgebra`/`simba`) is recorded; it is not an affected
  vulnerability and is monitored for a maintained upstream replacement.
- Full `packaging/package.ps1`: pass with dependency-license notices, CycloneDX SBOMs,
  clean-room demo/validate/solve/verify, both uncertainty methods, one-byte tamper exit 5,
  overwrite refusal exit 4, complete-distribution personal-path scan, and checksums.

## Reproduced artifacts

- Optimized executable: 1,435,648 bytes.
- Executable SHA-256: `dd67d284a8862b1f3c7174074197a2db993e9a08b9087c1bb7d43cfe18150977`.
- Private review ZIP SHA-256: `44205a75acfafc5ed24b967628b77c82235d7e4d67ca8ba4917352a5f33150cb`.
- Distribution members: 32, including the product license, runtime executable, docs,
  fixtures, dependency notices, two CycloneDX SBOMs, and complete checksums.

## Scientific boundary

The reference cases are independently derived rather than copied from Ordinata output.
Verification proves bundle consistency and unchanged bytes; it is not a signature and
does not prove that the declared linear model, uncertainties, assumptions, thresholds,
or conclusions are scientifically appropriate.

## Remaining hold

- Obtain a fresh-context numerical/security review of the exact committed candidate.
- Push only to the private Flintglade repository after both checks pass.
