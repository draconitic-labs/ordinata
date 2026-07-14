//! What a bundle is, what verification proves, and every way of breaking one.
//!
//! The tamper tests are written as a ladder. Each rung is what a forger has to fix
//! after the rung below caught them, and each rung is a *different* check with a
//! *different* name, so a failing verification says which layer noticed rather than
//! just "no". The top of the ladder is re-deriving the bundle from scratch, which is
//! not forgery, and which `verify` has never claimed to detect: it is not a signature,
//! and the module that implements it says so out loud.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ordinata_core::bundle::{CHECKSUMS, CONTENT_MEMBERS, MANIFEST, PAYLOAD_MEMBERS};
use ordinata_core::canonical::sha256_hex;
use ordinata_core::{build_bundle, load_project, solve, verify_bundle, write_bundle, Bundle};
use serde_json::Value;
use time::OffsetDateTime;

/// A fixed observation time. Every bundle in this file is built at the same instant, so
/// that anything that differs between two of them differs because of the numbers.
fn fixed_time() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("a valid timestamp")
}

const PROJECT: &str = r#"{
    "schemaVersion": 1,
    "title": "Bundle case",
    "designCsv": "design.csv",
    "observationsCsv": "observations.csv",
    "regularization": { "mode": "explicit", "lambda": 0.01 },
    "uncertainty": { "method": "both", "confidenceLevel": 0.95, "bootstrapSamples": 300, "bootstrapSeed": 17 },
    "assumptions": ["The response is linear in an intercept and a slope."],
    "limitations": ["Synthetic. Measures nothing."]
}"#;

const DESIGN: &str =
    "observation_id,intercept,slope\nb0,1,0\nb1,1,1\nb2,1,2\nb3,1,3\nb4,1,4\nb5,1,5\n";
const OBSERVATIONS: &str = "observation_id,value,standard_error\nb0,1.1,0.2\nb1,1.9,0.2\nb2,3.2,0.2\nb3,3.9,0.2\nb4,5.3,0.2\nb5,5.8,0.2\n";

struct Written {
    _directory: tempfile::TempDir,
    /// The bundle directory on disk.
    path: PathBuf,
    bundle: Bundle,
}

/// Build the standard bundle and write it out.
fn written() -> Written {
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("design.csv"), DESIGN).unwrap();
    std::fs::write(source.join("observations.csv"), OBSERVATIONS).unwrap();
    let project = source.join("project.json");
    std::fs::write(&project, PROJECT).unwrap();

    let problem = load_project(&project).expect("should load");
    let solution = solve(&problem).expect("should solve");
    let bundle = build_bundle(&problem, &solution, fixed_time()).expect("should build");

    let out = directory.path().join("bundle");
    let path = write_bundle(&bundle, &out, &problem.input_paths).expect("should write");

    Written {
        _directory: directory,
        path,
        bundle,
    }
}

fn read_member(root: &Path, member: &str) -> Vec<u8> {
    std::fs::read(root.join(member)).unwrap_or_else(|e| panic!("cannot read {member}: {e}"))
}

fn write_member(root: &Path, member: &str, bytes: &[u8]) {
    std::fs::write(root.join(member), bytes)
        .unwrap_or_else(|e| panic!("cannot write {member}: {e}"));
}

/// Rewrite `SHA256SUMS.txt` so that it agrees with whatever is on disk now. This is
/// what a forger who understood the first layer would do.
fn recompute_checksums(root: &Path) {
    let mut lines: Vec<String> = Vec::new();
    for member in PAYLOAD_MEMBERS.iter().copied().chain([MANIFEST]) {
        let bytes = read_member(root, member);
        lines.push(format!("{}  {member}", sha256_hex(&bytes)));
    }
    lines.sort();
    let mut text = lines.join("\n");
    text.push('\n');
    write_member(root, CHECKSUMS, text.as_bytes());
}

fn failures(report: &ordinata_core::VerificationReport) -> Vec<&'static str> {
    report.failures().map(|check| check.name).collect()
}

// ---------------------------------------------------------------------------
// A bundle that has not been touched verifies.
// ---------------------------------------------------------------------------

#[test]
fn a_freshly_written_bundle_verifies_and_says_what_it_is() {
    let case = written();
    let report = verify_bundle(&case.path).expect("verification should run");

    assert!(
        report.passed(),
        "a bundle straight out of `solve` must verify: {:?}",
        failures(&report)
    );
    assert_eq!(report.kind, "directory");

    let summary = report.summary.expect("a readable bundle has a summary");
    assert_eq!(summary.title, "Bundle case");
    assert!(!summary.synthetic, "user data is never synthetic");
    assert_eq!(summary.result_content_hash, case.bundle.result_content_hash);

    // Every check that ran, ran to a verdict. A verification that quietly skipped the
    // deep check and still said "ok" would be worse than one that failed.
    for check in &report.checks {
        assert_eq!(
            check.status, "pass",
            "{} was {}: {}",
            check.name, check.status, check.detail
        );
    }
}

#[test]
fn a_bundle_holds_exactly_the_members_a_bundle_holds() {
    let case = written();
    let found: Vec<&str> = case.bundle.members().map(|(path, _)| path).collect();
    let mut expected: Vec<&str> = PAYLOAD_MEMBERS.to_vec();
    expected.push(MANIFEST);
    expected.push(CHECKSUMS);
    expected.sort_unstable();
    assert_eq!(found, expected);

    // And every one of them is really on disk.
    for member in &expected {
        assert!(
            case.path.join(member).is_file(),
            "{member} is missing from the written bundle"
        );
    }
}

// ---------------------------------------------------------------------------
// Determinism: the numbers do not depend on the clock, and two runs agree.
// ---------------------------------------------------------------------------

#[test]
fn a_fixed_time_bundle_is_reproducible_byte_for_byte() {
    let first = written();
    let second = written();

    assert_eq!(
        first.bundle.result_content_hash, second.bundle.result_content_hash,
        "identical inputs, configuration, seed, and observation time must agree"
    );

    // Not just the hash: the actual bytes of every member, including the manifest and
    // the report, because the observation time was pinned for both.
    for (member, bytes) in first.bundle.members() {
        assert_eq!(
            Some(bytes),
            second.bundle.member(member),
            "{member} differs between two runs of the same problem at the same instant"
        );
    }
}

#[test]
fn the_result_content_hash_is_pinned() {
    // A golden value. It is not sacred -- any deliberate change to the numbers, the
    // canonical form, or the bundle layout will move it, and moving it is fine when
    // that is what was meant. What it catches is the change nobody meant: a refactor
    // that quietly alters a number, a dependency that reorders a map, a formatting
    // routine that starts rounding. Any of those would silently invalidate every
    // bundle Ordinata has ever written, and this is the test that makes that loud.
    //
    // Update this together with `spec::MODEL_VERSION` when the change is intentional.
    const GOLDEN: &str = "7a7ba44da48e743f653ebdda6ac7eb87e43e7e3ad116ce077ffa399edda763c9";
    let case = written();
    assert_eq!(
        case.bundle.result_content_hash, GOLDEN,
        "the content hash moved; if that was deliberate, bump spec::MODEL_VERSION and update this constant"
    );
}

// ---------------------------------------------------------------------------
// Both intervals survive into the bundle.
// ---------------------------------------------------------------------------

#[test]
fn asking_for_both_intervals_gets_both_intervals_everywhere() {
    let case = written();

    // parameters.csv carries each method in columns of its own.
    let parameters = String::from_utf8(case.bundle.member("parameters.csv").unwrap().to_vec())
        .expect("parameters.csv is UTF-8");
    let mut lines = parameters.lines();
    let header: Vec<&str> = lines.next().expect("a header").split(',').collect();
    for column in [
        "analytic_lower",
        "analytic_upper",
        "bootstrap_lower",
        "bootstrap_upper",
        "interval_source",
    ] {
        assert!(
            header.contains(&column),
            "parameters.csv must carry {column}: {header:?}"
        );
    }

    let index = |name: &str| header.iter().position(|c| *c == name).unwrap();
    let mut rows = 0;
    for line in lines {
        let fields: Vec<&str> = line.split(',').collect();
        rows += 1;
        for column in [
            "analytic_lower",
            "analytic_upper",
            "bootstrap_lower",
            "bootstrap_upper",
        ] {
            let value = fields[index(column)];
            assert!(
                !value.is_empty() && value.parse::<f64>().is_ok(),
                "{column} is {value:?}, but both methods were requested and both were emitted"
            );
        }
        // The two intervals are different claims and must not have been collapsed into
        // one another.
        let analytic = (
            fields[index("analytic_lower")].parse::<f64>().unwrap(),
            fields[index("analytic_upper")].parse::<f64>().unwrap(),
        );
        let bootstrap = (
            fields[index("bootstrap_lower")].parse::<f64>().unwrap(),
            fields[index("bootstrap_upper")].parse::<f64>().unwrap(),
        );
        assert_ne!(
            analytic, bootstrap,
            "the analytic and bootstrap intervals came out identical, which means one of them was copied from the other"
        );
        assert!(analytic.0 < analytic.1 && bootstrap.0 < bootstrap.1);
    }
    assert_eq!(rows, 2, "two parameters");

    // diagnostics.json carries both, with their statuses.
    let diagnostics: Value =
        serde_json::from_slice(case.bundle.member("diagnostics.json").unwrap()).unwrap();
    let uncertainty = &diagnostics["uncertainty"];
    assert_eq!(uncertainty["analytic"]["status"], "emitted");
    assert_eq!(uncertainty["bootstrap"]["status"], "emitted");
    for method in ["analytic", "bootstrap"] {
        for bound in ["lower", "upper"] {
            let array = uncertainty[method][bound]
                .as_array()
                .unwrap_or_else(|| panic!("{method}.{bound} must be an array"));
            assert_eq!(
                array.len(),
                2,
                "{method}.{bound} must cover both parameters"
            );
        }
    }
    assert_eq!(uncertainty["bootstrap"]["requestedSamples"], 300);
    assert_eq!(uncertainty["bootstrap"]["successfulSamples"], 300);
    assert_eq!(uncertainty["bootstrap"]["seed"], 17);

    // report.html shows both, side by side.
    let report = String::from_utf8(case.bundle.member("report.html").unwrap().to_vec()).unwrap();
    assert!(
        report.contains("Analytic lower"),
        "the report hides the analytic interval"
    );
    assert!(
        report.contains("Bootstrap lower"),
        "the report hides the bootstrap interval"
    );
    assert!(
        !report.contains("<script"),
        "the report must carry no script"
    );
}

// ---------------------------------------------------------------------------
// The tamper ladder.
// ---------------------------------------------------------------------------

#[test]
fn rung_one_editing_a_number_breaks_its_checksum() {
    let case = written();
    let mut fit = String::from_utf8(read_member(&case.path, "fit.csv")).unwrap();
    fit = fit.replace("1.1", "9.9");
    write_member(&case.path, "fit.csv", fit.as_bytes());

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    assert!(
        failures(&report).contains(&"member_checksums"),
        "editing a member must break its recorded checksum: {:?}",
        failures(&report)
    );
}

#[test]
fn rung_two_fixing_the_checksums_leaves_the_manifest_disagreeing() {
    let case = written();
    let mut fit = String::from_utf8(read_member(&case.path, "fit.csv")).unwrap();
    fit = fit.replace("1.1", "9.9");
    write_member(&case.path, "fit.csv", fit.as_bytes());
    recompute_checksums(&case.path);

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    let names = failures(&report);
    assert!(
        names.contains(&"manifest_payload"),
        "the manifest records its own hash of every member, so fixing SHA256SUMS is not enough: {names:?}"
    );
    assert!(
        !names.contains(&"member_checksums"),
        "the checksums really were repaired, so that layer must now pass: {names:?}"
    );
}

#[test]
fn rung_three_fixing_the_manifest_too_leaves_the_content_hash_disagreeing() {
    let case = written();

    let mut fit = String::from_utf8(read_member(&case.path, "fit.csv")).unwrap();
    fit = fit.replace("1.1", "9.9");
    write_member(&case.path, "fit.csv", fit.as_bytes());

    // Repair the manifest's payload entry for fit.csv, then the checksums.
    let mut manifest: Value = serde_json::from_slice(&read_member(&case.path, MANIFEST)).unwrap();
    let bytes = read_member(&case.path, "fit.csv");
    for entry in manifest["payload"].as_array_mut().unwrap() {
        if entry["path"] == "fit.csv" {
            entry["sha256"] = Value::String(sha256_hex(&bytes));
            entry["bytes"] = Value::from(bytes.len());
        }
    }
    write_member(
        &case.path,
        MANIFEST,
        serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
    );
    recompute_checksums(&case.path);

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    let names = failures(&report);
    assert!(
        names.contains(&"result_content_hash"),
        "the deepest layer: the numbers no longer hash to what the manifest says they do: {names:?}"
    );
    assert!(
        !names.contains(&"member_checksums") && !names.contains(&"manifest_payload"),
        "the shallower layers were repaired and must now pass, or this test is not testing the deep one: {names:?}"
    );
}

#[test]
fn a_missing_member_is_caught_by_name() {
    for member in [
        "fit.csv",
        "parameters.csv",
        "diagnostics.json",
        "validation.json",
        "report.html",
        "inputs/design.csv",
    ] {
        let case = written();
        std::fs::remove_file(case.path.join(member)).unwrap();

        let report = verify_bundle(&case.path).unwrap();
        assert!(!report.passed(), "removing {member} must fail verification");
        let names = failures(&report);
        assert!(
            names.contains(&"bundle_shape"),
            "removing {member} must be caught as a missing member: {names:?}"
        );
        let detail = report
            .checks
            .iter()
            .find(|c| c.name == "bundle_shape")
            .map(|c| c.detail.clone())
            .unwrap_or_default();
        assert!(
            detail.contains(member),
            "the failure must name {member}: {detail}"
        );
    }
}

#[test]
fn an_extra_file_smuggled_in_is_not_part_of_a_bundle() {
    let case = written();
    write_member(&case.path, "notes.txt", b"a file nobody accounted for");

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    let names = failures(&report);
    assert!(
        names.contains(&"bundle_shape"),
        "a file that is not part of a bundle must be named as such: {names:?}"
    );
}

#[test]
fn a_manifest_that_names_a_key_twice_is_not_read_in_either_direction() {
    let case = written();
    // Two `resultContent` blocks. serde_json's default would take the last one, which
    // is how a forger would install a hash of their own while leaving an honest-looking
    // one on the line above for a human to read.
    let text = String::from_utf8(read_member(&case.path, MANIFEST)).unwrap();
    let hostile = text.replacen(
        "\"resultContent\":",
        "\"resultContent\": {\"members\": [], \"sha256\": \"\", \"note\": \"\"},\n  \"resultContent\":",
        1,
    );
    write_member(&case.path, MANIFEST, hostile.as_bytes());
    recompute_checksums(&case.path);

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    let manifest_check = report
        .checks
        .iter()
        .find(|c| c.name == "manifest_present")
        .expect("the manifest check must run");
    assert_eq!(manifest_check.status, "fail");
    assert!(
        manifest_check.detail.contains("more than once"),
        "the duplicate key must be the stated reason: {}",
        manifest_check.detail
    );
}

#[test]
fn a_manifest_that_drops_a_member_from_its_payload_is_refused() {
    let case = written();
    let mut manifest: Value = serde_json::from_slice(&read_member(&case.path, MANIFEST)).unwrap();
    manifest["payload"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["path"] != "validation.json");
    write_member(
        &case.path,
        MANIFEST,
        serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
    );
    recompute_checksums(&case.path);

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    assert!(
        failures(&report).contains(&"manifest_payload"),
        "a manifest that simply omits a member it does not want examined must be refused: {:?}",
        failures(&report)
    );
}

#[test]
fn a_manifest_that_describes_a_member_twice_is_refused() {
    let case = written();
    let mut manifest: Value = serde_json::from_slice(&read_member(&case.path, MANIFEST)).unwrap();
    let payload = manifest["payload"].as_array_mut().unwrap();
    let duplicate = payload
        .iter()
        .find(|entry| entry["path"] == "fit.csv")
        .cloned()
        .unwrap();
    payload.push(duplicate);
    write_member(
        &case.path,
        MANIFEST,
        serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
    );
    recompute_checksums(&case.path);

    let report = verify_bundle(&case.path).unwrap();
    assert!(!report.passed());
    let detail = report
        .checks
        .iter()
        .find(|c| c.name == "manifest_payload")
        .map(|c| c.detail.clone())
        .unwrap_or_default();
    assert!(
        detail.contains("twice"),
        "two descriptions of one member is not decidable, and must say so: {detail}"
    );
}

#[test]
fn a_checksum_file_that_points_outside_the_bundle_is_refused_before_anything_opens_it() {
    let case = written();
    let hostile = format!("{}  ../../../etc/passwd\n", "a".repeat(64));
    write_member(&case.path, CHECKSUMS, hostile.as_bytes());

    let err = verify_bundle(&case.path).expect_err("an unsafe path is an error, not a report");
    assert_eq!(err.code(), "unsafe_checksum_path");
    assert_eq!(err.exit_code(), 5);
}

// ---------------------------------------------------------------------------
// A bundle directory holds regular files, and nothing that redirects elsewhere.
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn a_junction_inside_a_bundle_is_refused_rather_than_followed() {
    // A junction is the realistic version of this attack on Windows, because unlike a
    // symbolic link it needs no privilege at all: any user can make one. A verifier
    // that recursed into it would walk straight out of the bundle and hash whatever it
    // found on the other side.
    let case = written();
    let outside = case.path.parent().unwrap().join("elsewhere");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("planted.csv"), "not part of any bundle").unwrap();

    let link = case.path.join("inputs2");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output();

    match made {
        Ok(output) if link.exists() => {
            let err = verify_bundle(&case.path)
                .expect_err("a bundle containing a junction is not a bundle");
            assert_eq!(err.code(), "not_a_bundle", "{err}");
            assert!(
                err.to_string().contains("reparse point"),
                "the refusal must name what it found: {err}"
            );
            let _ = output;
        }
        _ => {
            // mklink is unavailable in this environment. The attribute this test is
            // about is the same one a symlink sets, and the symlink case below covers
            // it, so this is a gap in coverage rather than a silent pass.
            eprintln!(
                "SKIPPED: could not create a junction in this environment; \
                 the reparse-point rejection is still covered by the symlink test"
            );
        }
    }
}

#[test]
fn a_symbolic_link_inside_a_bundle_is_refused_rather_than_followed() {
    let case = written();
    let outside = case.path.parent().unwrap().join("secret.csv");
    std::fs::write(&outside, "not part of any bundle").unwrap();
    let link = case.path.join("planted.csv");

    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&outside, &link).is_ok();

    if !made {
        // Symbolic links need a privilege on Windows that an ordinary test run does not
        // have. Saying so beats pretending the check ran.
        eprintln!("SKIPPED: this environment cannot create a symbolic link");
        return;
    }

    let err = verify_bundle(&case.path).expect_err("a bundle containing a link is not a bundle");
    assert_eq!(err.code(), "not_a_bundle");
    assert!(err.to_string().contains("reparse point") || err.to_string().contains("link"));
}

// ---------------------------------------------------------------------------
// Archives.
// ---------------------------------------------------------------------------

/// ZIP counts sizes in 32 bits and name lengths in 16. Test archives are tiny, so these
/// cannot fail -- but they are written as conversions rather than `as` casts so that a
/// test which one day builds something large fails loudly instead of writing a
/// wrapped-around length into a header and testing nothing.
fn small_u32(value: usize) -> u32 {
    u32::try_from(value).expect("a test archive is far below 4 GiB")
}
fn small_u16(value: usize) -> u16 {
    u16::try_from(value).expect("a test archive name is far below 64 KiB")
}

/// A minimal store-only ZIP writer. Tests only: Ordinata reads archives and never
/// writes one.
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, content) in files {
        let crc = crc32(content);
        let offset = small_u32(out.len());

        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&20_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes()); // stored
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&small_u32(content.len()).to_le_bytes());
        out.extend_from_slice(&small_u32(content.len()).to_le_bytes());
        out.extend_from_slice(&small_u16(name.len()).to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(content);

        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&small_u32(content.len()).to_le_bytes());
        central.extend_from_slice(&small_u32(content.len()).to_le_bytes());
        central.extend_from_slice(&small_u16(name.len()).to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }

    let directory_offset = small_u32(out.len());
    let directory_size = small_u32(central.len());
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&small_u16(files.len()).to_le_bytes());
    out.extend_from_slice(&small_u16(files.len()).to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out
}

/// Every member of a real bundle, keyed by path, for feeding to `zip`.
fn members_of(bundle: &Bundle) -> BTreeMap<String, Vec<u8>> {
    bundle
        .members()
        .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
        .collect()
}

fn zip_of(members: &BTreeMap<String, Vec<u8>>, prefix: &str) -> Vec<u8> {
    let named: Vec<(String, Vec<u8>)> = members
        .iter()
        .map(|(path, bytes)| (format!("{prefix}{path}"), bytes.clone()))
        .collect();
    let borrowed: Vec<(&str, &[u8])> = named
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    zip(&borrowed)
}

#[test]
fn a_zipped_bundle_verifies_with_or_without_a_wrapping_directory() {
    let case = written();
    let members = members_of(&case.bundle);
    let directory = tempfile::tempdir().unwrap();

    for (label, prefix) in [("flat", ""), ("wrapped", "my-bundle/")] {
        let path = directory.path().join(format!("{label}.zip"));
        std::fs::write(&path, zip_of(&members, prefix)).unwrap();

        let report = verify_bundle(&path).expect("a zipped bundle should read");
        assert!(
            report.passed(),
            "the {label} archive must verify: {:?}",
            failures(&report)
        );
        assert_eq!(report.kind, "archive");
    }
}

#[test]
fn an_archive_whose_common_prefix_traverses_is_not_stripped_into_innocence() {
    // Every entry is `../<member>`, so the "common prefix" is `..`. Stripping it
    // without looking would turn a set of traversing names into a set of clean ones
    // and hand them straight to the verifier. The prefix is held to the same rule as
    // any other component, so it is not stripped, and the names are refused as written.
    let case = written();
    let members = members_of(&case.bundle);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hostile.zip");
    std::fs::write(&path, zip_of(&members, "../")).unwrap();

    let err = verify_bundle(&path).expect_err("a traversing archive is not a bundle");
    assert_eq!(err.code(), "unsafe_checksum_path", "{err}");
    assert_eq!(err.exit_code(), 5);
}

#[test]
fn an_archive_with_two_names_that_are_one_file_on_windows_is_refused() {
    // `manifest.json` and `Manifest.json` are two ZIP entries and one file on any
    // case-insensitive filesystem. An archive holding both means different things on
    // different machines, and a bundle does not get to do that.
    let bytes = zip(&[
        ("manifest.json", b"{}".as_slice()),
        ("Manifest.json", b"{\"different\": true}".as_slice()),
    ]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("collision.zip");
    std::fs::write(&path, bytes).unwrap();

    let err = verify_bundle(&path).expect_err("a case collision is not decidable");
    assert_eq!(err.code(), "archive_unreadable");
    assert!(err.to_string().contains("case"), "{err}");
}

#[test]
fn an_archive_that_declares_more_than_ordinata_will_hold_is_refused_before_decompression() {
    // The bomb that a per-entry budget does not stop.
    //
    // Three entries, each declaring 250 MiB. Every one of them is *individually* under
    // the 256 MiB per-member cap, so a reader that checks the remaining budget as it
    // arrives at each entry waves the first two through -- half a gigabyte of memory,
    // already spent -- and only objects when it reaches the third. Adding the declared
    // sizes up first costs nothing and refuses the whole archive while it is still a
    // few hundred bytes on disk.
    //
    // The payloads here are one byte each. Only the central directory's declared
    // uncompressed sizes are enormous, which is exactly the shape of the real thing.
    let mut bytes = zip(&[
        ("fit.csv", b"a".as_slice()),
        ("parameters.csv", b"b".as_slice()),
        ("manifest.json", b"c".as_slice()),
    ]);

    let claim: u32 = 250 << 20; // under the 256 MiB per-member cap, on purpose
    let signature = 0x0201_4b50_u32.to_le_bytes();
    let mut rewritten = 0;
    let mut at = 0;
    while at + 46 <= bytes.len() {
        if bytes[at..at + 4] == signature {
            bytes[at + 24..at + 28].copy_from_slice(&claim.to_le_bytes());
            rewritten += 1;
            at += 46;
        } else {
            at += 1;
        }
    }
    assert_eq!(
        rewritten, 3,
        "all three central directory entries must be rewritten"
    );
    // 3 x 250 MiB = 750 MiB declared, over the 512 MiB whole-bundle budget, while no
    // single entry is over the per-member cap.
    assert!(3 * u64::from(claim) > (512 << 20));
    assert!(u64::from(claim) < (256 << 20));

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bomb.zip");
    std::fs::write(&path, bytes).unwrap();

    let err = verify_bundle(&path).expect_err("a declared expansion over the budget is refused");
    assert_eq!(err.code(), "archive_unreadable");
    assert!(
        err.to_string().contains("nothing was decompressed"),
        "the refusal must make clear that the bomb never went off: {err}"
    );
}

#[test]
fn an_archive_larger_than_the_cap_is_refused_without_being_read_whole() {
    // The cap is applied to the *read*, not after it. A file of any size beyond the
    // limit costs `limit + 1` bytes of memory to refuse, not its own size. Before this
    // was fixed, `verify` allocated the entire file first and consulted the limit
    // afterwards, which is the wrong order for a program whose entire job is being
    // pointed at files it does not trust.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("huge.zip");
    let file = std::fs::File::create(&path).unwrap();
    // Sparse on NTFS and on every filesystem that matters: this costs no disk.
    file.set_len((256 << 20) + 4096).unwrap();
    drop(file);

    let err = verify_bundle(&path).expect_err("an oversized archive is refused");
    assert_eq!(err.code(), "archive_unreadable");
    assert!(
        err.to_string().contains("nothing was decompressed"),
        "{err}"
    );
}

#[test]
fn a_file_that_is_not_an_archive_at_all_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.txt");
    std::fs::write(&path, "this is not a bundle and not a zip").unwrap();

    let err = verify_bundle(&path).expect_err("a text file is not a bundle");
    assert_eq!(err.code(), "not_a_bundle");
}

// ---------------------------------------------------------------------------
// The content hash covers what it says it covers.
// ---------------------------------------------------------------------------

#[test]
fn every_member_the_content_hash_covers_actually_moves_it() {
    // If a member were listed in CONTENT_MEMBERS but the hash did not really depend on
    // it, the deepest verification layer would be quietly weaker than it claims. Change
    // each covered member in turn and require the recomputed hash to notice.
    for member in CONTENT_MEMBERS {
        let case = written();
        let mut bytes = read_member(&case.path, member);
        bytes.extend_from_slice(b"\n");
        write_member(&case.path, member, &bytes);

        // Repair everything shallower, so that the only check left to fail is the deep
        // one. If it passes, the hash was not really covering this member.
        let mut manifest: Value =
            serde_json::from_slice(&read_member(&case.path, MANIFEST)).unwrap();
        for entry in manifest["payload"].as_array_mut().unwrap() {
            if entry["path"] == member {
                entry["sha256"] = Value::String(sha256_hex(&bytes));
                entry["bytes"] = Value::from(bytes.len());
            }
        }
        write_member(
            &case.path,
            MANIFEST,
            serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
        );
        recompute_checksums(&case.path);

        let report = verify_bundle(&case.path).unwrap();
        assert!(
            failures(&report).contains(&"result_content_hash"),
            "{member} is listed as covered by the result content hash, but changing it did not move the hash: {:?}",
            failures(&report)
        );
    }
}
