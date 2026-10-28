//! Microphone capture and Groq transcription.
//!
//! The API key is read from `GROQ_API_KEY` at request time. It is not written
//! to a note, a config file, or an error string.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, Stream};
use reqwest::blocking::multipart;

use crate::locale;

const GROQ_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const GROQ_MODEL: &str = "whisper-large-v3-turbo";
/// Mono 16-bit samples that stay under Groq's upload limit.
const MAX_SAMPLES: usize = 12_000_000;

pub(crate) struct VoiceOutcome {
    pub text: String,
    pub truncated: bool,
}

struct CapturedAudio {
    samples: Vec<f32>,
    sample_rate: u32,
    truncated: bool,
}

pub(crate) struct VoiceCapture {
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<SampleBuf>>,
    sample_rate: u32,
    thread: Option<JoinHandle<Result<(), String>>>,
}

#[derive(Default)]
struct SampleBuf {
    samples: Vec<f32>,
    truncated: bool,
}

impl VoiceCapture {
    /// Start capture on `device_name`.
    ///
    /// `None` and a blank name use the system default input.
    pub(crate) fn start(device_name: Option<String>) -> Result<Self, String> {
        let device_name = device_name.filter(|name| !name.trim().is_empty());
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(SampleBuf {
            samples: Vec::new(),
            truncated: false,
        }));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let stop_flag = Arc::clone(&stop);
        let buffer = Arc::clone(&samples);
        let thread = thread::spawn(move || capture_loop(stop_flag, buffer, ready_tx, device_name));
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(sample_rate)) => Ok(Self {
                stop,
                samples,
                sample_rate,
                thread: Some(thread),
            }),
            Ok(Err(err)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(locale::text("mic.start").to_string())
            }
        }
    }

    fn stop(mut self) -> Result<CapturedAudio, String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| locale::text("mic.thread").to_string())??;
        }
        let buf = match self.samples.lock() {
            Ok(mut buf) => std::mem::take(&mut *buf),
            Err(poison) => std::mem::take(&mut *poison.into_inner()),
        };
        Ok(CapturedAudio {
            samples: buf.samples,
            sample_rate: self.sample_rate,
            truncated: buf.truncated,
        })
    }
}

impl Drop for VoiceCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Stop `capture`, encode a WAV, and transcribe it.
///
/// An empty clip returns an error and does not call Groq.
/// Model name and optional language sent with one transcript request.
pub(crate) struct SpeechRequest {
    pub model: String,
    pub language: Option<String>,
}

pub(crate) fn voice_to_text(
    capture: VoiceCapture,
    key: String,
    speech: SpeechRequest,
) -> Result<VoiceOutcome, String> {
    let audio = capture.stop()?;
    if audio.samples.is_empty() {
        return Err(locale::text("mic.empty").to_string());
    }
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(locale::text("capture.groq_key").to_string());
    }
    let wav = encode_wav(&audio.samples, audio.sample_rate)?;
    let text = transcribe_wav(&wav, &key, &speech)?;
    Ok(VoiceOutcome {
        text,
        truncated: audio.truncated,
    })
}

/// Names of the input devices the host can open.
///
/// Duplicate and blank names are dropped. A host error returns an empty list.
pub(crate) fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for device in devices {
        let Ok(name) = device.name() else {
            continue;
        };
        let name = name.trim().to_string();
        if name.is_empty() || names.iter().any(|existing| existing == &name) {
            continue;
        }
        names.push(name);
    }
    names
}

fn open_input(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
    match name {
        None => host
            .default_input_device()
            .ok_or_else(|| locale::text("mic.none").to_string()),
        Some(name) => host
            .input_devices()
            .map_err(|err| locale::format("mic.list", &[("err", &err.to_string())]))?
            .find(|device| device.name().ok().as_deref() == Some(name))
            .ok_or_else(|| locale::format("mic.missing", &[("name", name)])),
    }
}

fn capture_loop(
    stop: Arc<AtomicBool>,
    buffer: Arc<Mutex<SampleBuf>>,
    ready: std::sync::mpsc::Sender<Result<u32, String>>,
    device_name: Option<String>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = match open_input(&host, device_name.as_deref()) {
        Ok(device) => device,
        Err(err) => {
            let _ = ready.send(Err(err.clone()));
            return Err(err);
        }
    };
    let config = match device.default_input_config() {
        Ok(config) => config,
        Err(err) => {
            let err = locale::format("mic.open", &[("err", &err.to_string())]);
            let _ = ready.send(Err(err.clone()));
            return Err(err);
        }
    };
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            build_stream::<f32>(&device, &config.into(), Arc::clone(&buffer), channels)
        }
        SampleFormat::I16 => {
            build_stream::<i16>(&device, &config.into(), Arc::clone(&buffer), channels)
        }
        SampleFormat::U16 => {
            build_stream::<u16>(&device, &config.into(), Arc::clone(&buffer), channels)
        }
        _ => Err(locale::text("mic.format").to_string()),
    };
    let stream = match stream {
        Ok(stream) => stream,
        Err(err) => {
            let _ = ready.send(Err(err.clone()));
            return Err(err);
        }
    };
    if let Err(err) = stream.play() {
        let err = locale::format("mic.start_detail", &[("err", &err.to_string())]);
        let _ = ready.send(Err(err.clone()));
        return Err(err);
    }
    let _ = ready.send(Ok(sample_rate));
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<SampleBuf>>,
    channels: u16,
) -> Result<Stream, String>
where
    T: Sample + cpal::SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(channels.max(1));
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                push_mono(&buffer, data, channels);
            },
            |err| {
                let _ = err;
            },
            None,
        )
        .map_err(|err| locale::format("mic.open", &[("err", &err.to_string())]))
}

fn push_mono<T>(buffer: &Mutex<SampleBuf>, data: &[T], channels: usize)
where
    T: Sample,
    f32: FromSample<T>,
{
    let mut buf = match buffer.lock() {
        Ok(buf) => buf,
        Err(poison) => poison.into_inner(),
    };
    if buf.samples.len() >= MAX_SAMPLES {
        buf.truncated = true;
        return;
    }
    if channels <= 1 {
        for sample in data {
            if buf.samples.len() >= MAX_SAMPLES {
                buf.truncated = true;
                break;
            }
            buf.samples.push(sample.to_sample::<f32>());
        }
        return;
    }
    for frame in data.chunks(channels) {
        if buf.samples.len() >= MAX_SAMPLES {
            buf.truncated = true;
            break;
        }
        let sum: f32 = frame.iter().map(|sample| sample.to_sample::<f32>()).sum();
        buf.samples.push(sum / channels as f32);
    }
}

pub(crate) fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, String> {
    if sample_rate == 0 {
        return Err(locale::text("mic.rate").to_string());
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec)
        .map_err(|_| locale::text("mic.encode").to_string())?;
    for sample in samples {
        let clipped = sample.clamp(-1.0, 1.0);
        let value = (clipped * f32::from(i16::MAX)) as i16;
        writer
            .write_sample(value)
            .map_err(|_| locale::text("mic.encode").to_string())?;
    }
    writer
        .finalize()
        .map_err(|_| locale::text("mic.encode").to_string())?;
    Ok(cursor.into_inner())
}

fn transcribe_wav(wav: &[u8], api_key: &str, speech: &SpeechRequest) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| locale::text("mic.groq_client").to_string())?;
    let part = multipart::Part::bytes(wav.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|_| locale::text("mic.encode").to_string())?;
    let model = if speech.model.trim().is_empty() {
        GROQ_MODEL
    } else {
        speech.model.as_str()
    };
    let mut form = multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "json");
    if let Some(language) = speech
        .language
        .as_deref()
        .map(str::trim)
        .filter(|language| !language.is_empty())
    {
        form = form.text("language", language.to_string());
    }
    let response = client
        .post(GROQ_URL)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .map_err(|_| locale::text("mic.groq_reach").to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(locale::format(
            "mic.groq_failed",
            &[("status", &status.to_string())],
        ));
    }
    let body = response
        .text()
        .map_err(|_| locale::text("mic.groq_unreadable").to_string())?;
    transcript_from_json(&body).map_err(|_| locale::text("mic.groq_unexpected").to_string())
}

pub(crate) fn transcript_from_json(body: &str) -> Result<String, ()> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| ())?;
    let text = value
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_reads_text_and_rejects_invalid_json() {
        assert_eq!(
            transcript_from_json(r#"{"text":" hello "}"#).unwrap(),
            "hello"
        );
        assert_eq!(transcript_from_json("{}").unwrap(), "");
        assert!(transcript_from_json("nope").is_err());
    }

    #[test]
    fn encode_wav_round_trips_one_sample() {
        let wav = encode_wav(&[0.5], 16_000).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        let samples: Vec<i16> = reader.samples().map(|sample| sample.unwrap()).collect();
        assert_eq!(samples.len(), 1);
        assert!(samples[0] > 16_000);
    }

    #[test]
    fn downmix_averages_stereo_frames() {
        let buffer = Mutex::new(SampleBuf::default());
        push_mono(&buffer, &[1.0_f32, 3.0, 2.0, 4.0], 2);
        let buf = buffer.lock().unwrap();
        assert_eq!(buf.samples, vec![2.0, 3.0]);
    }
}
