//! Safe path handling.
//!
//! Every path that reaches the filesystem passes through this module. The rules are
//! strict on purpose:
//!
//! * a path declared inside `project.json` is a forward-slash relative path with no
//!   `.`, `..`, drive letter, or backslash, and it must still resolve *inside* the
//!   project directory after the operating system has followed every link and
//!   reparse point;
//! * an output directory must not exist, must have an existing parent, and must not
//!   overlap any input;
//! * a path named inside a bundle or an archive is checked before it is opened, not
//!   after.
//!
//! The resolve-then-compare order matters. Checking the declared string for `..` is
//! not enough on its own, because a symlink or a Windows junction can point outside
//! the root without any `..` appearing in the text. So the declared string is
//! checked first (cheap, catches the obvious), and then the *canonicalized* result
//! is required to sit beneath the canonicalized root (authoritative, catches links).

use std::path::{Path, PathBuf};

use crate::error::{IoError, PathError, Result};
use crate::spec::{MAX_PATH_BYTES, MAX_PATH_COMPONENTS};

/// Windows treats these as devices no matter which directory they appear in, and
/// opening one can block or interact with hardware. They are refused everywhere,
/// including on platforms where they would be ordinary names, so that a project is
/// portable and a bundle written on Linux cannot become unreadable on Windows.
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Where the project lives, after resolution.
pub(crate) struct ProjectLocation {
    /// The canonical path of `project.json`.
    pub project_file: PathBuf,
    /// The canonical directory that every declared input must sit beneath.
    pub root: PathBuf,
}

/// Is this component a Windows reserved device name, with or without an extension?
fn is_reserved_device(component: &str) -> Option<&'static str> {
    let stem = component.split('.').next().unwrap_or(component);
    RESERVED_DEVICE_NAMES
        .iter()
        .find(|name| stem.eq_ignore_ascii_case(name))
        .copied()
}

/// Check a single path component for the rules that hold everywhere.
fn check_component(component: &str) -> std::result::Result<(), String> {
    if component.is_empty() {
        return Err("it has an empty component, which usually means a doubled separator".into());
    }
    if component == "." || component == ".." {
        return Err(format!("it contains a {component:?} component"));
    }
    if component.len() > 255 {
        return Err("a component is longer than 255 bytes".into());
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err("a component ends with a dot or a space, which Windows silently strips".into());
    }
    for c in component.chars() {
        if c.is_control() {
            return Err("it contains a control character".into());
        }
        if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\\') {
            return Err(format!(
                "it contains {c:?}, which is not portable in a path"
            ));
        }
    }
    Ok(())
}

/// Validate a relative path declared inside `project.json`.
///
/// This checks the *text*. It says nothing about where the path will actually land;
/// [`resolve_declared_input`] is what enforces that.
fn check_declared_relative<'a>(field: &str, declared: &'a str) -> Result<Vec<&'a str>> {
    if declared.is_empty() {
        return Err(PathError::UnacceptablePath {
            path: declared.to_string(),
            detail: "it is empty".into(),
        }
        .into());
    }
    if declared.len() > MAX_PATH_BYTES {
        return Err(PathError::UnacceptablePath {
            path: crate::text::for_message(declared),
            detail: format!("it is longer than {MAX_PATH_BYTES} bytes"),
        }
        .into());
    }
    if declared.contains('\\') {
        return Err(PathError::UnacceptablePath {
            path: declared.to_string(),
            detail: "it contains a backslash; declare inputs with forward slashes so that a project reads the same on every platform".into(),
        }
        .into());
    }
    if declared.starts_with('/') {
        return Err(PathError::NotRelative {
            field: field.to_string(),
            path: declared.to_string(),
        }
        .into());
    }
    // A Windows drive-relative path such as `C:design.csv` has no leading slash but
    // is emphatically not relative to the project.
    if declared.contains(':') {
        return Err(PathError::NotRelative {
            field: field.to_string(),
            path: declared.to_string(),
        }
        .into());
    }

    let components: Vec<&str> = declared.split('/').collect();
    if components.len() > MAX_PATH_COMPONENTS {
        return Err(PathError::UnacceptablePath {
            path: declared.to_string(),
            detail: format!("it has more than {MAX_PATH_COMPONENTS} components"),
        }
        .into());
    }
    for component in &components {
        if *component == "." || *component == ".." {
            return Err(PathError::Traversal {
                field: field.to_string(),
                path: declared.to_string(),
                component: (*component).to_string(),
            }
            .into());
        }
        if let Some(name) = is_reserved_device(component) {
            return Err(PathError::ReservedDeviceName {
                path: declared.to_string(),
                name: name.to_string(),
            }
            .into());
        }
        if let Err(detail) = check_component(component) {
            return Err(PathError::UnacceptablePath {
                path: declared.to_string(),
                detail,
            }
            .into());
        }
    }
    Ok(components)
}

/// Resolve `project.json` and the directory every input must live beneath.
///
/// # Errors
///
/// Returns [`IoError::Read`] if the file cannot be reached and [`PathError::NotAFile`]
/// if it is not a regular file.
pub(crate) fn locate_project(path: &Path) -> Result<ProjectLocation> {
    let canonical = std::fs::canonicalize(path).map_err(|e| IoError::read(path, e))?;
    let metadata = std::fs::metadata(&canonical).map_err(|e| IoError::read(&canonical, e))?;
    if !metadata.is_file() {
        return Err(PathError::NotAFile {
            path: crate::error::path_string(&canonical),
        }
        .into());
    }
    let root = canonical
        .parent()
        .ok_or_else(|| PathError::UnacceptablePath {
            path: crate::error::path_string(&canonical),
            detail: "it has no parent directory".into(),
        })?
        .to_path_buf();
    Ok(ProjectLocation {
        project_file: canonical,
        root,
    })
}

/// Resolve one declared input against the project root.
///
/// # Errors
///
/// Returns a [`PathError`] if the declared path is unsafe or escapes the root, and
/// an [`IoError`] if it cannot be reached.
pub(crate) fn resolve_declared_input(root: &Path, field: &str, declared: &str) -> Result<PathBuf> {
    let components = check_declared_relative(field, declared)?;

    let mut joined = root.to_path_buf();
    for component in components {
        joined.push(component);
    }

    // Authoritative check: ask the operating system where this really lands, then
    // require that answer to sit beneath the root. This is what catches a symlink,
    // a junction, or any other reparse point that points out of the project.
    let resolved = std::fs::canonicalize(&joined).map_err(|e| IoError::read(&joined, e))?;
    if !resolved.starts_with(root) {
        return Err(PathError::EscapesRoot {
            path: declared.to_string(),
            resolved: crate::error::path_string(&resolved),
            root: crate::error::path_string(root),
        }
        .into());
    }
    let metadata = std::fs::metadata(&resolved).map_err(|e| IoError::read(&resolved, e))?;
    if !metadata.is_file() {
        return Err(PathError::NotAFile {
            path: crate::error::path_string(&resolved),
        }
        .into());
    }
    Ok(resolved)
}

/// Where a bundle will be staged and where it will finally land.
#[derive(Debug)]
pub(crate) struct OutputPlan {
    /// The directory the caller asked for. It does not exist yet.
    pub final_dir: PathBuf,
    /// A sibling directory that receives the bytes, and is renamed into place only
    /// once every member has been written.
    pub staging_dir: PathBuf,
}

/// Plan an atomic bundle write to `out`, refusing to overwrite anything and refusing
/// to put the output anywhere that overlaps an input.
///
/// # Errors
///
/// Returns [`PathError::OutputExists`], [`PathError::OutputParentMissing`],
/// [`PathError::OutputOverlapsInput`], or [`PathError::StagingExists`].
pub(crate) fn plan_output(out: &Path, inputs: &[PathBuf]) -> Result<OutputPlan> {
    let display = crate::error::path_string(out);
    if crate::error::path_string(out).len() > MAX_PATH_BYTES {
        return Err(PathError::UnacceptablePath {
            path: display,
            detail: format!("it is longer than {MAX_PATH_BYTES} bytes"),
        }
        .into());
    }

    let name = out
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| PathError::UnacceptablePath {
            path: display.clone(),
            detail: "it does not end in a usable directory name".into(),
        })?
        .to_string();
    if let Some(reserved) = is_reserved_device(&name) {
        return Err(PathError::ReservedDeviceName {
            path: display,
            name: reserved.to_string(),
        }
        .into());
    }
    if let Err(detail) = check_component(&name) {
        return Err(PathError::UnacceptablePath {
            path: display,
            detail,
        }
        .into());
    }

    // The parent must already exist. Ordinata creates the bundle, not the tree it
    // sits in, so a typo in the parent is a refusal rather than a surprise mkdir.
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    let parent_canonical = match parent {
        Some(p) => std::fs::canonicalize(p).map_err(|_| PathError::OutputParentMissing {
            path: crate::error::path_string(p),
        })?,
        None => std::env::current_dir().map_err(|e| IoError::read(Path::new("."), e))?,
    };
    if !parent_canonical.is_dir() {
        return Err(PathError::OutputParentMissing {
            path: crate::error::path_string(&parent_canonical),
        }
        .into());
    }

    let final_dir = parent_canonical.join(&name);
    if final_dir.exists() {
        return Err(PathError::OutputExists {
            path: crate::error::path_string(&final_dir),
        }
        .into());
    }

    // An output that contains an input would let a solve quietly consume its own
    // bundle on the next run, and an output that *is* an input would destroy it.
    for input in inputs {
        if input.starts_with(&final_dir) || final_dir.starts_with(input) {
            return Err(PathError::OutputOverlapsInput {
                output: crate::error::path_string(&final_dir),
                input: crate::error::path_string(input),
            }
            .into());
        }
    }

    // The staging name is derived from the process ID so that two concurrent runs
    // writing neighbouring bundles cannot collide. A leftover from a killed run is
    // reported rather than deleted: Ordinata does not remove directories it did not
    // certainly create.
    let staging_dir =
        parent_canonical.join(format!(".{name}.ordinata-staging-{}", std::process::id()));
    if staging_dir.exists() {
        return Err(PathError::StagingExists {
            path: crate::error::path_string(&staging_dir),
        }
        .into());
    }

    Ok(OutputPlan {
        final_dir,
        staging_dir,
    })
}

/// Check a path named *inside* a bundle or an archive, before anything opens it.
///
/// Bundle member paths are a much narrower language than filesystem paths: they are
/// forward-slash relative paths over `[A-Za-z0-9._-]`. Anything else is refused, so
/// a checksum file or an archive index cannot talk Ordinata into touching a path of
/// its choosing.
pub(crate) fn member_path_rule_broken(path: &str) -> Option<String> {
    if path.is_empty() {
        return Some("it is empty".into());
    }
    if path.len() > 255 {
        return Some("it is longer than 255 bytes".into());
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Some("it is not a forward-slash relative path".into());
    }
    let components: Vec<&str> = path.split('/').collect();
    if components.len() > 8 {
        return Some("it has more than 8 components".into());
    }
    for component in components {
        if component.is_empty() {
            return Some("it has an empty component".into());
        }
        if component == "." || component == ".." {
            return Some(format!("it contains a {component:?} component"));
        }
        if is_reserved_device(component).is_some() {
            return Some("it names a reserved device".into());
        }
        if component.ends_with('.') || component.ends_with(' ') {
            return Some("a component ends with a dot or a space".into());
        }
        for c in component.chars() {
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') {
                return Some(format!(
                    "it contains {c:?}; bundle members use only ASCII letters, digits, underscore, dot, and hyphen"
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;

    #[test]
    fn traversal_is_refused_in_declared_inputs() {
        for hostile in [
            "../secrets.csv",
            "a/../../b.csv",
            "./design.csv",
            "..",
            "sub/./x.csv",
        ] {
            let err = check_declared_relative("designCsv", hostile).unwrap_err();
            assert_eq!(err.code(), "path_traversal", "{hostile:?} -> {err}");
        }
    }

    #[test]
    fn absolute_and_drive_paths_are_refused() {
        for hostile in [
            "/etc/passwd",
            "/design.csv",
            "C:/design.csv",
            "C:design.csv",
            "//server/share/x.csv",
        ] {
            let err = check_declared_relative("designCsv", hostile).unwrap_err();
            assert!(
                matches!(err.code(), "path_not_relative" | "path_unacceptable"),
                "{hostile:?} -> {} ({err})",
                err.code()
            );
        }
    }

    #[test]
    fn backslashes_are_refused_so_a_windows_path_cannot_smuggle_traversal() {
        // On a Unix host `..\..\x` is a single legal filename. Refusing backslashes
        // outright means a project file cannot mean one thing on Linux and another
        // on Windows.
        let err = check_declared_relative("designCsv", "..\\..\\etc\\passwd").unwrap_err();
        assert_eq!(err.code(), "path_unacceptable");
    }

    #[test]
    fn reserved_device_names_are_refused() {
        for hostile in ["NUL", "nul.csv", "data/CON", "com1.csv", "aux"] {
            let err = check_declared_relative("designCsv", hostile).unwrap_err();
            assert_eq!(err.code(), "path_reserved_device_name", "{hostile:?}");
        }
    }

    #[test]
    fn ordinary_relative_paths_are_accepted() {
        for good in ["design.csv", "data/design.csv", "a/b/c/design-1.csv"] {
            assert!(
                check_declared_relative("designCsv", good).is_ok(),
                "{good:?} should be accepted"
            );
        }
    }

    #[test]
    fn member_paths_reject_the_checksum_path_attacks() {
        for hostile in [
            "../../etc/passwd",
            "/etc/passwd",
            "C:/windows/system32/x",
            "a/../../../b",
            "..",
            "sub\\file",
            "NUL",
            "file with space",
            "weird;name",
            "",
        ] {
            assert!(
                member_path_rule_broken(hostile).is_some(),
                "{hostile:?} must be refused as a bundle member path"
            );
        }
    }

    #[test]
    fn member_paths_accept_exactly_what_ordinata_writes() {
        for good in [
            "manifest.json",
            "parameters.csv",
            "fit.csv",
            "diagnostics.json",
            "validation.json",
            "report.html",
            "SHA256SUMS.txt",
            "inputs/project.json",
            "inputs/design.csv",
            "inputs/observations.csv",
        ] {
            assert!(
                member_path_rule_broken(good).is_none(),
                "{good:?} should be accepted: {:?}",
                member_path_rule_broken(good)
            );
        }
    }

    #[test]
    fn output_must_not_already_exist() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("bundle");
        std::fs::create_dir(&existing).unwrap();
        let err = plan_output(&existing, &[]).unwrap_err();
        assert_eq!(err.code(), "output_exists");
    }

    #[test]
    fn output_must_not_contain_an_input() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let input = project.join("design.csv");
        std::fs::write(&input, "x").unwrap();
        let input = std::fs::canonicalize(&input).unwrap();

        // Writing the bundle *into* the project directory would make the bundle
        // contain its own inputs.
        let err = plan_output(&project, std::slice::from_ref(&input)).unwrap_err();
        assert_eq!(err.code(), "output_exists");

        // And a fresh directory that would enclose the input is refused too.
        let enclosing = std::fs::canonicalize(dir.path()).unwrap().join("new");
        let sneaky_input = enclosing.join("design.csv");
        let err = plan_output(&enclosing, &[sneaky_input]).unwrap_err();
        assert_eq!(err.code(), "output_overlaps_input");
    }

    #[test]
    fn output_parent_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("missing").join("bundle");
        let err = plan_output(&nested, &[]).unwrap_err();
        assert_eq!(err.code(), "output_parent_missing");
    }

    #[test]
    fn declared_input_may_not_escape_the_root_through_a_link() {
        // Symlink creation needs privilege on Windows, so this test asserts the
        // property on platforms where it can actually build the trap, and asserts
        // the textual rule everywhere.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.csv"), "id,v\n").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret.csv"), root.join("link.csv")).unwrap();
            let err = resolve_declared_input(&root, "designCsv", "link.csv").unwrap_err();
            assert_eq!(err.code(), "path_escapes_root");
        }
        #[cfg(windows)]
        {
            // Requires SeCreateSymbolicLinkPrivilege; skip silently when unavailable
            // rather than failing an unprivileged test run.
            if std::os::windows::fs::symlink_file(outside.join("secret.csv"), root.join("link.csv"))
                .is_ok()
            {
                let err = resolve_declared_input(&root, "designCsv", "link.csv").unwrap_err();
                assert_eq!(err.code(), "path_escapes_root");
            }
        }
    }
}
