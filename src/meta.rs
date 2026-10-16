//! Modelo de datos y obtención de metadatos.
//!
//! Álbumes, artistas y playlists se piden a los servidores internos de Spotify a
//! través de la sesión de librespot, en lotes de hasta 100 entidades por petición.
//! La Web API solo se usa para búsqueda y biblioteca (ver `webapi.rs`).

use anyhow::{Result, anyhow};
use librespot_core::{FileId, Session, SpotifyUri};
use librespot_metadata::{
    Album, Artist, Lyrics, Metadata, Playlist, Track,
    image::{ImageSize, Images},
    lyrics::SyncType,
};
use librespot_protocol::{
    extended_metadata::{BatchedEntityRequest, EntityRequest, ExtensionQuery},
    extension_kind::ExtensionKind,
};
use protobuf::{EnumOrUnknown, Message};
use serde_json::Value;
use std::collections::HashMap;

pub const KIND_ALBUM: i32 = 0;
pub const KIND_ARTIST: i32 = 1;
pub const KIND_PLAYLIST: i32 = 2;
pub const KIND_LIKED: i32 = 3;
pub const KIND_TRACK: i32 = 4;
pub const LIKED_URI: &str = "spotify:collection:tracks";

#[derive(Clone, Debug, Default)]
pub struct TrackInfo {
    pub uri: String,
    pub name: String,
    pub artists: Vec<(String, String)>,
    pub album: String,
    pub album_uri: String,
    pub duration_ms: u32,
    pub image_small: String,
    pub image_large: String,
    pub explicit: bool,
}

impl TrackInfo {
    pub fn artist_names(&self) -> String {
        self.artists
            .iter()
            .map(|(_, n)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn first_artist_uri(&self) -> String {
        self.artists.first().map(|(u, _)| u.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct CardInfo {
    pub uri: String,
    pub title: String,
    pub subtitle: String,
    pub image: String,
    pub kind: i32,
    pub owner_id: String,
}

pub fn liked_card(count: usize) -> CardInfo {
    CardInfo {
        uri: LIKED_URI.into(),
        title: "Tus me gusta".into(),
        subtitle: if count > 0 {
            format!("Playlist · {count} canciones")
        } else {
            "Playlist".into()
        },
        kind: KIND_LIKED,
        ..Default::default()
    }
}

pub fn fmt_total(ms: u64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        format!("{} h {} min", mins / 60, mins % 60)
    } else {
        format!("{mins} min")
    }
}

// ---------------------------------------------------------------- Web API (JSON)

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Elige la imagen más pequeña que mida al menos `min_px`, o la más grande disponible.
pub fn pick_image(images: &Value, min_px: u64) -> String {
    let Some(arr) = images.as_array() else {
        return String::new();
    };
    let mut best: Option<(u64, &str)> = None;
    for img in arr {
        let Some(url) = img["url"].as_str() else { continue };
        let w = img["width"].as_u64().unwrap_or(640);
        best = match best {
            None => Some((w, url)),
            Some((bw, bu)) => {
                let better = if w >= min_px { bw < min_px || w < bw } else { bw < min_px && w > bw };
                Some(if better { (w, url) } else { (bw, bu) })
            }
        };
    }
    best.map(|(_, u)| u.to_string()).unwrap_or_default()
}

pub fn track_from_json(v: &Value) -> Option<TrackInfo> {
    let uri = v["uri"].as_str()?;
    if !uri.starts_with("spotify:track:") {
        return None;
    }
    Some(TrackInfo {
        uri: uri.to_string(),
        name: s(&v["name"]),
        artists: v["artists"]
            .as_array()
            .map(|a| a.iter().map(|x| (s(&x["uri"]), s(&x["name"]))).collect())
            .unwrap_or_default(),
        album: s(&v["album"]["name"]),
        album_uri: s(&v["album"]["uri"]),
        duration_ms: v["duration_ms"].as_u64().unwrap_or(0) as u32,
        image_small: pick_image(&v["album"]["images"], 64),
        image_large: pick_image(&v["album"]["images"], 300),
        explicit: v["explicit"].as_bool().unwrap_or(false),
    })
}

fn joined_artists(v: &Value) -> String {
    v["artists"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

pub fn album_card(v: &Value) -> Option<CardInfo> {
    let uri = v["uri"].as_str()?;
    let year = v["release_date"].as_str().unwrap_or("").get(..4).unwrap_or("");
    let artists = joined_artists(v);
    Some(CardInfo {
        uri: uri.to_string(),
        title: s(&v["name"]),
        subtitle: match (year.is_empty(), artists.is_empty()) {
            (false, false) => format!("{year} · {artists}"),
            (true, _) => artists,
            (false, true) => year.to_string(),
        },
        image: pick_image(&v["images"], 300),
        kind: KIND_ALBUM,
        ..Default::default()
    })
}

pub fn artist_card(v: &Value) -> Option<CardInfo> {
    let uri = v["uri"].as_str()?;
    Some(CardInfo {
        uri: uri.to_string(),
        title: s(&v["name"]),
        subtitle: "Artista".into(),
        image: pick_image(&v["images"], 300),
        kind: KIND_ARTIST,
        ..Default::default()
    })
}

pub fn playlist_card(v: &Value) -> Option<CardInfo> {
    let uri = v["uri"].as_str()?;
    let owner = v["owner"]["display_name"].as_str().unwrap_or("");
    Some(CardInfo {
        uri: uri.to_string(),
        title: s(&v["name"]),
        subtitle: if owner.is_empty() {
            "Playlist".into()
        } else {
            format!("Playlist · {owner}")
        },
        image: pick_image(&v["images"], 300),
        kind: KIND_PLAYLIST,
        owner_id: s(&v["owner"]["id"]),
    })
}

// ----------------------------------------------------- Metadatos internos (librespot)

fn image_url(id: &FileId) -> String {
    format!(
        "https://i.scdn.co/image/{}",
        id.to_base16().unwrap_or_default().to_lowercase()
    )
}

fn pick_meta_image(images: &Images, large: bool) -> String {
    let order: &[ImageSize] = if large {
        &[ImageSize::DEFAULT, ImageSize::LARGE, ImageSize::XLARGE, ImageSize::SMALL]
    } else {
        &[ImageSize::SMALL, ImageSize::DEFAULT, ImageSize::LARGE]
    };
    for size in order {
        if let Some(img) = images.iter().find(|i| i.size == *size) {
            return image_url(&img.id);
        }
    }
    images.first().map(|i| image_url(&i.id)).unwrap_or_default()
}

fn album_images(a: &Album) -> &Images {
    if a.covers.is_empty() { &a.cover_group } else { &a.covers }
}

fn uri_str(u: &SpotifyUri) -> String {
    u.to_uri().unwrap_or_default()
}

pub fn track_from_meta(t: &Track) -> TrackInfo {
    let covers = album_images(&t.album);
    TrackInfo {
        uri: uri_str(&t.id),
        name: t.name.clone(),
        artists: t.artists.iter().map(|a| (uri_str(&a.id), a.name.clone())).collect(),
        album: t.album.name.clone(),
        album_uri: uri_str(&t.album.id),
        duration_ms: t.duration.max(0) as u32,
        image_small: pick_meta_image(covers, false),
        image_large: pick_meta_image(covers, true),
        explicit: t.is_explicit,
    }
}

fn album_kind_label(type_str: &str) -> &'static str {
    match type_str.to_ascii_lowercase().as_str() {
        "single" => "Sencillo",
        "ep" => "EP",
        "compilation" => "Recopilatorio",
        _ => "Álbum",
    }
}

pub fn album_card_from_meta(a: &Album) -> CardInfo {
    CardInfo {
        uri: uri_str(&a.id),
        title: a.name.clone(),
        subtitle: format!("{} · {}", a.date.year(), album_kind_label(&a.type_str)),
        image: pick_meta_image(album_images(a), true),
        kind: KIND_ALBUM,
        ..Default::default()
    }
}

fn artist_image(a: &Artist, large: bool) -> String {
    if !a.portrait_group.is_empty() {
        pick_meta_image(&a.portrait_group, large)
    } else {
        pick_meta_image(&a.portraits, large)
    }
}

/// Pide metadatos de muchas entidades en peticiones por lotes y conserva el orden.
async fn batch<T: Metadata>(session: &Session, uris: &[SpotifyUri], kind: ExtensionKind) -> Vec<T> {
    let mut out = Vec::with_capacity(uris.len());
    for chunk in uris.chunks(100) {
        let request = BatchedEntityRequest {
            entity_request: chunk
                .iter()
                .filter_map(|u| {
                    Some(EntityRequest {
                        entity_uri: u.to_uri().ok()?,
                        query: vec![ExtensionQuery {
                            extension_kind: EnumOrUnknown::new(kind),
                            ..Default::default()
                        }],
                        ..Default::default()
                    })
                })
                .collect(),
            ..Default::default()
        };
        let response = match session.spclient().get_extended_metadata(request).await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("metadatos por lotes ({kind:?}): {e}");
                continue;
            }
        };
        let mut found: HashMap<String, T> = HashMap::new();
        for array in response.extended_metadata {
            for data in array.extension_data {
                let Some(any) = data.extension_data.as_ref() else { continue };
                if any.value.is_empty() {
                    continue;
                }
                let Ok(uri) = SpotifyUri::from_uri(&data.entity_uri) else { continue };
                let Ok(msg) = T::Message::parse_from_bytes(&any.value) else { continue };
                if let Ok(item) = T::parse(&msg, &uri) {
                    found.insert(data.entity_uri.clone(), item);
                }
            }
        }
        for u in chunk {
            if let Some(item) = u.to_uri().ok().and_then(|k| found.remove(&k)) {
                out.push(item);
            }
        }
    }
    out
}

pub async fn tracks(session: &Session, uris: &[SpotifyUri]) -> Vec<TrackInfo> {
    batch::<Track>(session, uris, ExtensionKind::TRACK_V4)
        .await
        .iter()
        .map(track_from_meta)
        .collect()
}

pub struct AlbumPage {
    pub label: String,
    pub title: String,
    pub subtitle: String,
    pub image: String,
    pub tracks: Vec<TrackInfo>,
}

pub async fn album_page(session: &Session, uri: &str) -> Result<AlbumPage> {
    let id = SpotifyUri::from_uri(uri)?;
    let album = Album::get(session, &id).await?;
    let ids: Vec<SpotifyUri> = album.tracks().cloned().collect();
    let tracks = tracks(session, &ids).await;
    let total: u64 = tracks.iter().map(|t| t.duration_ms as u64).sum();
    let artists = album.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
    Ok(AlbumPage {
        label: album_kind_label(&album.type_str).into(),
        title: album.name.clone(),
        subtitle: format!(
            "{artists} · {} · {} canciones, {}",
            album.date.year(),
            tracks.len(),
            fmt_total(total)
        ),
        image: pick_meta_image(album_images(&album), true),
        tracks,
    })
}

pub struct ArtistPage {
    pub name: String,
    pub image: String,
    pub top: Vec<TrackInfo>,
    pub shelves: Vec<(String, Vec<CardInfo>)>,
}

pub async fn artist_page(session: &Session, uri: &str) -> Result<ArtistPage> {
    let id = SpotifyUri::from_uri(uri)?;
    let artist = Artist::get(session, &id).await?;
    let country = session.country();
    let mut top_ids: Vec<SpotifyUri> = artist.top_tracks.for_country(&country).0;
    if top_ids.is_empty() {
        top_ids = artist.top_tracks.first().map(|t| t.tracks.0.clone()).unwrap_or_default();
    }
    top_ids.truncate(10);

    let album_ids: Vec<SpotifyUri> = artist.albums_current().take(20).cloned().collect();
    let single_ids: Vec<SpotifyUri> = artist.singles_current().take(20).cloned().collect();
    let related_ids: Vec<SpotifyUri> = artist.related.iter().take(12).map(|a| a.id.clone()).collect();

    let (top, albums, singles, related) = tokio::join!(
        tracks(session, &top_ids),
        batch::<Album>(session, &album_ids, ExtensionKind::ALBUM_V4),
        batch::<Album>(session, &single_ids, ExtensionKind::ALBUM_V4),
        batch::<Artist>(session, &related_ids, ExtensionKind::ARTIST_V4),
    );

    let mut shelves = Vec::new();
    let mut push = |title: &str, cards: Vec<CardInfo>| {
        if !cards.is_empty() {
            shelves.push((title.to_string(), cards));
        }
    };
    let by_year = |mut v: Vec<Album>| {
        v.sort_by_key(|a| std::cmp::Reverse(a.date.as_timestamp_ms()));
        v.iter().map(album_card_from_meta).collect::<Vec<_>>()
    };
    push("Álbumes", by_year(albums));
    push("Sencillos y EP", by_year(singles));
    push(
        "Los fans también escuchan",
        related
            .iter()
            .map(|a| CardInfo {
                uri: uri_str(&a.id),
                title: a.name.clone(),
                subtitle: "Artista".into(),
                image: artist_image(a, true),
                kind: KIND_ARTIST,
                ..Default::default()
            })
            .collect(),
    );

    Ok(ArtistPage {
        name: artist.name.clone(),
        image: artist_image(&artist, true),
        top,
        shelves,
    })
}

pub struct PlaylistHead {
    pub title: String,
    pub description: String,
    pub image: String,
    pub items: Vec<SpotifyUri>,
}

pub async fn playlist_head(session: &Session, uri: &str) -> Result<PlaylistHead> {
    let id = SpotifyUri::from_uri(uri)?;
    let pl = Playlist::get(session, &id).await?;
    let image = pl
        .attributes
        .picture_sizes
        .iter()
        .find(|p| p.target_name == "default")
        .or_else(|| pl.attributes.picture_sizes.first())
        .map(|p| p.url.clone())
        .unwrap_or_default();
    Ok(PlaylistHead {
        title: pl.name().to_string(),
        description: strip_html(&pl.attributes.description),
        image,
        items: pl
            .tracks()
            .filter(|u| matches!(u, SpotifyUri::Track { .. }))
            .cloned()
            .collect(),
    })
}

fn strip_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
}

/// Letra de una canción: (inicio en ms o -1 si no está sincronizada, texto).
pub async fn lyrics(session: &Session, track_uri: &str) -> Result<Vec<(i32, String)>> {
    let SpotifyUri::Track { id } = SpotifyUri::from_uri(track_uri)? else {
        return Err(anyhow!("no es una canción"));
    };
    let l = Lyrics::get(session, &id).await?;
    let synced = l.lyrics.sync_type == SyncType::LineSynced;
    Ok(l
        .lyrics
        .lines
        .iter()
        .map(|line| {
            let start = if synced { line.start_time_ms.parse().unwrap_or(-1) } else { -1 };
            let text = if line.words.trim().is_empty() { "♪".to_string() } else { line.words.clone() };
            (start, text)
        })
        .collect())
}
