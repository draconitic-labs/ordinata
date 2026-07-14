# Explicit ridge around a nonzero prior with a non-uniform penalty

**Synthetic**, with an **independently derived** expected answer.

Same four points and same weights as `weighted-heteroscedastic`, so the difference in
the answer is entirely attributable to the regularization. That is deliberate: the
two cases together isolate the penalty term.

## Configuration

```
lambda   = 2
x_prior  = (1, 1)
L        = diag(1, 2)      so   L'L = diag(1, 4)
```

The prior is nonzero and the penalty is non-uniform, which is the case that catches
implementations that quietly assume one or both are trivial. A solver that shrank
toward zero instead of toward the prior, or that used a scalar penalty instead of the
declared diagonal, produces a different number here.

## Derivation by hand

The system to solve is the one in the specification:

```
(A'WA + lambda L'L) x = A'Wb + lambda L'L x_prior
```

From the heteroscedastic case, `A'WA = [[2.50, 2.25], [2.25, 4.25]]` and
`A'Wb = (5.75, 7.75)`. Then

```
lambda L'L         = 2 * diag(1, 4) = diag(2, 8)

A'WA + lambda L'L  = [ 4.50  2.25 ]
                     [ 2.25 12.25 ]

lambda L'L x_prior = (2*1*1, 2*4*1) = (2, 8)

right-hand side    = (5.75 + 2, 7.75 + 8) = (7.75, 15.75)
```

`det = 4.50*12.25 - 2.25^2 = 55.125 - 5.0625 = 50.0625 = 801/16`.

By Cramer's rule:

```
intercept = (7.75*12.25 - 2.25*15.75) / det = (94.9375 - 35.4375) * 16/801 = 59.5 * 16/801 = 952/801
slope     = (4.50*15.75 - 2.25* 7.75) / det = (70.875  - 17.4375) * 16/801 = 53.4375 * 16/801 = 855/801 = 95/89
```

```
intercept = 952/801 = 1.1885143570536829...
slope     =  95/89  = 1.0674157303370787...
```

Both have moved from their unregularized values (`112/89 = 1.2584...` and
`103/89 = 1.1573...`) toward the prior at `(1, 1)`, and the slope has moved further,
which is what a penalty of 2 on the slope against 1 on the intercept is supposed to
do.

## Why this is an independent check

Ordinata never forms `A'WA + lambda L'L` and never inverts it. It substitutes
`z = L (x - x_prior)`, which turns the problem into a standard-form Tikhonov problem
`min ||B z - r||^2 + lambda ||z||^2` with `B = A~ L^-1`, and reads the answer off a
single singular value decomposition of `B` using the filter coefficients
`sigma / (sigma^2 + lambda)`. The algebra above and the algebra in the solver have
almost nothing in common beyond being correct, which is exactly what makes the
agreement worth something.
