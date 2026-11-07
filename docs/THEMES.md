# TUI 主题

内置主题依次为 `aurora`、`classic`、`phosphor`、`neon`、`nord`、`sand`、`high-contrast`、`plain`。
使用 `cqs themes` 查看本地化名称，`cqs themes --json` 查看 `id`、`name`、`builtin`、`path`、`valid` 和 `error`。
无效文件不会阻止其他主题列出；文本模式向 stderr 报告错误，退出码仍为 0。

优先级：`--theme` > `CQS_THEME` > 配置中的 `theme` > `aurora`。空值不参与选择。
TUI 中按 `T` 打开风格面板；方向键或鼠标实时预览，Enter 保存，Esc 还原。
显式参数或环境变量会在下次启动时覆盖已保存的选择。

## 自定义文件

主题目录为 `$XDG_CONFIG_HOME/cqs/themes`，没有设置 XDG_CONFIG_HOME 时为 `$HOME/.config/cqs/themes`。
`--config` 不改变主题目录。每个 `*.toml` 的文件名主干即主题 ID，必须匹配 `[A-Za-z0-9_-]{1,32}`，且不得使用内置 ID。
顶层必须提供字符串 `name`、`background = "dark"` 或 `"light"`，以及 `[colors]`、`[patterns]` 两个表。
每个键都必须存在；每一层拒绝未知键。颜色接受 `default`、`#rrggbb` 或下列 ANSI 名称：

`black red green yellow blue magenta cyan white bright-black bright-red bright-green bright-yellow bright-blue bright-magenta bright-cyan bright-white`

以下是完整的 `my-nord.toml`：

```toml
name = "My Nord"
background = "dark"

[colors]
background = "#2e3440"
surface = "#3b4252"
surface_alt = "#434c5e"
selection = "#434c5e"
selection_text = "#eceff4"
selection_mark = "#8fc4d3"
dialog = "#3b4252"
dialog_selection = "#434c5e"
border = "#4c566a"
border_focus = "#8fc4d3"
rule = "#4c566a"
shadow = "#242933"
dim = "#4c566a"
text1 = "#eceff4"
text2 = "#d8dee9"
text3 = "#8f9bb3"
accent = "#8fc4d3"
accent2 = "#81a1c1"
on_accent = "#2e3440"
meter = "#8fc4d3"
info = "#a7bed4"
success = "#abc496"
warning = "#ebcb8b"
danger = "#dfb0b4"

[patterns]
border = "square"
title = "inline"
bar = "segment"
sparkline = "blocks"
selection = "bar"
checkbox = "radio"
dot = "circle"
rule = "solid"
keys = "keycap"
background = "paint"
gradient = false
tertiary = "color"
glyphs = "unicode"
```

## 颜色角色

| 角色 | 用途 |
| --- | --- |
| background | 绘制整屏背景时的底色 |
| surface / surface_alt | 面板 / 标题条与键帽 |
| selection / selection_text / selection_mark | 选中行底色、文字与标记 |
| dialog / dialog_selection | 对话框底色 / 对话框选择 |
| border / border_focus / rule | 边框、当前输入区域边框与分隔线 |
| shadow / dim | 阴影 / 弹层外的文字遮罩 |
| text1 / text2 / text3 | 主要文字、次要文字与辅助文字 |
| accent / accent2 / on_accent | 强调色、渐变终点与强调底色上的文字 |
| meter | 低于 70% 的常规仪表颜色 |
| info / success / warning / danger | 信息、成功、警告与错误 |

## 图案

| 键 | 允许值 |
| --- | --- |
| border | rounded, square, double, heavy, ascii, card, titlebar |
| title | inline, bracket, arrow, bar, label |
| bar | line, block, segment, square, hash |
| sparkline | blocks, braille, ascii |
| selection | fill, bar, inverse, arrow |
| checkbox | box, filled, diamond, radio, square, ascii |
| dot | circle, diamond, square, ascii |
| rule | solid, dotted, dashed, none, ascii |
| keys | keycap, plain, bracket |
| background | terminal, paint |
| gradient | true, false（布尔值） |
| tertiary | color, dim |
| glyphs | unicode, ascii |

## 状态字形

| 含义 | Unicode | ASCII |
| --- | --- | --- |
| 成功 / 失败 / 警告 | ✓ / ✗ / ! | + / x / ! |
| 已取消 / 待执行 / 跳过 | ■ / ○ / – | = / . / - |
| 分隔符 / 行标记 | · / › | \| / > |
| 标题箭头 / 选择箭头 | ▶ / ▸ | > / > |
| 接管终端 / 省略号 | ↗ / … | ^ / ... |
| Enter / 上下 / 左右 | ↵ / ↑↓ / ←→ | Enter / Up/Dn / Lt/Rt |
| 旋转指示 | ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ | - \ \| / |

字形、边框和图案不使用 emoji 或 Nerd Font。部分 Unicode 字符在终端采用双宽的 ambiguous-width 设置时可能错位，IPMI/串口建议 `cqs --theme plain --lang en-US`。本地化文字和参数值保持原文。

## 校验与回退

错误包括无效或保留 ID、读取失败、TOML 语法、缺少键、未知键、无效颜色与枚举值；错误包含路径及相关键/值。目录不存在表示没有自定义主题，不是错误。
选择未知或无效主题时回退到 aurora，并显示原因；面板中的无效条目保持上一份有效预览。
aurora 根据终端背景派生颜色；关闭检测或终端未回复时使用 classic 深色配色与图案，但 ID 仍为 aurora。
可在 config.toml 设置 `detect_background = false`。旧版本不认识 `theme` 和 `detect_background`，降级前需移除这两个键。
