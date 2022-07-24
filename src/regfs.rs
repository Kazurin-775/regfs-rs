use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use itertools::Itertools;
use uuid::Uuid;
use windows::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, E_FAIL, E_INVALIDARG, STATUS_CANNOT_DELETE, S_OK,
    },
    Storage::ProjectedFileSystem::*,
};

use crate::{
    dir_enum::SimpleDirEnumerator,
    fs_helper::SimpleFsHelper,
    projfs::{NotificationKind, OptionalFeatures, ProjFsBackend},
    reg_ops,
};

pub struct RegFs {
    state: Mutex<RegFsState>,
}

struct RegFsState {
    fs_helper: SimpleFsHelper,
    dir_enums: HashMap<Uuid, DirEnumerator>,
}

type DirEnumerator = SimpleDirEnumerator<std::vec::IntoIter<(String, Option<u32>)>>;

impl RegFs {
    pub fn new() -> RegFs {
        RegFs {
            state: Mutex::new(RegFsState {
                fs_helper: SimpleFsHelper::default(),
                dir_enums: HashMap::new(),
            }),
        }
    }
}

impl ProjFsBackend for RegFs {
    fn get_optional_features() -> OptionalFeatures {
        OptionalFeatures::NOTIFY
    }

    fn set_instance_handle(
        self: &Arc<Self>,
        instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    ) {
        log::debug!("RegFS backend initialized");
        self.state.lock().unwrap().fs_helper = SimpleFsHelper::new(instance_handle);
    }

    unsafe fn start_dir_enum(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> windows::core::HRESULT {
        let mut state = self.state.lock().unwrap();
        let result = (|| {
            let path = state
                .fs_helper
                .get_req_path(callback_data)
                .context("path is not valid UTF-8")?;

            log::trace!(
                "Start directory enumeration: ID {}, path {:?}",
                enumeration_id,
                path,
            );

            let enumerator = if path.is_empty() {
                // Root directory
                SimpleDirEnumerator::new(
                    reg_ops::HKEYS
                        .keys()
                        .map(|&name| (String::from(name), None))
                        .sorted_unstable(),
                )
            } else if let Some(key) = reg_ops::open_key(&path).context("open key")? {
                // Enumerate both subkeys and values
                let keys_iter = key.enum_keys().map_ok(|name| (name, None));
                let values_iter = key.enum_values().map_ok(|(name, value)| {
                    (
                        name,
                        Some(value.bytes.len().try_into().expect("integer overflow")),
                    )
                });

                let mut items: Vec<(String, Option<u32>)> =
                    Itertools::try_collect(keys_iter.chain(values_iter))
                        .context("enumerate subkeys and values")?;
                items.sort_unstable();

                SimpleDirEnumerator::new(items.into_iter())
            } else {
                // A non-existent key is specified
                return anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult());
            };

            state.dir_enums.insert(enumeration_id, enumerator);
            anyhow::Ok(S_OK)
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                log::error!("Error enumerating directory: {:#}", err);
                err.downcast::<windows::core::Error>()
                    .map(Into::into)
                    .unwrap_or(E_FAIL)
            }
        }
    }

    unsafe fn end_dir_enum(
        self: &Arc<Self>,
        _callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> windows::core::HRESULT {
        self.state.lock().unwrap().dir_enums.remove(&enumeration_id);
        S_OK
    }

    unsafe fn get_dir_enum(
        self: &Arc<Self>,
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
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
    ) -> windows::core::HRESULT {
        let state = self.state.lock().unwrap();
        let result = (|| {
            let path = state
                .fs_helper
                .get_req_path(callback_data)
                .context("invalid path specified")?;
            log::trace!("Get placeholder info: {:?}", path);

            if reg_ops::does_key_exist(&path).context("check key existence")? {
                state
                    .fs_helper
                    .write_placeholder_info(callback_data, None)
                    .context("write placeholder info")?;
                anyhow::Ok(S_OK)
            } else if let Some(value) =
                reg_ops::read_value(&path).context("check value existence")?
            {
                state
                    .fs_helper
                    .write_placeholder_info(
                        callback_data,
                        Some(value.bytes.len().try_into().expect("integer overflow")),
                    )
                    .context("write placeholder info")?;
                anyhow::Ok(S_OK)
            } else {
                anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult())
            }
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                log::error!("Error in get_placeholder_info: {:#}", err);
                err.downcast::<windows::core::Error>()
                    .map(Into::into)
                    .unwrap_or(E_FAIL)
            }
        }
    }

    unsafe fn get_file_data(
        self: &Arc<Self>,
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

            if let Some(value) = reg_ops::read_value(&path).context("read value")? {
                let mut buffer = state
                    .fs_helper
                    .alloc_aligned_buffer(value.bytes.len())
                    .context("allocate buffer")?;
                buffer.copy_from_slice(&value.bytes);
                state
                    .fs_helper
                    .write_file_data(callback_data, &buffer, 0)
                    .context("write file data")?;
                anyhow::Ok(S_OK)
            } else {
                anyhow::Ok(ERROR_FILE_NOT_FOUND.to_hresult())
            }
        })();
        match result {
            Ok(hresult) => hresult,
            Err(err) => {
                log::error!("Error in get_file_data: {:#}", err);
                err.downcast::<windows::core::Error>()
                    .map(Into::into)
                    .unwrap_or(E_FAIL)
            }
        }
    }

    unsafe fn notify(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        _is_dir: bool,
        kind: NotificationKind,
        dest_filename: windows::core::PCWSTR,
        _params: *mut PRJ_NOTIFICATION_PARAMETERS,
    ) -> windows::core::HRESULT {
        match kind {
            NotificationKind::FileOpened => (),
            NotificationKind::NewFileCreated => {
                log::debug!(
                    "New file created: {:?}",
                    callback_data.FilePathName.to_string(),
                );
            }
            NotificationKind::FileOverwritten | NotificationKind::FileHandleClosedFileModified => {
                log::debug!(
                    "File modified: {:?}",
                    callback_data.FilePathName.to_string(),
                );
            }
            NotificationKind::FileRenamed => {
                log::debug!(
                    "File renamed: {:?} -> {:?}",
                    callback_data.FilePathName.to_string(),
                    dest_filename.to_string(),
                );
            }
            NotificationKind::FileHandleClosedFileDeleted => {
                log::debug!("File deleted: {:?}", callback_data.FilePathName.to_string());
            }
            NotificationKind::PreDelete => {
                log::debug!(
                    "Denying file deletion: {:?}",
                    callback_data.FilePathName.to_string(),
                );
                return ERROR_ACCESS_DENIED.to_hresult();
            }
            NotificationKind::PreRename => {
                log::debug!(
                    "Denying file rename: {:?}",
                    callback_data.FilePathName.to_string(),
                );
                return STATUS_CANNOT_DELETE.to_hresult();
            }
            other => {
                log::warn!("Unknown notification kind: {:?}", other);
            }
        }
        S_OK
    }
}
