use std::{collections::HashMap, sync::Mutex};

use anyhow::Context;
use uuid::Uuid;
use windows::Win32::{
    Foundation::{BOOLEAN, ERROR_FILE_NOT_FOUND, E_INVALIDARG, E_OUTOFMEMORY, S_OK},
    Storage::ProjectedFileSystem::*,
};

use crate::{dir_enum::SimpleDirEnumerator, projfs::ProjFsBackend};

pub struct SimpleFs {
    state: Mutex<SimpleFsState>,
}

struct SimpleFsState {
    instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    dir_enums: HashMap<Uuid, DirEnumerator>,
}

type DirEnumerator = SimpleDirEnumerator<std::iter::Once<(&'static str, Option<u32>)>>;

const FILE_CONTENTS: &str = "Hello, Windows 10 projected FS!\r\n";

impl SimpleFs {
    pub fn new() -> SimpleFs {
        SimpleFs {
            state: Mutex::new(SimpleFsState {
                instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT::default(),
                dir_enums: HashMap::new(),
            }),
        }
    }

    fn enum_root_dir() -> DirEnumerator {
        SimpleDirEnumerator::new(std::iter::once((
            "Hello.txt",
            Some(FILE_CONTENTS.len() as u32),
        )))
    }
}

impl ProjFsBackend for SimpleFs {
    fn set_instance_handle(
        self: &std::sync::Arc<Self>,
        instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    ) {
        self.state.lock().unwrap().instance_handle = instance_handle;
    }

    unsafe fn start_dir_enum(
        self: &std::sync::Arc<Self>,
        _callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> windows::core::HRESULT {
        self.state
            .lock()
            .unwrap()
            .dir_enums
            .insert(enumeration_id, Self::enum_root_dir());
        S_OK
    }

    unsafe fn end_dir_enum(
        self: &std::sync::Arc<Self>,
        _callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> windows::core::HRESULT {
        self.state.lock().unwrap().dir_enums.remove(&enumeration_id);
        S_OK
    }

    unsafe fn get_dir_enum(
        self: &std::sync::Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
        search_expr: windows::core::PCWSTR,
        dir_entry_buffer_handle: PRJ_DIR_ENTRY_BUFFER_HANDLE,
    ) -> windows::core::HRESULT {
        match self
            .state
            .lock()
            .unwrap()
            .dir_enums
            .get_mut(&enumeration_id)
        {
            Some(dir_enum) => {
                dir_enum.get_dir_enum(callback_data, search_expr, dir_entry_buffer_handle);
                S_OK
            }
            None => E_INVALIDARG,
        }
    }

    unsafe fn get_placeholder_info(
        self: &std::sync::Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
    ) -> windows::core::HRESULT {
        let state = self.state.lock().unwrap();
        let result = (|| {
            if !callback_data
                .FilePathName
                .to_string()
                .ok()
                .as_ref()
                .map(|s| s == "Hello.txt")
                .unwrap_or(false)
            {
                return anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult());
            }

            let placeholder_info = PRJ_PLACEHOLDER_INFO {
                FileBasicInfo: PRJ_FILE_BASIC_INFO {
                    IsDirectory: BOOLEAN(0),
                    FileSize: FILE_CONTENTS.len() as i64,
                    ..Default::default()
                },
                ..Default::default()
            };

            PrjWritePlaceholderInfo(
                state.instance_handle,
                callback_data.FilePathName,
                &placeholder_info,
                std::mem::size_of::<PRJ_PLACEHOLDER_INFO>() as u32,
            )
            .context("write placeholder info")?;
            anyhow::Ok(S_OK)
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                eprintln!("Error in get_file_data: {:#}", err);
                E_INVALIDARG
            }
        }
    }

    unsafe fn get_file_data(
        self: &std::sync::Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        byte_offset: u64,
        length: u32,
    ) -> windows::core::HRESULT {
        let state = self.state.lock().unwrap();
        let result = (|| {
            if !callback_data
                .FilePathName
                .to_string()
                .ok()
                .as_ref()
                .map(|s| s == "Hello.txt")
                .unwrap_or(false)
            {
                return anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult());
            }

            let length = length.min(FILE_CONTENTS.len() as u32);
            let buffer = PrjAllocateAlignedBuffer(state.instance_handle, length as usize);
            if buffer.is_null() {
                return anyhow::Ok(E_OUTOFMEMORY);
            }

            std::slice::from_raw_parts_mut(buffer as *mut u8, length as usize)
                .copy_from_slice(FILE_CONTENTS[..length as usize].as_bytes());

            PrjWriteFileData(
                state.instance_handle,
                &callback_data.DataStreamId,
                buffer,
                byte_offset,
                length,
            )
            .context("write file data")?;

            PrjFreeAlignedBuffer(buffer);

            anyhow::Ok(S_OK)
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                eprintln!("Error in get_file_data: {:#}", err);
                E_INVALIDARG
            }
        }
    }
}
