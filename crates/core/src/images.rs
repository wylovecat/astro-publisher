//! Pasted images: where they land in the blog repository and how a post
//! references them.
//!
//! Pasting a screenshot into the editor has to answer two questions — what file
//! name to use, and what Markdown to insert — and both are pure decisions about
//! paths, so they live here rather than in the view. The view only supplies the
//! bytes it read off the clipboard and the media type the platform reported.
//!
//! Images go under the site's static directory, `public/images/`, and are
//! referenced by absolute URL (`/images/<name>`), which is how Astro serves
//! anything in `public/`.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Directory pasted images are written to, relative to the repository root.
pub const IMAGE_DIR: &str = "public/images";
/// URL prefix the site serves that directory under.
pub const URL_PREFIX: &str = "/images/";
/// Extensions a pasted image may carry. Anything else is refused rather than
/// written with a guessed extension the site would then fail to serve.
const KNOWN_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "webp", "gif", "svg", "bmp", "tiff", "ico"];

#[derive(Debug, Error)]
pub enum ImageError {
    #[error("剪贴板里的图片没有数据")]
    Empty,
    #[error("不认识的图片格式：`{0}`")]
    UnknownFormat(String),
    #[error("写入图片失败：{0}")]
    Io(String),
}

/// An image written into the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImage {
    /// Absolute path of the file on disk.
    pub path: PathBuf,
    /// URL the site serves it at, e.g. `/images/lg-P1001-20261013-183000.png`.
    pub url_path: String,
}

/// Write a pasted image into the repository.
///
/// `stamp` is supplied by the caller (rather than read from the clock here) so
/// the file name is reproducible in tests.
pub fn store_image(
    repo_root: &Path,
    post_stem: &str,
    extension: &str,
    bytes: &[u8],
    stamp: &str,
) -> Result<StoredImage, ImageError> {
    if bytes.is_empty() {
        return Err(ImageError::Empty);
    }

    let extension = normalize_extension(extension)
        .ok_or_else(|| ImageError::UnknownFormat(extension.trim().to_owned()))?;

    let name = format!("{}-{stamp}.{extension}", sanitize_stem(post_stem));
    let dir = repo_root.join(IMAGE_DIR);
    std::fs::create_dir_all(&dir).map_err(|err| ImageError::Io(err.to_string()))?;

    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|err| ImageError::Io(err.to_string()))?;

    Ok(StoredImage {
        path,
        url_path: format!("{URL_PREFIX}{name}"),
    })
}

/// The Markdown that inserts a stored image.
pub fn markdown_reference(url_path: &str, alt: &str) -> String {
    format!("![{alt}]({url_path})")
}

/// Lower-case extension, accepted only when the site can serve it.
fn normalize_extension(raw: &str) -> Option<String> {
    let extension = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    // `jpg` and `jpeg` are the same format; keep whichever was given.
    KNOWN_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(extension)
}

/// Make a post stem safe to use as part of a file name.
///
/// Post stems come from the content directory and are normally already clean
/// (`lg-P1001`), but they are user-owned data and end up in a path.
fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let trimmed = cleaned.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "image".to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("an-images-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    #[test]
    fn writes_under_public_images_and_returns_the_url() {
        let repo = scratch("store");
        let stored = store_image(&repo, "lg-P1001", "png", b"png-bytes", "20261013-183000")
            .expect("stores");

        assert_eq!(stored.path, repo.join("public/images/lg-P1001-20261013-183000.png"));
        assert_eq!(stored.url_path, "/images/lg-P1001-20261013-183000.png");
        assert_eq!(std::fs::read(&stored.path).expect("read back"), b"png-bytes");
        assert_eq!(markdown_reference(&stored.url_path, ""), "![](/images/lg-P1001-20261013-183000.png)");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn normalises_the_extension_and_sanitises_the_stem() {
        let repo = scratch("names");

        let stored = store_image(&repo, "lg-P1001", ".PNG", b"x", "s").expect("stores");
        assert!(stored.path.ends_with("lg-P1001-s.png"));

        // Separators and spaces must not survive into the path.
        let stored = store_image(&repo, "a/b c", "jpeg", b"x", "s").expect("stores");
        assert!(stored.path.ends_with("a-b-c-s.jpeg"));

        // A stem that sanitises away entirely still yields a usable name.
        let stored = store_image(&repo, "///", "png", b"x", "s").expect("stores");
        assert!(stored.path.ends_with("image-s.png"));

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn refuses_empty_bodies_and_unknown_formats() {
        let repo = scratch("refuse");

        assert!(matches!(
            store_image(&repo, "lg-P1001", "png", b"", "s"),
            Err(ImageError::Empty)
        ));
        assert!(matches!(
            store_image(&repo, "lg-P1001", "exe", b"x", "s"),
            Err(ImageError::UnknownFormat(_))
        ));
        // Nothing should have been left behind by the refusals.
        assert!(!repo.join(IMAGE_DIR).exists());

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn markdown_reference_carries_the_alt_text() {
        assert_eq!(
            markdown_reference("/images/a.png", "截图"),
            "![截图](/images/a.png)"
        );
    }
}
