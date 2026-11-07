# OpenRazer for Windows · 简体中文

[English](README.md) · 简体中文

在 Windows 10 / 11 上控制 Razer 键盘、鼠标、鼠标垫和受支持的耳机，无需 Razer Synapse、内核驱动或 Razer 账户。

本分支为 **OpenRazer Studio 及其安装器**加入简体中文，保留英文和俄文。覆盖首页、设备、灯光、宏、集成、更新、设置、系统托盘，以及安装、修复和卸载流程。Studio、后台服务、命令行和安装器均使用 **Rust**，运行时无需 Python。经典控制面板 `gui/`、旧版 Tk 界面和命令行暂未汉化；设备型号、协议标识、命令和 API 保持原样。

![OpenRazer Studio 简体中文灯光编辑器（模拟设备）](docs/images/studio-lighting-zh-CN.png)

## 切换中文

- 简体中文 Windows（中国大陆或新加坡）首次启动时会自动使用简体中文。
- 其他系统，或已保存过语言偏好的用户：打开 **Settings → Language → 简体中文**。切换立即生效，托盘菜单同步更新，选择会在下次启动时保留。
- 选择 **设置 → 语言 → 跟随 Windows** 可恢复跟随系统。
- 安装器读取 Studio 保存的语言偏好；首次安装时跟随系统语言。

程序从 Windows 字体目录加载微软雅黑，并以黑体、宋体作为后备，使中文在标题、正文和输入框中正常显示。如果精简版系统删除了这些字体，需要恢复 Windows 简体中文字体。

更新页面优先显示发布说明中的中文内容，没有中文说明时回退到英文。在线更新只检查 [imacte/openrazer-win 的 Releases](https://github.com/imacte/openrazer-win/releases)，并只接受该仓库的安装包与校验和；切换到本分支后会忽略旧更新源的缓存。仓库尚未发布安装包时，不会提供可下载的更新。

GitHub、社区和问题反馈入口均指向本仓库；问题请提交到 [Issues](https://github.com/imacte/openrazer-win/issues)。文档入口使用仓库内说明，不依赖 GitHub Pages。原作者署名、历史更新记录中的引用和 OpenRazer 上游链接保留原地址。

## 功能

- **首页**：查看连接的设备，快速调整灯效、颜色、亮度、DPI 档位和游戏模式，统一关闭或恢复灯光。
- **设备**：调整灵敏度、DPI 档位、轮询率、休眠时间和低电量提醒；受支持的耳机还可以设置均衡器、侧音等。
- **灯光**：按区域选择常亮、呼吸、光谱循环、波浪、响应、涟漪等灯效；Studio 模式支持逐键灯光和图层编辑。
- **配置文件**：每台设备最多保存十个灯光配置，也可关联应用，在该应用位于前台时自动切换。
- **宏**：录制或编辑按键、等待、文本输入和打开程序等步骤，再分配到支持的宏按键。
- **集成**：音乐律动、Windows 主题色，以及 Razer Chroma 游戏灯光联动。
- **维护**：查看后台服务状态，保存诊断报告，修复安装或卸载程序。

使用前请关闭 Razer Synapse，以免两个程序同时修改设备设置。带反作弊系统的游戏不宜添加 Chroma DLL；使用新版 Razer SDK 并验证官方签名的游戏也可能无法联动。具体设备能力以程序显示为准。

## 从源码运行汉化版

需要 Rust、Visual Studio 的 C++ 构建工具和 Windows SDK。Rust 版本由 `studio/rust-toolchain.toml` 指定。Python 仅用于可选的翻译检查、设备数据生成和与旧实现的对照测试。

```powershell
git clone https://github.com/imacte/openrazer-win.git
cd openrazer-win
cd studio
cargo build --locked -p openrazer-daemon -p openrazer-studio
.\target\debug\openrazer-studio.exe
```

Studio 会自动启动同目录下的 `openrazer-win.exe`。也可以手动启动后台：

```powershell
.\target\debug\openrazer-win.exe daemon start
```

在仓库根目录构建完整安装包：

```powershell
.\studio\tools\build-native.ps1
# 可选：同时构建内嵌后台的经典控制面板
.\studio\tools\build-native.ps1 -IncludeClassic
```

输出位于 `dist/`：`openrazer-studio-setup-zh-CN.exe` 是完整安装包，内含 Studio 和 Rust 后台；`openrazer-studio.exe` 是主程序，单独运行时需同目录放置 `openrazer-win.exe`。安装包的普通名称与 `-zh-CN` 名称内容相同，均保留三种界面语言。Studio 内嵌 32/64 位 Chroma DLL，发布构建静态链接 C 运行库。构建和运行均无需安装 Python。

发布流程见 [.github/workflows/release.yml](.github/workflows/release.yml)。旧 Python 实现仍保留，供兼容客户端、数据生成和回归对照使用，不打入原生安装包。

## 命令行常用操作

命令保持英文，便于继续使用现有脚本：

```powershell
openrazer-win list                       # 列出已连接设备
openrazer-win info                       # 查看设备详情
openrazer-win effect static red          # 红色常亮
openrazer-win effect spectrum            # 光谱循环
openrazer-win brightness 60              # 亮度 60%
openrazer-win dpi 1800                   # 鼠标 DPI
openrazer-win poll-rate 1000              # 轮询率
openrazer-win battery                    # 电池状态
openrazer-win profile save night         # 保存配置
openrazer-win profile load night         # 应用配置
openrazer-win doctor                     # 诊断设备连接问题
```

蓝牙 LE 支持已内置，通过 Windows 原生 WinRT 接口访问，无需额外安装包。使用模拟设备测试命令时，可运行：

```powershell
openrazer-win daemon start --emulate
openrazer-win list --emulate --json
openrazer-win effect static red --emulate --device 0203
openrazer-win daemon stop --emulate
```

模拟模式默认使用独立的 `demo` 配置目录，不会操作真实设备。`--direct` 可在后台停止时直接控制硬件；需要持续运行的灯效、宏和集成使用后台服务。旧版 `persistence.json`、`profiles.json`、`macros.json` 和 `integrations.json` 格式及 JSON-RPC 接口保持兼容。

## 翻译维护与检查

简体中文词条位于 [`studio/core/src/zh_cn.rs`](studio/core/src/zh_cn.rs)，以界面英文原文为键。`{}` 占位符的数量和顺序必须与原文一致。新增界面文字时，请同时维护俄文和简体中文表。

```powershell
cd studio
python tools/check_translations.py
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

检查覆盖缺失及过期词条、重复键、占位符、语言偏好保存、系统语言识别、中文日期和发布说明的语言回退。

项目采用 [GPL-2.0-or-later](LICENSE) 许可证。原始项目为 [Ar4ikov/openrazer-win](https://github.com/Ar4ikov/openrazer-win)，基于 [OpenRazer](https://github.com/openrazer/openrazer) 的工作。Razer、Chroma 和 Synapse 的商标归其各自权利人所有，本项目与 Razer Inc. 无关联。
