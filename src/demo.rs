//! Datos de ejemplo para revisar la interfaz sin cuenta (solo en debug).
//! Uso: SWIFTBEAT_DEMO=<página> donde página es -1 login, 0 inicio, 1 buscar, 2 colección, 3 artista, 4 ajustes.

use crate::{AppWindow, Card, LyricLine, State, TrackRow, ui};
use slint::ComponentHandle;

fn card(i: usize, kind: i32) -> Card {
    let titles = ["Noches de verano", "Indie en español", "Lo-fi para estudiar", "Clásicos del rock", "Rap latino", "Jazz tranquilo", "Electrónica", "Acústico"];
    Card {
        uri: format!("spotify:demo:{kind}:{i}").into(),
        title: titles[i % titles.len()].into(),
        subtitle: match kind {
            1 => "Artista".into(),
            0 => "2024 · Álbum".into(),
            _ => "Playlist · demo".into(),
        },
        image: "".into(),
        kind,
    }
}

fn track(i: usize) -> TrackRow {
    let names = ["Canción de ejemplo", "Otra canción con un título bastante largo para probar el recorte", "Ritmo", "Atardecer", "Ciudad"];
    TrackRow {
        kind: 0,
        index: i as i32,
        uri: format!("spotify:track:demo{i}").into(),
        title: names[i % names.len()].into(),
        artists: "Artista de prueba".into(),
        artist_uri: "spotify:artist:demo".into(),
        artist2: if i.is_multiple_of(2) { "Invitado".into() } else { "".into() },
        artist2_uri: "spotify:artist:demo2".into(),
        artist3: if i.is_multiple_of(4) { "Otro artista".into() } else { "".into() },
        artist3_uri: "spotify:artist:demo3".into(),
        more_artists: i.is_multiple_of(8),
        album: "Álbum de ejemplo".into(),
        album_uri: "spotify:album:demo".into(),
        duration: format!("{}:{:02}", 2 + i % 3, (i * 17) % 60).into(),
        image: "".into(),
        explicit: i % 4 == 1,
    }
}

pub fn fill(app: &AppWindow, page: i32) {
    let st = app.global::<State>();
    if page < 0 {
        st.set_screen(1);
        return;
    }
    st.set_screen(2);
    st.set_page(page);
    st.set_user_name("Demo".into());
    st.set_user_initial("D".into());
    st.set_greeting("Buenas noches".into());

    let mut lib = vec![Card { uri: "spotify:collection:tracks".into(), title: "Tus me gusta".into(), subtitle: "Playlist · 523 canciones".into(), image: "".into(), kind: 3 }];
    lib.extend((0..8).map(|i| card(i, 2)));
    lib.extend((0..4).map(|i| card(i, 0)));
    lib.extend((0..4).map(|i| card(i, 1)));
    st.set_library(ui::model(lib.clone()));
    st.set_quick(ui::model(lib[..8].to_vec()));
    st.set_shelves(ui::shelves(vec![
        ("Escuchado recientemente".into(), (0..10).map(|i| card(i, 0)).collect()),
        ("Tus playlists".into(), (0..10).map(|i| card(i, 2)).collect()),
        ("Tus artistas favoritos".into(), (0..10).map(|i| card(i, 1)).collect()),
    ]));

    st.set_search_text("rock".into());
    st.set_has_top(true);
    st.set_search_top(card(3, 1));
    st.set_search_tracks(ui::model((0..4).map(track).collect()));
    st.set_search_shelves(ui::shelves(vec![
        ("Artistas".into(), (0..8).map(|i| card(i, 1)).collect()),
        ("Álbumes".into(), (0..8).map(|i| card(i, 0)).collect()),
    ]));

    st.set_coll_uri("spotify:playlist:demo".into());
    st.set_coll_type("Playlist".into());
    st.set_coll_title("Lo-fi para estudiar".into());
    st.set_coll_description("Ritmos tranquilos para concentrarte.".into());
    st.set_coll_subtitle("Demo · 120 canciones, 6 h 12 min".into());
    ui::set_collection_rows(app, (0..120).map(track).collect());

    st.set_artist_uri("spotify:artist:demo".into());
    st.set_artist_name("Artista de prueba".into());
    st.set_artist_top(ui::model((0..5).map(track).collect()));
    st.set_artist_shelves(ui::shelves(vec![("Álbumes".into(), (0..8).map(|i| card(i, 0)).collect())]));

    st.set_np_uri("spotify:track:demo1".into());
    st.set_np_title("Canción de ejemplo".into());
    st.set_np_artists("Artista de prueba".into());
    st.set_np_duration(215_000);
    st.set_np_position(83_000);
    st.set_playing(true);
    st.set_shuffle(true);
    st.set_repeat(2);

    if std::env::var("SWIFTBEAT_PANEL").as_deref() == Ok("2") {
        st.set_right_panel(2);
        st.set_lyrics(ui::model(
            ["Primera línea de la letra", "Segunda línea un poco más larga que la anterior", "Estribillo", "♪", "Otra línea"]
                .iter()
                .enumerate()
                .map(|(i, t)| LyricLine { start: i as i32 * 4000, text: (*t).into() })
                .collect(),
        ));
        st.set_lyrics_active(1);
    } else if std::env::var("SWIFTBEAT_PANEL").as_deref() == Ok("1") {
        st.set_right_panel(1);
        st.set_queue(ui::model((0..10).map(track).collect()));
    }
}
