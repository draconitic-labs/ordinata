//! The command line, exercised as a real process.
//!
//! These run the actual binary, not a function that pretends to be one. That is the
//! point: the exit code, the split between the two output streams, and the shape of the
//! `--json` object are a published contract, and none of them can be checked by calling
//! into the library. A script somebody writes against Ordinata will see exactly what
//! these tests see.
//!
//! The rule the whole suite leans on: **standard output carries the `--json` object and
//! nothing else.** Every human-facing word, success included, goes to standard error.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;

fn ordinata() -> Command {
    Command::cargo_bin("ordinata").expect("the ordinata binary should build")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// A scratch project, copied out of `fixtures/` so that a test can never write into the
/// source tree.
struct Scratch {
    directory: tempfile::TempDir,
}

impl Scratch {
    fn from_fixture(name: &str) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let source = fixture(name);
        let target = directory.path().join("project");
        std::fs::create_dir(&target).unwrap();
        for file in ["project.json", "design.csv", "observations.csv"] {
            std::fs::copy(source.join(file), target.join(file))
                .unwrap_or_else(|e| panic!("cannot copy {file}: {e}"));
        }
        Scratch { directory }
    }

    fn empty() -> Self {
        Scratch {
            directory: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn project(&self) -> PathBuf {
        self.directory.path().join("project").join("project.json")
    }

    fn out(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the process should exit normally")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Parse standard output as exactly one JSON object, and fail loudly if it is anything
/// else. A caller piping `--json` straight into a parser must never meet a stray line.
fn json(output: &Output) -> Value {
    let text = std::str::from_utf8(&output.stdout).expect("stdout must be UTF-8");
    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("standard output is not one JSON object ({e}); it was:\n{text}"))
}

/// Every `--json` object carries the same envelope, whatever the command did.
fn assert_envelope(value: &Value, command: &str, status: &str, exit: i32) {
    assert_eq!(value["schemaVersion"], 1, "{value}");
    assert_eq!(value["tool"]["name"], "ordinata", "{value}");
    assert_eq!(value["command"], command, "{value}");
    assert_eq!(value["status"], status, "{value}");
    assert_eq!(value["exitCode"], exit, "{value}");
    assert!(
        value["tool"]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "the envelope must name the version that produced it: {value}"
    );
}

// ---------------------------------------------------------------------------
// The basics.
// ---------------------------------------------------------------------------

#[test]
fn version_names_the_tool_and_nothing_personal() {
    let output = ordinata().arg("--version").output().unwrap();
    assert_eq!(code(&output), 0);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(
        text.trim(),
        format!("ordinata {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_names_the_support_contact_and_not_a_person() {
    let output = ordinata().arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("support@draconitic.com"), "{text}");
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

#[test]
fn validate_accepts_a_good_project_and_solves_nothing() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let output = ordinata()
        .args(["validate"])
        .arg(scratch.project())
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let value = json(&output);
    assert_envelope(&value, "validate", "ok", 0);
    assert_eq!(value["result"]["observations"], 5);
    assert_eq!(value["result"]["parameters"], 2);
    assert_eq!(value["result"]["synthetic"], false);

    // Every input is accounted for by hash, before anything is solved.
    let inputs = value["result"]["inputs"].as_array().expect("inputs");
    assert_eq!(inputs.len(), 3);
    for input in inputs {
        let hash = input["sha256"].as_str().expect("a sha256");
        assert_eq!(hash.len(), 64, "{input}");
        assert!(hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    assert!(
        stderr(&output).contains("Nothing was solved"),
        "validate must say that it did not solve: {}",
        stderr(&output)
    );
}

#[test]
fn validate_refuses_a_broken_schema_with_exit_two() {
    let scratch = Scratch::from_fixture("exact-recovery");
    std::fs::write(
        scratch.project(),
        r#"{ "schemaVersion": 1, "title": "", "designCsv": "design.csv" }"#,
    )
    .unwrap();

    let output = ordinata()
        .args(["validate"])
        .arg(scratch.project())
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(code(&output), 2, "an input refusal is exit 2");
    let value = json(&output);
    assert_envelope(&value, "validate", "error", 2);
    assert_eq!(value["error"]["category"], "input");
    assert_eq!(value["error"]["code"], "project_schema");
}

#[test]
fn validate_refuses_a_project_that_is_not_there_with_exit_four() {
    let output = ordinata()
        .args(["validate", "no-such-project.json", "--json"])
        .output()
        .unwrap();

    assert_eq!(code(&output), 4, "a path refusal is exit 4");
    let value = json(&output);
    assert_eq!(value["error"]["category"], "io");
}

#[test]
fn validate_refuses_a_design_path_that_escapes_the_project_directory() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let text = std::fs::read_to_string(scratch.project()).unwrap();
    std::fs::write(
        scratch.project(),
        text.replace("\"design.csv\"", "\"../../../etc/passwd\""),
    )
    .unwrap();

    let output = ordinata()
        .args(["validate"])
        .arg(scratch.project())
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(code(&output), 4);
    let value = json(&output);
    assert_eq!(value["error"]["category"], "path");
    assert_eq!(value["error"]["code"], "path_traversal");
}

// ---------------------------------------------------------------------------
// solve
// ---------------------------------------------------------------------------

#[test]
fn solve_writes_a_bundle_that_verifies() {
    let scratch = Scratch::from_fixture("weighted-heteroscedastic");
    let out = scratch.out("bundle");

    let output = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(&out)
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let value = json(&output);
    assert_envelope(&value, "solve", "ok", 0);
    assert_eq!(value["result"]["rank"], 2);
    assert_eq!(value["result"]["synthetic"], false);
    assert_eq!(value["result"]["lambdaMode"], "explicit");

    let hash = value["result"]["resultContentHash"]
        .as_str()
        .expect("a content hash");
    assert_eq!(hash.len(), 64);

    // The members the object lists are the members that are on disk.
    for member in value["result"]["members"].as_array().expect("members") {
        let name = member.as_str().unwrap();
        assert!(
            out.join(name).is_file(),
            "{name} was listed but not written"
        );
    }

    // And the bundle it just wrote passes its own verifier, in a separate process.
    let verified = ordinata()
        .args(["verify"])
        .arg(&out)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(code(&verified), 0, "{}", stderr(&verified));
    let report = json(&verified);
    assert_envelope(&report, "verify", "ok", 0);
    assert_eq!(report["result"]["verification"], "ok");
    assert_eq!(report["result"]["bundle"]["resultContentHash"], hash);
}

#[test]
fn solve_never_overwrites_an_existing_bundle() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let out = scratch.out("bundle");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("precious.txt"), "someone else's work").unwrap();

    let output = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(&out)
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(code(&output), 4);
    assert_eq!(json(&output)["error"]["code"], "output_exists");
    assert_eq!(
        std::fs::read_to_string(out.join("precious.txt")).unwrap(),
        "someone else's work",
        "the refusal must leave the existing directory exactly as it was"
    );
    assert_eq!(
        std::fs::read_dir(&out).unwrap().count(),
        1,
        "and must not have written a single member into it"
    );
}

#[test]
fn solve_refuses_to_write_a_bundle_on_top_of_its_own_inputs() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let out = scratch.project().parent().unwrap().join("nested");

    // A bundle written *inside* the project directory would enclose the inputs it was
    // built from. That is refused before anything is written.
    let output = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(scratch.project().parent().unwrap())
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(code(&output), 4);
    assert_eq!(json(&output)["error"]["code"], "output_exists");

    // A fresh directory beside them is fine.
    let ok = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(code(&ok), 0, "{}", stderr(&ok));
}

#[test]
fn the_same_problem_at_the_same_instant_produces_the_same_bundle() {
    let scratch = Scratch::from_fixture("weighted-heteroscedastic");
    let run = |name: &str| -> Value {
        let output = ordinata()
            .args(["solve"])
            .arg(scratch.project())
            .arg("--out")
            .arg(scratch.out(name))
            .args(["--observed-at", "2026-01-01T00:00:00Z", "--json"])
            .output()
            .unwrap();
        assert_eq!(code(&output), 0, "{}", stderr(&output));
        json(&output)
    };

    let first = run("one");
    let second = run("two");
    assert_eq!(
        first["result"]["resultContentHash"],
        second["result"]["resultContentHash"]
    );
    assert_eq!(first["result"]["observedAt"], "2026-01-01T00:00:00Z");

    // Every member, byte for byte, including the manifest and the report: the
    // observation time was pinned, so nothing at all is left to vary.
    for member in first["result"]["members"].as_array().unwrap() {
        let name = member.as_str().unwrap();
        let a = std::fs::read(scratch.out("one").join(name)).unwrap();
        let b = std::fs::read(scratch.out("two").join(name)).unwrap();
        assert_eq!(a, b, "{name} differs between two runs at the same instant");
    }
}

#[test]
fn a_bad_observation_time_is_refused_before_anything_is_written() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let out = scratch.out("bundle");
    let output = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(&out)
        .args(["--observed-at", "last tuesday", "--json"])
        .output()
        .unwrap();

    assert_eq!(code(&output), 2);
    assert_eq!(json(&output)["error"]["code"], "invalid_timestamp");
    assert!(
        !out.exists(),
        "nothing may be written when the run is refused"
    );
}

#[test]
fn a_rank_deficient_project_solves_but_says_so_loudly() {
    let scratch = Scratch::from_fixture("rank-deficient");
    let out = scratch.out("bundle");

    let output = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(&out)
        .arg("--json")
        .output()
        .unwrap();

    // A rank-deficient design is not an error: the fit exists. What does not exist is
    // the uncertainty, and the bundle has to be unmistakable about that.
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let value = json(&output);
    assert_eq!(value["result"]["rank"], 2);
    assert_eq!(value["result"]["intervalSource"], "none");
    assert_eq!(
        value["result"]["validationStatus"], "fail",
        "the declared condition-number limit must fail, not be waived"
    );

    let codes: Vec<&str> = value["result"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    for expected in [
        "rank_deficient",
        "analytic_covariance_refused",
        "bootstrap_refused",
    ] {
        assert!(
            codes.contains(&expected),
            "expected {expected} in {codes:?}"
        );
    }

    // And parameters.csv carries no interval at all, rather than a narrow one.
    let parameters = std::fs::read_to_string(out.join("parameters.csv")).unwrap();
    for line in parameters.lines().skip(1) {
        let fields: Vec<&str> = line.split(',').collect();
        for index in [2, 3, 4, 6, 7, 8, 9] {
            assert!(
                fields[index].is_empty(),
                "a refused uncertainty must be absent, not zero: {line}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// demo
// ---------------------------------------------------------------------------

#[test]
fn demo_writes_a_bundle_that_shouts_that_it_is_synthetic() {
    let scratch = Scratch::empty();
    let out = scratch.out("demo");

    let output = ordinata()
        .args(["demo", "--out"])
        .arg(&out)
        .args(["--observed-at", "2026-01-01T00:00:00Z"])
        .output()
        .unwrap();

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("SYNTHETIC"),
        "the demo must say so on the terminal: {}",
        stderr(&output)
    );

    let manifest: Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["synthetic"], true);

    let report = std::fs::read_to_string(out.join("report.html")).unwrap();
    assert!(
        report.contains("Synthetic demonstration data"),
        "the report must carry the banner"
    );
    assert!(
        report.contains("None of it was measured"),
        "and must say plainly that nothing was measured"
    );

    // It verifies, like any other bundle.
    let verified = ordinata().args(["verify"]).arg(&out).output().unwrap();
    assert_eq!(code(&verified), 0, "{}", stderr(&verified));
    assert!(
        stderr(&verified).contains("SYNTHETIC"),
        "verify must repeat the warning: {}",
        stderr(&verified)
    );
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

#[test]
fn verify_reports_a_tampered_bundle_as_an_error_in_the_envelope_too() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let out = scratch.out("bundle");
    let solved = ordinata()
        .args(["solve"])
        .arg(scratch.project())
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(code(&solved), 0, "{}", stderr(&solved));

    // One byte.
    let fit = out.join("fit.csv");
    let text = std::fs::read_to_string(&fit).unwrap().replace('3', "8");
    std::fs::write(&fit, text).unwrap();

    let output = ordinata()
        .args(["verify"])
        .arg(&out)
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(code(&output), 5, "a broken bundle is exit 5");
    let value = json(&output);
    // A caller that reads only `status` must not be able to mistake this for a pass.
    assert_envelope(&value, "verify", "error", 5);
    assert_eq!(value["result"]["verification"], "failed");
    assert_eq!(value["error"]["code"], "verification_failed");

    let failed: Vec<&str> = value["result"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["status"] == "fail")
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(failed.contains(&"member_checksums"), "{failed:?}");
    assert!(
        stderr(&output).contains("FAILED"),
        "and a human must see it too: {}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_a_bundle_that_is_not_there() {
    let output = ordinata()
        .args(["verify", "no-such-bundle", "--json"])
        .output()
        .unwrap();
    assert_eq!(code(&output), 5);
    assert_eq!(json(&output)["error"]["code"], "bundle_not_found");
}

#[test]
fn verify_says_out_loud_that_it_is_not_a_signature() {
    let scratch = Scratch::empty();
    let out = scratch.out("demo");
    ordinata()
        .args(["demo", "--out"])
        .arg(&out)
        .output()
        .unwrap();

    let output = ordinata().args(["verify"]).arg(&out).output().unwrap();
    let text = stderr(&output);
    assert!(
        text.contains("not a signature"),
        "a passing verification must not be allowed to look like proof of authorship: {text}"
    );
}

// ---------------------------------------------------------------------------
// The stream contract.
// ---------------------------------------------------------------------------

#[test]
fn standard_output_carries_the_json_object_and_nothing_else() {
    let scratch = Scratch::from_fixture("exact-recovery");

    // Success, and failure, on every command that takes --json.
    let cases: Vec<(Vec<String>, bool)> = vec![
        (
            vec![
                "validate".into(),
                scratch.project().display().to_string(),
                "--json".into(),
            ],
            true,
        ),
        (
            vec![
                "validate".into(),
                "no-such-file.json".into(),
                "--json".into(),
            ],
            false,
        ),
        (
            vec!["verify".into(), "no-such-bundle".into(), "--json".into()],
            false,
        ),
    ];

    for (args, ok) in cases {
        let output = ordinata().args(&args).output().unwrap();
        // Parses as exactly one object, whether the command succeeded or not.
        let value = json(&output);
        assert!(value.is_object(), "{args:?}");
        assert_eq!(
            value["status"] == "ok",
            ok,
            "the envelope status must match the outcome: {args:?}"
        );
        assert!(
            !output.stderr.is_empty(),
            "the human stream must never be empty: {args:?}"
        );
    }
}

#[test]
fn without_json_standard_output_is_completely_silent() {
    let scratch = Scratch::from_fixture("exact-recovery");
    let output = ordinata()
        .args(["validate"])
        .arg(scratch.project())
        .output()
        .unwrap();

    assert_eq!(code(&output), 0);
    assert!(
        output.stdout.is_empty(),
        "without --json there is nothing for a machine to read, so stdout must be empty; it was: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !output.stderr.is_empty(),
        "but a human still gets told what happened"
    );
}
