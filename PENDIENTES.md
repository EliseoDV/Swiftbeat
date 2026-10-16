# Pendientes

## ✅ La UI no se repinta al volver a la ventana (resuelto en 0.1.1)

Implementado en `src/repaint.rs` y en la capa invisible de `ui/app.slint`. A diferencia del plan de abajo:

- Solo se repinta al volver a primer plano (`WM_ACTIVATE`) o al restaurar desde minimizada (`WM_SIZE`). El caso opcional de `WM_PAINT` se descartó.
- `State.repaint` es un contador (`int`) en lugar de un `bool`, para que dos peticiones seguidas no se anulen.

### Síntomas

- Se minimiza Swiftbeat, o se deja tapada por otra app (navegador, Roblox), con música sonando.
- Al volver un rato después (Alt+Tab o clic en la barra de tareas), la UI sale vacía o a medias.
- Al pasar el ratón por las canciones de una playlist, solo aparecen esas filas.
- Cambiar el tamaño de la ventana lo arregla todo.
- La reproducción nunca se detiene: el fallo es solo visual.

### Causa

Es un fallo de Slint 1.18.1 con el renderizado parcial: solo se repinta lo que cambió y se da por hecho que el resto sigue en pantalla.

1. Al minimizar, Slint vacía su caché para repintar todo al volver (`occluded()` en `i-slint-backend-winit-1.18.1/renderer/sw.rs`, llamado desde el `Resized` de tamaño cero en `winitwindowadapter.rs`).
2. La música sigue sonando y el `Tick` de 500 ms (`src/backend.rs:274`) actualiza la barra de progreso. Ese primer fotograma se dibuja completo **mientras la ventana está minimizada**: gasta el vaciado de la caché y Windows lo descarta.
3. Al restaurar, Windows envía `WM_PAINT`, pero Slint cree que todo está al día y solo repinta lo que haya cambiado desde entonces, como la barra de progreso o las filas bajo el ratón.
4. Al redimensionar se crea un buffer nuevo y Slint lo dibuja todo; por eso se arregla.

Cuando otra app tapa la ventana sin minimizarla, es peor: winit nunca emite `Occluded` en Windows, así que la caché ni siquiera se vacía.

### Arreglo previsto: forzar un repintado completo al volver

No hace falta la feature `unstable-winit-030` de Slint. Se usa la API de Win32 directamente, igual que ya hacen `src/single.rs` y `src/media.rs`.

1. **Capa invisible en la UI.** En `ui/app.slint`, encima de todo, añadir un `Rectangle` que ocupe la ventana completa, sin eventos de entrada, cuyo `background` dependa de una propiedad nueva `State.repaint` (`bool`).
   - El color alterna entre `transparent` y uno casi transparente, como `#00000001`.
   - Al cambiar esa propiedad, Slint marca toda la ventana como sucia y la repinta entera.
   - Comprobar que de verdad provoca el repintado completo; si no, probar con otra propiedad de dibujo, como `opacity`.
2. **Detectar la vuelta a la ventana.** Nuevo módulo `src/repaint.rs` que subclasifique la ventana con `SetWindowSubclass` (comctl32).
   - El `HWND` se obtiene igual que en `media.rs`, con `window_handle()` y un reintento si la ventana aún no existe.
   - Mensajes a interceptar:
     - `WM_ACTIVATE` con `WA_ACTIVE` o `WA_CLICKACTIVE`: vuelta con Alt+Tab o con clic.
     - `WM_SIZE` con `SIZE_RESTORED` o `SIZE_MAXIMIZED`: vuelta desde minimizado.
     - Opcional: `WM_PAINT` cuando `GetUpdateRect` indique que Windows invalidó la ventana, por si el sistema descarta su contenido estando tapada.
   - En esos casos, invertir `State.repaint` y llamar siempre a `DefSubclassProc` para que winit siga recibiendo el mensaje.
3. **No tocar Slint dentro del procedimiento de ventana.** Cambiar la propiedad con `slint::Timer::single_shot(Duration::ZERO, ...)` para evitar reentradas mientras Slint está a mitad de un evento.

**Coste:** un repintado completo cada vez que se vuelve a la ventana, de unos pocos milisegundos. No afecta a la reproducción, la red ni los datos. Si fallara, el comportamiento sería el mismo que ahora.

### Cómo comprobarlo

1. Poner una canción y abrir una playlist larga.
2. Minimizar Swiftbeat durante un minuto y restaurarla: la UI debe verse completa.
3. Dejarla tapada por el navegador o Roblox varios minutos y volver con Alt+Tab: la UI debe verse completa.
4. Comprobar en el Administrador de tareas que la CPU en reposo no ha subido.

### Otras ideas descartadas por ahora

- **No dibujar mientras está minimizada:** arregla el caso de minimizar, pero no el de quedar tapada.
- **Usar `slint::winit_030`:** funcionaría, pero su API puede cambiar entre versiones de Slint.
- **Reportarlo a Slint:** lo ideal es que repinten todo cuando Windows invalida la ventana. Conviene abrir un issue con estos pasos para reproducirlo.
