use std::{env, fs, process, time::Instant};

use flutz_soundfont_tools::sfark::{
    decode_sfark_to_sf2_diagnostics_until_block, decode_sfark_to_sf2_diagnostics_with_progress,
    parse_header,
};

const DEFAULT_PROGRESS_BLOCKS: usize = 32;

fn main() {
    let config = Config::parse(env::args().skip(1).collect()).unwrap_or_else(|message| {
        eprintln!("{message}");
        eprintln!(
            "usage: sfark_diagnostics <input.sfArk> <unchecked.sf2> [oracle.sf2] [--progress-blocks N] [--max-blocks N]"
        );
        process::exit(1);
    });

    let input = fs::read(&config.input_path).unwrap_or_else(|error| {
        eprintln!("failed to read input: {error}");
        process::exit(2);
    });
    let header = parse_header(&input).unwrap_or_else(|error| {
        eprintln!("failed to parse sfArk header: {error}");
        process::exit(2);
    });

    let started = Instant::now();
    let mut last_reported_block = 0usize;
    eprintln!(
        "decode started: input={} progress_every={} blocks max_blocks={}",
        config.input_path,
        config.progress_blocks,
        config
            .max_blocks
            .map(|value| value.to_string())
            .unwrap_or_else(|| "all".to_owned())
    );
    let mut progress = |progress: flutz_soundfont_tools::sfark::DecodeProgress| {
        if progress.audio_block_index == 0
            || progress.audio_block_index == last_reported_block
            || progress.audio_block_index % config.progress_blocks != 0
        {
            return;
        }

        last_reported_block = progress.audio_block_index;
        let percent = f64::from(progress.total_written) * 100.0 / f64::from(progress.original_size);
        eprintln!(
            "progress: {:>6.2}% block={} section={} decoded={} elapsed={}s",
            percent,
            progress.audio_block_index,
            progress.section,
            progress.total_written,
            started.elapsed().as_secs()
        );
    };
    let diagnostics = match config.max_blocks {
        Some(max_blocks) => {
            decode_sfark_to_sf2_diagnostics_until_block(&input, max_blocks, &mut progress)
        }
        None => decode_sfark_to_sf2_diagnostics_with_progress(&input, &mut progress),
    }
    .unwrap_or_else(|error| {
        eprintln!("sfArk decode failed: {error}");
        process::exit(2);
    });

    eprintln!(
        "decode finished in {}s; writing unchecked output",
        started.elapsed().as_secs()
    );
    fs::write(&config.output_path, &diagnostics.output).unwrap_or_else(|error| {
        eprintln!("failed to write unchecked output: {error}");
        process::exit(2);
    });

    println!(
        "decoded {} bytes; checksum actual={:08x} expected={:08x}",
        diagnostics.output.len(),
        diagnostics.actual_file_check,
        diagnostics.expected_file_check
    );

    if let Some(oracle_path) = config.oracle_path {
        let oracle = fs::read(oracle_path).unwrap_or_else(|error| {
            eprintln!("failed to read oracle: {error}");
            process::exit(2);
        });

        println!(
            "oracle {} bytes; unchecked {} bytes",
            oracle.len(),
            diagnostics.output.len()
        );

        match first_difference(&diagnostics.output, &oracle) {
            Some(index) => {
                println!(
                    "first difference at byte {index}: unchecked={:02x?} oracle={:02x?}",
                    diagnostics.output.get(index),
                    oracle.get(index)
                );
                if let Some(location) =
                    audio_location(index, header.audio_start, header.post_audio_start)
                {
                    println!(
                        "audio location: block={} byte_in_block={} sample_in_block={}",
                        location.block, location.byte_in_block, location.sample_in_block
                    );
                }
            }
            None => println!("unchecked output matches oracle bytes"),
        }
    }
}

struct AudioLocation {
    block: usize,
    byte_in_block: usize,
    sample_in_block: usize,
}

fn audio_location(
    byte_offset: usize,
    audio_start: u32,
    post_audio_start: u32,
) -> Option<AudioLocation> {
    let audio_start = audio_start as usize;
    let post_audio_start = post_audio_start as usize;
    if byte_offset < audio_start || byte_offset >= post_audio_start {
        return None;
    }

    let relative = byte_offset - audio_start;
    let byte_in_block = relative % 8192;
    Some(AudioLocation {
        block: relative / 8192,
        byte_in_block,
        sample_in_block: byte_in_block / 2,
    })
}

struct Config {
    input_path: String,
    output_path: String,
    oracle_path: Option<String>,
    progress_blocks: usize,
    max_blocks: Option<usize>,
}

impl Config {
    fn parse(args: Vec<String>) -> std::result::Result<Self, String> {
        let mut positional = Vec::new();
        let mut progress_blocks = DEFAULT_PROGRESS_BLOCKS;
        let mut max_blocks = None;
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--progress-blocks" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        "--progress-blocks requires a positive integer".to_owned()
                    })?;
                    progress_blocks = value
                        .parse::<usize>()
                        .map_err(|_| "--progress-blocks requires a positive integer".to_owned())?;
                    if progress_blocks == 0 {
                        return Err("--progress-blocks must be greater than zero".to_owned());
                    }
                }
                "--max-blocks" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "--max-blocks requires a positive integer".to_owned())?;
                    let parsed = value
                        .parse::<usize>()
                        .map_err(|_| "--max-blocks requires a positive integer".to_owned())?;
                    if parsed == 0 {
                        return Err("--max-blocks must be greater than zero".to_owned());
                    }
                    max_blocks = Some(parsed);
                }
                value if value.starts_with('-') => {
                    return Err(format!("unknown option: {value}"));
                }
                value => positional.push(value.to_owned()),
            }
            index += 1;
        }

        if positional.len() < 2 || positional.len() > 3 {
            return Err("expected 2 or 3 positional arguments".to_owned());
        }

        Ok(Self {
            input_path: positional[0].clone(),
            output_path: positional[1].clone(),
            oracle_path: positional.get(2).cloned(),
            progress_blocks,
            max_blocks,
        })
    }
}

fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    left.iter()
        .zip(right.iter())
        .position(|(left, right)| left != right)
        .or_else(|| (left.len() != right.len()).then_some(left.len().min(right.len())))
}
