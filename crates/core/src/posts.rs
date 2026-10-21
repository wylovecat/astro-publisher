//! Post listing, reading, and frontmatter-level edits.
//!
//! Scope: the blog content directory (`src/data/blog`). Files whose name starts
//! with `_` are excluded — the site's content collection uses the glob
//! `**/[^_]*.md`, so they are not posts.

use crate::frontmatter::{Frontmatter, MarkdownFile, FrontmatterError};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from post operations.
#[derive(Debug, Error)]
pub enum PostError {
    #[error(transparent)]
    Frontmatter(#[from] FrontmatterError),
    #[error("failed to enumerate {path}: {source}")]
    Walk {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("post {0} already exists")]
    AlreadyExists(String),
}

/// A post on disk.
#[derive(Debug, Clone)]
pub struct Post {
    /// Absolute path to the `.md` file.
    pub path: PathBuf,
    /// Path relative to the blog root, e.g. `lgj/lg-B2600.md`.
    pub relative: String,
    /// First path segment when nested, e.g. `lgj`. `None` for the blog root.
    pub group: Option<String>,
    pub frontmatter: Frontmatter,
    /// Body with the frontmatter block removed.
    pub body: String,
}

impl Post {
    /// URL path as the site would publish it (`/posts/...`).
    ///
    /// Mirrors `src/utils/getPath.ts`: directory segments are slugified and the
    /// last segment is the explicit `slug` when present, else the file stem.
    pub fn url_path(&self) -> String {
        let mut segments: Vec<String> = Vec::new();
        if let Some(group) = &self.group {
            if !group.starts_with('_') {
                segments.push(site_group_segment(group));
            }
        }

        let last = match self.frontmatter.slug.as_deref() {
            Some(s) if !s.trim().is_empty() => slugify(s.trim()),
            _ => slugify(&file_stem(&self.path)),
        };
        segments.push(last);
        format!("/posts/{}", segments.join("/"))
    }

    /// Whether this post matches a free-text query from a search box.
    ///
    /// Looks at the title, slug, path, and tags. The body is deliberately left
    /// out: the desktop list re-filters on every keystroke, and scanning whole
    /// articles there would make typing crawl.
    pub fn matches_query(&self, query: &str) -> bool {
        let title = self.frontmatter.title.as_deref().unwrap_or_default();
        let slug = self.frontmatter.slug.as_deref().unwrap_or_default();

        text_matches(title, query)
            || text_matches(slug, query)
            || text_matches(&self.relative, query)
            || self
                .frontmatter
                .tags
                .iter()
                .any(|tag| text_matches(tag, query))
    }
}

/// Case-insensitive substring test behind [`Post::matches_query`].
///
/// An empty or whitespace-only query matches everything, so callers can hand
/// over raw input text without special-casing "no filter".
pub fn text_matches(haystack: &str, query: &str) -> bool {
    let needle = query.trim();
    needle.is_empty() || haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Slugify a directory segment exactly the way the site does.
///
/// `src/utils/getPath.ts` runs every directory segment through
/// `slugifyStr`, which is lodash's `kebabCase`. That is not the same as the
/// simpler [`slugify`] used for the last URL segment: kebabCase treats a
/// letter/digit boundary and underscores as word breaks, so `csp-j2` becomes
/// `csp-j-2` and `csp_j2` becomes `csp-j-2`. Using the raw directory name
/// produces links (and live-preview URLs) the site never serves.
pub fn site_group_segment(group: &str) -> String {
    let chars: Vec<char> = group.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();

    for (index, ch) in chars.iter().copied().enumerate() {
        if !ch.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }

        let prev = index.checked_sub(1).map(|i| chars[i]);
        let next = chars.get(index + 1).copied();
        let boundary = !word.is_empty()
            && prev.is_some_and(|p| {
                // letter <-> digit
                (p.is_numeric() != ch.is_numeric())
                    // lower -> upper (fooBar)
                    || (p.is_lowercase() && ch.is_uppercase())
                    // end of an acronym (XMLHttp -> XML / Http)
                    || (p.is_uppercase() && ch.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
        if boundary {
            words.push(std::mem::take(&mut word));
        }
        word.push(ch);
    }
    if !word.is_empty() {
        words.push(word);
    }

    words
        .into_iter()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

/// Slugify a path segment the way `getPath.ts` does (lowercase, spaces to `-`).
fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if ch == '-' || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if ch.is_whitespace() {
            if !last_dash && !out.is_empty() {
                out.push('-');
                last_dash = true;
            }
        } else {
            // Non-ASCII (CJK) characters are kept as-is by Astro's slugifier.
            out.push(ch);
            last_dash = false;
        }
    }
    out.trim_matches('-').to_owned()
}

/// True when the file is a post the site would collect.
fn is_post_file(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return false;
    };
    if name.starts_with('_') || !name.ends_with(".md") {
        return false;
    }
    // Directories starting with `_` are excluded too.
    path.components().all(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| !s.starts_with('_'))
            .unwrap_or(false)
    })
}

/// Iterate every post file under `blog_dir` (skipping `_`-prefixed names).
pub fn iter_post_files(blog_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(blog_dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('_') {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else if name.ends_with(".md") {
            out.push(path);
        }
    }
}

/// List every post under the blog directory.
pub fn list_posts(blog_dir: &Path) -> Result<Vec<Post>, PostError> {
    let mut posts = Vec::new();
    for path in iter_post_files(blog_dir) {
        if !is_post_file(&path) {
            continue;
        }
        let file = crate::frontmatter::read_markdown(&path)?;
        posts.push(to_post(path, blog_dir, file));
    }
    Ok(posts)
}

fn to_post(path: PathBuf, blog_dir: &Path, file: MarkdownFile) -> Post {
    let relative = path
        .strip_prefix(blog_dir)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string());

    let group = relative.split_once('/').map(|(g, _)| g.to_owned());

    Post {
        path,
        relative,
        group,
        frontmatter: file.frontmatter,
        body: file.body,
    }
}

/// Read a single post.
pub fn get_post(path: &Path, blog_dir: &Path) -> Result<Post, PostError> {
    let file = crate::frontmatter::read_markdown(path)?;
    Ok(to_post(path.to_path_buf(), blog_dir, file))
}

/// All tags across the corpus, sorted and de-duplicated.
pub fn list_tags(blog_dir: &Path) -> Result<Vec<String>, PostError> {
    let mut tags: Vec<String> = list_posts(blog_dir)?
        .into_iter()
        .flat_map(|p| p.frontmatter.tags)
        .collect();
    tags.sort();
    tags.dedup();
    Ok(tags)
}

/// A file stem derived from a post title.
///
/// ASCII-only and dash-separated, because the stem becomes a file name and,
/// for a post with no explicit `slug`, the last URL segment. A title written
/// entirely in CJK has no ASCII to keep, so it falls back to a timestamp rather
/// than to an empty name — a caller then names the file explicitly.
pub fn slugify_stem(title: &str) -> String {
    let ascii: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = ascii
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        // Same trick the editor uses for pasted image names: keep the leading
        // digits of the site-style timestamp.
        let digits: String = crate::now_local().chars().filter(char::is_ascii_digit).collect();
        let stamp = if digits.len() >= 14 {
            &digits[..14]
        } else {
            digits.as_str()
        };
        format!("post-{stamp}")
    } else {
        collapsed
    }
}

/// Input for creating a plain post (not a course chapter).
#[derive(Debug, Clone)]
pub struct NewPost {
    /// Subdirectory under the blog root, e.g. `solu`. Empty for the root.
    pub group: String,
    /// File stem, without extension. Also the URL segment when `slug` is empty.
    pub file_name: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// Explicit frontmatter slug; empty means "derive from the file name".
    pub slug: Option<String>,
    pub draft: bool,
    pub featured: bool,
    pub author: String,
}

/// Create a new plain post. Fails if the target file already exists.
pub fn create_post(blog_dir: &Path, input: &NewPost) -> Result<Post, PostError> {
    let dir = if input.group.trim().is_empty() {
        blog_dir.to_path_buf()
    } else {
        blog_dir.join(input.group.trim())
    };
    let path = dir.join(format!("{}.md", input.file_name.trim()));

    if path.exists() {
        return Err(PostError::AlreadyExists(path.display().to_string()));
    }

    let now = crate::now_local();
    let frontmatter = Frontmatter {
        author: Some(input.author.clone()),
        pub_datetime: Some(now.clone()),
        mod_datetime: Some(now),
        title: Some(input.title.clone()),
        featured: Some(input.featured),
        draft: Some(input.draft),
        slug: input
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_owned()),
        tags: input.tags.clone(),
        description: Some(input.description.clone()),
    };

    let file = MarkdownFile {
        frontmatter,
        body: format!("\n## {}\n\n", input.title),
        line_ending: "\n".to_owned(),
        had_bom: true,
    };

    crate::frontmatter::write_markdown(&path, &file)?;

    let mut post = get_post(&path, blog_dir)?;
    post.body = file.body;
    Ok(post)
}

/// A frontmatter-level edit, applied surgically.
pub fn update_frontmatter(
    path: &Path,
    edit: impl FnOnce(&mut Frontmatter),
) -> Result<Frontmatter, PostError> {
    let mut file = crate::frontmatter::read_markdown(path)?;
    edit(&mut file.frontmatter);
    crate::frontmatter::write_markdown(path, &file)?;
    Ok(file.frontmatter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_keeps_cjk() {
        assert_eq!(slugify("Solu"), "solu");
        assert_eq!(slugify("lg-P1001"), "lg-p1001");
        assert_eq!(slugify("_0x0001"), "_0x0001");
    }

    #[test]
    fn directory_segments_match_lodash_kebab_case() {
        // Values checked against the site's `lodash.kebabcase` directly.
        assert_eq!(site_group_segment("csp-j2"), "csp-j-2");
        assert_eq!(site_group_segment("csp_j2"), "csp-j-2");
        assert_eq!(site_group_segment("lg-B2600"), "lg-b-2600");
        assert_eq!(site_group_segment("P5660"), "p-5660");
        assert_eq!(site_group_segment("fooBar"), "foo-bar");
        assert_eq!(site_group_segment("XMLHttpRequest"), "xml-http-request");
        assert_eq!(site_group_segment("lgj"), "lgj");
        assert_eq!(site_group_segment("深入浅出"), "深入浅出");
    }

    #[test]
    fn url_path_slugifies_the_directory_but_not_the_explicit_slug() {
        let post = Post {
            path: PathBuf::from("/blog/csp-j2/lg-P5660.md"),
            relative: "csp-j2/lg-P5660.md".to_owned(),
            group: Some("csp-j2".to_owned()),
            frontmatter: Frontmatter {
                slug: Some("_0x0013".to_owned()),
                ..Default::default()
            },
            body: String::new(),
        };
        assert_eq!(post.url_path(), "/posts/csp-j-2/_0x0013");
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(text_matches("anything", ""));
        assert!(text_matches("anything", "   "));
    }

    #[test]
    fn text_matches_ignores_case_and_surrounding_space() {
        assert!(text_matches("lg-P1001 阶乘", " p1001 "));
        assert!(text_matches("区间合并", "区间"));
        assert!(!text_matches("区间合并", "线段"));
    }

    #[test]
    fn matches_query_covers_title_slug_path_and_tags() {
        let post = Post {
            path: PathBuf::from("/blog/lgj/lg-P1001.md"),
            relative: "lgj/lg-P1001.md".to_owned(),
            group: Some("lgj".to_owned()),
            frontmatter: Frontmatter {
                title: Some("A+B Problem".to_owned()),
                slug: Some("a-plus-b".to_owned()),
                tags: vec!["模拟".to_owned(), "入门".to_owned()],
                ..Default::default()
            },
            body: "the quick brown fox".to_owned(),
        };

        assert!(post.matches_query("a+b"), "title is searched");
        assert!(post.matches_query("A-PLUS"), "slug search is case-insensitive");
        assert!(post.matches_query("lgj/"), "path is searched");
        assert!(post.matches_query("入门"), "tags are searched");

        assert!(!post.matches_query("线段树"), "unrelated query does not match");
        // The body is intentionally out of scope: searching it would mean
        // rescanning every article on each keystroke.
        assert!(!post.matches_query("quick"), "body is not searched");
    }
}
