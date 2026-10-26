use crate::auth::{self, Token};
use anyhow::{Result, anyhow, bail};
use librespot_core::Session;
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::RwLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const BASE: &str = "https://api.spotify.com/v1";

/// Cliente mínimo de la Web API. Usa el Client ID del usuario y, si Spotify
/// rechaza la petición (endpoints recortados en modo desarrollo o límite de
/// cuota), reintenta una vez con el token de la sesión de librespot.
pub struct WebApi {
    http: reqwest::Client,
    client_id: String,
    token: Mutex<Token>,
    token_path: PathBuf,
    session: RwLock<Session>,
    fallback: Mutex<Option<(String, Instant)>>,
}

impl WebApi {
    pub fn new(
        http: reqwest::Client,
        client_id: String,
        token: Token,
        token_path: PathBuf,
        session: Session,
    ) -> Self {
        Self {
            http,
            client_id,
            token: Mutex::new(token),
            token_path,
            session: RwLock::new(session),
            fallback: Mutex::new(None),
        }
    }

    pub fn set_session(&self, session: Session) {
        if let Ok(mut s) = self.session.write() {
            *s = session;
        }
    }

    /// Token del usuario, renovado si caducó o si es el que Spotify acaba de
    /// rechazar. Si otra petición ya lo renovó mientras esta esperaba su turno,
    /// se usa ese sin pedir otro.
    async fn user_token(&self, rejected: Option<&str>) -> Result<String> {
        let mut t = self.token.lock().await;
        if rejected == Some(t.access_token.as_str()) || !t.is_fresh() {
            *t = auth::refresh(&self.http, &self.client_id, &t.refresh_token).await?;
            t.save(&self.token_path);
        }
        Ok(t.access_token.clone())
    }

    async fn fallback_token(&self) -> Result<String> {
        let mut cached = self.fallback.lock().await;
        if let Some((tok, exp)) = cached.as_ref()
            && Instant::now() < *exp
        {
            return Ok(tok.clone());
        }
        let session = self
            .session
            .read()
            .map_err(|_| anyhow!("sesión bloqueada"))?
            .clone();
        let t = session
            .login5()
            .auth_token()
            .await
            .map_err(|e| anyhow!("token de sesión: {e}"))?;
        let exp = Instant::now() + t.expires_in.saturating_sub(Duration::from_secs(60));
        *cached = Some((t.access_token.clone(), exp));
        Ok(t.access_token)
    }

    async fn send(
        &self,
        method: &Method,
        url: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response> {
        let mut req = self.http.request(method.clone(), url).bearer_auth(token);
        req = match body {
            Some(b) => req.json(b),
            None if *method != Method::GET => req.header("content-length", "0"),
            None => req,
        };
        Ok(req.send().await?)
    }

    pub async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{BASE}{path}")
        };
        let token = self.user_token(None).await?;
        let mut resp = self.send(&method, &url, &token, body).await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            resp = self.send(&method, &url, &self.user_token(Some(&token)).await?, body).await?;
        }
        let status = resp.status();
        if matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::TOO_MANY_REQUESTS
        ) {
            match self.fallback_token().await {
                Ok(tok) => {
                    log::info!("{method} {path} -> {status}; reintento con el token de la sesión");
                    resp = self.send(&method, &url, &tok, body).await?;
                }
                Err(e) => log::warn!("sin token de respaldo: {e}"),
            }
        }
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let snippet = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_string();
            bail!("{method} {path} -> {status} {snippet}");
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.request(Method::GET, path, None).await
    }

    /// Recorre una lista paginada (`items` + `next`) entregando cada página.
    pub async fn paged(&self, path: &str, max: usize, mut on_page: impl FnMut(&[Value])) -> Result<()> {
        let mut next = Some(path.to_string());
        let mut count = 0;
        while let Some(p) = next.take() {
            let v = self.get(&p).await?;
            // /me/following devuelve la página dentro de { "artists": { ... } }
            let page = if v.get("items").is_some() {
                &v
            } else {
                v.as_object().and_then(|o| o.values().next()).unwrap_or(&v)
            };
            let items = page["items"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            on_page(items);
            count += items.len();
            if count >= max || items.is_empty() {
                break;
            }
            next = page["next"].as_str().map(String::from);
        }
        Ok(())
    }
}
