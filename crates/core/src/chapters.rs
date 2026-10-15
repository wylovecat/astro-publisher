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
use crate::outline::{
    ListMarker, Marker, Outline, OutlineEntry, UnwrittenEntry, render_entry,
};
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
    #[error("the outline entry on line {line} has no problem id, so no chapter file can be named after it")]
    MissingProblemId { line: usize },
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

/// Values a chapter template may interpolate.
#[derive(Debug, Clone, Copy)]
pub struct TemplateVars<'a> {
    pub problem_id: &'a str,
    /// Full problem name, e.g. `【深基3.例1】数字比较`.
    pub title: &'a str,
    /// Site-wide sequence slug, e.g. `_0x0013`.
    pub slug: &'a str,
    /// The counter behind it, e.g. `0x0013`.
    pub counter: &'a str,
    pub book_title: &'a str,
    pub author: &'a str,
    /// Site-style timestamp (`2026-10-16T09:30:00.000+08:00`).
    pub date: &'a str,
}

/// Fill `{{name}}` placeholders in a body template.
///
/// Placeholders are `{{problem_id}}`, `{{title}}`, `{{slug}}`, `{{counter}}`,
/// `{{book_title}}`, `{{author}}`, `{{date}}`.
///
/// An **unknown** placeholder is left as written rather than replaced with an
/// empty string: a template that silently loses `{{section_heading}}` produces
/// chapters with a missing section, and an empty hole is much harder to notice
/// than one that is still spelled out. Whitespace inside the braces is ignored,
/// so `{{ title }}` works too.
pub fn render_template(template: &str, vars: &TemplateVars<'_>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        let (before, tail) = rest.split_at(start);
        out.push_str(before);

        let Some(end) = tail.find("}}") else {
            // Unterminated: the rest is literal text, not a placeholder.
            out.push_str(tail);
            return out;
        };
        let name = tail[2..end].trim();
        let after = &tail[end + 2..];

        match value_of(name, vars) {
            Some(value) => out.push_str(value),
            None => {
                out.push_str("{{");
                out.push_str(name);
                out.push_str("}}");
            }
        }
        rest = after;
    }

    out.push_str(rest);
    out
}

fn value_of<'a>(name: &str, vars: &TemplateVars<'a>) -> Option<&'a str> {
    Some(match name {
        "problem_id" => vars.problem_id,
        "title" => vars.title,
        "slug" => vars.slug,
        "counter" => vars.counter,
        "book_title" => vars.book_title,
        "author" => vars.author,
        "date" => vars.date,
        _ => return None,
    })
}

/// The body a new chapter should start from.
///
/// `template` is the user's configured file, or `None` for the built-in
/// skeleton. Nothing else decides this, so every front end produces the same
/// chapter from the same inputs.
pub fn chapter_body(template: Option<&str>, vars: &TemplateVars<'_>) -> String {
    // Empty-checked without trimming the value: `str::trim` would also eat the
    // template's own leading/trailing newlines, which is how a body ends up
    // glued to the next frontmatter field.
    match template {
        Some(template) if !template.trim().is_empty() => render_template(template, vars),
        _ => chapter_body_template(),
    }
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

/// What creating a chapter *would* do, without writing anything.
///
/// The wizard needs this to show the user the exact slug and outline line before
/// they commit: `create_chapter` allocates a site-wide sequence and rewrites one
/// line of the outline, which is too much to do speculatively.
#[derive(Debug, Clone)]
pub struct ChapterPreview {
    /// The slug that would be allocated right now.
    pub slug: String,
    /// Decimal counter behind the slug (`0x0013` for `_0x0013`).
    pub counter: u32,
    /// Frontmatter title, i.e. `0x0013 【深基3.例1】数字比较`.
    pub title_line: String,
    /// Path the chapter file would be written to.
    pub chapter_path: PathBuf,
    /// Path of the outline that would be edited.
    pub outline_path: PathBuf,
    /// The outline line after the edit.
    pub outline_after: String,
    /// The outline line before the edit, when the entry already existed.
    pub outline_before: Option<String>,
    /// True when no line exists yet and one would be inserted.
    pub inserted_entry: bool,
}

/// Compute the effect of creating a chapter, touching no files.
pub fn preview_chapter(
    blog_dir: &Path,
    book: &BookRef,
    input: &NewChapter,
) -> Result<ChapterPreview, ChapterError> {
    let group = book.group();
    let file_name = format!("lg-{}.md", input.problem_id.trim());
    let chapter_path = blog_dir.join(&group).join(&file_name);

    if chapter_path.exists() {
        return Err(ChapterError::ChapterExists(
            chapter_path.display().to_string(),
        ));
    }

    let outline_path = blog_dir.join(book.index_path());
    let outline = Outline::read(&outline_path)?;

    let next_slug = slug::allocate_slug(blog_dir);
    let counter = slug::parse_counter(&next_slug).unwrap_or(1);

    let section = outline
        .find_sections(&input.section)
        .into_iter()
        .next()
        .ok_or_else(|| ChapterError::SectionNotFound {
            section: input.section.clone(),
            file: outline_path.display().to_string(),
        })?
        .clone();

    let existing = find_in_section(&outline, &section, &input.problem_id).cloned();
    let inserted_entry = existing.is_none();
    let video_marker = match input.video_url.as_deref().map(str::trim) {
        Some(url) if !url.is_empty() => Marker::Linked {
            label: "视频".to_owned(),
            href: url.to_owned(),
        },
        _ => Marker::Placeholder,
    };
    let solution_marker = Marker::Linked {
        label: "题解".to_owned(),
        href: format!("/posts/{group}/{next_slug}"),
    };

    let (entry, before) = match existing {
        Some(mut entry) => {
            let before = Some(entry.raw.clone());
            entry.solution = solution_marker;
            if input.video_url.is_some() {
                entry.video = video_marker;
            }
            if input.check_implies_solution {
                entry.checked = true;
            }
            (entry, before)
        }
        None => {
            let marker = outline.section_marker(&section);
            (
                OutlineEntry {
                    line: 0,
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
                },
                None,
            )
        }
    };

    let title_line = format!(
        "{} {}",
        slug::format_title_counter(counter),
        input.title.trim()
    );

    Ok(ChapterPreview {
        slug: next_slug,
        counter,
        title_line,
        chapter_path,
        outline_path,
        outline_after: render_entry(&entry),
        outline_before: before,
        inserted_entry,
    })
}

/// Create a chapter and wire it into the book outline.
///
/// `blog_dir` is `src/data/blog`; everything else is resolved relative to it.
pub fn create_chapter(
    blog_dir: &Path,
    book: &BookRef,
    input: &NewChapter,
    body_template: Option<&str>,
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
        body: chapter_body(
            body_template,
            &TemplateVars {
                problem_id: input.problem_id.trim(),
                title: input.title.trim(),
                slug: &next_slug,
                counter: &slug::format_title_counter(counter),
                book_title: &book.title,
                author: &input.author,
                date: &crate::now_local(),
            },
        ),
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

/// Build the creation input for an outline entry that has no 题解 link yet.
///
/// The only fields the wizard asks for are already in the outline (problem id,
/// title, owning section), so this is the "one click" path: no form, no preview,
/// nothing to mistype.
pub fn input_from_entry(
    entry: &UnwrittenEntry,
    author: &str,
    check_implies_solution: bool,
) -> Result<NewChapter, ChapterError> {
    let problem_id = entry
        .problem_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or(ChapterError::MissingProblemId { line: entry.line })?;

    Ok(NewChapter {
        problem_id: problem_id.to_owned(),
        title: entry.title.trim().to_owned(),
        section: entry.section.trim().to_owned(),
        video_url: None,
        author: author.to_owned(),
        check_implies_solution,
    })
}

/// Create the chapter for an outline entry that has no 题解 link yet.
///
/// Thin wrapper over [`create_chapter`] for the outline page's one-click button:
/// everything it needs is already on the entry. Errors are the same ones
/// `create_chapter` raises, so the caller reports them unchanged.
pub fn create_chapter_from_entry(
    blog_dir: &Path,
    book: &BookRef,
    entry: &UnwrittenEntry,
    author: &str,
    check_implies_solution: bool,
    body_template: Option<&str>,
) -> Result<ChapterOutcome, ChapterError> {
    let input = input_from_entry(entry, author, check_implies_solution)?;
    create_chapter(blog_dir, book, &input, body_template)
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

    /// Copy the synthetic fixture tree so a test may write to it.
    fn scratch() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "an-preview-chapter-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        copy_tree(&src, &base);
        base
    }

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("create scratch dir");
        for entry in std::fs::read_dir(from).expect("read fixture dir") {
            let entry = entry.expect("dir entry");
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).expect("copy fixture file");
            }
        }
    }

    fn book_ref() -> BookRef {
        BookRef {
            id: "basic".into(),
            title: "《Sample Course Book 基础篇》".into(),
            index_id: "group/group-index".into(),
        }
    }

    fn input() -> NewChapter {
        NewChapter {
            problem_id: "P2003".into(),
            title: "三数之和".into(),
            section: "2.3 综合练习".into(),
            video_url: None,
            author: "Example Author".into(),
            check_implies_solution: true,
        }
    }

    #[test]
    fn preview_reports_the_slug_and_line_without_writing() {
        let base = scratch();
        let blog = base.join("blog");
        let index_path = blog.join("group/group-index.md");
        let before = std::fs::read_to_string(&index_path).expect("index readable");

        let preview = preview_chapter(&blog, &book_ref(), &input()).expect("preview succeeds");

        // The fixtures carry `_0x0001`; the next free slug is `_0x0002`.
        assert_eq!(preview.slug, "_0x0002");
        assert_eq!(preview.title_line, "0x0002 三数之和");
        assert!(!preview.inserted_entry, "P2003 already has an outline line");
        assert!(preview.outline_before.is_some());

        // The preview must not have touched anything.
        let untouched = std::fs::read_to_string(&index_path).expect("index readable");
        assert_eq!(before, untouched);
        assert!(!preview.chapter_path.exists(), "preview must not create the file");

        // The previewed line matches what an actual creation produces.
        let outcome = create_chapter(&blog, &book_ref(), &input(), None).expect("creation succeeds");
        assert_eq!(outcome.slug, preview.slug);
        assert_eq!(outcome.outline_after, preview.outline_after);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn preview_agrees_with_creation_for_an_inserted_line() {
        let base = scratch();
        let blog = base.join("blog");

        let mut request = input();
        request.problem_id = "B9999".into();
        request.section = "2.4 额外加练".into();
        request.title = "【样例9.例9】测试题目".into();

        let preview = preview_chapter(&blog, &book_ref(), &request).expect("preview succeeds");
        assert!(preview.inserted_entry, "a missing problem needs a new line");
        assert!(preview.outline_before.is_none());

        let outcome = create_chapter(&blog, &book_ref(), &request, None).expect("creation succeeds");
        assert_eq!(outcome.outline_after, preview.outline_after);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn preview_refuses_an_existing_chapter_and_an_unknown_section() {
        let base = scratch();
        let blog = base.join("blog");

        let mut existing = input();
        existing.problem_id = "B1000".into();
        existing.section = "1. 输出练习".into();
        assert!(matches!(
            preview_chapter(&blog, &book_ref(), &existing),
            Err(ChapterError::ChapterExists(_))
        ));

        let mut unknown = input();
        unknown.problem_id = "B8888".into();
        unknown.section = "9.9 并不存在的小节".into();
        assert!(matches!(
            preview_chapter(&blog, &book_ref(), &unknown),
            Err(ChapterError::SectionNotFound { .. })
        ));

        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;

    fn vars() -> TemplateVars<'static> {
        TemplateVars {
            problem_id: "B2620",
            title: "【深基3.例1】数字比较",
            slug: "_0x0013",
            counter: "0x0013",
            book_title: "《深入浅出程序设计竞赛 基础篇》",
            author: "Blog Author",
            date: "2026-10-16T09:30:00.000+08:00",
        }
    }

    #[test]
    fn fills_the_documented_placeholders() {
        let template = "{{counter}} {{title}}（{{problem_id}}）\n\n## 题目描述\n\n## 代码实现\n";
        let rendered = render_template(template, &vars());
        assert!(rendered.starts_with("0x0013 【深基3.例1】数字比较（B2620）\n"));
        assert!(rendered.contains("## 代码实现"));
    }

    #[test]
    fn tolerates_spacing_inside_the_braces() {
        assert!(render_template("{{ title }}", &vars()).contains("数字比较"));
    }

    #[test]
    fn an_unknown_placeholder_is_left_visible_rather_than_emptied() {
        // Silently dropping `{{section_heading}}` would produce a chapter with
        // a missing section and nothing to notice it by.
        let rendered = render_template("a {{nope}} b {{problem_id}}", &vars());
        assert!(rendered.contains("{{nope}}"), "got {rendered:?}");
        assert!(rendered.contains("B2620"));
    }

    #[test]
    fn an_unterminated_brace_is_literal_text() {
        assert_eq!(render_template("100% {{ oops", &vars()), "100% {{ oops");
    }

    #[test]
    fn no_template_falls_back_to_the_built_in_skeleton() {
        let body = chapter_body(None, &vars());
        for heading in ["## 题目描述", "## 输入格式", "## 代码实现"] {
            assert!(body.contains(heading), "missing {heading}");
        }

        // An all-whitespace file is treated as "not configured" too.
        let body = chapter_body(Some("  \n\t"), &vars());
        assert!(body.contains("## 题目描述"));
    }

    #[test]
    fn a_template_file_replaces_the_skeleton_entirely() {
        let body = chapter_body(Some("## {{title}}\n"), &vars());
        assert_eq!(body, "## 【深基3.例1】数字比较\n");
        assert!(!body.contains("题目描述"), "the built-in sections must not linger");
    }
}
