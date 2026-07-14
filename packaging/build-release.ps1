<#
.SYNOPSIS
    Build the Ordinata release binary with deterministic, machine-independent paths.

.DESCRIPTION
    A Rust binary embeds the source path of every panic site it can reach, because that
    is what `file!()` expands to. For this workspace's own crates Cargo passes relative
    paths, so those are already harmless. For the *dependencies* it passes wherever the
    registry was unpacked -- which on a developer's machine is a directory named after
    that developer. Shipping it tells every customer who built the binary, from what
    account, on what drive. Before this script existed, `target/release/ordinata.exe`
    contained 52 copies of exactly that.

    `--remap-path-prefix` rewrites those paths as rustc records them. It cannot live in
    `.cargo/config.toml`, because the values are absolute paths that differ on every
    machine and Cargo does not interpolate variables into `rustflags`. So it is computed
    here, at build time, and this script is the only supported way to produce a release.

    What the dependency paths become is `/cargo/registry/src/index.crates.io-<hash>/
    <crate>-<version>/src/...`. That is deliberate rather than merely acceptable: the
    index hash is a public constant and the crate name and version are public facts,
    so a panic message still says exactly which line of which released crate it came
    from, while naming no user, no drive, and no directory that exists anywhere.

    `crates/ordinata-cli/tests/artifact.rs` is the gate that proves it worked.

.PARAMETER SkipVerify
    Build only, and do not run the artifact scan afterwards. For iterating.
#>
[CmdletBinding()]
param(
    [switch]$SkipVerify
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Cargo writes its progress to stderr, and under `$ErrorActionPreference = 'Stop'`
# PowerShell turns any stderr from a native program into a terminating error -- even
# when that program exited 0. So native calls run with the preference relaxed, and their
# real verdict, the exit code, is checked explicitly. Ignoring $LASTEXITCODE would be the
# actual mistake; this only stops PowerShell from inventing a failure that did not happen.
function Invoke-Native {
    param([Parameter(Mandatory = $true)][string]$What, [Parameter(Mandatory = $true)][scriptblock]$Command)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Command
    } finally {
        $ErrorActionPreference = $previous
    }
    if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# Where the registry actually lives on this machine. Cargo's own default when
# CARGO_HOME is unset is $USERPROFILE\.cargo, and that is the path that ends up in the
# binary, so that is the path that has to be remapped.
$cargoHome = if ($env:CARGO_HOME) {
    $env:CARGO_HOME
} else {
    Join-Path $env:USERPROFILE '.cargo'
}

# rustc splits FROM=TO at the *last* `=`, so a Windows path on the left is safe: paths
# do not contain `=`. Both prefixes are mapped, because a build launched from outside
# the workspace passes absolute paths for the local crates too.
$remap = @(
    "--remap-path-prefix=$cargoHome=/cargo",
    "--remap-path-prefix=$root=/ordinata",

    # LLVM's MinGW linker otherwise writes the current wall-clock time into the PE
    # COFF header. Two clean builds of the same commit then differ by four bytes even
    # though their code and data are identical. Suppress that timestamp so the release
    # executable is reproducible rather than merely path-clean.
    "-C link-arg=-Wl,--no-insert-timestamp"
) -join ' '

Write-Host "Ordinata release build" -ForegroundColor Cyan
Write-Host "  workspace   $root"
Write-Host "  cargo home  (remapped to /cargo)"
Write-Host "  remapping   dependency and checkout paths out of the binary"
Write-Host ""

$env:RUSTFLAGS = $remap

Push-Location $root
try {
    Invoke-Native 'cargo build --release' { cargo build --release --locked }
} finally {
    Pop-Location
}

$exe = Join-Path $root 'target\release\ordinata.exe'
if (-not (Test-Path $exe)) { throw "the release build did not produce $exe" }

$size = (Get-Item $exe).Length
$hash = (Get-FileHash -Algorithm SHA256 $exe).Hash.ToLower()
Write-Host ""
Write-Host "Built $exe" -ForegroundColor Green
Write-Host "  bytes   $size"
Write-Host "  sha256  $hash"

if (-not $SkipVerify) {
    Write-Host ""
    Write-Host "Scanning the artifact for local and personal paths..." -ForegroundColor Cyan

    # RUSTFLAGS is cleared first, deliberately. The scan only reads the file that was
    # just built; it does not need the remap flags itself, and leaving them set would
    # change the fingerprint of the *test* profile and force a needless rebuild of every
    # dev-dependency. Remapping is a property of the artifact, not of the tool that
    # inspects it.
    Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue

    Push-Location $root
    try {
        # ORDINATA_SCAN_REQUIRED turns "there was nothing to scan" into a failure, so the
        # release path cannot take the skip that a bare `cargo test` is allowed to take.
        $env:ORDINATA_SCAN_REQUIRED = '1'
        Invoke-Native 'the artifact scan' {
            cargo test --locked -p ordinata-cli --test artifact -- --nocapture
        }
    } finally {
        Remove-Item Env:\ORDINATA_SCAN_REQUIRED -ErrorAction SilentlyContinue
        Pop-Location
    }
    Write-Host "Artifact scan clean: the binary names no account, no drive, and no directory." -ForegroundColor Green
}
