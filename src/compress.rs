use rtk_engine::block::{Block, ContentFormat, Origin, Provenance, Subject};
use rtk_engine::chain::run_chain_guarded;
use rtk_engine::method::{Method, MethodCtx};
use rtk_engine::methods::{ANSI, DEDUP, chain_for};
use rtk_engine::strategy;
use rtk_engine::tokens::estimate_tokens;
use serde::Serialize;

static LOSSLESS_CHAIN: [&dyn Method; 2] = [&ANSI, &DEDUP];

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
        // Sniff the command behind the output (ls -l, grep, find, git, cargo,
        // pytest, …) and apply its filter. Clients like shell-tool only send
        // `mode=auto`, so without this a bare `ls -l` would fall through the
        // generic term chain unchanged.
        if let Some((name, text)) = rtk::filter_api::auto(input) {
            return CompressOutput {
                text,
                chain: vec![format!("auto>filter:{name}")],
                lossy: true,
                changed: true,
            };
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
        // Last resort, shape-only: whitespace-aligned tables (`df`, `docker ps`,
        // `kubectl get`, short `systemctl` listings). Ordered after the strategy
        // on purpose — a 200-line `ps aux` is already collapsed ~88% by
        // `prefix_rep`, and squeezing its padding instead would be a big loss.
        if let Some((name, text)) = rtk::filter_api::auto_fallback(input) {
            return CompressOutput {
                text,
                chain: vec![format!("auto>filter:{name}")],
                lossy: true,
                changed: true,
            };
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
    fn auto_detects_ls_variants() {
        // -la (dot entries), -lh (human sizes), ACL marker, Chinese locale and
        // ls -R (directory headers) must all be recognised, then compacted.
        let cases = [
            "total 12\ndrwxr-xr-x 5 root root 4096 Nov  7 10:00 .\n-rw-r--r-- 1 root root 341 Nov  7 09:00 a.go\n",
            "total 8\n-rw-r--r-- 1 root root 1.2K Nov  7 09:00 big.log\n-rw-r--r-- 1 root root 341 Nov  7 09:00 a.go\n",
            "total 4\n-rw-r--r--+ 1 root root 10 Nov  7 10:00 x.txt\n",
            "总计 12\ndrwxr-xr-x 2 root root 4096 6月28日 17:28 AI\n-rw-r--r-- 1 root root 541 10月18日 23:00 a.py\n",
            r#".:
total 16
-rw-r--r-- 1 root root 341 Nov  7 09:00 a.go
-rw-r--r-- 1 root root 120 Nov  7 09:00 b.go
drwxr-xr-x 2 root root 4096 Nov  7 10:00 sub

./sub:
total 8
-rw-r--r-- 1 root root 900 Nov  7 09:00 c.go
-rw-r--r-- 1 root root 800 Nov  7 09:00 d.go
"#,
        ];
        for raw in cases {
            assert_eq!(
                rtk::filter_api::auto(raw).map(|(n, _)| n),
                Some("ls"),
                "not detected as ls: {raw:?}"
            );
        }
    }

    #[test]
    fn auto_ignores_plain_text_and_partial_permission_lines() {
        assert!(rtk::filter_api::auto("hello\nworld\n").is_none());
        assert!(rtk::filter_api::auto("").is_none());
        // One permission-looking line among plain text is not enough.
        let mixed = "just some output\n-rwxr-xr-x  1 me me 10 Jan 1 12:00 run.sh\nother text\n";
        assert!(rtk::filter_api::auto(mixed).is_none());
    }

    #[test]
    fn auto_ls_preserves_human_readable_sizes() {
        // `ls -lh` sizes must survive: "1.2K" used to collapse to "1B" because
        // the parser fell back to the numeric nlink column.
        let raw = "total 8\n\
                   -rw-r--r-- 1 root root 1.2K Nov  7 09:00 big.log\n\
                   -rw-r--r-- 1 root root  341 Nov  7 09:00 a.go\n";
        let out = compress(raw, term(false, Mode::Auto));
        assert!(out.text.contains("1.2K"), "{}", out.text);
        assert!(out.text.contains("341B"), "{}", out.text);
    }

    #[test]
    fn auto_detects_grep_and_find() {
        let mut grep = String::new();
        for i in 0..20 {
            grep.push_str(&format!(
                "src/components/deep/nested/file_with_long_name.rs:{}:    let value_{} = compute({});\n",
                10 + i,
                i,
                i
            ));
        }
        assert_eq!(rtk::filter_api::auto(&grep).map(|(n, _)| n), Some("grep"));

        let mut find = String::new();
        for i in 0..30 {
            find.push_str(&format!("./very/long/common/prefix/dir/file_{i:02}.rs\n"));
        }
        assert_eq!(rtk::filter_api::auto(&find).map(|(n, _)| n), Some("find"));
    }

    const DF_RAW: &str = "Filesystem      Size  Used Avail Use% Mounted on\n\
                          /dev/sda2       196G  120G   66G  65% /\n\
                          /dev/sda1       511M  6.1M  505M   2% /boot/efi\n\
                          tmpfs           3.9G     0  3.9G   0% /dev/shm\n";

    #[test]
    fn auto_fallback_squeezes_a_small_table() {
        // A 14-line df falls under the shape strategy's thresholds, so nothing
        // used to touch it. The table fallback is the last resort that does.
        let out = compress(DF_RAW, term(false, Mode::Auto));
        assert!(out.changed, "{:?}", out.chain);
        assert_eq!(out.chain, vec!["auto>filter:table".to_string()]);
        assert!(out.text.len() < DF_RAW.len());
        assert!(
            out.text.contains("/dev/sda2 196G 120G 66G 65% /"),
            "{}",
            out.text
        );
        // Column order and the two-word header cell survive verbatim.
        assert!(out.text.contains("Use% Mounted on"), "{}", out.text);
    }

    #[test]
    fn auto_fallback_is_not_consulted_for_large_tables() {
        // A 200-row ps aux is collapsed ~88% by `shape:prefix_rep`; squeezing
        // padding instead would be a big regression, so the table fallback must
        // stay behind the strategy.
        let mut raw = String::from("USER   PID %CPU %MEM COMMAND\n");
        for i in 0..200 {
            raw.push_str(&format!("root  {i:5}  0.0  0.1 /usr/lib/service-{i}/bin/run --flag\n"));
        }
        let out = compress(&raw, term(false, Mode::Auto));
        assert!(out.changed);
        assert!(
            out.chain[0].starts_with("shape:"),
            "table preempted the strategy: {:?}",
            out.chain
        );
        assert!(out.text.len() < raw.len() / 2, "{} bytes", out.text.len());
    }

    #[test]
    fn table_filter_is_individually_callable() {
        let out = rtk::filter_api::apply("table", DF_RAW).expect("table is registered");
        assert!(out.len() < DF_RAW.len());
        assert_eq!(
            rtk::filter_api::auto_fallback(DF_RAW).map(|(n, _)| n),
            Some("table")
        );
    }

    #[test]
    fn auto_fallback_ignores_non_tables() {
        assert!(rtk::filter_api::auto_fallback("hello\nworld\n").is_none());
        assert!(rtk::filter_api::auto_fallback("").is_none());
        // `ls -l` is the ls filter's job, not the table fallback's.
        assert!(rtk::filter_api::auto_fallback(LS_RAW).is_none());
    }
}