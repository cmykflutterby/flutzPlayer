use super::*;

pub(super) fn convert_soundfont(args: ConvertArgs) -> Result<(), String> {
    match normalized_extension(&args.input).as_deref() {
        Some("sf2") => fs::copy(&args.input, &args.output)
            .map(|_| ())
            .map_err(|error| format!("failed to copy SF2 input: {error}")),
        Some("sfark") => decode_sfark_file_to_sf2(&args.input, &args.output)
            .map_err(|error| format!("sfArk conversion failed: {error}")),
        Some(extension) => Err(format!("unsupported soundfont extension: .{extension}")),
        None => Err("input path has no extension".to_owned()),
    }
}

pub(super) fn normalized_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}
