use windows::Win32::{
    Foundation::{BOOLEAN, E_OUTOFMEMORY},
    Storage::ProjectedFileSystem::*,
};

#[derive(Default)]
pub struct SimpleFsHelper {
    instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
}

pub struct FsBuffer {
    ptr: *mut u8,
    len: usize,
}

impl SimpleFsHelper {
    pub fn new(instance_handle: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT) -> SimpleFsHelper {
        SimpleFsHelper { instance_handle }
    }

    pub unsafe fn get_req_path(
        &self,
        callback_data: &PRJ_CALLBACK_DATA,
    ) -> Result<String, std::string::FromUtf16Error> {
        callback_data.FilePathName.to_string()
    }

    pub unsafe fn write_placeholder_info(
        &self,
        callback_data: &PRJ_CALLBACK_DATA,
        file_size: Option<i64>,
    ) -> windows::core::Result<()> {
        let placeholder_info = PRJ_PLACEHOLDER_INFO {
            FileBasicInfo: PRJ_FILE_BASIC_INFO {
                IsDirectory: BOOLEAN(file_size.is_none() as u8),
                FileSize: file_size.unwrap_or(0),
                ..Default::default()
            },
            ..Default::default()
        };

        PrjWritePlaceholderInfo(
            self.instance_handle,
            callback_data.FilePathName,
            &placeholder_info,
            std::mem::size_of::<PRJ_PLACEHOLDER_INFO>() as u32,
        )
    }

    pub fn alloc_aligned_buffer(&self, size: usize) -> windows::core::Result<FsBuffer> {
        let ptr = unsafe { PrjAllocateAlignedBuffer(self.instance_handle, size) };
        if ptr.is_null() {
            Err(E_OUTOFMEMORY.into())
        } else {
            Ok(FsBuffer {
                ptr: ptr.cast(),
                len: size,
            })
        }
    }

    pub unsafe fn write_file_data(
        &self,
        callback_data: &PRJ_CALLBACK_DATA,
        buffer: &[u8],
        byte_offset: u64,
    ) -> windows::core::Result<()> {
        PrjWriteFileData(
            self.instance_handle,
            &callback_data.DataStreamId,
            buffer.as_ptr().cast(),
            byte_offset,
            buffer.len().try_into().expect("buffer too large"),
        )
    }
}

impl std::ops::Deref for FsBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl std::ops::DerefMut for FsBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for FsBuffer {
    fn drop(&mut self) {
        unsafe {
            PrjFreeAlignedBuffer(self.ptr.cast());
        }
    }
}
