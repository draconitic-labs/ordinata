# Privacy

## The short version

Ordinata does not talk to anyone. It has no network code, no telemetry, no account, no
licence check, no update check, and no crash reporter. It reads the files you point it at
and writes the one directory you name.

There is nothing to opt out of, because there is nothing switched on.

## What Ordinata reads

Exactly three files, all named by your `project.json`:

- `project.json` itself,
- the design CSV it declares,
- the observations CSV it declares.

Both CSVs must resolve **beneath the project directory**. Ordinata will not follow a path
out of it, by traversal or by link. It does not read your environment for anything that
affects a result, does not look in your home directory, does not have a config file, and
does not have a cache.

`ordinata demo` reads nothing at all: its data is generated in memory.

## What Ordinata writes

One directory, the one you passed to `--out`. It must not already exist. Members are
written to a staging directory beside it and moved into place with a single rename.

Nothing else. No dotfile, no cache, no log, no temporary file outside that staging
directory, no registry key.

## What is inside a bundle

A bundle is meant to be given to somebody, so it is worth knowing exactly what it carries.

**Your data.** `inputs/design.csv` and `inputs/observations.csv` are your inputs in
canonical row and column order. `inputs/project.json` is your project, normalized. If your
observations are sensitive, **the bundle contains them**, and giving somebody the bundle
gives them your data. That is the point of a self-contained evidence bundle, and it is the
thing to think about before sending one.

**Your words.** The title, description, assumptions, limitations, and citation labels are
reproduced in the manifest and the report exactly as you wrote them.

**Hashes.** SHA-256 of each input, as supplied, and of every member.

**What the run was.** Tool version, model version, and the platform triple the binary was
built for (for example `x86_64-pc-windows-msvc`).

**When you said it was.** `observedAt`, which defaults to the wall clock at the time of the
run, and which you can set explicitly with `--observed-at`.

## What is not inside a bundle

- No username, no hostname, no machine name, no domain.
- No absolute path. The manifest records each input by the **path as your project declared
  it** — `design.csv` — and never by where it happened to sit on your disk.
- No environment variables.
- No IP address, no MAC address, no serial number, no hardware identifier.
- No process ID. (One appears transiently in the name of the staging directory, so that two
  concurrent runs cannot collide. It is renamed away before the bundle exists and is not
  recorded anywhere.)

The one thing that leaves your machine is the thing you choose to send.

## The timestamp, and how to make it not matter

`observedAt` is a wall-clock time, and a wall-clock time says something about you.

The manifest's `resultContent.sha256` **excludes it**, deliberately. It covers only the
members that carry numbers, so two runs over identical inputs with identical configuration
and seed produce an identical result content hash even though they were made at different
moments and their archives differ byte for byte. The *content* is reproducible; the
*artifact* records when it was made.

If you would rather the artifact did not record when it was made either, pass
`--observed-at` with a value of your choosing. Every byte of the bundle is then a function
of your inputs alone.

## The report

`report.html` is a single self-contained file. No script, no stylesheet link, no image, no
font, no anchor, no remote asset of any kind. Opening it makes no request to anything. It
renders identically on a machine with no network and reveals nothing to anybody on a machine
that has one.

A citation URI you supplied is printed as **text**, never as a link. A project is a document
someone else may have written, and turning a string from it into a clickable target is how a
`javascript:` URI or a tracking pixel would get a foot in the door. Ordinata never fetches a
citation and never invites your browser to either.

## The build and the binary

The release binary carries no path from the machine that built it: no account name, no home
directory, no checkout location. That is enforced by `--remap-path-prefix` at build time and
by an artifact scan that the release script cannot skip. See [SECURITY.md](SECURITY.md).

## Contact

`support@flintglade.com`. Ordinata itself will never contact us on your behalf.
