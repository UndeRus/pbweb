//! tiny_http file server: JSON API + static single-file web UI (Crosspoint-style).
//! Routes:
//!   GET  / , /files               -> embedded index.html (SPA: home + file manager)
//!   GET  /api/status              -> {version, ip, mode, device, wifi}
//!   GET  /api/roots               -> {"internal":bool,"sdcard":bool}
//!   GET  /api/files?path=int:/x   -> [FileEntry]
//!   GET  /api/download?path=...   -> file bytes (single shot)
//!   GET  /api/zip?path=int:/dir   -> folder as .zip (stored, no compression)
//!   POST /api/mkdir               -> {"path","name"}
//!   POST /api/rename              -> {"path","name"}
//!   POST /api/move                -> {"path","dest"} (dest = existing folder, same root)
//!   POST /api/delete              -> {"path"} or {"paths":[...]}
//!   POST /api/upload?path=int:/x  -> multipart/form-data (one or many files)

use pb_core::{is_root, list_dir, resolve_confined, valid_filename, Roots};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

static INDEX_HTML: &str = include_str!("../../../web/index.html");

pub fn now_ms() -> u64 {    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Human size: 850 -> "850 Б", 2048 -> "2,0 КБ", 5M -> "5,0 МБ".
pub fn format_size(n: u64) -> String {
    let s = if n >= 1 << 30 {
        format!("{:.1} ГБ", n as f64 / (1 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} МБ", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} КБ", n as f64 / 1024.0)
    } else {
        return format!("{n} Б");
    };
    s.replace('.', ",")
}

#[derive(Debug, Default)]
pub struct UploadState {
    pub file: Mutex<String>,
    pub received: AtomicU64,
    pub total: AtomicU64,
    pub started_ms: AtomicU64,
    pub done_msg: Mutex<String>,
    pub active: AtomicBool,
    /// Set when an upload finishes (ok or fail) so the UI paints one
    /// final clean frame. Consumed by the progress hook wrapper.
    pub finished: AtomicBool,
    last_hook_ms: AtomicU64,
}

impl UploadState {
    fn start(&self, total: u64) {
        *self.file.lock().unwrap() = String::new();
        self.received.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.started_ms.store(now_ms(), Ordering::Relaxed);
        *self.done_msg.lock().unwrap() = String::new();
        self.active.store(true, Ordering::Relaxed);
        self.finished.store(false, Ordering::Relaxed);
    }
    fn reset(&self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub uploaded_bytes: AtomicU64,
    pub log: Mutex<Vec<String>>,
    pub upload: UploadState,
    pub hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Failed auth attempts (brute-force guard).
    pub auth_fails: AtomicU64,
    /// Epoch ms until which auth attempts are rejected.
    pub blocked_until_ms: AtomicU64,
    /// Epoch ms of the last served request (idle auto-stop in pb-app).
    pub last_activity_ms: AtomicU64,
}

const LOG_RING: usize = 200;
/// Fails before progressive block kicks in.
const AUTH_FAILS_BEFORE_BLOCK: u64 = 5;
/// Base block after the free attempts, doubles per extra fail, capped.
const AUTH_BLOCK_BASE_MS: u64 = 30_000;
const AUTH_BLOCK_MAX_MS: u64 = 600_000;

/// 6-digit pairing PIN as shown on the e-ink screen. Randomness comes from
/// the OS (/dev/urandom on device/Linux); fallback is time+pid hash.
/// std-only on purpose — no new dependencies for the ARM build.
/// NOTE: read EXACTLY 4 bytes — never fs::read() the whole file, because
/// /dev/urandom has no EOF and read-to-end would loop forever eating RAM.
pub fn gen_pin() -> String {
    let rnd: u32 = std::fs::File::open("/dev/urandom")
        .ok()
        .and_then(|mut f| pin_from_reader(&mut f))
        .unwrap_or_else(|| {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9e3779b97f4a7c15);
            let pid = std::process::id() as u64;
            // splitmix64, good enough for a LAN pairing code fallback
            let mut z = t.wrapping_add(pid).wrapping_add(0x9e3779b97f4a7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            (z ^ (z >> 31)) as u32
        });
    format!("{:06}", rnd % 1_000_000)
}

/// Read exactly 4 bytes of entropy. Split out so tests can prove we never
/// read-to-end (which would hang forever on EOF-less files like urandom).
fn pin_from_reader(r: &mut dyn std::io::Read) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

/// Constant-time string compare (no early exit on content).
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl Stats {
    pub fn push(&self, line: String) {
        let mut l = self.log.lock().unwrap();
        l.push(line);
        while l.len() > LOG_RING {
            l.remove(0);
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
    }
    pub fn tail(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    /// Redraw/progress callback (set by the device UI). Throttled to 2/sec.
    pub fn set_hook(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.hook.lock().unwrap() = Some(f);
    }
    pub fn fire_hook(&self) {
        let now = now_ms();
        let last = self.upload.last_hook_ms.load(Ordering::Relaxed);
        if now.wrapping_sub(last) < 500 {
            return;
        }
        self.upload.last_hook_ms.store(now, Ordering::Relaxed);
        if let Some(h) = self.hook.lock().unwrap().clone() {
            (h)();
        }
    }
    /// Unthrottled redraw (upload start/finish — otherwise the final state
    /// can be swallowed by the throttle and the screen sticks mid-progress).
    pub fn fire_hook_forced(&self) {
        self.upload.last_hook_ms.store(0, Ordering::Relaxed);
        self.fire_hook();
    }
    /// (active, file, received, total, started_ms, done_msg)
    pub fn upload_snapshot(&self) -> (bool, String, u64, u64, u64, String) {
        let u = &self.upload;
        (
            u.active.load(Ordering::Relaxed),
            u.file.lock().unwrap().clone(),
            u.received.load(Ordering::Relaxed),
            u.total.load(Ordering::Relaxed),
            u.started_ms.load(Ordering::Relaxed),
            u.done_msg.lock().unwrap().clone(),
        )
    }
    /// Ready-to-draw upload line + percent (None when total unknown or idle).
    /// Active: "↑ name\n45% • 2,4 МБ/с". Done: "✓ names (size)". Idle: "".
    pub fn upload_display(&self) -> (String, Option<u8>) {
        let (active, file, rec, tot, started, done) = self.upload_snapshot();
        if active {
            let pct = if tot > 0 {
                Some((rec.saturating_mul(100) / tot).min(100) as u8)
            } else {
                None
            };
            let el = (now_ms().saturating_sub(started).max(200) as f64) / 1000.0;
            let speed = format_size((rec as f64 / el) as u64);
            let name = if file.is_empty() {
                "данные".to_string()
            } else {
                file
            };
            let line = match pct {
                Some(p) => format!("↑ {name}\n{p}% • {speed}/с"),
                None => format!("↑ {name}\n{} • {speed}/с", format_size(rec)),
            };
            (line, pct)
        } else if !done.is_empty() {
            (done, None)
        } else {
            (String::new(), None)
        }
    }
}

pub struct Server {
    pub roots: Roots,
    pub stats: Arc<Stats>,
    live: Mutex<Option<Arc<tiny_http::Server>>>,
    /// Pairing PIN shown on the e-ink screen. Fresh per Server instance,
    /// i.e. per СТАРТ; STOP drops it with the server object.
    pub auth_token: String,
}

impl Server {
    pub fn new(roots: Roots) -> Self {
        Self {
            roots,
            stats: Arc::new(Stats::default()),
            live: Mutex::new(None),
            auth_token: gen_pin(),
        }
    }

    /// Check the pairing token from `?token=` or `X-Auth-Token`.
    /// Wrong guesses count towards a progressive block (30s doubling,
    /// capped at 10 min) so the 6-digit space can't be brute-forced
    /// over LAN. A correct token resets the counters.
    pub fn check_auth(&self, presented: Option<&str>) -> bool {
        let now = now_ms();
        if now < self.stats.blocked_until_ms.load(Ordering::Relaxed) {
            return false;
        }
        let ok = presented.map(|t| ct_eq(t, &self.auth_token)).unwrap_or(false);
        if ok {
            self.stats.auth_fails.store(0, Ordering::Relaxed);
            self.stats.blocked_until_ms.store(0, Ordering::Relaxed);
            true
        } else {
            let fails = self.stats.auth_fails.fetch_add(1, Ordering::Relaxed) + 1;
            if fails >= AUTH_FAILS_BEFORE_BLOCK {
                let shift = (fails - AUTH_FAILS_BEFORE_BLOCK).min(5);
                let block = (AUTH_BLOCK_BASE_MS << shift).min(AUTH_BLOCK_MAX_MS);
                self.stats
                    .blocked_until_ms
                    .store(now.saturating_add(block), Ordering::Relaxed);
            }
            false
        }
    }

    pub fn serve(&self, port: u16) -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
        let (srv, p) = self.bind_shared(port)?;
        self.run_shared(&srv);
        Ok(p)
    }

    /// Bind the first free port in port..port+10 without serving yet, so the
    /// caller can publish the URL before blocking in the accept loop.
    /// The handle is kept for shutdown().
    pub fn bind_shared(
        &self,
        port: u16,
    ) -> Result<(Arc<tiny_http::Server>, u16), Box<dyn std::error::Error + Send + Sync>> {
        for p in port..port + 10 {
            match tiny_http::Server::http(format!("0.0.0.0:{p}")) {
                Ok(srv) => {
                    let shared = Arc::new(srv);
                    *self.live.lock().unwrap() = Some(shared.clone());
                    return Ok((shared, p));
                }
                Err(e) => {
                    let msg = e.to_string();
                    if !msg.contains("in use") && !msg.contains("address") {
                        return Err(e);
                    }
                }
            }
        }
        Err("no free port".into())
    }

    /// Bind the first free port in port..port+10 without serving yet, so the
    /// caller can publish the URL before blocking in the accept loop.
    pub fn bind(
        &self,
        port: u16,
    ) -> Result<(tiny_http::Server, u16), Box<dyn std::error::Error + Send + Sync>> {
        // kept for API compat; prefer bind_shared (supports shutdown)
        for p in port..port + 10 {
            match tiny_http::Server::http(format!("0.0.0.0:{p}")) {
                Ok(srv) => return Ok((srv, p)),
                Err(e) => {
                    let msg = e.to_string();
                    if !msg.contains("in use") && !msg.contains("address") {
                        return Err(e);
                    }
                }
            }
        }
        Err("no free port".into())
    }

    /// Blocking accept loop for an already-bound server.
    pub fn run_on(&self, srv: tiny_http::Server) {
        self.run(srv)
    }

    /// Blocking accept loop for a shared server. Ends when shutdown() is
    /// called (tiny_http unblocks the waiter internally).
    pub fn run_shared(&self, srv: &Arc<tiny_http::Server>) {
        for mut req in srv.incoming_requests() {
            let url = req.url().to_owned();
            let method = req.method().as_str().to_owned();
            let peer = req
                .remote_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "-".into());
            let (resp, code, cleanup) = self.route(&mut req, &method, &url, &peer);
            let _ = req.respond(resp);
            if let Some(p) = cleanup {
                // temp transfer file (zip): response fully sent, drop it
                let _ = std::fs::remove_file(p);
            }
            self.stats
                .push(format!("{peer} {method} {url} -> {code}"));
        }
        *self.live.lock().unwrap() = None;
    }

    /// Stop a running server: unblocks the accept loop and closes the socket.
    /// Safe to call from another thread (e.g. a STOP button handler).
    pub fn shutdown(&self) {
        use std::time::Duration;
        let handle = self.live.lock().unwrap().take();
        if let Some(srv) = handle {
            let addr = srv.server_addr();
            srv.unblock(); // end our message loop
            drop(srv); // close=true; Drop's own self-wake fails on Windows,
            // so wake the accept thread ourselves via loopback (always works):
            // it then sees close=true, exits and releases the port.
            match addr {
                tiny_http::ListenAddr::IP(listen) => {
                    let loopback = std::net::SocketAddr::new(
                        std::net::IpAddr::from([127, 0, 0, 1]),
                        listen.port(),
                    );
                    if let Ok(s) =
                        std::net::TcpStream::connect_timeout(&loopback, Duration::from_secs(2))
                    {
                        let _ = s.shutdown(std::net::Shutdown::Both);
                    }
                }
                #[cfg(unix)]
                _ => {}
            }
        }
    }

    pub fn is_live(&self) -> bool {
        self.live.lock().unwrap().is_some()
    }

    fn run(&self, srv: tiny_http::Server) {
        for mut req in srv.incoming_requests() {
            let url = req.url().to_owned();
            let method = req.method().as_str().to_owned();
            let peer = req
                .remote_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "-".into());
            let (resp, code, cleanup) = self.route(&mut req, &method, &url, &peer);
            let _ = req.respond(resp);
            if let Some(p) = cleanup {
                let _ = std::fs::remove_file(p);
            }
            self.stats
                .push(format!("{peer} {method} {url} -> {code}"));
        }
    }

    fn route(
        &self,
        req: &mut tiny_http::Request,
        method: &str,
        url: &str,
        peer: &str,
    ) -> (tiny_http::ResponseBox, u16, Option<PathBuf>) {
        self.stats
            .last_activity_ms
            .store(now_ms(), Ordering::Relaxed);
        let (path, query) = match url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (url, ""),
        };
        let q = |k: &str| {
            query.split('&').find_map(|kv| {
                let (kk, vv) = kv.split_once('=')?;
                (kk == k).then(|| urlencoding::decode(vv).unwrap_or_default().into_owned())
            })
        };
        // Pairing gate: everything under /api/ needs the e-ink PIN,
        // via ?token= (links/QR) or X-Auth-Token (fetch). No details out.
        if path.starts_with("/api/") {
            let header_token = header_value(req, "x-auth-token");
            let presented = q("token").or(header_token);
            if !self.check_auth(presented.as_deref()) {
                self.stats.push(format!("{peer} auth fail {method} {url}"));
                return err(401, "unauthorized");
            }
            // Mutations additionally require same-origin (or no Origin,
            // e.g. curl). The token is authoritative; this kills drive-by
            // cross-site form/fetch abuse as defense in depth.
            if method == "POST" && !origin_ok(req) {
                self.stats.push(format!("{peer} origin reject {method} {url}"));
                return err(403, "forbidden");
            }
        }
        match (method, path) {
            ("GET", "/" | "/files") => (
                tiny_http::Response::from_string(INDEX_HTML)
                    .with_header(
                        "Content-Type: text/html; charset=utf-8"
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    )
                    .boxed(),
                200,
                None,
            ),
            ("GET", "/api/status") => {
                // Real interface IP first (getifaddrs on Linux/device),
                // UDP-route trick as fallback (works on Windows host).
                let ip = pb_sys::lan_ip()
                    .or_else(first_non_loopback_ip)
                    .unwrap_or_default();
                let v = serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "ip": ip,
                    "mode": "STA",
                    "device": "PocketBook",
                    "wifi": true,
                    "roots": {
                        "internal": true,
                        "sdcard": self.roots.sdcard.is_some(),
                    },
                });
                json(v)
            }
            ("GET", "/api/roots") => {
                let v = serde_json::json!({
                    "internal": true,
                    "sdcard": self.roots.sdcard.is_some(),
                });
                json(v)
            }
            ("GET", "/api/files") => {
                let p = q("path").unwrap_or_else(|| "int:/".into());
                let fq = pb_core::FileQuery {
                    q: q("q").unwrap_or_default(),
                    sort: match q("sort").as_deref() {
                        Some("size") => pb_core::SortKey::Size,
                        Some("mtime") => pb_core::SortKey::Mtime,
                        _ => pb_core::SortKey::Name,
                    },
                    order: match q("order").as_deref() {
                        Some("desc") => pb_core::SortOrder::Desc,
                        _ => pb_core::SortOrder::Asc,
                    },
                    limit: q("limit")
                        .and_then(|s| s.parse::<usize>().ok())
                        .map(|n| n.clamp(1, 1000))
                        .unwrap_or(200),
                    offset: q("offset")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0),
                };
                match resolve_confined(&self.roots, &p) {
                    Ok((id, full)) => {
                        let base = if id == "sd" {
                            self.roots.sdcard.clone().unwrap()
                        } else {
                            self.roots.internal.clone()
                        };
                        match list_dir(&base, &id, &full) {
                            Ok(items) => {
                                let (page, total) = pb_core::query_files(items, &fq);
                                json(serde_json::json!({
                                    "path": p,
                                    "offset": fq.offset,
                                    "limit": fq.limit,
                                    "total": total,
                                    "items": page,
                                }))
                            }
                            Err(e) => {
                                self.stats.push(format!("files failed: {e}"));
                                err(500, "internal error")
                            }
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/exists") => {
                // Batch existence check for the upload-overwrite confirm.
                // Body: {"dir": "int:/books", "names": ["a.epub", ...]}.
                // Answers only which names exist — never file contents.
                let v = read_json_body(req);
                let dir = v.get("dir").and_then(|x| x.as_str()).unwrap_or("int:/");
                let names: Vec<String> = v
                    .get("names")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str())
                            .take(500)
                            .map(|s| s.to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                match resolve_confined(&self.roots, dir) {
                    Ok((_, full)) => {
                        if !full.is_dir() {
                            return err(400, "not a folder");
                        }
                        let mut exists = Vec::new();
                        for nm in &names {
                            if nm.contains('/') || nm.contains('\\') || !valid_filename(nm) {
                                continue;
                            }
                            if full.join(nm).exists() {
                                exists.push(nm.clone());
                            }
                        }
                        json(serde_json::json!({ "exists": exists }))
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("GET", "/api/download") => {
                let p = match q("path") {
                    Some(p) => p,
                    None => return err(400, "path required"),
                };
                match resolve_confined(&self.roots, &p) {
                    Ok((_, full)) => download_file(&full),
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/mkdir") => {
                let v = read_json_body(req);
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("int:/");
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
                if !valid_filename(name) || name.contains('/') || name.contains('\\') {
                    return err(400, "bad name");
                }
                match resolve_confined(&self.roots, path) {
                    Ok((_, mut full)) => {
                        full.push(name);
                        match std::fs::create_dir(&full) {
                            Ok(_) => json(serde_json::json!({"ok": true})),
                            Err(e) => {
                                self.stats.push(format!("mkdir failed: {e}"));
                                err(500, "internal error")
                            }
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/rename") => {
                let v = read_json_body(req);
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
                if !valid_filename(name) || name.contains('/') || name.contains('\\') {
                    return err(400, "bad name");
                }
                match resolve_confined(&self.roots, path) {
                    Ok((id, full)) => {
                        if !full.exists() {
                            return err(404, "not found");
                        }
                        if is_root(&self.roots, &id, &full) {
                            return err(400, "cannot rename storage root");
                        }
                        let dest = match full.parent() {
                            Some(par) => par.join(name),
                            None => return err(400, "bad path"),
                        };
                        if dest.exists() {
                            return err(409, "already exists");
                        }
                        match std::fs::rename(&full, &dest) {
                            Ok(_) => json(serde_json::json!({"ok": true})),
                            Err(e) => {
                                self.stats.push(format!("rename failed: {e}"));
                                err(500, "internal error")
                            }
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/move") => {
                let v = read_json_body(req);
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let dest_dir = v.get("dest").and_then(|x| x.as_str()).unwrap_or("");
                let (src_id, src_full) = match resolve_confined(&self.roots, path) {
                    Ok(x) => x,
                    Err(e) => return err(403, &e),
                };
                let (dst_id, mut dst_full) = match resolve_confined(&self.roots, dest_dir) {
                    Ok(x) => x,
                    Err(e) => return err(403, &e),
                };
                if src_id != dst_id {
                    return err(400, "cross-root move not supported");
                }
                if is_root(&self.roots, &src_id, &src_full) {
                    return err(400, "cannot move storage root");
                }
                if !dst_full.is_dir() {
                    return err(400, "dest not a folder");
                }
                if let Some(name) = src_full.file_name() {
                    dst_full.push(name);
                } else {
                    return err(400, "bad path");
                }
                if dst_full.exists() {
                    return err(409, "already exists");
                }
                match std::fs::rename(&src_full, &dst_full) {
                    Ok(_) => json(serde_json::json!({"ok": true})),
                    Err(e) => {
                        self.stats.push(format!("move failed: {e}"));
                        err(500, "internal error")
                    }
                }
            }
            ("POST", "/api/delete") => {
                let v = read_json_body(req);
                let mut paths: Vec<String> = Vec::new();
                if let Some(arr) = v.get("paths").and_then(|x| x.as_array()) {
                    for x in arr {
                        if let Some(s) = x.as_str() {
                            paths.push(s.to_owned());
                        }
                    }
                } else if let Some(s) = v.get("path").and_then(|x| x.as_str()) {
                    if !s.is_empty() {
                        paths.push(s.to_owned());
                    }
                }
                if paths.is_empty() {
                    return err(400, "path required");
                }
                // refuse storage roots up front (deterministic 400, nothing touched)
                for p in &paths {
                    if let Ok((id, full)) = resolve_confined(&self.roots, p) {
                        if is_root(&self.roots, &id, &full) {
                            return err(400, "cannot delete storage root");
                        }
                    }
                }
                let mut failed: Vec<String> = Vec::new();
                for p in &paths {
                    match resolve_confined(&self.roots, p) {
                        Ok((_, full)) => {
                            let r = if full.is_dir() {
                                // only empty folders, like Crosspoint
                                std::fs::remove_dir(&full)
                            } else {
                                std::fs::remove_file(&full).map(|_| ())
                            };
                            if let Err(e) = r {
                                failed.push(format!("{p}: {e}"));
                            }
                        }
                        Err(e) => failed.push(format!("{p}: {e}")),
                    }
                }
                if failed.is_empty() {
                    json(serde_json::json!({"ok": true, "deleted": paths.len()}))
                } else {
                    // paths stay in the device log only, never in responses
                    for f in &failed {
                        self.stats.push(format!("delete failed: {f}"));
                    }
                    err(500, "internal error")
                }
            }
            ("GET", "/api/zip") => {
                let p = match q("path") {
                    Some(p) => p,
                    None => return err(400, "path required"),
                };
                match resolve_confined(&self.roots, &p) {
                    Ok((id, full)) => {
                        if !full.is_dir() {
                            return err(400, "not a folder");
                        }
                        if is_root(&self.roots, &id, &full) {
                            return err(400, "cannot zip storage root");
                        }
                        match zip_collect(&full) {
                            Err(e) => {
                                self.stats.push(format!("zip failed: {e}"));
                                err(500, "internal error")
                            }
                            Ok(files) => {
                                let name = full
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "folder".into());
                                let ctype = "Content-Type: application/zip"
                                    .parse::<tiny_http::Header>();
                                let disp = format!(
                                    "Content-Disposition: attachment; filename*=UTF-8''{}.zip",
                                    urlencoding::encode(&name)
                                )
                                .parse::<tiny_http::Header>();
                                match (ctype, disp) {
                                    (Ok(ct), Ok(dp)) => {
                                        // zip streams from a temp file on flash
                                        // (never RAM); deleted after respond.
                                        let tmp = unique_tmp(&full, "zip");
                                        match zip_dir_to_file(&files, &tmp) {
                                            Ok(()) => match std::fs::File::open(&tmp) {
                                                Ok(fh) => (
                                                    tiny_http::Response::from_file(fh)
                                                        .with_header(ct)
                                                        .with_header(dp)
                                                        .boxed(),
                                                    200,
                                                    // deleted by the serve loop
                                                    // after the transfer
                                                    Some(tmp),
                                                ),
                                                Err(e) => {
                                                    let _ = std::fs::remove_file(&tmp);
                                                    self.stats.push(format!("zip failed: {e}"));
                                                    err(500, "internal error")
                                                }
                                            },
                                            Err(e) => {
                                                let _ = std::fs::remove_file(&tmp);
                                                self.stats.push(format!("zip failed: {e}"));
                                                err(500, "internal error")
                                            }
                                        }
                                    }
                                    _ => err(500, "internal error"),
                                }
                            }
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/upload") => {
                let p = q("path").unwrap_or_else(|| "int:/".into());
                let (.., full) = match resolve_confined(&self.roots, &p) {
                    Ok(x) => x,
                    Err(e) => return err(403, &e),
                };
                if !full.is_dir() {
                    return err(400, "target not a dir");
                }
                let resp = match save_multipart(&self.stats, req, &full) {
                    Ok(n) => json(serde_json::json!({"ok": true, "bytes": n})),
                    Err(e) => err(500, &e),
                };
                // upload over (ok or fail): force the final clean UI frame
                finish_upload(&self.stats);
                resp
            }
            _ => err(404, "not found"),
        }
    }
}

/// Mark upload finished (ok or fail) and force one last hook fire so the
/// device UI paints the final state.
pub fn finish_upload(stats: &Arc<Stats>) {
    stats.upload.finished.store(true, Ordering::Relaxed);
    stats.fire_hook_forced();
}

fn json(v: serde_json::Value) -> (tiny_http::ResponseBox, u16, Option<PathBuf>) {
    (
        tiny_http::Response::from_string(v.to_string())
            .with_header(
                "Content-Type: application/json; charset=utf-8"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            )
            .boxed(),
        200,
        None,
    )
}

fn err(code: u16, msg: &str) -> (tiny_http::ResponseBox, u16, Option<PathBuf>) {
    (
        tiny_http::Response::from_string(format!("{{\"error\":\"{msg}\"}}"))
            .with_status_code(code)
            .with_header(
                "Content-Type: application/json"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            )
            .boxed(),
        code,
        None,
    )
}

fn download_file(full: &PathBuf) -> (tiny_http::ResponseBox, u16, Option<PathBuf>) {
    // Stream from disk (Response::from_file), never load the whole file
    // into RAM — ebooks can exceed the device's free memory.
    match std::fs::File::open(full) {
        Ok(fh) => {
            let name = full
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            // mime_guess yields static ASCII tokens; only the filename-bearing
            // header can fail on hostile names — map that to generic 500.
            let mime = mime_guess::from_path(full)
                .first_or_octet_stream()
                .to_string();
            let ctype = format!("Content-Type: {mime}").parse::<tiny_http::Header>();
            let disp = format!(
                "Content-Disposition: attachment; filename*=UTF-8''{}",
                urlencoding::encode(&name)
            )
            .parse::<tiny_http::Header>();
            match (ctype, disp) {
                (Ok(ct), Ok(dp)) => (
                    tiny_http::Response::from_file(fh)
                        .with_header(ct)
                        .with_header(dp)
                        .boxed(),
                    200,
                    None,
                ),
                _ => err(500, "internal error"),
            }
        }
        Err(_) => err(404, "not found"),
    }
}

/// Cap for a single upload request body (checked against Content-Length
/// up front, then enforced again while streaming).
const UPLOAD_CAP: u64 = 512 * 1024 * 1024;

/// Minimal multipart/form-data parser (std only), STREAMING: part bodies
/// go straight to `.part` files in the target dir (same fs, atomic rename
/// on completion) — RAM stays flat regardless of upload size.
/// Delimiters are only recognized as full `--boundary` lines, so a boundary
/// string inside file content can never corrupt the upload (the old
/// split-everywhere approach could).
/// Reads the body in chunks so upload progress (bytes/speed) stays live.
fn save_multipart(
    stats: &Arc<Stats>,
    req: &mut tiny_http::Request,
    dir: &std::path::Path,
) -> Result<u64, String> {
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|h| {
            (
                h.field.as_str().to_string().to_ascii_lowercase(),
                h.value.as_str().to_owned(),
            )
        })
        .collect();
    let ctype = headers
        .iter()
        .find(|(k, _)| k == "content-type")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let boundary = ctype
        .split("boundary=")
        .nth(1)
        .ok_or_else(|| "missing multipart boundary".to_string())?
        .trim()
        .trim_matches('"')
        .to_owned();
    if boundary.is_empty() || boundary.len() > 256 {
        return Err("bad multipart boundary".to_string());
    }
    let total = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    // refuse giants before touching disk or RAM
    if total > UPLOAD_CAP {
        return Err("file too large (512MB cap)".into());
    }

    stats.upload.start(total);
    stats.fire_hook_forced();
    let (n, names) = parse_multipart_stream(stats, req.as_reader(), &boundary, dir)?;
    stats
        .uploaded_bytes
        .fetch_add(n, Ordering::Relaxed);
    let shown = if names.len() <= 3 {
        names.join(", ")
    } else {
        format!("{} files", names.len())
    };
    *stats.upload.file.lock().unwrap() = shown.clone();
    *stats.upload.done_msg.lock().unwrap() =
        format!("{} ({})", shown, format_size(n));
    stats.upload.reset();
    stats.fire_hook_forced();
    Ok(n)
}

/// Streaming core: parse one multipart body from any reader, writing part
/// bodies straight to temp files. Returns (bytes_written, file_names).
/// Separated from tiny_http so tests can feed adversarial chunk splits.
fn parse_multipart_stream(
    stats: &Arc<Stats>,
    reader: &mut dyn std::io::Read,
    boundary: &str,
    dir: &std::path::Path,
) -> Result<(u64, Vec<String>), String> {
    use std::io::Write as _;
    const CHUNK: usize = 32 * 1024;
    const MAX_HEADERS: usize = 64 * 1024;

    // "--boundary", and "\r\n--boundary" as seen between parts
    let marker: Vec<u8> = format!("--{boundary}").into_bytes();
    let mut delim = vec![b'\r', b'\n'];
    delim.extend_from_slice(&marker);

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    let mut names: Vec<String> = Vec::new();
    let mut total_written: u64 = 0;
    let mut received: u64 = 0;
    let mut named = false;
    let mut tmp_paths: Vec<PathBuf> = Vec::new();
    let mut cur: Option<(std::fs::File, PathBuf, String, u64)> = None; // file, tmp, name, size
    let mut skipping = false; // current part has no filename: discard body
    let mut started = false; // first boundary consumed
    let mut done = false;
    let mut eof = false;
    let cleanup = |paths: &[PathBuf]| {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    };

    'read: loop {
        if !eof {
            match reader.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    received += n as u64;
                    if received > UPLOAD_CAP {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("file too large (512MB cap)".into());
                    }
                    stats.upload.received.store(received, Ordering::Relaxed);
                    // filename usually arrives in the first chunk — show it live
                    if !named && buf.len() > 512 {
                        if let Some(nm) = first_filename(&buf) {
                            *stats.upload.file.lock().unwrap() = nm;
                            named = true;
                        }
                    }
                    stats.fire_hook();
                }
                Err(e) => {
                    cleanup(&tmp_paths);
                    stats.upload.reset();
                    return Err(e.to_string());
                }
            }
        }
        'process: loop {
            if done {
                break 'read;
            }
            // --- boundary + headers phase (no open file) ---
            if cur.is_none() && !skipping {
                if !started {
                    if buf.len() < marker.len() + 2 {
                        if eof {
                            cleanup(&tmp_paths);
                            stats.upload.reset();
                            return Err("bad multipart body".into());
                        }
                        break 'process;
                    }
                    if !buf.starts_with(&marker) {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("bad multipart body".into());
                    }
                    started = true;
                    if buf[marker.len()..].starts_with(b"--") {
                        done = true;
                        buf.clear();
                        break 'process;
                    }
                    if buf[marker.len()..].starts_with(b"\r\n") {
                        buf.drain(..marker.len() + 2);
                    } else if eof {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("bad multipart body".into());
                    } else {
                        break 'process; // split suffix, wait for more
                    }
                } else {
                    // continuation: previous part ended, expect "\r\n--marker..."
                    // (already consumed with the previous delimiter)
                }
                if let Some(i) = find_bytes(&buf, b"\r\n\r\n") {
                    let hraw = buf[..i].to_vec();
                    buf.drain(..i + 4);
                    match part_filename(&hraw) {
                        Some(fname) => {
                            let tmp = unique_tmp(dir, "part");
                            match std::fs::File::create(&tmp) {
                                Ok(f) => {
                                    tmp_paths.push(tmp.clone());
                                    cur = Some((f, tmp, fname, 0));
                                }
                                Err(e) => {
                                    cleanup(&tmp_paths);
                                    stats.upload.reset();
                                    return Err(e.to_string());
                                }
                            }
                        }
                        None => skipping = true, // form field, not a file
                    }
                } else {
                    if buf.len() > MAX_HEADERS {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("part headers too large".into());
                    }
                    if eof {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("truncated multipart body".into());
                    }
                    break 'process;
                }
            }
            // --- body phase ---
            // A delimiter starting at j is decidable iff its full window
            // plus the 2 suffix bytes are either present or refutable.
            // Scan every start position with a complete window; retain only
            // the undecidable tail (delim.len()-1 bytes max). Flushing the
            // rejected prefix keeps RAM flat AND lets the window advance —
            // a fixed-size tail would pin the search window forever and the
            // delimiter would never match on slow trickles.
            let mut found: Option<usize> = None;
            {
                let mut j = 0;
                while j + delim.len() <= buf.len() {
                    if &buf[j..j + delim.len()] == &delim[..] {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
            }
            match found {
                Some(i) => {
                    let after = i + delim.len();
                    if buf.len() < after + 2 {
                        if eof {
                            cleanup(&tmp_paths);
                            stats.upload.reset();
                            return Err("truncated multipart body".into());
                        }
                        break 'process; // suffix split across chunks
                    }
                    let data = buf[..i].to_vec();
                    let is_end = buf[after..].starts_with(b"--");
                    let is_next = buf[after..].starts_with(b"\r\n");
                    if !is_end && !is_next {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("bad multipart body".into());
                    }
                    if let Some((mut f, tmp, name, mut size)) = cur.take() {
                        if !data.is_empty() {
                            if f.write_all(&data).is_err() {
                                cleanup(&tmp_paths);
                                stats.upload.reset();
                                return Err("write failed".into());
                            }
                            size += data.len() as u64;
                        }
                        drop(f);
                        let dest = dir.join(&name);
                        if std::fs::rename(&tmp, &dest).is_err() {
                            cleanup(&tmp_paths);
                            stats.upload.reset();
                            return Err("write failed".into());
                        }
                        names.push(name);
                        total_written += size;
                    }
                    // skipping (form field): just drop the bytes
                    buf.drain(..after + 2);
                    skipping = false;
                    if is_end {
                        done = true;
                    }
                }
                None => {
                    if eof {
                        cleanup(&tmp_paths);
                        stats.upload.reset();
                        return Err("truncated multipart body".into());
                    }
                    // flush every fully-rejected start position, retaining
                    // only the undecidable tail (delim.len()-1 bytes max).
                    // A start at j is rejected only with a complete window,
                    // so nothing flushed can ever begin a delimiter.
                    let retain = (delim.len() - 1).min(buf.len());
                    let drain_end = buf.len() - retain;
                    if drain_end > 0 {
                        if let Some((f, _, _, size)) = cur.as_mut() {
                            if f.write_all(&buf[..drain_end]).is_err() {
                                cleanup(&tmp_paths);
                                stats.upload.reset();
                                return Err("write failed".into());
                            }
                            *size += drain_end as u64;
                        }
                        // skipped form fields are discarded the same way,
                        // or their bodies would pile up in RAM
                        buf.drain(..drain_end);
                    }
                    break 'process;
                }
            }
        }
        if eof && (done || buf.is_empty()) {
            break 'read;
        }
        if eof {
            // drained everything without a closing delimiter
            cleanup(&tmp_paths);
            stats.upload.reset();
            return Err("truncated multipart body".into());
        }
    }

    if total_written == 0 {
        stats.upload.reset();
        return Err("no files found".into());
    }
    Ok((total_written, names))
}
/// Unique temp path inside `dir` (same filesystem, so renames are atomic).
fn unique_tmp(dir: &std::path::Path, tag: &str) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        ".pbweb-{tag}-{}-{}-{}.tmp",
        std::process::id(),
        now_ms(),
        n
    ))
}

/// filename="..." from a disposition header block (basename only).
/// Rejects control characters outright (they would break HTTP headers
/// downstream and mangle the on-disk listing).
fn part_filename(hraw: &[u8]) -> Option<String> {
    let hstr = String::from_utf8_lossy(hraw);
    let line = hstr
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("content-disposition"))?;
    let raw = line
        .split("filename*=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_owned())
        .or_else(|| {
            line.split("filename=")
                .nth(1)
                .map(|s| s.trim().trim_matches('"').trim().to_owned())
        })?;
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if !valid_filename(base) {
        return None;
    }
    Some(base.to_owned())
}

/// First filename found anywhere in (partial) body — for live progress label.
fn first_filename(body: &[u8]) -> Option<String> {
    let key = b"filename=";
    let i = find_bytes(body, key)?;
    let mut rest = &body[i + key.len()..];
    // value is usually quoted: filename="z.pdf"
    if rest.first() == Some(&b'"') {
        rest = &rest[1..];
    }
    let end = rest
        .iter()
        .position(|&c| c == b'"' || c == b'\r' || c == b'\n' || c == b';')
        .unwrap_or(rest.len().min(128));
    let s = String::from_utf8_lossy(&rest[..end]).into_owned();
    let base = s.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        return None;
    }
    Some(base.to_owned())
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read a header value case-insensitively.
fn header_value(req: &tiny_http::Request, name: &str) -> Option<String> {
    req.headers().iter().find_map(|h| {
        h.field
            .as_str()
            .to_string()
            .eq_ignore_ascii_case(name)
            .then(|| h.value.as_str().to_owned())
    })
}

/// Cross-origin guard for mutations. Requests without Origin/Referer
/// (curl, same-origin navigations, our own fetch) pass — the pairing
/// token is authoritative. A present Origin must point back at this
/// server's own Host, otherwise it's a foreign page driving the API.
fn origin_ok(req: &tiny_http::Request) -> bool {
    let origin = header_value(req, "origin").or_else(|| header_value(req, "referer"));
    let Some(origin) = origin else {
        return true;
    };
    if origin == "null" {
        return false;
    }
    let Some(host) = header_value(req, "host") else {
        return true;
    };
    let auth = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&origin);
    let auth = auth.split('/').next().unwrap_or(auth);
    auth.eq_ignore_ascii_case(&host)
}

/// JSON-only request bodies (1MB cap). The old form-encoded fallback was
/// removed on purpose: urlencoded POSTs don't trigger CORS preflight,
/// which made every mutation CSRF-able from a foreign page.
fn read_json_body(req: &mut tiny_http::Request) -> serde_json::Value {
    use std::io::Read as _;
    let mut body = String::new();
    let _ = req.as_reader().take(1 << 20).read_to_string(&mut body);
    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null)
}

/// Best-effort LAN IP without extra deps: UDP-connect trick gives us the
/// local address the OS would use; parse it back from the socket.
fn first_non_loopback_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // No packets are sent by connect() on UDP.
    s.connect("192.168.1.1:80").ok()?;
    // Local addr after connect on some stacks reflects the route source.
    // Fallback chain: keep it simple, return what we got if usable.
    let ip = s.local_addr().ok()?.ip().to_string();
    if ip.starts_with("0.") || ip == "127.0.0.1" {
        // Try reading default route source via /proc (Linux/PocketBook).
        #[cfg(target_os = "linux")]
        {
            if let Ok(route) = std::fs::read_to_string("/proc/net/route") {
                for line in route.lines().skip(1) {
                    let f: Vec<&str> = line.split_whitespace().collect();
                    if f.len() > 2 && f[1] == "00000000" {
                        // default route via f[2] gateway — still not our IP.
                        // Last resort: first global-scope addr from /proc/net/fib_trie is
                        // overkill; return None and let UI show placeholder.
                        let _ = f;
                        break;
                    }
                }
            }
            return None;
        }
        #[cfg(not(target_os = "linux"))]
        return None;
    }
    Some(ip)
}

/// Zip a folder (recursive, stored without compression). Guards:
/// max 2000 files, max 512MB total — returns Err otherwise.
/// Symlinks are never followed (no escape, no cycles).
/// Phase 1 (this fn): metadata-only walk. Phase 2 (zip_dir_to_file):
/// streaming write, RAM stays flat.
fn zip_collect(dir: &std::path::Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut total: u64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| e.to_string())?;
        for e in rd {
            let e = e.map_err(|e| e.to_string())?;
            let p = e.path();
            // lstat, not metadata: symlinks are skipped, never followed
            let md = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
            if md.file_type().is_symlink() {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                total += md.len();
                if total > 512 * 1024 * 1024 {
                    return Err("folder too large (512MB cap)".into());
                }
                let rel = p
                    .strip_prefix(dir)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((p, rel));
                if files.len() > 2000 {
                    return Err("too many files (2000 cap)".into());
                }
            }
        }
    }
    Ok(files)
}

/// Stream collected files into a zip archive on disk (stored, no
/// compression — ebooks are already compressed). RAM stays flat.
fn zip_dir_to_file(files: &[(PathBuf, String)], dest: &std::path::Path) -> Result<(), String> {
    let f = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for (path, name) in files {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        let mut fh = std::fs::File::open(path).map_err(|e| e.to_string())?;
        std::io::copy(&mut fh, &mut zw).map_err(|e| e.to_string())?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_reads_exactly_four_bytes() {
        // Regression: gen_pin must never read-to-end (infinite on urandom).
        // An endless 0xAB stream must yield after exactly 4 bytes.
        struct Endless;
        impl std::io::Read for Endless {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                for b in out.iter_mut() {
                    *b = 0xAB;
                }
                Ok(out.len())
            }
        }
        let mut r = Endless;
        assert_eq!(
            pin_from_reader(&mut r),
            Some(u32::from_le_bytes([0xAB; 4]))
        );
    }

    #[test]
    fn pin_format() {        for _ in 0..50 {
            let p = gen_pin();
            assert_eq!(p.len(), 6, "PIN must be 6 chars");
            assert!(p.chars().all(|c| c.is_ascii_digit()), "digits only: {p}");
        }
    }

    #[test]
    fn auth_allows_token_and_blocks_bruteforce() {
        let srv = Server::new(Roots {
            internal: std::env::temp_dir(),
            sdcard: None,
        });
        let tok = srv.auth_token.clone();
        assert!(srv.check_auth(Some(&tok)), "correct PIN passes");
        assert!(!srv.check_auth(Some("000000")), "wrong PIN fails");
        assert!(!srv.check_auth(None), "missing token fails");
        // burn through the free attempts with wrong PINs
        for _ in 0..10 {
            assert!(!srv.check_auth(Some("000001")));
        }
        // now blocked: even the right PIN is rejected until timeout
        assert!(
            !srv.check_auth(Some(&tok)),
            "progressive block must hold"
        );
        // block state is observable for the log/UI
        assert!(srv.stats.blocked_until_ms.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn fmt_sizes() {        assert_eq!(format_size(0), "0 Б");
        assert_eq!(format_size(999), "999 Б");
        assert_eq!(format_size(2048), "2,0 КБ");
        assert_eq!(format_size(5 * 1024 * 1024), "5,0 МБ");
    }

    #[test]
    fn filenames_parsed() {
        let h = b"Content-Disposition: form-data; name=\"files\"; filename=\"C:\\books\\a.epub\"\r\nContent-Type: x";
        assert_eq!(part_filename(h).as_deref(), Some("a.epub"));
        assert_eq!(
            first_filename(b"--b\r\nContent-Disposition: form-data; filename=\"z.pdf\"\r\n\r\ndata"),
            Some("z.pdf".to_string())
        );
        assert_eq!(first_filename(b"nope"), None);
        // CRLF inside the value is cut by line splitting + valid_filename,
        // so control chars can never reach the filesystem or headers
        assert_eq!(
            part_filename(b"Content-Disposition: form-data; filename=\"a\r\nb\"").as_deref(),
            Some("a")
        );
        assert_eq!(part_filename(b"Content-Disposition: filename=\"a\rb\""), None);
    }

    #[test]
    fn crlf_filename_part_is_neutered() {
        let d = stream_tmpdir("crlf");
        let mut body = b"--C\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a\r\nb\"\r\n\r\nZZ\r\n".to_vec();
        body.extend_from_slice(b"--C\r\nContent-Disposition: form-data; name=\"files\"; filename=\"ok.txt\"\r\n\r\nOK\r\n--C--\r\n");
        let (n, names) = parse_all(&d, "C", &body, 7).unwrap();
        // hostile name degrades to a safe stub, legit file lands intact
        assert_eq!(names, vec!["a", "ok.txt"]);
        assert_eq!(n, 4);
        let mut on_disk: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        assert_eq!(on_disk, vec!["a", "ok.txt"]);
        // and neither name carries control characters
        assert!(on_disk.iter().all(|s| !s.chars().any(|c| c.is_control())));
        std::fs::remove_dir_all(&d).ok();
    }

    /// Reader that yields at most `step` bytes per call: forces every
    /// delimiter/header split the streaming parser must survive.
    struct ChunkReader {
        data: Vec<u8>,
        pos: usize,
        step: usize,
    }
    impl std::io::Read for ChunkReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let n = self.step.min(out.len()).min(self.data.len() - self.pos);
            out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn multipart_body(boundary: &str, parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        for (name, data) in parts {
            b.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            b.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\n\r\n")
                    .as_bytes(),
            );
            b.extend_from_slice(data);
            b.extend_from_slice(b"\r\n");
        }
        b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        b
    }

    fn parse_all(dir: &std::path::Path, boundary: &str, body: &[u8], step: usize) -> Result<(u64, Vec<String>), String> {
        let stats = Arc::new(Stats::default());
        let mut r = ChunkReader { data: body.to_vec(), pos: 0, step };
        parse_multipart_stream(&stats, &mut r, boundary, dir)
    }

    fn stream_tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pbweb-stream-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Live HTTP server for route-level security tests (auth, CSRF,
    /// root guards, caps). Distinct fixed port per test — tests run
    /// in parallel.
    fn live_server(tag: &str, port: u16) -> (Arc<Server>, String, std::thread::JoinHandle<()>, PathBuf) {
        let d = stream_tmpdir(tag);
        let srv = Arc::new(Server::new(Roots {
            internal: d.clone(),
            sdcard: None,
        }));
        let tok = srv.auth_token.clone();
        let (handle, p) = srv.bind_shared(port).expect("bind test server");
        assert_eq!(p, port);
        let w = {
            let srv = srv.clone();
            std::thread::spawn(move || srv.run_shared(&handle))
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        (srv, tok, w, d)
    }

    fn stop_live(srv: &Arc<Server>, w: std::thread::JoinHandle<()>, dir: &std::path::Path) {
        srv.shutdown();
        let _ = w.join();
        std::fs::remove_dir_all(dir).ok();
    }

    /// Raw HTTP/1.1 exchange with Connection: close, so the server ends
    /// the response and read_to_end terminates (keep-alive would block).
    fn http_raw(port: u16, head: &str, body: &[u8]) -> (u16, Vec<u8>) {
        use std::io::{Read as _, Write as _};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(std::time::Duration::from_secs(15)))
            .unwrap();
        let full = format!("{head}\r\nConnection: close\r\n\r\n");
        s.write_all(full.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let code = String::from_utf8_lossy(&out)
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (code, out)
    }

    #[test]
    fn csrf_origin_mismatch_rejected() {
        let (srv, tok, w, d) = live_server("csrf", 18081);
        let json_ct = "Content-Type: application/json";
        let post = |origin: Option<&str>, body: &str| {
            let mut head = format!(
                "POST /api/mkdir?token={tok} HTTP/1.1\r\nHost: 127.0.0.1:18081\r\n{json_ct}\r\nContent-Length: {}",
                body.len()
            );
            if let Some(o) = origin {
                head.push_str(&format!("\r\nOrigin: {o}"));
            }
            http_raw(18081, &head, body.as_bytes()).0
        };
        // no Origin (curl-style): allowed with valid token
        assert_eq!(post(None, r#"{"path":"int:/","name":"ok"}"#), 200);
        // foreign Origin driving the same authed mutation: blocked
        assert_eq!(
            post(Some("http://evil.com"), r#"{"path":"int:/","name":"evil"}"#),
            403
        );
        assert!(!d.join("evil").exists(), "CSRF must not create the dir");
        assert!(d.join("ok").is_dir());
        // same-origin fetch style passes
        assert_eq!(
            post(
                Some("http://127.0.0.1:18081"),
                r#"{"path":"int:/","name":"ok2"}"#
            ),
            200
        );
        assert!(d.join("ok2").is_dir());
        stop_live(&srv, w, &d);
    }

    #[test]
    fn root_mutations_rejected() {
        let (srv, tok, w, d) = live_server("roots", 18082);
        let base = format!("http://127.0.0.1:18082/api");
        let post = |path: &str, body: &str| {
            http_raw(
                18082,
                &format!(
                    "POST {path}?token={tok} HTTP/1.1\r\nHost: 127.0.0.1:18082\r\nContent-Type: application/json\r\nContent-Length: {}",
                    body.len()
                ),
                body.as_bytes(),
            )
            .0
        };
        assert_eq!(post("/api/rename", r#"{"path":"int:/","name":"pwned"}"#), 400);
        assert_eq!(post("/api/move", r#"{"path":"int:/","dest":"int:/"}"#), 400);
        assert_eq!(post("/api/delete", r#"{"path":"int:/"}"#), 400);
        let _ = base;
        // zip root: also refused before any walk happens
        let (code, _) = http_raw(
            18082,
            &format!("GET /api/zip?path=int:/&token={tok} HTTP/1.1\r\nHost: 127.0.0.1:18082"),
            b"",
        );
        assert_eq!(code, 400);
        stop_live(&srv, w, &d);
    }

    #[test]
    fn giant_content_length_refused_upfront() {
        let (srv, tok, w, d) = live_server("oomcap", 18083);
        // 999999999 declared, zero body bytes sent: must fail fast
        // without reading gigabytes (server never touches the body).
        let t0 = std::time::Instant::now();
        let (code, body) = http_raw(
            18083,
            &format!("POST /api/upload?path=int:/&token={tok} HTTP/1.1\r\nHost: 127.0.0.1:18083\r\nContent-Type: multipart/form-data; boundary=B\r\nContent-Length: 999999999"),
            b"",
        );
        assert_eq!(code, 500);
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
        let txt = String::from_utf8_lossy(&body);
        assert!(txt.contains("file too large"), "unexpected: {txt}");
        stop_live(&srv, w, &d);
    }

    #[test]
    fn multipart_fuzz_never_panics_nor_hangs() {
        // deterministic xorshift: reproducible corpus, no RNG dependency
        fn rnd(state: &mut u64) -> u8 {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            (*state & 0xFF) as u8
        }
        let alphabet = b"--BOUNDARY\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x.bin\"\r\n\r\n0123456789abcdef";
        for seed in 0..200u64 {
            let mut st = 0x9E3779B97F4A7C15u64.wrapping_add(seed.wrapping_mul(0xBF58476D1CE4E5B9));
            let len = 32 + (rnd(&mut st) as usize % 256);
            let mut body = Vec::with_capacity(len + 32);
            for _ in 0..len {
                body.push(alphabet[(rnd(&mut st) as usize) % alphabet.len()]);
            }
            body.extend_from_slice(b"\r\n--BOUNDARY--\r\n");
            let d = stream_tmpdir(&format!("fuzz{seed}"));
            let stats = Arc::new(Stats::default());
            for step in [1usize, 3, 29] {
                let mut r = ChunkReader { data: body.clone(), pos: 0, step };
                // must return (ok or clean Err), never panic, never hang
                let _ = parse_multipart_stream(&stats, &mut r, "BOUNDARY", &d);
            }
            // no temp droppings escape, whatever happened
            let leftovers: Vec<_> = std::fs::read_dir(&d)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name().to_string_lossy().starts_with(".pbweb-part-")
                })
                .collect();
            assert!(leftovers.is_empty(), "tmp leak: {leftovers:?}");
            std::fs::remove_dir_all(&d).ok();
        }
    }

    #[test]
    fn streaming_roundtrip_all_chunk_sizes() {
        // content deliberately contains the boundary string: the old
        // split-everywhere parser corrupted such uploads
        let tricky: Vec<u8> = b"aaa--BOUND1234\r\nbbb--BOUND1234--ccc\r\n".to_vec();
        let body = multipart_body("BOUND1234", &[("a.epub", &tricky), ("plain.txt", b"hi")]);
        for step in [1usize, 2, 3, 5, 7, 64, 4096, 1 << 20] {
            let d = stream_tmpdir(&format!("rt{step}"));
            let (n, names) =
                parse_all(&d, "BOUND1234", &body, step).expect("must parse at any split");
            assert_eq!(names, vec!["a.epub", "plain.txt"]);
            assert_eq!(n, (tricky.len() + 2) as u64);
            assert_eq!(std::fs::read(d.join("a.epub")).unwrap(), tricky);
            assert_eq!(std::fs::read(d.join("plain.txt")).unwrap(), b"hi");
            // no temp droppings left behind
            assert!(std::fs::read_dir(&d).unwrap().count() == 2);
            std::fs::remove_dir_all(&d).ok();
        }
    }

    #[test]
    fn streaming_rejects_garbage() {
        let d = stream_tmpdir("bad");
        assert!(parse_all(&d, "B", b"total garbage{{{", 3).is_err());
        assert!(parse_all(&d, "B", b"--B\r\nno-headers-here", 5).is_err());
        // truncated mid-body
        let mut body = multipart_body("B2", &[("x.bin", b"0123456789abcdef")]);
        body.truncate(body.len() - 4);
        assert!(parse_all(&d, "B2", &body, 2).is_err());
        // form field without filename is skipped, real file still lands
        let mut mixed = b"--M\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n".to_vec();
        mixed.extend_from_slice(b"--M\r\nContent-Disposition: form-data; name=\"files\"; filename=\"k.txt\"\r\n\r\nK\r\n--M--\r\n");
        let (n, names) = parse_all(&d, "M", &mixed, 3).unwrap();
        assert_eq!(names, vec!["k.txt"]);
        assert_eq!(n, 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn finish_upload_forces_final_hook() {        // Regression: the throttled tick must not swallow the finish frame
        // (device screen used to stick mid-progress on upload completion).
        use std::sync::atomic::AtomicBool;
        let s = Arc::new(Stats::default());
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        s.set_hook(Arc::new(move || f.store(true, Ordering::Relaxed)));
        // simulate a tick that just fired -> next fire is throttled away
        s.upload.last_hook_ms.store(now_ms(), Ordering::Relaxed);
        s.fire_hook();
        assert!(!fired.load(Ordering::Relaxed), "throttled tick must skip");
        finish_upload(&s);
        assert!(fired.load(Ordering::Relaxed), "finish must force");
        assert!(s.upload.finished.load(Ordering::Relaxed));
    }

    #[test]
    fn zip_collects_without_following_symlinks() {
        let d = stream_tmpdir("zipwalk");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), b"a-content").unwrap();
        std::fs::write(d.join("sub").join("b.bin"), b"b-content-123").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            // escape attempt + self-loop: both must be skipped, no hang
            symlink("/etc/hostname", d.join("evil")).unwrap();
            symlink(&d, d.join("sub").join("loop")).unwrap();
        }
        let files = zip_collect(&d).expect("walk must succeed");
        let names: Vec<&str> = files.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"sub/b.bin"));
        assert_eq!(files.len(), 2);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn zip_streams_to_disk_with_content_intact() {
        let d = stream_tmpdir("zipdisk");
        let payload = vec![0xABu8; 300_000];
        std::fs::write(d.join("big.bin"), &payload).unwrap();
        std::fs::write(d.join("note.txt"), b"hello-zip").unwrap();
        let files = zip_collect(&d).unwrap();
        let tmp = unique_tmp(&d, "ziptest");
        zip_dir_to_file(&files, &tmp).expect("zip write");
        let raw = std::fs::read(&tmp).unwrap();
        // stored (uncompressed): payload bytes present verbatim
        assert!(raw.len() > payload.len());
        assert!(find_bytes(&raw, b"hello-zip").is_some());
        assert!(find_bytes(&raw, &[0xAB; 16]).is_some());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn shutdown_unblocks_loop() {        let dir = std::env::temp_dir().join(format!(
            "pbweb-shutdown-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let srv = Arc::new(Server::new(Roots {
            internal: dir.clone(),
            sdcard: None,
        }));
        let (handle, _port) = srv.bind_shared(18080).expect("bind");
        assert!(srv.is_live());
        let worker = {
            let srv = srv.clone();
            std::thread::spawn(move || srv.run_shared(&handle))
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        srv.shutdown();
        assert!(!srv.is_live());
        let mut done = false;
        for _ in 0..50 {
            if worker.is_finished() {
                done = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(done, "accept loop did not stop after shutdown");
        let _ = worker.join();
        // re-bind works after shutdown (STOP -> START cycle). tiny_http's
        // internal accept thread needs a moment to release the socket.
        let mut bound = false;
        for _ in 0..50 {
            match srv.bind_shared(18080) {
                Ok(_) => {
                    bound = true;
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        assert!(bound, "re-bind failed after shutdown");
        srv.shutdown();
        std::fs::remove_dir_all(&dir).ok();
    }
}

