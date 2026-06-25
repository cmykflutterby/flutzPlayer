#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for (extension, prog_id, friendly_name) in flutz_app::windows_file_association_specs() {
        println!(
            "extension={} prog_id={} friendly_name={} status=ok",
            extension, prog_id, friendly_name
        );
    }
    println!("status=ok");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    println!("status=ok unsupported_platform=non-windows");
}
