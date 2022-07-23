use std::{collections::HashMap, sync::Mutex};

use anyhow::Context;
use uuid::Uuid;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, E_FAIL, E_INVALIDARG, S_OK},
        Storage::ProjectedFileSystem::*,
    },
};

use crate::{dir_enum::SimpleDirEnumerator, fs_helper::SimpleFsHelper, projfs::ProjFsBackend};

pub struct SimpleFs {
    state: Mutex<SimpleFsState>,
}

struct SimpleFsState {
    fs_helper: SimpleFsHelper,
    dir_enums: HashMap<Uuid, DirEnumerator>,
}

type DirEnumerator = SimpleDirEnumerator<std::iter::Once<(&'static str, Option<u32>)>>;

const FILE_CONTENTS: &str = "Hello, Windows 10 projected FS!\r\n";

impl SimpleFs {
    pub fn new() -> SimpleFs {
        SimpleFs {
            state: Mutex::new(SimpleFsState {
                fs_helper: SimpleFsHelper::default(),
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
        log::debug!("Simple FS backend initialized");
        self.state.lock().unwrap().fs_helper = SimpleFsHelper::new(instance_handle);
    }

    unsafe fn start_dir_enum(
        self: &std::sync::Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> windows::core::HRESULT {
        log::trace!(
            "Start directory enumeration: ID {}, path {:?}",
            enumeration_id,
            callback_data.FilePathName.to_string(),
        );
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
        log::trace!("End directory enumeration: ID {}", enumeration_id);
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
        log::trace!(
            "Get directory enumeration: ID {}, path {:?}, search {:?}",
            enumeration_id,
            callback_data.FilePathName.to_string(),
            Option::<PCWSTR>::from(search_expr).map(|p| p.to_string()),
        );
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
            let path = state
                .fs_helper
                .get_req_path(callback_data)
                .context("invalid path specified")?;
            log::trace!("Get placeholder info: {:?}", path);
            if path != "Hello.txt" {
                return anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult());
            }

            state
                .fs_helper
                .write_placeholder_info(callback_data, Some(FILE_CONTENTS.len() as i64))
                .context("write placeholder info")?;

            anyhow::Ok(S_OK)
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                log::error!("Error in get_file_data: {:#}", err);
                E_FAIL
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
            let path = state
                .fs_helper
                .get_req_path(callback_data)
                .context("invalid path specified")?;
            log::trace!(
                "Get file data: {:?}; offset {}, len {}",
                path,
                byte_offset,
                length,
            );
            if path != "Hello.txt" {
                return anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult());
            }

            // The provider is allowed to provide more bytes than expected,
            // since the data provided will be written to the underlying
            // storage device.
            // Here, we simply provide all of the data to the backend,
            // eliminating the need to compute positions and buffer sizes.
            // This also helps us avoid some unnecessary complications such
            // as buffer alignments.
            // For more details, see PrjWriteFileData's documentation.
            assert!(byte_offset + length as u64 <= FILE_CONTENTS.len() as u64);
            let length = FILE_CONTENTS.len();
            let mut buffer = state
                .fs_helper
                .alloc_aligned_buffer(length)
                .context("allocate buffer")?;
            buffer.copy_from_slice(FILE_CONTENTS.as_bytes());
            state
                .fs_helper
                .write_file_data(callback_data, &buffer, 0)
                .context("write file data")?;

            anyhow::Ok(S_OK)
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                eprintln!("Error in get_file_data: {:#}", err);
                E_FAIL
            }
        }
    }
}
