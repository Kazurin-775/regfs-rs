mod dir_enum;
mod fs_helper;
mod projfs;
mod simple_fs;

use std::path::PathBuf;

use projfs::ProjFs;
use simple_fs::SimpleFs;
use windows::{core::PCWSTR, Win32::Storage::ProjectedFileSystem::*};

fn main() {
    let mut args = std::env::args_os();
    if args.len() != 2 {
        eprintln!("Usage: regfs-rs.exe <Virtualization Root Path>");
        std::process::exit(1);
    }

    let root_path = PathBuf::from(args.nth(1).unwrap());

    let mut notification_mappings = PRJ_NOTIFICATION_MAPPING {
        NotificationBitMask: PRJ_NOTIFY_FILE_OPENED | PRJ_NOTIFY_PRE_RENAME | PRJ_NOTIFY_PRE_DELETE,
        NotificationRoot: PCWSTR::from_raw(b"\0\0".as_ptr().cast()),
    };
    let opts = PRJ_STARTVIRTUALIZING_OPTIONS {
        Flags: PRJ_FLAG_NONE,
        PoolThreadCount: 1,
        ConcurrentThreadCount: 1,
        NotificationMappings: &mut notification_mappings,
        NotificationMappingsCount: 1,
    };

    let mut proj_fs = ProjFs::new(root_path, opts, SimpleFs::new());
    proj_fs
        .start()
        .expect("failed to start projection file system");

    println!("Press Enter to stop projection.");
    let mut buf = String::new();
    std::io::stdin()
        .read_line(&mut buf)
        .expect("failed to read stdin");

    proj_fs.stop();
}
