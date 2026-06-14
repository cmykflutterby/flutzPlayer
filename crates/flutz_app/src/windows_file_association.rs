#![allow(unsafe_code)]

use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    ptr, slice,
};

use windows_sys::Win32::{
    Foundation::ERROR_FILE_NOT_FOUND,
    System::Registry::{
        RegCloseKey, RegCreateKeyW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, REG_SZ,
    },
    UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST},
};

const CLASSES_ROOT: &str = "Software\\Classes";
const OPEN_VERB: &str = "open";
const OPEN_VERB_NAME: &str = "Open with flutzPlayer";

struct FileAssociationSpec {
    extension: &'static str,
    prog_id: &'static str,
    friendly_type_name: &'static str,
}

const FILE_ASSOCIATIONS: &[FileAssociationSpec] = &[
    FileAssociationSpec {
        extension: ".fmid",
        prog_id: "flutzplayer.fmid",
        friendly_type_name: "flutzPlayer Project",
    },
    FileAssociationSpec {
        extension: ".mid",
        prog_id: "flutzplayer.mid",
        friendly_type_name: "MIDI Sequence",
    },
    FileAssociationSpec {
        extension: ".fplist",
        prog_id: "flutzplayer.fplist",
        friendly_type_name: "flutzPlayer Playlist",
    },
];

pub fn ensure_file_associations() -> io::Result<()> {
    let executable = env::current_exe()?;
    let command = format!("\"{}\" \"%1\"", executable.display());
    let icon = format!("\"{}\",0", executable.display());

    let mut changed = false;
    for association in FILE_ASSOCIATIONS {
        changed |= ensure_default_string_value(
            &classes_path(association.prog_id),
            association.friendly_type_name,
        )?;
        changed |= ensure_default_string_value(
            &classes_path(&format!("{}\\DefaultIcon", association.prog_id)),
            &icon,
        )?;
        changed |= ensure_default_string_value(
            &classes_path(&format!("{}\\shell", association.prog_id)),
            OPEN_VERB,
        )?;
        changed |= ensure_default_string_value(
            &classes_path(&format!("{}\\shell\\{OPEN_VERB}", association.prog_id)),
            OPEN_VERB_NAME,
        )?;
        changed |= ensure_default_string_value(
            &classes_path(&format!("{}\\shell\\{OPEN_VERB}\\command", association.prog_id)),
            &command,
        )?;

        let extension_path = classes_path(association.extension);
        changed |= ensure_default_string_value(&extension_path, association.prog_id)?;
    }

    if changed {
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                ptr::null(),
                ptr::null(),
            );
        }
    }

    Ok(())
}

fn classes_path(suffix: &str) -> String {
    format!("{CLASSES_ROOT}\\{suffix}")
}

fn ensure_default_string_value(subkey: &str, expected: &str) -> io::Result<bool> {
    let existing = read_default_string_value(subkey)?;
    if existing.as_deref() == Some(expected) {
        return Ok(false);
    }

    let key = RegistryKey::create(HKEY_CURRENT_USER, subkey)?;
    key.set_default_string(expected)?;
    Ok(true)
}

fn read_default_string_value(subkey: &str) -> io::Result<Option<String>> {
    let key = match RegistryKey::open(HKEY_CURRENT_USER, subkey) {
        Ok(key) => key,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    key.read_default_string()
}

struct RegistryKey(HKEY);

impl RegistryKey {
    fn open(root: HKEY, subkey: &str) -> io::Result<Self> {
        let mut handle: HKEY = ptr::null_mut();
        let subkey_wide = wide_null(OsStr::new(subkey));
        let status = unsafe { RegOpenKeyExW(root, subkey_wide.as_ptr(), 0, KEY_READ, &mut handle) };
        win32_result(status)?;
        Ok(Self(handle))
    }

    fn create(root: HKEY, subkey: &str) -> io::Result<Self> {
        let mut handle: HKEY = ptr::null_mut();
        let subkey_wide = wide_null(OsStr::new(subkey));
        let status = unsafe { RegCreateKeyW(root, subkey_wide.as_ptr(), &mut handle) };
        win32_result(status)?;
        Ok(Self(handle))
    }

    fn read_default_string(&self) -> io::Result<Option<String>> {
        let mut value_type = 0u32;
        let mut byte_len = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                ptr::null(),
                ptr::null_mut(),
                &mut value_type,
                ptr::null_mut(),
                &mut byte_len,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        win32_result(status)?;
        if value_type != REG_SZ {
            return Ok(None);
        }
        if byte_len == 0 {
            return Ok(Some(String::new()));
        }

        let mut buffer = vec![0u8; byte_len as usize];
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                ptr::null(),
                ptr::null_mut(),
                &mut value_type,
                buffer.as_mut_ptr(),
                &mut byte_len,
            )
        };
        win32_result(status)?;
        if value_type != REG_SZ {
            return Ok(None);
        }

        let wide_len = byte_len as usize / std::mem::size_of::<u16>();
        let wide = unsafe { slice::from_raw_parts(buffer.as_ptr().cast::<u16>(), wide_len) };
        let end = wide
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(wide.len());
        Ok(Some(
            OsString::from_wide(&wide[..end])
                .to_string_lossy()
                .into_owned(),
        ))
    }

    fn set_default_string(&self, value: &str) -> io::Result<()> {
        let wide_value = wide_null(OsStr::new(value));
        let byte_len = (wide_value.len() * std::mem::size_of::<u16>()) as u32;
        let status = unsafe {
            RegSetValueExW(
                self.0,
                ptr::null(),
                0,
                REG_SZ,
                wide_value.as_ptr().cast::<u8>(),
                byte_len,
            )
        };
        win32_result(status)
    }
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn win32_result(status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
