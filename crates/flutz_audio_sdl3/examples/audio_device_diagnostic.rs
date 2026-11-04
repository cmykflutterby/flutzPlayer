use std::{thread, time::Duration};

use flutz_audio_sdl3::{AudioDeviceConfig, SdlAudioOutput};

fn main() {
    let mut output =
        match SdlAudioOutput::open_f32_stream(AudioDeviceConfig::default(), |samples| {
            samples.fill(0.0);
        }) {
            Ok(output) => output,
            Err(error) => {
                println!("AUDIO_ENVIRONMENT_LIMITATION stage=open detail={error}");
                return;
            }
        };

    if let Err(error) = output.resume() {
        println!("AUDIO_ENVIRONMENT_LIMITATION stage=resume detail={error}");
        return;
    }
    thread::sleep(Duration::from_millis(250));
    let diagnostics = output.diagnostics();
    let device_name = diagnostics
        .opened_device_name
        .as_deref()
        .unwrap_or("unknown");
    if device_name.to_ascii_lowercase().contains("dummy") {
        println!(
            "AUDIO_ENVIRONMENT_LIMITATION stage=device detail=SDL opened non-hardware device '{device_name}'"
        );
        return;
    }
    println!(
        "AUDIO_DEVICE_OK id={} name={} sample_rate={} channels={} state={:?}",
        diagnostics.opened_device_id,
        device_name,
        diagnostics.requested_format.sample_rate,
        diagnostics.requested_format.channels,
        diagnostics.state,
    );
}
