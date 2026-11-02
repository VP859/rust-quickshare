//! Files that can run code when opened.
//!
//! Receiving a photo is harmless; receiving `invoice.pdf.exe` or `update.apk` is
//! not, and a file that arrives by itself from an Auto-Accept device can be opened
//! without much thought. [`file_risk`] classifies a name by its extension so the
//! interfaces can warn, and [`Config::auto_accepts_file`](crate::config::Config::auto_accepts_file)
//! keeps such files out of Auto-Accept. On Windows received files also get a
//! Mark-of-the-Web (see [`mark_from_network`]), so SmartScreen and Office warn
//! before they are opened, as for files from a browser.

use rust_i18n::t;
use std::path::Path;

/// Kind of danger of a received file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Risk {
    /// A program or installer (`.exe`, `.msi`, `.apk`, `.AppImage`, `.deb`, …).
    Program,
    /// A script or shortcut (`.bat`, `.ps1`, `.sh`, `.js`, `.lnk`, `.desktop`, …).
    Script,
    /// An Office document with macros (`.docm`, `.xlsm`, …).
    Macro,
    /// The name contains characters that reverse the text direction, which can
    /// make `gnp.exe` look like `exe.png`.
    DisguisedName,
}

impl Risk {
    /// One-line warning in the active language, for terminal interfaces.
    pub fn warning(self) -> String {
        match self {
            Self::Program => t!("risk.program"),
            Self::Script => t!("risk.script"),
            Self::Macro => t!("risk.macro"),
            Self::DisguisedName => t!("risk.disguised"),
        }
        .into_owned()
    }

    /// Short identifier for the frontends (`program`, `script`, `macro`, `disguised`).
    pub fn code(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Script => "script",
            Self::Macro => "macro",
            Self::DisguisedName => "disguised",
        }
    }
}

/// Extensions of programs and installers (lower case, without the dot).
const PROGRAMS: &[&str] = &[
    "exe", "com", "scr", "msi", "msp", "msix", "appx", "dll", "cpl", "apk", "xapk", "apks",
    "appimage", "deb", "rpm", "pkg", "dmg", "run", "bin", "jar", "app",
];

/// Extensions of scripts and shortcuts.
const SCRIPTS: &[&str] = &[
    "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "reg", "lnk",
    "url", "sh", "bash", "zsh", "csh", "command", "desktop", "scpt", "py", "pl", "rb", "php",
];

/// Extensions of Office documents that may contain macros.
const MACROS: &[&str] = &[
    "docm", "dotm", "xlsm", "xltm", "xlam", "pptm", "potm", "ppam", "ppsm", "sldm",
];

/// Whether `ch` changes the direction of the text around it (U+202A–U+202E,
/// U+2066–U+2069), which is used to disguise file extensions.
pub fn is_bidi_control(ch: char) -> bool {
    matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Danger of a file called `name`, judged by its last extension (so
/// `invoice.pdf.exe` is a program) and by direction-changing characters;
/// `None` for names that are not known to be dangerous.
///
/// # Examples
///
/// ```
/// use ferry::transfer::risk::{Risk, file_risk};
///
/// assert_eq!(file_risk("holiday.JPG"), None);
/// assert_eq!(file_risk("setup.EXE"), Some(Risk::Program));
/// assert_eq!(file_risk("invoice.pdf.exe"), Some(Risk::Program));
/// assert_eq!(file_risk("run.sh"), Some(Risk::Script));
/// assert_eq!(file_risk("budget.xlsm"), Some(Risk::Macro));
/// assert_eq!(file_risk("photo\u{202e}gnp.exe"), Some(Risk::DisguisedName));
/// ```
pub fn file_risk(name: &str) -> Option<Risk> {
    if name.chars().any(is_bidi_control) {
        return Some(Risk::DisguisedName);
    }
    let extension = Path::new(name.trim())
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase();
    let extension = extension.as_str();
    if PROGRAMS.contains(&extension) {
        Some(Risk::Program)
    } else if SCRIPTS.contains(&extension) {
        Some(Risk::Script)
    } else if MACROS.contains(&extension) {
        Some(Risk::Macro)
    } else {
        None
    }
}

/// The most serious [`Risk`] among `names`, if any.
pub fn worst_risk<'a>(names: impl IntoIterator<Item = &'a String>) -> Option<Risk> {
    names
        .into_iter()
        .filter_map(|n| file_risk(n))
        .min_by_key(|r| match r {
            Risk::DisguisedName => 0,
            Risk::Program => 1,
            Risk::Script => 2,
            Risk::Macro => 3,
        })
}

/// Contents of the `Zone.Identifier` stream that marks a file as coming from
/// the Internet zone (`ZoneId=3`), as browsers do.
pub const ZONE_IDENTIFIER: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

/// Marks a file that arrived over the network.
///
/// On Windows this writes the Mark-of-the-Web (the `Zone.Identifier` NTFS stream),
/// so SmartScreen, Office Protected View and the shell warn before the file runs.
/// Best effort: file systems without streams (FAT, exFAT, network shares) are
/// skipped. Elsewhere nothing is needed: Unix systems and Android do not execute
/// a file that was created without the execute permission, and received files never
/// have it.
pub fn mark_from_network(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        if let Err(e) = std::fs::write(&stream, ZONE_IDENTIFIER) {
            tracing::debug!("No Mark-of-the-Web for {}: {e}", path.display());
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_the_last_extension_ignoring_case() {
        for (name, risk) in [
            ("setup.exe", Some(Risk::Program)),
            ("SETUP.EXE", Some(Risk::Program)),
            ("app.AppImage", Some(Risk::Program)),
            ("update.apk", Some(Risk::Program)),
            ("a.b.c.ps1", Some(Risk::Script)),
            ("start.desktop", Some(Risk::Script)),
            ("sheet.xlsm", Some(Risk::Macro)),
            ("photo.jpg", None),
            ("photo.jpg.txt", None),
            ("notes", None),
            ("exe", None),
            (".exe", None),
            ("archive.zip", None),
            ("document.pdf", None),
            ("setup.exe ", Some(Risk::Program)),
        ] {
            assert_eq!(file_risk(name), risk, "{name:?}");
        }
    }

    #[test]
    fn direction_changing_characters_disguise_the_extension() {
        // "photo" + RLO + "gnp.exe" is shown as "photoexe.png" by many interfaces.
        assert_eq!(file_risk("photo\u{202e}gnp.exe"), Some(Risk::DisguisedName));
        assert_eq!(file_risk("a\u{2067}b.txt"), Some(Risk::DisguisedName));
        assert!(is_bidi_control('\u{202e}') && !is_bidi_control('a'));
    }

    #[test]
    fn the_worst_name_of_a_batch_counts() {
        let names = vec!["a.jpg".to_string(), "b.sh".to_string(), "c.exe".to_string()];
        assert_eq!(worst_risk(&names), Some(Risk::Program));
        assert_eq!(worst_risk(&names[..1]), None);
        let names = vec!["x.xlsm".to_string(), "y\u{202e}.png".to_string()];
        assert_eq!(worst_risk(&names), Some(Risk::DisguisedName));
    }

    #[cfg(windows)]
    #[test]
    fn windows_files_get_the_mark_of_the_web() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("received.txt");
        std::fs::write(&file, b"data").unwrap();
        mark_from_network(&file);
        let mut stream = file.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        // NTFS keeps it; a temporary directory on another file system may not.
        if let Ok(mark) = std::fs::read_to_string(&stream) {
            assert_eq!(mark, ZONE_IDENTIFIER);
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"data", "content unchanged");
    }
}
