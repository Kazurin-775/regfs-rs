# regfs-rs

A re-implementation of [RegFS](https://github.com/Microsoft/Windows-classic-samples/tree/main/Samples/ProjectedFileSystem), a sample project for the [Windows Projected File System (ProjFS)](https://docs.microsoft.com/en-us/windows/desktop/projfs/projected-file-system), in the Rust language.

The original project is written in C++, and licensed by Microsoft under the [MIT License](./LICENSE-RegFS).

For more information, please refer to the [original project](https://github.com/Microsoft/Windows-classic-samples/tree/main/Samples/ProjectedFileSystem).

## Running

This project can be run in almost exactly the same way as the original project, except that one should use `cargo build` to build the project, instead of Visual Studio. Also, please make sure you have ProjFS enabled on your local system (details in the original project's documentation).

Logs are disabled by default. To enable logging, set the environment variable `RUST_LOG` to the log level you want, e.g. `debug` or `trace`.

## Notes

Note that several quirks exist in this project, due to the project's simplified implementation. (These quirks also exist in the original project.) For example:

- The file system cannot distinguish between different value types; values are represented in their raw forms (e.g. DWORD values are simply represented as 4 bytes, and strings are represented by null-terminated wide strings).
- It is not able to display keys and values with illegal characters (such as `*` and `/`) in their names (interestingly, these characters are not prohibited in registry hives).
- Keys or values whose names end with `.` may not be accessible (may result in an error when accessed).
- Most file attributes (creation / modification time, security attributes, etc.) are not present; specifically, only file names and sizes are supplied.
- If the program is run multiple times, multiple instances of a same file / directory may show up in the directory listings (probably due to the absence of correct file attributes).

Due to the lack of write support, any changes made to the file system will not be reflected in the system registry. It may be non-trivial to add write support to the current implementation.
