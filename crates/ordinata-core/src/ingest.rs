//! CSV ingestion and the normalized problem.
//!
//! Two things happen here that the rest of the library then gets to take for granted.
//!
//! **Rows are joined by ID, never by position.** The design and the observations must
//! name exactly the same set of observation IDs, one to one. A file whose rows are in
//! a different order is not a different problem.
//!
//! **The problem is put into canonical order**: observation rows ascending by ID and
//! parameter columns ascending by name, both by UTF-8 byte order. This is not
//! cosmetic. It means that permuting the rows of `observations.csv`, or the columns
//! of `design.csv`, produces a *bit-identical* matrix, and therefore a bit-identical
//! solve and a bit-identical content hash. Permutation invariance is a structural
//! property of the normalization rather than a numerical coincidence that a test has
//! to keep checking to a tolerance.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

use nalgebra::{DMatrix, DVector};

use crate::canonical;
use crate::error::{Error, InputError, IoError, NumericalError, Result};
use crate::paths;
use crate::project::{
    ProjectFile, RegularizationMode, SourceCitation, UncertaintyMethod, UncertaintySpec,
    ValidationSpec,
};
use crate::spec;
use crate::text::{for_message, identifier_rule_broken};

/// How lambda will be chosen, after the project has been range-checked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LambdaPlan {
    /// Use exactly this lambda. May be zero.
    Explicit(f64),
    /// Search this inclusive log-spaced grid by generalized cross-validation.
    Gcv {
        /// Smallest candidate, and the exact first grid point.
        min: f64,
        /// Largest candidate, and the exact last grid point.
        max: f64,
        /// How many candidates, inclusive of both endpoints.
        steps: usize,
    },
}

/// The bootstrap settings, once they are known to be present.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapPlan {
    /// How many resamples the project asked for. Ordinata never quietly reduces this.
    pub samples: usize,
    /// The seed the project declared.
    pub seed: u64,
}

/// What the project asked for, in resolved form.
#[derive(Debug, Clone)]
pub struct UncertaintyPlan {
    /// Which uncertainty to attempt.
    pub method: UncertaintyMethod,
    /// Interval coverage.
    pub confidence_level: f64,
    /// Present exactly when the method asks for a bootstrap.
    pub bootstrap: Option<BootstrapPlan>,
}

/// One of the files the problem came from, recorded for provenance.
#[derive(Debug, Clone)]
pub struct SourceInput {
    /// `project`, `design`, or `observations`.
    pub role: String,
    /// The path as the project declared it, or a synthetic name for the demo.
    pub path: String,
    /// SHA-256 of the bytes exactly as they were supplied.
    pub sha256: String,
    /// Size of those bytes.
    pub bytes: u64,
}

/// A normalized, fully checked problem. Everything downstream reads this and nothing
/// else.
#[derive(Debug, Clone)]
pub struct Problem {
    /// What the user called this problem.
    pub title: String,
    /// Optional prose.
    pub description: Option<String>,
    /// Observation IDs, ascending.
    pub observation_ids: Vec<String>,
    /// Parameter names, ascending.
    pub parameters: Vec<String>,
    /// The design matrix, rows and columns in canonical order.
    pub design: DMatrix<f64>,
    /// Observed values, in canonical row order.
    pub values: DVector<f64>,
    /// Declared standard errors, all finite and positive.
    pub standard_errors: DVector<f64>,
    /// The point the penalty pulls toward, aligned to `parameters`. Default zero.
    pub prior: DVector<f64>,
    /// The diagonal penalty weight, aligned to `parameters`. All positive. Default one.
    pub penalty: DVector<f64>,
    /// Whether a prior was declared, as opposed to defaulted.
    pub prior_declared: bool,
    /// Whether a penalty was declared, as opposed to defaulted.
    pub penalty_declared: bool,
    /// How lambda will be chosen.
    pub lambda_plan: LambdaPlan,
    /// What uncertainty to attempt.
    pub uncertainty: UncertaintyPlan,
    /// What the user asserts about the model.
    pub assumptions: Vec<String>,
    /// What the user asserts the result cannot support.
    pub limitations: Vec<String>,
    /// Citations, as supplied. Never fetched.
    pub sources: Vec<SourceCitation>,
    /// The user's declared thresholds.
    pub validation: Option<ValidationSpec>,
    /// The files this came from.
    pub source_inputs: Vec<SourceInput>,
    /// Where those files actually live, resolved and canonical. Empty for the demo,
    /// which has no files. A bundle write refuses to overlap any of these.
    pub input_paths: Vec<std::path::PathBuf>,
    /// True only for the built-in demo. Never true for user data.
    pub synthetic: bool,
}

impl Problem {
    /// Number of matched observations.
    pub fn rows(&self) -> usize {
        self.observation_ids.len()
    }

    /// Number of parameters.
    pub fn columns(&self) -> usize {
        self.parameters.len()
    }
}

/// A parsed `design.csv`, still in file order.
struct DesignTable {
    parameters: Vec<String>,
    ids: Vec<String>,
    rows: Vec<Vec<f64>>,
}

/// A parsed `observations.csv`, still in file order.
struct ObservationTable {
    ids: Vec<String>,
    values: Vec<f64>,
    standard_errors: Vec<f64>,
}

fn malformed(file: &str, detail: impl Into<String>) -> Error {
    InputError::CsvMalformed {
        file: file.to_string(),
        detail: detail.into(),
    }
    .into()
}

/// Turn a `csv` crate error into one of ours, keeping the distinction between a file
/// that is not UTF-8 and a file whose shape is wrong.
fn csv_error(file: &str, error: &csv::Error) -> Error {
    match error.kind() {
        csv::ErrorKind::Utf8 { .. } => malformed(
            file,
            "it is not valid UTF-8; Ordinata reads only UTF-8 text",
        ),
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => {
            let line = pos.as_ref().map_or(0, csv::Position::line);
            malformed(
                file,
                format!("line {line} has {len} fields but the header declares {expected_len}"),
            )
        }
        _ => malformed(file, error.to_string()),
    }
}

fn check_identifier(file: &str, context: &str, value: &str) -> Result<()> {
    if let Some(detail) = identifier_rule_broken(value) {
        return Err(InputError::InvalidIdentifier {
            file: file.to_string(),
            context: context.to_string(),
            value: for_message(value),
            detail: detail.to_string(),
        }
        .into());
    }
    Ok(())
}

fn parse_cell(file: &str, row: usize, column: &str, raw: &str) -> Result<f64> {
    if raw.len() > spec::MAX_FIELD_BYTES {
        return Err(malformed(
            file,
            format!(
                "row {row}, column {column:?}: the field is {} bytes, more than the {} allowed",
                raw.len(),
                spec::MAX_FIELD_BYTES
            ),
        ));
    }
    // `f64::from_str` happily accepts "NaN", "inf", and "1e999" (which overflows to
    // an infinity). The finiteness check below is what refuses all three.
    let parsed: f64 = raw.parse().map_err(|_| InputError::NonFiniteCell {
        file: file.to_string(),
        row,
        column: column.to_string(),
        value: for_message(raw),
    })?;
    if !parsed.is_finite() {
        return Err(InputError::NonFiniteCell {
            file: file.to_string(),
            row,
            column: column.to_string(),
            value: for_message(raw),
        }
        .into());
    }
    Ok(parsed)
}

fn reader<R: Read>(source: R) -> csv::Reader<R> {
    csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(source)
}

fn parse_design<R: Read>(source: R, file: &str) -> Result<DesignTable> {
    let mut rdr = reader(source);
    let headers = rdr.headers().map_err(|e| csv_error(file, &e))?.clone();
    if headers.is_empty() {
        return Err(InputError::CsvEmpty {
            file: file.to_string(),
        }
        .into());
    }
    let mut fields = headers.iter();
    match fields.next() {
        Some("observation_id") => {}
        other => {
            return Err(InputError::CsvHeader {
                file: file.to_string(),
                detail: format!(
                    "the first column must be named observation_id, not {:?}",
                    for_message(other.unwrap_or(""))
                ),
            }
            .into())
        }
    }

    let parameters: Vec<String> = fields.map(str::to_string).collect();
    if !(spec::MIN_PARAMETERS..=spec::MAX_PARAMETERS).contains(&parameters.len()) {
        return Err(InputError::CountOutOfRange {
            what: format!("parameter columns in {file}"),
            found: parameters.len(),
            min: spec::MIN_PARAMETERS,
            max: spec::MAX_PARAMETERS,
        }
        .into());
    }
    let mut seen = BTreeSet::new();
    for name in &parameters {
        check_identifier(file, "parameter name", name)?;
        if !seen.insert(name.clone()) {
            return Err(InputError::DuplicateIdentifier {
                file: file.to_string(),
                context: "parameter name".into(),
                value: name.clone(),
            }
            .into());
        }
    }

    let mut ids = Vec::new();
    let mut rows: Vec<Vec<f64>> = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for (index, record) in rdr.records().enumerate() {
        let record = record.map_err(|e| csv_error(file, &e))?;
        let row_number = index + 1;
        if rows.len() >= spec::MAX_OBSERVATIONS {
            return Err(InputError::CountOutOfRange {
                what: format!("data rows in {file}"),
                found: rows.len() + 1,
                min: spec::MIN_OBSERVATIONS,
                max: spec::MAX_OBSERVATIONS,
            }
            .into());
        }
        let id = record.get(0).unwrap_or("");
        check_identifier(file, "observation_id", id)?;
        if !seen_ids.insert(id.to_string()) {
            return Err(InputError::DuplicateIdentifier {
                file: file.to_string(),
                context: "observation_id".into(),
                value: id.to_string(),
            }
            .into());
        }
        let mut values = Vec::with_capacity(parameters.len());
        for (column_index, name) in parameters.iter().enumerate() {
            let raw = record.get(column_index + 1).unwrap_or("");
            values.push(parse_cell(file, row_number, name, raw)?);
        }
        ids.push(id.to_string());
        rows.push(values);
    }

    if rows.is_empty() {
        return Err(InputError::CsvEmpty {
            file: file.to_string(),
        }
        .into());
    }
    Ok(DesignTable {
        parameters,
        ids,
        rows,
    })
}

fn parse_observations<R: Read>(source: R, file: &str) -> Result<ObservationTable> {
    const EXPECTED: [&str; 3] = ["observation_id", "value", "standard_error"];
    let mut rdr = reader(source);
    let headers = rdr.headers().map_err(|e| csv_error(file, &e))?.clone();
    if headers.is_empty() {
        return Err(InputError::CsvEmpty {
            file: file.to_string(),
        }
        .into());
    }
    let found: Vec<&str> = headers.iter().collect();
    if found != EXPECTED {
        return Err(InputError::CsvHeader {
            file: file.to_string(),
            detail: format!(
                "the header must be exactly observation_id,value,standard_error, not {:?}",
                for_message(&found.join(","))
            ),
        }
        .into());
    }

    let mut ids = Vec::new();
    let mut values = Vec::new();
    let mut standard_errors = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, record) in rdr.records().enumerate() {
        let record = record.map_err(|e| csv_error(file, &e))?;
        let row_number = index + 1;
        if ids.len() >= spec::MAX_OBSERVATIONS {
            return Err(InputError::CountOutOfRange {
                what: format!("data rows in {file}"),
                found: ids.len() + 1,
                min: spec::MIN_OBSERVATIONS,
                max: spec::MAX_OBSERVATIONS,
            }
            .into());
        }
        let id = record.get(0).unwrap_or("");
        check_identifier(file, "observation_id", id)?;
        if !seen.insert(id.to_string()) {
            return Err(InputError::DuplicateIdentifier {
                file: file.to_string(),
                context: "observation_id".into(),
                value: id.to_string(),
            }
            .into());
        }
        let value = parse_cell(file, row_number, "value", record.get(1).unwrap_or(""))?;

        let raw_error = record.get(2).unwrap_or("");
        let standard_error =
            parse_cell(file, row_number, "standard_error", raw_error).map_err(|_| {
                InputError::InvalidStandardError {
                    file: file.to_string(),
                    row: row_number,
                    value: for_message(raw_error),
                }
            })?;
        // A zero or negative standard error is not a small uncertainty. It is a
        // statement that the observation is infinitely informative, and it would put
        // an infinity into the weight matrix.
        if standard_error <= 0.0 {
            return Err(InputError::InvalidStandardError {
                file: file.to_string(),
                row: row_number,
                value: for_message(raw_error),
            }
            .into());
        }

        ids.push(id.to_string());
        values.push(value);
        standard_errors.push(standard_error);
    }

    if ids.is_empty() {
        return Err(InputError::CsvEmpty {
            file: file.to_string(),
        }
        .into());
    }
    Ok(ObservationTable {
        ids,
        values,
        standard_errors,
    })
}

/// Resolve a prior or penalty map against the parameters the design actually has.
///
/// A map that is present must name every parameter and no others. Half-specifying a
/// prior is almost always a typo, and silently defaulting the rest would bury it.
fn resolve_parameter_map(
    field: &str,
    declared: Option<&BTreeMap<String, f64>>,
    parameters: &[String],
    default: f64,
) -> Result<(DVector<f64>, bool)> {
    let Some(map) = declared else {
        return Ok((DVector::from_element(parameters.len(), default), false));
    };

    let known: BTreeSet<&str> = parameters.iter().map(String::as_str).collect();
    let given: BTreeSet<&str> = map.keys().map(String::as_str).collect();

    let unknown: Vec<&str> = given.difference(&known).copied().collect();
    let missing: Vec<&str> = known.difference(&given).copied().collect();
    if !unknown.is_empty() || !missing.is_empty() {
        let mut detail = String::new();
        if !unknown.is_empty() {
            let _ = write!(detail, "no parameter is named {:?}", unknown.join(", "));
        }
        if !missing.is_empty() {
            if !detail.is_empty() {
                detail.push_str("; ");
            }
            let _ = write!(detail, "nothing was given for {:?}", missing.join(", "));
        }
        return Err(InputError::ParameterMapMismatch {
            field: field.to_string(),
            expected: parameters.len(),
            detail,
        }
        .into());
    }

    let values = parameters
        .iter()
        .map(|name| map.get(name).copied().unwrap_or(default))
        .collect::<Vec<_>>();
    Ok((DVector::from_vec(values), true))
}

/// Refuse a problem whose matrices would not fit the declared working-set budget.
///
/// This runs before the design matrix is built, and therefore before the solve
/// allocates its scaled copies and the decomposition allocates its singular vectors.
/// Refusing here is the difference between a typed error and an allocator failure.
///
/// Within the declared dimension limits this never fires; `spec::MAX_SOLVE_BYTES`
/// explains why that is deliberate, and `spec::budgets_admit_the_largest_declared_problem`
/// is the test that keeps it true.
fn check_solve_budget(rows: usize, columns: usize) -> Result<()> {
    let needed = spec::solve_bytes_estimate(rows, columns);
    if needed > spec::MAX_SOLVE_BYTES {
        return Err(InputError::OversizedWork {
            detail: format!(
                "a {rows}-row, {columns}-parameter problem needs about {needed} bytes for the design, its scaled copies, and the decomposition, which is more than the {} Ordinata will allocate; Ordinata refuses before the allocation rather than after it",
                spec::MAX_SOLVE_BYTES
            ),
        }
        .into());
    }
    Ok(())
}

/// Refuse work that would not finish in a reasonable time.
///
/// The bootstrap projects every resample through the retained left singular vectors,
/// so `rows * retained * samples` is the real unit of work. The retained count is not
/// known until the decomposition runs, so the budget is checked against `min(rows,
/// parameters)`, which can only be larger, so the estimate can only refuse early and
/// never late. Ordinata refuses rather than silently lowering the sample count, so the
/// number in the bundle is always the number that was asked for.
fn check_bootstrap_budget(rows: usize, columns: usize, samples: usize) -> Result<()> {
    let bound = std::cmp::min(rows, columns);
    let per_sample = (rows as u128) * (bound as u128);
    let work = per_sample * (samples as u128);
    if work <= spec::MAX_BOOTSTRAP_WORK {
        return Ok(());
    }

    let admissible = spec::MAX_BOOTSTRAP_WORK / per_sample.max(1);
    // Telling a user "ask for fewer than 40" when the schema's own floor is 100 would
    // be advice they cannot take. At that size the honest answer is that this problem
    // does not get a bootstrap at all, and the analytic covariance is what is left.
    // `spec::budgets_admit_the_largest_declared_problem` asserts this branch is
    // unreachable for any shape the input contract accepts; it is here because a
    // message that could contradict itself is worse than one that cannot.
    let advice = if admissible < spec::MIN_BOOTSTRAP_SAMPLES as u128 {
        format!(
            "at this size no bootstrap is affordable at all, because even the smallest the schema allows ({}) would cost {} steps; use uncertainty.method \"analytic\" for a problem this large",
            spec::MIN_BOOTSTRAP_SAMPLES,
            per_sample * (spec::MIN_BOOTSTRAP_SAMPLES as u128)
        )
    } else {
        format!("at this size the largest admissible bootstrapSamples is {admissible}")
    };

    Err(InputError::OversizedWork {
        detail: format!(
            "a {rows}-row, {columns}-parameter problem with {samples} bootstrap samples needs {work} projection steps, more than the {} Ordinata will spend; {advice}",
            spec::MAX_BOOTSTRAP_WORK
        ),
    }
    .into())
}

/// Refuse a problem whose weighted form cannot be represented.
///
/// Every value is finite on its own, but `value / standard_error` can still overflow
/// when a large observation is paired with a tiny error. That would put an infinity
/// into the matrix Ordinata is about to decompose, so it is caught here, named, and
/// refused.
fn check_scaling(
    design: &DMatrix<f64>,
    values: &DVector<f64>,
    errors: &DVector<f64>,
    ids: &[String],
    parameters: &[String],
) -> Result<()> {
    for (i, error) in errors.iter().enumerate() {
        let id = ids.get(i).map_or("?", String::as_str);
        let scaled_value = values[i] / error;
        if !scaled_value.is_finite() {
            return Err(NumericalError::NonFiniteScaled {
                detail: format!(
                    "observation {id}: value / standard_error overflows to {scaled_value:?}"
                ),
            }
            .into());
        }
        for (j, name) in parameters.iter().enumerate() {
            let scaled = design[(i, j)] / error;
            if !scaled.is_finite() {
                return Err(NumericalError::NonFiniteScaled {
                    detail: format!(
                        "observation {id}, parameter {name}: design / standard_error overflows to {scaled:?}"
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}

/// The two tables, joined by ID and put into canonical order.
struct Normalized {
    observation_ids: Vec<String>,
    parameters: Vec<String>,
    design: DMatrix<f64>,
    values: DVector<f64>,
    standard_errors: DVector<f64>,
}

/// Join by ID, refuse anything that is not an exact one-to-one match, and reorder into
/// the canonical form the rest of the library takes for granted.
///
/// This is where permutation invariance comes from. Rows go into ascending order by
/// observation ID and columns into ascending order by parameter name, both by UTF-8
/// byte order, *before* any arithmetic happens. Permuting a file therefore produces a
/// bit-identical matrix, not merely an equivalent one -- which is why the property
/// tests can assert it bit for bit instead of to a tolerance.
///
/// # Errors
///
/// Returns [`InputError::IdMismatch`] when the two files do not name the same set of
/// observations, [`InputError::CountOutOfRange`] when the matched row count is outside
/// the accepted range, and [`InputError::OversizedWork`] when the shape would not fit
/// the working-set budget.
fn normalize(design: &DesignTable, observations: &ObservationTable) -> Result<Normalized> {
    // Exact one-to-one join. Order in either file is irrelevant.
    let design_ids: BTreeSet<&str> = design.ids.iter().map(String::as_str).collect();
    let observed_ids: BTreeSet<&str> = observations.ids.iter().map(String::as_str).collect();
    if design_ids != observed_ids {
        return Err(InputError::IdMismatch {
            design_only: design_ids
                .difference(&observed_ids)
                .map(|s| (*s).to_string())
                .collect(),
            observations_only: observed_ids
                .difference(&design_ids)
                .map(|s| (*s).to_string())
                .collect(),
        }
        .into());
    }

    let rows = design.ids.len();
    if !(spec::MIN_OBSERVATIONS..=spec::MAX_OBSERVATIONS).contains(&rows) {
        return Err(InputError::CountOutOfRange {
            what: "matched observations".into(),
            found: rows,
            min: spec::MIN_OBSERVATIONS,
            max: spec::MAX_OBSERVATIONS,
        }
        .into());
    }
    let columns = design.parameters.len();

    // The last point at which the shape is known and nothing large has been built from
    // it yet. Everything after this line allocates.
    check_solve_budget(rows, columns)?;

    let mut row_order: Vec<usize> = (0..rows).collect();
    row_order.sort_by(|a, b| design.ids[*a].as_bytes().cmp(design.ids[*b].as_bytes()));

    let mut column_order: Vec<usize> = (0..columns).collect();
    column_order.sort_by(|a, b| {
        design.parameters[*a]
            .as_bytes()
            .cmp(design.parameters[*b].as_bytes())
    });

    let parameters: Vec<String> = column_order
        .iter()
        .map(|j| design.parameters[*j].clone())
        .collect();
    let observation_ids: Vec<String> = row_order.iter().map(|i| design.ids[*i].clone()).collect();

    let observation_index: BTreeMap<&str, usize> = observations
        .ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();

    let mut matrix = DMatrix::zeros(rows, columns);
    let mut values = DVector::zeros(rows);
    let mut errors = DVector::zeros(rows);
    for (target_row, source_row) in row_order.iter().enumerate() {
        for (target_column, source_column) in column_order.iter().enumerate() {
            matrix[(target_row, target_column)] = design.rows[*source_row][*source_column];
        }
        let id = &design.ids[*source_row];
        let observation = *observation_index
            .get(id.as_str())
            .ok_or_else(|| Error::Internal(format!("joined ID {id} vanished")))?;
        values[target_row] = observations.values[observation];
        errors[target_row] = observations.standard_errors[observation];
    }

    Ok(Normalized {
        observation_ids,
        parameters,
        design: matrix,
        values,
        standard_errors: errors,
    })
}

/// Join, order, and check. This is the single place a `Problem` is born.
fn assemble(
    project: ProjectFile,
    design: &DesignTable,
    observations: &ObservationTable,
    source_inputs: Vec<SourceInput>,
    input_paths: Vec<std::path::PathBuf>,
    synthetic: bool,
) -> Result<Problem> {
    let Normalized {
        observation_ids,
        parameters,
        design: matrix,
        values,
        standard_errors: errors,
    } = normalize(design, observations)?;
    let rows = observation_ids.len();
    let columns = parameters.len();

    let (prior, prior_declared) = resolve_parameter_map(
        "regularization.prior",
        project.regularization.prior.as_ref(),
        &parameters,
        0.0,
    )?;
    let (penalty, penalty_declared) = resolve_parameter_map(
        "regularization.penalty",
        project.regularization.penalty.as_ref(),
        &parameters,
        1.0,
    )?;

    check_scaling(&matrix, &values, &errors, &observation_ids, &parameters)?;

    let lambda_plan = match project.regularization.mode {
        RegularizationMode::Explicit => LambdaPlan::Explicit(
            project
                .regularization
                .lambda
                .ok_or_else(|| Error::Internal("explicit mode without a lambda".into()))?,
        ),
        RegularizationMode::Gcv => LambdaPlan::Gcv {
            min: project
                .regularization
                .min_lambda
                .ok_or_else(|| Error::Internal("gcv mode without a minLambda".into()))?,
            max: project
                .regularization
                .max_lambda
                .ok_or_else(|| Error::Internal("gcv mode without a maxLambda".into()))?,
            steps: project
                .regularization
                .steps
                .ok_or_else(|| Error::Internal("gcv mode without steps".into()))?,
        },
    };

    let uncertainty = resolve_uncertainty(&project.uncertainty, rows, columns)?;

    Ok(Problem {
        title: project.title,
        description: project.description,
        observation_ids,
        parameters,
        design: matrix,
        values,
        standard_errors: errors,
        prior,
        penalty,
        prior_declared,
        penalty_declared,
        lambda_plan,
        uncertainty,
        assumptions: project.assumptions,
        limitations: project.limitations,
        sources: project.sources,
        validation: project.validation,
        source_inputs,
        input_paths,
        synthetic,
    })
}

fn resolve_uncertainty(
    spec_in: &UncertaintySpec,
    rows: usize,
    columns: usize,
) -> Result<UncertaintyPlan> {
    let bootstrap = if spec_in.method.wants_bootstrap() {
        let samples = spec_in
            .bootstrap_samples
            .ok_or_else(|| Error::Internal("bootstrap without a sample count".into()))?;
        let seed = spec_in
            .bootstrap_seed
            .ok_or_else(|| Error::Internal("bootstrap without a seed".into()))?;
        check_bootstrap_budget(rows, columns, samples)?;
        Some(BootstrapPlan { samples, seed })
    } else {
        None
    };
    Ok(UncertaintyPlan {
        method: spec_in.method,
        confidence_level: spec_in.confidence_level,
        bootstrap,
    })
}

/// Read one input file exactly once, into bounded memory.
///
/// The bytes this returns are the only bytes that go anywhere: they are what gets
/// hashed into the bundle's provenance, and they are what gets parsed into the
/// numbers. That is the whole point of the function existing.
///
/// Hashing the file and then reopening it to parse it -- which is the obvious way to
/// write this, and the way it *was* written -- leaves a window in which the file can
/// change between the two reads. A bundle produced through that window records the
/// checksum of one file next to the answer to another, and it does so silently,
/// because every individual step succeeded. Provenance has to be a statement about the
/// bytes that produced the result, not about whatever was lying at the same path a
/// moment later. One read makes that true by construction rather than by luck.
///
/// The cap is applied to the *read*, not to the size the filesystem reported, because
/// that size is itself a fact that can go stale: a file is free to grow after it has
/// been measured. `take(limit + 1)` reads one byte past the limit, which is exactly
/// enough to know the limit was broken and is all the memory a hostile file gets to
/// spend proving it.
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|e| IoError::read(path, e))?;
    let reported = file.metadata().map_err(|e| IoError::read(path, e))?.len();

    let capacity = usize::try_from(reported.min(limit).saturating_add(1)).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    std::io::Read::take(file, limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| IoError::read(path, e))?;

    if bytes.len() as u64 > limit {
        // The file grew past the cap, or was always over it. Either way the honest
        // number to report is the larger of what was measured and what was actually
        // read, so the message never understates the size that was refused.
        return Err(Error::Internal(format!(
            "{} exceeds its {limit} byte limit at {} bytes",
            crate::error::path_string(path),
            reported.max(bytes.len() as u64)
        )));
    }
    Ok(bytes)
}

/// Read a declared CSV input, mapping the size cap onto the CSV-shaped error.
fn read_csv_input(path: &Path, label: &str) -> Result<Vec<u8>> {
    read_bounded(path, spec::MAX_CSV_BYTES).map_err(|error| match error {
        Error::Internal(_) => InputError::CsvTooLarge {
            file: label.to_string(),
            bytes: std::fs::metadata(path).map_or(spec::MAX_CSV_BYTES + 1, |m| m.len()),
            limit: spec::MAX_CSV_BYTES,
        }
        .into(),
        other => other,
    })
}

/// Load a project and both of its CSVs from disk.
///
/// # Errors
///
/// Returns a [`PathError`](crate::error::PathError) if any declared path is unsafe,
/// an [`IoError`] if a file cannot be read, and an [`InputError`] if any file fails
/// its schema.
pub(crate) fn load(project_path: &Path) -> Result<Problem> {
    let location = paths::locate_project(project_path)?;

    let project_bytes = read_bounded(&location.project_file, spec::MAX_PROJECT_BYTES).map_err(
        |error| match error {
            Error::Internal(_) => InputError::ProjectTooLarge {
                bytes: std::fs::metadata(&location.project_file)
                    .map_or(spec::MAX_PROJECT_BYTES + 1, |m| m.len()),
                limit: spec::MAX_PROJECT_BYTES,
            }
            .into(),
            other => other,
        },
    )?;
    let project = crate::project::parse(&project_bytes)?;

    let design_path =
        paths::resolve_declared_input(&location.root, "designCsv", &project.design_csv)?;
    let observations_path = paths::resolve_declared_input(
        &location.root,
        "observationsCsv",
        &project.observations_csv,
    )?;

    let design_label = project.design_csv.clone();
    let observations_label = project.observations_csv.clone();

    // Read once, then hash and parse the same bytes. The order below is load-bearing:
    // nothing between the read and the hash, and nothing between the hash and the
    // parse, ever touches the filesystem again.
    let design_bytes = read_csv_input(&design_path, &design_label)?;
    let observations_bytes = read_csv_input(&observations_path, &observations_label)?;

    let source_inputs = vec![
        SourceInput {
            role: "project".into(),
            path: file_name(&location.project_file),
            sha256: canonical::sha256_hex(&project_bytes),
            bytes: project_bytes.len() as u64,
        },
        SourceInput {
            role: "design".into(),
            path: design_label.clone(),
            sha256: canonical::sha256_hex(&design_bytes),
            bytes: design_bytes.len() as u64,
        },
        SourceInput {
            role: "observations".into(),
            path: observations_label.clone(),
            sha256: canonical::sha256_hex(&observations_bytes),
            bytes: observations_bytes.len() as u64,
        },
    ];

    let design = parse_design(design_bytes.as_slice(), &design_label)?;
    let observations = parse_observations(observations_bytes.as_slice(), &observations_label)?;

    let input_paths = vec![
        location.project_file.clone(),
        design_path,
        observations_path,
    ];

    assemble(
        project,
        &design,
        &observations,
        source_inputs,
        input_paths,
        false,
    )
}

/// Build a problem from bytes that are already in memory. This is how the demo makes
/// its synthetic problem without writing anything to disk first.
///
/// # Errors
///
/// As [`load`], minus the path errors.
pub(crate) fn load_from_bytes(
    project_bytes: &[u8],
    design_bytes: &[u8],
    observations_bytes: &[u8],
    synthetic: bool,
) -> Result<Problem> {
    let project = crate::project::parse(project_bytes)?;
    let design_label = project.design_csv.clone();
    let observations_label = project.observations_csv.clone();
    let design = parse_design(design_bytes, &design_label)?;
    let observations = parse_observations(observations_bytes, &observations_label)?;

    let source_inputs = vec![
        SourceInput {
            role: "project".into(),
            path: "project.json".into(),
            sha256: canonical::sha256_hex(project_bytes),
            bytes: project_bytes.len() as u64,
        },
        SourceInput {
            role: "design".into(),
            path: design_label,
            sha256: canonical::sha256_hex(design_bytes),
            bytes: design_bytes.len() as u64,
        },
        SourceInput {
            role: "observations".into(),
            path: observations_label,
            sha256: canonical::sha256_hex(observations_bytes),
            bytes: observations_bytes.len() as u64,
        },
    ];

    assemble(
        project,
        &design,
        &observations,
        source_inputs,
        Vec::new(),
        synthetic,
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project.json")
        .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    const PROJECT: &str = r#"{
        "schemaVersion": 1,
        "title": "T",
        "designCsv": "design.csv",
        "observationsCsv": "observations.csv",
        "regularization": { "mode": "explicit", "lambda": 0.0 },
        "uncertainty": { "method": "analytic", "confidenceLevel": 0.95 },
        "assumptions": ["a"],
        "limitations": ["l"]
    }"#;

    fn problem(design: &str, observations: &str) -> Result<Problem> {
        load_from_bytes(
            PROJECT.as_bytes(),
            design.as_bytes(),
            observations.as_bytes(),
            false,
        )
    }

    /// Exactly equal, bit for bit.
    ///
    /// These are not comparisons that want a tolerance. Normalization sorts rows and
    /// columns before any arithmetic happens, so a permuted input produces a
    /// *bit-identical* matrix rather than an equivalent one, and a cell parsed from
    /// `"1"` is exactly `1.0` and not approximately it. Asserting to a tolerance would
    /// be asserting something weaker than what is true.
    #[track_caller]
    fn exactly(got: f64, expected: f64) {
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "got {got}, expected exactly {expected}"
        );
    }

    #[test]
    fn rows_and_columns_come_out_in_canonical_order() {
        // Columns are declared b,a and rows are declared r2,r1. Both come back sorted.
        let p = problem(
            "observation_id,b,a\nr2,20,2\nr1,10,1\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
        )
        .unwrap();
        assert_eq!(p.parameters, vec!["a", "b"]);
        assert_eq!(p.observation_ids, vec!["r1", "r2"]);
        // Row r1 had a=1, b=10; after sorting the columns that is [1, 10].
        exactly(p.design[(0, 0)], 1.0);
        exactly(p.design[(0, 1)], 10.0);
        exactly(p.design[(1, 0)], 2.0);
        exactly(p.design[(1, 1)], 20.0);
        exactly(p.values[0], 1.0);
        exactly(p.values[1], 2.0);
    }

    #[test]
    fn permuting_rows_or_columns_produces_a_bit_identical_matrix() {
        let a = problem(
            "observation_id,x,y\nr1,1,2\nr2,3,4\n",
            "observation_id,value,standard_error\nr1,10,1\nr2,20,2\n",
        )
        .unwrap();
        let b = problem(
            "observation_id,y,x\nr2,4,3\nr1,2,1\n",
            "observation_id,value,standard_error\nr2,20,2\nr1,10,1\n",
        )
        .unwrap();
        assert_eq!(a.design, b.design);
        assert_eq!(a.values, b.values);
        assert_eq!(a.standard_errors, b.standard_errors);
        assert_eq!(a.parameters, b.parameters);
        assert_eq!(a.observation_ids, b.observation_ids);
    }

    #[test]
    fn the_join_is_by_id_not_by_position() {
        // Observation rows are in the opposite order from the design rows.
        let p = problem(
            "observation_id,x\nr1,1\nr2,2\n",
            "observation_id,value,standard_error\nr2,200,2\nr1,100,1\n",
        )
        .unwrap();
        assert_eq!(p.observation_ids, vec!["r1", "r2"]);
        exactly(p.values[0], 100.0); // r1 must keep its own value
        exactly(p.values[1], 200.0);
        exactly(p.standard_errors[0], 1.0);
    }

    #[test]
    fn a_missing_or_extra_id_is_refused() {
        let err = problem(
            "observation_id,x\nr1,1\nr2,2\nr3,3\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "id_mismatch");
        assert!(err.to_string().contains("r3"), "{err}");

        let err = problem(
            "observation_id,x\nr1,1\nr2,2\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\nr9,9,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "id_mismatch");
        assert!(err.to_string().contains("r9"), "{err}");
    }

    #[test]
    fn duplicate_ids_and_headers_are_refused() {
        let err = problem(
            "observation_id,x\nr1,1\nr1,2\n",
            "observation_id,value,standard_error\nr1,1,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "duplicate_identifier");

        let err = problem(
            "observation_id,x,x\nr1,1,1\nr2,2,2\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "duplicate_identifier");
    }

    #[test]
    fn non_finite_and_overflowing_cells_are_refused() {
        for bad in ["NaN", "inf", "-inf", "1e999", "", "abc", "1,2"] {
            let design = format!("observation_id,x\nr1,{bad}\nr2,2\n");
            let err = problem(
                &design,
                "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
            )
            .unwrap_err();
            assert!(
                matches!(err.code(), "non_finite_cell" | "csv_malformed"),
                "{bad:?} -> {} ({err})",
                err.code()
            );
        }
    }

    #[test]
    fn a_zero_or_negative_standard_error_is_refused() {
        for bad in ["0", "-1", "0.0", "-0.0"] {
            let observations = format!("observation_id,value,standard_error\nr1,1,{bad}\nr2,2,1\n");
            let err = problem("observation_id,x\nr1,1\nr2,2\n", &observations).unwrap_err();
            assert_eq!(err.code(), "invalid_standard_error", "{bad}");
        }
    }

    #[test]
    fn a_ragged_row_is_refused() {
        let err = problem(
            "observation_id,x,y\nr1,1,2\nr2,3\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "csv_malformed");
    }

    #[test]
    fn a_wrong_header_is_refused() {
        let err = problem(
            "id,x\nr1,1\nr2,2\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "csv_header");

        let err = problem(
            "observation_id,x\nr1,1\nr2,2\n",
            "observation_id,value,sigma\nr1,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "csv_header");
    }

    #[test]
    fn invalid_utf8_is_refused_as_such() {
        // The row has the right field count, so the only thing wrong with it is that
        // the ID is not UTF-8. That is what the error has to say.
        let mut design: Vec<u8> = b"observation_id,x\nr1,1\n".to_vec();
        design.extend_from_slice(&[0xff, 0xfe]);
        design.extend_from_slice(b",2\n");
        let err = load_from_bytes(
            PROJECT.as_bytes(),
            &design,
            b"observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
            false,
        )
        .unwrap_err();
        assert_eq!(err.code(), "csv_malformed");
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }

    #[test]
    fn an_overflowing_weight_is_refused_even_though_every_cell_is_finite() {
        // 1e300 / 1e-300 is an infinity, so the weighted problem cannot be formed
        // even though neither number is out of range on its own.
        let err = problem(
            "observation_id,x\nr1,1e300\nr2,2\n",
            "observation_id,value,standard_error\nr1,1,1e-300\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "non_finite_scaled");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn a_prior_must_name_every_parameter_or_none() {
        let project = PROJECT.replace(
            r#""regularization": { "mode": "explicit", "lambda": 0.0 }"#,
            r#""regularization": { "mode": "explicit", "lambda": 1.0, "prior": { "x": 1.0 } }"#,
        );
        let err = load_from_bytes(
            project.as_bytes(),
            b"observation_id,x,y\nr1,1,2\nr2,3,4\n",
            b"observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
            false,
        )
        .unwrap_err();
        assert_eq!(err.code(), "parameter_map_mismatch");
        assert!(err.to_string().contains('y'), "{err}");
    }

    #[test]
    fn an_unknown_prior_name_is_refused() {
        let project = PROJECT.replace(
            r#""regularization": { "mode": "explicit", "lambda": 0.0 }"#,
            r#""regularization": { "mode": "explicit", "lambda": 1.0, "prior": { "x": 1.0, "typo": 2.0 } }"#,
        );
        let err = load_from_bytes(
            project.as_bytes(),
            b"observation_id,x\nr1,1\nr2,3\n",
            b"observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
            false,
        )
        .unwrap_err();
        assert_eq!(err.code(), "parameter_map_mismatch");
        assert!(err.to_string().contains("typo"), "{err}");
    }

    #[test]
    fn too_few_observations_are_refused() {
        let err = problem(
            "observation_id,x\nr1,1\n",
            "observation_id,value,standard_error\nr1,1,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "count_out_of_range");
    }

    #[test]
    fn the_bootstrap_budget_refuses_rather_than_silently_truncating() {
        // Well inside the budget.
        assert!(check_bootstrap_budget(100, 5, 10_000).is_ok());
        // The largest problem with the largest bootstrap is far outside it.
        let err = check_bootstrap_budget(50_000, 256, 10_000).unwrap_err();
        assert_eq!(err.code(), "oversized_work");
        assert_eq!(err.exit_code(), 2);
        // And the message tells the user exactly what they may ask for instead.
        assert!(err.to_string().contains("largest admissible"), "{err}");
    }

    #[test]
    fn hostile_identifiers_are_refused_in_both_files() {
        let err = problem(
            "observation_id,=cmd|' /C calc'!A0\nr1,1\nr2,2\n",
            "observation_id,value,standard_error\nr1,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_identifier");

        let err = problem(
            "observation_id,x\n<script>,1\nr2,2\n",
            "observation_id,value,standard_error\n<script>,1,1\nr2,2,1\n",
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_identifier");
    }
}
