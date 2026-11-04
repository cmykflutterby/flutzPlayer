use super::*;

pub(super) fn trace_audio_block(block_index: usize) -> bool {
    std::env::var("FLUTZ_SFARK_TRACE_BLOCK")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index)
}

pub(super) fn dump_trace_words(block_env: &str, path_env: &str, block_index: usize, words: &[i16]) {
    let should_dump = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    if !should_dump {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(words.len() * 2);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

pub(super) fn dump_trace_i32_words(
    block_env: &str,
    subwindow_env: &str,
    path_env: &str,
    block_index: usize,
    subwindow: usize,
    words: &[i32],
) {
    let should_dump_block = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    let should_dump_subwindow = std::env::var(subwindow_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(subwindow);
    if !should_dump_block || !should_dump_subwindow {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

pub(super) fn dump_trace_f32_words(
    block_env: &str,
    subwindow_env: &str,
    path_env: &str,
    block_index: usize,
    subwindow: usize,
    words: &[f32],
) {
    let should_dump_block = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    let should_dump_subwindow = std::env::var(subwindow_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(subwindow);
    if !should_dump_block || !should_dump_subwindow {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

pub(super) fn dump_trace_f32_matrix(
    block_env: &str,
    subwindow_env: &str,
    path_env: &str,
    block_index: usize,
    subwindow: usize,
    rows: &[[f32; LPC_MAX_COEFFICIENTS + 1]; LPC_HISTORY_SIZE],
    columns: usize,
) {
    let should_dump_block = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    let should_dump_subwindow = std::env::var(subwindow_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(subwindow);
    if !should_dump_block || !should_dump_subwindow {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(rows.len() * columns * 4);
    for row in rows {
        for value in row.iter().take(columns) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

pub(super) fn adler32(initial: u32, bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let mut s1 = initial & 0xffff;
    let mut s2 = (initial >> 16) & 0xffff;

    for byte in bytes {
        s1 = (s1 + u32::from(*byte)) % MOD_ADLER;
        s2 = (s2 + s1) % MOD_ADLER;
    }

    (s2 << 16) | s1
}
