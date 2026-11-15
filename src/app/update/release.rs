//! GitHub-Releases auswerten: Antwort von `releases/latest` lesen, Dateien des Releases finden, `SHA256SUMS.txt` lesen,
//! Adressen prüfen (nur HTTPS, nur GitHub, nur dieses Repository) und entscheiden, was angeboten wird.

use super::version::Version;
use serde_json::Value;

/// Einzige Abfrage-Adresse (öffentliche API ohne Anmeldung; liefert nur reguläre Releases, keine Entwürfe/Pre-Releases).
pub const LATEST_URL: &str = "https://api.github.com/repos/zibbozz/paneldeck/releases/latest";
/// Präfix der Download-Adressen der Release-Dateien.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/zibbozz/paneldeck/releases/download/";
/// Präfix der Release-Seiten („Was ist neu“).
pub const PAGE_PREFIX: &str = "https://github.com/zibbozz/paneldeck/releases/";
pub const SUMS_NAME: &str = "SHA256SUMS.txt";

/// Hosts, zu denen eine Anfrage (auch nach Weiterleitung) gehen darf. GitHub leitet Release-Dateien von github.com auf
/// `release-assets.githubusercontent.com` (früher `objects.githubusercontent.com`) weiter.
pub const HOSTS: [&str; 4] = ["api.github.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

pub const MAX_JSON: u64 = 1 << 20;
pub const MAX_SUMS: u64 = 64 << 10;
pub const MAX_ASSET: u64 = 200 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub html_url: String,
    pub assets: Vec<Asset>,
}

pub fn setup_name(v: &Version) -> String {
    format!("Paneldeck-Setup-{v}.exe")
}

pub fn zip_name(v: &Version) -> String {
    format!("paneldeck-{v}-windows-x64.zip")
}

/// Pfad von `paneldeck.exe` im portablen ZIP.
pub fn zip_exe_entry(v: &Version) -> String {
    format!("paneldeck-{v}/paneldeck.exe")
}

/// Zerlegt eine HTTPS-Adresse in (Host, Pfad mit Abfrage). Lehnt alles ab, was nicht eindeutig ist:
/// anderes Schema, Benutzerangaben (`user@`), abweichender Port, Fragmente, Leer-/Steuerzeichen.
pub fn split_https(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') || url.contains('#') {
        return None;
    }
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.contains('@') {
        return None;
    }
    let host = match authority.split_once(':') {
        Some((h, "443")) => h,
        Some(_) => return None,
        None => authority,
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let path = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    Some((host.to_ascii_lowercase(), path))
}

/// Darf diese Adresse abgerufen werden? Nur HTTPS zu den GitHub-Hosts in `HOSTS` (genauer Hostname, keine Unterdomains).
pub fn allowed_url(url: &str) -> bool {
    split_https(url).is_some_and(|(h, _)| HOSTS.contains(&h.as_str()))
}

/// Release-Seite dieses Repositorys (nur dann wird sie im Browser geöffnet).
pub fn safe_release_page(url: &str) -> bool {
    url.starts_with(PAGE_PREFIX) && split_https(url).is_some() && !url[PAGE_PREFIX.len()..].contains("..")
}

/// Antwort von `releases/latest` lesen. Entwürfe, Pre-Releases und Tags, die keine reguläre Version `vX.Y.Z` sind, werden abgelehnt.
pub fn parse_latest(json: &[u8]) -> Result<Release, String> {
    let v: Value = serde_json::from_slice(json).map_err(|e| format!("JSON: {e}"))?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Err("pre-release/draft".into());
    }
    let tag = v["tag_name"].as_str().ok_or("tag_name")?.to_string();
    let version = Version::from_tag(&tag).ok_or_else(|| format!("tag {tag}"))?;
    let html_url = v["html_url"].as_str().unwrap_or_default().to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x["name"].as_str()?.to_string(),
                        url: x["browser_download_url"].as_str()?.to_string(),
                        size: x["size"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { tag, version, html_url, assets })
}

impl Release {
    /// Datei des Releases mit genau diesem Namen, deren Adresse auf dieses Repository und dieses Tag zeigt.
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        let want = format!("{DOWNLOAD_PREFIX}{}/{name}", self.tag);
        self.assets.iter().find(|a| a.name == name && a.url == want)
    }
}

/// `SHA256SUMS.txt` im Format von `sha256sum`: `<64 hex>  <datei>` (Binärmodus `<hex> *<datei>` ebenso). Ungültige Zeilen
/// werden übersprungen; ein BOM am Anfang und CRLF stören nicht.
pub fn parse_sums(text: &str) -> Vec<([u8; 32], String)> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r');
            let (hash, rest) = l.split_once(' ')?;
            let name = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;
            let name = name.trim_end();
            if name.is_empty() || name.contains(['/', '\\']) {
                return None;
            }
            Some((super::sha256::parse_hex(hash)?, name.to_string()))
        })
        .collect()
}

/// Erwartete Prüfsumme einer Datei; `None`, wenn sie fehlt oder widersprüchlich doppelt vorkommt.
pub fn expected_hash(sums: &[([u8; 32], String)], name: &str) -> Option<[u8; 32]> {
    let mut found = sums.iter().filter(|(_, n)| n == name).map(|(h, _)| *h);
    let first = found.next()?;
    found.all(|h| h == first).then_some(first)
}

/// Art der Installation (bestimmt, welche Datei geladen wird).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Mit dem Installer eingerichtet: Setup still in denselben Ordner ausführen.
    Installed,
    /// Portables ZIP: `paneldeck.exe` ersetzen.
    Portable,
}

/// Was mit einer neueren Version möglich ist.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// Automatisch installierbar: Datei samt Prüfsummen-Datei.
    Install { asset: Asset, sums: Asset },
    /// Nur ankündigen (Link zur Release-Seite).
    AnnounceOnly(NoInstall),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoInstall {
    /// Release ohne `SHA256SUMS.txt` (2.0.0 und älter).
    NoChecksums,
    /// Die passende Datei (Setup bzw. ZIP) fehlt im Release.
    NoAsset,
    /// Kein Windows: hier gibt es nichts zu installieren.
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: Version,
    pub html_url: String,
    pub plan: Plan,
    pub major: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    UpToDate,
    /// Neuere Version; `skipped` = der Nutzer hat genau diese Version übersprungen.
    Newer { offer: Offer, skipped: bool },
}

/// Entscheidung für ein abgefragtes Release: nie ein Downgrade, keine gleiche Version.
pub fn evaluate(current: &Version, rel: &Release, flavor: Option<Flavor>, skip_version: &str) -> Outcome {
    if rel.version <= *current {
        return Outcome::UpToDate;
    }
    let plan = match flavor {
        None => Plan::AnnounceOnly(NoInstall::Unsupported),
        Some(f) => {
            let name = match f {
                Flavor::Installed => setup_name(&rel.version),
                Flavor::Portable => zip_name(&rel.version),
            };
            match (rel.asset(SUMS_NAME), rel.asset(&name)) {
                (None, _) => Plan::AnnounceOnly(NoInstall::NoChecksums),
                (Some(_), None) => Plan::AnnounceOnly(NoInstall::NoAsset),
                (Some(_), Some(a)) if a.size > MAX_ASSET => Plan::AnnounceOnly(NoInstall::NoAsset),
                (Some(s), Some(a)) => Plan::Install { asset: a.clone(), sums: s.clone() },
            }
        }
    };
    let offer = Offer { version: rel.version.clone(), html_url: rel.html_url.clone(), plan, major: current.is_major_jump(&rel.version) };
    Outcome::Newer { skipped: !skip_version.is_empty() && skip_version == rel.version.to_string(), offer }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Gekürzte echte Antwort der GitHub-API (Felder, die wir lesen, plus Beiwerk).
    pub(crate) fn fixture(tag: &str, with_sums: bool) -> String {
        let v = tag.trim_start_matches('v');
        let mut assets = vec![
            format!(r#"{{"name":"paneldeck-{v}-windows-x64.zip","size":17399219,"content_type":"application/zip","browser_download_url":"https://github.com/zibbozz/paneldeck/releases/download/{tag}/paneldeck-{v}-windows-x64.zip","uploader":{{"login":"x"}}}}"#),
            format!(r#"{{"name":"Paneldeck-Setup-{v}.exe","size":15499036,"browser_download_url":"https://github.com/zibbozz/paneldeck/releases/download/{tag}/Paneldeck-Setup-{v}.exe"}}"#),
        ];
        if with_sums {
            assets.push(format!(r#"{{"name":"SHA256SUMS.txt","size":180,"browser_download_url":"https://github.com/zibbozz/paneldeck/releases/download/{tag}/SHA256SUMS.txt"}}"#));
        }
        format!(
            r#"{{"url":"https://api.github.com/repos/zibbozz/paneldeck/releases/1","html_url":"https://github.com/zibbozz/paneldeck/releases/tag/{tag}","id":1,"tag_name":"{tag}","target_commitish":"release/2.1","name":"Paneldeck {v} – Title","draft":false,"prerelease":false,"created_at":"2026-11-15T08:00:00Z","assets":[{}],"body":"Text"}}"#,
            assets.join(",")
        )
    }

    #[test]
    fn parses_fixture() {
        let r = parse_latest(fixture("v2.1.0", true).as_bytes()).unwrap();
        assert_eq!(r.version, Version::new(2, 1, 0));
        assert_eq!(r.tag, "v2.1.0");
        assert_eq!(r.html_url, "https://github.com/zibbozz/paneldeck/releases/tag/v2.1.0");
        assert_eq!(r.assets.len(), 3);
        assert_eq!(r.asset("Paneldeck-Setup-2.1.0.exe").unwrap().size, 15499036);
        assert!(r.asset(SUMS_NAME).is_some());
        assert!(r.asset("nope").is_none());
    }

    #[test]
    fn rejects_bad_releases() {
        let pre = fixture("v2.1.0", true).replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert!(parse_latest(pre.as_bytes()).is_err());
        let draft = fixture("v2.1.0", true).replace(r#""draft":false"#, r#""draft":true"#);
        assert!(parse_latest(draft.as_bytes()).is_err());
        assert!(parse_latest(fixture("v2.1.0-rc.1", true).as_bytes()).is_err());
        assert!(parse_latest(fixture("2.1.0", true).as_bytes()).is_err());
        assert!(parse_latest(b"not json").is_err());
        assert!(parse_latest(b"{}").is_err());
        // Fehlende Assets sind kein Fehler (nur ankündigen)
        assert!(parse_latest(br#"{"tag_name":"v3.0.0"}"#).unwrap().assets.is_empty());
    }

    #[test]
    fn asset_must_point_to_this_repo_and_tag() {
        let j = fixture("v2.1.0", true).replace("https://github.com/zibbozz/paneldeck/releases/download/v2.1.0/Paneldeck-Setup", "https://github.com/evil/paneldeck/releases/download/v2.1.0/Paneldeck-Setup");
        let r = parse_latest(j.as_bytes()).unwrap();
        assert!(r.asset("Paneldeck-Setup-2.1.0.exe").is_none());
        let j = fixture("v2.1.0", true).replace("download/v2.1.0/paneldeck-2.1.0", "download/v2.0.0/paneldeck-2.1.0");
        assert!(parse_latest(j.as_bytes()).unwrap().asset("paneldeck-2.1.0-windows-x64.zip").is_none());
    }

    #[test]
    fn sums_format() {
        let h1 = "a".repeat(64);
        let h2 = "B".repeat(64);
        let text = format!("\u{feff}{h1}  Paneldeck-Setup-2.1.0.exe\r\n{h2} *paneldeck-2.1.0-windows-x64.zip\n\nGarbage line\n{h1}  ../evil.exe\nabc  short.txt\n");
        let s = parse_sums(&text);
        assert_eq!(s.len(), 2);
        assert_eq!(expected_hash(&s, "Paneldeck-Setup-2.1.0.exe"), Some([0xaa; 32]));
        assert_eq!(expected_hash(&s, "paneldeck-2.1.0-windows-x64.zip"), Some([0xbb; 32]));
        assert_eq!(expected_hash(&s, "SHA256SUMS.txt"), None);
        // Widersprüchlich doppelt → nicht vertrauen
        let twice = format!("{h1}  a.exe\n{h2}  a.exe\n");
        assert_eq!(expected_hash(&parse_sums(&twice), "a.exe"), None);
        let same = format!("{h1}  a.exe\n{h1}  a.exe\n");
        assert!(expected_hash(&parse_sums(&same), "a.exe").is_some());
    }

    #[test]
    fn url_whitelist() {
        assert!(allowed_url(LATEST_URL));
        assert!(allowed_url("https://github.com/zibbozz/paneldeck/releases/download/v2.1.0/x.zip"));
        assert!(allowed_url("https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sp=r&sig=a%2Fb"));
        assert!(allowed_url("https://objects.githubusercontent.com/x"));
        assert!(allowed_url("https://GitHub.com:443/x"));
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.com/x",
            "https://evil.com/github.com",
            "https://sub.github.com/x",
            "https://user@github.com/x",
            "https://github.com@evil.com/x",
            "https://github.com:8443/x",
            "https://github.com/x#frag",
            "https://github.com/a b",
            "https://github.com\\@evil.com/",
            "ftp://github.com/x",
            "https://",
            "//github.com/x",
            "",
        ] {
            assert!(!allowed_url(bad), "{bad}");
        }
        assert_eq!(split_https("https://api.github.com?x=1"), Some(("api.github.com".into(), "/?x=1".into())));
        assert_eq!(split_https("https://github.com"), Some(("github.com".into(), "/".into())));
    }

    #[test]
    fn release_page_check() {
        assert!(safe_release_page("https://github.com/zibbozz/paneldeck/releases/tag/v2.1.0"));
        assert!(!safe_release_page("https://github.com/zibbozz/paneldeck-evil/releases/tag/v2.1.0"));
        assert!(!safe_release_page("https://github.com/zibbozz/paneldeck/releases/../../evil"));
        assert!(!safe_release_page("http://github.com/zibbozz/paneldeck/releases/tag/v2.1.0"));
        assert!(!safe_release_page("javascript:alert(1)"));
        assert!(!safe_release_page(""));
    }

    #[test]
    fn evaluate_offers() {
        let cur = Version::new(2, 1, 0);
        let r = |tag: &str, sums: bool| parse_latest(fixture(tag, sums).as_bytes()).unwrap();
        // Gleich oder älter: nie anbieten (kein Downgrade)
        assert_eq!(evaluate(&cur, &r("v2.1.0", true), Some(Flavor::Installed), ""), Outcome::UpToDate);
        assert_eq!(evaluate(&cur, &r("v2.0.0", true), Some(Flavor::Portable), ""), Outcome::UpToDate);
        // Neuere Version: passende Datei je Installationsart
        let Outcome::Newer { offer, skipped } = evaluate(&cur, &r("v2.2.0", true), Some(Flavor::Installed), "") else { panic!() };
        assert!(!skipped && !offer.major);
        let Plan::Install { asset, sums } = &offer.plan else { panic!() };
        assert_eq!(asset.name, "Paneldeck-Setup-2.2.0.exe");
        assert_eq!(sums.name, SUMS_NAME);
        let Outcome::Newer { offer, .. } = evaluate(&cur, &r("v2.2.0", true), Some(Flavor::Portable), "") else { panic!() };
        let Plan::Install { asset, .. } = &offer.plan else { panic!() };
        assert_eq!(asset.name, "paneldeck-2.2.0-windows-x64.zip");
        // Ohne Prüfsummen nur ankündigen
        let Outcome::Newer { offer, .. } = evaluate(&cur, &r("v2.2.0", false), Some(Flavor::Installed), "") else { panic!() };
        assert_eq!(offer.plan, Plan::AnnounceOnly(NoInstall::NoChecksums));
        // Fehlende Datei, kein Windows
        let mut rel = r("v2.2.0", true);
        rel.assets.retain(|a| !a.name.ends_with(".zip"));
        let Outcome::Newer { offer, .. } = evaluate(&cur, &rel, Some(Flavor::Portable), "") else { panic!() };
        assert_eq!(offer.plan, Plan::AnnounceOnly(NoInstall::NoAsset));
        let Outcome::Newer { offer, .. } = evaluate(&cur, &r("v2.2.0", true), None, "") else { panic!() };
        assert_eq!(offer.plan, Plan::AnnounceOnly(NoInstall::Unsupported));
        // Zu große Datei wird nicht geladen
        let mut rel = r("v2.2.0", true);
        rel.assets.iter_mut().for_each(|a| a.size = MAX_ASSET + 1);
        let Outcome::Newer { offer, .. } = evaluate(&cur, &rel, Some(Flavor::Installed), "") else { panic!() };
        assert_eq!(offer.plan, Plan::AnnounceOnly(NoInstall::NoAsset));
        // Hauptversion und übersprungene Version
        let Outcome::Newer { offer, skipped } = evaluate(&cur, &r("v3.0.0", true), Some(Flavor::Installed), "3.0.0") else { panic!() };
        assert!(offer.major && skipped);
        let Outcome::Newer { skipped, .. } = evaluate(&cur, &r("v3.0.1", true), Some(Flavor::Installed), "3.0.0") else { panic!() };
        assert!(!skipped, "eine neuere Version als die übersprungene wird wieder angeboten");
        // Eigene Pre-Release-Version: die reguläre gleiche Nummer ist neuer
        assert!(matches!(evaluate(&Version::parse("2.2.0-dev").unwrap(), &r("v2.2.0", true), Some(Flavor::Installed), ""), Outcome::Newer { .. }));
    }
}
