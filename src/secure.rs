//! Datos de sesión cifrados con DPAPI de Windows: solo el mismo usuario, en el
//! mismo equipo, puede descifrarlos. Fuera de Windows se guardan tal cual.

use std::{fs, path::Path};

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    fn run(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input = Blob {
            len: u32::try_from(data.len()).ok()?,
            data: data.as_ptr().cast_mut(),
        };
        let mut output = Blob {
            len: 0,
            data: std::ptr::null_mut(),
        };
        use std::ptr::{null, null_mut};
        let flags = CRYPTPROTECT_UI_FORBIDDEN;
        // SAFETY: `input` apunta a `data`, que vive durante la llamada, y DPAPI no
        // lo modifica. `output` lo reserva Windows y se libera abajo.
        let ok = unsafe {
            if protect {
                CryptProtectData(&input, null(), null(), null(), null(), flags, &mut output)
            } else {
                CryptUnprotectData(&input, null_mut(), null(), null(), null(), flags, &mut output)
            }
        };
        if ok == 0 || output.data.is_null() {
            return None;
        }
        // SAFETY: Windows devolvió `output.len` bytes válidos en `output.data`
        // (reservados con LocalAlloc). Se copian, se borran y se liberan.
        unsafe {
            let out = std::slice::from_raw_parts(output.data, output.len as usize).to_vec();
            std::ptr::write_bytes(output.data, 0, output.len as usize);
            LocalFree(output.data.cast());
            Some(out)
        }
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, true)
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, false)
    }
}

#[cfg(windows)]
use win::{protect, unprotect};

#[cfg(not(windows))]
fn protect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

/// Cifra `data` y lo guarda en `path`. Devuelve si se pudo.
pub fn write(path: &Path, data: &[u8]) -> bool {
    let Some(sealed) = protect(data) else {
        log::warn!("no se pudo cifrar {}", path.display());
        return false;
    };
    match fs::write(path, sealed) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("no se pudo guardar {}: {e}", path.display());
            false
        }
    }
}

/// Lee y descifra `path`. `None` si no existe o no se puede descifrar.
pub fn read(path: &Path) -> Option<Vec<u8>> {
    unprotect(&fs::read(path).ok()?)
}
