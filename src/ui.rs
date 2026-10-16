use crate::{
    AppWindow, Card, Shelf, State, TrackRow,
    meta::{CardInfo, TrackInfo},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

/// La cabecera de una colección (392px) se dibuja en 7 filas de 56px.
pub const HEADER_SLICES: i32 = 7;

#[derive(Clone)]
pub struct Ui(pub slint::Weak<AppWindow>);

impl Ui {
    pub fn run(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let _ = self.0.upgrade_in_event_loop(move |ui| f(&ui));
    }

    pub fn state(&self, f: impl FnOnce(&State<'_>) + Send + 'static) {
        self.run(move |ui| f(&ui.global::<State>()));
    }
}

pub fn fmt_ms(ms: u32) -> String {
    format!("{}:{:02}", ms / 60_000, ms / 1000 % 60)
}

pub fn track_row(t: &TrackInfo, index: usize) -> TrackRow {
    let artist = |i: usize| t.artists.get(i).cloned().unwrap_or_default();
    let (a1_uri, a1) = artist(0);
    let (a2_uri, a2) = artist(1);
    let (a3_uri, a3) = artist(2);
    TrackRow {
        kind: 0,
        index: index as i32,
        uri: t.uri.as_str().into(),
        title: t.name.as_str().into(),
        artists: a1.into(),
        artist_uri: a1_uri.into(),
        artist2: a2.into(),
        artist2_uri: a2_uri.into(),
        artist3: a3.into(),
        artist3_uri: a3_uri.into(),
        more_artists: t.artists.len() > 3,
        album: t.album.as_str().into(),
        album_uri: t.album_uri.as_str().into(),
        duration: fmt_ms(t.duration_ms).into(),
        image: t.image_small.as_str().into(),
        explicit: t.explicit,
    }
}

pub fn track_rows(tracks: &[TrackInfo], offset: usize) -> Vec<TrackRow> {
    tracks.iter().enumerate().map(|(i, t)| track_row(t, offset + i)).collect()
}

pub fn card(c: &CardInfo) -> Card {
    Card {
        uri: c.uri.as_str().into(),
        title: c.title.as_str().into(),
        subtitle: c.subtitle.as_str().into(),
        image: c.image.as_str().into(),
        kind: c.kind,
    }
}

pub fn cards(cs: &[CardInfo]) -> Vec<Card> {
    cs.iter().map(card).collect()
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Debe llamarse en el hilo de la UI (los modelos no son `Send`).
pub fn shelves(v: Vec<(String, Vec<Card>)>) -> ModelRc<Shelf> {
    model(
        v.into_iter()
            .map(|(title, items)| Shelf { title: title.into(), items: model(items) })
            .collect(),
    )
}

thread_local! {
    static COLLECTION: RefCell<Option<Rc<VecModel<TrackRow>>>> = const { RefCell::new(None) };
}

/// Sustituye las filas de la colección abierta (cabecera incluida).
pub fn set_collection_rows(ui: &AppWindow, rows: Vec<TrackRow>) {
    let mut all: Vec<TrackRow> = (0..HEADER_SLICES)
        .map(|i| TrackRow { kind: 1, index: i, ..Default::default() })
        .collect();
    all.extend(rows);
    let m = Rc::new(VecModel::from(all));
    COLLECTION.set(Some(m.clone()));
    ui.global::<State>().set_coll_rows(ModelRc::from(m));
}

/// Añade filas al final sin recrear la lista (carga progresiva de playlists).
pub fn append_collection_rows(rows: Vec<TrackRow>) {
    COLLECTION.with_borrow(|m| {
        if let Some(m) = m {
            m.extend(rows);
        }
    });
}
