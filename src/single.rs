//! Instancia única: si Swiftbeat ya está abierta, se trae su ventana al frente
//! en lugar de abrir otra.

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn IsIconic(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn acquire(wait: bool) -> bool {
        let name = wide("Local\\Swiftbeat.SingleInstance");
        // Al reiniciar, la instancia anterior tarda un momento en cerrarse.
        let tries = if wait { 50 } else { 1 };
        for i in 0..tries {
            // SAFETY: `name` es UTF-16 terminado en cero. El mutex se mantiene abierto
            // (a propósito) hasta que termina el proceso.
            let (handle, err) = unsafe { (CreateMutexW(std::ptr::null(), 0, name.as_ptr()), GetLastError()) };
            if handle == 0 || err != ERROR_ALREADY_EXISTS {
                return true;
            }
            // Hay que soltar este identificador: mientras siga abierto, el mutex
            // existe y nunca se podría adquirir.
            // SAFETY: `handle` es un identificador válido devuelto por CreateMutexW.
            unsafe {
                CloseHandle(handle);
            }
            if i + 1 < tries {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        let class = wide("Window Class");
        let title = wide("Swiftbeat");
        // SAFETY: cadenas UTF-16 terminadas en cero; el HWND solo se usa si existe.
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
            if hwnd != 0 {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd);
            }
        }
        false
    }
}

/// Argumento con el que se abre la instancia nueva al pulsar «Reiniciar».
pub const RESTART_ARG: &str = "--restart";

/// Devuelve `false` si ya había otra instancia (y la ha traído al frente). Con
/// `wait`, espera unos segundos a que la otra se cierre antes de rendirse.
pub fn acquire(wait: bool) -> bool {
    #[cfg(windows)]
    {
        win::acquire(wait)
    }
    #[cfg(not(windows))]
    {
        let _ = wait;
        true
    }
}
