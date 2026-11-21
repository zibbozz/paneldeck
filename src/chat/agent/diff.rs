//! Zeilen-Diff für die Freigabe von `edit_file`/`write_file` (#338): gemeinsamer Anfang/Ende, dazwischen eine
//! LCS-Tabelle (bei sehr großen Mittelteilen: alles alt entfernt, alles neu hinzugefügt). Unterschiede nur im
//! Zeilenende (`\r\n`/`\n`) zählen nicht.

/// Eine Zeile der Anzeige.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// Unverändert (Kontext), mit Zeilennummer der neuen Fassung.
    Same(usize, String),
    Add(usize, String),
    /// Entfernt, mit Zeilennummer der alten Fassung.
    Del(usize, String),
    /// So viele unveränderte Zeilen ausgelassen.
    Gap(usize),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Diff {
    pub lines: Vec<Line>,
    pub added: usize,
    pub removed: usize,
    /// Die Anzeige wurde nach `max_lines` Zeilen abgeschnitten.
    pub truncated: bool,
}

/// Größte LCS-Tabelle (Zellen); darüber wird der Mittelteil als ganz ersetzt gezeigt.
const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Add,
    Del,
}

/// Diff von `old` nach `new` mit `context` Kontextzeilen; höchstens `max_lines` Zeilen in der Anzeige.
pub fn diff(old: &str, new: &str, context: usize, max_lines: usize) -> Diff {
    let a: Vec<&str> = old.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let b: Vec<&str> = new.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let am = &a[pre..a.len() - suf];
    let bm = &b[pre..b.len() - suf];
    let mut ops: Vec<Op> = vec![Op::Same; pre];
    ops.extend(middle(am, bm));
    ops.extend(std::iter::repeat_n(Op::Same, suf));
    render(&a, &b, &ops, context, max_lines)
}

fn middle(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 || (n + 1).saturating_mul(m + 1) > MAX_CELLS {
        let mut v = vec![Op::Del; n];
        v.extend(std::iter::repeat_n(Op::Add, m));
        return v;
    }
    // t[i][j] = LCS-Länge von a[i..] und b[j..].
    let w = m + 1;
    let mut t = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * w + j] = if a[i] == b[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut v = Vec::with_capacity(n + m);
    while i < n && j < m {
        if a[i] == b[j] {
            v.push(Op::Same);
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            v.push(Op::Del);
            i += 1;
        } else {
            v.push(Op::Add);
            j += 1;
        }
    }
    v.extend(std::iter::repeat_n(Op::Del, n - i));
    v.extend(std::iter::repeat_n(Op::Add, m - j));
    v
}

fn render(a: &[&str], b: &[&str], ops: &[Op], context: usize, max_lines: usize) -> Diff {
    let mut d = Diff::default();
    // Positionen der Änderungen, um Kontext zu bestimmen.
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| **o != Op::Same).map(|(k, _)| k).collect();
    let near = |k: usize| -> bool {
        let p = changed.partition_point(|&c| c < k.saturating_sub(context));
        changed.get(p).is_some_and(|&c| c <= k + context)
    };
    let (mut i, mut j) = (0usize, 0usize);
    let mut gap = 0usize;
    let push = |d: &mut Diff, l: Line| {
        if d.lines.len() >= max_lines {
            d.truncated = true;
        } else {
            d.lines.push(l);
        }
    };
    for (k, op) in ops.iter().enumerate() {
        match op {
            Op::Same => {
                if near(k) {
                    if gap > 0 {
                        push(&mut d, Line::Gap(gap));
                        gap = 0;
                    }
                    push(&mut d, Line::Same(j + 1, b[j].to_string()));
                } else {
                    gap += 1;
                }
                i += 1;
                j += 1;
            }
            Op::Del => {
                if gap > 0 {
                    push(&mut d, Line::Gap(gap));
                    gap = 0;
                }
                push(&mut d, Line::Del(i + 1, a[i].to_string()));
                d.removed += 1;
                i += 1;
            }
            Op::Add => {
                if gap > 0 {
                    push(&mut d, Line::Gap(gap));
                    gap = 0;
                }
                push(&mut d, Line::Add(j + 1, b[j].to_string()));
                d.added += 1;
                j += 1;
            }
        }
    }
    if gap > 0 && !d.lines.is_empty() {
        push(&mut d, Line::Gap(gap));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_with_context_and_gaps() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 10\n", "line ten\nline 10b\n");
        let d = diff(&old, &new, 2, 100);
        assert_eq!((d.added, d.removed), (2, 1));
        assert_eq!(d.lines.first(), Some(&Line::Gap(7)));
        assert!(d.lines.contains(&Line::Del(10, "line 10".into())));
        assert!(d.lines.contains(&Line::Add(10, "line ten".into())) && d.lines.contains(&Line::Add(11, "line 10b".into())));
        assert_eq!(d.lines.last(), Some(&Line::Gap(8)));
    }

    #[test]
    fn new_files_line_endings_and_caps() {
        let d = diff("", "a\nb\n", 3, 100);
        assert_eq!((d.added, d.removed, d.lines.len()), (2, 0, 2));
        let d = diff("a\r\nb\r\n", "a\nb\n", 3, 100);
        assert_eq!((d.added, d.removed), (0, 0), "nur Zeilenenden: keine Änderung");
        assert!(d.lines.is_empty());
        let big: String = (0..500).map(|i| format!("{i}\n")).collect();
        let d = diff("", &big, 3, 50);
        assert!(d.truncated && d.lines.len() == 50 && d.added == 500);
        // sehr große Mittelteile: ganz ersetzt statt LCS
        let a: String = (0..3000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..3000).map(|i| format!("b{i}\n")).collect();
        let d = diff(&a, &b, 0, 10);
        assert_eq!((d.added, d.removed), (3000, 3000));
    }

    #[test]
    fn lcs_keeps_common_lines() {
        let d = diff("a\nb\nc\nd\n", "a\nx\nc\ny\n", 5, 100);
        assert_eq!((d.added, d.removed), (2, 2));
        assert!(d.lines.contains(&Line::Same(3, "c".into())));
    }
}
