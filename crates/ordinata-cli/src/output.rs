//! The two ways Ordinata talks: one stable JSON object, or prose for a human.
//!
//! The rule is absolute and it is what makes the tool scriptable: **standard output
//! carries the `--json` object and nothing else**. Every human-facing word, including
//! success messages, goes to standard error. A caller can therefore pipe stdout
//! straight into a JSON parser without filtering, and can still watch the run in a
//! terminal.

use ordinata_core::error::Error;
use ordinata_core::spec;
use ordinata_core::verify::VerificationReport;
use ordinata_core::{Bundle, Problem, Solution};
use serde_json::{json, Value};

/// Wrap a command result in the stable envelope.
fn envelope(command: &str, status: &str, exit_code: i32, body: Value) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("schemaVersion".into(), json!(spec::CLI_SCHEMA_VERSION));
    object.insert(
        "tool".into(),
        json!({ "name": spec::TOOL_NAME, "version": spec::TOOL_VERSION }),
    );
    object.insert("command".into(), json!(command));
    object.insert("status".into(), json!(status));
    object.insert("exitCode".into(), json!(exit_code));
    if let Value::Object(fields) = body {
        for (key, value) in fields {
            object.insert(key, value);
        }
    }
    Value::Object(object)
}

/// The JSON object for a failure.
pub fn error_json(command: &str, error: &Error) -> Value {
    envelope(
        command,
        "error",
        error.exit_code(),
        json!({
            "error": {
                "category": error.category(),
                "code": error.code(),
                "message": error.to_string(),
            }
        }),
    )
}

/// The JSON object for a successful `validate`.
pub fn validate_json(problem: &Problem) -> Value {
    envelope(
        "validate",
        "ok",
        0,
        json!({
            "result": {
                "title": problem.title,
                "observations": problem.rows(),
                "parameters": problem.columns(),
                "parameterNames": problem.parameters,
                "synthetic": problem.synthetic,
                "inputs": problem.source_inputs.iter().map(|input| json!({
                    "role": input.role,
                    "path": input.path,
                    "sha256": input.sha256,
                    "bytes": input.bytes,
                })).collect::<Vec<_>>(),
            }
        }),
    )
}

/// The JSON object for a successful `solve` or `demo`.
pub fn solve_json(
    command: &str,
    bundle: &Bundle,
    solution: &Solution,
    written_to: &str,
    validation_status: &str,
) -> Value {
    envelope(
        command,
        "ok",
        0,
        json!({
            "result": {
                "bundle": written_to,
                "observedAt": bundle.observed_at,
                "resultContentHash": bundle.result_content_hash,
                "synthetic": bundle.synthetic,
                "lambda": solution.lambda,
                "lambdaMode": solution.lambda_mode,
                "rank": solution.rank,
                "intervalSource": solution.interval_source.as_str(),
                "validationStatus": validation_status,
                "warnings": solution.warnings.iter().map(|w| json!({
                    "code": w.code,
                    "message": w.message,
                })).collect::<Vec<_>>(),
                "members": bundle.members().map(|(path, _)| path).collect::<Vec<_>>(),
            }
        }),
    )
}

/// The JSON object for `verify`, whether it passed or not.
pub fn verify_json(report: &VerificationReport, exit_code: i32) -> Value {
    let status = if report.passed() { "ok" } else { "error" };
    let mut body = json!({
        "result": {
            "source": report.source,
            "kind": report.kind,
            "verification": report.status,
            "members": report.members,
            "checks": report.checks.iter().map(|check| json!({
                "name": check.name,
                "status": check.status,
                "detail": check.detail,
            })).collect::<Vec<_>>(),
            "bundle": report.summary.as_ref().map(|summary| json!({
                "toolVersion": summary.tool_version,
                "modelVersion": summary.model_version,
                "observedAt": summary.observed_at,
                "title": summary.title,
                "synthetic": summary.synthetic,
                "resultContentHash": summary.result_content_hash,
            })),
        }
    });

    // A failed verification is an error in the envelope as well as in the report, so
    // that a caller checking only `status` cannot mistake a tampered bundle for a
    // good one.
    if !report.passed() {
        let failures: Vec<String> = report
            .failures()
            .map(|check| format!("{}: {}", check.name, check.detail))
            .collect();
        if let Some(object) = body.as_object_mut() {
            object.insert(
                "error".into(),
                json!({
                    "category": "bundle",
                    "code": "verification_failed",
                    "message": format!("{} check(s) failed: {}", failures.len(), failures.join("; ")),
                }),
            );
        }
    }

    envelope("verify", status, exit_code, body)
}

/// Human-readable summary of a solve, for standard error.
pub fn human_solve(bundle: &Bundle, solution: &Solution, written_to: &str, validation: &str) {
    if bundle.synthetic {
        eprintln!("SYNTHETIC DEMONSTRATION DATA. Nothing below was measured.");
    }
    eprintln!("Wrote bundle to {written_to}");
    eprintln!("  observed at        {}", bundle.observed_at);
    eprintln!("  result content     {}", bundle.result_content_hash);
    eprintln!(
        "  lambda             {} ({})",
        solution.lambda, solution.lambda_mode
    );
    eprintln!("  rank               {}", solution.rank);
    eprintln!("  interval           {}", solution.interval_source.as_str());
    eprintln!("  declared checks    {validation}");
    if solution.warnings.is_empty() {
        eprintln!("  warnings           none");
    } else {
        eprintln!("  warnings           {}", solution.warnings.len());
        for warning in &solution.warnings {
            eprintln!("    [{}] {}", warning.code, warning.message);
        }
    }
}

/// Human-readable summary of a validate, for standard error.
pub fn human_validate(problem: &Problem) {
    eprintln!("Project is valid: {}", problem.title);
    eprintln!(
        "  {} observations, {} parameters ({})",
        problem.rows(),
        problem.columns(),
        problem.parameters.join(", ")
    );
    for input in &problem.source_inputs {
        eprintln!(
            "  {:<13} {} ({} bytes)",
            input.role, input.sha256, input.bytes
        );
    }
    eprintln!("Nothing was solved. Run `ordinata solve` to produce a bundle.");
}

/// Human-readable verification report, for standard error.
pub fn human_verify(report: &VerificationReport) {
    eprintln!("Verifying {} ({})", report.source, report.kind);
    for check in &report.checks {
        let mark = match check.status {
            "pass" => "ok  ",
            "fail" => "FAIL",
            _ => "skip",
        };
        eprintln!("  [{mark}] {:<24} {}", check.name, check.detail);
    }
    if let Some(summary) = &report.summary {
        eprintln!();
        eprintln!("  title              {}", summary.title);
        eprintln!("  written by         ordinata {}", summary.tool_version);
        eprintln!("  model version      {}", summary.model_version);
        eprintln!("  observed at        {}", summary.observed_at);
        eprintln!("  result content     {}", summary.result_content_hash);
        if summary.synthetic {
            eprintln!(
                "  SYNTHETIC          this bundle declares itself synthetic demonstration data"
            );
        }
    }
    eprintln!();
    if report.passed() {
        eprintln!("VERIFIED. Every member is present and hashes to what the bundle records.");
        eprintln!("This proves the bundle is intact. It is not a signature: it does not say who");
        eprintln!("produced it, and anyone with Ordinata can produce a bundle that verifies.");
    } else {
        eprintln!("FAILED. This bundle is not what it says it is.");
    }
}
