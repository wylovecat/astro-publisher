//! MCP server for the Astro blog: the `core` content operations exposed as
//! tools an AI assistant can call.
//!
//! This binary is a thin adapter, exactly like the CLI. Every tool body is a
//! call into `astro_publisher_core`, so an assistant that creates a chapter goes
//! through the same slug allocation, the same atomic write-with-rollback, and
//! the same validation gate as the desktop app. Nothing is reimplemented here.
//!
//! Two deliberate safety choices:
//!
//! * **Publishing defaults to a dry run.** `publish` takes `dry_run` and defaults
//!   it to `true`; actually pushing to the live site must be asked for
//!   explicitly. A tool call is not a confirmation.
//! * **Writing and reading are separate tools.** `preview_chapter` touches no
//!   files, so an assistant can show the user the exact slug and outline line it
//!   is about to write before calling `create_chapter`.
//!
//! The server speaks JSON-RPC over stdin/stdout. Nothing may print to stdout
//! except the protocol: diagnostics go to stderr (which MCP clients surface as
//! server logs).

use astro_publisher_core::config::Config;
use astro_publisher_core::{books, chapters, outline, posts, problem, publish, validate};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use serde_json::json;

/// Tool arguments. Field names and doc comments become the tool's JSON Schema,
/// which is what the assistant sees, so they are written for that audience.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListPostsArgs {
    /// Include drafts. Defaults to true (this is a content-authoring tool, not
    /// the published site).
    #[serde(default)]
    include_drafts: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReadPostArgs {
    /// Path of the post as reported by `list_posts` (`relative`), or its slug.
    path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct BookArgs {
    /// Book id from `list_books`, e.g. `basic`.
    book_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ProblemArgs {
    /// Luogu problem id, e.g. `P5703`.
    problem_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PreviewChapterArgs {
    /// Book id from `list_books`.
    book_id: String,
    /// Luogu problem id, e.g. `B2620`.
    problem_id: String,
    /// Problem name, e.g. `【深基3.例1】数字比较`. Use `lookup_problem` to get it.
    title: String,
    /// Outline section (chapter heading) the problem belongs to, exactly as
    /// `list_sections` reports it.
    section: String,
    /// Optional Bilibili video URL to link on the outline line.
    #[serde(default)]
    video_url: Option<String>,
    /// Whether `[x]` should follow the solution link (basic-book semantics).
    #[serde(default)]
    check_implies_solution: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreateChapterArgs {
    /// Book id from `list_books`.
    book_id: String,
    /// Luogu problem id, e.g. `B2620`.
    problem_id: String,
    /// Problem name, e.g. `【深基3.例1】数字比较`.
    title: String,
    /// Outline section (chapter heading) the problem belongs to.
    section: String,
    /// Optional Bilibili video URL to link on the outline line.
    #[serde(default)]
    video_url: Option<String>,
    /// Whether `[x]` should follow the solution link (basic-book semantics).
    #[serde(default)]
    check_implies_solution: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreatePostArgs {
    /// Subdirectory under the blog root, e.g. `solu`. Omit or pass "" for the
    /// blog root.
    #[serde(default)]
    group: Option<String>,
    /// File stem without extension. Defaults to a slugified `title`.
    #[serde(default)]
    file_name: Option<String>,
    title: String,
    #[serde(default)]
    tags: Vec<String>,
    /// Create as a draft: invisible on the site until it is published.
    #[serde(default)]
    draft: Option<bool>,
    /// Explicit frontmatter slug. Empty means the URL uses the file name.
    #[serde(default)]
    slug: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LinkArgs {
    /// Book id from `list_books`.
    book_id: String,
    /// Luogu problem id, e.g. `B2620`.
    problem_id: String,
    /// Outline section, exactly as `list_sections` reports it. Required when the
    /// same problem id appears under more than one section.
    #[serde(default)]
    section: Option<String>,
    /// For `link_solution`: the post's site URL, e.g. `/posts/lgj/_0x0013`.
    #[serde(default)]
    post_url: Option<String>,
    /// For `link_video`: the external video URL.
    #[serde(default)]
    video_url: Option<String>,
    /// For `set_check`: the new checkbox state. Required by that tool.
    #[serde(default)]
    checked: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PublishArgs {
    /// Commit message.
    message: String,
    /// Validate and build only; neither commit nor push. Defaults to **true**,
    /// because a tool call is not a confirmation to publish to the live site.
    /// Set to `false` only when the user has explicitly asked to publish.
    #[serde(default)]
    dry_run: Option<bool>,
    /// Run `pnpm build` before committing. Defaults to the configured setting.
    #[serde(default)]
    run_build: Option<bool>,
}

/// The server. Holds only the resolved configuration; all state lives in the
/// repository, so a fresh process always sees the current files.
#[derive(Clone)]
struct BlogServer {
    config: Config,
}

impl BlogServer {
    fn new() -> Self {
        Self {
            config: Config::load(),
        }
    }

    /// Repository root, or a tool-level error explaining how to set it.
    fn repo(&self) -> Result<std::path::PathBuf, String> {
        if self.config.repo_path.trim().is_empty() {
            return Err(
                "还没有配置博客仓库路径。请在桌面应用的「设置」页填写，\
                 或设置环境变量 ASTRO_PUBLISHER_REPO。"
                    .to_owned(),
            );
        }
        Ok(self.config.repo())
    }

    fn book_ref(&self, book_id: &str) -> Result<chapters::BookRef, String> {
        let repo = self.repo()?;
        books::get_book(&repo, book_id)
            .map(|book| book.as_book_ref())
            .map_err(|err| err.to_string())
    }

    /// Path of a book's outline post, resolved from its id.
    fn outline_path(&self, book_id: &str) -> Result<std::path::PathBuf, String> {
        let repo = self.repo()?;
        let book = books::get_book(&repo, book_id).map_err(|err| err.to_string())?;
        Ok(repo.join("src/data/blog").join(book.index_path()))
    }

    /// Turn a completed outline edit into the tool result, which carries the
    /// line before and after: an assistant pasting one line into a file the user
    /// also edits by hand needs to be able to show what changed.
    fn edit_result(
        edit: Result<outline::LineEdit, outline::OutlineError>,
        what: &str,
    ) -> CallToolResult {
        match edit {
            Ok(edit) => tool_json(json!({
                "changed": what,
                "path": edit.path.display().to_string(),
                "line": edit.line + 1,
                "section": edit.section,
                "before": edit.before,
                "after": edit.after,
            })),
            Err(err) => tool_error(format!("{what}失败：{err}")),
        }
    }
}

#[tool_router]
impl BlogServer {
    #[tool(
        description = "List the blog's posts: relative path, title, draft flag, URL and tags."
    )]
    fn list_posts(&self, Parameters(args): Parameters<ListPostsArgs>) -> CallToolResult {
        let include_drafts = args.include_drafts.unwrap_or(true);
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };

        match posts::list_posts(&repo.join("src/data/blog")) {
            Ok(list) => {
                let rows: Vec<_> = list
                    .iter()
                    .filter(|post| include_drafts || !post.frontmatter.draft.unwrap_or(false))
                    .map(|post| {
                        json!({
                            "relative": post.relative,
                            "title": post.frontmatter.title,
                            "draft": post.frontmatter.draft.unwrap_or(false),
                            "url": post.url_path(),
                            "tags": post.frontmatter.tags,
                        })
                    })
                    .collect();

                if rows.is_empty() {
                    return tool_error("没有找到文章。".to_owned());
                }
                tool_json(json!({ "count": rows.len(), "posts": rows }))
            }
            Err(err) => tool_error(format!("读取文章失败：{err}")),
        }
    }

    #[tool(
        description = "Read one post in full: frontmatter fields plus the Markdown body."
    )]
    fn read_post(&self, Parameters(args): Parameters<ReadPostArgs>) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };
        let blog = repo.join("src/data/blog");

        // Accept either a relative path from `list_posts` or a bare slug.
        let candidate = blog.join(args.path.trim().trim_start_matches('/'));
        let path = if candidate.is_file() {
            candidate
        } else {
            match posts::list_posts(&blog) {
                Ok(list) => match list.into_iter().find(|post| {
                    post.relative == args.path.trim()
                        || post.frontmatter.slug.as_deref() == Some(args.path.trim())
                }) {
                    Some(post) => post.path,
                    None => {
                        return tool_error(format!(
                            "没有找到文章 `{}`。先用 list_posts 看可用的路径。",
                            args.path
                        ));
                    }
                },
                Err(err) => return tool_error(format!("读取文章失败：{err}")),
            }
        };

        match std::fs::read_to_string(&path) {
            Ok(text) => tool_json(json!({ "path": path.display().to_string(), "text": text })),
            Err(err) => tool_error(format!("读取 {} 失败：{err}", path.display())),
        }
    }

    #[tool(description = "List the registered course books (id, title, outline post id).")]
    fn list_books(&self) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };

        match books::list_books(&repo) {
            Ok(list) => tool_json(json!({
                "books": list.iter().map(|book| json!({
                    "id": book.id,
                    "title": book.title,
                    "index_id": book.index_id,
                })).collect::<Vec<_>>(),
            })),
            Err(err) => tool_error(format!("读取书籍登记表失败：{err}")),
        }
    }

    #[tool(
        description = "List a book's outline sections, i.e. the valid `section` values for preview_chapter and create_chapter."
    )]
    fn list_sections(&self, Parameters(args): Parameters<BookArgs>) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };
        let book = match self.book_ref(&args.book_id) {
            Ok(book) => book,
            Err(message) => return tool_error(message),
        };

        let path = repo.join("src/data/blog").join(book.index_path());
        match astro_publisher_core::outline::Outline::read(&path) {
            Ok(outline) => {
                let sections: Vec<_> = outline
                    .sections
                    .iter()
                    .map(|section| {
                        json!({
                            "title": section.text,
                            "entries": section.entries.len(),
                        })
                    })
                    .collect();
                tool_json(json!({ "book": book.title, "sections": sections }))
            }
            Err(err) => tool_error(format!("读取大纲失败：{err}")),
        }
    }

    #[tool(
        description = "Look a Luogu problem up: name, difficulty and tags, by problem id. Use this to fill in `title` for a chapter."
    )]
    fn lookup_problem(&self, Parameters(args): Parameters<ProblemArgs>) -> CallToolResult {
        match problem::lookup_cached(&args.problem_id) {
            Ok(info) => tool_json(json!({
                "pid": info.pid,
                "name": info.name,
                "difficulty": info.difficulty,
                "difficulty_label": info.difficulty_label(),
                "tags": info.tags,
            })),
            Err(err) => tool_error(format!("抓取题目信息失败：{err}")),
        }
    }

    #[tool(
        description = "Run the validation gate (frontmatter structure + broken /posts/ links). Errors block publishing; warnings do not."
    )]
    fn validate(&self) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };

        let report = validate::validate_all(&repo.join("src/data/blog"));
        tool_json(json!({
            "has_errors": report.has_errors(),
            "findings": report.findings.iter().map(|finding| json!({
                "severity": format!("{:?}", finding.severity),
                "path": finding.path,
                "message": finding.message,
            })).collect::<Vec<_>>(),
        }))
    }

    #[tool(
        description = "Show exactly what create_chapter would do — the slug it would allocate, the chapter path, and the outline line before/after — without writing anything. Call this first and show the user."
    )]
    fn preview_chapter(&self, Parameters(args): Parameters<PreviewChapterArgs>) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };
        let book = match self.book_ref(&args.book_id) {
            Ok(book) => book,
            Err(message) => return tool_error(message),
        };

        let input = chapters::NewChapter {
            problem_id: args.problem_id,
            title: args.title,
            section: args.section,
            video_url: args.video_url,
            author: self.config.author.clone(),
            check_implies_solution: args.check_implies_solution.unwrap_or(true),
        };

        match chapters::preview_chapter(&repo.join("src/data/blog"), &book, &input) {
            Ok(preview) => tool_json(json!({
                "slug": preview.slug,
                "title_line": preview.title_line,
                "chapter_path": preview.chapter_path.display().to_string(),
                "outline_path": preview.outline_path.display().to_string(),
                "outline_before": preview.outline_before,
                "outline_after": preview.outline_after,
                "inserts_new_line": preview.inserted_entry,
            })),
            Err(err) => tool_error(format!("预演失败：{err}")),
        }
    }

    #[tool(
        description = "Create a course chapter: write the chapter file and update the book outline in one atomic operation (rolls back on failure). Call preview_chapter first and confirm with the user."
    )]
    fn create_chapter(&self, Parameters(args): Parameters<CreateChapterArgs>) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };
        let book = match self.book_ref(&args.book_id) {
            Ok(book) => book,
            Err(message) => return tool_error(message),
        };

        let input = chapters::NewChapter {
            problem_id: args.problem_id,
            title: args.title,
            section: args.section,
            video_url: args.video_url,
            author: self.config.author.clone(),
            check_implies_solution: args.check_implies_solution.unwrap_or(true),
        };

        // Read before writing: a configured-but-unreadable template has to fail
        // the call, not quietly produce a chapter from a different skeleton.
        let template = match self.config.chapter_template_text(&repo) {
            Ok(template) => template,
            Err(message) => return tool_error(message),
        };

        match chapters::create_chapter(
            &repo.join("src/data/blog"),
            &book,
            &input,
            template.as_deref(),
        ) {
            Ok(outcome) => tool_json(json!({
                "chapter_path": outcome.chapter_path.display().to_string(),
                "slug": outcome.slug,
                "title_line": outcome.title_line,
                "outline_after": outcome.outline_after,
                "inserts_new_line": outcome.inserted_entry,
            })),
            Err(err) => tool_error(format!("创建章节失败：{err}")),
        }
    }

    #[tool(
        description = "Create a plain post (not a course chapter) with the standard frontmatter. Returns its path and URL."
    )]
    fn create_post(&self, Parameters(args): Parameters<CreatePostArgs>) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };

        let file_name = args
            .file_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| posts::slugify_stem(&args.title));

        let input = posts::NewPost {
            group: args.group.unwrap_or_default(),
            file_name: file_name.clone(),
            title: args.title.clone(),
            description: format!("{}。", args.title.trim()),
            tags: args.tags,
            slug: args.slug,
            draft: args.draft.unwrap_or(false),
            featured: false,
            author: self.config.author.clone(),
        };

        match posts::create_post(&repo.join("src/data/blog"), &input) {
            Ok(post) => tool_json(json!({
                "relative": post.relative,
                "url": post.url_path(),
                "draft": post.frontmatter.draft.unwrap_or(false),
            })),
            Err(err) => tool_error(format!("创建文章失败：{err}")),
        }
    }

    #[tool(
        description = "Link an outline entry's 📖 solution marker to an existing post. Use when the chapter was written outside the tool, or to repoint a link. Does NOT touch the [x] checkbox — what [x] means differs per book, so use set_check for that."
    )]
    fn link_solution(&self, Parameters(args): Parameters<LinkArgs>) -> CallToolResult {
        let path = match self.outline_path(&args.book_id) {
            Ok(path) => path,
            Err(message) => return tool_error(message),
        };
        let Some(post_url) = args.post_url else {
            return tool_error("link_solution 需要 post_url（站内路径，如 /posts/lgj/_0x0013）".to_owned());
        };

        Self::edit_result(
            outline::set_link(
                &path,
                &args.problem_id,
                args.section.as_deref(),
                outline::MarkerKind::Solution,
                &post_url,
            ),
            "关联题解",
        )
    }

    #[tool(
        description = "Link an outline entry's 📺 video marker to a published video URL (Bilibili etc.). Does NOT touch the [x] checkbox."
    )]
    fn link_video(&self, Parameters(args): Parameters<LinkArgs>) -> CallToolResult {
        let path = match self.outline_path(&args.book_id) {
            Ok(path) => path,
            Err(message) => return tool_error(message),
        };
        let Some(video_url) = args.video_url else {
            return tool_error("link_video 需要 video_url".to_owned());
        };

        Self::edit_result(
            outline::set_link(
                &path,
                &args.problem_id,
                args.section.as_deref(),
                outline::MarkerKind::Video,
                &video_url,
            ),
            "关联视频",
        )
    }

    #[tool(
        description = "Tick or untick an outline entry's [x] checkbox. Explicit because the meaning differs per book: the basic book tracks written 题解, the advanced book tracks videos. Call list_sections first to get the exact section text."
    )]
    fn set_check(&self, Parameters(args): Parameters<LinkArgs>) -> CallToolResult {
        let path = match self.outline_path(&args.book_id) {
            Ok(path) => path,
            Err(message) => return tool_error(message),
        };
        let Some(checked) = args.checked else {
            return tool_error("set_check 需要 checked（true 勾选，false 取消）".to_owned());
        };

        Self::edit_result(
            outline::set_checked(&path, &args.problem_id, args.section.as_deref(), checked),
            if checked { "勾选" } else { "取消勾选" },
        )
    }

    #[tool(
        description = "Run the publish pipeline (validate → pnpm build → git commit → git push). DEFAULTS TO A DRY RUN: nothing is committed or pushed unless dry_run is explicitly false. Only set dry_run=false when the user has clearly asked to publish; it pushes to the live site."
    )]
    async fn publish(&self, Parameters(args): Parameters<PublishArgs>) -> CallToolResult {
        let repo = match self.repo() {
            Ok(repo) => repo,
            Err(message) => return tool_error(message),
        };

        let options = publish::PublishOptions {
            repo,
            message: args.message,
            run_build: args.run_build.unwrap_or(self.config.run_local_build),
            // A tool call is not a confirmation: publishing is opt-in.
            dry_run: args.dry_run.unwrap_or(true),
            git_remote: self.config.git_remote.clone(),
            git_branch: self.config.git_branch.clone(),
            // An assistant call has no session behind it to draw a file list
            // from, so this keeps the historical "stage everything" behaviour
            // rather than inventing a selection the user never confirmed.
            stage_paths: None,
        };

        // Collect the pipeline's own log lines so the assistant can report what
        // actually happened, not just pass/fail. The sink is behind a mutex so
        // the closure is `Send`: the tool macro boxes the returned future in a
        // `Send` one.
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = log.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut emit = move |event: publish::PublishEvent| {
            let line = match event {
                publish::PublishEvent::Log(output) => output.text,
                publish::PublishEvent::Step { index, state, .. } => {
                    format!("步骤 {} {:?}", index + 1, state)
                }
                publish::PublishEvent::Finished(_) => return,
            };
            if let Ok(mut lines) = sink.lock() {
                lines.push(line);
            }
        };

        let outcome = publish::run(&options, cancel, &mut emit).await;
        let log = log.lock().map(|lines| lines.clone()).unwrap_or_default();

        match outcome {
            Ok(summary) => tool_json(json!({
                "dry_run": options.dry_run,
                "message": summary.message,
                "log": log,
            })),
            Err(err) => {
                tool_error(format!(
                    "发布未完成：{err}\n\n{}",
                    log.join("\n")
                ))
            }
        }
    }
}

#[tool_handler(
    name = "astro-publisher",
    instructions = "Content operations for an Astro blog: list and read posts, create posts and course chapters, maintain a course outline (link solutions and videos, tick entries), look up Luogu problem metadata, validate the content tree, and publish. Create, link and publish tools write to the repository; preview_chapter is read-only and should be used first. Call list_sections before editing an outline entry so the section text matches exactly. publish defaults to a dry run."
)]
impl ServerHandler for BlogServer {}

/// A tool that ran and failed.
///
/// This is a *tool-level* error, not a protocol error: the message is rendered
/// by the client, so the assistant (and the user) actually sees what went wrong.
/// Returning it as ordinary success text would be worse than useless — an
/// assistant would read a failure as a result.
fn tool_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// A successful tool result carrying JSON.
fn tool_json(value: serde_json::Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // stdout is the JSON-RPC channel; the SDK owns it from here on. Anything
    // this server wants to say goes to stderr, which clients show as logs.
    eprintln!("astro-publisher-mcp {}", env!("CARGO_PKG_VERSION"));

    let service = BlogServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
