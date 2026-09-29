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

    use std::sync::atomic::{AtomicU32, Ordering};

    /// Partials since last full refresh (anti-ghosting, PocketPuzzles pattern).
    static PARTIAL_COUNT: AtomicU32 = AtomicU32::new(0);

    /// Present a repainted rect: fast partial update, with a periodic full
    /// refresh to wipe e-ink ghosting.
    fn present(r: pb_ui::Rect) {
        unsafe {
            let c = r.clamp_to(iv::ScreenWidth(), iv::ScreenHeight());
            if c.is_empty() {
                return;
            }
            let n = PARTIAL_COUNT.fetch_add(1, Ordering::Relaxed);
            if (n + 1) % pb_ui::FULL_EVERY_N_PARTIALS == 0 {
                iv::FullUpdate();
            } else {
                iv::PartialUpdate(c.x, c.y, c.w, c.h);
            }
        }
    }

    /// Open a font, paint with it, close it. Keeps partial paths simple;
    /// a font open costs milliseconds.
    fn with_font(size: i32, color: i32, paint: impl FnOnce(*mut std::os::raw::c_void)) {
        unsafe {
            let f = iv::OpenFont(cstring("LiberationSans").as_ptr(), size, 1);
            if f.is_null() {
                log_line("paint: OpenFont NULL");
                return;
            }
            iv::SetFont(f, color);
            paint(f);
            iv::CloseFont(f);
        }
    }

    /// Full redraw: ClearScreen + paint everything + FullUpdate.
    /// Used on INIT, tab switches and state transitions. Resets the
    /// partial counter and caches screen size / QR size for workers.
    fn draw(state: &mut UiState) {
        state.dirty = None;
        unsafe {
            iv::ClearScreen();
        }
        let w = unsafe { iv::ScreenWidth() };
        let h = unsafe { iv::ScreenHeight() };
        state.screen_w = w;
        state.screen_h = h;
        let tab = state.tab;
        match tab {
            pb_ui::Tab::Status => {
                paint_status(state, w, h);
            }
            pb_ui::Tab::Files => {
                paint_dir(state, w);
                paint_rows_block(state, w, h);
            }
            pb_ui::Tab::Log => {
                paint_log(state, w, h);
            }
        }
        paint_buttons(state, w, h);
        unsafe {
            iv::FullUpdate();
        }
        PARTIAL_COUNT.store(0, Ordering::Relaxed);
    }

    /// Paint one file row (white-fill first so a moved highlight leaves
    /// no ghost). Returns its rect for present().
    fn paint_row(state: &UiState, w: i32, gi: usize, per: usize) -> Option<pb_ui::Rect> {
        let i = gi.checked_sub(state.files_page * per)?;
        let entry = list_current_dir(&state.current_dir).into_iter().nth(gi)?;
        let r = pb_ui::row_rect(i, w);
        let sel = gi == state.selection;
        let mark = if sel { "> " } else { "   " };
        let ic = if entry.is_dir { "[Папка] " } else { "[Файл] " };
        with_font(F_BODY, iv::BLACK, |_| unsafe {
            iv::FillArea(r.x, r.y, r.w, r.h, if sel { iv::LGRAY } else { iv::WHITE });
            let line = cstring(&format!("{mark}{ic}{}", entry.name));
            iv::DrawTextRect(
                r.x + 8, r.y, r.w - 16, r.h,
                line.as_ptr(), iv::ALIGN_LEFT | iv::VALIGN_MIDDLE,
            );
        });
        Some(r)
    }

    /// Paint the whole visible rows block (dir changes, page changes).
    /// White-fills the block first so shorter listings leave no ghosts.
    /// Returns the repainted rect.
    fn paint_rows_block(state: &UiState, w: i32, h: i32) -> pb_ui::Rect {
        let layout = pb_ui::files_layout(w, h);
        unsafe {
            iv::FillArea(
                layout.rows.x, layout.rows.y,
                layout.rows.w, layout.rows.h, iv::WHITE,
            );
        }
        for i in 0..layout.per_page {
            let gi = state.files_page * layout.per_page + i;
            // paint_row re-lists per row; fine for small dirs
            let _ = paint_row(state, w, gi, layout.per_page);
        }
        layout.rows
    }

    /// Paint the Files dir header line. Returns its rect.
    fn paint_dir(state: &UiState, w: i32) -> pb_ui::Rect {
        use pb_ui::GAP;
        let r = pb_ui::files_layout(w, state.screen_h.max(1)).dir;
        with_font(F_BODY, iv::BLACK, |_| unsafe {
            iv::FillArea(r.x, r.y, r.w, r.h, iv::WHITE);
            let dir = cstring(&format!("Файлы: {}", state.current_dir));
            iv::DrawTextRect(r.x, r.y, r.w, r.h, dir.as_ptr(), iv::ALIGN_LEFT);
        });
        let _ = GAP;
        r
    }

    /// Paint the message line (status tab). Returns its rect.
    fn paint_message(state: &UiState, w: i32, h: i32) -> pb_ui::Rect {
        let r = pb_ui::message_rect(w, h);
        with_font(F_TITLE, iv::BLACK, |_| unsafe {
            iv::FillArea(r.x, r.y, r.w, r.h, iv::WHITE);
            let m = cstring(&state.message);
            iv::DrawTextRect(r.x, r.y, r.w, r.h, m.as_ptr(), iv::ALIGN_LEFT);
        });
        r
    }

    /// Paint the wifi line (status tab). Returns its rect.
    fn paint_wifi(state: &UiState, w: i32, h: i32) -> pb_ui::Rect {
        let l = pb_ui::status_layout(w, h, 0, false, false);
        with_font(F_BODY, iv::BLACK, |_| unsafe {
            iv::FillArea(l.wifi.x, l.wifi.y, l.wifi.w, l.wifi.h, iv::WHITE);
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
            iv::DrawTextRect(
                l.wifi.x, l.wifi.y, l.wifi.w, l.wifi.h,
                wl.as_ptr(), iv::ALIGN_LEFT,
            );
        });
        l.wifi
    }

    /// Paint the server-state line (status tab). Returns its rect.
    fn paint_state(state: &UiState, w: i32, h: i32) -> pb_ui::Rect {
        let l = pb_ui::status_layout(w, h, 0, false, false);
        with_font(F_STATE, iv::BLACK, |_| unsafe {
            iv::FillArea(l.state.x, l.state.y, l.state.w, l.state.h, iv::WHITE);
            let stxt = if state.server_on {
                "Сервер запущен"
            } else if state.wifi_connecting {
                "Подключение..."
            } else {
                "Сервер остановлен"
            };
            let s = cstring(stxt);
            iv::DrawTextRect(
                l.state.x, l.state.y, l.state.w, l.state.h,
                s.as_ptr(), iv::ALIGN_LEFT,
            );
        });
        l.state
    }

    /// Paint title + state + url blocks (status tab).
    fn paint_head(state: &UiState, w: i32, h: i32) {
        let l = pb_ui::status_layout(w, h, 0, false, false);
        with_font(F_TITLE, iv::BLACK, |_| unsafe {
            iv::FillArea(l.title.x, l.title.y, l.title.w, l.title.h, iv::WHITE);
            let title = cstring("PBWeb - передача файлов");
            iv::DrawTextRect(
                l.title.x, l.title.y, l.title.w, l.title.h,
                title.as_ptr(), iv::ALIGN_LEFT,
            );
        });
        paint_state(state, w, h);
        with_font(F_URL, iv::BLACK, |_| unsafe {
            iv::FillArea(l.url.x, l.url.y, l.url.w, l.url.h, iv::WHITE);
            let url = if state.server_on {
                state.url()
            } else {
                "нажми СТАРТ".to_owned()
            };
            let u = cstring(&url);
            iv::DrawTextRect(
                l.url.x, l.url.y, l.url.w, l.url.h,
                u.as_ptr(), iv::ALIGN_LEFT,
            );
        });
    }

    /// Paint the upload progress block. Returns painted rect (None if idle).
    fn paint_upload(state: &UiState, w: i32, h: i32) -> Option<pb_ui::Rect> {
        let srv = SERVER.get()?;
        let (line, pct) = srv.stats.upload_display();
        if line.is_empty() {
            return None;
        }
        // Bar geometry is always reserved: a finished bar must be wiped
        // clean, otherwise its ghost stays under the done message.
        let l = pb_ui::status_layout(w, h, state.qr_size_px, true, true);
        let lr = l.upload_line?;
        let br = l.upload_bar?;
        with_font(F_BODY, iv::BLACK, |_| unsafe {
            iv::FillArea(lr.x, lr.y, lr.w, lr.h, iv::WHITE);
            let ul = cstring(&line);
            iv::DrawTextRect(lr.x, lr.y, lr.w, lr.h, ul.as_ptr(), iv::ALIGN_LEFT);
        });
        if let Some(p) = pct {
            unsafe {
                iv::DrawRect(br.x, br.y, br.w, 30, iv::BLACK);
                let fill = (br.w as u32 * p as u32 / 100) as i32;
                if fill > 4 {
                    iv::FillArea(br.x + 2, br.y + 2, fill - 4, 26, iv::BLACK);
                }
            }
        } else {
            unsafe {
                iv::FillArea(br.x, br.y, br.w, br.h, iv::WHITE);
            }
        }
        Some(lr.union(&br))
    }

    /// Paint the whole status content (full-draw path). Caches the QR size
    /// in state *before* painting upload, so partial repaints later reuse
    /// the exact same geometry.
    fn paint_status(state: &mut UiState, w: i32, h: i32) {
        paint_title_block(w);
        paint_head(state, w, h);
        // QR size: encode once, cache in layout math.
        // The QR carries the paired URL (?token=) so a scan lands
        // straight in the session, no manual PIN entry.
        let mut qr_size = 0;
        if state.server_on && usable_ip(&state.ip) {
            if let Ok(code) = qrcode::QrCode::new(state.url_with_token().as_bytes()) {
                let modules = code.width() as i32;
                // reserve room for the hint line + upload block + message
                let max_h = h - pb_ui::BOTTOM_H - 260 - 410;
                if let Some(l) = pb_ui::qr_layout(modules, w - 2 * pb_ui::GAP, max_h) {
                    qr_size = l.size_px;
                }
            }
        }
        state.qr_size_px = qr_size;
        paint_mid_sized(state, w, h, qr_size);
        paint_upload(state, w, h);
        paint_message(state, w, h);
    }

    fn paint_title_block(w: i32) {
        // title shares the status layout geometry
        let l = pb_ui::status_layout(w, 1448, 0, false, false);
        with_font(F_TITLE, iv::BLACK, |_| unsafe {
            iv::FillArea(l.title.x, l.title.y, l.title.w, l.title.h, iv::WHITE);
            let title = cstring("PBWeb - передача файлов");
            iv::DrawTextRect(
                l.title.x, l.title.y, l.title.w, l.title.h,
                title.as_ptr(), iv::ALIGN_LEFT,
            );
        });
    }

    /// Steps/QR painter used when the QR size is already known.
    fn paint_mid_sized(state: &UiState, w: i32, h: i32, qr_size: i32) {
        let show_qr = state.server_on && usable_ip(&state.ip) && qr_size > 0;
        let l = pb_ui::status_layout(w, h, if show_qr { qr_size } else { 0 }, false, false);
        if show_qr {
            if let Ok(code) = qrcode::QrCode::new(state.url_with_token().as_bytes()) {
                let modules = code.width() as i32;
                let scale = qr_size / (modules + 2 * pb_ui::QR_QUIET);
                if scale >= 1 {
                    if let Some(q) = l.qr {
                        unsafe {
                            iv::FillArea(q.x, q.y, q.w, q.h, iv::WHITE);
                            for my in 0..modules {
                                for mx in 0..modules {
                                    if matches!(code[(mx as usize, my as usize)], qrcode::Color::Dark) {
                                        iv::FillArea(
                                            q.x + (pb_ui::QR_QUIET + mx) * scale,
                                            q.y + (pb_ui::QR_QUIET + my) * scale,
                                            scale, scale, iv::BLACK,
                                        );
                                    }
                                }
                            }
                        }
                        with_font(F_TITLE, iv::BLACK, |_| unsafe {
                            let hint = if state.auth_token.is_empty() {
                                "Отсканируй камерой телефона".to_owned()
                            } else {
                                format!(
                                    "Отсканируй камерой • Код {}",
                                    group_pin(&state.auth_token)
                                )
                            };
                            let hint = cstring(&hint);
                            iv::DrawTextRect(
                                l.mid.x, l.mid.y + qr_size + 8, l.mid.w, 44,
                                hint.as_ptr(), iv::ALIGN_CENTER,
                            );
                        });
                        return;
                    }
                }
            }
        }
        with_font(F_TITLE, iv::BLACK, |_| unsafe {
            iv::FillArea(l.mid.x, l.mid.y, l.mid.w, l.mid.h, iv::WHITE);
            // keyonly builds describe keys (no touch hints for old readers).
            #[cfg(feature = "keyonly")]
            let steps = cstring("1. Подключи телефон к этому WiFi\n2. Открой адрес выше в браузере\n3. ОК - старт/стоп, НАЗАД - выход");
            #[cfg(not(feature = "keyonly"))]
            let steps = cstring("1. Подключи телефон к этому WiFi\n2. Открой адрес выше в браузере\n3. СТОП - остановить, ВЫХОД - выйти");
            iv::DrawTextRect(
                l.mid.x, l.mid.y, l.mid.w, l.mid.h,
                steps.as_ptr(), iv::ALIGN_LEFT,
            );
        });
    }

    /// Paint the log area. Returns its rect.
    fn paint_log(state: &UiState, w: i32, h: i32) -> pb_ui::Rect {
        let r = pb_ui::log_rect(w, h);
        let _ = state;
        with_font(F_TITLE, iv::BLACK, |_| unsafe {
            iv::FillArea(r.x, r.y, r.w, r.h, iv::WHITE);
            let l = cstring(&log_text());
            iv::DrawTextRect(r.x, r.y, r.w, r.h, l.as_ptr(), iv::ALIGN_LEFT);
        });
        r
    }

    /// Paint one bottom button. All buttons are white with black text;
    /// `pressed` inverts to a black flash (visible on e-ink, unlike a
    /// flash on an already-black button).
    fn paint_button(btn: &pb_ui::Btn, pressed: bool) {
        with_font(F_BODY, if pressed { iv::WHITE } else { iv::BLACK }, |_| unsafe {
            if pressed {
                iv::FillArea(btn.x, btn.y, btn.w, btn.h, iv::BLACK);
            } else {
                iv::FillArea(btn.x, btn.y, btn.w, btn.h, iv::WHITE);
                iv::DrawRect(btn.x, btn.y, btn.w, btn.h, iv::BLACK);
                iv::DrawRect(btn.x + 3, btn.y + 3, btn.w - 6, btn.h - 6, iv::BLACK);
            }
            let lb = cstring(&btn.label);
            iv::DrawTextRect(
                btn.x, btn.y, btn.w, btn.h,
                lb.as_ptr(), iv::ALIGN_CENTER | iv::VALIGN_MIDDLE,
            );
        });
    }

    fn paint_buttons(state: &UiState, w: i32, h: i32) {
        for b in &pb_ui::bottom_buttons(state.tab, state.server_on, w, h) {
            paint_button(b, false);
        }
    }

    /// Route a worker-requested partial repaint (consumed from state.dirty).
    fn paint_dirty(st: &Arc<Mutex<UiState>>, d: pb_ui::Dirty) {
        let Ok(s) = st.lock() else { return };
        let (w, h) = (s.screen_w, s.screen_h);
        use pb_ui::Dirty::*;
        match (s.tab, d) {
            (pb_ui::Tab::Status, Message) => {
                let r = paint_message(&s, w, h);
                drop(s);
                present(r);
            }
            (pb_ui::Tab::Status, Upload) => {
                if let Some(r) = paint_upload(&s, w, h) {
                    drop(s);
                    present(r);
                }
            }
            (pb_ui::Tab::Status, Wifi) => {
                paint_wifi(&s, w, h);
                let l = pb_ui::status_layout(w, h, 0, false, false);
                drop(s);
                present(l.wifi);
            }
            (pb_ui::Tab::Log, Log) => {
                let r = paint_log(&s, w, h);
                drop(s);
                present(r);
            }
            (_, Full) => {
                drop(s);
                if let Ok(mut s) = st.lock() {
                    draw(&mut s);
                }
            }
            _ => {}
        }
    }

    /// IP is usable for a QR/link when it looks like a real address.
    fn usable_ip(ip: &str) -> bool {
        !(ip.is_empty() || ip == "?" || ip == "-") && ip.contains('.')
    }

    /// Group a 6-digit PIN for the e-ink screen: "123456" -> "123 456".
    fn group_pin(pin: &str) -> String {
        if pin.len() == 6 {
            format!("{} {}", &pin[..3], &pin[3..])
        } else {
            pin.to_owned()
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
                draw(&mut s);
                return 1;
            }
            x if x == iv::EVT_SHOW => {
                // Worker-requested partial repaint (progress, countdown);
                // anything else (dialogs, foregrounding) gets a full draw.
                if let Some(d) = s.dirty.take() {
                    drop(s);
                    paint_dirty(&st, d);
                    return 1;
                }
                draw(&mut s);
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
                draw(&mut s);
                return 1;
            }
            x if x == iv::EVT_NET_DISCONNECTED => {
                // only the wifi line changes -> partial, no flash
                log_line("net disconnected event");
                s.wifi_on = false;
                let (w, h) = live_wh(&s);
                drop(s);
                if let Ok(s) = st.lock() {
                    let r = paint_wifi(&s, w, h);
                    drop(s);
                    present(r);
                }
                return 1;
            }
            x if x == iv::EVT_SCANSTOPPED => {
                // Library rescan finished (resident scanner.app broadcast).
                // Only meaningful if we asked for one; otherwise ignore so
                // system scans don't rewrite our screen.
                if !s.library_scanning {
                    return 0;
                }
                s.library_scanning = false;
                let changes = iv::db_changes();
                log_line(&format!("library scan stopped, db_changes={changes:?}"));
                s.message = "Библиотека обновлена".into();
                let (w, h) = live_wh(&s);
                drop(s);
                // only the message line changes -> partial, no flash
                if let Ok(s) = st.lock() {
                    let r = paint_message(&s, w, h);
                    drop(s);
                    present(r);
                }
                return 1;
            }
            x if x == iv::EVT_KEYPRESS => {
                log_line(&format!("key p1={p1}"));
                // All key routing lives in pb-ui (pure + host-tested);
                // the handler only executes the resulting action.
                match pb_ui::key_action(s.tab, p1) {
                    pb_ui::KeyAction::ToggleServer => {
                        let on = s.server_on;
                        drop(s);
                        toggle_server(&st, on);
                        return 1;
                    }
                    pb_ui::KeyAction::NextTab => {
                        s.next_tab();
                        draw(&mut s);
                        return 1;
                    }
                    pb_ui::KeyAction::PrevTab => {
                        s.prev_tab();
                        draw(&mut s);
                        return 1;
                    }
                    pb_ui::KeyAction::MoveSel(d) => {
                        if s.tab != pb_ui::Tab::Files {
                            s.tab = pb_ui::Tab::Files;
                            draw(&mut s);
                            return 1;
                        }
                        let per = pb_ui::files_per_page(unsafe { iv::ScreenHeight() });
                        let len = file_count(&s.current_dir);
                        let old = s.selection;
                        s.move_sel(len, d, per);
                        let new = s.selection;
                        // selection move repaints two rows only (no full flash)
                        let (w, _) = live_wh(&s);
                        drop(s);
                        repaint_rows(&st, w, per, &[old, new]);
                        return 1;
                    }
                    pb_ui::KeyAction::Page(d) => {
                        if s.tab != pb_ui::Tab::Files {
                            s.tab = pb_ui::Tab::Files;
                            draw(&mut s);
                            return 1;
                        }
                        let (w, h) = live_wh(&s);
                        let per = pb_ui::files_per_page(h);
                        let len = file_count(&s.current_dir);
                        s.move_sel(len, d * per as isize, per);
                        drop(s);
                        repaint_files_region(&st, w, h);
                        return 1;
                    }
                    pb_ui::KeyAction::Enter => {
                        // OK/RIGHT in Files: descend like a tap on the row.
                        // (Tapping a file only selects it, so keys lose nothing.)
                        if s.tab != pb_ui::Tab::Files {
                            return 1;
                        }
                        let next = descend(&s.current_dir.clone(), s.selection);
                        let (w, h) = live_wh(&s);
                        if let Some(next) = next {
                            s.current_dir = next;
                            s.selection = 0;
                            s.files_page = 0;
                            drop(s);
                            repaint_files_region(&st, w, h);
                        }
                        return 1;
                    }
                    pb_ui::KeyAction::Up => {
                        // LEFT/BACK in Files: up a dir; at root back to
                        // Status (a second BACK then exits — no accidents).
                        if let Some(par) = pb_ui::UiState::parent_dir(&s.current_dir.clone()) {
                            s.current_dir = par;
                            s.selection = 0;
                            s.files_page = 0;
                            let (w, h) = live_wh(&s);
                            drop(s);
                            // dir + rows only, no full flash
                            repaint_files_region(&st, w, h);
                        } else {
                            s.tab = pb_ui::Tab::Status;
                            draw(&mut s);
                        }
                        return 1;
                    }
                    pb_ui::KeyAction::Exit => {
                        exit_app(&s);
                        return 1;
                    }
                    pb_ui::KeyAction::None => {}
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

    /// Screen size, preferring the INIT-cached values (workers must not
    /// call into InkView for this).
    fn live_wh(s: &UiState) -> (i32, i32) {
        let w = if s.screen_w > 0 {
            s.screen_w
        } else {
            unsafe { iv::ScreenWidth() }
        };
        let h = if s.screen_h > 0 {
            s.screen_h
        } else {
            unsafe { iv::ScreenHeight() }
        };
        (w, h)
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

    fn set_text(st: &Arc<Mutex<UiState>>, msg: &str) {
        if let Ok(mut s) = st.lock() {
            s.message = msg.into();
        }
    }

    /// GUI-side message update: text + message-only partial (no full flash).
    /// Skipped when the message line isn't visible (Files/Log tabs).
    fn set_msg_partial(st: &Arc<Mutex<UiState>>, msg: &str) {
        set_text(st, msg);
        if let Ok(s) = st.lock() {
            if s.tab != pb_ui::Tab::Status {
                return;
            }
            let (w, h) = live_wh(&s);
            let r = paint_message(&s, w, h);
            drop(s);
            present(r);
        }
    }

    /// Worker-side partial repaint request: marks the region dirty and wakes
    /// the GUI thread, which paints just that rect (PocketPuzzles pattern).
    /// Skipped entirely when the region isn't visible — no wasted wakeups.
    fn request_partial(st: &Arc<Mutex<UiState>>, d: pb_ui::Dirty) {
        let mut send = false;
        if let Ok(mut s) = st.lock() {
            let visible = matches!(
                (s.tab, d),
                (pb_ui::Tab::Status, pb_ui::Dirty::Message)
                    | (pb_ui::Tab::Status, pb_ui::Dirty::Upload)
                    | (pb_ui::Tab::Status, pb_ui::Dirty::Wifi)
                    | (pb_ui::Tab::Log, pb_ui::Dirty::Log)
                    | (_, pb_ui::Dirty::Full)
            );
            if visible {
                s.dirty = Some(d);
                send = true;
            }
        }
        if send {
            request_redraw();
        }
    }

    /// Refresh wifi_on/ssid from NetInfo (best effort, never panics).
    /// FW5/6 only — see the pro903 variant below.
    #[cfg(not(feature = "pro903"))]
    fn refresh_net_state(s: &mut UiState) {
        if let Some(n) = iv::netinfo_full() {
            s.wifi_on = n.connected != 0;
            s.ssid = unsafe { iv::take_c_string(n.name.as_ptr().cast_mut()) }.unwrap_or_default();
        } else {
            s.wifi_on = unsafe { iv::QueryNetwork() != 0 };
        }
    }

    /// Pro 903 / FW2: iv_netinfo is opaque in the old headers (real layout
    /// unknown), so NetInfo is never touched — QueryNetwork() is the only
    /// state signal. SSID stays empty → the wifi line shows "WiFi: включён".
    #[cfg(feature = "pro903")]
    fn refresh_net_state(s: &mut UiState) {
        s.wifi_on = unsafe { iv::QueryNetwork() != 0 };
    }

    // Async-connect callback: only logged, polling NetInfo is the truth.
    extern "C" fn net_cb(status: i32) -> i32 {
        log_line(&format!("NetConnectAsync cb status={status}"));
        0
    }

    /// Repaint listed selection rows (counted as one partial).
    fn repaint_rows(st: &Arc<Mutex<UiState>>, w: i32, per: usize, idxs: &[usize]) {
        let Ok(s) = st.lock() else { return };
        let mut union: Option<pb_ui::Rect> = None;
        for &gi in idxs {
            if let Some(r) = paint_row(&s, w, gi, per) {
                union = Some(match union {
                    Some(u) => u.union(&r),
                    None => r,
                });
            }
        }
        drop(s);
        if let Some(r) = union {
            present(r);
        }
    }

    /// Tap routing: bottom buttons -> file rows. True if handled.
    /// (No top bar anymore — tabs switch via the middle button / PREV/NEXT.)
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
                // instant press feedback on the button rect (PocketPuzzles
                // pattern: InvertArea + PartialUpdate, no full flash)
                if let Some(b) = btns.iter().find(|b| b.id == id) {
                    paint_button(b, true);
                    present(pb_ui::btn_rect(b));
                }
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
                            }
                            // dir line + rows block only
                            repaint_files_region(st, w, h);
                        } else {
                            press_start(st);
                        }
                    }
                    pb_ui::BtnId::Tabs => {
                        if let Ok(mut s) = st.lock() {
                            s.next_tab();
                            draw(&mut s);
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
                    let old = s.selection;
                    let next = descend(&s.current_dir.clone(), idx);
                    let nav = next.is_some();
                    drop(s);
                    if let Ok(mut s) = st.lock() {
                        s.selection = idx;
                        if let Some(next) = next {
                            s.current_dir = next;
                            s.selection = 0;
                            s.files_page = 0;
                        }
                    }
                    if nav {
                        repaint_files_region(st, w, h);
                    } else {
                        repaint_rows(st, w, per, &[old, idx]);
                    }
                    return true;
                }
            }
            false
        }
    }

    /// Repaint dir header + rows block (UP navigation, dir changes).
    fn repaint_files_region(st: &Arc<Mutex<UiState>>, w: i32, h: i32) {
        let Ok(s) = st.lock() else { return };
        let dir = paint_dir(&s, w);
        let rows = paint_rows_block(&s, w, h);
        drop(s);
        present(dir.union(&rows));
    }

    /// STOP button: real shutdown (unblocks the accept loop, closes socket).
    /// WiFi stays on so START works instantly afterwards.
    /// Then asks the resident scanner.app to reindex the library
    /// (broadcast EVT_STARTSCAN). Repaints message + buttons + mid block
    /// as three partials instead of one full flash.
    fn stop_server(st: &Arc<Mutex<UiState>>) {
        log_line("stop pressed");
        if let Some(srv) = SERVER.get() {
            srv.shutdown();
        }
        fire_scan_broadcast();
        if let Ok(mut s) = st.lock() {
            s.server_on = false;
            s.wifi_connecting = false;
            s.auth_token.clear();
            #[cfg(not(feature = "pro903"))]
            {
                s.library_scanning = true;
                s.message = "Сервер остановлен. Обновляю библиотеку...".into();
            }
            #[cfg(feature = "pro903")]
            {
                // No scanner service on FW2 (see fire_scan_broadcast).
                s.library_scanning = false;
                s.message = "Сервер остановлен".into();
            }
        }
        if let Ok(s) = st.lock() {
            let (w, h) = live_wh(&s);
            let m = paint_message(&s, w, h);
            paint_buttons(&s, w, h);
            let mut bar: Option<pb_ui::Rect> = None;
            for b in &pb_ui::bottom_buttons(s.tab, s.server_on, w, h) {
                bar = Some(match bar {
                    Some(u) => u.union(&pb_ui::btn_rect(b)),
                    None => pb_ui::btn_rect(b),
                });
            }
            paint_mid_sized(&s, w, h, 0);
            let mid = pb_ui::status_layout(w, h, 0, false, false).mid;
            drop(s);
            present(m);
            present(mid);
            if let Some(b) = bar {
                present(b);
            }
        }
    }

    /// Ask the resident scanner.app service to rescan the library, exactly
    /// like PBScanClient::startDeviceScan() does: broadcast EVT_STARTSCAN
    /// (0xD7) to every task. Fire-and-forget; completion arrives later as
    /// EVT_SCANSTOPPED in our own handler. Everything is logged.
    /// FW5/6 only — see the pro903 stub below.
    #[cfg(not(feature = "pro903"))]
    fn fire_scan_broadcast() {
        let rc = unsafe { iv::SendEventTo(iv::TASK_BROADCAST, iv::EVT_STARTSCAN, 0, 0) };
        let mode = read_scanmode();
        let changes = iv::db_changes();
        let scanning = iv::scan_flag();
        log_line(&format!(
            "library scan: broadcast rc={rc} scanmode={mode:?} scanning={scanning:?} db_changes={changes:?}"
        ));
    }

    /// Pro 903 / FW2: no resident scanner.app service and no STARTSCAN
    /// broadcast — the symbol isn't even linked. Files still land on disk
    /// immediately; the user refreshes Library manually (same as any
    /// FW2 sideload). Deliberately a no-op so STOP/ВЫХОД stay instant.
    #[cfg(feature = "pro903")]
    fn fire_scan_broadcast() {
        log_line("library scan: skipped (no scanner service on FW2)");
    }

    /// Read the library scan policy (0=Off, 1=Once, 2=Auto) from global.cfg.
    /// Diagnostics only — the 0xD7 handler path doesn't check it, and neither
    /// do we: the setting stays the user's own business.
    /// FW5/6 only (the pro903 broadcast is a no-op that never reads it).
    #[cfg(not(feature = "pro903"))]
    fn read_scanmode() -> Option<i32> {
        let content = std::fs::read_to_string("/mnt/ext1/system/config/global.cfg").ok()?;
        crate::parse_scanmode(&content)
    }

    /// START button / MENU / OK: wifi prompt, then async connect in worker.
    /// Runs on the GUI thread (dialogs block here, never in the worker).
    fn press_start(st: &Arc<Mutex<UiState>>) {
        let (server_on, connecting) = match st.lock() {
            Ok(s) => (s.server_on, s.wifi_connecting),
            Err(_) => return,
        };
        if server_on {
            set_msg_partial(st, "Сервер уже запущен");
            return;
        }
        if connecting {
            set_msg_partial(st, "Уже подключаюсь, подожди...");
            return;
        }
        #[cfg(not(feature = "pro903"))]
        {
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
            begin_connect(st);
        }
        #[cfg(feature = "pro903")]
        {
            // FW2 has no blocking DialogSynchro: show the async prompt and
            // continue in dialog_cb (GUI thread) when the user answers.
            unsafe {
                iv::Dialog(
                    iv::ICON_QUESTION,
                    cstring("WiFi").as_ptr(),
                    cstring("Включить WiFi и запустить сервер файлов?").as_ptr(),
                    cstring("Да").as_ptr(),
                    cstring("Нет").as_ptr(),
                    Some(dialog_cb),
                );
            }
            log_line("wifi prompt shown (async, FW2)");
        }
    }

    /// OK/MENU global toggle: START when idle, real STOP when serving.
    /// (Previously keys could only start; stopping needed a tap.)
    fn toggle_server(st: &Arc<Mutex<UiState>>, server_on: bool) {
        if server_on {
            stop_server(st);
        } else {
            press_start(st);
        }
    }

    /// Shared START prelude once the user said Yes (blocking prompt on
    /// FW5/6, async dialog_cb on FW2): flag + paint, then the worker.
    fn begin_connect(st: &Arc<Mutex<UiState>>) {
        if let Ok(mut s) = st.lock() {
            if s.server_on || s.wifi_connecting {
                return;
            }
            s.wifi_connecting = true;
            s.message = "Запуск...".into();
            // state + message lines only (dialog follows, its close path
            // does a full draw via EVT_SHOW fallback)
            let (w, h) = live_wh(&s);
            let rs = paint_state(&s, w, h);
            let rm = paint_message(&s, w, h);
            drop(s);
            present(rs);
            present(rm);
        }
        wifi_and_serve(st.clone());
    }

    /// FW2 async WiFi-prompt answer (runs on the GUI thread).
    /// Button 1 = "Да" → same prelude as the blocking prompt elsewhere.
    #[cfg(feature = "pro903")]
    unsafe extern "C" fn dialog_cb(button: i32) {
        log_line(&format!("wifi prompt answer={button}"));
        if button != 1 {
            return;
        }
        if let Some(st) = STATE.get().cloned() {
            begin_connect(&st);
        }
    }

    fn exit_app(_s: &UiState) {
        log_line("exit: scan broadcast + rescan + CloseApp");
        // The scan outlives us: scanner.app is a resident service, so the
        // reindex continues (and finishes) after our process exits.
        fire_scan_broadcast();
        crate::library_rescan();
        unsafe {
            iv::iv_sync();
            iv::CloseApp();
        }
    }

    /// Online check.
    /// FW5/6: NetInfo()->connected ONLY. QueryNetwork() has undocumented
    /// semantics (nonzero even when offline) and must not gate connecting.
    /// Pro 903 / FW2: iv_netinfo is opaque in the old headers, so NetInfo
    /// is never touched — QueryNetwork() is the signal, and the blocking
    /// NetConnect return code is authoritative (see the worker below).
    #[cfg(not(feature = "pro903"))]
    fn online() -> bool {
        iv::netinfo_connected().unwrap_or(false)
    }
    #[cfg(feature = "pro903")]
    fn online() -> bool {
        unsafe { iv::QueryNetwork() != 0 }
    }

    /// One-line dump of NetInfo + interfaces for the log.
    /// FW5/6 only — see the pro903 variant below.
    #[cfg(not(feature = "pro903"))]
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

    /// Pro 903 / FW2: NetInfo layout unknown → skipped; QueryNetwork +
    /// interfaces only. Enough to diagnose connect failures from pbweb.log.
    #[cfg(feature = "pro903")]
    fn net_dump() -> String {
        let q = unsafe { iv::QueryNetwork() };
        let base = format!("query_network={q} netinfo=SKIPPED(FW2)");
        #[cfg(target_os = "linux")]
        {
            if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
                let mut ifs: Vec<String> = rd
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                ifs.sort();
                return format!("{base} ifs={}", ifs.join(","));
            }
        }
        base
    }

    /// Non-blocking wifi flow in a worker thread. NEVER calls blocking
    /// NetConnect*: silent attempt, then NetConnectAsync + poll NetInfo
    /// with a 45s cap. UI updates go through shared state + SendEvent.
    fn wifi_and_serve(st: Arc<Mutex<UiState>>) {
        let st2 = st.clone();
        // Small explicit stack: the device has ~256MB RAM total and the
        // worker only does sequential I/O + small buffers.
        let spawn = std::thread::Builder::new()
            .name("pbweb-wifi".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                iv::postpone_poweroff();
                log_line(&format!("worker start: {}", net_dump()));
                if !online() {
                    // 1) silent attempt: no dialogs, quick
                    set_text(&st2, "Подключение к WiFi...");
                    request_partial(&st2, pb_ui::Dirty::Message);
                    log_line("worker: calling NetConnectSilent...");
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
                            // FW2 (Pro 903): no async API at all. Blocking
                            // NetConnect(NULL) is safe here — this IS the
                            // worker thread, the e-ink UI never hangs.
                            log_line("worker: NetConnectAsync missing, legacy blocking NetConnect...");
                            set_text(&st2, "Подключение к WiFi...");
                            request_partial(&st2, pb_ui::Dirty::Message);
                            log_line("worker: calling legacy NetConnect(NULL)...");
                            let rc = unsafe { iv::NetConnect(std::ptr::null()) };
                            log_line(&format!("worker: legacy NetConnect rc={rc}"));
                            if rc != 0 && !online() {
                                log_line(&format!("worker: legacy connect failed: {}", net_dump()));
                                set_text(
                                    &st2,
                                    "Нет WiFi. Подключись в настройках и нажми СТАРТ.",
                                );
                                request_partial(&st2, pb_ui::Dirty::Message);
                                finish_connecting(&st2);
                                return;
                            }
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
                            set_text(&st2, &format!("Подключение к WiFi... {}с", waited / 2));
                            request_partial(&st2, pb_ui::Dirty::Message);
                        }
                        if waited >= 90 {
                            log_line(&format!("worker: wifi timeout: {}", net_dump()));
                            set_text(
                                &st2,
                                "Нет WiFi. Подключись в настройках и нажми СТАРТ.",
                            );
                            request_partial(&st2, pb_ui::Dirty::Message);
                            finish_connecting(&st2);
                            return;
                        }
                    }
                }
                // online: publish state, bind, serve (blocking until STOP/exit)
                let ip = crate::primary_ip().unwrap_or_else(|| "?".into());
                let roots = Roots::device_defaults();
                let srv = Arc::new(Server::new(roots));
                // progress hook: repaint only the upload block as bytes
                // arrive (PocketPuzzles pattern: no full flash per tick).
                // When an upload just finished, paint one full frame instead
                // so the final state is exact (no bar ghost, no stuck %).
                let hook_st = st2.clone();
                let hook_stats = srv.stats.clone();
                srv.stats.set_hook(Arc::new(move || {
                    if hook_stats.upload.finished.swap(false, Ordering::Relaxed) {
                        if let Ok(mut s) = hook_st.lock() {
                            s.dirty = Some(pb_ui::Dirty::Full);
                        }
                        request_redraw();
                    } else {
                        request_partial(&hook_st, pb_ui::Dirty::Upload);
                    }
                }));
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
                            s.auth_token = srv.auth_token.clone();
                            s.message = "Сервер запущен".into();
                            s.server_generation = s.server_generation.wrapping_add(1);
                        }
                        request_redraw();
                        spawn_idle_watchdog(st2.clone(), srv.clone());
                        srv.run_shared(&http); // blocks until shutdown()/exit
                        log_line("worker: serve loop ended");
                        // stopped via СТОП (or socket died): back to idle
                        if let Ok(mut s) = st2.lock() {
                            s.server_on = false;
                            s.wifi_connecting = false;
                            s.auth_token.clear();
                            if s.message == "Сервер запущен" {
                                s.message = "Сервер остановлен".into();
                            }
                        }
                        request_redraw();
                    }
                    Err(e) => {
                        log_line(&format!("worker: bind failed: {e}"));
                        set_text(&st2, &format!("Ошибка запуска: {e}"));
                        request_partial(&st2, pb_ui::Dirty::Message);
                    }
                }
                finish_connecting(&st2);
            });
        if let Err(e) = spawn {
            log_line(&format!("thread spawn failed: {e}"));
            set_text(&st, &format!("Ошибка потока: {e}"));
            request_partial(&st, pb_ui::Dirty::Message);
            finish_connecting(&st);
        }
    }

    /// Idle watchdog: stops a forgotten server after IDLE_TIMEOUT_MS of
    /// no requests (and no active upload). GUI work stays on the GUI
    /// thread — here only state + SendEvent. Dies with stale generations.
    fn spawn_idle_watchdog(st: Arc<Mutex<UiState>>, srv: Arc<Server>) {
        const IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1000;
        let gen = st
            .lock()
            .map(|s| s.server_generation)
            .unwrap_or(u64::MAX);
        std::thread::Builder::new()
            .name("pbweb-idle".into())
            .stack_size(256 * 1024)
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                let cur_gen = st.lock().map(|s| s.server_generation).unwrap_or(u64::MAX);
                if cur_gen != gen || !srv.is_live() {
                    break; // superseded or already stopped
                }
                let idle =
                    pb_server::now_ms().saturating_sub(srv.stats.last_activity_ms.load(Ordering::Relaxed));
                let uploading = srv.stats.upload.active.load(Ordering::Relaxed);
                if !uploading && idle > IDLE_TIMEOUT_MS {
                    log_line("idle watchdog: stopping forgotten server");
                    srv.shutdown();
                    fire_scan_broadcast();
                    if let Ok(mut s) = st.lock() {
                        s.server_on = false;
                        s.wifi_connecting = false;
                        s.auth_token.clear();
                        s.library_scanning = true;
                        s.message = "Остановлен по таймеру. Обновляю библиотеку...".into();
                    }
                    request_redraw();
                    break;
                }
            })
            .ok();
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
        let tag = if cfg!(feature = "pro903") { " (pro903)" } else { "" };
        log_line(&format!("pbweb {} starting{tag}", env!("CARGO_PKG_VERSION")));
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

/// Parse the library scan policy from global.cfg content.
/// Format is `scanmode=N` lines (0=Off, 1=Once, 2=Auto); first match wins.
/// (pro903 builds never read it on-device; kept for the host unit test.)
#[cfg(any(test, not(feature = "pro903")))]
fn parse_scanmode(content: &str) -> Option<i32> {
    for line in content.lines() {
        let t = line.trim().trim_start_matches(['#', ';']).trim_start();
        let Some(rest) = t.strip_prefix("scanmode") else {
            continue;
        };
        let val = rest.trim_start_matches(['=', ' ', '\t', ':']);
        let digits: String = val.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        return digits.parse::<i32>().ok();
    }
    None
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
        println!("host dev pairing PIN: {}", srv.auth_token);
        let _ = srv.serve(8080);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_encodes_typical_url() {
        // device URL must fit a scannable code (v1..v5 => 21..37 modules)
        let code = qrcode::QrCode::new(b"http://192.168.88.84:8080").unwrap();
        assert!((21..=41).contains(&(code.width() as i32)));
        // finder pattern corner is dark
        assert!(matches!(code[(0, 0)], qrcode::Color::Dark));
    }

    #[test]
    fn scanmode_parsing() {
        assert_eq!(super::parse_scanmode("scanmode=2\n"), Some(2));
        assert_eq!(super::parse_scanmode("# comment\nscanmode = 0\n"), Some(0));
        assert_eq!(super::parse_scanmode("scanmode:1\r\n"), Some(1));
        assert_eq!(super::parse_scanmode("other=5\n"), None);
        assert_eq!(super::parse_scanmode(""), None);
        assert_eq!(super::parse_scanmode("scanmode=\n"), None);
    }
}
