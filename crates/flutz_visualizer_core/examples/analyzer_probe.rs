use flutz_visualizer_core::{VisualizerAnalyzer, VisualizerAnalyzerConfig};

const SAMPLE_RATE: u32 = 44_100;
const FRAMES: usize = 4096;

fn main() {
    run_case("silence", &silence(FRAMES));
    run_case("sine_440", &sine(FRAMES, 440.0));
    run_case("sine_1000", &sine(FRAMES, 1_000.0));
    run_case("two_tone", &two_tone(FRAMES, 220.0, 2_000.0));
    run_case("broadband", &broadband(FRAMES));
}

fn run_case(name: &str, mono_samples: &[f32]) {
    let mut analyzer =
        VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(SAMPLE_RATE));
    let delta = mono_samples.len() as f32 / SAMPLE_RATE as f32;
    let frame = analyzer.ingest_mono(mono_samples, delta);
    let dominant = frame.dominant_band_index().unwrap_or(0);
    let dominant_level = frame
        .bands
        .get(dominant)
        .map(|band| band.state.live_level_norm)
        .unwrap_or(0.0);
    print!(
        "{{\"case\":\"{}\",\"band_count\":{},\"dominant_band\":{},\"dominant_level\":{:.6},\"aggregate_peak\":{:.6},\"aggregate_rms\":{:.6},\"levels\":[",
        name,
        frame.band_count(),
        dominant,
        dominant_level,
        frame.aggregate_peak,
        frame.aggregate_rms
    );
    for (index, band) in frame.bands.iter().enumerate() {
        if index > 0 {
            print!(",");
        }
        print!(
            "{{\"i\":{},\"lo\":{:.2},\"c\":{:.2},\"hi\":{:.2},\"live\":{:.6},\"column\":{:.6},\"peak\":{:.6}}}",
            band.definition.band_index,
            band.definition.lower_hz,
            band.definition.center_hz,
            band.definition.upper_hz,
            band.state.live_level_norm,
            band.state.column_level_norm,
            band.state.peak_square_level_norm
        );
    }
    println!("]}}");
}

fn silence(frames: usize) -> Vec<f32> {
    vec![0.0; frames]
}

fn sine(frames: usize, hz: f32) -> Vec<f32> {
    (0..frames)
        .map(|index| {
            let phase = index as f32 * hz * std::f32::consts::TAU / SAMPLE_RATE as f32;
            phase.sin() * 0.75
        })
        .collect()
}

fn two_tone(frames: usize, first_hz: f32, second_hz: f32) -> Vec<f32> {
    let first = sine(frames, first_hz);
    let second = sine(frames, second_hz);
    first
        .into_iter()
        .zip(second)
        .map(|(left, right)| (left + right) * 0.5)
        .collect()
}

fn broadband(frames: usize) -> Vec<f32> {
    let mut state = 0x1234_5678u32;
    (0..frames)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let unit = ((state >> 8) as f32 / 0x00ff_ffff as f32) * 2.0 - 1.0;
            unit * 0.35
        })
        .collect()
}
