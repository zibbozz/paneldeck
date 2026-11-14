//! Kommandozeilenargumente: `--open <ordner> [--claude]`, `--tray`.

use std::sync::OnceLock;

#[derive(Default, Debug, Clone)]
pub struct Cli {
    pub open: Option<String>,
    pub claude: bool,
    pub tray: bool,
}

static CLI: OnceLock<Cli> = OnceLock::new();

pub fn parse(args: &[String]) -> Cli {
    let mut c = Cli::default();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--open" => {
                if let Some(v) = it.next() {
                    // `--open "C:\"` (Laufwerkswurzel): Windows liest `\"` als maskiertes Anführungszeichen, der Rest der
                    // Kommandozeile landet im selben Argument (`C:" --claude`). Hier wieder auftrennen.
                    match v.split_once('"') {
                        Some((path, rest)) => {
                            c.open = Some(format!("{path}\\").replace("\\\\", "\\"));
                            if rest.split_whitespace().any(|t| t == "--claude") {
                                c.claude = true;
                            }
                            if rest.split_whitespace().any(|t| t == "--tray") {
                                c.tray = true;
                            }
                        }
                        None => c.open = Some(v.clone()),
                    }
                }
            }
            "--claude" => c.claude = true,
            "--tray" => c.tray = true,
            _ => {}
        }
    }
    c
}

pub fn init() -> &'static Cli {
    CLI.get_or_init(|| parse(&std::env::args().collect::<Vec<_>>()))
}

pub fn get() -> &'static Cli {
    CLI.get().unwrap_or_else(|| init())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let a: Vec<String> = ["x.exe", "--open", "C:\\Dev\\app", "--claude", "--tray"].iter().map(|s| s.to_string()).collect();
        let c = parse(&a);
        assert_eq!(c.open.as_deref(), Some("C:\\Dev\\app"));
        assert!(c.claude && c.tray);
        assert!(parse(&["x".to_string()]).open.is_none());
        // Laufwerkswurzel: `--open "C:\" --claude` kommt als ein Argument `C:" --claude`
        let a: Vec<String> = ["x.exe", "--open", "C:\\\" --claude"].iter().map(|s| s.to_string()).collect();
        let c = parse(&a);
        assert_eq!(c.open.as_deref(), Some("C:\\"));
        assert!(c.claude);
    }
}
