# The model

This document states exactly what Ordinata computes. Every rule and threshold named
here is also embedded in `manifest.json` under `solver`, so a reader who has a bundle
and not this file can still recover the rules that produced their numbers.

If any rule below changes, `spec::MODEL_VERSION` is bumped. A bundle written by a
different model version is a bundle whose numbers were made by different rules, and it
says so on its face.

## The problem

Given a design `A` (rows are observations, columns are parameters), observed values
`b`, and a declared standard error `s_i` for each observation, Ordinata minimizes

```
|| A~ x - b~ ||^2  +  lambda * || L (x - x_prior) ||^2
```

where `A~` and `b~` are `A` and `b` with row `i` divided by `s_i`, `L` is the declared
positive diagonal penalty (default: the identity), and `x_prior` is the declared prior
(default: zero).

This is the normal-equation system

```
(A' W A + lambda L'L) x = A' W b + lambda L'L x_prior,   W = diag(1 / s_i^2)
```

but Ordinata never forms it. Forming `A'WA` squares the condition number, and inverting
it is precisely the unstable step this instrument exists to avoid.

## How it is solved

Substituting `y = x - x_prior` and then `z = L y` removes the cross terms:

```
minimize  || B z - r ||^2 + lambda * || z ||^2,
  where   B = A~ L^-1   and   r = b~ - A~ x_prior
```

One singular value decomposition `B = U S V'` then answers every question:

```
z(lambda) = V diag( s_i / (s_i^2 + lambda) ) U' r
```

The coefficient `s_i / (s_i^2 + lambda)` is the point. Written this way, nothing is ever
divided by a small singular value: as `s_i` shrinks the coefficient shrinks with it,
rather than exploding the way `1/s_i` would.

Because the decomposition does not depend on `lambda`, every GCV candidate costs `O(k)`
and every bootstrap refit is a single projection. The expensive step happens once.

In code the coefficient is computed as `1 / (s + lambda/s)` rather than from `s*s`,
because `s*s` overflows for a singular value above about `1e154` and a design with
entries near the top of the double range is something a user can actually hand us.

## Numerical rank

```
rank = #{ i : sigma_i > max(rows, parameters) * sigma_max * f64::EPSILON }
```

A singular value at or below that tolerance is treated as exactly zero and its component
is dropped. Keeping it would let a direction the data says nothing about contribute to
the estimate on the strength of rounding noise.

Rank zero is a refusal (`zero_rank`, exit 3): a design that carries no information about
any parameter is not a problem Ordinata can answer.

## Condition number

```
condition = sigma_max / sigma_min,
  reported only when rank == parameters
```

The comparison is against the **parameter count**, not against the number of singular
values that came back, and the difference is not academic.

A thin decomposition of an `m x n` design returns `min(m, n)` singular values. When
`m < n` — two observations, three parameters — it returns two, and both may be well
above the tolerance. Comparing the rank to *that* number calls the design full rank and
hands out a finite condition number for a system that does not determine its parameters
at all. The missing singular values are not absent; they are zero.

So: a design whose rank is below its parameter count has no finite condition number,
`diagnostics.json` records `null`, and a declared `conditionNumberMax` **fails**. It is
not excused from the check. A design that determines nothing is not a well-conditioned
one; it is the limiting case of a badly conditioned one.

Above `1e8`, a finite condition number raises the `ill_conditioned` warning.

## Choosing lambda

`explicit` uses the declared value, which may be zero.

`gcv` searches an inclusive log-spaced grid of `steps` candidates between `minLambda`
and `maxLambda`. The endpoints are the exact declared values, not `exp(ln(x))` round
trips of them.

```
score(lambda) = rows * weighted_rss(lambda) / (rows - effective_dof(lambda))^2
effective_dof(lambda) = sum_i sigma_i^2 / (sigma_i^2 + lambda)
```

Every candidate is scored and every candidate is reported, including the ones whose
score is undefined, which are marked `undefined` rather than dropped. A candidate whose
denominator has collapsed — residual degrees of freedom below `rows * 1e-9` — has no
score. A grid on which *no* candidate has a score is a refusal
(`ill_defined_degrees_of_freedom`, exit 3).

Candidates are scanned in ascending lambda and the last strict-or-equal minimum wins, so
an exact tie resolves toward the **larger** lambda: when two regularization strengths fit
the data equally well, Ordinata takes the more conservative one.

If the winner is at either end of the grid, the `gcv_selected_grid_endpoint` warning
fires. A minimum on the edge of the grid is a minimum *of the grid*, not necessarily of
the function, and the difference between a selected lambda and a trusted one is worth
saying out loud.

## Uncertainty

Ordinata computes an interval two ways, and it does not reconcile them, because they are
not the same claim.

### Analytic

After row scaling the noise has unit variance by construction, so the linear estimator
`x = x_prior + G r` with `G = L^-1 V D U'` has covariance `G G'`, and only its diagonal
is needed:

```
var(x_j) = (1 / L_j^2) * sum_i (V_ji * D_i)^2,   D_i = sigma_i / (sigma_i^2 + lambda)
interval = estimate +/- z * sqrt(var),           z = Phi^-1((1 + confidence) / 2)
```

The residual scale is **not** re-estimated from the fit. The user declared the standard
errors; substituting a scale read back off the residuals would quietly replace their
stated uncertainties with ours. The reduced chi-square is reported instead, so a mismatch
between the two is visible rather than absorbed.

When `lambda > 0` the `analytic_covariance_excludes_regularization_bias` warning fires
and quantifies the pull: it reports how many of the degrees of freedom the data could
have supported the penalty has spent. **The interval is the sampling spread of the
regularized estimate. It is an interval around the pulled estimate, not around the
truth.** Regularization trades variance for bias, and the analytic interval sees only the
variance half of that trade.

`Phi^-1` is Wichura's AS241 (algorithm PPND16), written out rather than taken from a
statistics crate, so that it is specifiable in a paragraph and cannot be moved by a
dependency's minor version.

### Bootstrap

Standardized residuals are leverage-corrected, centred, resampled with replacement,
added back to the fit, and the model is refit **with the same lambda**. Because the
design does not change, the decomposition is reused and each refit is one projection.

```
key    = SHA-256("ordinata/bootstrap/v1" || seed.to_le_bytes())
stream = ChaCha20(key)
index  = rejection sampling of next_u64() against the largest multiple of the row count
bound  = linear interpolation at rank (n - 1) * p on the ascending sample
```

The seed is hashed rather than used raw, so neighbouring seeds give unrelated streams and
the label pins the stream to this use. Given the seed and these three rules, a third party
can regenerate the exact indices without Ordinata.

**The bootstrap is an uncertainty aid. It is not a posterior.** It answers "how much
would this estimate move if the noise had fallen differently, assuming the noise is drawn
from the residuals I actually saw", and nothing more. It does not know about the prior,
it does not know about model error, and it inherits every assumption in the project.

### When both are asked for

Both are computed and **both are exported**, in columns of their own, in
`parameters.csv`, in `diagnostics.json`, and in `report.html`. Neither is dropped, and
neither is averaged into the other.

They will not agree in general, and that is the point. The analytic interval trusts the
standard errors the project declared. The bootstrap interval trusts the residuals the fit
actually left. A wide gap between them is a statement about the declared errors — it is
the reduced chi-square, made visible — and it is information a reviewer opens the bundle
to find. The `interval_lower`/`interval_upper` columns carry a primary interval for a
reader with one column of space, `interval_source` says which method it came from, and
nothing is discarded to make room for it.

## When Ordinata refuses to give an interval

Two cases, and one rule behind both.

**The design is rank deficient** (`rank < parameters`). Along a direction the data cannot
see, a regularized estimator has almost no *variance* — it simply returns the prior there,
run after run — while carrying unbounded *bias*. Both the covariance and the bootstrap
would therefore report a nearly zero interval for a number the data did not constrain at
all. That is the precise failure this instrument exists to refuse. Ordinata publishes the
estimate, the rank, and the warning, and publishes **no uncertainty whatsoever**.

**There are no residual degrees of freedom** (`rows - effective_dof <= rows * 1e-9`). The
model has used up the data reproducing it, and nothing is left over to say how well it
did. An interval drawn from such a fit describes the arithmetic, not the world.

The bootstrap refuses in two further cases of its own: a point the fit passes exactly
through has no standardized residual to resample, and residuals with no spread (reduced
chi-square at or below `1e-6`) would manufacture an interval of zero width — which would
not be a confident result, but a fabricated one.

A refused uncertainty is **absent**, not zero. The columns are empty and the reason is
carried in `diagnostics.json` and stated in the report.

## Diagnostics that are always reported

| quantity | meaning |
|---|---|
| `rank`, `rankTolerance`, `singularValues` | what the data can and cannot see |
| `conditionNumber` | `null` when the design is not identifiable |
| `effectiveDegreesOfFreedom` | `trace(H)`, what the fit spent |
| `residualDegreesOfFreedom` | `rows - effective_dof`, what is left to judge it with |
| `weightedRootMeanSquare` | `sqrt(weighted_rss / rows)` |
| `reducedChiSquare` | `weighted_rss / residual_dof`; near 1 when the declared errors are right |
| `maxLeverage`, `maxAbsStandardizedResidual` | which observations the fit is leaning on |

A reduced chi-square below `0.1` or above `10` raises
`residual_scale_inconsistent_with_declared_errors`. Far from 1 in either direction, the
declared standard errors do not describe the residuals, and the two interval methods will
disagree in width for exactly that reason.

## What this model is not

It is not a nonlinear fitter, a constrained optimizer, a Bayesian posterior, a sampler, a
PDE solver, or a model-selection procedure. It does not choose your design, check your
assumptions, or know whether a linear model is the right one. It fits the model you
declared to the data you declared, and it tells you what the data could and could not
determine.

See [LIMITS.md](LIMITS.md) for the bounds, and [VALIDATION.md](VALIDATION.md) for the
evidence that this document is true of the code.
