use crate::meta::TrackInfo;
use librespot_core::{Session, SpotifyUri};
use librespot_playback::{
    audio_backend::Sink,
    config::{Bitrate, PlayerConfig},
    mixer::{self, Mixer, MixerConfig},
    player::{Player, PlayerEventChannel},
};
use rand::seq::SliceRandom;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Off,
    Context,
    Track,
}

/// Reproductor local: librespot decodifica y reproduce; aquí vive la cola.
pub struct Playback {
    player: Arc<Player>,
    mixer: Arc<dyn Mixer>,
    pub context_uri: String,
    context: Vec<TrackInfo>,
    order: Vec<usize>,
    pos: usize,
    queue: VecDeque<TrackInfo>,
    pub current: Option<TrackInfo>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub playing: bool,
    /// librespot confirmó que la canción actual está cargada (evento Playing o
    /// Paused). Hasta entonces el contador no avanza y la UI muestra «cargando».
    pub started: bool,
    /// Cuándo se pidió la carga actual, para detectar cargas que nunca terminan.
    pub load_at: Instant,
    /// Último aviso de avance del reproductor, para detectar atascos a mitad de canción.
    pub progress_at: Instant,
    pos_ms: u32,
    pos_at: Instant,
}

fn volume_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * u16::MAX as f32) as u16
}

impl Playback {
    pub fn new(session: Session, bitrate: u16, normalise: bool, volume: f32) -> (Self, PlayerEventChannel) {
        let mixer = mixer::find(None).expect("mezclador por software")(MixerConfig::default())
            .expect("abrir el mezclador");
        mixer.set_volume(volume_u16(volume));
        let config = PlayerConfig {
            bitrate: match bitrate {
                96 => Bitrate::Bitrate96,
                320 => Bitrate::Bitrate320,
                _ => Bitrate::Bitrate160,
            },
            normalisation: normalise,
            // Aviso de posición cada segundo: sirve para saber que el audio avanza.
            position_update_interval: Some(Duration::from_secs(1)),
            ..Default::default()
        };
        let player = Player::new(config, session, mixer.get_soft_volume(), || {
            Box::new(crate::audio::Output::new()) as Box<dyn Sink>
        });
        let events = player.get_player_event_channel();
        (
            Self {
                player,
                mixer,
                context_uri: String::new(),
                context: Vec::new(),
                order: Vec::new(),
                pos: 0,
                queue: VecDeque::new(),
                current: None,
                shuffle: false,
                repeat: Repeat::Off,
                playing: false,
                started: false,
                load_at: Instant::now(),
                progress_at: Instant::now(),
                pos_ms: 0,
                pos_at: Instant::now(),
            },
            events,
        )
    }

    pub fn set_session(&self, session: Session) {
        self.player.set_session(session);
    }

    pub fn stop(&mut self) {
        self.player.stop();
        self.playing = false;
        self.started = false;
    }

    pub fn play_context(&mut self, uri: String, tracks: Vec<TrackInfo>, start: Option<usize>) {
        if tracks.is_empty() {
            return;
        }
        let start = start
            .filter(|i| *i < tracks.len())
            .unwrap_or_else(|| if self.shuffle { rand::random_range(0..tracks.len()) } else { 0 });
        self.context_uri = uri;
        self.context = tracks;
        self.rebuild_order(start);
        if !self.load_current(true) {
            self.next(false);
        }
    }

    fn rebuild_order(&mut self, first: usize) {
        let mut order: Vec<usize> = (0..self.context.len()).collect();
        if self.shuffle {
            order.retain(|&i| i != first);
            order.shuffle(&mut rand::rng());
            order.insert(0, first);
            self.pos = 0;
        } else {
            self.pos = first;
        }
        self.order = order;
    }

    /// Pide a librespot la canción. Devuelve `false` si no se puede reproducir
    /// (archivos locales, URIs desconocidas…), para saltarla.
    fn load(&mut self, track: TrackInfo, play: bool) -> bool {
        let uri = match SpotifyUri::from_uri(&track.uri) {
            Ok(uri) => uri,
            Err(e) => {
                log::warn!("URI no reproducible {}: {e}", track.uri);
                return false;
            }
        };
        self.player.load(uri, play, 0);
        self.current = Some(track);
        self.playing = play;
        self.started = false;
        self.load_at = Instant::now();
        self.progress_at = Instant::now();
        self.set_pos(0);
        true
    }

    /// Vuelve a pedir la canción actual en la posición en que se quedó. Se usa
    /// tras reconectar la sesión, que deja colgadas las cargas de la anterior.
    pub fn reload(&mut self, play: bool) {
        let Some(t) = self.current.clone() else { return };
        let Ok(uri) = SpotifyUri::from_uri(&t.uri) else { return };
        let pos = self.position_ms();
        // Parar primero obliga a librespot a cargar de nuevo en lugar de
        // reutilizar el decodificador atascado de la sesión anterior.
        self.player.stop();
        self.player.load(uri, play, pos);
        self.playing = play;
        self.started = false;
        self.load_at = Instant::now();
        self.progress_at = Instant::now();
        self.set_pos(pos);
    }

    fn load_current(&mut self, play: bool) -> bool {
        match self.order.get(self.pos) {
            Some(&i) => {
                let t = self.context[i].clone();
                self.load(t, play)
            }
            None => false,
        }
    }

    /// Pasa a la siguiente canción. `auto` indica que la anterior terminó sola.
    /// Las canciones que no se pueden pedir se saltan sin detener la reproducción.
    pub fn next(&mut self, auto: bool) {
        let mut auto = auto;
        // Como mucho una vuelta completa, por si nada del contexto es reproducible.
        for _ in 0..=self.order.len() + self.queue.len() {
            if self.advance(auto) {
                return;
            }
            // Si la que falló era la repetida, no se insiste con ella.
            auto = false;
        }
        log::warn!("nada reproducible en el contexto");
        self.playing = false;
        self.started = false;
    }

    /// Un paso de `next`. Devuelve `false` si la canción elegida no se pudo pedir.
    fn advance(&mut self, auto: bool) -> bool {
        if auto
            && self.repeat == Repeat::Track
            && let Some(t) = self.current.clone()
        {
            return self.load(t, true);
        }
        if let Some(t) = self.queue.pop_front() {
            return self.load(t, true);
        }
        if self.order.is_empty() {
            return true;
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
            self.load_current(true)
        } else if self.repeat != Repeat::Off {
            self.pos = 0;
            self.load_current(true)
        } else {
            // Fin del contexto: vuelve a la primera canción en pausa.
            self.pos = 0;
            if !self.load_current(false) {
                self.playing = false;
                self.started = false;
            }
            true
        }
    }

    pub fn prev(&mut self) {
        if self.position_ms() > 3000 || self.order.is_empty() {
            self.seek(0);
            return;
        }
        if self.pos > 0 {
            self.pos -= 1;
        } else if self.repeat == Repeat::Context {
            self.pos = self.order.len() - 1;
        } else {
            self.seek(0);
            return;
        }
        if !self.load_current(true) {
            self.next(false);
        }
    }

    pub fn peek_next(&self) -> Option<&TrackInfo> {
        if self.repeat == Repeat::Track {
            return self.current.as_ref();
        }
        if let Some(t) = self.queue.front() {
            return Some(t);
        }
        let next = if self.pos + 1 < self.order.len() {
            self.pos + 1
        } else if self.repeat == Repeat::Context {
            0
        } else {
            return None;
        };
        self.order.get(next).map(|&i| &self.context[i])
    }

    pub fn preload_next(&self) {
        if let Some(uri) = self.peek_next().and_then(|t| SpotifyUri::from_uri(&t.uri).ok()) {
            self.player.preload(uri);
        }
    }

    pub fn toggle(&mut self) {
        if self.current.is_none() {
            return;
        }
        if self.playing {
            self.pos_ms = self.position_ms();
            self.player.pause();
        } else if !self.started {
            // librespot no tiene la canción (se detuvo o no llegó a cargar): se pide otra vez.
            self.reload(true);
            return;
        } else {
            self.pos_at = Instant::now();
            self.progress_at = Instant::now();
            self.player.play();
        }
        self.playing = !self.playing;
    }

    pub fn seek(&mut self, ms: u32) {
        if self.current.is_some() {
            self.player.seek(ms);
            self.set_pos(ms);
            self.progress_at = Instant::now();
        }
    }

    pub fn set_volume(&self, v: f32) {
        self.mixer.set_volume(volume_u16(v));
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        if let Some(&current) = self.order.get(self.pos) {
            self.rebuild_order(current);
        }
    }

    pub fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::Context,
            Repeat::Context => Repeat::Track,
            Repeat::Track => Repeat::Off,
        };
    }

    pub fn enqueue(&mut self, track: TrackInfo) {
        self.queue.push_back(track);
    }

    /// Lo que sonará después: primero la cola manual y luego el contexto.
    pub fn upcoming(&self, max: usize) -> Vec<TrackInfo> {
        let mut out: Vec<TrackInfo> = self.queue.iter().take(max).cloned().collect();
        let mut p = self.pos + 1;
        while out.len() < max && p < self.order.len() {
            out.push(self.context[self.order[p]].clone());
            p += 1;
        }
        out
    }

    /// Salta a la posición `idx` de la lista devuelta por `upcoming`.
    pub fn skip_to(&mut self, idx: usize) {
        let loaded = if idx < self.queue.len() {
            self.queue.drain(..idx);
            match self.queue.pop_front() {
                Some(t) => self.load(t, true),
                None => true,
            }
        } else {
            let k = idx - self.queue.len();
            self.queue.clear();
            if self.pos + 1 + k < self.order.len() {
                self.pos += 1 + k;
                self.load_current(true)
            } else {
                true
            }
        };
        if !loaded {
            self.next(false);
        }
    }

    /// Posición estimada, sin pasar de la duración de la canción.
    pub fn position_ms(&self) -> u32 {
        let raw = self.raw_position_ms();
        match self.duration_ms() {
            0 => raw,
            d => raw.min(d),
        }
    }

    /// Posición estimada sin recortar. Solo avanza cuando el audio de verdad suena.
    pub fn raw_position_ms(&self) -> u32 {
        if self.playing && self.started {
            self.pos_ms + self.pos_at.elapsed().as_millis() as u32
        } else {
            self.pos_ms
        }
    }

    /// Se pidió reproducir pero librespot aún no ha empezado.
    pub fn buffering(&self) -> bool {
        self.playing && !self.started && self.current.is_some()
    }

    pub fn duration_ms(&self) -> u32 {
        self.current.as_ref().map(|t| t.duration_ms).unwrap_or(0)
    }

    pub fn set_pos(&mut self, ms: u32) {
        self.pos_ms = ms;
        self.pos_at = Instant::now();
    }

    pub fn is_current(&self, uri: &SpotifyUri) -> bool {
        let u = uri.to_uri().unwrap_or_default();
        self.current.as_ref().is_some_and(|t| t.uri == u)
    }
}
