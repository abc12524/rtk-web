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
    "tree",
    "tsc",
    "wc",
];

fn lookup(name: &str) -> Option<FilterFn> {
    Some(match name {
        "cargo" | "cargo-test" => crate::cmds::rust::cargo_cmd::filter_cargo_test,
        "ctest" => crate::cmds::system::ctest_cmd::filter_ctest_output,
        "deno" => crate::cmds::js::deno_cmd::filter_deno_output,
        "ecs" => crate::cmds::php::ecs_cmd::filter_ecs_output,
        "git-diff" => git_diff,
        "git-log" => git_log,
        "git-status" => crate::cmds::git::git_cmd::format_status_output,
        "go-build" => crate::cmds::go::go_cmd::filter_go_build,
        "go-test" => crate::cmds::go::go_cmd::filter_go_test_pipe,
        "log" => crate::cmds::system::log_cmd::run_stdin_str,
        "ls" => ls_long,
        "ls-short" => ls_short,
        "mypy" => crate::cmds::python::mypy_cmd::filter_mypy_output,
        "paratest" | "pest" | "php-test" => crate::cmds::php::test_output::filter_test_runner_output,
        "phpunit" => crate::cmds::php::phpunit_cmd::filter_phpunit_output,
        "phpstan" => phpstan,
        "pint" => pint,
        "prettier" => crate::cmds::js::prettier_cmd::filter_prettier_output,
        "pytest" => crate::cmds::python::pytest_cmd::filter_pytest_output,
        "ruff-check" => crate::cmds::python::ruff_cmd::filter_ruff_check_json,
        "ruff-format" => crate::cmds::python::ruff_cmd::filter_ruff_format,
        "sqlfluff-lint" => crate::cmds::python::sqlfluff_cmd::filter_sqlfluff_lint_json,
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
}