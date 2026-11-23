# TUI 主题配色与界面样式

CqShell 的外观系统拆分为两个解耦维度：**配色方案（`theme`）**与 **UI 设计样式（`style`）**。
每套内置配色都自带一套专属推荐 UI 设计风格；当样式设为 `auto`（跟随配色）时切换配色会自动呈现对应设计语言，也可随时手动锁定任意样式自由混搭。

## 内置配色与默认样式

| 配色 ID | 名称 | 默认样式 ID | 视觉与动效特征 |
| --- | --- | --- | --- |
| `auto` | 自动适配 | `rounded` 现代圆角 | 探测终端深浅背景派生色阶；`╭─╮` 圆角卡、聚焦边框柔光、高精度轨道进度条 |
| `obsidian` | 曜石 | `bento` 悬浮层叠 | 深邃暗黑 Bento：无硬外框、靠多层表面色阶分层、左强调条呼吸 |
| `daylight` | 澄明 | `editorial` 极简杂志 | 洁净浅色瑞士排印：无左右底边杂框、顶部粗体标题与精致横线、克制淡入动效 |
| `tokyo-night` | 东京夜 | `hud` 精密角框 | 赛博 HUD `┏━ ━┓ / ┗━ ━┛` 高亮四角框、`◆/◇` 菱形符、分段刻度进度条、角标流光 |
| `catppuccin` | 摩卡 | `studio` 柔光顶栏 | 卡片顶部铺满 `SurfaceAlt` 柔光标题色带、无硬框层次、`▸` 指示光标 |
| `rose-pine` | 玫瑰松 | `atelier` 锐角书斋 | 利落 `┌─┐` 直角框、`[ 标题 ]` 标头、点线 `┄` 分隔、分段刻度进度条 |
| `high-contrast` | 高对比 | `heavy` 粗框反色 | 纯黑底、`┏━┓` 加粗边框、实心反色选中、`[KEY]` 键帽（无障碍极致对比） |
| `plain` | 朴素 | `ascii` 纯文本 ASCII | 终端默认颜色与纯 ASCII 边框字形、零底色填充（串口 / IPMI 安全） |

## CLI 与环境变量

- `cqs themes` / `cqs themes --json`：查看所有内置及自定义配色（JSON 包含 `id`、`name`、`default_style`、`builtin`、`path`、`valid`、`error`）。无效自定义主题文件向 stderr 报告诊断信息，退出码仍为 0。
- `cqs styles` / `cqs styles --json`：列出 `auto`（跟随配色）与全部 8 套内置 UI 样式。
- 配色优先级：`--theme` > `CQS_THEME` > 配置 `theme` > `auto`（空值不参与选择）。
- 样式优先级：`--style` > `CQS_STYLE` > 配置 `style` > `auto`（空值不参与选择）。

## TUI 外观与主题抽屉（`T`）

在 TUI 中按 `T` 打开右侧「外观与主题」双标签实时预览抽屉：
- `←→` / `Tab` / 鼠标点击在 **`[配色]`** 与 **`[样式]`** 分页间切换
- `↑↓` / `j k` / 鼠标滚轮实时预览配色（平滑 RGB 色彩渐变过渡）或 UI 样式
- `B` 实时切换**透明终端背景**（无框样式在透明模式下自动补精致细边框以保持清晰边界）
- `M` 实时切换**减弱动效模式**（关闭色彩渐变、抽屉揭幕与环境呼吸动效，适合弱网 SSH）
- `Enter` 一并保存配色、样式及开关到 `config.toml`，`Esc` 完整还原

旧版配置中遗留的主题 ID（如 `aurora`、`classic`、`nord`、`neon`、`phosphor`、`sand`）在读取 `config.toml` 时会自动平滑映射至最接近的现代配色。

## 外观配置项（`config.toml`）

```toml
theme = "auto"
style = "auto" # 省略或 "auto" 表示跟随配色默认样式，也可指定如 "bento" / "hud" 等
detect_background = true
transparent_bg = false
reduced_motion = false
sidebar_collapsed = false
```

## 自定义主题文件

主题目录为 `$XDG_CONFIG_HOME/cqs/themes`，未设置 `XDG_CONFIG_HOME` 时为 `$HOME/.config/cqs/themes`。
`--config` 不改变主题目录。每个 `*.toml` 的文件名主干即主题 ID，必须匹配 `[A-Za-z0-9_-]{1,32}`，且不得使用内置 ID。

顶层字段：
- `name`（必填）：非空字符串，不含控制字符
- `appearance`（必填）：`"dark"` 或 `"light"`
- `style`（可选）：该配色的默认推荐 UI 样式 ID（缺省为 `"rounded"`）
- `[colors]`（必填）：全部 20 个表面与语义颜色角色（拒绝缺键或未知键）

颜色值接受 `default`、`#rrggbb` 或下列 ANSI 16 名称：
`black red green yellow blue magenta cyan white bright-black bright-red bright-green bright-yellow bright-blue bright-magenta bright-cyan bright-white`

完整示例 `my-slate.toml`：

```toml
name = "My Slate"
appearance = "dark"
style = "bento"

[colors]
canvas = "#0d1117"
sidebar = "#121821"
surface = "#161e2a"
surface_alt = "#1e293b"
elevated = "#1b2433"
selection = "#18313a"
border = "#2b394d"
border_focus = "#5eead4"
rule = "#212d3d"
shadow = "#05080d"
text1 = "#edf2f7"
text2 = "#a3b3c6"
text3 = "#73869c"
accent = "#5eead4"
accent2 = "#7dd3fc"
on_accent = "#091318"
info = "#7dd3fc"
success = "#4ade80"
warning = "#fbbf24"
danger = "#fb7185"
```

## 颜色角色

| 角色 | 用途 |
| --- | --- |
| `canvas` | 整屏底层画布背景（开启透明背景时继承终端） |
| `sidebar` | 左侧导航栏、顶部微状态条与底部日志抽屉底色 |
| `surface` | 主工作区卡片（概览 Bento 卡、操作列表、详情检查器） |
| `surface_alt` | 鼠标悬停轻高亮、键帽背景、进度条轨道底槽与顶栏色带底色 |
| `elevated` | 居中悬浮弹层与右侧外观抽屉底色 |
| `selection` | 当前选中行与选中快捷卡片底色 |
| `border` / `border_focus` / `rule` | 普通边框、聚焦/弹窗边框与内部分隔细线 |
| `shadow` | 悬浮弹层 1-cell 投影与压暗遮罩基准色 |
| `text1` / `text2` / `text3` | 主标题正文、次要说明与弱辅助文字 |
| `accent` / `accent2` / `on_accent` | 主强调色、副强调色与强调底色上的反色文字 |
| `info` / `success` / `warning` / `danger` | 进行中/信息、正常/成功、警告与危险/root 语义色 |

## 字形与串口兼容

字形、边框与进度条仅使用单宽安全的通用 Unicode 几何符号（`◆ ◈ ◇ ✦ ▎ ┃ ▸ ● ○ ■ □ ▪ ▫ ▄ ▅ ▁▂▃▄▅▆▇█ ✓ ✗ ↵ ↗`），不依赖 Emoji 或 Nerd Font。
在 IPMI / 串口或极简环境下可使用 `cqs --theme plain --lang en-US`（或 `--style ascii` 单独切换纯 ASCII 边框与字形）。

## 校验与回退

无效主题文件（保留或非法 ID、读取失败、TOML 语法错误、缺键、未知键、无效颜色/外观/样式值）会返回带路径与键名的明确错误，并在启动时回退至 `auto`；在外观抽屉中预览无效条目时会保持上一份有效预览并显示诊断原因。
`auto` 使用 `terminal-colorsaurus` 探测终端深浅色并派生满足 WCAG 对比度约束的完整表面色阶；关闭探测或终端未响应时使用 `obsidian` 深色表面。
