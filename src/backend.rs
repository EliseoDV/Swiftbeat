//! Lógica de la aplicación. Un único actor recibe órdenes de la UI y de tareas
//! internas por un canal; las cargas lentas se lanzan como tareas aparte y
//! devuelven su resultado al actor, así la reproducción nunca espera a la red.

use crate::{
    Api, AppWindow, LyricLine,
    auth::{self, Token},
    config::{self, Config},
    meta::{self, CardInfo, KIND_ALBUM, KIND_ARTIST, KIND_LIKED, KIND_PLAYLIST, KIND_TRACK, LIKED_URI, TrackInfo},
    player::{Playback, Repeat},
    ui::{self, Ui},
    webapi::WebApi,
};
use anyhow::{Result, anyhow};
use librespot_core::{Session, SessionConfig, authentication::Credentials, cache::Cache};
use librespot_playback::player::PlayerEvent;
use reqwest::Method;
use serde_json::{Value, json};
use slint::ComponentHandle;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub type Liked = Arc<RwLock<HashSet<String>>>;

/// Tiempo máximo que puede tardar una canción en empezar a sonar.
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Tiempo máximo sin avisos de avance mientras suena (llegan cada segundo).
const STALL_TIMEOUT: Duration = Duration::from_secs(12);
/// Tiempo máximo para conectar una sesión nueva.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Un hueco así entre dos ticks de 500 ms indica que el equipo estuvo suspendido.
const SLEEP_GAP: Duration = Duration::from_secs(20);

#[derive(Clone, PartialEq, Debug)]
enum Route {
    Home,
    Search,
    Settings,
    Liked,
    Album(String),
    Artist(String),
    Playlist(String),
}

pub enum Cmd {
    Login(String),
    Logout,
    Navigate(i32, String),
    Home,
    Search(String),
    GoSearch,
    Settings,
    Back,
    Forward,
    PlayRow(i32, i32),
    PlayCard(i32, String),
    TogglePlay,
    SetPlaying(bool),
    Next,
    Prev,
    Seek(u32),
    Volume(f32),
    Shuffle,
    Repeat,
    Enqueue(i32, i32),
    ToggleLike(String),
    CreatePlaylist,
    AddToPlaylist(i32, i32, String),
    LibraryFilter(i32),
    RightPanel(i32),
    Bitrate(u16),
    Normalise(bool),
    Restart,
    // Internas
    Player(PlayerEvent),
    Page(u64, Page),
    Library(LibraryData),
    LikedChunk(Vec<TrackInfo>, bool),
    PlayLoaded(String, Vec<TrackInfo>),
    Lyrics(String, Result<Vec<(i32, String)>, String>),
    PlaylistCreated(CardInfo),
    Toast(String),
    /// El vigilante detectó que la reproducción se atascó.
    Recover(&'static str),
    Tick,
}

type Shelves = Vec<(String, Vec<CardInfo>)>;

pub enum Page {
    Home {
        quick: Vec<CardInfo>,
        shelves: Shelves,
    },
    Search {
        gen_id: u64,
        top: Option<CardInfo>,
        tracks: Vec<TrackInfo>,
        shelves: Shelves,
    },
    Collection {
        header: Option<Header>,
        tracks: Vec<TrackInfo>,
        reset: bool,
        done: bool,
    },
    Artist {
        name: String,
        image: String,
        top: Vec<TrackInfo>,
        shelves: Shelves,
    },
    Failed(String),
}

#[derive(Clone, Default)]
pub struct Header {
    label: String,
    title: String,
    subtitle: String,
    description: String,
    image: String,
}

#[derive(Default)]
pub struct LibraryData {
    me_id: String,
    me_name: String,
    me_image: String,
    playlists: Vec<CardInfo>,
    albums: Vec<CardInfo>,
    artists: Vec<CardInfo>,
}

struct Services {
    session: Session,
    api: Arc<WebApi>,
    playback: Playback,
    cache: Cache,
}

struct Backend {
    ui: Ui,
    tx: UnboundedSender<Cmd>,
    http: reqwest::Client,
    cfg: Config,
    cfg_dirty: bool,
    /// Calidad y normalización con las que arrancó el reproductor; si los ajustes
    /// difieren, la UI ofrece reiniciar.
    applied: (u16, bool),
    svc: Option<Services>,
    history: Vec<Route>,
    hist_pos: usize,
    nav_gen: u64,
    route: Route,
    page_uri: String,
    page_tracks: Vec<TrackInfo>,
    search_tracks: Vec<TrackInfo>,
    search_gen: Arc<AtomicU64>,
    artist_uri: String,
    artist_top: Vec<TrackInfo>,
    liked: Liked,
    liked_tracks: Vec<TrackInfo>,
    liked_done: bool,
    lib: LibraryData,
    filter: i32,
    known: HashMap<String, CardInfo>,
    right_panel: i32,
    lyrics_uri: String,
    lyrics: Vec<(i32, String)>,
    lyrics_active: i32,
    toast_until: Option<Instant>,
    unavailable_streak: u32,
    ticks: u32,
    /// Intentos de recuperar `recover_uri`. Se cuentan por canción para no entrar
    /// en bucle con una que siempre se corta en el mismo punto.
    recoveries: u32,
    recover_uri: String,
    /// Recuperaciones seguidas sin que nada llegue a sonar.
    fail_streak: u32,
    recover_pending: bool,
    /// La sesión puede estar muerta aunque librespot no lo sepa (el equipo
    /// estuvo suspendido o el reloj saltó): se reconecta antes de usarla.
    session_suspect: bool,
    last_wall: SystemTime,
}

fn sender(tx: &UnboundedSender<Cmd>) -> impl Fn(Cmd) + Clone + 'static {
    let tx = tx.clone();
    move |c| {
        let _ = tx.send(c);
    }
}

/// Conecta los callbacks de la UI con el canal de órdenes.
pub fn bind(ui: &AppWindow, tx: &UnboundedSender<Cmd>, liked: Liked) {
    let api = ui.global::<Api>();
    let s = sender(tx);
    let f = s.clone();
    api.on_login(move |id| f(Cmd::Login(id.to_string())));
    let f = s.clone();
    api.on_logout(move || f(Cmd::Logout));
    let f = s.clone();
    api.on_navigate(move |k, uri| f(Cmd::Navigate(k, uri.to_string())));
    let f = s.clone();
    api.on_go_home(move || f(Cmd::Home));
    let f = s.clone();
    api.on_go_search(move || f(Cmd::GoSearch));
    let f = s.clone();
    api.on_go_settings(move || f(Cmd::Settings));
    let f = s.clone();
    api.on_back(move || f(Cmd::Back));
    let f = s.clone();
    api.on_forward(move || f(Cmd::Forward));
    let f = s.clone();
    api.on_search(move |q| f(Cmd::Search(q.to_string())));
    let f = s.clone();
    api.on_play_row(move |src, i| f(Cmd::PlayRow(src, i)));
    let f = s.clone();
    api.on_play_card(move |k, uri| f(Cmd::PlayCard(k, uri.to_string())));
    let f = s.clone();
    api.on_toggle_play(move || f(Cmd::TogglePlay));
    let f = s.clone();
    api.on_next(move || f(Cmd::Next));
    let f = s.clone();
    api.on_prev(move || f(Cmd::Prev));
    let f = s.clone();
    api.on_seek(move |ms| f(Cmd::Seek(ms.max(0) as u32)));
    let f = s.clone();
    api.on_set_volume(move |v| f(Cmd::Volume(v)));
    let f = s.clone();
    api.on_toggle_shuffle(move || f(Cmd::Shuffle));
    let f = s.clone();
    api.on_cycle_repeat(move || f(Cmd::Repeat));
    let f = s.clone();
    api.on_enqueue(move |src, i| f(Cmd::Enqueue(src, i)));
    let f = s.clone();
    api.on_toggle_like(move |uri| f(Cmd::ToggleLike(uri.to_string())));
    let f = s.clone();
    api.on_create_playlist(move || f(Cmd::CreatePlaylist));
    let f = s.clone();
    api.on_add_to_playlist(move |src, i, pl| f(Cmd::AddToPlaylist(src, i, pl.to_string())));
    let f = s.clone();
    api.on_set_library_filter(move |k| f(Cmd::LibraryFilter(k)));
    let f = s.clone();
    api.on_set_right_panel(move |p| f(Cmd::RightPanel(p)));
    let f = s.clone();
    api.on_set_bitrate(move |b| f(Cmd::Bitrate(b.clamp(96, 320) as u16)));
    let f = s.clone();
    api.on_set_normalise(move |n| f(Cmd::Normalise(n)));
    let f = s;
    api.on_restart(move || f(Cmd::Restart));
    api.on_is_liked(move |uri, _| liked.read().map(|l| l.contains(uri.as_str())).unwrap_or(false));
    api.on_open_url(|url| {
        let _ = open::that_detached(url.as_str());
    });
}

pub async fn run(ui: Ui, tx: UnboundedSender<Cmd>, mut rx: UnboundedReceiver<Cmd>, http: reqwest::Client, liked: Liked) {
    let mut b = Backend {
        ui,
        tx: tx.clone(),
        http,
        cfg: Config::load(),
        cfg_dirty: false,
        applied: (0, false),
        svc: None,
        history: Vec::new(),
        hist_pos: 0,
        nav_gen: 0,
        route: Route::Home,
        page_uri: String::new(),
        page_tracks: Vec::new(),
        search_tracks: Vec::new(),
        search_gen: Arc::new(AtomicU64::new(0)),
        artist_uri: String::new(),
        artist_top: Vec::new(),
        liked,
        liked_tracks: Vec::new(),
        liked_done: false,
        lib: LibraryData::default(),
        filter: -1,
        known: HashMap::new(),
        right_panel: 0,
        lyrics_uri: String::new(),
        lyrics: Vec::new(),
        lyrics_active: -1,
        toast_until: None,
        unavailable_streak: 0,
        ticks: 0,
        recoveries: 0,
        recover_uri: String::new(),
        fail_streak: 0,
        recover_pending: false,
        session_suspect: false,
        last_wall: SystemTime::now(),
    };
    b.applied = (b.cfg.bitrate, b.cfg.normalise);
    b.init_ui();
    b.startup().await;

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            if tx.send(Cmd::Tick).is_err() {
                break;
            }
        }
    });

    while let Some(cmd) = rx.recv().await {
        b.handle(cmd).await;
    }
}

fn greeting() -> &'static str {
    let hour = time::OffsetDateTime::now_local().map(|t| t.hour()).unwrap_or(12);
    match hour {
        5..=11 => "Buenos días",
        12..=19 => "Buenas tardes",
        _ => "Buenas noches",
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn spotify_id(uri: &str) -> &str {
    uri.rsplit(':').next().unwrap_or(uri)
}

impl Backend {
    fn init_ui(&self) {
        let cfg = self.cfg.clone();
        self.ui.state(move |st| {
            st.set_volume(cfg.volume);
            st.set_bitrate(cfg.bitrate as i32);
            st.set_normalise(cfg.normalise);
            st.set_client_id(cfg.client_id.as_str().into());
            st.set_greeting(greeting().into());
        });
    }

    fn screen(&self, screen: i32, status: &str) {
        let status = status.to_string();
        self.ui.state(move |st| {
            st.set_screen(screen);
            st.set_status_text(status.into());
            st.set_busy(false);
        });
    }

    fn toast(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.toast_until = Some(Instant::now() + Duration::from_millis(2800));
        self.ui.state(move |st| st.set_toast(msg.into()));
    }

    // ------------------------------------------------------------ sesión

    async fn startup(&mut self) {
        let cache = match Cache::new(Some(config::data_dir()), None, None, None) {
            Ok(c) => c,
            Err(e) => {
                self.screen(1, &format!("No se pudo crear la caché: {e}"));
                return;
            }
        };
        let creds = cache.credentials();
        let token = Token::load(&config::token_path());
        match (self.cfg.client_id.is_empty(), creds, token) {
            (false, Some(creds), Some(token)) => {
                self.screen(0, "Conectando con Spotify…");
                if let Err(e) = self.start(cache, creds, token).await {
                    log::error!("arranque: {e:#}");
                    self.screen(1, &format!("No se pudo conectar: {e}"));
                }
            }
            _ => self.screen(1, ""),
        }
    }

    async fn start(&mut self, cache: Cache, creds: Credentials, token: Token) -> Result<()> {
        let session = Session::new(SessionConfig::default(), Some(cache.clone()));
        session.connect(creds, true).await?;
        let api = Arc::new(WebApi::new(
            self.http.clone(),
            self.cfg.client_id.clone(),
            token,
            config::token_path(),
            session.clone(),
        ));
        let (playback, mut events) =
            Playback::new(session.clone(), self.cfg.bitrate, self.cfg.normalise, self.cfg.volume);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                if tx.send(Cmd::Player(ev)).is_err() {
                    break;
                }
            }
        });
        self.svc = Some(Services { session, api, playback, cache });
        self.ui.state(|st| {
            st.set_screen(2);
            st.set_busy(false);
        });
        self.push_library();
        self.spawn_library();
        self.spawn_liked();
        self.navigate(Route::Home);
        Ok(())
    }

    async fn login(&mut self, client_id: String) {
        let client_id = client_id.trim().to_string();
        self.ui.state(|st| {
            st.set_busy(true);
            st.set_status_text("Paso 1 de 2: autoriza tu app en el navegador…".into());
        });
        if let Err(e) = self.login_inner(client_id).await {
            log::error!("login: {e:#}");
            let msg = format!("{e}");
            self.ui.state(move |st| {
                st.set_busy(false);
                st.set_status_text(msg.into());
            });
        }
    }

    async fn login_inner(&mut self, client_id: String) -> Result<()> {
        let web = auth::authorize(&self.http, &client_id, &auth::web_scopes()).await?;
        let _ = std::fs::create_dir_all(config::data_dir());
        web.save(&config::token_path());
        self.ui.state(|st| st.set_status_text("Paso 2 de 2: autoriza la reproducción en el navegador…".into()));
        let stream = auth::authorize(&self.http, auth::STREAMING_CLIENT_ID, &auth::streaming_scopes()).await?;
        self.cfg.client_id = client_id;
        self.cfg.save();
        self.ui.state(|st| st.set_status_text("Conectando…".into()));
        let cache = Cache::new(Some(config::data_dir()), None, None, None)?;
        self.start(cache, Credentials::with_access_token(stream.access_token), web).await
    }

    fn logout(&mut self) {
        if let Some(mut svc) = self.svc.take() {
            svc.playback.stop();
            svc.session.shutdown();
        }
        let _ = std::fs::remove_file(config::token_path());
        let _ = std::fs::remove_file(config::data_dir().join("credentials.json"));
        if let Ok(mut l) = self.liked.write() {
            l.clear();
        }
        self.liked_tracks.clear();
        self.liked_done = false;
        self.lib = LibraryData::default();
        self.history.clear();
        self.hist_pos = 0;
        self.ui.state(|st| {
            st.set_np_uri("".into());
            st.set_np_title("".into());
            st.set_np_artists("".into());
            st.set_playing(false);
        });
        self.screen(1, "Sesión cerrada.");
    }

    /// Reconecta la sesión de librespot si se cayó (red, suspensión del equipo…)
    /// y vuelve a pedir la canción actual, cuya carga quedó ligada a la anterior.
    async fn ensure_session(&mut self) {
        let Some(svc) = &self.svc else { return };
        if !svc.session.is_invalid() && !self.session_suspect {
            return;
        }
        if self.reconnect().await {
            let pb = self.pb();
            if pb.current.is_some() {
                let play = pb.playing;
                pb.reload(play);
                self.push_playstate();
            }
        }
    }

    /// Crea una sesión nueva y se la pasa al reproductor y a la Web API.
    async fn reconnect(&mut self) -> bool {
        let Some(svc) = &mut self.svc else { return false };
        let Some(creds) = svc.cache.credentials() else { return false };
        log::info!("reconectando la sesión");
        let session = Session::new(SessionConfig::default(), Some(svc.cache.clone()));
        let error = match tokio::time::timeout(CONNECT_TIMEOUT, session.connect(creds, true)).await {
            Ok(Ok(())) => {
                svc.playback.set_session(session.clone());
                svc.api.set_session(session.clone());
                let old = std::mem::replace(&mut svc.session, session);
                old.shutdown();
                self.session_suspect = false;
                return true;
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => "tiempo agotado".to_string(),
        };
        log::error!("reconexión: {error}");
        self.toast("Sin conexión con Spotify. Reintentando…");
        false
    }

    /// La canción actual no arranca o dejó de avanzar: reconecta y la vuelve a
    /// pedir donde se quedó. Si sigue sin sonar, salta a la siguiente y, al final,
    /// se rinde para no quedarse en bucle.
    async fn recover(&mut self, why: &str) {
        self.recover_pending = false;
        let Some(svc) = &self.svc else { return };
        let pb = &svc.playback;
        let Some(uri) = pb.current.as_ref().map(|t| t.uri.clone()) else { return };
        if !pb.playing {
            return;
        }
        let near_end = pb.duration_ms() > 0 && pb.raw_position_ms() + 3000 >= pb.duration_ms();
        if uri != self.recover_uri {
            self.recover_uri = uri;
            self.recoveries = 0;
        }
        self.recoveries += 1;
        self.fail_streak += 1;
        log::warn!("reproducción atascada ({why}), intento {} ({} seguidos)", self.recoveries, self.fail_streak);
        if self.fail_streak > 6 {
            self.fail_streak = 0;
            self.pb().stop();
            self.push_playstate();
            self.toast("No se pudo reproducir. Revisa tu conexión.");
            return;
        }
        self.reconnect().await;
        if near_end || self.recoveries > 2 {
            // Ya había terminado (se perdió el aviso de fin) o no hay forma de que suene.
            self.pb().next(true);
            self.after_track_change();
        } else {
            self.pb().reload(true);
            self.push_playstate();
        }
    }

    /// Pide una recuperación sin encolar varias a la vez.
    fn request_recover(&mut self, why: &'static str) {
        if !self.recover_pending {
            self.recover_pending = true;
            let _ = self.tx.send(Cmd::Recover(why));
        }
    }

    // ------------------------------------------------------------ órdenes

    async fn handle(&mut self, cmd: Cmd) {
        if matches!(
            cmd,
            Cmd::Navigate(..) | Cmd::Home | Cmd::Back | Cmd::Forward | Cmd::Search(_) | Cmd::PlayCard(..)
        ) {
            self.ensure_session().await;
        }
        match cmd {
            Cmd::Login(id) => self.login(id).await,
            Cmd::Logout => self.logout(),
            Cmd::Tick => self.tick(),
            Cmd::Toast(msg) => self.toast(msg),
            Cmd::Recover(why) => self.recover(why).await,
            _ if self.svc.is_none() => {}
            Cmd::Navigate(kind, uri) => self.navigate_to(kind, uri),
            Cmd::Home => self.navigate(Route::Home),
            Cmd::GoSearch => {
                if self.route != Route::Search {
                    self.navigate(Route::Search);
                }
            }
            Cmd::Settings => self.navigate(Route::Settings),
            Cmd::Search(q) => self.search(q),
            Cmd::Back => {
                if self.hist_pos > 0 {
                    self.hist_pos -= 1;
                    self.show(self.history[self.hist_pos].clone());
                }
            }
            Cmd::Forward => {
                if self.hist_pos + 1 < self.history.len() {
                    self.hist_pos += 1;
                    self.show(self.history[self.hist_pos].clone());
                }
            }
            Cmd::PlayRow(source, idx) => self.play_row(source, idx).await,
            Cmd::PlayCard(kind, uri) => self.play_card(kind, uri).await,
            Cmd::PlayLoaded(uri, tracks) => {
                if tracks.is_empty() {
                    self.toast("No hay nada que reproducir");
                } else {
                    self.ensure_session().await;
                    self.pb().play_context(uri, tracks, None);
                    self.after_track_change();
                }
            }
            Cmd::TogglePlay => {
                if self.pb_ref().current.is_none() {
                    // Sin nada cargado: reproduce la página abierta.
                    if !self.page_tracks.is_empty() {
                        self.play_row(0, -1).await;
                    }
                } else {
                    self.ensure_session().await;
                    self.pb().toggle();
                    self.push_playstate();
                }
            }
            Cmd::SetPlaying(play) => {
                if self.pb_ref().current.is_some() && self.pb_ref().playing != play {
                    self.ensure_session().await;
                    self.pb().toggle();
                    self.push_playstate();
                }
            }
            Cmd::Next => {
                self.ensure_session().await;
                self.pb().next(false);
                self.after_track_change();
            }
            Cmd::Prev => {
                self.ensure_session().await;
                self.pb().prev();
                self.after_track_change();
            }
            Cmd::Seek(ms) => {
                self.pb().seek(ms);
                self.push_playstate();
            }
            Cmd::Volume(v) => {
                self.pb_ref().set_volume(v);
                self.cfg.volume = v;
                self.cfg_dirty = true;
            }
            Cmd::Shuffle => {
                self.pb().toggle_shuffle();
                self.push_playstate();
                self.push_queue();
            }
            Cmd::Repeat => {
                self.pb().cycle_repeat();
                self.push_playstate();
            }
            Cmd::Enqueue(source, idx) => {
                if let Some(t) = self.track_at(source, idx) {
                    self.pb().enqueue(t);
                    self.toast("Añadida a la cola");
                    self.push_queue();
                }
            }
            Cmd::ToggleLike(uri) => self.toggle_like(uri),
            Cmd::CreatePlaylist => self.create_playlist(),
            Cmd::AddToPlaylist(source, idx, playlist) => self.add_to_playlist(source, idx, playlist),
            Cmd::LibraryFilter(f) => {
                self.filter = f;
                self.push_library();
            }
            Cmd::RightPanel(p) => {
                self.right_panel = p;
                self.ui.state(move |st| st.set_right_panel(p));
                self.push_queue();
                self.load_lyrics_if_needed();
            }
            Cmd::Bitrate(b) => {
                self.cfg.bitrate = b;
                self.cfg.save();
                self.ui.state(move |st| st.set_bitrate(b as i32));
                self.push_restart_needed();
            }
            Cmd::Normalise(n) => {
                self.cfg.normalise = n;
                self.cfg.save();
                self.ui.state(move |st| st.set_normalise(n));
                self.push_restart_needed();
            }
            Cmd::Restart => self.restart(),
            Cmd::Player(ev) => self.player_event(ev),
            Cmd::Page(gen_id, page) => self.page_loaded(gen_id, page),
            Cmd::Library(data) => self.library_loaded(data),
            Cmd::LikedChunk(chunk, done) => self.liked_chunk(chunk, done),
            Cmd::Lyrics(uri, result) => self.lyrics_loaded(uri, result),
            Cmd::PlaylistCreated(card) => {
                self.known.insert(card.uri.clone(), card.clone());
                let uri = card.uri.clone();
                self.lib.playlists.insert(0, card);
                self.push_library();
                self.navigate(Route::Playlist(uri));
                self.toast("Playlist creada");
            }
        }
    }

    fn push_restart_needed(&self) {
        let needed = (self.cfg.bitrate, self.cfg.normalise) != self.applied;
        self.ui.state(move |st| st.set_restart_needed(needed));
    }

    /// Abre una instancia nueva y cierra esta. La nueva espera a que esta suelte
    /// la instancia única (ver `single::acquire`).
    fn restart(&mut self) {
        self.cfg.save();
        self.cfg_dirty = false;
        if let Some(svc) = &mut self.svc {
            svc.playback.stop();
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                log::error!("reinicio: {e}");
                self.toast("No se pudo reiniciar. Cierra y abre Swiftbeat.");
                return;
            }
        };
        if let Err(e) = std::process::Command::new(exe).arg(crate::single::RESTART_ARG).spawn() {
            log::error!("reinicio: {e}");
            self.toast("No se pudo reiniciar. Cierra y abre Swiftbeat.");
            return;
        }
        log::info!("reiniciando");
        self.ui.run(|_| {
            let _ = slint::quit_event_loop();
        });
    }

    fn pb(&mut self) -> &mut Playback {
        &mut self.svc.as_mut().expect("servicios activos").playback
    }

    fn pb_ref(&self) -> &Playback {
        &self.svc.as_ref().expect("servicios activos").playback
    }

    fn api(&self) -> Arc<WebApi> {
        self.svc.as_ref().expect("servicios activos").api.clone()
    }

    fn session(&self) -> Session {
        self.svc.as_ref().expect("servicios activos").session.clone()
    }

    fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if let Some(until) = self.toast_until
            && Instant::now() >= until
        {
            self.toast_until = None;
            self.ui.state(|st| st.set_toast("".into()));
        }
        if self.cfg_dirty && self.ticks.is_multiple_of(4) {
            self.cfg.save();
            self.cfg_dirty = false;
        }
        let now = SystemTime::now();
        if now.duration_since(self.last_wall).is_ok_and(|gap| gap > SLEEP_GAP) {
            log::info!("el equipo estuvo suspendido: se reconectará la sesión");
            self.session_suspect = true;
        }
        self.last_wall = now;
        let Some(svc) = &self.svc else { return };
        if !svc.playback.playing {
            return;
        }
        let pb = &svc.playback;
        if pb.current.is_some() {
            if !pb.started && pb.load_at.elapsed() > LOAD_TIMEOUT {
                self.request_recover("la canción no empieza");
                return;
            }
            if pb.started && pb.progress_at.elapsed() > STALL_TIMEOUT {
                self.request_recover("el audio no avanza");
                return;
            }
        }
        let Some(svc) = &self.svc else { return };
        let pos = svc.playback.position_ms();
        let mut active = self.lyrics_active;
        if self.right_panel == 2 && self.lyrics.first().is_some_and(|l| l.0 >= 0) {
            active = self
                .lyrics
                .iter()
                .rposition(|(start, _)| *start >= 0 && *start as u32 <= pos)
                .map(|i| i as i32)
                .unwrap_or(-1);
        }
        let lyric_changed = active != self.lyrics_active;
        self.lyrics_active = active;
        self.ui.state(move |st| {
            st.set_np_position(pos as i32);
            if lyric_changed {
                st.set_lyrics_active(active);
            }
        });
    }

    // ------------------------------------------------------------ navegación

    fn navigate_to(&mut self, kind: i32, uri: String) {
        let route = match kind {
            KIND_ALBUM => Route::Album(uri),
            KIND_ARTIST => Route::Artist(uri),
            KIND_PLAYLIST => Route::Playlist(uri),
            KIND_LIKED => Route::Liked,
            KIND_TRACK => match self.search_tracks.iter().find(|t| t.uri == uri) {
                Some(t) if !t.album_uri.is_empty() => Route::Album(t.album_uri.clone()),
                _ => return,
            },
            _ => return,
        };
        self.navigate(route);
    }

    fn navigate(&mut self, route: Route) {
        if self.history.get(self.hist_pos) != Some(&route) || self.history.is_empty() {
            if !self.history.is_empty() {
                self.history.truncate(self.hist_pos + 1);
            }
            self.history.push(route.clone());
            if self.history.len() > 50 {
                self.history.remove(0);
            }
            self.hist_pos = self.history.len() - 1;
        }
        self.show(route);
    }

    fn show(&mut self, route: Route) {
        self.nav_gen += 1;
        let gen_id = self.nav_gen;
        self.route = route.clone();
        let can_back = self.hist_pos > 0;
        let can_forward = self.hist_pos + 1 < self.history.len();
        let page = match route {
            Route::Home => 0,
            Route::Search => 1,
            Route::Settings => 4,
            Route::Artist(_) => 3,
            _ => 2,
        };
        let loading = !matches!(route, Route::Search | Route::Settings);
        self.ui.state(move |st| {
            st.set_page(page);
            st.set_can_back(can_back);
            st.set_can_forward(can_forward);
            st.set_loading(loading);
            if page != 2 {
                st.set_coll_uri("".into());
            }
            if page != 3 {
                st.set_artist_uri("".into());
            }
        });

        let tx = self.tx.clone();
        match route {
            Route::Home => {
                let api = self.api();
                let known = self.known.clone();
                let liked_count = self.liked_tracks.len();
                tokio::spawn(async move {
                    let page = load_home(&api, &known, liked_count).await;
                    let _ = tx.send(Cmd::Page(gen_id, page));
                });
            }
            Route::Search | Route::Settings => {}
            Route::Liked => {
                self.page_uri = LIKED_URI.into();
                self.page_tracks = self.liked_tracks.clone();
                let header = self.liked_header();
                let rows = ui::track_rows(&self.page_tracks, 0);
                let done = self.liked_done;
                self.ui.run(move |app| {
                    set_header(app, LIKED_URI, &header, true);
                    ui::set_collection_rows(app, rows);
                    app.global::<crate::State>().set_loading(!done);
                });
            }
            Route::Album(uri) | Route::Playlist(uri) => {
                self.page_uri = uri.clone();
                self.page_tracks.clear();
                let hint = self.known.get(&uri).cloned().unwrap_or_default();
                let is_album = matches!(self.route, Route::Album(_));
                let header = Header {
                    label: if is_album { "Álbum".into() } else { "Playlist".into() },
                    title: hint.title.clone(),
                    subtitle: String::new(),
                    description: String::new(),
                    image: hint.image.clone(),
                };
                let u = uri.clone();
                self.ui.run(move |app| {
                    set_header(app, &u, &header, false);
                    ui::set_collection_rows(app, Vec::new());
                });
                let session = self.session();
                let api = self.api();
                tokio::spawn(async move {
                    if is_album {
                        let page = match meta::album_page(&session, &uri).await {
                            Ok(a) => Page::Collection {
                                header: Some(Header {
                                    label: a.label,
                                    title: a.title,
                                    subtitle: a.subtitle,
                                    description: String::new(),
                                    image: a.image,
                                }),
                                tracks: a.tracks,
                                reset: true,
                                done: true,
                            },
                            Err(e) => Page::Failed(format!("No se pudo abrir el álbum: {e}")),
                        };
                        let _ = tx.send(Cmd::Page(gen_id, page));
                    } else {
                        load_playlist(&session, &api, &uri, hint, gen_id, &tx).await;
                    }
                });
            }
            Route::Artist(uri) => {
                self.artist_uri = uri.clone();
                self.artist_top.clear();
                let hint = self.known.get(&uri).cloned().unwrap_or_default();
                let u = uri.clone();
                self.ui.state(move |st| {
                    st.set_artist_uri(u.into());
                    st.set_artist_name(hint.title.into());
                    st.set_artist_image(hint.image.into());
                    st.set_artist_top(ui::model(Vec::new()));
                    st.set_artist_shelves(ui::model(Vec::new()));
                });
                let session = self.session();
                tokio::spawn(async move {
                    let page = match meta::artist_page(&session, &uri).await {
                        Ok(a) => Page::Artist { name: a.name, image: a.image, top: a.top, shelves: a.shelves },
                        Err(e) => Page::Failed(format!("No se pudo abrir el artista: {e}")),
                    };
                    let _ = tx.send(Cmd::Page(gen_id, page));
                });
            }
        }
    }

    fn liked_header(&self) -> Header {
        let total: u64 = self.liked_tracks.iter().map(|t| t.duration_ms as u64).sum();
        Header {
            label: "Playlist".into(),
            title: "Tus me gusta".into(),
            subtitle: format!(
                "{} · {} canciones, {}",
                self.lib.me_name,
                self.liked_tracks.len(),
                meta::fmt_total(total)
            ),
            description: String::new(),
            image: String::new(),
        }
    }

    fn remember(&mut self, cards: &[CardInfo]) {
        for c in cards {
            self.known.entry(c.uri.clone()).or_insert_with(|| c.clone());
        }
    }

    fn page_loaded(&mut self, gen_id: u64, page: Page) {
        if let Page::Search { gen_id: g, top, tracks, shelves } = page {
            if g != self.search_gen.load(Ordering::SeqCst) {
                return;
            }
            for (_, cards) in &shelves {
                self.remember(cards);
            }
            if let Some(t) = &top {
                self.remember(std::slice::from_ref(t));
            }
            let rows = ui::track_rows(&tracks[..tracks.len().min(4)], 0);
            self.search_tracks = tracks;
            let shelves: Vec<_> = shelves.into_iter().map(|(t, c)| (t, ui::cards(&c))).collect();
            let top = top.map(|t| ui::card(&t));
            self.ui.run(move |app| {
                let st = app.global::<crate::State>();
                st.set_has_top(top.is_some());
                st.set_search_top(top.unwrap_or_default());
                st.set_search_tracks(ui::model(rows));
                st.set_search_shelves(ui::shelves(shelves));
                st.set_loading(false);
            });
            return;
        }
        if gen_id != self.nav_gen {
            return;
        }
        match page {
            Page::Search { .. } => {}
            Page::Failed(msg) => {
                log::warn!("{msg}");
                self.ui.state(|st| st.set_loading(false));
                self.toast(msg);
            }
            Page::Home { quick, shelves } => {
                self.remember(&quick);
                for (_, cards) in &shelves {
                    self.remember(cards);
                }
                let quick = ui::cards(&quick);
                let shelves: Vec<_> = shelves.into_iter().map(|(t, c)| (t, ui::cards(&c))).collect();
                self.ui.run(move |app| {
                    let st = app.global::<crate::State>();
                    st.set_greeting(greeting().into());
                    st.set_quick(ui::model(quick));
                    st.set_shelves(ui::shelves(shelves));
                    st.set_loading(false);
                });
            }
            Page::Collection { header, tracks, reset, done } => {
                let offset = if reset { 0 } else { self.page_tracks.len() };
                let rows = ui::track_rows(&tracks, offset);
                if reset {
                    self.page_tracks = tracks;
                } else {
                    self.page_tracks.extend(tracks);
                }
                let uri = self.page_uri.clone();
                self.ui.run(move |app| {
                    if let Some(h) = header {
                        set_header(app, &uri, &h, false);
                    }
                    if reset {
                        ui::set_collection_rows(app, rows);
                    } else if !rows.is_empty() {
                        ui::append_collection_rows(rows);
                    }
                    if done {
                        app.global::<crate::State>().set_loading(false);
                    }
                });
            }
            Page::Artist { name, image, top, shelves } => {
                for (_, cards) in &shelves {
                    self.remember(cards);
                }
                let rows = ui::track_rows(&top, 0);
                self.artist_top = top;
                let hint_image = self.known.get(&self.artist_uri).map(|c| c.image.clone()).unwrap_or_default();
                let image = if image.is_empty() { hint_image } else { image };
                let shelves: Vec<_> = shelves.into_iter().map(|(t, c)| (t, ui::cards(&c))).collect();
                self.ui.run(move |app| {
                    let st = app.global::<crate::State>();
                    st.set_artist_name(name.into());
                    st.set_artist_image(image.into());
                    st.set_artist_top(ui::model(rows));
                    st.set_artist_shelves(ui::shelves(shelves));
                    st.set_loading(false);
                });
            }
        }
    }

    fn search(&mut self, query: String) {
        if self.route != Route::Search {
            self.navigate(Route::Search);
        }
        let gen_id = self.search_gen.fetch_add(1, Ordering::SeqCst) + 1;
        if query.trim().is_empty() {
            self.search_tracks.clear();
            self.ui.state(|st| {
                st.set_has_top(false);
                st.set_search_tracks(ui::model(Vec::new()));
                st.set_search_shelves(ui::model(Vec::new()));
                st.set_loading(false);
            });
            return;
        }
        self.ui.state(|st| st.set_loading(true));
        let api = self.api();
        let current = self.search_gen.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            // Espera a que el usuario deje de escribir.
            tokio::time::sleep(Duration::from_millis(300)).await;
            if current.load(Ordering::SeqCst) != gen_id {
                return;
            }
            let q: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
            let page = match api.get(&format!("/search?q={q}&type=track,artist,album,playlist&limit=10")).await {
                Ok(v) => search_page(gen_id, &query, &v),
                Err(e) => {
                    log::warn!("búsqueda: {e}");
                    Page::Search { gen_id, top: None, tracks: Vec::new(), shelves: Vec::new() }
                }
            };
            let _ = tx.send(Cmd::Page(0, page));
        });
    }

    // ------------------------------------------------------------ reproducción

    fn track_at(&self, source: i32, idx: i32) -> Option<TrackInfo> {
        let idx = usize::try_from(idx).ok()?;
        match source {
            0 => self.page_tracks.get(idx).cloned(),
            1 => self.search_tracks.get(idx).cloned(),
            2 => self.artist_top.get(idx).cloned(),
            3 => self.pb_ref().upcoming(50).get(idx).cloned(),
            _ => None,
        }
    }

    async fn play_row(&mut self, source: i32, idx: i32) {
        self.ensure_session().await;
        let (uri, tracks) = match source {
            0 => (self.page_uri.clone(), self.page_tracks.clone()),
            1 => ("search".to_string(), self.search_tracks.clone()),
            2 => (self.artist_uri.clone(), self.artist_top.clone()),
            3 => {
                if let Ok(i) = usize::try_from(idx) {
                    self.pb().skip_to(i);
                    self.after_track_change();
                }
                return;
            }
            _ => return,
        };
        let start = usize::try_from(idx).ok();
        self.pb().play_context(uri, tracks, start);
        self.after_track_change();
    }

    async fn play_card(&mut self, kind: i32, uri: String) {
        if uri == self.page_uri && !self.page_tracks.is_empty() {
            self.play_row(0, -1).await;
            return;
        }
        if kind == KIND_LIKED {
            let tracks = self.liked_tracks.clone();
            let _ = self.tx.send(Cmd::PlayLoaded(LIKED_URI.into(), tracks));
            return;
        }
        let session = self.session();
        let api = self.api();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let tracks = match kind {
                KIND_ALBUM => meta::album_page(&session, &uri).await.map(|a| a.tracks).unwrap_or_default(),
                KIND_ARTIST => meta::artist_page(&session, &uri).await.map(|a| a.top).unwrap_or_default(),
                KIND_PLAYLIST => match meta::playlist_head(&session, &uri).await {
                    Ok(head) => meta::tracks(&session, &head.items[..head.items.len().min(500)]).await,
                    Err(_) => playlist_items_webapi(&api, &uri, 500).await.unwrap_or_default(),
                },
                _ => Vec::new(),
            };
            let _ = tx.send(Cmd::PlayLoaded(uri, tracks));
        });
    }

    /// Recuperaciones ya intentadas con la canción que suena.
    fn current_recoveries(&self) -> u32 {
        match self.svc.as_ref().and_then(|s| s.playback.current.as_ref()) {
            Some(t) if t.uri == self.recover_uri => self.recoveries,
            _ => 0,
        }
    }

    fn player_event(&mut self, ev: PlayerEvent) {
        let tried = self.current_recoveries();
        let Some(svc) = &mut self.svc else { return };
        let pb = &mut svc.playback;
        match ev {
            PlayerEvent::Playing { track_id, position_ms, .. } if pb.is_current(&track_id) => {
                self.unavailable_streak = 0;
                self.fail_streak = 0;
                pb.playing = true;
                pb.started = true;
                pb.progress_at = Instant::now();
                pb.set_pos(position_ms);
                self.push_playstate();
            }
            PlayerEvent::Paused { track_id, position_ms, .. } if pb.is_current(&track_id) => {
                pb.playing = false;
                pb.started = true;
                pb.set_pos(position_ms);
                self.push_playstate();
            }
            PlayerEvent::Seeked { track_id, position_ms, .. }
            | PlayerEvent::PositionCorrection { track_id, position_ms, .. }
            | PlayerEvent::PositionChanged { track_id, position_ms, .. }
                if pb.is_current(&track_id) =>
            {
                pb.progress_at = Instant::now();
                pb.set_pos(position_ms);
            }
            PlayerEvent::TimeToPreloadNextTrack { .. } => pb.preload_next(),
            PlayerEvent::EndOfTrack { track_id, .. } if pb.is_current(&track_id) => {
                // librespot también avisa de «fin» cuando la descarga falla a mitad
                // de canción. En ese caso se recupera donde iba en vez de saltarla.
                let early = pb.started && pb.duration_ms() > 0 && pb.position_ms() + 5000 < pb.duration_ms();
                if early && tried < 2 {
                    log::warn!("fin prematuro en {} de {} ms", pb.position_ms(), pb.duration_ms());
                    self.request_recover("la canción se cortó");
                    return;
                }
                pb.next(true);
                self.after_track_change();
            }
            PlayerEvent::Unavailable { track_id, .. } if pb.is_current(&track_id) => {
                self.unavailable_streak += 1;
                if self.unavailable_streak > 8 {
                    pb.stop();
                    self.push_playstate();
                    self.toast("No se pudo reproducir. Revisa tu conexión o tu cuenta Premium.");
                    return;
                }
                if self.unavailable_streak == 2 {
                    // Dos seguidas no suele ser casualidad: probablemente la sesión murió.
                    self.request_recover("canciones no disponibles");
                    return;
                }
                pb.next(true);
                self.after_track_change();
                self.toast("Canción no disponible, se ha saltado");
            }
            _ => {}
        }
    }

    fn push_playstate(&self) {
        let Some(svc) = &self.svc else { return };
        let pb = &svc.playback;
        let playing = pb.playing;
        let buffering = pb.buffering();
        let pos = pb.position_ms() as i32;
        let shuffle = pb.shuffle;
        let repeat = match pb.repeat {
            Repeat::Off => 0,
            Repeat::Context => 1,
            Repeat::Track => 2,
        };
        let ctx = pb.context_uri.clone();
        self.ui.state(move |st| {
            st.set_playing(playing);
            st.set_buffering(buffering);
            st.set_np_position(pos);
            st.set_shuffle(shuffle);
            st.set_repeat(repeat);
            st.set_ctx_uri(ctx.into());
            crate::media::set_playing(playing, pos.max(0) as u32);
        });
    }

    fn after_track_change(&mut self) {
        let Some(svc) = &self.svc else { return };
        let t = svc.playback.current.clone().unwrap_or_default();
        self.ui.state(move |st| {
            st.set_np_uri(t.uri.as_str().into());
            st.set_np_title(t.name.as_str().into());
            st.set_np_artists(t.artist_names().into());
            st.set_np_artist_uri(t.first_artist_uri().into());
            st.set_np_album_uri(t.album_uri.as_str().into());
            st.set_np_image(t.image_small.as_str().into());
            st.set_np_duration(t.duration_ms as i32);
            if !t.uri.is_empty() {
                crate::media::set_track(&t.name, &t.artist_names(), &t.album, &t.image_large, t.duration_ms);
            }
        });
        self.push_playstate();
        self.push_queue();
        self.load_lyrics_if_needed();
    }

    fn push_queue(&self) {
        if self.right_panel != 1 {
            return;
        }
        let Some(svc) = &self.svc else { return };
        let rows = ui::track_rows(&svc.playback.upcoming(50), 0);
        self.ui.state(move |st| st.set_queue(ui::model(rows)));
    }

    fn load_lyrics_if_needed(&mut self) {
        if self.right_panel != 2 {
            return;
        }
        let Some(svc) = &self.svc else { return };
        let Some(uri) = svc.playback.current.as_ref().map(|t| t.uri.clone()) else {
            self.ui.state(|st| {
                st.set_lyrics(ui::model(Vec::new()));
                st.set_lyrics_status("Reproduce algo para ver la letra.".into());
            });
            return;
        };
        if uri == self.lyrics_uri {
            return;
        }
        self.lyrics_uri = uri.clone();
        self.lyrics.clear();
        self.lyrics_active = -1;
        self.ui.state(|st| {
            st.set_lyrics(ui::model(Vec::new()));
            st.set_lyrics_active(-1);
            st.set_lyrics_status("Cargando letra…".into());
        });
        let session = svc.session.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = meta::lyrics(&session, &uri).await.map_err(|e| e.to_string());
            let _ = tx.send(Cmd::Lyrics(uri, result));
        });
    }

    fn lyrics_loaded(&mut self, uri: String, result: Result<Vec<(i32, String)>, String>) {
        if uri != self.lyrics_uri {
            return;
        }
        match result {
            Ok(lines) if !lines.is_empty() => {
                let rows: Vec<LyricLine> = lines
                    .iter()
                    .map(|(start, text)| LyricLine { start: *start, text: text.as_str().into() })
                    .collect();
                self.lyrics = lines;
                self.ui.state(move |st| st.set_lyrics(ui::model(rows)));
            }
            other => {
                if let Err(e) = other {
                    log::info!("letra: {e}");
                }
                self.ui.state(|st| st.set_lyrics_status("No hay letra disponible para esta canción.".into()));
            }
        }
    }

    // ------------------------------------------------------------ biblioteca

    fn spawn_library(&self) {
        let api = self.api();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut lib = LibraryData::default();
            match api.get("/me").await {
                Ok(me) => {
                    lib.me_id = s(&me["id"]);
                    lib.me_name = me["display_name"].as_str().map(String::from).unwrap_or_else(|| s(&me["id"]));
                    lib.me_image = meta::pick_image(&me["images"], 64);
                }
                Err(e) => log::warn!("/me: {e}"),
            }
            let r = api
                .paged("/me/playlists?limit=50", 2000, |items| {
                    lib.playlists.extend(items.iter().filter_map(meta::playlist_card))
                })
                .await;
            if let Err(e) = r {
                log::warn!("playlists: {e}");
            }
            let r = api
                .paged("/me/albums?limit=50", 2000, |items| {
                    lib.albums.extend(items.iter().filter_map(|i| meta::album_card(&i["album"])))
                })
                .await;
            if let Err(e) = r {
                log::warn!("álbumes: {e}");
            }
            let r = api
                .paged("/me/following?type=artist&limit=50", 2000, |items| {
                    lib.artists.extend(items.iter().filter_map(meta::artist_card))
                })
                .await;
            if let Err(e) = r {
                log::warn!("artistas: {e}");
            }
            let _ = tx.send(Cmd::Library(lib));
        });
    }

    fn spawn_liked(&self) {
        let api = self.api();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = api
                .paged("/me/tracks?limit=50", 20_000, |items| {
                    let chunk: Vec<TrackInfo> = items.iter().filter_map(|i| meta::track_from_json(&i["track"])).collect();
                    let _ = tx.send(Cmd::LikedChunk(chunk, false));
                })
                .await;
            if let Err(e) = r {
                log::warn!("me gusta: {e}");
            }
            let _ = tx.send(Cmd::LikedChunk(Vec::new(), true));
        });
    }

    fn library_loaded(&mut self, data: LibraryData) {
        self.lib = data;
        let all: Vec<CardInfo> = self
            .lib
            .playlists
            .iter()
            .chain(&self.lib.albums)
            .chain(&self.lib.artists)
            .cloned()
            .collect();
        for c in all {
            self.known.insert(c.uri.clone(), c);
        }
        let name = self.lib.me_name.clone();
        let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let image = self.lib.me_image.clone();
        self.ui.state(move |st| {
            st.set_user_name(name.into());
            st.set_user_initial(initial.into());
            st.set_user_image(image.into());
        });
        self.push_library();
        if self.route == Route::Home {
            // Recarga el inicio para usar los nombres de las playlists ya conocidas.
            self.show(Route::Home);
        }
    }

    fn push_library(&self) {
        let f = self.filter;
        let mut items: Vec<CardInfo> = Vec::new();
        if f == -1 || f == KIND_PLAYLIST {
            items.push(meta::liked_card(self.liked_tracks.len()));
            items.extend(self.lib.playlists.iter().cloned());
        }
        if f == -1 || f == KIND_ALBUM {
            items.extend(self.lib.albums.iter().cloned());
        }
        if f == -1 || f == KIND_ARTIST {
            items.extend(self.lib.artists.iter().cloned());
        }
        let me = self.lib.me_id.clone();
        let own: Vec<CardInfo> = self
            .lib
            .playlists
            .iter()
            .filter(|p| p.owner_id == me)
            .cloned()
            .collect();
        let items = ui::cards(&items);
        let own = ui::cards(&own);
        self.ui.state(move |st| {
            st.set_library_filter(f);
            st.set_library(ui::model(items));
            st.set_my_playlists(ui::model(own));
        });
    }

    fn liked_chunk(&mut self, chunk: Vec<TrackInfo>, done: bool) {
        if let Ok(mut set) = self.liked.write() {
            set.extend(chunk.iter().map(|t| t.uri.clone()));
        }
        let offset = self.liked_tracks.len();
        self.liked_tracks.extend(chunk.iter().cloned());
        self.ui.state(|st| st.set_liked_epoch(st.get_liked_epoch() + 1));
        if self.route == Route::Liked {
            let rows = ui::track_rows(&chunk, offset);
            self.page_tracks.extend(chunk);
            let header = self.liked_header();
            self.ui.run(move |app| {
                set_header(app, LIKED_URI, &header, true);
                if !rows.is_empty() {
                    ui::append_collection_rows(rows);
                }
                if done {
                    app.global::<crate::State>().set_loading(false);
                }
            });
        }
        if done {
            self.liked_done = true;
            self.push_library();
        }
    }

    fn find_track(&self, uri: &str) -> Option<TrackInfo> {
        let svc = self.svc.as_ref()?;
        svc.playback
            .current
            .iter()
            .chain(&self.page_tracks)
            .chain(&self.search_tracks)
            .chain(&self.artist_top)
            .find(|t| t.uri == uri)
            .cloned()
    }

    fn toggle_like(&mut self, uri: String) {
        let was_liked = self.liked.read().map(|l| l.contains(&uri)).unwrap_or(false);
        if let Ok(mut set) = self.liked.write() {
            if was_liked {
                set.remove(&uri);
            } else {
                set.insert(uri.clone());
            }
        }
        if was_liked {
            self.liked_tracks.retain(|t| t.uri != uri);
        } else if let Some(t) = self.find_track(&uri) {
            self.liked_tracks.insert(0, t);
        }
        self.ui.state(|st| st.set_liked_epoch(st.get_liked_epoch() + 1));
        if self.route == Route::Liked {
            self.show(Route::Liked);
        }
        self.toast(if was_liked { "Eliminada de Tus me gusta" } else { "Guardada en Tus me gusta" });

        let api = self.api();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let method = if was_liked { Method::DELETE } else { Method::PUT };
            if let Err(e) = api.request(method, &format!("/me/library?uris={uri}"), None).await {
                log::warn!("me gusta: {e}");
                let _ = tx.send(Cmd::Toast("No se pudo actualizar Tus me gusta".into()));
            }
        });
    }

    fn create_playlist(&mut self) {
        let n = self.lib.playlists.iter().filter(|p| p.owner_id == self.lib.me_id).count() + 1;
        let api = self.api();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let body = json!({ "name": format!("Mi playlist n.º {n}"), "public": false });
            match api.request(Method::POST, "/me/playlists", Some(&body)).await {
                Ok(v) => match meta::playlist_card(&v) {
                    Some(card) => {
                        let _ = tx.send(Cmd::PlaylistCreated(card));
                    }
                    None => {
                        let _ = tx.send(Cmd::Toast("Respuesta inesperada al crear la playlist".into()));
                    }
                },
                Err(e) => {
                    log::warn!("crear playlist: {e}");
                    let _ = tx.send(Cmd::Toast("No se pudo crear la playlist".into()));
                }
            }
        });
    }

    fn add_to_playlist(&mut self, source: i32, idx: i32, playlist: String) {
        let Some(track) = self.track_at(source, idx) else { return };
        let name = self.known.get(&playlist).map(|c| c.title.clone()).unwrap_or_else(|| "la playlist".into());
        let api = self.api();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let body = json!({ "uris": [track.uri] });
            let path = format!("/playlists/{}/items", spotify_id(&playlist));
            let msg = match api.request(Method::POST, &path, Some(&body)).await {
                Ok(_) => format!("Añadida a {name}"),
                Err(e) => {
                    log::warn!("añadir a playlist: {e}");
                    "No se pudo añadir a la playlist".into()
                }
            };
            let _ = tx.send(Cmd::Toast(msg));
        });
    }
}

fn set_header(app: &AppWindow, uri: &str, h: &Header, liked: bool) {
    let st = app.global::<crate::State>();
    st.set_coll_uri(uri.into());
    st.set_coll_type(h.label.as_str().into());
    st.set_coll_title(h.title.as_str().into());
    st.set_coll_subtitle(h.subtitle.as_str().into());
    st.set_coll_description(h.description.as_str().into());
    st.set_coll_image(h.image.as_str().into());
    st.set_coll_liked(liked);
}

// ---------------------------------------------------------------- cargadores

async fn load_home(api: &WebApi, known: &HashMap<String, CardInfo>, liked_count: usize) -> Page {
    let (recent, top_artists, albums, playlists, top_tracks) = tokio::join!(
        api.get("/me/player/recently-played?limit=50"),
        api.get("/me/top/artists?limit=12&time_range=short_term"),
        api.get("/me/albums?limit=12"),
        api.get("/me/playlists?limit=12"),
        api.get("/me/top/tracks?limit=20&time_range=medium_term"),
    );
    let items = |r: &Result<Value>| -> Vec<Value> {
        match r {
            Ok(v) => v["items"].as_array().cloned().unwrap_or_default(),
            Err(e) => {
                log::info!("inicio: {e}");
                Vec::new()
            }
        }
    };

    let mut seen = HashSet::new();
    let mut recent_cards = Vec::new();
    for item in items(&recent) {
        let ctx_uri = item["context"]["uri"].as_str().unwrap_or("");
        let card = known
            .get(ctx_uri)
            .cloned()
            .or_else(|| meta::album_card(&item["track"]["album"]));
        if let Some(c) = card
            && seen.insert(c.uri.clone())
        {
            recent_cards.push(c);
        }
        if recent_cards.len() >= 12 {
            break;
        }
    }
    let playlist_cards: Vec<CardInfo> = items(&playlists).iter().filter_map(meta::playlist_card).collect();
    let artist_cards: Vec<CardInfo> = items(&top_artists).iter().filter_map(meta::artist_card).collect();
    let album_cards: Vec<CardInfo> = items(&albums).iter().filter_map(|i| meta::album_card(&i["album"])).collect();
    let mut seen_albums = HashSet::new();
    let fav_albums: Vec<CardInfo> = items(&top_tracks)
        .iter()
        .filter_map(|t| meta::album_card(&t["album"]))
        .filter(|c| seen_albums.insert(c.uri.clone()))
        .take(12)
        .collect();

    let mut quick = vec![meta::liked_card(liked_count)];
    for c in recent_cards.iter().chain(&playlist_cards) {
        if quick.len() >= 8 {
            break;
        }
        if !quick.iter().any(|q| q.uri == c.uri) {
            quick.push(c.clone());
        }
    }

    let mut shelves = Vec::new();
    for (title, cards) in [
        ("Escuchado recientemente", recent_cards),
        ("Tus playlists", playlist_cards),
        ("Tus artistas favoritos", artist_cards),
        ("De tus canciones más escuchadas", fav_albums),
        ("Tus álbumes", album_cards),
    ] {
        if !cards.is_empty() {
            shelves.push((title.to_string(), cards));
        }
    }
    Page::Home { quick, shelves }
}

fn search_page(gen_id: u64, query: &str, v: &Value) -> Page {
    let list = |key: &str| v[key]["items"].as_array().cloned().unwrap_or_default();
    let tracks: Vec<TrackInfo> = list("tracks").iter().filter_map(meta::track_from_json).collect();
    let artists: Vec<CardInfo> = list("artists").iter().filter_map(meta::artist_card).collect();
    let albums: Vec<CardInfo> = list("albums").iter().filter_map(meta::album_card).collect();
    let playlists: Vec<CardInfo> = list("playlists").iter().filter_map(meta::playlist_card).collect();

    let q = query.trim().to_lowercase();
    let top = match artists.first() {
        Some(a) if a.title.to_lowercase().starts_with(&q) || tracks.is_empty() => Some(a.clone()),
        _ => tracks.first().map(|t| CardInfo {
            uri: t.uri.clone(),
            title: t.name.clone(),
            subtitle: format!("Canción · {}", t.artist_names()),
            image: t.image_large.clone(),
            kind: KIND_TRACK,
            ..Default::default()
        }),
    };
    let mut shelves = Vec::new();
    for (title, cards) in [("Artistas", artists), ("Álbumes", albums), ("Playlists", playlists)] {
        if !cards.is_empty() {
            shelves.push((title.to_string(), cards));
        }
    }
    Page::Search { gen_id, top, tracks, shelves }
}

/// Playlist por la sesión interna; si falla, por la Web API (solo playlists propias).
async fn load_playlist(
    session: &Session,
    api: &WebApi,
    uri: &str,
    hint: CardInfo,
    gen_id: u64,
    tx: &UnboundedSender<Cmd>,
) {
    let owner = hint.subtitle.split(" · ").nth(1).unwrap_or("").to_string();
    let send = |page: Page| {
        let _ = tx.send(Cmd::Page(gen_id, page));
    };
    let header = |title: &str, description: &str, image: &str, count: usize, total_ms: u64| Header {
        label: "Playlist".into(),
        title: if title.is_empty() { hint.title.clone() } else { title.to_string() },
        subtitle: {
            let mut parts = Vec::new();
            if !owner.is_empty() {
                parts.push(owner.clone());
            }
            parts.push(if total_ms > 0 {
                format!("{count} canciones, {}", meta::fmt_total(total_ms))
            } else {
                format!("{count} canciones")
            });
            parts.join(" · ")
        },
        description: description.to_string(),
        image: if hint.image.is_empty() { image.to_string() } else { hint.image.clone() },
    };

    match meta::playlist_head(session, uri).await {
        Ok(head) => {
            let count = head.items.len();
            send(Page::Collection {
                header: Some(header(&head.title, &head.description, &head.image, count, 0)),
                tracks: Vec::new(),
                reset: true,
                done: count == 0,
            });
            let mut total = 0u64;
            let mut first_image = String::new();
            for chunk in head.items.chunks(100) {
                let tracks = meta::tracks(session, chunk).await;
                total += tracks.iter().map(|t| t.duration_ms as u64).sum::<u64>();
                if first_image.is_empty() {
                    first_image = tracks.first().map(|t| t.image_large.clone()).unwrap_or_default();
                }
                send(Page::Collection { header: None, tracks, reset: false, done: false });
            }
            let image = if head.image.is_empty() { first_image } else { head.image.clone() };
            send(Page::Collection {
                header: Some(header(&head.title, &head.description, &image, count, total)),
                tracks: Vec::new(),
                reset: false,
                done: true,
            });
        }
        Err(e) => {
            log::info!("playlist por sesión falló ({e}); uso la Web API");
            match playlist_items_webapi(api, uri, 10_000).await {
                Ok(tracks) => {
                    let total = tracks.iter().map(|t| t.duration_ms as u64).sum();
                    let image = tracks.first().map(|t| t.image_large.clone()).unwrap_or_default();
                    send(Page::Collection {
                        header: Some(header("", "", &image, tracks.len(), total)),
                        tracks,
                        reset: true,
                        done: true,
                    });
                }
                Err(e) => send(Page::Failed(format!("No se pudo abrir la playlist: {e}"))),
            }
        }
    }
}

async fn playlist_items_webapi(api: &WebApi, uri: &str, max: usize) -> Result<Vec<TrackInfo>> {
    let mut tracks = Vec::new();
    api.paged(&format!("/playlists/{}/items?limit=100", spotify_id(uri)), max, |items| {
        tracks.extend(items.iter().filter_map(|i| {
            let t = if i["item"].is_object() { &i["item"] } else { &i["track"] };
            meta::track_from_json(t)
        }))
    })
    .await
    .map_err(|e| anyhow!("{e}"))?;
    Ok(tracks)
}
