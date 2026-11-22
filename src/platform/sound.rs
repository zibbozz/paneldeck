//! Klänge abspielen (#164, Reiter „Zeit“): Wecker, Timer-Ende, Vorschau.
//!
//! Ein eigener Faden nimmt die Aufträge an ([`play`], [`stop`], [`stop_all`]) – die Oberfläche wartet nie auf das
//! Audiogerät. Jede Wiedergabe hat einen Schlüssel (z. B. `alarm:3`, `preview`); eine neue mit demselben Schlüssel ersetzt
//! die alte. `looping` wiederholt den Klang, bis `stop` kommt.
//!
//! - Windows: `rodio` (WASAPI über cpal; WAV über hound, MP3 über symphonia). Ohne Audiogerät passiert nichts.
//! - Linux/macOS: die Programme des Systems (`afplay` bzw. `paplay`, `pw-play`, `aplay`), eingebaute Klänge als WAV im
//!   Temp-Ordner; fehlt ein Programm, bleibt es still (kein ALSA-Bauzwang für die Pakete).

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

#[cfg(windows)]
mod imp {
    use super::{Clip, Cmd};
    use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
    use std::collections::HashMap;
    use std::io::{BufReader, Cursor};
    use std::sync::mpsc::Receiver;

    pub fn run(rx: Receiver<Cmd>) {
        let mut stream: Option<OutputStream> = None;
        let mut sinks: HashMap<String, Sink> = HashMap::new();
        while let Ok(cmd) = rx.recv() {
            sinks.retain(|_, s| !s.empty());
            match cmd {
                Cmd::Play { key, clip, looping, volume } => {
                    if let Some(s) = sinks.remove(&key) {
                        s.stop();
                    }
                    if stream.is_none() {
                        stream = OutputStreamBuilder::open_default_stream().ok().map(|mut s| {
                            s.log_on_drop(false);
                            s
                        });
                    }
                    let Some(st) = stream.as_ref() else { continue };
                    let sink = Sink::connect_new(st.mixer());
                    sink.set_volume(volume);
                    let ok = match clip {
                        Clip::Wav(bytes) => append(&sink, Decoder::new(Cursor::new(bytes.to_vec())), looping),
                        Clip::File(p) => match std::fs::File::open(&p) {
                            Ok(f) => append(&sink, Decoder::new(BufReader::new(f)), looping),
                            Err(_) => false,
                        },
                    };
                    if ok {
                        sinks.insert(key, sink);
                    }
                }
                Cmd::Stop(key) => {
                    if let Some(s) = sinks.remove(&key) {
                        s.stop();
                    }
                }
                Cmd::StopAll => {
                    for (_, s) in sinks.drain() {
                        s.stop();
                    }
                }
            }
        }
    }

    fn append<R: std::io::Read + std::io::Seek + Send + Sync + 'static>(sink: &Sink, d: Result<Decoder<R>, rodio::decoder::DecoderError>, looping: bool) -> bool {
        let Ok(d) = d else { return false };
        if looping {
            sink.append(d.buffered().repeat_infinite());
        } else {
            sink.append(d);
        }
        true
    }
}

#[cfg(unix)]
mod imp {
    use super::{Clip, Cmd};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::Duration;

    struct Playing {
        path: PathBuf,
        looping: bool,
        volume: f32,
        child: Option<Child>,
    }

    pub fn run(rx: Receiver<Cmd>) {
        let mut playing: HashMap<String, Playing> = HashMap::new();
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Cmd::Play { key, clip, looping, volume }) => {
                    if let Some(mut p) = playing.remove(&key) {
                        kill(&mut p);
                    }
                    let Some(path) = file_of(&clip) else { continue };
                    let child = spawn(&path, volume);
                    if child.is_some() {
                        playing.insert(key, Playing { path, looping, volume, child });
                    }
                }
                Ok(Cmd::Stop(key)) => {
                    if let Some(mut p) = playing.remove(&key) {
                        kill(&mut p);
                    }
                }
                Ok(Cmd::StopAll) => {
                    for (_, mut p) in playing.drain() {
                        kill(&mut p);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // beendete Wiedergaben: wiederholen oder entfernen
            playing.retain(|_, p| {
                let done = p.child.as_mut().map(|c| matches!(c.try_wait(), Ok(Some(_)) | Err(_))).unwrap_or(true);
                if !done {
                    return true;
                }
                if p.looping {
                    p.child = spawn(&p.path, p.volume);
                    p.child.is_some()
                } else {
                    false
                }
            });
        }
    }

    fn kill(p: &mut Playing) {
        if let Some(c) = p.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Eingebaute Klänge als Datei im Temp-Ordner (einmal je Inhalt).
    fn file_of(clip: &Clip) -> Option<PathBuf> {
        match clip {
            Clip::File(p) => Some(p.clone()),
            Clip::Wav(bytes) => {
                let mut h: u64 = 1469598103934665603;
                for b in bytes.iter() {
                    h = (h ^ *b as u64).wrapping_mul(1099511628211);
                }
                let p = std::env::temp_dir().join(format!("paneldeck-sound-{h:016x}.wav"));
                if !p.exists() {
                    std::fs::write(&p, &bytes[..]).ok()?;
                }
                Some(p)
            }
        }
    }

    fn spawn(path: &PathBuf, volume: f32) -> Option<Child> {
        let quiet = |c: &mut Command| {
            c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        };
        #[cfg(target_os = "macos")]
        {
            let mut c = Command::new("/usr/bin/afplay");
            c.arg("-v").arg(format!("{volume:.2}")).arg(path);
            quiet(&mut c);
            return c.spawn().ok();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let tries: [(&str, Vec<String>); 3] = [
                ("paplay", vec![format!("--volume={}", (volume * 65536.0) as u32)]),
                ("pw-play", vec!["--volume".into(), format!("{volume:.2}")]),
                ("aplay", vec!["-q".into()]),
            ];
            for (prog, args) in tries {
                let mut c = Command::new(prog);
                c.args(&args).arg(path);
                quiet(&mut c);
                if let Ok(ch) = c.spawn() {
                    return Some(ch);
                }
            }
            None
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    pub fn run(rx: std::sync::mpsc::Receiver<super::Cmd>) {
        while rx.recv().is_ok() {}
    }
}
