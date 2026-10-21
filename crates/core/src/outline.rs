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
    #[error("no entry for problem `{problem_id}` in {file}{section}")]
    NoSuchEntry {
        problem_id: String,
        file: String,
        section: String,
    },
    #[error("problem `{problem_id}` appears {count} times in {file}{section}; pass a section to say which one")]
    AmbiguousEntry {
        problem_id: String,
        file: String,
        section: String,
        count: usize,
    },
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

/// An entry that has no 题解 link yet, paired with the section it lives in.
///
/// This is the input to "create the chapter for this one": the section heading is
/// what `chapters::create_chapter` needs to find the line again, and it cannot be
/// recovered from the entry alone because the same problem id appears in several
/// sections (plan §1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwrittenEntry {
    /// Heading text of the owning section, exactly as `find_sections` matches it.
    pub section: String,
    /// 0-based line index, unique within the file: the natural per-entry key.
    pub line: usize,
    pub checked: bool,
    /// `None` for a plain-text entry, which cannot be named after a problem.
    pub problem_id: Option<String>,
    pub title: String,
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
    // A label can contain balanced square brackets: CSP-J titles look like
    // `[P5660 [CSP-J 2019] 数字游戏](...)`. `\[[^\]]*\]` stops at the title's
    // closing bracket, so the link is not recognised at all and the entry loses
    // its title (the problem id still comes from the URL). Walk with a depth
    // counter instead; Markdown allows balanced brackets in link text.
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].1 != '[' {
            i += 1;
            continue;
        }
        let start = chars[i].0;
        let mut depth = 0usize;
        let mut end = None;
        let mut j = i;
        while j < chars.len() {
            match chars[j].1 {
                '[' => depth += 1,
                ']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(chars[j].0);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let Some(end) = end else {
            // No balancing `]` from here on, so no later link either.
            break;
        };
        if let Some(rest) = text[end + 1..].strip_prefix('(') {
            if let Some(close) = rest.find(')') {
                let href = &rest[..close];
                if !href.is_empty() && !href.chars().any(char::is_whitespace) {
                    return Some((text[start + 1..end].to_owned(), href.to_owned()));
                }
            }
        }
        i = j + 1;
    }
    None
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

    /// Entries whose 题解 marker is not a link, in document order.
    ///
    /// Walks `sections` rather than `entries` so each entry keeps the heading it
    /// sits under — the same problem id may appear in more than one section, and
    /// creation has to target the right one.
    pub fn unwritten(&self) -> Vec<UnwrittenEntry> {
        self.sections
            .iter()
            .flat_map(|section| {
                let heading = section.text.clone();
                section
                    .entries
                    .iter()
                    .filter(|entry| !entry.solution.is_linked())
                    .map(move |entry| UnwrittenEntry {
                        section: heading.clone(),
                        line: entry.line,
                        checked: entry.checked,
                        problem_id: entry.problem_id.clone(),
                        title: entry.title.clone(),
                    })
            })
            .collect()
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

// --- Edits ------------------------------------------------------------------
//
// `create_chapter` is not the only thing that changes an outline: a chapter can
// be written outside this tool (or before it existed), and a video may be
// published long after its 题解. These operations exist for those cases, and for
// the headless and AI front ends that have no outline screen to click through.
//
// They address an entry the way a human does — book, section, problem id —
// rather than by line number, because the same problem id appears in several
// sections (plan §1.2) and a line number is only meaningful to a process that
// has just parsed the file.

/// Which marker an edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Solution,
    Video,
}

impl MarkerKind {
    /// The label the marker is rendered with.
    pub fn label(self) -> &'static str {
        match self {
            MarkerKind::Solution => "题解",
            MarkerKind::Video => "视频",
        }
    }

    fn apply(&self, entry: &mut OutlineEntry, marker: Marker) {
        match self {
            MarkerKind::Solution => entry.solution = marker,
            MarkerKind::Video => entry.video = marker,
        }
    }

    fn get<'a>(&self, entry: &'a OutlineEntry) -> &'a Marker {
        match self {
            MarkerKind::Solution => &entry.solution,
            MarkerKind::Video => &entry.video,
        }
    }
}

/// One entry located for editing, with the section it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedEntry {
    pub line: usize,
    pub section: String,
    pub entry: OutlineEntry,
}

/// What a single-line edit changed, for reporting and for showing a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    pub path: PathBuf,
    pub line: usize,
    pub section: String,
    pub before: String,
    pub after: String,
}

/// Find the entries for `problem_id`, optionally restricted to one section.
///
/// Case-insensitive, because problem ids are typed by hand.
pub fn locate(
    outline: &Outline,
    problem_id: &str,
    section: Option<&str>,
) -> Vec<LocatedEntry> {
    let wanted = problem_id.trim().to_lowercase();
    let section_filter = section.map(|s| s.trim().to_lowercase());

    outline
        .sections
        .iter()
        .filter(|s| match &section_filter {
            Some(name) => s.text.trim().to_lowercase() == *name,
            None => true,
        })
        .flat_map(|s| {
            let heading = s.text.clone();
            // Cloned per section: the closure is `move`, and one `wanted` per
            // section would borrow it into the first iteration only.
            let wanted = wanted.clone();
            s.entries.iter().filter_map(move |entry| {
                let matches_problem = entry
                    .problem_id
                    .as_deref()
                    .map(|id| id.to_lowercase() == wanted)
                    .unwrap_or(false);
                matches_problem.then(|| LocatedEntry {
                    line: entry.line,
                    section: heading.clone(),
                    entry: entry.clone(),
                })
            })
        })
        .collect()
}

/// Locate exactly one entry, refusing to guess when the id is ambiguous.
///
/// A repeated problem id is not a corner case here — the plan records P5709
/// appearing under two sections — so "which one" is a question with a real
/// answer, and silently editing the first would corrupt the other.
pub fn locate_one(
    outline: &Outline,
    problem_id: &str,
    section: Option<&str>,
) -> Result<LocatedEntry, OutlineError> {
    let file = outline.path.display().to_string();
    let found = locate(outline, problem_id, section);
    let qualifier = match section {
        Some(name) => format!(" of section `{name}`"),
        None => String::new(),
    };
    match found.len() {
        0 => Err(OutlineError::NoSuchEntry {
            problem_id: problem_id.to_owned(),
            file,
            section: qualifier,
        }),
        1 => Ok(found.into_iter().next().expect("length checked")),
        count => Err(OutlineError::AmbiguousEntry {
            problem_id: problem_id.to_owned(),
            file,
            section: qualifier,
            count,
        }),
    }
}

/// Point a 📖/📺 marker at `href`.
pub fn set_link(
    path: &Path,
    problem_id: &str,
    section: Option<&str>,
    kind: MarkerKind,
    href: &str,
) -> Result<LineEdit, OutlineError> {
    edit(path, problem_id, section, |entry| {
        kind.apply(
            entry,
            Marker::Linked {
                label: kind.label().to_owned(),
                href: href.trim().to_owned(),
            },
        );
    })
}

/// Put a 📖/📺 marker back to its placeholder (`📖题解` / `📺视频`).
pub fn clear_link(
    path: &Path,
    problem_id: &str,
    section: Option<&str>,
    kind: MarkerKind,
) -> Result<LineEdit, OutlineError> {
    edit(path, problem_id, section, |entry| {
        kind.apply(entry, Marker::Placeholder);
    })
}

/// Tick or untick an entry's checkbox.
///
/// Deliberately separate from [`set_link`]: what the checkbox *means* differs
/// per book (基础篇 tracks 题解, 进阶篇 tracks 视频 — plan §1.2), so this tool does
/// not guess. [`crate::chapters::create_chapter`] is where that knowledge lives,
/// because it knows which book it is creating for.
pub fn set_checked(
    path: &Path,
    problem_id: &str,
    section: Option<&str>,
    checked: bool,
) -> Result<LineEdit, OutlineError> {
    edit(path, problem_id, section, |entry| {
        entry.checked = checked;
    })
}

/// Read, edit one entry, write back, and report the exact change.
fn edit(
    path: &Path,
    problem_id: &str,
    section: Option<&str>,
    mutate: impl FnOnce(&mut OutlineEntry),
) -> Result<LineEdit, OutlineError> {
    let mut outline = Outline::read(path)?;
    let located = locate_one(&outline, problem_id, section)?;
    let before = located.entry.raw.clone();

    let mut updated = located.entry.clone();
    mutate(&mut updated);
    // `raw` documented itself as the line as it stands, so a caller that
    // re-renders this entry gets the edited line rather than the pre-edit one.
    updated.raw = render_entry(&updated);
    outline.replace_entry(located.line, &updated)?;

    // Re-parse before committing: a rewrite that no longer parses is a rewrite
    // this tool should not make silently.
    let reparsed = Outline::parse(&outline.render(), path);
    if !reparsed.lines.get(located.line).map(|line| {
        parse_entry(line, located.line).is_some_and(|parsed| {
            parsed.checked == updated.checked
                && parsed.solution == updated.solution
                && parsed.video == updated.video
        })
    }) .unwrap_or(false) {
        return Err(OutlineError::SelfCheckFailed);
    }

    outline.write()?;

    Ok(LineEdit {
        path: path.to_path_buf(),
        line: located.line,
        section: located.section,
        before,
        after: outline.lines[located.line].clone(),
    })
}

/// The marker a line currently carries, for a status report.
pub fn marker_of(entry: &OutlineEntry, kind: MarkerKind) -> &Marker {
    kind.get(entry)
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

#[cfg(test)]
mod edit_tests {
    use super::*;

    /// A miniature outline with the two irregularities that matter: `*` as a
    /// list marker, and one problem id appearing under two sections.
    const SAMPLE: &str = "# Part1 入门\n\
        ## 1. 第一节\n\
        - [x] [P1001 A+B Problem](https://www.luogu.com.cn/problem/P1001) 📖[题解](/posts/lgj/_0x0001) 📺视频\n\
        * [ ] [P5709 圆的面积](https://www.luogu.com.cn/problem/P5709) 📖题解 📺视频\n\
        \n\
        ## 2. 第二节\n\
        - [ ] [P5709 圆的面积](https://www.luogu.com.cn/problem/P5709) 📖题解 📺视频   \n";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("an-outline-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("lg-index.md");
        std::fs::write(&path, SAMPLE).expect("write the outline");
        path
    }

    #[test]
    fn balanced_brackets_in_a_title_do_not_hide_it() {
        let line = "## 1. 测试\n- [ ] [P5660 [CSP-J 2019] 数字游戏](https://www.luogu.com.cn/problem/P5660) 📖题解 📺视频";
        let outline = Outline::parse(line, Path::new("csp-j2-index.md"));
        let entry = &outline.entries[0];

        assert_eq!(entry.problem_id.as_deref(), Some("P5660"));
        assert_eq!(
            entry.title, "[CSP-J 2019] 数字游戏",
            "the full label survives its nested brackets"
        );
        assert_eq!(outline.unwritten()[0].title, "[CSP-J 2019] 数字游戏");
    }

    #[test]
    fn linking_a_solution_touches_one_line_and_keeps_the_rest() {
        let path = scratch("link-solution");
        let before = std::fs::read_to_string(&path).expect("read");

        let edit = set_link(
            &path,
            "P5709",
            Some("1. 第一节"),
            MarkerKind::Solution,
            "/posts/lgj/_0x0009",
        )
        .expect("links");

        assert_eq!(edit.line, 3, "the P5709 line of section 1");
        assert!(edit.after.contains("📖[题解](/posts/lgj/_0x0009)"));
        assert!(edit.after.contains("📺视频"), "the video marker survives");

        let after = std::fs::read_to_string(&path).expect("read");
        let changed: Vec<usize> = before
            .lines()
            .zip(after.lines())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(changed, vec![3], "exactly the targeted line changed");

        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_repeated_problem_id_must_be_disambiguated() {
        let path = scratch("ambiguous");
        let err = set_link(
            &path,
            "p5709",
            None,
            MarkerKind::Video,
            "https://example.invalid/v",
        )
        .expect_err("refuses to guess");

        assert!(
            matches!(err, OutlineError::AmbiguousEntry { count: 2, .. }),
            "expected an ambiguity error, got {err}"
        );
        assert!(err.to_string().contains("pass a section"));

        // Naming the section resolves it, and case does not matter either way.
        let edit = set_link(
            &path,
            "p5709",
            Some("2. 第二节"),
            MarkerKind::Video,
            "https://example.invalid/v",
        )
        .expect("links");
        assert_eq!(edit.section, "2. 第二节");

        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn clearing_a_link_restores_the_placeholder() {
        let path = scratch("clear");
        let edit = clear_link(&path, "P1001", None, MarkerKind::Solution).expect("clears");
        assert!(edit.after.contains("📖题解"));
        assert!(!edit.after.contains("/posts/lgj/_0x0001"));
        assert_eq!(edit.before.split("📖").nth(1).map(str::trim), Some("[题解](/posts/lgj/_0x0001) 📺视频"));

        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn the_checkbox_is_independent_of_the_link() {
        let path = scratch("check");
        let edit = set_checked(&path, "P1001", None, false).expect("unticks");
        assert!(edit.after.starts_with("- [ ]"), "got {}", edit.after);
        assert!(
            edit.after.contains("📖[题解](/posts/lgj/_0x0001)"),
            "ticking is not link management"
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn an_unknown_problem_is_reported_rather_than_edited() {
        let path = scratch("missing");
        let err = set_checked(&path, "P9999", None, true).expect_err("no such entry");
        assert!(matches!(err, OutlineError::NoSuchEntry { .. }));
        // The file must be untouched.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            SAMPLE,
            "a failed lookup must not rewrite the outline"
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
