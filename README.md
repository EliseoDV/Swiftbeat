<p align="center">
  <img src="assets/swiftbeat.png" width="96" alt="Logo de Swiftbeat">
</p>

<h1 align="center">Swiftbeat</h1>

<p align="center">Cliente de escritorio ligero para Spotify en Windows, escrito en Rust.<br>
Interfaz familiar, una fracción de los recursos.</p>

| | Spotify oficial | Swiftbeat |
|---|---|---|
| Motor de UI | Chromium (CEF) | Slint, renderizado por software, sin navegador |
| Procesos | 5–8 | 1 |
| Memoria | 400–800 MB | ~40 MB en reposo |
| Instalación | ~400 MB | un `.exe` de 16 MB (instalador de 8 MB) |

## Instalación

1. Descarga `Swiftbeat-Setup-x.y.z.exe` desde [Releases](https://github.com/EliseoDV/Swiftbeat/releases).
2. Ejecútalo. Se instala solo para tu usuario, sin permisos de administrador, y crea los accesos directos del escritorio y del menú Inicio.
3. Para anclarla a la barra de tareas: con la app abierta, clic derecho en su icono y **Anclar a la barra de tareas**.

> Windows SmartScreen puede avisar de que el instalador no está firmado. Pulsa **Más información → Ejecutar de todas formas**.

Para desinstalarla: *Configuración → Aplicaciones → Swiftbeat → Desinstalar*.

### Requisitos

- Windows 10/11 x64.
- **Spotify Premium.** La reproducción usa [librespot](https://github.com/librespot-org/librespot), que solo funciona con cuentas Premium.
- **Tu propio Client ID** de Spotify, gratuito. Cada usuario debe crear el suyo, porque las apps en modo desarrollo solo admiten 5 usuarios:
  1. Entra en <https://developer.spotify.com/dashboard> → **Create app**.
  2. En **Redirect URIs** añade exactamente `http://127.0.0.1:8989/login`.
  3. Marca **Web API**, guarda y copia el **Client ID** de *Settings*. El *Client secret* no hace falta.
  4. Pégalo en Swiftbeat y acepta los dos permisos que se abren en el navegador.

El Client ID se guarda solo en tu equipo (`%APPDATA%\Swiftbeat`) y nunca sale de él.

## Funciones

- Inicio con saludo, accesos rápidos y estanterías: escuchado recientemente, playlists, artistas y álbumes.
- Búsqueda con resultado principal, canciones, artistas, álbumes y playlists.
- Páginas de playlist, álbum, artista y «Tus me gusta», con cabecera teñida según la portada.
- Biblioteca lateral con filtros.
- Reproductor: aleatorio, repetir (contexto o canción), barra de progreso, volumen y silencio.
- Cola y letra sincronizada en el panel derecho.
- Menú contextual: añadir a la cola, ir al artista o álbum, añadir a una playlist.
- Me gusta y crear playlists.
- Teclas multimedia y panel de medios de Windows con la portada.
- Atajos: `Espacio` reproduce o pausa, `Ctrl+←/→` cambia de canción, `Alt+←/→` navega atrás y adelante.
- Calidad de audio de 96, 160 o 320 kbps, y normalización de volumen.

## Compilar desde el código

Requisitos: [Rust](https://rustup.rs) estable y, para el instalador, [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`).

```powershell
cargo run --release                                          # ejecutar
powershell -ExecutionPolicy Bypass -File installer\build.ps1 # generar installer\Output\Swiftbeat-Setup-x.y.z.exe
```

Funciona con las toolchains `x86_64-pc-windows-msvc` y `x86_64-pc-windows-gnu`.

Para revisar la interfaz sin cuenta, en modo debug, con datos de ejemplo:

```powershell
$env:SWIFTBEAT_DEMO = "2"   # 0 inicio, 1 buscar, 2 colección, 3 artista, 4 ajustes
$env:SWIFTBEAT_PANEL = "1"  # opcional: 1 cola, 2 letra
cargo run
```

### Publicar una versión

1. Sube la versión en `Cargo.toml`.
2. Crea y sube la etiqueta: `git tag v0.2.0 && git push origin v0.2.0`.
3. GitHub Actions compila, genera el instalador y lo adjunta a la release (ver `.github/workflows/release.yml`).

El logo y sus reglas de uso están en `IconInfo/` (ver `IconInfo/GUIA-DE-MARCA.md`). `installer\make-icon.ps1` copia a `assets/` el icono de la app desde `IconInfo/windows/`.

## Cómo consigue ser ligera

- **Sin navegador embebido.** UI nativa con Slint, dibujada por software: no carga controladores de GPU y solo repinta lo que cambia.
- **Listas virtualizadas.** Una playlist de 5.000 canciones solo crea las filas visibles.
- **Imágenes bajo demanda.** Solo se descargan las portadas visibles, reducidas al tamaño en pantalla, en una caché LRU con un límite de 96 MB.
- **Metadatos por lotes.** Álbumes, artistas y playlists se piden en bloques de 100.
- **Runtime pequeño.** 2 hilos de Tokio más el hilo de audio; nada se ejecuta a 60 fps (las únicas animaciones continuas son los indicadores de carga, y solo mientras se ven).
- **Reproducción vigilada.** Si una canción no empieza en 10 s, deja de avanzar o se corta, Swiftbeat reconecta la sesión y la retoma donde iba; tras suspender el equipo también reconecta antes de reproducir.
- **Binario optimizado.** LTO completo, `codegen-units = 1`, `panic = "abort"` y sin símbolos.

## Arquitectura

```
src/
  main.rs     arranque: runtime, ventana, callbacks
  backend.rs  actor con la lógica: sesión, navegación, cargas, reproducción
  player.rs   librespot Player + cola, aleatorio y repetición
  meta.rs     modelo de datos, JSON de la Web API y metadatos por lotes
  webapi.rs   cliente Web API con renovación de token y respaldo
  auth.rs     OAuth PKCE con servidor de redirección local
  images.rs   caché de portadas en el hilo de la UI
  media.rs    controles multimedia de Windows (SMTC)
  ui.rs       conversión de datos a modelos de Slint
ui/           interfaz en Slint
installer/    script de Inno Setup, compilación del instalador y generador del logo
```

La UI envía órdenes por un canal a un único actor. Las cargas lentas se lanzan como tareas aparte, así que reproducir o pausar nunca espera a la red.

Desde febrero de 2026, las apps de la Web API en modo desarrollo no pueden leer álbumes, artistas ni playlists ajenas. Por eso Swiftbeat usa dos vías:

- **Web API (tu Client ID):** búsqueda y tu biblioteca. Si Spotify rechaza una petición, se reintenta una vez con el token de la sesión.
- **Sesión de librespot:** audio, metadatos de álbumes, artistas y playlists, y letras.

## Aviso legal

Swiftbeat es un proyecto independiente y **no está afiliado, patrocinado ni respaldado por Spotify**. «Spotify» es una marca de Spotify AB.

La reproducción usa librespot, un cliente no oficial del protocolo de Spotify, y su uso puede ir contra los términos de servicio de Spotify. Úsalo bajo tu responsabilidad. Swiftbeat no elude el DRM ni ofrece funciones de pago gratis: reproduce el audio de tu propia cuenta Premium, igual que spotifyd, ncspot o spotify-player.

## Licencia

[MIT](LICENSE).

Construido con [Slint](https://slint.dev), usado bajo su *Royalty-free License 2.0* (la atribución está en *Ajustes → Acerca de*).

<a href="https://slint.dev"><img src="https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png" alt="Made with Slint" height="48"></a>
