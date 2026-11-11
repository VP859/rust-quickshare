//! JSON messages of the LocalSend protocol v2 and their validation.
//!
//! Field names follow the specification (<https://github.com/localsend/protocol>)
//! and the official implementation; unknown fields are ignored, so newer
//! LocalSend versions that add fields keep working.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::pairing::clean_device_name;
use crate::protocol::messages::{MAX_FILE_NAME_LEN, MAX_FOLDER_DEPTH};
use crate::transfer::security::sanitize_filename;

/// Most files one `prepare-upload` request may offer.
pub const MAX_FILES: usize = 10_000;
/// Longest file ID accepted from a sender.
const MAX_ID_LEN: usize = 128;
/// Longest relative path (`folder/sub/file.jpg`) accepted from a sender.
const MAX_PATH_LEN: usize = 4096;

/// Description of a device: multicast announcements, `register`, `info` and
/// the `info` part of `prepare-upload`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Name shown to the user.
    pub alias: String,
    /// Protocol version (`major.minor`).
    pub version: String,
    /// Device model (`iPhone`, `Samsung`, `Windows`), if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    /// `mobile`, `desktop`, `web`, `headless` or `server`, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    /// SHA-256 of the device's certificate (uppercase hex) in HTTPS mode.
    #[serde(default)]
    pub fingerprint: String,
    /// Port of the device's HTTP server (announcements and `register`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `https` or `http` (announcements and `register`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Whether the device offers files for download in a browser.
    #[serde(default)]
    pub download: bool,
    /// In multicast messages: `true` for an announcement that expects an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
}

impl DeviceInfo {
    /// The alias reduced to one printable line of at most 64 bytes, or
    /// `LocalSend` when nothing is left.
    pub fn clean_alias(&self) -> String {
        let alias = clean_device_name(&self.alias);
        if alias.is_empty() {
            "LocalSend".to_string()
        } else {
            alias
        }
    }

    /// The device model reduced like the alias, if any.
    pub fn clean_model(&self) -> Option<String> {
        self.device_model
            .as_deref()
            .map(clean_device_name)
            .filter(|m| !m.is_empty())
    }
}

/// One file offered in `prepare-upload`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    /// ID chosen by the sender.
    pub id: String,
    /// Name, or a relative path with `/` for files of a folder.
    pub file_name: String,
    /// Size in bytes.
    pub size: u64,
    /// MIME type (`image/jpeg`).
    #[serde(default)]
    pub file_type: String,
    /// SHA-256 of the content (hex), if the sender computed it.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Preview data; for a text message the text itself.
    #[serde(default)]
    pub preview: Option<String>,
}

/// Body of `POST /api/localsend/v2/prepare-upload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareUpload {
    /// The sender.
    pub info: DeviceInfo,
    /// Offered files by ID.
    pub files: HashMap<String, FileInfo>,
}

/// Answer to an accepted `prepare-upload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadResponse {
    /// ID of the upload session.
    pub session_id: String,
    /// Token of every accepted file, by file ID.
    pub files: HashMap<String, String>,
}

/// An offered file after validation: safe folder and file names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedFile {
    /// ID chosen by the sender.
    pub id: String,
    /// Folders below the download folder (empty for a single file).
    pub folder: Vec<String>,
    /// Safe file name.
    pub name: String,
    /// Announced size in bytes.
    pub size: u64,
    /// Expected SHA-256 of the content, if the sender gave a valid one.
    pub sha256: Option<[u8; 32]>,
}

impl OfferedFile {
    /// Folder and name joined with `/`, for display.
    pub fn display_name(&self) -> String {
        self.folder
            .iter()
            .chain(std::iter::once(&self.name))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// Why a `prepare-upload` body was refused (HTTP 400).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOffer(pub String);

impl std::fmt::Display for InvalidOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidOffer {}

/// Splits a sender's relative path (`/` or `\` separated) into safe folder
/// names and a safe file name.
///
/// # Errors
///
/// Fails for empty names, `..`, absolute paths, too deep folders and names
/// that are too long.
pub fn split_path(raw: &str) -> Result<(Vec<String>, String), InvalidOffer> {
    let invalid = || InvalidOffer(format!("invalid file name: {raw:?}"));
    if raw.len() > MAX_PATH_LEN {
        return Err(invalid());
    }
    if raw.starts_with(['/', '\\']) {
        return Err(invalid());
    }
    let mut parts: Vec<&str> = raw
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let name = parts.pop().ok_or_else(invalid)?;
    if parts.len() > MAX_FOLDER_DEPTH
        || parts
            .iter()
            .chain([&name])
            .any(|p| p.len() > MAX_FILE_NAME_LEN)
    {
        return Err(invalid());
    }
    let clean = |p: &str| sanitize_filename(p).map_err(|_| invalid());
    let folder = parts
        .into_iter()
        .map(clean)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((folder, clean(name)?))
}

/// Parses a hex SHA-256 (either case); `None` for anything else.
pub fn parse_sha256(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

impl PrepareUpload {
    /// The text of a message: one `text/*` file whose preview is all of it
    /// (its size is the preview's length in UTF-8). `None` for anything else,
    /// e.g. a text file with only the beginning as its preview.
    pub fn message(&self) -> Option<&str> {
        let [(_, file)] = self.files.iter().collect::<Vec<_>>()[..] else {
            return None;
        };
        let text = file.preview.as_deref()?;
        (file.file_type.starts_with("text/") && !text.is_empty() && text.len() as u64 == file.size)
            .then_some(text)
    }

    /// Checks the limits and turns every offered file into an [`OfferedFile`],
    /// sorted by display name.
    ///
    /// # Errors
    ///
    /// Fails when no file or more than [`MAX_FILES`] are offered, an ID is
    /// too long, a name is unsafe or the sizes overflow.
    pub fn validate(&self) -> Result<Vec<OfferedFile>, InvalidOffer> {
        if self.files.is_empty() {
            return Err(InvalidOffer("no files offered".into()));
        }
        if self.files.len() > MAX_FILES {
            return Err(InvalidOffer(format!("more than {MAX_FILES} files")));
        }
        let mut total: u64 = 0;
        let mut files = Vec::with_capacity(self.files.len());
        for (id, file) in &self.files {
            if id.is_empty() || id.len() > MAX_ID_LEN {
                return Err(InvalidOffer("invalid file id".into()));
            }
            total = total
                .checked_add(file.size)
                .ok_or_else(|| InvalidOffer("sizes overflow".into()))?;
            let (folder, name) = split_path(&file.file_name)?;
            files.push(OfferedFile {
                id: id.clone(),
                folder,
                name,
                size: file.size,
                sha256: file.sha256.as_deref().and_then(parse_sha256),
            });
        }
        files.sort_by_key(OfferedFile::display_name);
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_example_of_the_specification() {
        // From the protocol README (comments and trailing commas removed).
        let body = r#"{
          "info": { "alias": "Nice Orange", "version": "2.0", "deviceModel": "Samsung",
                    "deviceType": "mobile", "fingerprint": "random string", "port": 53317,
                    "protocol": "https", "download": true },
          "files": {
            "some file id": { "id": "some file id", "fileName": "my image.png", "size": 324242,
              "fileType": "image/jpeg", "sha256": "*sha256 hash*", "preview": "*preview data*",
              "metadata": { "modified": "2021-01-01T12:34:56Z", "accessed": "2021-01-01T12:34:56Z" } },
            "another file id": { "id": "another file id", "fileName": "another image.jpg",
              "size": 1234, "fileType": "image/jpeg", "sha256": "*sha256 hash*",
              "preview": "*preview data*" }
          }
        }"#;
        let offer: PrepareUpload = serde_json::from_str(body).unwrap();
        assert_eq!(offer.info.clean_alias(), "Nice Orange");
        let files = offer.validate().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "another image.jpg");
        assert_eq!(files[1].size, 324_242);
        assert_eq!(files[1].sha256, None, "not a valid hash, so not checked");
    }

    #[test]
    fn folders_are_split_and_every_part_is_sanitized() {
        assert_eq!(
            split_path("MLB-002/checksums/backup.sha256").unwrap(),
            (
                vec!["MLB-002".into(), "checksums".into()],
                "backup.sha256".into()
            )
        );
        assert_eq!(
            split_path("a\\b.txt").unwrap(),
            (vec!["a".into()], "b.txt".into())
        );
        assert_eq!(split_path("./x//y.txt").unwrap().0, vec!["x".to_string()]);
        for bad in ["", "/", "/etc/passwd", "../x", "a/../../x", "\\\\server\\x"] {
            assert!(split_path(bad).is_err(), "{bad:?}");
        }
        assert_eq!(split_path("a/").unwrap(), (vec![], "a".to_string()));
        let deep = vec!["d"; MAX_FOLDER_DEPTH + 1].join("/") + "/f";
        assert!(split_path(&deep).is_err());
    }

    #[test]
    fn hashes_are_hex_of_either_case() {
        let lower = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_sha256(lower), parse_sha256(&lower.to_uppercase()));
        assert!(parse_sha256(lower).is_some());
        assert_eq!(parse_sha256("xyz"), None);
        assert_eq!(parse_sha256(&"é".repeat(32)), None);
    }

    #[test]
    fn limits_are_enforced() {
        let file = |name: &str, size| FileInfo {
            id: name.into(),
            file_name: name.into(),
            size,
            file_type: String::new(),
            sha256: None,
            preview: None,
        };
        let offer = |files: Vec<FileInfo>| PrepareUpload {
            info: DeviceInfo::default(),
            files: files.into_iter().map(|f| (f.id.clone(), f)).collect(),
        };
        assert!(offer(vec![]).validate().is_err());
        assert!(
            offer(vec![file("a", u64::MAX), file("b", 1)])
                .validate()
                .is_err()
        );
        assert!(offer(vec![file(&"x".repeat(200), 1)]).validate().is_err());
        assert_eq!(DeviceInfo::default().clean_alias(), "LocalSend");
    }
}
