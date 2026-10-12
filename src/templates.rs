//! Template map for `create` — a port of templates.ts.
//! The deprecated create-react-app is routed to the live create-vite.

pub struct CreateTarget {
    pub initializer: String,
    pub template_args: Vec<String>,
    pub note: Option<String>,
}

const CRA_NOTE: &str =
    "create-react-app is deprecated — creating a React project via Vite (the same thing, but alive).";

fn tmpl(t: &str) -> Vec<String> {
    vec!["--template".to_string(), t.to_string()]
}

/// The initializer name: react-app -> create-react-app; @scope/x -> @scope/create-x.
pub fn initializer_package(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('@') {
        if let Some((scope, pkg)) = rest.split_once('/') {
            if pkg.starts_with("create-") {
                return name.to_string();
            }
            return format!("@{scope}/create-{pkg}");
        }
        return format!("{name}/create");
    }
    if name.starts_with("create-") {
        name.to_string()
    } else {
        format!("create-{name}")
    }
}

pub fn resolve_create_target(name: &str) -> CreateTarget {
    match name.to_lowercase().as_str() {
        "react" | "react-app" | "cra" => CreateTarget {
            initializer: "create-vite".into(),
            template_args: tmpl("react"),
            note: Some(CRA_NOTE.into()),
        },
        "react-ts" => CreateTarget {
            initializer: "create-vite".into(),
            template_args: tmpl("react-ts"),
            note: Some(CRA_NOTE.into()),
        },
        "vue" => CreateTarget {
            initializer: "create-vite".into(),
            template_args: tmpl("vue"),
            note: None,
        },
        "vue-ts" => CreateTarget {
            initializer: "create-vite".into(),
            template_args: tmpl("vue-ts"),
            note: None,
        },
        "svelte" => CreateTarget {
            initializer: "create-vite".into(),
            template_args: tmpl("svelte"),
            note: None,
        },
        "vanilla" => CreateTarget {
            initializer: "create-vite".into(),
            template_args: tmpl("vanilla"),
            note: None,
        },
        "next" | "next-app" => CreateTarget {
            initializer: "create-next-app".into(),
            template_args: vec![],
            note: None,
        },
        other => CreateTarget {
            initializer: initializer_package(other),
            template_args: vec![],
            note: None,
        },
    }
}

/// Whether the user provided their own --template.
pub fn has_template_flag(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--template" || a == "-t" || a.starts_with("--template="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        assert_eq!(
            resolve_create_target("react-app").initializer,
            "create-vite"
        );
        assert_eq!(
            resolve_create_target("react").template_args,
            vec!["--template", "react"]
        );
        assert!(resolve_create_target("react").note.is_some());
        assert_eq!(resolve_create_target("next").initializer, "create-next-app");
        assert_eq!(resolve_create_target("foo").initializer, "create-foo");
        assert_eq!(initializer_package("@my/thing"), "@my/create-thing");
    }
}
