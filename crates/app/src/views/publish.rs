//! Publish view: the one-click pipeline with a live log.
//!
//! The pipeline itself lives in `astro_publisher_core::publish` and is shared
//! with the CLI; this view only drives it and renders the events it emits:
//! per-step status, streamed child-process output, and a stop button.
//!
//! Two details worth knowing before editing:
//!
//! - The run happens on a background task, so events arrive through a channel and
//!   are drained by a foreground task. Blocking the UI thread on `pnpm build`
//!   would freeze the window for the whole build.
//! - The build is streamed through `process::spawn_streaming`, which already
//!   forces UTF-8 on the child; Chinese build output must stay readable.

use crate::state::Config;
use astro_publisher_core::process::Stream;
use astro_publisher_core::publish::{self, PublishError, PublishEvent, StepState, STEP_COUNT, STEP_TITLES};
use gpui::prelude::*;
use gpui::{Context, Entity, IntoElement, Render, Window, div, px};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    label::Label,
    scroll::ScrollableElement,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Keep the log bounded: a full Astro build produces thousands of lines and the
/// view re-renders the whole log on every event.
const MAX_LOG_LINES: usize = 500;

/// The publish panel.
pub struct PublishView {
    config: Config,
    /// Commit message the pipeline will use.
    message: Entity<InputState>,
    /// Latest status per step, in pipeline order.
    steps: Vec<StepRow>,
    /// Streamed output, oldest first.
    log: Vec<(Stream, String)>,
    running: bool,
    /// Set by the stop button; the pipeline checks it between steps and kills a
    /// running build.
    cancel: Arc<AtomicBool>,
    notice: String,
}

/// One step's rendered state.
struct StepRow {
    state: StepState,
    detail: String,
}

impl PublishView {
    pub fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("提交信息，例如：补充 P2003 题解", window, cx);
            state.set_value(
                "docs: 更新博客内容",
                window,
                cx,
            );
            state
        });

        Self {
            config,
            message,
            steps: fresh_steps(),
            log: Vec::new(),
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            notice: String::new(),
        }
    }

    /// Start the pipeline in the background.
    fn start(&mut self, dry_run: bool, cx: &mut Context<Self>) {
        if self.running {
            return;
        }

        let message = self.message.read(cx).value().trim().to_owned();
        if message.is_empty() {
            self.notice = "请先填写提交信息".to_owned();
            cx.notify();
            return;
        }

        if !crate::state::looks_like_repo(&self.config.repo()) {
            self.notice = format!(
                "仓库路径无效：{}（到「设置」里改成博客仓库根目录）",
                self.config.repo().display()
            );
            cx.notify();
            return;
        }

        let options = publish::PublishOptions {
            repo: self.config.repo(),
            message,
            run_build: self.config.run_local_build,
            dry_run,
            git_remote: self.config.git_remote.clone(),
            git_branch: self.config.git_branch.clone(),
        };

        self.steps = fresh_steps();
        self.log.clear();
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.notice = if dry_run {
            "演练中：不提交、不推送".to_owned()
        } else {
            "发布中…".to_owned()
        };
        cx.notify();

        // The pipeline is async (tokio processes), so it runs on a background
        // executor and reports through a channel the UI drains.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PublishEvent>();
        let cancel = self.cancel.clone();

        let worker = cx.background_executor().spawn(async move {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = tx.send(PublishEvent::Log(astro_publisher_core::process::OutputLine {
                        stream: Stream::Stderr,
                        text: format!("无法启动异步运行时：{err}"),
                    }));
                    return;
                }
            };

            let mut emit = |event: PublishEvent| {
                let _ = tx.send(event);
            };
            let result = runtime.block_on(publish::run(&options, cancel, &mut emit));

            if let Err(err) = result {
                for line in describe_failure(&err) {
                    let _ = tx.send(PublishEvent::Log(astro_publisher_core::process::OutputLine {
                        stream: Stream::Stderr,
                        text: line,
                    }));
                }
            }
        });

        // Drain events on the UI thread so the panel updates live.
        cx.spawn(async move |this, cx| {
            worker.await;

            while let Some(event) = rx.recv().await {
                this.update(cx, |this, cx| {
                    this.apply(event, cx);
                })
                .ok();
            }

            this.update(cx, |this, cx| this.finish(cx)).ok();
        })
        .detach();
    }

    /// Ask the pipeline to stop.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.running {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.notice = "正在停止…（若正在构建会结束构建进程）".to_owned();
        cx.notify();
    }

    fn apply(&mut self, event: PublishEvent, cx: &mut Context<Self>) {
        match event {
            PublishEvent::Step {
                index,
                state,
                detail,
                ..
            } => {
                if let Some(row) = self.steps.get_mut(index) {
                    row.state = state;
                    row.detail = detail;
                }
            }
            PublishEvent::Log(line) => {
                self.log.push((line.stream, line.text));
                if self.log.len() > MAX_LOG_LINES {
                    let overflow = self.log.len() - MAX_LOG_LINES;
                    self.log.drain(0..overflow);
                }
            }
            PublishEvent::Finished(summary) => {
                self.notice = if summary.dry_run {
                    "演练完成，仓库未改动".to_owned()
                } else {
                    format!("已推送到 origin/{}，托管平台开始部署", summary.branch)
                };
            }
        }
        cx.notify();
    }

    /// The run ended, successfully or not.
    fn finish(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        // A failed step left its own detail; only fill in a message when the run
        // ended without one (cancellation, for example).
        if self
            .steps
            .iter()
            .all(|row| row.state != StepState::Running)
            && self.notice.starts_with("正在停止")
        {
            self.notice = "已停止".to_owned();
        }
        cx.notify();
    }

    fn controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().flex_1().child(Input::new(&self.message)))
            .child(
                Button::new("publish-run")
                    .label("发布")
                    .small()
                    .primary()
                    .loading(self.running)
                    .on_click(cx.listener(|this, _, _, cx| this.start(false, cx))),
            )
            .child(
                Button::new("publish-dry-run")
                    .label("演练")
                    .small()
                    .secondary()
                    .on_click(cx.listener(|this, _, _, cx| this.start(true, cx))),
            )
            .child(
                Button::new("publish-stop")
                    .label("停止")
                    .small()
                    .danger()
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
            )
            .child(
                Button::new("publish-open-eo")
                    .label("EdgeOne 控制台")
                    .small()
                    .secondary()
                    .on_click({
                        let url = self.config.eo_console_url.clone();
                        move |_, _, cx| cx.open_url(&url)
                    }),
            )
            .into_any_element()
    }

    fn step_list(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        div()
            .flex()
            .flex_col()
            .w(px(320.0))
            .flex_shrink_0()
            .gap_2()
            .children(self.steps.iter().enumerate().map(|(index, row)| {
                let (marker, color) = match row.state {
                    StepState::Pending => ("○", muted),
                    StepState::Running => ("◐", theme.primary),
                    StepState::Ok => ("✓", theme.success),
                    StepState::Skipped => ("–", muted),
                    StepState::Failed => ("✕", theme.danger),
                };
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
                            .child(Label::new(format!("{marker} {}. {}", index + 1, STEP_TITLES[index])).text_sm())
                            .child(Label::new(row.state.label()).text_xs().text_color(color)),
                    )
                    .child(Label::new(row.detail.clone()).text_xs().text_color(muted))
            }))
            .into_any_element()
    }

    fn log_panel(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        let body: gpui::AnyElement = if self.log.is_empty() {
            Label::new("运行后这里会显示校验、构建与 git 的实时输出。")
                .text_xs()
                .text_color(muted)
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .children(self.log.iter().map(|(stream, text)| {
                    let color = if *stream == Stream::Stderr {
                        theme.danger
                    } else {
                        theme.foreground
                    };
                    Label::new(text.clone()).text_xs().text_color(color)
                }))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .gap_2()
            .child(Label::new("日志").text_sm())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_scrollbar()
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for PublishView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out first: the sub-panels take `&mut Context` to build
        // their click handlers, and holding a theme borrow across them would
        // alias it.
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let _ = theme;

        let report = astro_publisher_core::validate::validate_all(&self.config.blog());
        let has_errors = report.has_errors();
        let preflight = if has_errors {
            format!(
                "预检：{} 个错误，发布会被拦下（到「大纲」或用 CLI check 看详情）",
                report.error_count()
            )
        } else {
            format!(
                "预检：{} 篇文章，{} 个警告{}",
                report.post_count,
                report.warning_count(),
                if self.config.run_local_build {
                    "，将执行 pnpm build"
                } else {
                    "，已关闭本地构建"
                }
            )
        };
        let notice = self.notice.clone();
        let controls = self.controls(cx);

        // These two only read `self` through `&Context`, so they must come after
        // `controls`, which needs `&mut`.
        let steps = self.step_list(cx);
        let log = self.log_panel(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .p_4()
            .gap_3()
            .overflow_hidden()
            .child(Label::new("发布流水线").text_lg())
            .child(
                Label::new("校验 → 本地构建 → git commit → git push → 托管平台自动构建部署；推送即发布。")
                    .text_xs()
                    .text_color(muted),
            )
            .child(controls)
            .child(
                Label::new(preflight)
                    .text_xs()
                    .text_color(if has_errors { danger } else { muted }),
            )
            .child(Label::new(notice).text_xs().text_color(muted))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .gap_4()
                    .child(steps)
                    .child(log),
            )
    }
}

fn fresh_steps() -> Vec<StepRow> {
    (0..STEP_COUNT)
        .map(|_| StepRow {
            state: StepState::Pending,
            detail: String::new(),
        })
        .collect()
}

/// Turn a failure into log lines, including the build tail the UI does not show.
fn describe_failure(err: &PublishError) -> Vec<String> {
    match err {
        PublishError::Build { tail } => tail
            .lines()
            .map(|line| line.to_owned())
            .collect(),
        PublishError::Validation { findings } => findings
            .iter()
            .map(|finding| format!("[错误] {} — {}", finding.path, finding.message))
            .collect(),
        other => vec![format!("✗ {other}")],
    }
}
