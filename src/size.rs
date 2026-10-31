//! Human readable sizes, shared by every interface and by error messages.

/// Formats a size with a readable unit: `512 B`, `12.3 KB`, `4.0 MB`, `1.25 GB`.
///
/// # Examples
///
/// ```
/// use ferry::size::human_size;
///
/// assert_eq!(human_size(18), "18 B");
/// assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
/// ```
pub fn human_size(bytes: u64) -> String {
    /// Units for successive powers of 1024.
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let decimals = if unit >= 3 { 2 } else { 1 };
    format!("{value:.decimals$} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn formats_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(12 * 1024 + 300), "12.3 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.50 GB");
    }
}
