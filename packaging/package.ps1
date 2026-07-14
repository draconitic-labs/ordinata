<#
.SYNOPSIS
    Run every release gate, build the distribution, and prove the result works outside
    the source tree.

.DESCRIPTION
    This is the whole release, in order, and it stops at the first thing that is wrong.
    Nothing here is advisory: a gate that fails ends the run.

        1  format            cargo fmt --check
        2  lint              cargo clippy --all-targets -- -D warnings
        3  tests             cargo test --workspace --locked
        4  advisories        cargo audit          (network; see -SkipAudit)
        5  licences          cargo about          (network; see -SkipNotices)
        6  build             packaging/build-release.ps1, with path remapping
        7  sbom              cargo cyclonedx
        8  clean-room smoke  demo / solve / verify, run from a scratch directory with
                             the binary copied out of the tree, against inputs it has
                             never seen, plus a tamper that must be caught
        9  artifact scan     every file in dist/ carries no local or personal path
       10  checksums         SHA256SUMS.txt over the distribution

    The clean-room smoke is the step that would catch a binary which only works where
    it was built: it runs the *copied* executable, from a *different* working directory,
    on inputs *outside* the repository, and it verifies a bundle it made itself and then
    fails to verify one it has quietly corrupted. A release that passes 1-7 and fails 8
    is a release that was never going to work on a customer's machine.

.PARAMETER SkipAudit
    Skip the advisory scan. It needs to fetch the RustSec database.

.PARAMETER SkipNotices
    Skip licence-notice generation. It needs to fetch licence texts.
#>
[CmdletBinding()]
param(
    [switch]$SkipAudit,
    [switch]$SkipNotices
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dist = Join-Path $root 'dist'

# The target triple is asked for rather than assumed. A distribution named for a triple
# it was not built for is worse than one with no triple in the name at all: the binary
# in it links against a different runtime than the name promises, and the person who
# finds that out is the customer.
$triple = (& rustc -vV | Select-String '^host: ').Line -replace '^host:\s*', ''
if (-not $triple) { throw "cannot determine the host target triple from rustc" }
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$stage = Join-Path $dist "ordinata-$version-$triple"

# The auxiliary cargo tools -- audit, about, cyclonedx -- are separately compiled
# binaries and may have been built against a different Windows runtime than the toolchain
# that is active here. When that happens they fail to *start*, with STATUS_DLL_NOT_FOUND
# (0xC0000135), which looks like a gate failure and is not one.
#
# Every rustup toolchain ships the runtime it was built with. Appending those directories
# to PATH lets a tool find the DLLs it needs. They are *appended*, not prepended, so that
# the `rustc` and `cargo` already on PATH keep winning the executable lookup while the DLL
# search -- which walks all of PATH -- still reaches them.
$toolchains = Join-Path $env:USERPROFILE '.rustup\toolchains'
if (Test-Path $toolchains) {
    $runtimeDirs = Get-ChildItem $toolchains -Directory |
        ForEach-Object { Join-Path $_.FullName 'bin' } |
        Where-Object { Test-Path $_ }
    if ($runtimeDirs) {
        $env:PATH = $env:PATH + ';' + ($runtimeDirs -join ';')
    }
}

# Cargo writes progress to stderr, and under `$ErrorActionPreference = 'Stop'` PowerShell
# turns any stderr from a native program into a terminating error -- even when it exited
# 0. So gates run with the preference relaxed and are judged on their exit code, which is
# the thing that actually says whether they passed.
$step = 0
function Gate([string]$name, [scriptblock]$body) {
    $script:step++
    Write-Host ""
    Write-Host "-- $script:step. $name --" -ForegroundColor Cyan

    $global:LASTEXITCODE = 0
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $body
    } finally {
        $ErrorActionPreference = $previous
    }
    if ($LASTEXITCODE -ne 0) {
        throw "GATE FAILED: $name (exit $LASTEXITCODE)"
    }
    Write-Host "   ok" -ForegroundColor Green
}

Push-Location $root
try {
    Gate 'format' { cargo fmt --all -- --check }
    Gate 'lint' { cargo clippy --workspace --all-targets --locked -- -D warnings }
    Gate 'tests' { cargo test --workspace --locked }

    if ($SkipAudit) {
        Write-Host ""
        Write-Host "── SKIPPED: advisory scan (-SkipAudit). This is a release gate; a release" -ForegroundColor Yellow
        Write-Host "   that skipped it has not been checked against the RustSec database." -ForegroundColor Yellow
    } else {
        Gate 'advisories' { cargo audit }
    }

    # ---- Build, with the path remapping that keeps the machine out of the binary ----
    Gate 'release build + artifact scan' {
        & (Join-Path $PSScriptRoot 'build-release.ps1')
        if ($LASTEXITCODE -ne 0) { throw "the release build failed" }
    }

    # ---- Stage the distribution -------------------------------------------------
    Write-Host ""
    Write-Host "── Staging $stage" -ForegroundColor Cyan
    if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
    New-Item -ItemType Directory -Force $stage | Out-Null
    New-Item -ItemType Directory -Force (Join-Path $stage 'docs') | Out-Null
    New-Item -ItemType Directory -Force (Join-Path $stage 'fixtures') | Out-Null

    Copy-Item (Join-Path $root 'target\release\ordinata.exe') $stage
    Copy-Item (Join-Path $root 'README.md') $stage
    Copy-Item (Join-Path $root 'PRODUCT-SPEC.md') $stage
    Copy-Item (Join-Path $root 'LICENSE') $stage
    Copy-Item (Join-Path $root 'docs\*.md') (Join-Path $stage 'docs')
    Copy-Item -Recurse (Join-Path $root 'fixtures\*') (Join-Path $stage 'fixtures')

    # ---- Licence notices --------------------------------------------------------
    if ($SkipNotices) {
        Write-Host ""
        Write-Host "── SKIPPED: licence notices (-SkipNotices). A distribution without them" -ForegroundColor Yellow
        Write-Host "   does not carry the attribution its dependencies require." -ForegroundColor Yellow
    } else {
        Gate 'licence notices' {
            cargo about generate --config (Join-Path $PSScriptRoot 'about.toml') `
                (Join-Path $PSScriptRoot 'about.hbs') `
                -o (Join-Path $stage 'THIRD-PARTY-NOTICES.html')
        }
    }

    # ---- SBOM -------------------------------------------------------------------
    Gate 'sbom' {
        cargo cyclonedx --format json --all
        if ($LASTEXITCODE -ne 0) { throw "cargo cyclonedx failed" }
    }
    $sbom = Get-ChildItem -Path $root -Filter '*.cdx.json' -Recurse -File |
        Where-Object { $_.FullName -notlike "*\target\*" -and $_.FullName -notlike "*\dist\*" }
    if (-not $sbom) { throw "cargo cyclonedx produced no SBOM" }
    foreach ($file in $sbom) {
        Copy-Item $file.FullName (Join-Path $stage $file.Name)
        # Keep the tree clean: the SBOM belongs in the distribution, not in the source.
        Remove-Item $file.FullName
    }
    # It has to be valid JSON, or it is a file rather than an SBOM.
    foreach ($file in Get-ChildItem (Join-Path $stage '*.cdx.json')) {
        $null = Get-Content $file.FullName -Raw | ConvertFrom-Json
        Write-Host "   sbom  $($file.Name) parses as JSON" -ForegroundColor Green
    }

    # ---- Clean-room smoke -------------------------------------------------------
    #
    # From here on nothing touches the source tree. The binary is the *copied* one, the
    # working directory is a scratch directory, and the inputs are written fresh.
    Write-Host ""
    Write-Host "── Clean-room smoke (outside the source tree)" -ForegroundColor Cyan

    $room = Join-Path ([System.IO.Path]::GetTempPath()) ("ordinata-cleanroom-" + [System.Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force $room | Out-Null
    $exe = Join-Path $stage 'ordinata.exe'

    # Ordinata writes *every* human-facing word to standard error, success included --
    # that is the contract that lets `--json` on stdout be piped straight into a parser.
    # PowerShell, under `Stop`, treats any stderr from a native program as a terminating
    # error, which would make a *correct* Ordinata run look like a crash. So the smoke
    # runs with the preference relaxed and every step is judged on its exit code, which
    # is the thing the contract actually specifies.
    $smokePreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'

    try {
        Push-Location $room

        & $exe --version
        if ($LASTEXITCODE -ne 0) { throw "smoke: --version failed" }

        # demo
        & $exe demo --out (Join-Path $room 'demo') --observed-at '2026-01-01T00:00:00Z'
        if ($LASTEXITCODE -ne 0) { throw "smoke: demo failed" }
        & $exe verify (Join-Path $room 'demo') | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "smoke: the demo bundle did not verify" }
        Write-Host "   demo   written and verified" -ForegroundColor Green

        # solve, on inputs the binary has never seen, in a directory it has never been
        $project = Join-Path $room 'project'
        New-Item -ItemType Directory -Force $project | Out-Null
        Set-Content -Path (Join-Path $project 'design.csv') -Encoding ascii -Value @(
            'observation_id,intercept,slope', 's1,1,0', 's2,1,1', 's3,1,2', 's4,1,3', 's5,1,4')
        Set-Content -Path (Join-Path $project 'observations.csv') -Encoding ascii -Value @(
            'observation_id,value,standard_error', 's1,1.02,0.05', 's2,2.05,0.05', 's3,2.98,0.05', 's4,4.01,0.05', 's5,5.03,0.05')
        Set-Content -Path (Join-Path $project 'project.json') -Encoding ascii -Value @'
{
  "schemaVersion": 1,
  "title": "Clean-room smoke",
  "designCsv": "design.csv",
  "observationsCsv": "observations.csv",
  "regularization": { "mode": "gcv", "minLambda": 1e-6, "maxLambda": 10.0, "steps": 21 },
  "uncertainty": { "method": "both", "confidenceLevel": 0.95, "bootstrapSamples": 500, "bootstrapSeed": 1 },
  "assumptions": ["The response is linear in an intercept and a slope."],
  "limitations": ["Synthetic smoke-test data. It measures nothing."]
}
'@

        & $exe validate (Join-Path $project 'project.json') | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "smoke: validate failed" }

        $bundle = Join-Path $room 'bundle'
        $solved = & $exe solve (Join-Path $project 'project.json') --out $bundle --observed-at '2026-01-01T00:00:00Z' --json | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw "smoke: solve failed" }
        Write-Host "   solve  content hash $($solved.result.resultContentHash)" -ForegroundColor Green

        & $exe verify $bundle | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "smoke: the solved bundle did not verify" }
        Write-Host "   verify passed on an intact bundle" -ForegroundColor Green

        # Both intervals really are in the file the customer opens.
        $header = (Get-Content (Join-Path $bundle 'parameters.csv') -TotalCount 1)
        foreach ($column in @('analytic_lower', 'analytic_upper', 'bootstrap_lower', 'bootstrap_upper')) {
            if ($header -notmatch [regex]::Escape($column)) {
                throw "smoke: parameters.csv is missing the $column column"
            }
        }
        Write-Host "   both analytic and bootstrap intervals present in parameters.csv" -ForegroundColor Green

        # Tamper: one byte, and the verifier must catch it and exit 5.
        $fit = Join-Path $bundle 'fit.csv'
        (Get-Content $fit -Raw).Replace('1.02', '9.99') | Set-Content $fit -NoNewline -Encoding ascii
        & $exe verify $bundle 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 5) {
            throw "smoke: a tampered bundle exited $LASTEXITCODE, and the contract says 5"
        }
        Write-Host "   verify caught a one-byte tamper and exited 5" -ForegroundColor Green

        # And it never overwrites.
        & $exe solve (Join-Path $project 'project.json') --out $bundle 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 4) {
            throw "smoke: solving over an existing bundle exited $LASTEXITCODE, and the contract says 4"
        }
        Write-Host "   solve refused to overwrite an existing bundle and exited 4" -ForegroundColor Green
    } finally {
        Pop-Location
        $ErrorActionPreference = $smokePreference
        Remove-Item -Recurse -Force $room -ErrorAction SilentlyContinue
    }

    # ---- Scan every file that is about to ship ----------------------------------
    Gate 'artifact scan over the whole distribution' {
        $env:ORDINATA_SCAN_DIR = $stage
        $env:ORDINATA_SCAN_REQUIRED = '1'
        try {
            cargo test --locked -p ordinata-cli --test artifact -- --nocapture
        } finally {
            Remove-Item Env:\ORDINATA_SCAN_DIR -ErrorAction SilentlyContinue
            Remove-Item Env:\ORDINATA_SCAN_REQUIRED -ErrorAction SilentlyContinue
        }
    }

    # ---- Checksums and archive --------------------------------------------------
    Write-Host ""
    Write-Host "── Checksums and archive" -ForegroundColor Cyan
    $lines = Get-ChildItem -Path $stage -Recurse -File | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
        "$((Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLower())  $relative"
    }
    Set-Content -Path (Join-Path $stage 'SHA256SUMS.txt') -Value $lines -Encoding ascii

    $zip = "$stage.zip"
    if (Test-Path $zip) { Remove-Item $zip }
    Compress-Archive -Path $stage -DestinationPath $zip
    $zipHash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    Set-Content -Path "$zip.sha256" -Value "$zipHash  $(Split-Path -Leaf $zip)" -Encoding ascii

    Write-Host ""
    Write-Host "RELEASE CANDIDATE BUILT" -ForegroundColor Green
    Write-Host "  $zip"
    Write-Host "  sha256  $zipHash"
    Write-Host "  members $((Get-ChildItem -Path $stage -Recurse -File).Count)"
    Write-Host ""
    Write-Host "This is a private review artifact. It is not published, and billing is not active."
} finally {
    Pop-Location
}
