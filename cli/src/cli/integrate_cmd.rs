//! `ferry integrate`: "Send with Ferry" in the file managers of this user
//! (Dolphin, Nautilus, Nemo) on Linux, for installs without a package (the
//! AppImage, `cargo install`). The deb and rpm packages install the same
//! entries for everyone; on Windows the installer adds "Send to → Ferry".

use anyhow::{Context, Result};
use rust_i18n::t;
use std::path::{Path, PathBuf};

/// Dolphin service menu.
const DOLPHIN: &str = include_str!("../../../dist/integration/ferry-send.desktop");
/// Nemo action.
const NEMO: &str = include_str!("../../../dist/integration/ferry-send.nemo_action");
/// Nautilus extension (needs nautilus-python).
const NAUTILUS_EXTENSION: &str = include_str!("../../../dist/integration/ferry-send-nautilus.py");

/// One installed file.
struct Entry {
    /// Where, relative to the data folder (`~/.local/share`).
    path: &'static str,
    /// Its content.
    content: String,
    /// Whether it must be executable (KDE requires it for a user's service
    /// menus, Nautilus for scripts).
    executable: bool,
}

/// The command that starts the GUI: `ferry-gui` when it is on `PATH`,
/// otherwise the one next to this program.
fn gui_command() -> Result<String> {
    let name = if cfg!(windows) {
        "ferry-gui.exe"
    } else {
        "ferry-gui"
    };
    let on_path = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()));
    if on_path {
        return Ok("ferry-gui".to_string());
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|p| p.is_file());
    match beside {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => anyhow::bail!(t!("integrate.no_gui")),
    }
}

/// Whether nautilus-python is installed (it loads `.py` extensions).
fn has_nautilus_python() -> bool {
    [
        "/usr/lib64",
        "/usr/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ]
    .iter()
    .flat_map(|lib| {
        ["extensions-4", "extensions-3.0"].map(|v| {
            Path::new(lib)
                .join("nautilus")
                .join(v)
                .join("libnautilus-python.so")
        })
    })
    .any(|p| p.exists())
}

/// The entries for `command`; Nautilus gets the extension with
/// nautilus-python, otherwise a script in its Scripts submenu.
fn entries(command: &str, nautilus_python: bool) -> Vec<Entry> {
    // `ferry-gui` is replaced only when another path is needed; quoted for
    // paths with spaces (the .desktop and Nemo formats both accept quotes).
    let exec = if command == "ferry-gui" {
        "ferry-gui".to_string()
    } else {
        format!("\"{command}\"")
    };
    let with_command = |text: &str| text.replace("ferry-gui --", &format!("{exec} --"));
    let mut list = vec![
        Entry {
            path: "kio/servicemenus/ferry-send.desktop",
            content: with_command(DOLPHIN),
            executable: true,
        },
        Entry {
            path: "nemo/actions/ferry-send.nemo_action",
            content: with_command(NEMO),
            executable: false,
        },
    ];
    if nautilus_python {
        list.push(Entry {
            path: "nautilus-python/extensions/ferry-send.py",
            content: NAUTILUS_EXTENSION
                .replace("COMMAND = \"ferry-gui\"", &format!("COMMAND = {command:?}")),
            executable: false,
        });
    } else {
        list.push(Entry {
            path: "nautilus/scripts/Send with Ferry",
            content: format!(
                "#!/bin/sh\n# \"Send with Ferry\" in the Scripts menu of Nautilus (`ferry integrate`).\nexec {exec} -- \"$@\"\n"
            ),
            executable: true,
        });
    }
    list
}

/// Every path an install may have created, for removing.
const ALL_PATHS: [&str; 4] = [
    "kio/servicemenus/ferry-send.desktop",
    "nemo/actions/ferry-send.nemo_action",
    "nautilus-python/extensions/ferry-send.py",
    "nautilus/scripts/Send with Ferry",
];

/// Writes the entries into `data_dir`; returns the written paths.
fn install(data_dir: &Path, entries: &[Entry]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for entry in entries {
        let path = data_dir.join(entry.path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &entry.content)
            .with_context(|| t!("integrate.write", path = path.display()))?;
        #[cfg(unix)]
        if entry.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        written.push(path);
    }
    Ok(written)
}

/// Removes what [`install`] may have written; returns the removed paths.
fn remove(data_dir: &Path) -> Vec<PathBuf> {
    ALL_PATHS
        .iter()
        .map(|p| data_dir.join(p))
        .filter(|p| std::fs::remove_file(p).is_ok())
        .collect()
}

/// `ferry integrate [--remove]`.
///
/// # Errors
///
/// Fails when the GUI cannot be found or a file cannot be written.
pub fn run_integrate(remove_entries: bool) -> Result<()> {
    if !cfg!(target_os = "linux") {
        println!("{}", t!("integrate.not_linux"));
        return Ok(());
    }
    let data_dir = dirs::data_dir().context("no data folder")?;
    if remove_entries {
        for path in remove(&data_dir) {
            println!("- {}", path.display());
        }
        println!("{}", t!("integrate.removed"));
        return Ok(());
    }
    let command = gui_command()?;
    for path in install(&data_dir, &entries(&command, has_nautilus_python()))? {
        println!("+ {}", path.display());
    }
    println!("{}", t!("integrate.done"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_call_the_gui_with_the_files() {
        let list = entries("ferry-gui", true);
        let dolphin = &list[0].content;
        assert!(dolphin.contains("Exec=ferry-gui -- %F"));
        assert!(list[1].content.contains("Exec=ferry-gui -- %F"));
        assert!(list[2].content.contains("COMMAND = \"ferry-gui\""));

        // Not on PATH: the full path, quoted where the format needs it.
        let list = entries("/opt/My Apps/ferry-gui", false);
        assert!(
            list[0]
                .content
                .contains("Exec=\"/opt/My Apps/ferry-gui\" -- %F")
        );
        let script = &list[2];
        assert_eq!(script.path, "nautilus/scripts/Send with Ferry");
        assert!(
            script
                .content
                .contains("exec \"/opt/My Apps/ferry-gui\" -- \"$@\"")
        );
        assert!(script.executable);
    }

    #[test]
    fn install_and_remove_in_a_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        let written = install(dir.path(), &entries("ferry-gui", false)).unwrap();
        assert_eq!(written.len(), 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &str| {
                std::fs::metadata(dir.path().join(p))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            };
            assert_eq!(
                mode("kio/servicemenus/ferry-send.desktop"),
                0o755,
                "KDE ignores it otherwise"
            );
            assert_eq!(mode("nautilus/scripts/Send with Ferry"), 0o755);
        }
        assert_eq!(remove(dir.path()).len(), 3);
        assert!(remove(dir.path()).is_empty());
    }
}
