//! Small line diff (Myers) producing unified-style hunks as `DiffLine`s.

use crate::{DiffKind, DiffLine};

/// Context lines around each change.
const CONTEXT: usize = 3;
/// Edit-distance cap for Myers; beyond it the changed middle is shown as delete-all + add-all.
const MAX_D: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// (old index, new index)
    Equal(usize, usize),
    Del(usize),
    Ins(usize),
}

pub(crate) struct LineDiff {
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
}

pub(crate) fn diff_text(old: &str, new: &str) -> LineDiff {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = diff_ops(&a, &b);
    let added = ops.iter().filter(|o| matches!(o, Op::Ins(_))).count();
    let removed = ops.iter().filter(|o| matches!(o, Op::Del(_))).count();
    LineDiff { lines: hunks(&ops, &a, &b), added, removed }
}

fn diff_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let am = &a[prefix..a.len() - suffix];
    let bm = &b[prefix..b.len() - suffix];

    let mut ops: Vec<Op> = (0..prefix).map(|i| Op::Equal(i, i)).collect();
    let middle = myers(am, bm).unwrap_or_else(|| {
        (0..am.len()).map(Op::Del).chain((0..bm.len()).map(Op::Ins)).collect()
    });
    ops.extend(middle.into_iter().map(|op| match op {
        Op::Equal(x, y) => Op::Equal(x + prefix, y + prefix),
        Op::Del(x) => Op::Del(x + prefix),
        Op::Ins(y) => Op::Ins(y + prefix),
    }));
    let (ao, bo) = (a.len() - suffix, b.len() - suffix);
    ops.extend((0..suffix).map(|i| Op::Equal(ao + i, bo + i)));
    normalize_runs(ops)
}

/// Within each run of changes, put deletions before insertions.
fn normalize_runs(ops: Vec<Op>) -> Vec<Op> {
    let mut out = Vec::with_capacity(ops.len());
    let mut dels = Vec::new();
    let mut ins = Vec::new();
    for op in ops {
        match op {
            Op::Del(_) => dels.push(op),
            Op::Ins(_) => ins.push(op),
            Op::Equal(..) => {
                out.append(&mut dels);
                out.append(&mut ins);
                out.push(op);
            }
        }
    }
    out.append(&mut dels);
    out.append(&mut ins);
    out
}

/// Myers' O((N+M)D) diff. `None` if the edit distance exceeds `MAX_D`.
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    if n == 0 || m == 0 {
        return Some((0..a.len()).map(Op::Del).chain((0..b.len()).map(Op::Ins)).collect());
    }
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // trace[d] = v[-d..=d] at the start of step d (i.e. the state after step d-1).
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let idx = |k: isize| (k + offset) as usize;
    let mut found = None;
    'outer: for d in 0..=max.min(MAX_D) as isize {
        trace.push(v[idx(-d)..=idx(d)].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
                v[idx(k + 1)]
            } else {
                v[idx(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx(k)] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    found?;

    // Backtrack.
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let snap = &trace[d as usize];
        let at = |k: isize| snap[(k + d) as usize];
        let k = x - y;
        let (prev_x, prev_y) = if d == 0 {
            (0, 0)
        } else {
            let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
            let px = at(prev_k);
            (px, px - prev_k)
        };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(Op::Equal(x as usize, y as usize));
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Ins((y - 1) as usize));
            } else {
                ops.push(Op::Del((x - 1) as usize));
            }
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    Some(ops)
}

/// Hunks with `CONTEXT` lines of context; hunks are contiguous runs of `DiffLine`s, separated
/// by jumps in line numbers.
fn hunks(ops: &[Op], a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let changed: Vec<usize> =
        ops.iter().enumerate().filter(|(_, o)| !matches!(o, Op::Equal(..))).map(|(i, _)| i).collect();
    let mut keep = vec![false; ops.len()];
    for &i in &changed {
        let lo = i.saturating_sub(CONTEXT);
        let hi = (i + CONTEXT).min(ops.len() - 1);
        keep[lo..=hi].iter_mut().for_each(|k| *k = true);
    }
    ops.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(op, _)| match *op {
            Op::Equal(_, y) => DiffLine { kind: DiffKind::Context, line_no: y + 1, text: b[y].to_string() },
            Op::Del(x) => DiffLine { kind: DiffKind::Del, line_no: x + 1, text: a[x].to_string() },
            Op::Ins(y) => DiffLine { kind: DiffKind::Add, line_no: y + 1, text: b[y].to_string() },
        })
        .collect()
}

/// Plain-text unified-ish rendering (for tests and model-facing summaries).
#[cfg(test)]
pub(crate) fn render(lines: &[DiffLine]) -> String {
    lines
        .iter()
        .map(|l| {
            let sign = match l.kind {
                DiffKind::Context => ' ',
                DiffKind::Add => '+',
                DiffKind::Del => '-',
            };
            format!("{sign}{} {}", l.line_no, l.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(a: &[&str], b: &[&str], ops: &[Op]) -> (Vec<String>, Vec<String>) {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for op in ops {
            match *op {
                Op::Equal(x, y) => {
                    assert_eq!(a[x], b[y]);
                    old.push(a[x].to_string());
                    new.push(b[y].to_string());
                }
                Op::Del(x) => old.push(a[x].to_string()),
                Op::Ins(y) => new.push(b[y].to_string()),
            }
        }
        (old, new)
    }

    #[test]
    fn myers_roundtrip() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("a\nb\nc", "a\nb\nc"),
            ("a\nb\nc", "a\nx\nc"),
            ("a\nb\nc\nd\ne", "x\na\nc\ne\ny"),
            ("", "a\nb"),
            ("a\nb", ""),
            ("a\nb\na\nb\nc", "b\na\nc\nb\na"),
        ];
        for (o, n) in cases {
            let a: Vec<&str> = o.lines().collect();
            let b: Vec<&str> = n.lines().collect();
            let ops = diff_ops(&a, &b);
            let (old, new) = apply(&a, &b, &ops);
            assert_eq!(old, a);
            assert_eq!(new, b);
        }
    }

    #[test]
    fn minimal_change() {
        let d = diff_text("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!((d.added, d.removed), (1, 1));
        assert_eq!(render(&d.lines), " 1 a\n-2 b\n+2 B\n 3 c");
    }

    #[test]
    fn hunks_have_context_and_split() {
        let old: String = (1..=20).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l2\n", "L2\n").replace("l18\n", "L18\n");
        let d = diff_text(&old, &new);
        let nums: Vec<usize> = d.lines.iter().map(|l| l.line_no).collect();
        // hunk 1: 1, -2, +2, 3, 4, 5 ; hunk 2: 15, 16, 17, -18, +18, 19, 20
        assert_eq!(nums, vec![1, 2, 2, 3, 4, 5, 15, 16, 17, 18, 18, 19, 20]);
        assert_eq!(d.lines[1].kind, DiffKind::Del);
        assert_eq!(d.lines[2].kind, DiffKind::Add);
        assert_eq!(d.lines[2].text, "L2");
    }
}
