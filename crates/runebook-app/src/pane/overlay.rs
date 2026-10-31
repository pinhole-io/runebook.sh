//! Wayland note page.
//!
//! WebKitGTK cannot draw into a Wayland surface it did not create.
//! The page is its own layer-shell surface, placed over the preview column.
//! Both surfaces use the output scale, so the type stays sharp.
//!
//! A failure hides the page. It does not print a path.

use super::place::{Cover, Origin, sway_cover, sway_origin};

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
const PAINT_HOLD: Duration = Duration::from_millis(400);
/// Stay hidden briefly after the note is confirmed, so the drawn frame is ready.
const PAINT_GAP: Duration = Duration::from_millis(48);
/// Stay unmapped after a return until the page rect stops moving.
const POSE_HOLD: Duration = Duration::from_millis(48);
/// Show anyway if the rect never settles.
const POSE_LIMIT: Duration = Duration::from_millis(300);
const MAX_SIDE: i32 = 16_000;

/// The note page for a Wayland window.
pub(super) struct NoteOverlay {
    webview: wry::WebView,
    window: gtk::Window,
    wanted: Rc<Cell<bool>>,
    rewrite: Cell<bool>,
    keyboard: Rc<Cell<i32>>,
    placed: Rc<Cell<Option<(i32, i32, i32, i32)>>>,
    parked: Cell<bool>,
    /// The page left the screen. Load the shell again the next time it is shown.
    needs_shell: Rc<Cell<bool>>,
    /// Hide the window until this load's note is confirmed. `(started, generation)`.
    held: Cell<Option<(Instant, u64)>>,
    /// When the note was confirmed. The page stays hidden for `PAINT_GAP`.
    acked_at: Cell<Option<(u64, Instant)>>,
    /// The page is hidden until the note is drawn. Keep asking for frames.
    needs_frame: Rc<Cell<bool>>,
    /// Coming back from off screen. Unmapped until the rect holds.
    landing: Cell<Option<Landing>>,
    monitor: Cell<Option<(i32, i32)>>,
    cache: Cell<Option<CachedOrigin>>,
    /// Last color painted behind the page. A clear color leaves light specks.
    page_color: Cell<Option<(u8, u8, u8)>>,
}

#[derive(Clone, Copy)]
struct Landing {
    goal: (i32, i32, i32, i32),
    changed: Instant,
    started: Instant,
}

#[derive(Clone, Copy)]
struct CachedOrigin {
    at: Instant,
    epoch: u64,
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
    color: (u8, u8, u8),
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
    // An RGB window cannot fade. The layer would show the note while it loads.
    window.set_app_paintable(true);
    if let Some(screen) = gtk::prelude::WidgetExt::screen(&window)
        && let Some(visual) = screen.rgba_visual()
    {
        window.set_visual(Some(&visual));
    }
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
    paint_page_color(&webview, &window, color);
    super::page::allow_media_scheme(&webview);
    let keyboard = Rc::new(Cell::new(KEYBOARD_NONE));
    let placed = Rc::new(Cell::new(None));
    let needs_shell = Rc::new(Cell::new(false));
    let wanted = Rc::new(Cell::new(false));
    let needs_frame = Rc::new(Cell::new(false));
    remember_page(
        &window,
        Rc::clone(&keyboard),
        Rc::clone(&placed),
        Rc::clone(&needs_shell),
        Rc::clone(&wanted),
        Rc::clone(&needs_frame),
    );
    Some(cx.new(|_| NoteOverlay {
        webview,
        window,
        wanted,
        rewrite: Cell::new(false),
        keyboard,
        placed,
        parked: Cell::new(false),
        needs_shell,
        held: Cell::new(None),
        acked_at: Cell::new(None),
        needs_frame,
        landing: Cell::new(None),
        monitor: Cell::new(None),
        cache: Cell::new(None),
        page_color: Cell::new(Some(color)),
    }))
}

/// Fill the page with the theme color.
///
/// A clear WebKit background leaves light specks on the left edge.
fn paint_page_color(webview: &wry::WebView, window: &gtk::Window, color: (u8, u8, u8)) {
    let (red, green, blue) = color;
    {
        use webkit2gtk::WebViewExt;
        use wry::WebViewExtUnix;
        webview.webview().set_background_color(&gtk::gdk::RGBA::new(
            f64::from(red) / 255.0,
            f64::from(green) / 255.0,
            f64::from(blue) / 255.0,
            1.0,
        ));
    }
    let provider = gtk::CssProvider::new();
    let css = format!("window {{ background-color: rgb({red}, {green}, {blue}); }}");
    if provider.load_from_data(css.as_bytes()).is_err() {
        return;
    }
    window
        .style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

impl NoteOverlay {
    pub(super) fn set_showing(&self, showing: bool, rewrite: bool) {
        let was = self.wanted.replace(showing);
        self.rewrite.set(rewrite);
        if !showing {
            self.hide_native();
            return;
        }
        // Settings and Edit take the page out of the tree. WebKit drops the
        // document while it is parked. Load the shell once it is on screen.
        if !was && self.parked.get() {
            self.needs_shell.set(true);
        }
    }

    pub(super) fn eval(&self, script: &str) -> bool {
        self.webview.evaluate_script(script).is_ok()
    }

    /// Match the unpainted page to the theme. Skip the call when the color is unchanged.
    pub(super) fn set_page_color(&self, color: (u8, u8, u8)) {
        if self.page_color.get() == Some(color) {
            return;
        }
        self.page_color.set(Some(color));
        paint_page_color(&self.webview, &self.window, color);
    }

    fn place(&self, bounds: Bounds<Pixels>, window: &Window) {
        if !self.wanted.get() {
            self.hide_native();
            return;
        }
        let Some(origin) = self.origin(window) else {
            // The window is not in the tree. The compositor drops the page.
            self.needs_shell.set(true);
            self.hide_native();
            return;
        };
        if !origin.visible {
            self.needs_shell.set(true);
        } else if self
            .monitor
            .get()
            .is_some_and(|pinned| pinned != (origin.output_x, origin.output_y))
        {
            // The window changed output. The old layer stays on the old output.
            self.needs_shell.set(true);
        }
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
        let was = self.placed.get();
        // Parking moves the page off every output. GTK still reports it as
        // visible, so the compositor will not map it again until a real hide
        // and show. That is the Settings and Edit return path.
        let reload = self.needs_shell.replace(false);
        // A mapped page slides in from off screen when the margins change.
        // Unmap first. Show only after the rect has stopped moving.
        let arriving = was.is_none();
        let remap = reload || arriving || !self.window.is_visible();
        if reload {
            let generation = super::page::LOAD_GEN
                .load(Ordering::Relaxed)
                .saturating_add(1);
            self.held.set(Some((Instant::now(), generation)));
            self.acked_at.set(None);
            self.paint_alpha(0.0);
        }
        if remap {
            self.window.hide();
            self.monitor.set(None);
        }
        if remap || was != Some(next) {
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
            if arriving {
                let now = Instant::now();
                self.landing.set(Some(Landing {
                    goal: next,
                    changed: now,
                    started: now,
                }));
            }
        }
        self.settle_landing(next);
        self.parked.set(false);
        self.apply_keyboard(api, raw);
        if self.landing.get().is_none() && !self.window.is_visible() {
            self.pin_monitor(api, origin);
            self.window.show_all();
        }
        if reload {
            self.paint_alpha(0.0);
            super::page::reload_shell(&self.webview);
        }
        if self.landing.get().is_some() {
            self.needs_frame.set(true);
        } else {
            self.reveal();
        }
    }

    /// Keep the page unmapped while its rect is still moving.
    fn settle_landing(&self, next: (i32, i32, i32, i32)) {
        let Some(mut landing) = self.landing.get() else {
            return;
        };
        let now = Instant::now();
        if now.saturating_duration_since(landing.started) >= POSE_LIMIT {
            self.landing.set(None);
            return;
        }
        if landing.goal != next {
            landing.goal = next;
            landing.changed = now;
            self.landing.set(Some(landing));
            self.needs_frame.set(true);
            return;
        }
        if now.saturating_duration_since(landing.changed) < POSE_HOLD {
            self.needs_frame.set(true);
            return;
        }
        self.landing.set(None);
    }

    /// Show the page. A shell reload stays invisible until the note is drawn.
    fn reveal(&self) {
        let Some((started, generation)) = self.held.get() else {
            self.needs_frame.set(false);
            self.paint_alpha(1.0);
            return;
        };
        let now = Instant::now();
        let timed_out = now.saturating_duration_since(started) >= PAINT_HOLD;
        let acked = super::page::ACK_GEN.load(Ordering::Relaxed) >= generation;
        if timed_out || (acked && self.paint_settled(generation, now)) {
            self.held.set(None);
            self.acked_at.set(None);
            self.needs_frame.set(false);
            self.paint_alpha(1.0);
            return;
        }
        self.needs_frame.set(true);
        self.paint_alpha(0.0);
    }

    /// `true` once the confirmed note has had time to reach the screen.
    fn paint_settled(&self, generation: u64, now: Instant) -> bool {
        match self.acked_at.get() {
            Some((seen, at)) if seen == generation => {
                now.saturating_duration_since(at) >= PAINT_GAP
            }
            _ => {
                self.acked_at.set(Some((generation, now)));
                false
            }
        }
    }

    fn paint_alpha(&self, opacity: f64) {
        use wry::WebViewExtUnix;
        self.window.set_opacity(opacity);
        self.webview.webview().set_opacity(opacity);
        if let Some(gdk_window) = self.window.window() {
            gdk_window.set_opacity(opacity);
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
        let epoch = ORIGIN_EPOCH.load(Ordering::Relaxed);
        if let Some(cached) = self.cache.get()
            && cached.epoch == epoch
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
        self.cache.set(Some(CachedOrigin {
            at: now,
            epoch,
            origin,
        }));
        origin
    }

    fn hide_native(&self) {
        if self.parked.get() {
            return;
        }
        // Unmapping the window makes WebKit load the shell again.
        conceal_window(&self.window, &self.keyboard, &self.placed);
        self.parked.set(true);
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
    cx.spawn(async move |root, cx| {
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
            // Wayland does not draw an idle window. `notify` does not start a
            // frame, so the page stayed blank until the mouse moved.
            let preview = super::page::PREVIEW_WAIT.load(Ordering::Relaxed)
                || super::page::PREVIEW_RESHOWN.load(Ordering::Relaxed);
            if preview || page_needs_frame() {
                root.update(cx, |this, cx| {
                    if preview {
                        let retry = this
                            .pane
                            .as_mut()
                            .is_some_and(|pane| pane.retry_preview());
                        if retry {
                            cx.notify();
                        }
                    }
                    ask_frame(cx);
                })
                .ok();
            }
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
        }
    })
    .detach();
}

struct PageSlot {
    window: gtk::Window,
    keyboard: Rc<Cell<i32>>,
    placed: Rc<Cell<Option<(i32, i32, i32, i32)>>>,
    needs_shell: Rc<Cell<bool>>,
    wanted: Rc<Cell<bool>>,
    needs_frame: Rc<Cell<bool>>,
}

thread_local! {
    static PAGES: RefCell<Vec<PageSlot>> = RefCell::new(Vec::new());
}

static COVERED: AtomicBool = AtomicBool::new(false);
/// Bumped when focus changes, so a stale "covered" position is not reused.
static ORIGIN_EPOCH: AtomicU64 = AtomicU64::new(0);

fn remember_page(
    window: &gtk::Window,
    keyboard: Rc<Cell<i32>>,
    placed: Rc<Cell<Option<(i32, i32, i32, i32)>>>,
    needs_shell: Rc<Cell<bool>>,
    wanted: Rc<Cell<bool>>,
    needs_frame: Rc<Cell<bool>>,
) {
    PAGES.with(|pages| {
        pages.borrow_mut().push(PageSlot {
            window: window.clone(),
            keyboard,
            placed,
            needs_shell,
            wanted,
            needs_frame,
        });
    });
}

fn forget_page(window: &gtk::Window) {
    PAGES.with(|pages| {
        pages
            .borrow_mut()
            .retain(|page| page.window.as_ptr() != window.as_ptr());
    });
}

/// Keep the page mapped and move it off the output.
fn conceal_window(
    window: &gtk::Window,
    keyboard: &Cell<i32>,
    placed: &Cell<Option<(i32, i32, i32, i32)>>,
) {
    if let Some(api) = layer_api() {
        let raw = gtk_ptr(window);
        // SAFETY: `raw` is a note page this process owns. The library is loaded.
        unsafe {
            (api.set_margin)(raw, EDGE_LEFT, -MAX_SIDE);
            (api.set_margin)(raw, EDGE_TOP, -MAX_SIDE);
            (api.set_keyboard)(raw, KEYBOARD_NONE);
        }
    }
    placed.set(None);
    keyboard.set(KEYBOARD_NONE);
}

fn page_covered() -> bool {
    COVERED.load(Ordering::Relaxed)
}

/// Hide the page while another window is in front.
///
/// A return asks GPUI to draw until the page is placed. A workspace change
/// asks for a new shell. A focus change does not.
fn hide_when_covered() {
    let Some(cover) = sway_cover(crate::icon::APP_ID, 0, 0) else {
        return;
    };
    let covered = !matches!(cover, Cover::Clear);
    let was = COVERED.swap(covered, Ordering::Relaxed);
    if was != covered {
        ORIGIN_EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    if !covered {
        return;
    }
    let absent = matches!(cover, Cover::Absent);
    PAGES.with(|pages| {
        for page in pages.borrow().iter() {
            if absent {
                page.needs_shell.set(true);
            }
            if page.window.is_visible() {
                conceal_window(&page.window, &page.keyboard, &page.placed);
            }
        }
    });
}

/// `true` while a shown page is off screen, or hidden until the note is drawn.
fn page_needs_frame() -> bool {
    if COVERED.load(Ordering::Relaxed) {
        return false;
    }
    PAGES.with(|pages| {
        pages.borrow().iter().any(|page| {
            page.wanted.get() && (page.placed.get().is_none() || page.needs_frame.get())
        })
    })
}

/// Start a Wayland frame. `notify` alone leaves the window idle.
fn ask_frame(cx: &mut Context<crate::Root>) {
    cx.notify();
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, _| {
            super::present_scan(window);
        });
    }
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
