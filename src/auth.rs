use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};

/// Client ID del cliente oficial de escritorio: es el único que Spotify acepta
/// para iniciar una sesión de reproducción con librespot.
pub const STREAMING_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
pub const REDIRECT_URI: &str = "http://127.0.0.1:8989/login";
const REDIRECT_ADDR: &str = "127.0.0.1:8989";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

const LIBRARY_SCOPES: &[&str] = &[
    "user-read-private",
    "user-library-read",
    "user-library-modify",
    "playlist-read-private",
    "playlist-read-collaborative",
    "playlist-modify-private",
    "playlist-modify-public",
    "user-follow-read",
    "user-follow-modify",
    "user-top-read",
    "user-read-recently-played",
];

pub fn streaming_scopes() -> Vec<&'static str> {
    let mut s = vec![
        "streaming",
        "user-read-playback-state",
        "user-modify-playback-state",
        "user-read-currently-playing",
    ];
    s.extend_from_slice(LIBRARY_SCOPES);
    s
}

pub fn web_scopes() -> Vec<&'static str> {
    LIBRARY_SCOPES.to_vec()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

impl Token {
    pub fn is_fresh(&self) -> bool {
        now() + 60 < self.expires_at
    }

    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_vec(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_b64(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Swiftbeat</title>\
<body style=\"background:#121212;color:#fff;font-family:Segoe UI,sans-serif;display:grid;place-items:center;height:90vh\">\
<h2>Listo, ya puedes volver a Swiftbeat.</h2></body>";

/// Autorización OAuth con PKCE: abre el navegador y espera la redirección local.
pub async fn authorize(http: &reqwest::Client, client_id: &str, scopes: &[&str]) -> Result<Token> {
    let verifier = random_b64(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_b64(16);
    let scope = scopes.join(" ");
    let url = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
            ("state", state.as_str()),
            ("scope", scope.as_str()),
        ],
    )?;

    let listener = TcpListener::bind(REDIRECT_ADDR)
        .await
        .context("no se pudo abrir el puerto 8989 (¿otra app lo está usando?)")?;
    open::that_detached(url.as_str()).context("no se pudo abrir el navegador")?;

    let code = tokio::time::timeout(Duration::from_secs(300), wait_for_code(listener, state))
        .await
        .map_err(|_| {
            anyhow!("Tiempo agotado. Revisa que la Redirect URI de tu app sea exactamente {REDIRECT_URI}")
        })??;

    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?;
    parse_token(resp, None).await
}

pub async fn refresh(http: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<Token> {
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await?;
    parse_token(resp, Some(refresh_token)).await
}

async fn parse_token(resp: reqwest::Response, old_refresh: Option<&str>) -> Result<Token> {
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = body["error_description"]
            .as_str()
            .or(body["error"].as_str())
            .unwrap_or("");
        bail!("Spotify rechazó el token ({status}): {msg}");
    }
    Ok(Token {
        access_token: body["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("respuesta sin access_token"))?
            .to_string(),
        refresh_token: body["refresh_token"]
            .as_str()
            .or(old_refresh)
            .unwrap_or_default()
            .to_string(),
        expires_at: now() + body["expires_in"].as_u64().unwrap_or(3600),
    })
}

/// Acepta conexiones hasta recibir una redirección válida. Cada conexión se atiende
/// en su propia tarea para que una pre-conexión vacía del navegador no bloquee.
async fn wait_for_code(listener: TcpListener, state: String) -> Result<String> {
    let (tx, mut rx) = mpsc::channel::<Result<String>>(1);
    loop {
        tokio::select! {
            Some(result) = rx.recv() => return result,
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                tokio::spawn(handle_redirect(stream, state.clone(), tx.clone()));
            }
        }
    }
}

async fn handle_redirect(mut stream: TcpStream, state: String, tx: mpsc::Sender<Result<String>>) {
    let mut buf = vec![0u8; 4096];
    let n = match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => n,
        _ => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    let Ok(url) = url::Url::parse(&format!("http://localhost{target}")) else {
        return;
    };
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();

    if let Some(err) = query.get("error") {
        respond(&mut stream, "200 OK", "Acceso cancelado. Puedes cerrar esta pestaña.").await;
        let _ = tx.send(Err(anyhow!("Spotify rechazó el acceso: {err}"))).await;
        return;
    }
    match (query.get("code"), query.get("state")) {
        (Some(code), Some(s)) if *s == state => {
            respond(&mut stream, "200 OK", DONE_PAGE).await;
            let _ = tx.send(Ok(code.clone())).await;
        }
        _ => respond(&mut stream, "404 Not Found", "").await,
    }
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}
