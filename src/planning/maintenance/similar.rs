//! Lokaler Textvergleich für mögliche Duplikate: Ähnlichkeit zweier Titel in Prozent (normalisierte Levenshtein-Distanz auf
//! Zeichen, nach Kleinschreibung und zusammengefasstem Leerraum; Satzzeichen zählen nicht).

/// Text für den Vergleich: klein, nur Buchstaben/Ziffern, Wörter mit einem Leerzeichen getrennt.
pub fn normalize(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Levenshtein-Distanz auf Zeichen (zwei Zeilen Speicher).
pub fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Ähnlichkeit zweier schon normalisierter Texte in Prozent (0–100). Zwei leere Texte gelten nicht als ähnlich (0).
pub fn percent_normalized(a: &str, b: &str) -> u32 {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let max = a.len().max(b.len());
    // Schneller Ausschluss: der Längenunterschied allein begrenzt die Ähnlichkeit nach oben.
    let d = levenshtein(&a, &b);
    (((max - d.min(max)) as f64 / max as f64) * 100.0).round() as u32
}

/// Ähnlichkeit zweier Texte in Prozent (normalisiert vorher).
pub fn percent(a: &str, b: &str) -> u32 {
    percent_normalized(&normalize(a), &normalize(b))
}

/// Obergrenze der Ähnlichkeit allein aus den Längen (für den schnellen Ausschluss beim Vergleich vieler Paare).
pub fn length_bound(len_a: usize, len_b: usize) -> u32 {
    let max = len_a.max(len_b);
    if max == 0 {
        return 0;
    }
    ((len_a.min(len_b) as f64 / max as f64) * 100.0).floor() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_and_near_identical_titles() {
        assert_eq!(percent("Login fails on Safari", "Login fails on Safari"), 100);
        assert_eq!(percent("Login fails on Safari", "login  fails on safari!"), 100, "Groß-/Kleinschreibung, Leerraum, Satzzeichen zählen nicht");
        assert!(percent("Login fails on Safari", "Login fails in Safari") >= 80);
        assert!(percent("Login fails on Safari", "Export to CSV is slow") < 50);
        assert_eq!(percent("", ""), 0);
        assert_eq!(percent("abc", ""), 0);
    }

    #[test]
    fn levenshtein_basics() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(levenshtein(&c("kitten"), &c("sitting")), 3);
        assert_eq!(levenshtein(&c(""), &c("abc")), 3);
        assert_eq!(levenshtein(&c("äöü"), &c("aöü")), 1, "auf Zeichen, nicht Bytes");
    }

    #[test]
    fn length_bound_is_an_upper_bound() {
        for (a, b) in [("abc", "abcdef"), ("Login fails", "Login fails on Safari"), ("x", "y")] {
            let (na, nb) = (normalize(a), normalize(b));
            assert!(percent_normalized(&na, &nb) <= length_bound(na.chars().count(), nb.chars().count()));
        }
    }
}
