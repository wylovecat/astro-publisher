//! Book registry access (`src/data/books.json`).
//!
//! The site keeps its book metadata in JSON so both TypeScript (via a thin
//! `books.ts` loader) and this tool can read and write it without parsing TS
//! source (plan §6).

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
    std::fs::write(&path, text).map_err(|source| BooksError::Write {
        path: path.display().to_string(),
        source,
    })
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
}
