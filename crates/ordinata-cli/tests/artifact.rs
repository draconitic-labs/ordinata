//! Does the thing we ship carry anything about the machine that built it?
//!
//! A Rust binary embeds the source path of every `panic!`, `assert!`, and `unwrap` it
//! can reach, because that is what `file!()` expands to. For the crates in this
//! workspace those paths are relative and harmless. For the *dependencies*, they are
//! wherever Cargo unpacked the registry, which on a developer's machine is a directory
//! with that developer's name in it. Ship that and the binary quietly tells every
//! customer who built it, from what account, on what drive.
//!
//! The fix is `--remap-path-prefix`, applied by `packaging/build-release.ps1`. This is
//! the test that proves the fix worked, and keeps proving it.
//!
//! # What it looks for
//!
//! Nothing personal is written down here. That would be self-defeating: a scanner that
//! greps for a developer's name has that developer's name in it, committed to the
//! repository, forever. Instead the markers are *computed from the environment the scan
//! runs in* -- this account's name, this home directory, this Cargo home, this
//! checkout -- plus a handful of structural patterns that no shipped artifact should
//! contain on any machine. The scan therefore asks the only question that matters:
//! "does this artifact say anything about the computer it was built on?"
//!
//! # What it deliberately allows
//!
//! `/cargo/registry/src/index.crates.io-<hash>/<crate>-<version>/src/...` is the
//! remapped form, and it is *good*: the registry index hash is a public constant and
//! the crate name and version are public facts that a reader of a panic message wants.
//! It names no user, no drive, and no directory that exists on anybody's machine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

/// One thing that must not appear in a shipped artifact, and why.
struct Marker {
    /// What a failure should call it.
    label: String,
    /// The bytes to look for. Matched case-insensitively over ASCII.
    needle: String,
}

impl Marker {
    fn new(label: &str, needle: impl Into<String>) -> Self {
        Marker {
            label: label.to_string(),
            needle: needle.into(),
        }
    }
}

fn workspace_root() -> PathBuf {
    let canonical = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root should resolve");
    // Canonicalizing on Windows produces an extended-length path, `\\?\E:\...`. That
    // prefix is a fact about the API that answered, not about the path, and a binary
    // would never contain it -- so a needle that kept it would match nothing, and this
    // marker would silently never fire. Strip it, or the scan is decorative.
    let text = canonical.display().to_string();
    PathBuf::from(text.strip_prefix(r"\\?\UNC\").map_or_else(
        || text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
        |unc| format!(r"\\{unc}"),
    ))
}

/// Every drive-letter absolute path in `bytes`, as text.
///
/// A bare `C:\` is not enough to go on: three bytes that happen to spell it turn up in
/// compiled machine code by pure chance, and an early version of this scan duly failed
/// on one. What makes a path a path is what *follows* the drive: a run of path
/// characters long enough to be a real one, containing at least one further separator.
/// Random code does not do that.
fn drive_absolute_paths(bytes: &[u8]) -> Vec<String> {
    fn is_path_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'\\' | b'/' | b'.' | b'-' | b'_' | b' ')
    }

    let mut found = Vec::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let separator = bytes[i + 2];
        let has_boundary = i == 0
            || !matches!(bytes[i - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-');
        if bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && matches!(separator, b'\\' | b'/')
            && has_boundary
            && !is_remote_url_context(bytes, i)
        {
            let mut end = i + 3;
            while end < bytes.len() && is_path_byte(bytes[end]) {
                end += 1;
            }
            let tail = &bytes[i + 3..end];
            if tail.len() >= 6 && tail.iter().any(|b| matches!(b, b'\\' | b'/')) {
                found.push(String::from_utf8_lossy(&bytes[i..end]).into_owned());
                i = end;
                continue;
            }
        }
        i += 1;
    }
    found
}

/// Is the candidate drive-shaped text part of a remote URL rather than a local path?
/// `https://host/C:/manual` is a perfectly valid URL and must not fail a release scan;
/// `file:///C:/build/source` is explicitly local and must still be caught.
fn is_remote_url_context(bytes: &[u8], candidate: usize) -> bool {
    let start = bytes[..candidate]
        .iter()
        .rposition(|b| {
            b.is_ascii_whitespace()
                || matches!(
                    b,
                    b'\0'
                        | b'"'
                        | b'\''
                        | b'<'
                        | b'>'
                        | b'('
                        | b')'
                        | b'['
                        | b']'
                        | b'{'
                        | b'}'
                        | b'='
                )
        })
        .map_or(0, |index| index + 1);
    let prefix = &bytes[start..candidate];
    [b"http://".as_slice(), b"https://", b"ftp://"]
        .iter()
        .any(|scheme| {
            prefix.len() >= scheme.len() && prefix[..scheme.len()].eq_ignore_ascii_case(scheme)
        })
}

/// Every marker, computed fresh from the environment this scan is running in.
///
/// Two kinds. The *local* ones are facts about this machine, read from the environment
/// so that none of them has to be committed. The *structural* ones are shapes that no
/// artifact should carry on any machine, and they are what makes the scan mean
/// something when it runs somewhere with a boring username.
fn markers() -> Vec<Marker> {
    let mut markers = Vec::new();

    // --- Local: who and where is this? -------------------------------------------
    for key in ["USERNAME", "USER", "LOGNAME"] {
        if let Ok(name) = std::env::var(key) {
            // A username of "user" or "root" would match half the strings in any
            // binary. Those are exactly the cases where the structural markers below
            // are doing the work instead.
            if name.len() >= 4
                && !matches!(
                    name.to_lowercase().as_str(),
                    "user" | "root" | "runner" | "build"
                )
            {
                markers.push(Marker::new(
                    "the account name of the machine that built it",
                    name,
                ));
            }
        }
    }
    for key in ["USERPROFILE", "HOME", "CARGO_HOME"] {
        if let Ok(path) = std::env::var(key) {
            if path.len() >= 4 {
                markers.push(Marker::new(
                    "a home directory of the machine that built it",
                    path,
                ));
            }
        }
    }
    // The default Cargo home, which is where the registry lives unless CARGO_HOME says
    // otherwise, and therefore the actual source of the leak this test exists for.
    if let Ok(profile) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        markers.push(Marker::new(
            "the Cargo registry of the machine that built it",
            format!("{profile}\\.cargo"),
        ));
        markers.push(Marker::new(
            "the Cargo registry of the machine that built it",
            format!("{profile}/.cargo"),
        ));
    }
    markers.push(Marker::new(
        "the source checkout it was built from",
        workspace_root().display().to_string(),
    ));

    // --- Structural: true on any machine ------------------------------------------
    // Each of these is long enough that a chance occurrence in compiled code is not a
    // realistic worry, unlike a bare drive letter. Drive-letter paths are found by
    // `drive_absolute_paths` instead, which looks at what follows the colon.
    for needle in [
        "\\Users\\",
        "/Users/",
        "/home/",
        "\\.cargo\\registry",
        "/.cargo/registry",
    ] {
        markers.push(Marker::new("a local absolute path", needle));
    }

    markers
}

/// Case-insensitive ASCII substring search over raw bytes. The artifact is not text, so
/// it is never decoded; a path embedded in it is ASCII either way.
fn contains_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// Every marker that appears in `bytes`, with a little of the surrounding text so that
/// a failure shows what was actually found rather than only that something was.
fn scan(bytes: &[u8]) -> Vec<String> {
    let mut findings = Vec::new();
    for marker in markers() {
        if let Some(at) = contains_ignore_ascii_case(bytes, marker.needle.as_bytes()) {
            let from = at.saturating_sub(16);
            let to = (at + marker.needle.len() + 64).min(bytes.len());
            let context: String = bytes[from..to]
                .iter()
                .map(|b| {
                    if b.is_ascii_graphic() || *b == b' ' {
                        *b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            findings.push(format!(
                "{} ({:?}) at byte {at}: ...{context}...",
                marker.label, marker.needle
            ));
        }
    }
    for path in drive_absolute_paths(bytes) {
        findings.push(format!("a local absolute path: {path:?}"));
    }
    findings
}

/// The pinned Rust gnullvm runtime retains the public llvm-mingw CI checkout path in
/// its vendor-built DLL. It identifies an upstream GitHub Actions runner, not this
/// machine or whoever built Ordinata. Packaging copies and hashes that exact DLL from
/// the pinned sysroot, so this one known provenance string is accounted for without
/// weakening scans of Ordinata's executable or any other shipped file.
fn accounted_vendor_runtime_finding(path: &Path, bytes: &[u8], finding: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("libunwind.dll"))
        && finding.contains("a local absolute path (\"/home/\")")
        && contains_ignore_ascii_case(bytes, b"/home/runner/work/llvm-mingw/llvm-mingw/").is_some()
}

/// Read the PE/COFF timestamp when `bytes` is a Windows executable.
///
/// LLVM's MinGW linker writes the current wall-clock time here unless the release
/// build explicitly disables it. That four-byte field is enough to make two otherwise
/// identical clean builds hash differently.
fn pe_coff_timestamp(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return None;
    }
    let header = u32::from_le_bytes(bytes[0x3c..0x40].try_into().ok()?) as usize;
    let timestamp_end = header.checked_add(12)?;
    if timestamp_end > bytes.len() || &bytes[header..header + 4] != b"PE\0\0" {
        return None;
    }
    Some(u32::from_le_bytes(
        bytes[header + 8..timestamp_end].try_into().ok()?,
    ))
}

// ---------------------------------------------------------------------------
// The scanner works.
// ---------------------------------------------------------------------------

#[test]
fn the_scanner_finds_a_leak_when_there_is_one() {
    // A scanner that cannot fail proves nothing about the artifact it passes. These are
    // the exact shapes the release build used to contain, and each must be caught.
    let leaks: [&[u8]; 4] = [
        b"\x00\x00C:\\Users\\somebody\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\time-0.3.53\\src\\lib.rs\x00",
        b"padding /home/somebody/.cargo/registry/src/x.rs padding",
        b"E:\\some\\checkout\\crates\\ordinata-core\\src\\solve.rs",
        b"\\Users\\anybody\\anything",
    ];
    for leak in leaks {
        assert!(
            !scan(leak).is_empty(),
            "the scanner missed {:?}",
            String::from_utf8_lossy(leak)
        );
    }
}

#[test]
fn the_scanner_finds_forward_slash_and_file_uri_drive_paths() {
    for leak in [
        b"C:/build/deps/thing.rs".as_slice(),
        b"file:///D:/private/checkouts/thing.rs",
        b"path+file:///E:/workspace/crates/ordinata-core#0.1.0",
    ] {
        assert!(
            !scan(leak).is_empty(),
            "the scanner missed {:?}",
            String::from_utf8_lossy(leak)
        );
    }
}

#[test]
fn the_scanner_does_not_mistake_remote_urls_for_drive_paths() {
    for url in [
        b"https://example.com/a/b".as_slice(),
        b"http://example.test/C:/published/manual",
        b"ftp://downloads.example.test/E:/release/archive.zip",
    ] {
        assert!(
            scan(url).is_empty(),
            "a remote URL was reported as a local path: {:?}: {:?}",
            String::from_utf8_lossy(url),
            scan(url)
        );
    }
}

#[test]
fn only_the_named_vendor_runtime_accounts_for_its_public_ci_path() {
    let bytes = b"/home/runner/work/llvm-mingw/llvm-mingw/mingw-w64/source.c";
    let finding = scan(bytes).pop().expect("the structural scan must fire");
    assert!(accounted_vendor_runtime_finding(
        Path::new("libunwind.dll"),
        bytes,
        &finding
    ));
    assert!(!accounted_vendor_runtime_finding(
        Path::new("ordinata.exe"),
        bytes,
        &finding
    ));
    assert!(!accounted_vendor_runtime_finding(
        Path::new("libunwind.dll"),
        b"/home/private/build/source.c",
        &finding
    ));
}

#[test]
fn the_scanner_passes_the_remapped_form_that_a_good_build_produces() {
    // This is what a correctly remapped binary contains, and it must not be flagged.
    // The registry index hash is a public constant; the crate name and version are
    // public facts a panic message is *better* for carrying. Neither says anything
    // about a machine.
    let good: &[u8] =
        b"/cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\nalgebra-0.34.1\\src\\linalg\\svd.rs\x00\
          crates\\ordinata-core\\src\\solve.rs\x00/rustc/abcdef/library/std/src/panic.rs";
    assert!(
        scan(good).is_empty(),
        "a correctly remapped artifact was flagged: {:?}",
        scan(good)
    );
}

#[test]
fn a_drive_letter_that_is_only_machine_code_is_not_a_path() {
    // Byte-for-byte out of the compiled binary, where `43 3A 5C` -- "C:\" -- turns up
    // inside an instruction stream by chance. An earlier version of this scan flagged
    // it and would have failed every release for a coincidence. What distinguishes a
    // path from three lucky bytes is that a path keeps going.
    let machine_code: &[u8] = b"\x48\x39\xc6\x0f\x85\x43\x3a\x5c\x1d\x4d\x89\xd2\x74\x08\x48\x8b";
    assert!(
        scan(machine_code).is_empty(),
        "a chance byte sequence was reported as a path: {:?}",
        scan(machine_code)
    );

    // But a real one, in the same shape of surrounding noise, is still caught.
    let real: &[u8] = b"\x48\x39\xc6\x0f\x85C:\\build\\deps\\thing.rs\x00\x4d\x89";
    assert!(
        !scan(real).is_empty(),
        "a real absolute path was missed among the noise"
    );
}

#[test]
fn the_pe_timestamp_parser_reads_the_coff_header() {
    let mut executable = vec![0_u8; 0x8c];
    executable[..2].copy_from_slice(b"MZ");
    executable[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
    executable[0x80..0x84].copy_from_slice(b"PE\0\0");
    executable[0x88..0x8c].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
    assert_eq!(pe_coff_timestamp(&executable), Some(0x1234_5678));
    assert_eq!(pe_coff_timestamp(b"not a PE file"), None);
}

// ---------------------------------------------------------------------------
// The artifact is clean.
// ---------------------------------------------------------------------------

/// Which files to scan.
///
/// `ORDINATA_SCAN_DIR` points the scan at a directory and scans everything in it; that
/// is what `packaging/package.ps1` sets, so that the gate covers the whole distribution
/// and not only the executable. With nothing set, the scan falls back to the release
/// binary in `target/`, which is what a developer running `cargo test` after a release
/// build wants.
fn targets() -> Vec<PathBuf> {
    if let Ok(directory) = std::env::var("ORDINATA_SCAN_DIR") {
        let mut files = Vec::new();
        let mut stack = vec![PathBuf::from(directory)];
        while let Some(path) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        return files;
    }

    let binary = workspace_root()
        .join("target")
        .join("release")
        .join(if cfg!(windows) {
            "ordinata.exe"
        } else {
            "ordinata"
        });
    if binary.is_file() {
        vec![binary]
    } else {
        Vec::new()
    }
}

#[test]
fn the_shipped_artifact_carries_no_local_or_personal_path() {
    let targets = targets();

    if targets.is_empty() {
        // Say so rather than passing quietly. The packaging gate sets
        // ORDINATA_SCAN_REQUIRED, which turns this into a failure, so the release path
        // can never take the skip.
        assert!(
            std::env::var("ORDINATA_SCAN_REQUIRED").is_err(),
            "the scan was required but there was nothing to scan; run packaging/build-release.ps1 first"
        );
        eprintln!(
            "NOTICE: no release artifact to scan (target/release/ordinata is absent), so this \
             test checked the scanner and not the binary. Run packaging/build-release.ps1, or \
             the whole of packaging/package.ps1, to make this a real check."
        );
        return;
    }

    let mut dirty = Vec::new();
    for path in &targets {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
        for finding in scan(&bytes) {
            if accounted_vendor_runtime_finding(path, &bytes, &finding) {
                continue;
            }
            dirty.push(format!("{}: {finding}", path.display()));
        }
    }

    assert!(
        dirty.is_empty(),
        "the artifact names the machine that built it, in {} place(s):\n  {}\n\n\
         Build it through packaging/build-release.ps1, which sets the --remap-path-prefix \
         flags that keep the registry and checkout paths out of the binary.",
        dirty.len(),
        dirty.join("\n  ")
    );

    eprintln!("scanned {} artifact(s), all clean", targets.len());
}

#[test]
fn the_windows_release_has_no_wall_clock_timestamp() {
    if !cfg!(windows) {
        return;
    }

    let artifacts: Vec<_> = targets()
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("ordinata.exe"))
        })
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            pe_coff_timestamp(&bytes).map(|timestamp| (path, timestamp))
        })
        .collect();

    if artifacts.is_empty() {
        assert!(
            std::env::var("ORDINATA_SCAN_REQUIRED").is_err(),
            "the reproducibility scan was required but no Windows release executable was found"
        );
        return;
    }

    for (path, timestamp) in artifacts {
        assert_eq!(
            timestamp,
            0,
            "{} carries a wall-clock PE/COFF timestamp; build through packaging/build-release.ps1",
            path.display()
        );
    }
}
