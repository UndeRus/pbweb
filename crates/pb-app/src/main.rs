//! pbweb-app entry: InkView event loop on device, plain server on host.

use pb_core::Roots;
use pb_server::Server;
#[cfg(feature = "device")]
use pb_ui::UiState;
#[cfg(feature = "device")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "device")]
mod device {
    use super::*;
    use pb_sys as iv;
    use std::ffi::CString;
    use std::sync::OnceLock;

    static STATE: OnceLock<Arc<Mutex<UiState>>> = OnceLock::new();
    /// Running HTTP server (for СТОП from the GUI thread).
    static SERVER: OnceLock<Arc<Server>> = OnceLock::new();
    const LOG_PATH: &str = "/mnt/ext1/pbweb.log";

    pub fn log_line(s: &str) {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(LOG_PATH)
        {
            let _ = writeln!(f, "{s}");
        }
    }

    fn cstring(s: &str) -> CString {
        CString::new(s).unwrap_or_default()
    }

    // Font sizes: everything big for e-ink + touch.
    const F_TITLE: i32 = 30;
    const F_BODY: i32 = 36;
    const F_STATE: i32 = 44;
    const F_URL: i32 = 60;

    fn open_fonts() -> Option<(*mut std::os::raw::c_void, *mut std::os::raw::c_void)> {
        unsafe {
            let small = iv::OpenFont(cstring("LiberationSans").as_ptr(), F_TITLE, 1);
            let body = iv::OpenFont(cstring("LiberationSans").as_ptr(), F_BODY, 1);
            if small.is_null() || body.is_null() {
                log_line("draw: OpenFont NULL");
                for f in [small, body] {
                    if !f.is_null() {
                        iv::CloseFont(f);
                    }
                }
                iv::FullUpdate();
                return None;
            }
            Some((small, body))
        }
    }

    fn open_font_big() -> *mut std::os::raw::c_void {
        unsafe { iv::OpenFont(cstring("LiberationSans").as_ptr(), F_URL, 1) }
    }

    fn open_font_state() -> *mut std::os::raw::c_void {
        unsafe { iv::OpenFont(cstring("LiberationSans").as_ptr(), F_STATE, 1) }
    }

    fn draw(state: &UiState) {
        use pb_ui::{bottom_buttons, BOTTOM_H, GAP};
        unsafe {
            iv::ClearScreen();
            let w = iv::ScreenWidth();
            let h = iv::ScreenHeight();
            let Some((f_small, f_body)) = open_fonts() else {
                return;
            };
            match state.tab {
                pb_ui::Tab::Status => draw_status(f_small, f_body, state, w, h),
                pb_ui::Tab::Files => {
                    iv::SetFont(f_body, iv::BLACK);
                    let dir = cstring(&format!("Файлы: {}", state.current_dir));
                    iv::DrawTextRect(GAP, 16, w - 2 * GAP, 56, dir.as_ptr(), iv::ALIGN_LEFT);
                    draw_file_rows(f_body, state, w);
                }
                pb_ui::Tab::Log => {
                    iv::SetFont(f_small, iv::BLACK);
                    let l = cstring(&log_text());
                    iv::DrawTextRect(
                        GAP,
                        16,
                        w - 2 * GAP,
                        h - 16 - BOTTOM_H - 16,
                        l.as_ptr(),
                        iv::ALIGN_LEFT,
                    );
                }
            }
            // bottom buttons: 3 big touch targets
            let btns = bottom_buttons(state.tab, state.server_on, w, h);
            for b in &btns {
                let primary = b.id == pb_ui::BtnId::Primary;
                if primary {
                    iv::FillArea(b.x, b.y, b.w, b.h, iv::BLACK);
                    iv::SetFont(f_body, iv::WHITE);
                } else {
                    iv::FillArea(b.x, b.y, b.w, b.h, iv::WHITE);
                    iv::DrawRect(b.x, b.y, b.w, b.h, iv::BLACK);
                    iv::DrawRect(b.x + 3, b.y + 3, b.w - 6, b.h - 6, iv::BLACK);
                    iv::SetFont(f_body, iv::BLACK);
                }
                let lb = cstring(&b.label);
                iv::DrawTextRect(
                    b.x,
                    b.y,
                    b.w,
                    b.h,
                    lb.as_ptr(),
                    iv::ALIGN_CENTER | iv::VALIGN_MIDDLE,
                );
            }
            iv::CloseFont(f_small);
            iv::CloseFont(f_body);
            iv::FullUpdate();
        }
    }

    /// Status tab: state, huge URL, wifi, steps, upload progress, message.
    fn draw_status(
        f_small: *mut std::os::raw::c_void,
        f_body: *mut std::os::raw::c_void,
        state: &UiState,
        w: i32,
        h: i32,
    ) {
        use pb_ui::{BOTTOM_H, GAP};
        unsafe {
            let mut y = 16;
            iv::SetFont(f_small, iv::BLACK);
            let title = cstring("PBWeb - передача файлов");
            iv::DrawTextRect(GAP, y, w - 2 * GAP, 44, title.as_ptr(), iv::ALIGN_LEFT);
            y += 52;
            let f_state = open_font_state();
            if !f_state.is_null() {
                iv::SetFont(f_state, iv::BLACK);
                let stxt = if state.server_on {
                    "Сервер запущен"
                } else if state.wifi_connecting {
                    "Подключение..."
                } else {
                    "Сервер остановлен"
                };
                let s = cstring(stxt);
                iv::DrawTextRect(GAP, y, w - 2 * GAP, 64, s.as_ptr(), iv::ALIGN_LEFT);
                iv::CloseFont(f_state);
            }
            y += 72;
            let f_url = open_font_big();
            if !f_url.is_null() {
                iv::SetFont(f_url, iv::BLACK);
                let url = if state.server_on {
                    state.url()
                } else {
                    "нажми СТАРТ".to_owned()
                };
                let u = cstring(&url);
                iv::DrawTextRect(GAP, y, w - 2 * GAP, 200, u.as_ptr(), iv::ALIGN_LEFT);
                iv::CloseFont(f_url);
            }
            y += 208;
            iv::SetFont(f_body, iv::BLACK);
            let wifi = if state.wifi_on {
                if state.ssid.is_empty() {
                    "WiFi: включён".to_owned()
                } else {
                    format!("WiFi: {}", state.ssid)
                }
            } else {
                "WiFi: выключен".to_owned()
            };
            let wl = cstring(&wifi);
            iv::DrawTextRect(GAP, y, w - 2 * GAP, 54, wl.as_ptr(), iv::ALIGN_LEFT);
            y += 62;
            iv::SetFont(f_small, iv::BLACK);
            let steps = cstring("1. Подключи телефон к этому WiFi\n2. Открой адрес выше в браузере\n3. СТОП - остановить, ВЫХОД - выйти");
            iv::DrawTextRect(GAP, y, w - 2 * GAP, 130, steps.as_ptr(), iv::ALIGN_LEFT);
            y += 138;
            // upload progress (file, percent, speed, bar)
            if let Some(srv) = SERVER.get() {
                let (line, pct) = srv.stats.upload_display();
                if !line.is_empty() {
                    iv::SetFont(f_body, iv::BLACK);
                    let ul = cstring(&line);
                    iv::DrawTextRect(GAP, y, w - 2 * GAP, 110, ul.as_ptr(), iv::ALIGN_LEFT);
                    y += 112;
                    if let Some(p) = pct {
                        let bw = w - 2 * GAP;
                        iv::DrawRect(GAP, y, bw, 30, iv::BLACK);
                        let fill = (bw as u32 * p as u32 / 100) as i32;
                        if fill > 4 {
                            iv::FillArea(GAP + 2, y + 2, fill - 4, 26, iv::BLACK);
                        }
                        y += 38;
                    }
                }
            }
            // message line above buttons
            let _ = y;
            iv::SetFont(f_small, iv::BLACK);
            let m = cstring(&state.message);
            iv::DrawTextRect(
                GAP,
                h - BOTTOM_H - 52,
                w - 2 * GAP,
                48,
                m.as_ptr(),
                iv::ALIGN_LEFT,
            );
        }
    }

    fn draw_file_rows(f_body: *mut std::os::raw::c_void, state: &UiState, w: i32) {
        use pb_ui::{GAP, LIST_Y0, ROW_H};
        unsafe {
            iv::SetFont(f_body, iv::BLACK);
            let listing = list_current_dir(&state.current_dir);
            let per = pb_ui::files_per_page(iv::ScreenHeight());
            for (i, e) in listing.iter().skip(state.files_page * per).take(per).enumerate() {
                let gi = state.files_page * per + i;
                let y = LIST_Y0 + (i as i32) * ROW_H;
                if gi == state.selection {
                    iv::FillArea(GAP, y, w - 2 * GAP, ROW_H - 6, iv::LGRAY);
                }
                let mark = if gi == state.selection { "> " } else { "   " };
                let ic = if e.is_dir { "[Папка] " } else { "[Файл] " };
                let line = cstring(&format!("{mark}{ic}{}", e.name));
                iv::DrawTextRect(
                    GAP + 8,
                    y,
                    w - 2 * GAP - 16,
                    ROW_H - 6,
                    line.as_ptr(),
                    iv::ALIGN_LEFT | iv::VALIGN_MIDDLE,
                );
            }
            if listing.is_empty() {
                let e = cstring("(папка пуста)");
                iv::DrawTextRect(
                    GAP,
                    LIST_Y0,
                    w - 2 * GAP,
                    ROW_H,
                    e.as_ptr(),
                    iv::ALIGN_LEFT,
                );
            }
        }
    }

    fn list_current_dir(dir: &str) -> Vec<pb_core::FileEntry> {
        let roots = Roots::device_defaults();
        pb_core::resolve_safe(&roots, dir)
            .ok()
            .and_then(|(id, full)| {
                let base = if id == "sd" {
                    roots.sdcard.clone().unwrap_or(roots.internal.clone())
                } else {
                    roots.internal.clone()
                };
                pb_core::list_dir(&base, &id, &full).ok()
            })
            .unwrap_or_default()
    }

    /// Log tab: recent HTTP requests from the running server.
    fn log_text() -> String {
        if let Some(srv) = SERVER.get() {
            let tail = srv.stats.tail();
            if tail.is_empty() {
                return "Журнал пуст.\nОткрой адрес в браузере.".to_owned();
            }
            let mut out = String::from("Запросы:\n");
            for l in tail.iter().rev().take(14) {
                out.push_str(l);
                out.push('\n');
            }
            out
        } else {
            "Сервер ещё не запускался.\nНажми СТАРТ.".to_owned()
        }
    }

    unsafe extern "C" fn handler(evt: i32, p1: i32, p2: i32) -> i32 {
        let Some(st) = STATE.get().cloned() else {
            return 0;
        };
        // Tap = POINTERDOWN/TOUCHDOWN with x/y in par1/par2 (PocketPuzzles
        // pattern: act on DOWN for e-ink responsiveness).
        if evt == iv::EVT_TOUCHDOWN || evt == iv::EVT_POINTERDOWN {
            let (x, y) = touch_xy(evt, p1, p2);
            log_line(&format!("tap evt={evt} x={x} y={y}"));
            if handle_tap(&st, x, y) {
                return 1;
            }
        }
        let Ok(mut s) = st.lock() else { return 0 };
        match evt {
            x if x == iv::EVT_INIT => {
                iv::SetPanelType(0);
                s.message = "Нажми СТАРТ".into();
                log_line("init ok");
                draw(&s);
                return 1;
            }
            x if x == iv::EVT_SHOW => {
                draw(&s);
                return 1;
            }
            x if x == iv::EVT_EXIT => {
                log_line("exit");
                return 0;
            }
            x if x == iv::EVT_NET_CONNECTED => {
                log_line("net connected event");
                s.wifi_on = true;
                refresh_net_state(&mut s);
                if let Some(ip) = crate::primary_ip() {
                    s.ip = ip;
                }
                if !s.server_on && !s.wifi_connecting {
                    s.message = "WiFi есть. Нажми СТАРТ.".into();
                }
                draw(&s);
                return 1;
            }
            x if x == iv::EVT_NET_DISCONNECTED => {
                log_line("net disconnected event");
                s.wifi_on = false;
                draw(&s);
                return 1;
            }
            x if x == iv::EVT_KEYPRESS => {
                log_line(&format!("key p1={p1}"));
                const KEY_UP: i32 = 0x11;
                const KEY_DOWN: i32 = 0x12;
                if p1 == iv::KEY_BACK {
                    if s.tab == pb_ui::Tab::Files {
                        if let Some(par) = pb_ui::UiState::parent_dir(&s.current_dir.clone()) {
                            s.current_dir = par;
                            s.selection = 0;
                            s.files_page = 0;
                            draw(&s);
                            return 1;
                        }
                    }
                    exit_app(&s);
                    return 1;
                }
                if p1 == 0x17 /* MENU */ || p1 == iv::KEY_OK {
                    drop(s);
                    press_start(&st);
                    return 1;
                }
                if p1 == iv::KEY_NEXT {
                    s.next_tab();
                    draw(&s);
                    return 1;
                }
                if p1 == iv::KEY_PREV {
                    s.prev_tab();
                    draw(&s);
                    return 1;
                }
                if p1 == KEY_UP || p1 == KEY_DOWN {
                    if s.tab != pb_ui::Tab::Files {
                        s.tab = pb_ui::Tab::Files;
                    }
                    let per = pb_ui::files_per_page(iv::ScreenHeight());
                    let len = file_count(&s.current_dir);
                    s.move_sel(len, if p1 == KEY_DOWN { 1 } else { -1 }, per);
                    draw(&s);
                    return 1;
                }
            }
            _ => {}
        }
        0
    }

    /// Tap coordinates: like PocketPuzzles, use par1/par2 directly for both
    /// POINTER* and TOUCH* events (FW delivers x/y there).
    fn touch_xy(_evt: i32, p1: i32, p2: i32) -> (i32, i32) {
        (p1, p2)
    }

    fn file_count(dir: &str) -> usize {
        let roots = Roots::device_defaults();
        pb_core::resolve_safe(&roots, dir)
            .ok()
            .and_then(|(id, full)| {
                let base = if id == "sd" {
                    roots.sdcard.clone().unwrap_or(roots.internal.clone())
                } else {
                    roots.internal.clone()
                };
                pb_core::list_dir(&base, &id, &full).ok()
            })
            .map(|v| v.len())
            .unwrap_or(0)
    }

    fn descend(dir: &str, sel: usize) -> Option<String> {
        let roots = Roots::device_defaults();
        let (id, full) = pb_core::resolve_safe(&roots, dir).ok()?;
        let base = if id == "sd" {
            roots.sdcard.clone().unwrap_or(roots.internal.clone())
        } else {
            roots.internal.clone()
        };
        let items = pb_core::list_dir(&base, &id, &full).ok()?;
        let e = items.get(sel)?;
        if e.is_dir {
            Some(e.path.clone())
        } else {
            None
        }
    }

    /// Our own handler fn, saved so the worker thread can wake the GUI
    /// via SendEvent (no GetEventHandler needed).
    static HANDLER_FN: OnceLock<unsafe extern "C" fn(i32, i32, i32) -> i32> = OnceLock::new();

    /// Ask the GUI thread to redraw (safe to call from the worker thread).
    fn request_redraw() {
        if let Some(f) = HANDLER_FN.get() {
            unsafe {
                iv::SendEvent(Some(*f), iv::EVT_SHOW, 0, 0);
            }
        }
    }

    fn set_msg(st: &Arc<Mutex<UiState>>, msg: &str) {
        if let Ok(mut s) = st.lock() {
            s.message = msg.into();
        }
        request_redraw();
    }

    /// Refresh wifi_on/ssid from NetInfo (best effort, never panics).
    fn refresh_net_state(s: &mut UiState) {
        if let Some(n) = iv::netinfo_full() {
            s.wifi_on = n.connected != 0;
            s.ssid = unsafe { iv::take_c_string(n.name.as_ptr().cast_mut()) }.unwrap_or_default();
        } else {
            s.wifi_on = unsafe { iv::QueryNetwork() != 0 };
        }
    }

    // Async-connect callback: only logged, polling NetInfo is the truth.
    extern "C" fn net_cb(status: i32) -> i32 {
        log_line(&format!("NetConnectAsync cb status={status}"));
        0
    }

    /// Tap routing: bottom buttons -> file rows. True if handled.
    /// (No top bar anymore — tabs switch via the ЭКРАН button / PREV/NEXT.)
    fn handle_tap(st: &Arc<Mutex<UiState>>, x: i32, y: i32) -> bool {
        use pb_ui::{bottom_buttons, hit_button, row_at};
        unsafe {
            let w = iv::ScreenWidth();
            let h = iv::ScreenHeight();
            let Ok(s) = st.lock() else { return false };
            let tab = s.tab;
            let server_on = s.server_on;
            // 1) bottom buttons
            let btns = bottom_buttons(tab, server_on, w, h);
            if let Some(id) = hit_button(&btns, x, y) {
                drop(s);
                match id {
                    pb_ui::BtnId::Primary => {
                        if server_on {
                            stop_server(st);
                        } else if tab == pb_ui::Tab::Files {
                            if let Ok(mut s) = st.lock() {
                                if let Some(par) =
                                    pb_ui::UiState::parent_dir(&s.current_dir.clone())
                                {
                                    s.current_dir = par;
                                    s.selection = 0;
                                    s.files_page = 0;
                                } else {
                                    s.tab = pb_ui::Tab::Status;
                                }
                                draw(&s);
                            }
                        } else {
                            press_start(st);
                        }
                    }
                    pb_ui::BtnId::Tabs => {
                        if let Ok(mut s) = st.lock() {
                            s.next_tab();
                            draw(&s);
                        }
                    }
                    pb_ui::BtnId::Exit => {
                        if let Ok(s) = st.lock() {
                            exit_app(&s);
                        }
                    }
                }
                return true;
            }
            // 2) file rows
            if tab == pb_ui::Tab::Files {
                let per = pb_ui::files_per_page(h);
                let total = file_count(&s.current_dir);
                if let Some(idx) = row_at(y, s.files_page * per, per, total) {
                    drop(s);
                    if let Ok(mut s) = st.lock() {
                        s.selection = idx;
                        if let Some(next) = descend(&s.current_dir.clone(), idx) {
                            s.current_dir = next;
                            s.selection = 0;
                            s.files_page = 0;
                        }
                        draw(&s);
                    }
                    return true;
                }
            }
            false
        }
    }

    /// STOP button: real shutdown (unblocks the accept loop, closes socket).
    /// WiFi stays on so START works instantly afterwards.
    fn stop_server(st: &Arc<Mutex<UiState>>) {
        log_line("stop pressed");
        if let Some(srv) = SERVER.get() {
            srv.shutdown();
        }
        if let Ok(mut s) = st.lock() {
            s.server_on = false;
            s.wifi_connecting = false;
            s.message = "Сервер остановлен".into();
            draw(&s);
        }
    }

    /// START button / MENU / OK: wifi prompt, then async connect in worker.
    /// Runs on the GUI thread (DialogSynchro blocks here, never in worker).
    fn press_start(st: &Arc<Mutex<UiState>>) {
        let (server_on, connecting) = match st.lock() {
            Ok(s) => (s.server_on, s.wifi_connecting),
            Err(_) => return,
        };
        if server_on {
            set_msg(st, "Сервер уже запущен");
            return;
        }
        if connecting {
            set_msg(st, "Уже подключаюсь, подожди...");
            return;
        }
        let ans = unsafe {
            iv::DialogSynchro(
                iv::ICON_QUESTION,
                cstring("WiFi").as_ptr(),
                cstring("Включить WiFi и запустить сервер файлов?").as_ptr(),
                cstring("Да").as_ptr(),
                cstring("Нет").as_ptr(),
                std::ptr::null(),
            )
        };
        log_line(&format!("wifi prompt answer={ans}"));
        if ans != 1 {
            return;
        }
        if let Ok(mut s) = st.lock() {
            s.wifi_connecting = true;
            s.message = "Запуск...".into();
            draw(&s);
        }
        wifi_and_serve(st.clone());
    }

    fn exit_app(_s: &UiState) {
        log_line("exit: rescan + CloseApp");
        crate::library_rescan();
        unsafe {
            iv::iv_sync();
            iv::CloseApp();
        }
    }

    /// Online = NetInfo()->connected ONLY. QueryNetwork() has undocumented
    /// semantics (nonzero even when offline) and must not gate connecting.
    fn online() -> bool {
        iv::netinfo_connected().unwrap_or(false)
    }

    /// One-line dump of NetInfo + interfaces for the log.
    fn net_dump() -> String {
        let mut out = String::new();
        match iv::netinfo_full() {
            Some(n) => {
                let name =
                    unsafe { iv::take_c_string(n.name.as_ptr().cast_mut()) }.unwrap_or_default();
                let dev =
                    unsafe { iv::take_c_string(n.device.as_ptr().cast_mut()) }.unwrap_or_default();
                let prefix =
                    unsafe { iv::take_c_string(n.prefix.as_ptr().cast_mut()) }.unwrap_or_default();
                out.push_str(&format!(
                    "netinfo connected={} name={name} device={dev} prefix={prefix}",
                    n.connected
                ));
            }
            None => out.push_str("netinfo MISSING"),
        }
        #[cfg(target_os = "linux")]
        {
            // enumerate interfaces without getifaddrs plumbing: /sys/class/net
            if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
                let mut ifs: Vec<String> = rd
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                ifs.sort();
                out.push_str(&format!(" ifs={}", ifs.join(",")));
            }
        }
        out
    }

    /// Non-blocking wifi flow in a worker thread. NEVER calls blocking
    /// NetConnect*: silent attempt, then NetConnectAsync + poll NetInfo
    /// with a 45s cap. UI updates go through shared state + SendEvent.
    fn wifi_and_serve(st: Arc<Mutex<UiState>>) {
        let st2 = st.clone();
        let spawn = std::thread::Builder::new()
            .name("pbweb-wifi".into())
            .spawn(move || {
                iv::postpone_poweroff();
                log_line(&format!("worker start: {}", net_dump()));
                if !online() {
                    // 1) silent attempt: no dialogs, quick
                    set_msg(&st2, "Подключение к WiFi...");
                    match iv::net_connect_silent() {
                        Some(rc) => log_line(&format!("worker: silent rc={rc}")),
                        None => log_line("worker: NetConnectSilent missing"),
                    }
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    log_line(&format!("worker after silent: {}", net_dump()));
                }
                if !online() {
                    // 2) async attempt: system dialog, non-blocking
                    log_line("worker: NetConnectAsync...");
                    match iv::net_connect_async(net_cb) {
                        Some(rc) => log_line(&format!("worker: async started rc={rc}")),
                        None => {
                            log_line("worker: NetConnectAsync missing!");
                            set_msg(&st2, "Нет WiFi. Подключись в настройках и нажми СТАРТ.");
                            finish_connecting(&st2);
                            return;
                        }
                    }
                    // poll up to ~45s
                    let mut waited = 0;
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        waited += 1;
                        if online() {
                            break;
                        }
                        if waited % 4 == 0 {
                            set_msg(&st2, &format!("Подключение к WiFi... {}с", waited / 2));
                        }
                        if waited >= 90 {
                            log_line(&format!("worker: wifi timeout: {}", net_dump()));
                            set_msg(
                                &st2,
                                "Нет WiFi. Подключись в настройках и нажми СТАРТ.",
                            );
                            finish_connecting(&st2);
                            return;
                        }
                    }
                }
                // online: publish state, bind, serve (blocking until STOP/exit)
                let ip = crate::primary_ip().unwrap_or_else(|| "?".into());
                let roots = Roots::device_defaults();
                let srv = Arc::new(Server::new(roots));
                // progress hook: redraw the device screen as bytes arrive
                srv.stats
                    .set_hook(Arc::new(|| request_redraw()));
                let _ = SERVER.set(srv.clone());
                match srv.bind_shared(8080) {
                    Ok((http, port)) => {
                        log_line(&format!("worker: listening {ip}:{port}"));
                        if let Ok(mut s) = st2.lock() {
                            refresh_net_state(&mut s);
                            s.wifi_on = true;
                            s.server_on = true;
                            s.wifi_connecting = false;
                            s.ip = ip;
                            s.port = port;
                            s.message = "Сервер запущен".into();
                        }
                        request_redraw();
                        srv.run_shared(&http); // blocks until shutdown()/exit
                        log_line("worker: serve loop ended");
                        // stopped via СТОП (or socket died): back to idle
                        if let Ok(mut s) = st2.lock() {
                            s.server_on = false;
                            s.wifi_connecting = false;
                            if s.message == "Сервер запущен" {
                                s.message = "Сервер остановлен".into();
                            }
                        }
                        request_redraw();
                    }
                    Err(e) => {
                        log_line(&format!("worker: bind failed: {e}"));
                        set_msg(&st2, &format!("Ошибка запуска: {e}"));
                    }
                }
                finish_connecting(&st2);
            });
        if let Err(e) = spawn {
            log_line(&format!("thread spawn failed: {e}"));
            set_msg(&st, &format!("Ошибка потока: {e}"));
            finish_connecting(&st);
        }
    }

    fn finish_connecting(st: &Arc<Mutex<UiState>>) {
        if let Ok(mut s) = st.lock() {
            s.wifi_connecting = false;
        }
        request_redraw();
    }

    pub fn run() {
        // Panic hook runs even with panic=abort: leave a trace in the log.
        std::panic::set_hook(Box::new(|info| {
            log_line(&format!("PANIC: {info}"));
        }));
        log_line("pbweb 0.2.0 starting");
        let _ = HANDLER_FN.set(handler);
        let state = Arc::new(Mutex::new(UiState::default()));
        let _ = STATE.set(state);
        unsafe {
            iv::InkViewMain(Some(handler));
        }
    }
}

fn primary_ip() -> Option<String> {
    // Real interface address first (getifaddrs on Linux/device).
    #[cfg(target_os = "linux")]
    if let Some(ip) = pb_sys::lan_ip() {
        return Some(ip);
    }
    // Fallback: UDP-route trick (works on Windows host; on Linux
    // getsockname usually stays 0.0.0.0, hence the getifaddrs above).
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.1.1:80").ok()?;
    let ip = s.local_addr().ok()?.ip().to_string();
    (!ip.starts_with("0.") && ip != "127.0.0.1").then_some(ip)
}

/// Best-effort library rescan after file changes.
/// Strategy (FW-agnostic): fs sync + poke explorer db mtime + EVT_FSCHANGED.
/// Real rescan happens when user opens Library; this just nudges it.
fn library_rescan() {
    #[cfg(feature = "device")]
    unsafe {
        pb_sys::iv_sync();
        // touch explorer db so mtime changes
        for db in [
            "/mnt/ext1/system/explorer-3/explorer-3.db",
            "/mnt/ext1/system/explorer/explorer.db",
        ] {
            if std::path::Path::new(db).exists() {
                let now = std::time::SystemTime::now();
                let _ = filetime_touch(db, now);
            }
        }
    }
}

#[cfg(feature = "device")]
fn filetime_touch(path: &str, _now: std::time::SystemTime) -> std::io::Result<()> {
    // utimensat without libc: open+write+truncate dance via std
    use std::fs::OpenOptions;
    let f = OpenOptions::new().write(true).open(path)?;
    let len = f.metadata()?.len();
    f.set_len(len)?;
    Ok(())
}

fn main() {
    #[cfg(feature = "device")]
    device::run();

    #[cfg(not(feature = "device"))]
    {
        // Host dev mode: serve current dir on 8080 for UI testing.
        let roots = Roots {
            internal: std::env::current_dir().unwrap(),
            sdcard: None,
        };
        let srv = Server::new(roots);
        println!("host dev server: see http://127.0.0.1:8080");
        let _ = srv.serve(8080);
    }
}
