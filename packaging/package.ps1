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
# rust-toolchain.toml pins the compiler version on every platform. A release also pins the host.
$env:RUSTUP_TOOLCHAIN = '1.96.1-x86_64-pc-windows-gnullvm'
$rustcInfo = & rustc -vV
if ($LASTEXITCODE -ne 0) { throw "rustc -vV failed" }
$triple = ($rustcInfo | Select-String '^host: ').Line -replace '^host:\s*', ''
if (-not $triple) { throw "cannot determine the host target triple from rustc" }
$release = ($rustcInfo | Select-String '^release: ').Line -replace '^release:\s*', ''
$expectedTriple = 'x86_64-pc-windows-gnullvm'
$expectedRelease = '1.96.1'
if ($triple -ne $expectedTriple -or $release -ne $expectedRelease) {
    throw "release packaging requires rustc $expectedRelease for $expectedTriple; found $release for $triple"
}
$sysroot = (& rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0 -or -not $sysroot) { throw "cannot determine the pinned rustc sysroot" }
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

# cargo-cyclonedx correctly identifies local workspace crates as path components, but
# its native identifiers contain the checkout's absolute path. Replace that path with
# a public, stable logical root and normalize relative workspace purls before either the
# privacy scan or the reproducibility comparison sees the files.
function Normalize-Sbom([string]$path, [string]$workspaceRoot, [string]$crateName) {
    $model = [System.IO.File]::ReadAllText($path) | ConvertFrom-Json
    # Both fields are optional in CycloneDX 1.5 and cargo-cyclonedx may populate them
    # from randomness or the wall clock. They describe this build invocation rather
    # than the locked dependency graph, so omit them from the reproducible artifact.
    $model.PSObject.Properties.Remove('serialNumber')
    if ($null -ne $model.metadata) {
        $model.metadata.PSObject.Properties.Remove('timestamp')
    }
    $text = $model | ConvertTo-Json -Depth 100
    $forwardRoot = $workspaceRoot.Replace('\', '/')
    $text = $text.Replace($forwardRoot, '/ordinata')
    $text = $text.Replace('file://..\\ordinata-core', 'file:///ordinata/crates/ordinata-core')
    $text = $text.Replace('file://../ordinata-core', 'file:///ordinata/crates/ordinata-core')
    $text = $text.Replace('file://.', "file:///ordinata/crates/$crateName")
    if ($text.IndexOf($forwardRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        throw "SBOM normalization left the checkout root in $path"
    }
    if ($text -match '(?i)(?:path\+)?file:/+[a-z]:[/\\]') {
        throw "SBOM normalization left an absolute drive file URI in $path"
    }
    [System.IO.File]::WriteAllText($path, "$text`n", [System.Text.UTF8Encoding]::new($false))
}

# Compress-Archive copies source timestamps into the archive. Those timestamps differ
# between clean checkouts, so two byte-identical distributions acquire different ZIPs.
# Write entries in relative-path order with one fixed legal ZIP timestamp instead.
function Write-DeterministicZip([string]$sourceDirectory, [string]$destinationPath) {
    Add-Type -AssemblyName System.IO.Compression
    if (Test-Path $destinationPath) { Remove-Item $destinationPath -Force }

    $rootName = Split-Path -Leaf $sourceDirectory
    $files = Get-ChildItem -Path $sourceDirectory -Recurse -File | ForEach-Object {
        [pscustomobject]@{
            Path = $_.FullName
            Relative = $_.FullName.Substring($sourceDirectory.Length + 1).Replace('\', '/')
        }
    } | Sort-Object Relative

    $stream = [System.IO.File]::Open(
        $destinationPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None)
    $archive = [System.IO.Compression.ZipArchive]::new(
        $stream,
        [System.IO.Compression.ZipArchiveMode]::Create,
        $false)
    try {
        $timestamp = [System.DateTimeOffset]::Parse('2000-01-01T00:00:00Z')
        foreach ($file in $files) {
            $entry = $archive.CreateEntry(
                "$rootName/$($file.Relative)",
                [System.IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = $timestamp
            $input = [System.IO.File]::OpenRead($file.Path)
            $output = $entry.Open()
            try {
                $input.CopyTo($output)
            } finally {
                $output.Dispose()
                $input.Dispose()
            }
        }
    } finally {
        $archive.Dispose()
        $stream.Dispose()
    }
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
    New-Item -ItemType Directory -Force (Join-Path $stage 'runtime-licenses') | Out-Null

    Copy-Item (Join-Path $root 'target\release\ordinata.exe') $stage
    Copy-Item (Join-Path $root 'README.md') $stage
    Copy-Item (Join-Path $root 'PRODUCT-SPEC.md') $stage
    Copy-Item (Join-Path $root 'LICENSE') $stage
    Copy-Item (Join-Path $root 'docs\*.md') (Join-Path $stage 'docs')
    Copy-Item -Recurse (Join-Path $root 'fixtures\*') (Join-Path $stage 'fixtures')

    # The gnullvm executable imports libunwind dynamically. Shipping only the EXE works
    # on this build machine because rustup's bin directory happens to be on PATH, then
    # fails on a customer's clean Windows install. Stage the exact runtime from the
    # pinned compiler sysroot beside the executable, and ship its licence terms.
    $runtime = Join-Path $sysroot 'bin\libunwind.dll'
    $runtimeLicenses = Join-Path $sysroot 'share\doc\rust\licenses'
    $apache = Join-Path $runtimeLicenses 'Apache-2.0.txt'
    $llvmException = Join-Path $runtimeLicenses 'LLVM-exception.txt'
    foreach ($required in @($runtime, $apache, $llvmException)) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "the pinned toolchain is missing required runtime material: $required"
        }
    }
    $exeText = [System.Text.Encoding]::ASCII.GetString(
        [System.IO.File]::ReadAllBytes((Join-Path $stage 'ordinata.exe')))
    if ($exeText.IndexOf('libunwind.dll', [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
        throw "the release executable does not import the libunwind runtime this package accounts for"
    }
    Copy-Item $runtime (Join-Path $stage 'libunwind.dll')
    Copy-Item $apache (Join-Path $stage 'runtime-licenses\Apache-2.0.txt')
    Copy-Item $llvmException (Join-Path $stage 'runtime-licenses\LLVM-exception.txt')
    $runtimeHash = (Get-FileHash -Algorithm SHA256 $runtime).Hash.ToLower()
    $runtimeBytes = (Get-Item $runtime).Length
    Set-Content -Path (Join-Path $stage 'RUNTIME-COMPONENTS.txt') -Encoding ascii -Value @(
        'Ordinata Windows runtime components',
        '',
        'component: libunwind.dll',
        "bytes: $runtimeBytes",
        "sha256: $runtimeHash",
        "source: Rust toolchain $expectedRelease for $expectedTriple",
        'license: Apache-2.0 WITH LLVM-exception',
        'license texts: runtime-licenses/Apache-2.0.txt; runtime-licenses/LLVM-exception.txt'
    )

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
    $previousSourceDateEpoch = $env:SOURCE_DATE_EPOCH
    $env:SOURCE_DATE_EPOCH = '0'
    try {
        Gate 'sbom' {
            cargo cyclonedx --format json --all --spec-version 1.5
            if ($LASTEXITCODE -ne 0) { throw "cargo cyclonedx failed" }
        }
    } finally {
        if ($null -eq $previousSourceDateEpoch) {
            Remove-Item Env:\SOURCE_DATE_EPOCH -ErrorAction SilentlyContinue
        } else {
            $env:SOURCE_DATE_EPOCH = $previousSourceDateEpoch
        }
    }
    $sbom = Get-ChildItem -Path $root -Filter '*.cdx.json' -Recurse -File |
        Where-Object { $_.FullName -notlike "*\target\*" -and $_.FullName -notlike "*\dist\*" }
    if (-not $sbom) { throw "cargo cyclonedx produced no SBOM" }
    foreach ($file in $sbom) {
        $destination = Join-Path $stage $file.Name
        Copy-Item $file.FullName $destination
        Normalize-Sbom $destination $root (Split-Path -Leaf $file.DirectoryName)
        # Keep the tree clean: the SBOM belongs in the distribution, not in the source.
        Remove-Item $file.FullName
    }
    Gate 'CycloneDX 1.5 schema-model validation' {
        $env:ORDINATA_SBOM_DIR = $stage
        $env:ORDINATA_SBOM_REQUIRED = '1'
        try {
            cargo test --locked -p ordinata-cli --test sbom -- --nocapture
        } finally {
            Remove-Item Env:\ORDINATA_SBOM_DIR -ErrorAction SilentlyContinue
            Remove-Item Env:\ORDINATA_SBOM_REQUIRED -ErrorAction SilentlyContinue
        }
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
    $builderPath = $env:PATH
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
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
        $env:PATH = $builderPath
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
    Write-DeterministicZip $stage $zip
    $zipHash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    Set-Content -Path "$zip.sha256" -Value "$zipHash  $(Split-Path -Leaf $zip)" -Encoding ascii

    # Prove the archive a customer receives, not merely the pre-archive staging tree.
    # The system-only PATH deliberately excludes rustup and every developer toolchain;
    # the executable can start only if the ZIP really contains its runtime beside it.
    Write-Host ""
    Write-Host "â”€â”€ Extracted-archive customer smoke (sanitized PATH)" -ForegroundColor Cyan
    $archiveRoom = Join-Path ([System.IO.Path]::GetTempPath()) ("ordinata-archive-" + [System.Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force $archiveRoom | Out-Null
    Expand-Archive -LiteralPath $zip -DestinationPath $archiveRoom
    $archiveStage = Join-Path $archiveRoom (Split-Path -Leaf $stage)
    $archiveExe = Join-Path $archiveStage 'ordinata.exe'
    if (-not (Test-Path -LiteralPath $archiveExe -PathType Leaf)) {
        throw "archive smoke: the ZIP did not contain the expected ordinata.exe"
    }

    $archivePath = $env:PATH
    $archivePreference = $ErrorActionPreference
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $ErrorActionPreference = 'Continue'
    try {
        Push-Location $archiveRoom
        & $archiveExe --version
        if ($LASTEXITCODE -ne 0) { throw "archive smoke: --version failed" }
        $archiveDemo = Join-Path $archiveRoom 'demo'
        & $archiveExe demo --out $archiveDemo --observed-at '2026-01-01T00:00:00Z'
        if ($LASTEXITCODE -ne 0) { throw "archive smoke: demo failed" }
        & $archiveExe verify $archiveDemo | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "archive smoke: the demo bundle did not verify" }
        if (-not (Test-Path -LiteralPath (Join-Path $archiveStage 'libunwind.dll') -PathType Leaf)) {
            throw "archive smoke: libunwind.dll was not shipped beside the executable"
        }
        Write-Host "   extracted EXE started, wrote a demo, and verified it without a toolchain PATH" -ForegroundColor Green
    } finally {
        Pop-Location
        $env:PATH = $archivePath
        $ErrorActionPreference = $archivePreference
        Remove-Item -Recurse -Force $archiveRoom -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "RELEASE CANDIDATE BUILT" -ForegroundColor Green
    Write-Host "  $zip"
    Write-Host "  sha256  $zipHash"
    Write-Host "  members $((Get-ChildItem -Path $stage -Recurse -File).Count)"
    Write-Host ""
    Write-Host "This is a review artifact. It is not published, and billing is not active."
} finally {
    Pop-Location
}
