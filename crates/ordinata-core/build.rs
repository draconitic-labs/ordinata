//! Records the target triple so that a bundle can say which build produced it.
//!
//! `TARGET` is only visible to a build script, and a bundle that does not name its
//! platform cannot honestly claim its numbers are reproducible "for this build".

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=ORDINATA_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
