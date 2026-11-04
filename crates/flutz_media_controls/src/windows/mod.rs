mod jump_list;
mod relay;
mod smtc;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

pub use smtc::WindowsMediaControlsBackend;

use crate::{MediaCommandWaker, MediaControlCommand};

/// Command queue shared by the SMTC event handlers and the relay thread.
#[derive(Clone, Default)]
struct CommandQueue {
    commands: Arc<Mutex<VecDeque<MediaControlCommand>>>,
    waker: Arc<Mutex<Option<MediaCommandWaker>>>,
}

impl CommandQueue {
    fn push(&self, command: MediaControlCommand) {
        let Ok(mut commands) = self.commands.lock() else {
            return;
        };
        commands.push_back(command);
        drop(commands);
        let waker = self.waker.lock().ok().and_then(|waker| waker.clone());
        if let Some(waker) = waker {
            waker();
        }
    }

    fn pop(&self) -> Option<MediaControlCommand> {
        self.commands
            .lock()
            .ok()
            .and_then(|mut commands| commands.pop_front())
    }

    fn set_waker(&self, waker: MediaCommandWaker) {
        if let Ok(mut slot) = self.waker.lock() {
            *slot = Some(waker);
        }
    }
}

pub fn register_jump_list_tasks() -> windows::core::Result<()> {
    jump_list::register_jump_list_tasks()
}

pub fn send_media_command_to_running_instance(command: MediaControlCommand) -> bool {
    relay::send_command_to_running_instance(command)
}
