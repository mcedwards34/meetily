use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};

use crate::audio::devices::configuration::{AudioDevice, DeviceType};

/// Configure Linux audio devices using ALSA.
///
/// System-audio (playback sink) entries are intentionally *not* enumerated here:
/// PipeWire/PulseAudio monitor sources are virtual objects in the PipeWire graph and
/// are not visible through ALSA's `snd_device_name_hint` enumeration, so they can't be
/// discovered this way. Plain hardware sinks are still picked up via the `discovery.rs`
/// fallback (`host.devices()`), and monitor-source resolution for capture happens at
/// stream-open time via `audio::capture::system::resolve_monitor_source`, which talks
/// to PulseAudio/PipeWire directly (through `pactl`) instead of going through ALSA.
pub fn configure_linux_audio(host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();

    // Add input devices
    for device in host.input_devices()? {
        if let Ok(name) = device.name() {
            devices.push(AudioDevice::new(name, DeviceType::Input));
        }
    }

    Ok(devices)
}