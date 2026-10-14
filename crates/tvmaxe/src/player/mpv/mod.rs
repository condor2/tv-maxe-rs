//! libmpv backend: rendering through the render API into a `GtkGLArea`
//! (works the same on Wayland and X11).

mod gl;
pub mod recorder;

use super::{Backend, BackendEvent, BackendHandler, PlaybackSettings, StreamOptions, VideoQuality};
use gtk::glib;
use gtk::prelude::*;
use libmpv2::Mpv;
use libmpv2::events::Event;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tvmaxe_core::i18n::{fill, tr};

/// Above this ratio of dropped frames we switch to the fast profile.
const MAX_DROP_RATIO: f64 = 0.3;

pub struct MpvBackend {
    area: gtk::GLArea,
    mpv: &'static Mpv,
    handler: RefCell<Option<BackendHandler>>,
    /// A URL is loaded (for buffering and frame statistics).
    loaded: Cell<bool>,
    buffering: Cell<Option<i64>>,
    /// mpv's "fast" profile (bilinear scaling) is active.
    fast: Rc<Cell<bool>>,
    /// The choice from Preferences.
    quality: Rc<Cell<VideoQuality>>,
    /// The User-Agent from Preferences (or the default one).
    user_agent: RefCell<String>,
    /// Last dropped-frame measurement: (counter, instant).
    drops: Cell<Option<(i64, Instant)>>,
}

impl MpvBackend {
    pub fn new() -> anyhow::Result<Rc<Self>> {
        // GTK applies the system language, including the number format (Romanian
        // uses "1,5"). libmpv refuses to start if LC_NUMERIC is not "C", because
        // it parses its numeric options with a decimal point.
        // SAFETY: called from the main thread, before any other mpv thread starts.
        unsafe {
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            init.set_property("hwdec", "auto-safe")?;
            init.set_property("ytdl", false)?;
            init.set_property("cache", "yes")?;
            init.set_property("demuxer-max-back-bytes", "32MiB")?;
            init.set_property("network-timeout", 15i64)?;
            init.set_property("keep-open", "no")?;
            init.set_property("user-agent", concat!("TV-Maxe/", env!("CARGO_PKG_VERSION")))?;
            // Depanare: TVMAXE_MPV_LOG=/tmp/mpv.log tvmaxe
            if let Ok(path) = std::env::var("TVMAXE_MPV_LOG") {
                init.set_property("log-file", path)?;
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("{}", fill(tr("cannot initialise libmpv (%1)"), &[&e])))?;
        // RenderContext<'a> borrows Mpv; the backend lives as long as the application.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));

        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_has_depth_buffer(false);
        area.set_has_stencil_buffer(false);

        let render: Rc<RefCell<Option<RenderContext<'static>>>> = Rc::default();
        let fast: Rc<Cell<bool>> = Rc::default();
        let quality: Rc<Cell<VideoQuality>> = Rc::default();
        let (redraw_tx, redraw_rx) = async_channel::bounded::<()>(1);

        // The mpv thread asks for a redraw → GTK main loop.
        glib::spawn_future_local(glib::clone!(
            #[weak]
            area,
            async move {
                while redraw_rx.recv().await.is_ok() {
                    area.queue_render();
                }
            }
        ));

        area.connect_realize(glib::clone!(
            #[strong]
            render,
            #[strong]
            fast,
            #[strong]
            quality,
            move |area| {
                area.make_current();
                if let Some(err) = area.error() {
                    eprintln!("GLArea: {err}");
                    return;
                }
                // On software-emulated GL, quality scaling cannot keep up with 25 fps.
                match quality.get() {
                    VideoQuality::Fast => apply_fast_profile(mpv, &fast, "chosen in Preferences"),
                    VideoQuality::Auto if gl::is_software_renderer() => {
                        apply_fast_profile(mpv, &fast, "software OpenGL rendering")
                    }
                    _ => {}
                }
                let ctx = mpv.create_render_context(vec![
                    RenderParam::ApiType(RenderParamApiType::OpenGl),
                    RenderParam::InitParams(OpenGLInitParams {
                        get_proc_address: gl::get_proc_address,
                        ctx: (),
                    }),
                ]);
                match ctx {
                    Ok(mut ctx) => {
                        let tx = redraw_tx.clone();
                        ctx.set_update_callback(move || {
                            let _ = tx.try_send(());
                        });
                        *render.borrow_mut() = Some(ctx);
                    }
                    Err(e) => eprintln!("mpv render context: {e}"),
                }
            }
        ));

        area.connect_unrealize(glib::clone!(
            #[strong]
            render,
            move |area| {
                area.make_current();
                if let Some(mut ctx) = render.borrow_mut().take() {
                    // libmpv2 frees the callback *before*
                    // mpv_render_context_free, while mpv's "vo" thread
                    // can still call it (use-after-free → crash when the
                    // window closes). Replace it first with a non-capturing
                    // closure: mpv no longer calls the old callback after the
                    // replacement, and the new one is zero-sized, so calling
                    // it reads no freed memory.
                    ctx.set_update_callback(|| {});
                    drop(ctx);
                }
            }
        ));

        area.connect_render(glib::clone!(
            #[strong]
            render,
            move |area, _| {
                if let Some(ctx) = render.borrow().as_ref() {
                    let s = area.scale_factor();
                    if let Err(e) = ctx.render::<()>(
                        gl::current_draw_fbo(),
                        area.width() * s,
                        area.height() * s,
                        true,
                    ) {
                        eprintln!("mpv render: {e}");
                    }
                }
                glib::Propagation::Stop
            }
        ));

        let backend = Rc::new(Self {
            area,
            mpv,
            handler: RefCell::default(),
            loaded: Cell::new(false),
            buffering: Cell::new(None),
            fast,
            quality,
            user_agent: RefCell::new(tvmaxe_core::net::DEFAULT_USER_AGENT.to_string()),
            drops: Cell::new(None),
        });
        backend.spawn_event_poll();
        Ok(backend)
    }

    fn emit(&self, ev: BackendEvent) {
        if let Some(f) = self.handler.borrow().as_ref() {
            f(ev);
        }
    }

    /// mpv events, read without blocking from the main loop.
    fn spawn_event_poll(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(b) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            while let Some(ev) = b.mpv.wait_event(0.0) {
                let out = match ev {
                    // Errors from EndFile arrive here as Err.
                    Err(e) => Some(BackendEvent::Failed(describe_error(&e))),
                    Ok(Event::StartFile) => Some(BackendEvent::Connecting),
                    Ok(Event::PlaybackRestart) => Some(BackendEvent::Playing),
                    // 0 = end of file: the live stream closed.
                    Ok(Event::EndFile(0)) => Some(BackendEvent::Ended),
                    Ok(Event::Shutdown) => return glib::ControlFlow::Break,
                    Ok(_) => None,
                };
                if let Some(out) = out {
                    if matches!(out, BackendEvent::Failed(_) | BackendEvent::Ended) {
                        b.loaded.set(false);
                    }
                    b.emit(out);
                }
            }
            if b.loaded.get() {
                b.check_frame_drops();
                b.check_buffering();
            }
            glib::ControlFlow::Continue
        });
    }

    fn check_buffering(&self) {
        let waiting = self
            .mpv
            .get_property::<bool>("paused-for-cache")
            .unwrap_or(false);
        let state = waiting.then(|| {
            self.mpv
                .get_property::<i64>("cache-buffering-state")
                .unwrap_or(0)
        });
        if state != self.buffering.get() {
            self.buffering.set(state);
            self.emit(state.map_or(BackendEvent::Playing, BackendEvent::Buffering));
        }
    }

    /// If the system cannot keep up (mpv drops many frames), switch to the fast
    /// profile so the screen does not stay black on slow computers.
    fn check_frame_drops(&self) {
        if self.fast.get() || self.quality.get() != VideoQuality::Auto {
            return;
        }
        let Ok(count) = self.mpv.get_property::<i64>("frame-drop-count") else {
            return;
        };
        let now = Instant::now();
        let Some((prev, since)) = self.drops.get() else {
            self.drops.set(Some((count, now)));
            return;
        };
        let secs = now.duration_since(since).as_secs_f64();
        if secs < 3.0 {
            return;
        }
        self.drops.set(Some((count, now)));
        let fps = self
            .mpv
            .get_property::<f64>("estimated-vf-fps")
            .unwrap_or(25.0);
        let expected = fps * secs;
        if expected > 0.0 && (count - prev) as f64 / expected > MAX_DROP_RATIO {
            apply_fast_profile(self.mpv, &self.fast, "too many dropped frames");
        }
    }
}

impl Backend for MpvBackend {
    fn widget(&self) -> gtk::Widget {
        self.area.clone().upcast()
    }

    fn set_event_handler(&self, handler: BackendHandler) {
        *self.handler.borrow_mut() = Some(handler);
    }

    fn load(&self, url: &str, http: &StreamOptions) -> Result<(), String> {
        apply_http(self.mpv, http, &self.user_agent.borrow());
        self.loaded.set(true);
        self.buffering.set(None);
        self.drops.set(None);
        let _ = self.mpv.set_property("pause", false);
        self.mpv
            .command("loadfile", &[url, "replace"])
            .map_err(|e| fill(tr("Cannot start the stream: %1"), &[&e]))
    }

    fn stop(&self) {
        self.loaded.set(false);
        let _ = self.mpv.command("stop", &[]);
    }

    fn set_paused(&self, paused: bool) {
        let _ = self.mpv.set_property("pause", paused);
    }

    fn is_paused(&self) -> bool {
        self.mpv.get_property::<bool>("pause").unwrap_or(false)
    }

    fn set_volume(&self, percent: f64) {
        let _ = self.mpv.set_property("volume", percent);
    }

    fn apply_settings(&self, s: &PlaybackSettings) {
        let m = self.mpv;
        let _ = m.set_property(
            "hwdec",
            if s.hardware_decoding {
                "auto-safe"
            } else {
                "no"
            },
        );
        let _ = m.set_property("cache-secs", f64::from(s.cache_secs));
        let _ = m.set_property("network-timeout", f64::from(s.network_timeout_secs));
        let ua = s
            .user_agent
            .as_deref()
            .unwrap_or(tvmaxe_core::net::DEFAULT_USER_AGENT);
        let _ = m.set_property("user-agent", ua);
        *self.user_agent.borrow_mut() = ua.to_string();
        self.quality.set(s.video_quality);
        // Switching to "fast" works at any time; switching back only on restart
        // (the mpv profile has no undo).
        if s.video_quality == VideoQuality::Fast {
            apply_fast_profile(m, &self.fast, "chosen in Preferences");
        }
    }

    fn position(&self) -> Option<f64> {
        self.mpv.get_property::<f64>("playback-time").ok()
    }
}

fn apply_fast_profile(mpv: &Mpv, fast: &Cell<bool>, why: &str) {
    if !fast.replace(true) {
        eprintln!("TV-Maxe: fast video profile ({why})");
        let _ = mpv.command("apply-profile", &["fast"]);
    }
}

/// Per-channel HTTP headers, set before every URL; a channel without special
/// headers goes back to `default_ua` (from Preferences). The Referer goes as an
/// extra header so it is not sent at all (not even empty) when the channel
/// does not need it.
fn apply_http(mpv: &Mpv, http: &StreamOptions, default_ua: &str) {
    let _ = mpv.command("change-list", &["http-header-fields", "clr", ""]);
    if let Some(r) = http.referrer.as_deref().filter(|r| !r.is_empty()) {
        let _ = mpv.command(
            "change-list",
            &["http-header-fields", "append", &format!("Referer: {r}")],
        );
    }
    let ua = http.user_agent.as_deref().unwrap_or(default_ua);
    let _ = mpv.set_property("user-agent", ua);
}

/// `mpv_error` codes (mpv/client.h) → readable messages.
fn describe_error(e: &libmpv2::Error) -> String {
    match e {
        libmpv2::Error::Raw(code) => match *code {
            -13 => tr("The channel cannot be loaded (offline or wrong URL)").into(),
            -14 => tr("Cannot initialise audio output").into(),
            -15 => tr("Cannot initialise video output").into(),
            -16 => tr("Unknown stream format").into(),
            -17 => tr("Stream without playable audio or video").into(),
            other => fill(tr("Playback failed (mpv code %1)"), &[&other]),
        },
        other => fill(tr("Playback failed: %1"), &[other]),
    }
}
