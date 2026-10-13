//! Chapter creation: the tool's highest-value operation.
//!
//! Creating a course chapter touches two files that must stay consistent:
//!
//! 1. the chapter post itself (`<group>/lg-<problem>.md`), built from the same
//!    frontmatter/body template the existing chapters use;
//! 2. the book's outline line, which links the chapter and marks it done.
//!
//! The write is all-or-nothing: on any failure the chapter file is removed and
//! the outline is restored from the in-memory snapshot taken before editing
//! (plan §3.2, step 6).

use crate::frontmatter::{Frontmatter, MarkdownFile};
use crate::outline::{ListMarker, Marker, Outline, OutlineEntry, render_entry};
use crate::slug;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChapterError {
    #[error(transparent)]
    Outline(#[from] crate::outline::OutlineError),
    #[error(transparent)]
    Frontmatter(#[from] crate::frontmatter::FrontmatterError),
    #[error("book `{0}` was not found in books.json")]
    BookNotFound(String),
    #[error("section `{section}` was not found in {file}")]
    SectionNotFound { section: String, file: String },
    #[error("chapter file already exists: {0}")]
    ChapterExists(String),
    #[error("outline did not contain the new chapter link after writing; rolled back")]
    SelfCheckFailed,
    #[error("rollback failed for {path}: {source}")]
    RollbackFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// The book a chapter belongs to, as read from `books.json`.
#[derive(Debug, Clone)]
pub struct BookRef {
    pub id: String,
    pub title: String,
    /// Outline post id, e.g. `lgj/lg-index`.
    pub index_id: String,
}

impl BookRef {
    /// Content subdirectory holding this book's chapters (`lgj`).
    pub fn group(&self) -> String {
        self.index_id
            .split_once('/')
            .map(|(group, _)| group.to_owned())
            .unwrap_or_else(|| self.index_id.clone())
    }

    /// Outline file path relative to the blog root (`lgj/lg-index.md`).
    pub fn index_path(&self) -> String {
        format!("{}.md", self.index_id)
    }
}

/// Input for creating a chapter.
#[derive(Debug, Clone)]
pub struct NewChapter {
    /// Problem id, e.g. `B2620`.
    pub problem_id: String,
    /// Full problem title, e.g. `【深基3.例1】数字比较`.
    pub title: String,
    /// Outline section (chapter heading) the problem belongs to.
    pub section: String,
    /// Optional Bilibili video URL to link on the outline line.
    pub video_url: Option<String>,
    pub author: String,
    /// When true, `[x]` is kept in sync with the solution link (lgj semantics).
    /// `lgs` marks progress with videos, so it leaves the checkbox alone.
    pub check_implies_solution: bool,
}

/// What a successful creation did, for reporting and diff preview.
#[derive(Debug, Clone)]
pub struct ChapterOutcome {
    pub chapter_path: PathBuf,
    pub slug: String,
    pub title_line: String,
    pub outline_path: PathBuf,
    /// The outline line before the edit, if the entry already existed.
    pub outline_before: Option<String>,
    pub outline_after: String,
    pub inserted_entry: bool,
}

/// Render the body skeleton every chapter uses (plan §1.3).
pub fn chapter_body_template() -> String {
    "\n## 题目描述\n\n\n## 输入格式\n\n\n## 输出格式\n\n\n## 题目分析\n\n\n## 代码实现\n\n```cpp\n\n```\n".to_owned()
}

/// Build the frontmatter for a new chapter.
pub fn chapter_frontmatter(
    counter: u32,
    title: &str,
    book_title: &str,
    author: &str,
    slug_value: &str,
) -> Frontmatter {
    let now = crate::now_local();
    Frontmatter {
        author: Some(author.to_owned()),
        pub_datetime: Some(now.clone()),
        mod_datetime: Some(now),
        // Titles read `0x0001 【深基1.例2】简单的分苹果`.
        title: Some(format!("{} {title}", slug::format_title_counter(counter))),
        featured: Some(false),
        draft: Some(false),
        slug: Some(slug_value.to_owned()),
        tags: vec![book_title.to_owned()],
        description: Some(format!("{title}题解。")),
    }
}

/// Create a chapter and wire it into the book outline.
///
/// `blog_dir` is `src/data/blog`; everything else is resolved relative to it.
pub fn create_chapter(
    blog_dir: &Path,
    book: &BookRef,
    input: &NewChapter,
) -> Result<ChapterOutcome, ChapterError> {
    let group = book.group();
    let file_name = format!("lg-{}.md", input.problem_id.trim());
    let chapter_path = blog_dir.join(&group).join(&file_name);

    if chapter_path.exists() {
        return Err(ChapterError::ChapterExists(
            chapter_path.display().to_string(),
        ));
    }

    let outline_path = blog_dir.join(book.index_path());
    let mut outline = Outline::read(&outline_path)?;

    // Snapshot for rollback: the exact bytes we are about to change.
    let outline_snapshot = outline.render();

    // Allocate the next site-wide `_0xNNNN`.
    let next_slug = slug::allocate_slug(blog_dir);
    let counter = slug::parse_counter(&next_slug).unwrap_or(1);

    // Locate the target section and, if present, an existing line for this problem.
    let section = outline
        .find_sections(&input.section)
        .into_iter()
        .next()
        .ok_or_else(|| ChapterError::SectionNotFound {
            section: input.section.clone(),
            file: outline_path.display().to_string(),
        })?
        .clone();

    let variant = find_in_section(&outline, &section, &input.problem_id);

    // --- Build the outline edit -------------------------------------------------
    let solution_marker = Marker::Linked {
        label: "题解".to_owned(),
        href: format!("/posts/{group}/{next_slug}"),
    };
    let video_marker = match input.video_url.as_deref().map(str::trim) {
        Some(url) if !url.is_empty() => Marker::Linked {
            label: "视频".to_owned(),
            href: url.to_owned(),
        },
        _ => Marker::Placeholder,
    };

    let (updated_entry, inserted, before) = match variant {
        Some(entry) => {
            let mut updated = entry.clone();
            updated.solution = solution_marker;
            if input.video_url.is_some() {
                updated.video = video_marker;
            }
            if input.check_implies_solution {
                updated.checked = true;
            }
            let before = entry.raw.clone();
            let line = entry.line;
            outline.replace_entry(line, &updated)?;
            (updated, false, Some(before))
        }
        None => {
            // Append after the section's last entry, inheriting its marker style.
            let marker = outline.section_marker(&section);
            let insert_at = section
                .entries
                .last()
                .map(|e| e.line)
                .unwrap_or(section.line);

            let entry = OutlineEntry {
                // Filled in below once we know its final index.
                line: insert_at + 1,
                checked: input.check_implies_solution,
                marker: if marker == ListMarker::Plus {
                    ListMarker::Dash
                } else {
                    marker
                },
                problem_id: Some(input.problem_id.trim().to_owned()),
                title: input.title.trim().to_owned(),
                luogu_url: Some(format!(
                    "https://www.luogu.com.cn/problem/{}",
                    input.problem_id.trim()
                )),
                solution: solution_marker,
                video: video_marker,
                raw: String::new(),
            };

            outline.insert_entry_after(insert_at, &entry)?;
            (entry, true, None)
        }
    };

    // --- Write the chapter file, then verify, then commit -----------------------
    let frontmatter = chapter_frontmatter(
        counter,
        input.title.trim(),
        &book.title,
        &input.author,
        &next_slug,
    );
    let file = MarkdownFile {
        frontmatter,
        body: chapter_body_template(),
        line_ending: "\n".to_owned(),
        had_bom: true,
    };

    if let Err(err) = crate::frontmatter::write_markdown(&chapter_path, &file) {
        // Nothing on disk changed yet for the outline; simply propagate.
        let _ = std::fs::remove_file(&chapter_path);
        return Err(ChapterError::Frontmatter(err));
    }

    // Re-parse the edited outline and confirm the new link resolves.
    let reparsed = Outline::parse(&outline.render(), &outline_path);
    let href = format!("/posts/{group}/{next_slug}");
    let found = reparsed
        .entries
        .iter()
        .any(|e| e.solution.href() == Some(href.as_str()));

    if !found {
        rollback(&chapter_path, &outline, &outline_snapshot)?;
        return Err(ChapterError::SelfCheckFailed);
    }

    if let Err(err) = outline.write() {
        rollback(&chapter_path, &outline, &outline_snapshot)?;
        return Err(ChapterError::Outline(err));
    }

    let title_line = format!(
        "{} {}",
        slug::format_title_counter(counter),
        input.title.trim()
    );

    Ok(ChapterOutcome {
        chapter_path,
        slug: next_slug,
        title_line,
        outline_path,
        outline_before: before,
        outline_after: render_entry(&updated_entry),
        inserted_entry: inserted,
    })
}

/// Restore the pre-edit state: delete the chapter file, put the outline back.
fn rollback(
    chapter_path: &Path,
    outline: &Outline,
    snapshot: &str,
) -> Result<(), ChapterError> {
    let _ = std::fs::remove_file(chapter_path);
    std::fs::write(&outline.path, snapshot).map_err(|source| ChapterError::RollbackFailed {
        path: outline.path.display().to_string(),
        source,
    })
}

/// Find this problem's entry within a section (line-indexed, not global).
fn find_in_section<'a>(
    outline: &'a Outline,
    section: &crate::outline::OutlineSection,
    problem_id: &str,
) -> Option<&'a OutlineEntry> {
    outline
        .entries
        .iter()
        .find(|e| {
            e.line >= section.line
                && section
                    .entries
                    .iter()
                    .any(|s| s.line == e.line)
                && e.problem_id.as_deref() == Some(problem_id)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_title_uses_zero_padded_hex() {
        let fm = chapter_frontmatter(19, "【深基3.例1】数字比较", "《深入浅出》", "作者", "_0x0013");
        assert_eq!(fm.title.as_deref(), Some("0x0013 【深基3.例1】数字比较"));
        assert_eq!(fm.slug.as_deref(), Some("_0x0013"));
        assert_eq!(fm.tags, vec!["《深入浅出》".to_owned()]);
        assert_eq!(fm.description.as_deref(), Some("【深基3.例1】数字比较题解。"));
        assert_eq!(fm.draft, Some(false));
        assert_eq!(fm.featured, Some(false));
    }

    #[test]
    fn book_ref_resolves_group_and_index_path() {
        let book = BookRef {
            id: "basic".into(),
            title: "《深入浅出程序设计竞赛 基础篇》".into(),
            index_id: "lgj/lg-index".into(),
        };
        assert_eq!(book.group(), "lgj");
        assert_eq!(book.index_path(), "lgj/lg-index.md");
    }

    #[test]
    fn body_template_has_the_standard_sections() {
        let body = chapter_body_template();
        for heading in ["## 题目描述", "## 输入格式", "## 输出格式", "## 题目分析", "## 代码实现"] {
            assert!(body.contains(heading), "missing {heading}");
        }
        assert!(body.contains("```cpp"), "cpp fence missing");
    }
}
