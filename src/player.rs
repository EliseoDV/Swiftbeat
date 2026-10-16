use crate::meta::TrackInfo;
use librespot_core::{Session, SpotifyUri};
use librespot_playback::{
    audio_backend,
    config::{AudioFormat, Bitrate, PlayerConfig},
    mixer::{self, Mixer, MixerConfig},
    player::{Player, PlayerEventChannel},
};
use rand::seq::SliceRandom;
use std::{collections::VecDeque, sync::Arc, time::Instant};

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
            ..Default::default()
        };
        let backend = audio_backend::find(None).expect("backend de audio");
        let player = Player::new(config, session, mixer.get_soft_volume(), move || {
            backend(None, AudioFormat::default())
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
        self.load_current(true);
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

    fn load(&mut self, track: TrackInfo, play: bool) {
        match SpotifyUri::from_uri(&track.uri) {
            Ok(uri) => self.player.load(uri, play, 0),
            Err(e) => log::warn!("URI inválida {}: {e}", track.uri),
        }
        self.current = Some(track);
        self.playing = play;
        self.set_pos(0);
    }

    fn load_current(&mut self, play: bool) {
        if let Some(&i) = self.order.get(self.pos) {
            let t = self.context[i].clone();
            self.load(t, play);
        }
    }

    /// Pasa a la siguiente canción. `auto` indica que la anterior terminó sola.
    pub fn next(&mut self, auto: bool) {
        if auto
            && self.repeat == Repeat::Track
            && let Some(t) = self.current.clone()
        {
            self.load(t, true);
            return;
        }
        if let Some(t) = self.queue.pop_front() {
            self.load(t, true);
            return;
        }
        if self.order.is_empty() {
            return;
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
            self.load_current(true);
        } else if self.repeat != Repeat::Off {
            self.pos = 0;
            self.load_current(true);
        } else {
            // Fin del contexto: vuelve a la primera canción en pausa.
            self.pos = 0;
            self.load_current(false);
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
        self.load_current(true);
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
        } else {
            self.pos_at = Instant::now();
            self.player.play();
        }
        self.playing = !self.playing;
    }

    pub fn seek(&mut self, ms: u32) {
        if self.current.is_some() {
            self.player.seek(ms);
            self.set_pos(ms);
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
        if idx < self.queue.len() {
            self.queue.drain(..idx);
            if let Some(t) = self.queue.pop_front() {
                self.load(t, true);
            }
        } else {
            let k = idx - self.queue.len();
            self.queue.clear();
            if self.pos + 1 + k < self.order.len() {
                self.pos += 1 + k;
                self.load_current(true);
            }
        }
    }

    pub fn position_ms(&self) -> u32 {
        if self.playing {
            self.pos_ms + self.pos_at.elapsed().as_millis() as u32
        } else {
            self.pos_ms
        }
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
