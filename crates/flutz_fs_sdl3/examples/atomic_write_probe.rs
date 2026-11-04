use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use flutz_fs_sdl3::TEMPORARY_FILE_PREFIX;

const CHILD_BYTES: usize = 192 * 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--child") {
        let destination = PathBuf::from(args.get(2).ok_or("missing child destination")?);
        flutz_fs_sdl3::atomic_write(&destination, &vec![b'N'; CHILD_BYTES])?;
        return Ok(());
    }

    let base = PathBuf::from(args.get(1).ok_or("pass an isolated probe directory")?);
    fs::create_dir_all(&base)?;
    let work = base.join(format!("atomic-probe-{}", std::process::id()));
    fs::create_dir(&work)?;

    let destination = work.join("nested").join("preferences.ini");
    let parent = destination.parent().ok_or("missing parent")?;
    flutz_fs_sdl3::ensure_writable_directory(parent)?;
    flutz_fs_sdl3::atomic_write(&destination, b"initial")?;
    require(fs::read(&destination)? == b"initial", "initial write")?;
    flutz_fs_sdl3::atomic_write(&destination, b"updated")?;
    require(fs::read(&destination)? == b"updated", "atomic replacement")?;
    require(no_temporary_files(parent)?, "successful cleanup")?;
    println!("PASS create, replace, and clean same-directory temporary files");

    let directory_destination = work.join("existing-directory");
    fs::create_dir(&directory_destination)?;
    let sentinel = directory_destination.join("sentinel");
    fs::write(&sentinel, b"unchanged")?;
    require(
        flutz_fs_sdl3::atomic_write(&directory_destination, b"must fail").is_err(),
        "rename failure surfaced",
    )?;
    require(
        fs::read(&sentinel)? == b"unchanged",
        "failed write preserved target",
    )?;
    require(
        no_temporary_files(&work)?,
        "failed write cleaned temporary file",
    )?;
    println!("PASS failed replacement preserves the destination and cleans the temporary file");

    let first = vec![b'A'; 16 * 1024];
    let second = vec![b'B'; 16 * 1024];
    flutz_fs_sdl3::atomic_write(&destination, &first)?;
    let finished = Arc::new(AtomicBool::new(false));
    let reader_finished = Arc::clone(&finished);
    let reader_path = destination.clone();
    let reader_first = first.clone();
    let reader_second = second.clone();
    let reader = thread::spawn(move || -> Result<usize, String> {
        let mut reads = 0;
        while !reader_finished.load(Ordering::Acquire) {
            let bytes = fs::read(&reader_path).map_err(|error| error.to_string())?;
            if bytes != reader_first && bytes != reader_second {
                return Err("reader observed a missing, partial, or mixed file".to_owned());
            }
            reads += 1;
        }
        Ok(reads)
    });
    for index in 0..50 {
        flutz_fs_sdl3::atomic_write(&destination, if index % 2 == 0 { &second } else { &first })?;
    }
    finished.store(true, Ordering::Release);
    let reads = reader.join().map_err(|_| "reader panicked")??;
    require(reads > 0, "concurrent reader ran")?;
    println!("PASS concurrent readers observed {reads} complete old-or-new files");

    let interrupted = work.join("interrupted.ini");
    fs::write(&interrupted, b"previous preferences")?;
    let mut child = Command::new(env::current_exe()?)
        .arg("--child")
        .arg(&interrupted)
        .stdout(Stdio::null())
        .spawn()?;
    let prefix = format!("{TEMPORARY_FILE_PREFIX}{}-", child.id());
    let started = Instant::now();
    let (temporary, observed_bytes) = loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("writer exited before interruption: {status}").into());
        }
        let mut found = None;
        for entry in fs::read_dir(&work)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                found = Some((entry.path(), entry.metadata()?.len()));
                break;
            }
        }
        if let Some(temporary) = found {
            break temporary;
        }
        if started.elapsed() > Duration::from_secs(30) {
            child.kill()?;
            child.wait()?;
            return Err("timed out waiting for an in-progress temporary write".into());
        }
        thread::sleep(Duration::from_micros(100));
    };
    child.kill()?;
    child.wait()?;
    require(
        fs::read(&interrupted)? == b"previous preferences",
        "interrupted write preserved previous preferences",
    )?;
    if temporary.exists() {
        fs::remove_file(temporary)?;
    }
    println!(
        "PASS terminated writer left the previous preferences intact (temporary bytes observed: {observed_bytes})"
    );
    println!("atomic_write_probe: PASS");
    Ok(())
}

fn require(condition: bool, description: &str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(format!("FAIL {description}").into())
    }
}

fn no_temporary_files(directory: &Path) -> Result<bool, std::io::Error> {
    for entry in fs::read_dir(directory)? {
        if entry?
            .file_name()
            .to_string_lossy()
            .starts_with(TEMPORARY_FILE_PREFIX)
        {
            return Ok(false);
        }
    }
    Ok(true)
}
