pub(super) fn print_report(label: &str, report: &flutz_mixer::MixReport) {
    println!("{label}_output_frames: {}", report.output.frame_count());
    for strip in &report.strips {
        println!(
            "{label}_strip {} channel={} program={} percussion={} peak={:.6} rms={:.6} gain={:.6} smart_mix={:.6} lookahead={:.6} normalization={:.6} audible={} solo_active={}",
            strip.identity.strip_id.0,
            strip.identity.midi_channel,
            strip.identity.midi_program,
            strip.identity.is_percussion,
            strip.input_meter.peak,
            strip.input_meter.rms,
            strip.applied_gain,
            strip.smart_mix_gain,
            strip.lookahead_gain,
            strip.normalization_gain,
            strip.audible,
            strip.solo_active
        );
    }
    println!("{label}_output_peak: {:.6}", report.output_meter.peak);
    println!("{label}_output_rms: {:.6}", report.output_meter.rms);
    println!(
        "{label}_output_headroom: {:.6}",
        report.output_meter.headroom
    );
    println!(
        "{label}_allocation_stats processed_inputs={} allocated_bytes={} prepared_frame_bytes={} output_bytes={} reports_bytes={} scratch_growth_bytes={} retained_scratch_bytes={} retained_prepared_frame_bytes={}",
        report.allocation_stats.processed_inputs,
        report.allocation_stats.allocated_bytes,
        report.allocation_stats.prepared_rendered_frame_bytes,
        report.allocation_stats.output_bytes,
        report.allocation_stats.reports_bytes,
        report.allocation_stats.scratch_growth_bytes,
        report.allocation_stats.retained_scratch_bytes,
        report
            .allocation_stats
            .retained_scratch_prepared_frame_bytes
    );
}
