//! Regressions for the things Ordinata must refuse, and for the diagnostics that say
//! why.
//!
//! Every test here corresponds to a way the instrument could have produced a number
//! that looked fine and meant nothing. They are grouped by the failure they prevent
//! rather than by the module they exercise, because that is the thing that has to stay
//! fixed; which function happens to hold the check is an implementation detail.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;

use std::path::PathBuf;

use ordinata_core::solve::UncertaintyStatus;
use ordinata_core::{build_bundle, load_project, solve, Problem, Solution};
use serde_json::Value;
use time::OffsetDateTime;

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

fn fixed_time() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("a valid timestamp")
}

/// Solve, bundle, and hand back the bundle's own `validation.json`.
fn validation_of(problem: &Problem, solution: &Solution) -> Value {
    let bundle = build_bundle(problem, solution, fixed_time()).expect("should build");
    let bytes = bundle.member("validation.json").expect("validation.json");
    serde_json::from_slice(bytes).expect("validation.json is JSON")
}

fn check_named<'a>(validation: &'a Value, name: &str) -> &'a Value {
    validation
        .get("checks")
        .and_then(Value::as_array)
        .expect("checks")
        .iter()
        .find(|check| check.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("no check named {name}"))
}

// ---------------------------------------------------------------------------
// An underdetermined design has no condition number, and no limit excuses it.
// ---------------------------------------------------------------------------

/// Two observations, three parameters. This is the case the old code got wrong.
///
/// A thin decomposition of a 2x3 design returns *two* singular values, because that is
/// how many a 2x3 matrix has. The rank is 2. Comparing the rank to the number of
/// singular values that came back therefore said "full rank", and Ordinata published a
/// finite condition number -- one that could sail through a declared
/// `conditionNumberMax` -- for a system that does not determine its parameters at all.
///
/// The comparison has to be against the *parameters*, which is the question a
/// condition number is asked. There is a third direction in parameter space that these
/// two observations say nothing about; its singular value is zero; the condition number
/// is not finite; and a declared limit is failed, not waived.
const UNDERDETERMINED: &str = r#"{
    "schemaVersion": 1,
    "title": "Two observations, three parameters",
    "designCsv": "design.csv",
    "observationsCsv": "observations.csv",
    "regularization": { "mode": "explicit", "lambda": 0.5 },
    "uncertainty": { "method": "both", "confidenceLevel": 0.95, "bootstrapSamples": 200, "bootstrapSeed": 1 },
    "assumptions": ["Linear in three parameters."],
    "limitations": ["Synthetic. There are fewer observations than parameters."],
    "validation": {
        "conditionNumberMax": {
            "value": 1000000.0,
            "source": "A generous limit, declared to prove that an underdetermined design fails it rather than slipping under it."
        }
    }
}"#;

fn underdetermined() -> Case {
    write_case(
        UNDERDETERMINED,
        "observation_id,a,b,c\nr1,1,2,3\nr2,4,5,7\n",
        "observation_id,value,standard_error\nr1,1.0,0.1\nr2,2.0,0.1\n",
    )
}

#[test]
fn an_underdetermined_design_reports_no_condition_number() {
    let case = underdetermined();
    let problem = load_project(&case.project).expect("should load");
    let solution = solve(&problem).expect("an underdetermined system still has a fit");

    assert_eq!(problem.rows(), 2);
    assert_eq!(problem.columns(), 3);
    assert_eq!(
        solution.singular_values.len(),
        2,
        "a 2x3 design has exactly two singular values; this is the fact the old bug tripped over"
    );
    assert_eq!(solution.rank, 2, "and its numerical rank is both of them");

    // Rank 2 of 3 parameters: not identifiable, so there is no condition number.
    assert!(
        solution.condition_number.is_none(),
        "a design with 3 parameters and rank 2 has a zero singular value in the direction it cannot see, so its condition number is not finite; it came back as {:?}",
        solution.condition_number
    );
    assert!(
        solution.warnings.iter().any(|w| w.code == "rank_deficient"),
        "the rank deficiency must be stated, not merely implied: {:?}",
        solution.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn an_underdetermined_design_fails_a_declared_condition_number_limit() {
    let case = underdetermined();
    let problem = load_project(&case.project).unwrap();
    let solution = solve(&problem).unwrap();
    let validation = validation_of(&problem, &solution);

    let check = check_named(&validation, "condition_number");
    assert_eq!(
        check.get("status").and_then(Value::as_str),
        Some("fail"),
        "a design that determines nothing must not pass a conditioning limit: {check}"
    );
    assert_eq!(
        check.get("observed"),
        Some(&Value::Null),
        "and there must be no number in the observed column to pass it with"
    );
    assert_eq!(
        validation.get("status").and_then(Value::as_str),
        Some("fail"),
        "the overall status must carry the failure up, not bury it"
    );
    let detail = check
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        detail.contains("rank 2") && detail.contains("3 parameters"),
        "the detail must compare the rank to the parameters, which is the comparison that matters: {detail}"
    );
}

#[test]
fn an_underdetermined_design_publishes_no_uncertainty_of_either_kind() {
    let case = underdetermined();
    let problem = load_project(&case.project).unwrap();
    let solution = solve(&problem).unwrap();

    // The project asked for both. It gets neither, and it is told why.
    assert_eq!(solution.analytic.status, UncertaintyStatus::Refused);
    assert_eq!(solution.bootstrap.status, UncertaintyStatus::Refused);
    assert!(solution.analytic.standard_errors.is_empty());
    assert!(solution.bootstrap.lower.is_empty());
    assert_eq!(solution.interval_source.as_str(), "none");
}

#[test]
fn a_square_design_that_is_actually_full_rank_still_gets_its_condition_number() {
    // The fix must not be a blanket refusal. A design with as many parameters as it
    // has rank is identifiable, and its condition number is reported as it always was.
    let case = write_case(
        r#"{
            "schemaVersion": 1,
            "title": "Full rank",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": { "mode": "explicit", "lambda": 0.0 },
            "uncertainty": { "method": "analytic", "confidenceLevel": 0.95 },
            "assumptions": ["Linear."],
            "limitations": ["Synthetic."]
        }"#,
        "observation_id,a,b\nr1,1,0\nr2,1,1\nr3,1,2\nr4,1,3\n",
        "observation_id,value,standard_error\nr1,1,0.1\nr2,2,0.1\nr3,3.1,0.1\nr4,3.9,0.1\n",
    );
    let problem = load_project(&case.project).unwrap();
    let solution = solve(&problem).unwrap();

    assert_eq!(solution.rank, 2);
    assert_eq!(problem.columns(), 2);
    let condition = solution
        .condition_number
        .expect("a full-rank design has a condition number");
    assert!(
        condition.is_finite() && condition >= 1.0,
        "condition number came out as {condition}"
    );
    assert_eq!(solution.analytic.status, UncertaintyStatus::Emitted);
}

// ---------------------------------------------------------------------------
// Resource budgets refuse before they spend, not after.
// ---------------------------------------------------------------------------

/// A `rows` x 200 design and its observations. The values do not matter: the budget is
/// refused on the shape alone, before a single number is looked at.
fn sized_case(rows: usize, samples: usize) -> Case {
    const COLUMNS: usize = 200;
    let mut design = String::from("observation_id");
    for j in 0..COLUMNS {
        let _ = write!(design, ",p{j}");
    }
    design.push('\n');
    let mut observations = String::from("observation_id,value,standard_error\n");
    for i in 0..rows {
        let _ = write!(design, "r{i:05}");
        for j in 0..COLUMNS {
            let _ = write!(design, ",{}", (i + j) % 7);
        }
        design.push('\n');
        let _ = writeln!(observations, "r{i:05},{},0.5", i % 5);
    }

    let project = format!(
        r#"{{
            "schemaVersion": 1,
            "title": "Bootstrap budget",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": {{ "mode": "explicit", "lambda": 1.0 }},
            "uncertainty": {{ "method": "bootstrap", "confidenceLevel": 0.95, "bootstrapSamples": {samples}, "bootstrapSeed": 1 }},
            "assumptions": ["Linear."],
            "limitations": ["Synthetic."]
        }}"#
    );
    write_case(&project, &design, &observations)
}

#[test]
fn an_oversized_bootstrap_is_refused_before_any_of_it_is_spent() {
    // The refusal happens at load: the design matrix has not been built, the
    // decomposition has not run, and not one resample has been drawn. A budget that
    // fired after the work would be a report, not a budget.

    // 300 x 200 x 10,000 = 6e8 projection steps, comfortably inside the budget.
    assert!(
        load_project(&sized_case(300, 10_000).project).is_ok(),
        "a problem inside the budget must still load"
    );

    // 1,200 x 200 x 10,000 = 2.4e9 steps, over it.
    let err = load_project(&sized_case(1_200, 10_000).project)
        .expect_err("1,200 x 200 x 10,000 must be refused");
    assert_eq!(err.code(), "oversized_work");
    assert_eq!(
        err.exit_code(),
        2,
        "an oversized request is an input refusal"
    );

    // The advice has to be advice the user can act on: a sample count they are
    // actually allowed to ask for, which means at or above the schema's own floor of
    // 100. A refusal that says "try 40" when the minimum is 100 is a dead end.
    let message = err.to_string();
    let admissible = message
        .split("largest admissible bootstrapSamples is ")
        .nth(1)
        .and_then(|tail| tail.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("the refusal must name an admissible sample count: {message}"));
    assert!(
        admissible >= 100,
        "the message offered {admissible} samples, below the schema's own minimum of 100, so it is advice the user cannot take: {message}"
    );

    // And the number it offers must actually be admissible.
    assert!(
        load_project(&sized_case(1_200, admissible).project).is_ok(),
        "the refusal offered {admissible} samples, and that must be a request Ordinata accepts"
    );
}

// ---------------------------------------------------------------------------
// Provenance is a statement about the bytes that produced the numbers.
// ---------------------------------------------------------------------------

fn provenance_project() -> &'static str {
    r#"{
        "schemaVersion": 1,
        "title": "Provenance",
        "designCsv": "design.csv",
        "observationsCsv": "observations.csv",
        "regularization": { "mode": "explicit", "lambda": 0.0 },
        "uncertainty": { "method": "analytic", "confidenceLevel": 0.95 },
        "assumptions": ["Linear."],
        "limitations": ["Synthetic."]
    }"#
}

fn recorded_hash(problem: &Problem, role: &str) -> String {
    problem
        .source_inputs
        .iter()
        .find(|input| input.role == role)
        .expect("the role must be recorded")
        .sha256
        .clone()
}

#[test]
fn a_load_is_a_snapshot_and_a_later_edit_cannot_reach_into_it() {
    let case = write_case(
        provenance_project(),
        "observation_id,a\nr1,1\nr2,2\n",
        "observation_id,value,standard_error\nr1,2,0.5\nr2,4,0.5\n",
    );
    let observations = case.project.with_file_name("observations.csv");

    let problem = load_project(&case.project).unwrap();
    let before = recorded_hash(&problem, "observations");
    assert_eq!(
        problem.values[0].to_bits(),
        2.0_f64.to_bits(),
        "the value read is exactly the value written"
    );

    // Change the file on disk after the load. Nothing downstream may notice.
    std::fs::write(
        &observations,
        "observation_id,value,standard_error\nr1,999,0.5\nr2,999,0.5\n",
    )
    .unwrap();

    let solution = solve(&problem).unwrap();
    let bundle = build_bundle(&problem, &solution, fixed_time()).unwrap();

    assert_eq!(
        recorded_hash(&problem, "observations"),
        before,
        "the recorded hash is of the bytes that were read, not of whatever is at the path now"
    );
    let normalized = bundle.member("inputs/observations.csv").unwrap();
    let text = std::str::from_utf8(normalized).unwrap();
    assert!(
        text.contains("r1,2,0.5") && !text.contains("999"),
        "the bundle carries the data it solved, not the data that replaced it: {text}"
    );
}

#[test]
fn the_recorded_hash_always_belongs_to_the_bytes_that_were_solved() {
    // The regression for the two-read bug.
    //
    // A loader that hashes a file and then *reopens* it to parse it has a window
    // between the two reads. A file that changes inside that window produces a bundle
    // that records the checksum of one input beside the answer to a different one --
    // silently, because both reads succeeded, and undetectably, because the checksum
    // it records is a real checksum of a real file.
    //
    // This runs the loader while the observations file flips between two contents. The
    // scheduling is up to the operating system, but the assertion is not: whichever
    // content's hash was recorded, the numbers must be the numbers *that* content
    // produces. A single-read loader cannot fail it. The old two-read loader fails it
    // within a few hundred iterations.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const A: &str = "observation_id,value,standard_error\nr1,2,0.5\nr2,4,0.5\n";
    const B: &str = "observation_id,value,standard_error\nr1,9,0.5\nr2,7,0.5\n";

    let case = write_case(provenance_project(), "observation_id,a\nr1,1\nr2,2\n", A);
    let observations = case.project.with_file_name("observations.csv");

    let hash_of = |text: &str| ordinata_core::canonical::sha256_hex(text.as_bytes());
    let (hash_a, hash_b) = (hash_of(A), hash_of(B));
    assert_ne!(hash_a, hash_b);

    let stop = Arc::new(AtomicBool::new(false));
    let flipper = {
        let stop = Arc::clone(&stop);
        let path = observations.clone();
        std::thread::spawn(move || {
            let mut which = false;
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::write(&path, if which { A } else { B });
                which = !which;
            }
        })
    };

    let mut seen_a = 0_u32;
    let mut seen_b = 0_u32;
    let mut loaded = 0_u32;
    for _ in 0..400 {
        // A torn write is a malformed CSV and a legitimate refusal; only successful
        // loads carry a claim worth checking.
        let Ok(problem) = load_project(&case.project) else {
            continue;
        };
        loaded += 1;
        let recorded = recorded_hash(&problem, "observations");
        let values = (problem.values[0], problem.values[1]);

        if recorded == hash_a {
            seen_a += 1;
            assert_eq!(
                values,
                (2.0, 4.0),
                "the bundle would have recorded the checksum of one file and the answer to another"
            );
        } else if recorded == hash_b {
            seen_b += 1;
            assert_eq!(
                values,
                (9.0, 7.0),
                "the bundle would have recorded the checksum of one file and the answer to another"
            );
        }
        // Any other hash is a torn write: its bytes are neither A nor B, so there is
        // nothing to compare it against, and Ordinata parsing it at all is fine.
    }

    stop.store(true, Ordering::Relaxed);
    flipper.join().expect("the flipper thread should finish");

    assert!(
        loaded > 0,
        "the race never actually ran: no load succeeded, so this test proved nothing"
    );
    assert!(
        seen_a + seen_b > 0,
        "no load observed either whole file, so this test proved nothing (loaded {loaded} times)"
    );
}
