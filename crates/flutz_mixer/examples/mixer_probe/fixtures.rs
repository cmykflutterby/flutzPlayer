use super::*;

pub(super) fn probe_identity(strip_id: u64, channel: u8, program: u8) -> MixerStripIdentity {
    MixerStripIdentity {
        strip_id: StripId(strip_id),
        soundfont_id: SoundFontId::new("probe"),
        midi_channel: channel,
        midi_program: program,
        is_percussion: channel == 9,
    }
}

pub(super) fn fixture_strip(
    strip_id: u64,
    soundfont_id: &str,
    channel: u8,
    program: u8,
    amplitude: f32,
    pan: f64,
    controls: MixerStripControls,
    frame_count: usize,
) -> MixerStripInput {
    let frames = (0..frame_count)
        .map(|index| {
            let phase = index as f32 / 512.0;
            let value = (phase * std::f32::consts::TAU * (channel as f32 + 1.0)).sin() * amplitude;
            StereoFrame {
                left: value,
                right: value * 0.75,
            }
        })
        .collect();

    MixerStripInput {
        identity: MixerStripIdentity {
            strip_id: StripId(strip_id),
            soundfont_id: SoundFontId::new(soundfont_id),
            midi_channel: channel,
            midi_program: program,
            is_percussion: channel == 9,
        },
        controls: MixerStripControls { pan, ..controls },
        automatic_processing: true,
        block: AudioBlock { frames },
    }
}

pub(super) fn block_strip(
    strip_id: u64,
    soundfont_id: &str,
    channel: u8,
    program: u8,
    frames: Vec<StereoFrame>,
    controls: MixerStripControls,
) -> MixerStripInput {
    block_strip_with_auto(
        strip_id,
        soundfont_id,
        channel,
        program,
        frames,
        controls,
        true,
    )
}

pub(super) fn block_strip_with_auto(
    strip_id: u64,
    soundfont_id: &str,
    channel: u8,
    program: u8,
    frames: Vec<StereoFrame>,
    controls: MixerStripControls,
    automatic_processing: bool,
) -> MixerStripInput {
    MixerStripInput {
        identity: MixerStripIdentity {
            strip_id: StripId(strip_id),
            soundfont_id: SoundFontId::new(soundfont_id),
            midi_channel: channel,
            midi_program: program,
            is_percussion: channel == 9,
        },
        controls,
        automatic_processing,
        block: AudioBlock { frames },
    }
}
