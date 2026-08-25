#![allow(unsafe_code)]

use std::{env, mem::ManuallyDrop, path::Path};

use windows::{
    core::{IUnknown, Interface, Result, BSTR, HSTRING},
    Win32::{
        Foundation::PROPERTYKEY,
        Storage::EnhancedStorage::PKEY_Title,
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize,
                StructuredStorage::{
                    PropVariantClear, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0,
                    PROPVARIANT_0_0_0,
                },
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            },
            Variant::VT_BSTR,
        },
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection},
            DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
            PropertiesSystem::IPropertyStore,
            ShellLink,
        },
    },
};

use crate::{media_command_argument, MediaControlCommand, MEDIA_COMMAND_ARGUMENT_NAME};

const JUMP_LIST_TASKS: &[JumpListTask] = &[
    JumpListTask {
        title: "Play/Pause",
        description: "Toggle flutzPlayer playback",
        command: MediaControlCommand::TogglePlayPause,
    },
    JumpListTask {
        title: "Stop",
        description: "Stop flutzPlayer playback",
        command: MediaControlCommand::Stop,
    },
    JumpListTask {
        title: "Next Track",
        description: "Play the next flutzPlayer playlist track",
        command: MediaControlCommand::NextTrack,
    },
    JumpListTask {
        title: "Previous Track",
        description: "Play the previous flutzPlayer playlist track",
        command: MediaControlCommand::PreviousTrack,
    },
];

struct JumpListTask {
    title: &'static str,
    description: &'static str,
    command: MediaControlCommand,
}

pub fn register_jump_list_tasks() -> Result<()> {
    let executable = env::current_exe().map_err(|error| windows::core::Error::from(error))?;
    let _apartment = ComApartment::initialize();
    unsafe { register_jump_list_tasks_inner(&executable) }
}

unsafe fn register_jump_list_tasks_inner(executable: &Path) -> Result<()> {
    let destination_list: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)? };
    let mut min_slots = 0u32;
    let _removed: IObjectArray = unsafe { destination_list.BeginList(&mut min_slots)? };

    let tasks: IObjectCollection =
        unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)? };
    for task in JUMP_LIST_TASKS {
        let link = unsafe { create_shell_link(executable, task)? };
        let unknown: IUnknown = link.cast()?;
        unsafe {
            tasks.AddObject(&unknown)?;
        }
    }

    let task_array: IObjectArray = tasks.cast()?;
    unsafe {
        destination_list.AddUserTasks(&task_array)?;
        destination_list.CommitList()
    }
}

unsafe fn create_shell_link(executable: &Path, task: &JumpListTask) -> Result<IShellLinkW> {
    let command = media_command_argument(&task.command).expect("jump list commands are verbs");
    let executable = HSTRING::from(executable.to_string_lossy().as_ref());
    let arguments = HSTRING::from(format!("{MEDIA_COMMAND_ARGUMENT_NAME} {command}"));
    let description = HSTRING::from(task.description);

    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        link.SetPath(&executable)?;
        link.SetArguments(&arguments)?;
        link.SetDescription(&description)?;
        link.SetIconLocation(&executable, 0)?;
    }

    let property_store: IPropertyStore = link.cast()?;
    let mut title = propvariant_from_bstr(task.title);
    let result = unsafe { set_title_property(&property_store, &mut title) };
    unsafe {
        let _ = PropVariantClear(&mut title);
    }
    result?;

    Ok(link)
}

unsafe fn set_title_property(
    property_store: &IPropertyStore,
    title: &mut PROPVARIANT,
) -> Result<()> {
    let key: PROPERTYKEY = PKEY_Title;
    unsafe {
        property_store.SetValue(&key, title)?;
        property_store.Commit()
    }
}

fn propvariant_from_bstr(value: &str) -> PROPVARIANT {
    PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    bstrVal: ManuallyDrop::new(BSTR::from(value)),
                },
            }),
        },
    }
}

struct ComApartment {
    initialized: bool,
}

impl ComApartment {
    fn initialize() -> Self {
        let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        Self { initialized }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
