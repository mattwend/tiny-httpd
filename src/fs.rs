use std::{
    path::{Component, Path, PathBuf},
    str,
};

use thiserror::Error;
use tokio::fs::{self, File};

/// A safely resolved file ready to be served.
#[derive(Debug)]
pub(crate) struct ResolvedFile {
    /// Canonical filesystem path to the resolved file.
    pub(crate) canonical_path: PathBuf,
    /// Open file handle obtained during resolution to avoid a resolve/open race.
    pub(crate) file: File,
    /// File length in bytes from metadata gathered during resolution.
    pub(crate) content_length: u64,
}

/// Errors returned while mapping a request path to a safe filesystem path.
#[derive(Debug, Error)]
pub(crate) enum ResolveError {
    #[error("request path must start with `/`")]
    BadTarget,
    #[error("invalid percent encoding in request path")]
    InvalidPercentEncoding,
    #[error("request path is not valid UTF-8 after percent decoding")]
    InvalidUtf8,
    #[error("request path contains an encoded slash")]
    EncodedSlash,
    #[error("request path contains a null byte")]
    NullByte,
    #[error("request path contains a parent-directory component")]
    Traversal,
    #[error("candidate escapes the configured content root")]
    Escape,
    #[error("requested file was not found")]
    NotFound,
    #[error("filesystem error while resolving request path: {0}")]
    Io(#[from] std::io::Error),
}

/// Decodes and validates the request path, then returns relative lookup candidates.
///
/// # Arguments
/// * `request_path` - URI path component from the HTTP request target.
///
/// # Returns
/// Candidate relative paths in RFC lookup order.
///
/// # Errors
/// Returns [`ResolveError`] for malformed percent encoding, invalid UTF-8,
/// traversal components, or request paths that do not begin with `/`.
pub(crate) fn candidate_paths(request_path: &str) -> Result<Vec<PathBuf>, ResolveError> {
    if !request_path.starts_with('/') {
        return Err(ResolveError::BadTarget);
    }

    let decoded = decode_percent_path(request_path)?;
    if !decoded.starts_with('/') {
        return Err(ResolveError::BadTarget);
    }
    if decoded != "/" && decoded.starts_with("//") {
        return Err(ResolveError::BadTarget);
    }

    let relative = &decoded[1..];
    if relative.contains("//") {
        return Err(ResolveError::BadTarget);
    }
    validate_relative_path(relative)?;

    if relative.is_empty() {
        return Ok(vec![PathBuf::from("index.html")]);
    }

    if decoded.ends_with('/') {
        return Ok(vec![Path::new(relative).join("index.html")]);
    }

    Ok(vec![
        PathBuf::from(relative),
        Path::new(relative).join("index.html"),
    ])
}

/// Resolves a request path to a canonical, contained regular file and opens it.
///
/// # Arguments
/// * `content_root` - Canonical content root from startup validation.
/// * `request_path` - URI path from the incoming request.
///
/// # Returns
/// A [`ResolvedFile`] containing the canonical path, an open file handle, and
/// the file length gathered during resolution.
///
/// # Errors
/// Returns [`ResolveError`] when the path is malformed, unsafe, missing, not a
/// regular file, outside `content_root`, or cannot be inspected or opened.
pub(crate) async fn resolve_file(
    content_root: &Path,
    request_path: &str,
) -> Result<ResolvedFile, ResolveError> {
    let candidates = candidate_paths(request_path)?;
    let mut allow_directory_index_fallback = false;

    for (index, candidate) in candidates.iter().enumerate() {
        if index == 1 && !allow_directory_index_fallback {
            continue;
        }

        let full_path = content_root.join(candidate);
        let canonical = match fs::canonicalize(&full_path).await {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(ResolveError::Io(error)),
        };

        // There is a theoretical TOCTOU window between canonicalize and
        // open: an attacker with write access to the content root could
        // replace the file with a symlink between the two calls.  This is
        // accepted because the server is designed to run in a minimal
        // container where the content root is read-only.
        if !canonical.starts_with(content_root) {
            return Err(ResolveError::Escape);
        }

        let metadata = fs::metadata(&canonical).await?;
        if index == 0 && candidates.len() == 2 {
            allow_directory_index_fallback = metadata.is_dir();
        }
        if metadata.is_file() {
            let file = File::open(&canonical).await?;
            return Ok(ResolvedFile {
                canonical_path: canonical,
                file,
                content_length: metadata.len(),
            });
        }
    }

    Err(ResolveError::NotFound)
}

/// Rejects path components that would escape or invalidate relative lookup.
fn validate_relative_path(relative: &str) -> Result<(), ResolveError> {
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Err(ResolveError::Traversal),
            Component::RootDir | Component::Prefix(_) => return Err(ResolveError::BadTarget),
        }
    }
    Ok(())
}

/// Percent-decodes request path while rejecting encoded slashes and null bytes.
fn decode_percent_path(path: &str) -> Result<String, ResolveError> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(ResolveError::InvalidPercentEncoding);
            }
            let high = hex_value(bytes[index + 1]).ok_or(ResolveError::InvalidPercentEncoding)?;
            let low = hex_value(bytes[index + 2]).ok_or(ResolveError::InvalidPercentEncoding)?;
            let decoded_byte = (high << 4) | low;
            if decoded_byte == b'/' {
                return Err(ResolveError::EncodedSlash);
            }
            if decoded_byte == b'\0' {
                return Err(ResolveError::NullByte);
            }
            decoded.push(decoded_byte);
            index += 3;
        } else {
            if bytes[index] == b'\0' {
                return Err(ResolveError::NullByte);
            }
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).map_err(|_| ResolveError::InvalidUtf8)
}

/// Converts one ASCII hex digit into numeric value.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
