# 博客发布工具（algorithmnote-publisher）实施计划

> **来源**：这是 2026-10-12 规划会话的产物，原先只存在于会话归档（artifact）里。代码注释中引用的
> `plan §x.y`（如 `plan §3.1`、`plan §10 风险表`）都指向本文档。2026-10-14 复制入仓库以便检索。
>
> **阅读提示**：本文写于工具还计划作为博客仓库的子目录 `publisher/` 时，有几处已与现状不符——
> 工具现为**独立仓库**（本仓库），按用户级安装包分发；§6 的站点改动只完成了 `books.json` 迁移
> （`eslint.config.js` / `.prettierignore` 的 `publisher/` 忽略已不适用）；博客仓库 push `main`
> 现在会跑 CI（原文写「push main 无 CI」）。各里程碑的完成情况以 `README.md` 的「状态」一节为准。

## Summary

为 `D:\astro-paper`（algorithmnote.cn，Astro 7 + AstroPaper 信奥博客）构建一个**单人使用的桌面发布工具**，把「新增文章 / 新增书籍章节 / 关联题解与视频 / 勾选进度 / 发布上线」从手写 Markdown + 命令行，变成表单 + 按钮。

关键设计决策（均已与用户确认或经调查核实）：

1. **技术栈：Rust + GPUI**（用户拍板）。UI 用 **gpui-component 0.7.x**（`longbridge/gpui-kit`，Apache-2.0，生产验证，自带代码编辑器/Markdown 渲染/Dock 布局/虚拟表格）。产出单一原生小体积二进制，覆盖 Win11 + 两台 Linux Mint。
2. **先做 spike 再正式开工**：用 1–2 天验证五个风险点（双平台编译、编辑器中文 IME、katex 渲染、pnpm/git 子进程、笔记本编译时间），全过才进入 M0；IME 或渲染卡死则回退 Electron 方案（本计划 §9 留有回退说明）。
3. **发布链路**：现状是 EdgeOne Pages **已关联 GitHub 仓库自动构建**（push 到 `main` 即触发部署）。「一键发布」终点是 `git push`，不需要 EO CLI，也不需要迁移项目类型。
4. **核心层：`core` crate** 承载全部内容操作（大纲解析/章节创建/slug 分配/校验），纯 Rust、无 GPUI 依赖、可 `cargo test`、可编译出 CLI 做无头验证；GPUI app 只是它的一层壳。
5. **预览**：`pulldown-cmark` 解析 Markdown + **`katex` crate（KaTeX 本体的 Rust 绑定，quick-js 引擎）** 渲染公式 → HTML 交给 gpui-component 的富文本渲染。与站点 rehype-katex 同源，保真度高。草稿在 `astro dev` 下不可见（`postFilter` 无条件过滤 draft），所以必须有自己的预览；另留「astro dev 真实预览」按钮兜底。
6. **schema 策略**：frontmatter 校验在 Rust 侧做一份 serde 镜像（表单级提示用），**权威校验仍是发布流水线里的 `pnpm build`（含 astro check）**——镜像漂移只会影响提示，不会放出坏发布。
7. **借鉴 Gridea Pro（GPL-3.0，只借鉴设计不搬代码）**：CJK 字数统计、一键部署流水线 UI、**MCP 服务暴露内容操作**（M2，用官方 Rust SDK `rmcp`）。
8. **不再考虑 Obsidian 共存**（用户已弃用），但仍坚持「外科手术式」写文件，保证 git diff 干净。
9. **edgeone CLI v1.6.41 已装**（EdgeOne Makers agent-setup）：仅作备选通道（需另建「直接上传」型项目 + token），默认不用。

非目标：多用户/鉴权、移动端、富文本编辑、EO 项目迁移、站点功能改造（评论/统计等）、远程 Web 访问。

---

## 1. 现状盘点（已核实事实）

### 1.1 站点与内容模型

| 项 | 事实 |
| --- | --- |
| 仓库 | `github.com/wylovecat/blog`（SSH remote，分支只有 `main`，直接推 main，无 PR 流程） |
| 技术栈 | Astro 7.2.8 + AstroPaper，pnpm 10.28.2，Node ≥22.12 |
| 内容 | `src/data/blog/**/*.md`（glob loader，`**/[^_]*.md`，26 篇）。子目录：`lgj/`（基础篇章节 18 篇）、`lgs/`（进阶篇，仅 index）、`solu/`（题解 5 篇）、`examples/`、`_releases/` |
| frontmatter | zod schema 在 `src/content.config.ts`：author / pubDatetime / modDatetime / title / featured / draft / **slug（显式指定 URL 末段）** / tags / description 等 |
| URL 规则 | `src/utils/getPath.ts`：`/posts/<子目录>/<slug或文件名>` |
| 书籍注册 | `src/data/books.ts`：`BookDef[]`（id/title/subtitle/description/indexId/coverClass），纯数据、无 import |
| 大纲解析 | `src/utils/getBooks.ts` + `getSectionProgress.ts`：正则逐行解析任务列表，`[x]`=checked，`📖[label](/posts/...)`=一篇已写题解 |
| 构建 | `pnpm build` = gen-og-fonts → astro check（校验 frontmatter 类型）→ astro build → pagefind → copy-pagefind |
| CI | `.github/workflows/ci.yml` 仅 PR 触发（lint/format/build），**push main 无 CI、无 deploy workflow** |
| 部署 | EdgeOne Pages 关联 GitHub 自动构建，push main → EO 构建部署 → algorithmnote.cn |
| 草稿行为 | `src/utils/postFilter.ts`：`!data.draft` 无条件过滤——**draft 在 dev 服务器也不可见** |

### 1.2 大纲行语法（两本书实际格式有差异，解析器必须兼容）

观察到的真实行（`lgj/lg-index.md` 与 `lgs/lgs-index.md`）：

```
- [x] [B2600 【深基1.例2】简单的分苹果](https://www.luogu.com.cn/problem/B2600) 📖[题解](/posts/lgj/_0x0001) 📺视频
* [ ] [P5705 数字反转](https://www.luogu.com.cn/problem/P5705) 📖题解 📺视频
- [x] [P1102 A-B数对](https://www.luogu.com.cn/problem/P1102) 📖题解 📺[视频](https://www.bilibili.com/video/bv1xZ4y1y7QL)
```

语法要点：

- 列表符 `-` 或 `*` 都存在；checkbox `[x]`/`[ ]`。
- 题目 = 第一个 Markdown 链接，href 是洛谷 URL（题号从 `luogu.com.cn/problem/<ID>` 提取；**不要用 `\b` 或 `\[([^\]]*)\]` 配对中文标题**——本项目已有教训；题号一律从 URL 提取）。
- 题解标记：`📖题解`（未写）↔ `📖[题解](/posts/<group>/<slug>)`（已写）。
- 视频标记：`📺视频`（占位）↔ `📺[视频](<bilibili URL>)`（已发布）。lgj 全部未链接；lgs 全部已链接。
- **`[x]` 语义两本书不同**：lgj 中 `[x]`=题解已写（与题解链接同步）；lgs 中 `[x]` 但题解无链接（靠视频标记完成）。工具写 lgj 行时保持「勾选 ⇔ 题解链接」同步；对 lgs 行只改标记、不动勾选语义。
- 同一题号可出现多次（P5709 出现在两个小节），写操作按「小节 + 题号」定位，不按题号全局替换。
- 行尾可能有多余空格，写回时保留行内其他内容不动。

### 1.3 slug 与章节模板（已核实）

- `_0x` slug 是**全站统一序列**：`_0x0000`（solu/how-to-write-solutions）到 `_0x0012`（lgj 最新），下一个 = `_0x0013`，小写十六进制 4 位补零。分配规则 = 扫描全部 `src/data/blog/**/*.md` 的 `slug: _0xXXXX`，max+1。
- 章节文件名：`<group>/lg-<题号>.md`（如 `lgj/lg-B2620.md`）。
- 章节 frontmatter 模板（照 `lgj/lg-B2600.md`）：
  - `title: 0x0001 【深基1.例2】简单的分苹果`（slug 的 hex 部分去掉下划线 + 空格 + 题目全名）
  - `tags: [《深入浅出程序设计竞赛 基础篇》]`（= 所属书 title）
  - `description: 【深基1.例2】简单的分苹果题解。`
  - `slug: _0x0001`，`featured: false`，`draft: false`，`pubDatetime`/`modDatetime` = ISO `+08:00`
- 章节正文骨架：`## 题目描述` → `## 输入格式` → `## 输出格式` → `## 题目分析` → `## 代码实现`（含 ```` ```cpp ```` 代码块）。
- 普通文章（solu 等）：`slug:` 留空时 URL 用文件名；tags 自由（如 `模拟`、`CSP-J`）。

### 1.4 用户约束（已确认）

- 技术栈：**Rust + GPUI（gpui-component）**。
- 不再用 Obsidian，无需共存设计。
- 独立桌面窗口，只自己用（无鉴权，配置存本地）。
- 编辑器不要富文本。
- EO = git 关联自动构建，发布终点 = push main。
- 三台机器：Win11（家）/ Linux Mint（办公室 32GB）/ Linux Mint 笔记本（16GB，会移动）。

---

## 2. 总体架构

### 2.1 仓库布局（Cargo workspace，放在博客仓库内）

```
博客仓库 D:\astro-paper\
├── src/ …                        # 站点（改动极小，见 §6）
└── publisher/                    # 新增：发布工具（Cargo workspace）
    ├── Cargo.toml                # workspace 定义 + 共享依赖版本
    ├── Cargo.lock                # 提交入库，锁定 GPUI 生态版本
    ├── crates/
    │   ├── core/                 # 内容操作库（纯 Rust，无 GPUI 依赖）
    │   │   ├── src/lib.rs
    │   │   ├── src/frontmatter.rs   # frontmatter 解析/写回（serde_yaml）
    │   │   ├── src/outline.rs       # 大纲行解析 + 外科手术式写回
    │   │   ├── src/posts.rs         # 文章 CRUD / draft / tags
    │   │   ├── src/chapters.rs      # createChapter（原子多文件 + 回滚）
    │   │   ├── src/books.rs         # books.json 读写（见 §6 站点改造）
    │   │   ├── src/slug.rs          # _0x 序列扫描与分配
    │   │   ├── src/validate.rs      # 全量校验（frontmatter 镜像 + 坏链）
    │   │   ├── src/preview.rs       # pulldown-cmark + katex → HTML
    │   │   ├── src/process.rs       # tokio 子进程（pnpm/git/dev server）流式输出
    │   │   └── src/bin/cli.rs       # 无头 CLI：new-post/new-chapter/check/publish
    │   └── app/                  # GPUI 桌面应用
    │       ├── src/main.rs
    │       ├── src/views/           # dashboard / posts / editor / books / publish / settings
    │       └── assets/              # 图标、字体
    └── tests/fixtures/           # 从真实仓库复制的内容样本（lg-index.md 等）
```

依赖基线（实施时以 `cargo add` 解析为准，`Cargo.lock` 提交锁定）：

| 依赖 | 用途 | 备注 |
| --- | --- | --- |
| `gpui` + `gpui-component = "0.7"` | UI（编辑器/表格/树/Dock/表单/通知） | gpui 版本与 gpui-component 对齐；精确锁定，升级单独评估 |
| `gpui-component` features | `tree-sitter-markdown`（编辑器高亮） | 只开需要的 tree-sitter 语言，控制编译时间 |
| `serde` / `serde_yaml` / `serde_json` / `toml` | frontmatter、books.json、配置 | |
| `pulldown-cmark` | Markdown → HTML（预览） | GFM 选项打开（表格/任务列表/删除线/脚注） |
| `katex = "0.4"` | 公式 → HTML（quick-js 内置引擎） | KaTeX 本体，与站点同源；数学块先抽出交给 katex，再拼回 HTML |
| `regex` | 大纲行/题号/slug 解析 | |
| `tokio`（process, io-util） | 子进程与异步 | app 内由 GPUI executor 驱动 |
| `chrono` | pubDatetime 生成（`+08:00`） | |
| `directories` | 配置目录（Win `%APPDATA%` / Lin `~/.config`） | |
| `anyhow` / `thiserror` | 错误 | core 用 thiserror 定义错误类型 |
| `reqwest`（M2） | 洛谷题目拉取 | 后期 |

### 2.2 进程模型

- 单进程 GPUI 应用；长任务（`pnpm build`、`pnpm dev`、`git push`）在 tokio 子进程里跑，stdout/stderr 流式进 UI 日志面板。
- core 不碰全局状态，所有函数接受 `repo_path: &Path` → app、CLI、未来的 MCP 共用同一份逻辑。
- 操作串行队列：发布流水线运行期间锁定编辑类操作（UI 置灰）。
- 配置文件 `config.toml`（`{ repo_path, author, git_remote, git_branch, eo_console_url, skip_local_build }`），放系统配置目录，**永不入库**。
- Windows 下 `pnpm` 解析为 `pnpm.cmd`；子进程环境强制 UTF-8，输出按 UTF-8 解码（避免 GBK 乱码）。

---

## 3. 内容核心库（core crate）设计

### 3.1 API（与 app/CLI 共用）

| 函数 | 行为 |
| --- | --- |
| `list_posts(repo)` | 扫描 blog 下全部 md，返回 frontmatter + 路径 + 所属目录 + draft |
| `get_post / save_post` | 读/写单篇（frontmatter + body 分离；写回只重写 `---` 围栏区，正文逐字节保留，含代码块里的 `---`） |
| `create_post(input)` | 普通文章/题解：目录（`solu` 或根）、title、tags、description、slug（可空）、draft；按模板生成 md |
| `set_draft / set_featured / set_tags` | frontmatter 定点修改 |
| `allocate_slug(repo)` | 扫描 `_0xXXXX` 取 max+1（`_0x0013` 起） |
| `create_chapter(input)` | **章节创建（核心操作）**，见 §3.2 |
| `parse_outline(path)` | 大纲 → 树（Part/Chapter/Section/条目），条目含 checkbox、题号、📖/📺 标记状态、原始行号 |
| `link_solution(input)` | 指定「大纲文件 + 行号 + 题解文章」→ `📖题解` 换成 `📖[题解](/posts/<group>/<slug>)`；lgj 语义书同步勾 `[x]` |
| `link_video(input)` | `📺视频` → `📺[视频](<url>)` |
| `add_outline_entry / add_outline_heading` | 小节末尾插规范新行 / 新增标题行（列表符跟随小节既有风格） |
| `list_books / create_book` | 读写 `books.json`（站点改造见 §6）；新增书同时生成 `<group>/<group>-index.md` 骨架 |
| `list_tags(repo)` | 汇总现有 tags 供自动补全 |
| `validate_all(repo)` | 全部 md 过 frontmatter 镜像校验；大纲 `/posts/...` 链接解析到真实文章（坏链清单）；结构化报告 |
| `render_preview(markdown)` | pulldown-cmark（GFM）+ `$...$`/`$$...$$` 抽出交 `katex::render` → 最终 HTML |

frontmatter 镜像校验：serde struct 对应 §1.1 字段（author/pubDatetime/modDatetime/title/featured/draft/slug/tags/description…），检查必填、日期格式、tags 为字符串数组。**它是表单提示级；权威校验 = 发布流水线中的 `pnpm build`（astro check）。**

### 3.2 create_chapter（原子多文件操作）

输入：`{ book_id, section定位（行号或小节标题）, problem_id, 题目全名（含【深基X.例Y】）, video_url? }`

1. 校验：题号格式、所属书存在、slug 未占用。
2. `slug = allocate_slug()`；hex 序号 = slug 去下划线。
3. 写章节文件 `<group>/lg-<problemId>.md`（§1.3 模板）。
4. 改大纲：已存在同题号行 → 替换 `📖题解` 为 `📖[题解](/posts/<group>/<slug>)`（lgj 语义书同步 `[ ]`→`[x]`；有 video_url 同步 📺）；不存在 → 小节末尾插规范行：`- [x] [<题号> <题目全名>](https://www.luogu.com.cn/problem/<题号>) 📖[题解](/posts/<group>/<slug>) 📺视频`。
5. 重解析自验：新链接可解析、行数变化符合预期。
6. **回滚**：任一步失败 → 删已建章节文件、大纲写回改前快照（写前内存留整文件副本）。

所有文件读写：UTF-8 无 BOM；读取时记录行尾风格（LF/CRLF），写回沿用。

---

## 4. GPUI 应用设计（app crate）

### 4.1 界面布局（gpui-component 组件映射）

主窗口用 **DockArea**：左栏大纲树 / 中栏编辑器 / 右栏预览，可拖拽重排。

| 页面 | 组件 | 说明 |
| --- | --- | --- |
| 仪表盘 | 卡片 + 进度条 | 书籍进度（parse_outline 统计）、最近修改、未关联题解数、坏链数 |
| 文章列表 | `Table`（虚拟滚动） | 目录/标题/tags/draft/时间，筛选搜索；点击进编辑器 |
| 编辑器 | `Editor`（code editor，`tree-sitter-markdown`） | Markdown 源码编辑；状态栏 CJK 字数 + 预计阅读时长；frontmatter 用表单区编辑（Input/Select/Switch/DatePicker） |
| 预览 | gpui-component 富文本/HTML 渲染 | `render_preview` 输出；与编辑器联动刷新（防抖 ~300ms） |
| 书籍管理 | `Tree` + 右键菜单/Dialog | 大纲树；节点操作：新增标题、新增题目行、关联题解、关联视频、勾选切换；「新增章节」向导（步骤 Dialog：选书→选小节→填题号/题目名/视频→预览 diff→确认） |
| 发布面板 | 步骤条 + 日志区（`VirtualList`） | §5 流水线；每步状态/重试/日志 |
| 设置 | 表单 | repo 路径、作者、git remote/branch、EO 控制台链接、「发布前本地构建」开关 |

- 「真实预览」按钮：spawn `pnpm dev`（4321），调系统浏览器打开对应文章 URL（草稿在 dev 不可见，该按钮仅对非草稿启用）。
- 主题：亮/暗跟随系统（gpui-component 主题机制）。
- 中文 IME：编辑器组件的中文输入是 **spike 验证项**（尤其 Linux Mint + fcitx）。

### 4.2 窗口与系统集成

- 单实例（锁文件或系统单实例机制）。
- 关闭时杀子进程树（dev server；Windows `taskkill /T`，Linux 进程组 kill）。
- 退出前未保存提示（编辑器 dirty 标记）。

---

## 5. 发布流水线（一键发布）

步骤（每步可单独重试，失败即停并展示日志尾部）：

1. **校验**：`validate_all()`（frontmatter 镜像 + 大纲坏链）。error 不允许继续，warning 确认后可继续。
2. **本地构建**（默认开，设置里可关）：`pnpm build`，流式输出。**这是 frontmatter 的权威校验**——镜像没抓到的错误在这里兜底。失败 → 停止，不提交。
3. **git 提交**：`git status --porcelain` 展示改动清单；**默认只 add 工具本次会话操作过的文件**（精确路径），可勾选追加；提交信息自动生成（沿用仓库风格，如 `content(lgj): 新增 B2620 数字比较 章节`）、提交前可编辑。git 用系统 `git` 命令 spawn（不引 git2，命令集小：status/add/commit/push/pull）。
4. **git push** `origin main`。non-fast-forward → 停止并提示（不自动 rebase/强推；提供「`git pull --rebase` 后重试」按钮，冲突则停）。
5. **EO 部署**：push 成功即完成；面板显示「已推送，EdgeOne Pages 正在自动构建」+ 控制台链接。EO 构建失败时线上保持上一版本，不属工具故障域；M2 可考虑接腾讯云 API 轮询状态。

---

## 6. 站点仓库改动清单（最小侵入）

| 文件 | 改动 | 说明 |
| --- | --- | --- |
| `src/data/books.json` | **新增** | `BOOKS` 数组的数据化迁移（Rust 侧无法安全解析 TS 源码，JSON 对站点和工具都友好） |
| `src/data/books.ts` | 修改 | 保留 `BookDef` 类型与 `BOOKS` 导出，改为 `import books from "./books.json"` + 类型断言；所有消费方（`getBooks.ts` 等）零改动 |
| `eslint.config.js` | 修改 | ignores 增加 `"publisher/**"` |
| `.prettierignore` | 修改 | 增加 `publisher/` |
| `.gitignore` | 修改 | 增加 `publisher/target/`（Rust 构建产物，可能上 GB） |
| `publisher/**` | **新增** | 工具本体（Cargo workspace） |

不改：`pnpm-workspace.yaml`、CI workflow、`src/pages|utils|components`、`content.config.ts`（不再抽 schema——权威校验靠 build，Rust 侧镜像独立维护）。

验证：books.json 迁移后跑根 `pnpm build` 确认站点行为不变（书籍页、进度统计不变）。

---

## 7. Spike（M0 前置，1–2 天）

目的：在投入完整开发前证伪五个风险。在 `publisher/` 里建最小 demo（不计入正式代码质量要求）：

| # | 验证项 | 通过标准 |
| --- | --- | --- |
| 1 | 双平台编译 | Win11 与 Linux Mint（办公室机）都能 `cargo build` 出 gpui-component demo 窗口并运行（Linux 需装 `libxcb/wayland/vulkan` 相关系统包，记录清单） |
| 2 | 编辑器中文 IME | code editor 组件在两平台用中文输入法（Mint 上 fcitx）连续输入一段话，无丢字/错位/预编辑串残留 |
| 3 | 公式预览链路 | `pulldown-cmark + katex crate` 渲染一段含 `$...$`/`$$...$$`/代码块的真实题解正文 → gpui 富文本渲染出来可读（公式渲染结果与线上页面目测一致） |
| 4 | 子进程 | tokio spawn `pnpm build`（Windows 走 `pnpm.cmd`）流式读取输出无乱码、退出码正确 |
| 5 | 笔记本编译时间 | 16GB 笔记本冷编译 demo 可接受（≤15 分钟；增量 ≤1 分钟） |

出口：5 项全过 → 进入 M0。**第 2、3 项若卡死且无合理绕行 → 回退 Electron 方案**（原 Electron 架构已在本计划历史版本中论证过，renderer/core 设计与 §3 同构，重出计划很快）。

---

## 8. 里程碑

| 阶段 | 内容 | 出口标准 |
| --- | --- | --- |
| Spike | §7 | 五项全过 |
| M0：core + CLI | §3 全部 core API + `cargo test` + `cli.rs`（new-post/new-chapter/check/publish） | §9.1 全绿；CLI 在真实仓库跑一次 create-chapter 后 `pnpm build` 绿，随后 git checkout 还原 |
| M1：GPUI app | §4 全部页面 + §5 发布流水线 | §9.2 全过（双平台） |
| M2：增强（可拆后续会话） | 洛谷题号自动拉题目名（reqwest 抓题目页解析，失败回退手输）、图片粘贴上传（存 `public/assets/images/` 插入 `/assets/images/...`）、**MCP server（官方 Rust SDK `rmcp`，stdio；tools：list_posts/create_post/create_chapter/link_solution/link_video/validate/publish，publish 默认关闭需配置开启——借鉴 Gridea Pro 的 `DEPLOY_ENABLED` 门）**、EO 部署状态轮询（需腾讯云 API，待定）、章节模板可配置 | 另行验收 |

---

## 9. 测试与验收

### 9.1 core 单测（`cargo test`，fixtures 复制真实样本）

- **大纲解析 round-trip**：真实 `lg-index.md`/`lgs-index.md`；解析→改一行→写回→diff 仅该行变化（含行尾空格、列表符保留）。
- **语法兼容**：`-`/`*` 列表符、已链/未链 📖/📺 四种组合、中文标题含【】、同题号多行按行号定位。
- **slug 分配**：fixture 最大 `_0x0012` → 得 `_0x0013`；空库从 `_0x0001`。
- **create_chapter 原子性**：注入大纲写入失败 → 章节文件回滚删除、大纲字节级还原。
- **frontmatter 写回**：正文含 `---` 代码块不误伤；CRLF 样本行尾保持。
- **预览**：含公式/表格/任务列表/代码块的样本渲染不 panic，katex 错误时降级为原样文本不炸页面。

### 9.2 端到端手工验收（M1 完成标准）

1. 根仓库 `pnpm build` 全绿（books.json 迁移后）。
2. 工具「新增章节」：填 B2620 → 生成 `lgj/lg-B2620.md` + 大纲行更新；`git diff --stat` 恰好 2 个文件；`pnpm build` 通过；`/books/basic` 对应小节显示已勾选且链接可达。
3. 「新增文章」到 solu：tags/draft 正确；draft 不进构建产物。
4. 「关联视频」：lgs 某行 `📺视频` → `📺[视频](url)`，diff 仅一行。
5. 「发布」：改一篇文章 → 一键 → build 绿 → commit+push 成功 → EO 控制台出现新部署（人工确认）。
6. 人为制造 non-fast-forward → 工具停在 push 步并给出 rebase 选项，不强推。
7. Win11 与 Linux Mint 各跑通 2、5。

---

## 10. 风险与未决事项

| 项 | 状态 | 应对 |
| --- | --- | --- |
| GPUI 生态 1.0 前 API 变动 | 已识别 | `Cargo.lock` 提交锁定；gpui-component 精确版本；升级单独评估，不随日常构建漂移 |
| gpui-component fork 分散（longbridge / gpui-ce / OpenLLT 等） | 已识别 | 只用官方 `longbridge/gpui-kit`（crates.io `gpui-component`，发布者 huacnlee）；不碰三方 fork |
| 编辑器中文 IME（尤其 Linux fcitx） | **spike #2 验证** | 卡死且无绕行 → 回退 Electron |
| 公式渲染保真 | 风险低 | `katex` crate 是 KaTeX 本体绑定（quick-js），与站点 rehype-katex 同源；spike #3 实测确认；个别宏不支持时降级显示原文 |
| `katex` crate 最后更新 2023-02 | 已知 | KaTeX 输出稳定、依赖少；若编译问题可切 `duktape` backend 或 vendor 固定版本 |
| 大纲格式未来手工漂移 | 长期风险 | 写操作后重解析自验；`validate_all` 进发布流水线第 1 步兜底 |
| frontmatter 镜像与 zod schema 漂移 | 已识别 | 镜像只影响表单提示；权威校验 = 流水线 `pnpm build`；schema 变更时手动同步镜像（字段极少，约 12 个） |
| Rust 冷编译时间（笔记本 16GB） | spike #5 验证 | 只开必要 tree-sitter feature；`mold`/`lld` 链接器、`sccache` 可选 |
| 分支策略 | 默认沿用现状直接推 main | 配置里预留 `git_branch` |
| EO 远程构建失败不可见 | MVP 接受 | 控制台链接；M2 接 API |
| gpui-component 的 webview 能力（0.4.x 曾有 `webview`/wry feature，0.7.x 待核实） | 待实施时核实 | 有则内嵌 astro dev 预览；无则外开系统浏览器（已按此设计，不构成阻塞） |

## 11. 关键外部参考

- gpui-component / GPUI Kit（UI 组件库，Apache-2.0）：https://gpui-kit.com · https://crates.io/crates/gpui-component（0.7.3，2026-10-12 发布）
- GPUI（Zed 的 UI 框架）：https://gpui.rs · https://github.com/zed-industries/zed
- katex crate（KaTeX Rust 绑定）：https://github.com/xu-cheng/katex-rs · https://crates.io/crates/katex
- pulldown-cmark：https://github.com/pulldown-cmark/pulldown-cmark
- rmcp（MCP 官方 Rust SDK，M2 用）：https://github.com/modelcontextprotocol/rust-sdk
- EdgeOne Pages 文档（git 集成自动构建）：https://cloud.tencent.com/document/product/1552
- Gridea Pro（设计借鉴，GPL-3.0）：https://github.com/Gridea-Pro/gridea-pro
- Gridea（原始灵感）：https://github.com/getgridea/gridea
