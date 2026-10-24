//! Opening received files and folders with the system's default applications.

use rust_i18n::t;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Opens a file or folder with the default application (Linux, macOS, Windows).
///
/// # Errors
///
/// Fails when the path does not exist, the platform is unsupported or the
/// opener program cannot be started.
pub fn open_path(path: &Path) -> io::Result<()> {
    ensure_exists(path)?;
    spawn_detached(opener(path, false)?)
}

/// Opens the folder containing `path`, selecting the file on Windows and macOS.
///
/// # Errors
///
/// See [`open_path`].
pub fn reveal_path(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return open_path(path);
    }
    ensure_exists(path)?;
    spawn_detached(opener(path, true)?)
}

fn ensure_exists(path: &Path) -> io::Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            t!("system.not_found", path = path.display()).into_owned(),
        ))
    }
}

#[cfg(target_os = "linux")]
fn opener(path: &Path, reveal: bool) -> io::Result<Command> {
    let target = if reveal {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let mut cmd = Command::new("xdg-open");
    cmd.arg(target);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn opener(path: &Path, reveal: bool) -> io::Result<Command> {
    let mut cmd = Command::new("open");
    if reveal {
        cmd.arg("-R");
    }
    cmd.arg(path);
    Ok(cmd)
}

#[cfg(windows)]
fn opener(path: &Path, reveal: bool) -> io::Result<Command> {
    let mut cmd = Command::new("explorer");
    if reveal {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        cmd.arg(arg);
    } else {
        cmd.arg(path);
    }
    Ok(cmd)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn opener(_path: &Path, _reveal: bool) -> io::Result<Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        t!("system.unsupported").into_owned(),
    ))
}

fn spawn_detached(mut cmd: Command) -> io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
