use crate::logging;
use std::{
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*, NetworkManagement::IpHelper::*, System::Threading::*, UI::WindowsAndMessaging::*,
};

pub const SHOW: u32 = WM_APP + 5;
pub const VERSION: u32 = WM_APP + 6;
pub const TOKEN: LRESULT = 0x5752_0003;
pub const QUIT: u32 = WM_APP + 4;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub struct Guard(HANDLE);
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

unsafe fn request(window: HWND, message: u32) -> Option<usize> {
    let mut result = 0;
    (SendMessageTimeoutW(
        window,
        message,
        0,
        0,
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        700,
        &mut result,
    ) != 0)
        .then_some(result)
}

unsafe fn radio_process(pid: u32) -> Option<HANDLE> {
    let process = OpenProcess(
        PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
        0,
        pid,
    );
    if process.is_null() {
        return None;
    }
    // Only recover this product's executable; never terminate an unrelated app.
    let mut path = [0_u16; 32768];
    let mut length = path.len() as u32;
    let verified = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) != 0
        && std::path::Path::new(&String::from_utf16_lossy(&path[..length as usize]))
            .file_name()
            .is_some_and(|name| {
                name.to_string_lossy().eq_ignore_ascii_case("WifiRadio.exe")
                    || name.to_string_lossy().starts_with("WifiRadio-")
            });
    if !verified {
        CloseHandle(process);
        return None;
    }
    Some(process)
}

unsafe fn close_previous(window: HWND) -> Result<(), String> {
    let mut pid = 0;
    GetWindowThreadProcessId(window, &mut pid);
    let Some(process) = radio_process(pid) else {
        return Err("Не удалось закрыть предыдущую рацию. Проверьте, что файл называется WifiRadio.exe и запущен без прав администратора.".into());
    };
    PostMessageW(window, WM_CANCELMODE, 0, 0);
    PostMessageW(window, QUIT, 0, 0);
    if WaitForSingleObject(process, 2000) == WAIT_TIMEOUT {
        logging::write("recovering unresponsive previous radio");
        if TerminateProcess(process, 0) == 0 || WaitForSingleObject(process, 2000) != WAIT_OBJECT_0
        {
            CloseHandle(process);
            return Err(
                "Старая рация не завершилась. Новый запуск отменён; подробности в wifiradio.log."
                    .into(),
            );
        }
    }
    CloseHandle(process);
    Ok(())
}

unsafe fn recover_orphaned_port() {
    let mut length = 0;
    if GetExtendedUdpTable(ptr::null_mut(), &mut length, 0, 2, UDP_TABLE_OWNER_PID, 0)
        != ERROR_INSUFFICIENT_BUFFER
    {
        return;
    }
    let mut words = vec![0_u32; (length as usize).div_ceil(4)];
    if GetExtendedUdpTable(
        words.as_mut_ptr().cast(),
        &mut length,
        0,
        2,
        UDP_TABLE_OWNER_PID,
        0,
    ) != 0
    {
        return;
    }
    let offset = std::mem::offset_of!(MIB_UDPTABLE_OWNER_PID, table);
    if length as usize <= offset {
        return;
    }
    let count = (words[0] as usize)
        .min((length as usize - offset) / std::mem::size_of::<MIB_UDPROW_OWNER_PID>());
    let rows = words
        .as_ptr()
        .cast::<u8>()
        .add(offset)
        .cast::<MIB_UDPROW_OWNER_PID>();
    for index in 0..count {
        let row = *rows.add(index);
        if u16::from_be(row.dwLocalPort as u16) != crate::protocol::PORT
            || row.dwOwningPid == std::process::id()
        {
            continue;
        }
        if let Some(process) = radio_process(row.dwOwningPid) {
            logging::write(&format!(
                "recovering windowless radio holding UDP port; pid={}",
                row.dwOwningPid
            ));
            TerminateProcess(process, 0);
            WaitForSingleObject(process, 2000);
            CloseHandle(process);
        }
    }
}

/// A new launch either receives a confirmed activation or owns the startup lock.
/// Every failure is returned to the visible launcher, never silently discarded.
pub fn enter(restart: bool) -> Result<Option<Guard>, String> {
    unsafe {
        let class = wide("WifiRadioWindow");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let window = FindWindowW(class.as_ptr(), ptr::null());
            if !window.is_null() {
                let version = request(window, VERSION);
                if version == Some(TOKEN as usize) && !restart {
                    let mut pid = 0;
                    GetWindowThreadProcessId(window, &mut pid);
                    AllowSetForegroundWindow(pid);
                    if request(window, SHOW) == Some(1) && IsWindowVisible(window) != 0 {
                        logging::write("activated existing window");
                        return Ok(None);
                    }
                }
                logging::write("replacing previous radio instance");
                close_previous(window)?;
            }
            let mutex = CreateMutexW(
                ptr::null(),
                1,
                wide("Local\\WifiRadio.PrivatePair.v3").as_ptr(),
            );
            let error = GetLastError();
            if mutex.is_null() {
                return Err(format!(
                    "Не удалось открыть блокировку запуска: ошибка Windows {error}"
                ));
            }
            if error != ERROR_ALREADY_EXISTS {
                recover_orphaned_port();
                return Ok(Some(Guard(mutex)));
            }
            let result = WaitForSingleObject(mutex, 0);
            if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
                recover_orphaned_port();
                return Ok(Some(Guard(mutex)));
            }
            CloseHandle(mutex);
            if Instant::now() >= deadline {
                return Err("Другой экземпляр рации не создал окно за 5 секунд. Повторите запуск. Подробности в wifiradio.log.".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}
