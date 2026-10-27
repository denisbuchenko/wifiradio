use crate::{
    engine::{Command, Phase, View},
    network,
};
use std::{
    cell::RefCell,
    mem::zeroed,
    ptr,
    rc::Rc,
    sync::{mpsc, Arc, Mutex},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::EM_SETLIMITTEXT, Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
};

const CLASS: &str = "WifiRadioConnection";
const PEER: usize = 301;
const APPLY: usize = 302;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

struct Dialog {
    commands: mpsc::Sender<Command>,
    view: Arc<Mutex<View>>,
    local: HWND,
    edit: HWND,
    status: HWND,
    font: HFONT,
}

impl Drop for Dialog {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
        }
    }
}

impl Dialog {
    unsafe fn refresh(&self) {
        let view = self.view.lock().unwrap().clone();
        let addresses = view
            .network
            .interfaces
            .iter()
            .map(|i| i.ip.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let local = if addresses.is_empty() {
            "Сеть Wi-Fi/Ethernet не найдена".into()
        } else {
            format!("IP этого компьютера:\r\n{addresses}")
        };
        set_text(self.local, &local);
        let connection = match view.phase {
            Phase::Offline => "Вторая рация пока не отвечает".into(),
            _ => format!("На связи: {}", view.peer.as_deref().unwrap_or("")),
        };
        let mode = view
            .network
            .manual_peer
            .map_or("Автоматический поиск".into(), |ip| {
                format!("Прямое подключение: {ip}")
            });
        let mut text = format!("{connection}\r\n{mode}\r\n\r\nUDP 49735 · отправлено: {} · получено: {}\r\nОт второй рации: {}",
            view.network.sent, view.network.received, view.network.authenticated);
        if let Some(error) = view.network.last_error.as_ref().or(view.error.as_ref()) {
            text.push_str(&format!("\r\n{error}"));
        }
        set_text(self.status, &text);
    }

    unsafe fn apply(&self, window: HWND) {
        let mut input = [0_u16; 64];
        let length = GetWindowTextW(self.edit, input.as_mut_ptr(), input.len() as i32);
        let result = (|| {
            let peer =
                network::validate_peer(&String::from_utf16_lossy(&input[..length as usize]))?;
            if let Some(ip) = peer {
                network::validate_local_peer(ip, &self.view.lock().unwrap().network.interfaces)?;
            }
            network::save_peer(peer)
                .map_err(|e| format!("Не удалось сохранить IP рядом с программой: {e}"))?;
            self.commands
                .send(Command::SetPeer(peer))
                .map_err(|_| "Рация уже закрыта".to_string())
        })();
        if let Err(error) = result {
            MessageBoxW(
                window,
                wide(&error).as_ptr(),
                wide("Рядом · подключение").as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
}

unsafe fn set_text(window: HWND, text: &str) {
    let mut current = vec![0; GetWindowTextLengthW(window) as usize + 1];
    GetWindowTextW(window, current.as_mut_ptr(), current.len() as i32);
    if current != wide(text) {
        SetWindowTextW(window, wide(text).as_ptr());
    }
}

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut RefCell<Dialog>;
    if message == WM_NCDESTROY && !state.is_null() {
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        KillTimer(window, 1);
        drop(Rc::from_raw(state));
        return DefWindowProcW(window, message, wparam, lparam);
    }
    if message == WM_CLOSE || (message == WM_COMMAND && wparam & 0xffff == IDCANCEL as usize) {
        DestroyWindow(window);
        return 0;
    }
    if !state.is_null() {
        // A nested native message may destroy the owner while a message box is open.
        Rc::increment_strong_count(state);
        let cell = Rc::from_raw(state);
        if let Ok(state) = cell.try_borrow() {
            match message {
                WM_TIMER => {
                    state.refresh();
                    return 0;
                }
                WM_COMMAND if [APPLY, IDOK as usize].contains(&(wparam & 0xffff)) => {
                    state.apply(window);
                    return 0;
                }
                _ => {}
            }
        };
    }
    DefWindowProcW(window, message, wparam, lparam)
}

pub unsafe fn show(owner: HWND, commands: mpsc::Sender<Command>, view: Arc<Mutex<View>>) {
    let class = wide(CLASS);
    let existing = FindWindowW(class.as_ptr(), ptr::null());
    if !existing.is_null() {
        ShowWindow(existing, SW_SHOWNORMAL);
        SetForegroundWindow(existing);
        return;
    }
    let instance = GetModuleHandleW(ptr::null());
    let mut wc: WNDCLASSW = zeroed();
    wc.lpfnWndProc = Some(procedure);
    wc.hInstance = instance;
    wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
    wc.hbrBackground = (COLOR_BTNFACE + 1) as HBRUSH;
    wc.lpszClassName = class.as_ptr();
    RegisterClassW(&wc);
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 440,
        bottom: 432,
    };
    AdjustWindowRect(&mut rect, style, 0);
    let window = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class.as_ptr(),
        wide("Рядом · подключение").as_ptr(),
        style,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        rect.right - rect.left,
        rect.bottom - rect.top,
        owner,
        ptr::null_mut(),
        instance,
        ptr::null(),
    );
    if window.is_null() {
        return;
    }
    let font = CreateFontW(
        -15,
        0,
        0,
        0,
        FW_NORMAL as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Segoe UI").as_ptr(),
    );
    let control = |class: &str, text: &str, style, id, x, y, width, height| {
        let child = CreateWindowExW(
            if class == "EDIT" { WS_EX_CLIENTEDGE } else { 0 },
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            width,
            height,
            window,
            id as HMENU,
            instance,
            ptr::null(),
        );
        SendMessageW(child, WM_SETFONT, font as usize, 1);
        child
    };
    let local = control("STATIC", "", 0, 0, 20, 18, 400, 55);
    control("STATIC", "IP второго компьютера:", 0, 0, 20, 87, 400, 24);
    let peer = view
        .lock()
        .unwrap()
        .network
        .manual_peer
        .map_or(String::new(), |ip| ip.to_string());
    let edit = control(
        "EDIT",
        &peer,
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        PEER,
        20,
        117,
        228,
        29,
    );
    SendMessageW(edit, EM_SETLIMITTEXT, 63, 0);
    control(
        "BUTTON",
        "Подключить",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        APPLY,
        260,
        116,
        160,
        31,
    );
    control(
        "STATIC",
        "Пустое поле — автоматический поиск.",
        0,
        0,
        20,
        156,
        400,
        24,
    );
    let status = control(
        "EDIT",
        "",
        ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL | WS_TABSTOP,
        303,
        20,
        194,
        400,
        142,
    );
    control("STATIC", "Если VPN блокирует локальную сеть, добавьте WifiRadio.exe в его исключения на обоих ПК. Брандмауэр должен разрешать частную сеть.", 0, 0, 20, 348, 400, 72);
    let state = Rc::new(RefCell::new(Dialog {
        commands,
        view,
        local,
        edit,
        status,
        font,
    }));
    state.borrow().refresh();
    SetWindowLongPtrW(window, GWLP_USERDATA, Rc::into_raw(state) as isize);
    SetTimer(window, 1, 500, None);
    ShowWindow(window, SW_SHOWNORMAL);
    SetForegroundWindow(window);
    SetFocus(edit);
}
