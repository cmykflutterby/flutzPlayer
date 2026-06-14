use std::{env, fs::File, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../../assets/flutzplayer-icon.png");

    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    compile_windows_icon().expect("failed to compile flutzPlayer Windows icon resources");
}

fn compile_windows_icon() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let icon_png = manifest_dir.join("../../assets/flutzplayer-icon.png");
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let icon_ico = out_dir.join("flutzplayer-icon.ico");

    let image = image::open(&icon_png)?.into_rgba8();
    let (width, height) = image.dimensions();
    let icon_image = ico::IconImage::from_rgba_data(width, height, image.into_raw());
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    icon_dir.add_entry(ico::IconDirEntry::encode(&icon_image)?);
    let mut icon_file = File::create(&icon_ico)?;
    icon_dir.write(&mut icon_file)?;

    winresource::WindowsResource::new()
        .set_icon(icon_ico.to_string_lossy().as_ref())
        .compile()?;

    Ok(())
}
