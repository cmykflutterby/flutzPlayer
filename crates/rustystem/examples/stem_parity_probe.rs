use std::{
    collections::BTreeMap,
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use flutz_core::{SoundFontId, StripId};
use flutz_dat::{
    assets::DAT_ENTRY_FLAG_DEFAULT,
    read::{extract_all_entries, read_dat_file},
};
use flutz_mixer::{
    AudioBlock, MeterReading, MixerEngine, MixerSettings, MixerStripControls, MixerStripIdentity,
    MixerStripInput, StereoFrame,
};
use rustystem::{
    MidiFile, MidiFileSequencer, SoundFont, StemRenderBlock, StemRenderRequest, Synthesizer,
    SynthesizerSettings,
};

fn main() {
    let args = ProbeArgs::parse(env::args().skip(1).collect());
    let mut report_lines = Vec::new();
    let midi_files = discover_midi_files(&args.midi_path);
    if midi_files.is_empty() {
        report_line(
            &mut report_lines,
            "stem_parity_probe: no_midi_files path={}",
            &[args.midi_path.display().to_string()],
        );
        write_report(&report_lines);
        return;
    }

    let dat_soundfont = load_dat_soundfont(&args.dat_path, args.soundfont_id.as_deref());
    report_line(&mut report_lines, "stem_parity_probe: ok", &[]);
    report_line(&mut report_lines, "soundfont_source: dat", &[]);
    report_line(
        &mut report_lines,
        "dat_input: {}",
        &[args.dat_path.display().to_string()],
    );
    report_line(
        &mut report_lines,
        "soundfont_id: {}",
        std::slice::from_ref(&dat_soundfont.internal_id),
    );
    report_line(
        &mut report_lines,
        "soundfont_dat_files: {}",
        &[dat_soundfont.dat_files.join(", ")],
    );
    report_line(
        &mut report_lines,
        "soundfont_bytes: {}",
        &[dat_soundfont.bytes.len().to_string()],
    );
    report_line(
        &mut report_lines,
        "midi_file_count: {}",
        &[midi_files.len().to_string()],
    );

    let soundfont_id = dat_soundfont.internal_id.clone();
    let dat_files = dat_soundfont.dat_files.join(", ");
    let mut cursor = Cursor::new(dat_soundfont.bytes);
    let soundfont = match SoundFont::new(&mut cursor) {
        Ok(soundfont) => Arc::new(soundfont),
        Err(error) => {
            eprintln!(
                "stem_parity_probe: dat_soundfont_load_error id={soundfont_id} dat_files={dat_files} error={error:?}; DAT soundfont bytes should already be RustyStem/RustySynth-compatible, so revalidate DAT packing/unpacking byte identity for this entry"
            );
            std::process::exit(1);
        }
    };
    let settings = synth_settings();

    for midi_path in midi_files {
        let result = run_one(
            &soundfont,
            &settings,
            &midi_path,
            args.frames,
            &soundfont_id,
        );
        report_line(
            &mut report_lines,
            "case: {}",
            &[midi_path.display().to_string()],
        );
        report_line(
            &mut report_lines,
            "frames: {}",
            &[result.frames.to_string()],
        );
        report_line(
            &mut report_lines,
            "stem_count: {}",
            &[result.stem_count.to_string()],
        );
        report_line(
            &mut report_lines,
            "visible_stem_count: {}",
            &[result.visible_stem_count.to_string()],
        );
        report_line(
            &mut report_lines,
            "global_residual_stem_count: {}",
            &[result.global_residual_stem_count.to_string()],
        );
        report_line(
            &mut report_lines,
            "global_residual_peak: {:.9}",
            &[format!("{:.9}", result.global_residual_meter.peak)],
        );
        report_line(
            &mut report_lines,
            "global_residual_rms: {:.9}",
            &[format!("{:.9}", result.global_residual_meter.rms)],
        );
        report_line(
            &mut report_lines,
            "row_mute_leak_peak: {:.9}",
            &[format!("{:.9}", result.row_mute_leak_meter.peak)],
        );
        report_line(
            &mut report_lines,
            "row_mute_leak_rms: {:.9}",
            &[format!("{:.9}", result.row_mute_leak_meter.rms)],
        );
        report_line(
            &mut report_lines,
            "direct_peak: {:.9}",
            &[format!("{:.9}", result.direct_meter.peak)],
        );
        report_line(
            &mut report_lines,
            "mixed_peak: {:.9}",
            &[format!("{:.9}", result.mixed_meter.peak)],
        );
        report_line(
            &mut report_lines,
            "direct_rms: {:.9}",
            &[format!("{:.9}", result.direct_meter.rms)],
        );
        report_line(
            &mut report_lines,
            "mixed_rms: {:.9}",
            &[format!("{:.9}", result.mixed_meter.rms)],
        );
        report_line(
            &mut report_lines,
            "raw_sum_max_abs_diff: {:.9}",
            &[format!("{:.9}", result.raw_sum_max_abs_diff)],
        );
        report_line(
            &mut report_lines,
            "raw_sum_rms_diff: {:.9}",
            &[format!("{:.9}", result.raw_sum_rms_diff)],
        );
        report_line(
            &mut report_lines,
            "max_abs_diff: {:.9}",
            &[format!("{:.9}", result.max_abs_diff)],
        );
        report_line(
            &mut report_lines,
            "rms_diff: {:.9}",
            &[format!("{:.9}", result.rms_diff)],
        );
        for stem in &result.stems {
            report_raw(
                &mut report_lines,
                format!(
                    "stem_index: {} id={} ch={} program={} percussion={} global={} peak={:.9} rms={:.9}",
                    stem.index,
                    stem.soundfont_id,
                    stem.channel
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_owned()),
                    stem.program
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_owned()),
                    stem.is_percussion,
                    stem.is_global_residual,
                    stem.meter.peak,
                    stem.meter.rms,
                ),
            );
        }
        for control in &result.controls {
            report_raw(
                &mut report_lines,
                format!(
                    "control_case: stem_index={} ch={} program={} percussion={} mute_leak_max_abs_diff={:.9} mute_leak_rms_diff={:.9} mute_output_peak={:.9} solo_max_abs_diff={:.9} solo_rms_diff={:.9} solo_output_peak={:.9}",
                    control.stem_index,
                    control.channel,
                    control.program,
                    control.is_percussion,
                    control.mute_leak_max_abs_diff,
                    control.mute_leak_rms_diff,
                    control.mute_output_meter.peak,
                    control.solo_max_abs_diff,
                    control.solo_rms_diff,
                    control.solo_output_meter.peak,
                ),
            );
        }
    }
    write_report(&report_lines);
}

fn report_line(lines: &mut Vec<String>, template: &str, args: &[String]) {
    let mut line = template.to_owned();
    for arg in args {
        line = line.replacen("{}", arg, 1);
        line = line.replacen("{:.9}", arg, 1);
    }
    println!("{line}");
    lines.push(line);
}

fn report_raw(lines: &mut Vec<String>, line: String) {
    println!("{line}");
    lines.push(line);
}

fn write_report(lines: &[String]) {
    let report_dir = Path::new("_local/runtime-tests/measurements");
    if let Err(error) = fs::create_dir_all(report_dir) {
        eprintln!(
            "stem_parity_probe: report_dir_error path={} error={error}",
            report_dir.display()
        );
        return;
    }
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let report_path = report_dir.join(format!("stem-parity-probe-{seconds}.txt"));
    if let Err(error) = fs::write(&report_path, lines.join("\n")) {
        eprintln!(
            "stem_parity_probe: report_write_error path={} error={error}",
            report_path.display()
        );
    } else {
        println!("report_path: {}", report_path.display());
    }
}

fn run_one(
    soundfont: &Arc<SoundFont>,
    settings: &SynthesizerSettings,
    midi_path: &Path,
    frames: usize,
    soundfont_id: &str,
) -> ParityResult {
    let midi_bytes = fs::read(midi_path).expect("MIDI should be readable");
    let mut midi_cursor = Cursor::new(midi_bytes);
    let midi = Arc::new(MidiFile::new(&mut midi_cursor).expect("MIDI should load"));

    let mut direct = render_direct(soundfont, settings, &midi, frames);
    let stems = render_channel_program_stems(soundfont, settings, &midi, frames, soundfont_id);
    let mut raw_sum = sum_stems(&stems, frames);
    let mut mixed = mix_stems(&stems, frames);
    let stem_reports = describe_stems(&stems);
    let global_residual_stem_count = stem_reports
        .iter()
        .filter(|stem| stem.is_global_residual)
        .count();
    let visible_stem_count = stem_reports
        .len()
        .saturating_sub(global_residual_stem_count);
    let global_residual_meter =
        MeterReading::from_interleaved(&sum_global_residual_stems(&stems, frames));
    let row_mute_output = mix_stems_with_app_controls(&stems, frames, |_, stem| {
        if is_global_residual_stem(stem) {
            MixerStripControls::default()
        } else {
            MixerStripControls {
                mute: true,
                ..MixerStripControls::default()
            }
        }
    });
    let row_mute_leak_meter = MeterReading::from_interleaved(&row_mute_output);
    let control_reports = control_probe_reports(&stems, frames);

    let direct_meter = MeterReading::from_interleaved(&direct);
    let mixed_meter = MeterReading::from_interleaved(&mixed);
    let mut direct_for_raw_sum = direct.clone();
    let (raw_sum_max_abs_diff, raw_sum_rms_diff) = diff(&mut direct_for_raw_sum, &mut raw_sum);
    let (max_abs_diff, rms_diff) = diff(&mut direct, &mut mixed);

    ParityResult {
        frames,
        stem_count: stems.len(),
        visible_stem_count,
        global_residual_stem_count,
        global_residual_meter,
        row_mute_leak_meter,
        stems: stem_reports,
        controls: control_reports,
        direct_meter,
        mixed_meter,
        raw_sum_max_abs_diff,
        raw_sum_rms_diff,
        max_abs_diff,
        rms_diff,
    }
}

fn render_direct(
    soundfont: &Arc<SoundFont>,
    settings: &SynthesizerSettings,
    midi: &Arc<MidiFile>,
    frames: usize,
) -> Vec<f32> {
    let synthesizer = Synthesizer::new(soundfont, settings).expect("synthesizer should create");
    let mut sequencer = MidiFileSequencer::new(synthesizer);
    sequencer.play(midi, false);
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    sequencer.render(&mut left, &mut right);
    interleave(&left, &right)
}

fn render_channel_program_stems(
    soundfont: &Arc<SoundFont>,
    settings: &SynthesizerSettings,
    midi: &Arc<MidiFile>,
    frames: usize,
    soundfont_id: &str,
) -> Vec<StemRenderBlock> {
    let synthesizer = Synthesizer::new(soundfont, settings).expect("synthesizer should create");
    let mut sequencer = MidiFileSequencer::new(synthesizer);
    sequencer.play(midi, false);
    let request = StemRenderRequest::channel_program(soundfont_id);
    sequencer.render_stems(&request, frames).blocks
}

fn mix_stems(stems: &[StemRenderBlock], frames: usize) -> Vec<f32> {
    mix_stems_with_controls(stems, frames, |_, _| MixerStripControls::default())
}

fn mix_stems_with_controls(
    stems: &[StemRenderBlock],
    frames: usize,
    controls_for: impl Fn(usize, &StemRenderBlock) -> MixerStripControls,
) -> Vec<f32> {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let inputs = stems
        .iter()
        .enumerate()
        .map(|(index, stem)| MixerStripInput {
            identity: MixerStripIdentity {
                strip_id: StripId(index as u64 + 1),
                soundfont_id: SoundFontId::new(stem.identity.soundfont_id.clone()),
                midi_channel: stem.identity.midi_channel.unwrap_or(0),
                midi_program: stem.identity.midi_program.unwrap_or(0),
                is_percussion: stem.identity.is_percussion,
            },
            controls: controls_for(index, stem),
            automatic_processing: !is_global_residual_stem(stem),
            block: AudioBlock {
                frames: (0..stem.frame_count())
                    .map(|frame| StereoFrame {
                        left: stem.left[frame],
                        right: stem.right[frame],
                    })
                    .collect(),
            },
        })
        .collect::<Vec<_>>();
    let report = engine.mix(&inputs).expect("neutral mixer should mix stems");
    let mut output = vec![0.0; frames * 2];
    for (index, frame) in report.output.frames.iter().enumerate() {
        output[index * 2] = frame.left;
        output[index * 2 + 1] = frame.right;
    }
    output
}

fn mix_stems_with_app_controls(
    stems: &[StemRenderBlock],
    frames: usize,
    controls_for_visible: impl Fn(usize, &StemRenderBlock) -> MixerStripControls,
) -> Vec<f32> {
    let visible_controls = stems
        .iter()
        .enumerate()
        .filter(|(_, stem)| !is_global_residual_stem(stem))
        .map(|(index, stem)| controls_for_visible(index, stem))
        .collect::<Vec<_>>();
    let residual_controls = residual_controls_from_visible(&visible_controls);

    mix_stems_with_controls(stems, frames, |index, stem| {
        if is_global_residual_stem(stem) {
            residual_controls
        } else {
            controls_for_visible(index, stem)
        }
    })
}

fn residual_controls_from_visible(visible_controls: &[MixerStripControls]) -> MixerStripControls {
    if visible_controls.is_empty() {
        return MixerStripControls::default();
    }

    let muted = visible_controls.iter().any(|controls| controls.mute);
    let solo_count = visible_controls
        .iter()
        .filter(|controls| controls.solo)
        .count();
    let partial_solo = solo_count > 0 && solo_count < visible_controls.len();
    let row_solo = solo_count == visible_controls.len();

    MixerStripControls {
        mute: muted || partial_solo,
        solo: row_solo,
        ..MixerStripControls::default()
    }
}

fn control_probe_reports(stems: &[StemRenderBlock], frames: usize) -> Vec<ControlProbeReport> {
    let visible_stem_count = stems
        .iter()
        .filter(|stem| !is_global_residual_stem(stem))
        .count();

    stems
        .iter()
        .enumerate()
        .filter(|(_, stem)| !is_global_residual_stem(stem))
        .map(|(target_index, target_stem)| {
            let mute_output = mix_stems_with_app_controls(stems, frames, |index, stem| {
                if index == target_index {
                    MixerStripControls {
                        mute: true,
                        ..MixerStripControls::default()
                    }
                } else if is_global_residual_stem(stem) {
                    MixerStripControls::default()
                } else {
                    MixerStripControls::default()
                }
            });
            let mut expected_mute = sum_stems_matching(stems, frames, |index, stem| {
                index != target_index && !is_global_residual_stem(stem)
            });
            let mut mute_output_for_diff = mute_output.clone();
            let (mute_leak_max_abs_diff, mute_leak_rms_diff) =
                diff(&mut mute_output_for_diff, &mut expected_mute);

            let solo_output = mix_stems_with_app_controls(stems, frames, |index, _| {
                if index == target_index {
                    MixerStripControls {
                        solo: true,
                        ..MixerStripControls::default()
                    }
                } else {
                    MixerStripControls::default()
                }
            });
            let mut expected_solo = if visible_stem_count == 1 {
                sum_stems_matching(stems, frames, |index, stem| {
                    index == target_index || is_global_residual_stem(stem)
                })
            } else {
                stem_to_interleaved(target_stem, frames)
            };
            let mut solo_output_for_diff = solo_output.clone();
            let (solo_max_abs_diff, solo_rms_diff) =
                diff(&mut solo_output_for_diff, &mut expected_solo);

            ControlProbeReport {
                stem_index: target_index,
                channel: target_stem.identity.midi_channel.unwrap_or(0),
                program: target_stem.identity.midi_program.unwrap_or(0),
                is_percussion: target_stem.identity.is_percussion,
                mute_leak_max_abs_diff,
                mute_leak_rms_diff,
                mute_output_meter: MeterReading::from_interleaved(&mute_output),
                solo_max_abs_diff,
                solo_rms_diff,
                solo_output_meter: MeterReading::from_interleaved(&solo_output),
            }
        })
        .collect()
}

fn describe_stems(stems: &[StemRenderBlock]) -> Vec<StemProbeReport> {
    stems
        .iter()
        .enumerate()
        .map(|(index, stem)| StemProbeReport {
            index,
            soundfont_id: stem.identity.soundfont_id.clone(),
            channel: stem.identity.midi_channel,
            program: stem.identity.midi_program,
            is_percussion: stem.identity.is_percussion,
            is_global_residual: is_global_residual_stem(stem),
            meter: MeterReading::from_interleaved(&stem_to_interleaved(stem, stem.frame_count())),
        })
        .collect()
}

fn is_global_residual_stem(stem: &StemRenderBlock) -> bool {
    stem.identity.midi_channel.is_none()
        && stem.identity.midi_program.is_none()
        && !stem.identity.is_percussion
}

fn stem_to_interleaved(stem: &StemRenderBlock, frames: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames * 2];
    for frame in 0..stem.frame_count().min(frames) {
        output[frame * 2] = stem.left[frame];
        output[frame * 2 + 1] = stem.right[frame];
    }
    output
}

fn sum_global_residual_stems(stems: &[StemRenderBlock], frames: usize) -> Vec<f32> {
    sum_stems_matching(stems, frames, |_, stem| is_global_residual_stem(stem))
}

fn sum_stems_matching(
    stems: &[StemRenderBlock],
    frames: usize,
    include: impl Fn(usize, &StemRenderBlock) -> bool,
) -> Vec<f32> {
    let mut output = vec![0.0; frames * 2];
    for (index, stem) in stems.iter().enumerate() {
        if !include(index, stem) {
            continue;
        }
        for frame in 0..stem.frame_count().min(frames) {
            output[frame * 2] += stem.left[frame];
            output[frame * 2 + 1] += stem.right[frame];
        }
    }
    output
}

fn sum_stems(stems: &[StemRenderBlock], frames: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames * 2];
    for stem in stems {
        for frame in 0..stem.frame_count().min(frames) {
            output[frame * 2] += stem.left[frame];
            output[frame * 2 + 1] += stem.right[frame];
        }
    }
    output
}

fn load_dat_soundfont(dat_path: &Path, requested_id: Option<&str>) -> DatSoundFontBytes {
    let dat_paths = collect_dat_paths(dat_path);
    let mut soundfonts = BTreeMap::<String, DatSoundFontBytes>::new();

    for dat_path in dat_paths {
        let dat_bytes = read_dat_file(&dat_path).unwrap_or_else(|error| {
            eprintln!(
                "stem_parity_probe: dat_read_error path={} error={error}",
                dat_path.display()
            );
            std::process::exit(1);
        });
        let entries = extract_all_entries(&dat_bytes).unwrap_or_else(|error| {
            eprintln!(
                "stem_parity_probe: dat_extract_error path={} error={error}",
                dat_path.display()
            );
            std::process::exit(1);
        });

        for (record, payload) in entries {
            if record.entry.asset_type != "soundfont" || record.entry.runtime_format != "sf2" {
                continue;
            }
            if requested_id
                .map(|id| id != record.entry.internal_id)
                .unwrap_or(false)
            {
                continue;
            }

            let dat_file = dat_path.display().to_string();
            let is_default = record.flags & DAT_ENTRY_FLAG_DEFAULT != 0;
            soundfonts
                .entry(record.entry.internal_id.clone())
                .and_modify(|soundfont| {
                    soundfont.bytes.extend_from_slice(&payload);
                    soundfont.is_default |= is_default;
                    if !soundfont.dat_files.contains(&dat_file) {
                        soundfont.dat_files.push(dat_file.clone());
                    }
                })
                .or_insert_with(|| DatSoundFontBytes {
                    internal_id: record.entry.internal_id,
                    bytes: payload,
                    dat_files: vec![dat_file],
                    is_default,
                });
        }
    }

    if let Some(requested_id) = requested_id {
        return soundfonts.remove(requested_id).unwrap_or_else(|| {
            eprintln!("stem_parity_probe: dat_soundfont_not_found id={requested_id}");
            std::process::exit(1);
        });
    }

    soundfonts
        .into_values()
        .find(|soundfont| soundfont.is_default)
        .unwrap_or_else(|| {
            eprintln!(
                "stem_parity_probe: no_default_dat_soundfont hint=run build.ps1 to generate DAT files with a default soundfont flag"
            );
            std::process::exit(1);
        })
}

fn collect_dat_paths(input: &Path) -> Vec<PathBuf> {
    let metadata = fs::metadata(input).unwrap_or_else(|error| {
        eprintln!(
            "stem_parity_probe: dat_input_error path={} error={error}",
            input.display()
        );
        std::process::exit(1);
    });

    let mut paths = if metadata.is_dir() {
        fs::read_dir(input)
            .unwrap_or_else(|error| {
                eprintln!(
                    "stem_parity_probe: dat_dir_read_error path={} error={error}",
                    input.display()
                );
                std::process::exit(1);
            })
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"))
            })
            .collect::<Vec<_>>()
    } else {
        vec![input.to_owned()]
    };
    paths.sort();
    if paths.is_empty() {
        eprintln!(
            "stem_parity_probe: no_dat_files path={} hint=run build.ps1 to generate _local/generated-assets/dat/assets.dat",
            input.display()
        );
        std::process::exit(1);
    }
    paths
}

fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    let mut output = vec![0.0; left.len().min(right.len()) * 2];
    for frame in 0..output.len() / 2 {
        output[frame * 2] = left[frame];
        output[frame * 2 + 1] = right[frame];
    }
    output
}

fn diff(left: &mut [f32], right: &mut [f32]) -> (f32, f32) {
    let len = left.len().min(right.len());
    let mut max_abs_diff = 0.0f32;
    let mut sum_square = 0.0f64;
    for index in 0..len {
        let value = (left[index] - right[index]).abs();
        max_abs_diff = max_abs_diff.max(value);
        sum_square += value as f64 * value as f64;
    }
    let rms_diff = if len == 0 {
        0.0
    } else {
        (sum_square / len as f64).sqrt() as f32
    };
    (max_abs_diff, rms_diff)
}

fn discover_midi_files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_owned()];
    }
    if !path.is_dir() {
        return Vec::new();
    }

    let mut files = fs::read_dir(path)
        .expect("MIDI directory should be readable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mid"))
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn synth_settings() -> SynthesizerSettings {
    let mut settings = SynthesizerSettings::new(48_000);
    settings.block_size = 512;
    settings.maximum_polyphony = 128;
    settings.enable_reverb_and_chorus = true;
    settings
}

struct ProbeArgs {
    dat_path: PathBuf,
    soundfont_id: Option<String>,
    midi_path: PathBuf,
    frames: usize,
}

impl ProbeArgs {
    fn parse(args: Vec<String>) -> Self {
        let mut dat_path = PathBuf::from("_local/generated-assets/dat");
        let mut soundfont_id = None::<String>;
        let mut midi_path = PathBuf::from("MIDI Files/rendering-parity-midi");
        let mut frames = 48_000usize;
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--dat" | "--dat-dir" => {
                    index += 1;
                    dat_path = PathBuf::from(args.get(index).expect("--dat needs path"));
                }
                "--soundfont-id" => {
                    index += 1;
                    soundfont_id = Some(args.get(index).expect("--soundfont-id needs id").clone());
                }
                "--midi" | "--midi-dir" => {
                    index += 1;
                    midi_path = PathBuf::from(args.get(index).expect("--midi needs path"));
                }
                "--frames" => {
                    index += 1;
                    frames = args
                        .get(index)
                        .expect("--frames needs count")
                        .parse()
                        .expect("--frames should be a number");
                }
                other => panic!("unknown argument: {other}"),
            }
            index += 1;
        }
        Self {
            dat_path,
            soundfont_id,
            midi_path,
            frames,
        }
    }
}

struct DatSoundFontBytes {
    internal_id: String,
    bytes: Vec<u8>,
    dat_files: Vec<String>,
    is_default: bool,
}

struct ParityResult {
    frames: usize,
    stem_count: usize,
    visible_stem_count: usize,
    global_residual_stem_count: usize,
    global_residual_meter: MeterReading,
    row_mute_leak_meter: MeterReading,
    stems: Vec<StemProbeReport>,
    controls: Vec<ControlProbeReport>,
    direct_meter: MeterReading,
    mixed_meter: MeterReading,
    raw_sum_max_abs_diff: f32,
    raw_sum_rms_diff: f32,
    max_abs_diff: f32,
    rms_diff: f32,
}

struct StemProbeReport {
    index: usize,
    soundfont_id: String,
    channel: Option<u8>,
    program: Option<u8>,
    is_percussion: bool,
    is_global_residual: bool,
    meter: MeterReading,
}

struct ControlProbeReport {
    stem_index: usize,
    channel: u8,
    program: u8,
    is_percussion: bool,
    mute_leak_max_abs_diff: f32,
    mute_leak_rms_diff: f32,
    mute_output_meter: MeterReading,
    solo_max_abs_diff: f32,
    solo_rms_diff: f32,
    solo_output_meter: MeterReading,
}
