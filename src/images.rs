//! Caché de portadas en el hilo de la UI.
//!
//! La UI pide cada imagen con `Img.get(url, tamaño, epoch)`. Solo se descargan las
//! imágenes de los elementos visibles (las listas están virtualizadas), se reducen
//! al tamaño en pantalla y se guardan con un presupuesto de memoria fijo (LRU).
//! Al llegar imágenes nuevas se incrementa `Img.epoch` para que se repinten.

use crate::{AppWindow, Img};
use image::imageops::FilterType;
use slint::{Color, ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::{runtime::Handle, sync::Semaphore};

const BUDGET_BYTES: usize = 48 * 1024 * 1024;
const SIZES: [u32; 8] = [48, 64, 96, 128, 192, 256, 384, 640];

struct Entry {
    image: Image,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    colors: HashMap<String, Color>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    bytes: usize,
    tick: u64,
    bump_scheduled: bool,
}

struct Ctx {
    ui: slint::Weak<AppWindow>,
    rt: Handle,
    http: reqwest::Client,
    permits: Arc<Semaphore>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

pub fn install(ui: &AppWindow, rt: Handle, http: reqwest::Client) {
    CTX.set(Some(Ctx {
        ui: ui.as_weak(),
        rt,
        http,
        permits: Arc::new(Semaphore::new(6)),
    }));
    let img = ui.global::<Img>();
    img.on_get(|url, px, _| get(&url, px));
    img.on_tint(|url, _| tint(&url));
}

fn scale_factor() -> f32 {
    CTX.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.ui.upgrade())
            .map(|ui| ui.window().scale_factor())
            .unwrap_or(1.0)
    })
}

fn bucket(px: i32) -> u32 {
    let want = (px.max(1) as f32 * scale_factor()).ceil() as u32;
    SIZES.iter().copied().find(|s| *s >= want).unwrap_or(640)
}

fn get(url: &str, px: i32) -> Image {
    if url.is_empty() {
        return Image::default();
    }
    let size = bucket(px);
    let key = format!("{size}|{url}");
    CACHE.with_borrow_mut(|c| {
        c.tick += 1;
        let tick = c.tick;
        if let Some(e) = c.entries.get_mut(&key) {
            e.used = tick;
            return e.image.clone();
        }
        if !c.failed.contains(&key) && c.pending.insert(key.clone()) {
            fetch(url.to_string(), size, key);
        }
        Image::default()
    })
}

fn tint(url: &str) -> Color {
    if url == "liked" {
        return Color::from_rgb_u8(0x50, 0x38, 0xa0);
    }
    CACHE.with_borrow(|c| c.colors.get(url).copied())
        .unwrap_or(Color::from_rgb_u8(0x40, 0x40, 0x40))
}

fn fetch(url: String, size: u32, key: String) {
    CTX.with_borrow(|ctx| {
        let Some(ctx) = ctx else { return };
        let http = ctx.http.clone();
        let permits = ctx.permits.clone();
        ctx.rt.spawn(async move {
            let _permit = permits.acquire_owned().await;
            let bytes = match http.get(&url).send().await.and_then(|r| r.error_for_status()) {
                Ok(r) => r.bytes().await.ok(),
                Err(e) => {
                    log::debug!("imagen {url}: {e}");
                    None
                }
            };
            let decoded = match bytes {
                Some(b) => tokio::task::spawn_blocking(move || decode(&b, size))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let _ = slint::invoke_from_event_loop(move || finish(key, url, decoded));
        });
    });
}

/// Decodifica y reduce para que el lado corto mida `size` px. Devuelve también el color medio.
fn decode(bytes: &[u8], size: u32) -> Option<(SharedPixelBuffer<Rgb8Pixel>, [u8; 3])> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let short = w.min(h).max(1);
    let img = if short > size {
        let k = size as f32 / short as f32;
        img.resize_exact(
            ((w as f32 * k).round() as u32).max(1),
            ((h as f32 * k).round() as u32).max(1),
            FilterType::Triangle,
        )
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for p in rgb.pixels().step_by(7) {
        sum[0] += p[0] as u64;
        sum[1] += p[1] as u64;
        sum[2] += p[2] as u64;
        n += 1;
    }
    let n = n.max(1);
    let avg = [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8];
    let buf = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(rgb.as_raw(), rgb.width(), rgb.height());
    Some((buf, avg))
}

/// Ajusta el color medio para que sirva de fondo: canal máximo en ~150 y algo más saturado.
fn header_color([r, g, b]: [u8; 3]) -> Color {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let mean = (r + g + b) / 3.0;
    let sat = |c: f32| mean + (c - mean) * 1.35;
    let (r, g, b) = (sat(r).max(0.0), sat(g).max(0.0), sat(b).max(0.0));
    let k = 150.0 / r.max(g).max(b).max(1.0);
    Color::from_rgb_u8(
        (r * k).min(255.0) as u8,
        (g * k).min(255.0) as u8,
        (b * k).min(255.0) as u8,
    )
}

fn finish(key: String, url: String, decoded: Option<(SharedPixelBuffer<Rgb8Pixel>, [u8; 3])>) {
    let schedule = CACHE.with_borrow_mut(|c| {
        c.pending.remove(&key);
        let Some((buf, avg)) = decoded else {
            c.failed.insert(key);
            return false;
        };
        let bytes = buf.width() as usize * buf.height() as usize * 3;
        c.colors.insert(url, header_color(avg));
        c.tick += 1;
        let used = c.tick;
        c.entries.insert(key, Entry { image: Image::from_rgb8(buf), bytes, used });
        c.bytes += bytes;
        while c.bytes > BUDGET_BYTES {
            let Some(oldest) = c.entries.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else {
                break;
            };
            if let Some(e) = c.entries.remove(&oldest) {
                c.bytes -= e.bytes;
            }
        }
        if c.bump_scheduled {
            false
        } else {
            c.bump_scheduled = true;
            true
        }
    });
    if schedule {
        // Agrupa varias imágenes en un solo repintado.
        slint::Timer::single_shot(Duration::from_millis(40), || {
            CACHE.with_borrow_mut(|c| c.bump_scheduled = false);
            CTX.with_borrow(|ctx| {
                if let Some(ui) = ctx.as_ref().and_then(|c| c.ui.upgrade()) {
                    let img = ui.global::<Img>();
                    img.set_epoch(img.get_epoch() + 1);
                }
            });
        });
    }
}
