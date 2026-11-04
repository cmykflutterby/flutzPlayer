#![allow(unsafe_code)]

use std::{
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

use windows_sys::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_BROKEN_PIPE, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
            GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            CreateFileW, FlushFileBuffers, ReadFile, WriteFile, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
        },
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW,
            PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT,
        },
    },
};

use crate::{media_command_argument, media_command_from_argument, MediaControlCommand};

use super::CommandQueue;

const MEDIA_COMMAND_PIPE_NAME: &str = r"\\.\pipe\flutzplayer-media-controls";
const MEDIA_COMMAND_PIPE_TIMEOUT_MS: u32 = 250;
const MEDIA_COMMAND_PIPE_BUFFER_BYTES: u32 = 128;

pub struct WindowsMediaCommandRelayServer {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WindowsMediaCommandRelayServer {
    pub fn shutdown(mut self) {
        self.running.store(false, Ordering::Release);
        let _ = write_command_text("shutdown");
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start_command_server(commands: CommandQueue) -> Option<WindowsMediaCommandRelayServer> {
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::Builder::new()
        .name("flutz-media-command-relay".to_owned())
        .spawn(move || run_command_server(server_running, commands))
        .ok()?;

    Some(WindowsMediaCommandRelayServer {
        running,
        thread: Some(thread),
    })
}

pub fn send_command_to_running_instance(command: MediaControlCommand) -> bool {
    media_command_argument(&command).is_some_and(write_command_text)
}

fn run_command_server(running: Arc<AtomicBool>, commands: CommandQueue) {
    let pipe_name = wide_null(MEDIA_COMMAND_PIPE_NAME);
    while running.load(Ordering::Acquire) {
        let pipe = unsafe {
            CreateNamedPipeW(
                pipe_name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                1,
                MEDIA_COMMAND_PIPE_BUFFER_BYTES,
                MEDIA_COMMAND_PIPE_BUFFER_BYTES,
                MEDIA_COMMAND_PIPE_TIMEOUT_MS,
                ptr::null(),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            break;
        }

        let connected = unsafe { ConnectNamedPipe(pipe, ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            read_one_command(pipe, &commands);
            unsafe {
                let _ = FlushFileBuffers(pipe);
                let _ = DisconnectNamedPipe(pipe);
            }
        }
        unsafe {
            let _ = CloseHandle(pipe);
        }
    }
}

fn read_one_command(pipe: HANDLE, commands: &CommandQueue) {
    let mut buffer = [0u8; MEDIA_COMMAND_PIPE_BUFFER_BYTES as usize];
    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            pipe,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut read,
            ptr::null_mut(),
        )
    } != 0;
    if !ok || read == 0 {
        return;
    }

    let text = String::from_utf8_lossy(&buffer[..read as usize]);
    let Some(command) = media_command_from_argument(&text) else {
        return;
    };
    commands.push(command);
}

fn write_command_text(command: &str) -> bool {
    let pipe_name = wide_null(MEDIA_COMMAND_PIPE_NAME);
    let pipe_name = pipe_name.as_ptr();
    if !wait_for_pipe(pipe_name) {
        return false;
    }

    let pipe = unsafe {
        CreateFileW(
            pipe_name,
            GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        return false;
    }

    let bytes = command.as_bytes();
    let mut written = 0u32;
    let ok = unsafe {
        WriteFile(
            pipe,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    } != 0;
    unsafe {
        let _ = FlushFileBuffers(pipe);
        let _ = CloseHandle(pipe);
    }

    ok || unsafe { GetLastError() } == ERROR_BROKEN_PIPE
}

fn wait_for_pipe(pipe_name: PCWSTR) -> bool {
    (unsafe { WaitNamedPipeW(pipe_name, MEDIA_COMMAND_PIPE_TIMEOUT_MS) } != 0)
        || (unsafe { GetLastError() } != ERROR_PIPE_BUSY)
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
