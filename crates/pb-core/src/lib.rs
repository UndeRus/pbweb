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

/// Base directory for a root id ("int" or "sd").
pub fn root_base(roots: &Roots, id: &str) -> Option<PathBuf> {
    match id {
        "sd" => roots.sdcard.clone(),
        _ => Some(roots.internal.clone()),
    }
}

/// True when `full` IS the storage root itself (e.g. "int:/").
/// Mutating the root (rename/move/delete/zip-target checks) is forbidden:
/// renaming it would move the whole internal storage.
pub fn is_root(roots: &Roots, id: &str, full: &Path) -> bool {
    match root_base(roots, id) {
        Some(base) => full == base,
        None => true,
    }
}

/// File names accepted from the network: no control characters (kills
/// HTTP header injection via Content-Disposition), sane length,
/// no dot-dot games. Directories are validated component-wise by
/// resolve_safe; this is the extra gate for leaf names.
pub fn valid_filename(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name.len() > 255 {
        return false;
    }
    !name.chars().any(|c| c.is_control())
}

/// Resolve like resolve_safe(), then verify the result stays inside the
/// root after symlink resolution. Textual `..` checks are not enough:
/// any symlink component (pre-existing on device) would otherwise let
/// reads/writes/deletes/zip escape the root or loop forever.
/// Returns the logical (non-canonicalized) path so display paths keep
/// working; confinement is proven on canonical forms.
///
/// Every *existing* component is lstat'ed and rejected if it's a symlink;
/// symlinks simply aren't operable through the API (documented limitation).
/// Non-existent tails (mkdir targets, rename destinations) pass through
/// after their existing ancestors check out.
/// NOTE: check-then-use TOCTOU remains in theory (an attacker with a shell
/// on the device could swap components mid-request); the LAN-only threat
/// model without shell access makes this acceptable.
pub fn resolve_confined(roots: &Roots, req_path: &str) -> Result<(String, PathBuf), String> {
    let (id, full) = resolve_safe(roots, req_path)?;
    let base = root_base(roots, &id).ok_or_else(|| "no sdcard".to_string())?;
    let base_c = base.canonicalize().map_err(|e| e.to_string())?;
    let rel = full.strip_prefix(&base).map_err(|_| "outside root".to_string())?;
    let mut cur = base_c.clone();
    let mut comps = rel.components().peekable();
    while let Some(comp) = comps.next() {
        let name = match comp {
            Component::Normal(p) => p,
            Component::CurDir => continue,
            _ => return Err("bad component".into()),
        };
        cur.push(name);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => {
                return Err("symlink escape".into())
            }
            Ok(_) => {} // real file/dir — keep walking
            Err(_) if cur.exists() => {
                // exists but unreadable: fail closed
                return Err("unreadable path".into());
            }
            Err(_) => {
                // not created yet: the rest can't be links either
                for rest in comps.by_ref() {
                    match rest {
                        Component::Normal(r) => cur.push(r),
                        Component::CurDir => {}
                        _ => return Err("bad component".into()),
                    }
                }
                break;
            }
        }
    }
    if !cur.starts_with(&base_c) {
        return Err("outside root".into());
    }
    Ok((id, full))
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

/// Sort key for file listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Mtime,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Listing query: substring search + sort + pagination.
/// Directories always come first (e-reader convention); sort applies
/// within each group so folders don't drown among files.
#[derive(Debug, Clone)]
pub struct FileQuery {
    pub q: String,
    pub sort: SortKey,
    pub order: SortOrder,
    pub limit: usize,
    pub offset: usize,
}

impl Default for FileQuery {
    fn default() -> Self {
        Self {
            q: String::new(),
            sort: SortKey::Name,
            order: SortOrder::Asc,
            limit: 200,
            offset: 0,
        }
    }
}

/// Apply search/sort/pagination to a listing. Returns (page, total).
pub fn query_files(items: Vec<FileEntry>, q: &FileQuery) -> (Vec<FileEntry>, usize) {
    let needle = q.q.to_lowercase();
    let mut v: Vec<FileEntry> = if needle.is_empty() {
        items
    } else {
        items
            .into_iter()
            .filter(|e| e.name.to_lowercase().contains(&needle))
            .collect()
    };
    v.sort_by(|a, b| {
        let dir_ord = b.is_dir.cmp(&a.is_dir);
        if dir_ord != std::cmp::Ordering::Equal {
            return dir_ord;
        }
        let inner = match q.sort {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Mtime => a.mtime.cmp(&b.mtime),
        };
        match q.order {
            SortOrder::Asc => inner,
            SortOrder::Desc => inner.reverse(),
        }
    });
    let total = v.len();
    let page = v.into_iter().skip(q.offset).take(q.limit.max(1)).collect();
    (page, total)
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

    fn sample_entries() -> Vec<FileEntry> {
        vec![
            FileEntry { name: "b.epub".into(), path: "int:/b.epub".into(), is_dir: false, size: 300, mtime: 3 },
            FileEntry { name: "dir".into(), path: "int:/dir".into(), is_dir: true, size: 0, mtime: 9 },
            FileEntry { name: "a.epub".into(), path: "int:/a.epub".into(), is_dir: false, size: 100, mtime: 1 },
            FileEntry { name: "C.pdf".into(), path: "int:/C.pdf".into(), is_dir: false, size: 200, mtime: 2 },
        ]
    }

    #[test]
    fn query_search_sort_page() {
        // default: name asc, dirs first
        let (page, total) = query_files(sample_entries(), &FileQuery::default());
        assert_eq!(total, 4);
        assert_eq!(page[0].name, "dir");
        assert_eq!(page[1].name, "a.epub");
        // search is case-insensitive substring
        let q = FileQuery { q: "EPUB".into(), ..FileQuery::default() };
        let (page, total) = query_files(sample_entries(), &q);
        assert_eq!(total, 2);
        assert!(page.iter().all(|e| e.name.to_lowercase().contains("epub")));
        // size desc, dirs still first
        let q = FileQuery { sort: SortKey::Size, order: SortOrder::Desc, ..FileQuery::default() };
        let (page, _) = query_files(sample_entries(), &q);
        assert_eq!(page[0].name, "dir");
        assert_eq!(page[1].name, "b.epub");
        assert_eq!(page[3].name, "a.epub");
        // mtime asc
        let q = FileQuery { sort: SortKey::Mtime, ..FileQuery::default() };
        let (page, _) = query_files(sample_entries(), &q);
        assert_eq!(page[1].name, "a.epub");
        // pagination
        let q = FileQuery { limit: 2, offset: 1, ..FileQuery::default() };
        let (page, total) = query_files(sample_entries(), &q);
        assert_eq!(total, 4);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].name, "a.epub");
        // offset past end -> empty page, total intact
        let q = FileQuery { offset: 99, ..FileQuery::default() };
        let (page, total) = query_files(sample_entries(), &q);
        assert!(page.is_empty());
        assert_eq!(total, 4);
    }

    #[test]
    fn root_guard() {
        let (_t, r) = tmp_roots();
        let (id, full) = resolve_safe(&r, "int:/").unwrap();
        assert!(is_root(&r, &id, &full));
        let (_, sub) = resolve_safe(&r, "int:/books").unwrap();
        assert!(!is_root(&r, &id, &sub));
    }

    #[test]
    fn filename_policy() {
        assert!(valid_filename("book.epub"));
        assert!(valid_filename("my book (1).fb2"));
        assert!(!valid_filename(""));
        assert!(!valid_filename("."));
        assert!(!valid_filename(".."));
        assert!(!valid_filename("a\rb\nc"));
        assert!(!valid_filename("a\x1bc"));
        assert!(!valid_filename(&"x".repeat(256)));
        assert_eq!("x".repeat(255).len(), 255);
        assert!(valid_filename(&"x".repeat(255)));
    }

    #[test]
    fn confined_allows_normal_paths() {
        let (_t, r) = tmp_roots();
        let (id, full) = resolve_confined(&r, "int:/books/a.epub").unwrap();
        assert_eq!(id, "int");
        assert!(full.ends_with("books/a.epub"));
        // not-yet-existing target resolves through existing parent
        let (_, newf) = resolve_confined(&r, "int:/books/new dir/x.pdf").unwrap();
        assert!(newf.ends_with("new dir/x.pdf"));
    }

    #[cfg(unix)]
    #[test]
    fn confined_blocks_symlink_escape() {
        use std::os::unix::fs::symlink;
        let (_t, r) = tmp_roots();
        let outside = std::env::temp_dir().join(format!(
            "pbweb-outside-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        symlink(&outside, r.internal.join("link")).unwrap();
        // direct escape through the link
        assert!(resolve_confined(&r, "int:/link/secret.txt").is_err());
        // nested escape
        fs::create_dir_all(r.internal.join("sub")).unwrap();
        symlink(&outside, r.internal.join("sub/link2")).unwrap();
        assert!(resolve_confined(&r, "int:/sub/link2/secret.txt").is_err());
        // the link itself is not operable either
        assert!(resolve_confined(&r, "int:/link").is_err());
        let _ = fs::remove_dir_all(&outside);
    }
}
