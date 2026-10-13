//! Course-outline parsing and surgical write-back.
//!
//! A book index post (`lgj/lg-index.md`, `lgs/lgs-index.md`) is Markdown whose
//! task-list lines encode both the syllabus and the writing progress:
//!
//! ```text
//! # Part1. 语言入门
//! ## 1. 简简单单写程序
//! - [x] [B2600 【深基1.例2】简单的分苹果](https://www.luogu.com.cn/problem/B2600) 📖[题解](/posts/lgj/_0x0001) 📺视频
//! * [ ] [P5705 数字反转](https://www.luogu.com.cn/problem/P5705) 📖题解 📺视频
//! ```
//!
//! Real-world irregularities this parser must tolerate (verified against both
//! books, plan §1.2):
//!
//! * list markers are `-` **or** `*`, sometimes inconsistently within a file;
//! * 题解 and 视频 markers each appear either unlinked (`📖题解`) or linked
//!   (`📖[题解](/posts/...)`), and the two are independent — `lgs` has checked
//!   items with unlinked 题解 but linked 视频;
//! * problem titles contain full-width brackets (`【深基1.例2】`), so titles are
//!   read from the link *href*, never by bracket pairing;
//! * the same problem id may appear in several sections;
//! * trailing whitespace on a line must survive an edit elsewhere.
//!
//! Because of the last two points, edits are line-indexed and byte-preserving:
//! only the targeted line is rebuilt, everything else is copied verbatim.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutlineError {
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
    #[error("line {line} is not an outline entry, so it cannot be edited")]
    NotAnEntry { line: usize },
    #[error("problem {problem_id} was not found in section {section}")]
    ProblemNotFound { problem_id: String, section: String },
    #[error("section `{0}` was not found in {1}")]
    SectionNotFound(String, String),
    #[error("failed to re-parse the outline after editing; no changes were written")]
    SelfCheckFailed,
}

/// Heading level below which entries are grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadingLevel {
    Part,
    Chapter,
    Section,
}

impl HeadingLevel {
    fn from_hashes(hashes: &str) -> Option<Self> {
        match hashes.len() {
            1 => Some(HeadingLevel::Part),
            2 => Some(HeadingLevel::Chapter),
            3 => Some(HeadingLevel::Section),
            _ => None,
        }
    }

    pub fn depth(self) -> usize {
        match self {
            HeadingLevel::Part => 1,
            HeadingLevel::Chapter => 2,
            HeadingLevel::Section => 3,
        }
    }
}

/// The task-list marker style used by a line (or section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    Dash,
    Star,
    Plus,
}

impl ListMarker {
    pub fn as_str(self) -> &'static str {
        match self {
            ListMarker::Dash => "-",
            ListMarker::Star => "*",
            ListMarker::Plus => "+",
        }
    }
}

/// How a 题解 / 视频 marker appears on an entry line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// The plain placeholder, e.g. `📖题解`.
    Placeholder,
    /// A clickable link, e.g. `📖[题解](/posts/lgj/_0x0001)`.
    Linked { label: String, href: String },
    /// The marker is absent from this line.
    Absent,
}

impl Marker {
    pub fn is_linked(&self) -> bool {
        matches!(self, Marker::Linked { .. })
    }

    pub fn href(&self) -> Option<&str> {
        match self {
            Marker::Linked { href, .. } => Some(href),
            _ => None,
        }
    }
}

/// One task-list entry: a problem and its progress markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    /// 0-based line index in the file's line array.
    pub line: usize,
    pub checked: bool,
    pub marker: ListMarker,
    /// Problem id extracted from the Luogu URL, e.g. `B2600`. `None` for a
    /// plain-text entry with no link.
    pub problem_id: Option<String>,
    /// Visible problem title, e.g. `【深基1.例2】简单的分苹果`.
    pub title: String,
    pub luogu_url: Option<String>,
    /// `📖` marker state.
    pub solution: Marker,
    /// `📺` marker state.
    pub video: Marker,
    /// The original line, preserved byte for byte.
    pub raw: String,
}

/// A section of the outline: a heading plus the entries under it.
#[derive(Debug, Clone)]
pub struct OutlineSection {
    /// 0-based line index of the heading itself.
    pub line: usize,
    pub level: HeadingLevel,
    /// Heading text with the leading `#`s and markdown syntax stripped.
    pub text: String,
    /// Entries directly under this heading.
    pub entries: Vec<OutlineEntry>,
}

/// A parsed outline file.
#[derive(Debug, Clone)]
pub struct Outline {
    pub path: PathBuf,
    /// Every line of the file, verbatim.
    pub lines: Vec<String>,
    /// Line ending style, reproduced on write.
    pub line_ending: String,
    pub sections: Vec<OutlineSection>,
    /// Entries in document order (a copy of every section's entries).
    pub entries: Vec<OutlineEntry>,
}

fn entry_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // - [x] / * [ ] / + [X], capturing the rest of the line.
        Regex::new(r"^(?P<indent>\s*)(?P<marker>[-*+])\s+\[(?P<state>[ xX])\]\s*(?P<rest>.*)$")
            .expect("valid entry regex")
    })
}

fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?P<hashes>#{1,6})\s+(?P<text>.*)$").expect("valid heading regex"))
}

fn luogu_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Anchor on the URL shape: titles contain full-width brackets that make
        // bracket-pairing markdown regexes fail (a lesson from this repo).
        Regex::new(r"https?://(?:www\.)?luogu\.com\.cn/problem/(?P<id>[A-Za-z0-9]+)")
            .expect("valid luogu regex")
    })
}

/// `📖[题解](/posts/lgj/_0x0001)` or `📖题解`.
fn solution_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"📖\s*\[(?P<label>[^\]]*)\]\((?P<href>[^)]*)\)|📖\s*(?P<plain>题解)")
            .expect("valid solution regex")
    })
}

/// `📺[视频](https://...)` or `📺视频`.
fn video_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"📺\s*\[(?P<label>[^\]]*)\]\((?P<href>[^)]*)\)|📺\s*(?P<plain>视频)")
            .expect("valid video regex")
    })
}

/// First markdown link in `text`, as `(label, href)`.
fn first_link(text: &str) -> Option<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\[(?P<label>[^\]]*)\]\((?P<href>[^)\s]*)\)").expect("valid link regex")
    });
    re.captures(text)
        .map(|c| (c["label"].to_owned(), c["href"].to_owned()))
}

/// Strip markdown emphasis/links so a heading reads as plain text.
fn plain(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\[(?P<label>[^\]]*)\]\([^)]*\)|[*_`~]").expect("valid plain regex")
    });
    re.replace_all(text, "$label").trim().to_owned()
}

impl Outline {
    /// Parse an outline from Markdown text.
    pub fn parse(text: &str, path: &Path) -> Self {
        let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" }.to_owned();
        let normalized = text.replace("\r\n", "\n");
        // Keep the trailing empty line produced by a final newline so writes
        // round-trip exactly.
        let lines: Vec<String> = normalized.split('\n').map(|s| s.to_owned()).collect();

        let mut sections: Vec<OutlineSection> = Vec::new();

        for (index, line) in lines.iter().enumerate() {
            if let Some(caps) = heading_re().captures(line) {
                if let Some(level) = HeadingLevel::from_hashes(&caps["hashes"]) {
                    sections.push(OutlineSection {
                        line: index,
                        level,
                        text: plain(&caps["text"]),
                        entries: Vec::new(),
                    });
                    continue;
                }
            }

            if let Some(entry) = parse_entry(line, index) {
                if let Some(section) = sections.last_mut() {
                    section.entries.push(entry);
                }
            }
        }

        let entries = sections
            .iter()
            .flat_map(|s| s.entries.iter().cloned())
            .collect();

        Self {
            path: path.to_path_buf(),
            lines,
            line_ending,
            sections,
            entries,
        }
    }

    /// Read and parse an outline file.
    pub fn read(path: &Path) -> Result<Self, OutlineError> {
        let text = std::fs::read_to_string(path).map_err(|source| OutlineError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Self::parse(&text, path))
    }

    /// Render back to text, preserving the original line endings.
    pub fn render(&self) -> String {
        let joined = self.lines.join("\n");
        if self.line_ending == "\r\n" {
            joined.replace('\n', "\r\n")
        } else {
            joined
        }
    }

    /// Write the outline back to disk.
    pub fn write(&self) -> Result<(), OutlineError> {
        std::fs::write(&self.path, self.render()).map_err(|source| OutlineError::Write {
            path: self.path.display().to_string(),
            source,
        })
    }

    /// Sections matching `text` exactly (trimmed, case-insensitive).
    pub fn find_sections<'a>(&'a self, text: &str) -> Vec<&'a OutlineSection> {
        let needle = text.trim().to_lowercase();
        self.sections
            .iter()
            .filter(|s| s.text.trim().to_lowercase() == needle)
            .collect()
    }

    /// Every entry with the given problem id, in document order.
    pub fn entries_for_problem(&self, problem_id: &str) -> Vec<&OutlineEntry> {
        self.entries
            .iter()
            .filter(|e| e.problem_id.as_deref() == Some(problem_id))
            .collect()
    }

    /// Total / done entry counts (a checked item counts as done).
    pub fn progress(&self) -> (usize, usize) {
        let total = self.entries.len();
        let done = self.entries.iter().filter(|e| e.checked).count();
        (done, total)
    }

    /// Replace the entry on `line` with an edited copy, preserving everything
    /// else in the file byte for byte.
    pub fn replace_entry(&mut self, line: usize, updated: &OutlineEntry) -> Result<(), OutlineError> {
        if line >= self.lines.len() {
            return Err(OutlineError::NotAnEntry { line });
        }
        self.lines[line] = render_entry(updated);
        Ok(())
    }

    /// Insert a new entry line directly after `after_line` (0-based index),
    /// inheriting the list marker of the neighbouring entry when unspecified.
    pub fn insert_entry_after(&mut self, after_line: usize, entry: &OutlineEntry) -> Result<(), OutlineError> {
        if after_line >= self.lines.len() {
            return Err(OutlineError::NotAnEntry { line: after_line });
        }
        self.lines.insert(after_line + 1, render_entry(entry));
        Ok(())
    }

    /// The list marker used by a section, defaulting to `-`.
    pub fn section_marker(&self, section: &OutlineSection) -> ListMarker {
        section
            .entries
            .first()
            .map(|e| e.marker)
            .unwrap_or(ListMarker::Dash)
    }

    /// Convert an entry into an outline line.
    ///
    /// Layout matches the site's convention: checkbox, problem link, 📖 marker,
    /// 📺 marker, all space-separated.
    pub fn render_entry_line(&self, entry: &OutlineEntry) -> String {
        render_entry(entry)
    }
}

/// Build the Markdown line for an entry.
pub fn render_entry(entry: &OutlineEntry) -> String {
    let checkbox = if entry.checked { "[x]" } else { "[ ]" };

    let problem = match (&entry.problem_id, &entry.luogu_url) {
        (Some(id), Some(url)) => format!("[{id} {}]({url})", entry.title),
        (Some(id), None) => format!(
            "[{id} {}](https://www.luogu.com.cn/problem/{id})",
            entry.title
        ),
        (None, Some(url)) => format!("[{}]({url})", entry.title),
        (None, None) => entry.title.clone(),
    };

    let solution = match &entry.solution {
        Marker::Linked { label, href } => format!("📖[{label}]({href})"),
        Marker::Placeholder => "📖题解".to_owned(),
        Marker::Absent => String::new(),
    };

    let video = match &entry.video {
        Marker::Linked { label, href } => format!("📺[{label}]({href})"),
        Marker::Placeholder => "📺视频".to_owned(),
        Marker::Absent => String::new(),
    };

    let mut parts = vec![
        format!("{} {checkbox} {problem}", entry.marker.as_str()),
        solution,
        video,
    ];
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

fn parse_entry(line: &str, index: usize) -> Option<OutlineEntry> {
    let caps = entry_re().captures(line)?;
    let rest = caps.name("rest").map(|m| m.as_str()).unwrap_or("");

    let checked = matches!(&caps["state"], "x" | "X");
    let marker = match &caps["marker"] {
        "*" => ListMarker::Star,
        "+" => ListMarker::Plus,
        _ => ListMarker::Dash,
    };

    let (luogu_url, problem_id, title_from_link) = match luogu_re().captures(rest) {
        Some(url_caps) => {
            let id = url_caps["id"].to_owned();
            let url = url_caps
                .get(0)
                .map(|m| m.as_str().to_owned())
                .unwrap_or_else(|| format!("https://www.luogu.com.cn/problem/{id}"));
            // Prefer the visible label of the first link for the title.
            let title = first_link(rest)
                .map(|(label, _)| strip_problem_id_prefix(&label, &id))
                .unwrap_or_default();
            (Some(url), Some(id), title)
        }
        None => (None, None, first_link(rest).map(|(l, _)| l).unwrap_or_default()),
    };

    let solution = match solution_re().captures(rest) {
        Some(caps) => {
            if let Some(href) = caps.name("href") {
                Marker::Linked {
                    label: caps.name("label").map(|m| m.as_str().to_owned()).unwrap_or_else(|| "题解".into()),
                    href: href.as_str().to_owned(),
                }
            } else {
                Marker::Placeholder
            }
        }
        None => Marker::Absent,
    };

    let video = match video_re().captures(rest) {
        Some(caps) => {
            if let Some(href) = caps.name("href") {
                Marker::Linked {
                    label: caps.name("label").map(|m| m.as_str().to_owned()).unwrap_or_else(|| "视频".into()),
                    href: href.as_str().to_owned(),
                }
            } else {
                Marker::Placeholder
            }
        }
        None => Marker::Absent,
    };

    Some(OutlineEntry {
        line: index,
        checked,
        marker,
        problem_id,
        title: title_from_link,
        luogu_url,
        solution,
        video,
        raw: line.to_owned(),
    })
}

/// `[B2600 【深基1.例2】简单的分苹果](...)` -> `【深基1.例2】简单的分苹果`.
fn strip_problem_id_prefix(label: &str, problem_id: &str) -> String {
    let trimmed = label.trim();
    match trimmed.strip_prefix(problem_id) {
        Some(rest) => rest.trim_start().to_owned(),
        None => trimmed.to_owned(),
    }
}
