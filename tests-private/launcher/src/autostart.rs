use super::*;

#[test]
fn the_entry_starts_the_tray_without_a_browser_from_the_run_key() {
    let e = entry().unwrap();
    assert_eq!(
        e.name, "Portal",
        "the Run value the installer writes and removes"
    );
    assert_eq!(e.args, ["--no-browser"]);
    assert!(e.run_key);
    assert_eq!(e.program, std::env::current_exe().unwrap());
}

/// Writes and removes the real Run value (on Linux, the autostart entry):
/// `cargo test -p portal-launcher -- --ignored`. Leaves it absent.
#[test]
#[ignore = "changes the Run registry key"]
fn live_toggle() {
    assert!(
        !enabled(),
        "a Portal sign-in entry already exists; not touching it"
    );
    set(true).unwrap();
    assert!(enabled());
    #[cfg(windows)]
    {
        let listed = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                NAME,
            ])
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        assert!(listed.contains("--no-browser"), "{listed}");
    }
    set(false).unwrap();
    assert!(!enabled());
    set(false).unwrap();
}
