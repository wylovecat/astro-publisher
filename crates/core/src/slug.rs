//! `_0x` slug allocation.
//!
//! Course chapters use an explicit frontmatter slug of the form `_0x0001`,
//! allocated from a **site-wide** sequence (not per book): the current maximum
//! across all posts is `_0x0012`, so the next chapter gets `_0x0013`.
//!
//! See the plan §1.3.

use crate::frontmatter::Frontmatter;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

fn slug_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^_0x([0-9a-fA-F]{1,8})$").expect("valid slug regex"))
}

/// Parse a `_0xNNNN` slug into its numeric value.
pub fn parse_counter(slug: &str) -> Option<u32> {
    let caps = slug_re().captures(slug.trim())?;
    u32::from_str_radix(&caps[1], 16).ok()
}

/// Format a counter as a slug, lower-case hex padded to at least 4 digits.
pub fn format_slug(counter: u32) -> String {
    format!("_0x{counter:04x}")
}

/// The hex form used in chapter titles, e.g. `0x0001` (no underscore).
pub fn format_title_counter(counter: u32) -> String {
    format!("0x{counter:04x}")
}

/// Highest allocated counter across the given frontmatters, if any.
pub fn max_counter<'a>(frontmatters: impl IntoIterator<Item = &'a Frontmatter>) -> Option<u32> {
    frontmatters
        .into_iter()
        .filter_map(|fm| fm.slug.as_deref())
        .filter_map(parse_counter)
        .max()
}

/// Allocate the next slug given the current maximum.
///
/// The sequence starts at `_0x0001`: `_0x0000` is reserved for the
/// non-chapter `how-to-write-solutions` post, and the convention is that
/// chapters begin at 1 (see plan §1.3).
pub fn next_slug(current_max: Option<u32>) -> String {
    let next = current_max.map(|m| m + 1).unwrap_or(1);
    format_slug(next)
}

/// Scan a directory tree for post frontmatters and return the next free slug.
///
/// Reads only the frontmatter block of each `.md` file, skipping files whose
/// name starts with `_` (excluded from the collection by the site's glob).
pub fn allocate_slug(repo_blog_dir: &Path) -> String {
    next_slug(scan_max_counter(repo_blog_dir))
}

/// Highest `_0x` counter found under `blog_dir`.
pub fn scan_max_counter(blog_dir: &Path) -> Option<u32> {
    let mut highest: Option<u32> = None;

    for path in crate::posts::iter_post_files(blog_dir) {
        let Ok(file) = crate::frontmatter::read_markdown(&path) else {
            continue;
        };
        if let Some(counter) = file.frontmatter.slug.as_deref().and_then(parse_counter) {
            highest = Some(highest.map_or(counter, |h| h.max(counter)));
        }
    }

    highest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        assert_eq!(parse_counter("_0x0012"), Some(18));
        assert_eq!(parse_counter("_0x000a"), Some(10));
        assert_eq!(parse_counter("_0x0000"), Some(0));
        assert_eq!(parse_counter("lg-P1001"), None);
        assert_eq!(parse_counter(" _0x000f "), Some(15));

        assert_eq!(format_slug(19), "_0x0013");
        assert_eq!(format_slug(10), "_0x000a");
        assert_eq!(format_title_counter(1), "0x0001");
    }

    #[test]
    fn allocates_next_after_max() {
        assert_eq!(next_slug(Some(18)), "_0x0013");
        assert_eq!(next_slug(None), "_0x0001");
    }
}
