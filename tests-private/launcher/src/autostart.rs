use super::*;

#[test]
fn run_command_quotes_the_path() {
    let exe = Path::new(r"C:\Users\A B\AppData\Local\Programs\Portal\portal-launcher.exe");
    assert_eq!(
        run_command(exe),
        r#""C:\Users\A B\AppData\Local\Programs\Portal\portal-launcher.exe" --no-browser"#
    );
}

/// Writes and removes the real Run value:
/// `cargo test -p portal-launcher -- --ignored`. Leaves it absent.
#[cfg(windows)]
#[test]
#[ignore = "changes the Run registry key"]
fn live_toggle() {
    assert!(
        !enabled(),
        "a Portal Run value already exists; not touching it"
    );
    set(true).unwrap();
    assert!(enabled());
    let listed = std::process::Command::new("reg")
        .args(["query", &format!(r"HKCU\{RUN_KEY}"), "/v", VALUE_NAME])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(listed.contains("--no-browser"), "{listed}");
    set(false).unwrap();
    assert!(!enabled());
    set(false).unwrap();
}
