//! The typed error taxonomy.
//!
//! Every refusal Ordinata makes is one of these values. Each carries a stable
//! machine-readable [`Error::code`], a [`Error::category`], and the process
//! [`Error::exit_code`] the CLI must use. The mapping is part of the product
//! contract, so changing a code is a breaking change.

use std::fmt::Write as _;
use std::path::Path;

/// The result type used throughout the library.
pub type Result<T> = std::result::Result<T, Error>;

/// Process exit code for a successful run.
pub const EXIT_OK: i32 = 0;
/// Process exit code for an input or validation refusal.
pub const EXIT_INPUT: i32 = 2;
/// Process exit code for a numerical refusal.
pub const EXIT_NUMERICAL: i32 = 3;
/// Process exit code for an I/O or path refusal.
pub const EXIT_IO: i32 = 4;
/// Process exit code for a bundle integrity failure.
pub const EXIT_BUNDLE: i32 = 5;
/// Process exit code for a violated internal invariant.
pub const EXIT_INTERNAL: i32 = 10;

/// Everything that can go wrong, grouped by the exit code it produces.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The project, its schema, or its data files are not acceptable.
    #[error(transparent)]
    Input(#[from] InputError),
    /// A path is unsafe or refers somewhere Ordinata will not go.
    #[error(transparent)]
    Path(#[from] PathError),
    /// The filesystem refused an operation.
    #[error(transparent)]
    Io(#[from] IoError),
    /// The problem is not one Ordinata can answer honestly.
    #[error(transparent)]
    Numerical(#[from] NumericalError),
    /// A bundle is missing, malformed, or does not match its own checksums.
    #[error(transparent)]
    Bundle(#[from] BundleError),
    /// An invariant this program guarantees was violated. This is always a bug.
    #[error("internal invariant violated: {0}")]
    Internal(String),
}

impl Error {
    /// The coarse category, used as the `category` field of the CLI JSON object.
    pub fn category(&self) -> &'static str {
        match self {
            Error::Input(_) => "input",
            Error::Path(_) => "path",
            Error::Io(_) => "io",
            Error::Numerical(_) => "numerical",
            Error::Bundle(_) => "bundle",
            Error::Internal(_) => "internal",
        }
    }

    /// The stable machine-readable code, used as the `code` field of the CLI JSON
    /// object and as the thing tests and scripts match on.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Input(e) => e.code(),
            Error::Path(e) => e.code(),
            Error::Io(e) => e.code(),
            Error::Numerical(e) => e.code(),
            Error::Bundle(e) => e.code(),
            Error::Internal(_) => "internal_invariant",
        }
    }

    /// The process exit code this error must produce.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Input(_) => EXIT_INPUT,
            Error::Path(_) | Error::Io(_) => EXIT_IO,
            Error::Numerical(_) => EXIT_NUMERICAL,
            Error::Bundle(_) => EXIT_BUNDLE,
            Error::Internal(_) => EXIT_INTERNAL,
        }
    }
}

/// Render a path for a human.
///
/// Canonicalizing on Windows produces an extended-length path such as
/// `\\?\C:\data\project`. It is correct, it is what the filesystem was actually
/// asked about, and it is not what anybody typed. Showing it back to a user is
/// noise, so the prefix is stripped for display only; the path Ordinata *uses* is
/// always the canonical one.
///
/// Paths are rendered lossily: a path that cannot be shown as UTF-8 has already been
/// refused, and error text must never itself fail.
pub fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

fn show(path: &Path) -> String {
    display_path(path)
}

/// Render a bounded preview of a list of identifiers, so a mismatch involving ten
/// thousand IDs still produces a message a human can read.
fn preview(ids: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut out = String::new();
    for (i, id) in ids.iter().take(SHOWN).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{id}");
    }
    if ids.len() > SHOWN {
        let _ = write!(out, " (and {} more)", ids.len() - SHOWN);
    }
    out
}

/// Input and schema refusals. These map to exit code 2.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// The project file is larger than [`crate::spec::MAX_PROJECT_BYTES`].
    #[error("project file is {bytes} bytes, which exceeds the {limit} byte limit")]
    ProjectTooLarge {
        /// Observed size.
        bytes: u64,
        /// Configured limit.
        limit: u64,
    },
    /// The project file is not JSON, or does not match schema version 1.
    #[error("project file is not a valid schema-1 project: {detail}")]
    ProjectSchema {
        /// What the deserializer objected to.
        detail: String,
    },
    /// The project declares a schema version Ordinata does not implement.
    #[error("project declares schemaVersion {found}, but this build only accepts {expected}")]
    UnsupportedSchemaVersion {
        /// The version the project declared.
        found: u32,
        /// The version this build accepts.
        expected: u32,
    },
    /// A field is present and well typed but outside its accepted range.
    #[error("{field}: {detail}")]
    InvalidField {
        /// Dotted path of the offending field.
        field: String,
        /// Why it was refused.
        detail: String,
    },
    /// A CSV input is larger than [`crate::spec::MAX_CSV_BYTES`].
    #[error("{file} is {bytes} bytes, which exceeds the {limit} byte limit")]
    CsvTooLarge {
        /// Which file.
        file: String,
        /// Observed size.
        bytes: u64,
        /// Configured limit.
        limit: u64,
    },
    /// A CSV input could not be parsed, including invalid UTF-8 and ragged rows.
    #[error("{file} is not valid CSV: {detail}")]
    CsvMalformed {
        /// Which file.
        file: String,
        /// What the parser objected to.
        detail: String,
    },
    /// A CSV input has no header row.
    #[error("{file} has no header row")]
    CsvEmpty {
        /// Which file.
        file: String,
    },
    /// A CSV header does not have the required shape.
    #[error("{file} has an unusable header: {detail}")]
    CsvHeader {
        /// Which file.
        file: String,
        /// What was wrong.
        detail: String,
    },
    /// An identifier is empty, too long, or uses characters outside the accepted set.
    ///
    /// The accepted set exists so that no observation ID or parameter name can ever
    /// become a spreadsheet formula, an HTML tag, or a quoting ambiguity in an
    /// output file.
    #[error("{file}: {context} {value:?} is not an accepted identifier: {detail}")]
    InvalidIdentifier {
        /// Which file.
        file: String,
        /// Where the identifier appeared, for example `parameter name`.
        context: String,
        /// A bounded, quoted rendering of the offending value.
        value: String,
        /// Which rule it broke.
        detail: String,
    },
    /// The same identifier appears twice where it must be unique.
    #[error("{file}: {context} {value:?} appears more than once")]
    DuplicateIdentifier {
        /// Which file.
        file: String,
        /// Where the identifier appeared.
        context: String,
        /// The repeated value.
        value: String,
    },
    /// A numeric cell is not a finite decimal.
    #[error("{file} row {row}, column {column:?}: {value:?} is not a finite decimal number")]
    NonFiniteCell {
        /// Which file.
        file: String,
        /// 1-based data row, not counting the header.
        row: usize,
        /// Column name.
        column: String,
        /// A bounded rendering of the cell.
        value: String,
    },
    /// A standard error is zero, negative, or not finite.
    #[error("{file} row {row}: standard_error {value:?} must be finite and greater than zero")]
    InvalidStandardError {
        /// Which file.
        file: String,
        /// 1-based data row.
        row: usize,
        /// A bounded rendering of the cell.
        value: String,
    },
    /// The design and observation IDs are not an exact one-to-one match.
    #[error("design and observation IDs do not match one to one: {} only in design [{}]; {} only in observations [{}]",
        design_only.len(), preview(design_only), observations_only.len(), preview(observations_only))]
    IdMismatch {
        /// IDs present in `design.csv` but not `observations.csv`.
        design_only: Vec<String>,
        /// IDs present in `observations.csv` but not `design.csv`.
        observations_only: Vec<String>,
    },
    /// A count is outside the accepted range.
    #[error("{what} is {found}, which is outside the accepted range {min}..={max}")]
    CountOutOfRange {
        /// What was counted.
        what: String,
        /// The observed count.
        found: usize,
        /// Smallest accepted count.
        min: usize,
        /// Largest accepted count.
        max: usize,
    },
    /// A prior or penalty map does not name exactly the declared parameters.
    #[error("{field} must name exactly the {expected} design parameters: {detail}")]
    ParameterMapMismatch {
        /// Which field, `regularization.prior` or `regularization.penalty`.
        field: String,
        /// How many parameters the design declares.
        expected: usize,
        /// Which names were unknown or missing.
        detail: String,
    },
    /// The requested work exceeds the declared budget.
    ///
    /// Ordinata refuses rather than silently truncating, so the number of bootstrap
    /// samples in the bundle is always the number the project asked for.
    #[error("{detail}")]
    OversizedWork {
        /// The budget, the request, and the largest admissible request.
        detail: String,
    },
    /// An observation timestamp is not RFC 3339.
    #[error("observed-at {value:?} is not an RFC 3339 timestamp: {detail}")]
    InvalidTimestamp {
        /// The value supplied.
        value: String,
        /// What the parser objected to.
        detail: String,
    },
}

impl InputError {
    /// The stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            InputError::ProjectTooLarge { .. } => "project_too_large",
            InputError::ProjectSchema { .. } => "project_schema",
            InputError::UnsupportedSchemaVersion { .. } => "unsupported_schema_version",
            InputError::InvalidField { .. } => "invalid_field",
            InputError::CsvTooLarge { .. } => "csv_too_large",
            InputError::CsvMalformed { .. } => "csv_malformed",
            InputError::CsvEmpty { .. } => "csv_empty",
            InputError::CsvHeader { .. } => "csv_header",
            InputError::InvalidIdentifier { .. } => "invalid_identifier",
            InputError::DuplicateIdentifier { .. } => "duplicate_identifier",
            InputError::NonFiniteCell { .. } => "non_finite_cell",
            InputError::InvalidStandardError { .. } => "invalid_standard_error",
            InputError::IdMismatch { .. } => "id_mismatch",
            InputError::CountOutOfRange { .. } => "count_out_of_range",
            InputError::ParameterMapMismatch { .. } => "parameter_map_mismatch",
            InputError::OversizedWork { .. } => "oversized_work",
            InputError::InvalidTimestamp { .. } => "invalid_timestamp",
        }
    }
}

/// Path refusals. These map to exit code 4.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// A declared input path is absolute, rooted, or carries a drive prefix.
    #[error("{field} {path:?} must be a relative path beneath the project directory")]
    NotRelative {
        /// Which project field declared it.
        field: String,
        /// The offending value.
        path: String,
    },
    /// A declared input path contains `..` or `.`.
    #[error("{field} {path:?} must not contain {component:?} components")]
    Traversal {
        /// Which project field declared it.
        field: String,
        /// The offending value.
        path: String,
        /// The component that was refused.
        component: String,
    },
    /// A declared path names a Windows reserved device.
    #[error("{path:?} names the reserved device {name:?}")]
    ReservedDeviceName {
        /// The offending value.
        path: String,
        /// The reserved name found in it.
        name: String,
    },
    /// A path contains characters Ordinata will not handle.
    #[error("{path:?} is not an acceptable path: {detail}")]
    UnacceptablePath {
        /// The offending value.
        path: String,
        /// Which rule it broke.
        detail: String,
    },
    /// A path resolved, through links or reparse points, to somewhere outside the root.
    #[error("{path} resolves to {resolved}, which is outside the project directory {root}")]
    EscapesRoot {
        /// The declared path.
        path: String,
        /// Where it actually pointed.
        resolved: String,
        /// The root it had to stay inside.
        root: String,
    },
    /// A path exists but is not a regular file.
    #[error("{path} is not a regular file")]
    NotAFile {
        /// The offending path.
        path: String,
    },
    /// The output directory already exists. Ordinata never overwrites a bundle.
    #[error(
        "output directory {path} already exists; Ordinata never overwrites an existing bundle"
    )]
    OutputExists {
        /// The output that was requested.
        path: String,
    },
    /// The parent of the output directory does not exist.
    #[error("parent directory {path} of the requested output does not exist")]
    OutputParentMissing {
        /// The missing parent.
        path: String,
    },
    /// The output would contain, or be contained by, one of the inputs.
    #[error("output {output} overlaps input {input}")]
    OutputOverlapsInput {
        /// The output that was requested.
        output: String,
        /// The input it collided with.
        input: String,
    },
    /// A leftover temporary directory blocks an atomic write.
    #[error("staging directory {path} already exists; remove it and run again")]
    StagingExists {
        /// The staging path.
        path: String,
    },
}

impl PathError {
    /// The stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            PathError::NotRelative { .. } => "path_not_relative",
            PathError::Traversal { .. } => "path_traversal",
            PathError::ReservedDeviceName { .. } => "path_reserved_device_name",
            PathError::UnacceptablePath { .. } => "path_unacceptable",
            PathError::EscapesRoot { .. } => "path_escapes_root",
            PathError::NotAFile { .. } => "path_not_a_file",
            PathError::OutputExists { .. } => "output_exists",
            PathError::OutputParentMissing { .. } => "output_parent_missing",
            PathError::OutputOverlapsInput { .. } => "output_overlaps_input",
            PathError::StagingExists { .. } => "staging_exists",
        }
    }
}

/// Filesystem refusals. These map to exit code 4.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    /// A file or directory could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// What was being read.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A file or directory could not be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// What was being written.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A directory could not be created.
    #[error("cannot create directory {path}: {source}")]
    CreateDir {
        /// What was being created.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The final atomic rename failed, so no bundle was published.
    #[error("cannot move staged bundle {from} into place at {to}: {source}")]
    Publish {
        /// The staging directory.
        from: String,
        /// The requested output.
        to: String,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl IoError {
    /// Construct a read error for `path`.
    pub fn read(path: &Path, source: std::io::Error) -> Self {
        IoError::Read {
            path: show(path),
            source,
        }
    }

    /// Construct a write error for `path`.
    pub fn write(path: &Path, source: std::io::Error) -> Self {
        IoError::Write {
            path: show(path),
            source,
        }
    }

    /// Construct a directory-creation error for `path`.
    pub fn create_dir(path: &Path, source: std::io::Error) -> Self {
        IoError::CreateDir {
            path: show(path),
            source,
        }
    }

    /// Construct a publish error for the final rename.
    pub fn publish(from: &Path, to: &Path, source: std::io::Error) -> Self {
        IoError::Publish {
            from: show(from),
            to: show(to),
            source,
        }
    }

    /// The stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            IoError::Read { .. } => "io_read",
            IoError::Write { .. } => "io_write",
            IoError::CreateDir { .. } => "io_create_dir",
            IoError::Publish { .. } => "io_publish",
        }
    }
}

/// Numerical refusals. These map to exit code 3.
///
/// Each of these is a case where Ordinata could produce a number but the number
/// would not mean anything. Producing it anyway is the failure mode this product
/// exists to avoid.
#[derive(Debug, thiserror::Error)]
pub enum NumericalError {
    /// The SVD did not converge within its iteration budget.
    #[error("the singular value decomposition did not converge for a {rows}x{parameters} design")]
    SvdDidNotConverge {
        /// Rows in the scaled design.
        rows: usize,
        /// Columns in the scaled design.
        parameters: usize,
    },
    /// Every singular value is at or below the rank tolerance.
    #[error("the scaled design has numerical rank 0 at tolerance {tolerance:e}: it carries no information about any parameter")]
    ZeroRank {
        /// The rank tolerance that was applied.
        tolerance: f64,
    },
    /// Scaling produced a value that is not finite, typically from dividing a large
    /// value by a tiny standard error.
    #[error("{detail}")]
    NonFiniteScaled {
        /// Which cell overflowed, and by what operation.
        detail: String,
    },
    /// The solve produced a value that is not finite. This should be unreachable and
    /// is reported rather than written into a bundle.
    #[error("the solve produced a value that is not finite: {detail}")]
    NonFiniteResult {
        /// Where it appeared.
        detail: String,
    },
    /// No GCV candidate has a usable denominator.
    #[error("generalized cross-validation is undefined across the whole declared grid: {detail}")]
    IllDefinedDegreesOfFreedom {
        /// The effective degrees of freedom that made every candidate unusable.
        detail: String,
    },
}

impl NumericalError {
    /// The stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            NumericalError::SvdDidNotConverge { .. } => "svd_did_not_converge",
            NumericalError::ZeroRank { .. } => "zero_rank",
            NumericalError::NonFiniteScaled { .. } => "non_finite_scaled",
            NumericalError::NonFiniteResult { .. } => "non_finite_result",
            NumericalError::IllDefinedDegreesOfFreedom { .. } => "ill_defined_degrees_of_freedom",
        }
    }
}

/// Bundle integrity failures. These map to exit code 5.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The bundle path does not exist.
    #[error("no bundle at {path}")]
    NotFound {
        /// What was looked for.
        path: String,
    },
    /// The bundle is not a directory or a readable archive.
    #[error("{path} is not an Ordinata bundle: {detail}")]
    NotABundle {
        /// What was looked at.
        path: String,
        /// Why it was rejected.
        detail: String,
    },
    /// A bundle member could not be parsed.
    #[error("bundle member {member} is malformed: {detail}")]
    MalformedMember {
        /// Which member.
        member: String,
        /// What the parser objected to.
        detail: String,
    },
    /// The archive container is malformed or uses a feature Ordinata will not read.
    #[error("archive is not readable: {detail}")]
    Archive {
        /// What was wrong.
        detail: String,
    },
    /// A checksum line does not have the required shape.
    #[error("SHA256SUMS.txt line {line} is malformed")]
    ChecksumLine {
        /// 1-based line number.
        line: usize,
    },
    /// A checksum entry names a path Ordinata will not open.
    #[error("SHA256SUMS.txt names an unsafe path {path:?}: {detail}")]
    UnsafeChecksumPath {
        /// The offending path.
        path: String,
        /// Which rule it broke.
        detail: String,
    },
    /// A member does not hash to its recorded value.
    #[error(
        "bundle member {member} does not match its checksum: recorded {expected}, found {actual}"
    )]
    ChecksumMismatch {
        /// Which member.
        member: String,
        /// The hash recorded in the bundle.
        expected: String,
        /// The hash of the bytes actually present.
        actual: String,
    },
    /// A member named in the bundle is not present.
    #[error("bundle member {member} is named in {inventory} but is not present")]
    MissingMember {
        /// Which member.
        member: String,
        /// Which inventory named it.
        inventory: String,
    },
    /// A file is present that the bundle does not account for.
    #[error("bundle contains {member}, which is not accounted for in {inventory}")]
    UnexpectedMember {
        /// Which file.
        member: String,
        /// Which inventory should have listed it.
        inventory: String,
    },
    /// The manifest disagrees with the bundle it describes.
    #[error("manifest does not describe this bundle: {detail}")]
    ManifestMismatch {
        /// What disagreed.
        detail: String,
    },
    /// The numbers in the bundle do not hash to the result hash the manifest records.
    ///
    /// This is the deepest check: it fails when the bytes were changed and every
    /// checksum was recomputed to match.
    #[error("recomputed result content hash {actual} does not match the manifest's {expected}")]
    ResultHashMismatch {
        /// The hash the manifest records.
        expected: String,
        /// The hash of the content actually present.
        actual: String,
    },
}

impl BundleError {
    /// The stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            BundleError::NotFound { .. } => "bundle_not_found",
            BundleError::NotABundle { .. } => "not_a_bundle",
            BundleError::MalformedMember { .. } => "malformed_member",
            BundleError::Archive { .. } => "archive_unreadable",
            BundleError::ChecksumLine { .. } => "checksum_line_malformed",
            BundleError::UnsafeChecksumPath { .. } => "unsafe_checksum_path",
            BundleError::ChecksumMismatch { .. } => "checksum_mismatch",
            BundleError::MissingMember { .. } => "missing_member",
            BundleError::UnexpectedMember { .. } => "unexpected_member",
            BundleError::ManifestMismatch { .. } => "manifest_mismatch",
            BundleError::ResultHashMismatch { .. } => "result_hash_mismatch",
        }
    }
}

/// Helper for the many places that need an owned display string for a path.
///
/// A `&PathBuf` coerces to `&Path`, so there is one of these rather than two.
pub(crate) fn path_string(path: &Path) -> String {
    show(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    #[test]
    fn exit_codes_follow_the_published_contract() {
        let cases: Vec<(Error, i32, &str)> = vec![
            (
                InputError::CsvEmpty {
                    file: "design.csv".into(),
                }
                .into(),
                2,
                "input",
            ),
            (
                PathError::OutputExists { path: "out".into() }.into(),
                4,
                "path",
            ),
            (
                IoError::Read {
                    path: "x".into(),
                    source: std::io::Error::other("boom"),
                }
                .into(),
                4,
                "io",
            ),
            (
                NumericalError::ZeroRank { tolerance: 1e-15 }.into(),
                3,
                "numerical",
            ),
            (
                BundleError::NotFound { path: "b".into() }.into(),
                5,
                "bundle",
            ),
            (Error::Internal("bug".into()), 10, "internal"),
        ];
        for (err, exit, category) in cases {
            assert_eq!(err.exit_code(), exit, "{err}");
            assert_eq!(err.category(), category, "{err}");
            assert!(!err.code().is_empty());
        }
    }

    #[test]
    fn id_mismatch_preview_stays_bounded() {
        let many: Vec<String> = (0..1000).map(|i| format!("id{i}")).collect();
        let err = InputError::IdMismatch {
            design_only: many,
            observations_only: vec![],
        };
        let text = err.to_string();
        assert!(text.contains("and 995 more"), "{text}");
        assert!(text.len() < 300, "message must stay readable: {text}");
    }
}
