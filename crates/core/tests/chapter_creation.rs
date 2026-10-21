//! Chapter creation against a scratch copy of synthetic content.
//!
//! Covers the tool's headline guarantee: creating a chapter writes exactly two
//! files, keeps the site valid, and rolls back cleanly if anything fails midway.
//!
//! The fixtures are synthetic, so no real blog content is required to run these.

use astro_publisher_core::books::{Book, get_book, list_books};
use astro_publisher_core::chapters::{
    ChapterError, NewChapter, create_chapter, create_chapter_from_entry,
};
use astro_publisher_core::outline::{Outline, UnwrittenEntry};
use astro_publisher_core::posts;
use std::path::{Path, PathBuf};

/// Copy the fixture content tree into a scratch directory we may write to.
struct Scratch {
    root: PathBuf,
    blog: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "an-publisher-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        copy_tree(&src, &base);

        let root = base.clone();
        let blog = base.join("blog");
        Self { root, blog }
    }

    /// A book definition pointing at the synthetic basic index.
    fn basic_book() -> Book {
        Book {
            id: "basic".into(),
            title: "《Sample Course Book 基础篇》".into(),
            subtitle: None,
            description: "d".into(),
            index_id: "group/group-index".into(),
            cover_class: None,
        }
    }
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

#[test]
fn creates_a_chapter_and_updates_the_outline_atomically() {
    let scratch = Scratch::new();
    let book = Scratch::basic_book();

    let index_path = scratch.blog.join("group/group-index.md");
    let before = std::fs::read_to_string(&index_path).expect("index readable");
    let before_outline = Outline::parse(&before, &index_path);

    // P2003 is a real, still-unwritten problem in section 2.3.
    let input = NewChapter {
        problem_id: "P2003".into(),
        title: "三数之和".into(),
        section: "2.3 综合练习".into(),
        video_url: Some("https://video.example.invalid/v/new123".into()),
        author: "Example Author".into(),
        check_implies_solution: true,
    };

    let outcome = create_chapter(&scratch.blog, &book.as_book_ref(), &input, None)
        .expect("chapter creation succeeds");

    // --- The chapter file exists with the expected frontmatter ---------------
    assert!(outcome.chapter_path.exists());
    assert_eq!(
        outcome.chapter_path.file_name().unwrap(),
        "lg-P2003.md",
        "chapter file is named after the problem"
    );

    let post = posts::get_post(&outcome.chapter_path, &scratch.blog).expect("chapter parses");
    assert_eq!(post.frontmatter.slug.as_deref(), Some(outcome.slug.as_str()));
    assert_eq!(post.frontmatter.tags, vec![book.title.clone()]);
    assert_eq!(post.frontmatter.draft, Some(false));
    assert!(post.frontmatter.title.as_deref().unwrap().starts_with("0x"));
    // Body skeleton is the standard one.
    for heading in ["## 题目描述", "## 输入格式", "## 输出格式", "## 题目分析", "## 代码实现"] {
        assert!(post.body.contains(heading), "missing {heading}");
    }

    // --- Only the target line changed in the outline -------------------------
    let after = std::fs::read_to_string(&index_path).expect("index readable");
    let after_outline = Outline::parse(&after, &index_path);

    assert_eq!(
        before_outline.lines.len(),
        after_outline.lines.len(),
        "an existing entry must be edited in place, not appended"
    );

    let changed: Vec<usize> = before
        .lines()
        .zip(after.lines())
        .enumerate()
        .filter(|(_, (b, a))| b != a)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(changed.len(), 1, "exactly one outline line changes");
    assert!(!outcome.inserted_entry, "P2003 already had a line to edit");

    // --- The outline now links the chapter -----------------------------------
    let entry = after_outline
        .entries_for_problem("P2003")
        .into_iter()
        .next()
        .expect("entry present");
    assert_eq!(
        entry.solution.href(),
        Some(format!("/posts/group/{}", outcome.slug).as_str())
    );
    assert!(entry.checked, "a linked solution implies checked");
    assert_eq!(
        entry.video.href(),
        Some("https://video.example.invalid/v/new123")
    );

    // --- The new link resolves to the file we just wrote ---------------------
    let posts_now = posts::list_posts(&scratch.blog).expect("list posts");
    let urls: Vec<String> = posts_now.iter().map(|p| p.url_path()).collect();
    let expected = format!("/posts/group/{}", outcome.slug);
    assert!(
        urls.contains(&expected),
        "expected {expected} among {} urls",
        urls.len()
    );

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn inserts_a_line_when_the_problem_is_not_in_the_outline() {
    let scratch = Scratch::new();
    let book = Scratch::basic_book();

    let index_path = scratch.blog.join("group/group-index.md");
    let before = std::fs::read_to_string(&index_path).expect("index readable");

    // A problem id that does not appear anywhere in the outline.
    let input = NewChapter {
        problem_id: "B9999".into(),
        title: "【样例9.例9】测试题目".into(),
        section: "2.4 额外加练".into(),
        video_url: None,
        author: "Example Author".into(),
        check_implies_solution: true,
    };

    let outcome = create_chapter(&scratch.blog, &book.as_book_ref(), &input, None)
        .expect("chapter creation succeeds");
    assert!(outcome.inserted_entry, "a new line had to be inserted");

    let after = std::fs::read_to_string(&index_path).expect("index readable");
    let after_outline = Outline::parse(&after, &index_path);

    assert_eq!(
        after.lines().count(),
        before.lines().count() + 1,
        "inserting adds exactly one line"
    );

    let entry = after_outline
        .entries_for_problem("B9999")
        .into_iter()
        .next()
        .expect("new entry present");
    assert_eq!(entry.title, "【样例9.例9】测试题目");
    assert!(entry.checked);
    assert_eq!(entry.video, astro_publisher_core::outline::Marker::Placeholder);

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn outline_links_use_the_site_slugified_group() {
    let scratch = Scratch::new();
    // A book whose directory has a letter/digit boundary: the site's getPath
    // runs directories through lodash kebabCase, so `csp-j2` publishes under
    // `/posts/csp-j-2/`, not under the raw directory name.
    std::fs::create_dir_all(scratch.blog.join("csp-j2")).expect("create group dir");
    std::fs::copy(
        scratch.blog.join("group/group-index.md"),
        scratch.blog.join("csp-j2/csp-j2-index.md"),
    )
    .expect("copy outline");

    let book = Book {
        id: "csp-j2".into(),
        title: "《CSP-J2》".into(),
        subtitle: None,
        description: "d".into(),
        index_id: "csp-j2/csp-j2-index".into(),
        cover_class: None,
    };
    let input = NewChapter {
        problem_id: "P9999".into(),
        title: "数字游戏".into(),
        section: "2.3 综合练习".into(),
        video_url: None,
        author: "Example Author".into(),
        check_implies_solution: true,
    };

    let outcome =
        create_chapter(&scratch.blog, &book.as_book_ref(), &input, None).expect("create chapter");
    assert!(
        outcome.outline_after.contains("/posts/csp-j-2/"),
        "link must use the site's slugified directory: {}",
        outcome.outline_after
    );
    assert!(!outcome.outline_after.contains("/posts/csp-j2/"));
    // The file itself still lives under the real directory name.
    assert_eq!(
        outcome.chapter_path,
        scratch.blog.join("csp-j2/lg-P9999.md")
    );

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn refuses_to_overwrite_an_existing_chapter() {
    let scratch = Scratch::new();
    let book = Scratch::basic_book();

    // B1000 already has a chapter file in the fixtures.
    let input = NewChapter {
        problem_id: "B1000".into(),
        title: "【样例1.例1】打印问候".into(),
        section: "1. 输出练习".into(),
        video_url: None,
        author: "Example Author".into(),
        check_implies_solution: true,
    };

    let err = create_chapter(&scratch.blog, &book.as_book_ref(), &input, None)
        .expect_err("must refuse to clobber an existing chapter");
    assert!(matches!(err, ChapterError::ChapterExists(_)), "got {err:?}");

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn reports_an_unknown_section_without_touching_disk() {
    let scratch = Scratch::new();
    let book = Scratch::basic_book();

    let index_path = scratch.blog.join("group/group-index.md");
    let before = std::fs::read_to_string(&index_path).expect("index readable");

    let input = NewChapter {
        problem_id: "B8888".into(),
        title: "题目".into(),
        section: "9.9 并不存在的小节".into(),
        video_url: None,
        author: "Example Author".into(),
        check_implies_solution: true,
    };

    let err = create_chapter(&scratch.blog, &book.as_book_ref(), &input, None)
        .expect_err("unknown section must fail");
    assert!(matches!(err, ChapterError::SectionNotFound { .. }), "got {err:?}");

    // Nothing was created and the outline is untouched.
    assert!(!scratch.blog.join("group/lg-B8888.md").exists());
    let after = std::fs::read_to_string(&index_path).expect("index readable");
    assert_eq!(before, after, "outline must be byte-identical after a failure");

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn allocates_the_next_site_wide_slug() {
    let scratch = Scratch::new();
    // The fixtures carry `_0x0000` (the guide post) and `_0x0001` (a chapter),
    // so the next free slug is `_0x0002`.
    let next = astro_publisher_core::slug::allocate_slug(&scratch.blog);
    assert_eq!(next, "_0x0002", "site-wide sequence continues past the maximum");

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn slug_allocation_scans_every_post_and_ignores_non_slug_fields() {
    // The full-corpus behaviour (max `_0x0012` -> next `_0x0013`) is exercised
    // against a synthetic tree so the assertion does not depend on which
    // fixture files happen to be present.
    let base = std::env::temp_dir().join(format!("an-publisher-slugs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let blog = base.join("blog/solutions");
    std::fs::create_dir_all(&blog).expect("create dirs");

    for (name, slug, title) in [
        ("a.md", "_0x000a", "题目 A"),
        ("b.md", "_0x0012", "题目 B"),
        ("c.md", "custom-slug", "普通文章"),
    ] {
        let body = format!(
            "---\nauthor: a\ntitle: {title}\npubDatetime: 2026-01-01T00:00:00.000+08:00\ndraft: false\nslug: {slug}\ntags:\n  - t\ndescription: d\n---\n\n正文\n"
        );
        std::fs::write(blog.join(name), body).expect("write fixture post");
    }

    assert_eq!(
        astro_publisher_core::slug::allocate_slug(&base.join("blog")),
        "_0x0013",
        "next slug follows the numeric maximum, ignoring non-_0x slugs"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn one_click_creation_from_an_outline_entry_uses_the_books_own_words() {
    let scratch = Scratch::new();
    let book = Scratch::basic_book();
    let index_path = scratch.blog.join("group/group-index.md");

    // Exactly what the outline page hands the button: the entry as parsed, plus
    // its section. No form, no hand-typed title.
    let outline = Outline::read(&index_path).expect("index readable");
    let entry = outline
        .unwritten()
        .into_iter()
        .find(|e| e.problem_id.as_deref() == Some("P2004"))
        .expect("P2004 is listed but unwritten");
    let entry_title = entry.title.clone();

    let outcome = create_chapter_from_entry(
        &scratch.blog,
        &book.as_book_ref(),
        &entry,
        "Example Author",
        true,        None,
    )
    .expect("one-click creation succeeds");

    // The file is named after the problem, and the entry's own title is used.
    assert_eq!(
        outcome.chapter_path.file_name().unwrap(),
        "lg-P2004.md",
        "named after the problem id taken from the outline"
    );
    assert!(
        outcome.title_line.ends_with(&entry_title),
        "title comes from the outline: {} (wanted {entry_title})",
        outcome.title_line
    );

    // ...and the outline line it came from is now linked.
    let after = Outline::read(&index_path).expect("index re-readable");
    let rewritten = after
        .entries_for_problem("P2004")
        .into_iter()
        .next()
        .expect("P2004 still present");
    assert_eq!(
        rewritten.solution.href(),
        Some(format!("/posts/group/{}", outcome.slug).as_str())
    );
    assert!(rewritten.checked);

    // It left the unwritten list, which is what makes the button disappear.
    assert!(
        !after
            .unwritten()
            .iter()
            .any(|e| e.problem_id.as_deref() == Some("P2004")),
        "a created problem is no longer offered"
    );

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn one_click_creation_leaves_the_checkbox_alone_when_progress_is_tracked_by_video() {
    // 进阶篇 semantics: `[x]` already means "video done", so linking a solution
    // must not be what ticks the box.
    let scratch = Scratch::new();
    let book = Book {
        id: "advanced".into(),
        title: "《Sample Course Book 进阶篇》".into(),
        subtitle: None,
        description: "d".into(),
        index_id: "advanced/advanced-index".into(),
        cover_class: None,
    };
    let index_path = scratch.blog.join("advanced/advanced-index.md");

    let outline = Outline::read(&index_path).expect("index readable");
    let entry = outline
        .unwritten()
        .into_iter()
        .find(|e| e.problem_id.as_deref() == Some("P3008"))
        .expect("P3008 is listed but unwritten");

    create_chapter_from_entry(
        &scratch.blog,
        &book.as_book_ref(),
        &entry,
        "Example Author",
        false,        None,
    )
    .expect("one-click creation succeeds");

    let after = Outline::read(&index_path).expect("index re-readable");
    let rewritten = after
        .entries_for_problem("P3008")
        .into_iter()
        .next()
        .expect("P3008 still present");
    assert!(rewritten.solution.is_linked(), "solution link was written");
    assert!(
        !rewritten.checked,
        "the box is the book's video progress, not the solution's"
    );
    assert_eq!(
        rewritten.video,
        astro_publisher_core::outline::Marker::Placeholder,
        "no video URL was offered, so its placeholder stays"
    );

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn an_entry_without_a_problem_id_is_refused_before_anything_is_written() {
    let scratch = Scratch::new();
    let book = Scratch::basic_book();
    let index_path = scratch.blog.join("group/group-index.md");
    let before = std::fs::read_to_string(&index_path).expect("index readable");

    // A plain-text outline line: nothing to name the chapter file after.
    let entry = UnwrittenEntry {
        section: "1. 输出练习".into(),
        line: 14,
        checked: false,
        problem_id: None,
        title: "没有题号的一行".into(),
    };

    let err = create_chapter_from_entry(
        &scratch.blog,
        &book.as_book_ref(),
        &entry,
        "Example Author",
        true,        None,
    )
    .expect_err("an entry with no problem id cannot be created");
    assert!(matches!(err, ChapterError::MissingProblemId { line: 14 }), "got {err:?}");

    assert_eq!(
        before,
        std::fs::read_to_string(&index_path).expect("index readable"),
        "outline must be byte-identical after a refusal"
    );

    let _ = std::fs::remove_dir_all(&scratch.root);
}

#[test]
fn reads_the_site_book_registry_shape() {
    // The fixture tree has no books.json; this asserts the API contract with a
    // temporary registry so the CLI/app can rely on it.
    let base = std::env::temp_dir().join(format!("an-publisher-books-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src/data")).expect("create dirs");

    let json = r#"[
      {"id":"basic","title":"《基础篇》","description":"d","indexId":"group/group-index"}
    ]"#;
    std::fs::write(base.join("src/data/books.json"), json).expect("write books.json");

    let books = list_books(&base).expect("list books");
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].group(), "group");

    let one = get_book(&base, "basic").expect("get book");
    assert_eq!(one.title, "《基础篇》");

    let _ = std::fs::remove_dir_all(&base);
}
