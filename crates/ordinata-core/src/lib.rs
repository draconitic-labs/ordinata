//! Ordinata core: a bounded weighted linear inverse-problem instrument.
//!
//! Ordinata takes a declared linear calibration problem, solves it by singular value
//! decomposition with optional diagonal Tikhonov regularization around a prior, and
//! emits a portable evidence bundle that a third party can verify offline.
//!
//! # What this library will not do
//!
//! It does not fit nonlinear models, impose constraints, run a sampler, or claim a
//! posterior. It does not touch the network, spawn a process, read an environment
//! variable, or evaluate anything the caller supplies as code. Fitted estimates,
//! predictions, and bootstrap samples are never described as measurements.
//!
//! It also declines to answer questions it cannot answer honestly. A rank-deficient
//! design gets an estimate, a rank, and a warning, and *no* uncertainty at all,
//! because along a direction the data cannot see, both the covariance and the
//! bootstrap would report a near-zero interval for a number the observations never
//! constrained. See [`solve`].
//!
//! # Shape of the API
//!
//! ```text
//! load_project ---> Problem ---> solve ---> Solution
//!                      |                       |
//!                      +-----------+-----------+
//!                                  |
//!                            build_bundle ---> Bundle ---> write_bundle (atomic)
//!                                                                |
//!                                                          verify_bundle
//! ```
//!
//! Each stage is independently usable, none of them knows the CLI exists, and none of
//! them has a side effect beyond the one file tree `write_bundle` creates.
//!
//! ```no_run
//! # fn main() -> ordinata_core::Result<()> {
//! use std::path::Path;
//! use time::OffsetDateTime;
//!
//! let problem = ordinata_core::load_project(Path::new("project.json"))?;
//! let solution = ordinata_core::solve(&problem)?;
//! let bundle = ordinata_core::build_bundle(&problem, &solution, OffsetDateTime::now_utc())?;
//! let written = ordinata_core::write_bundle(&bundle, Path::new("out"), &problem.input_paths)?;
//!
//! let report = ordinata_core::verify_bundle(&written)?;
//! assert!(report.passed());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod archive;
mod ingest;
mod numeric;
mod paths;
mod report;
mod text;

pub mod bundle;
pub mod canonical;
pub mod demo;
pub mod error;
pub mod project;
pub mod solve;
pub mod spec;
pub mod verify;

pub use bundle::{build_bundle, write_bundle, Bundle};
pub use demo::demo_problem;
pub use error::{Error, Result};
pub use ingest::{BootstrapPlan, LambdaPlan, Problem, SourceInput, UncertaintyPlan};
pub use solve::{
    solve, Analytic, Bootstrap, GcvCandidate, IntervalSource, Solution, UncertaintyStatus, Warning,
};
pub use verify::{verify_bundle, BundleSummary, Check, VerificationReport};

use std::path::Path;

/// Load and fully validate a project and its two CSV inputs.
///
/// Nothing is solved. This is what `ordinata validate` runs, and it is the only
/// function that touches a path the caller supplied.
///
/// # Errors
///
/// Returns [`error::PathError`] if a declared path is unsafe or escapes the project
/// directory, [`error::IoError`] if a file cannot be read, [`error::InputError`] if
/// any file fails its schema, and [`error::NumericalError`] if the weighted problem
/// cannot even be formed (for example when `value / standard_error` overflows).
pub fn load_project(path: &Path) -> Result<Problem> {
    ingest::load(path)
}
