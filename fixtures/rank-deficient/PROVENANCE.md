# Rank-deficient design

**Synthetic.** Constructed to be exactly degenerate.

## The trap

The design has three columns:

```
offset         = 1
tempC          = 0, 10, 20, 30, 40
tempF_shifted  = 0, 18, 36, 54, 72
```

and `tempF_shifted = 1.8 * tempC`, exactly, at every row. This is not a contrived
error. It is what happens when a spreadsheet grows a "temperature in Fahrenheit"
column and the offset gets dropped somewhere along the way, and it is the sort of
thing that survives review because both columns *look* like real data.

Three parameters, but only two independent directions. The numerical rank is 2.

## Why the answer is not just "wide intervals"

The tempting thing for a solver to do is to regularize, produce some pair of
coefficients, and report an interval for each. That would be wrong in a specific and
dangerous way.

Along the unidentified direction -- increase the `tempC` coefficient by `1.8*d` and
decrease `tempF_shifted` by `d`, and every prediction is unchanged -- a regularized
estimator does not wobble. It returns whatever the prior and the penalty put there,
the same way every time. So its *sampling variance* along that direction is
essentially zero, while its *bias* is unbounded: the truth could be anywhere along
that line.

A covariance would therefore report a nearly zero interval. A residual bootstrap
would too, and for the same reason: resampling the residuals does not move an
estimate the residuals never constrained. Both methods would confidently print a
narrow interval around a number the data says nothing about.

That is the single most dangerous thing this instrument could do, so it does not do
it. When `rank < parameters`, Ordinata publishes the estimate, the rank, the singular
values, and a warning, and publishes **no uncertainty at all** -- neither analytic nor
bootstrap, regardless of which the project asked for.

## What Ordinata is expected to say about it

* numerical rank 2 of 3, with the third singular value at or below the tolerance;
* `conditionNumber` reported as `null`, because a rank-deficient matrix does not have
  a finite one;
* the declared `conditionNumberMax` check **fails** -- a matrix with no finite
  condition number is not within any limit, and being unmeasurable is not the same as
  being excused;
* a `rank_deficient` warning naming the number of unresolved directions;
* `analytic.status = refused` and `bootstrap.status = refused`, each carrying the
  reason above, even though the project asked for `both`;
* `interval_source = none` in `parameters.csv`, with the interval columns empty
  rather than zero.
