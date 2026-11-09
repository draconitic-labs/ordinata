# Security

## The threat model

Ordinata is a local program that reads files a person points it at, and reads bundles
that may have come from somebody else. Those are the two trust boundaries, and they are
different.

**Your own project files** are hostile in the way that data is hostile: a spreadsheet
export can contain a formula, a CSV can contain a byte sequence that is not UTF-8, a
column can be duplicated, a path can point somewhere you did not mean. Ordinata's job is
to refuse those clearly rather than to do something surprising with them.

**A bundle from elsewhere** is hostile in the way that an attachment is hostile. Verifying
it must not let it write anywhere, read anywhere, or consume the machine.

What Ordinata is **not** defending against: a hostile process already running as you, a
compromised toolchain, or a bundle from an attacker who also has your machine. It is also
not a sandbox for arbitrary code, because it never runs any.

## What Ordinata does not do, ever

- **No network.** Not at startup, not during a solve, not during verification. There is no
  code path that opens a socket. Citation URIs in a project are recorded as text and never
  fetched; the HTML report renders them as plain text, never as links.
- **No subprocess.** Ordinata never spawns a program.
- **No plugins, no scripting, no evaluation.** There is no configuration value that becomes
  code.
- **No environment variables that change a number.** Nothing about the numerical result
  depends on the environment.
- **`#![forbid(unsafe_code)]`** in both crates. There is no `unsafe` block in Ordinata.
- **No secrets.** Ordinata has no credentials, no config file with a token, and nothing to
  put in one.

## Injection is prevented by the grammar, not by escaping

Observation IDs and parameter names are held to `[A-Za-z0-9_][A-Za-z0-9_.-]*`, at most 64
bytes.

That is not fussiness. An identifier that **cannot begin** with `=`, `+`, `-`, or `@`, and
**cannot contain** `<`, `>`, `&`, a quote, a comma, or a newline, is structurally incapable
of becoming a spreadsheet formula, an HTML tag, or a CSV quoting ambiguity. The defence is
in what the grammar admits, not in a downstream escaping pass that someone might one day
forget to apply. `=cmd|' /C calc'!A0` is not a parameter name Ordinata will accept, so no
Ordinata CSV can ever contain one.

Non-ASCII identifiers are refused too, so two parameters whose names look identical but
differ in code points cannot silently collide in a result table.

**Free text** — titles, assumptions, limitations, citation labels — is a different matter,
because a limitation genuinely might need to say `x < 3`. Free text is accepted broadly,
bounded in length, stripped of control characters, and HTML-escaped at every point where it
is rendered.

## Paths

A path declared inside `project.json` is a forward-slash relative path. Ordinata refuses,
by text: absolute paths, drive letters, drive-relative paths (`C:design.csv`), backslashes,
`.` and `..` components, empty components, Windows reserved device names (`NUL`, `CON`,
`COM1`…), control characters, components ending in a dot or a space, and paths longer than
4,096 bytes or 32 components.

Then — and this is the part the text check cannot do — the path is **canonicalized**, and
the result is required to sit beneath the canonicalized project directory. A symbolic link
or a Windows junction can point outside the project without any `..` appearing anywhere in
the text. Checking the string is cheap and catches the obvious; asking the operating system
where the path really lands is authoritative and catches the rest.

**Output** must not exist, must have an existing parent, must not be a reserved device, and
must not overlap any input. An output that contained an input would let the next run quietly
consume its own bundle; an output that *was* an input would destroy it.

Writing is atomic: members go into a staging directory beside the target and are moved into
place with a single rename once every one has been written and flushed. A run that is killed
halfway leaves a staging directory, never a bundle that looks complete and is not. **Ordinata
never overwrites an existing bundle**, and a leftover staging directory is reported rather
than deleted — Ordinata does not remove directories it did not certainly create.

### Residual risk

Between canonicalizing a path and opening it there is a window in which the path could be
swapped for a link. Closing it entirely needs platform-specific handle-based reopening, which
v0.1 does not do. The window requires an attacker who can already write into your project
directory while you are running Ordinata; if they can do that, they can simply edit the CSV.
It is recorded here because it is real, not because it is the weakest link.

## Reading a bundle you did not make

`verify` accepts a directory or a ZIP archive, and it treats both as hostile.

**Directories.** Every entry is checked with `symlink_metadata`, which does not follow links.
Any **reparse point** — a symbolic link, a Windows junction, or any tag that does not exist
yet — is a refusal, checked *before* the entry is treated as a directory. This matters
specifically because a junction is not a symbolic link: on Windows it needs no privilege at
all, any user can create one, and a verifier that recursed into one would walk straight out
of the bundle and hash whatever it found on the other side.

**Archives.** Ordinata has its own small ZIP reader rather than a dependency, because the
attack surface of a ZIP reader *is* the point and it should be small enough to read in one
sitting. It reads only `store` and `deflate`; every other method is refused by name. No
encryption, no ZIP64, no multi-disk, no self-extracting prefix. The central directory is
authoritative, so a local header that disagrees with it cannot smuggle in a second
interpretation of the same file. Every entry's CRC-32 is checked against the value the
archive itself declares.

Every entry name is checked against the bundle-member grammar — `[A-Za-z0-9._-]`,
forward-slash relative, no `..`, no reserved devices, at most 8 components — **before**
anything is opened. An entry called `../../.ssh/authorized_keys` is a refusal, not a write.

Three subtleties, each of which the obvious implementation gets wrong:

- **A common top-level directory is stripped**, because zipping a bundle directory is the
  natural thing to do and produces `my-bundle/manifest.json`. But the prefix is held to the
  same rule as any other component. An archive whose every entry is `../manifest.json` has
  the perfectly good common prefix `..`; stripping it without looking would turn a set of
  traversing names into a set of innocent ones and wave them straight past the check that
  exists to catch them.
- **Two names that differ only in case are refused.** `manifest.json` and `Manifest.json`
  are two entries in a ZIP and one file on Windows. An archive holding both means different
  things on different machines, and a bundle does not get to do that.
- **The declared sizes are summed before anything is decompressed.** See
  [LIMITS.md](LIMITS.md).

## Duplicate JSON keys

JSON's grammar permits `{"lambda": 0.0, "lambda": 1e9}`, and every mainstream parser,
`serde_json` included, quietly keeps the last one.

That is a fine default for a config file and a disqualifying one for evidence: two readers of
the same bytes can disagree about what they say, which is exactly the property an evidence
bundle exists to deny. A forger can leave an honest-looking value on the line a human reads
and a different one on the line the parser takes.

Every JSON document that reaches Ordinata from outside — a `project.json`, a `manifest.json`
inside somebody's bundle — is parsed by `canonical::parse_strict`, which **refuses a repeated
key at any depth** and names the key it found. Trailing content after the first value is
refused too: a file that is two documents in a trench coat is not one Ordinata will guess
about.

## What a passing verification proves, and what it does not

It proves the bundle is **internally consistent and unchanged**: every member hashes to what
`SHA256SUMS.txt` records, the manifest describes exactly the members that are present and
describes each of them once, and the numbers hash to the result content hash the manifest
carries.

**It is not a signature.** Anyone with a copy of Ordinata can produce a bundle that verifies.
Verification tells you a bundle is intact; it does not tell you who made it, or that they were
honest. Signing is out of scope for v0.1, and pretending otherwise would be worse than not
having it. The CLI says so, in those words, on every successful verification.

**It does not re-run the solver.** This is a choice, not an omission. Floating-point results
are only bit-reproducible for a given binary on a given target, so a verifier that recomputed
the fit would report "tampering" whenever a colleague checked a bundle on a different machine
— which is exactly the wrong answer to give. Verification checks integrity; reproducibility is
claimed only where it actually holds.

## The build

The release binary is built through `packaging/build-release.ps1`, which sets
`--remap-path-prefix` so that the paths rustc embeds in panic sites name no user, no drive,
and no directory that exists on any machine. `crates/ordinata-cli/tests/artifact.rs` is the
gate: it scans every file in the distribution for anything that identifies the machine that
built it, and the release script cannot skip it.

The dependency budget is small and every crate in it is named in `PRODUCT-SPEC.md`. `cargo
audit` runs as a release gate; `cargo about` fails the build on a licence that is not on the
accepted list.

## Reporting

`support@draconitic.com`.
