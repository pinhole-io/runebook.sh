//! Play one vault video or audio file inside the preview.
//!
//! `ffmpeg` writes JPEG frames and PCM. `cpal` plays the PCM. The path is an
//! argument, not a shell string. Errors do not include a filesystem path.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use gpui_kit::{Image, ImageFormat, RenderImage, SvgRenderer};

const MAX_JPEG: usize = 8 * 1024 * 1024;
const MAX_QUEUED: usize = 48_000 * 2;
/// Most frames kept for one clip. A longer file plays this prefix.
const FRAME_LIMIT: usize = 600;

/// A video or audio file named by a preview row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipKind {
    Video,
    Audio,
}

/// A preview row whose only content is a media link.
pub(crate) struct MediaLink {
    pub label: String,
    pub href: String,
    pub kind: ClipKind,
}

/// Parse a row that is only `[label](file)` or `[label](<file>)`.
pub(crate) fn media_link(row: &str) -> Option<MediaLink> {
    let row = row.trim();
    let rest = row.strip_prefix('[')?;
    let (label, rest) = rest.split_once(']')?;
    let rest = rest.strip_prefix('(')?;
    let (href, rest) = split_href(rest)?;
    if !rest.is_empty() {
        return None;
    }
    let label = label.trim();
    let href = href.trim();
    if label.is_empty() || href.is_empty() || href.contains(['\n', '\r']) {
        return None;
    }
    let kind = clip_kind(href)?;
    Some(MediaLink {
        label: label.to_string(),
        href: href.to_string(),
        kind,
    })
}

fn split_href(rest: &str) -> Option<(&str, &str)> {
    if let Some(rest) = rest.strip_prefix('<') {
        let (href, rest) = rest.split_once('>')?;
        let rest = rest.strip_prefix(')')?;
        return Some((href, rest));
    }
    let (href, rest) = rest.split_once(')')?;
    Some((href, rest))
}

fn clip_kind(href: &str) -> Option<ClipKind> {
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let (_, ext) = path.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "webm" | "mkv" | "mov" => Some(ClipKind::Video),
        "mp3" | "wav" | "ogg" | "m4a" | "flac" => Some(ClipKind::Audio),
        _ => None,
    }
}

/// A frame rate as `num/den` frames per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rate {
    pub num: u32,
    pub den: u32,
}

/// Length and frame rate of a clip. `fps` is absent when the file has no picture.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClipTiming {
    pub fps: Option<Rate>,
    pub duration: Duration,
}

const MIN_FRAME_WIDTH: u32 = 320;
const MAX_FRAME_WIDTH: u32 = 1920;

/// Even pixel width for one playback frame.
///
/// `raw` is the note width in device pixels. The result stays inside
/// 320..=1920 so the atlas texture matches the window.
pub(crate) fn frame_width(raw: u32) -> u32 {
    (raw & !1).clamp(MIN_FRAME_WIDTH, MAX_FRAME_WIDTH)
}

fn scale_filter(width: u32) -> String {
    format!(
        "scale=w='min({},iw)':h=-2:flags=lanczos",
        frame_width(width)
    )
}

/// One JPEG frame for `path`, sized the same way as playback.
pub(crate) fn poster_args(path: &Path, start: Duration, width: u32) -> Vec<OsString> {
    let mut args = head(path, start);
    args.extend([
        "-an".into(),
        "-vf".into(),
        scale_filter(width).into(),
        "-frames:v".into(),
        "1".into(),
        "-pix_fmt".into(),
        "yuvj444p".into(),
        "-q:v".into(),
        "1".into(),
        "-f".into(),
        "image2pipe".into(),
        "-vcodec".into(),
        "mjpeg".into(),
        "pipe:1".into(),
    ]);
    args
}

/// JPEG frames for `path` at `rate`, starting at `start`.
///
/// `-re` reads the file at its own rate. The `fps` filter is that rate,
/// capped at 60. `width` is the note width in device pixels.
pub(crate) fn video_args(
    path: &Path,
    rate: Option<Rate>,
    start: Duration,
    width: u32,
) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
    ];
    push_seek(&mut args, start);
    args.extend([
        "-re".into(),
        "-i".into(),
        path.as_os_str().to_os_string(),
        "-an".into(),
        "-vf".into(),
        video_filter(rate, width).into(),
        "-pix_fmt".into(),
        "yuvj444p".into(),
        "-q:v".into(),
        "1".into(),
        "-f".into(),
        "image2pipe".into(),
        "-vcodec".into(),
        "mjpeg".into(),
        "-flush_packets".into(),
        "1".into(),
        "pipe:1".into(),
    ]);
    args
}

/// Every JPEG for `path`, written as fast as `ffmpeg` can go.
///
/// This is [`video_args`] without `-re`. The clock starts after these frames exist.
pub(crate) fn preload_args(path: &Path, rate: Option<Rate>, width: u32) -> Vec<OsString> {
    video_args(path, rate, Duration::ZERO, width)
        .into_iter()
        .filter(|arg| arg != "-re")
        .collect()
}

/// One decoded playback frame and the JPEG it came from.
pub(crate) struct FilmFrame {
    pub jpeg: Vec<u8>,
    pub image: Arc<RenderImage>,
}

/// Frames ready to play. `duration` is the file length, or the loaded prefix.
pub(crate) struct LoadedFilm {
    pub frames: Vec<FilmFrame>,
    pub timing: ClipTiming,
    pub duration: Duration,
}

/// Read and decode every frame of `path`. `stop` cancels the work.
///
/// This does not open the speaker and does not spawn a shell.
pub(crate) fn load_film(
    path: &Path,
    width: u32,
    renderer: SvgRenderer,
    stop: &AtomicBool,
) -> Result<LoadedFilm, String> {
    let timing = probe_timing(path);
    let mut child = spawn_ffmpeg(preload_args(path, timing.fps, width))?;
    let mut stdout = match take_stdout(&mut child) {
        Ok(stdout) => stdout,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };
    let frames = decode_stream(&mut stdout, renderer, stop);
    let _ = child.kill();
    let _ = child.wait();
    let interval = frame_interval(timing.fps);
    let duration = film_duration(frames.len(), interval, timing.duration);
    Ok(LoadedFilm {
        frames,
        timing,
        duration,
    })
}

fn film_duration(frames: usize, interval: Duration, file: Duration) -> Duration {
    if frames == FRAME_LIMIT && !interval.is_zero() {
        let loaded = interval.saturating_mul(u32::try_from(frames).unwrap_or(u32::MAX));
        if file.is_zero() || loaded < file {
            return loaded;
        }
    }
    file
}

/// How many JPEGs to decode at once. One thread makes the loader wait on each frame.
fn worker_count() -> usize {
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
}

/// Read JPEGs from `input` and decode them on several threads.
///
/// Decode starts while `ffmpeg` is still writing. Frame order stays the file order.
fn decode_stream(
    input: &mut impl Read,
    renderer: SvgRenderer,
    stop: &AtomicBool,
) -> Vec<FilmFrame> {
    let workers = worker_count();
    thread::scope(|scope| {
        let mut senders = Vec::with_capacity(workers);
        let mut joins = Vec::with_capacity(workers);
        for _ in 0..workers {
            let (sender, receiver) = sync_channel(2);
            let renderer = renderer.clone();
            joins.push(scope.spawn(move || decode_worker(receiver, renderer, stop)));
            senders.push(sender);
        }
        let mut index = 0usize;
        pull_jpegs(input, stop, |jpeg| {
            let sent = senders[index % workers].send((index, jpeg)).is_ok();
            index += 1;
            sent
        });
        drop(senders);
        let mut frames = Vec::new();
        for join in joins {
            if let Ok(part) = join.join() {
                frames.extend(part);
            }
        }
        frames.sort_by_key(|(index, _)| *index);
        frames.into_iter().map(|(_, frame)| frame).collect()
    })
}

fn decode_worker(
    receiver: Receiver<(usize, Vec<u8>)>,
    renderer: SvgRenderer,
    stop: &AtomicBool,
) -> Vec<(usize, FilmFrame)> {
    let mut frames = Vec::new();
    while let Ok((index, jpeg)) = receiver.recv() {
        if stop.load(Ordering::Relaxed) {
            continue;
        }
        let Some(image) = decode_jpeg(&jpeg, &renderer) else {
            continue;
        };
        frames.push((index, FilmFrame { jpeg, image }));
    }
    frames
}

fn decode_jpeg(jpeg: &[u8], renderer: &SvgRenderer) -> Option<Arc<RenderImage>> {
    let source = Image::from_bytes(ImageFormat::Jpeg, jpeg.to_vec());
    source.to_image_data(renderer.clone()).ok()
}

/// The frame to paint at `position`.
pub(crate) fn frame_slot(position: Duration, interval: Duration, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let index = if interval.is_zero() {
        0
    } else {
        frame_index(position, interval) as usize
    };
    Some(index.min(len - 1))
}

/// Signed 16-bit little-endian PCM for `path`, starting at `start`.
pub(crate) fn audio_args(path: &Path, rate: u32, channels: u16, start: Duration) -> Vec<OsString> {
    let mut args = head(path, start);
    args.extend([
        "-vn".into(),
        "-ac".into(),
        channels.to_string().into(),
        "-ar".into(),
        rate.to_string().into(),
        "-f".into(),
        "s16le".into(),
        "pipe:1".into(),
    ]);
    args
}

fn head(path: &Path, start: Duration) -> Vec<OsString> {
    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
    ];
    push_seek(&mut args, start);
    args.push("-i".into());
    args.push(path.as_os_str().to_os_string());
    args
}

fn push_seek(args: &mut Vec<OsString>, start: Duration) {
    let Some(stamp) = seek_stamp(start) else {
        return;
    };
    args.push("-ss".into());
    args.push(stamp.into());
}

fn seek_stamp(start: Duration) -> Option<String> {
    if start < Duration::from_millis(50) {
        return None;
    }
    Some(format!("{:.3}", start.as_secs_f64()))
}

fn video_filter(rate: Option<Rate>, width: u32) -> String {
    let scale = scale_filter(width);
    match rate.map(output_rate) {
        Some(rate) => format!("fps={}/{},{scale}", rate.num, rate.den),
        None => scale,
    }
}

/// Cap a file rate at 60 fps. Faster files would miss the preview clock.
fn output_rate(rate: Rate) -> Rate {
    let fps = f64::from(rate.num) / f64::from(rate.den);
    if fps > 60.0 {
        Rate { num: 60, den: 1 }
    } else {
        rate
    }
}

#[cfg(test)]
fn is_frame_due(presented: u64, elapsed: Duration, interval: Duration) -> bool {
    if interval.is_zero() {
        return true;
    }
    frame_index(elapsed, interval) >= presented
}

#[cfg(test)]
fn next_presented(presented: u64, elapsed: Duration, interval: Duration) -> u64 {
    let shown = presented.saturating_add(1);
    if interval.is_zero() {
        return shown;
    }
    frame_index(elapsed, interval).saturating_add(1).max(shown)
}

fn frame_index(elapsed: Duration, interval: Duration) -> u64 {
    let step = interval.as_nanos().max(1);
    (elapsed.as_nanos() / step) as u64
}

pub(crate) fn frame_interval(rate: Option<Rate>) -> Duration {
    let Some(rate) = rate.map(output_rate) else {
        return Duration::ZERO;
    };
    let nanos = (1_000_000_000.0 * f64::from(rate.den) / f64::from(rate.num)).round() as u64;
    Duration::from_nanos(nanos.max(1))
}

fn parse_rate(text: &str) -> Option<Rate> {
    let (num, den) = text.trim().split_once('/')?;
    let num = num.parse().ok()?;
    let den = den.parse().ok()?;
    if num == 0 || den == 0 {
        return None;
    }
    Some(Rate { num, den })
}

/// Read `ffprobe` compact output. This does not spawn a process.
fn parse_probe(text: &str) -> ClipTiming {
    let mut fps = None;
    let mut duration = Duration::ZERO;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("duration=")
            && let Ok(secs) = rest.parse::<f64>()
            && secs.is_finite()
            && secs > 0.0
        {
            duration = Duration::from_secs_f64(secs);
        }
        if line.contains("codec_type=video")
            && let Some(rate) = line
                .split('|')
                .find_map(|part| part.strip_prefix("avg_frame_rate=").and_then(parse_rate))
        {
            fps = Some(rate);
        }
    }
    ClipTiming { fps, duration }
}

/// Ask `ffprobe` for the length and picture rate. Failure yields an empty timing.
pub(crate) fn probe_timing(path: &Path) -> ClipTiming {
    let Ok(output) = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,avg_frame_rate")
        .arg("-of")
        .arg("compact=p=0")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return ClipTiming::default();
    };
    if !output.status.success() {
        return ClipTiming::default();
    }
    parse_probe(&String::from_utf8_lossy(&output.stdout))
}

/// One clip. Dropping it stops `ffmpeg` and the speaker.
pub(crate) struct Playback {
    href: String,
    kind: ClipKind,
    origin: Duration,
    started: Instant,
    duration: Duration,
    rate: Option<Rate>,
    stop: Arc<AtomicBool>,
    audio_stop: Arc<AtomicBool>,
    video_done: Arc<AtomicBool>,
    drained: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    /// Video frames are already decoded. Finish only when the clock ends.
    clock_only: bool,
    children: Vec<Child>,
    threads: Vec<JoinHandle<()>>,
    stream: Option<Stream>,
}

/// A started clip. `notice` is set when the picture can play and the speaker cannot.
pub(crate) struct Started {
    pub playback: Playback,
    pub notice: Option<String>,
}

/// Start `path` at `at`. `muted` silences the speaker and still consumes samples.
/// This does not spawn a shell.
pub(crate) fn start(
    path: &Path,
    href: String,
    kind: ClipKind,
    at: Duration,
    muted: bool,
) -> Result<Started, String> {
    let timing = probe_timing(path);
    open_playback(path, href, kind, at, muted, timing, false)
}

/// Start the clock and the speaker for a clip whose frames are already decoded.
pub(crate) fn start_video(
    path: &Path,
    href: String,
    at: Duration,
    muted: bool,
    timing: ClipTiming,
) -> Result<Started, String> {
    open_playback(path, href, ClipKind::Video, at, muted, timing, true)
}

fn open_playback(
    path: &Path,
    href: String,
    kind: ClipKind,
    at: Duration,
    muted: bool,
    timing: ClipTiming,
    clock_only: bool,
) -> Result<Started, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let audio_stop = Arc::new(AtomicBool::new(false));
    let video_done = Arc::new(AtomicBool::new(false));
    let audio_done = Arc::new(AtomicBool::new(false));
    let drained = Arc::new(AtomicBool::new(false));
    let muted = Arc::new(AtomicBool::new(muted));
    let samples = Arc::new(Mutex::new(VecDeque::new()));
    let mut children = Vec::new();
    let mut threads = Vec::new();
    let mut stream = None;
    let mut notice = None;

    match open_audio(
        path,
        at,
        &audio_stop,
        &audio_done,
        &drained,
        &samples,
        &muted,
    ) {
        Ok(audio) => {
            children.push(audio.child);
            threads.push(audio.thread);
            stream = Some(audio.stream);
        }
        Err(err) if kind == ClipKind::Audio => {
            stop.store(true, Ordering::Relaxed);
            stop_children(&mut children);
            for thread in threads.drain(..) {
                let _ = thread.join();
            }
            return Err(err);
        }
        Err(err) => notice = Some(err),
    }

    Ok(Started {
        playback: Playback {
            href,
            kind,
            origin: at,
            started: Instant::now(),
            duration: timing.duration,
            rate: timing.fps.map(output_rate),
            stop,
            audio_stop,
            video_done,
            drained,
            muted,
            clock_only,
            children,
            threads,
            stream,
        },
        notice,
    })
}

impl Playback {
    pub(crate) fn href(&self) -> &str {
        &self.href
    }

    pub(crate) fn duration(&self) -> Duration {
        self.duration
    }

    pub(crate) fn position(&self) -> Duration {
        let at = self.origin.saturating_add(self.started.elapsed());
        if self.duration.is_zero() {
            at
        } else {
            at.min(self.duration)
        }
    }

    pub(crate) fn timing(&self) -> ClipTiming {
        ClipTiming {
            fps: self.rate,
            duration: self.duration,
        }
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub(crate) fn finished(&self) -> bool {
        if self.clock_done() {
            return true;
        }
        if self.clock_only {
            return false;
        }
        match self.kind {
            ClipKind::Video => {
                self.video_done.load(Ordering::Acquire)
                    && (self.stream.is_none() || self.drained.load(Ordering::Acquire))
            }
            ClipKind::Audio => self.drained.load(Ordering::Acquire),
        }
    }

    fn clock_done(&self) -> bool {
        !self.duration.is_zero()
            && self.origin.saturating_add(self.started.elapsed()) >= self.duration
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.audio_stop.store(true, Ordering::Relaxed);
        stop_children(&mut self.children);
        self.stream.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

struct AudioParts {
    child: Child,
    thread: JoinHandle<()>,
    stream: Stream,
}

fn open_audio(
    path: &Path,
    start: Duration,
    stop: &Arc<AtomicBool>,
    audio_done: &Arc<AtomicBool>,
    drained: &Arc<AtomicBool>,
    samples: &Arc<Mutex<VecDeque<i16>>>,
    muted: &Arc<AtomicBool>,
) -> Result<AudioParts, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "Could not open audio playback".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|_| "Could not open audio playback".to_string())?;
    let rate = supported.sample_rate().0;
    let channels = supported.channels();
    if rate == 0 || channels == 0 {
        return Err("Could not open audio playback".to_string());
    }
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let mut child = spawn_ffmpeg(audio_args(path, rate, channels, start))?;
    let stdout = match take_stdout(&mut child) {
        Ok(stdout) => stdout,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };
    let queue = Arc::clone(samples);
    let stop_flag = Arc::clone(stop);
    let done = Arc::clone(audio_done);
    let thread = thread::spawn(move || {
        pump_pcm(stdout, &queue, &stop_flag);
        done.store(true, Ordering::Release);
    });

    let stream = match build_stream(
        &device,
        &config,
        sample_format,
        samples,
        audio_done,
        drained,
        muted,
    ) {
        Ok(stream) => stream,
        Err(err) => {
            stop.store(true, Ordering::Relaxed);
            let _ = child.kill();
            let _ = child.wait();
            let _ = thread.join();
            return Err(err);
        }
    };
    if stream.play().is_err() {
        stop.store(true, Ordering::Relaxed);
        let _ = child.kill();
        let _ = child.wait();
        drop(stream);
        let _ = thread.join();
        return Err("Could not open audio playback".to_string());
    }
    Ok(AudioParts {
        child,
        thread,
        stream,
    })
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: SampleFormat,
    samples: &Arc<Mutex<VecDeque<i16>>>,
    done: &Arc<AtomicBool>,
    drained: &Arc<AtomicBool>,
    muted: &Arc<AtomicBool>,
) -> Result<Stream, String> {
    match format {
        SampleFormat::F32 => build_out(device, config, samples, done, drained, muted, write_f32),
        SampleFormat::I16 => build_out(device, config, samples, done, drained, muted, write_i16),
        SampleFormat::U16 => build_out(device, config, samples, done, drained, muted, write_u16),
        _ => Err("Could not open audio playback".to_string()),
    }
}

fn build_out<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: &Arc<Mutex<VecDeque<i16>>>,
    done: &Arc<AtomicBool>,
    drained: &Arc<AtomicBool>,
    muted: &Arc<AtomicBool>,
    write: fn(&mut T, i16),
) -> Result<Stream, String>
where
    T: cpal::SizedSample + 'static,
{
    let samples = Arc::clone(samples);
    let done = Arc::clone(done);
    let drained = Arc::clone(drained);
    let muted = Arc::clone(muted);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for sample in data.iter_mut() {
                    write(sample, pop_sample(&samples, &done, &drained, &muted));
                }
            },
            |_| {},
            None,
        )
        .map_err(|_| "Could not open audio playback".to_string())
}

fn write_f32(sample: &mut f32, value: i16) {
    *sample = f32::from(value) / 32768.0;
}

fn write_i16(sample: &mut i16, value: i16) {
    *sample = value;
}

fn write_u16(sample: &mut u16, value: i16) {
    *sample = (i32::from(value) + 32768) as u16;
}

fn pop_sample(
    samples: &Mutex<VecDeque<i16>>,
    done: &AtomicBool,
    drained: &AtomicBool,
    muted: &AtomicBool,
) -> i16 {
    let mut queue = match samples.lock() {
        Ok(queue) => queue,
        Err(err) => err.into_inner(),
    };
    let sample = if let Some(sample) = queue.pop_front() {
        sample
    } else {
        if done.load(Ordering::Acquire) {
            drained.store(true, Ordering::Release);
        }
        0
    };
    if muted.load(Ordering::Relaxed) {
        0
    } else {
        sample
    }
}

fn spawn_ffmpeg(args: Vec<OsString>) -> Result<Child, String> {
    match Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Ok(child),
        Err(err) if err.kind() == ErrorKind::NotFound => Err("ffmpeg is not installed".to_string()),
        Err(err) => Err(format!("Could not start ffmpeg: {err}")),
    }
}

/// The first frame of `path`, or nothing when `ffmpeg` cannot read it.
pub(crate) fn poster_jpeg(path: &Path, start: Duration, width: u32) -> Option<Vec<u8>> {
    let mut child = spawn_ffmpeg(poster_args(path, start, width)).ok()?;
    let mut stdout = match take_stdout(&mut child) {
        Ok(stdout) => stdout,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let mut output = Vec::new();
    let _ = stdout.read_to_end(&mut output);
    let _ = child.wait();
    next_jpeg(&mut output)
}

fn take_stdout(child: &mut Child) -> Result<std::process::ChildStdout, String> {
    child
        .stdout
        .take()
        .ok_or_else(|| "Could not start ffmpeg".to_string())
}

fn stop_children(children: &mut [Child]) {
    for child in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
fn collect_jpegs(mut input: impl Read, stop: &AtomicBool) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    pull_jpegs(&mut input, stop, |jpeg| {
        frames.push(jpeg);
        true
    });
    frames
}

fn pull_jpegs(input: &mut impl Read, stop: &AtomicBool, mut on_jpeg: impl FnMut(Vec<u8>) -> bool) {
    let mut pending = Vec::new();
    let mut frames = 0usize;
    let mut chunk = [0u8; 64 * 1024];
    while frames < FRAME_LIMIT && !stop.load(Ordering::Relaxed) {
        let count = match input.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        pending.extend_from_slice(&chunk[..count]);
        while frames < FRAME_LIMIT
            && let Some(jpeg) = next_jpeg(&mut pending)
        {
            frames += 1;
            if !on_jpeg(jpeg) || stop.load(Ordering::Relaxed) {
                return;
            }
        }
    }
}

#[cfg(test)]
fn enqueue(slot: &Mutex<VecDeque<Vec<u8>>>, jpeg: Vec<u8>, stop: &AtomicBool) {
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let mut queue = match slot.lock() {
        Ok(queue) => queue,
        Err(err) => err.into_inner(),
    };
    queue.push_back(jpeg);
    while queue.len() > 2 {
        queue.pop_front();
    }
}

/// Width divided by height for a JPEG, from its start-of-frame marker.
pub(crate) fn jpeg_ratio(bytes: &[u8]) -> Option<f32> {
    let mut index = 0;
    while index + 8 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if marker == 0xC0 || marker == 0xC1 || marker == 0xC2 {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]);
            if width == 0 || height == 0 {
                return None;
            }
            return Some(f32::from(width) / f32::from(height));
        }
        index += 1;
    }
    None
}

fn next_jpeg(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        let start = buf.windows(2).position(|pair| pair == [0xFF, 0xD8])?;
        if start > 0 {
            buf.drain(..start);
        }
        if buf.len() < 4 {
            return None;
        }
        let Some(rel) = buf[2..].windows(2).position(|pair| pair == [0xFF, 0xD9]) else {
            if buf.len() > MAX_JPEG {
                let keep = *buf.last()?;
                buf.clear();
                buf.push(keep);
            }
            return None;
        };
        let end = rel + 4;
        if end > MAX_JPEG {
            buf.drain(..2);
            continue;
        }
        let jpeg = buf[..end].to_vec();
        buf.drain(..end);
        return Some(jpeg);
    }
}

fn pump_pcm(mut input: impl Read, queue: &Mutex<VecDeque<i16>>, stop: &AtomicBool) {
    let mut pending = Vec::new();
    let mut chunk = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        let count = match input.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        pending.extend_from_slice(&chunk[..count]);
        let pairs = pending.len() / 2;
        if pairs == 0 {
            continue;
        }
        let mut samples = Vec::with_capacity(pairs);
        for pair in pending[..pairs * 2].chunks_exact(2) {
            samples.push(i16::from_le_bytes([pair[0], pair[1]]));
        }
        pending.drain(..pairs * 2);
        while !stop.load(Ordering::Relaxed) && queued(queue) >= MAX_QUEUED {
            thread::sleep(Duration::from_millis(10));
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        push_samples(queue, samples);
    }
}

fn queued(queue: &Mutex<VecDeque<i16>>) -> usize {
    match queue.lock() {
        Ok(queue) => queue.len(),
        Err(err) => err.into_inner().len(),
    }
}

fn push_samples(queue: &Mutex<VecDeque<i16>>, samples: Vec<i16>) {
    let mut queue = match queue.lock() {
        Ok(queue) => queue,
        Err(err) => err.into_inner(),
    };
    queue.extend(samples);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_recording_link_is_video() {
        let link = media_link("[screen recording](day-1.mp4)").expect("link");
        assert_eq!(link.label, "screen recording");
        assert_eq!(link.href, "day-1.mp4");
        assert_eq!(link.kind, ClipKind::Video);
    }

    #[test]
    fn angle_bracket_link_is_video() {
        let link = media_link("[screen recording](<my day.mp4>)").expect("link");
        assert_eq!(link.href, "my day.mp4");
        assert_eq!(link.kind, ClipKind::Video);
    }

    #[test]
    fn mp3_link_is_audio() {
        let link = media_link("[lecture](talk.mp3)").expect("link");
        assert_eq!(link.kind, ClipKind::Audio);
    }

    #[test]
    fn a_normal_paragraph_is_not_a_clip() {
        assert!(media_link("A normal paragraph").is_none());
        assert!(media_link("See [screen recording](day-1.mp4) later").is_none());
        assert!(media_link("[shot](photo.png)").is_none());
    }

    #[test]
    fn ffmpeg_args_keep_the_path_as_one_argument() {
        let path = Path::new("my day.mp4");
        let rate = Rate { num: 24, den: 1 };
        let video = video_args(path, Some(rate), Duration::ZERO, 1200);
        let audio = audio_args(path, 48_000, 2, Duration::from_secs(3));
        assert_eq!(arg_after(&video, "-i"), path.as_os_str());
        assert_eq!(arg_after(&audio, "-i"), path.as_os_str());
        assert_eq!(arg_after(&audio, "-ss"), "3.000");
        let poster = poster_args(path, Duration::ZERO, 1200);
        assert_eq!(arg_after(&poster, "-i"), path.as_os_str());
        assert_eq!(arg_after(&poster, "-frames:v"), "1");
        assert_eq!(arg_after(&video, "-q:v"), "1");
        assert_eq!(arg_after(&poster, "-q:v"), "1");
        assert_eq!(arg_after(&video, "-pix_fmt"), "yuvj444p");
        assert_eq!(arg_after(&video, "-flush_packets"), "1");
        assert!(video.iter().any(|arg| arg == "-re"));
        let preload = preload_args(path, Some(rate), 1200);
        assert!(!preload.iter().any(|arg| arg == "-re"));
        assert_eq!(arg_after(&preload, "-q:v"), "1");
        assert_eq!(arg_after(&preload, "-pix_fmt"), "yuvj444p");
        assert_eq!(
            arg_after(&preload, "-vf"),
            "fps=24/1,scale=w='min(1200,iw)':h=-2:flags=lanczos"
        );
        assert!(!video.iter().any(|arg| arg == "-ss"));
        assert_eq!(
            arg_after(&video, "-vf"),
            "fps=24/1,scale=w='min(1200,iw)':h=-2:flags=lanczos"
        );
        assert_eq!(
            arg_after(&poster, "-vf"),
            "scale=w='min(1200,iw)':h=-2:flags=lanczos"
        );
        let fast = video_args(path, Some(Rate { num: 120, den: 1 }), Duration::ZERO, 1200);
        assert!(
            arg_after(&fast, "-vf")
                .to_string_lossy()
                .starts_with("fps=60/1,")
        );
        let wide = video_args(path, Some(rate), Duration::ZERO, 4000);
        assert!(
            arg_after(&wide, "-vf")
                .to_string_lossy()
                .contains("min(1920,iw)")
        );
        let narrow = poster_args(path, Duration::ZERO, 100);
        assert!(
            arg_after(&narrow, "-vf")
                .to_string_lossy()
                .contains("min(320,iw)")
        );
        assert!(video.iter().any(|arg| arg == "image2pipe"));
        assert!(audio.iter().any(|arg| arg == "s16le"));
        assert!(!poster.iter().any(|arg| arg == "sh"));
        assert!(!video.iter().any(|arg| arg == "sh"));
        assert!(!audio.iter().any(|arg| arg == "sh"));
    }

    #[test]
    fn frame_width_is_even_and_capped() {
        assert_eq!(frame_width(1200), 1200);
        assert_eq!(frame_width(941), 940);
        assert_eq!(frame_width(100), 320);
        assert_eq!(frame_width(4000), 1920);
    }

    #[test]
    fn jpeg_ratio_reads_the_frame_size() {
        let bytes = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x04, 0x38, 0x07, 0x80,
        ];
        let ratio = jpeg_ratio(&bytes).expect("ratio");
        assert!((ratio - (1920.0 / 1080.0)).abs() < 0.001);
    }

    #[test]
    fn jpeg_frames_split_on_soi_and_eoi() {
        let mut buf = vec![0x00, 0xFF, 0xD8, 0x11, 0xFF, 0xD9, 0xFF, 0xD8];
        let frame = next_jpeg(&mut buf).expect("frame");
        assert_eq!(frame, vec![0xFF, 0xD8, 0x11, 0xFF, 0xD9]);
        assert_eq!(buf, vec![0xFF, 0xD8]);
        assert!(next_jpeg(&mut buf).is_none());
    }

    #[test]
    fn probe_text_reads_rate_and_duration() {
        let timing = parse_probe(
            "codec_type=video|avg_frame_rate=30/1\n\
             codec_type=audio|avg_frame_rate=0/0\n\
             duration=13.933333\n",
        );
        assert_eq!(timing.fps, Some(Rate { num: 30, den: 1 }));
        assert!(parse_rate("0/0").is_none());
        let secs = timing.duration.as_secs_f64();
        assert!((secs - 13.933333).abs() < 0.001);
    }

    #[test]
    fn one_second_at_thirty_fps_shows_thirty_frames() {
        let interval = frame_interval(Some(Rate { num: 30, den: 1 }));
        assert!(interval > Duration::from_millis(30) && interval < Duration::from_millis(40));
        let mut presented = 0u64;
        let mut shown = 0u32;
        let mut elapsed = Duration::ZERO;
        while elapsed < Duration::from_secs(1) {
            if is_frame_due(presented, elapsed, interval) {
                presented = next_presented(presented, elapsed, interval);
                shown += 1;
            }
            elapsed += Duration::from_millis(8);
        }
        assert!((28..=32).contains(&shown), "shown {shown}");
    }

    #[test]
    fn collect_jpegs_stops_at_the_frame_limit() {
        let one = [0xFF, 0xD8, 0x11, 0xFF, 0xD9];
        let mut bytes = Vec::new();
        for _ in 0..FRAME_LIMIT + 5 {
            bytes.extend_from_slice(&one);
        }
        let stop = AtomicBool::new(false);
        let frames = collect_jpegs(bytes.as_slice(), &stop);
        assert_eq!(frames.len(), FRAME_LIMIT);
    }

    #[test]
    fn frame_slot_follows_the_clock() {
        let interval = frame_interval(Some(Rate { num: 30, den: 1 }));
        let slot = frame_slot(Duration::from_millis(500), interval, 30).expect("slot");
        assert!((14..=16).contains(&slot), "slot {slot}");
        assert_eq!(frame_slot(Duration::from_secs(10), interval, 30), Some(29));
        assert!(frame_slot(Duration::ZERO, interval, 0).is_none());
    }

    #[test]
    fn a_full_frame_queue_keeps_the_newest() {
        let slot = Mutex::new(VecDeque::new());
        let stop = AtomicBool::new(false);
        for mark in 0..5u8 {
            enqueue(&slot, vec![mark], &stop);
        }
        let queue = slot.lock().expect("queue");
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0], vec![3]);
        assert_eq!(queue[1], vec![4]);
    }

    fn arg_after<'a>(args: &'a [OsString], flag: &str) -> &'a std::ffi::OsStr {
        let index = args.iter().position(|arg| arg == flag).expect(flag);
        args[index + 1].as_os_str()
    }
}
