//! Application state shared by the views.
//!
//! The persisted settings and the repository detection live in
//! `astro_publisher_core::config`, because the CLI and the MCP server need the
//! same answers; they are re-exported here so views keep importing them from
//! `crate::state`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use astro_publisher_core::config::{Config, looks_like_repo};

/// Files this session of the app wrote to, repo-relative.
///
/// The publish pipeline needs to know which files "the user" touched, so that
/// committing does not become a `git add -A` over whatever else the blog
/// checkout happens to contain (plan §5, step 3). Every write path registers
/// here: saving a post, pasting an image, creating a chapter, editing an outline.
///
/// Cloning shares one set, which is how the views get it: each of them is a
/// separate gpui entity, and a shared handle is simpler than threading an
/// event back to the app shell from each of them.
#[derive(Clone, Default)]
pub struct SessionFiles(Arc<Mutex<BTreeSet<String>>>);

impl SessionFiles {
    /// Record a file the session changed. Returns false if it already was in
    /// the set, so callers can ignore duplicates.
    ///
    /// Absolute paths are made repo-relative; anything outside the repository
    /// is kept verbatim, because refusing it would hide the write rather than
    /// surface it.
    pub fn record(&self, repo: &Path, path: &Path) -> bool {
        let relative = path
            .strip_prefix(repo)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.is_empty() {
            return false;
        }
        self.0.lock().expect("session file set").insert(relative)
    }

    /// Whether this session wrote the given repo-relative path.
    pub fn contains(&self, path: &str) -> bool {
        self.0
            .lock()
            .expect("session file set")
            .contains(&path.replace('\\', "/"))
    }

    /// How many files this session has written.
    pub fn len(&self) -> usize {
        self.0.lock().expect("session file set").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_are_stored_relative_to_the_repository() {
        let files = SessionFiles::default();
        assert!(
            files.record(
                Path::new("/srv/blog"),
                Path::new("/srv/blog/src/data/blog/lgj/lg-B2620.md")
            )
        );
        assert!(files.contains("src/data/blog/lgj/lg-B2620.md"));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn the_same_file_is_recorded_once() {
        let files = SessionFiles::default();
        assert!(files.record(Path::new("/srv/blog"), Path::new("/srv/blog/a.md")));
        assert!(!files.record(Path::new("/srv/blog"), Path::new("/srv/blog/a.md")));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn membership_ignores_windows_separators() {
        let files = SessionFiles::default();
        files.record(Path::new(r"C:\blog"), Path::new(r"C:\blog\public\images\a.png"));
        assert!(files.contains("public/images/a.png"));
        assert!(files.contains(r"public\images\a.png"));
        assert!(!files.contains("public/images/b.png"));
    }

    #[test]
    fn a_clone_shares_one_set() {
        let files = SessionFiles::default();
        let clone = files.clone();
        clone.record(Path::new("/srv/blog"), Path::new("/srv/blog/a.md"));
        assert!(files.contains("a.md"), "views share the session's files");
    }
}

/// Which section of the app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Posts,
    Editor,
    Chapter,
    Outline,
    Publish,
    Settings,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Dashboard => "仪表盘",
            View::Posts => "文章",
            View::Editor => "编辑",
            View::Chapter => "新建章节",
            View::Outline => "大纲",
            View::Publish => "发布",
            View::Settings => "设置",
        }
    }

    pub fn all() -> [View; 7] {
        [
            View::Dashboard,
            View::Posts,
            View::Editor,
            View::Chapter,
            View::Outline,
            View::Publish,
            View::Settings,
        ]
    }
}

/// A compact row for the post list.
#[derive(Debug, Clone)]
pub struct PostRow {
    pub path: PathBuf,
    pub relative: String,
    pub title: String,
    pub draft: bool,
    pub url: String,
    pub tags: Vec<String>,
    /// `modDatetime`, falling back to `pubDatetime`: the site's own notion of
    /// when a post last changed. `None` when the frontmatter has neither.
    pub modified: Option<String>,
}

impl PostRow {
    pub fn from_post(post: &astro_publisher_core::posts::Post) -> Self {
        Self {
            path: post.path.clone(),
            relative: post.relative.clone(),
            title: post
                .frontmatter
                .title
                .clone()
                .unwrap_or_else(|| "(无标题)".to_owned()),
            draft: post.frontmatter.draft.unwrap_or(false),
            url: post.url_path(),
            tags: post.frontmatter.tags.clone(),
            modified: post
                .frontmatter
                .mod_datetime
                .clone()
                .or_else(|| post.frontmatter.pub_datetime.clone()),
        }
    }

    /// Free-text match behind the post page's search box.
    ///
    /// Covers the same ground as
    /// `astro_publisher_core::posts::Post::matches_query`, over the fields this
    /// compact row keeps, so the list and the editor agree on what a query
    /// matches.
    pub fn matches_query(&self, query: &str) -> bool {
        astro_publisher_core::posts::text_matches(&self.title, query)
            || astro_publisher_core::posts::text_matches(&self.relative, query)
            || astro_publisher_core::posts::text_matches(&self.url, query)
            || self
                .tags
                .iter()
                .any(|tag| astro_publisher_core::posts::text_matches(tag, query))
    }
}

/// Load the post list for a configuration, tolerating an unset repo path.
pub fn load_posts(config: &Config) -> anyhow::Result<Vec<PostRow>> {
    let repo = config.repo();
    let blog = config.blog();

    if !looks_like_repo(&repo) {
        anyhow::bail!(
            "仓库路径不对：{}\n请在「设置」里改成博客仓库根目录（含 package.json 与 src/data/blog），\
             或设置环境变量 ASTRO_PUBLISHER_REPO。",
            repo.display()
        );
    }

    if !blog.is_dir() {
        anyhow::bail!("仓库里找不到内容目录：{}", blog.display());
    }

    let posts = astro_publisher_core::posts::list_posts(&blog)?;
    Ok(posts.iter().map(PostRow::from_post).collect())
}

