use crate::acp::ContentBlock;
use crate::config::SttConfig;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use image::ImageReader;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tracing::{debug, error};

/// Reusable HTTP client for downloading attachments (shared across adapters).
pub static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("static HTTP client must build")
});

/// Maximum dimension (width or height) for resized images.
const IMAGE_MAX_DIMENSION_PX: u32 = 1200;

/// JPEG quality for compressed output.
const IMAGE_JPEG_QUALITY: u8 = 75;

/// Download an image from a URL, resize/compress it, and return as a ContentBlock.
/// Pass `auth_token` for platforms that require authentication (e.g. Slack private files).
pub async fn download_and_encode_image(
    url: &str,
    mime_hint: Option<&str>,
    filename: &str,
    size: u64,
    auth_token: Option<&str>,
) -> Option<ContentBlock> {
    const MAX_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

    if url.is_empty() {
        return None;
    }

    let mime = mime_hint.or_else(|| {
        filename
            .rsplit('.')
            .next()
            .and_then(|ext| match ext.to_lowercase().as_str() {
                "png" => Some("image/png"),
                "jpg" | "jpeg" => Some("image/jpeg"),
                "gif" => Some("image/gif"),
                "webp" => Some("image/webp"),
                _ => None,
            })
    });

    let Some(mime) = mime else {
        debug!(filename, "skipping non-image attachment");
        return None;
    };
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    if !mime.starts_with("image/") {
        debug!(filename, mime, "skipping non-image attachment");
        return None;
    }

    if size > MAX_SIZE {
        error!(filename, size, "image exceeds 10MB limit");
        return None;
    }

    let mut req = HTTP_CLIENT.get(url);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let response = match req.send().await {
        Ok(resp) => resp,
        Err(e) => {
            error!(url, error = %e, "download failed");
            return None;
        }
    };
    if !response.status().is_success() {
        error!(url, status = %response.status(), "HTTP error downloading image");
        return None;
    }
    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            error!(url, error = %e, "read failed");
            return None;
        }
    };

    if bytes.len() as u64 > MAX_SIZE {
        error!(
            filename,
            size = bytes.len(),
            "downloaded image exceeds limit"
        );
        return None;
    }

    let (output_bytes, output_mime) = match resize_and_compress(&bytes) {
        Ok(result) => result,
        Err(e) => {
            if bytes.len() > 1024 * 1024 {
                error!(filename, error = %e, size = bytes.len(), "resize failed and original too large, skipping");
                return None;
            }
            debug!(filename, error = %e, "resize failed, using original");
            (bytes.to_vec(), mime.to_string())
        }
    };

    debug!(
        filename,
        original_size = bytes.len(),
        compressed_size = output_bytes.len(),
        "image processed"
    );

    let encoded = BASE64.encode(&output_bytes);
    Some(ContentBlock::Image {
        media_type: output_mime,
        data: encoded,
    })
}

/// Download an audio file and transcribe it via the configured STT provider.
/// Pass `auth_token` for platforms that require authentication.
pub async fn download_and_transcribe(
    url: &str,
    filename: &str,
    mime_type: &str,
    size: u64,
    stt_config: &SttConfig,
    auth_token: Option<&str>,
) -> Option<String> {
    const MAX_SIZE: u64 = 25 * 1024 * 1024; // 25 MB (Whisper API limit)

    if size > MAX_SIZE {
        error!(filename, size, "audio exceeds 25MB limit");
        return None;
    }

    let mut req = HTTP_CLIENT.get(url);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        error!(url, status = %resp.status(), "audio download failed");
        return None;
    }
    let bytes = resp.bytes().await.ok()?.to_vec();

    crate::stt::transcribe(
        &HTTP_CLIENT,
        stt_config,
        bytes,
        filename.to_string(),
        mime_type,
    )
    .await
}

/// Resize image so longest side <= IMAGE_MAX_DIMENSION_PX, then encode as JPEG.
/// GIFs are passed through unchanged to preserve animation.
pub fn resize_and_compress(raw: &[u8]) -> Result<(Vec<u8>, String), image::ImageError> {
    let reader = ImageReader::new(Cursor::new(raw)).with_guessed_format()?;

    let format = reader.format();

    if format == Some(image::ImageFormat::Gif) {
        return Ok((raw.to_vec(), "image/gif".to_string()));
    }

    let img = reader.decode()?;
    let (w, h) = (img.width(), img.height());

    let img = if w > IMAGE_MAX_DIMENSION_PX || h > IMAGE_MAX_DIMENSION_PX {
        let max_side = std::cmp::max(w, h);
        let ratio = f64::from(IMAGE_MAX_DIMENSION_PX) / f64::from(max_side);
        let new_w = (f64::from(w) * ratio) as u32;
        let new_h = (f64::from(h) * ratio) as u32;
        img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut buf = Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, IMAGE_JPEG_QUALITY);
    img.write_with_encoder(encoder)?;

    Ok((buf.into_inner(), "image/jpeg".to_string()))
}

/// Check if a MIME type is audio.
pub fn is_audio_mime(mime: &str) -> bool {
    mime.starts_with("audio/")
}

/// Check if an attachment is a video file.
pub fn is_video_file(filename: &str, content_type: Option<&str>) -> bool {
    let mime = content_type.unwrap_or("");
    let mime_base = mime.split(';').next().unwrap_or(mime).trim();
    if mime_base.starts_with("video/") {
        return true;
    }

    filename
        .rsplit('.')
        .next()
        .map(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "mp4" | "mov" | "m4v" | "webm" | "mkv" | "avi"
            )
        })
        .unwrap_or(false)
}

/// Extensions recognised as text-based files that can be inlined into the prompt.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "csv", "log", "md", "json", "jsonl", "yaml", "yml", "toml", "xml", "rs", "py", "js",
    "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h", "hpp", "rb", "sh", "bash", "zsh", "fish",
    "ps1", "bat", "sql", "html", "css", "scss", "less", "ini", "cfg", "conf", "env",
];

/// Exact filenames (no extension) recognised as text files.
const TEXT_FILENAMES: &[&str] = &[
    "dockerfile",
    "makefile",
    "justfile",
    "rakefile",
    "gemfile",
    "procfile",
    "vagrantfile",
    ".gitignore",
    ".dockerignore",
    ".editorconfig",
];

/// MIME types recognised as text-based (beyond `text/*`).
const TEXT_MIME_TYPES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-yaml",
    "application/x-sh",
    "application/toml",
    "application/x-toml",
];

/// Check if a file is text-based and can be inlined into the prompt.
pub fn is_text_file(filename: &str, content_type: Option<&str>) -> bool {
    let mime = content_type.unwrap_or("");
    let mime_base = mime.split(';').next().unwrap_or(mime).trim();
    if mime_base.starts_with("text/") || TEXT_MIME_TYPES.contains(&mime_base) {
        return true;
    }
    // Check extension
    if filename.contains('.') {
        if let Some(ext) = filename.rsplit('.').next() {
            if TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                return true;
            }
        }
    }
    // Check exact filename (Dockerfile, Makefile, etc.)
    TEXT_FILENAMES.contains(&filename.to_lowercase().as_str())
}

/// Download a text-based file and return it as a ContentBlock::Text.
/// Files larger than 512 KB are skipped to avoid bloating the prompt.
///
/// Pass `auth_token` for platforms that require authentication (e.g. Slack private files).
///
/// Note: the caller already guards total size via a total cap; the per-file
/// MAX_SIZE check here is intentional defense-in-depth so this function remains
/// self-contained and safe when called from other contexts.
pub async fn download_and_read_text_file(
    url: &str,
    filename: &str,
    size: u64,
    auth_token: Option<&str>,
) -> Option<(ContentBlock, u64)> {
    const MAX_SIZE: u64 = 512 * 1024; // 512 KB

    if size > MAX_SIZE {
        tracing::warn!(filename, size, "text file exceeds 512KB limit, skipping");
        return None;
    }

    let mut req = HTTP_CLIENT.get(url);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url, error = %e, "text file download failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(url, status = %resp.status(), "text file download failed");
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    let actual_size = bytes.len() as u64;

    // Defense-in-depth: verify actual download size
    if actual_size > MAX_SIZE {
        tracing::warn!(
            filename,
            size = actual_size,
            "downloaded text file exceeds 512KB limit, skipping"
        );
        return None;
    }

    // from_utf8_lossy returns Cow::Borrowed for valid UTF-8 (zero-copy)
    let text = String::from_utf8_lossy(&bytes).into_owned();

    // Dynamic fence: keep adding backticks until the fence doesn't appear in content
    let mut fence = "```".to_string();
    while text.contains(fence.as_str()) {
        fence.push('`');
    }

    debug!(filename, bytes = text.len(), "text file inlined");
    Some((
        ContentBlock::Text {
            text: format!("[File: {filename}]\n{fence}\n{text}\n{fence}"),
        },
        actual_size,
    ))
}

// --- Workspace attachment handoff (unsupported file types) ---
//
// These helpers cover the fourth branch of the adapter attachment loop: when
// an attachment is neither audio (handled by STT), nor a recognised text file
// (inlined into the prompt), nor an image (base64-encoded), we persist it
// under the agent's working directory so the agent can read it from its
// filesystem, and emit a `ContentBlock::ResourceLink` pointing at the
// saved path. The original audio / text / image branches are untouched.

/// Maximum size for a workspace-handoff attachment (25 MB).
const ATTACHMENT_MAX_SIZE: u64 = 25 * 1024 * 1024;

/// Returns true if the filename extension or MIME hint matches a type that
/// `download_and_encode_image` would accept. Used by the 4th-branch gate so
/// oversized / corrupt images aren't silently routed through persistence.
pub fn is_image_type(filename: &str, mime_hint: Option<&str>) -> bool {
    if let Some(mime) = mime_hint {
        let base = mime.split(';').next().unwrap_or(mime).trim();
        if base.starts_with("image/") {
            return true;
        }
    }
    filename
        .rsplit('.')
        .next()
        .map(str::to_lowercase)
        .map(|ext| matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp"))
        .unwrap_or(false)
}

/// Sanitize a user-supplied string into a safe single path segment.
/// Keeps ASCII `[A-Za-z0-9._-]`, replaces everything else with `_`, trims
/// leading/trailing dots, and falls back to `fallback` if the result is empty.
pub fn sanitize_path_segment(input: &str, fallback: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('.').to_string();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

/// A chat attachment that has been downloaded and saved under the agent's
/// working directory. Carries enough context to build the ACP resource block
/// and a human-readable summary line for the prompt.
#[derive(Debug, Clone)]
pub struct PersistedAttachment {
    uri: String,
    relative_path: String,
    filename: String,
    mime_type: String,
    size: u64,
}

impl PersistedAttachment {
    /// Build the ACP content block pointing at this attachment. The
    /// capability-aware serializer in `acp::connection` will transmit it as
    /// `resource_link`; agents without `embeddedContext` handle it natively.
    pub fn to_content_block(&self) -> ContentBlock {
        ContentBlock::ResourceLink {
            uri: self.uri.clone(),
            name: self.filename.clone(),
            mime_type: self.mime_type.clone(),
            size: self.size,
        }
    }
}

/// Download an attachment and persist it under
/// `<working_dir>/.openab/attachments/<platform>/<channel>/<message>/<filename>`.
/// Returns `None` if any step fails (download, write, cap) — callers treat
/// this as a best-effort drop rather than a hard error.
#[allow(clippy::too_many_arguments)]
pub async fn persist_attachment_from_url(
    working_dir: &Path,
    url: &str,
    filename: &str,
    size_hint: u64,
    mime_hint: Option<&str>,
    platform: &str,
    channel_id: &str,
    message_id: &str,
    auth_token: Option<&str>,
) -> Option<PersistedAttachment> {
    if url.is_empty() {
        return None;
    }
    if size_hint > ATTACHMENT_MAX_SIZE {
        tracing::warn!(
            filename,
            size = size_hint,
            limit = ATTACHMENT_MAX_SIZE,
            "attachment exceeds workspace-handoff limit, skipping"
        );
        return None;
    }

    let mut req = HTTP_CLIENT.get(url);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url, error = %e, "attachment download failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(url, status = %resp.status(), "attachment download failed");
        return None;
    }
    let header_mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(url, error = %e, "attachment read failed");
            return None;
        }
    };
    let actual_size = bytes.len() as u64;
    if actual_size > ATTACHMENT_MAX_SIZE {
        tracing::warn!(
            filename,
            size = actual_size,
            "downloaded attachment exceeds workspace-handoff limit"
        );
        return None;
    }

    let mime = mime_hint
        .map(str::to_string)
        .or(header_mime)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let mime = mime
        .split(';')
        .next()
        .unwrap_or(mime.as_str())
        .trim()
        .to_string();

    let rel_dir = PathBuf::from(".openab")
        .join("attachments")
        .join(sanitize_path_segment(platform, "platform"))
        .join(sanitize_path_segment(channel_id, "channel"))
        .join(sanitize_path_segment(message_id, "message"));
    let full_dir = working_dir.join(&rel_dir);
    if let Err(e) = tokio::fs::create_dir_all(&full_dir).await {
        tracing::warn!(dir = %full_dir.display(), error = %e, "failed to create attachments dir");
        return None;
    }
    let safe_filename = sanitize_path_segment(filename, "attachment");
    let full_path = full_dir.join(&safe_filename);
    if let Err(e) = tokio::fs::write(&full_path, &bytes).await {
        tracing::warn!(path = %full_path.display(), error = %e, "failed to write attachment");
        return None;
    }

    let absolute = full_path.canonicalize().unwrap_or_else(|_| full_path.clone());
    let uri = format!("file://{}", absolute.to_string_lossy());
    let relative_path = rel_dir
        .join(&safe_filename)
        .to_string_lossy()
        .into_owned();

    debug!(filename = %safe_filename, size = actual_size, path = %relative_path, "attachment persisted");

    Some(PersistedAttachment {
        uri,
        relative_path,
        filename: safe_filename,
        mime_type: mime,
        size: actual_size,
    })
}

/// Compose a "[Attached files]" summary text block listing each persisted
/// attachment by filename + size + relative path. Returns `None` when the
/// list is empty so the caller can skip prepending an empty summary.
pub fn build_attachment_summary(persisted: &[PersistedAttachment]) -> Option<String> {
    if persisted.is_empty() {
        return None;
    }
    use std::fmt::Write as _;
    let mut out = String::from("[Attached files]\n");
    for att in persisted {
        let _ = writeln!(
            out,
            "- {} ({}, {} bytes) — saved at {}",
            att.filename, att.mime_type, att.size, att.relative_path,
        );
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::new(width, height);
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn large_image_resized_to_max_dimension() {
        let png = make_png(3000, 2000);
        let (compressed, mime) = resize_and_compress(&png).unwrap();

        assert_eq!(mime, "image/jpeg");
        let result = image::load_from_memory(&compressed).unwrap();
        assert!(result.width() <= IMAGE_MAX_DIMENSION_PX);
        assert!(result.height() <= IMAGE_MAX_DIMENSION_PX);
    }

    #[test]
    fn small_image_keeps_original_dimensions() {
        let png = make_png(800, 600);
        let (compressed, mime) = resize_and_compress(&png).unwrap();

        assert_eq!(mime, "image/jpeg");
        let result = image::load_from_memory(&compressed).unwrap();
        assert_eq!(result.width(), 800);
        assert_eq!(result.height(), 600);
    }

    #[test]
    fn landscape_image_respects_aspect_ratio() {
        let png = make_png(4000, 2000);
        let (compressed, _) = resize_and_compress(&png).unwrap();

        let result = image::load_from_memory(&compressed).unwrap();
        assert_eq!(result.width(), 1200);
        assert_eq!(result.height(), 600);
    }

    #[test]
    fn portrait_image_respects_aspect_ratio() {
        let png = make_png(2000, 4000);
        let (compressed, _) = resize_and_compress(&png).unwrap();

        let result = image::load_from_memory(&compressed).unwrap();
        assert_eq!(result.width(), 600);
        assert_eq!(result.height(), 1200);
    }

    #[test]
    fn compressed_output_is_smaller_than_original() {
        let png = make_png(3000, 2000);
        let (compressed, _) = resize_and_compress(&png).unwrap();

        assert!(
            compressed.len() < png.len(),
            "compressed {} should be < original {}",
            compressed.len(),
            png.len()
        );
    }

    #[test]
    fn gif_passes_through_unchanged() {
        let gif: Vec<u8> = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3B,
        ];
        let (output, mime) = resize_and_compress(&gif).unwrap();

        assert_eq!(mime, "image/gif");
        assert_eq!(output, gif);
    }

    #[test]
    fn invalid_data_returns_error() {
        let garbage = vec![0x00, 0x01, 0x02, 0x03];
        assert!(resize_and_compress(&garbage).is_err());
    }

    #[test]
    fn video_file_detects_mime_and_common_extensions() {
        assert!(is_video_file("clip.bin", Some("video/mp4")));
        assert!(is_video_file("clip.mp4", None));
        assert!(is_video_file("clip.MOV", None));
        assert!(!is_video_file("notes.txt", Some("text/plain")));
    }

    #[test]
    fn sanitize_path_segment_applies_whitelist_and_fallback() {
        assert_eq!(sanitize_path_segment("log.txt", "x"), "log.txt");
        assert_eq!(
            sanitize_path_segment("report with spaces.pdf", "x"),
            "report_with_spaces.pdf"
        );
        assert_eq!(sanitize_path_segment("日本語.zip", "x"), "___.zip");
        // `/` → `_`, then leading dots stripped to block path traversal.
        assert_eq!(sanitize_path_segment("../evil", "x"), "_evil");
        assert_eq!(sanitize_path_segment("", "fallback"), "fallback");
        assert_eq!(sanitize_path_segment("....", "fallback"), "fallback");
    }

    #[test]
    fn is_image_type_accepts_known_mime_and_extension() {
        assert!(is_image_type("cat.PNG", None));
        assert!(is_image_type("cat", Some("image/jpeg")));
        assert!(is_image_type("cat.webp", Some("application/octet-stream")));
        assert!(!is_image_type("report.pdf", Some("application/pdf")));
        assert!(!is_image_type("notes.md", None));
        assert!(!is_image_type("", None));
    }

    fn fake_persisted(filename: &str, size: u64) -> PersistedAttachment {
        PersistedAttachment {
            uri: format!("file:///w/{filename}"),
            relative_path: format!(".openab/attachments/discord/c/m/{filename}"),
            filename: filename.to_string(),
            mime_type: "application/pdf".to_string(),
            size,
        }
    }

    #[test]
    fn attachment_summary_lists_all_entries() {
        let persisted = vec![
            fake_persisted("a.pdf", 1234),
            fake_persisted("b.pdf", 5678),
        ];
        let summary = build_attachment_summary(&persisted).expect("non-empty");
        assert!(summary.starts_with("[Attached files]"));
        assert!(summary.contains("a.pdf"));
        assert!(summary.contains("1234"));
        assert!(summary.contains("b.pdf"));
        assert!(summary.contains("5678"));
        assert!(summary.contains(".openab/attachments/discord/c/m/a.pdf"));
    }

    #[test]
    fn attachment_summary_is_none_for_empty_list() {
        assert!(build_attachment_summary(&[]).is_none());
    }

    #[test]
    fn persisted_attachment_produces_resource_link() {
        let att = fake_persisted("data.bin", 99);
        match att.to_content_block() {
            ContentBlock::ResourceLink {
                uri,
                name,
                mime_type,
                size,
            } => {
                assert_eq!(uri, "file:///w/data.bin");
                assert_eq!(name, "data.bin");
                assert_eq!(mime_type, "application/pdf");
                assert_eq!(size, 99);
            }
            _ => panic!("expected ResourceLink"),
        }
    }
}
