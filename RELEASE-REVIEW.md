# Ordinata v0.1 private release review

Date: 2026-07-14
Disposition: fresh-context GO for private GitHub review; no public release or billing activation.

Reviewed commits:

- Release implementation and deterministic package: `b3d59f3496269c7619e16cb47fe5bd53e27461cb`.
- Cross-platform CI repair: `0609865715adfdb817b7feeb48c55b74740f7660`.

## Reproduced gates

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo test --workspace --locked`: pass, 193 tests including real-binary CLI,
  independent numerical references, properties, hostile inputs, archive attacks,
  tamper ladder, bundle determinism, runtime/scanner regressions, CycloneDX model
  validation, and documentation tests.
- RustSec locked-graph scan: zero known vulnerabilities. One unmaintained transitive
  package warning (`paste` through `nalgebra`/`simba`) is recorded; it is not an affected
  vulnerability and is monitored for a maintained upstream replacement.
- Full unskipped `packaging/package.ps1`: pass with dependency and runtime licence notices,
  model-valid CycloneDX 1.5 SBOMs, clean-room demo/validate/solve/verify, both uncertainty
  methods, one-byte tamper exit 5, overwrite refusal exit 4, complete-distribution
  personal-path scan, deterministic checksums/ZIP, and extracted-archive demo/verify under
  a sanitized customer `PATH`.
- Two package runs from clean checkout locations produced byte-identical executables,
  runtime DLLs, normalized SBOMs, checksum manifests, and ZIPs.

## Reproduced artifacts

- Optimized executable: 1,434,112 bytes; SHA-256
  `46a50f2aab82d7bf352a70fd055b49eb8f48091f77122e016a0d0638a0115c49`.
- Pinned `libunwind.dll`: 90,624 bytes; SHA-256
  `13bf4e99b0193634ebdeb0bbdeff9753b39f1d04799183f664111014aabf4c2c`.
- Normalized CLI SBOM SHA-256:
  `606ffb619bd4944ff12ca355ac4b369a98de16755ae91dba97926990c1e29de7`.
- Normalized core SBOM SHA-256:
  `c575ac1a73acbad4a380d49ec8b458160a7bbd12d3b63f384ed4f1692892ebff`.
- Private review ZIP: 816,300 bytes; SHA-256
  `ecba91bcfb227710cda8b2dd6cf76edc89023fed8721328c2235f23ad626083f`.
- Distribution members: 36, including the product and runtime licences, executable,
  exact runtime DLL and manifest, docs, fixtures, dependency notices, two validated
  CycloneDX SBOMs, and complete checksums.

## Scientific boundary

The reference cases are independently derived rather than copied from Ordinata output.
Verification proves bundle consistency and unchanged bytes; it is not a signature and
does not prove that the declared linear model, uncertainties, assumptions, thresholds,
or conclusions are scientifically appropriate.

## Independent review

- A fresh numerical/security/package review found no reportable security or numerical
  defect and reproduced the complete Windows package twice from differently located
  clean checkouts. Both 36-member archives were byte-identical.
- A separate fresh CI review reproduced the exact Rust 1.96.1 Linux and Windows hosts,
  strict Clippy, and all host-applicable locked tests. It independently downloaded the
  official LLVM-MinGW `20260616` UCRT x86_64 archive, verified the pinned SHA-256 before
  extraction, and proved the gnullvm linker resolved from that verified toolchain.
- All commits and packaged public identity use `Flintglade <support@flintglade.com>`.

## Remaining release boundary

- The private repository push is permitted only to obtain the real GitHub-hosted Ubuntu
  and Windows Actions result. Both jobs must pass before the private review handoff is
  complete.
- Public release, billing, package signing, and checkout remain inactive. A later public
  release requires Authenticode or an explicitly accepted unsigned-release policy.
- The locked graph has zero known vulnerabilities. The allowed unmaintained transitive
  `paste` advisory remains monitored and is not a known affected vulnerability.
