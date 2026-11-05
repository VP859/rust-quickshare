//! Files handed to the desktop app by a file manager: "Send with Ferry" in
//! Dolphin, Nautilus, Nemo or Windows' "Send to" runs `ferry-gui -- <paths>`.
//!
//! The paths of the first start, and of every later start (which the
//! single-instance plugin hands over to the running app), are kept here until
//! the frontend takes them with `take_shared_files` and asks for a recipient.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Paths waiting for the frontend.
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Most paths taken from one start (a file manager may pass thousands).
const MAX_PATHS: usize = 10_000;

/// Percent-decodes the path of a `file://` URI; `None` for other text or a
/// path that is not UTF-8.
fn file_uri_path(arg: &str) -> Option<String> {
    let rest = arg.strip_prefix("file://")?;
    // `file:///home/a` or `file://localhost/home/a`; other hosts are remote.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).ok()?;
    // Windows: `file:///C:/Users/…`.
    Some(match path.get(1..3) {
        Some(drive) if drive.ends_with(':') => path[1..].to_string(),
        _ => path,
    })
}

/// The existing files and folders named by the command line `args` (the
/// program name first), relative ones resolved against `cwd`. Options
/// (`--minimized`) are skipped; after `--` everything is a path.
pub fn paths_from_args(args: &[String], cwd: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut only_paths = false;
    for arg in args.iter().skip(1) {
        if !only_paths && arg == "--" {
            only_paths = true;
            continue;
        }
        if !only_paths && arg.starts_with('-') {
            continue;
        }
        let path = match file_uri_path(arg) {
            Some(path) => PathBuf::from(path),
            None if arg.contains("://") => continue,
            None => PathBuf::from(arg),
        };
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        // Same separators as the system's own paths (`C:/a/b` from a URI → `C:\a\b`).
        let path: PathBuf = path.components().collect();
        if path.exists() {
            paths.push(path.to_string_lossy().into_owned());
        }
        if paths.len() >= MAX_PATHS {
            break;
        }
    }
    paths
}

/// Keeps `paths` for the frontend; returns whether there were any.
pub fn add(paths: Vec<String>) -> bool {
    if paths.is_empty() {
        return false;
    }
    if let Ok(mut pending) = PENDING.lock() {
        pending.extend(paths);
    }
    true
}

/// Takes (and forgets) the waiting paths.
pub fn take() -> Vec<String> {
    PENDING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("ferry-gui")
            .chain(list.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn files_folders_and_uris_are_taken_options_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("zdjęcie 1.jpg");
        std::fs::write(&file, b"x").unwrap();
        let folder = dir.path().join("Album");
        std::fs::create_dir(&folder).unwrap();
        let dash = dir.path().join("-weird.txt");
        std::fs::write(&dash, b"x").unwrap();

        // A real file URI: `file:///home/…` on Unix, `file:///C:/Users/…` on Windows.
        let mut path = file.to_string_lossy().replace('\\', "/");
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        let uri = format!("file://{}", path.replace(' ', "%20").replace('ę', "%C4%99"));
        let got = paths_from_args(
            &args(&[
                "--minimized",
                &uri,
                "Album",
                "missing.txt",
                "https://example.org/x",
                "--",
                "-weird.txt",
            ]),
            dir.path(),
        );
        let expected: Vec<String> = [&file, &folder, &dash]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, expected);
        assert!(paths_from_args(&args(&[]), dir.path()).is_empty());
        assert!(
            paths_from_args(&args(&["-weird.txt"]), dir.path()).is_empty(),
            "an option before --"
        );
    }

    #[test]
    fn file_uris_are_decoded() {
        assert_eq!(
            file_uri_path("file:///home/a%20b.txt").as_deref(),
            Some("/home/a b.txt")
        );
        assert_eq!(
            file_uri_path("file://localhost/tmp/x").as_deref(),
            Some("/tmp/x")
        );
        assert_eq!(
            file_uri_path("file:///C:/Users/a.txt").as_deref(),
            Some("C:/Users/a.txt")
        );
        assert_eq!(file_uri_path("file://server/share/x"), None, "remote host");
        assert_eq!(file_uri_path("file:///%ff"), None);
        assert_eq!(file_uri_path("/home/a"), None);
    }

    #[test]
    fn waiting_paths_are_taken_once() {
        assert!(!add(Vec::new()));
        assert!(add(vec!["/a".into()]));
        add(vec!["/b".into()]);
        assert_eq!(take(), ["/a", "/b"]);
        assert!(take().is_empty());
    }
}
