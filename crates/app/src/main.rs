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

mod state;
mod views;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds, WindowOptions, div, px,
    size,
};
use gpui_component::{ActiveTheme, Root, Sizable, TitleBar, button::{Button, ButtonVariants}, label::Label};
use gpui_platform::application;

use state::{Config, View};

/// The application shell: sidebar navigation plus the active view.
pub struct PublisherApp {
    config: Config,
    view: View,
    posts: Vec<state::PostRow>,
    /// Currently open post, when the editor view is active.
    open_post: Option<Entity<views::editor::EditorView>>,
    /// Chapter wizard, created the first time the view is opened so its inputs
    /// and running state survive switching pages.
    chapter_wizard: Option<Entity<views::chapter::ChapterWizard>>,
    /// Publish panel, kept for the same reason: a run in flight must not be
    /// dropped because the user looked at another page.
    publish_panel: Option<Entity<views::publish::PublishView>>,
    /// Repository-path field on the settings page.
    repo_input: Option<Entity<gpui_component::input::InputState>>,
    notice: Option<String>,
}

impl PublisherApp {
    fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let mut app = Self {
            config,
            view: View::Posts,
            posts: Vec::new(),
            open_post: None,
            chapter_wizard: None,
            publish_panel: None,
            repo_input: None,
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

    fn chapter_entity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<views::chapter::ChapterWizard> {
        if let Some(existing) = &self.chapter_wizard {
            return existing.clone();
        }
        let entity = cx.new(|cx| {
            views::chapter::ChapterWizard::new(self.config.clone(), window, cx)
        });
        self.chapter_wizard = Some(entity.clone());
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
        let entity =
            cx.new(|cx| views::publish::PublishView::new(self.config.clone(), window, cx));
        self.publish_panel = Some(entity.clone());
        entity
    }

    /// Open a post in the editor view.
    pub fn open_post(&mut self, row: &state::PostRow, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.new(|cx| views::editor::EditorView::new(row.clone(), window, cx));
        self.open_post = Some(entity);
        self.view = View::Editor;
        cx.notify();
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
            View::Posts => views::posts::render(&self.posts, window, cx),
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
            View::Outline => views::outline::render(&self.config, cx).into_any_element(),
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
                let app = cx.new(|cx| {
                    let mut app = PublisherApp::new(config.clone(), cx);
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
                cx.new(|cx| Root::new(app, window, cx))
            },
        )
        .expect("failed to open window");

        cx.activate(true);
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
        "posts" => Some(View::Posts),
        "editor" => Some(View::Editor),
        "chapter" => Some(View::Chapter),
        "outline" => Some(View::Outline),
        "publish" => Some(View::Publish),
        "settings" => Some(View::Settings),
        _ => None,
    }
}
