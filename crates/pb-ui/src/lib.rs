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
            label: "ЭКРАН".to_string(),
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
        assert_eq!(hit_button(&bs, 100, 1448 - 60), Some(BtnId::Primary));
        assert_eq!(hit_button(&bs, 536, 1448 - 60), Some(BtnId::Tabs));
        assert_eq!(hit_button(&bs, 1000, 1448 - 60), Some(BtnId::Exit));
        assert_eq!(hit_button(&bs, 10, 10), None);
        let bs_on = bottom_buttons(Tab::Status, true, 1072, 1448);
        assert_eq!(bs_on[0].label, "СТОП");
        let bf = bottom_buttons(Tab::Files, false, 1072, 1448);
        assert_eq!(bf[0].label, "ВВЕРХ");
        let bl = bottom_buttons(Tab::Log, true, 1072, 1448);
        assert_eq!(bl.len(), 3);
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
}
