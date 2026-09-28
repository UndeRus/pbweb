//! E-ink UI state machine: Status / Files / Log tabs.
//! Rendering goes through pb-sys; this crate only owns state + layout math
//! so it stays unit-testable on host.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Files,
    Log,
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub tab: Tab,
    pub wifi_on: bool,
    pub server_on: bool,
    pub wifi_connecting: bool,
    pub ssid: String,
    pub ip: String,
    pub port: u16,
    pub message: String,
    pub files_page: usize,
    pub current_dir: String,
    pub selection: usize,
    pub log_lines: Vec<String>,
    /// Cached screen size (set at INIT; worker threads must not call
    /// ScreenWidth/Height, so they use these).
    pub screen_w: i32,
    pub screen_h: i32,
    /// Last drawn QR square side in px (0 = steps mode). Needed to locate
    /// the upload block without re-encoding the QR off the GUI thread.
    pub qr_size_px: i32,
    /// Pending partial repaint for the next EVT_SHOW. Set by worker
    /// threads, consumed by the GUI handler. None = full redraw.
    pub dirty: Option<Dirty>,
    /// Library rescan requested, completion (EVT_SCANSTOPPED) not seen yet.
    /// Guards against duplicate broadcasts and spurious stop events.
    pub library_scanning: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Status,
            wifi_on: false,
            server_on: false,
            wifi_connecting: false,
            ssid: String::new(),
            ip: String::from("-"),
            port: 8080,
        message: String::from("Нажми СТАРТ"),
        files_page: 0,
        current_dir: String::from("int:/"),
        selection: 0,
        log_lines: Vec::new(),
        screen_w: 1072,
        screen_h: 1448,
        qr_size_px: 0,
        dirty: None,
        library_scanning: false,
        }
    }
}

impl UiState {
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.ip, self.port)
    }
    pub fn next_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Status => Tab::Files,
            Tab::Files => Tab::Log,
            Tab::Log => Tab::Status,
        };
    }
    pub fn prev_tab(&mut self) {
        self.tab = match self.tab {
            Tab::Status => Tab::Log,
            Tab::Files => Tab::Status,
            Tab::Log => Tab::Files,
        };
    }
    pub fn push_log(&mut self, line: String) {
        self.log_lines.push(line);
        if self.log_lines.len() > 40 {
            self.log_lines.remove(0);
        }
    }
    /// Move selection within a list of `len` items; returns page to show.
    pub fn move_sel(&mut self, len: usize, delta: isize, per_page: usize) {
        if len == 0 {
            self.selection = 0;
            self.files_page = 0;
            return;
        }
        let mut s = self.selection as isize + delta;
        s = s.clamp(0, len as isize - 1);
        self.selection = s as usize;
        self.files_page = self.selection / per_page.max(1);
    }
    pub fn parent_dir(dir: &str) -> Option<String> {
        let (root, rest) = dir.split_once(":/")?;
        let rest = rest.trim_matches('/');
        if rest.is_empty() {
            return None;
        }
        match rest.rsplit_once('/') {
            Some((par, _)) if !par.is_empty() => Some(format!("{root}:/{par}")),
            _ => Some(format!("{root}:/")),
        }
    }
}

/// Pure layout helper: rows per page for file list given screen height.
pub fn files_per_page(screen_h: i32) -> usize {
    let usable = (screen_h - 340).max(200) as usize;
    (usable / ROW_H as usize).clamp(3, 16)
}

// ---- Big touch buttons (all actions reachable by tap) ----

pub const BOTTOM_H: i32 = 150;
pub const ROW_H: i32 = 72;
pub const GAP: i32 = 16;
/// Y of the first file row (must match draw code in pb-app).
pub const LIST_Y0: i32 = 96;

/// File-row index for a tap at height y, or None if outside rows.
pub fn row_at(y: i32, page_first: usize, per_page: usize, total: usize) -> Option<usize> {
    if y < LIST_Y0 {
        return None;
    }
    let idx = page_first + ((y - LIST_Y0) / ROW_H) as usize;
    let page_end = (page_first + per_page).min(total);
    (idx < page_end).then_some(idx)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtnId {
    Primary,
    Tabs,
    Exit,
}

#[derive(Debug, Clone)]
pub struct Btn {
    pub id: BtnId,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub label: String,
}

/// Bottom button bar: always 3 big buttons.
/// Primary = START/STOP (status, log) or UP (files).
/// The middle button shows the *destination* tab so its purpose is obvious.
pub fn bottom_buttons(tab: Tab, server_on: bool, sw: i32, sh: i32) -> Vec<Btn> {
    let y = sh - BOTTOM_H + 20;
    let h = BOTTOM_H - 40;
    let w = (sw - 4 * GAP) / 3;
    let primary_label = match tab {
        Tab::Files => "ВВЕРХ".to_string(),
        _ => {
            if server_on {
                "СТОП".to_string()
            } else {
                "СТАРТ".to_string()
            }
        }
    };
    // same cycle as UiState::next_tab: Status -> Files -> Log -> Status
    let tabs_label = match tab {
        Tab::Status => "ФАЙЛЫ",
        Tab::Files => "ЖУРНАЛ",
        Tab::Log => "СТАТУС",
    };
    vec![
        Btn {
            id: BtnId::Primary,
            x: GAP,
            y,
            w,
            h,
            label: primary_label,
        },
        Btn {
            id: BtnId::Tabs,
            x: 2 * GAP + w,
            y,
            w,
            h,
            label: tabs_label.to_string(),
        },
        Btn {
            id: BtnId::Exit,
            x: 3 * GAP + 2 * w,
            y,
            w,
            h,
            label: "ВЫХОД".to_string(),
        },
    ]
}

pub fn hit_button(btns: &[Btn], x: i32, y: i32) -> Option<BtnId> {
    btns
        .iter()
        .find(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h)
        .map(|b| b.id)
}

// ---- QR code layout (server URL on the status screen) ----

/// Modules of quiet zone around the code (spec minimum is 4).
pub const QR_QUIET: i32 = 4;
/// Max module size in px — bigger is not more readable, just taller.
pub const QR_MAX_SCALE: i32 = 10;
/// Min module size in px for reliable phone scanning.
pub const QR_MIN_SCALE: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QrLayout {
    /// px per module
    pub scale: i32,
    /// full side in px, quiet zone included
    pub size_px: i32,
}

/// Fit a `modules x modules` QR code into a max_w/max_h box.
/// Returns None when even the minimum readable size doesn't fit.
pub fn qr_layout(modules: i32, max_w: i32, max_h: i32) -> Option<QrLayout> {
    if modules <= 0 || max_w <= 0 || max_h <= 0 {
        return None;
    }
    let units = modules + 2 * QR_QUIET;
    let scale = (max_w / units).min(max_h / units).min(QR_MAX_SCALE);
    if scale < QR_MIN_SCALE {
        return None;
    }
    Some(QrLayout {
        scale,
        size_px: units * scale,
    })
}

// ---- Partial-update regions (PocketPuzzles pattern) ----

/// Full-update every Nth partial repaint to wipe e-ink ghosting.
pub const FULL_EVERY_N_PARTIALS: u32 = 10;

/// Screen rectangle for partial repaints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn union(&self, o: &Rect) -> Rect {
        let x1 = self.x.min(o.x);
        let y1 = self.y.min(o.y);
        let x2 = (self.x + self.w).max(o.x + o.w);
        let y2 = (self.y + self.h).max(o.y + o.h);
        Rect {
            x: x1,
            y: y1,
            w: x2 - x1,
            h: y2 - y1,
        }
    }
    /// Clamp into the screen; zero-area when fully outside.
    pub fn clamp_to(&self, sw: i32, sh: i32) -> Rect {
        let x = self.x.clamp(0, sw);
        let y = self.y.clamp(0, sh);
        Rect {
            x,
            y,
            w: (self.w).clamp(0, sw - x),
            h: (self.h).clamp(0, sh - y),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// What to repaint on the next EVT_SHOW instead of a full redraw.
/// Set by worker threads, consumed by the GUI handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    Message,
    Upload,
    Wifi,
    Log,
    /// Full redraw (e.g. upload finished — guarantees a clean final frame).
    Full,
}

/// Geometry of the status tab. Mirrors draw_status() flow exactly:
/// title(44) + state(64) + url(200) + wifi(54), then QR+hint or steps(130),
/// then upload line(110) + bar(30). `qr_size_px = 0` selects steps mode.
pub struct StatusLayout {
    pub title: Rect,
    pub state: Rect,
    pub url: Rect,
    pub wifi: Rect,
    /// Steps text rect, or bounding box of QR square + hint.
    pub mid: Rect,
    /// Centered QR square itself (None in steps mode).
    pub qr: Option<Rect>,
    pub upload_line: Option<Rect>,
    pub upload_bar: Option<Rect>,
    pub message: Rect,
}

pub fn status_layout(
    w: i32,
    h: i32,
    qr_size_px: i32,
    has_upload_line: bool,
    has_upload_bar: bool,
) -> StatusLayout {
    let fw = (w - 2 * GAP).max(0);
    let mut y = 16;
    let title = Rect {
        x: GAP,
        y,
        w: fw,
        h: 44,
    };
    y += 52;
    let state = Rect {
        x: GAP,
        y,
        w: fw,
        h: 64,
    };
    y += 72;
    let url = Rect {
        x: GAP,
        y,
        w: fw,
        h: 200,
    };
    y += 208;
    let wifi = Rect {
        x: GAP,
        y,
        w: fw,
        h: 54,
    };
    y += 62;
    let (mid, qr) = if qr_size_px > 0 {
        let qh = qr_size_px + 8 + 44;
        let r = Rect {
            x: GAP,
            y,
            w: fw,
            h: qh,
        };
        let q = Rect {
            x: (w - qr_size_px) / 2,
            y,
            w: qr_size_px,
            h: qr_size_px,
        };
        y += qh + 8;
        (r, Some(q))
    } else {
        let r = Rect {
            x: GAP,
            y,
            w: fw,
            h: 130,
        };
        y += 138;
        (r, None)
    };
    let mut upload_line = None;
    let mut upload_bar = None;
    if has_upload_line {
        upload_line = Some(Rect {
            x: GAP,
            y,
            w: fw,
            h: 110,
        });
        y += 112;
        if has_upload_bar {
            upload_bar = Some(Rect {
                x: GAP,
                y,
                w: fw,
                h: 30,
            });
            y += 38;
        }
    }
    let _ = y;
    StatusLayout {
        title,
        state,
        url,
        wifi,
        mid,
        qr,
        upload_line,
        upload_bar,
        message: message_rect(w, h),
    }
}

/// Union of upload line + bar (single PartialUpdate for progress ticks).
pub fn upload_union(l: &StatusLayout) -> Option<Rect> {
    match (l.upload_line, l.upload_bar) {
        (Some(a), Some(b)) => Some(a.union(&b)),
        (Some(a), None) => Some(a),
        _ => None,
    }
}

pub struct FilesLayout {
    pub dir: Rect,
    pub rows: Rect,
    pub per_page: usize,
}

pub fn files_layout(w: i32, h: i32) -> FilesLayout {
    let per_page = files_per_page(h);
    FilesLayout {
        dir: Rect {
            x: GAP,
            y: 16,
            w: (w - 2 * GAP).max(0),
            h: 56,
        },
        rows: Rect {
            x: GAP,
            y: LIST_Y0,
            w: (w - 2 * GAP).max(0),
            h: per_page as i32 * ROW_H,
        },
        per_page,
    }
}

/// Full highlight rect of file row `i` (matches draw_file_rows).
pub fn row_rect(i: usize, w: i32) -> Rect {
    Rect {
        x: GAP,
        y: LIST_Y0 + i as i32 * ROW_H,
        w: (w - 2 * GAP).max(0),
        h: ROW_H - 6,
    }
}

pub fn log_rect(w: i32, h: i32) -> Rect {
    Rect {
        x: GAP,
        y: 16,
        w: (w - 2 * GAP).max(0),
        h: (h - 16 - BOTTOM_H - 16).max(0),
    }
}

pub fn message_rect(w: i32, h: i32) -> Rect {
    Rect {
        x: GAP,
        y: h - BOTTOM_H - 52,
        w: (w - 2 * GAP).max(0),
        h: 48,
    }
}

/// Button hitbox as a repaint rect.
pub fn btn_rect(b: &Btn) -> Rect {
    Rect {
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_cycles() {
        let mut s = UiState::default();
        s.next_tab();
        assert_eq!(s.tab, Tab::Files);
        s.next_tab();
        assert_eq!(s.tab, Tab::Log);
        s.next_tab();
        assert_eq!(s.tab, Tab::Status);
    }
    #[test]
    fn pager_sane() {
        assert!(files_per_page(1024) >= 4);
        assert!(files_per_page(1448) >= 6);
    }
    #[test]
    fn nav_clamps_and_pages() {
        let mut s = UiState::default();
        s.move_sel(25, 30, 10);
        assert_eq!(s.selection, 24);
        assert_eq!(s.files_page, 2);
        s.move_sel(25, -100, 10);
        assert_eq!(s.selection, 0);
        assert_eq!(UiState::parent_dir("int:/a/b"), Some("int:/a".into()));
        assert_eq!(UiState::parent_dir("int:/a"), Some("int:/".into()));
        assert_eq!(UiState::parent_dir("int:/"), None);
    }
    #[test]
    fn buttons_cover_bottom_and_hit() {
        // 1072x1448 (PB633): 3 tall buttons
        let bs = bottom_buttons(Tab::Status, false, 1072, 1448);
        assert_eq!(bs.len(), 3);
        assert!(bs.iter().all(|b| b.h >= 100));
        assert_eq!(bs[0].label, "СТАРТ");
        assert_eq!(bs[1].label, "ФАЙЛЫ");
        assert_eq!(hit_button(&bs, 100, 1448 - 60), Some(BtnId::Primary));
        assert_eq!(hit_button(&bs, 536, 1448 - 60), Some(BtnId::Tabs));
        assert_eq!(hit_button(&bs, 1000, 1448 - 60), Some(BtnId::Exit));
        assert_eq!(hit_button(&bs, 10, 10), None);
        let bs_on = bottom_buttons(Tab::Status, true, 1072, 1448);
        assert_eq!(bs_on[0].label, "СТОП");
        let bf = bottom_buttons(Tab::Files, false, 1072, 1448);
        assert_eq!(bf[0].label, "ВВЕРХ");
        assert_eq!(bf[1].label, "ЖУРНАЛ");
        let bl = bottom_buttons(Tab::Log, true, 1072, 1448);
        assert_eq!(bl.len(), 3);
        assert_eq!(bl[1].label, "СТАТУС");
    }
    #[test]
    fn row_hit_testing() {
        // 5 rows on page starting at 0, 8 total: rows 0..5 hittable
        assert_eq!(row_at(LIST_Y0 + 10, 0, 5, 8), Some(0));
        assert_eq!(row_at(LIST_Y0 + 4 * ROW_H + 63, 0, 5, 8), Some(4));
        assert_eq!(row_at(LIST_Y0 + 5 * ROW_H, 0, 5, 8), None);
        assert_eq!(row_at(LIST_Y0, 0, 5, 8), Some(0));
        assert_eq!(row_at(10, 0, 5, 8), None);
        // last partial page: only 3 rows exist
        assert_eq!(row_at(LIST_Y0 + 2 * ROW_H, 5, 5, 8), Some(7));
        assert_eq!(row_at(LIST_Y0 + 3 * ROW_H, 5, 5, 8), None);
    }
    #[test]
    fn qr_layout_math() {
        // v2 code (25 modules) + 8 quiet = 33 units; capped at scale 10
        let l = qr_layout(25, 1040, 700).unwrap();
        assert_eq!(l.scale, 10);
        assert_eq!(l.size_px, 330);
        // tight box shrinks scale, keeps quiet zone inside size_px
        let l = qr_layout(29, 200, 200).unwrap();
        assert_eq!(l.scale, 200 / 37);
        assert_eq!(l.size_px, 37 * l.scale);
        // too small -> None, garbage -> None
        assert!(qr_layout(29, 50, 50).is_none());
        assert!(qr_layout(0, 500, 500).is_none());
        assert!(qr_layout(25, 0, 500).is_none());
    }
    #[test]
    fn status_layout_steps_mode() {
        // PB633 1072x1448, no QR, no upload: exact y-flow from draw_status
        let l = status_layout(1072, 1448, 0, false, false);
        assert_eq!(l.title, Rect { x: 16, y: 16, w: 1040, h: 44 });
        assert_eq!(l.state, Rect { x: 16, y: 68, w: 1040, h: 64 });
        assert_eq!(l.url, Rect { x: 16, y: 140, w: 1040, h: 200 });
        assert_eq!(l.wifi, Rect { x: 16, y: 348, w: 1040, h: 54 });
        assert_eq!(l.mid, Rect { x: 16, y: 410, w: 1040, h: 130 });
        assert!(l.qr.is_none());
        assert!(l.upload_line.is_none());
        assert_eq!(l.message, Rect { x: 16, y: 1246, w: 1040, h: 48 });
    }
    #[test]
    fn status_layout_qr_upload_mode() {
        // serving with 330px QR + progress bar
        let l = status_layout(1072, 1448, 330, true, true);
        assert_eq!(l.mid, Rect { x: 16, y: 410, w: 1040, h: 382 });
        assert_eq!(l.qr, Some(Rect { x: 371, y: 410, w: 330, h: 330 }));
        assert_eq!(l.upload_line, Some(Rect { x: 16, y: 800, w: 1040, h: 110 }));
        assert_eq!(l.upload_bar, Some(Rect { x: 16, y: 912, w: 1040, h: 30 }));
        // upload block ends (942) well above the fixed message line (1246)
        let u = upload_union(&l).unwrap();
        assert_eq!(u, Rect { x: 16, y: 800, w: 1040, h: 142 });
        assert!(u.y + u.h <= l.message.y);
        assert!(upload_union(&status_layout(1072, 1448, 0, false, false)).is_none());
    }
    #[test]
    fn files_log_message_rects() {
        let f = files_layout(1072, 1448);
        assert_eq!(f.dir, Rect { x: 16, y: 16, w: 1040, h: 56 });
        assert_eq!(f.rows.y, LIST_Y0);
        assert_eq!(f.rows.h, f.per_page as i32 * ROW_H);
        // row_rect matches row_at hit-testing
        assert_eq!(row_rect(2, 1072).y, LIST_Y0 + 2 * ROW_H);
        assert_eq!(row_rect(2, 1072).h, ROW_H - 6);
        // message sits above the buttons, log rect inside the screen
        let m = message_rect(1072, 1448);
        let bs = bottom_buttons(Tab::Status, false, 1072, 1448);
        assert!(m.y + m.h <= bs[0].y);
        let l = log_rect(1072, 1448);
        assert!(l.y + l.h <= bs[0].y);
        // rect utils
        let a = Rect { x: 10, y: 10, w: 20, h: 20 };
        let b = Rect { x: 25, y: 25, w: 20, h: 20 };
        assert_eq!(a.union(&b), Rect { x: 10, y: 10, w: 35, h: 35 });
        assert_eq!(Rect { x: -5, y: 1400, w: 2000, h: 200 }.clamp_to(1072, 1448),
            Rect { x: 0, y: 1400, w: 1072, h: 48 });
    }
}
