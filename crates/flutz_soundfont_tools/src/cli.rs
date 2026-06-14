use std::{env, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Convert(ConvertArgs),
    DiagnoseFmid(DiagnoseFmidArgs),
    DiagnoseMidi(DiagnoseMidiArgs),
    Extract(ExtractArgs),
    Inspect(InspectArgs),
    Pack(PackArgs),
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertArgs {
    pub input: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractArgs {
    pub input: PathBuf,
    pub output_dir: PathBuf,
    pub internal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnoseFmidArgs {
    pub input: PathBuf,
    pub dat_input: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnoseMidiArgs {
    pub input: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectArgs {
    pub input: PathBuf,
    pub inspect_font: Option<String>,
    pub dump_coverage: bool,
    pub dump_index: bool,
    pub dump_pack_report: bool,
    pub format: InspectFormat,
    pub preset: Option<u32>,
    pub instrument: Option<u32>,
    pub sample: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackArgs {
    pub manifest: PathBuf,
    pub output: PathBuf,
    pub base_dir: PathBuf,
    pub registry: Option<PathBuf>,
    pub chunk_size: Option<u64>,
    pub max_file_size: Option<u64>,
}

pub fn parse_env_args() -> Result<CliCommand, String> {
    parse_args(env::args().skip(1))
}

pub fn parse_args<I, S>(args: I) -> Result<CliCommand, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut extract = false;
    let mut diagnose_fmid = false;
    let mut diagnose_midi = false;
    let mut inspect = false;
    let mut pack = false;
    let mut input = None;
    let mut output = None;
    let mut internal_id = None;
    let mut base_dir = PathBuf::from(".");
    let mut dat_input = None;
    let mut registry = None;
    let mut chunk_size = None;
    let mut max_file_size = None;
    let mut inspect_font = None;
    let mut dump_coverage = false;
    let mut dump_index = false;
    let mut dump_pack_report = false;
    let mut inspect_format = InspectFormat::Text;
    let mut preset = None;
    let mut instrument = None;
    let mut sample = None;

    let mut args = args.into_iter().map(Into::into).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(CliCommand::Help),
            "--extract" => extract = true,
            "--diagnose-fmid" => diagnose_fmid = true,
            "--diagnose-midi" => diagnose_midi = true,
            "--inspect" => inspect = true,
            "--pack" => pack = true,
            "--input" | "-i" => input = next_path_value(&mut args, &arg)?,
            "--output" | "-o" => output = next_path_value(&mut args, &arg)?,
            "--id" => internal_id = Some(next_string_value(&mut args, &arg)?),
            "--base-dir" => base_dir = PathBuf::from(next_string_value(&mut args, &arg)?),
            "--dat-input" => dat_input = Some(PathBuf::from(next_string_value(&mut args, &arg)?)),
            "--registry" => registry = Some(PathBuf::from(next_string_value(&mut args, &arg)?)),
            "--inspect-font" => inspect_font = Some(next_string_value(&mut args, &arg)?),
            "--dump-coverage" => dump_coverage = true,
            "--dump-index" => dump_index = true,
            "--dump-pack-report" => dump_pack_report = true,
            "--format" => {
                let value = next_string_value(&mut args, &arg)?;
                inspect_format = match value.as_str() {
                    "text" => InspectFormat::Text,
                    "json" => InspectFormat::Json,
                    _ => return Err(format!("invalid --format value: {value}")),
                };
            }
            "--preset" => {
                let value = next_string_value(&mut args, &arg)?;
                preset = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --preset value: {value}"))?,
                );
            }
            "--instrument" => {
                let value = next_string_value(&mut args, &arg)?;
                instrument = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --instrument value: {value}"))?,
                );
            }
            "--sample" => {
                let value = next_string_value(&mut args, &arg)?;
                sample = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --sample value: {value}"))?,
                );
            }
            "--chunk-size" => {
                let value = next_string_value(&mut args, &arg)?;
                chunk_size = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --chunk-size value: {value}"))?,
                );
            }
            "--max-file-size" => {
                let value = next_string_value(&mut args, &arg)?;
                max_file_size = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --max-file-size value: {value}"))?,
                );
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }

    let mode_count = [extract, diagnose_fmid, diagnose_midi, inspect, pack]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();
    if mode_count > 1 {
        return Err(
            "--diagnose-fmid, --diagnose-midi, --extract, --inspect, and --pack cannot be used together".to_owned(),
        );
    }

    let input = input.ok_or_else(|| "missing required --input path".to_owned())?;
    let inspect_options_used = inspect_font.is_some()
        || dump_coverage
        || dump_index
        || dump_pack_report
        || inspect_format != InspectFormat::Text
        || preset.is_some()
        || instrument.is_some()
        || sample.is_some();

    if pack {
        if dat_input.is_some() {
            return Err("--dat-input is only valid with --diagnose-fmid".to_owned());
        }
        let output = output.ok_or_else(|| "missing required --output path".to_owned())?;
        if internal_id.is_some() {
            return Err("--id is only valid with --extract".to_owned());
        }
        if inspect_options_used {
            return Err("inspect options are only valid with --inspect".to_owned());
        }
        Ok(CliCommand::Pack(PackArgs {
            manifest: input,
            output,
            base_dir,
            registry,
            chunk_size,
            max_file_size,
        }))
    } else if diagnose_fmid {
        if output.is_some() {
            return Err("--output is not valid with --diagnose-fmid".to_owned());
        }
        if internal_id.is_some() {
            return Err("--id is only valid with --extract".to_owned());
        }
        if inspect_options_used {
            return Err("inspect options are only valid with --inspect".to_owned());
        }
        Ok(CliCommand::DiagnoseFmid(DiagnoseFmidArgs {
            input,
            dat_input,
        }))
    } else if diagnose_midi {
        if output.is_some() {
            return Err("--output is not valid with --diagnose-midi".to_owned());
        }
        if internal_id.is_some() {
            return Err("--id is only valid with --extract".to_owned());
        }
        if dat_input.is_some() {
            return Err("--dat-input is only valid with --diagnose-fmid".to_owned());
        }
        if inspect_options_used {
            return Err("inspect options are only valid with --inspect".to_owned());
        }
        Ok(CliCommand::DiagnoseMidi(DiagnoseMidiArgs { input }))
    } else if inspect {
        if dat_input.is_some() {
            return Err("--dat-input is only valid with --diagnose-fmid".to_owned());
        }
        if output.is_some() {
            return Err("--output is not valid with --inspect".to_owned());
        }
        if internal_id.is_some() {
            return Err("--id is only valid with --extract".to_owned());
        }
        Ok(CliCommand::Inspect(InspectArgs {
            input,
            inspect_font,
            dump_coverage,
            dump_index,
            dump_pack_report,
            format: inspect_format,
            preset,
            instrument,
            sample,
        }))
    } else if extract {
        if dat_input.is_some() {
            return Err("--dat-input is only valid with --diagnose-fmid".to_owned());
        }
        if inspect_options_used {
            return Err("inspect options are only valid with --inspect".to_owned());
        }
        let output = output.ok_or_else(|| "missing required --output path".to_owned())?;
        Ok(CliCommand::Extract(ExtractArgs {
            input,
            output_dir: output,
            internal_id,
        }))
    } else if internal_id.is_some() {
        Err("--id is only valid with --extract".to_owned())
    } else if dat_input.is_some() {
        Err("--dat-input is only valid with --diagnose-fmid".to_owned())
    } else if inspect_options_used {
        Err("inspect options are only valid with --inspect".to_owned())
    } else {
        let output = output.ok_or_else(|| "missing required --output path".to_owned())?;
        Ok(CliCommand::Convert(ConvertArgs { input, output }))
    }
}

pub fn help_text() -> &'static str {
    "Usage:\n  flutz_soundfont_tools --input <source.sfArk|source.sf2> --output <output.sf2>\n  flutz_soundfont_tools --pack --input <dat-manifest.toml> --output <assets.dat> [--base-dir <repo-root>] [--chunk-size <bytes>] [--max-file-size <bytes>] [--registry <default-registry.toml> (deprecated, ignored)]\n  flutz_soundfont_tools --extract --input <assets.dat|assets-000.dat|dat-directory> --output <directory> [--id <internal_id>]\n  flutz_soundfont_tools --inspect --input <assets.dat|assets-000.dat|dat-directory> [--inspect-font <internal_id>] [--dump-coverage] [--dump-index] [--dump-pack-report] [--preset <id>] [--instrument <id>] [--sample <id>] [--format text|json]\n  flutz_soundfont_tools --diagnose-fmid --input <project.fmid> [--dat-input <assets.dat|assets-000.dat|dat-directory>]\n  flutz_soundfont_tools --diagnose-midi --input <file.mid>"
}

fn next_path_value<I>(args: &mut I, flag: &str) -> Result<Option<PathBuf>, String>
where
    I: Iterator<Item = String>,
{
    Ok(Some(PathBuf::from(next_string_value(args, flag)?)))
}

fn next_string_value<I>(args: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing value for {flag}"))
}
