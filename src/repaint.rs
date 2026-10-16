//! Repintado completo al volver a la ventana.
//!
//! El renderizado parcial de Slint solo repinta lo que cambió. Mientras la ventana
//! está minimizada o tapada, la barra de progreso sigue avanzando y Slint da por
//! dibujado un fotograma que Windows descarta; al volver, solo aparecían las zonas
//! que cambiaban después. Aquí se detecta la vuelta y se cambia `State.repaint`,
//! que obliga a repintar la capa que cubre toda la ventana (ver `ui/app.slint`).

use crate::AppWindow;

#[cfg(windows)]
mod win {
    use crate::{AppWindow, State};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::ComponentHandle;
    use std::{
        cell::{Cell, RefCell},
        time::Duration,
    };

    type SubclassProc = unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize;

    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn SetWindowSubclass(hwnd: isize, subclass: SubclassProc, id: usize, data: usize) -> i32;
        fn DefSubclassProc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    const WM_SIZE: u32 = 0x0005;
    const WM_ACTIVATE: u32 = 0x0006;
    const WA_INACTIVE: usize = 0;
    const SIZE_RESTORED: usize = 0;
    const SIZE_MINIMIZED: usize = 1;
    const SIZE_MAXIMIZED: usize = 2;

    thread_local! {
        static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
        static MINIMIZED: Cell<bool> = const { Cell::new(false) };
        static PENDING: Cell<bool> = const { Cell::new(false) };
    }

    /// Subclasifica la ventana en cuanto exista. Con winit la ventana se crea al
    /// arrancar el bucle de eventos, así que se reintenta.
    pub fn install(app: slint::Weak<AppWindow>) {
        slint::Timer::single_shot(Duration::ZERO, move || attempt(app, 0));
    }

    fn attempt(app: slint::Weak<AppWindow>, tries: u32) {
        let Some(ui) = app.upgrade() else { return };
        APP.set(Some(app.clone()));
        match subclass(&ui) {
            Ok(()) => log::info!("repintado al volver a la ventana activo"),
            Err(e) if tries < 50 => {
                log::debug!("repintado al volver aún no disponible: {e}");
                slint::Timer::single_shot(Duration::from_millis(100), move || attempt(app, tries + 1));
            }
            Err(e) => log::warn!("repintado al volver: {e}"),
        }
    }

    fn subclass(app: &AppWindow) -> Result<(), String> {
        let window = app.window().window_handle();
        let handle = window.window_handle().map_err(|e| format!("sin ventana nativa: {e}"))?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return Err("la ventana no es Win32".into());
        };
        // SAFETY: el HWND es el de nuestra ventana y `window_proc` vive todo el programa.
        if unsafe { SetWindowSubclass(win32.hwnd.get(), window_proc, 1, 0) } == 0 {
            return Err("SetWindowSubclass falló".into());
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        _id: usize,
        _data: usize,
    ) -> isize {
        match msg {
            // Vuelve desde minimizada.
            WM_SIZE => match wparam {
                SIZE_MINIMIZED => MINIMIZED.set(true),
                SIZE_RESTORED | SIZE_MAXIMIZED if MINIMIZED.replace(false) => schedule(),
                _ => {}
            },
            // Vuelve a primer plano (Alt+Tab o clic). Si aún está minimizada, ya lo pedirá
            // WM_SIZE al restaurarse; pedirlo ahora se gastaría en un fotograma descartado.
            WM_ACTIVATE if wparam & 0xFFFF != WA_INACTIVE && (wparam >> 16) & 0xFFFF == 0 => schedule(),
            _ => {}
        }
        // SAFETY: se reenvía el mensaje tal cual al procedimiento de winit.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    /// Fuera del procedimiento de ventana, para no tocar Slint a mitad de un evento.
    /// Varias peticiones seguidas se agrupan en un solo repintado.
    fn schedule() {
        if PENDING.replace(true) {
            return;
        }
        let _ = slint::invoke_from_event_loop(|| {
            PENDING.set(false);
            APP.with_borrow(|app| {
                if let Some(ui) = app.as_ref().and_then(|w| w.upgrade()) {
                    let state = ui.global::<State>();
                    state.set_repaint(state.get_repaint().wrapping_add(1));
                }
            });
        });
    }
}

pub fn install(app: slint::Weak<AppWindow>) {
    #[cfg(windows)]
    win::install(app);
    #[cfg(not(windows))]
    let _ = app;
}
