use super::*;

#[test]
fn password_failures_are_replaced() {
    assert_eq!(
        public_error("bad password SECRETTOKEN"),
        "Kodi setup failed."
    );
    assert_eq!(
        public_error("resolver token rejected"),
        "Kodi setup failed."
    );
    assert_eq!(public_error(""), "Kodi setup failed.");
    assert_eq!(public_error("Kodi exited."), "Kodi exited.");
}
