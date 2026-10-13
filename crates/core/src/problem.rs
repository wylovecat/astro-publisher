//! Luogu problem metadata: turn a problem id into its name, difficulty and tags.
//!
//! The chapter wizard knows the problem id (`P5703`) long before it knows the
//! problem *name* (`【深基2.例5】苹果采购`), and that name is what both the outline
//! line and the frontmatter title are built from. Retyping it by hand is the
//! slowest part of creating a chapter, so we look it up instead.
//!
//! The problem page ships its whole record in a `lentille-context` script tag,
//! so one GET is enough. The tag ids inside that record are meaningless without
//! the site's tag table, which costs a second (best-effort) request.
//!
//! Only [`lookup`] and [`lookup_cached`] touch the network: [`parse_problem_html`]
//! and [`parse_tag_table`] are pure, and [`lookup_with`] takes the HTTP call as a
//! closure, so the whole module is testable without a network.
//!
//! Results are cached under the platform cache directory, keyed by problem id, so
//! creating a whole section's chapters costs one request per problem and a later
//! offline session can reuse what was already fetched.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

const PROBLEM_URL: &str = "https://www.luogu.com.cn/problem/";
const TAGS_URL: &str = "https://www.luogu.com.cn/_lfe/tags";
/// Luogu answers some requests with an empty body when there is no User-Agent.
const USER_AGENT: &str = "astro-publisher/0.1 (+https://github.com/wylovecat/astro-publisher)";
/// The script tag holding the page's JSON payload.
const CONTEXT_OPEN: &str = "<script id=\"lentille-context\" type=\"application/json\">";
const CONTEXT_CLOSE: &str = "</script>";

/// Difficulty levels, indexed by the number Luogu reports. 0 means unrated.
const DIFFICULTY_LABELS: [&str; 8] = [
    "暂无评定",
    "入门",
    "普及−",
    "普及/提高−",
    "普及+/提高",
    "提高+/省选−",
    "省选/NOI−",
    "NOI/NOI+/CTSC",
];

#[derive(Debug, Error)]
pub enum ProblemError {
    #[error("`{0}` 不是有效的洛谷题号")]
    InvalidPid(String),
    #[error("请求洛谷失败：{0}")]
    Http(String),
    #[error("洛谷返回了 HTTP {0}")]
    Status(u16),
    #[error("题目页面里没有题目数据（题号可能不存在）")]
    NoData,
}

/// A problem as the wizard needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemInfo {
    pub pid: String,
    /// Full problem name, e.g. `【深基2.例5】苹果采购`.
    pub name: String,
    /// Luogu difficulty level, 0..=7.
    #[serde(default)]
    pub difficulty: Option<u8>,
    /// Chinese tag names, resolved from the tag table when it was available.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl ProblemInfo {
    /// Human-readable difficulty (`提高+/省选−`).
    pub fn difficulty_label(&self) -> Option<&'static str> {
        self.difficulty.and_then(difficulty_label)
    }
}

/// Name of a difficulty level, or `None` for an out-of-range value.
pub fn difficulty_label(level: u8) -> Option<&'static str> {
    DIFFICULTY_LABELS.get(level as usize).copied()
}

/// Accept the id as typed, but only if it can also be a file name component:
/// it ends up in `lg-<id>.md`, so path separators and dots must not get through.
pub fn normalize_pid(raw: &str) -> Option<String> {
    let pid = raw.trim();
    if pid.is_empty() {
        return None;
    }
    let usable = pid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    usable.then(|| pid.to_owned())
}

/// Parse the problem record out of a problem page.
///
/// `tag_names` maps Luogu tag ids to their Chinese names; pass an empty map to
/// get the name and difficulty without tags.
pub fn parse_problem_html(
    html: &str,
    tag_names: &HashMap<i64, String>,
) -> Result<ProblemInfo, ProblemError> {
    let json = lentille_context(html).ok_or(ProblemError::NoData)?;
    let value: serde_json::Value = serde_json::from_str(json).map_err(|_| ProblemError::NoData)?;

    // A missing record means Luogu served an error page rather than a problem.
    let problem = value
        .get("data")
        .and_then(|data| data.get("problem"))
        .ok_or(ProblemError::NoData)?;

    let name = problem
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err(ProblemError::NoData);
    }

    let pid = problem
        .get("pid")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_owned();

    let difficulty = problem
        .get("difficulty")
        .and_then(|v| v.as_u64())
        .map(|value| value as u8);

    let tags = problem
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_i64())
                .filter_map(|id| tag_names.get(&id).cloned())
                .collect()
        })
        .unwrap_or_default();

    Ok(ProblemInfo {
        pid,
        name,
        difficulty,
        tags,
    })
}

/// Parse the site's tag table (`{"tags":[{"id":1,"name":"模拟"}, …]}`).
///
/// Returns an empty map when the payload is not the shape we expect: tag names
/// are a nicety, never a reason to fail a lookup.
pub fn parse_tag_table(json: &str) -> HashMap<i64, String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return HashMap::new();
    };

    value
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    let id = tag.get("id")?.as_i64()?;
                    let name = tag.get("name")?.as_str()?.to_owned();
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Look a problem up, doing the HTTP calls through `fetch`.
///
/// Split out from [`lookup`] so the network stays out of the test suite.
pub fn lookup_with<F>(pid: &str, fetch: F) -> Result<ProblemInfo, ProblemError>
where
    F: Fn(&str) -> Result<String, ProblemError>,
{
    let pid = normalize_pid(pid).ok_or_else(|| ProblemError::InvalidPid(pid.trim().to_owned()))?;

    let html = fetch(&format!("{PROBLEM_URL}{pid}"))?;

    // Tag names need a second request; if it fails we still have the name.
    let tag_names = fetch(TAGS_URL)
        .map(|json| parse_tag_table(&json))
        .unwrap_or_default();

    let mut info = parse_problem_html(&html, &tag_names)?;
    if info.pid.is_empty() {
        info.pid = pid;
    }
    Ok(info)
}

/// Look a problem up over the network.
pub fn lookup(pid: &str) -> Result<ProblemInfo, ProblemError> {
    lookup_with(pid, http_get)
}

/// Look a problem up, reusing the on-disk cache when it has one.
pub fn lookup_cached(pid: &str) -> Result<ProblemInfo, ProblemError> {
    let pid = normalize_pid(pid).ok_or_else(|| ProblemError::InvalidPid(pid.trim().to_owned()))?;

    if let Some(dir) = cache_dir() {
        if let Some(cached) = read_cache(&dir, &pid) {
            return Ok(cached);
        }
    }

    let info = lookup(&pid)?;

    if let Some(dir) = cache_dir() {
        // Caching is an optimisation; an unwritable cache must not fail a lookup
        // that already succeeded.
        let _ = write_cache(&dir, &info);
    }

    Ok(info)
}

/// Directory holding cached problem records.
pub fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "astro-publisher", "astro-publisher")
        .map(|dirs| dirs.cache_dir().join("problems"))
}

/// Read a cached problem record.
pub fn read_cache(dir: &Path, pid: &str) -> Option<ProblemInfo> {
    let text = std::fs::read_to_string(dir.join(format!("{pid}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write a problem record to the cache, creating the directory if needed.
pub fn write_cache(dir: &Path, info: &ProblemInfo) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string_pretty(info)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(dir.join(format!("{}.json", info.pid)), text)
}

/// The one place that talks to the network.
fn http_get(url: &str) -> Result<String, ProblemError> {
    let response = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|err| ProblemError::Http(err.to_string()))?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(ProblemError::Status(status));
    }

    response
        .into_body()
        .read_to_string()
        .map_err(|err| ProblemError::Http(err.to_string()))
}

/// The JSON inside `<script id="lentille-context">…</script>`.
fn lentille_context(html: &str) -> Option<&str> {
    let start = html.find(CONTEXT_OPEN)? + CONTEXT_OPEN.len();
    let rest = &html[start..];
    let end = rest.find(CONTEXT_CLOSE)?;
    Some(rest[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed copy of a real problem page: the payload shape is what matters.
    const SAMPLE_HTML: &str = r#"<!DOCTYPE html><html><head>
<title>P1001 A+B Problem - 洛谷</title>
<script id="lentille-context" type="application/json">{"instance":"main","template":"problem.show","status":200,"locale":"zh-CN","data":{"problem":{"pid":"P1001","type":"P","name":"A+B Problem","difficulty":1,"fullScore":100,"tags":[1,6]}}}</script>
</head><body></body></html>"#;

    const SAMPLE_TAGS: &str =
        r#"{"tags":[{"id":1,"name":"模拟","type":2},{"id":6,"name":"图论","type":2}]}"#;

    fn tag_names() -> HashMap<i64, String> {
        parse_tag_table(SAMPLE_TAGS)
    }

    #[test]
    fn parses_the_problem_record_and_resolves_tag_names() {
        let info = parse_problem_html(SAMPLE_HTML, &tag_names()).expect("parses");

        assert_eq!(info.pid, "P1001");
        assert_eq!(info.name, "A+B Problem");
        assert_eq!(info.difficulty, Some(1));
        assert_eq!(info.difficulty_label(), Some("入门"));
        assert_eq!(info.tags, vec!["模拟".to_owned(), "图论".to_owned()]);
    }

    #[test]
    fn keeps_the_problem_without_a_tag_table() {
        // Without the table the ids cannot be resolved, but that must not lose
        // the name — it is the part the outline is built from.
        let info = parse_problem_html(SAMPLE_HTML, &HashMap::new()).expect("parses");
        assert_eq!(info.name, "A+B Problem");
        assert!(info.tags.is_empty());
    }

    #[test]
    fn reports_no_data_for_a_page_without_the_record() {
        let html = "<html><head><title>洛谷</title></head><body>404</body></html>";
        assert!(matches!(
            parse_problem_html(html, &HashMap::new()),
            Err(ProblemError::NoData)
        ));

        // A record with an empty name is just as useless.
        let empty = r#"<script id="lentille-context" type="application/json">{"data":{"problem":{"pid":"P1","name":"  "}}}</script>"#;
        assert!(matches!(
            parse_problem_html(empty, &HashMap::new()),
            Err(ProblemError::NoData)
        ));
    }

    #[test]
    fn parses_the_tag_table_and_tolerates_junk() {
        assert_eq!(tag_names().get(&6).map(String::as_str), Some("图论"));
        assert!(parse_tag_table("not json").is_empty());
        assert!(parse_tag_table(r#"{"tags":"nope"}"#).is_empty());
    }

    #[test]
    fn rejects_ids_that_cannot_be_used_as_file_names() {
        assert_eq!(normalize_pid(" P1001 ").as_deref(), Some("P1001"));
        assert_eq!(normalize_pid("AT_abc123_a").as_deref(), Some("AT_abc123_a"));
        assert_eq!(normalize_pid("").as_deref(), None);
        assert_eq!(normalize_pid("   ").as_deref(), None);
        assert_eq!(normalize_pid("../etc/passwd").as_deref(), None);
        assert_eq!(normalize_pid("P1001/../x").as_deref(), None);
    }

    #[test]
    fn lookup_asks_for_the_problem_and_the_tag_table() {
        let fetch = |url: &str| -> Result<String, ProblemError> {
            if url == TAGS_URL {
                Ok(SAMPLE_TAGS.to_owned())
            } else if url == "https://www.luogu.com.cn/problem/P1001" {
                Ok(SAMPLE_HTML.to_owned())
            } else {
                Err(ProblemError::Status(404))
            }
        };

        let info = lookup_with(" P1001 ", fetch).expect("lookup succeeds");
        assert_eq!(info.pid, "P1001");
        assert_eq!(info.name, "A+B Problem");
        assert_eq!(info.tags, vec!["模拟".to_owned(), "图论".to_owned()]);
    }

    #[test]
    fn a_failing_tag_lookup_still_returns_the_problem_name() {
        let fetch = |url: &str| -> Result<String, ProblemError> {
            if url == TAGS_URL {
                Err(ProblemError::Http("offline".to_owned()))
            } else {
                Ok(SAMPLE_HTML.to_owned())
            }
        };

        let info = lookup_with("P1001", fetch).expect("lookup succeeds");
        assert_eq!(info.name, "A+B Problem");
        assert!(info.tags.is_empty());
    }

    #[test]
    fn an_invalid_id_never_reaches_the_network() {
        let fetch = |_: &str| -> Result<String, ProblemError> {
            panic!("the fetcher must not be called for an unusable id");
        };
        assert!(matches!(
            lookup_with("../escape", fetch),
            Err(ProblemError::InvalidPid(_))
        ));
    }

    #[test]
    fn cache_round_trips_through_json() {        let dir = std::env::temp_dir().join(format!("an-problem-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let info = ProblemInfo {
            pid: "P1001".to_owned(),
            name: "A+B Problem".to_owned(),
            difficulty: Some(1),
            tags: vec!["模拟".to_owned()],
        };

        write_cache(&dir, &info).expect("write cache");
        assert_eq!(read_cache(&dir, "P1001"), Some(info));
        assert_eq!(read_cache(&dir, "P9999"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rest of the module is offline; this is the only test that proves the
    /// real HTTP + TLS path works. Run with `cargo test -p
    /// astro-publisher-core -- --ignored live_lookup`.
    #[test]
    #[ignore = "hits the network"]
    fn live_lookup_against_luogu() {
        let info = lookup("P1001").expect("network lookup succeeds");
        assert_eq!(info.pid, "P1001");
        assert_eq!(info.name, "A+B Problem");
        assert_eq!(info.difficulty, Some(1));
        println!("{info:?}");
    }
}
