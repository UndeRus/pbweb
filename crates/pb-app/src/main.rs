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

    fn draw(state: &UiState) {
        use pb_ui::{bottom_buttons, BOTTOM_H, GAP, HEADER_H};
        unsafe {
            iv::ClearScreen();
            let w = iv::ScreenWidth();
            let h = iv::ScreenHeight();
            // header
            iv::FillArea(0, 0, w, HEADER_H, iv::BLACK);
            let f_title = iv::OpenFont(cstring("LiberationSans").as_ptr(), 30, 1);
            if f_title.is_null() {
                log_line("draw: OpenFont title NULL");
                iv::FullUpdate();
                return;
            }
            iv::SetFont(f_title, iv::WHITE);
            let tabname = match state.tab {
                pb_ui::Tab::Status => "STATUS",
                pb_ui::Tab::Files => "FILES",
                pb_ui::Tab::Log => "LOG",
            };
            let t = cstring(&format!("PBWeb  {tabname}"));
            iv::DrawString(GAP, HEADER_H - 24, t.as_ptr());
            iv::CloseFont(f_title);

            let f_body = iv::OpenFont(cstring("LiberationSans").as_ptr(), 32, 1);
            let f_big = iv::OpenFont(cstring("LiberationSans").as_ptr(), 46, 1);
            if f_body.is_null() || f_big.is_null() {
                log_line("draw: OpenFont body/big NULL");
                if !f_body.is_null() {
                    iv::CloseFont(f_body);
                }
                if !f_big.is_null() {
                    iv::CloseFont(f_big);
                }
                iv::FullUpdate();
                return;
            }
            let content_y = HEADER_H + 12;
            let content_h = h - HEADER_H - BOTTOM_H - 24;
            match state.tab {
                pb_ui::Tab::Status => {
                    let wifi = if state.wifi_on {
                        if state.ssid.is_empty() {
                            "WiFi: ON".to_owned()
                        } else {
                            format!("WiFi: {}", state.ssid)
                        }
                    } else if state.wifi_connecting {
                        "WiFi: connecting...".to_owned()
                    } else {
                        "WiFi: OFF".to_owned()
                    };
                    iv::SetFont(f_body, iv::BLACK);
                    let wline = cstring(&wifi);
                    iv::DrawTextRect(
                        GAP,
                        content_y,
                        w - 2 * GAP,
                        60,
                        wline.as_ptr(),
                        iv::ALIGN_LEFT,
                    );
                    iv::SetFont(f_big, iv::BLACK);
                    let url = if state.server_on {
                        state.url()
                    } else {
                        "press START".to_owned()
                    };
                    let u = cstring(&url);
                    iv::DrawTextRect(
                        GAP,
                        content_y + 70,
                        w - 2 * GAP,
                        220,
                        u.as_ptr(),
                        iv::ALIGN_LEFT,
                    );
                    iv::SetFont(f_body, iv::BLACK);
                    let m = cstring(&state.message);
                    iv::DrawTextRect(
                        GAP,
                        content_y + 300,
                        w - 2 * GAP,
                        content_h - 300,
                        m.as_ptr(),
                        iv::ALIGN_LEFT,
                    );
                }
                pb_ui::Tab::Files => {
                    iv::SetFont(f_body, iv::BLACK);
                    let dir = cstring(&state.current_dir);
                    iv::DrawTextRect(GAP, content_y, w - 2 * GAP, 56, dir.as_ptr(), iv::ALIGN_LEFT);
                    draw_file_rows(f_body, state, w);
                }
                pb_ui::Tab::Log => {
                    iv::SetFont(f_body, iv::BLACK);
                    let l = cstring(&log_text(state));
                    iv::DrawTextRect(
                        GAP,
                        content_y,
                        w - 2 * GAP,
                        content_h,
                        l.as_ptr(),
                        iv::ALIGN_LEFT,
                    );
                }
            }
            // bottom buttons (big touch targets)
            let btns = bottom_buttons(state.tab, state.server_on, w, h);
            for b in &btns {
                let primary = b.id == pb_ui::BtnId::StartStop && !state.server_on;
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
            // silence unused import if files_per_page unused here
            iv::CloseFont(f_body);
            iv::CloseFont(f_big);
            iv::FullUpdate();
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
                let y = LIST_Y0 + 56 + (i as i32) * ROW_H;
                if gi == state.selection {
                    iv::FillArea(GAP, y, w - 2 * GAP, ROW_H - 6, iv::LGRAY);
                }
                let mark = if gi == state.selection { "> " } else { "   " };
                let ic = if e.is_dir { "[D] " } else { "[F] " };
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
                let e = cstring("(empty folder)");
                iv::DrawTextRect(
                    GAP,
                    LIST_Y0 + 56,
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

    fn log_text(state: &UiState) -> String {
        let mut out = String::from("server log:\n");
        for l in state.log_lines.iter().rev().take(12) {
            out.push_str(l);
            out.push('\n');
        }
        if out.len() < 20 {
            out.push_str("(empty — start server, open URL)\n");
        }
        out
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
                s.message = "Press START".into();
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
                    s.message = "WiFi connected. Press START.".into();
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

    /// Tap routing: bottom buttons -> top tabs -> file rows. True if handled.
    fn handle_tap(st: &Arc<Mutex<UiState>>, x: i32, y: i32) -> bool {
        use pb_ui::{bottom_buttons, hit_button, row_at, HEADER_H};
        unsafe {
            let w = iv::ScreenWidth();
            let h = iv::ScreenHeight();
            let Ok(s) = st.lock() else { return false };
            let tab = s.tab;
            // 1) bottom buttons
            let btns = bottom_buttons(tab, s.server_on, w, h);
            if let Some(id) = hit_button(&btns, x, y) {
                drop(s);
                match id {
                    pb_ui::BtnId::StartStop => press_start(st),
                    pb_ui::BtnId::Exit => {
                        if let Ok(s) = st.lock() {
                            exit_app(&s);
                        }
                    }
                    pb_ui::BtnId::Up => {
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
                    }
                }
                return true;
            }
            // 2) top tab bar
            if y < HEADER_H {
                drop(s);
                if let Ok(mut s) = st.lock() {
                    if x < w / 3 {
                        s.tab = pb_ui::Tab::Status;
                    } else if x < 2 * w / 3 {
                        s.tab = pb_ui::Tab::Files;
                    } else {
                        s.tab = pb_ui::Tab::Log;
                    }
                    draw(&s);
                }
                return true;
            }
            // 3) file rows
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

    /// START button / MENU / OK: wifi prompt, then async connect in worker.
    /// Runs on the GUI thread (DialogSynchro blocks here, never in worker).
    fn press_start(st: &Arc<Mutex<UiState>>) {
        let (server_on, connecting) = match st.lock() {
            Ok(s) => (s.server_on, s.wifi_connecting),
            Err(_) => return,
        };
        if server_on {
            set_msg(st, "server already running");
            return;
        }
        if connecting {
            set_msg(st, "already connecting, wait...");
            return;
        }
        let ans = unsafe {
            iv::DialogSynchro(
                iv::ICON_QUESTION,
                cstring("WiFi").as_ptr(),
                cstring("Turn on WiFi and start the file server?").as_ptr(),
                cstring("Yes").as_ptr(),
                cstring("No").as_ptr(),
                std::ptr::null(),
            )
        };
        log_line(&format!("wifi prompt answer={ans}"));
        if ans != 1 {
            return;
        }
        if let Ok(mut s) = st.lock() {
            s.wifi_connecting = true;
            s.message = "Starting...".into();
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
                    set_msg(&st2, "Connecting to WiFi...");
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
                            set_msg(&st2, "No async WiFi API. Connect in Settings, then START.");
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
                            set_msg(&st2, &format!("Connecting to WiFi... {}s", waited / 2));
                        }
                        if waited >= 90 {
                            log_line(&format!("worker: wifi timeout: {}", net_dump()));
                            set_msg(
                                &st2,
                                "WiFi timeout. Connect in Settings, then press START.",
                            );
                            finish_connecting(&st2);
                            return;
                        }
                    }
                }
                // online: publish state, bind, serve (blocking)
                let ip = crate::primary_ip().unwrap_or_else(|| "?".into());
                let roots = Roots::device_defaults();
                let srv = Server::new(roots);
                match srv.bind(8080) {
                    Ok((http, port)) => {
                        log_line(&format!("worker: listening {ip}:{port}"));
                        if let Ok(mut s) = st2.lock() {
                            refresh_net_state(&mut s);
                            s.wifi_on = true;
                            s.server_on = true;
                            s.wifi_connecting = false;
                            s.ip = ip;
                            s.port = port;
                            s.message = "server running".into();
                        }
                        request_redraw();
                        srv.run_on(http); // blocks until process exit
                        log_line("worker: serve loop ended");
                    }
                    Err(e) => {
                        log_line(&format!("worker: bind failed: {e}"));
                        set_msg(&st2, &format!("bind failed: {e}"));
                    }
                }
                finish_connecting(&st2);
            });
        if let Err(e) = spawn {
            log_line(&format!("thread spawn failed: {e}"));
            set_msg(&st, &format!("thread failed: {e}"));
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
        log_line("pbweb 0.1.2 starting");
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
