//! Whole-corpus validation: frontmatter mirror checks plus outline link
//! integrity.
//!
//! This is the *advisory* layer. The authoritative frontmatter check is
//! `astro check` inside `pnpm build`, which the publish pipeline runs before
//! committing anything (plan §3.1, §5 step 2). Findings here are surfaced in
//! the form and in the publish preflight so mistakes are caught early.

use crate::frontmatter::Frontmatter;
use crate::outline::Outline;
use crate::posts;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks publishing.
    Error,
    /// Reported, but the author may proceed.
    Warning,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    /// Path relative to the blog root, as shown to the user.
    pub path: String,
    pub message: String,
}

impl Finding {
    fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            path: path.into(),
            message: message.into(),
        }
    }

    fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Result of a corpus-wide validation run.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub post_count: usize,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
    }

    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    pub fn error_count(&self) -> usize {
        self.errors().count()
    }

    pub fn warning_count(&self) -> usize {
        self.warnings().count()
    }
}

/// Validate every post under `blog_dir`.
pub fn validate_all(blog_dir: &Path) -> Report {
    let mut report = Report::default();

    let post_list = match posts::list_posts(blog_dir) {
        Ok(list) => list,
        Err(err) => {
            report
                .findings
                .push(Finding::error("<corpus>", format!("无法枚举文章：{err}")));
            return report;
        }
    };

    report.post_count = post_list.len();

    // Frontmatter mirror checks.
    for post in &post_list {
        validate_frontmatter(&post.relative, &post.frontmatter, &mut report);
    }

    // Duplicate explicit slugs are a real hazard: they would collide in URLs.
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for post in &post_list {
        if let Some(slug) = post
            .frontmatter
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            // Only slugs inside the same group can collide (URLs include the dir).
            let key = format!("{}::{slug}", post.group.clone().unwrap_or_default());
            if let Some(previous) = seen.insert(key, post.relative.clone()) {
                report.findings.push(Finding::error(
                    &post.relative,
                    format!("slug `{slug}` 与 {previous} 重复"),
                ));
            }
        }
    }

    // Outline link integrity: every `/posts/...` link in an index must resolve.
    let known_urls: HashSet<String> = post_list.iter().map(|p| p.url_path()).collect();
    for post in &post_list {
        if !is_outline_candidate(&post.relative) {
            continue;
        }
        let path = post.path.clone();
        let Ok(outline) = Outline::read(&path) else {
            continue;
        };
        for entry in &outline.entries {
            let Some(href) = entry.solution.href() else {
                continue;
            };
            if !href.starts_with("/posts/") {
                continue;
            }
            if !known_urls.contains(href.trim_end_matches('/')) {
                report.findings.push(Finding::error(
                    &post.relative,
                    format!(
                        "第 {} 行的题解链接指向不存在的文章：{href}",
                        entry.line + 1
                    ),
                ));
            }
        }
    }

    report
}

/// Index posts are those whose file name ends with `-index`.
fn is_outline_candidate(relative: &str) -> bool {
    relative
        .rsplit('/')
        .next()
        .map(|name| name.ends_with("-index.md"))
        .unwrap_or(false)
}

/// Mirror of the site's zod schema, as user-facing checks.
fn validate_frontmatter(relative: &str, fm: &Frontmatter, report: &mut Report) {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        report
            .findings
            .push(Finding::error(relative, "缺少 title"));
    }

    if fm
        .description
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        report
            .findings
            .push(Finding::error(relative, "缺少 description"));
    }

    match fm.pub_datetime.as_deref() {
        None => report
            .findings
            .push(Finding::error(relative, "缺少 pubDatetime")),
        Some(value) => {
            if chrono::DateTime::parse_from_rfc3339(value).is_err() {
                // The site's own pipeline tolerates several shapes here (YAML
                // parses `2026-1-23` as a date and Zod coerces it), so a
                // non-canonical timestamp is a consistency nit, not something
                // that should block a publish.
                report.findings.push(Finding::warning(
                    relative,
                    format!("pubDatetime 不是规范的 ISO 时间：{value}（建议 YYYY-MM-DDTHH:mm:ss.SSS+08:00）"),
                ));
            }
        }
    }

    if fm.tags.is_empty() {
        report
            .findings
            .push(Finding::warning(relative, "tags 为空"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(title: Option<&str>, pub_dt: Option<&str>, tags: Vec<&str>) -> Frontmatter {
        Frontmatter {
            author: Some("a".into()),
            pub_datetime: pub_dt.map(|s| s.to_owned()),
            mod_datetime: None,
            title: title.map(|s| s.to_owned()),
            featured: None,
            draft: None,
            slug: None,
            tags: tags.into_iter().map(|s| s.to_owned()).collect(),
            description: Some("d".into()),
        }
    }

    #[test]
    fn accepts_a_complete_frontmatter() {
        let mut report = Report::default();
        validate_frontmatter(
            "lgj/lg-B2600.md",
            &fm(
                Some("0x0001 【深基1.例2】简单的分苹果"),
                Some("2025-09-16T01:55:41.000+08:00"),
                vec!["《深入浅出程序设计竞赛 基础篇》"],
            ),
            &mut report,
        );
        assert!(!report.has_errors(), "unexpected: {:?}", report.findings);
        assert_eq!(report.warning_count(), 0);
    }

    #[test]
    fn flags_missing_and_malformed_fields() {
        let mut report = Report::default();
        validate_frontmatter("solu/x.md", &fm(None, Some("not-a-date"), vec![]), &mut report);

        let messages: Vec<&str> = report.findings.iter().map(|f| f.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("title")), "{messages:?}");
        // A malformed timestamp is advisory: the site's pipeline still builds.
        assert!(
            messages.iter().any(|m| m.contains("pubDatetime")),
            "{messages:?}"
        );
        assert_eq!(report.error_count(), 1, "only the missing title blocks");
        assert_eq!(report.warning_count(), 2, "timestamp + empty tags");
    }

    #[test]
    fn a_missing_timestamp_blocks_but_an_unpadded_one_does_not() {
        let mut report = Report::default();
        validate_frontmatter("a.md", &fm(Some("t"), None, vec!["x"]), &mut report);
        assert!(report.has_errors(), "a missing pubDatetime is required by the schema");

        let mut report = Report::default();
        // The shape found in the wild on lgj/lg-P5704.md.
        validate_frontmatter(
            "lgj/lg-P5704.md",
            &fm(Some("t"), Some("2026-1-23T23:00:00.000+08:00"), vec!["x"]),
            &mut report,
        );
        assert!(!report.has_errors(), "an unpadded date must not block publishing");
        assert_eq!(report.warning_count(), 1);
    }
}
