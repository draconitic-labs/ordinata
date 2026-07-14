# File formats

Everything Ordinata reads and everything it writes.

## Input: `project.json`

Schema version 1. **Unknown fields are refused**, so a misspelled key is a loud error and
not a silently dropped setting. **A repeated key is refused**, so the document means one
thing or nothing.

```json
{
  "schemaVersion": 1,
  "title": "Calibration of instrument 4",
  "description": "Optional prose.",

  "designCsv": "design.csv",
  "observationsCsv": "observations.csv",

  "regularization": {
    "mode": "gcv",
    "minLambda": 1e-6,
    "maxLambda": 1e2,
    "steps": 41,
    "prior":   { "baseline": 0.4, "gain": 2.0 },
    "penalty": { "baseline": 1.0, "gain": 2.0 }
  },

  "uncertainty": {
    "method": "both",
    "confidenceLevel": 0.95,
    "bootstrapSamples": 2000,
    "bootstrapSeed": 7
  },

  "assumptions": ["The response is linear in the baseline and the gain."],
  "limitations": ["Valid only between 10 and 40 degrees."],

  "sources": [
    { "label": "Run book, page 12", "uri": "internal://runbook", "date": "2026-03-01" }
  ],

  "validation": {
    "weightedRmseMax":     { "value": 2.0,    "source": "Acceptance criterion, section 4." },
    "conditionNumberMax":  { "value": 5000.0, "source": "House rule." }
  }
}
```

### `regularization`

`mode` is `explicit` or `gcv`, and the fields for the other mode must **not** be present —
setting `lambda` while asking for `gcv` is an error naming the offending field, not a
guess about which you meant.

| field | mode | rule |
|---|---|---|
| `lambda` | `explicit` | finite, `>= 0`. Zero is a legitimate request: it means no regularization. |
| `minLambda` | `gcv` | finite, `> 0` (the grid is spaced in log space, so zero has no logarithm) |
| `maxLambda` | `gcv` | finite, `> minLambda` |
| `steps` | `gcv` | 3 – 200 |
| `prior` | either | optional; **if present, must name every parameter and no others** |
| `penalty` | either | optional; every value finite and `> 0` |

A half-specified prior is almost always a typo, and silently defaulting the rest would bury
it. So a prior that is present must be complete. Absent means zero; an absent penalty means
one.

A zero penalty is refused: it leaves that parameter unregularized, which is not what a
diagonal penalty means. If that is what you want, say so with `lambda: 0`.

### `uncertainty`

| field | rule |
|---|---|
| `method` | `analytic`, `bootstrap`, or `both` |
| `confidenceLevel` | 0.5 – 0.999 |
| `bootstrapSamples` | 100 – 10,000. Required exactly when the method asks for a bootstrap. |
| `bootstrapSeed` | any unsigned 64-bit integer. **Required** when the method asks for a bootstrap — an unseeded bootstrap would make the bundle irreproducible. |

Setting bootstrap fields when the method does not ask for a bootstrap is an error, not a
no-op.

### `assumptions` and `limitations`

Both are required and both must be non-empty. A result with no declared assumptions is not
a reviewable result, and Ordinata will not pretend otherwise. 1 – 64 entries each, at most
2,000 bytes per entry.

### `sources`

Recorded and printed. **Never fetched.** `date` is `YYYY-MM-DD`.

### `validation`

Your thresholds, reported as *your* checks. Ordinata does not invent any of its own and does
not endorse yours. `conditionNumberMax` must be at least 1, because no matrix has a condition
number below 1.

## Input: `design.csv`

```csv
observation_id,baseline,drift,gain
run-00,1,0.12,0.10
run-01,1,2.34,0.55
```

First column is exactly `observation_id`. The rest are 1 – 256 parameter columns, whose names
become the parameter names. UTF-8, no BOM, unquoted.

## Input: `observations.csv`

```csv
observation_id,value,standard_error
run-00,0.68,0.021
run-01,1.79,0.0255
```

The header is exactly `observation_id,value,standard_error`. `standard_error` must be finite
and **strictly greater than zero**: a zero or negative standard error is not a small
uncertainty, it is a statement that the observation is infinitely informative, and it would
put an infinity into the weight matrix.

## The join

The two files must name **exactly the same set of observation IDs, one to one**. Rows are
joined by ID, never by position: a file whose rows are in a different order is not a different
problem. A missing or extra ID on either side is an error that names the offenders.

2 – 50,000 matched rows.

## Identifiers

Observation IDs and parameter names: `[A-Za-z0-9_][A-Za-z0-9_.-]*`, at most 64 bytes, ASCII
only. See [SECURITY.md](SECURITY.md) for why the set is this narrow — in short, an identifier
that cannot start with `=` and cannot contain `<` is structurally incapable of becoming a
spreadsheet formula or an HTML tag.

## Numbers

Every numeric cell must be a **finite decimal**. `NaN`, `inf`, `-inf`, and anything that
overflows to an infinity (`1e999`) are refused by name and position.

Note that a cell can be finite and the problem still not be representable: `1e300` divided by
a standard error of `1e-300` is an infinity. That is caught too, as a `non_finite_scaled`
refusal, before the decomposition.

---

# The bundle

```
manifest.json            provenance, configuration, every rule and threshold, every hash
inputs/project.json      the normalized project — itself a valid project that re-solves
inputs/design.csv        the design, in canonical order
inputs/observations.csv  the observations, in canonical order
parameters.csv           estimates, both intervals, prior, penalty
fit.csv                  observed, predicted, residual, standardized residual
diagnostics.json         rank, singular values, lambda, the whole GCV grid, warnings
validation.json          your declared checks and whether they passed
report.html              a static, escaped, offline summary
SHA256SUMS.txt           every member above, hashed
```

The bundle carries its own **normalized inputs**, so it is self-contained: the numbers can be
traced back to the exact matrix they came from without the original files, and
`inputs/project.json` is itself a valid schema-1 project that re-solves to the same answer.

## `parameters.csv`

```csv
name,estimate,standard_error,interval_lower,interval_upper,interval_source,analytic_lower,analytic_upper,bootstrap_lower,bootstrap_upper,prior,penalty
```

`interval_lower`/`interval_upper` carry a **primary** interval for a reader with one column of
space; `interval_source` says which method it came from (`analytic`, `bootstrap`, or `none`).

`analytic_*` and `bootstrap_*` carry **each method that was emitted, in columns of its own**.
Asking for `both` gets both. Nothing is dropped to make room for the primary pair.

An empty cell means **refused or not requested** — never zero. See [MODEL.md](MODEL.md) for
when Ordinata declines to publish an interval at all, and why a near-zero interval for an
unconstrained parameter is the failure this product exists to prevent.

## `fit.csv`

```csv
observation_id,observed,prediction,residual,standardized_residual,standard_error
```

`standardized_residual` is empty where the fit passes exactly through the point and the
quantity does not exist.

## `diagnostics.json`

Dimensions, rank, rank tolerance, every singular value, the condition number (`null` when the
design is not identifiable), the lambda mode and value, **every GCV candidate with its score
or an explicit `undefined`**, weighted residual statistics, effective and residual degrees of
freedom, both uncertainty blocks with their statuses and reasons, and every warning.

## `validation.json`

Each declared check, its threshold and the source you gave for it, the observed value, and
`pass` / `fail` / `not_applicable`. Failures are never hidden and the overall status carries
them up.

## `manifest.json`

Everything above, plus: the tool and model versions, the build target, `observedAt`, the
`synthetic` flag, the SHA-256 of each **original input as supplied**, the SHA-256 and byte
count of **every payload member**, and `resultContent.sha256`.

### `resultContent.sha256`

The hash of the numerical content. It covers exactly the seven members that carry numbers, and
deliberately **excludes** `report.html` and the manifest itself, both of which embed the
wall-clock observation time.

Two runs over identical inputs with identical configuration and seed therefore produce an
identical result content hash, even though they were made at different moments and their
archives differ byte for byte. **The content is reproducible; the artifact records when it was
made.** Pin `--observed-at` and even the archive becomes a function of your inputs alone.

It is a hash of hashes — a sorted list of `(path, sha256)` over the content members, plus the
model and schema versions — rather than a re-serialization of the numbers. That is exactly as
strong (a changed number changes a member hash, which changes this), it cannot drift out of
step with the bytes on disk, and it gives the verifier nothing to reparse and therefore
nothing to reparse wrongly.

## `SHA256SUMS.txt`

Standard `sha256sum` format: 64 lowercase hex characters, two spaces, the path. Every member
except itself, ascending by path. Paths are forward-slash relative over `[A-Za-z0-9._-]`, and
anything else is refused before it is opened.
