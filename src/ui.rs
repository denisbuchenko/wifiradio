use crate::{
    engine::{Command, Handle, Phase, View},
    startup,
};
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const TRAY_MESSAGE: u32 = WM_APP + 1;
const PRESS_MESSAGE: u32 = WM_APP + 2;
const RELEASE_MESSAGE: u32 = WM_APP + 3;
const QUIT_MESSAGE: u32 = WM_APP + 4;
const TALK_ID: usize = 100;
const MENU_ID: usize = 101;
const OPEN: u32 = 200;
const AUTOSTART: u32 = 201;
const HELP: u32 = 202;
const EXIT: u32 = 203;
const BG: u32 = rgb(21, 25, 29);
const FG: u32 = rgb(243, 242, 235);
const MUTED: u32 = rgb(147, 153, 157);
const YELLOW: u32 = rgb(246, 203, 84);
const GREY: u32 = rgb(57, 63, 68);
const INK: u32 = rgb(33, 32, 26);

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct App {
    engine: Handle,
    window: HWND,
    talk: HWND,
    menu: HWND,
    snapshot: View,
    holding: bool,
    icon: HICON,
    tray_added: bool,
    taskbar_message: u32,
    font: HFONT,
    small: HFONT,
    title: HFONT,
    label: HFONT,
}

impl App {
    unsafe fn press(&mut self) {
        if self.holding || self.snapshot.peer.is_none() || self.snapshot.phase == Phase::Listening {
            return;
        }
        self.holding = true;
        let _ = self.engine.commands.send(Command::Begin);
        InvalidateRect(self.talk, ptr::null(), 0);
    }

    unsafe fn release(&mut self) {
        if !self.holding {
            return;
        }
        self.holding = false;
        let _ = self.engine.commands.send(Command::End);
        InvalidateRect(self.talk, ptr::null(), 0);
    }

    unsafe fn show(&self) {
        ShowWindow(self.window, SW_SHOWNORMAL);
        SetForegroundWindow(self.window);
        SetFocus(self.talk);
    }

    unsafe fn tray(&mut self, add: bool) {
        let new_icon = radio_icon(self.snapshot.peer.is_some());
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.window;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = new_icon;
        let tip = wide(match self.snapshot.phase {
            Phase::Offline => "Рядом · ждём вторую рацию",
            Phase::Ready => "Рядом · можно говорить",
            Phase::Preparing => "Рядом · три тона перед разговором",
            Phase::Talking => "Рядом · ваш микрофон включён",
            Phase::Listening => "Рядом · вам говорят",
        });
        data.szTip[..tip.len()].copy_from_slice(&tip);
        let result = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data);
        if result != 0 {
            self.tray_added = true;
        }
        if !self.icon.is_null() {
            DestroyIcon(self.icon);
        }
        self.icon = new_icon;
    }

    unsafe fn refresh(&mut self) {
        let snapshot = self.engine.view.lock().unwrap().clone();
        if snapshot == self.snapshot {
            return;
        }
        let new_error = snapshot
            .error
            .clone()
            .filter(|error| Some(error) != self.snapshot.error.as_ref());
        self.snapshot = snapshot;
        if self.snapshot.peer.is_none() || self.snapshot.phase == Phase::Listening {
            self.release();
        }
        EnableWindow(
            self.talk,
            i32::from(self.snapshot.peer.is_some() && self.snapshot.phase != Phase::Listening),
        );
        let label = match self.snapshot.phase {
            Phase::Offline => "Ждём вторую рацию",
            Phase::Preparing => "Подождите три тона",
            Phase::Talking => "Говорите — микрофон включён",
            Phase::Listening => "Вам говорят",
            Phase::Ready => "Держать и говорить",
        };
        SetWindowTextW(self.talk, wide(label).as_ptr());
        self.tray(false);
        InvalidateRect(self.window, ptr::null(), 0);
        InvalidateRect(self.talk, ptr::null(), 0);
        if let Some(error) = new_error {
            self.release();
            MessageBoxW(
                self.window,
                wide(&error).as_ptr(),
                wide("Рядом · не удалось подключиться").as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    unsafe fn popup(&mut self, at_cursor: bool) {
        self.release();
        let menu = CreatePopupMenu();
        AppendMenuW(
            menu,
            MF_STRING,
            OPEN as usize,
            wide("Открыть рацию").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        AppendMenuW(
            menu,
            MF_STRING | if startup::enabled() { MF_CHECKED } else { 0 },
            AUTOSTART as usize,
            wide("Запускать вместе с Windows").as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING,
            HELP as usize,
            wide("Как подключиться").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        AppendMenuW(menu, MF_STRING, EXIT as usize, wide("Выход").as_ptr());
        let mut point: POINT = zeroed();
        if at_cursor {
            GetCursorPos(&mut point);
        } else {
            let mut rect: RECT = zeroed();
            GetWindowRect(self.menu, &mut rect);
            point.x = rect.right;
            point.y = rect.bottom;
        }
        SetForegroundWindow(self.window);
        let action = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTALIGN | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            self.window,
            ptr::null(),
        );
        PostMessageW(self.window, WM_NULL, 0, 0);
        DestroyMenu(menu);
        match action as u32 {
            OPEN => self.show(),
            AUTOSTART => {
                if let Err(error) = startup::set(!startup::enabled()) {
                    MessageBoxW(
                        self.window,
                        wide(&error).as_ptr(),
                        wide("Рядом").as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            HELP => {
                let peer = self.snapshot.peer.as_deref().unwrap_or("пока не найден");
                let mut text = format!(
                    "1. Скопируйте этот же WifiRadio.exe на второй компьютер.\n\n\
                    2. Подключите оба компьютера к одному роутеру. Wi-Fi и кабель подходят.\n\n\
                    3. Разрешите программе доступ в частную сеть в окне брандмауэра Windows.\n\n\
                    4. Когда кнопка станет жёлтой, держите её или пробел и говорите. После трёх тонов дождитесь надписи «Говорите». Отпустите, чтобы услышать ответ.\n\n\
                    Крестик прячет окно в трей. Выход полностью закрывает рацию.\n\n\
                    Вторая рация: {peer}\nПорт: UDP 49735. Интернет не нужен.\n\n\
                    Две копии этого файла используют общий приватный ключ. Другие сборки автоматически к ним не подключаются."
                );
                if let Some(error) = &self.snapshot.error {
                    text.push_str(&format!("\n\nОшибка: {error}"));
                }
                MessageBoxW(
                    self.window,
                    wide(&text).as_ptr(),
                    wide("Рядом · как подключиться").as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            EXIT => {
                PostMessageW(self.window, QUIT_MESSAGE, 0, 0);
            }
            _ => {}
        }
    }
}

unsafe fn app(window: HWND) -> Option<&'static RefCell<App>> {
    (GetWindowLongPtrW(window, GWLP_USERDATA) as *const RefCell<App>).as_ref()
}

unsafe extern "system" fn talk_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let parent = GetParent(window);
    match message {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            SetFocus(window);
            SetCapture(window);
            SendMessageW(parent, PRESS_MESSAGE, 0, 0);
            return 0;
        }
        WM_LBUTTONUP => {
            SendMessageW(parent, RELEASE_MESSAGE, 0, 0);
            if GetCapture() == window {
                ReleaseCapture();
            }
            return 0;
        }
        WM_KEYDOWN if wparam == VK_SPACE as usize => {
            if lparam & (1 << 30) == 0 {
                SendMessageW(parent, PRESS_MESSAGE, 0, 0);
            }
            return 0;
        }
        WM_KEYUP if wparam == VK_SPACE as usize => {
            SendMessageW(parent, RELEASE_MESSAGE, 0, 0);
            return 0;
        }
        WM_CAPTURECHANGED | WM_CANCELMODE | WM_KILLFOCUS => {
            SendMessageW(parent, RELEASE_MESSAGE, 0, 0);
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(window, Some(talk_proc), 1);
        }
        _ => {}
    }
    DefSubclassProc(window, message, wparam, lparam)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == QUIT_MESSAGE {
        DestroyWindow(window);
        return 0;
    }
    let Some(cell) = app(window) else {
        return DefWindowProcW(window, message, wparam, lparam);
    };
    // Native dialogs and controls can synchronously re-enter the window procedure.
    // Ignore nested app work instead of aliasing mutable Rust references.
    let Ok(mut app) = cell.try_borrow_mut() else {
        return DefWindowProcW(window, message, wparam, lparam);
    };
    if message == app.taskbar_message {
        app.tray(true);
        return 0;
    }
    match message {
        WM_TIMER => {
            app.refresh();
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(&app);
            0
        }
        WM_DRAWITEM => {
            let item = &*(lparam as *const DRAWITEMSTRUCT);
            draw_button(&app, item);
            1
        }
        WM_COMMAND if (wparam & 0xffff) == MENU_ID => {
            app.popup(false);
            0
        }
        PRESS_MESSAGE => {
            app.press();
            0
        }
        RELEASE_MESSAGE => {
            app.release();
            0
        }
        WM_ACTIVATE if wparam & 0xffff == WA_INACTIVE as usize => {
            app.release();
            0
        }
        WM_CANCELMODE => {
            app.release();
            0
        }
        WM_KEYDOWN if wparam == VK_ESCAPE as usize => {
            app.release();
            ShowWindow(window, SW_HIDE);
            0
        }
        WM_CLOSE => {
            app.release();
            ShowWindow(window, SW_HIDE);
            0
        }
        WM_SIZE if wparam == SIZE_MINIMIZED as usize => {
            app.release();
            ShowWindow(window, SW_HIDE);
            0
        }
        TRAY_MESSAGE => {
            match lparam as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => app.show(),
                WM_RBUTTONUP => app.popup(true),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            app.release();
            let _ = app.engine.commands.send(Command::Shutdown);
            let mut data: NOTIFYICONDATAW = zeroed();
            data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = window;
            data.uID = 1;
            Shell_NotifyIconW(NIM_DELETE, &data);
            KillTimer(window, 1);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn fill(dc: HDC, rect: &RECT, color: u32) {
    let brush = CreateSolidBrush(color);
    FillRect(dc, rect, brush);
    DeleteObject(brush);
}

unsafe fn ellipse(dc: HDC, rect: RECT, color: u32) {
    let brush = CreateSolidBrush(color);
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    Ellipse(dc, rect.left, rect.top, rect.right, rect.bottom);
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(brush);
}

unsafe fn text(dc: HDC, content: &str, rect: RECT, font: HFONT, color: u32, flags: u32) {
    let old = SelectObject(dc, font);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    let mut rect = rect;
    let string = wide(content);
    DrawTextW(
        dc,
        string.as_ptr(),
        string.len() as i32 - 1,
        &mut rect,
        flags | DT_NOPREFIX,
    );
    SelectObject(dc, old);
}

unsafe fn paint(app: &App) {
    let mut ps: PAINTSTRUCT = zeroed();
    let output = BeginPaint(app.window, &mut ps);
    let dc = CreateCompatibleDC(output);
    let bitmap = CreateCompatibleBitmap(output, 360, 454);
    let old = SelectObject(dc, bitmap);
    fill(
        dc,
        &RECT {
            left: 0,
            top: 0,
            right: 360,
            bottom: 454,
        },
        BG,
    );
    text(
        dc,
        "РАЦИЯ ДЛЯ ДВОИХ",
        RECT {
            left: 28,
            top: 24,
            right: 290,
            bottom: 44,
        },
        app.small,
        MUTED,
        DT_LEFT,
    );
    text(
        dc,
        "Рядом",
        RECT {
            left: 26,
            top: 44,
            right: 290,
            bottom: 88,
        },
        app.title,
        FG,
        DT_LEFT,
    );
    let (status, color) = match app.snapshot.phase {
        Phase::Offline => ("Ждём вторую рацию", MUTED),
        Phase::Ready => ("Вторая рация в сети", YELLOW),
        Phase::Preparing => ("Готовимся к разговору", YELLOW),
        Phase::Talking => ("Ваш микрофон включён", YELLOW),
        Phase::Listening => ("Вам говорят", YELLOW),
    };
    ellipse(
        dc,
        RECT {
            left: 29,
            top: 100,
            right: 36,
            bottom: 107,
        },
        color,
    );
    text(
        dc,
        status,
        RECT {
            left: 44,
            top: 92,
            right: 335,
            bottom: 117,
        },
        app.font,
        color,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
    let hint = if app.snapshot.error.is_some() {
        "Не удалось подключиться.\nПодробности в меню ⋯"
    } else {
        match app.snapshot.phase {
            Phase::Offline => "Запустите эту же программу\nна втором компьютере.",
            Phase::Preparing => "Дождитесь трёх коротких тонов.\nПока держите кнопку.",
            Phase::Talking => "Говорите.\nОтпустите кнопку, чтобы слушать.",
            Phase::Listening => "Слушайте.\nСкоро можно будет ответить.",
            Phase::Ready => "Из комнаты в комнату.\nПросто держите кнопку и говорите.",
        }
    };
    text(
        dc,
        hint,
        RECT {
            left: 22,
            top: 367,
            right: 338,
            bottom: 414,
        },
        app.font,
        MUTED,
        DT_CENTER,
    );
    text(
        dc,
        "Только ваша локальная сеть",
        RECT {
            left: 20,
            top: 429,
            right: 340,
            bottom: 449,
        },
        app.small,
        rgb(107, 115, 120),
        DT_CENTER,
    );
    BitBlt(output, 0, 0, 360, 454, dc, 0, 0, SRCCOPY);
    SelectObject(dc, old);
    DeleteObject(bitmap);
    DeleteDC(dc);
    EndPaint(app.window, &ps);
}

unsafe fn draw_button(app: &App, item: &DRAWITEMSTRUCT) {
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, BG);
    if item.CtlID == MENU_ID as u32 {
        let color = if item.itemState & ODS_SELECTED != 0 {
            YELLOW
        } else {
            MUTED
        };
        for x in [12, 20, 28] {
            ellipse(
                dc,
                RECT {
                    left: x,
                    top: 17,
                    right: x + 4,
                    bottom: 21,
                },
                color,
            );
        }
        return;
    }
    let connected = app.snapshot.peer.is_some();
    let pressed = app.holding || app.snapshot.phase == Phase::Talking;
    let color = if !connected {
        GREY
    } else if pressed {
        rgb(232, 178, 49)
    } else {
        YELLOW
    };
    let ink = if connected { INK } else { rgb(166, 173, 176) };
    let margin = if pressed { 10 } else { 6 };
    ellipse(
        dc,
        RECT {
            left: 0,
            top: 0,
            right: 240,
            bottom: 240,
        },
        if connected {
            rgb(63, 57, 36)
        } else {
            rgb(36, 42, 46)
        },
    );
    ellipse(
        dc,
        RECT {
            left: margin,
            top: margin,
            right: 240 - margin,
            bottom: 240 - margin,
        },
        color,
    );
    let pen = CreatePen(PS_SOLID, 4, ink);
    let old_pen = SelectObject(dc, pen);
    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
    RoundRect(dc, 108, 56, 132, 97, 18, 18);
    Arc(dc, 99, 67, 141, 109, 99, 88, 141, 88);
    MoveToEx(dc, 120, 109, ptr::null_mut());
    LineTo(dc, 120, 121);
    MoveToEx(dc, 109, 122, ptr::null_mut());
    LineTo(dc, 131, 122);
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    DeleteObject(pen);
    let label = match app.snapshot.phase {
        Phase::Offline => "Пока никого",
        Phase::Preparing => "Три тона…",
        Phase::Talking => "Говорите",
        Phase::Listening => "Слушаем",
        Phase::Ready => "Держать и говорить",
    };
    text(
        dc,
        label,
        RECT {
            left: 12,
            top: 142,
            right: 228,
            bottom: 172,
        },
        app.label,
        ink,
        DT_CENTER | DT_SINGLELINE,
    );
    let sub = if connected {
        "или удерживайте пробел"
    } else {
        "ждём подключения"
    };
    text(
        dc,
        sub,
        RECT {
            left: 12,
            top: 176,
            right: 228,
            bottom: 198,
        },
        app.small,
        ink,
        DT_CENTER | DT_SINGLELINE,
    );
    if item.itemState & ODS_FOCUS != 0 && connected {
        // The bright ring also provides a visible keyboard focus indicator.
        let pen = CreatePen(PS_DOT, 1, ink);
        let old_pen = SelectObject(dc, pen);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Ellipse(dc, 15, 15, 225, 225);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        DeleteObject(pen);
    }
}

unsafe fn font(height: i32, weight: u32) -> HFONT {
    CreateFontW(
        -height,
        0,
        0,
        0,
        weight as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Segoe UI").as_ptr(),
    )
}

unsafe fn radio_icon(connected: bool) -> HICON {
    let mut pixels = [0_u8; 32 * 32 * 4];
    for y in 0_i32..32 {
        for x in 0_i32..32 {
            let circle = (x - 16).pow(2) + (y - 16).pow(2) <= 225;
            if !circle {
                continue;
            }
            let glyph = ((13..=18).contains(&x) && (7..=18).contains(&y))
                || ((10..=21).contains(&x) && (20..=22).contains(&y))
                || ((15..=16).contains(&x) && (18..=26).contains(&y))
                || ((11..=20).contains(&x) && y == 26);
            let color = if glyph {
                INK
            } else if connected {
                YELLOW
            } else {
                rgb(147, 153, 157)
            };
            let index = (y * 32 + x) as usize * 4;
            pixels[index] = (color >> 16) as u8;
            pixels[index + 1] = (color >> 8) as u8;
            pixels[index + 2] = color as u8;
            pixels[index + 3] = 255;
        }
    }
    CreateIcon(
        GetModuleHandleW(ptr::null()),
        32,
        32,
        1,
        32,
        [0_u8; 128].as_ptr(),
        pixels.as_ptr(),
    )
}

pub fn run(background: bool, no_autostart: bool) {
    unsafe {
        // System scaling keeps the fixed-size interface readable on high-DPI PCs.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED);
        let mutex = CreateMutexW(
            ptr::null(),
            0,
            wide("Local\\WifiRadio.PrivatePair.v1").as_ptr(),
        );
        if mutex.is_null() {
            return;
        }
        let class = wide("WifiRadioWindow");
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let window = FindWindowW(class.as_ptr(), ptr::null());
            if !window.is_null() {
                ShowWindow(window, SW_SHOWNORMAL);
                SetForegroundWindow(window);
            }
            CloseHandle(mutex);
            return;
        }
        let instance = GetModuleHandleW(ptr::null());
        let mut wc: WNDCLASSW = zeroed();
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpfnWndProc = Some(window_proc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        wc.lpszClassName = class.as_ptr();
        wc.hIcon = radio_icon(true);
        if RegisterClassW(&wc) == 0 {
            CloseHandle(mutex);
            return;
        }
        let mut app = Box::new(App {
            engine: Handle::start(),
            window: ptr::null_mut(),
            talk: ptr::null_mut(),
            menu: ptr::null_mut(),
            snapshot: View::default(),
            holding: false,
            icon: ptr::null_mut(),
            tray_added: false,
            taskbar_message: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            font: font(15, FW_NORMAL),
            small: font(12, FW_NORMAL),
            title: font(34, FW_SEMIBOLD),
            label: font(18, FW_SEMIBOLD),
        });
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 360,
            bottom: 454,
        };
        AdjustWindowRect(&mut rect, style, 0);
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Рядом · Wi-Fi рация").as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if window.is_null() {
            CloseHandle(mutex);
            return;
        }
        app.window = window;
        // Attach after controls are created, then release all borrows before the
        // message loop. RefCell also guards modal native-message reentrancy.
        app.talk = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Ждём вторую рацию").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32 | WS_DISABLED,
            60,
            121,
            240,
            240,
            window,
            TALK_ID as HMENU,
            instance,
            ptr::null(),
        );
        SetWindowSubclass(app.talk, Some(talk_proc), 1, 0);
        app.menu = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Меню").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            292,
            26,
            42,
            38,
            window,
            MENU_ID as HMENU,
            instance,
            ptr::null(),
        );
        let app = Box::new(RefCell::new(*app));
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            (&*app as *const RefCell<App>) as isize,
        );
        let mut state = app.borrow_mut();
        state.tray(true);
        SetTimer(window, 1, 60, None);
        if !no_autostart {
            if let Err(error) = startup::initialize() {
                MessageBoxW(
                    window,
                    wide(&error).as_ptr(),
                    wide("Рядом · автозапуск").as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
        }
        if !background {
            state.show();
        }
        drop(state);
        InvalidateRect(window, ptr::null(), 0);
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        let app = app.borrow();
        DestroyIcon(app.icon);
        DestroyIcon(wc.hIcon);
        for font in [app.font, app.small, app.title, app.label] {
            DeleteObject(font);
        }
        CloseHandle(mutex);
    }
}
