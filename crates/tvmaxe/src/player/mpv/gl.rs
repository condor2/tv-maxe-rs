//! Rezolvarea funcțiilor OpenGL pentru libmpv.
//!
//! GTK 4 creează contextul GL (EGL pe Wayland și, de regulă, și pe X11; GLX
//! ca rezervă). mpv are nevoie de un `get_proc_address` pentru contextul
//! curent, așa că încărcăm dinamic EGL și GLX, fără link la libGL/libEGL.

use libloading::Library;
use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;

type GetProcAddr = unsafe extern "C" fn(*const c_char) -> *mut c_void;

struct Loader {
    procs: Vec<(Library, GetProcAddr)>,
}

fn loader() -> &'static Loader {
    static LOADER: OnceLock<Loader> = OnceLock::new();
    LOADER.get_or_init(|| {
        let candidates: [(&str, &[u8]); 2] = [
            ("libEGL.so.1", b"eglGetProcAddress\0"),
            ("libGL.so.1", b"glXGetProcAddressARB\0"),
        ];
        let mut procs = Vec::new();
        for (lib, sym) in candidates {
            // SAFETY: bibliotecile de sistem GL/EGL nu rulează cod nesigur la încărcare.
            let Ok(library) = (unsafe { Library::new(lib) }) else {
                continue;
            };
            // SAFETY: semnătura corespunde API-ului EGL/GLX.
            if let Ok(f) = unsafe { library.get::<GetProcAddr>(sym).map(|s| *s) } {
                procs.push((library, f));
            }
        }
        Loader { procs }
    })
}

/// Pasat la `OpenGLInitParams::get_proc_address`.
pub fn get_proc_address(_ctx: &(), name: &str) -> *mut c_void {
    let Ok(cname) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    for (_, f) in &loader().procs {
        // SAFETY: apelat din callback-ul mpv, cu contextul GLArea curent.
        let p = unsafe { f(cname.as_ptr()) };
        if !p.is_null() {
            return p;
        }
    }
    std::ptr::null_mut()
}

/// `true` dacă GL-ul e emulat pe procesor (llvmpipe, softpipe, swrast):
/// acolo scalarea de calitate a mpv e prea lentă pentru video live.
pub fn is_software_renderer() -> bool {
    const GL_RENDERER: u32 = 0x1F01;
    type GetString = unsafe extern "C" fn(u32) -> *const c_char;
    let p = get_proc_address(&(), "glGetString");
    if p.is_null() {
        return false;
    }
    // SAFETY: pointer nenul către glGetString; contextul GL e curent (realize).
    let s = unsafe {
        let f = std::mem::transmute::<*mut c_void, GetString>(p);
        let r = f(GL_RENDERER);
        if r.is_null() {
            return false;
        }
        std::ffi::CStr::from_ptr(r).to_string_lossy().to_lowercase()
    };
    ["llvmpipe", "softpipe", "swrast", "software"]
        .iter()
        .any(|k| s.contains(k))
}

/// FBO-ul legat de GtkGLArea în timpul semnalului `render` (nu e 0).
pub fn current_draw_fbo() -> i32 {
    const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
    type GetIntegerv = unsafe extern "C" fn(u32, *mut i32);

    static GET_INTEGERV: OnceLock<Option<GetIntegerv>> = OnceLock::new();
    let f = GET_INTEGERV.get_or_init(|| {
        let p = get_proc_address(&(), "glGetIntegerv");
        // SAFETY: pointer nenul către glGetIntegerv.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, GetIntegerv>(p) })
    });

    let mut fbo = 0;
    if let Some(f) = f {
        // SAFETY: contextul GL e curent (suntem în handler-ul `render`).
        unsafe { f(GL_DRAW_FRAMEBUFFER_BINDING, &mut fbo) };
    }
    fbo
}
