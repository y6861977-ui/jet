//! jet CLI wrapper: the command router and its output. The core modules are
//! declared in `lib.rs` and pulled in here via the glob `use crate::*` (the
//! same ones `jet-server` reuses).

// Pull in every core module from the library (semver, installer, audit, ui, ...).
use crate::*;
use std::io::IsTerminal;
use std::path::PathBuf;

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn format_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MB", b as f64 / (1024.0 * 1024.0))
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");
    let rest = args.get(1..).unwrap_or(&[]);

    let result: Result<(), String> = match command {
        "info" | "v" | "show" => cmd_info(rest),
        "install" | "i" | "add" => cmd_install(rest),
        "uninstall" | "remove" | "rm" | "un" => cmd_uninstall(rest),
        "global" | "g" => cmd_global(rest),
        "self" => cmd_self(rest),
        "audit" => cmd_audit(rest),
        #[cfg(feature = "pro")]
        "explain" | "ex" => cmd_explain(rest),
        #[cfg(feature = "pro")]
        "diff" => cmd_diff(rest),
        #[cfg(feature = "pro")]
        "license" | "lic" => cmd_license(rest),
        "create" | "c" => cmd_create(rest),
        "exec" | "x" => cmd_exec(rest),
        "tree" | "t" | "ls" => cmd_tree(rest),
        "history" | "hist" => cmd_history(),
        "dashboard" | "serve" => dashboard::serve(&history::default_dir()),
        "registry" | "reg" | "r" => cmd_registry(rest),
        "set" => cmd_set(rest),
        "get" => cmd_get(rest),
        "cache" | "ca" => cmd_cache(rest),
        "help" | "h" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-V" | "-v" => {
            print_version();
            Ok(())
        }
        other => {
            ui::eprint_error(&format!("Unknown command: {}", ui::ered(other)));
            if let Some(g) = complete::suggest_command(other) {
                ui::eprint_hint(&format!("Did you mean:  jet {g}"));
            }
            ui::eprint_hint("List of commands:  jet help");
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        if command == "audit" && rest.iter().any(|a| a == "--json") {
            eprintln!("{}", audit_cli::error_json(&e));
            std::process::exit(1);
        }
        ui::eprint_error(&e);
        for h in complete::error_hints(&e) {
            ui::eprint_hint(&h);
        }
        std::process::exit(1);
    }
}

/// `jet license [activate <key> | status | remove]` — the jet Pro license.
#[cfg(feature = "pro")]
fn cmd_license(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("status") | Some("show") => license::status(),
        Some("activate") | Some("add") => {
            let key = args
                .get(1)
                .ok_or("Specify a key: jet license activate <key>")?;
            license::activate(key)
        }
        Some("remove") | Some("rm") | Some("deactivate") => license::remove(),
        Some(other) => Err(format!(
            "Unknown license subcommand: {other}\n  jet license [activate <key> | status | remove]"
        )),
    }
}

/// `jet self uninstall [--yes|-y]` — remove the jet tool itself from the machine
/// (data dir ~/.jet, PATH entry, and the binary). Not to be confused with
/// `jet uninstall <package>`, which removes a package from node_modules.
fn cmd_self(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("uninstall") | Some("remove") => {
            let yes = args.iter().any(|a| a == "--yes" || a == "-y");
            self_uninstall::uninstall(yes)
        }
        None => Err("Specify a subcommand: jet self uninstall [--yes]".to_string()),
        Some(other) => Err(format!(
            "Unknown self subcommand: {other}\n  jet self uninstall [--yes]"
        )),
    }
}

/// `jet explain <pkg>[@version]` — show what a package will do when installed
/// (install scripts, dependencies, size) BEFORE installing it.
#[cfg(feature = "pro")]
fn cmd_explain(args: &[String]) -> Result<(), String> {
    let target = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .ok_or("Specify a package: jet explain <pkg>[@version]")?;
    let (name, spec) = explain::split_spec(target);
    explain::explain(&name, spec.as_deref())
}

/// `jet diff <pkg> <verA> <verB>` — what changed between versions (dependencies,
/// install scripts, size). Special emphasis on newly added install scripts.
#[cfg(feature = "pro")]
fn cmd_diff(args: &[String]) -> Result<(), String> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let [name, va, vb] = positional.as_slice() else {
        return Err("Usage: jet diff <pkg> <verA> <verB>".to_string());
    };
    explain::diff(name, va, vb)
}

fn cmd_info(args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or("Specify a package name: jet info <pkg>")?;
    let pkg = registry::fetch_packument(name)?;
    let latest = pkg.dist_tags.get("latest").cloned().unwrap_or_default();

    // Label width is set by the longest one so the values line up in a column.
    const LABELS: [&str; 3] = ["latest version", "total versions", "dependencies"];
    let w = ui::max_width(LABELS.into_iter());

    println!("\n{}", ui::heading(&pkg.name));
    println!("{}", ui::field(LABELS[0], w, &ui::green(&latest)));
    println!(
        "{}",
        ui::field(LABELS[1], w, &pkg.versions.len().to_string())
    );

    if let Some(m) = pkg.versions.get(&latest) {
        println!(
            "{}",
            ui::field(LABELS[2], w, &m.dependencies.len().to_string())
        );
        let mut deps: Vec<_> = m.dependencies.iter().collect();
        deps.sort();
        if !deps.is_empty() {
            println!();
            let dw = ui::max_width(deps.iter().map(|(d, _)| d.as_str()));
            for (d, r) in deps {
                println!("{}{} {}", ui::PAD, ui::pad_right(d, dw), ui::dim(r));
            }
        }
    }
    println!();
    Ok(())
}

/// Whether the arguments include a global-install flag.
fn has_global_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "-g" || a == "--global")
}

fn merge_install_requests(
    mut existing: Vec<(String, String)>,
    requested: &[(String, String)],
) -> Vec<(String, String)> {
    for (name, range) in requested {
        if let Some((_, current_range)) = existing.iter_mut().find(|(item, _)| item == name) {
            current_range.clone_from(range);
        } else {
            existing.push((name.clone(), range.clone()));
        }
    }
    existing
}

fn cmd_install(args: &[String]) -> Result<(), String> {
    if has_global_flag(args) {
        return cmd_install_global(args);
    }
    let targets: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let root = cwd();
    // Each installer step is a dimmed line with "→" under the heading.
    let mut log = |m: &str| println!("{}", ui::step(m));

    if targets.is_empty() {
        let manifest = match project::read_manifest(&root) {
            Some(m) => m,
            None if root.join("package.json").exists() => {
                return Err("package.json exists but can't be read as JSON. Make sure it is valid UTF-8 JSON (in PowerShell write the file via Set-Content -Encoding utf8).".to_string());
            }
            None => {
                return Err("package.json not found. Specify a package: jet install <name>".to_string());
            }
        };
        let deps = project::collect_dependencies(&manifest);
        if deps.is_empty() {
            println!("\n{}", ui::heading("Install"));
            println!("{}\n", ui::step("package.json has no dependencies."));
            return Ok(());
        }
        let what = ui::plural(deps.len(), "dependency", "dependencies");
        println!(
            "\n{}",
            ui::heading(&format!("Install — {what} from package.json"))
        );
        let report = installer::install(
            &root,
            &deps,
            &installer::InstallOptions {
                use_lock: true,
                record_history: true,
            },
            &mut log,
        )?;
        print_summary(&report);
        return Ok(());
    }

    let requested: Vec<(String, String)> =
        targets.iter().map(|t| spec::split_name_range(t)).collect();
    let existing = match project::read_manifest(&root) {
        Some(manifest) => project::collect_dependencies(&manifest),
        None if root.join("package.json").exists() => {
            return Err("package.json exists but can't be read as JSON. Make sure it is valid UTF-8 JSON (in PowerShell write the file via Set-Content -Encoding utf8).".to_string());
        }
        None => Vec::new(),
    };
    let install_requests = merge_install_requests(existing, &requested);
    let list = requested
        .iter()
        .map(|(n, r)| format!("{n}@{r}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("\n{}", ui::heading(&format!("Install — {list}")));
    let report = installer::install(
        &root,
        &install_requests,
        &installer::InstallOptions {
            use_lock: false,
            record_history: true,
        },
        &mut log,
    )?;

    for (name, _) in &requested {
        if let Some(r) = report.roots.iter().find(|r| &r.name == name) {
            project::add_dependency(&root, name, &format!("^{}", r.version))?;
            println!(
                "{}",
                ui::step(&format!("In package.json: \"{name}\": \"^{}\"", r.version))
            );
        }
    }
    print_summary(&report);
    if !report.failed_roots.is_empty() {
        let n = ui::plural(report.failed_roots.len(), "package", "packages");
        eprintln!(
            "\n{}",
            ui::ered(&format!("{} Failed to install: {n}", ui::FAIL))
        );
        let w = ui::max_width(report.failed_roots.iter().map(|f| f.name.as_str()));
        for f in &report.failed_roots {
            eprintln!(
                "{}{} {} {}",
                ui::PAD,
                ui::ered(ui::FAIL),
                ui::pad_right(&f.name, w),
                ui::edim(&f.message)
            );
        }
        eprintln!();
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_install_global(args: &[String]) -> Result<(), String> {
    let targets: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if targets.is_empty() {
        return Err(
            "Specify a package: jet install -g <name>\n  e.g.: jet install -g typescript".to_string(),
        );
    }
    let requested: Vec<(String, String)> =
        targets.iter().map(|t| spec::split_name_range(t)).collect();
    let list = requested
        .iter()
        .map(|(n, r)| format!("{n}@{r}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "\n{}",
        ui::heading(&format!("Global install — {list}"))
    );

    let mut log = |m: &str| println!("{}", ui::step(m));
    let commands = global::install(&requested, &mut log)?;

    println!("\n{} {}", ui::green(ui::OK), ui::bold_green("Done"));
    let what = ui::plural(commands.len(), "command", "commands");
    println!(
        "{}",
        ui::step(&format!(
            "Commands added ({what}): {}",
            commands.join(", ")
        ))
    );

    // Without the directory on PATH the commands won't be found — say plainly how to fix it.
    let bin = global::bin_dir();
    if !global::bin_dir_on_path() {
        println!(
            "\n{}",
            ui::yellow(&format!("{} Command directory is not on PATH:", ui::WARN))
        );
        println!("{}{}", ui::PAD, ui::bold(&bin.display().to_string()));
        println!(
            "{}",
            ui::hint("Add it permanently (PowerShell), then restart the terminal:")
        );
        println!(
            "{}{}",
            ui::PAD,
            ui::dim(&format!(
                "[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path','User') + ';{}', 'User')",
                bin.display()
            )),
        );
    } else {
        println!(
            "{}",
            ui::hint(&format!(
                "Commands are ready to run (directory on PATH: {})",
                bin.display()
            ))
        );
    }
    println!();
    Ok(())
}

fn cmd_uninstall(args: &[String]) -> Result<(), String> {
    let names: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect();
    if names.is_empty() {
        let how = if has_global_flag(args) {
            "jet uninstall -g <name>"
        } else {
            "jet uninstall <name>"
        };
        return Err(format!("Specify a package: {how}"));
    }

    let removed = if has_global_flag(args) {
        global::uninstall(&names)?
    } else {
        uninstall_local(&cwd(), &names)?
    };

    println!("\n{} {}", ui::green(ui::OK), ui::bold_green("Removed"));
    for name in &removed {
        println!("{}", ui::step(name));
    }
    println!();
    Ok(())
}

/// Local uninstall: take the package off disk, clean it out of package.json, and
/// prune the lock. We prune the lock by reconstructing the tree from the remaining
/// dependencies (offline) — that also drops orphaned transitive packages.
fn uninstall_local(root: &std::path::Path, names: &[String]) -> Result<Vec<String>, String> {
    let node_modules = root.join("node_modules");
    let mut removed = Vec::new();

    for name in names {
        // The name goes into a path under remove_dir_all — validate before any FS ops.
        if !registry::is_valid_package_name(name) {
            return Err(format!("Invalid package name: {name:?}"));
        }
        let dir = node_modules.join(name);
        let in_manifest = project::remove_dependency(root, name)?;
        let on_disk = dir.exists();
        if !in_manifest && !on_disk {
            return Err(format!("{name} is not installed in this project"));
        }
        if on_disk {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("Can't remove {name}: {e}"))?;
        }
        removed.push(name.clone());
    }

    // Prune the lock down to the remaining dependencies.
    if let Some(lock) = lockfile::read_lock(root) {
        let remaining = project::read_manifest(root)
            .map(|m| project::collect_dependencies(&m))
            .unwrap_or_default();
        match lockresolve::resolve_from_lock(&remaining, &lock) {
            // The tree rebuilt from the lock — rewrite the lock with only what's reachable.
            Some(res) => lockfile::write_lock(root, &lockfile::build_lock(&res.flat))?,
            // The lock doesn't cover the remainder (rare) — at least drop the removed entries.
            None => {
                let mut pruned = lock;
                pruned.packages.retain(|key, _| {
                    lockfile::split_key(key)
                        .map(|(n, _)| !names.contains(&n))
                        .unwrap_or(true)
                });
                lockfile::write_lock(root, &pruned)?;
            }
        }
    }

    Ok(removed)
}

/// STEP 1 (Claude): a minimal command driver to demonstrate the audit.rs core.
/// The flags (--json/--severity/--fix/--exit-code/--no-cache), output polish, and
/// registration in the help are the layer on top, Codex's area (see CODEX.md).
fn cmd_audit(args: &[String]) -> Result<(), String> {
    let options = audit_cli::parse(args)?;
    if options.help {
        if options.json {
            let data = json::Json::Obj(std::collections::HashMap::from([
                ("ok".into(), json::Json::Bool(true)),
                ("data".into(), json::Json::Str(audit_cli::HELP.into())),
                ("warnings".into(), json::Json::Arr(vec![])),
            ]));
            println!("{}", json::stringify_pretty(&data));
        } else {
            println!("{}", audit_cli::HELP);
        }
        return Ok(());
    }
    let report = audit::audit(
        &cwd(),
        &audit::AuditOptions {
            no_cache: options.no_cache,
            clear_cache: options.clear_cache,
        },
    )?;
    println!("{}", audit_cli::render(&report, &options));
    let code = audit_cli::exit_code(&report, &options);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
fn cmd_global(args: &[String]) -> Result<(), String> {
    // The only subcommand for now is list (and an empty call = list).
    match args.first().map(String::as_str) {
        None | Some("list") | Some("ls") => {
            let items = global::list();
            println!("\n{}", ui::heading("Global packages"));
            if items.is_empty() {
                println!(
                    "{}\n",
                    ui::step("Nothing installed yet (jet install -g <name>).")
                );
            } else {
                let w = ui::max_width(items.iter().map(|(n, _)| n.as_str()));
                for (name, version) in &items {
                    println!("{}{} {}", ui::PAD, ui::pad_right(name, w), ui::dim(version));
                }
                println!();
            }
            let bin = global::bin_dir();
            let path_state = if global::bin_dir_on_path() {
                ui::green("on PATH")
            } else {
                ui::yellow("NOT on PATH")
            };
            println!(
                "{}",
                ui::hint(&format!(
                    "Command directory: {} ({})",
                    bin.display(),
                    path_state
                ))
            );
            println!();
            Ok(())
        }
        Some(other) => Err(format!(
            "Unknown global subcommand: {other}. Available: list"
        )),
    }
}

fn print_summary(r: &installer::InstallReport) {
    let src = if r.from_lock {
        " (versions from lock)"
    } else {
        ""
    };
    println!(
        "\n{} {}{}",
        ui::green(ui::OK),
        ui::bold_green("Done"),
        ui::dim(src)
    );

    const LABELS: [&str; 3] = ["installed", "reused", "tarballs"];
    let w = ui::max_width(LABELS.into_iter());
    println!("{}", ui::field(LABELS[0], w, &r.installed.to_string()));
    println!("{}", ui::field(LABELS[1], w, &r.deduped.to_string()));
    println!(
        "{}",
        ui::field(
            LABELS[2],
            w,
            &format!("downloaded {}, from cache {}", r.downloaded, r.cached)
        )
    );

    for warning in &r.warnings {
        println!(
            "{}{} {}",
            ui::PAD,
            ui::yellow(ui::WARN),
            ui::yellow(warning)
        );
    }
    println!("\n{}\n", ui::hint("Lock file: jet-lock.json"));
}

fn cmd_create(args: &[String]) -> Result<(), String> {
    let target = args
        .first()
        .ok_or("Specify a template: jet create <name> [dir]\n  e.g.: jet create react my-app")?;
    let (name, range) = spec::split_name_range(target);
    let t = templates::resolve_create_target(&name);
    let spec_str = if range == "latest" {
        t.initializer.clone()
    } else {
        format!("{}@{}", t.initializer, range)
    };
    let mut forward = runner::strip_arg_separator(&args[1..]);

    if let Some(dir) = forward.iter().find(|a| !a.starts_with('-')) {
        let abs = cwd().join(dir);
        if abs.exists()
            && std::fs::read_dir(&abs)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
        {
            return Err(format!(
                "Directory \"{dir}\" already exists and is not empty — pick a different name."
            ));
        }
    }
    if !t.template_args.is_empty() && !templates::has_template_flag(&forward) {
        forward.extend(t.template_args);
    }
    println!(
        "\n{}",
        ui::heading(&format!("Creating project — {}", t.initializer))
    );
    if let Some(note) = t.note {
        println!("{}{} {}", ui::PAD, ui::yellow(ui::WARN), ui::yellow(&note));
    }
    println!();
    let code = runner::run_package_bin(&spec_str, &forward, &cwd())?;
    std::process::exit(code);
}

fn cmd_exec(args: &[String]) -> Result<(), String> {
    let target = args
        .first()
        .ok_or("Specify a package: jet exec <pkg> [args]")?;
    let forward = runner::strip_arg_separator(&args[1..]);
    let code = runner::run_package_bin(target, &forward, &cwd())?;
    std::process::exit(code);
}

fn cmd_tree(args: &[String]) -> Result<(), String> {
    let deps: Vec<(String, String)> = match args.iter().find(|a| !a.starts_with('-')) {
        Some(t) => vec![spec::split_name_range(t)],
        None => {
            let m = project::read_manifest(&cwd()).ok_or("package.json not found")?;
            project::collect_dependencies(&m)
        }
    };
    let res = resolver::resolve(&deps, |name| registry::fetch_packument(name));
    let mut seen = std::collections::HashSet::new();
    println!("\n{}", ui::heading("Dependency tree"));
    for (i, key) in res.roots.iter().enumerate() {
        render_tree(
            key,
            &res.flat,
            ui::PAD,
            i == res.roots.len() - 1,
            true,
            &mut seen,
        );
    }
    println!(
        "\n{}",
        ui::field(
            "unique packages",
            18,
            &ui::bold(&res.flat.len().to_string())
        )
    );
    for w in &res.warnings {
        println!("{}{} {}", ui::PAD, ui::yellow(ui::WARN), ui::yellow(w));
    }
    println!();
    Ok(())
}

fn render_tree(
    key: &str,
    flat: &std::collections::HashMap<String, resolver::ResolvedNode>,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    seen: &mut std::collections::HashSet<String>,
) {
    let Some(node) = flat.get(key) else { return };
    let connector = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };
    let dup = seen.contains(key) && !node.dependencies.is_empty();
    // The root is bold; a repeated subtree gets a dimmed note.
    let label = if is_root {
        ui::bold(key)
    } else {
        key.to_string()
    };
    let mark = if dup {
        ui::dim(" (already shown)")
    } else {
        String::new()
    };
    println!("{prefix}{}{label}{mark}", ui::dim(connector));
    seen.insert(key.to_string());
    if dup {
        return;
    }
    // The root's branches use the same indent as the root itself.
    let child_prefix = if is_root {
        prefix.to_string()
    } else {
        format!("{prefix}{}", if is_last { "   " } else { "│  " })
    };
    for (i, child) in node.dependencies.iter().enumerate() {
        render_tree(
            child,
            flat,
            &child_prefix,
            i == node.dependencies.len() - 1,
            false,
            seen,
        );
    }
}

fn cmd_history() -> Result<(), String> {
    let events = history::read_history(&history::default_dir());
    println!("\n{}", ui::heading("Install history"));
    if events.is_empty() {
        println!("{}", ui::step("Nothing yet."));
        println!(
            "\n{}\n",
            ui::hint("Install something:  jet install <name>")
        );
        return Ok(());
    }
    let recent: Vec<_> = events.iter().rev().take(20).collect();
    println!(
        "{}\n",
        ui::dim(&format!(
            "{}last {} of {}",
            ui::PAD,
            recent.len(),
            events.len()
        ))
    );

    // name@version in one column, size next, right-aligned.
    let rows: Vec<(String, String)> = recent
        .iter()
        .map(|e| {
            (
                format!("{}@{}", e.name, e.version),
                e.bytes.map(format_bytes).unwrap_or_else(|| "—".into()),
            )
        })
        .collect();
    let nw = ui::max_width(rows.iter().map(|(n, _)| n.as_str()));
    let sw = ui::max_width(rows.iter().map(|(_, s)| s.as_str()));
    for (name, size) in &rows {
        println!(
            "{}{}  {}",
            ui::PAD,
            ui::pad_right(name, nw),
            ui::dim(&ui::pad_left(size, sw))
        );
    }
    println!();
    Ok(())
}

fn cmd_registry(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("get") => {
            println!("{}", config::get_registry());
            Ok(())
        }
        Some("set") | Some("use") => {
            let arg = args.get(1).ok_or(
                "Specify a mirror: jet registry set <url | name | number>\n  e.g.: jet registry use yarn  |  jet registry set 2",
            )?;
            apply_registry(arg)
        }
        Some("reset") => {
            config::set_registry(config::DEFAULT_REGISTRY)?;
            println!(
                "\n{} Registry reset to the official {}\n",
                ui::green(ui::OK),
                ui::cyan(&config::get_registry())
            );
            Ok(())
        }
        Some("mirrors") => {
            let current = config::get_registry();
            println!("\n{}", ui::heading("Registry mirrors"));
            println!("{}\n", ui::field("current", 7, &ui::cyan(&current)));

            // Columns are sized from the data so the table won't skew when mirrors are added.
            let kw = ui::max_width(mirrors::MIRRORS.iter().map(|m| m.key));
            let nw = ui::max_width(mirrors::MIRRORS.iter().map(|m| m.name));
            for (i, m) in mirrors::MIRRORS.iter().enumerate() {
                let active = m.url.trim_end_matches('/') == current.trim_end_matches('/');
                let mark = if active {
                    ui::green(ui::ARROW)
                } else {
                    " ".to_string()
                };
                let key = ui::pad_right(m.key, kw);
                let key = if active {
                    ui::green(&key)
                } else {
                    ui::bold(&key)
                };
                println!(
                    "{}{mark} {} {key}  {}  {}",
                    ui::PAD,
                    ui::dim(&format!("{}.", i + 1)),
                    ui::pad_right(m.name, nw),
                    ui::dim(m.url)
                );
                // The note goes on a second line, under the mirror's name.
                println!("{}     {}{}", ui::PAD, " ".repeat(kw + 1), ui::dim(m.note));
            }
            if mirrors::MIRRORS.iter().any(|m| mirrors::is_insecure(m.url)) {
                println!(
                    "\n{} {}",
                    ui::yellow(ui::WARN),
                    ui::yellow(
                        "HTTP mirrors are flagged in their note: the connection to them is \
                         unencrypted, but tarball integrity is still verified via sha512."
                    )
                );
            }
            println!(
                "\n{}",
                ui::hint("On a network failure jet switches to a live mirror on its own.")
            );
            println!(
                "\n{}",
                ui::hint("Switch:  jet registry use <name>   or   jet registry set <number>")
            );
            println!("{}\n", ui::hint("Check:   jet registry check"));
            Ok(())
        }
        Some("check") => {
            println!("\n{}", ui::heading("Registry and mirror availability"));
            println!("{}\n", ui::step("Checking..."));
            let results = mirrors::check_all(&config::get_registry());
            let nw = ui::max_width(results.iter().map(|r| r.name.as_str()));
            let uw = ui::max_width(results.iter().map(|r| r.url.as_str()));
            let mut reachable = Vec::new();
            for r in results {
                let mark = if r.ok {
                    ui::green(ui::OK)
                } else {
                    ui::red(ui::FAIL)
                };
                let ms = format!("{} ms", r.ms);
                println!(
                    "{}{mark} {}  {}  {}",
                    ui::PAD,
                    ui::pad_right(&r.name, nw),
                    ui::dim(&ui::pad_right(&r.url, uw)),
                    ui::dim(&ui::pad_left(&ms, 7))
                );
                if r.ok {
                    reachable.push(r);
                }
            }
            if let Some(best) = reachable.iter().min_by_key(|r| r.ms) {
                // Suggest the short command with the mirror's name, if it's a known one.
                let known = mirrors::MIRRORS
                    .iter()
                    .find(|m| m.url.trim_end_matches('/') == best.url.trim_end_matches('/'));
                let cmd = known
                    .map(|m| format!("jet registry use {}", m.key))
                    .unwrap_or_else(|| format!("jet registry set {}", best.url));
                // The current registry's name already has "(current)" — use the clean mirror name.
                let name = known.map(|m| m.name).unwrap_or(&best.name);
                println!(
                    "\n{} Fastest: {} {}",
                    ui::green(ui::OK),
                    ui::bold(name),
                    ui::dim(&format!("({} ms)", best.ms))
                );
                println!("{}", ui::hint(&format!("Switch:  {cmd}")));
            }
            println!();
            Ok(())
        }
        Some(other) => Err(format!(
            "Unknown registry subcommand: {other}\n  available: get, set (use), reset, mirrors, check"
        )),
    }
}

/// Change the registry from user input (url | mirror name | number).
/// The shared entry point for `jet registry set` and `jet set registry`.
fn apply_registry(input: &str) -> Result<(), String> {
    // A mirror name/number -> its URL; otherwise treat the input as a URL.
    let url = mirrors::resolve_input(input)
        .map(|m| m.url.to_string())
        .unwrap_or_else(|| input.to_string());
    config::set_registry(&url)?;
    let now = config::get_registry();
    println!(
        "\n{} Registry changed to {}",
        ui::green(ui::OK),
        ui::cyan(&now)
    );
    // HTTP mirror — warn that the channel is unencrypted (integrity still protects you).
    if let Some(w) = mirrors::insecure_warning(&now) {
        println!("{} {}", ui::yellow(ui::WARN), ui::yellow(&w));
    }
    println!();
    Ok(())
}

/// What `jet set ...` / `jet get ...` is asked to change/show.
#[derive(Debug, PartialEq)]
enum Setting {
    /// A value exists only for `set` with an argument; otherwise we show the current one.
    Registry(Option<String>),
    Proxy(Option<String>),
    AuditTimeout(Option<String>),
}

/// Parse the arguments of `jet set <key> [value]` and `jet get <key>`.
/// A pure function — all validation is here, no side effects.
fn parse_setting(verb: &str, args: &[String]) -> Result<Setting, String> {
    match args.first().map(String::as_str) {
        None => Err(format!(
            "Specify a setting: jet {verb} <registry|proxy|audit.timeout>{}\n  e.g.: jet set registry yarn  |  jet get registry",
            if verb == "set" {
                " [value]"
            } else {
                ""
            }
        )),
        Some("registry" | "reg" | "r") => Ok(Setting::Registry(args.get(1).cloned())),
        Some("proxy") => Ok(Setting::Proxy(args.get(1).cloned())),
        Some("audit.timeout") => Ok(Setting::AuditTimeout(args.get(1).cloned())),
        Some(other) => {
            let mut msg = format!("Unknown setting: {other}");
            if complete::suggest_setting(other).is_some() {
                msg.push_str(&format!(
                    "\n  Did you mean:  jet {verb} registry"
                ));
            }
            msg.push_str("\n  Available settings: registry, proxy, audit.timeout");
            Err(msg)
        }
    }
}

fn cmd_set(args: &[String]) -> Result<(), String> {
    match parse_setting("set", args)? {
        Setting::Registry(Some(value)) => apply_registry(&value),
        Setting::Registry(None) => {
            println!("Current registry: {}", config::get_registry());
            println!("\nSwitch:  jet set registry <url | name | number>");
            println!("  e.g.:  jet set registry yarn  |  jet set registry 2");
            println!("List of mirrors:  jet registry mirrors");
            Ok(())
        }
        Setting::Proxy(Some(value)) => {
            config::set_value("proxy", value.trim())?;
            println!(
                "\n{} Proxy set: {}",
                ui::green(ui::OK),
                ui::cyan(value.trim())
            );
            Ok(())
        }
        Setting::Proxy(None) => {
            match config::proxy() {
                Some(p) => println!("Current proxy: {p}"),
                None => println!(
                    "No proxy set (using HTTP_PROXY / HTTPS_PROXY from the environment)."
                ),
            }
            println!("\nSet it:  jet set proxy http://host:port");
            Ok(())
        }
        Setting::AuditTimeout(Some(value)) => {
            let n: u64 = value.trim().parse().map_err(|_| {
                "The timeout is a number of seconds, e.g.: jet set audit.timeout 60".to_string()
            })?;
            config::set_value("audit.timeout", &n.to_string())?;
            println!(
                "\n{} audit timeout: {} s",
                ui::green(ui::OK),
                ui::cyan(&config::audit_timeout().to_string())
            );
            Ok(())
        }
        Setting::AuditTimeout(None) => {
            println!("audit timeout: {} s", config::audit_timeout());
            println!("\nSet it:  jet set audit.timeout 60");
            Ok(())
        }
    }
}

fn cmd_get(args: &[String]) -> Result<(), String> {
    match parse_setting("get", args)? {
        Setting::Registry(_) => println!("{}", config::get_registry()),
        Setting::Proxy(_) => println!("{}", config::proxy().unwrap_or_default()),
        Setting::AuditTimeout(_) => println!("{}", config::audit_timeout()),
    }
    Ok(())
}

fn cmd_cache(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("clear") => {
            let t = cache::clear();
            let s = store::clear();
            println!("\n{} {}", ui::green(ui::OK), ui::bold_green("Cache cleared"));
            println!("{}", ui::field("tarballs", 9, &t.to_string()));
            println!(
                "{}\n",
                ui::field("in store", 9, &ui::plural(s, "version", "versions"))
            );
            Ok(())
        }
        _ => {
            let (count, bytes) = cache::stats();
            const LABELS: [&str; 3] = ["path", "tarballs", "size"];
            let w = ui::max_width(LABELS.into_iter());
            println!("\n{}", ui::heading("Tarball cache"));
            println!(
                "{}",
                ui::field(
                    LABELS[0],
                    w,
                    &ui::cyan(&config::cache_dir().display().to_string())
                )
            );
            println!("{}", ui::field(LABELS[1], w, &count.to_string()));
            println!("{}", ui::field(LABELS[2], w, &format_bytes(bytes)));
            println!("\n{}\n", ui::hint("Clear:  jet cache clear"));
            Ok(())
        }
    }
}

/// The tool version. In a terminal — branded (`jet 0.1.0`); in a pipe — the bare
/// version string, so a script can read it easily (like `npm -v`).
fn print_version() {
    let v = env!("CARGO_PKG_VERSION");
    if std::io::stdout().is_terminal() {
        println!("{} {}", ui::bold("jet"), ui::cyan(v));
    } else {
        println!("{v}");
    }
}

fn print_help() {
    println!(
        "\n{} {}",
        ui::bold("jet"),
        ui::dim("— a native package manager (Rust)")
    );
    println!(
        "\n{}{}  jet {}",
        ui::PAD,
        ui::dim("Usage:"),
        ui::cyan("<command> [args]")
    );

    // Columns are shared across all sections, otherwise the descriptions stair-step.
    let commands = complete::known_commands();
    let names = ui::max_width(commands.iter().map(|c| c.name));
    let aliases: Vec<String> = commands
        .iter()
        .map(|c| c.aliases.join(", "))
        .collect();
    let alw = ui::max_width(aliases.iter().map(String::as_str));

    for group in complete::HELP_GROUPS {
        println!("\n{}", ui::bold(group));
        for (c, al) in commands.iter().zip(&aliases) {
            if &c.group != group {
                continue;
            }
            println!(
                "{}{}  {}  {}",
                ui::PAD,
                ui::green(&ui::pad_right(c.name, names)),
                ui::dim(&ui::pad_right(al, alw)),
                c.summary
            );
        }
    }

    println!("\n{}", ui::bold("Examples"));
    for (cmd, what) in [
        (
            "jet install react",
            "install a package and record it in package.json",
        ),
        (
            "jet install",
            "install everything from package.json (using the lock file)",
        ),
        (
            "jet install -g typescript",
            "install a CLI globally (command on PATH)",
        ),
        ("jet create react my-app", "create a new project"),
        ("jet info is-number", "a package's versions and dependencies"),
        ("jet registry check", "find the fastest mirror"),
    ] {
        println!(
            "{}{}  {}",
            ui::PAD,
            ui::cyan(&ui::pad_right(cmd, 24)),
            ui::dim(what)
        );
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn explicit_install_keeps_existing_dependencies() {
        let existing = vec![
            ("alpha".to_string(), "^1.0.0".to_string()),
            ("beta".to_string(), "~2.0.0".to_string()),
        ];
        let requested = vec![
            ("beta".to_string(), "3.0.0".to_string()),
            ("gamma".to_string(), "latest".to_string()),
        ];

        assert_eq!(
            merge_install_requests(existing, &requested),
            vec![
                ("alpha".to_string(), "^1.0.0".to_string()),
                ("beta".to_string(), "3.0.0".to_string()),
                ("gamma".to_string(), "latest".to_string()),
            ]
        );
    }

    #[test]
    fn set_registry_with_value() {
        assert_eq!(
            parse_setting("set", &args(&["registry", "yarn"])),
            Ok(Setting::Registry(Some("yarn".into())))
        );
        // Key aliases work the same as on the registry command.
        assert_eq!(
            parse_setting("set", &args(&["reg", "2"])),
            Ok(Setting::Registry(Some("2".into())))
        );
    }

    #[test]
    fn set_registry_without_value() {
        assert_eq!(
            parse_setting("set", &args(&["registry"])),
            Ok(Setting::Registry(None))
        );
        assert_eq!(
            parse_setting("get", &args(&["registry"])),
            Ok(Setting::Registry(None))
        );
    }

    #[test]
    fn set_without_key_explains_usage() {
        let err = parse_setting("set", &[]).unwrap_err();
        assert!(err.contains("registry|proxy|audit.timeout"));
        assert!(err.contains("[value]"));
        // get has no value — the hint is shorter.
        let err = parse_setting("get", &[]).unwrap_err();
        assert!(err.contains("registry|proxy|audit.timeout"));
        assert!(!err.contains("[value]"));
    }

    #[test]
    fn set_parses_proxy_and_audit_timeout() {
        assert_eq!(
            parse_setting("set", &args(&["proxy", "http://h:8080"])),
            Ok(Setting::Proxy(Some("http://h:8080".into())))
        );
        assert_eq!(
            parse_setting("set", &args(&["audit.timeout", "60"])),
            Ok(Setting::AuditTimeout(Some("60".into())))
        );
        assert_eq!(
            parse_setting("get", &args(&["proxy"])),
            Ok(Setting::Proxy(None))
        );
    }

    #[test]
    fn unknown_setting_errors_with_hint() {
        let err = parse_setting("set", &args(&["registy"])).unwrap_err();
        assert!(err.contains("Unknown setting: registy"));
        assert!(err.contains("jet set registry"));

        // A completely dissimilar key — no "did you mean", but with the list.
        let err = parse_setting("set", &args(&["zzzzzzzz"])).unwrap_err();
        assert!(err.contains("Available settings: registry"));
        assert!(!err.contains("Did you mean"));
    }
}
