# Validation

What has actually been checked, how, and what a passing suite does and does not prove.

## The principle

A test whose expected value came from running Ordinata and pasting the output proves only
that Ordinata is self-consistent — which it would be even if the solver were wrong.

So the reference cases are **independently derived**. The small ones are solved by hand from
the normal equations using Cramer's rule, and the algebra is written out in each case's
`PROVENANCE.md`. That is a completely different route to the answer than the singular value
decomposition the solver actually takes, which never forms `A'WA` at all. If the two disagree,
one of them is wrong, and the test does not assume which.

## Reference cases

Each lives in `fixtures/` with its derivation.

| case | what it pins down |
|---|---|
| `exact-recovery` | a noiseless system is recovered exactly, and the analytic covariance is the textbook `(A'A)^-1` |
| `weighted-heteroscedastic` | unequal standard errors are weighted correctly, against a hand solution |
| `ridge-prior-penalty` | Tikhonov around a **nonzero** prior with a **non-uniform** diagonal penalty, against a hand solution |
| `rank-deficient` | a duplicated column is seen as one, and uncertainty is refused rather than manufactured |
| `ill-conditioned` | GCV-selected regularization beats no regularization on **held-out truth** — for this instance |

Two of these deserve a note on how they are made to be able to fail.

**The weighted case** also asserts that the estimate is *not* the unweighted answer. A solver
that silently ignored the standard errors would otherwise pass the first assertion by
coincidence; this is what stops it.

**The ridge case** also asserts that the estimates land strictly *between* the data and the
prior, and that the harder-penalized parameter has moved a larger fraction of the way. A
solver that shrank toward zero instead of toward the prior would move the estimates in the
same direction, so direction alone proves nothing — but it would overshoot past the prior.

The `ill-conditioned` case is generated in the test rather than stored, because the whole point
of it is the held-out truth, and a fixture file carrying the truth alongside the observations
would be a fixture that lies about what a project can know. Its claim is deliberately narrow:
**on this instance, with this noise and this grid**, regularization beat no regularization.
That is not a claim that regularization always helps, and Ordinata never makes one.

## Properties

Asserted for arbitrary generated inputs, not just the cases we thought of.

- **Row permutation invariance**, and **parameter permutation and renaming**. These are
  asserted **bit for bit**, not to a tolerance. That is not bravado; it is a consequence of the
  design. Ordinata sorts rows by observation ID and columns by parameter name before it does
  any arithmetic, so a permuted input produces a numerically *identical* matrix. Permutation
  invariance is a structural property of the normalization, and testing it to a tolerance
  would be testing something weaker than what is true.
- **Scale consistency.** Multiplying every standard error by `k` leaves the estimate where it
  was and multiplies every standard error by exactly `k`. Both halves are checked, because a
  solver that dropped the weights entirely would pass the first half trivially.
- **Repeat-run hash equality.** Identical inputs, configuration, and seed give an identical
  result content hash.
- **The clock cannot reach the numbers.** Two bundles built at different observation times
  agree on every member the content hash covers, and disagree only in the manifest and the
  report.
- **No panics.** Generated bounded matrices and configurations produce either an answer or a
  *typed* refusal. An unwrap, an index out of bounds, or an arithmetic overflow anywhere in the
  numerical core would surface as a panic and be shrunk to a minimal case.

## Refusals

Every refusal in [LIMITS.md](LIMITS.md) and [SECURITY.md](SECURITY.md) has a test. The ones
worth calling out, because each is a bug that was actually found and fixed:

**An underdetermined design has no condition number.** Two observations, three parameters. The
thin decomposition returns two singular values, both well above tolerance, and the rank is 2.
Comparing the rank to *that* said "full rank" and published a finite condition number — one
that sailed through a declared `conditionNumberMax` — for a system that determines nothing.
The rank is now compared to the parameter count. The regression asserts the condition number
is absent, the declared limit **fails**, the overall validation status is `fail`, and both
uncertainties are refused.

**Provenance equals computation.** The loader used to hash each input and then *reopen* it to
parse it. A file that changed between the two reads produced a bundle recording the checksum
of one input beside the answer to another — silently, because both reads succeeded, and
undetectably, because the checksum it recorded was a real checksum of a real file. Inputs are
now read once into bounded memory and those exact bytes are both hashed and parsed.

The regression runs the loader in a loop while a writer thread flips the file between two
contents, and asserts the one thing that must survive: whichever content's hash was recorded,
the numbers must be the numbers *that* content produces. **This test was verified to fail** —
the two-read pattern was deliberately reintroduced, and it failed within a few hundred
iterations with exactly the intended message. A regression test that cannot fail proves
nothing, so it was made to.

**Budgets refuse before they spend.** The bootstrap work budget is checked at load, before the
matrix is built and before a single resample is drawn. The refusal names the largest sample
count you may ask for instead, and a test asserts that number is one Ordinata will actually
accept — advice below the schema's own floor of 100 would be advice a user cannot take.

**Duplicate JSON keys.** Refused at every trust boundary, at any depth, with the key named. The
test also asserts what `serde_json` would have done instead, so the thing being prevented is on
the record.

## The tamper ladder

Written as a ladder, because each rung is what a forger has to fix after the rung below caught
them, and each is a *different* check with a *different* name.

1. **Edit a number** → its member fails its checksum.
2. **Fix `SHA256SUMS.txt` too** → the manifest's own hash of that member disagrees.
3. **Fix the manifest as well** → the result content hash disagrees.
4. **Fix everything** → you have re-derived the bundle. That is not forgery, and `verify` has
   never claimed to detect it: **it is not a signature.**

Each rung asserts not only that the expected check failed but that the *shallower* ones now
pass — otherwise the test would not be exercising the layer it names.

Also tested: a missing member (each in turn, caught by name), an extra file smuggled in, a
manifest that quietly drops a member from its payload, a manifest that describes one twice, a
manifest with a duplicate key, a checksum line pointing outside the bundle, and — the deep one
— **every member the content hash claims to cover is changed in turn, with all the shallower
layers repaired, to prove the hash really does depend on it.** A member listed as covered but
not actually hashed would make the deepest layer quietly weaker than it claims.

## Archives

A zipped bundle verifies, with or without a wrapping directory. And:

- an archive whose common prefix is `..` is **not** stripped into innocence;
- two entry names that differ only in case are refused as undecidable;
- three entries each declaring 250 MiB — individually legal, jointly a bomb — are refused
  **before any of them is decompressed**;
- an archive over the size cap is refused **without being read whole**;
- a corrupted entry fails its own CRC-32; an encrypted or unsupported-method entry is refused
  by name.

## Reparse points

A **junction** inside a bundle directory is refused. This is the realistic version of the
attack on Windows, because unlike a symbolic link it needs no privilege at all — any user can
create one. The test creates a real junction and asserts the refusal names what it found. The
symbolic-link case is covered too, and skips loudly rather than silently when the environment
cannot create one.

## The command line

Run as a real process, because the exit code, the stream split, and the `--json` shape are a
published contract that cannot be checked by calling into the library.

Exit codes: `0` success, `2` input, `3` numerical, `4` path/IO, `5` bundle integrity, `10`
internal invariant. Every one is asserted against a real failure of that kind.

**Standard output carries the `--json` object and nothing else** — asserted by parsing stdout
as exactly one JSON value, on success *and* on failure. Without `--json`, stdout is completely
empty.

Also asserted end to end: `solve` never overwrites an existing bundle and leaves it untouched;
`solve` refuses to write a bundle that would enclose its own inputs; a bad `--observed-at` is
refused before anything is written; two runs at the same pinned instant produce byte-identical
members; `demo` shouts that it is synthetic in the terminal, the manifest, and the report; and
a tampered bundle exits 5 with `verification_failed` in the envelope, so a caller reading only
`status` cannot mistake it for a pass.

## The artifact

The release binary is scanned for anything that identifies the machine that built it. The
scanner is tested against a known-bad buffer (it must fire), a correctly remapped buffer (it
must not), and a chance `C:\` byte sequence inside machine code (it must not — an early version
did, and would have failed every release for a coincidence).

**This gate was verified to fail**: rebuilding without the remapping produced 58 findings across
every marker class. See [RELEASING.md](RELEASING.md).

## What the suite does not prove

- **Not that a linear model is right for your data.** Ordinata fits what you declared.
- **Not that your standard errors are correct.** It reports the reduced chi-square so you can
  see whether they look it.
- **Not that regularization improves your problem.** The one case where it demonstrably does is
  labelled as being about that one case.
- **Not bit-identical results across different machines or builds.** Floating point is
  reproducible for a given binary on a given target, and that is the only claim made.
- **Not that a verifying bundle is authentic.** It is not a signature.
