use std::{mem::size_of, ptr};
use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

const PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const NAME: &str = "WifiRadio";

pub fn enabled() -> bool {
    unsafe {
        let mut key = ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(PATH).as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let result = RegQueryValueExW(
            key,
            wide(NAME).as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        RegCloseKey(key);
        result == ERROR_SUCCESS
    }
}

pub fn set(enabled: bool) -> Result<(), String> {
    unsafe {
        let mut key = ptr::null_mut();
        let result = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(PATH).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        );
        if result != ERROR_SUCCESS {
            return Err(format!("Автозапуск: ошибка Windows {result}"));
        }
        let result = if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = wide(&format!("\"{}\" --background", exe.display()));
            RegSetValueExW(
                key,
                wide(NAME).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * size_of::<u16>()) as u32,
            )
        } else {
            RegDeleteValueW(key, wide(NAME).as_ptr())
        };
        RegCloseKey(key);
        if result == ERROR_SUCCESS || (!enabled && result == 2) {
            Ok(())
        } else {
            Err(format!("Автозапуск: ошибка Windows {result}"))
        }
    }
}

pub fn initialize() -> Result<(), String> {
    // A marker only records that the initial default has been applied. The Run
    // value itself stays authoritative, so Windows Settings can disable it.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let marker = exe.with_file_name("wifiradio.initialized");
    if marker.exists() && enabled() {
        // A versioned update can have a new filename. Preserve disabled startup,
        // but migrate an enabled entry so sign-in does not revive an old binary.
        set(true)?;
    }
    if !marker.exists() {
        set(true)?;
        std::fs::write(
            marker,
            b"Autostart was initialized. Toggle it in the tray menu.\r\n",
        )
        .map_err(|e| format!("Не удалось сохранить настройки: {e}"))?;
    }
    Ok(())
}
