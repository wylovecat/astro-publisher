//! New-chapter wizard: the form that drives `chapters::create_chapter`.
//!
//! The core operation already exists and is tested; what was missing was the form
//! that collects its inputs and shows what it will do. The wizard therefore does
//! the least interesting work possible itself: read the book registry, offer the
//! parsed sections, ask for the problem, then call `preview_chapter` so the user
//! sees the exact slug and outline line *before* anything is written.
//!
//! Creation runs on a background task: it walks the corpus to allocate the
//! site-wide `_0xNNNN` sequence, which is slow enough to stutter the UI.

use crate::state::Config;
use astro_publisher_core::books::{self, Book};
use astro_publisher_core::chapters::{self, ChapterOutcome, ChapterPreview, NewChapter};
use astro_publisher_core::outline::Outline;
use gpui::prelude::*;
use gpui::{App, Context, Entity, IntoElement, Render, Window, div, px};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputState},
    label::Label,
    scroll::ScrollableElement,
};
use std::sync::mpsc;

/// A book as the wizard offers it, with its parsed sections.
struct BookChoice {
    book: Book,
    sections: Vec<String>,
    /// Set when the outline could not be read.
    error: Option<String>,
}

/// What the wizard is currently doing.
enum Stage {
    Editing,
    Creating,
    Created(Box<ChapterOutcome>),
    Failed(String),
}

pub struct ChapterWizard {
    config: Config,
    books: Vec<BookChoice>,
    /// Index into `books`, or `None` before the first selection.
    selected: Option<usize>,
    /// Index into the selected book's sections.
    section_index: Option<usize>,
    problem: Entity<InputState>,
    title: Entity<InputState>,
    video: Entity<InputState>,
    /// `[x]` tracks the solution link (lgj semantics); `lgs` tracks videos.
    check_implies_solution: bool,
    preview: Option<ChapterPreview>,
    stage: Stage,
}

impl ChapterWizard {
    pub fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let problem = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("题号，例如 P1001", window, cx);
            state
        });
        let title = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("题目名，例如 【深基1.例2】简单的分苹果", window, cx);
            state
        });
        let video = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("视频链接（可留空）", window, cx);
            state
        });

        let wizard = Self {
            books: load_books(&config),
            config,
            selected: None,
            section_index: None,
            problem,
            title,
            video,
            check_implies_solution: true,
            preview: None,
            stage: Stage::Editing,
        };
        wizard
    }

    /// Current field values.
    fn input(&self, cx: &App) -> NewChapter {
        let problem = self.problem.read(cx).value().trim().to_owned();
        let title = self.title.read(cx).value().trim().to_owned();
        let video_url = self.video.read(cx).value().trim().to_owned();

        NewChapter {
            problem_id: problem,
            title,
            section: self.section_text().unwrap_or_default(),
            video_url: (!video_url.is_empty()).then_some(video_url),
            author: self.config.author.clone(),
            check_implies_solution: self.check_implies_solution,
        }
    }

    fn section_text(&self) -> Option<String> {
        let choice = self.books.get(self.selected?)?;
        choice
            .sections
            .get(self.section_index?)
            .cloned()
    }

    /// Why the wizard cannot create a chapter right now, if it cannot.
    fn blocking_reason(&self, cx: &App) -> Option<String> {
        let input = self.input(cx);
        if input.problem_id.is_empty() {
            return Some("请填写题号".to_owned());
        }
        if input.title.is_empty() {
            return Some("请填写题目名".to_owned());
        }
        if self.section_text().is_none() {
            return Some("请选择小节".to_owned());
        }
        None
    }

    /// Recompute the preview after a field changed.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = match (self.selected, self.blocking_reason(cx)) {
            (Some(index), None) => {
                let choice = &self.books[index];
                chapters::preview_chapter(&self.config.blog(), &choice.book.as_book_ref(), &self.input(cx))
                    .ok()
            }
            _ => None,
        };
        cx.notify();
    }

    fn select_book(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(index);
        self.section_index = self
            .books
            .get(index)
            .and_then(|choice| (!choice.sections.is_empty()).then_some(0));
        self.refresh_preview(cx);
        let _ = window;
    }

    fn select_section(&mut self, index: usize, cx: &mut Context<Self>) {
        self.section_index = Some(index);
        self.refresh_preview(cx);
    }

    /// Create the chapter, then report what was written.
    fn create(&mut self, cx: &mut Context<Self>) {
        if let Some(reason) = self.blocking_reason(cx) {
            self.stage = Stage::Failed(reason);
            cx.notify();
            return;
        }

        let Some(index) = self.selected else {
            return;
        };
        let Some(choice) = self.books.get(index) else {
            return;
        };
        let book_ref = choice.book.as_book_ref();
        let input = self.input(cx);
        let blog = self.config.blog();

        self.stage = Stage::Creating;
        cx.notify();

        let (tx, rx) = mpsc::channel::<Result<ChapterOutcome, String>>();
        let result = cx.background_executor().spawn(async move {
            let outcome = chapters::create_chapter(&blog, &book_ref, &input)
                .map_err(|err| err.to_string());
            let _ = tx.send(outcome);
        });

        cx.spawn(async move |this, cx| {
            result.await;
            let outcome = rx.recv().ok();
            this.update(cx, |this, cx| {
                this.stage = match outcome {
                    Some(Ok(outcome)) => {
                        this.preview = None;
                        Stage::Created(Box::new(outcome))
                    }
                    Some(Err(message)) => Stage::Failed(message),
                    None => Stage::Failed("创建过程没有返回结果".to_owned()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn field(&self, label: &str, input: &Entity<InputState>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .child(Label::new(label.to_owned()).text_xs())
            .child(Input::new(input))
    }

    fn book_picker(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        div()
            .flex()
            .flex_col()
            .w(px(280.0))
            .flex_shrink_0()
            .gap_2()
            .child(Label::new("选择书籍").text_sm())
            .children(self.books.iter().enumerate().map(|(index, choice)| {
                let active = self.selected == Some(index);
                div()
                    .id(("book", index))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |el| el.bg(theme.accent))
                    .text_color(if active { theme.accent_foreground } else { theme.foreground })
                    .child(Label::new(choice.book.title.clone()).text_sm())
                    .child(
                        Label::new(format!(
                            "{} · {} 个小节",
                            choice.book.index_path(),
                            choice.sections.len()
                        ))
                        .text_xs()
                        .text_color(if active { theme.accent_foreground } else { muted }),
                    )
                    .children(choice.error.clone().map(|err| {
                        Label::new(format!("读取失败：{err}"))
                            .text_xs()
                            .text_color(theme.danger)
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_book(index, window, cx);
                    }))
            }))
    }

    fn section_picker(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        div()
            .flex()
            .flex_col()
            .w(px(240.0))
            .flex_shrink_0()
            .gap_2()
            .child(Label::new("选择小节").text_sm())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_scrollbar()
                    .children(
                        self.books
                            .get(self.selected.unwrap_or(usize::MAX))
                            .map(|choice| choice.sections.clone())
                            .unwrap_or_default()
                            .into_iter()
                            .enumerate()
                            .map(|(index, text)| {
                                let active = self.section_index == Some(index);
                                div()
                                    .id(("section", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .when(active, |el| el.bg(theme.accent))
                                    .text_color(if active {
                                        theme.accent_foreground
                                    } else {
                                        theme.foreground
                                    })
                                    .child(Label::new(text).text_xs())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_section(index, cx);
                                    }))
                            }),
                    ),
            )
            .when(
                self.selected.is_none(),
                |el| el.child(Label::new("先选一本书").text_xs().text_color(muted)),
            )
    }

    fn preview_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        let mut rows: Vec<(String, String)> = Vec::new();
        if let Some(preview) = &self.preview {
            rows.push(("将分配 slug".to_owned(), preview.slug.clone()));
            rows.push(("章节标题".to_owned(), preview.title_line.clone()));
            rows.push(("章节文件".to_owned(), preview.chapter_path.display().to_string()));
            rows.push(("大纲文件".to_owned(), preview.outline_path.display().to_string()));
            rows.push((
                if preview.inserted_entry {
                    "大纲将新增一行".to_owned()
                } else {
                    "大纲将改写这一行".to_owned()
                },
                preview.outline_after.clone(),
            ));
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(Label::new("将执行的操作").text_sm())
            .when(rows.is_empty(), |el| {
                el.child(
                    Label::new("填完题号、题目名并选择小节后，这里会显示确切的 slug 与大纲行。")
                        .text_xs()
                        .text_color(muted),
                )
            })
            .children(rows.into_iter().map(|(name, value)| {
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(div().w(px(140.0)).flex_shrink_0().child(
                        Label::new(name).text_xs().text_color(muted),
                    ))
                    .child(div().flex_1().min_w(px(0.0)).child(
                        Label::new(value).text_xs(),
                    ))
            }))
    }

    fn outcome(&self) -> Option<gpui::AnyElement> {
        match &self.stage {
            Stage::Created(outcome) => Some(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .child(Label::new(format!("✓ 已创建 {}", outcome.title_line)).text_sm())
                    .child(
                        Label::new(format!(
                            "{} · slug {}",
                            outcome.chapter_path.display(),
                            outcome.slug
                        ))
                        .text_xs(),
                    )
                    .child(
                        Label::new(if outcome.inserted_entry {
                            "大纲中新增了一行。".to_owned()
                        } else {
                            "大纲中已有的那一行已更新。".to_owned()
                        })
                        .text_xs(),
                    )
                    .into_any_element(),
            ),
            Stage::Failed(message) => Some(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .child(Label::new(format!("✗ {message}")).text_sm())
                    .into_any_element(),
            ),
            _ => None,
        }
    }
}

impl Render for ChapterWizard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let busy = matches!(self.stage, Stage::Creating);

        // Without a readable registry there is nothing to pick from.
        if self.books.is_empty() {
            return empty_registry(cx, &self.config);
        }

        // Copy what the helpers need before taking `&mut` for the listener-built
        // elements (theme borrows would otherwise alias).
        let books = self.book_picker(cx);
        let sections = self.section_picker(cx);
        let preview = self.preview_panel(cx);
        let outcome = self.outcome();

        div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .gap_4()
            .child(books)
            .child(sections)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap_3()
                    .overflow_scrollbar()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .gap_2()
                            .child(self.field("题号", &self.problem))
                            .child(self.field("题目名", &self.title))
                            .child(self.field("视频链接（可留空）", &self.video))
                            .child(
                                Checkbox::new("check-implies-solution")
                                    .label("题解写完后勾选 [x]（进阶篇用视频标记进度，可关掉）")
                                    .checked(self.check_implies_solution)
                                    .on_click(cx.listener(|this, checked, _, cx| {
                                        this.check_implies_solution = *checked;
                                        this.refresh_preview(cx);
                                    })),
                            ),
                    )
                    .child(preview)
                    .when(outcome.is_some(), |el| el.child(outcome.unwrap()))
                    .child(
                        Label::new(if busy {
                            "创建中…".to_owned()
                        } else {
                            String::new()
                        })
                        .text_xs()
                        .text_color(muted),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("chapter-create")
                                    .label("创建章节")
                                    .small()
                                    .primary()
                                    .loading(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                            )
                            .child(
                                Label::new("创建会同时写入章节文件与大纲行，失败会整体回滚。")
                                    .text_xs()
                                    .text_color(muted),
                            ),
                    ),
            )
    }
}

/// Shown when `books.json` is missing or unreadable: without it the wizard has
/// nothing to offer, so say where it looked.
fn empty_registry(cx: &Context<ChapterWizard>, config: &Config) -> gpui::Div {
    let muted = cx.theme().muted_foreground;
    div()
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(Label::new("没有读到书籍登记表（src/data/books.json）").text_sm())
        .child(
            Label::new(config.blog().display().to_string())
                .text_xs()
                .text_color(muted),
        )
}

/// Books with their parsed section headings, for the two pickers.
fn load_books(config: &Config) -> Vec<BookChoice> {
    let Ok(list) = books::list_books(&config.repo()) else {
        return Vec::new();
    };

    list.into_iter()
        .map(|book| {
            let path = config.blog().join(book.index_path());
            match Outline::read(&path) {
                Ok(outline) => BookChoice {
                    book,
                    sections: outline
                        .sections
                        .iter()
                        .map(|section| section.text.clone())
                        .collect(),
                    error: None,
                },
                Err(err) => BookChoice {
                    book,
                    sections: Vec::new(),
                    error: Some(err.to_string()),
                },
            }
        })
        .collect()
}


