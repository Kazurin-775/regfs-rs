use std::{io::ErrorKind, os::windows::prelude::OsStrExt, path::PathBuf, sync::Arc};

use anyhow::Context;
use uuid::Uuid;
use windows::{
    core::{GUID, HRESULT, PCWSTR},
    Win32::{Foundation::BOOLEAN, Storage::ProjectedFileSystem::*},
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
    fn get_optional_features() -> OptionalFeatures;

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

    unsafe fn notify(
        self: &Arc<Self>,
        callback_data: &PRJ_CALLBACK_DATA,
        is_dir: bool,
        kind: NotificationKind,
        dest_filename: PCWSTR,
        params: *mut PRJ_NOTIFICATION_PARAMETERS,
    ) -> HRESULT;
}

#[derive(Debug, PartialEq, Eq)]
enum FsState {
    Ready,
    Running,
    Stopped,
}

bitflags::bitflags! {
    pub struct OptionalFeatures: u32 {
        const NOTIFY = 1;
        const QUERY_FILE_NAME = 2;
        const CANCEL_COMMAND = 4;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    FileOpened,
    NewFileCreated,
    FileOverwritten,
    PreDelete,
    PreRename,
    PreSetHardlink,
    FileRenamed,
    HardlinkCreated,
    FileHandleClosedNoModification,
    FileHandleClosedFileModified,
    FileHandleClosedFileDeleted,
    FilePreConvertToFull,
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
                log::debug!("Created new instance ID {}", instance_id);
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
                let instance_id = Uuid::from_slice(&instance_id).context("parse instance ID")?;
                log::debug!("Found old instance ID {}", instance_id);
                // TODO: do something with instance_id
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

        unsafe extern "system" fn notification_cb<B: ProjFsBackend>(
            callback_data: *const PRJ_CALLBACK_DATA,
            is_dir: BOOLEAN,
            notification: PRJ_NOTIFICATION,
            dest_filename: PCWSTR,
            params: *mut PRJ_NOTIFICATION_PARAMETERS,
        ) -> HRESULT {
            backend::<B>(callback_data).notify(
                &*callback_data,
                is_dir.0 != 0,
                notification.into(),
                dest_filename,
                params,
            )
        }

        let features = B::get_optional_features();

        PRJ_CALLBACKS {
            StartDirectoryEnumerationCallback: Some(start_dir_enum_cb::<B>),
            EndDirectoryEnumerationCallback: Some(end_dir_enum_cb::<B>),
            GetDirectoryEnumerationCallback: Some(get_dir_enum_cb::<B>),
            GetPlaceholderInfoCallback: Some(get_placeholder_info_cb::<B>),
            GetFileDataCallback: Some(get_file_data_cb::<B>),
            QueryFileNameCallback: None,
            NotificationCallback: if features.contains(OptionalFeatures::NOTIFY) {
                Some(notification_cb::<B>)
            } else {
                None
            },
            CancelCommandCallback: None,
        }
    }

    pub fn stop(&mut self) {
        if self.state != FsState::Running {
            panic!(
                "stop() should only be called in a ready state; current state = {:?}",
                self.state,
            );
        }

        log::debug!("Stopping projection FS");
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

impl From<PRJ_NOTIFICATION> for NotificationKind {
    #[rustfmt::skip]
    fn from(n: PRJ_NOTIFICATION) -> Self {
        match n {
            PRJ_NOTIFICATION_FILE_OPENED => Self::FileOpened,
            PRJ_NOTIFICATION_NEW_FILE_CREATED => Self::NewFileCreated,
            PRJ_NOTIFICATION_FILE_OVERWRITTEN => Self::FileOverwritten,
            PRJ_NOTIFICATION_PRE_DELETE => Self::PreDelete,
            PRJ_NOTIFICATION_PRE_RENAME => Self::PreRename,
            PRJ_NOTIFICATION_PRE_SET_HARDLINK => Self::PreSetHardlink,
            PRJ_NOTIFICATION_FILE_RENAMED => Self::FileRenamed,
            PRJ_NOTIFICATION_HARDLINK_CREATED => Self::HardlinkCreated,
            PRJ_NOTIFICATION_FILE_HANDLE_CLOSED_NO_MODIFICATION =>
                Self::FileHandleClosedNoModification,
            PRJ_NOTIFICATION_FILE_HANDLE_CLOSED_FILE_MODIFIED =>
                Self::FileHandleClosedFileModified,
            PRJ_NOTIFICATION_FILE_HANDLE_CLOSED_FILE_DELETED =>
                Self::FileHandleClosedFileDeleted,
            PRJ_NOTIFICATION_FILE_PRE_CONVERT_TO_FULL =>
                Self::FilePreConvertToFull,
            other => panic!("unknown notification type: {}", other.0),
        }
    }
}

impl From<NotificationKind> for PRJ_NOTIFICATION {
    #[rustfmt::skip]
    fn from(n: NotificationKind) -> Self {
        use NotificationKind as K;
        match n {
            K::FileOpened => PRJ_NOTIFICATION_FILE_OPENED,
            K::NewFileCreated => PRJ_NOTIFICATION_NEW_FILE_CREATED,
            K::FileOverwritten => PRJ_NOTIFICATION_FILE_OVERWRITTEN,
            K::PreDelete => PRJ_NOTIFICATION_PRE_DELETE,
            K::PreRename => PRJ_NOTIFICATION_PRE_RENAME,
            K::PreSetHardlink => PRJ_NOTIFICATION_PRE_SET_HARDLINK,
            K::FileRenamed => PRJ_NOTIFICATION_FILE_RENAMED,
            K::HardlinkCreated => PRJ_NOTIFICATION_HARDLINK_CREATED,
            K::FileHandleClosedNoModification =>
                PRJ_NOTIFICATION_FILE_HANDLE_CLOSED_NO_MODIFICATION,
            K::FileHandleClosedFileModified =>
                PRJ_NOTIFICATION_FILE_HANDLE_CLOSED_FILE_MODIFIED,
            K::FileHandleClosedFileDeleted =>
                PRJ_NOTIFICATION_FILE_HANDLE_CLOSED_FILE_DELETED,
            K::FilePreConvertToFull =>
                PRJ_NOTIFICATION_FILE_PRE_CONVERT_TO_FULL,
        }
    }
}
