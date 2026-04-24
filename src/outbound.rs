//! Outbound local image upload: when an agent reply references a local image
//! inside its working directory (e.g. `![diagram](plot.png)`), rewrite the
//! Markdown to just the alt text and surface the actual file so the chat
//! adapter can upload it via `ChatAdapter::send_attachments`.
//!
//! The rewrite is conservative — it only accepts files that
//! (1) resolve inside `working_dir` (canonicalize comparison, no traversal),
//! (2) have an image extension we can upload (`png`/`jpg`/`jpeg`/`gif`/`webp`),
//! (3) are under 25 MB, and
//! (4) decode as the declared format via `image::guess_format`.
//! Anything that fails any check stays as-is in the text.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tracing::debug;

/// 25 MB matches the inbound workspace-handoff cap and is well within typical
/// chat upload limits (Discord 25 MB default, Slack 1 GB).
const MAX_OUTBOUND_SIZE: u64 = 25 * 1024 * 1024;

static IMAGE_MARKDOWN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").expect("static regex compiles"));

/// Scan `text` for Markdown image references pointing at files inside
/// `working_dir`. Returns the cleaned text (references replaced with their
/// alt text, or removed entirely when alt is empty) and the deduplicated list
/// of canonical paths that should be uploaded alongside the message.
///
/// If the cleanup consumes all non-whitespace content AND at least one path
/// was extracted, the returned text is replaced with `_(see attached file)_`
/// so the chat message still has visible body.
pub fn extract_local_image_uploads(text: &str, working_dir: &Path) -> (String, Vec<PathBuf>) {
    let Ok(canonical_root) = working_dir.canonicalize() else {
        return (text.to_string(), Vec::new());
    };

    let mut paths: Vec<PathBuf> = Vec::new();
    let cleaned = IMAGE_MARKDOWN_RE
        .replace_all(text, |caps: &regex::Captures| {
            let alt = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let target = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            match resolve_local_image_target(target, &canonical_root) {
                Some(path) => {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                    alt.to_string()
                }
                None => caps.get(0).map_or_else(String::new, |m| m.as_str().to_string()),
            }
        })
        .into_owned();

    let trimmed = cleaned.trim();
    let final_text = if trimmed.is_empty() && !paths.is_empty() {
        "_(see attached file)_".to_string()
    } else {
        cleaned
    };

    if !paths.is_empty() {
        debug!(count = paths.len(), "extracted local image uploads");
    }

    (final_text, paths)
}

/// Resolve a Markdown image target into a canonical path inside
/// `canonical_root`. Returns `None` for anything we refuse to upload
/// (external URLs, `data:` URIs, paths outside the working dir, unsupported
/// extensions, oversized files, bytes that don't decode as the declared
/// format).
fn resolve_local_image_target(target: &str, canonical_root: &Path) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:") {
        return None;
    }

    let raw_path = if let Some(rest) = target.strip_prefix("file://") {
        PathBuf::from(rest)
    } else {
        PathBuf::from(target)
    };
    let joined = if raw_path.is_absolute() {
        raw_path
    } else {
        canonical_root.join(raw_path)
    };
    let canonical = joined.canonicalize().ok()?;
    if !canonical.starts_with(canonical_root) {
        return None;
    }

    let ext = canonical
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_lowercase)?;
    let declared_format = match ext.as_str() {
        "png" => image::ImageFormat::Png,
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "gif" => image::ImageFormat::Gif,
        "webp" => image::ImageFormat::WebP,
        _ => return None,
    };

    let metadata = std::fs::metadata(&canonical).ok()?;
    if !metadata.is_file() {
        return None;
    }
    if metadata.len() > MAX_OUTBOUND_SIZE {
        return None;
    }

    let bytes = std::fs::read(&canonical).ok()?;
    let actual = image::guess_format(&bytes).ok()?;
    if actual != declared_format {
        return None;
    }

    Some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_png(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::new(w, h);
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        fs::write(path, buf.into_inner()).unwrap();
    }

    #[test]
    fn extracts_relative_image_and_replaces_with_alt_text() {
        let dir = tmpdir();
        let img = dir.path().join("diagram.png");
        write_png(&img, 4, 4);

        let input = "Here is ![A diagram](diagram.png) enjoy";
        let (out, paths) = extract_local_image_uploads(input, dir.path());

        assert_eq!(out, "Here is A diagram enjoy");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].file_name().unwrap(), "diagram.png");
    }

    #[test]
    fn accepts_file_uri_and_adds_empty_text_fallback() {
        let dir = tmpdir();
        let img = dir.path().join("plot.png");
        write_png(&img, 2, 2);
        let uri = format!("file://{}", img.canonicalize().unwrap().display());
        let input = format!("![]({uri})");

        let (out, paths) = extract_local_image_uploads(&input, dir.path());

        assert_eq!(out, "_(see attached file)_");
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn accepts_absolute_path_within_working_dir() {
        let dir = tmpdir();
        let img = dir.path().join("sub").join("pic.png");
        fs::create_dir_all(img.parent().unwrap()).unwrap();
        write_png(&img, 2, 2);
        let abs = img.canonicalize().unwrap();
        let input = format!("see ![pic]({}) ok", abs.display());

        let (out, paths) = extract_local_image_uploads(&input, dir.path());

        assert_eq!(out, "see pic ok");
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn rejects_external_urls_and_paths_outside_working_dir() {
        let dir = tmpdir();
        let outside = tmpdir();
        let outside_img = outside.path().join("escape.png");
        write_png(&outside_img, 2, 2);
        let outside_abs = outside_img.canonicalize().unwrap();

        let input = format!(
            "![remote](https://example.com/pic.png) ![out]({})",
            outside_abs.display()
        );
        let (out, paths) = extract_local_image_uploads(&input, dir.path());

        assert_eq!(out, input);
        assert!(paths.is_empty());
    }

    #[test]
    fn deduplicates_repeated_valid_images() {
        let dir = tmpdir();
        let img = dir.path().join("same.png");
        write_png(&img, 2, 2);

        let input = "![one](same.png) then ![two](same.png)";
        let (out, paths) = extract_local_image_uploads(input, dir.path());

        assert_eq!(out, "one then two");
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn rejects_invalid_image_content_even_with_image_extension() {
        let dir = tmpdir();
        let fake = dir.path().join("fake.png");
        fs::write(&fake, b"not actually a png").unwrap();

        let input = "![x](fake.png)";
        let (out, paths) = extract_local_image_uploads(input, dir.path());

        assert_eq!(out, input);
        assert!(paths.is_empty());
    }

    #[test]
    fn rejects_data_urls() {
        let dir = tmpdir();

        let input = "![x](data:image/png;base64,AAAA)";
        let (out, paths) = extract_local_image_uploads(input, dir.path());

        assert_eq!(out, input);
        assert!(paths.is_empty());
    }

    #[test]
    fn leaves_text_untouched_when_no_matches() {
        let dir = tmpdir();
        let input = "plain text with no images";
        let (out, paths) = extract_local_image_uploads(input, dir.path());

        assert_eq!(out, input);
        assert!(paths.is_empty());
    }
}
