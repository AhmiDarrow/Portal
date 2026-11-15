// Shared by the build scripts of `portal` and `portal-launcher`: embeds the
// app icon and version information into the Windows executable.

fn embed_windows_resources(description: &str) {
    println!("cargo:rerun-if-changed=../../assets/portal.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../../assets/portal.ico");
    let icon = icon.canonicalize().unwrap_or(icon);
    let icon = icon.display().to_string().replace('\\', "/").replace("//?/", "");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect();
    parts.resize(4, 0);
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Portal"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "LegalCopyright", "Copyright (c) 2026 The Portal authors. MIT License."
      VALUE "ProductName", "Portal"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("portal.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
