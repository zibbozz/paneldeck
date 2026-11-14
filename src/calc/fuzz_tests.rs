//! Fuzz-artige Prüfungen der Dauer-Eingabe mit festem Startwert: Müll darf nie abstürzen, Fehlerpositionen liegen im Text und die
//! Ausgabe lässt sich immer wieder einlesen.

use super::duration::*;
use crate::state::Lang;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "0", "1", "2", "7", "15", "30", "60", "99999999999999999999999", "0.5", "1,5", ".5", "5.", "1.2.3", "1,,5", "h", "m", "d", "min", "hours", "Std",
    "Tag", "t", "x", "µ", " ", "  ", "\t", "\n", "-", "+", "e", "1e5", "٣", "²", "😀", "ä", "İ", "ß", ":", "/", "NaN", "inf", "\u{0}", "\u{a0}",
];

#[test]
fn garbage_never_panics_and_error_positions_are_inside_the_text() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let hours = [8.0, 7.5, 24.0, 0.0, -3.0, f64::NAN, f64::INFINITY, 1e9, 0.0001];
    for _ in 0..40_000 {
        let n = 1 + rng.below(8);
        let s: String = (0..n).map(|_| PIECES[rng.below(PIECES.len())]).collect::<Vec<_>>().join(if rng.below(2) == 0 { " " } else { "" });
        let h = hours[rng.below(hours.len())];
        match parse_duration_with(&s, h) {
            Ok(m) => {
                let shown = format_duration(m, h, Lang::En);
                assert_eq!(parse_duration_with(&shown, h), Ok(m), "{s:?} -> {m} -> {shown:?} (Tag = {h} h)");
            }
            Err(e) => {
                assert!(e.pos <= s.chars().count(), "Position {} hinter dem Ende von {s:?}", e.pos);
                for lang in [Lang::De, Lang::En] {
                    assert!(!e.message(lang).is_empty());
                }
            }
        }
    }
}

#[test]
fn any_number_of_minutes_survives_print_and_read() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for h in [8.0, 7.5, 24.0, 0.5, 0.0001] {
        for m in [0u32, 1, 59, 60, 479, 480, 481, u32::MAX, u32::MAX - 1] {
            assert_eq!(parse_duration_with(&format_duration(m, h, Lang::En), h), Ok(m), "{m} bei {h} h");
        }
        for _ in 0..5_000 {
            let m = (rng.next() >> 32) as u32;
            assert_eq!(parse_duration_with(&format_duration(m, h, Lang::De), h), Ok(m), "{m} bei {h} h");
        }
    }
}
