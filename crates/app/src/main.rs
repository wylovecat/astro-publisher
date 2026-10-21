// The desktop app is a GUI binary on Windows. Without this the linker marks the
// executable as a console program and the loader attaches a console, so every
// launch shows a black window behind (or in front of) the app. Kept conditional
// on `debug_assertions` so `cargo run` still prints panics and logs to the
// terminal while developing.
//
// Hiding our own console is only half the fix: a console-subsystem child spawned
// by a GUI-subsystem parent has no console to inherit and gets a brand new one,
// so the publish pipeline's `git` / `pnpm` calls would each flash a window. Those
// run with `CREATE_NO_WINDOW` - see `hide_console` in the core crate.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop publisher for the Astro blog.
//!
//! The binary is a thin shell over `astro-publisher-core`: all content logic
//! (outline parsing, chapter creation, validation, preview rendering) lives in
//! the core crate so it stays testable and reusable by the CLI and, later, an
//! MCP server.
//!
//! Entry point pattern follows the GPUI Kit "executable application recipe":
//! `application().run(...)` → `gpui_component::init(cx)` → `open_window` with a
//! `Root` at the top level.

mod live_preview;
mod single_instance;
mod state;
mod views;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, IntoElement, Render, WeakEntity, Window, WindowBounds,
    WindowOptions, div, px, size,
};
use gpui_component::{ActiveTheme, Root, Sizable, TitleBar, WindowExt, button::{Button, ButtonVariants}, input::InputState, label::Label};
use gpui_platform::application;

use state::{Config, SessionFiles, View};

/// The application shell: sidebar navigation plus the active view.
pub struct PublisherApp {
    config: Config,
    /// Files this session wrote, shared with every view that writes.
    session: SessionFiles,
    view: View,
    posts: Vec<state::PostRow>,
    /// Currently open post, when the editor view is active.
    open_post: Option<Entity<views::editor::EditorView>>,
    /// Set once the user has answered "discard and exit", so the close handler
    /// stops asking.
    force_close: bool,
    /// Chapter wizard, created the first time the view is opened so its inputs
    /// and running state survive switching pages.
    chapter_wizard: Option<Entity<views::chapter::ChapterWizard>>,
    /// Outline page, kept for the same reason: a creation in flight must not be
    /// dropped because the user looked at another page.
    outline_view: Option<views::outline::OutlineEntity>,
    /// Publish panel, kept for the same reason: a run in flight must not be
    /// dropped because the user looked at another page.
    publish_panel: Option<Entity<views::publish::PublishView>>,
    /// The `astro dev` server behind the editor's "real preview". Owned by the
    /// app, not the editor, so switching posts does not restart it.
    live_preview: Option<Entity<live_preview::LivePreview>>,
    /// Repository-path field on the settings page.
    repo_input: Option<Entity<gpui_component::input::InputState>>,
    /// Add-book form on the settings page. Kept for the same reason as the
    /// other cached entities: typing survives a page switch.
    pub book_form: Option<views::settings::BookForm>,
    /// Search field on the post page. Its text filters the list.
    search_input: Entity<InputState>,
    notice: Option<String>,
}

impl PublisherApp {
    fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("搜索标题 / 路径 / 标签", window, cx);
            input
        });
        // Typing only re-renders the input itself, so the list needs an
        // explicit nudge to re-filter. `Context::subscribe` holds the
        // subscriber weakly, so detaching the handle is safe: the callback
        // stops firing once this app entity is dropped.
        // `InputState` implements `EventEmitter` for more than one event type,
        // so the subscription has to say which one it wants.
        cx.subscribe(
            &search_input,
            |_, _, _: &gpui_component::input::InputEvent, cx| cx.notify(),
        )
        .detach();

        let mut app = Self {
            config,
            session: SessionFiles::default(),
            view: View::Posts,
            posts: Vec::new(),
            open_post: None,
            force_close: false,
            chapter_wizard: None,
            outline_view: None,
            publish_panel: None,
            live_preview: None,
            repo_input: None,
            book_form: None,
            search_input,
            notice: None,
        };
        app.refresh_posts(cx);
        app
    }

    /// The settings page's repository-path field, created on first use so the
    /// field exists before the page renders.
    pub fn repo_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<gpui_component::input::InputState> {
        if let Some(existing) = &self.repo_input {
            return existing.clone();
        }
        let initial = self.config.repo_path.clone();
        let entity = cx.new(|cx| {
            let mut state = gpui_component::input::InputState::new(window, cx);
            state.set_placeholder(
                "例如 C:\\path\\to\\your-astro-blog（需含 package.json 与 src/data/blog）",
                window,
                cx,
            );
            state.set_value(initial, window, cx);
            state
        });
        self.repo_input = Some(entity.clone());
        entity
    }

    /// Apply the repository path typed into the settings field.
    pub fn apply_repo_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.repo_input.clone() else {
            return;
        };
        let typed = input.read(cx).value().to_string();
        let trimmed = typed.trim().trim_matches('"').to_owned();

        if trimmed.is_empty() {
            self.set_notice("仓库路径不能为空".to_owned(), cx);
            return;
        }

        let candidate = std::path::PathBuf::from(&trimmed);
        if !state::looks_like_repo(&candidate) {
            self.set_notice(
                format!("这个路径不像博客仓库（需要 package.json 与 src/data/blog）：{trimmed}"),
                cx,
            );
            return;
        }

        self.config.repo_path = trimmed.clone();
        if let Err(err) = self.config.save() {
            self.set_notice(format!("路径有效，但写入配置失败：{err}"), cx);
        } else {
            self.set_notice(format!("已切换到：{trimmed}"), cx);
        }

        // Reload the corpus from the newly selected repository.
        self.refresh_posts(cx);
        let _ = window;
    }

    /// Re-read the post list from disk.
    pub fn refresh_posts(&mut self, cx: &mut Context<Self>) {
        match state::load_posts(&self.config) {
            Ok(posts) => {
                self.notice = Some(format!("已加载 {} 篇文章", posts.len()));
                self.posts = posts;
            }
            Err(err) => {
                self.notice = Some(format!("读取失败：{err}"));
                self.posts.clear();
            }
        }
        cx.notify();
    }

    fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    /// Show a short status message in the header.
    pub fn set_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    /// Current configuration (read-only), for the settings page.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The post page's search field, so the view can render it next to the list.
    pub fn search_input(&self) -> &Entity<InputState> {
        &self.search_input
    }

    fn chapter_entity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<views::chapter::ChapterWizard> {
        if let Some(existing) = &self.chapter_wizard {
            return existing.clone();
        }
        let entity = cx.new(|cx| {
            views::chapter::ChapterWizard::new(
                self.config.clone(),
                self.session.clone(),
                window,
                cx,
            )
        });
        self.chapter_wizard = Some(entity.clone());
        entity
    }

    fn outline_entity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> views::outline::OutlineEntity {
        if let Some(existing) = &self.outline_view {
            return existing.clone();
        }
        let entity = cx.new(|cx| {
            views::outline::OutlineView::new(
                self.config.clone(),
                self.session.clone(),
                window,
                cx,
            )
        });
        // A chapter created from the outline is a new file, so the post list is
        // stale the moment it lands.
        cx.subscribe(
            &entity,
            |this, _, event: &views::outline::OutlineEvent, cx| match event {
                views::outline::OutlineEvent::Created { title_line } => {
                    this.refresh_posts(cx);
                    this.set_notice(format!("已创建 {title_line}，文章列表已刷新"), cx);
                }
            },
        )
        .detach();
        self.outline_view = Some(entity.clone());
        entity
    }

    fn publish_entity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<views::publish::PublishView> {
        if let Some(existing) = &self.publish_panel {
            return existing.clone();
        }
        let entity = cx.new(|cx| {
            views::publish::PublishView::new(
                self.config.clone(),
                self.session.clone(),
                window,
                cx,
            )
        });
        self.publish_panel = Some(entity.clone());
        entity
    }

    fn live_preview_entity(&mut self, cx: &mut Context<Self>) -> Entity<live_preview::LivePreview> {
        if let Some(existing) = &self.live_preview {
            return existing.clone();
        }
        let entity = cx.new(|cx| live_preview::LivePreview::new(self.config.clone(), cx));
        self.live_preview = Some(entity.clone());
        entity
    }

    /// Open a post in the editor view.
    pub fn open_post(&mut self, row: &state::PostRow, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.config.repo();
        let preview = self.live_preview_entity(cx);
        let entity = cx.new(|cx| {
            views::editor::EditorView::new(
                row.clone(),
                repo,
                self.session.clone(),
                preview,
                window,
                cx,
            )
        });
        self.open_post = Some(entity);
        self.view = View::Editor;
        cx.notify();
    }

    /// The title of the post with unsaved edits, if there is one.
    ///
    /// Read by the window-close handler, which is not an entity update and so
    /// cannot hold a borrow across it.
    pub fn unsaved_post(&self, cx: &App) -> Option<String> {
        let entity = self.open_post.as_ref()?;
        let dirty = entity.read(cx).is_dirty();
        dirty.then(|| entity.read(cx).title().to_owned())
    }

    /// Let the next close through without asking (plan §4.2).
    pub fn allow_close(&mut self) {
        self.force_close = true;
    }

    /// Whether closing is already allowed.
    pub fn may_close(&self) -> bool {
        self.force_close
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let accent = theme.accent;

        div()
            .flex()
            .flex_col()
            .w(px(190.0))
            .h_full()
            .p_3()
            .gap_1()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .px_2()
                    .py_3()
                    .child(Label::new("astro-publisher").text_base())
                    .child(
                        Label::new("博客发布工具")
                            .text_xs()
                            .text_color(muted),
                    ),
            )
            .children(View::all().into_iter().map(|view| {
                let active = self.view == view;
                div()
                    .id(("nav", view as usize))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |el| el.bg(accent))
                    .text_color(if active { theme.accent_foreground } else { theme.foreground })
                    .child(Label::new(view.title()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_view(view, cx);
                    }))
            }))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(Label::new(self.view.title()).text_lg())
                    .child(
                        Label::new(self.config.repo_path.clone())
                            .text_xs()
                            .text_color(muted),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .children(
                        self.notice
                            .clone()
                            .map(|text| Label::new(text).text_xs().text_color(muted)),
                    )
                    .child(
                        Button::new("refresh")
                            .label("重新加载")
                            .small()
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_posts(cx);
                            })),
                    ),
            )
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.view {
            View::Dashboard => {
                // The recent-posts list opens posts, so this view needs `self`
                // and the window, like the settings page.
                views::dashboard::render(self, window, cx).into_any_element()
            }
            View::Posts => views::posts::render(
                &self.posts,
                self.search_input().clone(),
                window,
                cx,
            ),
            View::Editor => match &self.open_post {
                Some(entity) => div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(entity.clone())
                    .into_any_element(),
                None => views::placeholder(
                    "还没有打开文章",
                    "到「文章」里选一篇开始编辑，或新建一篇。",
                    cx,
                )
                .into_any_element(),
            },
            View::Chapter => div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.chapter_entity(window, cx))
                .into_any_element(),
            View::Outline => div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.outline_entity(window, cx))
                .into_any_element(),
            View::Publish => div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.publish_entity(window, cx))
                .into_any_element(),
            View::Settings => {
                // The settings page owns an editable repository-path field, so it
                // needs the window (to create the input state) as well as self.
                views::settings::render(self, window, cx).into_any_element()
            }
        }
    }
}

impl Render for PublisherApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(self.header(cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .overflow_hidden()
                                    .child(self.body(window, cx)),
                            ),
                    ),
            )
    }
}

fn main() {
    // Verification helpers: `--open <n>` jumps into the editor for the n-th
    // post, `--view <name>` opens a named page directly. Both make the UI
    // reachable without driving it by hand.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let open_index = parse_open_index(&args);
    let start_view = parse_view(&args);

    // One instance only (plan §4.2). Two of them would fight over the dev
    // server port, over the publish pipeline and over the config file. The lock
    // outlives a crash on purpose: a lock whose owner is dead is taken over, so
    // a killed app never locks the user out of their own tool.
    let (_claim, _lock) = match single_instance::claim() {
        Ok(claimed) => claimed,
        Err(message) => {
            eprintln!("✗ {message}");
            std::process::exit(1);
        }
    };
    if let single_instance::Claim::InUse { pid } = _claim {
        single_instance::notify_already_running(pid);
        std::process::exit(0);
    }
    if let single_instance::Claim::Acquired {
        displaced: Some(pid),
    } = _claim
    {
        println!("上一次运行的进程 {pid} 异常退出，已接管实例锁");
    }

    // Children die with the app, however it dies (plan §4.2): the window-close
    // hook only runs on a clean close, so the `astro dev` server used to survive
    // a task-manager kill and keep port 4321.
    astro_publisher_core::job::install();

    // The bundled SVG icons (including the window control glyphs in the title
    // bar) have to be registered on the application itself: `gpui_component::init`
    // does not do it, and unregistered icons render as empty space rather than
    // failing, so the title bar shows up with blank buttons.
    application()
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
        gpui_component::init(cx);

        let config = Config::load();
        let bounds = Bounds::centered(None, size(px(1280.0), px(860.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                ..Default::default()
            },
            move |window, cx| {
                // Nothing sets these by default on X11: the window appears with
                // an empty WM_NAME / WM_CLASS, so the taskbar entry reads
                // "N/A N/A", Alt-Tab shows no label, and the desktop entry's
                // StartupWMClass cannot match it. The in-app title bar is drawn
                // by gpui-component and is unaffected.
                window.set_window_title("astro-publisher");
                window.set_app_id("astro-publisher");

                let app = cx.new(|cx| {
                    let mut app = PublisherApp::new(config.clone(), window, cx);
                    if let Some(view) = start_view {
                        app.set_view(view, cx);
                    }
                    if let Some(index) = open_index {
                        if let Some(row) = app.posts.get(index).cloned() {
                            app.open_post(&row, window, cx);
                        }
                    }
                    app
                });

                // Ask before throwing away unsaved edits (plan §4.2). Returning
                // false vetoes the close, so the answer is never lost.
                //
                // The handle is a weak clone rather than the entity itself:
                // `on_window_should_close` takes an `Fn`, so the callback cannot
                // hand ownership anywhere, and `Root` below still needs the
                // entity to render.
                let closer = app.clone();
                let closer_weak = app.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    if closer.read(cx).may_close() {
                        return true;
                    }
                    let Some(title) = closer.read(cx).unsaved_post(cx) else {
                        return true;
                    };
                    confirm_close(window, cx, title, closer_weak.clone());
                    false
                });

                cx.new(|cx| Root::new(app, window, cx))
            },
        )
        .expect("failed to open window");

        cx.activate(true);
    });
}

/// The answer to "there are unsaved edits".
///
/// Two buttons, and the destructive one is the affirmative one. A three-way
/// dialog would also offer "save and quit", but reaching that takes two steps
/// anyway (cancel, Ctrl+S, close), while making it a button puts a one-click
/// path to saving an accidental keystroke next to a one-click path to losing
/// work. The description says so, and the safe answer is the default direction:
/// the close is vetoed until the user actually chooses.
fn confirm_close(window: &mut Window, cx: &mut App, title: String, app: WeakEntity<PublisherApp>) {
    window.open_alert_dialog(cx, move |alert, _window, _cx| {
        // Cloned per invocation, not moved: the builder is an `Fn`, so a
        // closure that took the handle by value could only ever run once.
        let accept = app.clone();
        alert
            .title("有未保存的修改")
            .description(format!(
                "《{title}》还有没有保存的修改。\n\n\
                 「放弃并退出」会丢掉它们；要保留就先返回编辑器，按 Ctrl+S 保存后再关窗口。"
            ))
            .confirm()
            .ok_text("放弃并退出")
            .cancel_text("返回编辑器")
            .ok_variant(gpui_component::button::ButtonVariant::Danger)
            .on_ok(move |_, _window, cx| {
                // A dead entity means the app is already tearing down, which is
                // the outcome we wanted anyway.
                let _ = accept.update(cx, |app, _cx| app.allow_close());
                cx.quit();
                true
            })
    });
}

/// Read `--open <n>` from the command line.
fn parse_open_index(args: &[String]) -> Option<usize> {
    let position = args.iter().position(|a| a == "--open")?;
    args.get(position + 1)?.parse().ok()
}

/// Read `--view <posts|editor|outline|publish|settings>` from the command line.
fn parse_view(args: &[String]) -> Option<View> {
    let position = args.iter().position(|a| a == "--view")?;
    match args.get(position + 1)?.to_ascii_lowercase().as_str() {
        "dashboard" => Some(View::Dashboard),
        "posts" => Some(View::Posts),
        "editor" => Some(View::Editor),
        "chapter" => Some(View::Chapter),
        "outline" => Some(View::Outline),
        "publish" => Some(View::Publish),
        "settings" => Some(View::Settings),
        _ => None,
    }
}
