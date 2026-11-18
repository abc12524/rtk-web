//! 预算裁剪：把已压缩的输出裁到调用方给的字符预算内，取舍**按内容重要性**而非位置。
//!
//! 为什么需要它：`cap` 用行数当预算（300 行），一条 5 字符的目录项和一条 2000 字符的
//! 日志行等价，所以压完之后可能是 291 字符，也可能是 16 000 字符；调用方（shell-tool /
//! go-shell / pi-shell-tool）只能在拿到结果后按字符数盲切，而盲切保留的是**头部**——
//! 正好把 `cap` 尾偏向保下来的报错切掉。
//!
//! 这里改成：调用方说"给我 N 字符以内"，服务端按下面的顺序留东西：
//!   1. 首行（表头 / 总计行）
//!   2. 命中错误关键字的所有行（从尾部优先，报错通常在后）
//!   3. 尾段若干行（延续 `cap` 的假设）
//!   4. 剩余预算在中间普通行里等间隔采样
//! 每一段空缺都写一条 `[… N lines elided …]`，与 `strategy` / `cap` 的既有标记一致。

/// 低于这个预算没有裁剪意义：连一条标记都放不下。
pub const MIN_BUDGET: usize = 200;
/// 上界只用来挡住误传的天文数字，超过就没必要裁了。
pub const MAX_BUDGET: usize = 4 << 20;

/// 命中即视为"必须留下"的关键字（小写比较）。
const IMPORTANT: &[&str] = &[
    "error", "warn", "fail", "fatal", "panic", "exception", "traceback", "denied", "refused",
    "not found", "no such file", "timeout", "timed out", "abort", "critical", "permission",
    "cannot ", "can't ", "command not found", "错误", "失败", "异常", "警告", "拒绝", "无法",
];

/// 降级阶梯：(尾段行数, 重要行上限, 采样点数)。
/// 从宽松到严格逐级尝试，第一个装得下的就用它。
const LADDER: &[(usize, usize, usize)] = &[
    (200, 400, 160),
    (120, 300, 100),
    (80, 200, 60),
    (50, 120, 40),
    (30, 80, 24),
    (20, 50, 12),
    (10, 30, 6),
    (5, 15, 3),
    (0, 10, 0),
    (0, 4, 0),
    (0, 0, 0),
];

#[derive(Debug, Clone)]
pub struct Fitted {
    pub text: String,
    /// 被省略掉的行数（所有标记之和），调用方可据此判断结果是否完整。
    pub elided_lines: usize,
}

/// 把 `text` 裁到 `max_chars` 个字符以内。
///
/// 已经装得下就返回 `None`（不动原文）。预算非法（小于 [`MIN_BUDGET`]）也返回 `None`，
/// 由调用方决定怎么报错——这里不 panic，避免把请求打成 500。
pub fn fit(text: &str, max_chars: usize) -> Option<Fitted> {
    if max_chars < MIN_BUDGET || text.chars().count() <= max_chars {
        return None;
    }

    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    if lines.len() == 1 {
        return Some(Fitted {
            text: hard_cut(lines[0], max_chars),
            elided_lines: 0,
        });
    }

    let important: Vec<usize> = (1..lines.len())
        .filter(|&i| is_important(lines[i]))
        .collect();

    // 行数预算必须服从字符预算：一行 1800 字符的输出，"留 200 行"是空话。
    // 先按平均行长估出预算装得下多少行，再拿它压住阶梯里的各级额度。
    let avg = text.chars().count() / lines.len();
    let affordable = (max_chars / avg.max(1)).max(1);

    for &(tail, imp_cap, samples) in LADDER {
        let keep = keep_set(
            lines.len(),
            &important,
            tail.min(affordable),
            imp_cap.min(affordable),
            samples.min(affordable),
        );
        let (rendered, elided) = render(&lines, &keep, max_chars);
        if rendered.chars().count() <= max_chars {
            return Some(Fitted {
                text: rendered,
                elided_lines: elided,
            });
        }
    }

    // 阶梯都装不下（预算极小的长行输出）：只留首行的前若干字符 + 一条标记。
    let head = hard_cut(lines[0], max_chars.saturating_sub(40));
    let out = format!("{head}\n[… {} lines elided …]", lines.len() - 1);
    Some(Fitted {
        text: truncate_to(out, max_chars),
        elided_lines: lines.len() - 1,
    })
}

fn is_important(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    IMPORTANT.iter().any(|kw| lower.contains(kw))
}

/// 选出要保留的行号集合：首行 + 尾段 + 重要行（后 N 条）+ 中间等间隔采样。
fn keep_set(
    total: usize,
    important: &[usize],
    tail: usize,
    imp_cap: usize,
    samples: usize,
) -> Vec<usize> {
    let mut keep: Vec<usize> = Vec::new();
    keep.push(0);

    let tail_start = total.saturating_sub(tail);
    keep.extend(tail_start..total);

    // 报错通常挨着尾部，所以重要行从后往前取，避免头部噪音把预算占满。
    for &i in important.iter().rev().take(imp_cap) {
        keep.push(i);
    }

    if samples > 0 {
        let taken: std::collections::HashSet<usize> = keep.iter().copied().collect();
        let pool: Vec<usize> = (1..total).filter(|i| !taken.contains(i)).collect();
        if !pool.is_empty() {
            // 等间隔取样，尽量让"看到的行"均匀覆盖整段输出。
            let step = (pool.len() as f64 / samples as f64).max(1.0);
            let mut i = 0.0;
            while (i as usize) < pool.len() {
                keep.push(pool[i as usize]);
                i += step;
            }
        }
    }

    keep.sort_unstable();
    keep.dedup();
    keep
}

/// 按保留集合渲染，空缺处写省略标记；返回 (文本, 省略行数)。
fn render(lines: &[&str], keep: &[usize], max_chars: usize) -> (String, usize) {
    let mut out = String::new();
    let mut elided = 0usize;
    let mut prev: Option<usize> = None;
    // 每行额度：留三成给省略标记，余下按保留行数平分；下限 200 字符，
    // 让普通短行不被无谓截断（装不下时靠阶梯减少行数，而不是把行切碎）。
    let line_cap = ((max_chars * 7 / 10) / keep.len().max(1)).max(200);

    for &i in keep {
        if let Some(p) = prev {
            if i > p + 1 {
                let gap = i - p - 1;
                elided += gap;
                out.push_str(&format!("[… {gap} lines elided …]\n"));
            }
        }
        out.push_str(&clip_line(lines[i], line_cap));
        out.push('\n');
        prev = Some(i);
    }
    if let Some(p) = prev {
        if p + 1 < lines.len() {
            let gap = lines.len() - p - 1;
            elided += gap;
            out.push_str(&format!("[… {gap} lines elided …]\n"));
        }
    }

    (out, elided)
}

fn clip_line(line: &str, cap: usize) -> String {
    let count = line.chars().count();
    if count <= cap {
        return line.to_string();
    }
    let kept: String = line.chars().take(cap).collect();
    format!("{kept} [+{} chars]", count - cap)
}

/// 单行截断：给标记留出余量，保证结果本身不超预算。
fn hard_cut(line: &str, cap: usize) -> String {
    let reserve = 48;
    let keep = cap.saturating_sub(reserve).max(1);
    let kept: String = line.chars().take(keep).collect();
    format!(
        "{kept}\n[… {} chars truncated …]",
        line.chars().count().saturating_sub(keep)
    )
}

fn truncate_to(text: String, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text;
    }
    let kept: String = text.chars().take(cap.saturating_sub(2)).collect();
    format!("{kept} …")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("2026-11-18 12:00:{:02} INFO  worker[{}] processed batch {i}", i % 60, i % 8))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn small_input_is_left_alone() {
        assert!(fit("a\nb\nc", 6000).is_none());
    }

    #[test]
    fn tiny_budget_is_rejected_rather_than_panicking() {
        assert!(fit(&log_lines(5000), MIN_BUDGET - 1).is_none());
    }

    #[test]
    fn result_never_exceeds_budget() {
        let text = log_lines(5000);
        for budget in [MIN_BUDGET, 400, 1000, 6000] {
            let fitted = fit(&text, budget).expect("over budget");
            let got = fitted.text.chars().count();
            assert!(got <= budget, "budget={budget} got={got}");
        }
    }

    #[test]
    fn trailing_error_survives_even_a_small_budget() {
        let mut text = log_lines(5000);
        text.push_str("\n2026-11-18 12:10:00 ERROR disk quota exceeded on /var/lib/data");
        let fitted = fit(&text, 6000).expect("over budget");
        assert!(
            fitted.text.contains("ERROR disk quota exceeded"),
            "尾部报错被裁掉了：\n{}",
            fitted.text
        );
        assert!(fitted.text.contains("lines elided"));
    }

    #[test]
    fn mid_stream_errors_are_kept_too() {
        let mut lines: Vec<String> = (0..2000).map(|i| format!("INFO step {i}")).collect();
        lines[900] = "ERROR mid-stream failure #42".to_string();
        let text = lines.join("\n");
        let fitted = fit(&text, 1200).expect("over budget");
        assert!(fitted.text.contains("ERROR mid-stream failure #42"));
    }

    #[test]
    fn first_line_is_always_kept() {
        let mut text = String::from("Filesystem Size Used Avail Use% Mounted on");
        for i in 0..3000 {
            text.push_str(&format!("\n/dev/sdX{i} 100G 40G 60G 40% /mnt/{i}"));
        }
        let fitted = fit(&text, 800).expect("over budget");
        assert!(fitted.text.starts_with("Filesystem Size Used Avail"));
    }

    #[test]
    fn elided_count_matches_what_was_dropped() {
        let text = log_lines(1000);
        let fitted = fit(&text, 2000).expect("over budget");
        let shown = fitted.text.lines().filter(|l| !l.contains("lines elided")).count();
        assert_eq!(shown + fitted.elided_lines, 1000);
    }

    #[test]
    fn single_huge_line_is_cut_inside_not_dropped() {
        let text = "x".repeat(50_000);
        let fitted = fit(&text, 500).expect("over budget");
        assert!(fitted.text.chars().count() <= 500);
        assert!(fitted.text.contains("chars truncated"));
    }

    #[test]
    fn long_lines_within_budget_are_clipped_per_line() {
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("{}\n", format!("line{i} ").repeat(300)));
        }
        let fitted = fit(&text, 4000).expect("over budget");
        assert!(fitted.text.chars().count() <= 4000);
        assert!(fitted.text.contains("[+"));
    }
}
