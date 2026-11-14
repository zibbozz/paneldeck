fn main() {
    // Windows-Ressourcen (Icon + Versionsinfo) in die .exe einbetten.
    // Beim Cross-Build (Linux → windows-gnu) ist das Ziel windows, der Host nicht.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let mut v: Vec<u32> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        v.resize(4, 0);
        let out = std::env::var("OUT_DIR").unwrap();
        let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("assets")
            .join("icon.ico")
            .to_string_lossy()
            .replace('\\', "/");
        let rc = format!(
            r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},{d}
PRODUCTVERSION {a},{b},{c},{d}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040704b0"
    BEGIN
      VALUE "ProductName", "Paneldeck"
      VALUE "FileDescription", "Paneldeck - Terminal-Zentrale"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "paneldeck.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0407, 1200
  END
END
"#,
            a = v[0], b = v[1], c = v[2], d = v[3]
        );
        let path = std::path::Path::new(&out).join("app.rc");
        std::fs::write(&path, rc).unwrap();
        embed_resource::compile(&path, embed_resource::NONE).manifest_required().unwrap();
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
