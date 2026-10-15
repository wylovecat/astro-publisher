//! Headless CLI for the content operations.
//!
//! Every capability the desktop app exposes is available here too, so the tool
//! works over SSH, in scripts, and on machines without a graphical session.
//!
//! ```text
//! astro-publisher-core check       [--repo <path>]
//! astro-publisher-core books       [--repo <path>]
//! astro-publisher-core posts       [--repo <path>] [--drafts]
//! astro-publisher-core new-post    --title <t> --group <dir> --file-name <name> [--tag <t>]... [--draft]
//! astro-publisher-core new-chapter --book <id> --section <s> --problem <P1001> --title <t> [--video <url>]
//! astro-publisher-core link-solution --book <id> --problem <P1001> --post <url> [--section <s>]
//! astro-publisher-core link-video    --book <id> --problem <P1001> --url <url> [--section <s>]
//! astro-publisher-core unlink        --book <id> --problem <P1001> [--section <s>] [--clear solution|video|both]
//! astro-publisher-core set-check     --book <id> --problem <P1001> (--on | --off) [--section <s>]
//! astro-publisher-core preview     --file <path.md> | --text <markdown>
//! astro-publisher-core publish     --message <m> [--repo <path>] [--stage <path>]... [--no-build] [--dry-run]
//! ```
//!
//! `publish` stages the whole tree unless `--stage` is given, which takes
//! repository-relative paths.

use astro_publisher_core::{books, chapters, posts, preview, process, publish, slug, validate};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return ExitCode::from(2);
    };

    let parsed = match Cli::parse(&args[1..]) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("参数错误：{err}");
            return ExitCode::from(2);
        }
    };

    let repo = parsed.repo.clone().unwrap_or_else(|| PathBuf::from("."));
    let blog = blog_dir(&repo);

    let result = match command {
        "check" => cmd_check(&blog),
        "books" => cmd_books(&repo),
        "posts" => cmd_posts(&blog, parsed.drafts),
        "new-post" => cmd_new_post(&blog, &parsed),
        "new-chapter" => cmd_new_chapter(&repo, &blog, &parsed),
        "link-solution" => cmd_link_solution(&repo, &blog, &parsed),
        "link-video" => cmd_link_video(&repo, &blog, &parsed),
        "unlink" => cmd_unlink(&repo, &blog, &parsed),
        "set-check" => cmd_set_check(&repo, &blog, &parsed),
        "preview" => cmd_preview(&parsed),
        "publish" => cmd_publish(&repo, &parsed),
        "help" | "--help" | "-h" => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("未知命令：{other}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("✗ {message}");
            ExitCode::FAILURE
        }
    }
}

/// `src/data/blog` for a repository root.
fn blog_dir(repo: &Path) -> PathBuf {
    repo.join("src").join("data").join("blog")
}

fn print_usage() {
    println!(
        "astro-publisher-core — blog content operations\n\
         \n\
         用法: astro-publisher-core <命令> [选项]\n\
         \n\
         命令:\n\
         \x20 check         校验全部文章的 frontmatter 与大纲链接\n\
         \x20 books         列出 books.json 中登记的书籍\n\
         \x20 posts         列出文章（--drafts 含草稿）\n\
         \x20 new-post      新建普通文章\n\
         \x20 new-chapter   新建课程章节并同步大纲\n\
         \x20 link-solution 给大纲某题关联题解（📖）\n\
         \x20 link-video    给大纲某题关联视频（📺）\n\
         \x20 unlink        清除关联，回到 📖题解 / 📺视频 占位\n\
         \x20 set-check     勾选或取消勾选大纲某题\n\
         \x20 preview       渲染 Markdown 预览（HTML）\n\
         \x20 publish       校验 → 构建 → 提交 → 推送\n\
         \n\
         大纲编辑选项（link-solution / link-video / unlink / set-check）:\n\
         \x20 --book <id>     书 id（books 列出）\n\
         \x20 --problem <id>  题号，如 P1001\n\
         \x20 --section <s>   小节标题；同一题号出现在多个小节时必须给\n\
         \x20 --post <url>    题解链接（站内 URL，如 /posts/lgj/_0x0013）\n\
         \x20 --url <url>     视频链接\n\
         \x20 --clear <which> unlink 清哪个：solution / video / both（默认 both）\n\
         \x20 --on | --off    set-check 勾选 / 取消勾选\n\
         \n\
         publish 选项:\n\
         \x20 --stage <path> 只暂存指定路径（可重复）；不给则暂存全部改动\n\
         \x20 --no-build    跳过本地 pnpm build\n\
         \x20 --dry-run     只校验与构建，不提交不推送\n\
         \n\
         通用选项:\n\
         \x20 --repo <path> 仓库根目录（默认当前目录）"
    );
}

/// Parsed command-line options.
#[derive(Debug, Default, Clone)]
struct Cli {
    repo: Option<PathBuf>,
    title: Option<String>,
    group: Option<String>,
    file_name: Option<String>,
    tags: Vec<String>,
    draft: bool,
    drafts: bool,
    book: Option<String>,
    section: Option<String>,
    problem: Option<String>,
    video: Option<String>,
    file: Option<PathBuf>,
    text: Option<String>,
    /// Site URL of the post a 题解 marker points at.
    post: Option<String>,
    /// External URL a 视频 marker points at.
    url: Option<String>,
    on: bool,
    off: bool,
    /// Which marker `unlink` should clear: `solution`, `video` or `both`.
    clear: Option<String>,
    message: Option<String>,
    /// Paths to stage explicitly; empty means "stage everything".
    stage: Vec<PathBuf>,
    no_build: bool,
    dry_run: bool,
}

impl Cli {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut cli = Cli::default();
        let mut i = 0;

        while i < args.len() {
            let arg = args[i].as_str();
            let mut take_value = |name: &str| -> Result<String, String> {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| format!("{name} 需要一个值"))
            };

            match arg {
                "--repo" => cli.repo = Some(PathBuf::from(take_value("--repo")?)),
                "--title" => cli.title = Some(take_value("--title")?),
                "--group" => cli.group = Some(take_value("--group")?),
                "--file-name" => cli.file_name = Some(take_value("--file-name")?),
                "--tag" => cli.tags.push(take_value("--tag")?),
                "--draft" => cli.draft = true,
                "--drafts" => cli.drafts = true,
                "--book" => cli.book = Some(take_value("--book")?),
                "--section" => cli.section = Some(take_value("--section")?),
                "--problem" => cli.problem = Some(take_value("--problem")?),
                "--video" => cli.video = Some(take_value("--video")?),
                "--file" => cli.file = Some(PathBuf::from(take_value("--file")?)),
                "--text" => cli.text = Some(take_value("--text")?),
                "--post" => cli.post = Some(take_value("--post")?),
                "--url" => cli.url = Some(take_value("--url")?),
                "--on" => cli.on = true,
                "--off" => cli.off = true,
                "--clear" => cli.clear = Some(take_value("--clear")?),
                "--message" | "-m" => cli.message = Some(take_value("--message")?),
                "--stage" => cli.stage.push(PathBuf::from(take_value("--stage")?)),
                "--no-build" => cli.no_build = true,
                "--dry-run" => cli.dry_run = true,
                other => return Err(format!("未知选项 {other}")),
            }
            i += 1;
        }

        Ok(cli)
    }

    fn require(&self, value: &Option<String>, name: &str) -> Result<String, String> {
        value
            .clone()
            .ok_or_else(|| format!("缺少必填参数 {name}"))
    }
}

// --- commands ---------------------------------------------------------------

fn cmd_check(blog: &Path) -> Result<ExitCode, String> {
    let report = validate::validate_all(blog);
    println!("已校验 {} 篇文章", report.post_count);

    for finding in &report.findings {
        let label = match finding.severity {
            validate::Severity::Error => "错误",
            validate::Severity::Warning => "警告",
        };
        println!("[{label}] {} — {}", finding.path, finding.message);
    }

    if report.has_errors() {
        println!(
            "\n✗ {} 个错误，{} 个警告",
            report.error_count(),
            report.warning_count()
        );
        return Ok(ExitCode::FAILURE);
    }

    println!("\n✓ 无错误（{} 个警告）", report.warning_count());
    Ok(ExitCode::SUCCESS)
}

fn cmd_books(repo: &Path) -> Result<ExitCode, String> {
    let list = books::list_books(repo).map_err(|e| e.to_string())?;
    if list.is_empty() {
        println!("books.json 中没有书籍");
        return Ok(ExitCode::SUCCESS);
    }

    for book in &list {
        println!("{}  {}", book.id, book.title);
        println!("    大纲: {}", book.index_path());
        if let Some(sub) = &book.subtitle {
            println!("    副标题: {sub}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_posts(blog: &Path, include_drafts: bool) -> Result<ExitCode, String> {
    let list = posts::list_posts(blog).map_err(|e| e.to_string())?;
    let mut shown = 0;

    for post in &list {
        let draft = post.frontmatter.draft.unwrap_or(false);
        if draft && !include_drafts {
            continue;
        }
        let mark = if draft { " [草稿]" } else { "" };
        println!(
            "{}{mark}\n    {}  ->  {}",
            post.frontmatter.title.as_deref().unwrap_or("(无标题)"),
            post.relative,
            post.url_path()
        );
        shown += 1;
    }

    println!("\n共 {shown} 篇（总计 {}）", list.len());
    Ok(ExitCode::SUCCESS)
}

fn cmd_new_post(blog: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let title = cli.require(&cli.title, "--title")?;
    let file_name = match &cli.file_name {
        Some(name) => name.clone(),
        None => posts::slugify_stem(&title),
    };

    let input = posts::NewPost {
        group: cli.group.clone().unwrap_or_default(),
        file_name,
        title: title.clone(),
        description: format!("{title}。"),
        tags: cli.tags.clone(),
        slug: None,
        draft: cli.draft,
        featured: false,
        author: default_author(),
    };

    let post = posts::create_post(blog, &input).map_err(|e| e.to_string())?;
    println!("✓ 已创建 {}", post.relative);
    println!("  URL: {}", post.url_path());
    if cli.draft {
        println!("  （草稿，发布前不会出现在站点上）");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_new_chapter(repo: &Path, blog: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let book_id = cli.require(&cli.book, "--book")?;
    let section = cli.require(&cli.section, "--section")?;
    let problem = cli.require(&cli.problem, "--problem")?;
    let title = cli.require(&cli.title, "--title")?;

    let book = books::get_book(repo, &book_id).map_err(|e| e.to_string())?;
    // Read the configured template before writing anything: an unreadable
    // template must fail the command, not silently produce a different chapter.
    let template = astro_publisher_core::config::Config::load().chapter_template_text(repo)?;

    let input = chapters::NewChapter {
        problem_id: problem.clone(),
        title: title.clone(),
        section: section.clone(),
        video_url: cli.video.clone(),
        author: default_author(),
        // Book-dependent: see `NewChapter::check_implies_solution`.
        check_implies_solution: true,
    };

    let outcome =
        chapters::create_chapter(blog, &book.as_book_ref(), &input, template.as_deref())
            .map_err(|e| e.to_string())?;

    println!("✓ 已创建章节  {}", outcome.title_line);
    println!("  章节文件: {}", outcome.chapter_path.display());
    println!("  大纲文件: {}", outcome.outline_path.display());
    println!("  slug: {}", outcome.slug);
    if outcome.inserted_entry {
        println!("  在大纲 `{section}` 中新增了一行");
    } else {
        println!("  已更新大纲中 `{section}` 的既有行");
    }
    Ok(ExitCode::SUCCESS)
}

/// Every outline edit shares this shape: book → outline file, then a
/// single-line change addressed the way a human would name it.
fn outline_path_of(repo: &Path, blog: &Path, book_id: &str) -> Result<PathBuf, String> {
    let book = books::get_book(repo, book_id).map_err(|e| e.to_string())?;
    let path = blog.join(book.index_path());
    if !path.is_file() {
        return Err(format!(
            "书 {book_id} 的大纲文件不存在：{}",
            path.display()
        ));
    }
    Ok(path)
}

/// Print the before/after of an outline edit, which is the only useful output:
/// one line changed in a file the user is also editing by hand.
fn report_edit(edit: &astro_publisher_core::outline::LineEdit) {
    println!(
        "✓ {} 第 {} 行（小节 `{}`）",
        edit.path.display(),
        edit.line + 1,
        edit.section
    );
    println!("  - {}", edit.before.trim_end());
    println!("  + {}", edit.after.trim_end());
}

fn cmd_link_solution(repo: &Path, blog: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let book = cli.require(&cli.book, "--book")?;
    let problem = cli.require(&cli.problem, "--problem")?;
    let href = cli.require(&cli.post, "--post")?;
    let path = outline_path_of(repo, blog, &book)?;

    let edit = astro_publisher_core::outline::set_link(
        &path,
        &problem,
        cli.section.as_deref(),
        astro_publisher_core::outline::MarkerKind::Solution,
        &href,
    )
    .map_err(|e| e.to_string())?;
    report_edit(&edit);
    Ok(ExitCode::SUCCESS)
}

fn cmd_link_video(repo: &Path, blog: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let book = cli.require(&cli.book, "--book")?;
    let problem = cli.require(&cli.problem, "--problem")?;
    let url = cli.require(&cli.url, "--url")?;
    let path = outline_path_of(repo, blog, &book)?;

    let edit = astro_publisher_core::outline::set_link(
        &path,
        &problem,
        cli.section.as_deref(),
        astro_publisher_core::outline::MarkerKind::Video,
        &url,
    )
    .map_err(|e| e.to_string())?;
    report_edit(&edit);
    Ok(ExitCode::SUCCESS)
}

/// Remove a link, putting the marker back to its placeholder.
fn cmd_unlink(repo: &Path, blog: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let book = cli.require(&cli.book, "--book")?;
    let problem = cli.require(&cli.problem, "--problem")?;
    let path = outline_path_of(repo, blog, &book)?;

    // No `--clear` (or `both`) means "clear everything on this row", which is
    // what "this row has nothing written yet" means to someone tidying the
    // outline. `--video` was not an option here on purpose: `new-chapter`
    // already spells `--video <url>`, and one flag meaning two things is how a
    // script ends up writing the wrong link.
    let kinds: Vec<astro_publisher_core::outline::MarkerKind> = match cli
        .clear
        .as_deref()
        .map(str::trim)
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("solution") | Some("题解") => vec![astro_publisher_core::outline::MarkerKind::Solution],
        Some("video") | Some("视频") => vec![astro_publisher_core::outline::MarkerKind::Video],
        None | Some("both") | Some("all") => vec![
            astro_publisher_core::outline::MarkerKind::Solution,
            astro_publisher_core::outline::MarkerKind::Video,
        ],
        Some(other) => {
            return Err(format!(
                "--clear 只接受 solution / video / both，收到 `{other}`"
            ));
        }
    };

    for kind in kinds {
        let edit = astro_publisher_core::outline::clear_link(
            &path,
            &problem,
            cli.section.as_deref(),
            kind,
        )
        .map_err(|e| e.to_string())?;
        println!("{} 已清除", kind.label());
        report_edit(&edit);
    }
    Ok(ExitCode::SUCCESS)
}

/// Tick or untick one entry.
///
/// Separate from the link commands on purpose: what `[x]` means differs per
/// book (基础篇 tracks 题解, 进阶篇 tracks 视频), so the caller decides rather
/// than the tool guessing.
fn cmd_set_check(repo: &Path, blog: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let book = cli.require(&cli.book, "--book")?;
    let problem = cli.require(&cli.problem, "--problem")?;
    let path = outline_path_of(repo, blog, &book)?;

    let checked = match (cli.on, cli.off) {
        (true, false) => true,
        (false, true) => false,
        _ => return Err("需要 --on 或 --off 其中之一".to_owned()),
    };

    let edit = astro_publisher_core::outline::set_checked(
        &path,
        &problem,
        cli.section.as_deref(),
        checked,
    )
    .map_err(|e| e.to_string())?;
    report_edit(&edit);
    Ok(ExitCode::SUCCESS)
}

fn cmd_preview(cli: &Cli) -> Result<ExitCode, String> {    let source = match (&cli.file, &cli.text) {
        (Some(path), _) => std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?,
        (None, Some(text)) => text.clone(),
        (None, None) => return Err("需要 --file 或 --text".to_owned()),
    };

    // For a whole post, render the body only: the frontmatter is form data, and
    // rendering it as Markdown would show up as a stray horizontal rule plus
    // raw YAML (matching what the editor view does).
    let body = match astro_publisher_core::frontmatter::MarkdownFile::parse(&source, "<input>") {
        Ok(file) => file.body,
        Err(_) => source,
    };

    let rendered = preview::render_preview(&body);
    println!("{}", rendered.html);

    if !rendered.math_failures.is_empty() {
        eprintln!(
            "\n⚠ {} 个公式未能渲染（已按原文显示）：{}",
            rendered.math_failures.len(),
            rendered.math_failures.join(" | ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_publish(repo: &Path, cli: &Cli) -> Result<ExitCode, String> {
    let message = cli.require(&cli.message, "--message")?;

    // A CLI run has no session to draw a selection from, so it stages the whole
    // tree unless told otherwise. `--stage` exists for the same reason the GUI
    // has a checklist: a blog checkout often has unrelated work in it (a config
    // tweak, a half-finished article), and one `git add -A` sweeps it in.
    if cli.stage.is_empty() {
        println!("提示：未指定 --stage，将暂存全部改动（与旧行为一致）");
    } else {
        println!("将只暂存 {} 个指定路径", cli.stage.len());
    }

    let options = publish::PublishOptions {
        repo: repo.to_path_buf(),
        message,
        run_build: !cli.no_build,
        dry_run: cli.dry_run,
        git_remote: "origin".to_owned(),
        git_branch: "main".to_owned(),
        stage_paths: (!cli.stage.is_empty()).then(|| cli.stage.clone()),
    };

    let cancel = Arc::new(AtomicBool::new(false));
    // The CLI prints the pipeline's events as they arrive; the desktop app
    // renders the same events into a log panel.
    let mut printer = |event: publish::PublishEvent| match event {
        publish::PublishEvent::Step {
            index,
            state,
            detail,
            ..
        } => println!(
            "[{}/{}] {:<8} {detail}",
            index + 1,
            publish::STEP_COUNT,
            state.label()
        ),
        publish::PublishEvent::Log(line) => match line.stream {
            process::Stream::Stderr => eprintln!("  {}", line.text),
            process::Stream::Stdout => println!("  {}", line.text),
        },
        publish::PublishEvent::Finished(summary) => {
            if summary.pushed {
                println!(
                    "✓ 已推送 origin/{}（{} 篇文章，{} 个警告）",
                    summary.branch, summary.post_count, summary.warning_count
                );
            } else {
                println!("✓ 演练完成，未改动仓库");
            }
        }
    };

    match run_blocking(publish::run(&options, cancel, &mut printer)) {
        Ok(_) => Ok(ExitCode::SUCCESS),
        // Validation errors were already listed line by line through the event
        // stream, so only the count is worth repeating here.
        Err(publish::PublishError::Validation { findings }) => {
            eprintln!("✗ 有 {} 个错误，已中止发布", findings.len());
            Ok(ExitCode::FAILURE)
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Block on the publish pipeline from the synchronous CLI.
///
/// The pipeline owns its own tokio runtime usage, so this only has to drive it to
/// completion and hand back the typed error for reporting.
fn run_blocking<F, T>(future: F) -> Result<T, publish::PublishError>
where
    F: std::future::Future<Output = Result<T, publish::PublishError>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}

fn default_author() -> String {
    // A neutral placeholder: real author names belong in the blog's own
    // frontmatter, not baked into a published tool.
    "Blog Author".to_owned()
}

/// Turn a title into a conservative file name.


/// Exposed for tests: the `_0x` sequence helper used by `new-chapter` output.
#[allow(dead_code)]
fn next_slug_for(blog: &Path) -> String {
    slug::allocate_slug(blog)
}

