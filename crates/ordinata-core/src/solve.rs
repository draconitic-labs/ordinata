//! The numerical core: decomposition, regularization, diagnostics, and uncertainty.
//!
//! # The problem
//!
//! Given a design `A`, observations `b`, and declared standard errors `s`, Ordinata
//! minimizes
//!
//! ```text
//! || A~ x - b~ ||^2  +  lambda * || L (x - x_prior) ||^2
//! ```
//!
//! where `A~` and `b~` are `A` and `b` with each row divided by that row's standard
//! error, and `L` is the declared positive diagonal penalty. This is exactly the
//! normal-equation system in the specification,
//!
//! ```text
//! (A' W A + lambda L'L) x = A' W b + lambda L'L x_prior
//! ```
//!
//! but it is never solved in that form, because forming `A'WA` squares the condition
//! number and inverting it is precisely the unstable step this instrument exists to
//! avoid.
//!
//! # How it is actually solved
//!
//! Substituting `y = x - x_prior` and then `z = L y` turns the problem into standard
//! form with no cross terms:
//!
//! ```text
//! minimize  || B z - r ||^2 + lambda * || z ||^2,
//!   where   B = A~ L^-1   and   r = b~ - A~ x_prior
//! ```
//!
//! One singular value decomposition `B = U S V'` then answers *every* question:
//!
//! ```text
//! z(lambda) = V diag( s_i / (s_i^2 + lambda) ) U' r
//! ```
//!
//! The coefficient `s_i / (s_i^2 + lambda)` is the whole trick. Written this way there
//! is no division by a small singular value anywhere: as `s_i` shrinks the coefficient
//! shrinks with it, instead of exploding the way `1/s_i` would. That is what "never
//! form an unstable inverse" means in practice.
//!
//! Because the decomposition does not depend on `lambda`, every GCV candidate costs
//! `O(k)` and every bootstrap refit costs one projection. The expensive step happens
//! once.
//!
//! # Components that carry no information
//!
//! A singular value at or below the rank tolerance is treated as exactly zero and its
//! component is dropped. Keeping it would let a direction the data says nothing about
//! contribute to the estimate on the strength of rounding noise.

use nalgebra::{DMatrix, DVector};

use crate::error::{Error, NumericalError, Result};
use crate::ingest::{LambdaPlan, Problem};
use crate::numeric::{bootstrap_stream, normal_quantile, percentile_of_sorted, uniform_below};
use crate::spec;

/// A named caution attached to a result. Warnings are never suppressed and never
/// downgraded: a bundle that has one says so on its face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Stable machine-readable code.
    pub code: &'static str,
    /// What it means, in a sentence a reviewer can act on.
    pub message: String,
}

impl Warning {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Warning {
            code,
            message: message.into(),
        }
    }
}

/// Whether an uncertainty was produced, refused, or never asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncertaintyStatus {
    /// The project did not ask for this method.
    NotRequested,
    /// The method ran and its numbers are present.
    Emitted,
    /// The method was asked for and Ordinata declined to answer.
    Refused,
}

impl UncertaintyStatus {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            UncertaintyStatus::NotRequested => "not_requested",
            UncertaintyStatus::Emitted => "emitted",
            UncertaintyStatus::Refused => "refused",
        }
    }
}

/// The analytic covariance result.
#[derive(Debug, Clone)]
pub struct Analytic {
    /// Emitted, refused, or not requested.
    pub status: UncertaintyStatus,
    /// Why, when the status is `Refused`.
    pub reason: Option<String>,
    /// Per-parameter standard errors, in canonical parameter order. Empty unless emitted.
    pub standard_errors: Vec<f64>,
    /// Lower interval bounds. Empty unless emitted.
    pub lower: Vec<f64>,
    /// Upper interval bounds. Empty unless emitted.
    pub upper: Vec<f64>,
}

/// The bootstrap result.
#[derive(Debug, Clone)]
pub struct Bootstrap {
    /// Emitted, refused, or not requested.
    pub status: UncertaintyStatus,
    /// Why, when the status is `Refused`.
    pub reason: Option<String>,
    /// How many resamples the project asked for.
    pub requested_samples: usize,
    /// How many produced a finite refit.
    pub successful_samples: usize,
    /// How many did not. Reported even when zero.
    pub failed_samples: usize,
    /// The declared seed.
    pub seed: Option<u64>,
    /// Lower percentile bounds. Empty unless emitted.
    pub lower: Vec<f64>,
    /// Upper percentile bounds. Empty unless emitted.
    pub upper: Vec<f64>,
}

/// One point on the GCV grid, scored or explicitly not.
#[derive(Debug, Clone, Copy)]
pub struct GcvCandidate {
    /// The candidate lambda.
    pub lambda: f64,
    /// Its GCV score, or `None` when the score is undefined.
    pub score: Option<f64>,
    /// The effective degrees of freedom this lambda would spend.
    pub effective_dof: f64,
    /// The weighted residual sum of squares this lambda would leave.
    pub weighted_rss: f64,
}

/// Which interval, if any, `parameters.csv` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalSource {
    /// The analytic covariance.
    Analytic,
    /// The bootstrap percentiles.
    Bootstrap,
    /// Neither was available. The interval columns are empty.
    None,
}

impl IntervalSource {
    /// The wire name, written into `parameters.csv` so an interval is never
    /// presented without saying where it came from.
    pub fn as_str(self) -> &'static str {
        match self {
            IntervalSource::Analytic => "analytic",
            IntervalSource::Bootstrap => "bootstrap",
            IntervalSource::None => "none",
        }
    }
}

/// Everything the solve found.
#[derive(Debug, Clone)]
pub struct Solution {
    /// Parameter estimates, in canonical parameter order.
    pub estimates: Vec<f64>,
    /// Fitted values, in the original units of `b`.
    pub predictions: Vec<f64>,
    /// `value - prediction`, in the original units.
    pub residuals: Vec<f64>,
    /// Leverage-corrected weighted residuals. `None` where the fit passes exactly
    /// through the point and the quantity does not exist.
    pub standardized_residuals: Vec<Option<f64>>,
    /// Diagonal of the influence matrix.
    pub leverages: Vec<f64>,
    /// The lambda that was used.
    pub lambda: f64,
    /// Whether that lambda was declared or selected.
    pub lambda_mode: &'static str,
    /// Every GCV candidate and its score. Empty when lambda was declared.
    pub gcv_candidates: Vec<GcvCandidate>,
    /// Numerical rank of the scaled, penalty-weighted design.
    pub rank: usize,
    /// The tolerance that rank was measured against.
    pub rank_tolerance: f64,
    /// All singular values, descending.
    pub singular_values: Vec<f64>,
    /// `sigma_max / sigma_min`, or `None` when the design is rank deficient and the
    /// ratio is not finite.
    pub condition_number: Option<f64>,
    /// Trace of the influence matrix.
    pub effective_dof: f64,
    /// `rows - effective_dof`.
    pub residual_dof: f64,
    /// Weighted residual sum of squares.
    pub weighted_rss: f64,
    /// `sqrt(weighted_rss / rows)`.
    pub weighted_rmse: f64,
    /// `weighted_rss / residual_dof`. Should sit near 1 when the declared standard
    /// errors are right. `None` when there are no residual degrees of freedom.
    pub reduced_chi_square: Option<f64>,
    /// The analytic covariance, or the reason there is none.
    pub analytic: Analytic,
    /// The bootstrap, or the reason there is none.
    pub bootstrap: Bootstrap,
    /// Which interval `parameters.csv` carries.
    pub interval_source: IntervalSource,
    /// Everything the reader needs to know that is not an error.
    pub warnings: Vec<Warning>,
}

/// `B = U S V'`, with components sorted and the rank decided.
struct Decomposition {
    /// `m x k`.
    u: DMatrix<f64>,
    /// `n x k`.
    v: DMatrix<f64>,
    /// `k`, descending.
    sigma: Vec<f64>,
    rank: usize,
    tolerance: f64,
}

impl Decomposition {
    /// The filter factors `F_i = s^2/(s^2 + lambda)` and solution coefficients
    /// `D_i = s/(s^2 + lambda)`, with dropped components pinned to zero.
    ///
    /// These are computed as `D = 1 / (s + lambda/s)` and `F = s * D` rather than
    /// from `s*s` directly. The two are the same quantity, but `s*s` overflows for a
    /// singular value above about 1e154, and a design with entries near the top of
    /// the double range is a thing a user can actually hand us. This form has no
    /// intermediate that can overflow when `s` and `lambda` are both finite.
    fn filters(&self, lambda: f64) -> (Vec<f64>, Vec<f64>) {
        let mut filter = vec![0.0; self.sigma.len()];
        let mut coefficient = vec![0.0; self.sigma.len()];
        for i in 0..self.rank {
            let s = self.sigma[i];
            let d = 1.0 / (s + lambda / s);
            coefficient[i] = d;
            filter[i] = s * d;
        }
        (filter, coefficient)
    }
}

/// Decompose the scaled, penalty-weighted design.
fn decompose(b: DMatrix<f64>, rows: usize, columns: usize) -> Result<Decomposition> {
    // A bounded iteration count so that a pathological matrix produces a typed
    // refusal instead of spinning forever.
    let svd = nalgebra::linalg::SVD::try_new(b, true, true, f64::EPSILON, 10_000).ok_or(
        NumericalError::SvdDidNotConverge {
            rows,
            parameters: columns,
        },
    )?;
    let u = svd
        .u
        .ok_or_else(|| Error::Internal("SVD returned no U".into()))?;
    let v_t = svd
        .v_t
        .ok_or_else(|| Error::Internal("SVD returned no V^T".into()))?;
    let raw: Vec<f64> = svd.singular_values.iter().copied().collect();

    if let Some(bad) = raw.iter().find(|s| !s.is_finite()) {
        return Err(NumericalError::NonFiniteResult {
            detail: format!("a singular value came back as {bad:?}"),
        }
        .into());
    }

    // nalgebra does not promise an order, and the whole determinism story depends on
    // one, so the components are sorted here: descending by value, ties broken by
    // original index so the result is a pure function of the matrix.
    let k = raw.len();
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_by(|a, b| raw[*b].total_cmp(&raw[*a]).then_with(|| a.cmp(b)));

    let sigma: Vec<f64> = order.iter().map(|i| raw[*i]).collect();
    let mut u_sorted = DMatrix::zeros(rows, k);
    let mut v_sorted = DMatrix::zeros(columns, k);
    for (target, source) in order.iter().enumerate() {
        u_sorted.set_column(target, &u.column(*source));
        // v_t is k x n, so its rows are the right singular vectors.
        v_sorted.set_column(target, &v_t.row(*source).transpose());
    }

    let sigma_max = sigma.first().copied().unwrap_or(0.0);
    #[allow(clippy::cast_precision_loss)]
    let tolerance = (rows.max(columns) as f64) * sigma_max * f64::EPSILON;
    let rank = sigma.iter().filter(|s| **s > tolerance).count();
    if rank == 0 {
        return Err(NumericalError::ZeroRank { tolerance }.into());
    }

    Ok(Decomposition {
        u: u_sorted,
        v: v_sorted,
        sigma,
        rank,
        tolerance,
    })
}

/// The inclusive log-spaced grid. The endpoints are the exact declared values rather
/// than `exp(ln(x))` round trips of them, so a grid from 1e-6 to 1e2 really does
/// contain 1e-6 and 1e2.
fn log_grid(min: f64, max: f64, steps: usize) -> Vec<f64> {
    let mut grid = Vec::with_capacity(steps);
    let ln_min = min.ln();
    let ln_max = max.ln();
    let last = steps - 1;
    for i in 0..steps {
        if i == 0 {
            grid.push(min);
        } else if i == last {
            grid.push(max);
        } else {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f64 / last as f64;
            grid.push((ln_min + t * (ln_max - ln_min)).exp());
        }
    }
    grid
}

/// Does this fit have residual degrees of freedom worth the name?
fn has_residual_dof(rows: usize, effective_dof: f64) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let m = rows as f64;
    (1.0 - effective_dof / m) > spec::RESIDUAL_DOF_FRACTION_FLOOR
}

/// Score one lambda from the decomposition alone, in `O(k)`.
fn score_candidate(
    decomposition: &Decomposition,
    projections: &[f64],
    orthogonal_rss: f64,
    rows: usize,
    lambda: f64,
) -> GcvCandidate {
    let (filter, _) = decomposition.filters(lambda);
    let mut rss = orthogonal_rss;
    for (i, f) in filter.iter().enumerate() {
        let residual = (f - 1.0) * projections[i];
        rss += residual * residual;
    }
    let effective_dof: f64 = filter.iter().sum();

    #[allow(clippy::cast_precision_loss)]
    let m = rows as f64;
    let score = if has_residual_dof(rows, effective_dof) && rss.is_finite() {
        let denominator = m - effective_dof;
        let value = m * rss / (denominator * denominator);
        value.is_finite().then_some(value)
    } else {
        None
    };

    GcvCandidate {
        lambda,
        score,
        effective_dof,
        weighted_rss: rss,
    }
}

/// Pick the winner. Candidates are scanned in ascending lambda and the last
/// strict-or-equal minimum wins, so an exact tie resolves toward the *larger*
/// lambda: when two regularization strengths fit the data equally well, Ordinata
/// takes the more conservative one.
fn select_lambda(candidates: &[GcvCandidate]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(score) = candidate.score else {
            continue;
        };
        match best {
            Some((_, best_score)) if score <= best_score => best = Some((index, score)),
            None => best = Some((index, score)),
            Some(_) => {}
        }
    }
    best.map(|(index, _)| index)
}

/// The problem in standard Tikhonov form.
struct Scaled {
    /// `A~`: the design with each row divided by its standard error.
    weighted_design: DMatrix<f64>,
    /// `b~`: the observations, likewise.
    weighted_values: DVector<f64>,
    /// `B = A~ L^-1`: the same again with the columns divided by the penalty. This is
    /// the matrix that gets decomposed, and it is consumed doing so.
    penalized_design: DMatrix<f64>,
}

/// Row-scale by the inverse standard error, then column-scale by the inverse penalty.
///
/// After both, the problem is in standard Tikhonov form and the decomposition can
/// answer every question about it. The joint size of these matrices is what
/// `ingest::check_solve_budget` refused before we got here.
fn scale(problem: &Problem) -> Scaled {
    let rows = problem.rows();
    let columns = problem.columns();

    let mut weighted_design = DMatrix::zeros(rows, columns);
    let mut weighted_values = DVector::zeros(rows);
    for i in 0..rows {
        let weight = 1.0 / problem.standard_errors[i];
        weighted_values[i] = problem.values[i] * weight;
        for j in 0..columns {
            weighted_design[(i, j)] = problem.design[(i, j)] * weight;
        }
    }

    let mut penalized_design = weighted_design.clone();
    for j in 0..columns {
        let inverse_penalty = 1.0 / problem.penalty[j];
        for i in 0..rows {
            penalized_design[(i, j)] *= inverse_penalty;
        }
    }

    Scaled {
        weighted_design,
        weighted_values,
        penalized_design,
    }
}

/// The lambda that will be used, how it was arrived at, and every candidate that was
/// considered on the way.
struct Chosen {
    lambda: f64,
    mode: &'static str,
    candidates: Vec<GcvCandidate>,
}

/// Take the declared lambda, or search the declared grid for one.
///
/// # Errors
///
/// Returns [`NumericalError::IllDefinedDegreesOfFreedom`] when no candidate on the
/// declared grid has a defined score.
fn choose_lambda(
    problem: &Problem,
    decomposition: &Decomposition,
    projections: &[f64],
    orthogonal_rss: f64,
    warnings: &mut Vec<Warning>,
) -> Result<Chosen> {
    let rows = problem.rows();
    let columns = problem.columns();

    let (min, max, steps) = match problem.lambda_plan {
        LambdaPlan::Explicit(lambda) => {
            return Ok(Chosen {
                lambda,
                mode: "explicit",
                candidates: Vec::new(),
            })
        }
        LambdaPlan::Gcv { min, max, steps } => (min, max, steps),
    };

    let candidates: Vec<GcvCandidate> = log_grid(min, max, steps)
        .iter()
        .map(|lambda| score_candidate(decomposition, projections, orthogonal_rss, rows, *lambda))
        .collect();

    let chosen = select_lambda(&candidates).ok_or_else(|| {
        NumericalError::IllDefinedDegreesOfFreedom {
            detail: format!(
                "every one of the {steps} candidates spends effectively all {rows} degrees of freedom, so the GCV denominator collapses and no candidate has a score; the design has {columns} parameters and numerical rank {}",
                decomposition.rank
            ),
        }
    })?;
    let lambda = candidates[chosen].lambda;

    // A minimum that sits on the edge of the grid is a minimum of the grid, not
    // necessarily of the function. Saying so is the difference between a selected
    // lambda and a trusted one.
    if chosen == 0 || chosen == candidates.len() - 1 {
        let edge = if chosen == 0 { "smallest" } else { "largest" };
        warnings.push(Warning::new(
            "gcv_selected_grid_endpoint",
            format!(
                "generalized cross-validation chose lambda = {lambda:e}, the {edge} value on the declared grid; the true minimum may lie outside the grid, so widen it before treating this lambda as optimal"
            ),
        ));
    }

    Ok(Chosen {
        lambda,
        mode: "gcv",
        candidates,
    })
}

/// The warnings that describe the fit itself, as opposed to the ones that describe a
/// refusal.
fn describe_fit(
    columns: usize,
    rank: usize,
    condition_number: Option<f64>,
    reduced_chi_square: Option<f64>,
    warnings: &mut Vec<Warning>,
) {
    if rank != columns {
        warnings.push(Warning::new(
            "rank_deficient",
            format!(
                "the scaled design has numerical rank {rank} but {columns} parameters, so the data does not determine {} independent direction(s) in parameter space; the estimate along those directions is whatever the prior and the penalty put there, not something the observations said",
                columns - rank
            ),
        ));
    }

    if let Some(condition) = condition_number {
        if condition > spec::ILL_CONDITIONED_WARN {
            warnings.push(Warning::new(
                "ill_conditioned",
                format!(
                    "the scaled design has condition number {condition:e}; small changes in the observations can move the estimate a great deal, and the intervals below are the honest consequence of that"
                ),
            ));
        }
    }

    if let Some(chi_square) = reduced_chi_square {
        if !(spec::RESIDUAL_SCALE_WARN_LOW..=spec::RESIDUAL_SCALE_WARN_HIGH).contains(&chi_square) {
            let direction = if chi_square < spec::RESIDUAL_SCALE_WARN_LOW {
                "larger"
            } else {
                "smaller"
            };
            warnings.push(Warning::new(
                "residual_scale_inconsistent_with_declared_errors",
                format!(
                    "the reduced chi-square is {chi_square:.3}, which is far from 1; the declared standard errors look {direction} than the residuals actually are, so the analytic intervals (which trust the declared errors) and the bootstrap intervals (which trust the residuals) will disagree in width"
                ),
            ));
        }
    }
}

/// The decomposition, and the data's remaining signal projected through it.
///
/// Everything after this point is `O(k)` per lambda. This is the expensive step, and it
/// happens exactly once no matter how many candidates the GCV grid holds or how many
/// resamples the bootstrap draws.
struct Prepared {
    decomposition: Decomposition,
    /// `U' r`: what the data still has to say, in the decomposition's own basis.
    projections: Vec<f64>,
    /// The part of `r` outside the column space of `U`. No lambda can touch it, so it
    /// is a constant in every residual sum of squares and is taken out once.
    orthogonal_rss: f64,
}

/// Scale, decompose, and project.
///
/// # Errors
///
/// Returns [`NumericalError::SvdDidNotConverge`] or [`NumericalError::ZeroRank`].
fn prepare(problem: &Problem) -> Result<Prepared> {
    let scaled = scale(problem);
    let decomposition = decompose(scaled.penalized_design, problem.rows(), problem.columns())?;

    // What the data still has to say once the prior's own prediction is removed.
    let prior_fit = &scaled.weighted_design * &problem.prior;
    let residual_signal = &scaled.weighted_values - &prior_fit;

    let projected = decomposition.u.tr_mul(&residual_signal);
    let projections: Vec<f64> = projected.iter().copied().collect();
    let projected_norm: f64 = projections.iter().map(|v| v * v).sum();
    let orthogonal_rss = (residual_signal.dot(&residual_signal) - projected_norm).max(0.0);

    Ok(Prepared {
        decomposition,
        projections,
        orthogonal_rss,
    })
}

/// What the fit spent, what it left, and how well conditioned the design was.
struct Diagnostics {
    effective_dof: f64,
    residual_dof: f64,
    weighted_rmse: f64,
    dof_available: bool,
    reduced_chi_square: Option<f64>,
    condition_number: Option<f64>,
}

/// Read the diagnostics off the decomposition and the residuals.
fn diagnose(
    decomposition: &Decomposition,
    filter: &[f64],
    weighted_rss: f64,
    rows: usize,
    columns: usize,
) -> Diagnostics {
    #[allow(clippy::cast_precision_loss)]
    let row_count = rows as f64;

    let effective_dof: f64 = filter.iter().sum();
    let residual_dof = row_count - effective_dof;
    let dof_available = has_residual_dof(rows, effective_dof);

    let sigma_max = decomposition.sigma.first().copied().unwrap_or(0.0);
    let sigma_min = decomposition.sigma.last().copied().unwrap_or(0.0);

    // A design determines its parameters only when its rank reaches its parameter count.
    // Everything downstream -- the condition number, both uncertainties, and the declared
    // condition-number check -- hangs off this one question, so it is asked once, here,
    // and asked about the *parameters* rather than about however many singular values the
    // decomposition happened to return. See `spec::CONDITION_NUMBER_RULE` for why the
    // difference is not academic: a thin decomposition of a 2x3 design returns two
    // singular values, and comparing the rank to *that* would call it full rank and
    // publish a finite condition number for a system that does not pin down its
    // parameters at all.
    let identifiable = decomposition.rank == columns;

    Diagnostics {
        effective_dof,
        residual_dof,
        weighted_rmse: (weighted_rss / row_count).sqrt(),
        dof_available,
        reduced_chi_square: dof_available.then(|| weighted_rss / residual_dof),
        condition_number: (identifiable && sigma_min > 0.0).then(|| sigma_max / sigma_min),
    }
}

/// The one rule, stated once, that governs *both* uncertainty methods.
///
/// A rank-deficient design does not merely make the intervals wide. Along a direction
/// the data cannot see, a regularized estimate has almost no *variance* -- the estimator
/// simply returns the prior there, run after run -- while carrying unbounded *bias*.
/// Both the covariance and the bootstrap would therefore report a nearly zero interval
/// for a number the data did not constrain at all. That is the precise failure this
/// instrument exists to refuse, so when the design is rank deficient Ordinata publishes
/// the estimate, the rank, and the warning, and publishes no uncertainty whatsoever.
///
/// The second case is the mirror image: a fit with no residual degrees of freedom has
/// spent the data reproducing itself, and an interval drawn from it would describe the
/// arithmetic rather than the world.
///
/// Returns the reason, or `None` when there is no reason and both methods may proceed.
fn uncertainty_refusal(
    columns: usize,
    rank: usize,
    rows: usize,
    effective_dof: f64,
    residual_dof: f64,
    dof_available: bool,
) -> Option<String> {
    if rank != columns {
        return Some(format!(
            "the scaled design has numerical rank {rank} but {columns} parameters. Along the {} unresolved direction(s) a regularized estimator has almost no variance and unbounded bias, so any interval computed here would report near-zero uncertainty for a quantity the observations do not constrain. Ordinata will not publish that number.",
            columns - rank
        ));
    }
    if !dof_available {
        return Some(format!(
            "the fit spends {effective_dof:.6} of its {rows} degrees of freedom, leaving {residual_dof:.6}. With no residual degrees of freedom the model reproduces the data exactly and nothing is left over to say how well it did, so an interval drawn from this fit would describe the arithmetic rather than the world."
        ));
    }
    None
}

/// Everything the estimate implies about the data it was fitted to.
struct Fit {
    estimates: Vec<f64>,
    predictions: DVector<f64>,
    residuals: Vec<f64>,
    weighted_residuals: Vec<f64>,
    weighted_rss: f64,
    leverages: Vec<f64>,
    standardized_residuals: Vec<Option<f64>>,
}

/// Turn the decomposition and a chosen lambda into the estimate and its residuals.
///
/// The estimate comes back from standard form by undoing the two substitutions in the
/// order they were made: `z = V (D . c)` is the solution in standard form, `y = L^-1 z`
/// undoes the column scaling, and `x = x_prior + y` undoes the shift. Nothing here
/// divides by a singular value; `D` already carries the regularized coefficient.
fn compute_fit(
    problem: &Problem,
    decomposition: &Decomposition,
    projections: &[f64],
    coefficient: &[f64],
    filter: &[f64],
) -> Fit {
    let rows = problem.rows();
    let columns = problem.columns();

    let weighted: DVector<f64> = DVector::from_iterator(
        projections.len(),
        projections
            .iter()
            .zip(coefficient.iter())
            .map(|(projection, d)| projection * d),
    );
    let standard_form = &decomposition.v * weighted;
    let estimates: Vec<f64> = (0..columns)
        .map(|j| problem.prior[j] + standard_form[j] / problem.penalty[j])
        .collect();

    let predictions = &problem.design * DVector::from_vec(estimates.clone());
    let mut residuals = vec![0.0; rows];
    let mut weighted_residuals = vec![0.0; rows];
    let mut weighted_rss = 0.0;
    for i in 0..rows {
        let residual = problem.values[i] - predictions[i];
        residuals[i] = residual;
        let weighted_residual = residual / problem.standard_errors[i];
        weighted_residuals[i] = weighted_residual;
        weighted_rss += weighted_residual * weighted_residual;
    }

    // Leverage: the diagonal of H = U F U'.
    let leverages: Vec<f64> = (0..rows)
        .map(|i| {
            filter
                .iter()
                .enumerate()
                .take(decomposition.rank)
                .map(|(j, f)| {
                    let component = decomposition.u[(i, j)];
                    f * component * component
                })
                .sum()
        })
        .collect();

    // A point the fit passes exactly through has `1 - leverage` at zero, and its
    // standardized residual is 0/0. It does not exist, so it is not invented.
    let standardized_residuals: Vec<Option<f64>> = leverages
        .iter()
        .zip(weighted_residuals.iter())
        .map(|(leverage, weighted_residual)| {
            let slack = 1.0 - leverage;
            (slack > spec::LEVERAGE_FLOOR).then(|| weighted_residual / slack.sqrt())
        })
        .collect();

    Fit {
        estimates,
        predictions,
        residuals,
        weighted_residuals,
        weighted_rss,
        leverages,
        standardized_residuals,
    }
}

/// Both uncertainty methods, and which of them `parameters.csv` shows by default.
struct Uncertainty {
    analytic: Analytic,
    bootstrap: Bootstrap,
    interval_source: IntervalSource,
}

/// Run both uncertainty methods, refusing both together where the fit does not support
/// either.
///
/// # Errors
///
/// Returns [`NumericalError::NonFiniteResult`] if a standard error comes out non-finite,
/// which the input checks are supposed to have made impossible.
#[allow(clippy::too_many_arguments)]
fn compute_uncertainty(
    problem: &Problem,
    decomposition: &Decomposition,
    coefficient: &[f64],
    filter: &[f64],
    projections: &[f64],
    fit: &Fit,
    diagnostics: &Diagnostics,
    lambda: f64,
    warnings: &mut Vec<Warning>,
) -> Result<Uncertainty> {
    let refusal = uncertainty_refusal(
        problem.columns(),
        decomposition.rank,
        problem.rows(),
        diagnostics.effective_dof,
        diagnostics.residual_dof,
        diagnostics.dof_available,
    );

    let analytic = compute_analytic(
        problem,
        decomposition,
        coefficient,
        filter,
        &fit.estimates,
        lambda,
        refusal.as_ref(),
        warnings,
    )?;

    let bootstrap = compute_bootstrap(
        problem,
        decomposition,
        coefficient,
        filter,
        projections,
        &fit.weighted_residuals,
        &fit.leverages,
        diagnostics.reduced_chi_square,
        refusal.as_ref(),
        warnings,
    )?;

    // Which interval `parameters.csv` puts in its *primary* columns. Both methods are
    // exported in columns of their own regardless -- this only picks the default for a
    // reader with one column of space, and `interval_source` names which one it is.
    let interval_source = if analytic.status == UncertaintyStatus::Emitted {
        IntervalSource::Analytic
    } else if bootstrap.status == UncertaintyStatus::Emitted {
        IntervalSource::Bootstrap
    } else {
        IntervalSource::None
    };

    Ok(Uncertainty {
        analytic,
        bootstrap,
        interval_source,
    })
}

/// Solve a normalized problem.
///
/// # Errors
///
/// Returns a [`NumericalError`] when the problem cannot be answered honestly: the
/// decomposition did not converge, the design carries no information at all, or the
/// declared GCV grid has no point at which the score is defined.
pub fn solve(problem: &Problem) -> Result<Solution> {
    let rows = problem.rows();
    let columns = problem.columns();

    let Prepared {
        decomposition,
        projections,
        orthogonal_rss,
    } = prepare(problem)?;

    let mut warnings = Vec::new();
    let chosen = choose_lambda(
        problem,
        &decomposition,
        &projections,
        orthogonal_rss,
        &mut warnings,
    )?;
    let lambda = chosen.lambda;

    if lambda < 0.0 || !lambda.is_finite() {
        return Err(Error::Internal(format!("selected a lambda of {lambda:?}")));
    }

    let (filter, coefficient) = decomposition.filters(lambda);
    let fit = compute_fit(problem, &decomposition, &projections, &coefficient, &filter);
    let diagnostics = diagnose(&decomposition, &filter, fit.weighted_rss, rows, columns);

    describe_fit(
        columns,
        decomposition.rank,
        diagnostics.condition_number,
        diagnostics.reduced_chi_square,
        &mut warnings,
    );

    let Uncertainty {
        analytic,
        bootstrap,
        interval_source,
    } = compute_uncertainty(
        problem,
        &decomposition,
        &coefficient,
        &filter,
        &projections,
        &fit,
        &diagnostics,
        lambda,
        &mut warnings,
    )?;

    let Fit {
        estimates,
        predictions,
        residuals,
        weighted_residuals: _,
        weighted_rss,
        leverages,
        standardized_residuals,
    } = fit;
    let Diagnostics {
        effective_dof,
        residual_dof,
        weighted_rmse,
        dof_available: _,
        reduced_chi_square,
        condition_number,
    } = diagnostics;

    let solution = Solution {
        estimates,
        predictions: predictions.iter().copied().collect(),
        residuals,
        standardized_residuals,
        leverages,
        lambda,
        lambda_mode: chosen.mode,
        gcv_candidates: chosen.candidates,
        rank: decomposition.rank,
        rank_tolerance: decomposition.tolerance,
        singular_values: decomposition.sigma.clone(),
        condition_number,
        effective_dof,
        residual_dof,
        weighted_rss,
        weighted_rmse,
        reduced_chi_square,
        analytic,
        bootstrap,
        interval_source,
        warnings,
    };
    check_finite(&solution)?;
    Ok(solution)
}

/// The covariance of the linear estimator, taking the declared standard errors at
/// their word.
///
/// After row scaling the noise has unit variance by construction, so the estimator
/// `x = x_prior + G r` with `G = L^-1 V D U'` has covariance `G G'`, and only its
/// diagonal is needed:
///
/// ```text
/// var(x_j) = (1 / L_j^2) * sum_i (V_ji * D_i)^2
/// ```
///
/// The residual scale is *not* re-estimated from the fit. The user declared the
/// standard errors; substituting a scale read back off the residuals would quietly
/// replace their stated uncertainties with ours. The reduced chi-square is reported
/// instead, so a mismatch between the two is visible rather than absorbed.
#[allow(clippy::too_many_arguments)]
fn compute_analytic(
    problem: &Problem,
    decomposition: &Decomposition,
    coefficient: &[f64],
    filter: &[f64],
    estimates: &[f64],
    lambda: f64,
    refusal: Option<&String>,
    warnings: &mut Vec<Warning>,
) -> Result<Analytic> {
    let refused = |reason: Option<String>, status: UncertaintyStatus| Analytic {
        status,
        reason,
        standard_errors: Vec::new(),
        lower: Vec::new(),
        upper: Vec::new(),
    };

    if !problem.uncertainty.method.wants_analytic() {
        return Ok(refused(None, UncertaintyStatus::NotRequested));
    }
    if let Some(reason) = refusal {
        warnings.push(Warning::new(
            "analytic_covariance_refused",
            format!("no analytic covariance was published: {reason}"),
        ));
        return Ok(refused(Some(reason.clone()), UncertaintyStatus::Refused));
    }

    let level = problem.uncertainty.confidence_level;
    let z = normal_quantile(0.5 * (1.0 + level)).ok_or_else(|| {
        Error::Internal(format!(
            "no normal quantile for a confidence level of {level}"
        ))
    })?;

    let columns = problem.columns();
    let mut standard_errors = vec![0.0; columns];
    let mut lower = vec![0.0; columns];
    let mut upper = vec![0.0; columns];
    for j in 0..columns {
        let mut variance = 0.0;
        for (i, d) in coefficient.iter().enumerate().take(decomposition.rank) {
            let component = decomposition.v[(j, i)] * d;
            variance += component * component;
        }
        let standard_error = variance.sqrt() / problem.penalty[j];
        if !standard_error.is_finite() {
            return Err(NumericalError::NonFiniteResult {
                detail: format!(
                    "the analytic standard error of parameter {} came out as {standard_error:?}",
                    problem.parameters[j]
                ),
            }
            .into());
        }
        let half_width = z * standard_error;
        standard_errors[j] = standard_error;
        lower[j] = estimates[j] - half_width;
        upper[j] = estimates[j] + half_width;
    }

    if lambda > 0.0 {
        // Quantify the pull rather than merely announcing it. A GCV-selected lambda
        // is almost always nonzero, so a warning that fires on `lambda > 0` and says
        // nothing else would be noise on every regularized run. What a reader needs
        // is how much freedom the penalty actually took away: `rank - effective_dof`
        // is the number of directions the data could have determined but the penalty
        // spent instead.
        let spent: f64 = filter.iter().sum();
        #[allow(clippy::cast_precision_loss)]
        let available = decomposition.rank as f64;
        let shrinkage = available - spent;
        warnings.push(Warning::new(
            "analytic_covariance_excludes_regularization_bias",
            format!(
                "lambda is {lambda:e}, so the estimate is pulled toward the prior. This fit spends {spent:.6} of the {available:.0} degrees of freedom the data could have supported, so the penalty has absorbed {shrinkage:.6} of them. The interval reported here is the sampling spread of the pulled estimate and does not include the pull itself: it is an interval around the regularized estimate, not around the truth."
            ),
        ));
    }

    Ok(Analytic {
        status: UncertaintyStatus::Emitted,
        reason: None,
        standard_errors,
        lower,
        upper,
    })
}

/// The two reasons a bootstrap declines even when the fit itself is sound.
///
/// Both are cases where the arithmetic would happily produce an interval and the
/// interval would be a fiction. They are separate from the rank and degrees-of-freedom
/// refusals that govern *both* uncertainty methods; these are the bootstrap's own.
fn bootstrap_refusal(
    problem: &Problem,
    leverages: &[f64],
    reduced_chi_square: Option<f64>,
) -> Option<String> {
    // A point the fit passes exactly through has no residual to resample, and its
    // standardized residual is 0/0.
    if let Some((index, leverage)) = leverages
        .iter()
        .enumerate()
        .find(|(_, h)| (1.0 - **h) <= spec::LEVERAGE_FLOOR)
    {
        return Some(format!(
            "observation {} has leverage {leverage:.12}, so the fit passes exactly through it and its standardized residual does not exist. A residual bootstrap cannot resample a residual that is not defined.",
            problem.observation_ids[index]
        ));
    }

    // Residuals with no spread would produce intervals with no width. That would not be
    // a confident result; it would be a manufactured one.
    match reduced_chi_square {
        Some(chi) if chi <= spec::DEGENERATE_RESIDUAL_REDUCED_CHI_SQUARE => Some(format!(
            "the reduced chi-square is {chi:e}: the model reproduces the observations to within rounding, so the residuals carry no spread to resample. A bootstrap of these residuals would report an interval of essentially zero width, which would state a certainty the data does not support."
        )),
        None => Some("there are no residual degrees of freedom to bootstrap".to_string()),
        Some(_) => None,
    }
}

/// Draw the resamples and refit each one, returning the per-parameter samples and how
/// many refits produced a value that is not finite.
///
/// This is the loop the work budget in `ingest::check_bootstrap_budget` was protecting:
/// `rows * retained * samples` multiply-adds, refused at load if it would be too many.
///
/// Each refit reuses the decomposition, so it is a single projection through `U` and
/// back through `V` rather than a fresh solve. The design has not changed -- only the
/// noise has -- so re-decomposing it would be doing the expensive thing to get the same
/// answer.
#[allow(clippy::too_many_arguments)]
fn resample(
    problem: &Problem,
    decomposition: &Decomposition,
    coefficient: &[f64],
    filter: &[f64],
    projections: &[f64],
    weighted_residuals: &[f64],
    leverages: &[f64],
    plan: crate::ingest::BootstrapPlan,
) -> (Vec<Vec<f64>>, usize) {
    let rows = problem.rows();
    let columns = problem.columns();

    // Leverage-corrected, then centered. Both corrections are standard and both matter:
    // without the leverage correction the resampled noise is too small, and without
    // centering the refits are biased.
    let mut pool: Vec<f64> = leverages
        .iter()
        .zip(weighted_residuals.iter())
        .map(|(leverage, residual)| residual / (1.0 - leverage).sqrt())
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let mean = pool.iter().sum::<f64>() / rows as f64;
    for value in &mut pool {
        *value -= mean;
    }

    // The deterministic part of every synthetic dataset: what the current fit predicts,
    // minus what the prior alone predicts, in scaled units. That is `A~ (x - x_prior)`,
    // which the decomposition already knows as `U F c` -- so the scaled design does not
    // need to be touched again.
    let mut deterministic: DVector<f64> = DVector::zeros(rows);
    for i in 0..rows {
        deterministic[i] = filter
            .iter()
            .enumerate()
            .take(decomposition.rank)
            .map(|(j, f)| decomposition.u[(i, j)] * f * projections[j])
            .sum::<f64>();
    }

    let mut rng = bootstrap_stream(plan.seed);
    let mut samples: Vec<Vec<f64>> = vec![Vec::with_capacity(plan.samples); columns];
    let mut failed = 0_usize;
    let mut synthetic = DVector::zeros(rows);

    for _ in 0..plan.samples {
        for i in 0..rows {
            let pick = uniform_below(&mut rng, rows);
            synthetic[i] = deterministic[i] + pool[pick];
        }
        let projected = decomposition.u.tr_mul(&synthetic);
        let weighted: DVector<f64> = DVector::from_iterator(
            projected.len(),
            projected
                .iter()
                .zip(coefficient.iter())
                .map(|(value, d)| value * d),
        );
        let standard_form = &decomposition.v * weighted;

        let estimate: Option<Vec<f64>> = (0..columns)
            .map(|j| {
                let value = problem.prior[j] + standard_form[j] / problem.penalty[j];
                value.is_finite().then_some(value)
            })
            .collect();

        match estimate {
            Some(estimate) => {
                for (column, value) in samples.iter_mut().zip(estimate) {
                    column.push(value);
                }
            }
            // A refit that came back non-finite is excluded from the percentiles and
            // counted. It is never quietly replaced with a plausible number.
            None => failed += 1,
        }
    }

    (samples, failed)
}

/// The seeded residual bootstrap.
///
/// Standardized residuals are resampled with replacement, added back to the fit, and
/// the model is refit with the *same* lambda. Because the design does not change, the
/// decomposition is reused and each refit is a single projection.
///
/// This is an uncertainty aid, not a posterior. It answers "how much would this
/// estimate move if the noise had fallen differently, assuming the noise is drawn
/// from the residuals I actually saw" and nothing more.
#[allow(clippy::too_many_arguments)]
fn compute_bootstrap(
    problem: &Problem,
    decomposition: &Decomposition,
    coefficient: &[f64],
    filter: &[f64],
    projections: &[f64],
    weighted_residuals: &[f64],
    leverages: &[f64],
    reduced_chi_square: Option<f64>,
    refusal: Option<&String>,
    warnings: &mut Vec<Warning>,
) -> Result<Bootstrap> {
    let Some(plan) = problem.uncertainty.bootstrap else {
        return Ok(Bootstrap {
            status: UncertaintyStatus::NotRequested,
            reason: None,
            requested_samples: 0,
            successful_samples: 0,
            failed_samples: 0,
            seed: None,
            lower: Vec::new(),
            upper: Vec::new(),
        });
    };

    let refuse = |reason: String, warnings: &mut Vec<Warning>| {
        warnings.push(Warning::new(
            "bootstrap_refused",
            format!("no bootstrap was published: {reason}"),
        ));
        Ok(Bootstrap {
            status: UncertaintyStatus::Refused,
            reason: Some(reason),
            requested_samples: plan.samples,
            successful_samples: 0,
            failed_samples: 0,
            seed: Some(plan.seed),
            lower: Vec::new(),
            upper: Vec::new(),
        })
    };

    if let Some(reason) = refusal {
        return refuse(reason.clone(), warnings);
    }
    if let Some(reason) = bootstrap_refusal(problem, leverages, reduced_chi_square) {
        return refuse(reason, warnings);
    }

    let columns = problem.columns();
    let (mut samples, failed) = resample(
        problem,
        decomposition,
        coefficient,
        filter,
        projections,
        weighted_residuals,
        leverages,
        plan,
    );

    let successful = plan.samples - failed;
    if failed > 0 {
        warnings.push(Warning::new(
            "bootstrap_samples_failed",
            format!(
                "{failed} of {} bootstrap refits produced a value that is not finite and were excluded from the percentiles",
                plan.samples
            ),
        ));
    }
    if successful < 2 {
        return refuse(
            format!(
                "only {successful} of {} bootstrap refits produced a finite estimate, which is not enough to read a percentile from",
                plan.samples
            ),
            warnings,
        );
    }

    let alpha = 1.0 - problem.uncertainty.confidence_level;
    let low = 0.5 * alpha;
    let high = 1.0 - low;
    let mut lower = vec![0.0; columns];
    let mut upper = vec![0.0; columns];
    for j in 0..columns {
        let column = samples
            .get_mut(j)
            .ok_or_else(|| Error::Internal("bootstrap lost a parameter".into()))?;
        column.sort_unstable_by(f64::total_cmp);
        lower[j] = percentile_of_sorted(column, low)
            .ok_or_else(|| Error::Internal("empty bootstrap percentile".into()))?;
        upper[j] = percentile_of_sorted(column, high)
            .ok_or_else(|| Error::Internal("empty bootstrap percentile".into()))?;
    }

    Ok(Bootstrap {
        status: UncertaintyStatus::Emitted,
        reason: None,
        requested_samples: plan.samples,
        successful_samples: successful,
        failed_samples: failed,
        seed: Some(plan.seed),
        lower,
        upper,
    })
}

/// Nothing that is not a finite number is allowed to reach a bundle.
fn check_finite(solution: &Solution) -> Result<()> {
    let mut groups: Vec<(&str, &[f64])> = vec![
        ("estimates", &solution.estimates),
        ("predictions", &solution.predictions),
        ("residuals", &solution.residuals),
        ("leverages", &solution.leverages),
        ("singular values", &solution.singular_values),
    ];
    groups.push((
        "analytic standard errors",
        &solution.analytic.standard_errors,
    ));
    groups.push(("bootstrap lower bounds", &solution.bootstrap.lower));
    groups.push(("bootstrap upper bounds", &solution.bootstrap.upper));

    for (name, values) in groups {
        if let Some(bad) = values.iter().find(|v| !v.is_finite()) {
            return Err(NumericalError::NonFiniteResult {
                detail: format!("the {name} contain {bad:?}"),
            }
            .into());
        }
    }
    for (name, value) in [
        ("weighted residual sum of squares", solution.weighted_rss),
        ("weighted RMSE", solution.weighted_rmse),
        ("effective degrees of freedom", solution.effective_dof),
        ("lambda", solution.lambda),
    ] {
        if !value.is_finite() {
            return Err(NumericalError::NonFiniteResult {
                detail: format!("the {name} came out as {value:?}"),
            }
            .into());
        }
    }
    Ok(())
}

impl Solution {
    /// The analytic interval for one parameter, if there is one.
    pub fn analytic_bounds(&self, index: usize) -> Option<(f64, f64)> {
        if self.analytic.status != UncertaintyStatus::Emitted {
            return None;
        }
        Some((
            *self.analytic.lower.get(index)?,
            *self.analytic.upper.get(index)?,
        ))
    }

    /// The bootstrap interval for one parameter, if there is one.
    pub fn bootstrap_bounds(&self, index: usize) -> Option<(f64, f64)> {
        if self.bootstrap.status != UncertaintyStatus::Emitted {
            return None;
        }
        Some((
            *self.bootstrap.lower.get(index)?,
            *self.bootstrap.upper.get(index)?,
        ))
    }

    /// The interval `parameters.csv` reports. Its provenance is written alongside it
    /// in the `interval_source` column, because an interval whose method is unstated
    /// cannot be interpreted.
    pub fn reported_interval(&self, index: usize) -> Option<(f64, f64)> {
        match self.interval_source {
            IntervalSource::Analytic => self.analytic_bounds(index),
            IntervalSource::Bootstrap => self.bootstrap_bounds(index),
            IntervalSource::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    /// Exactly equal, bit for bit. The grid's endpoints are the declared values
    /// *copied*, not `exp(ln(x))` round trips of them, so "exactly" is the claim being
    /// made and a tolerance would let a lossy round trip pass unnoticed.
    #[track_caller]
    fn exactly(got: f64, expected: f64) {
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "got {got:e}, expected exactly {expected:e}"
        );
    }

    #[test]
    fn the_log_grid_hits_its_declared_endpoints_exactly() {
        let grid = log_grid(1e-6, 1e2, 5);
        assert_eq!(grid.len(), 5);
        exactly(grid[0], 1e-6); // the first point is the declared minimum
        exactly(grid[4], 1e2); // and the last is the declared maximum
                               // Evenly spaced in log space.
        let ratios: Vec<f64> = grid.windows(2).map(|w| w[1] / w[0]).collect();
        for ratio in &ratios {
            assert!((ratio - ratios[0]).abs() < 1e-9, "{ratios:?}");
        }
        assert!(grid.windows(2).all(|w| w[0] < w[1]), "must ascend");
    }

    fn candidate(lambda: f64, score: Option<f64>) -> GcvCandidate {
        GcvCandidate {
            lambda,
            score,
            effective_dof: 1.0,
            weighted_rss: 1.0,
        }
    }

    #[test]
    fn an_exact_gcv_tie_resolves_toward_the_larger_lambda() {
        // Three candidates, the outer two tied at the minimum. The larger lambda wins.
        let candidates = vec![
            candidate(0.1, Some(1.0)),
            candidate(1.0, Some(2.0)),
            candidate(10.0, Some(1.0)),
        ];
        let chosen = select_lambda(&candidates).unwrap();
        assert_eq!(chosen, 2, "the tie must resolve to the larger lambda");
        exactly(candidates[chosen].lambda, 10.0);
    }

    #[test]
    fn every_candidate_being_tied_selects_the_largest_lambda() {
        let candidates: Vec<GcvCandidate> = (0..5)
            .map(|i| candidate(f64::from(i + 1), Some(3.0)))
            .collect();
        assert_eq!(select_lambda(&candidates), Some(4));
    }

    #[test]
    fn undefined_candidates_are_skipped_not_scored_as_zero() {
        let candidates = vec![
            candidate(0.1, None),
            candidate(1.0, Some(5.0)),
            candidate(10.0, None),
        ];
        assert_eq!(select_lambda(&candidates), Some(1));
    }

    #[test]
    fn a_grid_with_no_defined_candidate_selects_nothing() {
        let candidates = vec![candidate(0.1, None), candidate(1.0, None)];
        assert_eq!(select_lambda(&candidates), None);
    }

    #[test]
    fn a_strict_minimum_wins_regardless_of_position() {
        let candidates = vec![
            candidate(0.1, Some(9.0)),
            candidate(1.0, Some(1.0)),
            candidate(10.0, Some(9.0)),
        ];
        assert_eq!(select_lambda(&candidates), Some(1));
    }
}
