use std::{f32::consts::TAU, thread, time::Duration};

use flutz_audio_sdl3::{AudioDeviceConfig, SdlAudioOutput};

fn main() {
    let memory = std::env::args().any(|arg| arg == "--memory");
    let config = AudioDeviceConfig::default();
    let mut phase = 0.0f32;
    let phase_step = 440.0 / config.sample_rate as f32;
    let mut output = SdlAudioOutput::open_f32_stream(config, move |samples| {
        for frame in samples.chunks_exact_mut(2) {
            let sample = (phase * TAU).sin() * 0.05;
            frame[0] = sample;
            frame[1] = sample;
            phase = (phase + phase_step).fract();
        }
    })
    .expect("SDL audio stream should open");

    println!("sdl_audio_probe: opened");
    println!("config: {:?}", output.config());
    output.resume().expect("SDL audio stream should resume");
    println!("state_after_resume: {:?}", output.state());
    thread::sleep(Duration::from_millis(250));
    output.pause().expect("SDL audio stream should pause");
    println!("state_after_pause: {:?}", output.state());
    let stats = output.stats();
    let underruns = output.underrun_report();
    let diagnostics = output.diagnostics();
    println!("frames_requested: {}", stats.frames_requested);
    println!("frames_delivered: {}", stats.frames_delivered);
    println!("callbacks: {}", stats.callback_count);
    println!("underruns: {}", underruns.total_underruns);
    println!("last_missing_frames: {}", underruns.last_missing_frames);
    if memory {
        println!(
            "memory.ring_retained_bytes: {}",
            diagnostics.ring_retained_bytes
        );
        println!(
            "memory.callback_scratch_bytes: {}",
            diagnostics.callback_scratch_bytes
        );
        println!(
            "memory.producer_render_block_bytes: {}",
            diagnostics.producer_render_block_bytes
        );
        println!("memory.arena_binding: external-hook");
    }
    println!("diagnostics: {:?}", diagnostics);
}
