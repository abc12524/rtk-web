use rtk_engine::block::ContentFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatSpec {
    Auto,
    Known(ContentFormat),
}

impl FormatSpec {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "term" | "terminal" | "shell" | "text" => Some(Self::Known(ContentFormat::Term)),
            "json" => Some(Self::Known(ContentFormat::Json)),
            "web" | "html" => Some(Self::Known(ContentFormat::Web)),
            "matches" | "grep" | "search" => Some(Self::Known(ContentFormat::Matches)),
            "lockfile" => Some(Self::Known(ContentFormat::Lockfile)),
            _ => None,
        }
    }
}

const JSON_SNIFF_LIMIT: usize = 1024 * 1024;

pub fn detect(text: &str) -> ContentFormat {
    let head = text.trim_start();

    if text.len() <= JSON_SNIFF_LIMIT
        && (head.starts_with('{') || head.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(text).is_ok()
    {
        return ContentFormat::Json;
    }

    let lower = head.get(..15).unwrap_or(head).to_ascii_lowercase();
    if lower.starts_with("<!doctype") || lower.starts_with("<html") {
        return ContentFormat::Web;
    }

    if looks_like_matches(text) {
        return ContentFormat::Matches;
    }

    if looks_like_lockfile(text) {
        return ContentFormat::Lockfile;
    }

    ContentFormat::Term
}

pub fn detect_from_filename(name: &str) -> FormatSpec {
    let lower = name.to_ascii_lowercase();
    let base = lower
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(lower.as_str());

    if base == "cargo.lock" || base.ends_with(".lock") {
        FormatSpec::Known(ContentFormat::Lockfile)
    } else if base.ends_with(".json") {
        FormatSpec::Known(ContentFormat::Json)
    } else if base.ends_with(".html") || base.ends_with(".htm") {
        FormatSpec::Known(ContentFormat::Web)
    } else {
        FormatSpec::Known(ContentFormat::Term)
    }
}

fn looks_like_matches(text: &str) -> bool {
    let mut total = 0usize;
    let mut hits = 0usize;
    for line in text.lines().take(200) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        total += 1;
        if is_match_line(trimmed) {
            hits += 1;
        }
    }
    total >= 3 && hits * 100 >= total * 60
}

fn is_match_line(line: &str) -> bool {
    let Some((path, rest)) = line.split_once(':') else {
        return false;
    };
    if path.is_empty() || path.contains(' ') {
        return false;
    }
    let Some((line_no, _)) = rest.split_once(':') else {
        return false;
    };
    !line_no.is_empty() && line_no.chars().all(|c| c.is_ascii_digit())
}

fn looks_like_lockfile(text: &str) -> bool {
    let head: String = text.lines().take(8).collect::<Vec<_>>().join("\n");
    if head.contains("package-lock.json") {
        return false;
    }
    let cargo_lock = head.contains("name = ") && head.contains("version = ") && head.contains("source = ");
    let yarn_lock = head.contains("# yarn lockfile v1") || head.contains("__metadata:");
    cargo_lock || yarn_lock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_wins_when_it_parses() {
        assert_eq!(detect("{\n  \"a\": 1\n}"), ContentFormat::Json);
        assert_eq!(detect("[1, 2, 3]"), ContentFormat::Json);
    }

    #[test]
    fn html_is_web() {
        assert_eq!(detect("<!DOCTYPE html><html></html>"), ContentFormat::Web);
    }

    #[test]
    fn grep_like_output_is_matches() {
        let text = "src/a.rs:1:fn a() {}\nsrc/a.rs:9:fn b() {}\nsrc/c.rs:4:fn c() {}";
        assert_eq!(detect(text), ContentFormat::Matches);
    }

    #[test]
    fn cargo_lock_is_lockfile() {
        let text = "[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        assert_eq!(detect(text), ContentFormat::Lockfile);
    }

    #[test]
    fn shell_output_defaults_to_term() {
        let text = "warning: unused variable\n[x12]\ncompiling crate v0.1.0\nerror: failed";
        assert_eq!(detect(text), ContentFormat::Term);
    }

    #[test]
    fn filenames_select_a_format() {
        assert_eq!(
            detect_from_filename("build.log"),
            FormatSpec::Known(ContentFormat::Term)
        );
        assert_eq!(
            detect_from_filename("data.json"),
            FormatSpec::Known(ContentFormat::Json)
        );
        assert_eq!(
            detect_from_filename("page.HTML"),
            FormatSpec::Known(ContentFormat::Web)
        );
        assert_eq!(
            detect_from_filename("Cargo.lock"),
            FormatSpec::Known(ContentFormat::Lockfile)
        );
        assert_eq!(
            detect_from_filename("dir\\yarn.lock"),
            FormatSpec::Known(ContentFormat::Lockfile)
        );
    }

    #[test]
    fn format_aliases_parse() {
        assert_eq!(FormatSpec::parse("pwsh"), None);
        assert_eq!(FormatSpec::parse("shell"), Some(FormatSpec::Known(ContentFormat::Term)));
        assert_eq!(FormatSpec::parse("auto"), Some(FormatSpec::Auto));
    }
}