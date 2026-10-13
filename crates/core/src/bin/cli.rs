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
//! astro-publisher-core preview     --file <path.md> | --text <markdown>
//! astro-publisher-core publish     --message <m> [--repo <path>] [--no-build] [--dry-run]
//! ```

use astro_publisher_core::{books, chapters, posts, preview, process, slug, validate};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
         \x20 preview       渲染 Markdown 预览（HTML）\n\
         \x20 publish       校验 → 构建 → 提交 → 推送\n\
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
    message: Option<String>,
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
                "--message" | "-m" => cli.message = Some(take_value("--message")?),
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
        None => slugify_file_name(&title),
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
        chapters::create_chapter(blog, &book.as_book_ref(), &input).map_err(|e| e.to_string())?;

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

fn cmd_preview(cli: &Cli) -> Result<ExitCode, String> {
    let source = match (&cli.file, &cli.text) {
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
    let blog = blog_dir(repo);

    // 1. Validation is the gate.
    println!("[1/5] 校验内容…");
    let report = validate::validate_all(&blog);
    if report.has_errors() {
        for finding in report.errors() {
            eprintln!("[错误] {} — {}", finding.path, finding.message);
        }
        return Err(format!("有 {} 个错误，已中止发布", report.error_count()));
    }
    println!(
        "      ✓ {} 篇文章，{} 个警告",
        report.post_count,
        report.warning_count()
    );

    // 2. Local build is the authoritative frontmatter check.
    if cli.no_build {
        println!("[2/5] 跳过本地构建（--no-build）");
    } else {
        println!("[2/5] 构建站点（pnpm build）…");
        let status =
            run_blocking(process::run_collect("pnpm", &["build"], repo)).map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("构建失败：\n{}", status.tail(25)));
        }
        println!("      ✓ 构建通过");
    }

    if cli.dry_run {
        println!("[3/5] 跳过提交（--dry-run）");
        println!("[4/5] 跳过推送（--dry-run）");
        println!("[5/5] 完成（演练模式，未改动仓库）");
        return Ok(ExitCode::SUCCESS);
    }

    // 3. Stage and commit.
    println!("[3/5] 提交改动…");
    let status = run_blocking(process::run_collect("git", &["status", "--porcelain"], repo))
        .map_err(|e| e.to_string())?;
    if status.stdout_text().trim().is_empty() {
        return Err("没有需要提交的改动".to_owned());
    }

    run_blocking(process::run_collect("git", &["add", "-A"], repo)).map_err(|e| e.to_string())?;
    run_blocking(process::run_collect("git", &["commit", "-m", &message], repo))
        .map_err(|e| format!("提交失败：{e}"))?;
    println!("      ✓ 已提交：{message}");

    // 4. Push.
    println!("[4/5] 推送…");
    let branch = current_branch(repo)?;
    match run_blocking(process::run_collect("git", &["push", "origin", &branch], repo)) {
        Ok(_) => println!("      ✓ 已推送到 origin/{branch}"),
        Err(err) => {
            return Err(format!(
                "推送失败，未做任何自动 rebase/强推：\n{err}\n\
                 请手动处理：git pull --rebase origin {branch} 后重试"
            ));
        }
    }

    // 5. The hosting platform builds from the push.
    println!("[5/5] 完成。托管平台会在收到推送后自动构建部署。");
    Ok(ExitCode::SUCCESS)
}

fn current_branch(repo: &Path) -> Result<String, String> {
    let out = run_blocking(process::run_collect(
        "git",
        &["rev-parse", "--abbrev-ref", "HEAD"],
        repo,
    ))
    .map_err(|e| e.to_string())?;
    let branch = out.stdout_text().trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err("无法确定当前分支".to_owned());
    }
    Ok(branch)
}

/// Block on an async process helper from the synchronous CLI.
fn run_blocking<F, T>(future: F) -> Result<T, process::ProcessError>
where
    F: std::future::Future<Output = Result<T, process::ProcessError>>,
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
fn slugify_file_name(title: &str) -> String {
    let ascii: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = ascii
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        format!("post-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"))
    } else {
        collapsed
    }
}

/// Exposed for tests: the `_0x` sequence helper used by `new-chapter` output.
#[allow(dead_code)]
fn next_slug_for(blog: &Path) -> String {
    slug::allocate_slug(blog)
}
