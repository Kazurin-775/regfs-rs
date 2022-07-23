use std::{ffi::OsStr, iter::Peekable, os::windows::prelude::OsStrExt};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{BOOLEAN, ERROR_INSUFFICIENT_BUFFER},
        Storage::ProjectedFileSystem::*,
    },
};

pub struct SimpleDirEnumerator<I>
where
    I: Iterator,
{
    cur: Peekable<I>,
    start: I,
}

impl<'a, I, S> SimpleDirEnumerator<I>
where
    I: Iterator<Item = (S, Option<u32>)> + Clone,
    S: AsRef<str>,
{
    pub fn new(iter: I) -> SimpleDirEnumerator<I> {
        SimpleDirEnumerator {
            cur: iter.clone().peekable(),
            start: iter,
        }
    }

    pub unsafe fn get_dir_enum(
        &mut self,
        callback_data: *const PRJ_CALLBACK_DATA,
        search_expr: PCWSTR,
        dir_entry_buffer_handle: PRJ_DIR_ENTRY_BUFFER_HANDLE,
    ) {
        if (*callback_data).Flags.0 & PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN.0 != 0 {
            self.cur = self.start.clone().peekable();
        }

        while let Some((name, len)) = self.cur.peek().as_ref() {
            let name_wstr: Vec<u16> = OsStr::new(name.as_ref())
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // Check if the file name matches the search condition
            if PrjFileNameMatch(PCWSTR::from_raw(name_wstr.as_ptr()), search_expr).0 == 0 {
                self.cur.next();
                continue;
            }

            let file_info = PRJ_FILE_BASIC_INFO {
                IsDirectory: BOOLEAN(len.is_none() as u8),
                FileSize: len.unwrap_or(0) as i64,
                ..Default::default()
            };

            match PrjFillDirEntryBuffer(
                PCWSTR::from_raw(name_wstr.as_ptr()),
                &file_info,
                dir_entry_buffer_handle,
            ) {
                Ok(()) => {
                    self.cur.next();
                }
                Err(err) if err.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {
                    // The dir_entry_buffer is full, stop so that the client
                    // can process the items in the buffer.
                    log::debug!("Directory entry buffer full");
                    break;
                }
                Err(err) => {
                    // This branch will be reached if the file name supplied is
                    // invalid, such as when the file name contains '*' or '/'.
                    log::warn!(
                        "Failed to fill directory entry buffer for {:?}: {}",
                        name.as_ref(),
                        err,
                    );
                    // Skip this item.
                    self.cur.next();
                }
            }
        }
    }
}
