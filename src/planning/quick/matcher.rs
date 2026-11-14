//! Tolerantes Matching für die Suche im Quick-Menü.
//!
//! * Groß-/Kleinschreibung ist egal, Akzente und Umlaute werden normalisiert (`ä`→`a`, `ß`→`ss`, `é`→`e`); zusätzlich
//!   gilt die Schreibweise mit Doppelbuchstaben (`ae`, `oe`, `ue`), damit „prioritaet“ „Priorität“ findet.
//! * Die Eingabe darf aus mehreren Teilen bestehen („rel ver“): jeder Teil muss ein Wort des Textes treffen.
//! * Rangfolge (höher = besser): ganzer Text 1000 · Präfix des Textes 900 · Initialen („rv“ → *R*elease *V*ersion) 800 ·
//!   Wortanfang 700 · Initialen mit Wortteilen („relv“) 600 · Teilstring innerhalb eines Wortes 400. Innerhalb einer Stufe
//!   gewinnt der Text, der der Eingabe in der Länge näher ist (bis zu +30).

/// Normalisiert für den Vergleich: klein, Akzente weg, `ß`→`ss`. Mit `digraph` werden Umlaute zu `ae`/`oe`/`ue`.
fn fold_with(s: &str, digraph: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'ä' if digraph => out.push_str("ae"),
            'ö' if digraph => out.push_str("oe"),
            'ü' if digraph => out.push_str("ue"),
            'ä' | 'à' | 'á' | 'â' | 'ã' | 'å' | 'ā' => out.push('a'),
            'ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ø' | 'ō' => out.push('o'),
            'ü' | 'ù' | 'ú' | 'û' | 'ū' => out.push('u'),
            'è' | 'é' | 'ê' | 'ë' | 'ē' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' => out.push('i'),
            'ç' | 'ć' | 'č' => out.push('c'),
            'ñ' | 'ń' => out.push('n'),
            'ý' | 'ÿ' => out.push('y'),
            'š' | 'ś' => out.push('s'),
            'ž' | 'ź' | 'ż' => out.push('z'),
            'ß' => out.push_str("ss"),
            c => out.push(c),
        }
    }
    out
}

/// Normalisierte Form (ohne Doppelbuchstaben-Variante), z. B. für exakte Vergleiche.
pub fn fold(s: &str) -> String {
    fold_with(s, false)
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect()
}

/// Wie gut passt `query` zu `text`? `None` = gar nicht, sonst die Punktzahl (siehe Moduldoku). Leere Eingabe: `Some(0)`.
pub fn score(query: &str, text: &str) -> Option<u32> {
    if query.split_whitespace().next().is_none() {
        return Some(0);
    }
    let a = score_variant(&fold_with(query, false), &fold_with(text, false));
    let b = score_variant(&fold_with(query, true), &fold_with(text, true));
    a.max(b)
}

/// Beste Punktzahl über mehrere Bezeichnungen (z. B. Name und Übersetzung).
pub fn best_score<'a>(query: &str, texts: impl IntoIterator<Item = &'a str>) -> Option<u32> {
    texts.into_iter().filter_map(|t| score(query, t)).max()
}

fn score_variant(q: &str, t: &str) -> Option<u32> {
    let qn = q.split_whitespace().collect::<Vec<_>>().join(" ");
    let tn = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let bonus = |tier: u32| tier + 30u32.saturating_sub((tn.chars().count().saturating_sub(qn.chars().count())) as u32);
    if tn == qn {
        return Some(1000);
    }
    if tn.starts_with(&qn) {
        return Some(bonus(900));
    }
    let tw = words(&tn);
    let compact: Vec<char> = qn.chars().filter(|c| !c.is_whitespace()).collect();
    let hump = if compact.len() >= 2 && compact.iter().all(|c| c.is_alphanumeric()) { humps(&compact, &tw) } else { None };
    if hump == Some(true) {
        return Some(bonus(800));
    }
    // Mehrere Eingabeteile: jeder muss ein Wort treffen; die schwächste Stufe zählt.
    let mut tier = Some(u32::MAX);
    for tok in qn.split(' ') {
        let t_tier = if tw.iter().any(|w| w.starts_with(tok)) {
            700
        } else if tn.contains(tok) {
            400
        } else {
            tier = None;
            break;
        };
        tier = tier.map(|t| t.min(t_tier));
    }
    // Wortteile über mehrere Wörter („relv“) stehen zwischen Wortanfang und Teilstring.
    let best = tier.into_iter().chain(hump.map(|_| 600)).max()?;
    Some(bonus(best))
}

/// Läuft die Eingabe als Folge von Wortanfängen durch die Wörter? Jedes Wort darf übersprungen werden oder einen Anfang
/// (mindestens ein Buchstabe) der Eingabe aufnehmen, der mit dem Wortanfang beginnt. Ergebnis: `Some(true)`, wenn jedes
/// Wort nur einen Buchstaben beigetragen hat (reine Initialen), `Some(false)` bei längeren Wortteilen. Dynamische
/// Programmierung statt Ausprobieren: der Aufwand ist durch Eingabe × Wörter × Wortlänge begrenzt.
fn humps(q: &[char], words: &[&str]) -> Option<bool> {
    const MAX_QUERY: usize = 40;
    const MAX_WORDS: usize = 30;
    if q.len() > MAX_QUERY || words.len() > MAX_WORDS {
        return None;
    }
    let wc: Vec<Vec<char>> = words.iter().map(|w| w.chars().collect()).collect();
    let run = |single_only: bool| -> bool {
        // ok[wi][qi]: lässt sich q[qi..] aus words[wi..] bilden?
        let n = wc.len();
        let mut ok = vec![vec![false; q.len() + 1]; n + 1];
        ok[n][q.len()] = true;
        for wi in (0..n).rev() {
            for qi in (0..=q.len()).rev() {
                let mut v = ok[wi + 1][qi];
                if !v && qi < q.len() {
                    let max = if single_only { 1 } else { wc[wi].len().min(q.len() - qi) };
                    v = (1..=max).any(|k| k <= wc[wi].len() && q[qi..qi + k] == wc[wi][..k] && ok[wi + 1][qi + k]);
                }
                ok[wi][qi] = v;
            }
        }
        ok[0][0]
    };
    if run(true) {
        Some(true)
    } else if run(false) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, t: &str) -> Option<u32> {
        score(q, t)
    }

    #[test]
    fn folding_handles_case_accents_and_sharp_s() {
        assert_eq!(fold("Priorität"), "prioritat");
        assert_eq!(fold("ÄÖÜ ß É"), "aou ss e");
        assert_eq!(fold_with("Größe", true), "groesse");
        assert_eq!(fold("Straße"), "strasse");
    }

    #[test]
    fn empty_query_matches_everything_with_zero() {
        assert_eq!(s("", "Anything"), Some(0));
        assert_eq!(s("   ", "Anything"), Some(0));
    }

    #[test]
    fn case_does_not_matter() {
        assert_eq!(s("PRIORITY", "priority"), Some(1000));
        assert!(s("priori", "PrioRity").unwrap() >= 900);
    }

    #[test]
    fn umlauts_are_normalised_both_ways() {
        assert_eq!(s("prioritat", "Priorität"), Some(1000));
        assert_eq!(s("prioritaet", "Priorität"), Some(1000));
        assert_eq!(s("Priorität", "prioritaet"), Some(1000));
        assert!(s("ueber", "Übersicht").unwrap() >= 900);
        assert!(s("uber", "Übersicht").unwrap() >= 900);
        assert!(s("gross", "Größe").unwrap() >= 900);
        assert!(s("schaerfung", "Schärfung").is_some());
        assert!(s("cafe", "Café").is_some());
    }

    #[test]
    fn prefix_beats_word_start_beats_substring() {
        let prefix = s("rel", "Release version").unwrap();
        let word = s("ver", "Release version").unwrap();
        let sub = s("ease", "Release version").unwrap();
        assert!(prefix > word && word > sub, "{prefix} {word} {sub}");
        assert_eq!(prefix / 100, 9);
        assert_eq!(word / 100, 7);
        assert_eq!(sub / 100, 4);
        assert!(s("xyz", "Release version").is_none());
    }

    #[test]
    fn initials_match_words_in_order() {
        assert_eq!(s("rv", "Release version").unwrap() / 100, 8);
        assert_eq!(s("sp", "Story points").unwrap() / 100, 8);
        assert_eq!(s("str", "Steps to reproduce").unwrap() / 100, 8);
        assert_eq!(s("sr", "Steps to reproduce").unwrap() / 100, 8, "ein Wort darf fehlen");
        assert!(s("vr", "Release version").is_none(), "falsche Reihenfolge");
        assert_eq!(s("RV", "release version").unwrap() / 100, 8);
        // auch mit Wortteilen: „relv“ = Rel + V
        assert_eq!(s("relv", "Release version").unwrap() / 100, 6);
        assert!(s("rn", "Release version").is_none(), "Teile müssen am Wortanfang beginnen");
    }

    #[test]
    fn initials_rank_between_prefix_and_word_start() {
        let init = s("rv", "Release version").unwrap();
        let word = s("rv", "Prio rv thing").unwrap();
        assert!(init > word, "{init} {word}");
        assert!(s("re", "Release version").unwrap() > init);
    }

    #[test]
    fn multi_part_queries_need_every_part() {
        assert!(s("rel ver", "Release version").is_some());
        assert!(s("ver rel", "Release version").is_some(), "Reihenfolge der Teile ist frei");
        assert!(s("rel xyz", "Release version").is_none());
        assert_eq!(s("story poi", "Story points").unwrap() / 100, 9, "ganzer Präfix");
        assert_eq!(s("point stor", "Story points").unwrap() / 100, 7);
    }

    #[test]
    fn exact_beats_everything_and_shorter_wins_a_tie() {
        assert_eq!(s("done", "Done"), Some(1000));
        let a = s("st", "Status").unwrap();
        let b = s("st", "Story points").unwrap();
        let c = s("st", "Steps to reproduce").unwrap();
        assert!(a > b && b > c, "{a} {b} {c}");
    }

    #[test]
    fn best_score_takes_the_strongest_alias() {
        assert_eq!(best_score("priorität", ["Priority", "Priorität"]), Some(1000));
        assert_eq!(best_score("zzz", ["Priority", "Priorität"]), None);
    }

    #[test]
    fn nasty_inputs_are_safe() {
        let long = "a".repeat(5_000);
        let t = std::time::Instant::now();
        let _ = s(&long, &long);
        let _ = s("aaaaaaaaaaaaaaaaaaaa", "a a a a a a a a a a a a a a a a a a a a b");
        let _ = s("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaab", &"aa ".repeat(30));
        assert!(t.elapsed().as_secs() < 3, "Backtracking darf nicht ausufern");
        for q in ["\0", "💥", "{{x}}", "%", "\\", "(", "[a-z]+", "ß", "İ"] {
            let _ = s(q, "Release version");
        }
    }
}
