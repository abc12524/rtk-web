use rtk_engine::block::{Block, ContentFormat, Origin, Provenance, Subject};
use rtk_engine::chain::run_chain_guarded;
use rtk_engine::method::{Method, MethodCtx};
use rtk_engine::methods::{ANSI, DEDUP, chain_for};
use rtk_engine::strategy;
use rtk_engine::tokens::estimate_tokens;
use serde::Serialize;

static LOSSLESS_CHAIN: [&dyn Method; 2] = [&ANSI, &DEDUP];

/// Recognize command-specific output shapes in `auto` mode.
///
/// shell-tool and similar clients only send `mode=auto` and have no way to
/// know which command produced the text, so they never pass `filter=ls`.
/// The generic term chain (ansi/progress/dedup/cap) leaves a short `ls -l`
/// listing untouched; the `ls` filter is the only thing that compacts it.
/// Auto mode therefore sniffs the strongest signature — a permissions field
/// at the start of each non-header line — and routes the block through the
/// matching filter. `rtk::filter_api::apply` is panic-safe and never_worse,
/// so a false positive can only leave the text unchanged.
fn detect_command_filter(input: &str) -> Option<&'static str> {
    let mut nonblank = 0usize;
    let mut perm_lines = 0usize;
    for line in input.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with("total ") || line.starts_with("总计") {
            continue;
        }
        nonblank += 1;
        let bytes = line.as_bytes();
        let perms_ok = bytes.len() >= 11
            && matches!(bytes[0], b'-' | b'd' | b'l' | b'c' | b'b' | b'p' | b's')
            && bytes[1..10]
                .iter()
                .all(|c| matches!(c, b'r' | b'w' | b'x' | b's' | b'S' | b't' | b'T' | b'-'));
        if perms_ok {
            // GNU ls may append an ACL/SELinux marker (`+`, `.`, `@`) before
            // the column separator; accept an optional marker then whitespace.
            let rest = &bytes[10..];
            let separated = rest.first().is_some_and(|c| c.is_ascii_whitespace())
                || (matches!(rest.first(), Some(b'+' | b'.' | b'@'))
                    && rest.get(1).is_some_and(|c| c.is_ascii_whitespace()));
            if separated {
                perm_lines += 1;
            }
        }
    }
    if nonblank > 0 && perm_lines == nonblank {
        Some("ls")
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chain,
    Auto,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chain" | "default" => Some(Self::Chain),
            "auto" | "shape" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chain => "chain",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressOptions {
    pub format: ContentFormat,
    pub mode: Mode,
    pub lossless: bool,
    pub filter: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompressOutput {
    pub text: String,
    pub chain: Vec<String>,
    pub lossy: bool,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub bytes_saved: usize,
    pub saved_percent: f64,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

impl Stats {
    pub fn new(before: &str, after: &str) -> Self {
        let bytes_before = before.len();
        let bytes_after = after.len();
        let bytes_saved = bytes_before.saturating_sub(bytes_after);
        let saved_percent = if bytes_before == 0 {
            0.0
        } else {
            (bytes_saved as f64 * 1000.0 / bytes_before as f64).round() / 10.0
        };
        Self {
            bytes_before,
            bytes_after,
            bytes_saved,
            saved_percent,
            tokens_before: estimate_tokens(before),
            tokens_after: estimate_tokens(after),
        }
    }
}

pub fn compress(input: &str, options: CompressOptions) -> CompressOutput {
    if let Some(name) = options.filter.as_deref() {
        if let Some(text) = rtk::filter_api::apply(name, input) {
            return CompressOutput {
                changed: text != input,
                text,
                chain: vec![format!("filter:{name}")],
                lossy: true,
            };
        }
    }

    if options.lossless {
        return run(input, &LOSSLESS_CHAIN, false);
    }

    if options.mode == Mode::Auto {
        if let Some(name) = detect_command_filter(input) {
            if let Some(text) = rtk::filter_api::apply(name, input) {
                if text.len() < input.len() {
                    return CompressOutput {
                        text,
                        chain: vec![format!("auto>filter:{name}")],
                        lossy: true,
                        changed: true,
                    };
                }
            }
        }
        if let Some(rendered) = strategy::render(input, 0, estimate_tokens) {
            if rendered.body.len() < input.len() {
                return CompressOutput {
                    text: rendered.body,
                    chain: vec![format!("shape:{}", rendered.meta.decided_by)],
                    lossy: true,
                    changed: true,
                };
            }
        }
    }

    let chain = chain_for(options.format);
    let lossy = chain.iter().any(|method| method.info().lossy);
    run(input, chain, lossy)
}

fn run(input: &str, methods: &[&dyn Method], lossy: bool) -> CompressOutput {
    let origin = Origin::default();
    let provenance = Provenance::default();
    let block = Block {
        text: input,
        subject: Subject::ToolResult,
        format: ContentFormat::Term,
        origin: &origin,
        provenance: &provenance,
    };
    let ctx = MethodCtx::new("rtk-web");
    let chain: Vec<String> = methods.iter().map(|method| method.name().to_string()).collect();

    match run_chain_guarded(methods, &block, &ctx) {
        Some(text) => CompressOutput {
            changed: text != input,
            text,
            chain,
            lossy,
        },
        None => CompressOutput {
            text: input.to_string(),
            chain,
            lossy,
            changed: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(lossless: bool, mode: Mode) -> CompressOptions {
        CompressOptions {
            format: ContentFormat::Term,
            mode,
            lossless,
            filter: None,
        }
    }

    #[test]
    fn named_filter_runs_the_command_specific_parser() {
        let raw = "total 8\n\
                   drwxr-xr-x  2 root root 4096 Nov  7 10:00 src\n\
                   -rw-r--r--  1 root root  341 Nov  7 09:00 hello.go\n";
        let out = compress(
            raw,
            CompressOptions {
                format: ContentFormat::Term,
                mode: Mode::Chain,
                lossless: false,
                filter: Some("ls".to_string()),
            },
        );
        assert!(out.text.contains("755  src/"), "{}", out.text);
        assert!(out.text.contains("644  hello.go"), "{}", out.text);
        assert_eq!(out.chain, vec!["filter:ls".to_string()]);
    }

    #[test]
    fn ansisequences_are_removed() {
        let out = compress("\x1b[31mERROR\x1b[0m failed", term(false, Mode::Chain));
        assert_eq!(out.text, "ERROR failed");
        assert!(out.changed);
    }

    #[test]
    fn repeated_lines_are_folded_with_a_count() {
        let input = "warning: unused\nwarning: unused\nwarning: unused\nwarning: unused\n";
        let out = compress(input, term(false, Mode::Chain));
        assert_eq!(out.text, "warning: unused\n[x4]\n");
    }

    #[test]
    fn lossless_mode_never_caps_long_output() {
        let input: String = (0..1200).map(|i| format!("line {i}\n")).collect();
        let out = compress(&input, term(true, Mode::Chain));
        assert_eq!(out.text, input);
        assert!(!out.lossy);
    }

    #[test]
    fn chain_mode_caps_long_output_and_keeps_the_tail() {
        let mut input: String = (0..1200).map(|i| format!("line {i}\n")).collect();
        input.push_str("error: the thing that matters\n");
        let out = compress(&input, term(false, Mode::Chain));
        assert!(out.text.contains("lines elided]"));
        assert!(out.text.contains("error: the thing that matters"));
        assert!(out.lossy);
        assert!(out.text.len() < input.len());
    }

    #[test]
    fn auto_mode_never_grows_the_input() {
        let input: String = (0..300).map(|i| format!("line {i}\n")).collect();
        let out = compress(&input, term(false, Mode::Auto));
        assert!(out.text.len() <= input.len());
    }

    #[test]
    fn empty_input_is_unchanged() {
        let out = compress("", term(false, Mode::Chain));
        assert_eq!(out.text, "");
        assert!(!out.changed);
    }

    #[test]
    fn stats_report_one_decimal_percent() {
        let stats = Stats::new(&"a".repeat(1000), &"a".repeat(600));
        assert_eq!(stats.bytes_saved, 400);
        assert_eq!(stats.saved_percent, 40.0);
        assert_eq!(stats.tokens_before, 250);
        assert_eq!(stats.tokens_after, 150);
    }

    #[test]
    fn mode_aliases_parse() {
        assert_eq!(Mode::parse("shape"), Some(Mode::Auto));
        assert_eq!(Mode::parse("default"), Some(Mode::Chain));
        assert_eq!(Mode::parse("turbo"), None);
    }

    const LS_RAW: &str = "total 8\n\
                          drwxr-xr-x  2 root root 4096 Nov  7 10:00 src\n\
                          -rw-r--r--  1 root root  341 Nov  7 09:00 hello.go\n";

    #[test]
    fn auto_mode_compacts_ls_listing() {
        let out = compress(LS_RAW, term(false, Mode::Auto));
        assert!(out.changed);
        assert!(out.text.contains("755  src/"), "{}", out.text);
        assert!(out.text.contains("644  hello.go"), "{}", out.text);
        assert!(out.chain[0].starts_with("auto>filter:"));
        assert!(out.text.len() < LS_RAW.len());
    }

    #[test]
    fn chain_mode_leaves_ls_listing_untouched() {
        // Explicit `filter=ls` is required in chain mode; generic chain is a no-op.
        let out = compress(LS_RAW, term(false, Mode::Chain));
        assert!(!out.changed);
        assert_eq!(out.text, LS_RAW);
    }

    #[test]
    fn detect_rejects_plain_text_and_partial_match() {
        assert_eq!(detect_command_filter("hello\nworld\n"), None);
        assert_eq!(detect_command_filter(""), None);
        // One permission-looking line among plain text is not enough.
        let mixed = "just some output\n-rwxr-xr-x  1 me me 10 Jan 1 12:00 run.sh\nother text\n";
        assert_eq!(detect_command_filter(mixed), None);
        assert_eq!(detect_command_filter(LS_RAW), Some("ls"));
    }

    #[test]
    fn detect_accepts_acl_marker_and_chinese_locale() {
        let acl = "total 4\n-rw-r--r--+ 1 root root 10 Nov  7 10:00 a.txt\n\
                   drwxr-xr-x. 2 root root 4096 Nov  7 10:00 dir\n";
        assert_eq!(detect_command_filter(acl), Some("ls"));
        let zh = "总计 162880\ndrwxrwxr-x  2 root root 4096 6月28日 17:28 AI\n\
                  -rw-r--r--  1 root root  541 10月18日 23:00 adv_scan.py\n";
        assert_eq!(detect_command_filter(zh), Some("ls"));
    }
}