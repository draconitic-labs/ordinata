//! The `project.json` schema, version 1.
//!
//! Deserialization is strict: unknown fields are refused rather than ignored, so a
//! misspelled key is a loud error instead of a silently dropped setting. Every
//! numeric field is range-checked here, before any file is opened and long before
//! any arithmetic happens.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{InputError, Result};
use crate::spec;
use crate::text::{check_free_text, check_optional_text};

/// A `project.json` exactly as it was written, before anything is resolved.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectFile {
    /// Must be [`spec::PROJECT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// What this problem is.
    pub title: String,
    /// Optional longer prose.
    #[serde(default)]
    pub description: Option<String>,
    /// Relative path to the design matrix.
    pub design_csv: String,
    /// Relative path to the observations.
    pub observations_csv: String,
    /// How, and whether, to regularize.
    pub regularization: RegularizationSpec,
    /// Which uncertainty to attempt, and at what confidence.
    pub uncertainty: UncertaintySpec,
    /// What the user is asserting about the model. Never empty.
    pub assumptions: Vec<String>,
    /// What the user is asserting the result cannot support. Never empty.
    pub limitations: Vec<String>,
    /// Citations, as labels the user supplies. Ordinata never fetches them.
    #[serde(default)]
    pub sources: Vec<SourceCitation>,
    /// User-declared thresholds. These are the user's checks, not universal truth.
    #[serde(default)]
    pub validation: Option<ValidationSpec>,
}

/// Whether lambda is given or searched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RegularizationMode {
    /// The project states lambda outright.
    Explicit,
    /// Lambda is chosen by generalized cross-validation over a declared grid.
    Gcv,
}

impl RegularizationMode {
    /// The wire name, for messages and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            RegularizationMode::Explicit => "explicit",
            RegularizationMode::Gcv => "gcv",
        }
    }
}

/// The regularization block.
///
/// The fields are flat rather than an internally tagged enum so that a project that
/// sets `lambda` while asking for `gcv` gets a message naming the offending field,
/// instead of a generic "data did not match any variant".
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegularizationSpec {
    /// `explicit` or `gcv`.
    pub mode: RegularizationMode,
    /// The lambda to use, when the mode is `explicit`. May be zero.
    #[serde(default)]
    pub lambda: Option<f64>,
    /// Smallest lambda on the search grid, when the mode is `gcv`.
    #[serde(default)]
    pub min_lambda: Option<f64>,
    /// Largest lambda on the search grid, when the mode is `gcv`.
    #[serde(default)]
    pub max_lambda: Option<f64>,
    /// How many grid points, inclusive of both endpoints.
    #[serde(default)]
    pub steps: Option<usize>,
    /// The point the penalty pulls toward, per parameter. Absent means zero.
    #[serde(default)]
    pub prior: Option<BTreeMap<String, f64>>,
    /// The diagonal penalty weight, per parameter. Absent means one.
    #[serde(default)]
    pub penalty: Option<BTreeMap<String, f64>>,
}

/// Which uncertainty the project wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UncertaintyMethod {
    /// Closed-form covariance of the linear estimator.
    Analytic,
    /// Seeded residual bootstrap.
    Bootstrap,
    /// Both, reported separately.
    Both,
}

impl UncertaintyMethod {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            UncertaintyMethod::Analytic => "analytic",
            UncertaintyMethod::Bootstrap => "bootstrap",
            UncertaintyMethod::Both => "both",
        }
    }

    /// Does this method ask for the analytic covariance?
    pub fn wants_analytic(self) -> bool {
        matches!(self, UncertaintyMethod::Analytic | UncertaintyMethod::Both)
    }

    /// Does this method ask for the bootstrap?
    pub fn wants_bootstrap(self) -> bool {
        matches!(self, UncertaintyMethod::Bootstrap | UncertaintyMethod::Both)
    }
}

/// The uncertainty block.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UncertaintySpec {
    /// Which uncertainty to attempt.
    pub method: UncertaintyMethod,
    /// Interval coverage, between [`spec::MIN_CONFIDENCE`] and [`spec::MAX_CONFIDENCE`].
    pub confidence_level: f64,
    /// How many bootstrap resamples. Required exactly when the bootstrap is wanted.
    #[serde(default)]
    pub bootstrap_samples: Option<usize>,
    /// The seed. Required exactly when the bootstrap is wanted, because an unseeded
    /// bootstrap would make the bundle irreproducible.
    #[serde(default)]
    pub bootstrap_seed: Option<u64>,
}

/// A citation. Ordinata stores it and prints it. It never resolves it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceCitation {
    /// What the user calls this source.
    pub label: String,
    /// Where the user says it lives. Recorded as text, never fetched, never linked.
    #[serde(default)]
    pub uri: Option<String>,
    /// An ISO `YYYY-MM-DD` date.
    #[serde(default)]
    pub date: Option<String>,
}

/// The user's declared thresholds.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ValidationSpec {
    /// Largest weighted RMSE the user is willing to accept.
    #[serde(default)]
    pub weighted_rmse_max: Option<Threshold>,
    /// Largest condition number the user is willing to accept.
    #[serde(default)]
    pub condition_number_max: Option<Threshold>,
}

/// A threshold, and where the user got it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Threshold {
    /// The number to compare against.
    pub value: f64,
    /// Where the user says this number came from. Free text.
    #[serde(default)]
    pub source: Option<String>,
}

fn invalid(field: &str, detail: impl Into<String>) -> crate::Error {
    InputError::InvalidField {
        field: field.to_string(),
        detail: detail.into(),
    }
    .into()
}

fn require_finite(field: &str, value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid(field, format!("{value:?} is not a finite number")))
    }
}

/// Parse `project.json`.
///
/// The schema version is read before anything else so that a project written for a
/// future Ordinata is refused by version rather than by a confusing field error.
///
/// Parsing goes through [`crate::canonical::parse_strict`], so a project that names
/// the same key twice is refused rather than silently resolved in favour of whichever
/// one came last. `deny_unknown_fields` below then catches a key that is spelled
/// wrong; between them, a project means exactly one thing or it means nothing.
///
/// # Errors
///
/// Returns [`InputError::ProjectSchema`] or [`InputError::UnsupportedSchemaVersion`].
pub(crate) fn parse(bytes: &[u8]) -> Result<ProjectFile> {
    let value = crate::canonical::parse_strict(bytes)
        .map_err(|detail| InputError::ProjectSchema { detail })?;
    let declared = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| InputError::ProjectSchema {
            detail: "schemaVersion is missing or is not a whole number".into(),
        })?;
    let declared = u32::try_from(declared).unwrap_or(u32::MAX);
    if declared != spec::PROJECT_SCHEMA_VERSION {
        return Err(InputError::UnsupportedSchemaVersion {
            found: declared,
            expected: spec::PROJECT_SCHEMA_VERSION,
        }
        .into());
    }

    let project: ProjectFile =
        serde_json::from_value(value).map_err(|e| InputError::ProjectSchema {
            detail: e.to_string(),
        })?;
    validate(&project)?;
    Ok(project)
}

/// Range-check every field. Names in `prior` and `penalty` are checked later, once
/// the design has told us which parameters exist.
fn validate(project: &ProjectFile) -> Result<()> {
    check_free_text("title", &project.title, spec::MAX_TITLE_BYTES)?;
    check_optional_text("description", project.description.as_ref())?;

    for (field, items) in [
        ("assumptions", &project.assumptions),
        ("limitations", &project.limitations),
    ] {
        if items.is_empty() {
            return Err(invalid(
                field,
                "must list at least one entry; a result with no declared assumptions is not reviewable",
            ));
        }
        if items.len() > spec::MAX_TEXT_ITEMS {
            return Err(invalid(
                field,
                format!(
                    "has {} entries, more than the {} allowed",
                    items.len(),
                    spec::MAX_TEXT_ITEMS
                ),
            ));
        }
        for (i, item) in items.iter().enumerate() {
            check_free_text(&format!("{field}[{i}]"), item, spec::MAX_TEXT_BYTES)?;
        }
    }

    if project.sources.len() > spec::MAX_SOURCES {
        return Err(invalid(
            "sources",
            format!(
                "has {} entries, more than the {} allowed",
                project.sources.len(),
                spec::MAX_SOURCES
            ),
        ));
    }
    for (i, source) in project.sources.iter().enumerate() {
        check_free_text(
            &format!("sources[{i}].label"),
            &source.label,
            spec::MAX_TITLE_BYTES,
        )?;
        if let Some(uri) = &source.uri {
            check_free_text(&format!("sources[{i}].uri"), uri, 500)?;
        }
        if let Some(date) = &source.date {
            check_iso_date(&format!("sources[{i}].date"), date)?;
        }
    }

    validate_regularization(&project.regularization)?;
    validate_uncertainty(&project.uncertainty)?;

    if let Some(validation) = &project.validation {
        if let Some(t) = &validation.weighted_rmse_max {
            let value = require_finite("validation.weightedRmseMax.value", t.value)?;
            if value <= 0.0 {
                return Err(invalid(
                    "validation.weightedRmseMax.value",
                    "must be greater than zero",
                ));
            }
            check_optional_text("validation.weightedRmseMax.source", t.source.as_ref())?;
        }
        if let Some(t) = &validation.condition_number_max {
            let value = require_finite("validation.conditionNumberMax.value", t.value)?;
            if value < 1.0 {
                return Err(invalid(
                    "validation.conditionNumberMax.value",
                    "must be at least 1, because no matrix has a condition number below 1",
                ));
            }
            check_optional_text("validation.conditionNumberMax.source", t.source.as_ref())?;
        }
    }

    Ok(())
}

fn validate_regularization(reg: &RegularizationSpec) -> Result<()> {
    match reg.mode {
        RegularizationMode::Explicit => {
            let lambda = reg.lambda.ok_or_else(|| {
                invalid("regularization.lambda", "is required when mode is explicit")
            })?;
            let lambda = require_finite("regularization.lambda", lambda)?;
            if lambda < 0.0 {
                return Err(invalid("regularization.lambda", "must not be negative"));
            }
            for (field, present) in [
                ("regularization.minLambda", reg.min_lambda.is_some()),
                ("regularization.maxLambda", reg.max_lambda.is_some()),
                ("regularization.steps", reg.steps.is_some()),
            ] {
                if present {
                    return Err(invalid(field, "must not be set when mode is explicit"));
                }
            }
        }
        RegularizationMode::Gcv => {
            if reg.lambda.is_some() {
                return Err(invalid(
                    "regularization.lambda",
                    "must not be set when mode is gcv, because gcv is what chooses it",
                ));
            }
            let min = reg.min_lambda.ok_or_else(|| {
                invalid("regularization.minLambda", "is required when mode is gcv")
            })?;
            let max = reg.max_lambda.ok_or_else(|| {
                invalid("regularization.maxLambda", "is required when mode is gcv")
            })?;
            let steps = reg
                .steps
                .ok_or_else(|| invalid("regularization.steps", "is required when mode is gcv"))?;

            let min = require_finite("regularization.minLambda", min)?;
            let max = require_finite("regularization.maxLambda", max)?;
            // The grid is laid out in log space, so a zero or negative endpoint has
            // no logarithm and the grid would not exist.
            if min <= 0.0 {
                return Err(invalid(
                    "regularization.minLambda",
                    "must be greater than zero, because the grid is spaced in log space",
                ));
            }
            if max <= 0.0 {
                return Err(invalid(
                    "regularization.maxLambda",
                    "must be greater than zero",
                ));
            }
            if min >= max {
                return Err(invalid(
                    "regularization.minLambda",
                    "must be strictly less than maxLambda",
                ));
            }
            if !(spec::MIN_GCV_STEPS..=spec::MAX_GCV_STEPS).contains(&steps) {
                return Err(InputError::CountOutOfRange {
                    what: "regularization.steps".into(),
                    found: steps,
                    min: spec::MIN_GCV_STEPS,
                    max: spec::MAX_GCV_STEPS,
                }
                .into());
            }
        }
    }

    if let Some(prior) = &reg.prior {
        for (name, value) in prior {
            require_finite(&format!("regularization.prior[{name}]"), *value)?;
        }
    }
    if let Some(penalty) = &reg.penalty {
        for (name, value) in penalty {
            let value = require_finite(&format!("regularization.penalty[{name}]"), *value)?;
            if value <= 0.0 {
                return Err(invalid(
                    &format!("regularization.penalty[{name}]"),
                    "must be greater than zero; a zero penalty leaves the parameter unregularized, which is not what a diagonal penalty means",
                ));
            }
        }
    }
    Ok(())
}

fn validate_uncertainty(unc: &UncertaintySpec) -> Result<()> {
    let level = require_finite("uncertainty.confidenceLevel", unc.confidence_level)?;
    if !(spec::MIN_CONFIDENCE..=spec::MAX_CONFIDENCE).contains(&level) {
        return Err(invalid(
            "uncertainty.confidenceLevel",
            format!(
                "must be between {} and {}",
                spec::MIN_CONFIDENCE,
                spec::MAX_CONFIDENCE
            ),
        ));
    }

    if unc.method.wants_bootstrap() {
        let samples = unc.bootstrap_samples.ok_or_else(|| {
            invalid(
                "uncertainty.bootstrapSamples",
                "is required when the method asks for a bootstrap",
            )
        })?;
        if !(spec::MIN_BOOTSTRAP_SAMPLES..=spec::MAX_BOOTSTRAP_SAMPLES).contains(&samples) {
            return Err(InputError::CountOutOfRange {
                what: "uncertainty.bootstrapSamples".into(),
                found: samples,
                min: spec::MIN_BOOTSTRAP_SAMPLES,
                max: spec::MAX_BOOTSTRAP_SAMPLES,
            }
            .into());
        }
        if unc.bootstrap_seed.is_none() {
            return Err(invalid(
                "uncertainty.bootstrapSeed",
                "is required when the method asks for a bootstrap; an unseeded bootstrap would make the bundle irreproducible",
            ));
        }
    } else {
        for (field, present) in [
            (
                "uncertainty.bootstrapSamples",
                unc.bootstrap_samples.is_some(),
            ),
            ("uncertainty.bootstrapSeed", unc.bootstrap_seed.is_some()),
        ] {
            if present {
                return Err(invalid(
                    field,
                    "must not be set when the method does not ask for a bootstrap",
                ));
            }
        }
    }
    Ok(())
}

fn check_iso_date(field: &str, value: &str) -> Result<()> {
    let bad = || invalid(field, "must be an ISO date of the form YYYY-MM-DD");
    let bytes = value.as_bytes();
    if bytes.len() != 10 {
        return Err(bad());
    }
    for (i, b) in bytes.iter().enumerate() {
        let ok = match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        };
        if !ok {
            return Err(bad());
        }
    }
    let part = |from: usize, to: usize| -> u32 {
        value
            .get(from..to)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let (month, day) = (part(5, 7), part(8, 10));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(bad());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    fn base() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "title": "T",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": { "mode": "explicit", "lambda": 0.0 },
            "uncertainty": { "method": "analytic", "confidenceLevel": 0.95 },
            "assumptions": ["a"],
            "limitations": ["l"]
        })
    }

    fn parse_value(v: &serde_json::Value) -> Result<ProjectFile> {
        parse(v.to_string().as_bytes())
    }

    #[test]
    fn a_minimal_project_parses() {
        let project = parse_value(&base()).unwrap();
        assert_eq!(project.title, "T");
        assert_eq!(project.regularization.mode, RegularizationMode::Explicit);
    }

    #[test]
    fn unknown_fields_are_refused_rather_than_ignored() {
        let mut v = base();
        v["confidenceLevl"] = serde_json::json!(0.9);
        let err = parse_value(&v).unwrap_err();
        assert_eq!(err.code(), "project_schema");
        assert!(err.to_string().contains("confidenceLevl"), "{err}");
    }

    #[test]
    fn a_project_that_names_a_key_twice_is_refused() {
        // Hand-written rather than built through `json!`, because a `Value` cannot hold
        // a duplicate key: the whole point is what happens at the *text* boundary.
        // Without the strict parser serde would keep lambda = 1e9 and solve a problem
        // nobody asked for, with a project.json whose first line says otherwise.
        let hostile = r#"{
            "schemaVersion": 1,
            "title": "T",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": { "mode": "explicit", "lambda": 0.0, "lambda": 1e9 },
            "uncertainty": { "method": "analytic", "confidenceLevel": 0.95 },
            "assumptions": ["a"],
            "limitations": ["l"]
        }"#;
        let err = parse(hostile.as_bytes()).unwrap_err();
        assert_eq!(err.code(), "project_schema");
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("lambda"), "{err}");
    }

    #[test]
    fn a_duplicated_top_level_key_is_refused_too() {
        let hostile = r#"{
            "schemaVersion": 1,
            "title": "T",
            "title": "something else entirely",
            "designCsv": "design.csv",
            "observationsCsv": "observations.csv",
            "regularization": { "mode": "explicit", "lambda": 0.0 },
            "uncertainty": { "method": "analytic", "confidenceLevel": 0.95 },
            "assumptions": ["a"],
            "limitations": ["l"]
        }"#;
        let err = parse(hostile.as_bytes()).unwrap_err();
        assert_eq!(err.code(), "project_schema");
        assert!(err.to_string().contains("title"), "{err}");
    }

    #[test]
    fn a_future_schema_version_is_refused_by_version() {
        let mut v = base();
        v["schemaVersion"] = serde_json::json!(2);
        let err = parse_value(&v).unwrap_err();
        assert_eq!(err.code(), "unsupported_schema_version");
    }

    #[test]
    fn explicit_and_gcv_fields_may_not_be_mixed() {
        let mut v = base();
        v["regularization"] = serde_json::json!({ "mode": "gcv", "lambda": 1.0, "minLambda": 1e-6, "maxLambda": 1.0, "steps": 5 });
        let err = parse_value(&v).unwrap_err();
        assert_eq!(err.code(), "invalid_field");
        assert!(err.to_string().contains("regularization.lambda"), "{err}");

        let mut v = base();
        v["regularization"] = serde_json::json!({ "mode": "explicit", "lambda": 1.0, "steps": 5 });
        let err = parse_value(&v).unwrap_err();
        assert!(err.to_string().contains("regularization.steps"), "{err}");
    }

    #[test]
    fn a_gcv_grid_must_be_a_real_interval_in_log_space() {
        for (min, max, steps, why) in [
            (0.0, 1.0, 5, "zero lower endpoint has no logarithm"),
            (-1.0, 1.0, 5, "negative lower endpoint"),
            (1.0, 1.0, 5, "empty interval"),
            (10.0, 1.0, 5, "inverted interval"),
            (1e-6, 1.0, 2, "too few steps"),
            (1e-6, 1.0, 201, "too many steps"),
        ] {
            let mut v = base();
            v["regularization"] = serde_json::json!({ "mode": "gcv", "minLambda": min, "maxLambda": max, "steps": steps });
            assert!(parse_value(&v).is_err(), "should refuse: {why}");
        }
    }

    #[test]
    fn a_negative_lambda_is_refused_but_zero_is_allowed() {
        let mut v = base();
        v["regularization"] = serde_json::json!({ "mode": "explicit", "lambda": -1.0 });
        assert!(parse_value(&v).is_err());

        let mut v = base();
        v["regularization"] = serde_json::json!({ "mode": "explicit", "lambda": 0.0 });
        assert!(
            parse_value(&v).is_ok(),
            "an unregularized fit is a legitimate request"
        );
    }

    #[test]
    fn a_bootstrap_without_a_seed_is_refused() {
        let mut v = base();
        v["uncertainty"] = serde_json::json!({ "method": "bootstrap", "confidenceLevel": 0.95, "bootstrapSamples": 500 });
        let err = parse_value(&v).unwrap_err();
        assert!(err.to_string().contains("bootstrapSeed"), "{err}");
    }

    #[test]
    fn bootstrap_settings_are_refused_when_no_bootstrap_was_asked_for() {
        let mut v = base();
        v["uncertainty"] = serde_json::json!({ "method": "analytic", "confidenceLevel": 0.95, "bootstrapSamples": 500, "bootstrapSeed": 1 });
        assert!(parse_value(&v).is_err());
    }

    #[test]
    fn bootstrap_sample_count_is_bounded_on_both_sides() {
        for samples in [1_u64, 99, 10_001, 1_000_000] {
            let mut v = base();
            v["uncertainty"] = serde_json::json!({ "method": "bootstrap", "confidenceLevel": 0.95, "bootstrapSamples": samples, "bootstrapSeed": 7 });
            let err = parse_value(&v).unwrap_err();
            assert_eq!(err.code(), "count_out_of_range", "{samples}");
        }
    }

    #[test]
    fn confidence_level_is_bounded() {
        for level in [0.0, 0.49, 0.9999, 1.0, 1.5] {
            let mut v = base();
            v["uncertainty"] =
                serde_json::json!({ "method": "analytic", "confidenceLevel": level });
            assert!(parse_value(&v).is_err(), "{level} should be refused");
        }
    }

    #[test]
    fn assumptions_and_limitations_may_not_be_empty() {
        for field in ["assumptions", "limitations"] {
            let mut v = base();
            v[field] = serde_json::json!([]);
            let err = parse_value(&v).unwrap_err();
            assert!(err.to_string().contains(field), "{err}");
        }
    }

    #[test]
    fn a_zero_penalty_is_refused() {
        let mut v = base();
        v["regularization"] =
            serde_json::json!({ "mode": "explicit", "lambda": 1.0, "penalty": { "a": 0.0 } });
        assert!(parse_value(&v).is_err());
        let mut v = base();
        v["regularization"] =
            serde_json::json!({ "mode": "explicit", "lambda": 1.0, "penalty": { "a": -2.0 } });
        assert!(parse_value(&v).is_err());
    }

    #[test]
    fn html_and_formula_text_survives_in_free_text_because_the_report_escapes_it() {
        let mut v = base();
        v["title"] = serde_json::json!("Fit of <b>x</b> & y where x > 3");
        v["assumptions"] = serde_json::json!(["=cmd|' /C calc'!A0 is a string, not a formula"]);
        let project = parse_value(&v).unwrap();
        assert!(project.title.contains('<'));
    }

    #[test]
    fn control_characters_in_free_text_are_refused() {
        let mut v = base();
        v["title"] = serde_json::json!("bad\u{0}title");
        assert!(parse_value(&v).is_err());
    }

    #[test]
    fn iso_dates_are_checked() {
        assert!(check_iso_date("d", "2024-01-31").is_ok());
        for bad in [
            "2024-1-1",
            "24-01-01",
            "2024-13-01",
            "2024-01-32",
            "not a date",
            "2024-01-01T00:00:00Z",
        ] {
            assert!(check_iso_date("d", bad).is_err(), "{bad}");
        }
    }
}
