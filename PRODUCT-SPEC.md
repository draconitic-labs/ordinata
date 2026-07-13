# Ordinata v0.1 product specification

## Product and buyer

**Ordinata — Linear Inference Workbench** is a local-first instrument for researchers and engineers who repeatedly solve small-to-medium weighted linear inverse or calibration problems and need a reviewable result rather than an opaque notebook state.

The paid outcome is one portable evidence bundle containing normalized inputs, solver/version choices, selected regularization, parameter estimates and uncertainty, predictions and residuals, validation checks, declared assumptions and limitations, checksums, and a static human report.

Future launch hypothesis: free verifier and examples; $79 one-time Academic, $249 one-time Lab, and $999 Commercial team license with batch/project templates. This turn keeps source and release private and does not activate billing.

## Strict v0.1 scope

Included:

- weighted linear least squares;
- diagonal Tikhonov/ridge regularization around an optional prior;
- explicit lambda or deterministic generalized cross-validation over a declared log grid;
- rank, singular-value, condition, residual, effective-degrees-of-freedom, and GCV diagnostics;
- analytic covariance/intervals where defined;
- optional seeded residual bootstrap with bounded sample count;
- deterministic evidence bundle generation and offline tamper verification;
- a built-in synthetic demo labeled as synthetic.

Excluded:

- nonlinear models, general constraints, sparse/GPU/distributed solvers, PDE/ODE execution, Bayesian posterior claims, MCMC, learned surrogates, remote data, collaboration, accounts, plugins, arbitrary code, and safety/clinical/financial decisions.

## Input contract

`project.json` uses schema version 1 and contains:

- `title` and optional `description`;
- `designCsv` path relative to the project file;
- `observationsCsv` path relative to the project file;
- `regularization`: `explicit` with non-negative finite `lambda`, or `gcv` with finite positive `minLambda`, `maxLambda`, and 3-200 `steps`; optional prior and positive diagonal penalty vectors keyed by parameter name;
- `uncertainty`: `analytic`, `bootstrap`, or `both`; confidence level 0.5-0.999; bootstrap samples 100-10,000 and explicit unsigned seed;
- non-empty `assumptions` and `limitations` arrays;
- optional source citations as user-supplied labels/URIs/dates only; Ordinata never fetches them;
- optional validation thresholds for weighted RMSE and condition number, reported as user-declared checks rather than universal scientific truth.

`design.csv` has `observation_id` plus 1-256 unique parameter columns. `observations.csv` has `observation_id,value,standard_error`; standard error is finite and positive. Accept 2-50,000 matched rows. UTF-8, finite decimal values, unique IDs, exact one-to-one ID join, and bounded field lengths are mandatory.

Paths resolve beneath the project directory. Reject traversal, absolute paths, links/reparse points escaping the root, devices, and output overlapping an input.

## CLI and library contract

- `ordinata validate <project.json> [--json]`
- `ordinata solve <project.json> --out <new-directory> [--observed-at <RFC3339>] [--json]`
- `ordinata verify <bundle-directory-or-zip> [--json]`
- `ordinata demo --out <new-directory> [--observed-at <RFC3339>]`
- `ordinata --version`

Exit codes: 0 success; 2 input/validation; 3 numerical refusal; 4 I/O/path; 5 bundle integrity; 10 internal invariant. Human diagnostics go to stderr; `--json` stdout is one stable schema object. Never partially overwrite an existing bundle.

Library entry points validate and load a project, solve a normalized problem, build a bundle model, write atomically, and verify an existing bundle without network or process side effects.

## Numerical contract

- Scale rows by inverse standard error and solve with singular value decomposition.
- Explicitly report numerical rank using a versioned tolerance derived from matrix size, largest singular value, and machine epsilon.
- Solve `(A'WA + lambda L'L)x = A'Wb + lambda L'L x_prior` through an augmented SVD formulation; never form an unstable inverse for the estimate.
- GCV candidates are inclusive log-spaced values with stable tie-breaking toward the larger lambda. Report every candidate score.
- Analytic covariance is emitted only when residual degrees of freedom and matrix rank support it; otherwise return a typed, visible warning, not fabricated zero uncertainty.
- Bootstrap resamples standardized residuals with a named ChaCha seed stream, refits using the already-selected lambda, and reports percentile intervals plus failure count. It is an uncertainty aid, not a posterior distribution.
- Canonical numerical JSON rejects non-finite values and uses stable field/key/row/parameter ordering.

## Evidence bundle

- `manifest.json`: bundle/tool/model/schema versions, observed time, normalized configuration, assumptions, limitations, input file hashes, result content hashes, platform/target metadata, and declared synthetic flag.
- `parameters.csv`: name, estimate, standard error where defined, lower/upper interval, prior, penalty.
- `fit.csv`: observation ID, observed value, prediction, residual, standardized residual, standard error.
- `diagnostics.json`: dimensions, rank, singular values, condition number, lambda mode/value, GCV grid, weighted residual statistics, effective degrees of freedom, warnings.
- `validation.json`: named checks, declared threshold/source, observed value, pass/fail/not-applicable, and overall status without hiding failures.
- `report.html`: static escaped summary with provenance, assumptions, limitations, diagnostics, and result tables; no remote assets or JavaScript.
- `SHA256SUMS.txt`: complete payload coverage with safe relative paths.

The manifest's canonical result hash excludes wall-clock `observedAt` so two runs with identical inputs/config/seed have identical numerical content hashes. The archive itself may differ when the user chooses a different observation timestamp, and that distinction must be documented.

## Required evidence

Reference and regression cases:

- exact two-parameter recovery;
- weighted heteroscedastic fit checked against an independently calculated solution;
- explicit ridge with nonzero prior and diagonal penalties;
- rank-deficient system with visible rank and uncertainty refusal;
- ill-conditioned case where declared regularization improves held-out synthetic truth without claiming universal optimality;
- deterministic GCV tie and seeded bootstrap;
- tampered/missing/extra bundle members.

Properties and hostile inputs:

- row permutation invariance and parameter permutation/equivalent renaming;
- scale-consistent weighted solution;
- repeat-run canonical hash equality;
- no panics for generated bounded matrices/configs;
- invalid UTF-8, CSV formula-like labels, HTML characters, duplicate headers/IDs, missing/extra IDs, empty/constant columns, NaN/infinity/overflow, zero/negative error, huge dimensions, traversal/absolute/symlink escape, pre-existing output, and checksum path attacks.

## Repository and release

Rust workspace:

- `crates/ordinata-core`: schema, safe paths, CSV ingestion, normalized model, SVD solver, GCV, uncertainty, canonicalization, bundle/report, verification, typed errors.
- `crates/ordinata-cli`: clap commands, stable JSON/human presentation, exit mapping, atomic orchestration.
- `fixtures/`: synthetic and independently derived reference cases with provenance notes.
- `docs/`: MODEL, VALIDATION, LIMITS, SECURITY, PRIVACY, RELEASING, and file-format documentation.
- `packaging/`: deterministic Windows packaging and package verification/smoke scripts.

Preferred runtime dependencies: `serde`, `serde_json`, `csv`, `sha2`, `nalgebra`, `clap`, `thiserror`, `time`, `rand_chacha`, and `rand_distr`; any addition needs a trust-path note. Dev dependencies may include `proptest`, `assert_cmd`, `predicates`, and `tempfile`.

Release gates include format, strict Clippy, all tests, advisory/license scan, optimized build, clean-room CLI smoke, dependency notices, valid CycloneDX SBOM, complete checksums, archive path safety, personal-name/path/secret scan, and independent fresh-context review. GitHub is private; publish a private review release only after every local gate passes.
