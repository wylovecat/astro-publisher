//! Frontmatter parsing and surgical write-back.
//!
//! The blog's posts are plain Markdown with a YAML frontmatter block. Two rules
//! drive this module:
//!
//! 1. Only the `---` fenced block is ever rewritten; the body is preserved
//!    byte for byte (it may itself contain `---` inside code fences).
//! 2. Line endings are recorded on read and reproduced on write, so a file that
//!    uses CRLF does not silently become LF.

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrontmatterError {
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
    #[error("{path} does not start with a `---` frontmatter block")]
    Missing { path: String },
    #[error("unterminated frontmatter in {path}")]
    Unterminated { path: String },
    #[error("invalid YAML frontmatter in {path}: {source}")]
    Yaml {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

/// The frontmatter of a blog post, mirroring `src/content.config.ts`.
///
/// This is a *form-level* mirror: the authoritative validation is `astro check`
/// during `pnpm build` (see the plan, §3.1). Optional fields are `Option` so an
/// unknown/absent key is not an error here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Frontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(rename = "pubDatetime", default, skip_serializing_if = "Option::is_none")]
    pub pub_datetime: Option<String>,
    #[serde(rename = "modDatetime", default, skip_serializing_if = "Option::is_none")]
    pub mod_datetime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub featured: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A parsed Markdown file: frontmatter values plus the untouched body.
#[derive(Debug, Clone)]
pub struct MarkdownFile {
    pub frontmatter: Frontmatter,
    /// Everything after the closing `---`, excluding the newline that ended it.
    pub body: String,
    /// `\n` or `\r\n`, detected from the frontmatter block.
    pub line_ending: String,
    /// Whether the source file used a BOM.
    pub had_bom: bool,
}

impl MarkdownFile {
    /// Parse Markdown text. `path` is only used for error messages.
    pub fn parse(text: &str, path: &str) -> Result<Self, FrontmatterError> {
        let had_bom = text.starts_with('\u{feff}');
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);

        let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" }.to_owned();

        // The block must open on the very first line.
        let rest = text
            .strip_prefix("---\n")
            .or_else(|| text.strip_prefix("---\r\n"))
            .ok_or_else(|| FrontmatterError::Missing {
                path: path.to_owned(),
            })?;

        // Find the closing fence at the start of a line. Scanning line by line
        // keeps us from matching a `---` that lives inside the body's code.
        let mut offset = 0usize;
        let mut yaml_end = None;
        for line in rest.split_inclusive('\n') {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed == "---" {
                yaml_end = Some(offset);
                break;
            }
            offset += line.len();
        }

        let yaml_end = yaml_end.ok_or_else(|| FrontmatterError::Unterminated {
            path: path.to_owned(),
        })?;

        let yaml_text = &rest[..yaml_end];
        let after = &rest[yaml_end..];
        // Skip the closing fence line itself.
        let body = after
            .split_once('\n')
            .map(|(_, b)| b)
            .unwrap_or("")
            .to_owned();

        let frontmatter: Frontmatter =
            serde_yaml::from_str(yaml_text).map_err(|source| FrontmatterError::Yaml {
                path: path.to_owned(),
                source,
            })?;

        Ok(Self {
            frontmatter,
            body,
            line_ending,
            had_bom,
        })
    }

    /// Render back to Markdown with the recorded line endings.
    pub fn render(&self) -> Result<String, FrontmatterError> {
        let yaml = serde_yaml::to_string(&self.frontmatter).map_err(|source| {
            FrontmatterError::Yaml {
                path: "<serialize>".to_owned(),
                source,
            }
        })?;
        // serde_yaml already terminates with a newline.
        let mut out = String::new();
        if self.had_bom {
            out.push('\u{feff}');
        }
        out.push_str("---\n");
        out.push_str(&yaml);
        out.push_str("---\n");
        out.push_str(&self.body);

        if self.line_ending == "\r\n" {
            out = out.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        Ok(out)
    }

    /// Replace the frontmatter, ignoring a BOM already recorded.
    pub fn set_frontmatter(&mut self, frontmatter: Frontmatter) {
        self.frontmatter = frontmatter;
    }
}

/// Read and parse a Markdown file from disk.
pub fn read_markdown(path: &Path) -> Result<MarkdownFile, FrontmatterError> {
    let text = std::fs::read_to_string(path).map_err(|source| FrontmatterError::Read {
        path: path.display().to_string(),
        source,
    })?;
    MarkdownFile::parse(&text, &path.display().to_string())
}

/// Write a Markdown file back to disk (UTF-8, no BOM unless the source had one).
pub fn write_markdown(path: &Path, file: &MarkdownFile) -> Result<(), FrontmatterError> {
    let text = file.render()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| FrontmatterError::Write {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(path, text).map_err(|source| FrontmatterError::Write {
        path: path.display().to_string(),
        source,
    })
}
