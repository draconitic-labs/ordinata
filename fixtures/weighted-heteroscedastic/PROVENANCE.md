# Weighted fit with unequal standard errors

**Synthetic**, with an **independently derived** expected answer.

## Data

| id | x | y | standard error | weight `1/s^2` |
|----|---|---|----------------|----------------|
| q0 | 0 | 1 | 1 | 1 |
| q1 | 1 | 3 | 1 | 1 |
| q2 | 2 | 2 | 2 | 0.25 |
| q3 | 3 | 5 | 2 | 0.25 |

The last two points are twice as uncertain, so they carry a quarter of the weight.
They are also the two that pull hardest against the first two, which is what makes
the case discriminating: a solver that ignored the weights would not merely be
slightly off, it would be visibly off.

## Derivation by hand

The weighted normal equations are `(A'WA) x = A'Wb`, with

```
sum(w)     = 1 + 1 + 0.25 + 0.25                      = 2.50
sum(w x)   = 0 + 1 + 0.25*2 + 0.25*3                  = 2.25
sum(w x^2) = 0 + 1 + 0.25*4 + 0.25*9                  = 4.25
sum(w y)   = 1 + 3 + 0.25*2 + 0.25*5                  = 5.75
sum(w x y) = 0 + 3 + 0.25*2*2 + 0.25*3*5              = 7.75
```

so

```
A'WA = [ 2.50  2.25 ]      A'Wb = [ 5.75 ]
       [ 2.25  4.25 ]             [ 7.75 ]
```

`det = 2.50*4.25 - 2.25^2 = 10.625 - 5.0625 = 5.5625 = 89/16`.

By Cramer's rule:

```
intercept = (5.75*4.25 - 2.25*7.75) / det = (24.4375 - 17.4375) / (89/16) = 7 * 16/89 = 112/89
slope     = (2.50*7.75 - 2.25*5.75) / det = (19.375  - 12.9375) / (89/16) = 6.4375 * 16/89 = 103/89
```

```
intercept = 112/89 = 1.2584269662921348...
slope     = 103/89 = 1.1573033707865169...
```

The covariance of the unregularized weighted estimator, with the declared standard
errors setting the noise scale, is `(A'WA)^-1`:

```
(A'WA)^-1 = (16/89) [  4.25  -2.25 ]  =  [  68/89  -36/89 ]
                    [ -2.25   2.50 ]     [ -36/89   40/89 ]
```

so `var(intercept) = 68/89` and `var(slope) = 40/89`, giving

```
se(intercept) = sqrt(68/89) = 0.8740966...
se(slope)     = sqrt(40/89) = 0.6704016...
```

## Why this is an independent check

The derivation above forms `A'WA` explicitly and inverts a 2x2 matrix by Cramer's
rule. Ordinata does neither: it scales the rows, decomposes the result with a
singular value decomposition, and never forms `A'WA` at all, precisely because doing
so squares the condition number. Two different routes, one answer. If the solver
disagrees with this file, one of them is wrong, and the test does not care which.

For contrast, the *unweighted* fit of the same points has
`sum(1)=4, sum(x)=6, sum(x^2)=14, sum(y)=11, sum(xy)=22`, determinant
`4*14 - 6^2 = 20`, and therefore an intercept of `22/20 = 1.1` and a slope of
`22/20 = 1.1`. Both differ from the weighted answer in the first decimal place, so a
solver that quietly ignored the standard errors could not sneak past this test.
