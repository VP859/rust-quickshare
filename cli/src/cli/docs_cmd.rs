//! Shell completions and manual pages, generated from the command line
//! definition ([`Cli`]), so they never fall behind it.
//!
//! - `ferry completions SHELL` prints the completion script for one shell,
//!   for installs without a package (the shell / PowerShell installer).
//! - `ferry generate-docs DIR` (hidden) writes all of them to `DIR/completions`
//!   and `DIR/man`. The results are kept in `dist/` for the packages (`.deb`,
//!   `.rpm`, the release archives); a test checks that they are up to date.

use super::args::Cli;
use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_complete::Shell;
use std::path::Path;

/// The binary name the completions are registered for.
const BIN: &str = "ferry";

/// The shells with a completion file in the packages, and its file name.
pub const SHELL_FILES: [(Shell, &str); 5] = [
    (Shell::Bash, "ferry.bash"),
    (Shell::Zsh, "_ferry"),
    (Shell::Fish, "ferry.fish"),
    (Shell::PowerShell, "_ferry.ps1"),
    (Shell::Elvish, "ferry.elv"),
];

/// The command line definition as documented: without the version, so the
/// generated files change only when the commands do.
fn command() -> clap::Command {
    Cli::command().version(None::<&str>)
}

/// `ferry completions SHELL`: prints the completion script to standard output.
///
/// # Errors
///
/// Fails when standard output cannot be written, except when the reader
/// stopped early (`ferry completions bash | head`).
pub fn print_completions(shell: Shell) -> Result<()> {
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command(), BIN, &mut script);
    match std::io::Write::write_all(&mut std::io::stdout().lock(), &script) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

/// `ferry generate-docs DIR`: writes the completion scripts to
/// `DIR/completions` and a manual page per command to `DIR/man`.
///
/// # Errors
///
/// Fails when a folder or file cannot be written.
pub fn generate_docs(dir: &Path) -> Result<()> {
    let completions = dir.join("completions");
    let man = dir.join("man");
    for folder in [&completions, &man] {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("cannot create {}", folder.display()))?;
    }
    for (shell, file) in SHELL_FILES {
        let mut script = Vec::new();
        clap_complete::generate(shell, &mut command(), BIN, &mut script);
        let path = completions.join(file);
        std::fs::write(&path, script)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    let mut root = command().disable_help_subcommand(true);
    root.build();
    write_man_pages(root, &man)
        .with_context(|| format!("cannot write the manual pages to {}", man.display()))
}

/// A page for `cmd` and each visible subcommand (`ferry-peer-add.1`, …).
fn write_man_pages(cmd: clap::Command, dir: &Path) -> std::io::Result<()> {
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()).cloned() {
        write_man_pages(sub, dir)?;
    }
    clap_mangen::Man::new(cmd)
        .source("Ferry")
        .manual("Ferry Manual")
        .generate_to(dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The files in `dist/` are what the current commands generate. After a
    /// change to the command line run `cargo run -p ferry-cli -- generate-docs dist`.
    #[test]
    fn the_packaged_completions_and_manual_pages_are_up_to_date() {
        let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist");
        let fresh = tempfile::tempdir().unwrap();
        generate_docs(fresh.path()).unwrap();
        for folder in ["completions", "man"] {
            let mut expected: Vec<_> = std::fs::read_dir(fresh.path().join(folder))
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            expected.sort();
            let mut packaged: Vec<_> = std::fs::read_dir(dist.join(folder))
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            packaged.sort();
            assert_eq!(
                packaged, expected,
                "dist/{folder} is out of date: cargo run -p ferry-cli -- generate-docs dist"
            );
            for name in expected {
                assert!(
                    std::fs::read(fresh.path().join(folder).join(&name)).unwrap()
                        == std::fs::read(dist.join(folder).join(&name)).unwrap(),
                    "dist/{folder}/{} is out of date: cargo run -p ferry-cli -- generate-docs dist",
                    name.to_string_lossy()
                );
            }
        }
    }

    #[test]
    fn every_documented_shell_has_a_file() {
        for (shell, file) in SHELL_FILES {
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut command(), BIN, &mut script);
            let script = String::from_utf8(script).unwrap();
            assert!(
                script.contains("relay-only") || script.contains("network"),
                "{file}"
            );
            assert!(
                script.contains("completions"),
                "{file}: the command completes itself"
            );
        }
    }
}
