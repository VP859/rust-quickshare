//! File paths typed, completed or dragged into the terminal.

use std::path::{Path, PathBuf};

/// Expands `~`, strips quotes and a `file://` prefix and decodes `%XX` escapes.
pub(super) fn expand_path(raw: &str) -> PathBuf {
    let raw = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let raw = raw.strip_prefix("file://").unwrap_or(raw);
    let decoded = percent_decode(raw);
    if let Some(rest) = decoded.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    if decoded == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(decoded)
}

/// Minimal `%XX` decoding for `file://` paths pasted from file managers.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses one path or several (e.g. files dragged into a terminal, quoted or
/// escaped with `\ `).
pub fn parse_paths(raw: &str) -> Vec<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let single = expand_path(raw);
    if single.exists() {
        return vec![single];
    }
    shell_split(raw).iter().map(|p| expand_path(p)).collect()
}

/// Splits like a shell: whitespace separates, quotes group, `\` escapes a character.
fn shell_split(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', None) => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ('"' | '\'', None) => quote = Some(c),
            (c, Some(q)) if c == q => quote = None,
            (c, None) if c.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            (c, _) => current.push(c),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Path completion: a single match is completed fully, several matches to
/// their common prefix.
pub(super) fn complete_path(value: &str) -> Option<String> {
    let (dir_part, prefix) = match value.rfind('/') {
        Some(i) => (&value[..=i], &value[i + 1..]),
        None => ("", value),
    };
    let lookup_dir = if dir_part.is_empty() {
        PathBuf::from(".")
    } else {
        expand_path(dir_part)
    };
    let mut matches: Vec<(String, bool)> = std::fs::read_dir(&lookup_dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let hidden_ok = prefix.starts_with('.') || !name.starts_with('.');
            (hidden_ok && name.starts_with(prefix)).then(|| {
                let is_dir = e.path().is_dir();
                (name, is_dir)
            })
        })
        .collect();
    matches.sort();
    match matches.as_slice() {
        [] => None,
        [(name, is_dir)] => Some(format!(
            "{dir_part}{name}{}",
            if *is_dir { "/" } else { "" }
        )),
        many => {
            let first = &many[0].0;
            let common = many.iter().fold(first.len(), |len, (name, _)| {
                first
                    .chars()
                    .zip(name.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a.len_utf8())
                    .sum::<usize>()
                    .min(len)
            });
            (common > prefix.len()).then(|| format!("{dir_part}{}", &first[..common]))
        }
    }
}

/// Shortens paths inside the home folder to `~/…` for display.
pub fn display_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_dropped_paths() {
        assert_eq!(
            shell_split(r#"'/tmp/a b.txt' /tmp/c\ d.txt "/tmp/e.txt""#),
            vec!["/tmp/a b.txt", "/tmp/c d.txt", "/tmp/e.txt"]
        );
    }

    #[test]
    fn expands_file_urls_and_quotes() {
        assert_eq!(
            expand_path("'file:///tmp/my%20ph%C3%B6to.jpg'"),
            PathBuf::from("/tmp/my phöto.jpg")
        );
    }

    #[test]
    fn completes_paths() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("report-2026.pdf"), b"x")?;
        std::fs::write(dir.path().join("report-2025.pdf"), b"x")?;
        std::fs::create_dir(dir.path().join("photos"))?;
        let base = format!("{}/", dir.path().display());
        assert_eq!(
            complete_path(&format!("{base}rep")),
            Some(format!("{base}report-202"))
        );
        assert_eq!(
            complete_path(&format!("{base}ph")),
            Some(format!("{base}photos/"))
        );
        assert_eq!(complete_path(&format!("{base}missing")), None);
        Ok(())
    }
}
