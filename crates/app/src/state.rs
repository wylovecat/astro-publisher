//! Application state shared by the views.
//!
//! The persisted settings and the repository detection live in
//! `astro_publisher_core::config`, because the CLI and the MCP server need the
//! same answers; they are re-exported here so views keep importing them from
//! `crate::state`.

use std::path::PathBuf;

pub use astro_publisher_core::config::{Config, looks_like_repo};

/// Which section of the app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
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
            View::Posts => "文章",
            View::Editor => "编辑",
            View::Chapter => "新建章节",
            View::Outline => "大纲",
            View::Publish => "发布",
            View::Settings => "设置",
        }
    }

    pub fn all() -> [View; 6] {
        [
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
        }
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

