//! Copy Wayland frames into `ffmpeg`.
//!
//! The compositor decides whether this client may read the screen.
//! A failed copy returns an error and does not include pixel data.
//! Window titles are never passed to `ffmpeg`.

use crate::locale;

use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::raw::{c_char, c_int, c_void};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use super::{ScreenTarget, finish, focused_output_name};

const FRAME_WAIT: Duration = Duration::from_millis(33);
const MAX_EDGE: i32 = 8192;
const MAP_FAILED: *mut c_void = !0usize as *mut c_void;

pub(super) struct Grab {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: Option<JoinHandle<()>>,
}

impl Grab {
    pub(super) fn start(output: std::path::PathBuf, target: ScreenTarget) -> Result<Self, String> {
        let (region, output_name) = match target {
            ScreenTarget::Display => (None, focused_output_name()),
            ScreenTarget::Region {
                output,
                x,
                y,
                width,
                height,
            } => (Some((x, y, width, height)), Some(output)),
            ScreenTarget::Window(_) => {
                return Err(locale::text("screen.window_display").to_string());
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let stop_flag = Arc::clone(&stop);
        let child_slot = Arc::clone(&child);
        let thread = thread::spawn(move || {
            if let Err(err) = record(
                output,
                output_name,
                region,
                stop_flag,
                child_slot,
                ready_tx.clone(),
            ) {
                let _ = ready_tx.send(Err(err));
            }
        });
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                child,
                thread: Some(thread),
            }),
            Ok(Err(err)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                reap(&child);
                Err(err)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                reap(&child);
                Err(locale::text("screen.wayland_start").to_string())
            }
        }
    }

    pub(super) fn finish(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        reap(&self.child);
    }
}

fn reap(child: &Mutex<Option<Child>>) {
    let child = child.lock().ok().and_then(|mut slot| slot.take());
    if let Some(mut child) = child
        && super::wait_timeout(&mut child, Duration::from_secs(2)).is_none()
    {
        finish(&mut child, Duration::from_secs(3));
    }
}

fn record(
    path: std::path::PathBuf,
    output_name: Option<String>,
    region: Option<(i32, i32, i32, i32)>,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let conn = Connection::connect_to_env()
        .map_err(|_| locale::text("screen.wayland_missing").to_string())?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let mut state = State::default();
    let _registry = conn.display().get_registry(&qh, ());
    queue
        .roundtrip(&mut state)
        .map_err(|_| locale::text("screen.wayland_answer").to_string())?;
    queue
        .roundtrip(&mut state)
        .map_err(|_| locale::text("screen.wayland_answer").to_string())?;
    let manager = state
        .manager
        .clone()
        .ok_or_else(|| locale::text("screen.wayland_share").to_string())?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| locale::text("screen.wayland_share").to_string())?;
    let output = pick_output(&state.outputs, output_name.as_deref())?
        .proxy
        .clone();
    let screencopy_version = state.screencopy_version;
    let mut stdin = None;
    let mut mapping: Option<Mapping> = None;
    let mut pool: Option<wl_shm_pool::WlShmPool> = None;
    let mut started_sent = false;

    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        state.frame = FrameState::default();
        let frame = match region {
            Some((x, y, width, height)) => {
                manager.capture_output_region(1, &output, x, y, width, height, &qh, ())
            }
            None => manager.capture_output(1, &output, &qh, ()),
        };
        wait_frame(&mut queue, &mut state, &stop, |frame| {
            frame.buffer_done || frame.failed || (screencopy_version < 3 && frame.have_shm)
        })?;
        if stop.load(Ordering::Relaxed) {
            frame.destroy();
            break;
        }
        if state.frame.failed || !state.frame.have_shm {
            frame.destroy();
            if stdin.is_none() {
                return Err(locale::text("screen.pixel").to_string());
            }
            break;
        }
        let info = state.frame.info()?;
        if mapping
            .as_ref()
            .is_none_or(|current| current.len != info.len)
        {
            let created = Mapping::new(info.len)?;
            let next_pool = shm.create_pool(created.fd.as_fd(), info.len as i32, &qh, ());
            if let Some(old) = pool.take() {
                old.destroy();
            }
            pool = Some(next_pool);
            mapping = Some(created);
        }
        let Some(current_pool) = pool.as_ref() else {
            return Err(locale::text("screen.frame").to_string());
        };
        let buffer = current_pool.create_buffer(
            0,
            info.width,
            info.height,
            info.stride,
            info.format,
            &qh,
            (),
        );
        frame.copy(&buffer);
        wait_frame(&mut queue, &mut state, &stop, |frame| {
            frame.ready || frame.failed
        })?;
        frame.destroy();
        buffer.destroy();
        if stop.load(Ordering::Relaxed) && !state.frame.ready {
            break;
        }
        if state.frame.failed {
            if stdin.is_none() {
                return Err(locale::text("screen.copy").to_string());
            }
            break;
        }
        if stdin.is_none() {
            let mut child = spawn_ffmpeg(&path, info.width, info.height)?;
            let pipe = child
                .stdin
                .take()
                .ok_or_else(|| locale::text("screen.ffmpeg_frames").to_string())?;
            if let Ok(mut slot) = child_slot.lock() {
                *slot = Some(child);
            }
            stdin = Some(pipe);
        }
        let Some(bytes) = mapping.as_ref().map(Mapping::bytes) else {
            return Err(locale::text("screen.frame").to_string());
        };
        let Some(pipe) = stdin.as_mut() else {
            return Err(locale::text("screen.ffmpeg_frames").to_string());
        };
        let invert = state.frame.y_invert;
        write_frame(pipe, bytes, info, invert)?;
        if !started_sent {
            let _ = ready.send(Ok(()));
            started_sent = true;
        }
        let elapsed = started.elapsed();
        if elapsed < FRAME_WAIT {
            thread::sleep(FRAME_WAIT - elapsed);
        }
    }
    drop(stdin);
    Ok(())
}

struct OutputInfo {
    proxy: wl_output::WlOutput,
    name: String,
}

struct FrameInfo {
    format: wl_shm::Format,
    width: i32,
    height: i32,
    stride: i32,
    row_bytes: usize,
    len: usize,
}

#[derive(Default)]
struct FrameState {
    format: Option<wl_shm::Format>,
    width: u32,
    height: u32,
    stride: u32,
    y_invert: bool,
    have_shm: bool,
    buffer_done: bool,
    ready: bool,
    failed: bool,
}

impl FrameState {
    fn info(&self) -> Result<FrameInfo, String> {
        let format = self
            .format
            .ok_or_else(|| locale::text("screen.pixel").to_string())?;
        let width = i32::try_from(self.width).unwrap_or(0);
        let height = i32::try_from(self.height).unwrap_or(0);
        let stride = i32::try_from(self.stride).unwrap_or(0);
        if width <= 0 || height <= 0 || width > MAX_EDGE || height > MAX_EDGE || stride < width * 4
        {
            return Err(locale::text("screen.pixel").to_string());
        }
        let row_bytes = (width as usize).saturating_mul(4);
        let len = (stride as usize).saturating_mul(height as usize);
        if len == 0 || len > (MAX_EDGE as usize) * (MAX_EDGE as usize) * 4 {
            return Err(locale::text("screen.pixel").to_string());
        }
        Ok(FrameInfo {
            format,
            width,
            height,
            stride,
            row_bytes,
            len,
        })
    }
}

#[derive(Default)]
struct State {
    shm: Option<wl_shm::WlShm>,
    manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    screencopy_version: u32,
    outputs: Vec<OutputInfo>,
    frame: FrameState,
}

fn wait_frame(
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    stop: &AtomicBool,
    until: impl Fn(&FrameState) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !until(&state.frame) && !stop.load(Ordering::Relaxed) {
        if Instant::now() >= deadline {
            return Err(locale::text("screen.wayland_answer").to_string());
        }
        queue
            .dispatch_pending(state)
            .map_err(|_| locale::text("screen.wayland_answer").to_string())?;
        if until(&state.frame) || stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        queue
            .flush()
            .map_err(|_| locale::text("screen.wayland_answer").to_string())?;
        if !wait_readable(guard.connection_fd(), Duration::from_millis(100))? {
            drop(guard);
            continue;
        }
        match guard.read() {
            Ok(_) => {}
            Err(err) if wayland_blocked(&err) => {}
            Err(_) => return Err(locale::text("screen.wayland_answer").to_string()),
        }
    }
    Ok(())
}

fn wayland_blocked(err: &wayland_client::backend::WaylandError) -> bool {
    matches!(err, wayland_client::backend::WaylandError::Io(err) if err.kind() == std::io::ErrorKind::WouldBlock)
}

fn wait_readable(fd: BorrowedFd<'_>, timeout: Duration) -> Result<bool, String> {
    let mut fds = [PollFd {
        fd: fd.as_raw_fd(),
        events: 1,
        revents: 0,
    }];
    let millis = i32::try_from(timeout.as_millis()).unwrap_or(100);
    loop {
        let ready = unsafe { poll(fds.as_mut_ptr(), 1, millis) };
        if ready >= 0 {
            return Ok(ready > 0);
        }
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(locale::text("screen.wayland_answer").to_string());
    }
}

fn pick_output<'a>(
    outputs: &'a [OutputInfo],
    name: Option<&str>,
) -> Result<&'a OutputInfo, String> {
    if let Some(name) = name.filter(|name| !name.is_empty())
        && let Some(output) = outputs.iter().find(|output| output.name == name)
    {
        return Ok(output);
    }
    outputs
        .first()
        .ok_or_else(|| locale::text("screen.wayland_share").to_string())
}

fn spawn_ffmpeg(path: &std::path::Path, width: i32, height: i32) -> Result<Child, String> {
    let args = [
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "-video_size",
        &format!("{width}x{height}"),
        "-framerate",
        "30",
        "-i",
        "pipe:0",
        "-c:v",
        "libx264",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        "-preset",
        "ultrafast",
        &path.display().to_string(),
    ];
    let mut owned = Vec::with_capacity(args.len());
    for arg in args {
        owned.push(arg.to_string());
    }
    match Command::new("ffmpeg")
        .args(&owned)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Ok(child),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(locale::text("screen.ffmpeg_missing").to_string())
        }
        Err(err) => Err(locale::format(
            "screen.ffmpeg_start",
            &[("err", &err.to_string())],
        )),
    }
}

fn write_frame(
    stdin: &mut impl Write,
    bytes: &[u8],
    info: FrameInfo,
    y_invert: bool,
) -> Result<(), String> {
    let stride = info.stride as usize;
    let height = info.height as usize;
    if y_invert {
        for row in (0..height).rev() {
            let start = row * stride;
            stdin
                .write_all(&bytes[start..start + info.row_bytes])
                .map_err(|_| locale::text("screen.ffmpeg_stopped").to_string())?;
        }
    } else {
        for row in 0..height {
            let start = row * stride;
            stdin
                .write_all(&bytes[start..start + info.row_bytes])
                .map_err(|_| locale::text("screen.ffmpeg_stopped").to_string())?;
        }
    }
    Ok(())
}

struct Mapping {
    ptr: *mut u8,
    len: usize,
    fd: OwnedFd,
}

impl Mapping {
    fn new(len: usize) -> Result<Self, String> {
        let fd = create_memfd(len)?;
        let ptr = unsafe { mmap_fd(fd.as_fd(), len)? };
        Ok(Self { ptr, len, fd })
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            munmap(self.ptr.cast(), self.len);
        }
    }
}

fn create_memfd(len: usize) -> Result<OwnedFd, String> {
    let name = c"runebook-frame".as_ptr();
    let fd = unsafe { memfd_create(name, 1) };
    if fd < 0 {
        return Err(locale::text("screen.frame").to_string());
    }
    let len = i64::try_from(len).map_err(|_| locale::text("screen.frame").to_string())?;
    let truncated = unsafe { ftruncate(fd, len) };
    if truncated != 0 {
        unsafe { close(fd) };
        return Err(locale::text("screen.frame").to_string());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Map `fd` for reading and writing.
///
/// # Safety
///
/// `fd` must be a live memfd of at least `len` bytes. The returned pointer
/// is owned by the caller and freed with `munmap`.
unsafe fn mmap_fd(fd: impl AsFd, len: usize) -> Result<*mut u8, String> {
    let ptr = unsafe {
        mmap(
            std::ptr::null_mut(),
            len,
            1 | 2,
            1,
            fd.as_fd().as_raw_fd(),
            0,
        )
    };
    if ptr == MAP_FAILED || ptr.is_null() {
        return Err(locale::text("screen.frame").to_string());
    }
    Ok(ptr.cast())
}

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: u32) -> c_int;
    fn ftruncate(fd: c_int, length: i64) -> c_int;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn poll(fds: *mut PollFd, nfds: u64, timeout: c_int) -> c_int;
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

fn accepted_format(format: wl_shm::Format) -> bool {
    matches!(format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_shm" => {
                state.shm = Some(registry.bind(name, version.min(1), qh, ()));
            }
            "zwlr_screencopy_manager_v1" => {
                let bound = version.min(3);
                state.screencopy_version = bound;
                state.manager = Some(registry.bind(name, bound, qh, ()));
            }
            "wl_output" => {
                let proxy = registry.bind(name, version.min(4), qh, ());
                state.outputs.push(OutputInfo {
                    proxy,
                    name: String::new(),
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: <wl_output::WlOutput as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_output::Event::Name { name } = event else {
            return;
        };
        if let Some(slot) = state.outputs.iter_mut().find(|slot| slot.proxy == *output) {
            slot.name = name;
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: <wl_shm::WlShm as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: <wl_shm_pool::WlShmPool as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: <wl_buffer::WlBuffer as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _: <zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: <zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if let Ok(format) = format.into_result()
                    && accepted_format(format)
                {
                    state.frame.format = Some(format);
                    state.frame.width = width;
                    state.frame.height = height;
                    state.frame.stride = stride;
                    state.frame.have_shm = true;
                }
            }
            Event::BufferDone => state.frame.buffer_done = true,
            Event::Flags { flags } => {
                state.frame.y_invert = flags.into_result().is_ok_and(|value| value.bits() & 1 != 0);
            }
            Event::Ready { .. } => state.frame.ready = true,
            Event::Failed => state.frame.failed = true,
            Event::LinuxDmabuf { .. } | Event::Damage { .. } => {}
            _ => {}
        }
    }
}
