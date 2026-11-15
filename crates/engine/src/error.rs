use crate::rpc::RpcFail;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Message(String),
    #[error("Kodi was already running before this app. Restart leaves that window alone.")]
    ForeignRestart,
    #[error("Kodi was already running before this app. Quit leaves that window alone.")]
    ForeignQuit,
    #[error("Kodi was already running before this app. The window is left alone.")]
    ForeignWindow,
    #[error("{0}")]
    Unavailable(&'static str),
    #[error("{0}")]
    BadRequest(String),
    #[error("That item is gone.")]
    NotFound,
    #[error(transparent)]
    Rpc(#[from] RpcFail),
}

impl EngineError {
    pub fn public_message(&self) -> String {
        public_error(&self.to_string())
    }
}

/// Status and logs never carry a web password, an API key, or a sign-in token.
pub fn public_error(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || sensitive(trimmed) {
        return "Kodi setup failed.".to_string();
    }
    trimmed.to_string()
}

fn sensitive(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("password")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("api key")
        || lower.contains("token")
    {
        return true;
    }
    if let Some(scheme) = lower.find("://") {
        if lower[scheme..].contains('@') {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/error.rs"]
mod tests;
