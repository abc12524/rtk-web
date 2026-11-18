//! Whitespace-aligned table filter — `df`, `ps aux`, `docker ps`, `kubectl get`,
//! `systemctl list-units` and friends.
//!
//! These commands print a header row followed by columns padded to a fixed
//! width. A terminal needs that padding; a reader (human or model) does not —
//! the alignment is pure byte overhead once the values are already separated by
//! at least one space.
//!
//! The filter therefore does three things and nothing else:
//!
//! 1. squeezes runs of spaces/tabs inside a line down to a single space,
//!    keeping up to two leading spaces so `●`/tree-like markers stay attached;
//! 2. collapses consecutive identical rows into `... (xN)`;
//! 3. collapses runs of blank lines into one.
//!
//! Column order, header text and every cell value are preserved verbatim: no
//! column is dropped, reordered, renamed or truncated, so a row still reads
//! left-to-right exactly as the command printed it.

/// Indentation deeper than this is meaningless for a table row; keep a small
/// marker so prefixed rows still look prefixed.
const MAX_INDENT: usize = 2;

/// True when `input` is a headered, whitespace-aligned table.
///
/// The check is deliberately structural rather than command-based, because the
/// caller only ever sees the text. It requires:
///
/// - a first line that starts at column 0 and splits into 2..=20 cells;
/// - every header cell to carry at least one letter (so a row of bare numbers
///   is treated as data, not as a header);
/// - at least 2 data rows, at least 60% of which also split into 2+ cells.
///
/// The 60% — rather than "all" — is what real output looks like: a column whose
/// widest value is the one defining the width gets a single separating space, so
/// `systemd-journald.service` or a long `/dev/mapper/...` device legitimately
/// fails the 2+ space test while the rest of the table passes it.
///
/// Single-line output, prose, `ls -l` (`total 8` header has one cell) and tree
/// output all fail the header test.
pub fn looks_like_table(input: &str) -> bool {
    let mut lines = input.lines().map(str::trim_end).filter(|l| !l.is_empty());
    let Some(header) = lines.next() else {
        return false;
    };
    if header.starts_with([' ', '\t']) {
        return false;
    }

    let header_cells = split_cells(header);
    if !(2..=20).contains(&header_cells.len()) {
        return false;
    }
    if !header_cells.iter().all(|cell| is_header_cell(cell)) {
        return false;
    }

    let mut rows = 0usize;
    let mut aligned = 0usize;
    for line in lines {
        rows += 1;
        if split_cells(line.trim_start()).len() >= 2 {
            aligned += 1;
        }
    }

    rows >= 2 && aligned >= 2 && aligned * 10 >= rows * 6
}

/// Squeeze the table in `input`. See the module docs for exactly what changes.
pub fn filter_table(input: &str) -> String {
    if input.trim().is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(input.len());
    let mut pending: Option<(String, usize)> = None;

    for raw in input.lines() {
        let line = squeeze_line(raw);
        if line.is_empty() {
            flush(&mut out, &mut pending);
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            continue;
        }
        if let Some((text, count)) = pending.as_mut() {
            if *text == line {
                *count += 1;
                continue;
            }
        }
        flush(&mut out, &mut pending);
        pending = Some((line, 1));
    }
    flush(&mut out, &mut pending);

    while out.ends_with('\n') {
        out.pop();
    }
    if input.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn flush(out: &mut String, pending: &mut Option<(String, usize)>) {
    if let Some((text, count)) = pending.take() {
        out.push_str(&text);
        if count > 1 {
            out.push_str(&format!("  (x{count})"));
        }
        out.push('\n');
    }
}

/// Collapse intra-line whitespace runs; blank lines yield an empty string.
fn squeeze_line(raw: &str) -> String {
    let trimmed = raw.trim_end();
    let body = trimmed.trim_start();
    if body.is_empty() {
        return String::new();
    }

    let indent = trimmed.len() - body.len();
    let mut out = String::with_capacity(trimmed.len());
    for _ in 0..indent.min(MAX_INDENT) {
        out.push(' ');
    }

    let mut in_space = false;
    for ch in body.chars() {
        if ch == ' ' || ch == '\t' {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}

/// Split a line at runs of 2+ whitespace characters — how aligned tables
/// separate their columns. Single spaces inside a cell (a command line, a
/// description, `Mounted on`) are left intact.
fn split_cells(line: &str) -> Vec<&str> {
    let mut cells = Vec::new();
    let bytes = line.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            let run_start = i;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            if i - run_start >= 2 {
                if run_start > start {
                    cells.push(&line[start..run_start]);
                }
                start = i;
            }
        } else {
            i += 1;
        }
    }
    if start < bytes.len() {
        cells.push(&line[start..]);
    }
    cells
}

/// A header cell is a short label, not a value: it must contain a letter (or a
/// CJK ideograph) and stay within a sane width.
fn is_header_cell(cell: &str) -> bool {
    !cell.is_empty()
        && cell.chars().count() <= 32
        && cell
            .chars()
            .any(|c| c.is_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = r#"USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND
root           1  0.0  0.1  25124 13824 ?        Ss   11月17   1:22 /sbin/init
root           2  0.0  0.0      0     0 ?        S    11月17   0:00 [kthreadd]
root         141  0.0  0.0      0     0 ?        I<   11月17   0:00 [kworker/R-rcu_gp]
"#;

    const DF: &str = r#"Filesystem      Size  Used Avail Use% Mounted on
/dev/sda2       196G  120G   66G  65% /
/dev/sda1       511M  6.1M  505M   2% /boot/efi
tmpfs           3.9G     0  3.9G   0% /dev/shm
"#;

    const DOCKER: &str = r#"CONTAINER ID   IMAGE          COMMAND                  CREATED       STATUS       PORTS                  NAMES
a1b2c3d4e5f6   nginx:latest   "/docker-entrypoint.…"   2 hours ago   Up 2 hours   0.0.0.0:80->80/tcp     web
f6e5d4c3b2a1   redis:7        "redis-server"           3 hours ago   Up 3 hours   6379/tcp               cache
"#;

    const KUBECTL: &str = r#"NAME                        READY   STATUS    RESTARTS   AGE
web-6d4b7c8f9d-abcde        1/1     Running   0          5d
web-6d4b7c8f9d-fghij        1/1     Running   0          5d
"#;

    #[test]
    fn detects_real_table_shapes() {
        for sample in [PS, DF, DOCKER, KUBECTL] {
            assert!(looks_like_table(sample), "{sample}");
        }
    }

    #[test]
    fn detects_a_table_whose_widest_cell_leaves_one_space() {
        // `systemd-journald.service` defines the column width, so it is followed
        // by a single space — real output, and it must not defeat the detector.
        let raw = "UNIT                     LOAD   ACTIVE SUB     DESCRIPTION\n\
                   cron.service             loaded active running Regular background program\n\
                   systemd-journald.service loaded active running Journal Service\n\
                   ufw.service              loaded active exited  Uncomplicated firewall\n";
        assert!(looks_like_table(raw), "{raw}");
        let out = filter_table(raw);
        assert!(out.len() < raw.len(), "{out}");
        assert!(out.contains("systemd-journald.service loaded active running Journal Service"), "{out}");
    }

    #[test]
    fn rejects_non_tables() {
        let cases = [
            "",
            "hello\n",
            "hello\nworld\n",
            // `ls -l` starts with a one-cell `total` header: handled by `ls`.
            "total 8\n-rw-r--r-- 1 root root 341 Nov  7 09:00 a.go\n",
            // prose with occasional double spaces must not be treated as a table
            "This is a sentence  with a double space.\nAnd another line  here.\n",
            // tree output starts with a single-cell root line
            ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n",
        ];
        for case in cases {
            assert!(!looks_like_table(case), "{case:?}");
        }
    }

    #[test]
    fn squeezes_padding_but_keeps_every_value() {
        let out = filter_table(PS);
        assert!(out.len() < PS.len(), "{out}");
        assert!(out.starts_with("USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND\n"), "{out}");
        assert!(out.contains("root 1 0.0 0.1 25124 13824 ? Ss 11月17 1:22 /sbin/init"), "{out}");
        // COMMAND keeps its internal single spaces
        assert!(out.contains("[kworker/R-rcu_gp]"), "{out}");
        assert_eq!(out.lines().count(), PS.lines().count());
    }

    #[test]
    fn keeps_last_cell_words_intact() {
        // df's header column "Mounted on" and the mount point must survive
        let out = filter_table(DF);
        assert!(out.contains("Use% Mounted on"), "{out}");
        assert!(out.contains("/dev/sda2 196G 120G 66G 65% /"), "{out}");
    }

    #[test]
    fn collapses_duplicate_rows_with_a_count() {
        let raw = "NAME   NODE   STATUS\nweb    n1     Running\nweb    n1     Running\nweb    n1     Running\ndb     n2     Running\n";
        let out = filter_table(raw);
        assert!(out.contains("web n1 Running  (x3)"), "{out}");
        assert!(out.contains("db n2 Running"), "{out}");
        assert_eq!(out.lines().count(), 3, "{out}");
    }

    #[test]
    fn keeps_a_single_blank_line_between_blocks() {
        let raw = "UNIT   LOAD   ACTIVE\n\
                   a      loaded active\n\
                   \n\
                   \n\
                   LEGEND: something here\n";
        let out = filter_table(raw);
        assert!(!out.contains("\n\n\n"), "{out:?}");
        assert!(out.contains("LEGEND: something here"), "{out}");
    }

    #[test]
    fn preserves_a_short_marker_indent() {
        let raw = "UNIT          LOAD   ACTIVE\n\
                   \x20\x20● a.service loaded active\n\
                   \x20 ● b.service loaded inactive\n";
        let out = filter_table(raw);
        assert!(out.contains("  ● a.service loaded active"), "{out}");
        assert!(out.contains(" ● b.service loaded inactive"), "{out}");
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(filter_table(""), "");
        assert_eq!(filter_table("   \n\n"), "");
    }

    #[test]
    fn split_cells_keeps_single_spaces() {
        assert_eq!(split_cells("Avail Use% Mounted on"), vec!["Avail Use% Mounted on"]);
        assert_eq!(
            split_cells("Filesystem     1K-blocks    Used"),
            vec!["Filesystem", "1K-blocks", "Used"]
        );
    }
}
