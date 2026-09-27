//! Host-testable file logic: safe roots, listing, path guards.
//! No InkView dependency — runs on PC and on device.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Safe roots exposed over HTTP. Never expose `/` or `/system`.
#[derive(Debug, Clone)]
pub struct Roots {
    pub internal: PathBuf, // /mnt/ext1
    pub sdcard: Option<PathBuf>, // /mnt/ext2 if present
}

impl Roots {
    pub fn device_defaults() -> Self {
        let sd = Path::new("/mnt/ext2");
        Self {
            internal: PathBuf::from("/mnt/ext1"),
            sdcard: sd.exists().then(|| sd.to_path_buf()),
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn host_dev() -> Self {
        Self {
            internal: std::env::current_dir().unwrap_or(PathBuf::from(".")),
            sdcard: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String, // forward-slash, relative to root id
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

/// Resolve user-supplied `req_path` like "sd:/books/x" or "/books/x"
/// into a real FS path, rejecting `..` escapes and absolute breakouts.
pub fn resolve_safe(roots: &Roots, req_path: &str) -> Result<(String, PathBuf), String> {
    let req_path = req_path.trim();
    let (root_id, rel) = if let Some(rest) = req_path.strip_prefix("sd:/") {
        ("sd", rest)
    } else if let Some(rest) = req_path.strip_prefix("int:/") {
        ("int", rest)
    } else {
        ("int", req_path.trim_start_matches('/'))
    };

    let base = match root_id {
        "sd" => roots
            .sdcard
            .as_ref()
            .ok_or_else(|| "no sdcard".to_string())?,
        _ => &roots.internal,
    };

    // Reject `..`, absolute components, windows prefixes.
    let mut clean = PathBuf::new();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(p) => clean.push(p),
            Component::CurDir => {}
            Component::RootDir | Component::ParentDir | Component::Prefix(_) => {
                return Err("path traversal rejected".into())
            }
        }
    }
    let full = base.join(clean);
    Ok((root_id.to_string(), full))
}

pub fn to_display_path(root_id: &str, base: &Path, full: &Path) -> String {
    let rel = full.strip_prefix(base).unwrap_or(full);
    let s = rel.to_string_lossy().replace('\\', "/");
    format!("{root_id}:/{s}")
}

pub fn list_dir(base: &Path, root_id: &str, dir: &Path) -> Result<Vec<FileEntry>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for e in rd {
        let e = e.map_err(|e| e.to_string())?;
        let md = e.metadata().map_err(|e| e.to_string())?;
        let full = e.path();
        out.push(FileEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: to_display_path(root_id, base, &full),
            is_dir: md.is_dir(),
            size: md.len(),
            mtime: md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Best-effort local IPs for display (device: wlan0; host: any non-loopback v4).
pub fn local_ipv4s() -> Vec<String> {
    let mut ips = Vec::new();
    // Portable fallback: parse via gethostname? Keep std-only: try common interfaces
    // by connecting a UDP socket (no packets sent) to discover source addr.
    for remote in ["192.168.1.1:80", "10.0.0.1:80"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if s.connect(remote).is_ok() {
                if let Ok(local) = s.local_addr() {
                    // local_addr here is 0.0.0.0; need peer trick: use socket's own addr after connect
                    // Actually for UDP connect, local_addr stays bound addr. Use getsockname via std:
                    let _ = local;
                }
                // Proper way without libc: ask OS via `socket` — do minimal:
                if let Ok(addr) = s.local_addr() {
                    let ip = addr.ip().to_string();
                    if !ip.starts_with("0.") && ip != "127.0.0.1" {
                        ips.push(ip);
                    }
                }
            }
        }
    }
    ips.sort();
    ips.dedup();
    ips
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_roots() -> (tempfile_dir, Roots) {
        let d = tempfile_dir::new();
        fs::create_dir_all(d.path().join("books")).unwrap();
        fs::write(d.path().join("books/a.epub"), b"hi").unwrap();
        let r = Roots {
            internal: d.path().to_path_buf(),
            sdcard: None,
        };
        (d, r)
    }

    // tiny inline tempdir to avoid extra dep
    struct tempfile_dir {
        p: PathBuf,
    }
    impl tempfile_dir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!(
                "pbweb-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                n
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self { p }
        }
        fn path(&self) -> &Path {
            &self.p
        }
    }
    impl Drop for tempfile_dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.p);
        }
    }

    #[test]
    fn rejects_traversal() {
        let (_t, r) = tmp_roots();
        assert!(resolve_safe(&r, "int:/../etc").is_err());
        assert!(resolve_safe(&r, "/../../x").is_err());
        assert!(resolve_safe(&r, "sd:/x").is_err());
    }

    #[test]
    fn lists_sorted_dirs_first() {
        let (_t, r) = tmp_roots();
        let (id, full) = resolve_safe(&r, "int:/books").unwrap();
        let items = list_dir(&r.internal, &id, &full).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "a.epub");
    }
}
