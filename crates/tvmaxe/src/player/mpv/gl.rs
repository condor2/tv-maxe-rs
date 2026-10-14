//! Resolving OpenGL functions for libmpv.
//!
//! GTK 4 creates the GL context (EGL on Wayland and usually on X11 too; GLX
//! as a fallback). mpv needs a `get_proc_address` for the current context,
//! so EGL and GLX are loaded dynamically, without linking to libGL/libEGL.

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
            // SAFETY: the system GL/EGL libraries run no unsafe code when loaded.
            let Ok(library) = (unsafe { Library::new(lib) }) else {
                continue;
            };
            // SAFETY: the signature matches the EGL/GLX API.
            if let Ok(f) = unsafe { library.get::<GetProcAddr>(sym).map(|s| *s) } {
                procs.push((library, f));
            }
        }
        Loader { procs }
    })
}

/// Passed to `OpenGLInitParams::get_proc_address`.
pub fn get_proc_address(_ctx: &(), name: &str) -> *mut c_void {
    let Ok(cname) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    for (_, f) in &loader().procs {
        // SAFETY: called from the mpv callback, with the GLArea context current.
        let p = unsafe { f(cname.as_ptr()) };
        if !p.is_null() {
            return p;
        }
    }
    std::ptr::null_mut()
}

/// `true` if GL is emulated on the CPU (llvmpipe, softpipe, swrast):
/// there mpv's quality scaling is too slow for live video.
pub fn is_software_renderer() -> bool {
    const GL_RENDERER: u32 = 0x1F01;
    type GetString = unsafe extern "C" fn(u32) -> *const c_char;
    let p = get_proc_address(&(), "glGetString");
    if p.is_null() {
        return false;
    }
    // SAFETY: non-null pointer to glGetString; the GL context is current (realize).
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

/// The FBO bound by GtkGLArea during the `render` signal (not 0).
pub fn current_draw_fbo() -> i32 {
    const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
    type GetIntegerv = unsafe extern "C" fn(u32, *mut i32);

    static GET_INTEGERV: OnceLock<Option<GetIntegerv>> = OnceLock::new();
    let f = GET_INTEGERV.get_or_init(|| {
        let p = get_proc_address(&(), "glGetIntegerv");
        // SAFETY: non-null pointer to glGetIntegerv.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, GetIntegerv>(p) })
    });

    let mut fbo = 0;
    if let Some(f) = f {
        // SAFETY: the GL context is current (we are in the `render` handler).
        unsafe { f(GL_DRAW_FRAMEBUFFER_BINDING, &mut fbo) };
    }
    fbo
}
