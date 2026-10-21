//! Book registry access (`src/data/books.json`).
//!
//! The site keeps its book metadata in JSON so both TypeScript (via a thin
//! `books.ts` loader) and this tool can read and write it without parsing TS
//! source (plan §6).

use crate::frontmatter::{self, Frontmatter, MarkdownFile};
use crate::outline::Outline;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BooksError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid books.json at {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("no book with id `{0}`")]
    NotFound(String),
    #[error("book id `{0}` is not URL-safe: use ASCII letters, digits, `-` or `_`")]
    InvalidId(String),
    #[error("a book with id `{0}` already exists")]
    DuplicateId(String),
    #[error("indexId `{0}` must look like `<group>/<name>` (optionally ending in `.md`)")]
    InvalidIndexId(String),
    #[error("indexId `{0}` is already used by another book")]
    DuplicateIndexId(String),
    #[error("outline `{0}` already exists; register it instead of generating a skeleton")]
    OutlineExists(String),
    #[error("outline `{0}` does not exist; create it first or allow the skeleton to be generated")]
    OutlineMissing(String),
    #[error("outline `{path}` is not a usable post: {message}")]
    InvalidOutline { path: String, message: String },
    #[error("book title, description and author must not be empty")]
    EmptyField,
    #[error(transparent)]
    Frontmatter(#[from] crate::frontmatter::FrontmatterError),
    #[error("wrote `{path}` but could not remove it during rollback: {source}")]
    Rollback {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// One registered book. Field names match the site's `BookDef` interface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Book {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub description: String,
    /// Outline post id, e.g. `lgj/lg-index`.
    #[serde(rename = "indexId")]
    pub index_id: String,
    /// Optional cover/shelf colour class used by the site.
    #[serde(rename = "coverClass", default, skip_serializing_if = "Option::is_none")]
    pub cover_class: Option<String>,
}

impl Book {
    /// Content subdirectory holding this book's chapters (`lgj`).
    pub fn group(&self) -> String {
        self.index_id
            .split_once('/')
            .map(|(group, _)| group.to_owned())
            .unwrap_or_else(|| self.index_id.clone())
    }

    /// Path of the outline post relative to the blog root (`lgj/lg-index.md`).
    pub fn index_path(&self) -> String {
        format!("{}.md", self.index_id)
    }

    /// Convert to the chapter-creation input type.
    pub fn as_book_ref(&self) -> crate::chapters::BookRef {
        crate::chapters::BookRef {
            id: self.id.clone(),
            title: self.title.clone(),
            index_id: self.index_id.clone(),
        }
    }
}

/// Location of the registry inside the site repository.
pub fn books_json_path(repo_root: &Path) -> PathBuf {
    repo_root.join("src").join("data").join("books.json")
}

/// Read every registered book.
pub fn list_books(repo_root: &Path) -> Result<Vec<Book>, BooksError> {
    let path = books_json_path(repo_root);
    let text = std::fs::read_to_string(&path).map_err(|source| BooksError::Read {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| BooksError::Json {
        path: path.display().to_string(),
        source,
    })
}

/// Find one book by id.
pub fn get_book(repo_root: &Path, id: &str) -> Result<Book, BooksError> {
    list_books(repo_root)?
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| BooksError::NotFound(id.to_owned()))
}

/// Write the registry back, pretty-printed with 2-space indent and a trailing
/// newline (matching the committed file's formatting).
pub fn save_books(repo_root: &Path, books: &[Book]) -> Result<(), BooksError> {
    let path = books_json_path(repo_root);
    let mut text =
        serde_json::to_string_pretty(books).map_err(|source| BooksError::Json {
            path: path.display().to_string(),
            source,
        })?;
    text.push('\n');

    // Write-then-rename: a crash mid-write must not leave a truncated registry
    // that makes every book disappear on the next launch.
    let tmp = path.with_extension("json.tmp");
    if let Err(source) = std::fs::write(&tmp, text) {
        return Err(BooksError::Write {
            path: tmp.display().to_string(),
            source,
        });
    }
    if let Err(source) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(BooksError::Write {
            path: path.display().to_string(),
            source,
        });
    }
    Ok(())
}

/// Input for registering a new book.
#[derive(Debug, Clone)]
pub struct NewBook {
    /// Short URL-safe key. The site uses it as `/books/<id>/`.
    pub id: String,
    pub title: String,
    /// Optional subtitle shown under the cover.
    pub subtitle: Option<String>,
    pub description: String,
    /// Outline post id, e.g. `lgj/lg-index`. A trailing `.md` is accepted.
    pub index_id: String,
    /// Optional cover class used by the site.
    pub cover_class: Option<String>,
    /// Author written into the generated outline's frontmatter.
    pub author: String,
    /// When true, write `<indexId>.md` with a skeleton first. When false the
    /// outline must already exist; this is the "register an existing outline"
    /// path.
    pub create_outline: bool,
}

/// What `create_book` changed.
#[derive(Debug, Clone)]
pub struct NewBookOutcome {
    pub book: Book,
    pub outline_path: PathBuf,
    /// False when an existing outline was registered rather than generated.
    pub created_outline: bool,
}

/// Register a book, optionally writing its outline post from a skeleton.
///
/// The outline is written first and removed again if updating `books.json`
/// fails, so a failed call never leaves a registered book whose outline is
/// missing (the wizard would show it, with no sections, and look broken).
pub fn create_book(repo_root: &Path, input: &NewBook) -> Result<NewBookOutcome, BooksError> {
    let id = input.id.trim();
    let title = input.title.trim();
    let description = input.description.trim();
    let author = input.author.trim();
    if title.is_empty() || description.is_empty() || author.is_empty() {
        return Err(BooksError::EmptyField);
    }
    if !is_url_safe_id(id) {
        return Err(BooksError::InvalidId(id.to_owned()));
    }

    let index_id = normalize_index_id(&input.index_id)?;
    let mut books = list_books(repo_root)?;
    if books.iter().any(|book| book.id == id) {
        return Err(BooksError::DuplicateId(id.to_owned()));
    }
    if books.iter().any(|book| book.index_id == index_id) {
        return Err(BooksError::DuplicateIndexId(index_id));
    }

    let blog_dir = repo_root.join("src").join("data").join("blog");
    let outline_path = blog_dir.join(format!("{index_id}.md"));
    let existed = outline_path.exists();

    if existed {
        if input.create_outline {
            return Err(BooksError::OutlineExists(
                outline_path.display().to_string(),
            ));
        }
        // Registering an existing outline: make sure it is actually parseable,
        // otherwise the book would show up with zero sections.
        Outline::read(&outline_path).map_err(|err| BooksError::InvalidOutline {
            path: outline_path.display().to_string(),
            message: err.to_string(),
        })?;
    } else if !input.create_outline {
        return Err(BooksError::OutlineMissing(
            outline_path.display().to_string(),
        ));
    }

    let group_dir = outline_path.parent().map(Path::to_path_buf);
    let group_existed = group_dir.as_deref().is_some_and(Path::exists);

    if !existed {
        let now = crate::now_local();
        let file = MarkdownFile {
            frontmatter: Frontmatter {
                author: Some(author.to_owned()),
                pub_datetime: Some(now.clone()),
                mod_datetime: Some(now),
                // Matches the existing index posts: `0x0000 <title>题解目录`.
                title: Some(format!("0x0000 {title}题解目录")),
                featured: Some(true),
                draft: Some(false),
                // The site's index posts keep an empty slug and derive the URL
                // from the file name.
                slug: Some(String::new()),
                tags: vec![title.to_owned()],
                description: Some(description.to_owned()),
            },
            body: "\n## 第一章\n\n".to_owned(),
            line_ending: "\n".to_owned(),
            had_bom: true,
        };
        frontmatter::write_markdown(&outline_path, &file)?;
    }

    let book = Book {
        id: id.to_owned(),
        title: title.to_owned(),
        subtitle: input
            .subtitle
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
        description: description.to_owned(),
        index_id: index_id.clone(),
        cover_class: input
            .cover_class
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
    };

    books.push(book.clone());
    if let Err(err) = save_books(repo_root, &books) {
        if !existed {
            if let Err(source) = std::fs::remove_file(&outline_path) {
                return Err(BooksError::Rollback {
                    path: outline_path.display().to_string(),
                    source,
                });
            }
            // Do not leave an empty group directory behind when we created it.
            if !group_existed {
                if let Some(dir) = &group_dir {
                    let _ = std::fs::remove_dir(dir);
                }
            }
        }
        return Err(err);
    }

    Ok(NewBookOutcome {
        book,
        outline_path,
        created_outline: !existed,
    })
}

/// `[A-Za-z0-9_-]+`: the id becomes a site URL segment (`/books/<id>/`).
fn is_url_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Accept `group/name` or `group/name.md`; reject anything `Book::group` cannot
/// handle (it takes the first segment as the chapter directory).
fn normalize_index_id(raw: &str) -> Result<String, BooksError> {
    let mut text = raw.trim();
    if let Some(stripped) = text.strip_suffix(".md") {
        text = stripped;
    }
    let invalid = || BooksError::InvalidIndexId(raw.trim().to_owned());

    if text.is_empty() || text.starts_with('/') || text.ends_with('/') || text.contains('\\') {
        return Err(invalid());
    }
    if text.chars().any(char::is_whitespace) {
        return Err(invalid());
    }
    let mut parts = text.split('/');
    let group = parts.next().unwrap_or_default();
    let name = parts.next().ok_or_else(invalid)?;
    if group.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(invalid());
    }
    if group == "." || group == ".." || name == "." || name == ".." {
        return Err(invalid());
    }
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_group_and_index_path() {
        let book = Book {
            id: "basic".into(),
            title: "《深入浅出程序设计竞赛 基础篇》".into(),
            subtitle: Some("题解与学习笔记".into()),
            description: "d".into(),
            index_id: "lgj/lg-index".into(),
            cover_class: None,
        };
        assert_eq!(book.group(), "lgj");
        assert_eq!(book.index_path(), "lgj/lg-index.md");
        assert_eq!(book.as_book_ref().id, "basic");
    }

    #[test]
    fn deserializes_the_site_format() {
        let json = r#"[
          {
            "id": "basic",
            "title": "《深入浅出程序设计竞赛 基础篇》",
            "subtitle": "题解与学习笔记",
            "description": "按课程章节顺序整理的题解。",
            "indexId": "lgj/lg-index"
          }
        ]"#;
        let books: Vec<Book> = serde_json::from_str(json).expect("valid json");
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].index_id, "lgj/lg-index");
        assert_eq!(books[0].group(), "lgj");
    }

    #[test]
    fn round_trips_through_pretty_json() {
        let books = vec![Book {
            id: "advanced".into(),
            title: "《进阶篇》".into(),
            subtitle: None,
            description: "d".into(),
            index_id: "lgs/lgs-index".into(),
            cover_class: Some("cover-a".into()),
        }];
        let text = serde_json::to_string_pretty(&books).expect("serialize");
        let back: Vec<Book> = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(books, back);
    }

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "an-publisher-book-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("src/data/blog/group"))
                .expect("create fixture dirs");
            std::fs::write(
                root.join("src/data/books.json"),
                r#"[
  {"id":"basic","title":"《基础篇》","description":"d","indexId":"group/group-index"}
]
"#,
            )
            .expect("write registry");
            std::fs::write(
                root.join("src/data/blog/group/group-index.md"),
                "---\ntitle: index\n---\n\n## 1. 已有小节\n\n",
            )
            .expect("write existing outline");
            // An outline that exists on disk but is not in the registry, for
            // the "register an existing outline" path.
            std::fs::write(
                root.join("src/data/blog/group/second-index.md"),
                "---\ntitle: second\n---\n\n## 2. 另一小节\n\n",
            )
            .expect("write second outline");
            Self { root }
        }

        fn book(&self, id: &str, index_id: &str, create_outline: bool) -> NewBook {
            NewBook {
                id: id.to_owned(),
                title: "《新书》".to_owned(),
                subtitle: Some("副标题".to_owned()),
                description: "描述".to_owned(),
                index_id: index_id.to_owned(),
                cover_class: None,
                author: "作者".to_owned(),
                create_outline,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn creates_a_book_and_its_outline_skeleton() {
        let scratch = Scratch::new("create");
        let outcome = create_book(
            &scratch.root,
            &scratch.book("newbook", "nb/nb-index", true),
        )
        .expect("create book");

        assert!(outcome.created_outline);
        let books = list_books(&scratch.root).expect("list");
        assert_eq!(books.len(), 2);
        assert_eq!(books[1].id, "newbook");
        assert_eq!(books[1].index_id, "nb/nb-index");
        assert_eq!(books[1].subtitle.as_deref(), Some("副标题"));

        let outline = Outline::read(&outcome.outline_path).expect("outline parses");
        assert_eq!(outline.sections.len(), 1);
        assert_eq!(outline.sections[0].text, "第一章");

        let text = std::fs::read_to_string(&outcome.outline_path).expect("read outline");
        let parsed =
            MarkdownFile::parse(&text, "outline").expect("generated outline has frontmatter");
        assert_eq!(
            parsed.frontmatter.title.as_deref(),
            Some("0x0000 《新书》题解目录")
        );
        assert_eq!(parsed.frontmatter.author.as_deref(), Some("作者"));
        assert_eq!(parsed.frontmatter.tags, vec!["《新书》".to_owned()]);
        assert_eq!(parsed.frontmatter.draft, Some(false));
    }

    #[test]
    fn registers_an_existing_outline_without_touching_it() {
        let scratch = Scratch::new("existing");
        let before = std::fs::read_to_string(scratch.root.join("src/data/blog/group/second-index.md"))
            .expect("read");

        let outcome = create_book(
            &scratch.root,
            &scratch.book("second", "group/second-index.md", false),
        )
        .expect("register existing outline");

        assert!(!outcome.created_outline);
        assert_eq!(outcome.book.index_id, "group/second-index");
        assert_eq!(
            std::fs::read_to_string(&outcome.outline_path).expect("read"),
            before
        );
    }

    #[test]
    fn refuses_to_overwrite_an_existing_outline() {
        let scratch = Scratch::new("overwrite");
        let before = std::fs::read_to_string(scratch.root.join("src/data/books.json")).expect("read");

        let err = create_book(
            &scratch.root,
            &scratch.book("second", "group/second-index", true),
        )
        .expect_err("must refuse");
        assert!(matches!(err, BooksError::OutlineExists(_)), "got {err:?}");
        assert_eq!(
            std::fs::read_to_string(scratch.root.join("src/data/books.json")).expect("read"),
            before
        );
    }

    #[test]
    fn rejects_duplicate_ids_and_malformed_index_ids() {
        let scratch = Scratch::new("invalid");

        let err = create_book(
            &scratch.root,
            &scratch.book("basic", "nb/nb-index", true),
        )
        .expect_err("duplicate id");
        assert!(matches!(err, BooksError::DuplicateId(_)), "got {err:?}");

        for index in ["no-slash", "a/b/c", "a/.", "../x", "/abs"] {
            let err = create_book(&scratch.root, &scratch.book("x", index, true))
                .expect_err("bad index");
            assert!(
                matches!(err, BooksError::InvalidIndexId(_)),
                "{index}: got {err:?}"
            );
        }

        assert_eq!(list_books(&scratch.root).expect("list").len(), 1);
        assert!(!scratch.root.join("src/data/blog/x").exists());
    }

    #[test]
    fn missing_outline_is_an_error_when_skeleton_creation_is_off() {
        let scratch = Scratch::new("missing");
        let err = create_book(
            &scratch.root,
            &scratch.book("second", "group/missing-index", false),
        )
        .expect_err("missing outline");
        assert!(matches!(err, BooksError::OutlineMissing(_)), "got {err:?}");
        assert_eq!(list_books(&scratch.root).expect("list").len(), 1);
    }
}
