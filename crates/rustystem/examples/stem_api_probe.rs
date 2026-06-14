use rustystem::{
    StemError, StemIdentity, StemRenderBlock, StemRenderMode, StemRenderRequest, StemRenderSet,
};

fn main() {
    let mut failures = 0usize;
    check(
        "whole_soundfont_identity_reports_whole_soundfont_mode",
        whole_soundfont_identity_reports_whole_soundfont_mode(),
        &mut failures,
    );
    check(
        "channel_program_identity_preserves_stem_keys",
        channel_program_identity_preserves_stem_keys(),
        &mut failures,
    );
    check(
        "percussion_identity_uses_percussion_mode",
        percussion_identity_uses_percussion_mode(),
        &mut failures,
    );
    check(
        "render_request_marks_grouped_modes",
        render_request_marks_grouped_modes(),
        &mut failures,
    );
    check(
        "stem_block_rejects_mismatched_channels",
        stem_block_rejects_mismatched_channels(),
        &mut failures,
    );
    check(
        "stem_block_copies_to_interleaved_buffer",
        stem_block_copies_to_interleaved_buffer(),
        &mut failures,
    );
    check(
        "stem_render_set_rejects_frame_count_mismatch",
        stem_render_set_rejects_frame_count_mismatch(),
        &mut failures,
    );

    println!("stem_api_probe: failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}

fn check(name: &str, passed: bool, failures: &mut usize) {
    println!("stem_api_probe_case: {name} pass={passed}");
    if !passed {
        *failures += 1;
    }
}

fn whole_soundfont_identity_reports_whole_soundfont_mode() -> bool {
    let identity = StemIdentity::whole_soundfont("retro_gm");
    identity.soundfont_id == "retro_gm"
        && identity.midi_channel.is_none()
        && identity.midi_program.is_none()
        && !identity.is_percussion
        && identity.mode() == StemRenderMode::WholeSoundFont
}

fn channel_program_identity_preserves_stem_keys() -> bool {
    let identity = StemIdentity::channel_program("retro_gm", 4, 81, false);
    identity.soundfont_id == "retro_gm"
        && identity.midi_channel == Some(4)
        && identity.midi_program == Some(81)
        && !identity.is_percussion
        && identity.mode() == StemRenderMode::ChannelProgram
}

fn percussion_identity_uses_percussion_mode() -> bool {
    let identity = StemIdentity::percussion("retro_gm");
    identity.midi_channel == Some(9)
        && identity.is_percussion
        && identity.mode() == StemRenderMode::Percussion
}

fn render_request_marks_grouped_modes() -> bool {
    let whole = StemRenderRequest::whole_soundfont("retro_gm");
    let channel_program = StemRenderRequest::channel_program("retro_gm");
    !whole.requires_voice_grouping() && channel_program.requires_voice_grouping()
}

fn stem_block_rejects_mismatched_channels() -> bool {
    StemRenderBlock::new(
        StemIdentity::whole_soundfont("retro_gm"),
        vec![0.0, 1.0],
        vec![0.0],
    ) == Err(StemError::MismatchedChannels {
        left_frames: 2,
        right_frames: 1,
    })
}

fn stem_block_copies_to_interleaved_buffer() -> bool {
    let block = StemRenderBlock::whole_soundfont("retro_gm", vec![0.25, 0.5], vec![-0.25, -0.5]);
    let mut output = vec![0.0; 4];
    block.copy_to_interleaved(&mut output).is_ok() && output == vec![0.25, -0.25, 0.5, -0.5]
}

fn stem_render_set_rejects_frame_count_mismatch() -> bool {
    let block = StemRenderBlock::whole_soundfont("retro_gm", vec![0.0, 1.0], vec![0.0, 1.0]);
    StemRenderSet::new(1, vec![block])
        == Err(StemError::FrameCountMismatch {
            expected_frames: 1,
            actual_frames: 2,
        })
}
