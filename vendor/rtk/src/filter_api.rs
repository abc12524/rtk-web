//! Public, panic-safe facade over the command-specific output filters in
//! `src/cmds`.
//!
//! Every entry takes the raw text a command printed and returns a compact
//! representation. No command is executed here. Filters that genuinely need a
//! real argv, an exit code or the filesystem are wrapped with safe defaults or
//! deliberately left out.

use crate::core::guard::never_worse;

type FilterFn = fn(&str) -> String;

pub fn names() -> &'static [&'static str] {
    NAMES
}

/// Compress `input` with the named filter. Returns `None` when the name is not
/// registered. Panics inside a filter fall back to the input, and a filter that
/// grows the text is replaced by the original (`never_worse`).
pub fn apply(name: &str, input: &str) -> Option<String> {
    let filter = lookup(name)?;
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter(input)))
        .unwrap_or_else(|_| input.to_string());
    Some(never_worse(input, &output).to_string())
}

/// Pick and run a filter for `input` without being told the command name.
///
/// This is the entry point behind `mode=auto`: clients such as shell-tool send
/// raw command output with no `filter` parameter, so the service sniffs the
/// shape instead. Recognises `ls -l` variants first (the `pipe` detector does
/// not cover them), then defers to the pipe auto-detector for grep / find /
/// git / cargo / pytest and friends.
///
/// Returns `None` when nothing matches or the chosen filter does not produce
/// strictly fewer bytes — a false positive can then only leave the text as-is.
pub fn auto(input: &str) -> Option<(&'static str, String)> {
    if looks_like_ls_long(input) {
        if let Some(hit) = run_gated("ls", ls_long, input) {
            return Some(hit);
        }
    }

    let (name, filter) = crate::cmds::system::pipe_cmd::auto_detect_filter_named(input);
    if name.is_empty() {
        return None;
    }
    run_gated(name, filter, input)
}

/// Shape-only fallback for `mode=auto`, tried after the command filters *and*
/// the shape strategy have both declined to shrink the input.
///
/// This exists because of where the two existing mechanisms leave a gap:
///
/// - the command filters in [`auto`] need a recognisable command signature;
/// - the shape strategy needs volume — its `prefix_rep` / `unique` renderers
///   collapse a 200-line `ps aux` by ~88%, but a 14-line `df` falls under its
///   thresholds and is returned untouched.
///
/// Whitespace-aligned tables sit exactly in that gap: `df`, `docker ps`,
/// `kubectl get`, short `systemctl` listings. Keeping this a separate entry
/// point rather than a branch inside [`auto`] is deliberate — the detector here
/// works on shape alone, so it must never preempt a strategy that is already
/// compressing the same text far harder.
///
/// Returns `None` when the input is not a table or the squeeze does not make it
/// strictly smaller.
pub fn auto_fallback(input: &str) -> Option<(&'static str, String)> {
    if !crate::cmds::system::table::looks_like_table(input) {
        return None;
    }
    run_gated("table", crate::cmds::system::table::filter_table, input)
}

/// Run `filter` panic-safely and keep the result only when it is strictly
/// smaller than the input. `never_worse` already rejects a filter that would
/// grow the text; the length gate is what turns "no better" into "not handled",
/// so the caller can move on to the next mechanism.
fn run_gated(
    name: &'static str,
    filter: FilterFn,
    input: &str,
) -> Option<(&'static str, String)> {
    let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter(input)))
        .unwrap_or_else(|_| input.to_string());
    let output = never_worse(input, &output).to_string();
    if output.len() < input.len() {
        Some((name, output))
    } else {
        None
    }
}

/// True when every entry line starts with a `ls -l` permissions field.
///
/// Skips the `total`/`总计` header and `ls -R` directory headers, and tolerates
/// the GNU ACL/SELinux marker (`+`, `.`, `@`) after the nine mode bits. Needs at
/// least one entry, so empty or header-only input never matches.
fn looks_like_ls_long(input: &str) -> bool {
    let mut entries = 0usize;
    let mut permission_lines = 0usize;
    for line in input.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with("total ") || line.starts_with("总计") {
            continue;
        }
        let perms = starts_with_permissions(line);
        // `ls -R` prints "<dir>:" headers; ignore them without inflating the
        // entry count, so a recursive long listing still matches.
        if !perms && line.ends_with(':') {
            continue;
        }
        entries += 1;
        if perms {
            permission_lines += 1;
        }
    }
    entries > 0 && permission_lines == entries
}

fn starts_with_permissions(line: &str) -> bool {
    let bytes = line.as_bytes();
    if bytes.len() < 11 || !matches!(bytes[0], b'-' | b'd' | b'l' | b'c' | b'b' | b'p' | b's') {
        return false;
    }
    if !bytes[1..10]
        .iter()
        .all(|c| matches!(c, b'r' | b'w' | b'x' | b's' | b'S' | b't' | b'T' | b'-'))
    {
        return false;
    }
    let rest = &bytes[10..];
    rest.first().is_some_and(|c| c.is_ascii_whitespace())
        || (matches!(rest.first(), Some(b'+' | b'.' | b'@'))
            && rest.get(1).is_some_and(|c| c.is_ascii_whitespace()))
}

fn git_log(input: &str) -> String {
    crate::cmds::git::git_cmd::filter_log_output(input, 50, false)
}

fn git_diff(input: &str) -> String {
    crate::cmds::git::git_cmd::compact_diff(input, 500)
}

fn ls_long(input: &str) -> String {
    crate::cmds::system::ls::render(input, None, false, true, 0)
}

fn ls_short(input: &str) -> String {
    crate::cmds::system::ls::render(input, None, false, false, 0)
}

fn phpstan(input: &str) -> String {
    if input.trim_start().starts_with('{') {
        crate::cmds::php::phpstan_cmd::filter_phpstan_json(input)
    } else {
        crate::cmds::php::phpstan_cmd::filter_phpstan_text(input)
    }
}

fn pint(input: &str) -> String {
    if input.trim_start().starts_with('{') {
        crate::cmds::php::pint_cmd::filter_pint_json(input)
    } else {
        input.to_string()
    }
}

fn cargo_build(input: &str) -> String {
    crate::cmds::rust::cargo_cmd::filter_cargo_build_labeled(input, "build", 0)
}

fn wc(input: &str) -> String {
    crate::cmds::system::wc_cmd::filter_wc_output(
        input,
        &crate::cmds::system::wc_cmd::WcMode::Full,
    )
}

fn json_compact(input: &str) -> String {
    crate::cmds::system::json_cmd::filter_json_compact(input, 5)
        .unwrap_or_else(|_| input.to_string())
}

fn json_schema(input: &str) -> String {
    crate::cmds::system::json_cmd::filter_json_string(input, 5)
        .unwrap_or_else(|_| input.to_string())
}

fn kubectl_pods(input: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(value) => crate::cmds::cloud::container::format_kubectl_pods(&value),
        Err(_) => input.to_string(),
    }
}

fn kubectl_services(input: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(value) => crate::cmds::cloud::container::format_kubectl_services(&value),
        Err(_) => input.to_string(),
    }
}

fn mvn_test(input: &str) -> String {
    crate::cmds::jvm::mvn_cmd::filter_surefire(input, false)
}

fn mvn_compile(input: &str) -> String {
    crate::cmds::jvm::mvn_cmd::filter_compile(input, false)
}

fn mvn_package(input: &str) -> String {
    crate::cmds::jvm::mvn_cmd::filter_package(input, false)
}

fn mvn_quiet(input: &str) -> String {
    crate::cmds::jvm::mvn_cmd::filter_quiet(input, false)
}

const NAMES: &[&str] = &[
    "black",
    "cargo",
    "cargo-build",
    "cargo-clippy",
    "cargo-install",
    "cargo-nextest",
    "cargo-test",
    "ctest",
    "deno",
    "ecs",
    "eslint",
    "find",
    "gh-pr-checks",
    "git-branch",
    "git-diff",
    "git-log",
    "git-stash-list",
    "git-status",
    "glab-ci-status",
    "go-build",
    "go-test",
    "go-vet",
    "grep",
    "gradle-connected",
    "gradle-dependencies",
    "gradle-lint",
    "gradle-test",
    "json",
    "json-schema",
    "kubectl-pods",
    "kubectl-services",
    "lint",
    "log",
    "ls",
    "ls-short",
    "mvn-compile",
    "mvn-package",
    "mvn-quiet",
    "mvn-test",
    "mypy",
    "next",
    "ng",
    "npm",
    "paratest",
    "pest",
    "php-test",
    "phpstan",
    "phpunit",
    "pint",
    "pip-list",
    "pip-outdated",
    "pnpm-install",
    "prettier",
    "prisma-db-push",
    "prisma-generate",
    "prisma-migrate-deploy",
    "prisma-migrate-dev",
    "prisma-migrate-status",
    "psql",
    "pylint",
    "pytest",
    "rspec",
    "rspec-text",
    "rubocop",
    "rubocop-text",
    "ruff-check",
    "ruff-format",
    "sbt-compile",
    "sbt-run",
    "sbt-test",
    "sqlfluff-lint",
    "table",
    "tree",
    "tsc",
    "vitest",
    "wc",
];

fn lookup(name: &str) -> Option<FilterFn> {
    Some(match name {
        "cargo" | "cargo-test" => crate::cmds::rust::cargo_cmd::filter_cargo_test,
        "ctest" => crate::cmds::system::ctest_cmd::filter_ctest_output,
        "deno" => crate::cmds::js::deno_cmd::filter_deno_output,
        "ecs" => crate::cmds::php::ecs_cmd::filter_ecs_output,
        "find" | "fd" => crate::cmds::system::pipe_cmd::find_wrapper,
        "git-diff" => git_diff,
        "git-log" => git_log,
        "git-status" => crate::cmds::git::git_cmd::format_status_output,
        "go-build" => crate::cmds::go::go_cmd::filter_go_build,
        "go-test" => crate::cmds::go::go_cmd::filter_go_test_pipe,
        "grep" | "rg" => crate::cmds::system::pipe_cmd::grep_wrapper,
        "log" => crate::cmds::system::log_cmd::run_stdin_str,
        "ls" => ls_long,
        "ls-short" => ls_short,
        "mypy" => crate::cmds::python::mypy_cmd::filter_mypy_output,
        "vitest" => crate::cmds::system::pipe_cmd::vitest_wrapper,
        "paratest" | "pest" | "php-test" => crate::cmds::php::test_output::filter_test_runner_output,
        "phpunit" => crate::cmds::php::phpunit_cmd::filter_phpunit_output,
        "phpstan" => phpstan,
        "pint" => pint,
        "prettier" => crate::cmds::js::prettier_cmd::filter_prettier_output,
        "pytest" => crate::cmds::python::pytest_cmd::filter_pytest_output,
        "ruff-check" => crate::cmds::python::ruff_cmd::filter_ruff_check_json,
        "ruff-format" => crate::cmds::python::ruff_cmd::filter_ruff_format,
        "sqlfluff-lint" => crate::cmds::python::sqlfluff_cmd::filter_sqlfluff_lint_json,
        "table" => crate::cmds::system::table::filter_table,
        "tree" => crate::cmds::system::tree::filter_tree_output,
        "tsc" => crate::cmds::js::tsc_cmd::filter_tsc_output,
        "black" => crate::cmds::system::format_cmd::filter_black_output,
        "cargo-build" => cargo_build,
        "cargo-clippy" => crate::cmds::rust::cargo_cmd::filter_cargo_clippy,
        "cargo-install" => crate::cmds::rust::cargo_cmd::filter_cargo_install,
        "cargo-nextest" => crate::cmds::rust::cargo_cmd::filter_cargo_nextest,
        "eslint" => crate::cmds::js::lint_cmd::filter_eslint_json,
        "gh-pr-checks" => crate::cmds::git::gh_cmd::format_pr_checks,
        "git-branch" => crate::cmds::git::git_cmd::filter_branch_output,
        "git-stash-list" => crate::cmds::git::git_cmd::filter_stash_list,
        "glab-ci-status" => crate::cmds::git::glab_cmd::format_ci_status,
        "go-vet" => crate::cmds::go::go_cmd::filter_go_vet,
        "gradle-connected" => crate::cmds::jvm::gradlew_cmd::filter_connected,
        "gradle-dependencies" => crate::cmds::jvm::gradlew_cmd::filter_dependencies,
        "gradle-lint" => crate::cmds::jvm::gradlew_cmd::filter_lint,
        "gradle-test" => crate::cmds::jvm::gradlew_cmd::filter_test,
        "json" => json_compact,
        "json-schema" => json_schema,
        "kubectl-pods" => kubectl_pods,
        "kubectl-services" => kubectl_services,
        "lint" => crate::cmds::js::lint_cmd::filter_generic_lint,
        "mvn-compile" => mvn_compile,
        "mvn-package" => mvn_package,
        "mvn-quiet" => mvn_quiet,
        "mvn-test" => mvn_test,
        "next" => crate::cmds::js::next_cmd::filter_next_build,
        "ng" => crate::cmds::js::ng_cmd::filter_build,
        "npm" => crate::cmds::js::npm_cmd::filter_npm_output,
        "pip-list" => crate::cmds::python::pip_cmd::filter_pip_list,
        "pip-outdated" => crate::cmds::python::pip_cmd::filter_pip_outdated,
        "pnpm-install" => crate::cmds::js::pnpm_cmd::filter_pnpm_install,
        "prisma-db-push" => crate::cmds::js::prisma_cmd::filter_db_push,
        "prisma-generate" => crate::cmds::js::prisma_cmd::filter_prisma_generate,
        "prisma-migrate-deploy" => crate::cmds::js::prisma_cmd::filter_migrate_deploy,
        "prisma-migrate-dev" => crate::cmds::js::prisma_cmd::filter_migrate_dev,
        "prisma-migrate-status" => crate::cmds::js::prisma_cmd::filter_migrate_status,
        "psql" => crate::cmds::cloud::psql_cmd::filter_psql_output,
        "pylint" => crate::cmds::js::lint_cmd::filter_pylint_json,
        "rspec" => crate::cmds::ruby::rspec_cmd::filter_rspec_output,
        "rspec-text" => crate::cmds::ruby::rspec_cmd::filter_rspec_text,
        "rubocop" => crate::cmds::ruby::rubocop_cmd::filter_rubocop_json,
        "rubocop-text" => crate::cmds::ruby::rubocop_cmd::filter_rubocop_text,
        "sbt-compile" => crate::cmds::scala::sbt_cmd::filter_sbt_compile,
        "sbt-run" => crate::cmds::scala::sbt_cmd::filter_sbt_run,
        "sbt-test" => crate::cmds::scala::sbt_cmd::filter_sbt_test,
        "wc" => wc,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_name_resolves() {
        for name in names() {
            assert!(lookup(name).is_some(), "{name} is listed but not wired");
        }
    }

    #[test]
    fn unknown_name_returns_none() {
        assert_eq!(apply("does-not-exist", "text"), None);
    }

    #[test]
    fn ls_filters_a_long_listing() {
        let raw = "total 8\n\
                   drwxr-xr-x  2 root root 4096 Nov  7 10:00 src\n\
                   -rw-r--r--  1 root root  341 Nov  7 09:00 hello.go\n";
        let out = apply("ls", raw).expect("ls is registered");
        assert!(out.contains("755  src/"), "{out}");
        assert!(out.contains("644  hello.go"), "{out}");
        assert!(!out.contains("total"), "{out}");
    }

    #[test]
    fn ls_filters_a_chinese_locale_listing() {
        let raw = "总计 162880\n\
                   drwxrwxr-x  2 root root 4096 6月28日 17:28 AI\n\
                   -rw-r--r--  1 root root  541 10月18日 23:00 adv_scan.py\n\
                   drwxr-xr-x  2 root root 4096 2026年 4月15日 ansible\n";
        let out = apply("ls", raw).expect("ls is registered");
        assert!(out.contains("775  AI/"), "{out}");
        assert!(out.contains("644  adv_scan.py"), "{out}");
        assert!(out.contains("755  ansible/"), "{out}");
    }

    #[test]
    fn filters_never_grow_the_input() {
        let input = "plain text that no filter shrinks";
        let out = apply("mypy", input).expect("mypy is registered");
        assert!(out.len() <= input.len(), "{out}");
    }

    #[test]
    fn auto_picks_ls_for_long_listings() {
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
            assert_eq!(auto(raw).map(|(n, _)| n), Some("ls"), "{raw:?}");
        }
    }

    #[test]
    fn auto_covers_grep_and_find() {
        let mut grep = String::new();
        for i in 0..20 {
            grep.push_str(&format!(
                "src/components/deep/nested/file_with_long_name.rs:{}:    let value_{} = compute({});\n",
                10 + i,
                i,
                i
            ));
        }
        assert_eq!(auto(&grep).map(|(n, _)| n), Some("grep"));

        let mut find = String::new();
        for i in 0..30 {
            find.push_str(&format!("./very/long/common/prefix/dir/file_{i:02}.rs\n"));
        }
        assert_eq!(auto(&find).map(|(n, _)| n), Some("find"));
    }

    #[test]
    fn auto_returns_none_for_plain_text() {
        assert!(auto("hello\nworld\n").is_none());
        assert!(auto("").is_none());
    }

}