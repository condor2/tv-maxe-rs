//! A thin wrapper over libmpv's OpenGL render API.
//!
//! libmpv2's own `RenderContext` is not used because
//!
//! * its `render()` cannot pass `BLOCK_FOR_TARGET_TIME`. By default libmpv
//!   makes `render()` wait until the frame's display time, which here is the
//!   GTK main thread: the whole interface froze for seconds after a channel
//!   change or a pause. We draw when GTK asks, so we tell libmpv not to wait.
//! * it frees the update callback *before* the render context, while mpv's video
//!   thread can still call it (use-after-free when the window closes). Here
//!   the context is freed first.

use super::gl;
use libmpv2::Mpv;
use libmpv2_sys as sys;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

pub struct RenderContext {
    ctx: *mut sys::mpv_render_context,
    /// The update callback, kept alive until the context is freed.
    callback: *mut Box<dyn Fn() + Send>,
}

unsafe extern "C" fn proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: mpv passes a valid, NUL-terminated function name.
    match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(name) => gl::get_proc_address(&(), name),
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "C" fn update_trampoline(ctx: *mut c_void) {
    // SAFETY: `ctx` is the `callback` pointer given to mpv, alive until the
    // render context is freed.
    let callback = unsafe { &*(ctx as *const Box<dyn Fn() + Send>) };
    callback();
}

fn param(type_: sys::mpv_render_param_type, data: *mut c_void) -> sys::mpv_render_param {
    sys::mpv_render_param { type_, data }
}

impl RenderContext {
    /// Creates the context for the current OpenGL context. `on_update` is
    /// called from mpv's threads when a new frame is ready to be drawn.
    pub fn new(mpv: &Mpv, on_update: impl Fn() + Send + 'static) -> Result<Self, String> {
        let mut init = sys::mpv_opengl_init_params {
            get_proc_address: Some(proc_address),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut params = [
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                ptr::addr_of_mut!(init).cast(),
            ),
            param(0, ptr::null_mut()),
        ];
        let mut ctx = ptr::null_mut();
        // SAFETY: the parameters live through the call; mpv copies what it keeps.
        let err = unsafe {
            sys::mpv_render_context_create(&mut ctx, mpv.ctx.as_ptr(), params.as_mut_ptr())
        };
        if err < 0 || ctx.is_null() {
            return Err(format!("mpv_render_context_create failed (error {err})"));
        }
        let callback: *mut Box<dyn Fn() + Send> = Box::into_raw(Box::new(Box::new(on_update)));
        // SAFETY: `callback` stays valid until `Drop`, after the context is freed.
        unsafe {
            sys::mpv_render_context_set_update_callback(
                ctx,
                Some(update_trampoline),
                callback.cast(),
            );
        }
        Ok(Self { ctx, callback })
    }

    /// Must be called before every `render()`: lets mpv's video thread
    /// continue with the next frame.
    pub fn update(&self) {
        // SAFETY: `ctx` is valid until `Drop`.
        unsafe { sys::mpv_render_context_update(self.ctx) };
    }

    /// Draws the current frame into `fbo` (size in pixels), without waiting for
    /// the frame's display time.
    pub fn render(&self, fbo: i32, width: i32, height: i32) -> Result<(), String> {
        let mut target = sys::mpv_opengl_fbo {
            fbo,
            w: width,
            h: height,
            internal_format: 0,
        };
        // GtkGLArea's framebuffer has its origin at the bottom left.
        let mut flip_y: c_int = 1;
        let mut block: c_int = 0;
        let mut params = [
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                ptr::addr_of_mut!(target).cast(),
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                ptr::addr_of_mut!(flip_y).cast(),
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                ptr::addr_of_mut!(block).cast(),
            ),
            param(0, ptr::null_mut()),
        ];
        // SAFETY: the parameters live through the call; the GL context is current.
        let err = unsafe { sys::mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
        if err < 0 {
            return Err(format!("mpv_render_context_render failed (error {err})"));
        }
        Ok(())
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        // SAFETY: free first (it stops the callbacks), then the callback.
        unsafe {
            sys::mpv_render_context_free(self.ctx);
            drop(Box::from_raw(self.callback));
        }
    }
}
