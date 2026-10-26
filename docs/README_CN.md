<div align="center">
    <h2 align="center">Chatrans</h2>
    <p align="center">
        船舶世界 Korabli 的实时聊天解释器
        <br />
        <br />
        <a href="https://github.com/Yusux/Korabli-chatrans/blob/main/README.md">English Document</a>
        ·
        <a href="https://github.com/Yusux/Korabli-chatrans/blob/main/docs/README_CN.md">中文文档</a>
        <br />
        <a href="https://github.com/Yusux/Korabli-chatrans/issues">Bug 反馈</a>
        ·
        <a href="https://github.com/Yusux/Korabli-chatrans/issues">功能请求</a>
    </p>
</div>

## 项目简介

这是一个从 [wows-replays](https://github.com/lkolbly/wows-replays) 开发的 Korabli 实时聊天解释器。它监视游戏根目录下的 `replays` 文件夹，在对局进行中从 `temp.korablireplay` 和 `tempArenaInfo.json` 文件实时获取聊天消息，并通过**置顶悬浮面板**展示，可选接入 OpenAI / Anthropic 兼容的模型供应商做 LLM 翻译。

两种使用方式：

- **双击 `chatrans.exe`（GUI 模式，推荐）** —— 翻译面板悬浮在游戏上方，**不会弹出黑色控制台窗口**。首次运行会自动打开设置视图：选择游戏目录、填入模型供应商信息即可。自 v0.2 起内置的 WebSocket/SSE 网页客户端已移除，面板即客户端。
- **控制台模式（`--headless`，或不带 `gui` feature 的构建）** —— 把聊天原文打印到终端。控制台模式不做翻译，翻译能力在 GUI 面板中。

### 待办事项

- [ ] 游戏内聊天消息的解释界面

## 开始使用

编译后的可执行文件可以在 [releases](https://github.com/Yusux/Korabli-chatrans/releases) 页面中找到。选择正确的版本并下载可执行文件。

您也可以自己编译该项目，以下说明将帮助您编译该项目。

### 前置需求

- rustc
  - 安装：[Rustup](https://rustup.rs/)

### 编译

1. Clone 该仓库
    ``` sh
    git clone git@github.com:Yusux/Korabli-chatrans.git
    cd Korabli-chatrans
    ```
2. 更新子模块
    ``` sh
    git submodule update --init --recursive
    ```
    > P.S. 这一部分是为了获取从游戏内解包出的 scripts 文件夹。我们在 [Korabli-scripts](https://github.com/Yusux/Korabli-scripts) 中提供了更新时正式服最新的 scripts，但是不能保证在服务器每次更新版本时准时更新。如果游戏版本更新后，上一版本的 Chatrans 不能正常使用，请优先尝试更新该子模块或者自行解包 scripts 并放置在 `addons/scripts` 文件夹下
3. 使用如下命令编译项目
    ``` sh
    cargo build --release --features gui
    ```
    （不带 `--features gui` 会得到纯控制台构建。）
4. 编译后的可执行文件 `chatrans.exe` 将在 `target/release/` 目录中找到

## 使用方式

### GUI 模式（双击启动）

双击 `chatrans.exe` 即可 —— 无需任何参数：

1. **首次运行**：自动打开设置视图。Chatrans 会尝试自动发现游戏目录（Windows 注册表 → 常见路径如 `D:\Game\Korabli`）；失败时点**浏览…**手动选择游戏根目录（即包含 `replays` 文件夹的目录，如 `D:\Game\Korabli`）。
2. **模型供应商**（可选）：填写 OpenAI / Anthropic 兼容供应商（名称 / Base URL / API Key / 模型）。可以从供应商拉取模型列表，也可以手动填写模型 id。点「＋ 添加更多模型供应商」可配多个，单选按钮决定当前生效的那一个。密钥经 Windows DPAPI 加密存储（`secrets.dat`），永不落明文。
3. 点**保存配置**。游戏路径有效时面板立即开始监听对局。
4. 进入对局 —— 聊天行带 `[友]`/`[敌]` 徽标上屏；队伍频道内容由生效的模型供应商翻译。

面板**首次启动默认锁定**（点击穿透，绝不抢游戏的点击）；之后记住你上次的锁定状态，重启自动恢复。点锁形图标解锁后可拖动/缩放；锁定态下把鼠标悬停在顶栏 150ms 即可临时接管点图标。齿轮图标打开/关闭设置视图（设置视图为纯黑底）。设置页底部有**面板透明度**滑块（40%–100%，默认 100%）：拖动即时生效，松手写入 `placement.json`，重启后自动恢复；只调背景，文字与图标始终不透明。

配置文件位于 `%LOCALAPPDATA%\KorabliOne\`：

| 文件 | 内容 |
| --- | --- |
| `app.json` | 游戏目录（兼作"已配置"标志） |
| `translation.json` | 模型供应商配置（TomatoRadar 插件 schema 的超集 —— 旧插件配置自动迁移） |
| `secrets.dat` | API 密钥，DPAPI 加密 |
| `placement.json` | 面板位置/尺寸 + 锁定状态 + 透明度 |
| `logs/chatrans.log` | 运行日志（单文件上限 2MB，超出滚成 `.log.1`） |

> **一处用词差异**：界面上一律叫「模型供应商」，代码类型与 `translation.json` 的字段仍叫 `Endpoint` / `Endpoints` / `ActiveEndpointId` —— 这是 TomatoRadar 插件的既有 schema，改名会让老配置失效。两者指的是同一个东西。

### 控制台模式

``` powershell
.\chatrans.exe -r 'path\to\replays'
```

实时打印聊天原文：

``` text
── 新对局开始：北方海峡 ──
[友] Alice·Minotaur: gg wp
[敌] Bob: нужна поддержка
```

控制台模式不做翻译 —— 翻译请使用 GUI 面板。GUI 构建加 `--headless` 行为相同。

> 带任意参数启动时，程序会附着到当前终端（没有则新建一个控制台），所以命令行用法照常看得到输出；双击启动（无参数）则完全不占用控制台，日志写到 `%LOCALAPPDATA%\KorabliOne\logs\chatrans.log`。

``` text
Usage: chatrans.exe [OPTIONS] --replay-dir <REPLAY_DIR>

Options:
  -r, --replay-dir <REPLAY_DIR>
          The replay dir to use
  -t, --target-language <TARGET_LANGUAGE>
          The target language, where `zh` stands for Chinese, `en` stands for English. Default is `zh` [default: zh]
      --access-key-id <ACCESS_KEY_ID>
          The Aliyun access key id
      --access-key-secret <ACCESS_KEY_SECRET>
          The Aliyun access key secret
      --headless
          Run without the translation overlay panel (console mode)
  -h, --help
          Print help
  -V, --version
          Print version
```

## 贡献该项目

**非常感谢您所做的任何贡献**。 您可以 fork 该仓库，修改后提交 pull request。此外，如果您发现错误或想要请求功能，您可以也可以提 [Issue](https://github.com/Yusux/Korabli-chatrans/issues)

## 许可

根据 MIT 许可证分发，有关更多信息，请参阅 [LICENSE](https://github.com/Yusux/Korabli-chatrans/blob/main/LICENSE)
