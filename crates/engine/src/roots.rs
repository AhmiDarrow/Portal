//! Windows keeps only some trusted root certificates on disk and fetches the
//! rest from Windows Update the first time a program checks a site that needs
//! one. ffmpeg checks certificates without asking for that fetch, so on a new
//! PC a stream from such a site fails until something else, often a browser,
//! has visited it. One request through WinHTTP, which does ask, adds the root.

/// `https://host[:port]/path?query` as host, port and the path with its
/// query. None for anything else, or for an address with a login in it.
pub fn https_parts(url: &str) -> Option<(String, u16, String)> {
    let rest = url
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])?;
    let rest = rest.split('#').next().unwrap_or("");
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let object = match tail.chars().next() {
        Some('/') => tail.to_string(),
        Some(_) => format!("/{tail}"),
        None => "/".to_string(),
    };
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
        let (host, after) = inside.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(text) => text.parse::<u16>().ok().filter(|port| *port > 0)?,
        None => 443,
    };
    if host.is_empty()
        || host.contains(|ch: char| ch.is_whitespace() || ch.is_control())
        || object.contains(|ch: char| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }
    Some((host.to_string(), port, object))
}

/// Asks Windows to trust the root certificates of `url`'s host and of every
/// host it redirects to, fetching any that are missing. True when they are
/// all trusted afterwards. A HEAD request: no body is downloaded.
#[cfg(not(test))]
pub fn fetch_root(url: &str) -> bool {
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    };
    let Some((host, port, path)) = https_parts(url) else {
        return false;
    };
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() };
    let agent = wide("Portal");
    let server = wide(&host);
    let verb = wide("HEAD");
    let object = wide(&path);
    let null = std::ptr::null();
    unsafe {
        let session = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null, null, 0);
        if session.is_null() {
            return false;
        }
        WinHttpSetTimeouts(session, 10_000, 10_000, 10_000, 10_000);
        let connect = WinHttpConnect(session, server.as_ptr(), port, 0);
        let mut trusted = false;
        if !connect.is_null() {
            let request = WinHttpOpenRequest(
                connect,
                verb.as_ptr(),
                object.as_ptr(),
                null,
                null,
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            );
            if !request.is_null() {
                // Certificates are checked while the request is sent and
                // while redirects are followed.
                trusted = WinHttpSendRequest(request, null, 0, std::ptr::null(), 0, 0, 0) != 0
                    && WinHttpReceiveResponse(request, std::ptr::null_mut()) != 0;
                WinHttpCloseHandle(request);
            }
            WinHttpCloseHandle(connect);
        }
        WinHttpCloseHandle(session);
        trusted
    }
}

/// Tests never reach the network.
#[cfg(test)]
pub fn fetch_root(_url: &str) -> bool {
    false
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/roots.rs"]
mod tests;
