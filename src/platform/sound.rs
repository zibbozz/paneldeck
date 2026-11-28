//! Klänge abspielen (#164, Reiter „Zeit“): Wecker, Timer-Ende, Vorschau.
//!
//! Ein eigener Faden nimmt die Aufträge an ([`play`], [`stop`], [`stop_all`]) – die Oberfläche wartet nie auf das
//! Audiogerät. Jede Wiedergabe hat einen Schlüssel (z. B. `alarm:3`, `preview`); eine neue mit demselben Schlüssel ersetzt
//! die alte. `looping` wiederholt den Klang, bis `stop` kommt.
//!
//! - Windows: `rodio` (WASAPI über cpal; WAV über hound, MP3 über symphonia). Ohne Audiogerät passiert nichts.
//! - Linux/macOS: die Programme des Systems (`afplay` bzw. `paplay`, `pw-play`, `aplay` nur für WAV), eingebaute Klänge
//!   als WAV in `$XDG_RUNTIME_DIR` bzw. einem eigenen Ordner (0700) im Temp-Ordner; scheitert ein Programm, kommt das
//!   nächste, fehlen alle, bleibt es still (kein ALSA-Bauzwang für die Pakete).
//! - Höchstens [`MAX_PLAYING`] Klänge gleichzeitig.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};

/// Was abgespielt wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clip {
    /// WAV-Daten im Speicher (eingebaute Klänge).
    Wav(Arc<[u8]>),
    /// Eigene Datei (WAV oder MP3).
    File(PathBuf),
}

enum Cmd {
    Play { key: String, clip: Clip, looping: bool, volume: f32 },
    Stop(String),
    StopAll,
}

fn sender() -> Option<Sender<Cmd>> {
    static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
    let m = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let _ = std::thread::Builder::new().name("paneldeck-sound".into()).spawn(move || imp::run(rx));
        Mutex::new(tx)
    });
    m.lock().ok().map(|s| s.clone())
}

/// Spielt `clip` unter `key` ab (ersetzt eine laufende Wiedergabe mit demselben Schlüssel). `volume` 0…1.
pub fn play(key: &str, clip: Clip, looping: bool, volume: f32) {
    if cfg!(test) {
        return; // Tests spielen nie Klänge ab
    }
    if let Some(tx) = sender() {
        let _ = tx.send(Cmd::Play { key: key.to_string(), clip, looping, volume: volume.clamp(0.0, 1.0) });
    }
}

pub fn stop(key: &str) {
    if cfg!(test) {
        return;
    }
    if let Some(tx) = sender() {
        let _ = tx.send(Cmd::Stop(key.to_string()));
    }
}

/// Alle Wiedergaben beenden (beim Beenden von Paneldeck).
pub fn stop_all() {
    if cfg!(test) {
        return;
    }
    if let Some(tx) = sender() {
        let _ = tx.send(Cmd::StopAll);
    }
}

/// Höchstens so viele Klänge gleichzeitig; ein weiterer beendet den ältesten.
pub const MAX_PLAYING: usize = 8;

/// Führt einen Auftrag aus, ohne dass eine Panik im Decoder den Klang-Faden beendet (wirkt in Builds mit `panic =
/// "unwind"`; der Release-Build bricht bei einer Panik ohnehin ab).
fn guarded(f: impl FnOnce()) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
}

#[cfg(windows)]
mod imp {
    use super::{guarded, Clip, Cmd, MAX_PLAYING};
    use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
    use std::io::{BufReader, Cursor};
    use std::sync::mpsc::Receiver;

    pub fn run(rx: Receiver<Cmd>) {
        let mut stream: Option<OutputStream> = None;
        // in Startreihenfolge (die älteste zuerst)
        let mut sinks: Vec<(String, Sink)> = Vec::new();
        while let Ok(cmd) = rx.recv() {
            guarded(|| handle(cmd, &mut stream, &mut sinks));
        }
    }

    fn stop_key(sinks: &mut Vec<(String, Sink)>, key: &str) {
        sinks.retain(|(k, s)| {
            if k == key {
                s.stop();
            }
            k != key
        });
    }

    fn handle(cmd: Cmd, stream: &mut Option<OutputStream>, sinks: &mut Vec<(String, Sink)>) {
        sinks.retain(|(_, s)| !s.empty());
        match cmd {
            Cmd::Play { key, clip, looping, volume } => {
                stop_key(sinks, &key);
                while sinks.len() >= MAX_PLAYING {
                    let (_, s) = sinks.remove(0);
                    s.stop();
                }
                if stream.is_none() {
                    *stream = OutputStreamBuilder::open_default_stream().ok().map(|mut s| {
                        s.log_on_drop(false);
                        s
                    });
                }
                let Some(st) = stream.as_ref() else { return };
                let sink = Sink::connect_new(st.mixer());
                sink.set_volume(volume);
                let ok = match clip {
                    // eingebaute Klänge sind kurz (< 100 KB): im Speicher wiederholen
                    Clip::Wav(bytes) => match Decoder::new(Cursor::new(bytes.to_vec())) {
                        Ok(d) if looping => {
                            sink.append(d.buffered().repeat_infinite());
                            true
                        }
                        Ok(d) => {
                            sink.append(d);
                            true
                        }
                        Err(_) => false,
                    },
                    // eigene Dateien: beim Wiederholen neu dekodieren statt alle Abtastwerte zu puffern
                    Clip::File(p) => match std::fs::File::open(&p) {
                        Ok(f) if looping => Decoder::new_looped(BufReader::new(f)).map(|d| sink.append(d)).is_ok(),
                        Ok(f) => Decoder::new(BufReader::new(f)).map(|d| sink.append(d)).is_ok(),
                        Err(_) => false,
                    },
                };
                if ok {
                    sinks.push((key, sink));
                }
            }
            Cmd::Stop(key) => stop_key(sinks, &key),
            Cmd::StopAll => {
                for (_, s) in sinks.drain(..) {
                    s.stop();
                }
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{guarded, Clip, Cmd, MAX_PLAYING};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    /// Endet ein Abspielprogramm schneller, gilt es als gescheitert (nächstes Programm bzw. aufgeben).
    const QUICK_EXIT: Duration = Duration::from_millis(400);
    /// Nach so vielen Fehlschlägen hintereinander wird ein wiederholter Klang aufgegeben.
    const MAX_FAILS: u32 = 3;

    struct Playing {
        key: String,
        path: PathBuf,
        looping: bool,
        volume: f32,
        /// Index in [`players`] des aktuellen Programms.
        player: usize,
        child: Option<Child>,
        started: Instant,
        fails: u32,
    }

    #[derive(Default)]
    struct State {
        playing: Vec<Playing>,
        /// Eingebaute Klänge als Dateien (Inhalt-Hash → Datei), beim Beenden gelöscht.
        files: HashMap<u64, PathBuf>,
    }

    pub fn run(rx: Receiver<Cmd>) {
        let mut st = State::default();
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(cmd) => {
                    let quit = matches!(cmd, Cmd::StopAll);
                    guarded(|| handle(cmd, &mut st));
                    if quit {
                        for (_, f) in st.files.drain() {
                            let _ = std::fs::remove_file(f);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            guarded(|| poll(&mut st));
        }
    }

    fn handle(cmd: Cmd, st: &mut State) {
        match cmd {
            Cmd::Play { key, clip, looping, volume } => {
                stop_key(st, &key);
                while st.playing.len() >= MAX_PLAYING {
                    let mut p = st.playing.remove(0);
                    kill(&mut p);
                }
                let Some(path) = file_of(&clip, &mut st.files) else { return };
                let mut p = Playing { key, path, looping, volume, player: 0, child: None, started: Instant::now(), fails: 0 };
                if start_from(&mut p, 0) {
                    st.playing.push(p);
                }
            }
            Cmd::Stop(key) => stop_key(st, &key),
            Cmd::StopAll => {
                for mut p in st.playing.drain(..) {
                    kill(&mut p);
                }
            }
        }
    }

    fn stop_key(st: &mut State, key: &str) {
        st.playing.retain_mut(|p| {
            if p.key == key {
                kill(p);
                false
            } else {
                true
            }
        });
    }

    /// Beendete Wiedergaben: bei Fehlschlag das nächste Programm, bei Erfolg wiederholen (wenn gewünscht) oder entfernen.
    fn poll(st: &mut State) {
        st.playing.retain_mut(|p| {
            let status = match p.child.as_mut().map(|c| c.try_wait()) {
                Some(Ok(None)) => return true, // läuft noch
                Some(Ok(Some(s))) => Some(s),
                _ => None,
            };
            let failed = !status.is_some_and(|s| s.success()) || p.started.elapsed() < QUICK_EXIT;
            p.child = None;
            if failed {
                p.fails += 1;
                // nächstes Programm versuchen
                if p.fails < MAX_FAILS && start_from(p, p.player + 1) {
                    return true;
                }
                return false;
            }
            p.fails = 0;
            p.looping && start_from(p, p.player)
        });
    }

    /// Startet das erste brauchbare Programm ab Index `from`. `false`, wenn keins startet.
    fn start_from(p: &mut Playing, from: usize) -> bool {
        let list = players(&p.path, p.volume);
        for (i, (prog, args)) in list.iter().enumerate().skip(from) {
            let mut c = Command::new(prog);
            c.args(args).arg(&p.path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            if let Ok(ch) = c.spawn() {
                p.child = Some(ch);
                p.player = i;
                p.started = Instant::now();
                return true;
            }
        }
        false
    }

    /// Abspielprogramme in Reihenfolge; `aplay` nur für WAV (kann kein MP3).
    fn players(path: &Path, volume: f32) -> Vec<(&'static str, Vec<String>)> {
        #[cfg(target_os = "macos")]
        {
            let _ = path;
            vec![("/usr/bin/afplay", vec!["-v".into(), format!("{volume:.2}")])]
        }
        #[cfg(not(target_os = "macos"))]
        {
            let wav = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"));
            let mut v: Vec<(&'static str, Vec<String>)> = vec![("paplay", vec![format!("--volume={}", (volume * 65536.0) as u32)]), ("pw-play", vec!["--volume".into(), format!("{volume:.2}")])];
            if wav {
                v.push(("aplay", vec!["-q".into()]));
            }
            v
        }
    }

    fn kill(p: &mut Playing) {
        if let Some(c) = p.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        p.child = None;
    }

    /// Ordner für die Dateien der eingebauten Klänge: `$XDG_RUNTIME_DIR` (nur für den Benutzer), sonst ein eigener Ordner
    /// mit Rechten 0700 im Temp-Ordner (es muss ein echter Ordner des Benutzers sein, kein Link).
    fn sound_dir() -> Option<PathBuf> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute() && d.is_dir()) {
            return Some(d);
        }
        let uid = unsafe { libc::getuid() };
        let d = std::env::temp_dir().join(format!("paneldeck-sound-{uid}"));
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&d);
        let m = std::fs::symlink_metadata(&d).ok()?;
        (m.is_dir() && m.uid() == uid && m.permissions().mode() & 0o077 == 0).then_some(d)
    }

    /// Eingebaute Klänge als Datei (je Inhalt einmal, neu angelegt mit `create_new` unter zufälligem Namen).
    fn file_of(clip: &Clip, files: &mut HashMap<u64, PathBuf>) -> Option<PathBuf> {
        use std::io::Write;
        match clip {
            Clip::File(p) => Some(p.clone()),
            Clip::Wav(bytes) => {
                let mut h: u64 = 1469598103934665603;
                for b in bytes.iter() {
                    h = (h ^ *b as u64).wrapping_mul(1099511628211);
                }
                if let Some(p) = files.get(&h).filter(|p| p.is_file()) {
                    return Some(p.clone());
                }
                let dir = sound_dir()?;
                let mut rnd = [0u8; 8];
                getrandom::fill(&mut rnd).ok()?;
                let p = dir.join(format!("paneldeck-sound-{:016x}.wav", u64::from_le_bytes(rnd)));
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p).ok()?;
                f.write_all(bytes).ok()?;
                files.insert(h, p.clone());
                Some(p)
            }
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    pub fn run(rx: std::sync::mpsc::Receiver<super::Cmd>) {
        while rx.recv().is_ok() {}
    }
}
