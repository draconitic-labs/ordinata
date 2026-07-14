# Fixtures

Every case here is either **synthetic** (generated from a model this file states) or
**independently derived** (solved by hand, with the algebra written out, so that the
expected numbers do not come from Ordinata).

Nothing in this directory is measured data. None of it describes anything in the
world.

The point of an independently derived case is that it can fail. If the expected
values were produced by running Ordinata and pasting the output, the test would only
prove that Ordinata is self-consistent, which it would be even if the solver were
wrong. So for the small cases the solution is computed by hand from the normal
equations using Cramer's rule -- a completely different route to the answer than the
singular value decomposition the solver actually takes -- and the derivation is shown
in each case's `PROVENANCE.md`.

| case | what it pins down |
|------|-------------------|
| `exact-recovery` | a noiseless system is recovered exactly, and the analytic covariance is the textbook `(A'A)^-1` |
| `weighted-heteroscedastic` | unequal standard errors are weighted correctly, checked against a hand solution |
| `ridge-prior-penalty` | Tikhonov around a nonzero prior with a non-uniform diagonal penalty, checked against a hand solution |
| `rank-deficient` | a duplicated column is seen as a duplicated column, and uncertainty is refused rather than manufactured |
| `ill-conditioned` | regularization chosen by GCV beats no regularization at all on held-out truth, *for this instance* |

The `ill-conditioned` case is generated in the test rather than stored, because the
whole point of it is the held-out truth, and a fixture file that carried the truth
alongside the observations would be a fixture that lies about what a project can
know.
