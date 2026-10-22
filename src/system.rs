//! Otwieranie odebranych plików i folderów w domyślnych aplikacjach systemu.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Otwiera plik lub katalog domyślną aplikacją systemu (Linux, macOS, Windows).
pub fn open_path(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Nie istnieje: {}", path.display()),
        ));
    }
    spawn_detached(opener(path, false)?)
}

/// Otwiera katalog zawierający plik (na Windows i macOS z zaznaczeniem pliku).
pub fn reveal_path(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return open_path(path);
    }
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Nie istnieje: {}", path.display()),
        ));
    }
    spawn_detached(opener(path, true)?)
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
        "Otwieranie plików nie jest obsługiwane na tej platformie",
    ))
}

fn spawn_detached(mut cmd: Command) -> io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Odbieramy status w tle, aby nie zostawiać procesów zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
