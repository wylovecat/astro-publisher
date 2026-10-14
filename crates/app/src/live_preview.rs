//! The real preview: the blog's own `astro dev` server, opened in the browser.
//!
//! The editor's built-in preview is a re-implementation — the same KaTeX the
//! site uses, rendered by gpui instead of a browser. This is the site itself, so
//! what it shows is true by construction.
//!
//! It lives here rather than inside the editor view because the server outlives
//! any one post: `PublisherApp::open_post` builds a fresh `EditorView` every time
//! another post is opened, so a server owned by the editor would be killed on
//! every switch. The editor embeds this entity and tells it which post is open.
//!
//! Two things the plan called for and are easy to get wrong (plan §4.1, §4.2):
//! drafts are invisible to `astro dev` too (the site's `postFilter` drops them
//! unconditionally), so this is offered only for published posts; and the server
//! is a `node` process holding a port, so it is killed both on demand and when the
//! app quits.

use crate::state::{Config, looks_like_repo};
use astro_publisher_core::process::{self, OutputLine, Stream};
use gpui::prelude::*;
use gpui::{Context, IntoElement, Render, Subscription, Window, div};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    label::Label,
};

/// Astro's default dev port. Kept in one place so the URL the browser is sent to
/// and the port the server is asked for cannot drift apart.
const DEFAULT_PORT: u16 = 4321;

/// How long to wait for the server to announce its address before saying so.
/// A cold start in this project took ~6.4s, but a first-ever `pnpm dev` also has
/// to optimise dependencies.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Log lines kept for the panel: a dev server logs a few lines per page request.
const MAX_LOG: usize = 12;

/// The post the panel is currently offering to preview.
#[derive(Clone)]
struct Target {
    /// Site-relative URL, e.g. `/posts/lgj/_0x0001`.
    url_path: String,
    draft: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum Status {
    Idle,
    Starting,
    Running,
    Failed(String),
}

/// What the background worker reports back to the UI.
enum Event {
    /// The child's pid, so the server can be killed later.
    Started(u32),
    Line(OutputLine),
    /// The server announced its address on this port.
    Ready(u16),
    Timeout,
    Exited,
    Failed(String),
}

pub struct LivePreview {
    config: Config,
    target: Option<Target>,
    status: Status,
    /// Port the server actually ended up on: Astro moves on if 4321 is taken.
    port: u16,
    /// Pid of the running `pnpm dev`, for the kill paths.
    pid: Option<u32>,
    log: Vec<(Stream, String)>,
    notice: String,
    /// Registers the window-close hook; dropping the subscription unregisters it.
    _closed: Subscription,
}

impl LivePreview {
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        // The dev server has to go when the app does: it is a `node` process
        // holding a port, and nothing else will clean it up.
        //
        // The hook is `on_window_closed`, not `on_app_quit`: in this gpui version
        // the quit observers are only run by `App::shutdown`, which a window
        // close does not reach — measured, with a probe in the quit callback that
        // never fired while the process exited. Closed windows, on the other hand,
        // notify their observers before the app quits.
        //
        // A forced kill (task manager) still leaves the server behind; only an OS
        // job object could cover that, and this is the case that happens in
        // practice.
        let closed = cx.on_window_closed({
            let this = cx.weak_entity();
            move |app, _window| {
                let _ = this.update(app, |this, cx| this.stop(cx));
            }
        });

        Self {
            config,
            target: None,
            status: Status::Idle,
            port: DEFAULT_PORT,
            pid: None,
            log: Vec::new(),
            notice: String::new(),
            _closed: closed,
        }
    }

    /// Point the panel at the post the editor is showing.
    ///
    /// Called from the editor's render pass, so it deliberately does not notify:
    /// re-rendering another entity from inside a render would be a loop for no
    /// gain, since the panel is rendered immediately afterwards.
    pub fn set_target(&mut self, url_path: String, draft: bool) {
        let stale = match &self.target {
            Some(current) => current.url_path != url_path || current.draft != draft,
            None => true,
        };
        if stale {
            self.target = Some(Target { url_path, draft });
        }
    }

    /// Start the server if needed, then open the post in the browser.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        if target.draft {
            self.notice =
                "草稿在 `astro dev` 里同样不可见（站点无条件过滤 draft），先取消草稿再预览。"
                    .to_owned();
            cx.notify();
            return;
        }

        match self.status {
            Status::Running => self.open_browser(&target.url_path, cx),
            Status::Starting => {
                self.notice = "开发服务器还在启动，就绪后会自动打开浏览器。".to_owned();
                cx.notify();
            }
            _ => self.start(cx),
        }
    }

    /// Stop the server (and everything it spawned).
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(pid) = self.pid.take() {
            process::kill_tree_blocking(pid);
        }
        self.status = Status::Idle;
        self.notice = "已停止开发服务器。".to_owned();
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !looks_like_repo(&self.config.repo()) {
            self.notice = format!(
                "仓库路径无效：{}（到「设置」里改成博客仓库根目录）",
                self.config.repo().display()
            );
            cx.notify();
            return;
        }

        let repo = self.config.repo();
        let port = DEFAULT_PORT.to_string();
        self.status = Status::Starting;
        self.log.clear();
        self.notice = format!("正在启动 pnpm dev（端口 {DEFAULT_PORT}）…冷启动约 5–10 秒");
        cx.notify();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

        // Spawning, streaming and the readiness wait all need a tokio runtime,
        // so the whole worker owns one, as the publish panel's does. Blocking the
        // UI thread on a 6-second server start would freeze the window.
        //
        // Detached, not awaited: this worker lives exactly as long as the server
        // does, so the drain below has to read events as they arrive. Waiting for
        // this task first would mean the panel never left "starting" — the
        // readiness notice would sit in the channel forever.
        cx.background_executor()
            .spawn(async move {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = tx.send(Event::Failed(format!("无法启动异步运行时：{err}")));
                        return;
                    }
                };

                runtime.block_on(async move {
                    let Ok(mut server) =
                        process::spawn_background("pnpm", &["dev", "--port", &port], &repo)
                    else {
                        let _ = tx.send(Event::Failed(
                            "无法启动 pnpm dev，请看下面的日志".to_owned(),
                        ));
                        return;
                    };

                    if let Some(pid) = server.handle.id() {
                        let _ = tx.send(Event::Started(pid));
                    }

                    // The server runs for as long as the app does, so its output has
                    // to keep being drained: closing the pipes would make Astro's
                    // next write fail.
                    let deadline = tokio::time::sleep(READY_TIMEOUT);
                    tokio::pin!(deadline);
                    let mut timed_out = false;
                    let mut announced = false;

                    loop {
                        tokio::select! {
                            line = server.lines.recv() => match line {
                                Some(line) => {
                                    // Announce the first address we see, and keep
                                    // watching afterwards: a slow start still opens
                                    // the browser, just after the notice.
                                    if !announced {
                                        if let Some(port) = ready_port(&line.text) {
                                            announced = true;
                                            let _ = tx.send(Event::Ready(port));
                                        }
                                    }
                                    let _ = tx.send(Event::Line(line));
                                }
                                None => {
                                    let _ = tx.send(Event::Exited);
                                    return;
                                }
                            },
                            _ = &mut deadline, if !timed_out => {
                                timed_out = true;
                                let _ = tx.send(Event::Timeout);
                            }
                        }
                    }
                });
            })
            .detach();

        // Drain on the UI thread as events arrive, and stop when the worker drops
        // its sender (the server exited).
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.recv().await {
                this.update(cx, |this, cx| this.apply(event, cx)).ok();
            }
        })
        .detach();
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Started(pid) => self.pid = Some(pid),
            Event::Line(line) => {
                self.log.push((line.stream, line.text));
                if self.log.len() > MAX_LOG {
                    let overflow = self.log.len() - MAX_LOG;
                    self.log.drain(0..overflow);
                }
            }
            Event::Ready(port) => {
                self.port = port;
                self.status = Status::Running;
                let path = self
                    .target
                    .as_ref()
                    .map(|target| target.url_path.clone())
                    .unwrap_or_default();
                self.notice = format!("已打开 http://localhost:{port}{path}");
                self.open_browser(&path, cx);
            }
            Event::Timeout => {
                self.notice =
                    "启动得比预期慢（已等 60 秒），就绪后会自动打开浏览器；也可以看下面的日志。"
                        .to_owned();
            }
            Event::Exited => {
                self.status = Status::Idle;
                self.pid = None;
                self.notice = "开发服务器已退出。".to_owned();
            }
            Event::Failed(err) => {
                self.status = Status::Failed(err.clone());
                self.notice = err;
            }
        }
        cx.notify();
    }

    fn open_browser(&self, url_path: &str, cx: &mut Context<Self>) {
        let url = format!("http://localhost:{}{url_path}", self.port);
        cx.open_url(&url);
    }

    /// Short state for the panel's chip.
    fn status_text(&self) -> String {
        match &self.status {
            Status::Idle => "真实预览未启动".to_owned(),
            Status::Starting => format!("启动中（端口 {DEFAULT_PORT}）"),
            Status::Running => format!("运行中 · localhost:{}", self.port),
            Status::Failed(_) => "启动失败".to_owned(),
        }
    }
}

impl Render for LivePreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;

        let draft = self.target.as_ref().map(|t| t.draft).unwrap_or(false);
        let starting = self.status == Status::Starting;
        let running = self.status == Status::Running;
        let status = self.status_text();
        let notice = self.notice.clone();

        let chip_color = if matches!(self.status, Status::Failed(_)) {
            danger
        } else {
            muted
        };

        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .when(draft, |el| {
                el.child(
                    Label::new("草稿：dev 服务器同样不可见")
                        .text_xs()
                        .text_color(muted),
                )
            })
            .when(!draft, |el| {
                el.when(starting, |el| {
                    el.child(
                        Button::new("live-preview-starting")
                            .label("启动中")
                            .small()
                            .secondary()
                            .loading(true),
                    )
                })
                .when(!starting, |el| {
                    el.child(
                        Button::new("live-preview-open")
                            .label(if running {
                                "打开预览页"
                            } else {
                                "真实预览"
                            })
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.open(cx))),
                    )
                })
                .when(running, |el| {
                    el.child(
                        Button::new("live-preview-stop")
                            .label("停止")
                            .small()
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                    )
                })
            });

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(Label::new(status).text_xs().text_color(chip_color))
                    .child(controls),
            )
            .when(!notice.is_empty(), |el| {
                el.child(Label::new(notice).text_xs().text_color(muted))
            })
    }
}

/// The port from Astro's `┃ Local    http://localhost:4321/` line.
///
/// Read back from the log rather than assumed: if the preferred port is busy
/// Astro silently moves to the next free one, and the browser has to follow.
/// The line arrives wrapped in ANSI colour, so matching happens on the plain text.
pub fn ready_port(line: &str) -> Option<u16> {
    let text = process::strip_ansi(line);
    let start = text.find("localhost:")? + "localhost:".len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from the blog repository's `pnpm dev --port 4321`.
    const LOCAL_LINE: &str =
        "\u{1b}[2m\u{2503}\u{1b}[22m Local    \u{1b}[36mhttp://localhost:4321/\u{1b}[39m";
    const TIMING_LINE: &str = "\u{1b}[42m\u{1b}[1m astro \u{1b}[22m\u{1b}[49m \u{1b}[32mv7.2.8\u{1b}[39m \u{1b}[2mready in\u{1b}[22m 6388 \u{1b}[2mms\u{1b}[22m";

    #[test]
    fn reads_the_port_out_of_the_real_announcement() {
        assert_eq!(ready_port(LOCAL_LINE), Some(4321));
    }

    #[test]
    fn follows_astro_to_another_port() {
        assert_eq!(
            ready_port("\u{1b}[36mhttp://localhost:4400/\u{1b}[39m"),
            Some(4400)
        );
    }

    #[test]
    fn ignores_lines_without_an_address() {
        // The banner's other lines, and the "Network" hint, carry no port.
        assert_eq!(ready_port(TIMING_LINE), None);
        assert_eq!(ready_port("┃ Network  use --host to expose"), None);
        assert_eq!(ready_port("23:21:19 [vite] connected."), None);
        // "localhost:" with nothing after it must not parse as a port.
        assert_eq!(ready_port("localhost: is not a port"), None);
    }
}
