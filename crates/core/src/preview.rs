//! Markdown preview rendering: `pulldown-cmark` for structure plus the `katex`
//! crate for mathematics.
//!
//! The site renders math with `remark-math` + `rehype-katex`, i.e. the real
//! KaTeX. The `katex` crate is a binding to that same KaTeX implementation (it
//! executes KaTeX's JavaScript through an embedded engine), so the preview
//! matches the published site closely instead of approximating it.
//!
//! Rendering never fails the caller: a formula KaTeX cannot handle is emitted
//! as literal text (the same graceful degradation the plan calls for), because a
//! preview pane must not blow up on one bad `\command`.

use pulldown_cmark::{Options, Parser, html};
use std::sync::OnceLock;

/// Rendered preview of a Markdown document.
#[derive(Debug, Clone)]
pub struct Preview {
    /// HTML with math replaced by KaTeX markup.
    pub html: String,
    /// Formulas that failed to render, for a non-blocking UI hint.
    pub math_failures: Vec<String>,
}

/// Render Markdown to HTML, typesetting `$...$` and `$$...$$`.
pub fn render_preview(markdown: &str) -> Preview {
    let (with_placeholders, formulas) = extract_math(markdown);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(&with_placeholders, options);
    let mut body = String::new();
    html::push_html(&mut body, parser);

    // Put the rendered math back where the placeholders are.
    let mut math_failures = Vec::new();
    let mut out = body;
    for (index, (source, display)) in formulas.iter().enumerate() {
        let token = placeholder(index);
        let rendered = render_formula(source, *display);
        match rendered {
            Ok(html) => out = out.replace(&token, &html),
            Err(_) => {
                math_failures.push(source.clone());
                // Degrade to the original delimiters rather than dropping it.
                let fallback = if *display {
                    format!("$${source}$$")
                } else {
                    format!("${source}$")
                };
                out = out.replace(&token, &escape_html(&fallback));
            }
        }
    }

    Preview {
        html: out,
        math_failures,
    }
}

/// Render a single formula with KaTeX. `display` selects display mode.
pub fn render_formula(tex: &str, display: bool) -> Result<String, String> {
    katex::render_with_opts(
        tex,
        katex::Opts::builder()
            .display_mode(display)
            .throw_on_error(false)
            .build()
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// A token unlikely to appear in real content, used to park math while the
/// Markdown parser works on the rest.
fn placeholder(index: usize) -> String {
    format!("\u{0}MATHPH{index}\u{0}")
}

/// Replace `$$...$$` and `$...$` spans with placeholders.
///
/// Returns the rewritten Markdown and the extracted `(source, is_display)` list.
/// Fenced code blocks are left alone: a `$` inside shell or C++ code is not
/// math. Inline code spans are skipped for the same reason.
fn extract_math(markdown: &str) -> (String, Vec<(String, bool)>) {
    let mut formulas: Vec<(String, bool)> = Vec::new();
    let mut out = String::with_capacity(markdown.len());

    let mut in_fence: Option<String> = None;

    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();

        // Track fenced code blocks by their opening marker (``` or ~~~).
        if let Some(fence) = &in_fence {
            out.push_str(line);
            if trimmed.starts_with(fence.as_str()) {
                in_fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker: String = trimmed.chars().take_while(|c| *c == '`' || *c == '~').collect();
            in_fence = Some(marker);
            out.push_str(line);
            continue;
        }

        out.push_str(&replace_in_line(line, &mut formulas));
    }

    (out, formulas)
}

/// Extract math from a single non-fenced line.
fn replace_in_line(line: &str, formulas: &mut Vec<(String, bool)>) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];

        // Skip inline code spans so `$` inside them is literal.
        if ch == '`' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                i += 1;
            }
            if i < chars.len() {
                i += 1; // closing backtick
            }
            out.extend(&chars[start..i]);
            continue;
        }

        // Escaped dollar signs are literal.
        if ch == '\\' && i + 1 < chars.len() && chars[i + 1] == '$' {
            out.extend(&chars[i..i + 2]);
            i += 2;
            continue;
        }

        if ch == '$' {
            let display = i + 1 < chars.len() && chars[i + 1] == '$';
            let delimiter_len = if display { 2 } else { 1 };
            let content_start = i + delimiter_len;

            if let Some(end) = find_closing(&chars, content_start, delimiter_len) {
                let content: String = chars[content_start..end].iter().collect();
                // Empty spans ($. no content) are left alone.
                if !content.trim().is_empty() {
                    formulas.push((content, display));
                    out.push_str(&placeholder(formulas.len() - 1));
                    i = end + delimiter_len;
                    continue;
                }
            }
        }

        out.push(ch);
        i += 1;
    }

    out
}

/// Find the closing delimiter position for a math span.
fn find_closing(chars: &[char], from: usize, delimiter_len: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == '$' {
            if delimiter_len == 1 {
                return Some(i);
            }
            if i + 1 < chars.len() && chars[i + 1] == '$' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Cheap content statistics for the editor status bar (a Gridea-like touch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextStats {
    /// CJK characters, counted individually.
    pub cjk_chars: usize,
    /// Whitespace-separated words outside CJK runs.
    pub words: usize,
    pub lines: usize,
    /// Estimated reading time in minutes (300 CJK chars or 200 words per minute).
    pub reading_minutes: usize,
}

/// Count CJK characters, Latin words, lines, and an estimated reading time.
pub fn text_stats(text: &str) -> TextStats {
    let mut cjk = 0usize;
    let mut words = 0usize;
    let mut in_word = false;

    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
            in_word = false;
        } else if ch.is_alphanumeric() {
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }

    let lines = text.lines().count();
    let minutes = ((cjk as f64 / 300.0) + (words as f64 / 200.0)).ceil() as usize;

    TextStats {
        cjk_chars: cjk,
        words,
        lines,
        reading_minutes: minutes.max(1),
    }
}

/// Common CJK ranges (Han, plus the CJK punctuation the blog uses).
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF     // CJK Unified Ideographs
        | 0x3400..=0x4DBF   // Extension A
        | 0xF900..=0xFAFF   // Compatibility Ideographs
        | 0x3000..=0x303F   // CJK punctuation
        | 0xFF00..=0xFFEF   // Full-width forms
    )
}

/// Lazily initialised check that the KaTeX engine loads at all; used by the
/// spike/CLI to fail fast on a broken build.
pub fn katex_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| render_formula("x", false).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_inline_and_display_math() {
        let preview = render_preview("这里有 $10$ 个苹果。\n\n$$a^2 + b^2 = c^2$$\n");
        assert!(preview.math_failures.is_empty(), "{:?}", preview.math_failures);
        // KaTeX output carries its own class names.
        assert!(preview.html.contains("katex"), "html: {}", preview.html);
        assert!(!preview.html.contains("$$"), "raw display math left: {}", preview.html);
    }

    #[test]
    fn leaves_code_fences_untouched() {
        let md = "```cpp\nint main() { /* $not math$ */ }\n```\n";
        let preview = render_preview(md);
        assert!(preview.math_failures.is_empty());
        assert!(preview.html.contains("$not math$"), "html: {}", preview.html);
    }

    #[test]
    fn skips_inline_code_spans() {
        let preview = render_preview("The `$PATH` variable and $x$ math.");
        assert!(preview.html.contains("$PATH"), "html: {}", preview.html);
        assert!(preview.html.contains("katex"));
    }

    #[test]
    fn bad_formula_degrades_without_panicking() {
        // `\notacommand` is not valid KaTeX; it must not take the preview down.
        let preview = render_preview("bad: $\\notacommand{}$ end");
        // Either KaTeX recovered (throw_on_error(false) renders it in red) or we
        // fell back to literal text — both are acceptable, neither may panic.
        assert!(!preview.html.is_empty());
    }

    #[test]
    fn renders_tables_and_task_lists() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n";
        let preview = render_preview(md);
        assert!(preview.html.contains("<table>"), "html: {}", preview.html);
        assert!(preview.html.contains("type=\"checkbox\""), "html: {}", preview.html);
    }

    #[test]
    fn counts_cjk_and_words() {
        let stats = text_stats("这里有 10 个苹果\nsecond line");
        assert_eq!(stats.cjk_chars, 6, "6 Han chars");
        assert!(stats.words >= 3, "words: {}", stats.words);
        assert_eq!(stats.lines, 2);
        assert!(stats.reading_minutes >= 1);
    }

    #[test]
    fn engine_loads() {
        assert!(katex_available(), "KaTeX engine failed to initialise");
    }

    #[test]
    fn frontmatter_is_stripped_before_rendering() {
        // Mirrors what the CLI's `preview --file` and the editor view do: a whole
        // post file must not render its YAML block as body text.
        let whole_file = "---\ntitle: 0x0001 【深基1.例2】简单的分苹果\nslug: _0x0001\n---\n\n## 题目描述\n\n这里有 $10$ 个苹果。\n";
        let file =
            crate::frontmatter::MarkdownFile::parse(whole_file, "<test>").expect("parses");

        let preview = render_preview(&file.body);
        assert!(!preview.html.contains("slug:"), "frontmatter leaked: {}", preview.html);
        assert!(!preview.html.contains("<hr"), "frontmatter fence became a rule");
        assert!(preview.html.contains("题目描述"));
        assert!(preview.html.contains("katex"));
    }
}
