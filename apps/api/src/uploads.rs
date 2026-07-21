use axum::http::StatusCode;
use bytes::Bytes;

use crate::ApiError;

pub(crate) const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) struct UploadedFile {
    pub(crate) original_filename: String,
    pub(crate) content_type: &'static str,
    pub(crate) extension: &'static str,
    pub(crate) bytes: Bytes,
}

impl UploadedFile {
    pub(crate) fn validate(original_filename: String, bytes: Bytes) -> Result<Self, ApiError> {
        if original_filename.trim().is_empty()
            || original_filename.chars().count() > 255
            || original_filename.chars().any(char::is_control)
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The original filename is missing or too long.",
            ));
        }
        if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Files must be between 1 byte and 10 MiB.",
            ));
        }
        let (content_type, extension) = detect_file_type(&bytes).ok_or(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Upload a PDF, JPEG, PNG, or WebP file.",
        ))?;
        Ok(Self {
            original_filename,
            content_type,
            extension,
            bytes,
        })
    }
}

pub(crate) fn multipart_error(error: axum::extract::multipart::MultipartError) -> ApiError {
    tracing::debug!(%error, "invalid multipart upload request");
    ApiError(StatusCode::BAD_REQUEST, "The upload request was invalid.")
}

fn detect_file_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"%PDF-") {
        Some(("application/pdf", "pdf"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_file_signatures() {
        assert_eq!(
            UploadedFile::validate("menu.pdf".into(), Bytes::from_static(b"%PDF-1.7"))
                .unwrap()
                .content_type,
            "application/pdf"
        );
        assert!(
            UploadedFile::validate("menu.pdf".into(), Bytes::from_static(b"fake.pdf")).is_err()
        );
    }
}
