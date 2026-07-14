//! A deliberately small ZIP reader.
//!
//! `verify` accepts a bundle directory or an archive, because a bundle that travels
//! usually travels zipped, and the person who zipped it used whatever their operating
//! system gave them. So this has to read ordinary archives, not just ones Ordinata
//! made.
//!
//! It is written here, rather than pulled in, because the attack surface of a ZIP
//! reader is the whole point and it should be small enough to read in one sitting.
//! Everything it will not do is as important as what it will:
//!
//! * only `store` and `deflate`; every other method is refused by name;
//! * no encryption, no ZIP64, no multi-disk, no self-extracting prefix;
//! * the central directory is authoritative, so a local header that disagrees with
//!   it cannot smuggle in a second interpretation of the same file;
//! * every entry's decompressed size is capped *before* it is decompressed, so a zip
//!   bomb runs into the limit rather than the memory;
//! * every entry name is checked against [`crate::paths::member_path_rule_broken`]
//!   before anything is opened, so an entry called `../../.ssh/authorized_keys` is a
//!   refusal and not a write;
//! * every entry's CRC-32 is verified against the value the archive itself declares.

use std::collections::BTreeMap;

use crate::error::{BundleError, Result};
use crate::paths;
use crate::spec;

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;

const METHOD_STORE: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// A marker that says a file is probably a ZIP.
pub(crate) fn looks_like_archive(bytes: &[u8]) -> bool {
    bytes.starts_with(&LOCAL_SIGNATURE.to_le_bytes()) || bytes.starts_with(b"PK\x05\x06")
}

fn malformed(detail: impl Into<String>) -> crate::Error {
    BundleError::Archive {
        detail: detail.into(),
    }
    .into()
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16> {
    let slice = bytes
        .get(at..at + 2)
        .ok_or_else(|| malformed("it ends in the middle of a header"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32> {
    let slice = bytes
        .get(at..at + 4)
        .ok_or_else(|| malformed("it ends in the middle of a header"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// CRC-32, the reflected IEEE polynomial that ZIP uses.
///
/// Computed bit by bit. There is no table to get wrong, and at bundle sizes the
/// difference is not measurable.
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

/// One entry from the central directory.
struct CentralEntry {
    name: String,
    method: u16,
    crc: u32,
    compressed_size: usize,
    uncompressed_size: usize,
    local_offset: usize,
}

/// Find the end-of-central-directory record. It sits at the end, but a ZIP may carry
/// a trailing comment, so the signature is searched for backwards over the region a
/// comment could occupy.
fn find_eocd(bytes: &[u8]) -> Result<usize> {
    const EOCD_MIN: usize = 22;
    const MAX_COMMENT: usize = 65_535;
    if bytes.len() < EOCD_MIN {
        return Err(malformed("it is too short to be a ZIP archive"));
    }
    let earliest = bytes.len().saturating_sub(EOCD_MIN + MAX_COMMENT);
    let signature = EOCD_SIGNATURE.to_le_bytes();
    let mut at = bytes.len() - EOCD_MIN;
    loop {
        if bytes.get(at..at + 4) == Some(&signature[..]) {
            return Ok(at);
        }
        if at == earliest {
            return Err(malformed(
                "it has no end-of-central-directory record, so it is not a ZIP archive",
            ));
        }
        at -= 1;
    }
}

fn read_central_directory(bytes: &[u8]) -> Result<Vec<CentralEntry>> {
    let eocd = find_eocd(bytes)?;

    let entries_here = read_u16(bytes, eocd + 8)?;
    let entries_total = read_u16(bytes, eocd + 10)?;
    let directory_size = read_u32(bytes, eocd + 12)? as usize;
    let directory_offset = read_u32(bytes, eocd + 16)? as usize;

    if entries_total == u16::MAX || directory_offset == u32::MAX as usize {
        return Err(malformed(
            "it uses ZIP64 extensions, which Ordinata does not read; an evidence bundle is far too small to need them",
        ));
    }
    if entries_here != entries_total {
        return Err(malformed(
            "it is split across several disks, which Ordinata does not read",
        ));
    }
    if entries_total as usize > spec::MAX_BUNDLE_MEMBERS {
        return Err(malformed(format!(
            "it has {entries_total} entries, more than the {} Ordinata will open",
            spec::MAX_BUNDLE_MEMBERS
        )));
    }

    let end = directory_offset
        .checked_add(directory_size)
        .ok_or_else(|| malformed("its central directory offset overflows"))?;
    if end > bytes.len() {
        return Err(malformed(
            "its central directory runs past the end of the file",
        ));
    }

    let mut entries = Vec::with_capacity(entries_total as usize);
    let mut at = directory_offset;
    for _ in 0..entries_total {
        if read_u32(bytes, at)? != CENTRAL_SIGNATURE {
            return Err(malformed("its central directory is corrupt"));
        }
        let flags = read_u16(bytes, at + 8)?;
        let method = read_u16(bytes, at + 10)?;
        let crc = read_u32(bytes, at + 16)?;
        let compressed_size = read_u32(bytes, at + 20)? as usize;
        let uncompressed_size = read_u32(bytes, at + 24)? as usize;
        let name_length = read_u16(bytes, at + 28)? as usize;
        let extra_length = read_u16(bytes, at + 30)? as usize;
        let comment_length = read_u16(bytes, at + 32)? as usize;
        let local_offset = read_u32(bytes, at + 42)? as usize;

        if flags & 0x0001 != 0 {
            return Err(malformed(
                "it is encrypted, and Ordinata will not ask anyone for a password",
            ));
        }
        if compressed_size == u32::MAX as usize || uncompressed_size == u32::MAX as usize {
            return Err(malformed(
                "it uses ZIP64 sizes, which Ordinata does not read",
            ));
        }
        if uncompressed_size as u64 > spec::MAX_BUNDLE_MEMBER_BYTES {
            return Err(malformed(format!(
                "an entry declares {uncompressed_size} bytes, more than the {} Ordinata will decompress",
                spec::MAX_BUNDLE_MEMBER_BYTES
            )));
        }

        let name_start = at + 46;
        let name_bytes = bytes
            .get(name_start..name_start + name_length)
            .ok_or_else(|| malformed("an entry name runs past the end of the file"))?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| malformed("an entry name is not valid UTF-8"))?
            .to_string();

        entries.push(CentralEntry {
            name,
            method,
            crc,
            compressed_size,
            uncompressed_size,
            local_offset,
        });

        at = name_start
            .checked_add(name_length)
            .and_then(|v| v.checked_add(extra_length))
            .and_then(|v| v.checked_add(comment_length))
            .ok_or_else(|| malformed("its central directory offsets overflow"))?;
    }

    Ok(entries)
}

/// If every file in the archive sits under one common top-level directory, strip it.
///
/// Zipping a bundle directory is the natural thing to do, and it produces entries like
/// `my-bundle/manifest.json`. Refusing those would be pedantry.
///
/// The prefix itself is held to the same rule as any other path component, and that
/// is not a formality. An archive whose every entry is `../manifest.json`,
/// `../fit.csv`, and so on has the perfectly good common prefix `..`; stripping it
/// without looking would turn a set of traversing names into a set of innocent ones
/// and wave the archive straight through the check that exists to catch it. A prefix
/// that is not a plain, safe component is not stripped, so those names stay exactly as
/// hostile as they were written and are refused on their own merits.
fn common_prefix(names: &[String]) -> Option<String> {
    let first = names.first()?;
    let (prefix, _) = first.split_once('/')?;
    if paths::member_path_rule_broken(prefix).is_some() {
        return None;
    }
    if names
        .iter()
        .all(|name| name.starts_with(prefix) && name.as_bytes().get(prefix.len()) == Some(&b'/'))
    {
        Some(prefix.to_string())
    } else {
        None
    }
}

/// Decompress one entry and check it against everything the archive claims about it.
///
/// The local header is skipped over, not trusted: its name and extra field can disagree
/// with the central directory, and only the central directory counts. Trusting both
/// would let one file have two names, which is how a ZIP is made to mean different
/// things to different readers.
///
/// # Errors
///
/// Returns [`BundleError::Archive`] when the header is corrupt, the method is one
/// Ordinata does not read, the data runs off the end of the file, the decompressed size
/// disagrees with the declared size, or the CRC-32 does not match.
fn read_entry(bytes: &[u8], entry: &CentralEntry, name: &str) -> Result<Vec<u8>> {
    if read_u32(bytes, entry.local_offset)? != LOCAL_SIGNATURE {
        return Err(malformed(format!("the local header for {name} is corrupt")));
    }
    let local_name_length = read_u16(bytes, entry.local_offset + 26)? as usize;
    let local_extra_length = read_u16(bytes, entry.local_offset + 28)? as usize;
    let data_start = entry
        .local_offset
        .checked_add(30)
        .and_then(|v| v.checked_add(local_name_length))
        .and_then(|v| v.checked_add(local_extra_length))
        .ok_or_else(|| malformed("a local header offset overflows"))?;
    let data_end = data_start
        .checked_add(entry.compressed_size)
        .ok_or_else(|| malformed("an entry size overflows"))?;
    let data = bytes
        .get(data_start..data_end)
        .ok_or_else(|| malformed(format!("the data for {name} runs past the end of the file")))?;

    let content = match entry.method {
        METHOD_STORE => {
            if entry.compressed_size != entry.uncompressed_size {
                return Err(malformed(format!(
                    "{name} is stored uncompressed but declares two different sizes"
                )));
            }
            data.to_vec()
        }
        // The limit passed here is the entry's *own* declared size, which was capped
        // against the per-member limit and budgeted against the whole-bundle limit
        // before this function was reached. A payload that keeps expanding past what it
        // said it would runs into the limit rather than into the memory.
        METHOD_DEFLATE => {
            miniz_oxide::inflate::decompress_to_vec_with_limit(data, entry.uncompressed_size)
                .map_err(|e| {
                    malformed(format!(
                        "{name} did not decompress within its own declared size: {e:?}"
                    ))
                })?
        }
        other => {
            return Err(malformed(format!(
                "{name} uses compression method {other}, and Ordinata reads only stored (0) and deflated (8) entries"
            )))
        }
    };

    if content.len() != entry.uncompressed_size {
        return Err(malformed(format!(
            "{name} decompressed to {} bytes but declares {}",
            content.len(),
            entry.uncompressed_size
        )));
    }
    let actual = crc32(&content);
    if actual != entry.crc {
        return Err(malformed(format!(
            "{name} fails its own CRC-32: the archive declares {:08x} and the bytes are {actual:08x}",
            entry.crc
        )));
    }
    Ok(content)
}

/// Read every file out of an archive.
///
/// # Errors
///
/// Returns [`BundleError::Archive`] for anything malformed, unsupported, oversized,
/// or unsafely named.
pub(crate) fn read(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    if bytes.len() as u64 > spec::MAX_ARCHIVE_BYTES {
        return Err(malformed(format!(
            "it is {} bytes, more than the {} Ordinata will open",
            bytes.len(),
            spec::MAX_ARCHIVE_BYTES
        )));
    }

    let entries = read_central_directory(bytes)?;

    // Directory entries carry no data and are simply not interesting.
    let files: Vec<&CentralEntry> = entries
        .iter()
        .filter(|entry| !entry.name.ends_with('/'))
        .collect();
    if files.is_empty() {
        return Err(malformed("it contains no files"));
    }

    // Budget before work, and *all* of the budget before *any* of the work.
    //
    // Checking each entry against the remaining budget as the loop reaches it is not
    // enough, and the difference is the whole attack: three entries of 250 MiB each are
    // individually under the per-member cap, so a running check waves the first two
    // through -- decompressing half a gigabyte -- and only refuses when it arrives at
    // the third. The archive's own central directory says up front how much it intends
    // to expand to. Adding that up costs nothing, and it means a bomb is refused while
    // it is still a few kilobytes of compressed data on disk.
    let declared: u64 = files
        .iter()
        .try_fold(0_u64, |sum, entry| {
            sum.checked_add(entry.uncompressed_size as u64)
        })
        .ok_or_else(|| malformed("its declared sizes overflow"))?;
    if declared > spec::MAX_BUNDLE_BYTES {
        return Err(malformed(format!(
            "its entries declare {declared} bytes in total, more than the {} Ordinata will hold; nothing was decompressed",
            spec::MAX_BUNDLE_BYTES
        )));
    }

    let names: Vec<String> = files.iter().map(|entry| entry.name.clone()).collect();
    let prefix = common_prefix(&names);

    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total: u64 = 0;

    for entry in files {
        let name = match &prefix {
            Some(prefix) => entry
                .name
                .get(prefix.len() + 1..)
                .ok_or_else(|| malformed("an entry name is shorter than its own prefix"))?
                .to_string(),
            None => entry.name.clone(),
        };

        // Checked before anything is opened or written.
        if let Some(detail) = paths::member_path_rule_broken(&name) {
            return Err(BundleError::UnsafeChecksumPath {
                path: crate::text::for_message(&entry.name),
                detail,
            }
            .into());
        }
        if out.contains_key(&name) {
            return Err(malformed(format!(
                "it contains {name} twice, so which one is the bundle member is not decidable"
            )));
        }
        // `manifest.json` and `Manifest.json` are two entries in a ZIP and one file on
        // Windows. An archive holding both would verify here and unpack to something
        // else there, and which of the two a reader saw would depend on their operating
        // system. A bundle does not get to mean different things on different machines.
        if let Some(clash) = out
            .keys()
            .find(|existing| existing.eq_ignore_ascii_case(&name))
        {
            return Err(malformed(format!(
                "it contains both {clash} and {name}, which differ only in case; on a case-insensitive filesystem these are one file, so the archive does not have a single meaning"
            )));
        }

        let content = read_entry(bytes, entry, &name)?;

        // The declared size was budgeted for above and the bytes were checked against
        // it, so this can only agree; it is recomputed from the bytes actually held
        // rather than from the number the archive supplied, because the running total
        // must be a fact about memory and not a claim by the file.
        total = total
            .checked_add(content.len() as u64)
            .ok_or_else(|| malformed("the total size overflows"))?;
        if total > spec::MAX_BUNDLE_BYTES {
            return Err(malformed(format!(
                "it expands to more than the {} bytes Ordinata will hold",
                spec::MAX_BUNDLE_BYTES
            )));
        }

        out.insert(name, content);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    /// ZIP counts sizes in 32 bits and name lengths in 16. Test archives are tiny, so
    /// these conversions cannot fail -- but they are written as conversions rather than
    /// as `as` casts so that a test which one day builds something large fails loudly
    /// instead of writing a wrapped-around length into a header and testing nothing.
    fn small_u32(value: usize) -> u32 {
        u32::try_from(value).expect("a test archive is far below 4 GiB")
    }
    fn small_u16(value: usize) -> u16 {
        u16::try_from(value).expect("a test archive name is far below 64 KiB")
    }

    /// A minimal store-only ZIP writer, for tests only. Ordinata never writes archives.
    pub(crate) fn zip(files: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, content) in files {
            let (method, payload) = if deflate {
                (
                    METHOD_DEFLATE,
                    miniz_oxide::deflate::compress_to_vec(content, 6),
                )
            } else {
                (METHOD_STORE, content.to_vec())
            };
            let crc = crc32(content);
            let offset = small_u32(out.len());

            out.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
            out.extend_from_slice(&20_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&small_u32(payload.len()).to_le_bytes());
            out.extend_from_slice(&small_u32(content.len()).to_le_bytes());
            out.extend_from_slice(&small_u16(name.len()).to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&payload);

            central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&small_u32(payload.len()).to_le_bytes());
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
        out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&small_u16(files.len()).to_le_bytes());
        out.extend_from_slice(&small_u16(files.len()).to_le_bytes());
        out.extend_from_slice(&directory_size.to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out
    }

    #[test]
    fn crc32_matches_the_published_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_stored_archive_round_trips() {
        let bytes = zip(
            &[("manifest.json", b"{}"), ("fit.csv", b"a,b\n1,2\n")],
            false,
        );
        assert!(looks_like_archive(&bytes));
        let files = read(&bytes).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files["manifest.json"], b"{}");
        assert_eq!(files["fit.csv"], b"a,b\n1,2\n");
    }

    #[test]
    fn a_deflated_archive_round_trips() {
        let big = "x,y\n".repeat(500);
        let bytes = zip(&[("fit.csv", big.as_bytes())], true);
        let files = read(&bytes).unwrap();
        assert_eq!(files["fit.csv"], big.as_bytes());
    }

    #[test]
    fn a_common_top_level_directory_is_stripped() {
        let bytes = zip(
            &[
                ("my-bundle/manifest.json", b"{}"),
                ("my-bundle/inputs/design.csv", b"id\n"),
            ],
            false,
        );
        let files = read(&bytes).unwrap();
        assert!(files.contains_key("manifest.json"), "{:?}", files.keys());
        assert!(files.contains_key("inputs/design.csv"));
    }

    #[test]
    fn a_traversing_entry_name_is_refused_before_anything_is_opened() {
        for hostile in [
            "../../../etc/passwd",
            "/etc/passwd",
            "a/../../b",
            "C:/windows/x",
        ] {
            let bytes = zip(&[(hostile, b"payload"), ("manifest.json", b"{}")], false);
            let err = read(&bytes).unwrap_err();
            assert_eq!(err.code(), "unsafe_checksum_path", "{hostile}");
            assert_eq!(err.exit_code(), 5);
        }
    }

    #[test]
    fn a_corrupted_entry_fails_its_crc() {
        let mut bytes = zip(&[("fit.csv", b"a,b\n1,2\n")], false);
        // Flip a byte inside the stored payload.
        let position = bytes
            .windows(3)
            .position(|w| w == b"1,2")
            .expect("payload should be present");
        bytes[position] = b'9';
        let err = read(&bytes).unwrap_err();
        assert_eq!(err.code(), "archive_unreadable");
        assert!(err.to_string().contains("CRC-32"), "{err}");
    }

    #[test]
    fn a_truncated_archive_is_refused() {
        let bytes = zip(&[("manifest.json", b"{}")], false);
        let truncated = &bytes[..bytes.len() / 2];
        assert!(read(truncated).is_err());
    }

    #[test]
    fn an_unsupported_compression_method_is_refused_by_name() {
        let mut bytes = zip(&[("manifest.json", b"{}")], false);
        // Rewrite the method in the central directory to bzip2 (12).
        let directory = bytes
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE.to_le_bytes())
            .expect("central directory should be present");
        bytes[directory + 10] = 12;
        let err = read(&bytes).unwrap_err();
        assert!(err.to_string().contains("compression method 12"), "{err}");
    }

    #[test]
    fn an_encrypted_entry_is_refused() {
        let mut bytes = zip(&[("manifest.json", b"{}")], false);
        let directory = bytes
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE.to_le_bytes())
            .expect("central directory should be present");
        bytes[directory + 8] = 1; // general purpose bit 0: encrypted
        let err = read(&bytes).unwrap_err();
        assert!(err.to_string().contains("encrypted"), "{err}");
    }

    #[test]
    fn a_lying_uncompressed_size_is_refused_rather_than_decompressed() {
        // A zip bomb declares a small size and expands to a large one. The limit is
        // applied to the declared size *before* decompression, and the result is
        // checked against it afterwards, so neither the lie nor the expansion works.
        let big = vec![b'a'; 100_000];
        let mut bytes = zip(&[("fit.csv", &big)], true);
        let directory = bytes
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE.to_le_bytes())
            .expect("central directory should be present");
        // Claim it expands to only 10 bytes.
        bytes[directory + 24..directory + 28].copy_from_slice(&10_u32.to_le_bytes());
        let err = read(&bytes).unwrap_err();
        assert_eq!(err.code(), "archive_unreadable");
    }

    #[test]
    fn something_that_is_not_a_zip_is_refused() {
        assert!(read(b"not a zip at all").is_err());
        assert!(!looks_like_archive(b"not a zip at all"));
    }
}
