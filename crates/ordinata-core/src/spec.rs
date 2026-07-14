//! Versioned solver specification: every limit, threshold, and rule that can change
//! a number Ordinata reports lives here and is copied verbatim into every bundle.
//!
//! Nothing in this module may be changed without bumping [`MODEL_VERSION`]. A bundle
//! carries these values so that a reader who no longer has this source can still see
//! exactly which rules produced the result.

use serde_json::json;

/// Public product name. Never a personal name.
pub const TOOL_NAME: &str = "ordinata";

/// Publisher shown in bundle provenance and packaging metadata.
pub const PUBLISHER: &str = "Flintglade";

/// Support contact shown in bundle provenance and packaging metadata.
pub const SUPPORT_CONTACT: &str = "support@flintglade.com";

/// Tool version, from the crate manifest.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Schema version accepted in `project.json`.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// Schema version written into every bundle member that carries one.
pub const BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Numerical model version. Bump whenever any rule below changes a reported number.
pub const MODEL_VERSION: u32 = 1;

/// Schema version of the CLI `--json` object.
pub const CLI_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Size limits. These bound the work Ordinata will accept, so that a hostile or
// mistaken input is refused rather than exhausting the machine.
// ---------------------------------------------------------------------------

/// Smallest accepted number of matched observation rows.
pub const MIN_OBSERVATIONS: usize = 2;
/// Largest accepted number of matched observation rows.
pub const MAX_OBSERVATIONS: usize = 50_000;
/// Smallest accepted number of parameter columns.
pub const MIN_PARAMETERS: usize = 1;
/// Largest accepted number of parameter columns.
pub const MAX_PARAMETERS: usize = 256;

/// Largest accepted `project.json`, in bytes.
pub const MAX_PROJECT_BYTES: u64 = 1 << 20; // 1 MiB
/// Largest accepted CSV input, in bytes.
pub const MAX_CSV_BYTES: u64 = 128 << 20; // 128 MiB

/// Longest accepted observation ID or parameter name, in bytes.
pub const MAX_IDENTIFIER_BYTES: usize = 64;
/// Longest accepted single CSV field, in bytes.
pub const MAX_FIELD_BYTES: usize = 256;
/// Longest accepted project title, in bytes.
pub const MAX_TITLE_BYTES: usize = 200;
/// Longest accepted free-text entry (description, assumption, limitation), in bytes.
pub const MAX_TEXT_BYTES: usize = 2_000;
/// Largest accepted number of assumption or limitation entries.
pub const MAX_TEXT_ITEMS: usize = 64;
/// Largest accepted number of source citations.
pub const MAX_SOURCES: usize = 64;

/// Smallest accepted GCV grid size.
pub const MIN_GCV_STEPS: usize = 3;
/// Largest accepted GCV grid size.
pub const MAX_GCV_STEPS: usize = 200;

/// Smallest accepted bootstrap sample count.
pub const MIN_BOOTSTRAP_SAMPLES: usize = 100;
/// Largest accepted bootstrap sample count.
pub const MAX_BOOTSTRAP_SAMPLES: usize = 10_000;

/// Smallest accepted confidence level.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// Largest accepted confidence level.
pub const MAX_CONFIDENCE: f64 = 0.999;

/// Work budget for the bootstrap, counted as `rows * retained_components * samples`.
///
/// One unit is one multiply-add in the projection through the retained left singular
/// vectors, which is exactly what one bootstrap refit costs. The budget is a
/// wall-clock ceiling wearing a portable unit: on the machine this was calibrated on
/// an optimized build sustains about 6.3e8 units per second, so this ceiling is
/// roughly three seconds of work there and well under a minute on a machine several
/// times slower.
///
/// It is deliberately set so that *every* problem inside the declared dimension
/// limits can still afford [`MIN_BOOTSTRAP_SAMPLES`]; a contract that admitted a
/// problem size and then made its minimum bootstrap unaffordable would be a contract
/// with a hole in it. `budgets_admit_the_largest_declared_problem` pins that.
///
/// A request above the budget is refused with the largest admissible sample count,
/// never silently capped.
pub const MAX_BOOTSTRAP_WORK: u128 = 2_000_000_000;

/// Peak working-set budget for the matrices the solve holds at once, in bytes.
///
/// See [`solve_bytes_estimate`] for what is counted. The estimate is checked against
/// this before the first large allocation, so an oversized problem is a typed refusal
/// rather than an allocator failure or a swapping machine.
///
/// The largest problem the dimension limits admit (50,000 x 256) estimates at about
/// 513 MiB, so this budget refuses nothing that the input contract accepts. That is
/// the point: it is the guard rail that any future widening of the dimension limits
/// has to get past, not a second, stricter limit hiding behind the first.
pub const MAX_SOLVE_BYTES: u64 = 1 << 30; // 1 GiB

/// How the joint matrix and decomposition working set is estimated, stated so a
/// reader can check the arithmetic against their own machine.
pub const SOLVE_MEMORY_RULE: &str = "bytes = 8 * (3*rows*parameters + 2*rows*k + 2*parameters*k + k), where k = min(rows, parameters); the three row-by-parameter terms are the ingested design, its row-scaled copy, and the penalty-scaled copy the decomposition consumes, and the k terms are the left and right singular vectors before and after they are sorted";

/// The joint matrix and decomposition working set a problem of this shape needs, in
/// bytes, by [`SOLVE_MEMORY_RULE`].
///
/// This over-counts rather than under-counts: it assumes every retained component is
/// kept, which is only true at full rank. Over-counting is the safe direction, because
/// the estimate is what a refusal is based on.
///
/// Saturating arithmetic throughout, so that a hypothetical caller passing absurd
/// dimensions gets [`u64::MAX`] and a refusal, not a wrapped-around pass.
pub fn solve_bytes_estimate(rows: usize, parameters: usize) -> u64 {
    let m = rows as u64;
    let n = parameters as u64;
    let k = m.min(n);
    let elements = m
        .saturating_mul(n)
        .saturating_mul(3)
        .saturating_add(m.saturating_mul(k).saturating_mul(2))
        .saturating_add(n.saturating_mul(k).saturating_mul(2))
        .saturating_add(k);
    elements.saturating_mul(std::mem::size_of::<f64>() as u64)
}

/// Longest accepted filesystem path handed to Ordinata, in bytes.
pub const MAX_PATH_BYTES: usize = 4_096;
/// Largest accepted number of components in a relative path.
pub const MAX_PATH_COMPONENTS: usize = 32;

/// Largest accepted number of members in a bundle.
pub const MAX_BUNDLE_MEMBERS: usize = 64;
/// Largest accepted size of a single bundle member, in bytes.
pub const MAX_BUNDLE_MEMBER_BYTES: u64 = 256 << 20; // 256 MiB
/// Largest accepted total uncompressed size of a bundle, in bytes.
pub const MAX_BUNDLE_BYTES: u64 = 512 << 20; // 512 MiB
/// Largest accepted archive file, in bytes, before any decompression is attempted.
pub const MAX_ARCHIVE_BYTES: u64 = 256 << 20; // 256 MiB

// ---------------------------------------------------------------------------
// Numerical thresholds.
// ---------------------------------------------------------------------------

/// Human-readable statement of the numerical rank rule.
pub const RANK_TOLERANCE_RULE: &str = "sigma_i > max(rows, parameters) * sigma_max * f64::EPSILON";

/// When a condition number exists at all, and what it is measured over.
///
/// The subtlety this rule exists to pin down: a thin decomposition of an `m x n`
/// design produces only `min(m, n)` singular values, so when `m < n` the smallest
/// singular value it reports is *not* the smallest singular value the parameter space
/// has. The missing ones are zero. Comparing the rank to the number of singular values
/// that came back would therefore call a 2x3 design "full rank" and hand out a finite
/// condition number for a system that does not determine its parameters at all.
///
/// The rank is compared to the number of *parameters*, which is the only comparison
/// that answers the question a condition number is asked.
pub const CONDITION_NUMBER_RULE: &str = "condition = sigma_max / sigma_min, reported only when the numerical rank equals the number of parameter columns; a design whose rank is below its parameter count has a zero singular value in the directions the data cannot see, so its condition number is not finite and is reported as null rather than as the ratio of the singular values that happen to have been computed";

/// The SVD routine used, named so a reader can reproduce it.
pub const SVD_RULE: &str = "nalgebra::linalg::SVD (Golub-Reinsch), thin U, components sorted by descending singular value with index tie-break";

/// How the solution is formed once the SVD is known.
pub const SOLUTION_RULE: &str = "x = x_prior + L^-1 * V * diag(sigma_i / (sigma_i^2 + lambda)) * U^T * (b_scaled - A_scaled * x_prior), with components at or below the rank tolerance set to zero";

/// The generalized cross-validation score.
pub const GCV_RULE: &str =
    "score(lambda) = rows * weighted_rss(lambda) / (rows - effective_dof(lambda))^2";

/// How ties in the GCV grid are resolved.
pub const GCV_TIE_RULE: &str =
    "candidates are scanned in ascending lambda and the last strict-or-equal minimum wins, so an exact tie resolves toward the larger lambda";

/// How the log-spaced GCV grid is laid out.
pub const GCV_GRID_RULE: &str =
    "steps values with the exact declared endpoints; interior points are exp-interpolated in natural log space";

/// A fit whose residual degrees of freedom fall below `rows * this` is treated as
/// having none at all.
///
/// One threshold governs two decisions, because they are the same question asked
/// twice. A GCV candidate whose denominator `(rows - effective_dof)` has collapsed
/// has no usable score. A fit whose residual degrees of freedom have collapsed has
/// used up the data reproducing itself, and any interval drawn from it would be a
/// statement about arithmetic rather than about the world. Both are reported, never
/// papered over.
pub const RESIDUAL_DOF_FRACTION_FLOOR: f64 = 1e-9;

/// A row whose `1 - leverage` falls at or below this value has an undefined
/// standardized residual, because the fit passes exactly through it.
pub const LEVERAGE_FLOOR: f64 = 1e-8;

/// If the reduced chi-square falls at or below this value the empirical residuals
/// carry no usable spread and a residual bootstrap would manufacture zero width.
pub const DEGENERATE_RESIDUAL_REDUCED_CHI_SQUARE: f64 = 1e-6;

/// Below this reduced chi-square the declared standard errors look too large for the
/// observed residuals.
pub const RESIDUAL_SCALE_WARN_LOW: f64 = 0.1;

/// Above this reduced chi-square the declared standard errors look too small for the
/// observed residuals.
pub const RESIDUAL_SCALE_WARN_HIGH: f64 = 10.0;

/// Above this condition number the scaled design is reported as ill conditioned.
pub const ILL_CONDITIONED_WARN: f64 = 1e8;

/// Domain separation label for the bootstrap seed stream.
pub const BOOTSTRAP_SEED_DOMAIN: &str = "ordinata/bootstrap/v1";

/// How a raw seed becomes a `ChaCha20` key.
pub const BOOTSTRAP_SEED_RULE: &str = "chacha20_seed = SHA-256(\"ordinata/bootstrap/v1\" || seed.to_le_bytes()); indices are drawn from next_u64() by rejection sampling against the largest multiple of the row count";

/// How percentile interval bounds are read off the sorted bootstrap samples.
pub const PERCENTILE_RULE: &str =
    "linear interpolation at rank (n - 1) * p on the ascending sample (Hyndman-Fan type 7)";

/// How analytic intervals are formed.
pub const ANALYTIC_INTERVAL_RULE: &str = "estimate +/- z * standard_error, where z is the standard normal quantile at (1 + confidence) / 2 and the covariance is G G^T for the linear estimator G; the declared standard errors set the noise scale, which is not re-estimated from the residuals";

/// Canonical ordering rule for the normalized problem.
pub const CANONICAL_ORDER_RULE: &str = "observation rows ascending by observation_id and parameter columns ascending by name, both by UTF-8 byte order";

/// Canonical JSON rule used for every content hash.
pub const CANONICAL_JSON_RULE: &str = "UTF-8 JSON with object keys ascending by UTF-8 byte order, no insignificant whitespace, no non-finite numbers, and negative zero normalized to zero";

/// The full versioned specification, as it is embedded in every bundle manifest.
///
/// A reader with only the bundle can recover every rule and threshold that shaped
/// the numbers, without access to this source tree.
pub fn manifest_block() -> serde_json::Value {
    json!({
        "modelVersion": MODEL_VERSION,
        "rules": {
            "canonicalOrder": CANONICAL_ORDER_RULE,
            "canonicalJson": CANONICAL_JSON_RULE,
            "svd": SVD_RULE,
            "rankTolerance": RANK_TOLERANCE_RULE,
            "conditionNumber": CONDITION_NUMBER_RULE,
            "solution": SOLUTION_RULE,
            "solveMemory": SOLVE_MEMORY_RULE,
            "gcvScore": GCV_RULE,
            "gcvGrid": GCV_GRID_RULE,
            "gcvTieBreak": GCV_TIE_RULE,
            "analyticInterval": ANALYTIC_INTERVAL_RULE,
            "bootstrapSeed": BOOTSTRAP_SEED_RULE,
            "bootstrapPercentile": PERCENTILE_RULE,
        },
        "thresholds": {
            "residualDegreesOfFreedomFractionFloor": RESIDUAL_DOF_FRACTION_FLOOR,
            "leverageFloor": LEVERAGE_FLOOR,
            "degenerateResidualReducedChiSquare": DEGENERATE_RESIDUAL_REDUCED_CHI_SQUARE,
            "residualScaleWarnLow": RESIDUAL_SCALE_WARN_LOW,
            "residualScaleWarnHigh": RESIDUAL_SCALE_WARN_HIGH,
            "illConditionedWarn": ILL_CONDITIONED_WARN,
        },
        "limits": {
            "minObservations": MIN_OBSERVATIONS,
            "maxObservations": MAX_OBSERVATIONS,
            "minParameters": MIN_PARAMETERS,
            "maxParameters": MAX_PARAMETERS,
            "minGcvSteps": MIN_GCV_STEPS,
            "maxGcvSteps": MAX_GCV_STEPS,
            "minBootstrapSamples": MIN_BOOTSTRAP_SAMPLES,
            "maxBootstrapSamples": MAX_BOOTSTRAP_SAMPLES,
            "minConfidence": MIN_CONFIDENCE,
            "maxConfidence": MAX_CONFIDENCE,
            "maxBootstrapWork": MAX_BOOTSTRAP_WORK.to_string(),
            "maxSolveBytes": MAX_SOLVE_BYTES.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    #[test]
    fn budgets_admit_the_largest_declared_problem() {
        // The input contract promises to accept 50,000 rows and 256 parameters. If a
        // resource budget refused that shape, the two halves of the contract would
        // disagree and a user would meet a limit the documentation never mentioned.
        // These are the assertions that keep the promise and the budget in step: raise
        // a dimension limit without raising the budget and this fails here, in a test,
        // rather than in a refusal on somebody's data.
        let bytes = solve_bytes_estimate(MAX_OBSERVATIONS, MAX_PARAMETERS);
        assert!(
            bytes <= MAX_SOLVE_BYTES,
            "the largest admissible problem estimates at {bytes} bytes, over the {MAX_SOLVE_BYTES} budget"
        );

        let work = (MAX_OBSERVATIONS as u128)
            * (MAX_PARAMETERS.min(MAX_OBSERVATIONS) as u128)
            * (MIN_BOOTSTRAP_SAMPLES as u128);
        assert!(
            work <= MAX_BOOTSTRAP_WORK,
            "the largest admissible problem cannot afford even the smallest bootstrap: {work} units against a {MAX_BOOTSTRAP_WORK} budget"
        );
    }

    #[test]
    fn the_memory_estimate_grows_with_the_problem_and_never_wraps() {
        assert!(solve_bytes_estimate(100, 5) < solve_bytes_estimate(1_000, 5));
        assert!(solve_bytes_estimate(100, 5) < solve_bytes_estimate(100, 50));
        // A caller who asks for something absurd gets a saturated answer, which is
        // over the budget, which is a refusal. It must never wrap to a small number
        // and be waved through.
        assert_eq!(solve_bytes_estimate(usize::MAX, usize::MAX), u64::MAX);
        assert!(solve_bytes_estimate(usize::MAX, 256) > MAX_SOLVE_BYTES);
    }
}
