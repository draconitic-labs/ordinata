//! The evidence bundle: what it contains, and how it is written without ever
//! leaving a half-finished one behind.
//!
//! # Members
//!
//! ```text
//! manifest.json            provenance, configuration, every threshold, every hash
//! inputs/project.json      the normalized project, re-solvable as-is
//! inputs/design.csv        the design in canonical order
//! inputs/observations.csv  the observations in canonical order
//! parameters.csv           estimates, uncertainty, prior, penalty
//! fit.csv                  observed, predicted, residual, standardized residual
//! diagnostics.json         rank, singular values, lambda, GCV grid, warnings
//! validation.json          the user's declared checks and whether they passed
//! report.html              a static, escaped, offline summary
//! SHA256SUMS.txt           every member above, hashed
//! ```
//!
//! The bundle carries its own *normalized inputs*, so it is self-contained: the
//! numbers can be traced back to the exact matrix they came from without the original
//! files, and `inputs/project.json` is itself a valid project that re-solves to the
//! same answer.
//!
//! # The result content hash
//!
//! `manifest.resultContentHash` covers the seven members that carry numbers, and
//! deliberately excludes `report.html` and the manifest itself, because both embed
//! the wall-clock observation time. Two runs over identical inputs with an identical
//! configuration and seed therefore produce an identical result content hash even
//! though they were run at different moments and their archives differ byte for byte.
//! That distinction is the whole point: the *content* is reproducible; the *artifact*
//! records when it was made.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::canonical;
use crate::error::{Error, IoError, PathError, Result};
use crate::ingest::{LambdaPlan, Problem};
use crate::paths;
use crate::report;
use crate::solve::{IntervalSource, Solution};
use crate::spec;

/// The members that carry numbers, and therefore the members the result content hash
/// covers. Sorted, because the hash is over this list in this order.
pub const CONTENT_MEMBERS: [&str; 7] = [
    "diagnostics.json",
    "fit.csv",
    "inputs/design.csv",
    "inputs/observations.csv",
    "inputs/project.json",
    "parameters.csv",
    "validation.json",
];

/// The manifest describes these. It cannot describe itself, and it does not describe
/// the checksum file.
pub const PAYLOAD_MEMBERS: [&str; 8] = [
    "diagnostics.json",
    "fit.csv",
    "inputs/design.csv",
    "inputs/observations.csv",
    "inputs/project.json",
    "parameters.csv",
    "report.html",
    "validation.json",
];

/// The manifest's own name.
pub const MANIFEST: &str = "manifest.json";
/// The checksum file's name.
pub const CHECKSUMS: &str = "SHA256SUMS.txt";

/// A complete bundle, held in memory, ready to be written.
#[derive(Debug, Clone)]
pub struct Bundle {
    members: BTreeMap<String, Vec<u8>>,
    /// The hash of the numerical content, excluding wall-clock time.
    pub result_content_hash: String,
    /// The observation time, RFC 3339 in UTC.
    pub observed_at: String,
    /// The project title.
    pub title: String,
    /// True only for the built-in demo.
    pub synthetic: bool,
}

impl Bundle {
    /// Every member, ascending by path.
    pub fn members(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.members.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// One member's bytes.
    pub fn member(&self, path: &str) -> Option<&[u8]> {
        self.members.get(path).map(Vec::as_slice)
    }
}

/// Format a float so that it is short, readable, and reparses to exactly the same
/// `f64`.
///
/// Rust's `Display` never uses an exponent, so `1e300` would come out as a 301-digit
/// number, and `LowerExp` always uses one, so `1.5` would come out as `1.5e0`.
/// Neither is wrong; both are round-trip exact. Taking whichever is shorter gives a
/// CSV a human can read at ordinary magnitudes and a bounded field at extreme ones.
/// The choice is a pure function of the value, so it is deterministic.
pub(crate) fn format_number(value: f64) -> String {
    // The sign of a zero is an artifact of the arithmetic, not a fact; canonical JSON
    // drops it, so the CSVs drop it too and the two stay consistent.
    let value = if value == 0.0 { 0.0 } else { value };
    let plain = format!("{value}");
    let exponential = format!("{value:e}");
    if exponential.len() < plain.len() {
        exponential
    } else {
        plain
    }
}

fn csv_row(fields: &[String]) -> String {
    // Every field here is either an identifier from the accepted character set, a
    // number, or an empty string. None of them can contain a comma, a quote, or a
    // newline, so no quoting is possible and none is applied. That is a property of
    // the identifier grammar, not an assumption; see `text::identifier_rule_broken`.
    let mut line = fields.join(",");
    line.push('\n');
    line
}

fn optional_number_field(value: Option<f64>) -> String {
    value.map(format_number).unwrap_or_default()
}

/// The normalized design, in canonical order.
fn normalized_design(problem: &Problem) -> String {
    let mut out = String::new();
    let mut header = vec!["observation_id".to_string()];
    header.extend(problem.parameters.iter().cloned());
    out.push_str(&csv_row(&header));
    for (i, id) in problem.observation_ids.iter().enumerate() {
        let mut row = Vec::with_capacity(problem.columns() + 1);
        row.push(id.clone());
        for j in 0..problem.columns() {
            row.push(format_number(problem.design[(i, j)]));
        }
        out.push_str(&csv_row(&row));
    }
    out
}

/// The normalized observations, in canonical order.
fn normalized_observations(problem: &Problem) -> String {
    let mut out = String::new();
    out.push_str(&csv_row(&[
        "observation_id".into(),
        "value".into(),
        "standard_error".into(),
    ]));
    for (i, id) in problem.observation_ids.iter().enumerate() {
        out.push_str(&csv_row(&[
            id.clone(),
            format_number(problem.values[i]),
            format_number(problem.standard_errors[i]),
        ]));
    }
    out
}

/// The regularization block, as it appears in the normalized project and the manifest.
fn regularization_value(problem: &Problem) -> Result<Value> {
    let mut block = serde_json::Map::new();
    match problem.lambda_plan {
        LambdaPlan::Explicit(lambda) => {
            block.insert("mode".into(), json!("explicit"));
            block.insert("lambda".into(), canonical::number(lambda)?);
        }
        LambdaPlan::Gcv { min, max, steps } => {
            block.insert("mode".into(), json!("gcv"));
            block.insert("minLambda".into(), canonical::number(min)?);
            block.insert("maxLambda".into(), canonical::number(max)?);
            block.insert("steps".into(), json!(steps));
        }
    }
    if problem.prior_declared {
        let mut prior = serde_json::Map::new();
        for (j, name) in problem.parameters.iter().enumerate() {
            prior.insert(name.clone(), canonical::number(problem.prior[j])?);
        }
        block.insert("prior".into(), Value::Object(prior));
    }
    if problem.penalty_declared {
        let mut penalty = serde_json::Map::new();
        for (j, name) in problem.parameters.iter().enumerate() {
            penalty.insert(name.clone(), canonical::number(problem.penalty[j])?);
        }
        block.insert("penalty".into(), Value::Object(penalty));
    }
    Ok(Value::Object(block))
}

fn uncertainty_value(problem: &Problem) -> Result<Value> {
    let mut block = serde_json::Map::new();
    block.insert("method".into(), json!(problem.uncertainty.method.as_str()));
    block.insert(
        "confidenceLevel".into(),
        canonical::number(problem.uncertainty.confidence_level)?,
    );
    if let Some(plan) = problem.uncertainty.bootstrap {
        block.insert("bootstrapSamples".into(), json!(plan.samples));
        block.insert("bootstrapSeed".into(), json!(plan.seed));
    }
    Ok(Value::Object(block))
}

fn validation_value(problem: &Problem) -> Result<Value> {
    let Some(validation) = &problem.validation else {
        return Ok(Value::Null);
    };
    let mut block = serde_json::Map::new();
    if let Some(threshold) = &validation.weighted_rmse_max {
        block.insert(
            "weightedRmseMax".into(),
            json!({
                "value": canonical::number(threshold.value)?,
                "source": threshold.source.clone(),
            }),
        );
    }
    if let Some(threshold) = &validation.condition_number_max {
        block.insert(
            "conditionNumberMax".into(),
            json!({
                "value": canonical::number(threshold.value)?,
                "source": threshold.source.clone(),
            }),
        );
    }
    Ok(Value::Object(block))
}

fn sources_value(problem: &Problem) -> Value {
    Value::Array(
        problem
            .sources
            .iter()
            .map(|source| {
                json!({
                    "label": source.label,
                    "uri": source.uri,
                    "date": source.date,
                })
            })
            .collect(),
    )
}

/// The normalized project: a valid schema-1 project that re-solves to this same
/// answer, pointing at the normalized CSVs that sit beside it.
fn normalized_project(problem: &Problem) -> Result<Value> {
    let mut project = serde_json::Map::new();
    project.insert("schemaVersion".into(), json!(spec::PROJECT_SCHEMA_VERSION));
    project.insert("title".into(), json!(problem.title));
    if let Some(description) = &problem.description {
        project.insert("description".into(), json!(description));
    }
    project.insert("designCsv".into(), json!("design.csv"));
    project.insert("observationsCsv".into(), json!("observations.csv"));
    project.insert("regularization".into(), regularization_value(problem)?);
    project.insert("uncertainty".into(), uncertainty_value(problem)?);
    project.insert("assumptions".into(), json!(problem.assumptions));
    project.insert("limitations".into(), json!(problem.limitations));
    if !problem.sources.is_empty() {
        project.insert("sources".into(), sources_value(problem));
    }
    let validation = validation_value(problem)?;
    if !validation.is_null() {
        project.insert("validation".into(), validation);
    }
    Ok(Value::Object(project))
}

/// The parameter table.
///
/// When a project asks for `both`, both intervals are computed, and both appear here
/// in columns of their own. The earlier shape of this file had one `interval_lower` /
/// `interval_upper` pair and a `source` column naming which method had won, which
/// meant that asking for two intervals and getting one -- with the other discarded
/// unmentioned -- looked exactly like asking for one. That is the failure this product
/// exists to avoid, committed by the product itself.
///
/// So: `analytic_*` and `bootstrap_*` are each present whenever that method was
/// emitted, and empty whenever it was not. The `interval_*` columns still carry a
/// primary interval, because a reader with one column of space needs a defensible
/// default and the analytic one is it. `interval_source` says which. Nothing is
/// dropped to make room for it.
///
/// The two intervals answer different questions and will not agree in general: the
/// analytic one takes the declared standard errors at their word, and the bootstrap
/// one takes the residuals at theirs. When they disagree loudly, the reduced
/// chi-square in `diagnostics.json` says why, and a warning says so out loud.
fn parameters_csv(problem: &Problem, solution: &Solution) -> String {
    let mut out = String::new();
    out.push_str(&csv_row(&[
        "name".into(),
        "estimate".into(),
        "standard_error".into(),
        "interval_lower".into(),
        "interval_upper".into(),
        "interval_source".into(),
        "analytic_lower".into(),
        "analytic_upper".into(),
        "bootstrap_lower".into(),
        "bootstrap_upper".into(),
        "prior".into(),
        "penalty".into(),
    ]));
    for (j, name) in problem.parameters.iter().enumerate() {
        let standard_error = solution.analytic.standard_errors.get(j).copied();
        let reported = solution.reported_interval(j);
        let analytic = solution.analytic_bounds(j);
        let bootstrap = solution.bootstrap_bounds(j);
        out.push_str(&csv_row(&[
            name.clone(),
            format_number(solution.estimates[j]),
            optional_number_field(standard_error),
            optional_number_field(reported.map(|(low, _)| low)),
            optional_number_field(reported.map(|(_, high)| high)),
            solution.interval_source.as_str().to_string(),
            optional_number_field(analytic.map(|(low, _)| low)),
            optional_number_field(analytic.map(|(_, high)| high)),
            optional_number_field(bootstrap.map(|(low, _)| low)),
            optional_number_field(bootstrap.map(|(_, high)| high)),
            format_number(problem.prior[j]),
            format_number(problem.penalty[j]),
        ]));
    }
    out
}

fn fit_csv(problem: &Problem, solution: &Solution) -> String {
    let mut out = String::new();
    out.push_str(&csv_row(&[
        "observation_id".into(),
        "observed".into(),
        "prediction".into(),
        "residual".into(),
        "standardized_residual".into(),
        "standard_error".into(),
    ]));
    for (i, id) in problem.observation_ids.iter().enumerate() {
        out.push_str(&csv_row(&[
            id.clone(),
            format_number(problem.values[i]),
            format_number(solution.predictions[i]),
            format_number(solution.residuals[i]),
            optional_number_field(solution.standardized_residuals[i]),
            format_number(problem.standard_errors[i]),
        ]));
    }
    out
}

fn diagnostics_value(problem: &Problem, solution: &Solution) -> Result<Value> {
    let candidates: Result<Vec<Value>> = solution
        .gcv_candidates
        .iter()
        .map(|candidate| {
            Ok(json!({
                "lambda": canonical::number(candidate.lambda)?,
                "score": canonical::optional_number(candidate.score)?,
                "status": if candidate.score.is_some() { "scored" } else { "undefined" },
                "effectiveDegreesOfFreedom": canonical::number(candidate.effective_dof)?,
                "weightedResidualSumOfSquares": canonical::number(candidate.weighted_rss)?,
            }))
        })
        .collect();

    let regularization = json!({
        "mode": solution.lambda_mode,
        "lambda": canonical::number(solution.lambda)?,
        "grid": match problem.lambda_plan {
            LambdaPlan::Explicit(_) => Value::Null,
            LambdaPlan::Gcv { min, max, steps } => json!({
                "minLambda": canonical::number(min)?,
                "maxLambda": canonical::number(max)?,
                "steps": steps,
            }),
        },
        "candidates": Value::Array(candidates?),
    });

    let analytic = json!({
        "status": solution.analytic.status.as_str(),
        "reason": solution.analytic.reason,
        "standardErrors": canonical::number_array(&solution.analytic.standard_errors)?,
        "lower": canonical::number_array(&solution.analytic.lower)?,
        "upper": canonical::number_array(&solution.analytic.upper)?,
    });

    let bootstrap = json!({
        "status": solution.bootstrap.status.as_str(),
        "reason": solution.bootstrap.reason,
        "requestedSamples": solution.bootstrap.requested_samples,
        "successfulSamples": solution.bootstrap.successful_samples,
        "failedSamples": solution.bootstrap.failed_samples,
        "seed": solution.bootstrap.seed,
        "seedStream": spec::BOOTSTRAP_SEED_DOMAIN,
        "lower": canonical::number_array(&solution.bootstrap.lower)?,
        "upper": canonical::number_array(&solution.bootstrap.upper)?,
    });

    let max_leverage = solution.leverages.iter().copied().fold(0.0_f64, f64::max);
    let max_standardized = solution
        .standardized_residuals
        .iter()
        .filter_map(|v| v.map(f64::abs))
        .fold(0.0_f64, f64::max);

    Ok(json!({
        "schemaVersion": spec::BUNDLE_SCHEMA_VERSION,
        "dimensions": {
            "observations": problem.rows(),
            "parameters": problem.columns(),
        },
        "parameters": problem.parameters,
        "rank": solution.rank,
        "rankTolerance": canonical::number(solution.rank_tolerance)?,
        "singularValues": canonical::number_array(&solution.singular_values)?,
        "conditionNumber": canonical::optional_number(solution.condition_number)?,
        "regularization": regularization,
        "effectiveDegreesOfFreedom": canonical::number(solution.effective_dof)?,
        "residualDegreesOfFreedom": canonical::number(solution.residual_dof)?,
        "residuals": {
            "weightedSumOfSquares": canonical::number(solution.weighted_rss)?,
            "weightedRootMeanSquare": canonical::number(solution.weighted_rmse)?,
            "reducedChiSquare": canonical::optional_number(solution.reduced_chi_square)?,
            "maxAbsStandardizedResidual": canonical::number(max_standardized)?,
            "maxLeverage": canonical::number(max_leverage)?,
        },
        "uncertainty": {
            "confidenceLevel": canonical::number(problem.uncertainty.confidence_level)?,
            "intervalSource": solution.interval_source.as_str(),
            "analytic": analytic,
            "bootstrap": bootstrap,
        },
        "warnings": solution.warnings.iter().map(|w| json!({
            "code": w.code,
            "message": w.message,
        })).collect::<Vec<_>>(),
    }))
}

/// The user's declared checks. Ordinata reports them; it does not endorse them, and
/// it does not invent any of its own.
fn validation_report(problem: &Problem, solution: &Solution) -> Result<Value> {
    let mut checks = Vec::new();
    let mut any_pass = false;
    let mut any_fail = false;

    let declared = problem.validation.as_ref();

    // Weighted RMSE.
    {
        let threshold = declared.and_then(|v| v.weighted_rmse_max.as_ref());
        let observed = solution.weighted_rmse;
        let (status, detail) = match threshold {
            Some(t) if observed <= t.value => (
                "pass",
                format!(
                    "the weighted RMSE of {observed:.6} is within the declared limit of {}",
                    t.value
                ),
            ),
            Some(t) => (
                "fail",
                format!(
                    "the weighted RMSE of {observed:.6} exceeds the declared limit of {}",
                    t.value
                ),
            ),
            None => (
                "not_applicable",
                "the project declared no weighted RMSE limit, so nothing was checked".to_string(),
            ),
        };
        match status {
            "pass" => any_pass = true,
            "fail" => any_fail = true,
            _ => {}
        }
        checks.push(json!({
            "name": "weighted_rmse",
            "declaredThreshold": canonical::optional_number(threshold.map(|t| t.value))?,
            "thresholdSource": threshold.and_then(|t| t.source.clone()),
            "observed": canonical::number(observed)?,
            "status": status,
            "detail": detail,
        }));
    }

    // Condition number. A rank-deficient design has no finite condition number, and
    // that is a failure of any declared limit, not an exemption from it.
    {
        let threshold = declared.and_then(|v| v.condition_number_max.as_ref());
        let (status, detail) = match (threshold, solution.condition_number) {
            (None, _) => (
                "not_applicable",
                "the project declared no condition number limit, so nothing was checked"
                    .to_string(),
            ),
            (Some(_), None) => (
                "fail",
                format!(
                    "the scaled design has numerical rank {} but {} parameters, so its condition number is not finite and cannot be within any limit. A design that does not determine its parameters is not a well conditioned one; it is the limiting case of a badly conditioned one, and it fails this check rather than being excused from it.",
                    solution.rank,
                    problem.columns()
                ),
            ),
            (Some(t), Some(observed)) if observed <= t.value => (
                "pass",
                format!(
                    "the condition number of {observed:.6e} is within the declared limit of {:e}",
                    t.value
                ),
            ),
            (Some(t), Some(observed)) => (
                "fail",
                format!(
                    "the condition number of {observed:.6e} exceeds the declared limit of {:e}",
                    t.value
                ),
            ),
        };
        match status {
            "pass" => any_pass = true,
            "fail" => any_fail = true,
            _ => {}
        }
        checks.push(json!({
            "name": "condition_number",
            "declaredThreshold": canonical::optional_number(threshold.map(|t| t.value))?,
            "thresholdSource": threshold.and_then(|t| t.source.clone()),
            "observed": canonical::optional_number(solution.condition_number)?,
            "status": status,
            "detail": detail,
        }));
    }

    let overall = if any_fail {
        "fail"
    } else if any_pass {
        "pass"
    } else {
        "not_applicable"
    };

    Ok(json!({
        "schemaVersion": spec::BUNDLE_SCHEMA_VERSION,
        "status": overall,
        "checks": checks,
        "note": "These are the checks the project declared. They are the user's criteria, not a statement by Ordinata that the result is fit for any purpose.",
    }))
}

fn platform_value() -> Value {
    json!({
        "target": env!("ORDINATA_TARGET"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "pointerWidth": usize::BITS,
    })
}

/// Build the whole bundle.
///
/// # Errors
///
/// Returns [`Error::Internal`] if any number that reached this point is not finite;
/// by construction the solver has already refused those.
pub fn build_bundle(
    problem: &Problem,
    solution: &Solution,
    observed_at: OffsetDateTime,
) -> Result<Bundle> {
    let observed_at = observed_at
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|e| Error::Internal(format!("cannot format the observation time: {e}")))?;

    let mut members: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    members.insert(
        "inputs/project.json".into(),
        canonical::pretty_bytes(&normalized_project(problem)?)?,
    );
    members.insert(
        "inputs/design.csv".into(),
        normalized_design(problem).into_bytes(),
    );
    members.insert(
        "inputs/observations.csv".into(),
        normalized_observations(problem).into_bytes(),
    );
    members.insert(
        "parameters.csv".into(),
        parameters_csv(problem, solution).into_bytes(),
    );
    members.insert("fit.csv".into(), fit_csv(problem, solution).into_bytes());
    members.insert(
        "diagnostics.json".into(),
        canonical::pretty_bytes(&diagnostics_value(problem, solution)?)?,
    );
    members.insert(
        "validation.json".into(),
        canonical::pretty_bytes(&validation_report(problem, solution)?)?,
    );

    // The content hash covers exactly the members that carry numbers.
    let result_content_hash = result_content_hash(&members)?;

    let validation = members
        .get("validation.json")
        .ok_or_else(|| Error::Internal("validation.json vanished".into()))?;
    let validation_status: Value = serde_json::from_slice(validation)
        .map_err(|e| Error::Internal(format!("cannot reread validation.json: {e}")))?;

    members.insert(
        "report.html".into(),
        report::render(
            problem,
            solution,
            &observed_at,
            &result_content_hash,
            &validation_status,
        )
        .into_bytes(),
    );

    let manifest = manifest_value(
        problem,
        solution,
        &observed_at,
        &result_content_hash,
        &members,
    )?;
    members.insert(MANIFEST.into(), canonical::pretty_bytes(&manifest)?);

    let checksums = checksum_file(&members);
    members.insert(CHECKSUMS.into(), checksums.into_bytes());

    Ok(Bundle {
        members,
        result_content_hash,
        observed_at,
        title: problem.title.clone(),
        synthetic: problem.synthetic,
    })
}

/// Hash the numerical content: a sorted list of `(path, sha256)` over exactly the
/// content members, plus the model and schema versions that fix their meaning.
///
/// This is a hash of hashes rather than a re-serialization of the numbers, and that
/// is deliberate. It is exactly as strong (a changed number changes a member hash,
/// which changes this), it cannot drift out of step with the bytes on disk, and it
/// gives the verifier nothing to reparse and therefore nothing to reparse wrongly.
pub(crate) fn result_content_hash(members: &BTreeMap<String, Vec<u8>>) -> Result<String> {
    let mut listed = Vec::with_capacity(CONTENT_MEMBERS.len());
    for path in CONTENT_MEMBERS {
        let bytes = members
            .get(path)
            .ok_or_else(|| Error::Internal(format!("content member {path} is missing")))?;
        listed.push(json!({
            "path": path,
            "sha256": canonical::sha256_hex(bytes),
        }));
    }
    canonical::canonical_hash(&json!({
        "modelVersion": spec::MODEL_VERSION,
        "bundleSchemaVersion": spec::BUNDLE_SCHEMA_VERSION,
        "members": listed,
    }))
}

fn manifest_value(
    problem: &Problem,
    solution: &Solution,
    observed_at: &str,
    result_content_hash: &str,
    members: &BTreeMap<String, Vec<u8>>,
) -> Result<Value> {
    let mut payload = Vec::with_capacity(PAYLOAD_MEMBERS.len());
    for path in PAYLOAD_MEMBERS {
        let bytes = members
            .get(path)
            .ok_or_else(|| Error::Internal(format!("payload member {path} is missing")))?;
        payload.push(json!({
            "path": path,
            "sha256": canonical::sha256_hex(bytes),
            "bytes": bytes.len(),
        }));
    }

    let source_inputs: Vec<Value> = problem
        .source_inputs
        .iter()
        .map(|input| {
            json!({
                "role": input.role,
                "path": input.path,
                "sha256": input.sha256,
                "bytes": input.bytes,
            })
        })
        .collect();

    Ok(json!({
        "schemaVersion": spec::BUNDLE_SCHEMA_VERSION,
        "tool": {
            "name": spec::TOOL_NAME,
            "version": spec::TOOL_VERSION,
            "publisher": spec::PUBLISHER,
            "support": spec::SUPPORT_CONTACT,
        },
        "platform": platform_value(),
        "solver": spec::manifest_block(),
        "observedAt": observed_at,
        "synthetic": problem.synthetic,
        "project": {
            "title": problem.title,
            "description": problem.description,
            "assumptions": problem.assumptions,
            "limitations": problem.limitations,
            "sources": sources_value(problem),
            "dimensions": {
                "observations": problem.rows(),
                "parameters": problem.columns(),
            },
            "parameters": problem.parameters,
            "configuration": {
                "regularization": regularization_value(problem)?,
                "uncertainty": uncertainty_value(problem)?,
                "validation": validation_value(problem)?,
            },
        },
        "result": {
            "lambda": canonical::number(solution.lambda)?,
            "lambdaMode": solution.lambda_mode,
            "rank": solution.rank,
            "intervalSource": solution.interval_source.as_str(),
            "warningCodes": solution.warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        },
        "sourceInputs": source_inputs,
        "resultContent": {
            "members": CONTENT_MEMBERS,
            "sha256": result_content_hash,
            "note": "Covers the members that carry numbers. It excludes report.html and this manifest, both of which embed the wall-clock observation time, so that two runs with identical inputs, configuration, and seed agree here even though their files differ.",
        },
        "payload": payload,
    }))
}

/// `sha256sum` format: the digest, two spaces, the path. Every member except this
/// file itself, ascending by path.
fn checksum_file(members: &BTreeMap<String, Vec<u8>>) -> String {
    let mut out = String::new();
    for (path, bytes) in members {
        if path == CHECKSUMS {
            continue;
        }
        let _ = writeln!(out, "{}  {path}", canonical::sha256_hex(bytes));
    }
    out
}

/// Write a bundle to a new directory, atomically.
///
/// The bytes go into a staging directory beside the target and are moved into place
/// with a single rename once every member is written and flushed. A run that is
/// killed halfway leaves a staging directory, never a bundle that looks complete and
/// is not. Ordinata never overwrites an existing bundle.
///
/// `guard` lists paths the output must not overlap, so a bundle cannot be written on
/// top of, or around, its own inputs.
///
/// # Errors
///
/// Returns [`PathError::OutputExists`] if the target is already there, and an
/// [`IoError`] if any write fails.
pub fn write_bundle(bundle: &Bundle, out: &Path, guard: &[PathBuf]) -> Result<PathBuf> {
    let plan = paths::plan_output(out, guard)?;

    let result = write_staged(bundle, &plan.staging_dir);
    if result.is_err() {
        // Best effort: do not leave debris behind after a failure that was ours.
        let _ = std::fs::remove_dir_all(&plan.staging_dir);
    }
    result?;

    std::fs::rename(&plan.staging_dir, &plan.final_dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&plan.staging_dir);
        IoError::publish(&plan.staging_dir, &plan.final_dir, e)
    })?;

    Ok(plan.final_dir)
}

fn write_staged(bundle: &Bundle, staging: &Path) -> Result<()> {
    std::fs::create_dir(staging).map_err(|e| IoError::create_dir(staging, e))?;

    for (path, bytes) in &bundle.members {
        if let Some(detail) = paths::member_path_rule_broken(path) {
            return Err(PathError::UnacceptablePath {
                path: path.clone(),
                detail,
            }
            .into());
        }
        let target = staging.join(path);
        if let Some(parent) = target.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| IoError::create_dir(parent, e))?;
            }
        }
        write_file(&target, bytes)?;
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path).map_err(|e| IoError::write(path, e))?;
    file.write_all(bytes).map_err(|e| IoError::write(path, e))?;
    // Flush to the device before the rename, so a bundle that exists is a bundle that
    // is complete even if the machine loses power immediately afterwards.
    file.sync_all().map_err(|e| IoError::write(path, e))?;
    Ok(())
}

/// Which interval the report should describe, in words.
pub(crate) fn interval_description(solution: &Solution, confidence: f64) -> String {
    let percent = confidence * 100.0;
    match solution.interval_source {
        IntervalSource::Analytic => {
            format!("{percent:.1}% analytic interval from the covariance of the linear estimator")
        }
        IntervalSource::Bootstrap => format!(
            "{percent:.1}% bootstrap percentile interval from {} seeded resamples",
            solution.bootstrap.successful_samples
        ),
        IntervalSource::None => "none: Ordinata refused to publish an interval".to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    #[test]
    fn numbers_are_formatted_short_and_reparse_exactly() {
        let cases = [
            (1.0, "1"),
            (0.1, "0.1"),
            (-0.0, "0"),
            (1e300, "1e300"),
            (1e-300, "1e-300"),
            (1234.5, "1234.5"),
            (5e-324, "5e-324"),
        ];
        for (value, expected) in cases {
            let text = format_number(value);
            assert_eq!(text, expected, "{value:?}");
            assert!(
                text.len() <= spec::MAX_FIELD_BYTES,
                "a normalized CSV field must stay re-ingestable: {text}"
            );
        }
    }

    #[test]
    fn every_double_reparses_from_its_formatting() {
        // The extremes are named rather than spelled out: `f64::MAX` says what it is,
        // and a hand-typed 1.7976931348623157e308 says only that somebody typed it.
        let values = [
            0.0,
            1.0 / 3.0,
            std::f64::consts::PI,
            -f64::MAX,
            f64::MAX,
            f64::MIN_POSITIVE,
            5e-324, // the smallest subnormal
            123_456_789.987_654_32,
        ];
        for value in values {
            let text = format_number(value);
            let back: f64 = text.parse().unwrap();
            assert_eq!(back.to_bits(), value.to_bits(), "{value:?} -> {text}");
            assert!(text.len() <= spec::MAX_FIELD_BYTES, "{text}");
        }
    }

    #[test]
    fn the_content_and_payload_lists_are_sorted_and_consistent() {
        let mut sorted = CONTENT_MEMBERS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            CONTENT_MEMBERS.to_vec(),
            "CONTENT_MEMBERS must be sorted"
        );

        let mut payload = PAYLOAD_MEMBERS.to_vec();
        payload.sort_unstable();
        assert_eq!(
            payload,
            PAYLOAD_MEMBERS.to_vec(),
            "PAYLOAD_MEMBERS must be sorted"
        );

        // Everything hashed into the result content must actually be in the bundle.
        for member in CONTENT_MEMBERS {
            assert!(
                PAYLOAD_MEMBERS.contains(&member),
                "{member} is not in the payload"
            );
        }
        // The report is payload but not content, because it carries the clock.
        assert!(PAYLOAD_MEMBERS.contains(&"report.html"));
        assert!(!CONTENT_MEMBERS.contains(&"report.html"));
    }

    #[test]
    fn every_member_path_is_a_safe_relative_path() {
        for member in PAYLOAD_MEMBERS
            .iter()
            .chain(std::iter::once(&MANIFEST))
            .chain(std::iter::once(&CHECKSUMS))
        {
            assert!(
                paths::member_path_rule_broken(member).is_none(),
                "{member} is not a safe member path"
            );
        }
    }
}
