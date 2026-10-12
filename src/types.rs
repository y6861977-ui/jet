//! Shared core types — a port of types.ts.

use std::collections::HashMap;

/// Distribution data for one package version.
#[derive(Clone, Debug, Default)]
pub struct Dist {
    pub tarball: String,
    pub integrity: Option<String>,
    /// The unpacked package size in bytes (present in the abbreviated packument).
    pub unpacked_size: Option<u64>,
}

/// The manifest of one package version.
#[derive(Clone, Debug, Default)]
pub struct VersionManifest {
    pub name: String,
    pub version: String,
    pub dependencies: HashMap<String, String>,
    /// Optional dependencies: their failure doesn't bring down the install. Platform
    /// binaries are attached through them (see `platform.rs`).
    pub optional_dependencies: HashMap<String, String>,
    /// Platform constraints. An empty list means no constraints.
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    /// The package has an install script (preinstall/install/postinstall). We don't
    /// run them on principle, but we must say so honestly.
    pub has_install_script: bool,
    /// Lifecycle scripts from the manifest (name -> command), if the registry returned
    /// them in the packument. Needed by `jet explain`/`jet diff` to show WHAT would
    /// have run. Often not fully present — then we rely on `has_install_script`.
    pub scripts: HashMap<String, String>,
    /// The version's deprecation message, if any.
    pub deprecated: Option<String>,
    pub dist: Dist,
}

/// A package document from the registry (packument).
#[derive(Clone, Debug)]
pub struct Packument {
    pub name: String,
    pub dist_tags: HashMap<String, String>,
    pub versions: HashMap<String, VersionManifest>,
}
