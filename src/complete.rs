//! Commands, typo suggestions, and post-error hints — a port of complete.ts.

#[derive(Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    /// The help section the command is shown in (see `HELP_GROUPS`).
    pub group: &'static str,
}

/// The order of sections in `jet help`.
pub const HELP_GROUPS: &[&str] = &["Packages", "Projects", "Registry & cache", "Other"];

const BASE_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "install",
        aliases: &["i", "add"],
        summary: "install packages (the -g flag — globally)",
        group: "Packages",
    },
    CommandInfo {
        name: "uninstall",
        aliases: &["remove", "rm", "un"],
        summary: "remove a package (-g — global)",
        group: "Packages",
    },
    CommandInfo {
        name: "global",
        aliases: &["g"],
        summary: "list global packages and the command directory",
        group: "Packages",
    },
    CommandInfo {
        name: "info",
        aliases: &["v", "show"],
        summary: "a package's manifest and versions",
        group: "Packages",
    },
    CommandInfo {
        name: "tree",
        aliases: &["t", "ls"],
        summary: "dependency tree",
        group: "Packages",
    },
    CommandInfo {
        name: "create",
        aliases: &["c"],
        summary: "create a project (react/next/vue/...)",
        group: "Projects",
    },
    CommandInfo {
        name: "exec",
        aliases: &["x"],
        summary: "run a package's bin (like npx)",
        group: "Projects",
    },
    CommandInfo {
        name: "registry",
        aliases: &["reg", "r"],
        summary: "registry and mirrors",
        group: "Registry & cache",
    },
    CommandInfo {
        name: "set",
        aliases: &[],
        summary: "change a setting: jet set registry <url|name|number>",
        group: "Registry & cache",
    },
    CommandInfo {
        name: "get",
        aliases: &[],
        summary: "show a setting: jet get registry",
        group: "Registry & cache",
    },
    CommandInfo {
        name: "cache",
        aliases: &["ca"],
        summary: "tarball cache",
        group: "Registry & cache",
    },
    CommandInfo {
        name: "history",
        aliases: &["hist"],
        summary: "install history",
        group: "Other",
    },
    CommandInfo {
        name: "dashboard",
        aliases: &["serve"],
        summary: "local HTTP dashboard",
        group: "Other",
    },
    CommandInfo {
        name: "audit",
        aliases: &[],
        summary: "check packages against OSV.dev (flags: audit --help)",
        group: "Other",
    },
    CommandInfo {
        name: "self",
        aliases: &[],
        summary: "manage the jet tool itself (self uninstall)",
        group: "Other",
    },
    CommandInfo {
        name: "help",
        aliases: &["h"],
        summary: "help",
        group: "Other",
    },
];

/// Pro-only commands — compiled in only when the `pro` feature is enabled.
/// The free, open-source build omits these.
#[cfg(feature = "pro")]
const PRO_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "explain",
        aliases: &["ex"],
        summary: "what a package will do on install (scripts, deps) BEFORE installing",
        group: "Packages",
    },
    CommandInfo {
        name: "diff",
        aliases: &[],
        summary: "what changed between versions (scripts, dependencies)",
        group: "Packages",
    },
    CommandInfo {
        name: "license",
        aliases: &["lic"],
        summary: "activate/show the jet Pro license",
        group: "Other",
    },
];

/// Every command available in this build: the base set, plus the Pro set when
/// the `pro` feature is on.
pub fn known_commands() -> Vec<CommandInfo> {
    // `mut` is only needed in the Pro build, where the extend below runs.
    #[allow(unused_mut)]
    let mut v: Vec<CommandInfo> = BASE_COMMANDS.to_vec();
    #[cfg(feature = "pro")]
    v.extend_from_slice(PRO_COMMANDS);
    v
}

pub fn all_command_names() -> Vec<&'static str> {
    let mut v = Vec::new();
    for c in known_commands() {
        v.push(c.name);
        v.extend_from_slice(c.aliases);
    }
    v
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut dp: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = dp[0];
        dp[0] = i;
        for j in 1..=b.len() {
            let tmp = dp[j];
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            dp[j] = (dp[j] + 1).min(dp[j - 1] + 1).min(prev + cost);
            prev = tmp;
        }
    }
    dp[b.len()]
}

/// Suggest the closest command (for "did you mean").
pub fn suggest_command(input: &str) -> Option<&'static str> {
    let mut best: Option<&'static str> = None;
    let mut best_dist = usize::MAX;
    for name in all_command_names() {
        let d = levenshtein(input, name);
        if d < best_dist {
            best_dist = d;
            best = Some(name);
        }
    }
    let threshold = (input.len() * 2 / 5).max(2);
    best.filter(|_| best_dist <= threshold)
}

/// Setting keys for `jet set <key>` / `jet get <key>` (with the registry command's aliases).
pub const SETTING_KEYS: &[&str] = &["registry", "reg", "r"];

/// Suggest a setting key on a typo (e.g. `jet set registy`).
pub fn suggest_setting(input: &str) -> Option<&'static str> {
    let mut best: Option<&'static str> = None;
    let mut best_dist = usize::MAX;
    for key in SETTING_KEYS {
        let d = levenshtein(input, key);
        if d < best_dist {
            best_dist = d;
            best = Some(key);
        }
    }
    let threshold = (input.len() * 2 / 5).max(2);
    best.filter(|_| best_dist <= threshold)
}

/// Context-sensitive hints after an error.
pub fn error_hints(message: &str) -> Vec<String> {
    let mut hints = Vec::new();
    let m = message.to_lowercase();
    if m.contains("not found in the registry") {
        hints.push("Check the package name — it may be a typo.".into());
        hints.push("See the versions:  jet info <name>".into());
        hints.push("Check/switch the registry:  jet registry check".into());
    } else if m.contains("package.json") {
        hints.push("Create a package.json or name the package explicitly:  jet install <name>".into());
    } else if m.contains("invalid package name") {
        hints.push("The package name contains invalid characters or a path.".into());
    }
    hints.push("List of commands:  jet help".into());
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions() {
        assert_eq!(suggest_command("instal"), Some("install"));
        assert_eq!(suggest_command("creat"), Some("create"));
        assert_eq!(suggest_command("registyr"), Some("registry"));
        assert_eq!(suggest_command("zzzzzzzz"), None);
    }

    #[test]
    fn every_command_lands_in_a_known_help_group() {
        // Otherwise the command silently disappears from `jet help`.
        let commands = known_commands();
        for c in &commands {
            assert!(
                HELP_GROUPS.contains(&c.group),
                "command {} is in section {:?}, which is not in HELP_GROUPS",
                c.name,
                c.group
            );
        }
        for g in HELP_GROUPS {
            assert!(
                commands.iter().any(|c| &c.group == g),
                "empty help section: {g}"
            );
        }
    }

    #[test]
    fn setting_suggestions() {
        assert_eq!(suggest_setting("registy"), Some("registry"));
        assert_eq!(suggest_setting("regisrty"), Some("registry"));
        assert_eq!(suggest_setting("zzzzzzzz"), None);
    }

    #[test]
    fn set_and_get_are_known_commands() {
        assert!(all_command_names().contains(&"set"));
        assert!(all_command_names().contains(&"get"));
    }

    #[test]
    fn aliases_and_hints() {
        assert!(all_command_names().contains(&"i"));
        assert!(all_command_names().contains(&"reg"));
        assert!(
            error_hints("Package not found in the registry: x")
                .iter()
                .any(|h| h.contains("registry check"))
        );
        assert!(
            error_hints("anything at all")
                .iter()
                .any(|h| h.contains("jet help"))
        );
    }
}
