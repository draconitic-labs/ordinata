# Exact two-parameter recovery

**Synthetic.** Constructed, not measured.

## Construction

Five points at `x = 0, 1, 2, 3, 4` on the line `y = 3 + 2x`, giving
`y = 3, 5, 7, 9, 11`. Every standard error is exactly 1, so the weight matrix is the
identity and the weighted problem is the ordinary one.

## Independent derivation of the expected answer

With unit weights, `A' A` and `A' b` are

```
A'A = [ 5   10 ]      A'b = [ 35 ]
      [ 10  30 ]            [ 90 ]
```

because `sum(1) = 5`, `sum(x) = 0+1+2+3+4 = 10`, `sum(x^2) = 0+1+4+9+16 = 30`,
`sum(y) = 3+5+7+9+11 = 35`, and `sum(x*y) = 0+5+14+27+44 = 90`.

`det(A'A) = 5*30 - 10*10 = 150 - 100 = 50`.

By Cramer's rule:

```
intercept = (35*30 - 10*90) / 50 = (1050 - 900) / 50 = 150 / 50 = 3
slope     = ( 5*90 - 10*35) / 50 = ( 450 - 350) / 50 = 100 / 50 = 2
```

so the estimates are exactly `3` and `2`, which is the line the data was built from.

The covariance of the unregularized estimator with unit-variance noise is `(A'A)^-1`:

```
(A'A)^-1 = (1/50) [  30  -10 ]  =  [  0.6  -0.2 ]
                  [ -10    5 ]     [ -0.2   0.1 ]
```

so the standard errors are `sqrt(0.6) = 0.774596669241483...` and
`sqrt(0.1) = 0.316227766016838...`.

None of the above used a singular value decomposition. The solver does. That is the
point: the two routes have to agree.

## What Ordinata is expected to say about it

* rank 2 of 2, so uncertainty is published;
* residual sum of squares 0, and therefore a reduced chi-square of 0;
* a `residual_scale_inconsistent_with_declared_errors` warning, because residuals of
  exactly zero are not what standard errors of 1 predict. A fit this good is evidence
  that the declared uncertainties are wrong, and Ordinata says so instead of
  presenting a perfect fit as a triumph.
