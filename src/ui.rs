//! Unified CLI output style: ANSI palette, symbols, and layout helpers.
//!
//! Colors are printed only when the stream is a real terminal (and NO_COLOR
//! is not set); otherwise redirecting to a file would leave escape sequences
//! in it. The decision is made separately for stdout and stderr and cached
//! for the process.

use std::io::IsTerminal;
use std::sync::OnceLock;

// --- SGR codes. Not exposed: only through the functions below. ---
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

// --- Symbols. The same across every command. ---
/// Success.
pub const OK: &str = "✓";
/// Failure.
pub const FAIL: &str = "✗";
/// Section / current-item marker.
pub const BULLET: &str = "◆";
/// Step or hint.
pub const ARROW: &str = "→";
/// Warning.
pub const WARN: &str = "⚠";

/// Indent for content inside a section.
pub const PAD: &str = "  ";

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

fn stdout_colors() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !no_color() && std::io::stdout().is_terminal())
}

fn stderr_colors() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !no_color() && std::io::stderr().is_terminal())
}

fn paint(on: bool, code: &str, s: &str) -> String {
    if on {
        format!("{code}{s}{RESET}")
    } else {
        s.to_string()
    }
}

// --- Palette for stdout. These can't be nested (the inner RESET clears the
// outer style) — combinations have their own dedicated functions. ---

/// Heading, package name — bold.
pub fn bold(s: &str) -> String {
    paint(stdout_colors(), BOLD, s)
}

/// Secondary: labels, hints, paths.
pub fn dim(s: &str) -> String {
    paint(stdout_colors(), DIM, s)
}

/// Success.
pub fn green(s: &str) -> String {
    paint(stdout_colors(), GREEN, s)
}

/// Error.
pub fn red(s: &str) -> String {
    paint(stdout_colors(), RED, s)
}

/// Warning.
pub fn yellow(s: &str) -> String {
    paint(stdout_colors(), YELLOW, s)
}

/// Accent: commands, URLs, values.
pub fn cyan(s: &str) -> String {
    paint(stdout_colors(), CYAN, s)
}

/// Bold green — for the final "Done" line.
pub fn bold_green(s: &str) -> String {
    paint(stdout_colors(), &format!("{BOLD}{GREEN}"), s)
}

// --- Prebuilt lines ---

/// Section heading: `◆ Title` in bold.
pub fn heading(title: &str) -> String {
    format!("{} {}", cyan(BULLET), bold(title))
}

/// Progress step: `  → text`, dimmed.
pub fn step(text: &str) -> String {
    format!("{PAD}{} {}", dim(ARROW), dim(text))
}

/// A "label — value" pair with the label padded to `width` characters.
pub fn field(label: &str, width: usize, value: &str) -> String {
    format!("{PAD}{}  {value}", dim(&pad_right(label, width)))
}

/// Hint at the bottom of a block: `  → how to do it`.
pub fn hint(text: &str) -> String {
    format!("{PAD}{} {}", dim(ARROW), dim(text))
}

/// Pad a string on the right with spaces up to `width` visible characters.
/// Counted in characters (not bytes) — otherwise wide chars break the columns.
pub fn pad_right(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// Pad a string on the left with spaces up to `width` visible characters —
/// for columns of numbers and sizes, which look tidier right-aligned.
pub fn pad_left(s: &str, width: usize) -> String {
    let len = s.chars().count();
    format!("{}{s}", " ".repeat(width.saturating_sub(len)))
}

/// Neutralize untrusted text before printing it to the terminal (protection
/// against ANSI injection). Fields from a foreign API/registry (vulnerability
/// description, name) may carry escape sequences that recolor the terminal or
/// hide text. We strip ESC and other control characters, keeping newline and tab.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || ((c as u32) >= 0x20 && c != '\u{7f}'))
        .collect()
}

/// Pluralize a noun with its count: `1 package`, `2 packages`.
/// English has two forms — singular (n == 1) and plural (everything else).
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Width of the longest string in a set — for table columns.
pub fn max_width<'a>(items: impl Iterator<Item = &'a str>) -> usize {
    items.map(|s| s.chars().count()).max().unwrap_or(0)
}

// --- stderr: its own decision about colors ---

/// Error heading on stderr.
pub fn eprint_error(message: &str) {
    let head = paint(
        stderr_colors(),
        &format!("{BOLD}{RED}"),
        &format!("{FAIL} Error"),
    );
    eprintln!("\n{head}  {message}");
}

/// A hint line on stderr.
pub fn eprint_hint(text: &str) {
    let arrow = paint(stderr_colors(), DIM, ARROW);
    let text = paint(stderr_colors(), DIM, text);
    eprintln!("{PAD}{arrow} {text}");
}

/// Arbitrary text in red on stderr.
pub fn ered(s: &str) -> String {
    paint(stderr_colors(), RED, s)
}

/// Arbitrary text dimmed on stderr.
pub fn edim(s: &str) -> String {
    paint(stderr_colors(), DIM, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_counts_chars_not_bytes() {
        // Multi-byte chars: 4 characters but 5 bytes — pad to 4 visible columns.
        assert_eq!(pad_right("café", 6), "café  ");
        assert_eq!(pad_right("npm", 6), "npm   ");
        // A string longer than the width is neither truncated nor padded.
        assert_eq!(pad_right("very-long-name", 4), "very-long-name");
        // Same on the left — the size column.
        assert_eq!(pad_left("3.4 MB", 8), "  3.4 MB");
        assert_eq!(pad_left("—", 3), "  —");
        assert_eq!(pad_left("longish", 2), "longish");
    }

    #[test]
    fn english_plurals() {
        let dep = |n| plural(n, "dependency", "dependencies");
        assert_eq!(dep(1), "1 dependency");
        assert_eq!(dep(2), "2 dependencies");
        assert_eq!(dep(5), "5 dependencies");
        assert_eq!(dep(0), "0 dependencies");
        assert_eq!(dep(11), "11 dependencies");
        assert_eq!(dep(21), "21 dependencies");
    }

    #[test]
    fn widest_column() {
        assert_eq!(max_width(["npm", "npmmirror", "yarn"].into_iter()), 9);
        assert_eq!(max_width(["mirror"].into_iter()), 6);
        assert_eq!(max_width([].into_iter()), 0);
    }

    #[test]
    fn paint_respects_the_switch() {
        // Off (not a terminal / NO_COLOR) — not a single escape sequence.
        assert_eq!(paint(false, BOLD, "done"), "done");
        assert_eq!(paint(false, RED, "error"), "error");
        // On — style around the text and a mandatory reset at the end.
        let s = paint(true, GREEN, "ok");
        assert_eq!(s, "\x1b[32mok\x1b[0m");
        assert!(s.ends_with(RESET));
    }
}
