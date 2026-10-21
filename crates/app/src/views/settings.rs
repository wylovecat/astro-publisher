//! Settings view: repository path, author, git target, and publish options.
//!
//! The repository path is *editable* here on purpose: an installed copy lives
//! under the user's program directory, where walking up the filesystem can never
//! find the blog. Without this field the installed app would be unusable.
//!
//! The settings file itself lives in the platform config directory, never in the
//! repository.

use crate::PublisherApp;
use astro_publisher_core::{books, outline::Outline};
use gpui::prelude::*;
use gpui::{Context, Entity, IntoElement, Window, div, px};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    label::Label,
    scroll::ScrollableElement,
};

/// Inputs for the "add a book" form, kept on the app so they survive switching
/// pages. The fields live here rather than in a separate view because the form
/// is a single card inside Settings.
#[derive(Clone)]
pub struct BookForm {
    pub id: Entity<InputState>,
    pub title: Entity<InputState>,
    pub subtitle: Entity<InputState>,
    pub description: Entity<InputState>,
    pub index: Entity<InputState>,
    pub author: Entity<InputState>,
    pub status: Option<BookStatus>,
}

#[derive(Clone)]
pub enum BookStatus {
    Ok(String),
    Err(String),
}

impl PublisherApp {
    /// The add-book form, created on first use (the Settings page is the only
    /// consumer).
    pub fn book_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> BookForm {
        if let Some(existing) = &self.book_form {
            return existing.clone();
        }

        let author_default = self.config.author.clone();
        let id = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("例如 graph", window, cx);
            state
        });
        let title = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("《算法竞赛进阶指南》", window, cx);
            state
        });
        let subtitle = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("可选，例如 题解与学习笔记", window, cx);
            state
        });
        let description = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("留空则使用书名", window, cx);
            state
        });
        let index = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("留空则 <id>/<id>-index", window, cx);
            state
        });
        let author = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(author_default, window, cx);
            state
        });

        let form = BookForm {
            id,
            title,
            subtitle,
            description,
            index,
            author,
            status: None,
        };
        self.book_form = Some(form.clone());
        form
    }

    /// Create the book from the form, then make every book consumer reload.
    ///
    /// Synchronous on purpose: this writes two small files. The chapter wizard
    /// and outline view cache their book lists, so they are refreshed here
    /// rather than at the next launch.
    pub fn submit_book(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.book_form.clone() else {
            return;
        };
        let read = |input: &Entity<InputState>, cx: &Context<Self>| {
            input.read(cx).value().trim().to_owned()
        };

        let id = read(&form.id, cx);
        let title = read(&form.title, cx);
        let subtitle = read(&form.subtitle, cx);
        let description = read(&form.description, cx);
        let index = read(&form.index, cx);
        let author = read(&form.author, cx);

        let status = if id.is_empty() || title.is_empty() {
            BookStatus::Err("书籍 id 与书名不能为空".to_owned())
        } else {
            let input = books::NewBook {
                id: id.clone(),
                title: title.clone(),
                subtitle: (!subtitle.is_empty()).then_some(subtitle),
                description: if description.is_empty() {
                    title
                } else {
                    description
                },
                index_id: if index.is_empty() {
                    format!("{id}/{id}-index")
                } else {
                    index
                },
                cover_class: None,
                author: if author.is_empty() {
                    self.config.author.clone()
                } else {
                    author
                },
                create_outline: true,
            };

            match books::create_book(&self.config.repo(), &input) {
                Ok(outcome) => {
                    self.refresh_posts(cx);
                    self.reload_book_views(cx);
                    BookStatus::Ok(format!(
                        "已创建「{}」 · {}；大纲已生成，可在「新建章节」里选到",
                        outcome.book.title, outcome.book.id
                    ))
                }
                Err(err) => BookStatus::Err(err.to_string()),
            }
        };

        if let Some(stored) = self.book_form.as_mut() {
            stored.status = Some(status);
        }
        cx.notify();
    }

    /// Re-read the book registry in views that cache it. The dashboard collects
    /// on every render and the post list is refreshed by the caller.
    fn reload_book_views(&mut self, cx: &mut Context<Self>) {
        if let Some(wizard) = self.chapter_wizard.clone() {
            let _ = wizard.update(cx, |wizard, cx| wizard.reload_books(cx));
        }
        if let Some(outline) = self.outline_view.clone() {
            let _ = outline.update(cx, |outline, cx| outline.reload_books(cx));
        }
    }
}

/// Render the settings panel.
pub fn render(
    app: &mut PublisherApp,
    window: &mut Window,
    cx: &mut Context<PublisherApp>,
) -> impl IntoElement {
    // Copy theme colours out before touching `app` mutably: creating the input
    // entity needs `&mut`, which would otherwise alias the theme borrow.
    let border_color = cx.theme().border;
    let danger_color = cx.theme().danger;
    let muted = cx.theme().muted_foreground;

    let config = app.config().clone();
    let config_path = crate::state::Config::path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(未知)".to_owned());

    let repo_ok = crate::state::looks_like_repo(&config.repo());

    // The editable repository path and the add-book form, created on first
    // render of this page.
    let repo_input = app.repo_input(window, cx);
    let book_form = app.book_form(window, cx);

    let details: [(&str, String); 5] = [
        ("内容目录", config.blog().display().to_string()),
        ("默认作者", config.author.clone()),
        (
            "Git 远端 / 分支",
            format!("{} / {}", config.git_remote, config.git_branch),
        ),
        (
            "发布前本地构建",
            if config.run_local_build {
                "开启".to_owned()
            } else {
                "关闭".to_owned()
            },
        ),
        ("EdgeOne 控制台", config.eo_console_url.clone()),
    ];

    div()
        .flex()
        .flex_col()
        .size_full()
        .p_4()
        .gap_3()
        .overflow_y_scrollbar()
        .child(Label::new("设置").text_lg())
        // --- Repository path -------------------------------------------------
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(if repo_ok { border_color } else { danger_color })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(Label::new("博客仓库根目录").text_sm())
                        .child(
                            Label::new(if repo_ok {
                                "✓ 有效"
                            } else {
                                "✕ 无效：需要含 package.json 与 src/data/blog"
                            })
                            .text_xs()
                            .text_color(if repo_ok { muted } else { danger_color }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w_full()
                        .child(div().flex_1().child(Input::new(&repo_input)))
                        .child(
                            Button::new("apply-repo")
                                .label("应用并保存")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_repo_path(window, cx);
                                })),
                        ),
                )
                .child(
                    Label::new(format!("配置文件：{config_path}"))
                        .text_xs()
                        .text_color(muted),
                )
                .child(
                    Label::new(
                        "也可以设环境变量 ASTRO_PUBLISHER_REPO 覆盖（多机同步时用）。\
                         改完点「应用并保存」会写入配置并重新加载文章列表。",
                    )
                    .text_xs()
                    .text_color(muted),
                ),
        )
        // --- Books -----------------------------------------------------------
        .child(books_panel(&config, &book_form, cx))
        // --- Other settings (read-only for now) ------------------------------
        .children(details.into_iter().map(move |(name, value)| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(border_color)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(140.0))
                        .child(Label::new(name.to_owned()).text_sm()),
                )
                .child(Label::new(value).text_xs().text_color(muted))
        }))
}

/// The book registry: what is registered, and a form that adds one.
///
/// A book is two things on disk — an entry in `books.json` and an outline post
/// that `indexId` points at. The form writes both; registering an existing
/// outline is the CLI's `--no-outline` path.
fn books_panel(
    config: &crate::state::Config,
    form: &BookForm,
    cx: &mut Context<PublisherApp>,
) -> impl IntoElement {
    let theme = cx.theme();
    let border = theme.border;
    let muted = theme.muted_foreground;
    let danger = theme.danger;
    let success = theme.success;

    let registered = books::list_books(&config.repo());
    let rows: Vec<gpui::AnyElement> = match &registered {
        Err(err) => vec![Label::new(format!("读取 books.json 失败：{err}"))
            .text_xs()
            .text_color(danger)
            .into_any_element()],
        Ok(list) if list.is_empty() => vec![Label::new("还没有登记任何书籍")
            .text_xs()
            .text_color(muted)
            .into_any_element()],
        Ok(list) => list
            .iter()
            .map(|book| {
                let outline = config.blog().join(book.index_path());
                let (state, state_color) = match Outline::read(&outline) {
                    Ok(outline) => (
                        format!(
                            "{} · {} 个小节 · {} 道题",
                            book.index_path(),
                            outline.sections.len(),
                            outline.entries.len()
                        ),
                        muted,
                    ),
                    Err(err) => (format!("大纲读取失败：{err}"), danger),
                };
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(Label::new(book.title.clone()).text_sm())
                    .child(
                        Label::new(format!("{} · {}", book.id, state))
                            .text_xs()
                            .text_color(state_color),
                    )
                    .into_any_element()
            })
            .collect(),
    };

    let field = |label: &str, input: &Entity<InputState>, hint: &str| -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .child(Label::new(label.to_owned()).text_xs())
            .child(Input::new(input))
            .when(!hint.is_empty(), |el| {
                el.child(Label::new(hint.to_owned()).text_xs().text_color(muted))
            })
    };

    div()
        .flex()
        .flex_col()
        .w_full()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(Label::new("书籍").text_sm())
                .child(
                    Button::new("reload-books")
                        .label("刷新列表")
                        .small()
                        .secondary()
                        .on_click(cx.listener(|_, _, _, cx| cx.notify())),
                ),
        )
        .child(
            Label::new(
                "登记在 src/data/books.json。创建时会同时生成大纲目录页（含「## 第一章」），\
                 建完即可在「新建章节」里选到这本书。",
            )
            .text_xs()
            .text_color(muted),
        )
        .children(rows)
        .child(div().h(px(1.0)).w_full().bg(border))
        .child(Label::new("新增书籍").text_xs().text_color(muted))
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .w_full()
                .child(div().flex_1().child(field(
                    "书籍 id",
                    &form.id,
                    "只允许字母 / 数字 / - / _，站内地址为 /books/<id>/",
                )))
                .child(div().flex_1().child(field(
                    "大纲 id",
                    &form.index,
                    "留空则 <id>/<id>-index；必须且只能含一个 /",
                ))),
        )
        .child(field("书名", &form.title, ""))
        .child(field("副标题", &form.subtitle, "可选"))
        .child(field("简介", &form.description, "留空则使用书名"))
        .child(field(
            "大纲作者",
            &form.author,
            "写进生成的目录页 frontmatter",
        ))
        .child(
            Button::new("create-book")
                .label("创建书籍")
                .small()
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.submit_book(cx))),
        )
        .children(form.status.clone().map(|status| {
            let (text, color) = match status {
                BookStatus::Ok(text) => (text, success),
                BookStatus::Err(text) => (text, danger),
            };
            Label::new(text).text_xs().text_color(color)
        }))
}
