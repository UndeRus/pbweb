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

use pb_core::{list_dir, resolve_safe, Roots};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

static INDEX_HTML: &str = include_str!("../../../web/index.html");

#[derive(Debug, Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub uploaded_bytes: AtomicU64,
    pub log: Mutex<Vec<String>>,
}

impl Stats {
    pub fn push(&self, line: String) {
        let mut l = self.log.lock().unwrap();
        l.push(line);
        if l.len() > 60 {
            l.remove(0);
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
    }
    pub fn tail(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

pub struct Server {
    pub roots: Roots,
    pub stats: Arc<Stats>,
}

impl Server {
    pub fn new(roots: Roots) -> Self {
        Self {
            roots,
            stats: Arc::new(Stats::default()),
        }
    }

    pub fn serve(&self, port: u16) -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
        let (srv, p) = self.bind(port)?;
        self.run_on(srv);
        Ok(p)
    }

    /// Bind the first free port in port..port+10 without serving yet, so the
    /// caller can publish the URL before blocking in the accept loop.
    pub fn bind(
        &self,
        port: u16,
    ) -> Result<(tiny_http::Server, u16), Box<dyn std::error::Error + Send + Sync>> {
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

    fn run(&self, srv: tiny_http::Server) {
        for mut req in srv.incoming_requests() {
            let url = req.url().to_owned();
            let method = req.method().as_str().to_owned();
            self.stats
                .push(format!("{method} {url}"));
            let resp = self.route(&mut req, &method, &url);
            let _ = req.respond(resp);
        }
    }

    fn route(
        &self,
        req: &mut tiny_http::Request,
        method: &str,
        url: &str,
    ) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
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
        match (method, path) {
            ("GET", "/" | "/files") => tiny_http::Response::from_string(INDEX_HTML)
                .with_header(
                    "Content-Type: text/html; charset=utf-8".parse::<tiny_http::Header>().unwrap(),
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
                match resolve_safe(&self.roots, &p) {
                    Ok((id, full)) => {
                        let base = if id == "sd" {
                            self.roots.sdcard.clone().unwrap()
                        } else {
                            self.roots.internal.clone()
                        };
                        match list_dir(&base, &id, &full) {
                            Ok(items) => json(serde_json::to_value(items).unwrap()),
                            Err(e) => err(500, &e),
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("GET", "/api/download") => {
                let p = match q("path") {
                    Some(p) => p,
                    None => return err(400, "path required"),
                };
                match resolve_safe(&self.roots, &p) {
                    Ok((_, full)) => download_file(&full),
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/mkdir") => {
                let v = read_json_body(req);
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("int:/");
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
                if name.is_empty() || name.contains('/') || name.contains('\\') {
                    return err(400, "bad name");
                }
                match resolve_safe(&self.roots, path) {
                    Ok((_, mut full)) => {
                        full.push(name);
                        match std::fs::create_dir(&full) {
                            Ok(_) => json(serde_json::json!({"ok": true})),
                            Err(e) => err(500, &e.to_string()),
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/rename") => {
                let v = read_json_body(req);
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
                if name.is_empty()
                    || name.contains('/')
                    || name.contains('\\')
                    || name == "."
                    || name == ".."
                {
                    return err(400, "bad name");
                }
                match resolve_safe(&self.roots, path) {
                    Ok((_, full)) => {
                        if !full.exists() {
                            return err(404, "not found");
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
                            Err(e) => err(500, &e.to_string()),
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/move") => {
                let v = read_json_body(req);
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let dest_dir = v.get("dest").and_then(|x| x.as_str()).unwrap_or("");
                let (src_id, src_full) = match resolve_safe(&self.roots, path) {
                    Ok(x) => x,
                    Err(e) => return err(403, &e),
                };
                let (dst_id, mut dst_full) = match resolve_safe(&self.roots, dest_dir) {
                    Ok(x) => x,
                    Err(e) => return err(403, &e),
                };
                if src_id != dst_id {
                    return err(400, "cross-root move not supported");
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
                    Err(e) => err(500, &e.to_string()),
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
                    // compat: also accept form-encoded "path=..." bodies
                    if !s.is_empty() {
                        paths.push(s.to_owned());
                    }
                }
                if paths.is_empty() {
                    // fallback: raw form body "path=...&paths=[...]"
                    let mut body = String::new();
                    let _ = req.as_reader().read_to_string(&mut body);
                    for kv in body.split('&') {
                        if let Some(val) = kv.strip_prefix("path=") {
                            paths.push(
                                urlencoding::decode(val).unwrap_or_default().into_owned(),
                            );
                        }
                    }
                }
                if paths.is_empty() {
                    return err(400, "path required");
                }
                let mut failed: Vec<String> = Vec::new();
                for p in &paths {
                    match resolve_safe(&self.roots, p) {
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
                    err(500, &failed.join("; "))
                }
            }
            ("GET", "/api/zip") => {
                let p = match q("path") {
                    Some(p) => p,
                    None => return err(400, "path required"),
                };
                match resolve_safe(&self.roots, &p) {
                    Ok((_, full)) => {
                        if !full.is_dir() {
                            return err(400, "not a folder");
                        }
                        match zip_dir(&full) {
                            Ok(bytes) => {
                                let name = full
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "folder".into());
                                tiny_http::Response::from_data(bytes)
                                    .with_header(
                                        "Content-Type: application/zip"
                                            .parse::<tiny_http::Header>()
                                            .unwrap(),
                                    )
                                    .with_header(
                                        format!(
                                            "Content-Disposition: attachment; filename*=UTF-8''{}.zip",
                                            urlencoding::encode(&name)
                                        )
                                        .parse::<tiny_http::Header>()
                                        .unwrap(),
                                    )
                            }
                            Err(e) => err(500, &e),
                        }
                    }
                    Err(e) => err(403, &e),
                }
            }
            ("POST", "/api/upload") => {
                let p = q("path").unwrap_or_else(|| "int:/".into());
                let (.., full) = match resolve_safe(&self.roots, &p) {
                    Ok(x) => x,
                    Err(e) => return err(403, &e),
                };
                if !full.is_dir() {
                    return err(400, "target not a dir");
                }
                match save_multipart(req, &full) {
                    Ok(n) => {
                        self.stats
                            .uploaded_bytes
                            .fetch_add(n, Ordering::Relaxed);
                        json(serde_json::json!({"ok": true, "bytes": n}))
                    }
                    Err(e) => err(500, &e),
                }
            }
            _ => err(404, "not found"),
        }
    }
}

fn json(v: serde_json::Value) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string(v.to_string()).with_header(
        "Content-Type: application/json; charset=utf-8"
            .parse::<tiny_http::Header>()
            .unwrap(),
    )
}

fn err(code: u16, msg: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string(format!("{{\"error\":\"{msg}\"}}"))
        .with_status_code(code)
        .with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .unwrap(),
        )
}

fn download_file(full: &PathBuf) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    match std::fs::read(full) {
        Ok(bytes) => {
            let name = full
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let mime = mime_guess::from_path(full)
                .first_or_octet_stream()
                .to_string();
            tiny_http::Response::from_data(bytes)
                .with_header(format!("Content-Type: {mime}").parse::<tiny_http::Header>().unwrap())
                .with_header(
                    format!("Content-Disposition: attachment; filename*=UTF-8''{}", urlencoding::encode(&name))
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                )
        }
        Err(e) => err(404, &e.to_string()),
    }
}

/// Minimal multipart/form-data parser (std only): extracts each file part
/// by boundary and writes `filename` into `dir`. Guards filename to basename.
fn save_multipart(
    req: &mut tiny_http::Request,
    dir: &std::path::Path,
) -> Result<u64, String> {
    let ctype = req
        .headers()
        .iter()
        .find(|h| h.field.as_str().to_ascii_lowercase() == "content-type")
        .map(|h| h.value.as_str().to_owned())
        .unwrap_or_default();
    let boundary = ctype
        .split("boundary=")
        .nth(1)
        .ok_or_else(|| "missing multipart boundary".to_string())?
        .trim()
        .trim_matches('"')
        .to_owned();
    let mut body = Vec::new();
    req.as_reader()
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() > 512 * 1024 * 1024 {
        return Err("file too large (512MB cap)".into());
    }
    let marker = format!("--{boundary}").into_bytes();
    let mut total = 0u64;
    // split by boundary; each chunk: headers \r\n\r\n data \r\n
    let parts = split_bytes(&body, &marker);
    if parts.len() < 2 {
        return Err("bad multipart body".into());
    }
    for part in parts {
        if part.starts_with(b"--") || part.len() < 4 {
            continue;
        }
        let sep = b"\r\n\r\n";
        let idx = find_bytes(part, sep).ok_or("bad part headers")?;
        let (hraw, mut data) = part.split_at(idx);
        data = &data[sep.len()..];
        // strip trailing \r\n
        if data.ends_with(b"\r\n") {
            data = &data[..data.len() - 2];
        }
        let hstr = String::from_utf8_lossy(hraw);
        let fname = hstr
            .lines()
            .find(|l| l.to_ascii_lowercase().contains("content-disposition"))
            .and_then(|l| {
                l.split("filename*=")
                    .nth(1)
                    .map(|s| s.trim().trim_matches('"').to_owned())
                    .or_else(|| {
                        l.split("filename=")
                            .nth(1)
                            .map(|s| s.trim().trim_matches('"').trim().to_owned())
                    })
            })
            .unwrap_or_default();
        let fname = fname.rsplit(['/', '\\']).next().unwrap_or("").trim();
        if fname.is_empty() || fname == "." || fname == ".." {
            continue;
        }
        let dest = dir.join(fname);
        std::fs::write(&dest, data).map_err(|e| e.to_string())?;
        total += data.len() as u64;
    }
    if total == 0 {
        return Err("no files found".into());
    }
    Ok(total)
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn split_bytes<'a>(hay: &'a [u8], needle: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            out.push(&hay[start..i]);
            i += needle.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&hay[start..]);
    out
}

/// Read request body as JSON; accepts JSON and form-encoded `k=v&...` bodies.
fn read_json_body(req: &mut tiny_http::Request) -> serde_json::Value {
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        if !v.is_null() {
            return v;
        }
    }
    let mut map = serde_json::Map::new();
    for kv in body.split('&') {
        if let Some((k, val)) = kv.split_once('=') {
            map.insert(
                k.to_owned(),
                serde_json::Value::String(
                    urlencoding::decode(val).unwrap_or_default().into_owned(),
                ),
            );
        }
    }
    serde_json::Value::Object(map)
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
fn zip_dir(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut total: u64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| e.to_string())?;
        for e in rd {
            let e = e.map_err(|e| e.to_string())?;
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                total += e.metadata().map_err(|e| e.to_string())?.len();
                if total > 512 * 1024 * 1024 {
                    return Err("folder too large (512MB cap)".into());
                }
                files.push(p);
                if files.len() > 2000 {
                    return Err("too many files (2000 cap)".into());
                }
            }
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        for f in &files {
            let rel = f.strip_prefix(dir).unwrap_or(f);
            let name = rel.to_string_lossy().replace('\\', "/");
            zw.start_file(name, opts).map_err(|e| e.to_string())?;
            let data = std::fs::read(f).map_err(|e| e.to_string())?;
            zw.write_all(&data).map_err(|e| e.to_string())?;
        }
        zw.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

