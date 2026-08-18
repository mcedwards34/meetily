use std::pin::Pin;
use std::task::{Context, Poll};
use futures_util::{Stream, StreamExt};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};


#[cfg(target_os = "macos")]
use futures_channel::mpsc;
#[cfg(target_os = "macos")]
use super::core_audio::CoreAudioCapture;
#[cfg(target_os = "macos")]
use log::info;

/// System audio capture using Core Audio tap (macOS) or CPAL (other platforms)
pub struct SystemAudioCapture {
    _host: cpal::Host,
}

impl SystemAudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        Ok(Self { _host: host })
    }

    pub fn list_system_devices() -> Result<Vec<String>> {
        let host = cpal::default_host();
        let devices = host.output_devices()
            .map_err(|e| anyhow::anyhow!("Failed to enumerate output devices: {}", e))?;

        let mut device_names = Vec::new();
        for device in devices {
            if let Ok(name) = device.name() {
                device_names.push(name);
            }
        }

        Ok(device_names)
    }

    pub fn start_system_audio_capture(&self) -> Result<SystemAudioStream> {
        #[cfg(target_os = "macos")]
        {
            info!("Starting Core Audio system capture (macOS)");
            // Use Core Audio tap for system audio capture
            let core_audio = CoreAudioCapture::new()?;
            let core_audio_stream = core_audio.stream()?;
            let sample_rate = core_audio_stream.sample_rate();

            // Convert CoreAudioStream to SystemAudioStream
            let (tx, rx) = mpsc::unbounded::<Vec<f32>>();
            let (drop_tx, drop_rx) = std::sync::mpsc::channel::<()>();

            // Spawn task to forward Core Audio samples
            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut stream = core_audio_stream;
                let mut buffer = Vec::new();
                let chunk_size = 1024;

                loop {
                    // Check if we should stop
                    if drop_rx.try_recv().is_ok() {
                        break;
                    }

                    // Poll the Core Audio stream
                    match stream.next().await {
                        Some(sample) => {
                            buffer.push(sample);
                            if buffer.len() >= chunk_size {
                                if tx.unbounded_send(buffer.clone()).is_err() {
                                    break;
                                }
                                buffer.clear();
                            }
                        }
                        None => break,
                    }
                }

                // Send any remaining samples
                if !buffer.is_empty() {
                    let _ = tx.unbounded_send(buffer);
                }
            });

            let receiver = rx.map(futures_util::stream::iter).flatten();

            info!("Core Audio system capture started successfully");

            Ok(SystemAudioStream {
                drop_tx,
                sample_rate,
                receiver: Box::pin(receiver),
            })
        }

        #[cfg(not(target_os = "macos"))]
        {
            // For non-macOS platforms, you would implement WASAPI/ALSA loopback here
            anyhow::bail!("System audio capture not yet implemented for this platform")
        }
    }

    pub fn check_system_audio_permissions() -> bool {
        // Check if we can enumerate audio devices
        match cpal::default_host().output_devices() {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

pub struct SystemAudioStream {
    drop_tx: std::sync::mpsc::Sender<()>,
    sample_rate: u32,
    receiver: Pin<Box<dyn Stream<Item = f32> + Send + Sync>>,
}

impl Drop for SystemAudioStream {
    fn drop(&mut self) {
        let _ = self.drop_tx.send(());
    }
}

impl Stream for SystemAudioStream {
    type Item = f32;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.as_mut().poll_next_unpin(cx)
    }
}

impl SystemAudioStream {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Public interface for system audio capture
pub async fn start_system_audio_capture() -> Result<SystemAudioStream> {
    let capture = SystemAudioCapture::new()?;
    capture.start_system_audio_capture()
}

pub fn list_system_audio_devices() -> Result<Vec<String>> {
    SystemAudioCapture::list_system_devices()
}

pub fn check_system_audio_permissions() -> bool {
    SystemAudioCapture::check_system_audio_permissions()
}

// ---------------------------------------------------------------------------
// Linux system-audio capture via PulseAudio/PipeWire
//
// cpal's ALSA host cannot see or open PipeWire/PulseAudio monitor sources (they're
// virtual objects in the PipeWire graph, not ALSA hardware PCMs), so this bypasses
// cpal entirely for Linux system audio -- mirroring how the macOS path bypasses cpal
// for Core Audio (see `AudioStream::create_core_audio_stream` in stream.rs). Both
// paths feed samples into the same `AudioCapture::process_audio_data` integration
// point used by the cpal callback path.
// ---------------------------------------------------------------------------

/// Handle to a running PulseAudio monitor-source capture thread.
#[cfg(target_os = "linux")]
pub struct PulseAudioCaptureHandle {
    thread: Option<std::thread::JoinHandle<()>>,
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(target_os = "linux")]
impl PulseAudioCaptureHandle {
    /// Signal the capture thread to stop and wait for it to exit.
    pub fn stop(mut self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "linux")]
struct PactlSink {
    name: String,
    description: String,
    monitor_source: String,
}

/// Parse the block-formatted output of `pactl list sinks` into sink records.
#[cfg(target_os = "linux")]
fn parse_pactl_sinks(output: &str) -> Vec<PactlSink> {
    fn flush(sinks: &mut Vec<PactlSink>, current: Option<(Option<String>, Option<String>, Option<String>)>) {
        if let Some((Some(name), description, Some(monitor_source))) = current {
            sinks.push(PactlSink {
                name,
                description: description.unwrap_or_default(),
                monitor_source,
            });
        }
    }

    let mut sinks = Vec::new();
    let mut current: Option<(Option<String>, Option<String>, Option<String>)> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Sink #") {
            flush(&mut sinks, current.take());
            current = Some((None, None, None));
        } else if let Some(rest) = trimmed.strip_prefix("Name: ") {
            if let Some(c) = current.as_mut() {
                c.0 = Some(rest.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("Description: ") {
            if let Some(c) = current.as_mut() {
                c.1 = Some(rest.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("Monitor Source: ") {
            if let Some(c) = current.as_mut() {
                c.2 = Some(rest.to_string());
            }
        }
    }
    flush(&mut sinks, current);

    sinks
}

/// Resolve the PulseAudio/PipeWire monitor source for the given output sink name (or
/// the current default sink if `sink_name` is `None`), by shelling out to `pactl`.
///
/// `sink_name` here is whatever the rest of the app already has as an `AudioDevice`
/// name for the selected output device -- on Linux that's a cpal/ALSA-derived name,
/// not a PulseAudio name, so an exact match is only expected to succeed when it
/// happens to coincide; the fallbacks below exist for that reason.
#[cfg(target_os = "linux")]
pub fn resolve_monitor_source(sink_name: Option<&str>) -> anyhow::Result<String> {
    use std::process::Command;

    let default_sink_output = Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `pactl get-default-sink`: {}", e))?;
    let default_sink = String::from_utf8_lossy(&default_sink_output.stdout)
        .trim()
        .to_string();

    let sinks_output = Command::new("pactl")
        .args(["list", "sinks"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `pactl list sinks`: {}", e))?;
    let sinks = parse_pactl_sinks(&String::from_utf8_lossy(&sinks_output.stdout));

    if sinks.is_empty() {
        anyhow::bail!("pactl reported no sinks; is PulseAudio/PipeWire running?");
    }

    if let Some(name) = sink_name {
        if let Some(sink) = sinks.iter().find(|s| s.name == name) {
            return Ok(sink.monitor_source.clone());
        }

        // cpal (ALSA) device names and PulseAudio sink descriptions are different
        // namespaces (see meetily issue #437) -- try a case-insensitive substring
        // match as a best-effort fallback before giving up on the requested device.
        let needle = name.to_lowercase();
        if let Some(sink) = sinks
            .iter()
            .find(|s| s.description.to_lowercase().contains(&needle))
        {
            log::warn!(
                "Linux system audio: no exact PulseAudio sink match for '{}', using best-effort match '{}'",
                name, sink.description
            );
            return Ok(sink.monitor_source.clone());
        }

        log::warn!(
            "Linux system audio: could not resolve sink '{}' via PulseAudio, falling back to the default sink",
            name
        );
    }

    if let Some(sink) = sinks.iter().find(|s| s.name == default_sink) {
        return Ok(sink.monitor_source.clone());
    }

    sinks
        .first()
        .map(|s| s.monitor_source.clone())
        .ok_or_else(|| anyhow::anyhow!("No PulseAudio sinks available to resolve a monitor source from"))
}

/// Open a blocking PulseAudio recording stream against `monitor_source` and spawn a
/// dedicated thread that reads PCM from it and feeds `AudioCapture::process_audio_data`
/// -- the same integration point the cpal and Core Audio capture paths use.
#[cfg(target_os = "linux")]
pub fn start_monitor_capture(
    monitor_source: &str,
    capture: crate::audio::pipeline::AudioCapture,
) -> anyhow::Result<PulseAudioCaptureHandle> {
    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;

    // Request mono 48kHz directly from the PulseAudio server so it resamples
    // server-side; AudioCapture is constructed with this same rate, so its own
    // resampler stays disabled (see pipeline.rs's `needs_resampling` check).
    let spec = Spec {
        format: Format::FLOAT32NE,
        channels: 1,
        rate: 48_000,
    };
    if !spec.is_valid() {
        anyhow::bail!("Invalid PulseAudio sample spec (format/channels/rate)");
    }

    let simple = Simple::new(
        None, // default server
        "Meetily",
        Direction::Record,
        Some(monitor_source),
        "system-audio",
        &spec,
        None, // default channel map
        None, // default buffering attributes
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "Failed to open PulseAudio monitor source '{}': {}",
            monitor_source,
            e
        )
    })?;

    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop_flag = stop_flag.clone();
    let monitor_source_owned = monitor_source.to_string();

    let thread = std::thread::spawn(move || {
        // ~20ms @ 48kHz mono: frequent enough to keep stop() latency low without
        // spinning, matching the 1024-sample chunking used by the Core Audio path.
        const CHUNK_SAMPLES: usize = 960;
        let mut byte_buf = vec![0u8; CHUNK_SAMPLES * std::mem::size_of::<f32>()];

        log::info!(
            "PulseAudio system-audio capture started for monitor source '{}'",
            monitor_source_owned
        );

        while !thread_stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            if let Err(e) = simple.read(&mut byte_buf) {
                log::error!("PulseAudio read error on '{}': {}", monitor_source_owned, e);
                break;
            }

            let samples: Vec<f32> = byte_buf
                .chunks_exact(4)
                .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .collect();

            capture.process_audio_data(&samples);
        }

        log::info!(
            "PulseAudio system-audio capture stopped for monitor source '{}'",
            monitor_source_owned
        );
    });

    Ok(PulseAudioCaptureHandle {
        thread: Some(thread),
        stop_flag,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod linux_pulseaudio_tests {
    use super::*;
    use crate::audio::devices::configuration::{AudioDevice, DeviceType as ConfigDeviceType};
    use crate::audio::pipeline::AudioCapture;
    use crate::audio::recording_state::{AudioChunk, DeviceType as RecordingDeviceType, RecordingState};
    use std::sync::Arc;

    /// Empirical check against the real PulseAudio/PipeWire server on this machine:
    /// plays a 1kHz tone on the default sink while the monitor-source capture path
    /// runs, then asserts captured RMS energy is well above the silence floor. This
    /// is the same code path `AudioStream::create_pulseaudio_stream` uses in the real
    /// app. Requires a working PipeWire/PulseAudio session and `speaker-test`
    /// (alsa-utils) -- not suitable for CI, run explicitly with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn captures_real_audio_from_default_monitor() {
        let monitor =
            resolve_monitor_source(None).expect("failed to resolve default monitor source");
        println!("Resolved monitor source: {}", monitor);

        let state = RecordingState::new();
        state.start_recording().unwrap();

        // AudioCapture::process_audio_data delivers chunks via RecordingState's own
        // audio_sender (set here), not the `recording_sender` constructor arg below --
        // that field is unused dead code shared by the cpal/CoreAudio/PulseAudio paths
        // alike (see the pre-existing `field recording_sender is never read` warning).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AudioChunk>();
        state.set_audio_sender(tx);

        let device = Arc::new(AudioDevice::new(
            "test-system-audio".to_string(),
            ConfigDeviceType::Output,
        ));
        let capture = AudioCapture::new(
            device,
            state.clone(),
            48_000,
            1,
            RecordingDeviceType::System,
            None,
        );

        let handle =
            start_monitor_capture(&monitor, capture).expect("failed to start monitor capture");

        // Drain ~500ms of silence first to measure the noise floor.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut silence_samples: Vec<f32> = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            silence_samples.extend(chunk.data);
        }

        // Play a 1kHz test tone on the default sink for ~2 seconds.
        let mut player = std::process::Command::new("speaker-test")
            .args(["-t", "sine", "-f", "1000", "-l", "1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn speaker-test (is alsa-utils installed?)");
        std::thread::sleep(std::time::Duration::from_millis(2000));
        let _ = player.kill();
        let _ = player.wait();

        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut tone_samples: Vec<f32> = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            tone_samples.extend(chunk.data);
        }

        handle.stop();

        fn rms(samples: &[f32]) -> f32 {
            if samples.is_empty() {
                return 0.0;
            }
            (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
        }

        let silence_rms = rms(&silence_samples);
        let tone_rms = rms(&tone_samples);
        println!(
            "silence_rms={:.6} ({} samples), tone_rms={:.6} ({} samples)",
            silence_rms,
            silence_samples.len(),
            tone_rms,
            tone_samples.len()
        );

        assert!(
            !tone_samples.is_empty(),
            "captured zero samples while the tone was playing"
        );
        assert!(
            tone_rms > 0.01,
            "captured audio during tone playback has near-zero energy (RMS={:.6}) -- system audio capture is not working",
            tone_rms
        );
        assert!(
            tone_rms > silence_rms * 5.0,
            "tone RMS ({:.6}) is not meaningfully louder than silence RMS ({:.6})",
            tone_rms,
            silence_rms
        );
    }

    /// Confirms the capture thread starts and joins cleanly across repeated
    /// start/stop cycles -- a blocking PulseAudio read loop can't be `.abort()`ed like
    /// a tokio task, so `stop()` must reliably terminate it via the stop flag instead
    /// of hanging. Same ignore/run rationale as the test above.
    #[test]
    #[ignore]
    fn start_stop_cycles_do_not_hang() {
        let monitor =
            resolve_monitor_source(None).expect("failed to resolve default monitor source");

        for i in 0..3 {
            let state = RecordingState::new();
            state.start_recording().unwrap();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<AudioChunk>();
            state.set_audio_sender(tx);

            let device = Arc::new(AudioDevice::new(
                format!("test-system-audio-{}", i),
                ConfigDeviceType::Output,
            ));
            let capture = AudioCapture::new(
                device,
                state.clone(),
                48_000,
                1,
                RecordingDeviceType::System,
                None,
            );

            let handle = start_monitor_capture(&monitor, capture)
                .unwrap_or_else(|e| panic!("cycle {}: failed to start capture: {}", i, e));
            std::thread::sleep(std::time::Duration::from_millis(150));
            handle.stop();
            println!("cycle {} started and stopped cleanly", i);
        }
    }
}