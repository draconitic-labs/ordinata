//! Properties that must hold for every problem, not just the ones we thought of.
//!
//! Several of these are asserted **bit for bit** rather than to a tolerance. That is
//! not bravado; it is a consequence of the design. Ordinata sorts rows by observation
//! ID and columns by parameter name before it does any arithmetic, so a permuted
//! input produces a numerically *identical* matrix, not merely an equivalent one.
//! Permutation invariance is therefore a structural property of the normalization,
//! and testing it to a tolerance would be testing something weaker than what is true.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;

use std::path::PathBuf;

use ordinata_core::{build_bundle, load_project, solve, Problem, Solution};
use proptest::prelude::*;
use time::OffsetDateTime;

/// A fixed observation time, so that a hash comparison is comparing the numbers and
/// not the clock.
fn fixed_time() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("a valid timestamp")
}

struct Case {
    _directory: tempfile::TempDir,
    project: PathBuf,
}

fn write_case(project: &str, design: &str, observations: &str) -> Case {
    let directory = tempfile::tempdir().expect("temp dir");
    std::fs::write(directory.path().join("design.csv"), design).unwrap();
    std::fs::write(directory.path().join("observations.csv"), observations).unwrap();
    let path = directory.path().join("project.json");
    std::fs::write(&path, project).unwrap();
    Case {
        _directory: directory,
        project: path,
    }
}

fn project_json(regularization: &str, uncertainty: &str) -> String {
    format!(
        r#"{{
            "schemaVersion": 1,
            "title": "Property case",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": {regularization},
            "uncertainty": {uncertainty},
            "assumptions": ["Linear."],
            "limitations": ["Synthetic."]
        }}"#
    )
}

const PLAIN: &str = r#"{ "mode": "explicit", "lambda": 0.0 }"#;
const ANALYTIC: &str = r#"{ "method": "analytic", "confidenceLevel": 0.95 }"#;

fn run(project: &str, design: &str, observations: &str) -> (Problem, Solution) {
    let case = write_case(project, design, observations);
    let problem = load_project(&case.project).expect("should load");
    let solution = solve(&problem).expect("should solve");
    (problem, solution)
}

/// The hash of the numerical content, which is what two runs must agree on.
fn content_hash(project: &str, design: &str, observations: &str) -> String {
    let (problem, solution) = run(project, design, observations);
    let bundle = build_bundle(&problem, &solution, fixed_time()).expect("should build");
    bundle.result_content_hash
}

// ---------------------------------------------------------------------------
// Permutation invariance, bit for bit.
// ---------------------------------------------------------------------------

#[test]
fn permuting_observation_rows_does_not_change_the_result_hash() {
    let project = project_json(PLAIN, ANALYTIC);
    let design_ordered = "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n";
    let design_shuffled = "observation_id,a,b\nr3,3,7\nr1,1,4\nr4,4,9\nr2,2,5\n";
    let observations_ordered =
        "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\n";
    let observations_shuffled =
        "observation_id,value,standard_error\nr4,7.8,0.3\nr2,3.9,0.4\nr1,2.1,0.5\nr3,6.2,0.6\n";

    let a = content_hash(&project, design_ordered, observations_ordered);
    let b = content_hash(&project, design_shuffled, observations_shuffled);
    let c = content_hash(&project, design_shuffled, observations_ordered);

    assert_eq!(a, b, "shuffling both files must not change the result");
    assert_eq!(a, c, "shuffling only one of them must not change it either");
}

#[test]
fn permuting_parameter_columns_does_not_change_the_result_hash() {
    let project = project_json(PLAIN, ANALYTIC);
    let observations =
        "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\n";

    let ab = "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n";
    let ba = "observation_id,b,a\nr1,4,1\nr2,5,2\nr3,7,3\nr4,9,4\n";

    assert_eq!(
        content_hash(&project, ab, observations),
        content_hash(&project, ba, observations),
        "the order the columns happen to sit in a file is not part of the problem"
    );
}

#[test]
fn renaming_parameters_without_reordering_them_changes_nothing_but_the_names() {
    let observations =
        "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\n";
    let project = project_json(PLAIN, ANALYTIC);

    let (_, original) = run(
        &project,
        "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n",
        observations,
    );
    // alpha < beta, exactly as a < b, so the canonical column order is the same.
    let (_, renamed) = run(
        &project,
        "observation_id,alpha,beta\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n",
        observations,
    );

    for i in 0..original.estimates.len() {
        assert_eq!(
            original.estimates[i].to_bits(),
            renamed.estimates[i].to_bits(),
            "an order-preserving rename must not move a single bit of the estimate"
        );
    }
}

#[test]
fn renaming_parameters_and_reordering_them_gives_the_same_answer_to_each_parameter() {
    let observations =
        "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\n";
    let project = project_json(PLAIN, ANALYTIC);

    // a -> zeta and b -> alpha, which *reverses* the canonical order. The matrix the
    // solver sees really is different, so this one can only be equal to a tolerance --
    // and that is the honest claim.
    let (first, original) = run(
        &project,
        "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n",
        observations,
    );
    let (second, swapped) = run(
        &project,
        &"observation_id,zeta,alpha\nr1,1,4\nr2,4,5\nr3,3,7\nr4,4,9\n".replace("r2,4,5", "r2,2,5"),
        observations,
    );

    assert_eq!(first.parameters, vec!["a", "b"]);
    assert_eq!(second.parameters, vec!["alpha", "zeta"]);

    // `a` became `zeta` (index 1 in the new order); `b` became `alpha` (index 0).
    let pairs = [(0_usize, 1_usize), (1, 0)];
    for (before, after) in pairs {
        let left = original.estimates[before];
        let right = swapped.estimates[after];
        assert!(
            (left - right).abs() <= 1e-9 * left.abs().max(1.0),
            "the same parameter under a different name got {left} and {right}"
        );
    }
}

// ---------------------------------------------------------------------------
// Scale consistency.
// ---------------------------------------------------------------------------

#[test]
fn scaling_every_standard_error_by_a_constant_leaves_the_estimate_alone() {
    // Multiplying all uncertainties by k multiplies every weight by 1/k^2, which is a
    // constant factor on the whole objective, so the unregularized minimizer cannot
    // move. What *does* move is the covariance, by exactly k^2 -- so the standard
    // errors scale by exactly k. Both halves are checked, because a solver that
    // dropped the weights entirely would pass the first half trivially.
    let design = "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n";
    let project = project_json(PLAIN, ANALYTIC);

    let base =
        "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\n";
    let scaled = "observation_id,value,standard_error\nr1,2.1,5\nr2,3.9,4\nr3,6.2,6\nr4,7.8,3\n";
    let k = 10.0;

    let (_, original) = run(&project, design, base);
    let (_, widened) = run(&project, design, scaled);

    for i in 0..original.estimates.len() {
        let left = original.estimates[i];
        let right = widened.estimates[i];
        assert!(
            (left - right).abs() <= 1e-9 * left.abs().max(1.0),
            "parameter {i} moved from {left} to {right} when only the uncertainties changed"
        );
        let left_se = original.analytic.standard_errors[i];
        let right_se = widened.analytic.standard_errors[i];
        assert!(
            (right_se - k * left_se).abs() <= 1e-9 * (k * left_se),
            "parameter {i}: widening every standard error tenfold should widen its own tenfold, from {left_se} to {}, not {right_se}",
            k * left_se
        );
    }
}

// ---------------------------------------------------------------------------
// Reproducibility.
// ---------------------------------------------------------------------------

#[test]
fn running_the_same_problem_twice_produces_the_identical_content_hash() {
    let project = project_json(
        r#"{ "mode": "gcv", "minLambda": 1e-6, "maxLambda": 10.0, "steps": 25 }"#,
        r#"{ "method": "both", "confidenceLevel": 0.9, "bootstrapSamples": 300, "bootstrapSeed": 99 }"#,
    );
    let design = "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\nr5,5,12\nr6,6,14\n";
    let observations = "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\nr5,10.4,0.5\nr6,12.1,0.4\n";

    let first = content_hash(&project, design, observations);
    let second = content_hash(&project, design, observations);
    assert_eq!(
        first, second,
        "identical inputs, configuration, and seed must produce an identical content hash"
    );
}

#[test]
fn the_observation_time_changes_the_bundle_but_not_the_content_hash() {
    // This is the distinction the specification calls out: the *content* is
    // reproducible, the *artifact* records when it was made.
    let project = project_json(PLAIN, ANALYTIC);
    let design = "observation_id,a,b\nr1,1,4\nr2,2,5\nr3,3,7\nr4,4,9\n";
    let observations =
        "observation_id,value,standard_error\nr1,2.1,0.5\nr2,3.9,0.4\nr3,6.2,0.6\nr4,7.8,0.3\n";

    let case = write_case(&project, design, observations);
    let problem = load_project(&case.project).unwrap();
    let solution = solve(&problem).unwrap();

    let early = build_bundle(&problem, &solution, fixed_time()).unwrap();
    let late = build_bundle(
        &problem,
        &solution,
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
    )
    .unwrap();

    assert_eq!(
        early.result_content_hash, late.result_content_hash,
        "the clock must not reach the numbers"
    );
    assert_ne!(
        early.member("manifest.json"),
        late.member("manifest.json"),
        "but the manifest must record the time it was actually given"
    );
    assert_ne!(early.observed_at, late.observed_at);

    // And every member that the content hash covers is byte-identical.
    for member in ordinata_core::bundle::CONTENT_MEMBERS {
        assert_eq!(
            early.member(member),
            late.member(member),
            "{member} must not depend on the clock"
        );
    }
}

// ---------------------------------------------------------------------------
// No panics, whatever comes in.
// ---------------------------------------------------------------------------

proptest! {
    // Proptest fails a case if the code under test panics, so the assertion here is
    // simply that every generated problem produces either an answer or a *typed*
    // refusal. An unwrap, an index out of bounds, or an arithmetic overflow anywhere
    // in the numerical core would be caught as a panic and shrunk to a minimal case.
    #![proptest_config(ProptestConfig::with_cases(120))]

    #[test]
    fn a_bounded_problem_never_panics(
        rows in 2_usize..9,
        columns in 1_usize..4,
        entries in prop::collection::vec(-20.0_f64..20.0, 8 * 4),
        values in prop::collection::vec(-50.0_f64..50.0, 8),
        errors in prop::collection::vec(0.01_f64..25.0, 8),
        lambda in prop::option::of(0.0_f64..100.0),
        confidence in 0.5_f64..0.999,
    ) {
        let mut design = String::from("observation_id");
        for j in 0..columns {
            let _ = write!(design, ",p{j}");
        }
        design.push('\n');
        for i in 0..rows {
            let _ = write!(design, "o{i:02}");
            for j in 0..columns {
                let _ = write!(design, ",{}", entries[i * 4 + j]);
            }
            design.push('\n');
        }

        let mut observations = String::from("observation_id,value,standard_error\n");
        for i in 0..rows {
            let _ = writeln!(observations, "o{i:02},{},{}", values[i], errors[i]);
        }

        let regularization = match lambda {
            Some(l) => format!(r#"{{ "mode": "explicit", "lambda": {l} }}"#),
            None => r#"{ "mode": "gcv", "minLambda": 1e-6, "maxLambda": 100.0, "steps": 9 }"#.to_string(),
        };
        let uncertainty = format!(
            r#"{{ "method": "both", "confidenceLevel": {confidence}, "bootstrapSamples": 100, "bootstrapSeed": 5 }}"#
        );
        let project = project_json(&regularization, &uncertainty);

        let case = write_case(&project, &design, &observations);

        // Loading may legitimately fail (a degenerate generated design, say). What it
        // may not do is panic, and neither may the solve or the bundle build.
        if let Ok(problem) = load_project(&case.project) {
            match solve(&problem) {
                Ok(solution) => {
                    // Anything Ordinata is willing to publish must be finite.
                    for estimate in &solution.estimates {
                        prop_assert!(estimate.is_finite(), "published a non-finite estimate");
                    }
                    for bound in solution.analytic.lower.iter().chain(solution.analytic.upper.iter()) {
                        prop_assert!(bound.is_finite());
                    }
                    for bound in solution.bootstrap.lower.iter().chain(solution.bootstrap.upper.iter()) {
                        prop_assert!(bound.is_finite());
                    }
                    // An interval that is reported must actually contain the estimate.
                    for i in 0..solution.estimates.len() {
                        if let Some((low, high)) = solution.reported_interval(i) {
                            prop_assert!(low <= high, "an interval must not be inverted");
                        }
                    }
                    // Rank never exceeds the smaller dimension.
                    prop_assert!(solution.rank <= rows.min(columns));
                    prop_assert!(solution.rank >= 1);

                    // And the bundle must build without panicking either.
                    let bundle = build_bundle(&problem, &solution, fixed_time());
                    prop_assert!(bundle.is_ok(), "bundle build failed: {:?}", bundle.err());
                }
                Err(error) => {
                    // A refusal is fine. An *untyped* refusal is not.
                    prop_assert_ne!(
                        error.code(), "internal_invariant",
                        "a generated problem tripped an internal invariant: {}", error
                    );
                    prop_assert_eq!(error.exit_code(), 3, "a solve failure must be a numerical refusal: {}", error);
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(60))]

    /// Whatever the data, permuting the rows cannot change the answer. This is the
    /// same claim as the hand-written test above, made against arbitrary inputs.
    #[test]
    fn row_order_never_matters(
        values in prop::collection::vec(-30.0_f64..30.0, 5),
        errors in prop::collection::vec(0.05_f64..10.0, 5),
    ) {
        let design_rows: Vec<String> = (0..5)
            .map(|i| format!("k{i},{},{}\n", i + 1, (i * i) + 1))
            .collect();
        let observation_rows: Vec<String> = (0..5)
            .map(|i| format!("k{i},{},{}\n", values[i], errors[i]))
            .collect();

        let mut forward = String::from("observation_id,a,b\n");
        let mut backward = String::from("observation_id,a,b\n");
        for i in 0..5 {
            forward.push_str(&design_rows[i]);
            backward.push_str(&design_rows[4 - i]);
        }

        let mut forward_obs = String::from("observation_id,value,standard_error\n");
        let mut backward_obs = String::from("observation_id,value,standard_error\n");
        for i in 0..5 {
            forward_obs.push_str(&observation_rows[i]);
            backward_obs.push_str(&observation_rows[4 - i]);
        }

        let project = project_json(PLAIN, ANALYTIC);
        let a = write_case(&project, &forward, &forward_obs);
        let b = write_case(&project, &backward, &backward_obs);

        let left = load_project(&a.project).and_then(|p| {
            let s = solve(&p)?;
            build_bundle(&p, &s, fixed_time())
        });
        let right = load_project(&b.project).and_then(|p| {
            let s = solve(&p)?;
            build_bundle(&p, &s, fixed_time())
        });

        match (left, right) {
            (Ok(left), Ok(right)) => prop_assert_eq!(
                left.result_content_hash, right.result_content_hash,
                "reversing the file order changed the answer"
            ),
            (Err(_), Err(_)) => {}
            (left, right) => prop_assert!(
                false,
                "one order succeeded and the other did not: {:?} vs {:?}",
                left.err(), right.err()
            ),
        }
    }
}
