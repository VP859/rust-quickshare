//! Removing the location and other metadata from photos before they are sent.
//!
//! Photos from phones carry EXIF data: where they were taken (GPS), when, the
//! camera and its serial number, often a thumbnail, and editing software adds
//! XMP and comments. With [`set_strip_metadata`] on (off by default), JPEG and
//! PNG files are sent as a copy without them.
//!
//! The image data itself is not touched – no decoding, no new compression –
//! so the picture is exactly the same; only the metadata segments (JPEG) or
//! chunks (PNG) are left out. The one EXIF value that is kept is the
//! orientation, without which photos taken with a turned phone would show
//! sideways. Colour profiles (ICC) stay too. Other formats (HEIC, WebP, RAW,
//! videos) are sent as they are.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether metadata is removed from photos before they are sent.
static STRIP: AtomicBool = AtomicBool::new(false);

/// Turns removing metadata from photos on or off for this process (the
/// interfaces follow [`Config::strip_metadata`](crate::config::Config::strip_metadata)).
pub fn set_strip_metadata(on: bool) {
    STRIP.store(on, Ordering::Relaxed);
}

/// Whether metadata is removed from photos before they are sent.
pub fn strip_metadata() -> bool {
    STRIP.load(Ordering::Relaxed)
}

/// The kinds of files whose metadata can be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// JPEG (`.jpg`, `.jpeg`).
    Jpeg,
    /// PNG (`.png`).
    Png,
}

/// The kind of `path` by its contents (not its name).
fn kind_of(data: &[u8]) -> Option<Kind> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Kind::Jpeg)
    } else if data.starts_with(PNG_SIGNATURE) {
        Some(Kind::Png)
    } else {
        None
    }
}

/// Whether the name looks like a JPEG or PNG file (checked before reading it).
fn is_photo_name(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "jpe" | "png"
        )
    })
}

/// `data` without its metadata, or `None` when it is not a JPEG or PNG file.
///
/// # Errors
///
/// Fails when the file says it is a JPEG or PNG but its structure is broken:
/// it is then not sent at all, rather than with its metadata.
pub fn strip(data: &[u8]) -> Result<Option<Vec<u8>>> {
    match kind_of(data) {
        Some(Kind::Jpeg) => strip_jpeg(data).map(Some),
        Some(Kind::Png) => strip_png(data).map(Some),
        None => Ok(None),
    }
}

/// A copy of a photo without its metadata, deleted when dropped.
#[derive(Debug)]
pub struct StrippedCopy {
    /// The copy, with the same file name as the original.
    path: PathBuf,
}

impl StrippedCopy {
    /// Where the copy is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StrippedCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Folder of the copies: the system's temporary folder, on Android the
/// configuration folder (the app's private storage; it has no usable `/tmp`).
fn copies_dir() -> Result<PathBuf> {
    if cfg!(target_os = "android") {
        Ok(crate::config::paths::config_dir()?.join("stripped"))
    } else {
        Ok(std::env::temp_dir().join("ferry-stripped"))
    }
}

/// A copy of `path` without metadata, when removing it is on and the file is a
/// JPEG or PNG photo; `None` means: send the file as it is.
///
/// # Errors
///
/// Fails when the file cannot be read, its structure is broken, or the copy
/// cannot be written.
pub async fn copy_without_metadata(path: &Path) -> Result<Option<StrippedCopy>> {
    if !strip_metadata() || !is_photo_name(path) {
        return Ok(None);
    }
    let dir = copies_dir()?;
    copy_into(path, &dir).await
}

/// [`copy_without_metadata`] into `dir` (tests: a temporary folder).
async fn copy_into(path: &Path, dir: &Path) -> Result<Option<StrippedCopy>> {
    let source = path.to_path_buf();
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let data =
            std::fs::read(&source).with_context(|| t!("sender.read", path = source.display()))?;
        let Some(clean) =
            strip(&data).with_context(|| t!("metadata.broken", path = source.display()))?
        else {
            return Ok(None);
        };
        forget_old_copies(&dir);
        // Its own folder, so the copy keeps the original name.
        let folder = dir.join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&folder)?;
        let name = source.file_name().context("no file name")?;
        let copy = StrippedCopy {
            path: folder.join(name),
        };
        std::fs::write(&copy.path, clean)?;
        Ok(Some(copy))
    })
    .await
    .context("metadata task")?
}

/// Deletes copies left behind by a process that ended abruptly (older than a day).
fn forget_old_copies(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 24 * 3600);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// ---------------------------------------------------------------------------
// EXIF orientation
// ---------------------------------------------------------------------------

/// EXIF tag of the orientation.
const ORIENTATION: u16 = 0x0112;

/// The orientation (2–8) stored in a TIFF structure (the body of EXIF), if
/// it is not the normal one.
fn orientation(tiff: &[u8]) -> Option<u16> {
    let big = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    (0..count.min(512)).find_map(|i| {
        let entry = ifd + 2 + i * 12;
        // Type 3 is SHORT; the value is in the first two bytes of the field.
        (u16_at(entry)? == ORIENTATION && u16_at(entry + 2)? == 3)
            .then(|| u16_at(entry + 8))
            .flatten()
            .filter(|o| (2..=8).contains(o))
    })
}

/// A TIFF structure holding nothing but `orientation`.
fn orientation_only(orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::with_capacity(26);
    tiff.extend_from_slice(b"MM\0\x2A"); // big-endian TIFF
    tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD0 right after the header
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
    tiff.extend_from_slice(&ORIENTATION.to_be_bytes());
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes()); // one value
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]); // padding of the value field
    tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    tiff
}

// ---------------------------------------------------------------------------
// JPEG
// ---------------------------------------------------------------------------

/// Start of an EXIF APP1 segment.
const EXIF_HEADER: &[u8] = b"Exif\0\0";

/// The JPEG without metadata segments.
///
/// Kept: everything that makes the picture (tables, frame, scans), JFIF
/// (APP0), ICC colour profiles (APP2) and Adobe colour information (APP14).
/// Left out: EXIF and XMP (APP1), other application segments (APP3–APP13,
/// APP15: Photoshop/IPTC, multi-picture data …), comments, and anything after
/// the end of the image (e.g. a video of a "motion photo"). An EXIF
/// orientation is kept as a new, minimal EXIF segment.
fn strip_jpeg(data: &[u8]) -> Result<Vec<u8>> {
    let broken = || anyhow::anyhow!("not a valid JPEG");
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..2]);
    let mut i = 2;
    let mut scanned = false;
    loop {
        if data.get(i) != Some(&0xFF) {
            return Err(broken());
        }
        while data.get(i) == Some(&0xFF) {
            i += 1; // fill bytes
        }
        let marker = *data.get(i).ok_or_else(broken)?;
        i += 1;
        match marker {
            // Standalone markers without a length.
            0x01 | 0xD0..=0xD7 => out.extend_from_slice(&[0xFF, marker]),
            // End of the image: what follows (a "motion photo" video, …) is left out.
            0xD9 if scanned => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Ok(out);
            }
            0xD8 | 0xD9 => return Err(broken()),
            _ => {
                let len_bytes: [u8; 2] = data.get(i..i + 2).ok_or_else(broken)?.try_into()?;
                let len = usize::from(u16::from_be_bytes(len_bytes));
                if len < 2 {
                    return Err(broken());
                }
                let segment = data.get(i..i + len).ok_or_else(broken)?;
                let body = &segment[2..];
                if marker == 0xDA {
                    // Start of scan: the compressed image follows, up to the
                    // next marker – a 0xFF byte that is not followed by 0x00
                    // (an escaped 0xFF) or a restart marker. It is kept as it
                    // is; progressive images go on with more segments.
                    let mut end = i + len;
                    loop {
                        match data.get(end..end + 2) {
                            Some([0xFF, 0x00 | 0xD0..=0xD7]) => end += 2,
                            Some([0xFF, _]) => break,
                            Some(_) => end += 1,
                            None => return Err(broken()),
                        }
                    }
                    out.extend_from_slice(&[0xFF, marker]);
                    out.extend_from_slice(&data[i..end]);
                    scanned = true;
                    i = end;
                    continue;
                }
                let keep = match marker {
                    0xE0 | 0xEE => true,
                    0xE2 => body.starts_with(b"ICC_PROFILE\0"),
                    0xE1..=0xEF | 0xFE => false,
                    _ => true,
                };
                if keep {
                    out.extend_from_slice(&[0xFF, marker]);
                    out.extend_from_slice(segment);
                } else if marker == 0xE1
                    && let Some(o) = body.strip_prefix(EXIF_HEADER).and_then(orientation)
                {
                    let tiff = orientation_only(o);
                    let len = u16::try_from(2 + EXIF_HEADER.len() + tiff.len())?;
                    out.extend_from_slice(&[0xFF, 0xE1]);
                    out.extend_from_slice(&len.to_be_bytes());
                    out.extend_from_slice(EXIF_HEADER);
                    out.extend_from_slice(&tiff);
                }
                i += len;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

/// The first eight bytes of every PNG file.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The PNG without metadata chunks.
///
/// Left out: EXIF (`eXIf`, except a minimal one with the orientation), texts
/// (`tEXt`, `zTXt`, `iTXt` – also where XMP lives) and the modification time
/// (`tIME`), and anything after the end of the image. Everything else, the
/// colour information included, is kept as it is.
fn strip_png(data: &[u8]) -> Result<Vec<u8>> {
    let broken = || anyhow::anyhow!("not a valid PNG");
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(PNG_SIGNATURE);
    let mut i = PNG_SIGNATURE.len();
    loop {
        let len_bytes: [u8; 4] = data.get(i..i + 4).ok_or_else(broken)?.try_into()?;
        let len = usize::try_from(u32::from_be_bytes(len_bytes))?;
        let kind: [u8; 4] = data.get(i + 4..i + 8).ok_or_else(broken)?.try_into()?;
        let whole = data.get(i..i + 12 + len).ok_or_else(broken)?;
        if i == PNG_SIGNATURE.len() && &kind != b"IHDR" {
            return Err(broken());
        }
        match &kind {
            b"tEXt" | b"zTXt" | b"iTXt" | b"tIME" => {}
            b"eXIf" => {
                if let Some(o) = orientation(&whole[8..8 + len]) {
                    push_png_chunk(&mut out, b"eXIf", &orientation_only(o));
                }
            }
            _ => out.extend_from_slice(whole),
        }
        i += 12 + len;
        if &kind == b"IEND" {
            return Ok(out);
        }
    }
}

/// Appends a PNG chunk with its length and checksum.
fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = crc32(&[kind.as_slice(), body].concat());
    out.extend_from_slice(&crc.to_be_bytes());
}

/// CRC-32 (ISO-HDLC) as PNG uses it for its chunks.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG segment with its marker and length.
    fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
        s.extend_from_slice(body);
        s
    }

    /// EXIF with an orientation, a GPS pointer, a camera serial number and a
    /// thumbnail-like blob, little-endian like most cameras.
    fn camera_exif(orientation: u16) -> Vec<u8> {
        let mut tiff = b"II\x2A\0\x08\0\0\0".to_vec();
        let entries: [(u16, u16, u32, u32); 3] = [
            (0x0112, 3, 1, u32::from(orientation)),
            (0x8825, 4, 1, 0x40), // GPS IFD
            (0xA431, 2, 4, u32::from_le_bytes(*b"SN1\0")),
        ];
        tiff.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&kind.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            tiff.extend_from_slice(&value.to_le_bytes());
        }
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(b"GPS 52.2297N 21.0122E secret place");
        let mut exif = EXIF_HEADER.to_vec();
        exif.extend_from_slice(&tiff);
        exif
    }

    fn jpeg(orientation: u16) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend(segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        j.extend(segment(0xE1, &camera_exif(orientation)));
        j.extend(segment(
            0xE1,
            b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>Warsaw</x:xmpmeta>",
        ));
        j.extend(segment(0xE2, b"ICC_PROFILE\0\x01\x01profile"));
        j.extend(segment(0xED, b"Photoshop 3.0\0IPTC by Kate"));
        j.extend(segment(0xFE, b"comment: my secret"));
        j.extend(segment(0xDB, &[0; 65])); // quantization table
        j.extend(segment(0xC0, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0])); // frame
        j.extend(segment(0xDA, &[1, 1, 0, 0, 63, 0])); // scan
        j.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]); // data, stuffing, restart
        j.extend_from_slice(&[0xFF, 0xD9]);
        // A "motion photo": a video appended, which may hold 0xFF 0xD9 itself.
        j.extend_from_slice(b"MotionPhoto_Data \xFF\xD9 trailer with GPS");
        j
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn jpeg_metadata_is_removed_and_the_picture_kept() {
        let original = jpeg(6);
        let clean = strip(&original).unwrap().unwrap();
        for secret in [
            &b"GPS"[..],
            b"SN1",
            b"Warsaw",
            b"IPTC",
            b"secret",
            b"MotionPhoto",
        ] {
            assert!(
                !contains(&clean, secret),
                "{:?} is gone",
                String::from_utf8_lossy(secret)
            );
        }
        assert!(contains(&clean, b"JFIF"), "JFIF kept");
        assert!(contains(&clean, b"ICC_PROFILE"), "colour profile kept");
        assert!(
            clean.ends_with(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xD9]),
            "image data kept as it is"
        );
        // Only the orientation is left of the EXIF data.
        let exif_at = clean.windows(6).position(|w| w == EXIF_HEADER).unwrap();
        assert_eq!(orientation(&clean[exif_at + 6..]), Some(6));
        assert!(
            strip(&clean).unwrap().unwrap() == clean,
            "stripping twice changes nothing"
        );
    }

    #[test]
    fn a_normal_orientation_leaves_no_exif_at_all() {
        let clean = strip(&jpeg(1)).unwrap().unwrap();
        assert!(!contains(&clean, EXIF_HEADER));
    }

    #[test]
    fn broken_photos_are_refused_and_others_left_alone() {
        let original = jpeg(1);
        assert!(strip(&original[..40]).is_err(), "cut off");
        let mut no_scan = original.clone();
        let end = no_scan.len() - b"MotionPhoto_Data \xFF\xD9 trailer with GPS".len();
        no_scan.truncate(end - 2);
        assert!(strip(&no_scan).is_err(), "no end of image");
        assert!(strip(b"plain text").unwrap().is_none());
        assert!(strip(b"").unwrap().is_none());
    }

    fn png_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        push_png_chunk(&mut out, kind, body);
        out
    }

    #[test]
    fn png_metadata_is_removed_and_the_picture_kept() {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend(png_chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        png.extend(png_chunk(b"iCCP", b"sRGB\0\0profile"));
        png.extend(png_chunk(b"eXIf", &camera_exif(8)[6..]));
        png.extend(png_chunk(b"tEXt", b"Author\0Kate"));
        png.extend(png_chunk(
            b"iTXt",
            b"XML:com.adobe.xmp\0\0\0\0\0<x>Warsaw</x>",
        ));
        png.extend(png_chunk(b"tIME", &[7, 234, 11, 9, 12, 0, 0]));
        png.extend(png_chunk(b"IDAT", b"compressed pixels"));
        png.extend(png_chunk(b"IEND", b""));
        png.extend_from_slice(b"trailer");
        let clean = strip(&png).unwrap().unwrap();
        for secret in [&b"Kate"[..], b"Warsaw", b"GPS", b"SN1", b"tIME", b"trailer"] {
            assert!(
                !contains(&clean, secret),
                "{:?} is gone",
                String::from_utf8_lossy(secret)
            );
        }
        assert!(contains(&clean, b"iCCP") && contains(&clean, b"compressed pixels"));
        let exif_at = clean.windows(4).position(|w| w == b"eXIf").unwrap();
        assert_eq!(orientation(&clean[exif_at + 4..]), Some(8));
        assert!(clean.ends_with(&png_chunk(b"IEND", b"")));
        assert!(strip(&png[..30]).is_err());
    }

    #[test]
    fn the_png_checksum_is_the_standard_one() {
        // CRC-32 of "IEND" with no data, as in every PNG file.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[tokio::test]
    async fn copies_keep_the_name_and_disappear_when_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("IMG_1.jpg");
        std::fs::write(&photo, jpeg(1)).unwrap();
        let copies = dir.path().join("copies");
        let copy = copy_into(&photo, &copies).await.unwrap().unwrap();
        assert_eq!(copy.path().file_name(), photo.file_name());
        assert!(!contains(&std::fs::read(copy.path()).unwrap(), b"GPS"));
        let path = copy.path().to_path_buf();
        drop(copy);
        assert!(!path.exists() && !path.parent().unwrap().exists());
        let text = dir.path().join("notes.jpg");
        std::fs::write(&text, b"not really a photo").unwrap();
        assert!(copy_into(&text, &copies).await.unwrap().is_none());
    }
}
