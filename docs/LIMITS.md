# Limits and budgets

Every bound Ordinata enforces, what it is for, and why it is that number.

All of these are in `spec.rs`, and all of them are copied into `manifest.json` under
`solver.limits` and `solver.thresholds`, so a bundle carries the rules it was made
under.

## Why there are limits at all

Ordinata is pointed at files by people, and sometimes those files are wrong, and
occasionally they are hostile. A tool with no bounds meets a bad input by exhausting the
machine. A tool with bounds meets it by saying which bound it broke.

Every limit below is enforced **before** the work it bounds, not after. That is the
distinction that matters: a budget consulted after the allocation is a report, not a
budget.

## Problem size

| limit | value | why |
|---|---|---|
| observations | 2 – 50,000 | one row cannot fit anything; the ceiling is where the memory budget below starts to bite |
| parameters | 1 – 256 | a 256-column design is already far past where a linear model is reviewable |
| `project.json` | 1 MiB | a project file is configuration, not data |
| CSV input | 128 MiB each | a 50,000 × 256 design in text is well under this |
| identifier | 64 bytes | observation IDs and parameter names |
| CSV field | 256 bytes | any single cell |
| title | 200 bytes | |
| free text | 2,000 bytes each, 64 entries | assumptions, limitations, description |
| citations | 64 | |
| path | 4,096 bytes, 32 components | |

## Regularization and uncertainty

| limit | value |
|---|---|
| GCV grid steps | 3 – 200 |
| bootstrap samples | 100 – 10,000 |
| confidence level | 0.5 – 0.999 |

A bootstrap request is never silently reduced. If the work budget below refuses it, the
number of samples in the bundle is still the number the project asked for, because the
bundle was never written.

## The memory budget

```
bytes = 8 * (3*rows*parameters + 2*rows*k + 2*parameters*k + k),   k = min(rows, parameters)
budget = 1 GiB
```

The three `rows × parameters` terms are the ingested design, its row-scaled copy, and the
penalty-scaled copy the decomposition consumes. The `k` terms are the left and right
singular vectors, before and after they are sorted into descending order.

The estimate deliberately **over**-counts: it assumes every component is retained, which
is only true at full rank. Over-counting is the safe direction, because the estimate is
what a refusal is based on.

The check runs in `ingest::assemble`, at the last moment the shape is known and nothing
large has been built from it. Everything after that line allocates.

**The largest problem the dimension limits admit — 50,000 × 256 — estimates at about
513 MiB, so this budget refuses nothing that the input contract accepts.** That is
deliberate, and it is the whole design: a contract that promised to accept a size and
then refused it on a resource limit the documentation never mentioned would be a
contract with a hole in it. The budget is the guard rail that any future widening of the
dimension limits has to get past. `spec::budgets_admit_the_largest_declared_problem` is
the test that keeps the two in step: raise a dimension limit without raising the budget,
and it fails there rather than on somebody's data.

## The bootstrap work budget

```
work = rows * retained_components * samples
budget = 2,000,000,000
```

One unit is one multiply-add in the projection through the retained left singular
vectors, which is exactly what one bootstrap refit costs.

The number is a wall-clock ceiling wearing a portable unit. Measured on the machine this
was calibrated on — an optimized build, x86-64 — Ordinata sustains about **6.3 × 10⁸
units per second** (2,000 rows × 20 parameters × 10,000 samples = 4 × 10⁸ units in
0.63 s, including parse, decomposition, and bundle write). The budget is therefore
roughly three seconds of arithmetic there, and well under a minute on a machine several
times slower.

The retained count is not known until the decomposition has run, so the budget is checked
against `min(rows, parameters)`, which can only be larger. The estimate can therefore
refuse early but never late.

Like the memory budget, it is set so that **every problem inside the declared dimension
limits can still afford the minimum bootstrap**: 50,000 × 256 × 100 = 1.28 × 10⁹ units,
inside the budget. A refusal always names the largest sample count you may ask for
instead, and that number is always one you are actually allowed to ask for.

## Bundle and archive limits

| limit | value | why |
|---|---|---|
| members in a bundle | 64 | a bundle has ten |
| a single member | 256 MiB | |
| a whole bundle, uncompressed | 512 MiB | |
| an archive file, on disk | 256 MiB | applied to the **read**, not after it |
| archive directory depth | 8 | |

Two of these are worth spelling out, because the obvious implementation of each is wrong.

**The archive size cap is on the read.** `std::fs::read(path)` allocates the whole file
and only then tells you how big it was. A verifier is pointed at files it does not trust
by definition — that is its entire job — so it does not get to allocate a 40 GiB file in
order to discover that it is over the limit. `take(limit + 1)` reads one byte past the
cap, which is exactly enough to know the cap was broken.

**The whole-bundle cap is checked against the sum of all declared sizes, before any entry
is decompressed.** Checking each entry against the remaining budget as the loop reaches it
is not enough, and the difference is the attack: three entries of 250 MiB each are
individually under the per-member cap, so a running check waves the first two through —
half a gigabyte, already spent — and only objects at the third. The central directory says
up front how much the archive intends to expand to. Adding that up costs nothing, and it
means a bomb is refused while it is still a few hundred bytes on disk.

## Thresholds

| threshold | value | what it decides |
|---|---|---|
| rank tolerance | `max(rows, params) * sigma_max * f64::EPSILON` | which singular values are zero |
| residual d.o.f. floor | `rows * 1e-9` | when a fit has nothing left to judge it with |
| leverage floor | `1e-8` | when a standardized residual does not exist |
| degenerate residual | reduced chi-square ≤ `1e-6` | when a bootstrap would manufacture zero width |
| residual scale warning | reduced chi-square < `0.1` or > `10` | when the declared errors do not match the residuals |
| ill-conditioned warning | condition number > `1e8` | |

## What Ordinata will not do at any size

No network. No telemetry. No subprocess. No plugin. No code evaluation. No environment
variable that changes a number. No file written outside the one output directory you
named. See [SECURITY.md](SECURITY.md) and [PRIVACY.md](PRIVACY.md).
