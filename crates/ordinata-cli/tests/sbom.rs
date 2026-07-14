//! Release-only validation for the generated `CycloneDX` documents.
//!
//! JSON parsing is not an SBOM gate. The packaging script points this test at the
//! staged documents so they are decoded through `CycloneDX`'s versioned data model and
//! then checked for invalid fields and broken BOM references.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use cyclonedx_bom::prelude::{Bom, SpecVersion, Validate};

fn sboms(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(directory)
        .unwrap_or_else(|error| {
            panic!(
                "cannot read SBOM directory {}: {error}",
                directory.display()
            )
        })
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".cdx.json"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn staged_sboms_conform_to_the_cyclonedx_model() {
    let Ok(directory) = std::env::var("ORDINATA_SBOM_DIR") else {
        assert!(
            std::env::var("ORDINATA_SBOM_REQUIRED").is_err(),
            "SBOM validation was required but ORDINATA_SBOM_DIR was not set"
        );
        eprintln!("NOTICE: no staged SBOM directory; packaging sets one for the release gate");
        return;
    };

    let files = sboms(Path::new(&directory));
    assert!(
        !files.is_empty(),
        "no CycloneDX JSON documents found in {directory}"
    );

    for path in &files {
        let file = File::open(path)
            .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()));
        let bom = Bom::parse_from_json(BufReader::new(file)).unwrap_or_else(|error| {
            panic!(
                "{} is not a CycloneDX JSON document: {error}",
                path.display()
            )
        });
        assert_eq!(
            bom.spec_version,
            SpecVersion::V1_5,
            "{} is not the release's declared CycloneDX 1.5 model",
            path.display()
        );
        let validation = bom.validate();
        assert!(
            validation.passed(),
            "{} violates the CycloneDX model: {validation:?}",
            path.display()
        );
    }

    eprintln!(
        "validated {} CycloneDX 1.5 document(s) through the schema model",
        files.len()
    );
}

#[test]
fn model_validation_rejects_a_structurally_invalid_bom() {
    let invalid = br#"{
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": "not-a-urn-uuid",
        "version": 1,
        "components": []
    }"#;
    let bom = Bom::parse_from_json(invalid.as_slice()).expect("the JSON shape is parseable");
    assert!(
        !bom.validate().passed(),
        "the model validator accepted an invalid CycloneDX serial number"
    );
}
