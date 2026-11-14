//! Shell-sicheres Quoting und Bereinigung von Argumenten, die in eine Shell getippt werden.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// bash, sh, Git Bash, WSL
    Posix,
    /// PowerShell 5/7
    Pwsh,
    /// cmd.exe
    Cmd,
}

pub fn kind_of_program(program: &str) -> Kind {
    let stem = std::path::Path::new(program).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    match stem.as_str() {
        "pwsh" | "powershell" => Kind::Pwsh,
        "cmd" => Kind::Cmd,
        _ => Kind::Posix,
    }
}

pub fn kind_of_id(shell_id: &str) -> Kind {
    match shell_id {
        "pwsh" | "powershell" => Kind::Pwsh,
        "cmd" => Kind::Cmd,
        _ => Kind::Posix,
    }
}

/// Setzt `s` so in Anführungszeichen, dass die Shell es als EIN wörtliches Argument liest.
pub fn quote(kind: Kind, s: &str) -> String {
    match kind {
        Kind::Posix => format!("'{}'", s.replace('\'', r"'\''")),
        Kind::Pwsh => {
            // In PowerShell gelten auch typografische Apostrophe als Anführungszeichen; sie müssen verdoppelt werden.
            let mut out = String::with_capacity(s.len() + 2);
            out.push('\'');
            for c in s.chars() {
                out.push(c);
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    out.push(c);
                }
            }
            out.push('\'');
            out
        }
        Kind::Cmd => format!("\"{}\"", s.replace('"', "")),
    }
}

/// Nur Pfade/Argumente quoten, wenn nötig (Lesbarkeit beim Einfügen).
pub fn quote_if_needed(kind: Kind, s: &str) -> String {
    // PowerShell wertet auch typografische Anführungszeichen (‘ ’ ‚ ‛ “ ” „) als Quotes.
    let pwsh_special = |c: char| kind == Kind::Pwsh && matches!(c, '\u{2018}'..='\u{201E}');
    if s.is_empty() || s.chars().any(|c| pwsh_special(c) || c.is_whitespace() || "&()[]{}^=;!'\"+,`~$#%<>|*?\\".contains(c) && !(c == '\\' && kind != Kind::Posix)) {
        quote(kind, s)
    } else {
        s.to_string()
    }
}

/// Aus gespeicherten `claude_args` nur harmlose Token übernehmen (Flags und einfache Werte).
pub fn sanitize_claude_args(s: &str) -> String {
    fn flag(t: &str) -> bool {
        let b = t.as_bytes();
        t.starts_with('-') && t.len() > 1 && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
    }
    fn value(t: &str) -> bool {
        !t.starts_with('-') && !t.is_empty() && t.len() <= 80 && t.chars().all(|c| c.is_ascii_alphanumeric() || "._:[]/@-".contains(c))
    }
    s.split_whitespace().filter(|t| flag(t) || value(t)).collect::<Vec<_>>().join(" ")
}

/// Gültige Konversations-ID (UUID-Form)?
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_quoting_neutralizes_expansion() {
        assert_eq!(quote(Kind::Posix, "a b"), "'a b'");
        assert_eq!(quote(Kind::Posix, "it's $(x) `y`"), r"'it'\''s $(x) `y`'");
    }

    #[test]
    fn powershell_quoting_handles_smart_quotes() {
        assert_eq!(quote(Kind::Pwsh, "a'b"), "'a''b'");
        assert_eq!(quote(Kind::Pwsh, "a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        assert_eq!(quote(Kind::Pwsh, "$env:X"), "'$env:X'");
    }

    #[test]
    fn args_are_sanitized() {
        assert_eq!(sanitize_claude_args("--model opus --permission-mode acceptEdits"), "--model opus --permission-mode acceptEdits");
        assert_eq!(sanitize_claude_args("--model x;touch${IFS}/tmp/p $(id) `x` --ok"), "--model --ok");
        assert_eq!(sanitize_claude_args("--model opus[1m]"), "--model opus[1m]");
    }

    #[test]
    fn uuids() {
        assert!(is_uuid("5d28a95e-dbaf-4f27-b2f3-6c0c42fab338"));
        assert!(!is_uuid("x; rm -rf"));
    }
}
