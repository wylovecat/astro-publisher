//! Content operations for an Astro blog.
//!
//! Pure Rust, no UI dependencies: the GPUI app, the headless CLI and (M2) the
//! MCP server all share this crate.
//!
//! The blog's content model is documented in the plan; the short version:
//! posts are Markdown with YAML frontmatter under `src/data/blog/`, course
//! chapters are indexed by an outline post whose task list carries progress.

pub mod books;
pub mod chapters;
pub mod config;
pub mod frontmatter;
pub mod images;
pub mod math_dom;
pub mod outline;
pub mod posts;
pub mod preview;
pub mod problem;
pub mod publish;
pub mod process;
pub mod slug;
pub mod validate;

pub use preview::{Preview, TextStats, render_preview, text_stats};

/// Timestamp string in the format the site's frontmatter uses
/// (`2026-10-12T18:30:00.000+08:00`).
pub fn now_local() -> String {
    use chrono::{FixedOffset, SecondsFormat, Utc};
    // The blog is authored in China Standard Time; keep timestamps consistent
    // with every existing post regardless of the machine's local timezone.
    let tz = FixedOffset::east_opt(8 * 3600).expect("valid +08:00 offset");
    Utc::now()
        .with_timezone(&tz)
        .to_rfc3339_opts(SecondsFormat::Millis, false)
}
