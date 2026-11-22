//! Klänge des Reiters „Zeit“ (#164): einige mitgelieferte Klänge – im Programm **erzeugt** (Sinus-Synthese, WAV im
//! Speicher, keine fremden Dateien; gehören zu Paneldeck, MIT) – oder eine eigene WAV-/MP3-Datei.
//!
//! Ein Klang wird als Text gespeichert: `builtin:<name>` (siehe [`BUILTIN`]) oder `file:<pfad>`. Unbekanntes fällt auf
//! den Standardklang zurück. Abgespielt wird über `platform::sound`.

use crate::platform::sound::Clip;
use crate::state::Lang;
use std::f32::consts::PI;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub const RATE: u32 = 22_050;
/// Größte eigene Datei (MB), die abgespielt wird.
pub const MAX_FILE_MB: u64 = 20;

/// (Name, Deutsch, Englisch)
pub const BUILTIN: &[(&str, &str, &str)] = &[
    ("chime", "Glockenspiel", "Chime"),
    ("beep", "Piepen", "Beep"),
    ("digital", "Digital", "Digital"),
    ("bell", "Glocke", "Bell"),
    ("soft", "Sanft", "Soft"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundRef {
    Builtin(&'static str),
    File(PathBuf),
}

impl SoundRef {
    /// Liest `builtin:<name>` bzw. `file:<pfad>`; Unbekanntes → `fallback`.
    pub fn parse(s: &str, fallback: &'static str) -> SoundRef {
        if let Some(p) = s.strip_prefix("file:") {
            let p = p.trim();
            if !p.is_empty() {
                return SoundRef::File(PathBuf::from(p));
            }
        }
        let name = s.strip_prefix("builtin:").unwrap_or(s).trim();
        let fb = fallback.strip_prefix("builtin:").unwrap_or(fallback);
        let n = BUILTIN.iter().find(|b| b.0 == name).or_else(|| BUILTIN.iter().find(|b| b.0 == fb)).map(|b| b.0).unwrap_or("chime");
        SoundRef::Builtin(n)
    }

    pub fn key(&self) -> String {
        match self {
            SoundRef::Builtin(n) => format!("builtin:{n}"),
            SoundRef::File(p) => format!("file:{}", p.display()),
        }
    }

    /// Anzeigename (eingebauter Name in der Sprache bzw. Dateiname).
    pub fn label(&self, lang: Lang) -> String {
        match self {
            SoundRef::Builtin(n) => BUILTIN.iter().find(|b| b.0 == *n).map(|b| lang.t(b.1, b.2).to_string()).unwrap_or_else(|| n.to_string()),
            SoundRef::File(p) => p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }

    /// Zum Abspielen; eine eigene Datei nur, wenn sie existiert, WAV/MP3 ist und nicht zu groß – sonst `Err` mit Grund.
    pub fn clip(&self, lang: Lang) -> Result<Clip, String> {
        match self {
            SoundRef::Builtin(n) => Ok(Clip::Wav(builtin_wav(n))),
            SoundRef::File(p) => {
                let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                if ext != "wav" && ext != "mp3" {
                    return Err(lang.t("Nur WAV- oder MP3-Dateien.", "Only WAV or MP3 files.").into());
                }
                let meta = std::fs::metadata(p).map_err(|_| lang.t("Datei nicht gefunden.", "File not found.").to_string())?;
                if meta.len() > MAX_FILE_MB * 1024 * 1024 {
                    return Err(format!("{} ({MAX_FILE_MB} MB)", lang.t("Datei zu groß", "File too large")));
                }
                Ok(Clip::File(p.clone()))
            }
        }
    }
}

/// WAV-Daten eines eingebauten Klangs (einmal erzeugt, danach geteilt).
pub fn builtin_wav(name: &str) -> Arc<[u8]> {
    type Cache = Vec<(&'static str, Arc<[u8]>)>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let all = CACHE.get_or_init(|| BUILTIN.iter().map(|b| (b.0, Arc::<[u8]>::from(wav(&synth(b.0))))).collect());
    all.iter().find(|(n, _)| *n == name).map(|(_, d)| d.clone()).unwrap_or_else(|| all[0].1.clone())
}

/// Erzeugt die Abtastwerte (−1…1) eines eingebauten Klangs.
pub fn synth(name: &str) -> Vec<f32> {
    let mut out = Vec::new();
    match name {
        // drei aufsteigende Töne (C6, E6, G6) mit weichem Ausklingen
        "chime" => {
            for (i, f) in [1046.5_f32, 1318.5, 1568.0].into_iter().enumerate() {
                add_tone(&mut out, i as f32 * 0.28, 0.9, |t| {
                    let env = (-t * 4.5).exp();
                    env * ((2.0 * PI * f * t).sin() + 0.25 * (4.0 * PI * f * t).sin())
                });
            }
            pad(&mut out, 1.6);
        }
        // klassisches Weckerpiepen: viermal kurz, dann Pause
        "beep" => {
            for i in 0..4 {
                add_tone(&mut out, i as f32 * 0.2, 0.12, |t| soft_square(880.0, t));
            }
            pad(&mut out, 1.3);
        }
        // Digitaluhr: zweimal zwei hohe Pieptöne
        "digital" => {
            for s in [0.0_f32, 0.12, 0.5, 0.62] {
                add_tone(&mut out, s, 0.07, |t| soft_square(2000.0, t));
            }
            pad(&mut out, 1.2);
        }
        // Glocke: Grundton mit unharmonischen Teiltönen, langes Ausklingen
        "bell" => {
            let f = 523.25_f32;
            add_tone(&mut out, 0.0, 2.2, |t| {
                let partial = |m: f32, a: f32, d: f32| a * (-t * d).exp() * (2.0 * PI * f * m * t).sin();
                partial(1.0, 1.0, 1.6) + partial(2.0, 0.5, 2.4) + partial(2.76, 0.35, 3.0) + partial(5.4, 0.2, 5.0)
            });
            pad(&mut out, 2.3);
        }
        // sanft: zwei weich ein- und ausgeblendete Töne
        _ => {
            for (i, f) in [659.25_f32, 880.0].into_iter().enumerate() {
                add_tone(&mut out, i as f32 * 0.7, 0.9, |t| {
                    let env = (PI * t / 0.9).sin().powi(2);
                    env * (2.0 * PI * f * t).sin()
                });
            }
            pad(&mut out, 2.0);
        }
    }
    // auf 0,6 normieren, kurze Ein-/Ausblendung gegen Knacken
    let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs())).max(1e-6);
    let n = out.len();
    let fade = (RATE as usize / 200).max(1);
    for (i, s) in out.iter_mut().enumerate() {
        let edge = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
        *s = *s / peak * 0.6 * edge;
    }
    out
}

fn add_tone(out: &mut Vec<f32>, start: f32, dur: f32, f: impl Fn(f32) -> f32) {
    let s0 = (start * RATE as f32) as usize;
    let len = (dur * RATE as f32) as usize;
    if out.len() < s0 + len {
        out.resize(s0 + len, 0.0);
    }
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        // 5 ms Anstieg und Abfall je Ton
        let edge = ((i.min(len - 1 - i)) as f32 / (RATE as f32 * 0.005)).min(1.0);
        out[s0 + i] += f(t) * edge;
    }
}

fn soft_square(f: f32, t: f32) -> f32 {
    // erste ungerade Obertöne eines Rechtecks: kräftig, aber nicht kratzig
    (1..=7).step_by(2).map(|k| (2.0 * PI * f * k as f32 * t).sin() / k as f32).sum()
}

fn pad(out: &mut Vec<f32>, secs: f32) {
    let n = (secs * RATE as f32) as usize;
    if out.len() < n {
        out.resize(n, 0.0);
    }
}

/// 16-Bit-Mono-WAV aus Abtastwerten.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sounds_are_valid_short_wavs() {
        for (name, de, en) in BUILTIN {
            assert!(!de.is_empty() && !en.is_empty());
            let s = synth(name);
            let secs = s.len() as f32 / RATE as f32;
            assert!((1.0..=2.5).contains(&secs), "{name}: {secs} s");
            let peak = s.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.5 && peak <= 0.6001, "{name}: Spitze {peak}");
            assert!(s[0].abs() < 1e-3 && s[s.len() - 1].abs() < 1e-3, "{name}: ohne Knacken");
            let w = builtin_wav(name);
            assert_eq!(&w[..4], b"RIFF");
            assert_eq!(&w[8..16], b"WAVEfmt ");
            assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize, s.len() * 2);
            assert_eq!(w.len(), 44 + s.len() * 2);
        }
        assert_ne!(synth("chime"), synth("bell"));
    }

    #[test]
    fn sound_refs_parse_with_fallback() {
        assert_eq!(SoundRef::parse("builtin:bell", "builtin:chime"), SoundRef::Builtin("bell"));
        assert_eq!(SoundRef::parse("builtin:gibtsnicht", "builtin:beep"), SoundRef::Builtin("beep"));
        assert_eq!(SoundRef::parse("", "builtin:beep"), SoundRef::Builtin("beep"));
        assert_eq!(SoundRef::parse("file:C:\\a\\b.mp3", "builtin:beep"), SoundRef::File(PathBuf::from("C:\\a\\b.mp3")));
        assert_eq!(SoundRef::parse("file:", "builtin:beep"), SoundRef::Builtin("beep"));
        assert_eq!(SoundRef::Builtin("bell").label(Lang::De), "Glocke");
        assert_eq!(SoundRef::File(PathBuf::from("/x/wake.mp3")).label(Lang::En), "wake.mp3");
        assert_eq!(SoundRef::parse("builtin:soft", "x").key(), "builtin:soft");
    }

    #[test]
    fn own_files_are_checked_before_playing() {
        let dir = std::env::temp_dir().join(format!("paneldeck-test-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("ok.wav");
        std::fs::write(&ok, wav(&synth("beep"))).unwrap();
        assert!(matches!(SoundRef::File(ok.clone()).clip(Lang::En), Ok(Clip::File(p)) if p == ok));
        assert!(SoundRef::File(dir.join("x.ogg")).clip(Lang::En).is_err());
        assert_eq!(SoundRef::File(dir.join("fehlt.wav")).clip(Lang::En).unwrap_err(), "File not found.");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
