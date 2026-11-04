#![allow(unsafe_code)]

use std::{
    ffi::{CStr, CString},
    io,
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(not(windows))]
use sdl3_sys::everything::SDL_RenamePath;
use sdl3_sys::everything::{
    SDL_CloseIO, SDL_CreateDirectory, SDL_FlushIO, SDL_GetError, SDL_GetPathInfo, SDL_IOFromFile,
    SDL_IOStream, SDL_PathInfo, SDL_RemovePath, SDL_WriteIO, SDL_PATHTYPE_NONE,
};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// Prefix of the temporary files created beside an atomically written destination.
pub const TEMPORARY_FILE_PREFIX: &str = ".flutzplayer-";

/// Creates `directory` (and missing parents) if needed and verifies that a file
/// can be created in it.
pub fn ensure_writable_directory(directory: &Path) -> io::Result<()> {
    create_directory(directory)?;
    let (temporary, stream) = create_temporary_file(directory)?;
    stream.close()?;
    temporary.remove()
}

/// Replaces `path` with `contents` so readers observe either the previous or the
/// new file, never a partial one.
///
/// The data is written to a uniquely named temporary file in the destination's
/// directory, flushed to storage, closed, and renamed over the destination. A
/// failed temporary file is removed where possible. File I/O uses SDL3; the final
/// rename uses SDL3 except on Windows (see `rename_replacing`).
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination must have a file name",
        ));
    }
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_directory(directory)?;
    let (temporary, stream) = create_temporary_file(directory)?;
    stream.write_all(contents)?;
    stream.flush()?;
    stream.close()?;
    rename_replacing(&temporary.path, path)?;
    temporary.disarm();
    Ok(())
}

#[cfg(windows)]
fn rename_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    // TODO(fileio cleanup): SDL_RenamePath uses MoveFileExW, which fails while another
    // handle has the destination open; std::fs::rename uses POSIX-semantics replacement.
    std::fs::rename(source, destination)
}

#[cfg(not(windows))]
fn rename_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    let source = c_path(source)?;
    let destination = c_path(destination)?;
    if unsafe { SDL_RenamePath(source.as_ptr(), destination.as_ptr()) } {
        Ok(())
    } else {
        Err(sdl_error("rename failed"))
    }
}

struct TemporaryPath {
    path: PathBuf,
    armed: bool,
}

impl TemporaryPath {
    fn remove(mut self) -> io::Result<()> {
        self.armed = false;
        remove_path(&self.path)
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = remove_path(&self.path) {
                eprintln!(
                    "failed to remove temporary file {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

struct Stream(Option<NonNull<SDL_IOStream>>);

impl Stream {
    fn write_all(&self, mut contents: &[u8]) -> io::Result<()> {
        let stream = self.raw();
        while !contents.is_empty() {
            let written = unsafe { SDL_WriteIO(stream, contents.as_ptr().cast(), contents.len()) };
            if written == 0 {
                return Err(sdl_error("write failed"));
            }
            contents = &contents[written..];
        }
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        if unsafe { SDL_FlushIO(self.raw()) } {
            Ok(())
        } else {
            Err(sdl_error("flush failed"))
        }
    }

    fn close(mut self) -> io::Result<()> {
        let stream = self.0.take().expect("stream is open until closed");
        if unsafe { SDL_CloseIO(stream.as_ptr()) } {
            Ok(())
        } else {
            Err(sdl_error("close failed"))
        }
    }

    fn raw(&self) -> *mut SDL_IOStream {
        self.0.expect("stream is open until closed").as_ptr()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Some(stream) = self.0.take() {
            unsafe { SDL_CloseIO(stream.as_ptr()) };
        }
    }
}

fn create_temporary_file(directory: &Path) -> io::Result<(TemporaryPath, Stream)> {
    for _ in 0..128 {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "{TEMPORARY_FILE_PREFIX}{}-{sequence}.tmp",
            std::process::id()
        ));
        let file = c_path(&path)?;
        // `x` requests exclusive creation: CREATE_NEW on Windows, C11 fopen elsewhere.
        let stream = unsafe { SDL_IOFromFile(file.as_ptr(), c"wbx".as_ptr()) };
        if let Some(stream) = NonNull::new(stream) {
            return Ok((TemporaryPath { path, armed: true }, Stream(Some(stream))));
        }
        let error = sdl_error("could not create temporary file");
        if !path_exists(&path)? {
            return Err(error);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary file",
    ))
}

fn create_directory(directory: &Path) -> io::Result<()> {
    let path = c_path(directory)?;
    if unsafe { SDL_CreateDirectory(path.as_ptr()) } {
        Ok(())
    } else {
        Err(sdl_error("could not create directory"))
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    let file = c_path(path)?;
    if unsafe { SDL_RemovePath(file.as_ptr()) } {
        return Ok(());
    }
    let error = sdl_error("remove failed");
    if path_exists(path)? {
        Err(error)
    } else {
        Ok(())
    }
}

fn path_exists(path: &Path) -> io::Result<bool> {
    let file = c_path(path)?;
    let mut info = SDL_PathInfo::default();
    if unsafe { SDL_GetPathInfo(file.as_ptr(), &mut info) } {
        Ok(info.r#type != SDL_PATHTYPE_NONE)
    } else {
        Ok(false)
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path
        .to_str()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path is not valid UTF-8: {}", path.display()),
            )
        })?
        .as_bytes()
        .to_vec();
    CString::new(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path contains a NUL byte: {}", path.display()),
        )
    })
}

fn sdl_error(operation: &str) -> io::Error {
    let pointer = SDL_GetError();
    let message = if pointer.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    };
    if message.is_empty() {
        io::Error::other(operation.to_owned())
    } else {
        io::Error::other(format!("{operation}: {message}"))
    }
}
