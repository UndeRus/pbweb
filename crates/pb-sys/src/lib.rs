//! Minimal InkView FFI.
//!
//! Design: hard-require only symbols present in BOTH old (5.x) and new (6.x)
//! headers. Newer symbols (`NetInfo`, `NetConnect2`, `PostponeTimedPoweroff`,
//! `GetNetInfo`) are resolved at runtime via `dlsym` so the same binary
//! runs on PB633 (6.5) and older models without missing-symbol crashes.
#![allow(non_snake_case)]
#![allow(dead_code)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_ulong, c_void};

// ---- Events / keys (stable across versions, from inkview.h) ----
pub const EVT_INIT: c_int = 21;
pub const EVT_EXIT: c_int = 22;
pub const EVT_SHOW: c_int = 23;
pub const EVT_KEYPRESS: c_int = 25;
pub const EVT_POINTERUP: c_int = 29;
pub const EVT_POINTERDOWN: c_int = 30;
pub const EVT_POINTERMOVE: c_int = 31;
// Touchscreen events (FW 6.x header: 611/inkview.h).
// NOTE: 40/41/42 are EVT_KEYPRESS_EXT*, NOT touch — don't use those.
pub const EVT_TOUCHUP: c_int = 47;
pub const EVT_TOUCHDOWN: c_int = 48;
pub const EVT_TOUCHMOVE: c_int = 49;
// Filesystem notifications (FW 6.x header). NOTE: 72 is EVT_FSINCOMING,
// 73 is EVT_FSCHANGED — don't mix them up.
pub const EVT_FSINCOMING: c_int = 72;
pub const EVT_FSCHANGED: c_int = 73;
// Library scan service events (FW 6.5 header + firmware RE: the scanner.app
// event table answers 0xD6 with "Not Implemented", 0xD7 runs a device scan,
// 0xD8 is broadcast when the scan worker finishes).
// NOTE: EVT_STOPSCAN is intentionally never sent (server side is a no-op).
pub const EVT_STOPSCAN: c_int = 214;
pub const EVT_STARTSCAN: c_int = 215;
pub const EVT_SCANSTOPPED: c_int = 216;
/// SendEventTo target: broadcast to every task (monitor.app + services).
pub const TASK_BROADCAST: c_int = -3;

pub const KEY_PREV: c_int = 0x18;
pub const KEY_NEXT: c_int = 0x19;
pub const KEY_BACK: c_int = 0x1b;
pub const KEY_OK: c_int = 0x0a;
pub const KEY_MENU: c_int = 0x17;

pub const BLACK: c_int = 0x000000;
pub const DGRAY: c_int = 0x555555;
pub const LGRAY: c_int = 0xaaaaaa;
pub const WHITE: c_int = 0xffffff;

// Dialog icons (inkview.h)
pub const ICON_INFORMATION: c_int = 1;
pub const ICON_QUESTION: c_int = 2;

// DrawTextRect flags
pub const ALIGN_LEFT: c_int = 1;
pub const ALIGN_CENTER: c_int = 2;
pub const VALIGN_TOP: c_int = 16;
pub const VALIGN_MIDDLE: c_int = 32;

// Network events delivered to our handler by the system
pub const EVT_NET_CONNECTED: c_int = 256;
pub const EVT_NET_DISCONNECTED: c_int = 257;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IvNetinfo {
    pub connected: c_int,
    pub name: [c_char; 64],
    pub device: [c_char; 64],
    pub security: [c_char; 64],
    pub prefix: [c_char; 64],
    pub index: c_int,
    pub atime: c_int,
    pub speed: c_int,
    pub reserved: c_int,
    pub bytes_in: c_ulong,
    pub bytes_out: c_ulong,
    pub packets_in: c_ulong,
    pub packets_out: c_ulong,
}

pub type IvHandler = Option<unsafe extern "C" fn(c_int, c_int, c_int) -> c_int>;

#[cfg(feature = "device")]
#[link(name = "inkview")]
extern "C" {
    pub fn InkViewMain(h: IvHandler);
    pub fn CloseApp();
    pub fn ScreenWidth() -> c_int;
    pub fn ScreenHeight() -> c_int;
    pub fn ClearScreen();
    pub fn FullUpdate();
    pub fn PartialUpdate(x: c_int, y: c_int, w: c_int, h: c_int);
    pub fn FillArea(x: c_int, y: c_int, w: c_int, h: c_int, color: c_int);
    pub fn DrawRect(x: c_int, y: c_int, w: c_int, h: c_int, color: c_int);
    pub fn SetPanelType(t: c_int);
    pub fn OpenFont(name: *const c_char, size: c_int, aa: c_int) -> *mut c_void;
    pub fn CloseFont(f: *mut c_void);
    pub fn SetFont(f: *mut c_void, color: c_int);
    pub fn DrawTextRect(
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
        s: *const c_char,
        flags: c_int,
    ) -> *mut c_char;
    pub fn DrawString(x: c_int, y: c_int, s: *const c_char);
    pub fn Message(icon: c_int, title: *const c_char, text: *const c_char, timeout: c_int);
    pub fn DialogSynchro(
        icon: c_int,
        title: *const c_char,
        text: *const c_char,
        b1: *const c_char,
        b2: *const c_char,
        b3: *const c_char,
    ) -> c_int;
    pub fn QueryNetwork() -> c_int;
    pub fn NetConnect(name: *const c_char) -> c_int;
    pub fn NetDisconnect();
    pub fn GetHwAddress() -> *mut c_char;
    pub fn iv_sync();
    // NOTE: no hard-linked GetTouchInfo here on purpose: SDK 6.5 exports
    // only GetTouchInfoI(int), older FWs export GetTouchInfo(void).
    // Resolved at runtime via dlsym, see touch_slot() below.
    pub fn SendEvent(h: IvHandler, t: c_int, p1: c_int, p2: c_int);
    /// Deliver an event to another task by id (symbol verified present in
    /// 6.5 libinkview via nm). Used for the EVT_STARTSCAN broadcast to the
    /// resident scanner.app service.
    pub fn SendEventTo(task: c_int, t: c_int, p1: c_int, p2: c_int) -> c_int;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TouchInfo {
    pub x: c_int,
    pub y: c_int,
    pub pressure: c_int,
    pub id: c_int,
}

// ---- Stubs for host builds (cargo test on PC) ----
#[cfg(not(feature = "device"))]
pub unsafe fn InkViewMain(_h: IvHandler) {}
#[cfg(not(feature = "device"))]
pub unsafe fn CloseApp() {}
#[cfg(not(feature = "device"))]
pub unsafe fn ScreenWidth() -> c_int {
    758
}
#[cfg(not(feature = "device"))]
pub unsafe fn ScreenHeight() -> c_int {
    1024
}
#[cfg(not(feature = "device"))]
pub unsafe fn ClearScreen() {}
#[cfg(not(feature = "device"))]
pub unsafe fn FullUpdate() {}
#[cfg(not(feature = "device"))]
pub unsafe fn PartialUpdate(_x: c_int, _y: c_int, _w: c_int, _h: c_int) {}
#[cfg(not(feature = "device"))]
pub unsafe fn FillArea(_x: c_int, _y: c_int, _w: c_int, _h: c_int, _c: c_int) {}
#[cfg(not(feature = "device"))]
pub unsafe fn DrawRect(_x: c_int, _y: c_int, _w: c_int, _h: c_int, _c: c_int) {}
#[cfg(not(feature = "device"))]
pub unsafe fn SetPanelType(_t: c_int) {}
#[cfg(not(feature = "device"))]
pub unsafe fn OpenFont(_n: *const c_char, _s: c_int, _a: c_int) -> *mut c_void {
    std::ptr::null_mut()
}
#[cfg(not(feature = "device"))]
pub unsafe fn CloseFont(_f: *mut c_void) {}
#[cfg(not(feature = "device"))]
pub unsafe fn SetFont(_f: *mut c_void, _c: c_int) {}
#[cfg(not(feature = "device"))]
pub unsafe fn DrawTextRect(
    _x: c_int,
    _y: c_int,
    _w: c_int,
    _h: c_int,
    _s: *const c_char,
    _f: c_int,
) -> *mut c_char {
    std::ptr::null_mut()
}
#[cfg(not(feature = "device"))]
pub unsafe fn QueryNetwork() -> c_int {
    0
}
#[cfg(not(feature = "device"))]
pub unsafe fn SendEventTo(_task: c_int, _t: c_int, _p1: c_int, _p2: c_int) -> c_int {
    -1
}
#[cfg(not(feature = "device"))]
pub fn scan_flag() -> Option<bool> {
    None
}
#[cfg(not(feature = "device"))]
pub fn db_changes() -> Option<u32> {
    None
}
#[cfg(not(feature = "device"))]
pub unsafe fn NetDisconnect() {}
#[cfg(not(feature = "device"))]
pub unsafe fn iv_sync() {}

// ---- Optional newer symbols via dlsym (never link hard) ----
#[cfg(feature = "device")]
mod opt {
    use super::*;
    use std::ptr;

    #[link(name = "dl")]
    extern "C" {
        fn dlopen(name: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(h: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    const RTLD_NOW: c_int = 2;

    fn sym(name: &str) -> *mut c_void {
        unsafe {
            // NOTE: name must NOT contain a trailing '\0' — CString adds it.
            // (A previous version passed b"NetInfo\0" here, which made
            // CString::new fail and panicked on every WiFi attempt.)
            let cname = CString::new(name).unwrap_or_default();
            let h = dlopen(ptr::null(), RTLD_NOW);
            if h.is_null() {
                return ptr::null_mut();
            }
            dlsym(h, cname.as_ptr())
        }
    }

    /// Firmware 6.x: iv_netinfo* NetInfo(). Full struct copy (layout from
    /// SDK 6.5 sysroot inkview.h). None if the symbol is missing.
    pub fn netinfo_full() -> Option<super::IvNetinfo> {
        let p = sym("NetInfo");
        if p.is_null() {
            return None;
        }
        let f: extern "C" fn() -> *mut super::IvNetinfo = unsafe { std::mem::transmute(p) };
        let info = (f)();
        if info.is_null() {
            return None;
        }
        Some(unsafe { *info })
    }

    /// Newer firmwares: iv_netinfo* NetInfo(); 0 in `connected` == offline.
    /// Returns None if symbol missing or offline.
    pub fn netinfo_connected() -> Option<bool> {
        netinfo_full().map(|n| n.connected != 0)
    }

    /// Firmware 6.x: int NetConnectSilent(const char*) — connect without
    /// dialogs (NULL = last network). Returns NET_OK(0) on success.
    /// May block briefly; call from a worker thread only.
    pub fn net_connect_silent() -> Option<c_int> {
        let p = sym("NetConnectSilent");
        if p.is_null() {
            return None;
        }
        let f: extern "C" fn(*const c_char) -> c_int = unsafe { std::mem::transmute(p) };
        Some((f)(ptr::null()))
    }

    /// Status values delivered to the NetConnectAsync callback (NET_*).
    /// We only log them; polling NetInfo is the source of truth.
    pub type NetAsyncCb = extern "C" fn(c_int) -> c_int;

    /// Firmware 6.x: int NetConnectAsync(int (*cb)(int status)).
    /// Non-blocking: returns immediately, connection proceeds in background
    /// (system events EVT_NET_CONNECTED/DISCONNECTED follow).
    /// Returns None if the symbol is missing.
    pub fn net_connect_async(cb: NetAsyncCb) -> Option<c_int> {
        let p = sym("NetConnectAsync");
        if p.is_null() {
            return None;
        }
        let f: extern "C" fn(NetAsyncCb) -> c_int = unsafe { std::mem::transmute(p) };
        Some((f)(cb))
    }

    /// Newer firmwares: int NetConnect2(const char*, int show_hourglass).
    /// BLOCKING with system UI — last resort only.
    pub fn net_connect2_last() -> Option<c_int> {
        let p = sym("NetConnect2");
        if p.is_null() {
            return None;
        }
        let f: extern "C" fn(*const c_char, c_int) -> c_int = unsafe { std::mem::transmute(p) };
        Some((f)(ptr::null(), 1))
    }

    /// Keep device awake during transfers if present.
    pub fn postpone_poweroff() {
        let p = sym("PostponeTimedPoweroff");
        if p.is_null() {
            return;
        }
        let f: extern "C" fn() = unsafe { std::mem::transmute(p) };
        (f)();
    }

    /// Touch coordinates, FW-agnostic: SDK 6.5 has GetTouchInfoI(int slot),
    /// older firmwares have GetTouchInfo(void). Returns None if neither
    /// resolves (caller falls back to event par1/par2).
    pub fn touch_slot() -> Option<(c_int, c_int)> {        unsafe {
            let p = sym("GetTouchInfoI");
            if !p.is_null() {
                let f: extern "C" fn(c_int) -> *mut super::TouchInfo =
                    std::mem::transmute(p);
                let ti = (f)(0);
                if !ti.is_null() {
                    return Some(((*ti).x, (*ti).y));
                }
            }
            let p = sym("GetTouchInfo");
            if !p.is_null() {
                let f: extern "C" fn() -> *mut super::TouchInfo = std::mem::transmute(p);
                let ti = (f)();
                if !ti.is_null() {
                    return Some(((*ti).x, (*ti).y));
                }
            }
        }
        None
    }

    /// Offsets into the ivmpc global state block (libinkview BSS export).
    /// FIRMWARE-SPECIFIC: verified on U633 6.5.2915 by firmware RE;
    /// may differ on other FW versions — treat results as best-effort.
    const IVMPC_SCAN_FLAG_OFF: usize = 0x4C;
    const IVMPC_DB_CHANGES_OFF: usize = 0x41DC;

    fn ivmpc_base() -> *mut u8 {
        sym("ivmpc") as *mut u8
    }

    /// Nonzero while scanner.app is running a library scan. None if the
    /// symbol is missing (pure diagnostic helper, plain aligned loads).
    pub fn scan_flag() -> Option<bool> {
        let base = ivmpc_base();
        if base.is_null() {
            return None;
        }
        Some(unsafe { (base.add(IVMPC_SCAN_FLAG_OFF) as *const i16).read_unaligned() } != 0)
    }

    /// DB-changes counter bumped by the scanner. Compare before/after a
    /// scan broadcast to prove the library DB actually changed.
    pub fn db_changes() -> Option<u32> {
        let base = ivmpc_base();
        if base.is_null() {
            return None;
        }
        Some(unsafe { (base.add(IVMPC_DB_CHANGES_OFF) as *const u32).read_unaligned() })
    }
}

#[cfg(feature = "device")]
pub use opt::{
    db_changes, net_connect2_last, net_connect_async, net_connect_silent, netinfo_connected,
    netinfo_full, postpone_poweroff, scan_flag, touch_slot, NetAsyncCb,
};

#[cfg(not(feature = "device"))]
pub fn netinfo_connected() -> Option<bool> {
    None
}
#[cfg(not(feature = "device"))]
pub fn touch_slot() -> Option<(c_int, c_int)> {
    None
}
#[cfg(not(feature = "device"))]
pub fn netinfo_full() -> Option<IvNetinfo> {
    None
}
#[cfg(not(feature = "device"))]
pub type NetAsyncCb = extern "C" fn(c_int) -> c_int;
#[cfg(not(feature = "device"))]
pub fn net_connect_async(_cb: NetAsyncCb) -> Option<c_int> {
    None
}
#[cfg(not(feature = "device"))]
pub fn net_connect_silent() -> Option<c_int> {
    None
}
#[cfg(not(feature = "device"))]
pub fn net_connect2_last() -> Option<c_int> {
    None
}
#[cfg(not(feature = "device"))]
pub fn postpone_poweroff() {}

/// Unified "ensure wifi": prefer NetConnect2/NetInfo, fallback to
/// legacy NetConnect(NULL) + QueryNetwork. Returns true if online.
pub fn ensure_wifi() -> bool {
    postpone_poweroff();
    #[cfg(feature = "device")]
    unsafe {
        if let Some(c) = netinfo_connected() {
            if c {
                return true;
            }
        } else if QueryNetwork() != 0 {
            return true;
        }
        if let Some(rc) = net_connect2_last() {
            if rc == 0 {
                return netinfo_connected().unwrap_or(true);
            }
        }
        // legacy: NULL = last known network
        if NetConnect(std::ptr::null()) == 0 {
            return true;
        }
        QueryNetwork() != 0
    }
    #[cfg(not(feature = "device"))]
    {
        false
    }
}

pub fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

/// # Safety: ptr from GetHwAddress / DrawTextRect
pub unsafe fn take_c_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_owned())
}

// ---- LAN IP via libc getifaddrs (no extra deps, Linux only) ----
#[cfg(target_os = "linux")]
#[repr(C)]
struct IfAddrs {
    next: *mut IfAddrs,
    name: *const c_char,
    flags: std::os::raw::c_uint,
    addr: *const SockAddrIn,
    netmask: *const SockAddrIn,
    ifu: *const c_void,
    data: *mut c_void,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct SockAddrIn {
    family: u16,
    port: u16,
    ip: [u8; 4],
    _zero: [u8; 8],
}

#[cfg(target_os = "linux")]
extern "C" {
    fn getifaddrs(ifap: *mut *mut IfAddrs) -> c_int;
    fn freeifaddrs(ifa: *mut IfAddrs);
}

/// Real device LAN IP. Prefers wlan*, then eth* (PocketBook wifi is eth0),
/// then the first non-loopback IPv4. Works on host Linux and on the reader.
#[cfg(target_os = "linux")]
pub fn lan_ip() -> Option<String> {
    unsafe {
        let mut head: *mut IfAddrs = std::ptr::null_mut();
        if getifaddrs(&mut head) != 0 || head.is_null() {
            return None;
        }
        let mut eth: Option<String> = None;
        let mut fallback: Option<String> = None;
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.addr.is_null() && (*ifa.addr).family == 2 /* AF_INET */ {
                let ip = (*ifa.addr).ip;
                if ip[0] != 127 {
                    let name = CStr::from_ptr(ifa.name).to_string_lossy();
                    let s = format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                    if name.starts_with("wlan") {
                        freeifaddrs(head);
                        return Some(s);
                    }
                    if name.starts_with("eth") && eth.is_none() {
                        eth = Some(s);
                    } else if fallback.is_none() {
                        fallback = Some(s);
                    }
                }
            }
            cur = (*cur).next;
        }
        freeifaddrs(head);
        eth.or(fallback)
    }
}

#[cfg(not(target_os = "linux"))]
pub fn lan_ip() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Event/task constants must match the firmware (SDK 6.5 header +
    /// firmware RE: 0xD6 STOPSCAN / 0xD7 STARTSCAN / 0xD8 SCANSTOPPED,
    /// broadcast task 0xFFFFFFFD).
    #[test]
    fn scan_bus_constants() {
        assert_eq!(EVT_STOPSCAN, 0xD6);
        assert_eq!(EVT_STARTSCAN, 0xD7);
        assert_eq!(EVT_SCANSTOPPED, 0xD8);
        assert_eq!(TASK_BROADCAST, -3);
    }
}
