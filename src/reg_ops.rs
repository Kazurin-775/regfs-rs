use std::{collections::HashMap, io::ErrorKind};

use windows::{core::HRESULT, Win32::Foundation::E_FAIL};
use winreg::{RegKey, RegValue, HKEY};

lazy_static::lazy_static! {
    // Sadly, winreg::HKEY does not implement Sync, so we cannot store it in a
    // global variable. Since HKEY is just an alias for HANDLE, while HANDLEs
    // are magic pointers to opaque types, we simply use usizes to store them.
    pub static ref HKEYS: HashMap<&'static str, usize> = {
        let mut hk = HashMap::new();
        hk.insert("HKEY_CLASSES_ROOT", winreg::enums::HKEY_CLASSES_ROOT as _);
        hk.insert("HKEY_CURRENT_USER", winreg::enums::HKEY_CURRENT_USER as _);
        hk.insert("HKEY_LOCAL_MACHINE", winreg::enums::HKEY_LOCAL_MACHINE as _);
        hk.insert("HKEY_USERS", winreg::enums::HKEY_USERS as _);
        hk.insert("HKEY_CURRENT_CONFIG", winreg::enums::HKEY_CURRENT_CONFIG as _);
        hk
    };
}

fn open_key_internal(hkey: &str, path: &str) -> Result<Option<RegKey>, windows::core::Error> {
    if let Some(&hkey) = HKEYS.get(hkey) {
        match RegKey::predef(hkey as HKEY).open_subkey(path) {
            Ok(key) => Ok(Some(key)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => {
                log::warn!("Failed to open key {:?}: {}", path, err);
                Err(err
                    .raw_os_error()
                    .map(|code| HRESULT(code))
                    .unwrap_or(E_FAIL)
                    .into())
            }
        }
    } else {
        // The user specified a non-existent HKEY.
        Ok(None)
    }
}

pub fn open_key(key: &str) -> windows::core::Result<Option<RegKey>> {
    if let Some((hkey, path)) = key.split_once('\\') {
        // The user specified a subkey.
        open_key_internal(hkey, path)
    } else if let Some(&hkey) = HKEYS.get(&key) {
        // The user specified an HKEY.
        Ok(Some(RegKey::predef(hkey as HKEY)))
    } else {
        // The user specified a non-existent HKEY.
        Ok(None)
    }
}

pub fn does_key_exist(key: &str) -> windows::core::Result<bool> {
    if let Some((hkey, path)) = key.split_once('\\') {
        // The user specified a subkey.
        open_key_internal(hkey, path).map(|key| key.is_some())
    } else {
        // The user specified an HKEY.
        Ok(HKEYS.contains_key(key))
    }
}

pub fn read_value(path: &str) -> windows::core::Result<Option<RegValue>> {
    if let Some((path, name)) = path.rsplit_once('\\') {
        // Open the key
        if let Some(key) = open_key(path)? {
            // Find the value
            match key.get_raw_value(name) {
                Ok(value) => Ok(Some(value)),
                Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
                Err(err) => {
                    log::warn!("Failed to read value {:?}: {}", path, err);
                    Err(err
                        .raw_os_error()
                        .map(|code| HRESULT(code))
                        .unwrap_or(E_FAIL)
                        .into())
                }
            }
        } else {
            // The user specified a non-existent key.
            Ok(None)
        }
    } else {
        // The user specified an HKEY, not a value.
        Ok(None)
    }
}
