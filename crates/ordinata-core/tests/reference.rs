//! Numerical reference and regression cases.
//!
//! Every expected value in this file comes from somewhere other than Ordinata. The
//! small cases are solved by hand in the matching `fixtures/*/PROVENANCE.md`, using
//! the normal equations and Cramer's rule -- a route the solver never takes, since it
//! goes through a singular value decomposition and never forms `A'WA` at all. If the
//! two disagree, one is wrong, and the test does not assume which.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;

use std::path::{Path, PathBuf};

use ordinata_core::solve::UncertaintyStatus;
use ordinata_core::{load_project, solve, Problem, Solution};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .join("project.json")
}

fn load(name: &str) -> (Problem, Solution) {
    let problem = load_project(&fixture(name)).expect("fixture should load");
    let solution = solve(&problem).expect("fixture should solve");
    (problem, solution)
}

/// Assert that `got` is `expected`, to a relative tolerance.
fn close(got: f64, expected: f64, tolerance: f64, what: &str) {
    let scale = expected.abs().max(1.0);
    let error = (got - expected).abs() / scale;
    assert!(
        error <= tolerance,
        "{what}: got {got:.17}, expected {expected:.17}, relative error {error:e} exceeds {tolerance:e}"
    );
}

/// Exactly equal, bit for bit.
///
/// Used where the claim really is exactness rather than closeness: a declared lambda is
/// the number the project wrote down, and a grid endpoint is the declared value
/// *copied* rather than an `exp(ln(x))` round trip of it. A tolerance here would let a
/// lossy round trip pass unnoticed, which is the one thing these assertions exist to
/// catch. Everything that is genuinely approximate goes through `close` instead.
#[track_caller]
fn exactly(got: f64, expected: f64) {
    assert_eq!(
        got.to_bits(),
        expected.to_bits(),
        "got {got:e}, expected exactly {expected:e}"
    );
}

fn index_of(problem: &Problem, name: &str) -> usize {
    problem
        .parameters
        .iter()
        .position(|p| p == name)
        .unwrap_or_else(|| panic!("no parameter named {name}"))
}

// ---------------------------------------------------------------------------
// 1. Exact recovery.
// ---------------------------------------------------------------------------

#[test]
fn exact_two_parameter_recovery() {
    let (problem, solution) = load("exact-recovery");

    let intercept = index_of(&problem, "intercept");
    let slope = index_of(&problem, "slope");

    // The data lies exactly on 3 + 2x. Anything but 3 and 2 is a bug.
    close(solution.estimates[intercept], 3.0, 1e-12, "intercept");
    close(solution.estimates[slope], 2.0, 1e-12, "slope");

    // The covariance is the textbook (A'A)^-1 = [[0.6, -0.2], [-0.2, 0.1]].
    close(
        solution.analytic.standard_errors[intercept],
        0.6_f64.sqrt(),
        1e-12,
        "se(intercept)",
    );
    close(
        solution.analytic.standard_errors[slope],
        0.1_f64.sqrt(),
        1e-12,
        "se(slope)",
    );

    assert_eq!(solution.rank, 2);
    close(solution.effective_dof, 2.0, 1e-12, "effective dof");
    close(solution.residual_dof, 3.0, 1e-12, "residual dof");
    assert!(
        solution.weighted_rss < 1e-24,
        "a perfect fit must leave no residual: {}",
        solution.weighted_rss
    );
}

#[test]
fn a_perfect_fit_is_reported_as_evidence_against_the_declared_errors() {
    // Residuals of exactly zero are not what standard errors of 1 predict. A tool
    // that printed "excellent fit" here would be flattering the user. Ordinata warns.
    let (_, solution) = load("exact-recovery");
    assert!(
        solution
            .warnings
            .iter()
            .any(|w| w.code == "residual_scale_inconsistent_with_declared_errors"),
        "expected a residual-scale warning, got {:?}",
        solution.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 2. Weighted heteroscedastic fit against a hand solution.
// ---------------------------------------------------------------------------

#[test]
fn weighted_fit_matches_an_independently_derived_solution() {
    let (problem, solution) = load("weighted-heteroscedastic");

    let intercept = index_of(&problem, "intercept");
    let slope = index_of(&problem, "slope");

    // Derived by hand in fixtures/weighted-heteroscedastic/PROVENANCE.md.
    close(
        solution.estimates[intercept],
        112.0 / 89.0,
        1e-12,
        "intercept",
    );
    close(solution.estimates[slope], 103.0 / 89.0, 1e-12, "slope");

    // Covariance (A'WA)^-1 = [[68/89, -36/89], [-36/89, 40/89]].
    close(
        solution.analytic.standard_errors[intercept],
        (68.0_f64 / 89.0).sqrt(),
        1e-12,
        "se(intercept)",
    );
    close(
        solution.analytic.standard_errors[slope],
        (40.0_f64 / 89.0).sqrt(),
        1e-12,
        "se(slope)",
    );
}

#[test]
fn the_weights_are_actually_applied() {
    // The unweighted fit of the same four points gives 1.1 and 1.1 (also derived in
    // the provenance note). If the standard errors were being ignored, this is what
    // would come out, so this assertion is what stops a silently unweighted solver
    // from passing the test above by coincidence.
    let (problem, solution) = load("weighted-heteroscedastic");
    let intercept = index_of(&problem, "intercept");
    let slope = index_of(&problem, "slope");

    assert!(
        (solution.estimates[intercept] - 1.1).abs() > 0.1,
        "the intercept came out at the *unweighted* answer, so the weights were ignored"
    );
    assert!(
        (solution.estimates[slope] - 1.1).abs() > 0.05,
        "the slope came out at the *unweighted* answer, so the weights were ignored"
    );
}

// ---------------------------------------------------------------------------
// 3. Ridge around a nonzero prior, with a non-uniform diagonal penalty.
// ---------------------------------------------------------------------------

#[test]
fn ridge_with_prior_and_penalty_matches_an_independently_derived_solution() {
    let (problem, solution) = load("ridge-prior-penalty");

    let intercept = index_of(&problem, "intercept");
    let slope = index_of(&problem, "slope");

    // Derived by hand in fixtures/ridge-prior-penalty/PROVENANCE.md.
    close(
        solution.estimates[intercept],
        952.0 / 801.0,
        1e-12,
        "intercept",
    );
    close(solution.estimates[slope], 95.0 / 89.0, 1e-12, "slope");

    exactly(solution.lambda, 2.0);
    assert_eq!(solution.lambda_mode, "explicit");
}

#[test]
fn the_penalty_pulls_toward_the_prior_and_not_toward_zero() {
    // The unregularized answer is (112/89, 103/89) = (1.2584, 1.1573); the prior is
    // (1, 1). A solver that shrank toward zero instead of toward the prior would move
    // the estimates the same *direction* here, so direction alone proves nothing --
    // but it would overshoot past the prior. Checking that the estimates land
    // strictly between the data and the prior is what distinguishes the two.
    let (problem, solution) = load("ridge-prior-penalty");
    let intercept = index_of(&problem, "intercept");
    let slope = index_of(&problem, "slope");

    let unregularized = [112.0 / 89.0, 103.0 / 89.0];
    let prior = [1.0, 1.0];

    for (index, position) in [intercept, slope].iter().enumerate() {
        let estimate = solution.estimates[*position];
        assert!(
            estimate > prior[index] && estimate < unregularized[index],
            "parameter {index} landed at {estimate}, outside the interval between the prior {} and the unregularized fit {}",
            prior[index],
            unregularized[index]
        );
    }

    // And the harder-penalized slope has moved a larger fraction of the way to the
    // prior than the intercept has.
    let fraction = |estimate: f64, index: usize| {
        (unregularized[index] - estimate) / (unregularized[index] - prior[index])
    };
    let intercept_pull = fraction(solution.estimates[intercept], 0);
    let slope_pull = fraction(solution.estimates[slope], 1);
    assert!(
        slope_pull > intercept_pull,
        "the slope is penalized twice as hard, so it must be pulled further: slope {slope_pull:.4} vs intercept {intercept_pull:.4}"
    );
}

// ---------------------------------------------------------------------------
// 4. Rank deficiency, and the refusal that follows from it.
// ---------------------------------------------------------------------------

#[test]
fn a_rank_deficient_design_is_seen_and_its_uncertainty_is_refused() {
    let (problem, solution) = load("rank-deficient");

    assert_eq!(problem.columns(), 3);
    assert_eq!(
        solution.rank, 2,
        "the duplicated column must not count twice"
    );
    assert!(
        solution.condition_number.is_none(),
        "a rank-deficient matrix has no finite condition number, so none may be reported"
    );

    assert!(
        solution.warnings.iter().any(|w| w.code == "rank_deficient"),
        "the rank deficiency must be stated"
    );

    // The project asked for BOTH kinds of uncertainty. It gets neither, because both
    // would report a near-zero interval along the direction the data cannot see.
    assert_eq!(
        solution.analytic.status,
        UncertaintyStatus::Refused,
        "the analytic covariance must be refused"
    );
    assert_eq!(
        solution.bootstrap.status,
        UncertaintyStatus::Refused,
        "the bootstrap must be refused for the same reason"
    );
    assert_eq!(
        solution.interval_source,
        ordinata_core::IntervalSource::None
    );

    // Refused means absent, not zero.
    assert!(solution.analytic.standard_errors.is_empty());
    assert!(solution.analytic.lower.is_empty());
    assert!(solution.bootstrap.lower.is_empty());

    for reason in [
        solution.analytic.reason.as_ref(),
        solution.bootstrap.reason.as_ref(),
    ] {
        let text = reason.expect("a refusal must carry its reason");
        assert!(
            text.contains("rank"),
            "the reason must name the rank deficiency: {text}"
        );
    }
}

#[test]
fn the_fit_is_still_produced_and_still_fits() {
    // Refusing the uncertainty is not refusing the problem. The predictions are still
    // right; it is only the parameter-by-parameter attribution that is undetermined.
    let (problem, solution) = load("rank-deficient");
    for (index, residual) in solution.residuals.iter().enumerate() {
        assert!(
            residual.abs() < 0.1,
            "row {index} of a fit that reproduces the data should have a small residual, not {residual}"
        );
    }
    assert_eq!(solution.estimates.len(), problem.columns());
}

// ---------------------------------------------------------------------------
// 5. Regularization earns its keep on held-out truth. For this instance.
// ---------------------------------------------------------------------------

/// A deliberately ill-conditioned polynomial design, its noisy observations, and the
/// truth they were generated from. The truth is generated here and never written to a
/// project file, because a project cannot know it.
struct Synthetic {
    design: String,
    observations: String,
    /// The exact noiseless response at each point.
    truth: Vec<f64>,
}

fn ill_conditioned_polynomial() -> Synthetic {
    // Degree 6 on [0, 1] is a badly conditioned basis, which is exactly why nobody
    // should fit one unregularized, and exactly why it makes the point.
    const DEGREE: usize = 6;
    const POINTS: usize = 14;
    let coefficients = [1.0, 0.5, -0.30, 0.10, 0.05, -0.02, 0.01];

    // A tiny fixed LCG. Deterministic, self-contained, and not the bootstrap's RNG,
    // so this test cannot accidentally pass because of a quirk in that one.
    let mut state: u64 = 0x5eed_1234_9876_abcd;
    let mut noise = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Two draws, averaged, to get something vaguely bell-shaped. It only has to
        // be reproducible, not Gaussian.
        let a = ((state >> 33) as f64) / f64::from(u32::MAX) - 0.5;
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let b = ((state >> 33) as f64) / f64::from(u32::MAX) - 0.5;
        a + b
    };

    let mut design = String::from("observation_id");
    for power in 0..=DEGREE {
        let _ = write!(design, ",t{power}");
    }
    design.push('\n');

    let mut observations = String::from("observation_id,value,standard_error\n");
    let mut truth = Vec::with_capacity(POINTS);
    let standard_error = 0.02;

    for i in 0..POINTS {
        let t = i as f64 / (POINTS - 1) as f64;
        let mut exact = 0.0;
        let _ = write!(design, "s{i:02}");
        for (power, coefficient) in coefficients.iter().enumerate() {
            let basis = t.powi(i32::try_from(power).expect("a degree-6 polynomial fits in an i32"));
            let _ = write!(design, ",{basis}");
            exact += coefficient * basis;
        }
        design.push('\n');
        truth.push(exact);
        let _ = writeln!(
            observations,
            "s{i:02},{},{standard_error}",
            exact + standard_error * 3.0 * noise()
        );
    }

    Synthetic {
        design,
        observations,
        truth,
    }
}

fn project_with(regularization: &str) -> String {
    format!(
        r#"{{
            "schemaVersion": 1,
            "title": "Ill-conditioned polynomial",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": {regularization},
            "uncertainty": {{ "method": "analytic", "confidenceLevel": 0.95 }},
            "assumptions": ["The response is a degree-6 polynomial in t."],
            "limitations": ["Synthetic. A degree-6 polynomial basis on [0,1] is badly conditioned by construction."]
        }}"#
    )
}

/// Fit `regularization` to the synthetic data and return the root-mean-square error
/// of the fitted values against the *noiseless truth* -- which the fit never saw.
fn error_against_truth(data: &Synthetic, regularization: &str) -> (f64, Solution) {
    let directory = tempfile::tempdir().expect("temp dir");
    std::fs::write(directory.path().join("design.csv"), &data.design).unwrap();
    std::fs::write(
        directory.path().join("observations.csv"),
        &data.observations,
    )
    .unwrap();
    let project = directory.path().join("project.json");
    std::fs::write(&project, project_with(regularization)).unwrap();

    let problem = load_project(&project).expect("synthetic project should load");
    let solution = solve(&problem).expect("synthetic project should solve");

    let mut sum = 0.0;
    for (index, exact) in data.truth.iter().enumerate() {
        let error = solution.predictions[index] - exact;
        sum += error * error;
    }
    ((sum / data.truth.len() as f64).sqrt(), solution)
}

#[test]
fn on_an_ill_conditioned_problem_gcv_regularization_beats_no_regularization() {
    let data = ill_conditioned_polynomial();

    let (unregularized_error, unregularized) =
        error_against_truth(&data, r#"{ "mode": "explicit", "lambda": 0.0 }"#);
    let (regularized_error, regularized) = error_against_truth(
        &data,
        r#"{ "mode": "gcv", "minLambda": 1e-8, "maxLambda": 1e2, "steps": 61 }"#,
    );

    // The design really is nasty. If it were not, the comparison would prove nothing.
    let condition = unregularized
        .condition_number
        .expect("a full-rank design has a condition number");
    assert!(
        condition > 1e4,
        "this case is only interesting if the design is ill conditioned, and its condition number is {condition:e}"
    );

    // This is a statement about *this* problem, this noise, and this grid. It is not
    // a claim that regularization always helps, and Ordinata never makes that claim.
    assert!(
        regularized_error < unregularized_error,
        "on this instance, GCV-selected regularization (lambda = {:e}, RMSE against truth {regularized_error:.6}) did not beat no regularization (RMSE {unregularized_error:.6})",
        regularized.lambda
    );

    // And it bought that by spending fewer degrees of freedom.
    assert!(
        regularized.effective_dof < unregularized.effective_dof,
        "regularization should spend fewer degrees of freedom: {} vs {}",
        regularized.effective_dof,
        unregularized.effective_dof
    );
}

// ---------------------------------------------------------------------------
// 6. Determinism: GCV selection and the seeded bootstrap.
// ---------------------------------------------------------------------------

#[test]
fn gcv_selection_is_deterministic_and_reports_every_candidate() {
    let data = ill_conditioned_polynomial();
    let grid = r#"{ "mode": "gcv", "minLambda": 1e-8, "maxLambda": 1e2, "steps": 61 }"#;

    let (_, first) = error_against_truth(&data, grid);
    let (_, second) = error_against_truth(&data, grid);

    assert_eq!(
        first.lambda.to_bits(),
        second.lambda.to_bits(),
        "the same grid over the same data must select the identical lambda, bit for bit"
    );
    assert_eq!(
        first.gcv_candidates.len(),
        61,
        "every candidate is reported"
    );

    // Every candidate carries either a score or an explicit statement that it has
    // none. Nothing is silently dropped.
    for candidate in &first.gcv_candidates {
        assert!(candidate.lambda.is_finite());
        assert!(candidate.effective_dof.is_finite());
    }

    // The grid endpoints are exactly the declared ones.
    exactly(first.gcv_candidates[0].lambda, 1e-8);
    exactly(first.gcv_candidates[60].lambda, 1e2);

    // And the selected lambda is one of the candidates, not something interpolated.
    assert!(
        first
            .gcv_candidates
            .iter()
            .any(|c| c.lambda.to_bits() == first.lambda.to_bits()),
        "the selected lambda must be a candidate that was actually scored"
    );
}

#[test]
fn the_seeded_bootstrap_is_reproducible_and_the_seed_matters() {
    let run = |seed: u64| {
        let directory = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            directory.path().join("design.csv"),
            "observation_id,intercept,slope\nb0,1,0\nb1,1,1\nb2,1,2\nb3,1,3\nb4,1,4\nb5,1,5\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("observations.csv"),
            "observation_id,value,standard_error\nb0,1.1,0.2\nb1,1.9,0.2\nb2,3.2,0.2\nb3,3.9,0.2\nb4,5.3,0.2\nb5,5.8,0.2\n",
        )
        .unwrap();
        let project = directory.path().join("project.json");
        std::fs::write(
            &project,
            format!(
                r#"{{
                    "schemaVersion": 1,
                    "title": "Seeded bootstrap",
                    "designCsv": "design.csv",
                    "observationsCsv": "observations.csv",
                    "regularization": {{ "mode": "explicit", "lambda": 0.0 }},
                    "uncertainty": {{ "method": "bootstrap", "confidenceLevel": 0.95, "bootstrapSamples": 500, "bootstrapSeed": {seed} }},
                    "assumptions": ["Linear."],
                    "limitations": ["Synthetic."]
                }}"#
            ),
        )
        .unwrap();
        let problem = load_project(&project).unwrap();
        solve(&problem).unwrap()
    };

    let a = run(11);
    let b = run(11);
    let c = run(12);

    assert_eq!(a.bootstrap.status, UncertaintyStatus::Emitted);
    assert_eq!(a.bootstrap.successful_samples, 500);
    assert_eq!(a.bootstrap.failed_samples, 0);

    // Same seed, identical bounds, bit for bit.
    for i in 0..a.bootstrap.lower.len() {
        assert_eq!(
            a.bootstrap.lower[i].to_bits(),
            b.bootstrap.lower[i].to_bits(),
            "the same seed must give the identical bootstrap bound"
        );
        assert_eq!(
            a.bootstrap.upper[i].to_bits(),
            b.bootstrap.upper[i].to_bits()
        );
    }

    // A different seed gives a different draw. If it did not, the seed would be
    // decorative and the "reproducible" claim above would be vacuous.
    let differs = (0..a.bootstrap.lower.len())
        .any(|i| a.bootstrap.lower[i].to_bits() != c.bootstrap.lower[i].to_bits());
    assert!(differs, "changing the seed must change the resamples");

    // The bootstrap interval brackets the estimate. A percentile interval that did
    // not would mean the resampling had gone somewhere the fit never was.
    for i in 0..a.estimates.len() {
        assert!(
            a.bootstrap.lower[i] <= a.estimates[i] && a.estimates[i] <= a.bootstrap.upper[i],
            "parameter {i}: the estimate {} is outside its own bootstrap interval [{}, {}]",
            a.estimates[i],
            a.bootstrap.lower[i],
            a.bootstrap.upper[i]
        );
    }
}

#[test]
fn a_bootstrap_of_residuals_that_have_no_spread_is_refused() {
    // The exact-recovery fixture fits perfectly, so every residual is zero. Resampling
    // them would produce an interval of zero width -- a confident statement of perfect
    // certainty, manufactured out of nothing.
    let directory = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        directory.path().join("design.csv"),
        "observation_id,intercept,slope\np0,1,0\np1,1,1\np2,1,2\np3,1,3\np4,1,4\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("observations.csv"),
        "observation_id,value,standard_error\np0,3,1\np1,5,1\np2,7,1\np3,9,1\np4,11,1\n",
    )
    .unwrap();
    let project = directory.path().join("project.json");
    std::fs::write(
        &project,
        r#"{
            "schemaVersion": 1,
            "title": "Perfect fit",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": { "mode": "explicit", "lambda": 0.0 },
            "uncertainty": { "method": "bootstrap", "confidenceLevel": 0.95, "bootstrapSamples": 200, "bootstrapSeed": 3 },
            "assumptions": ["Linear."],
            "limitations": ["Synthetic, and exactly linear."]
        }"#,
    )
    .unwrap();

    let problem = load_project(&project).unwrap();
    let solution = solve(&problem).unwrap();

    assert_eq!(solution.bootstrap.status, UncertaintyStatus::Refused);
    let reason = solution
        .bootstrap
        .reason
        .expect("a refusal carries a reason");
    assert!(
        reason.contains("no spread") || reason.contains("chi-square"),
        "the refusal must explain itself: {reason}"
    );
    assert!(solution.bootstrap.lower.is_empty(), "refused means absent");
}
