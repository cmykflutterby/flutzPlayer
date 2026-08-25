mod jump_list;
mod relay;
mod smtc;

pub use smtc::WindowsMediaControlsBackend;

use crate::MediaControlCommand;

pub fn register_jump_list_tasks() -> windows::core::Result<()> {
    jump_list::register_jump_list_tasks()
}

pub fn send_media_command_to_running_instance(command: MediaControlCommand) -> bool {
    relay::send_command_to_running_instance(command)
}
