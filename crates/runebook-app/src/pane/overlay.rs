//! Wayland note page.
//!
//! WebKitGTK cannot draw into a Wayland surface it did not create.
//! The page is its own layer-shell surface, placed over the preview column.
//! Both surfaces use the output scale, so the type stays sharp.
//!
//! A failure hides the page. It does not print a path.

use super::place::{Origin, sway_covered, sway_origin};

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{Bounds, Context, Entity, IntoElement, Pixels, Render, Window, canvas, div};
use gtk::gdk::prelude::*;
use gtk::prelude::*;

const EDGE_LEFT: i32 = 0;
const EDGE_RIGHT: i32 = 1;
const EDGE_TOP: i32 = 2;
const EDGE_BOTTOM: i32 = 3;
const LAYER_TOP: i32 = 2;
const KEYBOARD_NONE: i32 = 0;
const KEYBOARD_EXCLUSIVE: i32 = 1;
const ORIGIN_HOLD: Duration = Duration::from_millis(100);
const MAX_SIDE: i32 = 16_000;

/// The note page for a Wayland window.
pub(super) struct NoteOverlay {
    webview: wry::WebView,
    window: gtk::Window,
    wanted: Cell<bool>,
    rewrite: Cell<bool>,
    keyboard: Cell<i32>,
    placed: Cell<Option<(i32, i32, i32, i32)>>,
    monitor: Cell<Option<(i32, i32)>>,
    cache: Cell<Option<CachedOrigin>>,
}

#[derive(Clone, Copy)]
struct CachedOrigin {
    at: Instant,
    origin: Option<Origin>,
}

struct LayerApi {
    is_supported: unsafe extern "C" fn() -> i32,
    init: unsafe extern "C" fn(*mut c_void),
    set_layer: unsafe extern "C" fn(*mut c_void, i32),
    set_anchor: unsafe extern "C" fn(*mut c_void, i32, i32),
    set_margin: unsafe extern "C" fn(*mut c_void, i32, i32),
    set_exclusive: unsafe extern "C" fn(*mut c_void, i32),
    set_keyboard: unsafe extern "C" fn(*mut c_void, i32),
    set_monitor: unsafe extern "C" fn(*mut c_void, *mut c_void),
}

/// Open the page. `None` when GTK or the layer shell cannot start.
pub(super) fn open(
    builder: wry::WebViewBuilder<'static>,
    dark: bool,
    cx: &mut Context<crate::Root>,
) -> Option<Entity<NoteOverlay>> {
    if !ensure_gtk(cx) {
        return None;
    }
    let api = layer_api()?;
    // SAFETY: `api` points at `libgtk-layer-shell.so.0`, which stays loaded.
    // GTK is initialized on this thread. The call only reads the display.
    if unsafe { (api.is_supported)() } == 0 {
        return None;
    }
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_decorated(false);
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    window.set_accept_focus(true);
    let raw = gtk_ptr(&window);
    // SAFETY: `raw` is the window above. The layer calls run before the window is mapped.
    // The function pointers come from the loaded layer-shell library.
    unsafe {
        (api.init)(raw);
        (api.set_layer)(raw, LAYER_TOP);
        (api.set_exclusive)(raw, 0);
        (api.set_anchor)(raw, EDGE_LEFT, 1);
        (api.set_anchor)(raw, EDGE_TOP, 1);
        (api.set_anchor)(raw, EDGE_RIGHT, 0);
        (api.set_anchor)(raw, EDGE_BOTTOM, 0);
        (api.set_keyboard)(raw, KEYBOARD_NONE);
    }
    let webview =
        wry::WebViewBuilderExtUnix::build_gtk(builder.with_visible(false), &window).ok()?;
    super::page::allow_media_scheme(&webview);
    remember_page(&window);
    Some(cx.new(|_| NoteOverlay {
        webview,
        window,
        wanted: Cell::new(false),
        rewrite: Cell::new(false),
        keyboard: Cell::new(KEYBOARD_NONE),
        placed: Cell::new(None),
        monitor: Cell::new(None),
        cache: Cell::new(None),
    }))
}

impl NoteOverlay {
    pub(super) fn set_showing(&self, showing: bool, rewrite: bool) {
        self.wanted.set(showing);
        self.rewrite.set(rewrite);
        if !showing {
            self.hide_native();
        }
    }

    pub(super) fn eval(&self, script: &str) -> bool {
        self.webview.evaluate_script(script).is_ok()
    }

    fn place(&self, bounds: Bounds<Pixels>, window: &Window) {
        if !self.wanted.get() {
            self.hide_native();
            return;
        }
        let Some(origin) = self.origin(window) else {
            self.hide_native();
            return;
        };
        if origin.covered() || page_covered() {
            self.hide_native();
            return;
        }
        let x = origin.x.saturating_add(round_px(bounds.origin.x));
        let y = origin.y.saturating_add(round_px(bounds.origin.y));
        let w = round_px(bounds.size.width);
        let h = round_px(bounds.size.height);
        if w < 1 || h < 1 || w > MAX_SIDE || h > MAX_SIDE {
            self.hide_native();
            return;
        }
        self.move_to(origin, x, y, w, h);
    }

    fn move_to(&self, origin: Origin, x: i32, y: i32, w: i32, h: i32) {
        let Some(api) = layer_api() else {
            return;
        };
        let raw = gtk_ptr(&self.window);
        let next = (x, y, w, h);
        if self.placed.get() != Some(next) {
            // SAFETY: `raw` is this overlay's window. The process still owns it.
            unsafe {
                (api.set_margin)(raw, EDGE_LEFT, x);
                (api.set_margin)(raw, EDGE_TOP, y);
                (api.set_margin)(raw, EDGE_RIGHT, 0);
                (api.set_margin)(raw, EDGE_BOTTOM, 0);
            }
            self.window.set_size_request(w, h);
            self.window.resize(w, h);
            self.placed.set(Some(next));
        }
        self.apply_keyboard(api, raw);
        if !self.window.is_visible() {
            self.pin_monitor(api, origin);
            self.window.show_all();
        }
    }

    fn apply_keyboard(&self, api: &LayerApi, raw: *mut c_void) {
        let mode = if self.rewrite.get() {
            KEYBOARD_EXCLUSIVE
        } else {
            KEYBOARD_NONE
        };
        if self.keyboard.replace(mode) == mode {
            return;
        }
        // SAFETY: `raw` is this overlay's window, and `api` is the loaded library.
        unsafe { (api.set_keyboard)(raw, mode) };
    }

    fn pin_monitor(&self, api: &LayerApi, origin: Origin) {
        let output = (origin.output_x, origin.output_y);
        if self.monitor.get() == Some(output) {
            return;
        }
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let global_x = origin.output_x.saturating_add(origin.x);
        let global_y = origin.output_y.saturating_add(origin.y);
        for index in 0..display.n_monitors() {
            let Some(monitor) = display.monitor(index) else {
                continue;
            };
            let geom = monitor.geometry();
            let inside = global_x >= geom.x()
                && global_y >= geom.y()
                && global_x < geom.x().saturating_add(geom.width())
                && global_y < geom.y().saturating_add(geom.height());
            if !inside {
                continue;
            }
            // SAFETY: both pointers belong to GTK objects this process owns.
            // The window is not mapped yet. Layer shell reads the monitor once.
            unsafe { (api.set_monitor)(gtk_ptr(&self.window), monitor_ptr(&monitor)) };
            self.monitor.set(Some(output));
            return;
        }
    }

    fn origin(&self, window: &Window) -> Option<Origin> {
        let now = Instant::now();
        if let Some(cached) = self.cache.get()
            && now.saturating_duration_since(cached.at) < ORIGIN_HOLD
        {
            return cached.origin;
        }
        let size = window.bounds().size;
        let fresh = sway_origin(
            crate::icon::APP_ID,
            round_px(size.width),
            round_px(size.height),
        );
        let origin = fresh.or_else(|| self.cache.get().and_then(|cached| cached.origin));
        self.cache.set(Some(CachedOrigin { at: now, origin }));
        origin
    }

    fn hide_native(&self) {
        if self.window.is_visible() {
            self.window.hide();
        }
        self.placed.set(None);
    }
}

impl Drop for NoteOverlay {
    fn drop(&mut self) {
        forget_page(&self.window);
        self.window.hide();
    }
}

impl Render for NoteOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().clone();
        div().size_full().child(
            canvas(
                move |bounds, window, cx| {
                    view.update(cx, |overlay, _| overlay.place(bounds, window));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }
}

fn ensure_gtk(cx: &mut Context<crate::Root>) -> bool {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return false;
    }
    // GTK can leave an EGL context current. GPUI then fails to draw the window.
    release_egl();
    drive_gtk(cx);
    true
}

fn drive_gtk(cx: &mut Context<crate::Root>) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |_root, cx| {
        let mut next_cover = Instant::now();
        loop {
            // A page that always has events must not keep the main thread.
            for _ in 0..32 {
                if !gtk::events_pending() {
                    break;
                }
                gtk::main_iteration_do(false);
            }
            release_egl();
            // The page is a layer above other windows. Hide it when another
            // window is focused. GPUI may stop painting after focus leaves.
            if Instant::now() >= next_cover {
                hide_when_covered();
                next_cover = Instant::now() + ORIGIN_HOLD;
            }
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
        }
    })
    .detach();
}

thread_local! {
    static PAGES: RefCell<Vec<gtk::Window>> = const { RefCell::new(Vec::new()) };
}

static COVERED: AtomicBool = AtomicBool::new(false);

fn remember_page(window: &gtk::Window) {
    PAGES.with(|pages| pages.borrow_mut().push(window.clone()));
}

fn forget_page(window: &gtk::Window) {
    PAGES.with(|pages| {
        pages
            .borrow_mut()
            .retain(|page| page.as_ptr() != window.as_ptr());
    });
}

fn page_covered() -> bool {
    COVERED.load(Ordering::Relaxed)
}

fn hide_when_covered() {
    let Some(covered) = sway_covered(crate::icon::APP_ID, 0, 0) else {
        return;
    };
    COVERED.store(covered, Ordering::Relaxed);
    if !covered {
        return;
    }
    PAGES.with(|pages| {
        for window in pages.borrow().iter() {
            if window.is_visible() {
                window.hide();
            }
        }
    });
}

fn release_egl() {
    static FUNCS: std::sync::OnceLock<Option<EglFns>> = std::sync::OnceLock::new();
    let Some(fns) = FUNCS.get_or_init(load_egl).as_ref() else {
        return;
    };
    // SAFETY: both pointers come from libEGL. A null display means no context is current.
    // Releasing it lets GPUI bind its own context on this thread.
    unsafe {
        let display = (fns.current_display)();
        if !display.is_null() {
            (fns.make_current)(
                display,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }
}

struct EglFns {
    current_display: unsafe extern "C" fn() -> *mut c_void,
    make_current: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> i32,
}

fn load_egl() -> Option<EglFns> {
    let soname = c"libEGL.so.1";
    // SAFETY: the soname is the system EGL library. The handle stays open.
    // The two symbols are the EGL 1.4 entry points of those names.
    unsafe {
        let handle = dlopen(soname.as_ptr(), 1);
        if handle.is_null() {
            return None;
        }
        Some(EglFns {
            current_display: symbol(handle, c"eglGetCurrentDisplay")?,
            make_current: symbol(handle, c"eglMakeCurrent")?,
        })
    }
}

fn layer_api() -> Option<&'static LayerApi> {
    static API: std::sync::OnceLock<Option<LayerApi>> = std::sync::OnceLock::new();
    API.get_or_init(load_layer).as_ref()
}

fn load_layer() -> Option<LayerApi> {
    let soname = c"libgtk-layer-shell.so.0";
    // SAFETY: the soname is a fixed library name. The handle stays open for the process.
    // Each symbol matches the gtk-layer-shell C header. A null symbol returns `None`.
    unsafe {
        let handle = dlopen(soname.as_ptr(), 1);
        if handle.is_null() {
            return None;
        }
        Some(LayerApi {
            is_supported: symbol(handle, c"gtk_layer_is_supported")?,
            init: symbol(handle, c"gtk_layer_init_for_window")?,
            set_layer: symbol(handle, c"gtk_layer_set_layer")?,
            set_anchor: symbol(handle, c"gtk_layer_set_anchor")?,
            set_margin: symbol(handle, c"gtk_layer_set_margin")?,
            set_exclusive: symbol(handle, c"gtk_layer_set_exclusive_zone")?,
            set_keyboard: symbol(handle, c"gtk_layer_set_keyboard_mode")?,
            set_monitor: symbol(handle, c"gtk_layer_set_monitor")?,
        })
    }
}

/// SAFETY: `name` is a C string. `handle` came from `dlopen` of the layer-shell library.
/// `T` is the C function pointer for that symbol.
unsafe fn symbol<T>(handle: *mut c_void, name: &CStr) -> Option<T> {
    let ptr = unsafe { dlsym(handle, name.as_ptr()) };
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&ptr) })
    }
}

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const i8, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
}

fn gtk_ptr(window: &gtk::Window) -> *mut c_void {
    window.as_ptr().cast()
}

fn monitor_ptr(monitor: &gtk::gdk::Monitor) -> *mut c_void {
    monitor.as_ptr().cast()
}

fn round_px(value: Pixels) -> i32 {
    let rounded = f32::from(value).round();
    if !rounded.is_finite() || rounded < i32::MIN as f32 || rounded > i32::MAX as f32 {
        return 0;
    }
    rounded as i32
}
