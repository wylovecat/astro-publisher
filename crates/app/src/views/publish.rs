//! Publish view: describes the pipeline and reports the current preflight state.
//!
//! The pipeline itself (validate → build → commit → push → EdgeOne auto-build)
//! is implemented once in the core library and driven by both this view and the
//! CLI; see `astro_publisher_core::process` and the CLI's `publish` command.

use crate::state::Config;
use astro_publisher_core::validate;
use gpui::prelude::*;
use gpui::{Context, IntoElement, div, px};
use gpui_component::{ActiveTheme, label::Label};

/// Render the publish panel.
pub fn render(config: &Config, cx: &Context<crate::PublisherApp>) -> impl IntoElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;

    let report = validate::validate_all(&config.blog());
    let has_errors = report.has_errors();

    let steps = [
        ("1. 校验内容", "frontmatter 与大纲链接".to_owned(), !has_errors),
        (
            "2. 本地构建",
            if config.run_local_build {
                "pnpm build（权威校验）".to_owned()
            } else {
                "已在设置中关闭".to_owned()
            },
            !has_errors,
        ),
        ("3. 提交", "git add + commit".to_owned(), !has_errors),
        (
            "4. 推送",
            format!("git push {} {}", config.git_remote, config.git_branch),
            !has_errors,
        ),
        (
            "5. 上线",
            "EdgeOne Pages 自动构建部署".to_owned(),
            !has_errors,
        ),
    ];

    div()
        .flex()
        .flex_col()
        .size_full()
        .p_4()
        .gap_4()
        .overflow_hidden()
        .child(Label::new("发布流水线").text_lg())
        .child(
            Label::new(
                "推送后由 EdgeOne Pages 拉取 main 分支自动构建部署；本地构建用于在推送前拦下 frontmatter 错误。",
            )
            .text_xs()
            .text_color(muted),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .p_3()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(if has_errors { theme.danger } else { theme.border })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            Label::new(if has_errors {
                                format!("预检未通过：{} 个错误", report.error_count())
                            } else {
                                format!(
                                    "预检通过：{} 篇文章，{} 个警告",
                                    report.post_count,
                                    report.warning_count()
                                )
                            })
                            .text_sm(),
                        ),
                )
                .children(report.findings.iter().take(10).map(|finding| {
                    let muted = theme.muted_foreground;
                    let label = match finding.severity {
                        validate::Severity::Error => "错误",
                        validate::Severity::Warning => "警告",
                    };
                    Label::new(format!("[{label}] {} — {}", finding.path, finding.message))
                        .text_xs()
                        .text_color(muted)
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap_2()
                .children(steps.into_iter().map(|(name, detail, ok)| {
                    let muted = theme.muted_foreground;
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            Label::new(if ok { "○" } else { "✕" }).text_sm(),
                        )
                        .child(Label::new(name.to_owned()).text_sm())
                        .child(Label::new(detail.to_owned()).text_xs().text_color(muted))
                })),
        )
        .child(
            Label::new("具体执行请使用命令行：astro-publisher-core publish --message \"...\"")
                .text_xs()
                .text_color(muted),
        )
        .child(
            Label::new("（应用内的「一键发布」按钮接在 M1 剩余工作里：需要把流水线的输出接进日志面板。）")
                .text_xs()
                .text_color(muted),
        )
        .child(div().h(px(4.0)))
}
