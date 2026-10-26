use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// Carpeta de datos: %APPDATA%\Swiftbeat
pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Swiftbeat")
}

/// Token de la Web API, cifrado (ver `secure`).
pub fn token_path() -> PathBuf {
    data_dir().join("webapi_token.bin")
}

/// Token de la Web API en claro, como lo guardaban las versiones hasta la 0.1.5.
pub fn legacy_token_path() -> PathBuf {
    data_dir().join("webapi_token.json")
}

/// Credenciales reutilizables de librespot, cifradas (ver `secure`).
pub fn credentials_path() -> PathBuf {
    data_dir().join("credentials.bin")
}

/// Donde librespot escribe las credenciales, en claro, al conectar.
pub fn plain_credentials_path() -> PathBuf {
    data_dir().join("credentials.json")
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub client_id: String,
    pub bitrate: u16,
    pub volume: f32,
    pub normalise: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            bitrate: 160,
            volume: 0.7,
            normalise: true,
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        data_dir().join("config.json")
    }

    pub fn load() -> Self {
        fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(data_dir());
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = fs::write(Self::path(), json);
        }
    }
}
