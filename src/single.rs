//! Instancia única: si Swiftbeat ya está abierta, se trae su ventana al frente
//! en lugar de abrir otra.

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
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

    pub fn acquire() -> bool {
        let name = wide("Local\\Swiftbeat.SingleInstance");
        // SAFETY: `name` es UTF-16 terminado en cero. El mutex se mantiene abierto
        // (a propósito) hasta que termina el proceso.
        let (handle, err) = unsafe { (CreateMutexW(std::ptr::null(), 0, name.as_ptr()), GetLastError()) };
        if handle == 0 || err != ERROR_ALREADY_EXISTS {
            return true;
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

/// Devuelve `false` si ya había otra instancia (y la ha traído al frente).
pub fn acquire() -> bool {
    #[cfg(windows)]
    {
        win::acquire()
    }
    #[cfg(not(windows))]
    {
        true
    }
}
