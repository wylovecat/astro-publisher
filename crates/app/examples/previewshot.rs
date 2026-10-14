//! Renders the editor's preview pane on its own, for eyeballing a change.
//!
//! Run with `cargo run -p astro-publisher --example previewshot`. The renderer
//! is included by path rather than depended on: it lives in a binary-only
//! crate, which an example cannot import from.
//!
//! Formula layout can only be judged by looking at it — no test can say whether
//! a superscript sits on the baseline — so this window is how changes to
//! `preview_math` get checked.

#[path = "../src/views/preview_math.rs"]
mod preview_math;

use gpui::prelude::*;
use gpui::{App, Bounds, IntoElement, Render, Window, WindowBounds, WindowOptions, div, px, size};
use gpui_component::scroll::ScrollableElement;
use gpui_component::text::TextView;
use gpui_platform::application;
use preview_math::MathPlugin;

const SAMPLE: &str = r#"## 代码块

```cpp
#include <cstdio>

int main() {
    // 缩进要保住
    if (n <= 1) return n;
    for (int i = 0; i < 10; ++i) {
        printf("%d\n", i * i);
    }
    return 0;
}
```

行内代码 `int x = 1;` 夹在句子里，参数是 `O(n log n)`。

## 公式

行内：$a^2 + b^2 = c^2$、$\frac{n(n+1)}{2}$、$\sqrt{x+1}$、$\pi \leq 3.14$。

上标下标：$x_i^2$、$a_{i,j}^{n}$、$\frac{\frac{1}{2}}{3}$。

块级：

$$\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$$

$$\lim_{x \to 0} \frac{\sin x}{x} = 1$$

$$\begin{cases} 1 & x > 0 \\ 0 & x \le 0 \end{cases}$$

$$\sqrt[3]{x+1} \quad \boxed{x = 1} \quad \text{当 } n \text{ 为偶数}$$
"#;

struct Shot;

impl Render for Shot {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().overflow_y_scrollbar().p_4().child(
            TextView::markdown("shot", SAMPLE.to_owned())
                .plugin(MathPlugin::inline())
                .plugin(MathPlugin::block()),
        )
    }
}

fn main() {
    application()
        .with_assets(gpui_kit_assets::Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            let bounds = Bounds::centered(None, size(px(900.0), px(1000.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_window, cx| cx.new(|_cx| Shot),
            )
            .expect("failed to open window");
            cx.activate(true);
        });
}
