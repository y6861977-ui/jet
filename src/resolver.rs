//! Dependency resolver — a port of resolver.ts.
//!
//! Builds the tree, resolving ranges into concrete versions. The source of
//! packuments is swappable (a closure) — a fake registry in tests, the network
//! in production. Storage model: a flat map `name@version -> node`, a node's
//! dependencies are keys in that map. Dedup and cycle-breaking then fall out
//! naturally: identical versions live as one entry, a cycle's back-edge is just a
//! reference to a key.
//!
//! Optional dependencies (`optionalDependencies`) are walked like regular ones,
//! with two differences: their failure isn't an error, and the node keeps its
//! platform constraints (`os`/`cpu`). Platform selection is done NOT here but at
//! the download stage — ALL variants make it into the tree and the lock. This is
//! a deliberate decision: a platform package's packument has to be fetched anyway
//! to learn its os/cpu, and once it's parsed, writing it into the lock costs not a
//! single extra byte of traffic and makes the lock cross-platform (a file built on
//! macOS works on Windows).

use crate::semver::{max_satisfying, parse_range};
use crate::types::Packument;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct ResolvedNode {
    pub name: String,
    pub version: String,
    pub requested: String,
    pub tarball: String,
    pub integrity: Option<String>,
    pub unpacked_size: Option<u64>,
    /// The `name@version` keys of the dependencies in the shared `flat` map.
    pub dependencies: Vec<String>,
    /// Platform constraints from the manifest. Empty lists — fits everyone.
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    /// The node is reachable ONLY through optionalDependencies. If the same package
    /// is required by someone as mandatory, the flag is cleared.
    pub optional: bool,
    /// The package has an install script, which we never run on principle.
    pub has_install_script: bool,
}

#[derive(Clone, Debug)]
pub struct FailedRoot {
    pub name: String,
    pub range: String,
    pub message: String,
}

pub struct ResolveResult {
    /// The keys of the root packages in `flat`.
    pub roots: Vec<String>,
    /// Unique packages `name@version -> node`.
    pub flat: HashMap<String, ResolvedNode>,
    pub warnings: Vec<String>,
    /// Root requirements that didn't resolve (the rest are in `roots`).
    pub failed_roots: Vec<FailedRoot>,
    /// At least one optional dependency didn't resolve due to the NETWORK (not
    /// because the platform didn't fit). These are different things: the first is
    /// fixed by a retry, the second is normal. We can't stay silent, or on a bad
    /// network the user quietly gets a tree without the native binary.
    pub optional_unreachable: bool,
}

/// Pick a concrete version by range or dist-tag ("latest", "next"…).
pub fn resolve_concrete_version(pkg: &Packument, range: &str) -> Result<String, String> {
    if let Some(v) = pkg.dist_tags.get(range) {
        return Ok(v.clone());
    }
    let all: Vec<&str> = pkg.versions.keys().map(|s| s.as_str()).collect();
    match max_satisfying(&all, &parse_range(range)) {
        Some(v) => Ok(v),
        None => Err(format!(
            "No version of {} satisfies \"{}\" (available: {} versions)",
            pkg.name,
            range,
            all.len()
        )),
    }
}

struct Resolver<F> {
    fetch: F,
    cache: HashMap<String, Packument>,
    flat: HashMap<String, ResolvedNode>,
    warnings: Vec<String>,
    optional_unreachable: bool,
}

impl<F> Resolver<F>
where
    F: FnMut(&str) -> Result<Packument, String>,
{
    fn ensure_packument(&mut self, name: &str) -> Result<(), String> {
        if !self.cache.contains_key(name) {
            let pkg = (self.fetch)(name)?;
            self.cache.insert(name.to_string(), pkg);
        }
        Ok(())
    }

    /// Resolve `name@range`, return the node's key in `flat`.
    ///
    /// `optional` — whether we arrived here through optionalDependencies. The flag
    /// only affects how a failure of this node's CHILDREN is treated and how the
    /// node itself is marked; it doesn't affect version selection.
    fn build(&mut self, name: &str, range: &str, optional: bool) -> Result<String, String> {
        self.ensure_packument(name)?;

        // Pull the needed data as a copy and drop the cache borrow — so we can then
        // recursively mutate self.flat freely.
        let (
            version,
            mut deps,
            mut opt_deps,
            tarball,
            integrity,
            size,
            os,
            cpu,
            has_install_script,
        ) = {
            let pkg = self.cache.get(name).expect("packument is cached");
            let version = resolve_concrete_version(pkg, range)?;
            let m = pkg
                .versions
                .get(&version)
                .ok_or_else(|| format!("Version {name}@{version} is missing from the package document"))?;
            (
                version,
                m.dependencies
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>(),
                m.optional_dependencies
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>(),
                m.dist.tarball.clone(),
                m.dist.integrity.clone(),
                m.dist.unpacked_size,
                m.os.clone(),
                m.cpu.clone(),
                m.has_install_script,
            )
        };

        let key = format!("{name}@{version}");
        // Already in the map — dedup (or a cycle back-edge). Don't re-resolve.
        if let Some(existing) = self.flat.get_mut(&key) {
            // The same package can be seen as both mandatory and optional.
            // Mandatory wins: otherwise its failure would be treated as harmless.
            if !optional {
                existing.optional = false;
            }
            return Ok(key);
        }

        // Register the node BEFORE walking children — so identical versions don't
        // diverge and the cycle breaks (a repeat request returns this same key from flat).
        self.flat.insert(
            key.clone(),
            ResolvedNode {
                name: name.to_string(),
                version,
                requested: range.to_string(),
                tarball,
                integrity,
                unpacked_size: size,
                dependencies: Vec::new(),
                os,
                cpu,
                optional,
                has_install_script,
            },
        );

        // Child order is deterministic (sorted by name) — independent of the HashMap.
        // Mandatory go first, optional after: so a node needed both ways ends up in
        // the map already without the optional flag.
        deps.sort_by(|a, b| a.0.cmp(&b.0));
        opt_deps.sort_by(|a, b| a.0.cmp(&b.0));
        let mut child_keys = Vec::new();
        for (dep_name, dep_range) in &deps {
            match self.build(dep_name, dep_range, optional) {
                Ok(child_key) => child_keys.push(child_key),
                Err(msg) => self.warnings.push(format!(
                    "{key}: couldn't resolve {dep_name}@{dep_range} — {msg}"
                )),
            }
        }
        for (dep_name, dep_range) in &opt_deps {
            match self.build(dep_name, dep_range, true) {
                Ok(child_key) => child_keys.push(child_key),
                Err(msg) => {
                    // A failed optional dependency isn't an install error.
                    // But the network and "no such package" are different things: the
                    // first is worth retrying, the second is not.
                    if crate::registry::looks_unreachable(&msg) {
                        self.optional_unreachable = true;
                        self.warnings.push(format!(
                            "{key}: optional dependency {dep_name}@{dep_range} didn't download — {msg}"
                        ));
                    }
                }
            }
        }
        self.flat
            .get_mut(&key)
            .expect("node was just inserted")
            .dependencies = child_keys;
        Ok(key)
    }
}

/// Build the dependency tree from the root requirements (name, range).
/// One root's failure does NOT bring down the rest — it goes to `failed_roots`.
pub fn resolve<F>(root_deps: &[(String, String)], fetch: F) -> ResolveResult
where
    F: FnMut(&str) -> Result<Packument, String>,
{
    let mut r = Resolver {
        fetch,
        cache: HashMap::new(),
        flat: HashMap::new(),
        warnings: Vec::new(),
        optional_unreachable: false,
    };
    let mut roots = Vec::new();
    let mut failed_roots = Vec::new();
    for (name, range) in root_deps {
        match r.build(name, range, false) {
            Ok(key) => roots.push(key),
            Err(message) => failed_roots.push(FailedRoot {
                name: name.clone(),
                range: range.clone(),
                message,
            }),
        }
    }
    ResolveResult {
        roots,
        flat: r.flat,
        warnings: r.warnings,
        failed_roots,
        optional_unreachable: r.optional_unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Dist, VersionManifest};

    /// Build a fake registry: name -> [(version, [(dep, range)])].
    fn fake_registry(
        defs: Vec<(&str, Vec<(&str, Vec<(&str, &str)>)>)>,
    ) -> impl FnMut(&str) -> Result<Packument, String> {
        registry_with(
            defs.into_iter()
                .map(|(n, vs)| Def {
                    name: n,
                    versions: vs,
                    ..Def::empty(n)
                })
                .collect(),
        )
    }

    /// A package description for the fake registry. A separate struct because
    /// platform packages also need optional dependencies and os/cpu.
    struct Def<'a> {
        name: &'a str,
        versions: Vec<(&'a str, Vec<(&'a str, &'a str)>)>,
        optional: Vec<(&'a str, &'a str)>,
        os: Vec<&'a str>,
        cpu: Vec<&'a str>,
        has_install_script: bool,
    }

    impl<'a> Def<'a> {
        fn empty(name: &'a str) -> Def<'a> {
            Def {
                name,
                versions: Vec::new(),
                optional: Vec::new(),
                os: Vec::new(),
                cpu: Vec::new(),
                has_install_script: false,
            }
        }
    }

    // `use<>` — the closure owns the parsed data and doesn't capture `Def`'s
    // lifetime; without it edition 2024 decides otherwise.
    fn registry_with(defs: Vec<Def<'_>>) -> impl FnMut(&str) -> Result<Packument, String> + use<> {
        let mut docs: HashMap<String, Packument> = HashMap::new();
        for def in defs {
            let name = def.name;
            let mut vmap = HashMap::new();
            let mut last = String::new();
            for (version, deps) in def.versions {
                let dependencies = deps
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                let optional_dependencies = def
                    .optional
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                vmap.insert(
                    version.to_string(),
                    VersionManifest {
                        name: name.to_string(),
                        version: version.to_string(),
                        dependencies,
                        optional_dependencies,
                        os: def.os.iter().map(|s| s.to_string()).collect(),
                        cpu: def.cpu.iter().map(|s| s.to_string()).collect(),
                        has_install_script: def.has_install_script,
                        scripts: HashMap::new(),
                        deprecated: None,
                        dist: Dist {
                            tarball: format!("https://fake/{name}/{version}.tgz"),
                            integrity: None,
                            unpacked_size: None,
                        },
                    },
                );
                last = version.to_string();
            }
            let mut dist_tags = HashMap::new();
            dist_tags.insert("latest".to_string(), last);
            docs.insert(
                name.to_string(),
                Packument {
                    name: name.to_string(),
                    dist_tags,
                    versions: vmap,
                },
            );
        }
        move |name: &str| {
            docs.get(name)
                .cloned()
                .ok_or_else(|| format!("Package not found in the registry: {name}"))
        }
    }

    fn dep(name: &str, range: &str) -> (String, String) {
        (name.to_string(), range.to_string())
    }

    #[test]
    fn recursion_three_levels() {
        let reg = fake_registry(vec![
            ("a", vec![("1.0.0", vec![("b", "^1.0.0")])]),
            ("b", vec![("1.0.0", vec![("c", "~2.0.0")])]),
            ("c", vec![("2.0.0", vec![]), ("2.0.5", vec![])]),
        ]);
        let res = resolve(&[dep("a", "^1.0.0")], reg);
        assert_eq!(res.roots, vec!["a@1.0.0"]);
        let a = &res.flat["a@1.0.0"];
        assert_eq!(a.dependencies, vec!["b@1.0.0"]);
        let b = &res.flat["b@1.0.0"];
        assert_eq!(
            b.dependencies,
            vec!["c@2.0.5"],
            "c resolved by ~2.0.0 to 2.0.5"
        );
        assert!(res.failed_roots.is_empty());
    }

    #[test]
    fn diamond_dedup() {
        // a -> b, a -> c, b -> d, c -> d : d is single in flat
        let reg = fake_registry(vec![
            ("a", vec![("1.0.0", vec![("b", "^1.0.0"), ("c", "^1.0.0")])]),
            ("b", vec![("1.0.0", vec![("d", "^1.0.0")])]),
            ("c", vec![("1.0.0", vec![("d", "^1.0.0")])]),
            ("d", vec![("1.0.0", vec![])]),
        ]);
        let res = resolve(&[dep("a", "^1.0.0")], reg);
        assert_eq!(res.flat.len(), 4, "exactly 4 unique packages (a,b,c,d)");
        assert_eq!(res.flat["b@1.0.0"].dependencies, vec!["d@1.0.0"]);
        assert_eq!(res.flat["c@1.0.0"].dependencies, vec!["d@1.0.0"]);
    }

    #[test]
    fn cycle_does_not_hang() {
        // x -> y -> x
        let reg = fake_registry(vec![
            ("x", vec![("1.0.0", vec![("y", "^1.0.0")])]),
            ("y", vec![("1.0.0", vec![("x", "^1.0.0")])]),
        ]);
        let res = resolve(&[dep("x", "^1.0.0")], reg);
        assert_eq!(res.flat.len(), 2, "2 unique packages (x, y)");
        assert_eq!(res.flat["x@1.0.0"].dependencies, vec!["y@1.0.0"]);
        assert_eq!(
            res.flat["y@1.0.0"].dependencies,
            vec!["x@1.0.0"],
            "the back-edge is just a reference"
        );
    }

    #[test]
    fn partial_failure_does_not_kill_batch() {
        let reg = fake_registry(vec![
            ("express", vec![("5.0.0", vec![])]),
            ("react", vec![("19.0.0", vec![])]),
            // nosuchpkg is absent
        ]);
        let res = resolve(
            &[
                dep("express", "^5.0.0"),
                dep("nosuchpkg", "latest"),
                dep("react", "^19.0.0"),
            ],
            reg,
        );
        let mut roots = res.roots.clone();
        roots.sort();
        assert_eq!(
            roots,
            vec!["express@5.0.0", "react@19.0.0"],
            "the successful ones resolved"
        );
        assert_eq!(res.failed_roots.len(), 1);
        assert_eq!(res.failed_roots[0].name, "nosuchpkg");
    }

    /// A realistic shape: a tool with eight platform variants.
    fn registry_with_platform_variants() -> impl FnMut(&str) -> Result<Packument, String> {
        registry_with(vec![
            Def {
                versions: vec![("2.1.258", vec![])],
                optional: vec![
                    ("tool-win32-x64", "2.1.258"),
                    ("tool-darwin-arm64", "2.1.258"),
                    ("tool-linux-x64", "2.1.258"),
                ],
                has_install_script: true,
                ..Def::empty("tool")
            },
            Def {
                versions: vec![("2.1.258", vec![])],
                os: vec!["win32"],
                cpu: vec!["x64"],
                ..Def::empty("tool-win32-x64")
            },
            Def {
                versions: vec![("2.1.258", vec![])],
                os: vec!["darwin"],
                cpu: vec!["arm64"],
                ..Def::empty("tool-darwin-arm64")
            },
            Def {
                versions: vec![("2.1.258", vec![])],
                os: vec!["linux"],
                cpu: vec!["x64"],
                ..Def::empty("tool-linux-x64")
            },
        ])
    }

    #[test]
    fn optional_dependencies_are_walked() {
        // They used to not be walked at all: the tree stopped at the main package,
        // and the native binary didn't arrive on any platform.
        let res = resolve(&[dep("tool", "latest")], registry_with_platform_variants());
        assert_eq!(res.roots, vec!["tool@2.1.258"]);
        assert_eq!(
            res.flat.len(),
            4,
            "the main package and all three platform variants"
        );
        let root = &res.flat["tool@2.1.258"];
        assert_eq!(root.dependencies.len(), 3, "all variants are children of the root");
        assert!(
            root.has_install_script,
            "the install-script flag reached the node"
        );
    }

    #[test]
    fn every_platform_variant_lands_in_the_tree() {
        // A deliberate decision: the installer does platform selection, while the
        // tree and lock stay cross-platform.
        let res = resolve(&[dep("tool", "latest")], registry_with_platform_variants());
        let win = &res.flat["tool-win32-x64@2.1.258"];
        assert_eq!(win.os, vec!["win32"]);
        assert_eq!(win.cpu, vec!["x64"]);
        assert!(win.optional, "arrived through optionalDependencies");
        assert!(
            res.flat.contains_key("tool-darwin-arm64@2.1.258"),
            "a foreign platform is in the tree too"
        );
        assert!(
            !res.flat["tool@2.1.258"].optional,
            "the tool itself is mandatory"
        );
    }

    #[test]
    fn missing_optional_does_not_fail_the_install() {
        let reg = registry_with(vec![Def {
            versions: vec![("1.0.0", vec![])],
            optional: vec![("no-such-thing", "^1.0.0")],
            ..Def::empty("app")
        }]);
        let res = resolve(&[dep("app", "^1.0.0")], reg);
        assert_eq!(res.roots, vec!["app@1.0.0"], "the root is installed");
        assert!(
            res.failed_roots.is_empty(),
            "a failed optional isn't a failed install"
        );
        assert!(
            res.warnings.is_empty(),
            "\"no such package\" isn't worth noise, a retry won't help"
        );
        assert!(!res.optional_unreachable, "this isn't a network problem");
    }

    #[test]
    fn unreachable_optional_is_reported_for_a_retry() {
        // Telling the network apart from "no such package" is essential: otherwise
        // on a bad network the user quietly gets a tree without the native binary.
        let reg = |name: &str| -> Result<Packument, String> {
            if name == "app" {
                let mut build = registry_with(vec![Def {
                    versions: vec![("1.0.0", vec![])],
                    optional: vec![("native", "^1.0.0")],
                    ..Def::empty("app")
                }]);
                build("app")
            } else {
                Err("Registry unreachable: curl: (28) Operation timed out".to_string())
            }
        };
        let res = resolve(&[dep("app", "^1.0.0")], reg);
        assert_eq!(res.roots, vec!["app@1.0.0"]);
        assert!(res.optional_unreachable, "the optional's network failure is flagged");
        assert_eq!(res.warnings.len(), 1);
        assert!(res.warnings[0].contains("native"));
    }

    #[test]
    fn required_wins_over_optional_for_the_same_package() {
        // shared is seen as both mandatory (by app) and optional (by tool).
        // The flag must clear, otherwise its failure would be treated as harmless.
        let reg = registry_with(vec![
            Def {
                versions: vec![("1.0.0", vec![("shared", "^1.0.0")])],
                optional: vec![("shared", "^1.0.0")],
                ..Def::empty("app")
            },
            Def {
                versions: vec![("1.0.0", vec![])],
                ..Def::empty("shared")
            },
        ]);
        let res = resolve(&[dep("app", "^1.0.0")], reg);
        assert!(!res.flat["shared@1.0.0"].optional, "mandatory wins");
    }

    #[test]
    fn dist_tag_latest() {
        let reg = fake_registry(vec![("pkg", vec![("1.0.0", vec![]), ("2.0.0", vec![])])]);
        let res = resolve(&[dep("pkg", "latest")], reg);
        assert_eq!(
            res.roots,
            vec!["pkg@2.0.0"],
            "latest -> the highest published"
        );
    }
}
