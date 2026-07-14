//! Offline bundle verification.
//!
//! # What a passing verification proves
//!
//! That the bundle is internally consistent and has not changed since it was written.
//! Every member hashes to what `SHA256SUMS.txt` records, the manifest describes
//! exactly the members that are present, and the numbers hash to the result content
//! hash the manifest carries. Nothing was added, removed, or edited.
//!
//! # What it does not prove
//!
//! **It is not a signature.** Anyone with a copy of Ordinata can produce a bundle
//! that verifies. Verification tells you the bundle is intact; it does not tell you
//! who made it or that they were honest. Signing is out of scope for v0.1, and
//! pretending otherwise would be worse than not having it.
//!
//! **It does not re-run the solver.** This is a deliberate choice, not an omission.
//! Floating-point results are only bit-reproducible for a given binary on a given
//! target. A verifier that recomputed the fit would report "tampering" whenever a
//! colleague checked a bundle on a different machine, which is exactly the wrong
//! answer to give. Verification therefore checks integrity, and reproducibility is
//! claimed only where it actually holds: the same build, on the same target, given
//! the same inputs.
//!
//! # Layers
//!
//! The checks are ordered so that the cheapest and most local failures are reported
//! first, and so that each one narrows what a tamperer would have had to do:
//!
//! 1. edit a byte, and its member fails its checksum;
//! 2. fix the checksum too, and the manifest's payload hash disagrees;
//! 3. fix that as well, and the result content hash disagrees;
//! 4. fix everything, and you have re-derived the bundle. See "not a signature".

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::archive;
use crate::bundle::{CHECKSUMS, CONTENT_MEMBERS, MANIFEST, PAYLOAD_MEMBERS};
use crate::canonical;
use crate::error::{BundleError, IoError, Result};
use crate::paths;
use crate::spec;

/// One verification step and how it went.
#[derive(Debug, Clone)]
pub struct Check {
    /// Stable name of the check.
    pub name: &'static str,
    /// `pass`, `fail`, or `skipped`.
    pub status: &'static str,
    /// What was found.
    pub detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            status: "pass",
            detail: detail.into(),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            status: "fail",
            detail: detail.into(),
        }
    }
    fn skipped(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            status: "skipped",
            detail: detail.into(),
        }
    }
}

/// What the bundle says about itself, once it is readable enough to say anything.
#[derive(Debug, Clone)]
pub struct BundleSummary {
    /// The tool version that wrote it.
    pub tool_version: String,
    /// The numerical model version.
    pub model_version: u64,
    /// The recorded observation time.
    pub observed_at: String,
    /// The project title.
    pub title: String,
    /// Whether the bundle declares itself synthetic.
    pub synthetic: bool,
    /// The result content hash the manifest records.
    pub result_content_hash: String,
}

/// The outcome of verifying one bundle.
#[derive(Debug, Clone)]
pub struct VerificationReport {
    /// What was verified.
    pub source: String,
    /// `directory` or `archive`.
    pub kind: &'static str,
    /// `ok` when every check passed, `failed` otherwise.
    pub status: &'static str,
    /// How many members were found.
    pub members: usize,
    /// Every check, in the order it ran.
    pub checks: Vec<Check>,
    /// What the bundle says about itself. Absent when it was too broken to say.
    pub summary: Option<BundleSummary>,
}

impl VerificationReport {
    /// Did every check pass?
    pub fn passed(&self) -> bool {
        self.status == "ok"
    }

    /// Every check that failed.
    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| c.status == "fail")
    }
}

/// Is this directory entry a reparse point of any kind?
///
/// On Windows a *junction* is not a symbolic link. `FileType::is_symlink` covers the
/// two reparse tags the standard library knows about, and a junction created with
/// `mklink /J` happens to be one of them -- but the reparse mechanism is open-ended,
/// and a bundle member that is any kind of reparse point is a bundle member that does
/// not hold the bytes it appears to hold. Reading the raw attribute bit refuses the
/// whole class, including the tags that have not been invented yet, rather than the
/// two the standard library currently names.
///
/// Everywhere else this is exactly `is_symlink`.
#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Read a bundle directory into memory, refusing anything that is not a plain file
/// with a safe relative name.
fn read_directory(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut members = BTreeMap::new();
    let mut total: u64 = 0;
    collect(root, root, &mut members, &mut total, 0)?;
    Ok(members)
}

fn collect(
    root: &Path,
    directory: &Path,
    members: &mut BTreeMap<String, Vec<u8>>,
    total: &mut u64,
    depth: usize,
) -> Result<()> {
    if depth > 8 {
        return Err(BundleError::NotABundle {
            path: crate::error::path_string(root),
            detail: "it nests directories more deeply than a bundle ever does".into(),
        }
        .into());
    }
    let entries = std::fs::read_dir(directory).map_err(|e| IoError::read(directory, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| IoError::read(directory, e))?;
        let path = entry.path();

        // `symlink_metadata` does not follow the link, so a link inside a bundle is
        // seen for what it is and refused rather than followed somewhere else. This is
        // checked *before* `is_dir`, because a junction reports itself as a directory
        // and recursing into one would walk straight out of the bundle.
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| IoError::read(&path, e))?;
        if is_reparse_point(&metadata) {
            return Err(BundleError::NotABundle {
                path: crate::error::path_string(&path),
                detail: "it contains a symbolic link, junction, or other reparse point; a bundle holds only regular files, and a member that redirects somewhere else does not hold the bytes it claims to".into(),
            }
            .into());
        }

        if metadata.is_dir() {
            collect(root, &path, members, total, depth + 1)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(BundleError::NotABundle {
                path: crate::error::path_string(&path),
                detail: "it contains something that is not a regular file".into(),
            }
            .into());
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|_| BundleError::NotABundle {
                path: crate::error::path_string(&path),
                detail: "it sits outside the bundle it was found in".into(),
            })?
            .to_string_lossy()
            .replace('\\', "/");

        if let Some(detail) = paths::member_path_rule_broken(&relative) {
            return Err(BundleError::UnsafeChecksumPath {
                path: relative,
                detail,
            }
            .into());
        }
        if members.len() >= spec::MAX_BUNDLE_MEMBERS {
            return Err(BundleError::NotABundle {
                path: crate::error::path_string(root),
                detail: format!(
                    "it has more than the {} members Ordinata will read",
                    spec::MAX_BUNDLE_MEMBERS
                ),
            }
            .into());
        }
        if metadata.len() > spec::MAX_BUNDLE_MEMBER_BYTES {
            return Err(BundleError::NotABundle {
                path: relative,
                detail: "a member is larger than Ordinata will read".into(),
            }
            .into());
        }

        let bytes = std::fs::read(&path).map_err(|e| IoError::read(&path, e))?;
        *total += bytes.len() as u64;
        if *total > spec::MAX_BUNDLE_BYTES {
            return Err(BundleError::NotABundle {
                path: crate::error::path_string(root),
                detail: "it is larger than Ordinata will read".into(),
            }
            .into());
        }
        members.insert(relative, bytes);
    }
    Ok(())
}

/// Parse `SHA256SUMS.txt`. Each line is a 64-character hex digest, two spaces, and a
/// safe relative path. Anything else is a refusal, including a path that would send
/// the verifier somewhere it should not go.
fn parse_checksums(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| BundleError::MalformedMember {
        member: CHECKSUMS.into(),
        detail: "it is not valid UTF-8".into(),
    })?;

    let mut out = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.is_empty() {
            continue;
        }
        let Some((digest, path)) = line.split_once("  ") else {
            return Err(BundleError::ChecksumLine { line: number }.into());
        };
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(BundleError::ChecksumLine { line: number }.into());
        }
        if digest.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(BundleError::ChecksumLine { line: number }.into());
        }
        if let Some(detail) = paths::member_path_rule_broken(path) {
            return Err(BundleError::UnsafeChecksumPath {
                path: crate::text::for_message(path),
                detail,
            }
            .into());
        }
        if out.insert(path.to_string(), digest.to_string()).is_some() {
            return Err(BundleError::UnsafeChecksumPath {
                path: path.to_string(),
                detail: "it is listed twice, so which digest applies is not decidable".into(),
            }
            .into());
        }
    }
    Ok(out)
}

/// Read an archive file into memory, enforcing the size cap on the *read* rather than
/// after it.
///
/// The obvious `std::fs::read(path)` allocates the whole file and only then asks how
/// big it was. A verifier is pointed at files it does not trust by definition -- that
/// is its entire job -- so it does not get to allocate a 40 GiB file in order to
/// discover that it is over the 256 MiB limit. The cap goes on the reader.
///
/// The size the filesystem reports is used only to pick a starting capacity, never as
/// the decision: a file is free to grow between the `stat` and the `read`, and
/// `take(limit + 1)` is a promise that holds either way.
fn read_archive_bytes(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).map_err(|e| IoError::read(path, e))?;
    let metadata = file.metadata().map_err(|e| IoError::read(path, e))?;
    if is_reparse_point(&metadata) || !metadata.is_file() {
        return Err(BundleError::NotABundle {
            path: crate::error::path_string(path),
            detail: "it is not a regular file".into(),
        }
        .into());
    }

    let limit = spec::MAX_ARCHIVE_BYTES;
    let capacity = usize::try_from(metadata.len().min(limit).saturating_add(1)).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| IoError::read(path, e))?;

    if bytes.len() as u64 > limit {
        return Err(BundleError::Archive {
            detail: format!(
                "it is larger than the {limit} bytes Ordinata will open; nothing was decompressed and nothing beyond the limit was read into memory"
            ),
        }
        .into());
    }
    if !archive::looks_like_archive(&bytes) {
        return Err(BundleError::NotABundle {
            path: crate::error::path_string(path),
            detail: "it is neither a directory nor a ZIP archive".into(),
        }
        .into());
    }
    Ok(bytes)
}

/// Every member of a bundle, by path.
type Members = BTreeMap<String, Vec<u8>>;

/// Join a list of problems into one detail string, or `None` when there are none.
fn joined(problems: &[String]) -> Option<String> {
    (!problems.is_empty()).then(|| problems.join("; "))
}

/// A bundle of this schema has exactly these files: no more, no fewer, no others.
///
/// Checking the set up front means an extra file smuggled in beside the real ones is
/// caught by name here, rather than only by whichever downstream check happens to trip
/// over it. It also lets every check below talk about "the members" without hedging
/// about what might be among them.
fn check_bundle_shape(members: &Members, checks: &mut Vec<Check>) {
    let expected: BTreeSet<&str> = PAYLOAD_MEMBERS
        .iter()
        .copied()
        .chain([MANIFEST, CHECKSUMS])
        .collect();
    let found: BTreeSet<&str> = members.keys().map(String::as_str).collect();

    let missing: Vec<&str> = expected.difference(&found).copied().collect();
    let extra: Vec<&str> = found.difference(&expected).copied().collect();

    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("missing: {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        problems.push(format!("not part of a bundle: {}", extra.join(", ")));
    }

    checks.push(match joined(&problems) {
        Some(detail) => Check::fail("bundle_shape", detail),
        None => Check::pass(
            "bundle_shape",
            format!("exactly the {} members a bundle has", expected.len()),
        ),
    });
}

/// `SHA256SUMS.txt` names every member that is here, and every member it names is here.
fn check_checksum_coverage(
    members: &Members,
    recorded: &BTreeMap<String, String>,
    checks: &mut Vec<Check>,
) {
    let present: Vec<&str> = members
        .keys()
        .map(String::as_str)
        .filter(|k| *k != CHECKSUMS)
        .collect();

    let missing: Vec<&str> = recorded
        .keys()
        .map(String::as_str)
        .filter(|path| !members.contains_key(*path))
        .collect();
    let mut unexpected: Vec<&str> = present
        .iter()
        .copied()
        .filter(|path| !recorded.contains_key(*path))
        .collect();
    // The checksum file cannot record its own digest: doing so would be a fixed point
    // it could never reach. A bundle whose inventory lists itself is malformed.
    if recorded.contains_key(CHECKSUMS) {
        unexpected.push(CHECKSUMS);
    }

    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("named but not present: {}", missing.join(", ")));
    }
    if !unexpected.is_empty() {
        problems.push(format!("present but not named: {}", unexpected.join(", ")));
    }

    checks.push(match joined(&problems) {
        Some(detail) => Check::fail("checksum_coverage", detail),
        None => Check::pass(
            "checksum_coverage",
            format!(
                "every one of the {} members is accounted for",
                present.len()
            ),
        ),
    });
}

/// The first rung of the ladder: every member hashes to the digest recorded for it.
fn check_member_checksums(
    members: &Members,
    recorded: &BTreeMap<String, String>,
    checks: &mut Vec<Check>,
) {
    let bad: Vec<String> = recorded
        .iter()
        .filter_map(|(path, digest)| {
            let bytes = members.get(path)?;
            let actual = canonical::sha256_hex(bytes);
            (&actual != digest).then(|| format!("{path} (recorded {digest}, found {actual})"))
        })
        .collect();

    checks.push(if bad.is_empty() {
        Check::pass(
            "member_checksums",
            "every member hashes to its recorded value",
        )
    } else {
        Check::fail(
            "member_checksums",
            format!("{} member(s) do not match: {}", bad.len(), bad.join("; ")),
        )
    });
}

/// What the bundle says about itself.
fn summarize(manifest: &Value) -> BundleSummary {
    let text = |pointer: &str, fallback: &str| {
        manifest
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string()
    };
    BundleSummary {
        tool_version: text("/tool/version", "unknown"),
        model_version: manifest
            .pointer("/solver/modelVersion")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        observed_at: text("/observedAt", "unknown"),
        title: text("/project/title", ""),
        synthetic: manifest
            .get("synthetic")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        result_content_hash: text("/resultContent/sha256", ""),
    }
}

/// The second rung: the manifest describes exactly this bundle, one entry per member.
///
/// Three separate things have to be true, and each is checked rather than inferred from
/// the others: the payload list is *exactly* the members a bundle of this schema has, no
/// member is described twice, and each description matches the bytes. Checking only the
/// third would let a manifest drop an inconvenient member from its list entirely;
/// checking only the first two would let it describe the right names with the wrong
/// hashes.
fn check_manifest_payload(members: &Members, manifest: &Value, checks: &mut Vec<Check>) {
    let Some(payload) = manifest.get("payload").and_then(Value::as_array) else {
        checks.push(Check::fail(
            "manifest_payload",
            "the manifest has no payload list",
        ));
        return;
    };

    let mut problems = Vec::new();
    let mut declared: BTreeMap<&str, &Value> = BTreeMap::new();

    for entry in payload {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            problems.push("a payload entry has no path".to_string());
            continue;
        };
        if declared.insert(path, entry).is_some() {
            problems.push(format!(
                "{path} is described twice, so which description applies is not decidable"
            ));
        }
    }

    let expected: Vec<&str> = PAYLOAD_MEMBERS.to_vec();
    let found: Vec<&str> = declared.keys().copied().collect();
    if found != expected {
        problems.push(format!(
            "the payload list is {found:?}, but a schema-{} bundle describes exactly {expected:?}",
            spec::BUNDLE_SCHEMA_VERSION
        ));
    }

    for (path, entry) in &declared {
        let Some(bytes) = members.get(*path) else {
            problems.push(format!("{path} is in the manifest but not in the bundle"));
            continue;
        };
        match entry.get("sha256").and_then(Value::as_str) {
            Some(expected) => {
                let actual = canonical::sha256_hex(bytes);
                if expected != actual {
                    problems.push(format!(
                        "{path} hashes to {actual}, but the manifest records {expected}"
                    ));
                }
            }
            None => problems.push(format!("{path} is described without a sha256")),
        }
        match entry.get("bytes").and_then(Value::as_u64) {
            Some(size) if size == bytes.len() as u64 => {}
            Some(size) => problems.push(format!(
                "{path} is {} bytes, but the manifest records {size}",
                bytes.len()
            )),
            None => problems.push(format!("{path} is described without a byte count")),
        }
    }

    // The other direction: a file sitting in the bundle that the manifest says nothing
    // about. The manifest cannot describe itself, and the checksum file is the inventory
    // rather than part of it, so those two are expected to be absent from the list.
    for path in members.keys().map(String::as_str) {
        if path != MANIFEST && path != CHECKSUMS && !declared.contains_key(path) {
            problems.push(format!(
                "{path} is in the bundle but the manifest does not describe it"
            ));
        }
    }

    checks.push(match joined(&problems) {
        Some(detail) => Check::fail("manifest_payload", detail),
        None => Check::pass(
            "manifest_payload",
            format!(
                "the manifest describes all {} payload members exactly once each",
                declared.len()
            ),
        ),
    });
}

/// The deepest rung: the numbers hash to what the manifest says they do.
///
/// This is the check that fails when the bytes were changed and every checksum was
/// recomputed to match.
///
/// # Errors
///
/// Propagates an internal error if a content member cannot be hashed, which cannot
/// happen once the completeness check above has passed.
fn check_result_content(
    members: &Members,
    manifest: &Value,
    checks: &mut Vec<Check>,
) -> Result<()> {
    let declared: Vec<&str> = manifest
        .pointer("/resultContent/members")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if declared != CONTENT_MEMBERS.to_vec() {
        checks.push(Check::fail(
            "result_content_members",
            format!(
                "the manifest says the result content covers {declared:?}, but a schema-{} bundle covers {CONTENT_MEMBERS:?}",
                spec::BUNDLE_SCHEMA_VERSION
            ),
        ));
        checks.push(Check::skipped(
            "result_content_hash",
            "the covered members are not the ones this schema defines",
        ));
        return Ok(());
    }
    checks.push(Check::pass(
        "result_content_members",
        "the result content covers the members this schema defines",
    ));

    if !CONTENT_MEMBERS
        .iter()
        .all(|member| members.contains_key(*member))
    {
        checks.push(Check::skipped(
            "result_content_hash",
            "a member the hash covers is missing",
        ));
        return Ok(());
    }

    let recomputed = crate::bundle::result_content_hash(members)?;
    let expected = manifest
        .pointer("/resultContent/sha256")
        .and_then(Value::as_str)
        .unwrap_or("");
    checks.push(if recomputed == expected {
        Check::pass(
            "result_content_hash",
            format!("the numbers hash to {recomputed}"),
        )
    } else {
        Check::fail(
            "result_content_hash",
            format!(
                "the numbers hash to {recomputed}, but the manifest records {expected}; the content of this bundle is not the content it claims"
            ),
        )
    });
    Ok(())
}

/// Verify a bundle directory or archive.
///
/// # Errors
///
/// Returns [`BundleError::NotFound`] if there is nothing at `path`, and
/// [`BundleError::Archive`] if an archive cannot be read at all. A bundle that reads
/// but does not check out is *not* an error: it comes back as a report whose status
/// is `failed` and whose checks say precisely what is wrong.
pub fn verify_bundle(path: &Path) -> Result<VerificationReport> {
    let source = crate::error::path_string(path);
    if !path.exists() {
        return Err(BundleError::NotFound { path: source }.into());
    }

    let (kind, members) = if path.is_dir() {
        ("directory", read_directory(path)?)
    } else {
        ("archive", archive::read(&read_archive_bytes(path)?)?)
    };

    let mut checks = Vec::new();
    let count = members.len();
    let done = |checks, summary| Ok(finish(source.clone(), kind, count, checks, summary));

    checks.push(Check::pass(
        "members_readable",
        format!("read {count} members"),
    ));
    check_bundle_shape(&members, &mut checks);

    // --- The inventory ------------------------------------------------------------
    let Some(checksum_bytes) = members.get(CHECKSUMS) else {
        checks.push(Check::fail(
            "checksums_present",
            format!("{CHECKSUMS} is missing, so the bundle cannot account for its own contents"),
        ));
        return done(checks, None);
    };
    checks.push(Check::pass(
        "checksums_present",
        format!("{CHECKSUMS} is present"),
    ));

    let recorded = parse_checksums(checksum_bytes)?;
    checks.push(Check::pass(
        "checksums_wellformed",
        format!("{} entries, every path safe and unique", recorded.len()),
    ));

    check_checksum_coverage(&members, &recorded, &mut checks);
    check_member_checksums(&members, &recorded, &mut checks);

    // --- The manifest ---------------------------------------------------------------
    let Some(manifest_bytes) = members.get(MANIFEST) else {
        checks.push(Check::fail(
            "manifest_present",
            format!("{MANIFEST} is missing"),
        ));
        return done(checks, None);
    };
    // Strict: a manifest that names a key twice is refused rather than read in whichever
    // direction the parser leans. A bundle whose manifest means two things is a bundle
    // that can be shown to say whatever its holder needs it to say.
    let manifest: Value = match canonical::parse_strict(manifest_bytes) {
        Ok(value) => value,
        Err(e) => {
            checks.push(Check::fail(
                "manifest_present",
                format!("{MANIFEST} is not usable JSON: {e}"),
            ));
            return done(checks, None);
        }
    };
    checks.push(Check::pass("manifest_present", "the manifest parses"));

    let summary = Some(summarize(&manifest));
    check_manifest_payload(&members, &manifest, &mut checks);
    check_result_content(&members, &manifest, &mut checks)?;

    done(checks, summary)
}

fn finish(
    source: String,
    kind: &'static str,
    members: usize,
    checks: Vec<Check>,
    summary: Option<BundleSummary>,
) -> VerificationReport {
    let status = if checks.iter().any(|c| c.status == "fail") {
        "failed"
    } else {
        "ok"
    };
    VerificationReport {
        source,
        kind,
        status,
        members,
        checks,
        summary,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_checksum_line_must_have_the_exact_shape() {
        // Good.
        let good = format!("{}  manifest.json\n", "a".repeat(64));
        assert_eq!(parse_checksums(good.as_bytes()).unwrap().len(), 1);

        // Bad in every way a line can be bad.
        for bad in [
            format!("{} manifest.json\n", "a".repeat(64)), // one space
            format!("{}  manifest.json\n", "a".repeat(63)), // short digest
            format!("{}  manifest.json\n", "A".repeat(64)), // uppercase
            format!("{}  manifest.json\n", "z".repeat(64)), // not hex
            "no digest at all\n".to_string(),
        ] {
            assert!(
                parse_checksums(bad.as_bytes()).is_err(),
                "should refuse: {bad:?}"
            );
        }
    }

    #[test]
    fn a_checksum_path_attack_is_refused() {
        for hostile in [
            "../../../etc/passwd",
            "/etc/passwd",
            "C:/windows/system32/drivers/etc/hosts",
            "a/../../b",
        ] {
            let line = format!("{}  {hostile}\n", "a".repeat(64));
            let err = parse_checksums(line.as_bytes()).unwrap_err();
            assert_eq!(err.code(), "unsafe_checksum_path", "{hostile}");
            assert_eq!(err.exit_code(), 5);
        }
    }

    #[test]
    fn a_path_listed_twice_is_refused() {
        let line = format!(
            "{}  manifest.json\n{}  manifest.json\n",
            "a".repeat(64),
            "b".repeat(64)
        );
        assert!(parse_checksums(line.as_bytes()).is_err());
    }

    #[test]
    fn a_missing_bundle_is_reported_as_such() {
        let err = verify_bundle(Path::new("no-such-bundle-anywhere")).unwrap_err();
        assert_eq!(err.code(), "bundle_not_found");
        assert_eq!(err.exit_code(), 5);
    }
}
