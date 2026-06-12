use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/adwords";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub struct TokenSet {
    pub refresh_token: String,
    pub access_token: String,
    #[allow(dead_code)]
    pub expires_in: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("could not open a local callback port: {0}")]
    Bind(std::io::Error),
    #[error("timed out after 5 min waiting for the browser sign-in to finish")]
    Timeout,
    #[error("Google sign-in was denied: {0}")]
    Denied(String),
    #[error("callback state did not match — aborting (possible cross-site request)")]
    StateMismatch,
    #[error("HTTP error talking to oauth2.googleapis.com: {0}")]
    Http(#[from] reqwest::Error),
    #[error("oauth2.googleapis.com returned {status}: {body}")]
    Token { status: u16, body: String },
    #[error("Google returned no refresh token — re-run `bidsmith auth login` to consent again")]
    NoRefreshToken,
    #[error("local callback I/O error: {0}")]
    Io(std::io::Error),
}

/// Run the full loopback authorization-code flow with PKCE: bind an ephemeral
/// local port, open the browser to Google's consent screen, capture the
/// redirect, and exchange the code for a refresh + access token.
pub fn authorize(client_id: &str, client_secret: &str) -> Result<TokenSet, OAuthError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(OAuthError::Bind)?;
    let port = listener.local_addr().map_err(OAuthError::Bind)?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let verifier = random_b64url(32);
    let challenge = pkce_challenge(&verifier);
    let state = random_b64url(16);

    let auth_url = format!(
        "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &access_type=offline&prompt=consent&code_challenge={}&code_challenge_method=S256&state={}",
        percent_encode(client_id),
        percent_encode(&redirect_uri),
        percent_encode(SCOPE),
        percent_encode(&challenge),
        percent_encode(&state),
    );

    println!("Opening your browser to sign in with Google…");
    println!("If it doesn't open automatically, paste this into a browser:\n");
    println!("  {auth_url}\n");
    let _ = open_browser(&auth_url);

    let code = wait_for_code(&listener, &state)?;
    exchange_code(client_id, client_secret, &code, &redirect_uri, &verifier)
}

fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String, OAuthError> {
    listener.set_nonblocking(true).map_err(OAuthError::Io)?;
    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            return Err(OAuthError::Timeout);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).map_err(OAuthError::Io)?;
                let request = String::from_utf8_lossy(&buf[..n]);
                let params = parse_query(request_target(&request));

                if let Some(err) = params.get("error") {
                    respond(&mut stream, &page("Sign-in failed", "You can close this tab and return to the terminal."));
                    return Err(OAuthError::Denied(err.clone()));
                }
                match (params.get("code"), params.get("state")) {
                    (Some(code), Some(state)) if state == expected_state => {
                        respond(&mut stream, &page("Signed in", "bidsmith is connected. You can close this tab and return to the terminal."));
                        return Ok(code.clone());
                    }
                    (Some(_), _) => {
                        respond(&mut stream, &page("Sign-in failed", "State mismatch. You can close this tab."));
                        return Err(OAuthError::StateMismatch);
                    }
                    _ => {
                        // An unrelated request (e.g. favicon). Keep waiting.
                        respond(&mut stream, &page("Waiting…", "Waiting for Google sign-in to complete."));
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(OAuthError::Io(e)),
        }
    }
}

fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<TokenSet, OAuthError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    let response = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&form)
        .send()?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(OAuthError::Token { status: status.as_u16(), body });
    }

    #[derive(Deserialize)]
    struct Resp {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        expires_in: u64,
    }
    let parsed: Resp = response.json()?;
    let refresh_token = parsed.refresh_token.ok_or(OAuthError::NoRefreshToken)?;
    Ok(TokenSet {
        refresh_token,
        access_token: parsed.access_token,
        expires_in: parsed.expires_in,
    })
}

fn open_browser(url: &str) -> std::io::Result<()> {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
}

fn respond(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn page(title: &str, message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:30rem;margin:5rem auto;text-align:center;color:#1a1a1a}}\
         h1{{font-size:1.4rem}}p{{color:#555}}</style></head>\
         <body><h1>{title}</h1><p>{message}</p></body></html>",
    )
}

fn request_target(request: &str) -> &str {
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
}

fn parse_query(target: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let query = match target.split_once('?') {
        Some((_, q)) => q.split('#').next().unwrap_or(""),
        None => return map,
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn random_b64url(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("OS random source unavailable");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&buf)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(pkce_challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn parse_query_decodes_pairs() {
        let map = parse_query("/?code=4%2F0Ab&state=xyz&scope=a+b");
        assert_eq!(map.get("code").map(String::as_str), Some("4/0Ab"));
        assert_eq!(map.get("state").map(String::as_str), Some("xyz"));
        assert_eq!(map.get("scope").map(String::as_str), Some("a b"));
    }

    #[test]
    fn request_target_extracts_path() {
        assert_eq!(request_target("GET /?code=abc HTTP/1.1\r\nHost: x"), "/?code=abc");
    }

    #[test]
    fn percent_encode_escapes_reserved() {
        assert_eq!(percent_encode("http://127.0.0.1:8080"), "http%3A%2F%2F127.0.0.1%3A8080");
        assert_eq!(percent_encode("aZ09-._~"), "aZ09-._~");
    }
}
