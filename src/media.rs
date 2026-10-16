//! Integración con los controles multimedia de Windows (teclas de reproducción
//! del teclado y panel de medios del sistema con la portada).

use crate::{AppWindow, backend::Cmd};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::{cell::RefCell, time::Duration};
use tokio::sync::mpsc::UnboundedSender;

thread_local! {
    static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
}

/// Registra los controles en cuanto exista la ventana nativa. Con winit la
/// ventana se crea al arrancar el bucle de eventos, así que se reintenta.
pub fn install_when_ready(app: slint::Weak<AppWindow>, tx: UnboundedSender<Cmd>) {
    slint::Timer::single_shot(Duration::ZERO, move || attempt(app, tx, 0));
}

fn attempt(app: slint::Weak<AppWindow>, tx: UnboundedSender<Cmd>, tries: u32) {
    let Some(ui) = app.upgrade() else { return };
    match install(&ui, tx.clone()) {
        Ok(()) => log::info!("controles multimedia de Windows activos"),
        Err(e) if tries < 50 => {
            log::debug!("controles multimedia aún no disponibles: {e}");
            slint::Timer::single_shot(Duration::from_millis(100), move || attempt(app, tx, tries + 1));
        }
        Err(e) => log::warn!("controles multimedia: {e}"),
    }
}

fn install(app: &AppWindow, tx: UnboundedSender<Cmd>) -> Result<(), String> {
    let window = app.window().window_handle();
    let handle = window.window_handle().map_err(|e| format!("sin ventana nativa: {e}"))?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return Err("la ventana no es Win32".into());
    };
    let config = PlatformConfig {
        display_name: "Swiftbeat",
        dbus_name: "swiftbeat",
        hwnd: Some(win32.hwnd.get() as *mut std::ffi::c_void),
    };
    let mut controls = MediaControls::new(config).map_err(|e| format!("{e:?}"))?;
    controls
        .attach(move |event| {
            log::info!("tecla multimedia: {event:?}");
            let cmd = match event {
                MediaControlEvent::Play => Cmd::SetPlaying(true),
                MediaControlEvent::Pause | MediaControlEvent::Stop => Cmd::SetPlaying(false),
                MediaControlEvent::Toggle => Cmd::TogglePlay,
                MediaControlEvent::Next => Cmd::Next,
                MediaControlEvent::Previous => Cmd::Prev,
                MediaControlEvent::SetPosition(MediaPosition(d)) => Cmd::Seek(d.as_millis() as u32),
                _ => return,
            };
            let _ = tx.send(cmd);
        })
        .map_err(|e| format!("{e:?}"))?;
    // Sin un estado inicial Windows no muestra la sesión ni le envía las teclas.
    let _ = controls.set_playback(MediaPlayback::Stopped);
    CONTROLS.set(Some(controls));
    Ok(())
}

pub fn set_track(title: &str, artist: &str, album: &str, cover: &str, duration_ms: u32) {
    CONTROLS.with_borrow_mut(|c| {
        if let Some(c) = c {
            let _ = c.set_metadata(MediaMetadata {
                title: Some(title),
                artist: Some(artist),
                album: Some(album),
                cover_url: (!cover.is_empty()).then_some(cover),
                duration: Some(Duration::from_millis(duration_ms as u64)),
            });
        }
    });
}

pub fn set_playing(playing: bool, position_ms: u32) {
    CONTROLS.with_borrow_mut(|c| {
        if let Some(c) = c {
            let progress = Some(MediaPosition(Duration::from_millis(position_ms as u64)));
            let _ = c.set_playback(if playing {
                MediaPlayback::Playing { progress }
            } else {
                MediaPlayback::Paused { progress }
            });
        }
    });
}
