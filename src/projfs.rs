use std::{io::ErrorKind, os::windows::prelude::OsStrExt, path::PathBuf, sync::Arc};

use anyhow::Context;
use uuid::Uuid;
use windows::{
    core::{GUID, HRESULT, PCWSTR},
    Win32::Storage::ProjectedFileSystem::*,
};

pub struct ProjFs<B>
where
    B: ProjFsBackend,
{
    root_path: PathBuf,
    root_path_wide: Vec<u16>,
    options: PRJ_STARTVIRTUALIZING_OPTIONS,
    backend: Box<Arc<B>>,
    instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    state: FsState,
}

pub trait ProjFsBackend: Send + Sync {
    fn set_instance_handle(self: &Arc<Self>, instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT);

    unsafe fn start_dir_enum(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> HRESULT;

    unsafe fn end_dir_enum(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
    ) -> HRESULT;

    unsafe fn get_dir_enum(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        enumeration_id: Uuid,
        search_expr: PCWSTR,
        dir_entry_buffer_handle: PRJ_DIR_ENTRY_BUFFER_HANDLE,
    ) -> HRESULT;

    unsafe fn get_placeholder_info(self: &Arc<Self>, callback_data: &PRJ_CALLBACK_DATA) -> HRESULT;

    unsafe fn get_file_data(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        byte_offset: u64,
        length: u32,
    ) -> HRESULT;
}

#[derive(Debug, PartialEq, Eq)]
enum FsState {
    Ready,
    Running,
    Stopped,
}

impl<B> ProjFs<B>
where
    B: ProjFsBackend,
{
    pub fn new(
        root_path: PathBuf,
        options: PRJ_STARTVIRTUALIZING_OPTIONS,
        backend: B,
    ) -> ProjFs<B> {
        let root_path_wide = root_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        ProjFs {
            root_path,
            root_path_wide,
            options,
            backend: Box::new(Arc::new(backend)),
            instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT::default(),
            state: FsState::Ready,
        }
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.state != FsState::Ready {
            panic!(
                "start() should only be called in a ready state; current state = {:?}",
                self.state,
            );
        }

        self.ensure_virtualization_root()
            .context("ensure_virtualization_root")?;

        let callbacks = self.create_callbacks();
        let instance_handle = unsafe {
            PrjStartVirtualizing(
                PCWSTR::from_raw(self.root_path_wide.as_ptr()),
                &callbacks,
                self.backend.as_ref() as *const Arc<B> as *const _,
                &self.options,
            )
        }
        .context("start virtualizing")?;
        // FIXME: Potential race condition here
        self.backend.set_instance_handle(instance_handle);
        self.instance_handle = instance_handle;
        self.state = FsState::Running;
        Ok(())
    }

    fn ensure_virtualization_root(&self) -> anyhow::Result<()> {
        const INSTANCE_ID_FILE: &str = ".projfs-id";

        match std::fs::create_dir(&self.root_path) {
            Ok(()) => {
                // This directory is newly created. Create a UUID for this
                // directory, and write it to a file named .projfs-id.
                let instance_id = Uuid::new_v4();
                std::fs::write(
                    self.root_path.join(INSTANCE_ID_FILE),
                    instance_id.as_bytes(),
                )
                .context("write instance ID file")?;
                // TODO: cleanup on error return

                unsafe {
                    PrjMarkDirectoryAsPlaceholder(
                        PCWSTR::from_raw(self.root_path_wide.as_ptr()),
                        PCWSTR::null(),
                        std::ptr::null(),
                        instance_id.as_bytes().as_ptr().cast(),
                    )
                }
                .context("mark directory as placeholder")?;

                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let instance_id = std::fs::read(self.root_path.join(INSTANCE_ID_FILE))
                    .context("read instance ID file")?;
                let _instance_id = Uuid::from_slice(&instance_id).context("parse instance ID")?;
                // TODO: do something with _instance_id
                Ok(())
            }
            result => result.context("create virtualization root"),
        }
    }

    fn create_callbacks(&self) -> PRJ_CALLBACKS {
        unsafe fn backend<'a, B>(callback_data: *const PRJ_CALLBACK_DATA) -> &'a Arc<B> {
            &*((*callback_data).InstanceContext as *const Arc<B>)
        }

        unsafe fn uuid(guid: *const GUID) -> Uuid {
            // Use byte reinterpretation instead of destruct-and-reconstruct
            // to gain performance
            Uuid::from_slice(std::slice::from_raw_parts(
                guid as *const u8,
                std::mem::size_of::<GUID>(),
            ))
            .unwrap() // this must not cause errors
        }

        unsafe extern "system" fn start_dir_enum_cb<B: ProjFsBackend>(
            callback_data: *const PRJ_CALLBACK_DATA,
            enumeration_id: *const GUID,
        ) -> HRESULT {
            backend::<B>(callback_data).start_dir_enum(&*callback_data, uuid(enumeration_id))
        }

        unsafe extern "system" fn end_dir_enum_cb<B: ProjFsBackend>(
            callback_data: *const PRJ_CALLBACK_DATA,
            enumeration_id: *const GUID,
        ) -> HRESULT {
            backend::<B>(callback_data).end_dir_enum(&*callback_data, uuid(enumeration_id))
        }

        unsafe extern "system" fn get_dir_enum_cb<B: ProjFsBackend>(
            callback_data: *const PRJ_CALLBACK_DATA,
            enumeration_id: *const GUID,
            search_expr: PCWSTR,
            dir_entry_buffer_handle: PRJ_DIR_ENTRY_BUFFER_HANDLE,
        ) -> HRESULT {
            backend::<B>(callback_data).get_dir_enum(
                &*callback_data,
                uuid(enumeration_id),
                search_expr,
                dir_entry_buffer_handle,
            )
        }

        unsafe extern "system" fn get_placeholder_info_cb<B: ProjFsBackend>(
            callback_data: *const PRJ_CALLBACK_DATA,
        ) -> HRESULT {
            backend::<B>(callback_data).get_placeholder_info(&*callback_data)
        }

        unsafe extern "system" fn get_file_data_cb<B: ProjFsBackend>(
            callback_data: *const PRJ_CALLBACK_DATA,
            byte_offset: u64,
            length: u32,
        ) -> HRESULT {
            backend::<B>(callback_data).get_file_data(&*callback_data, byte_offset, length)
        }

        PRJ_CALLBACKS {
            StartDirectoryEnumerationCallback: Some(start_dir_enum_cb::<B>),
            EndDirectoryEnumerationCallback: Some(end_dir_enum_cb::<B>),
            GetDirectoryEnumerationCallback: Some(get_dir_enum_cb::<B>),
            GetPlaceholderInfoCallback: Some(get_placeholder_info_cb::<B>),
            GetFileDataCallback: Some(get_file_data_cb::<B>),
            ..Default::default()
        }
    }

    pub fn stop(&mut self) {
        if self.state != FsState::Running {
            panic!(
                "stop() should only be called in a ready state; current state = {:?}",
                self.state,
            );
        }

        unsafe {
            PrjStopVirtualizing(self.instance_handle);
        }
        self.state = FsState::Stopped;
    }
}

impl<B> Drop for ProjFs<B>
where
    B: ProjFsBackend,
{
    fn drop(&mut self) {
        if self.state == FsState::Running {
            self.stop();
        }
    }
}
