//! The language table of the Syntax mode (#312). Ids are stable (they are stored in `settings.json` →
//! `editor.language_modes` / `editor.language_map` and per document in `editor-state.json`); only add, never rename.
//!
//! Besides the table, every other grammar of the bundled set (bat's grammars) can be picked; its id is its scope name
//! (e.g. `source.haskell`), see [`all_languages`].

/// Where the grammar of a language comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grammar {
    /// A grammar of the bundled set (bat via two-face), by its name.
    Bat(&'static str),
    /// One of our own grammars (`grammars/*.sublime-syntax`), by its name.
    Own(&'static str),
}

/// A language of the built-in table.
#[derive(Clone, Copy, Debug)]
pub struct Language {
    pub id: &'static str,
    pub name: &'static str,
    pub grammar: Grammar,
    /// File extensions (lowercase, without the dot).
    pub exts: &'static [&'static str],
    /// Exact file names (compared without case); a name ending in `.` matches as a prefix (`Dockerfile.` → `Dockerfile.dev`).
    pub names: &'static [&'static str],
    /// Interpreters in a shebang line (`#!/usr/bin/env python3` → `python`; digits and dots at the end are ignored).
    pub interpreters: &'static [&'static str],
    /// Names in a modeline (`vim: ft=…`, `-*- mode: … -*-`), lowercase.
    pub modelines: &'static [&'static str],
    /// Variables carry a sigil (`$x`, `%x%`): grammar scopes `variable.other.*` are coloured.
    pub sigil_vars: bool,
}

const fn lang(id: &'static str, name: &'static str, grammar: Grammar, exts: &'static [&'static str]) -> Language {
    Language { id, name, grammar, exts, names: &[], interpreters: &[], modelines: &[], sigil_vars: false }
}

const fn names(mut l: Language, n: &'static [&'static str]) -> Language {
    l.names = n;
    l
}

const fn interp(mut l: Language, i: &'static [&'static str]) -> Language {
    l.interpreters = i;
    l
}

const fn modes(mut l: Language, m: &'static [&'static str]) -> Language {
    l.modelines = m;
    l
}

const fn sigil(mut l: Language) -> Language {
    l.sigil_vars = true;
    l
}

use Grammar::{Bat, Own};

/// The built-in languages (issue #312 list plus a few common ones), sorted by name for the picker.
pub const LANGUAGES: &[Language] = &[
    sigil(modes(lang("autoit", "AutoIt", Own("AutoIt"), &["au3"]), &["autoit"])),
    sigil(interp(modes(lang("batch", "Batch", Bat("Batch File"), &["bat", "cmd", "btm"]), &["dosbatch", "bat", "batch"]), &[])),
    modes(lang("bibtex", "BibTeX", Bat("BibTeX"), &["bib"]), &["bib", "bibtex"]),
    modes(lang("c", "C", Bat("C"), &["c", "h"]), &["c"]),
    modes(lang("csharp", "C#", Bat("C#"), &["cs", "csx", "cake"]), &["cs", "csharp"]),
    modes(lang("cpp", "C++", Bat("C++"), &["cpp", "cc", "cxx", "c++", "cp", "hpp", "hh", "hxx", "h++", "inl", "ipp", "ixx", "cppm", "tpp"]), &["cpp", "c++"]),
    modes(names(lang("cmake", "CMake", Bat("CMake"), &["cmake"]), &["CMakeLists.txt"]), &["cmake"]),
    modes(lang("css", "CSS", Bat("CSS"), &["css"]), &["css"]),
    modes(lang("diff", "Diff", Bat("Diff"), &["diff", "patch", "rej"]), &["diff"]),
    modes(names(lang("dockerfile", "Dockerfile", Bat("Dockerfile"), &["dockerfile", "containerfile"]), &["Dockerfile", "Containerfile", "Dockerfile."]), &["dockerfile"]),
    modes(lang("go", "Go", Bat("Go"), &["go"]), &["go"]),
    modes(lang("html", "HTML", Bat("HTML"), &["html", "htm", "xhtml", "shtml", "hta"]), &["html"]),
    modes(lang("ini", "INI", Bat("INI"), &["ini", "inf", "reg", "cfg", "desktop", "url", "lng", "editorconfig"]), &["ini", "dosini"]),
    modes(lang("innosetup", "Inno Setup", Own("Inno Setup"), &["iss", "isi"]), &["iss", "innosetup"]),
    modes(lang("java", "Java", Bat("Java"), &["java", "bsh"]), &["java"]),
    interp(modes(lang("javascript", "JavaScript", Bat("JavaScript"), &["js", "mjs", "cjs", "htc"]), &["javascript", "js"]), &["node", "nodejs", "bun"]),
    modes(names(lang("json", "JSON", Bat("JSON"), &["json", "jsonc", "json5", "geojson", "webmanifest", "code-workspace", "har", "jsonl"]), &[".babelrc", ".eslintrc", ".prettierrc", "composer.lock", ".jshintrc"]), &["json"]),
    modes(lang("kotlin", "Kotlin", Bat("Kotlin"), &["kt", "kts"]), &["kotlin"]),
    modes(lang("latex", "LaTeX", Bat("LaTeX"), &["tex", "ltx", "latex"]), &["latex", "tex"]),
    modes(lang("less", "Less", Bat("Less"), &["less"]), &["less"]),
    interp(modes(lang("lua", "Lua", Bat("Lua"), &["lua", "luau", "rockspec"]), &["lua"]), &["lua", "luajit"]),
    sigil(interp(modes(names(lang("makefile", "Makefile", Bat("Makefile"), &["mk", "mak", "make"]), &["Makefile", "makefile", "GNUmakefile", "Makefile.am", "Makefile.in", "Makefile."]), &["make", "makefile"]), &["make"])),
    modes(lang("markdown", "Markdown", Bat("Markdown"), &["md", "markdown", "mdown", "mkd", "mkdn", "markdn"]), &["markdown", "md"]),
    modes(lang("nsis", "NSIS", Bat("NSIS"), &["nsi", "nsh"]), &["nsis"]),
    modes(lang("pascal", "Pascal / Delphi", Bat("Pascal"), &["pas", "pp", "dpr", "dpk", "lpr"]), &["pascal", "delphi"]),
    sigil(interp(modes(lang("perl", "Perl", Bat("Perl"), &["pl", "pm", "t", "pod", "psgi", "cgi"]), &["perl"]), &["perl"])),
    sigil(interp(modes(lang("php", "PHP", Bat("PHP"), &["php", "phtml", "php3", "php4", "php5", "php7", "php8", "phps", "phpt"]), &["php"]), &["php"])),
    modes(lang("plaintext", "Plain text", Bat("Plain Text"), &["txt", "text"]), &["text", "txt"]),
    sigil(interp(modes(lang("powershell", "PowerShell", Own("PowerShell"), &["ps1", "psm1", "psd1"]), &["ps1", "powershell", "pwsh"]), &["pwsh", "powershell"])),
    interp(modes(names(lang("python", "Python", Bat("Python"), &["py", "pyw", "pyi", "pyx", "pxd", "py3", "rpy", "gyp", "gypi"]), &["SConstruct", "SConscript", "wscript"]), &["python", "py"]), &["python", "pypy", "py"]),
    sigil(interp(modes(names(lang("ruby", "Ruby", Bat("Ruby"), &["rb", "rake", "gemspec", "ru", "rbw", "podspec", "thor", "jbuilder"]), &["Rakefile", "Gemfile", "Guardfile", "Podfile", "Vagrantfile", "Brewfile", "Capfile", "Fastfile"]), &["ruby"]), &["ruby", "jruby", "macruby", "rake"])),
    interp(modes(lang("rust", "Rust", Bat("Rust"), &["rs"]), &["rust"]), &["rust-script"]),
    modes(lang("scss", "SCSS", Bat("SCSS"), &["scss"]), &["scss"]),
    sigil(interp(modes(names(lang("shell", "Shell (Bash)", Bat("Bourne Again Shell (bash)"), &["sh", "bash", "zsh", "ksh", "ash", "dash", "bats", "command", "ebuild", "eclass"]), &[".bashrc", ".bash_profile", ".bash_login", ".bash_logout", ".bash_aliases", ".profile", ".zshrc", ".zprofile", ".zshenv", ".zlogin", ".kshrc", "PKGBUILD", "APKBUILD", ".envrc"]), &["sh", "bash", "zsh", "shell"]), &["sh", "bash", "zsh", "ksh", "dash", "ash", "mksh"])),
    modes(lang("sql", "SQL", Bat("SQL"), &["sql", "ddl", "dml", "psql", "pgsql", "mysql", "sqlite"]), &["sql"]),
    modes(lang("swift", "Swift", Bat("Swift"), &["swift"]), &["swift"]),
    modes(lang("tex", "TeX", Bat("TeX"), &["sty", "dtx", "ins"]), &["plaintex"]),
    modes(names(lang("toml", "TOML", Bat("TOML"), &["toml", "tml"]), &["Cargo.lock", "Pipfile", "poetry.lock", "uv.lock", "pdm.lock", "Gopkg.lock"]), &["toml"]),
    interp(modes(lang("typescript", "TypeScript", Bat("TypeScript"), &["ts", "mts", "cts"]), &["typescript", "ts"]), &["deno", "ts-node", "tsx"]),
    modes(lang("tsx", "TypeScript React / JSX", Bat("TypeScriptReact"), &["tsx", "jsx"]), &["tsx", "jsx", "typescriptreact"]),
    modes(lang("vb", "Visual Basic (VB.NET, VBA, VBScript)", Own("Visual Basic"), &["vb", "vbs", "vba", "bas", "frm", "ctl", "dsr", "vbhtml"]), &["vb", "vbnet", "vbscript", "vba"]),
    modes(lang("xml", "XML", Bat("XML"), &["xml", "ps1xml", "xsd", "xsl", "xslt", "svg", "xaml", "csproj", "vbproj", "fsproj", "vcxproj", "props", "targets", "nuspec", "resx", "plist", "wsdl", "rss", "atom", "manifest", "config", "wxs", "wxi", "xul", "kml", "gpx", "iml", "ruleset", "pubxml", "storyboard", "xib", "tld", "rng", "opml"]), &["xml"]),
    modes(lang("yaml", "YAML", Bat("YAML"), &["yaml", "yml", "sublime-syntax"]), &["yaml"]),
];

/// The built-in table entry for an id.
pub fn language(id: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.id == id)
}

/// One entry of the language picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LangInfo {
    pub id: String,
    pub name: String,
    /// From the built-in table (else another bundled grammar).
    pub builtin: bool,
    /// Extensions for display (`.rs, .rlib`).
    pub exts: Vec<String>,
}

/// All languages for the picker and the settings: the built-in table, then all other visible grammars of the bundled
/// set (sorted by name, id = scope name).
pub fn all_languages() -> &'static [LangInfo] {
    static ALL: std::sync::OnceLock<Vec<LangInfo>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<LangInfo> = LANGUAGES
            .iter()
            .map(|l| LangInfo { id: l.id.to_string(), name: l.name.to_string(), builtin: true, exts: l.exts.iter().map(|e| e.to_string()).collect() })
            .collect();
        let used: Vec<&str> = LANGUAGES.iter().filter_map(|l| if let Grammar::Bat(n) = l.grammar { Some(n) } else { None }).collect();
        let mut extra: Vec<LangInfo> = super::engine::bat_syntaxes()
            .filter(|s| !used.contains(&s.name.as_str()))
            .map(|s| LangInfo { id: s.scope.to_string(), name: s.name.clone(), builtin: false, exts: s.file_extensions.clone() })
            .collect();
        extra.sort_by_key(|l| l.name.to_lowercase());
        v.extend(extra);
        v
    })
}

/// Name of a language id for the status bar (`None` for unknown ids).
pub fn language_name(id: &str) -> Option<&'static str> {
    if let Some(l) = language(id) {
        return Some(l.name);
    }
    all_languages().iter().find(|l| l.id == id).map(|l| l.name.as_str())
}

/// Is this a known language id (table or another bundled grammar)?
pub fn is_known(id: &str) -> bool {
    language(id).is_some() || all_languages().iter().any(|l| l.id == id)
}
