// Sin consola en las compilaciones de release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod auth;
mod backend;
mod config;
#[cfg(debug_assertions)]
mod demo;
mod images;
mod logger;
mod media;
mod meta;
mod player;
mod repaint;
mod secure;
mod single;
mod ui;
mod webapi;

use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
    time::Duration,
};

slint::include_modules!();

/// Mismo identificador que ponen los accesos directos del instalador, para que
/// Windows agrupe la ventana con el icono anclado en la barra de tareas.
pub const APP_USER_MODEL_ID: &str = "Swiftbeat.Swiftbeat";

#[cfg(windows)]
fn set_app_id() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
    }
    let id: Vec<u16> = APP_USER_MODEL_ID.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `id` es una cadena UTF-16 terminada en cero que vive durante la llamada.
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
}

#[cfg(not(windows))]
fn set_app_id() {}

fn main() -> anyhow::Result<()> {
    // Antes que el log, para no vaciar el de la instancia que ya está abierta.
    let demo = cfg!(debug_assertions) && std::env::var_os("SWIFTBEAT_DEMO").is_some();
    let restarting = std::env::args().any(|a| a == single::RESTART_ARG);
    if !demo && !single::acquire(restarting) {
        return Ok(());
    }
    logger::init();
    log::info!("Swiftbeat {}", env!("CARGO_PKG_VERSION"));

    // Dos hilos bastan: la red es asíncrona y el audio tiene su propio hilo.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("swiftbeat")
        .enable_all()
        .build()?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("Swiftbeat/", env!("CARGO_PKG_VERSION")))
        .pool_max_idle_per_host(2)
        .timeout(Duration::from_secs(20))
        .build()?;

    set_app_id();
    let app = AppWindow::new()?;
    app.global::<State>().set_version(env!("CARGO_PKG_VERSION").into());
    images::install(&app, rt.handle().clone(), http.clone());

    #[cfg(debug_assertions)]
    if let Some(page) = std::env::var("SWIFTBEAT_DEMO").ok().and_then(|p| p.parse().ok()) {
        demo::fill(&app, page);
        repaint::install(app.as_weak());
        app.run()?;
        return Ok(());
    }

    let liked: backend::Liked = Arc::new(RwLock::new(HashSet::new()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    backend::bind(&app, &tx, liked.clone());

    app.show()?;
    repaint::install(app.as_weak());
    media::install_when_ready(app.as_weak(), tx.clone());
    rt.spawn(backend::run(ui::Ui(app.as_weak()), tx, rx, http, liked));
    slint::run_event_loop()?;
    rt.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}
