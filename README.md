# SideLine

An app of todo list.

Sideline 是基于 Tauri 2、Rust 和原生 HTML/CSS/JavaScript 的 Windows 桌面侧边待办工具。

## 功能

- 左右侧停靠、置顶、收起后点击展开；Windows AppBar 保留区与全屏自动隐藏。
- 待办拖动排序，双击或右键编辑正文及注释，支持多条注释。
- 已完成事项分区折叠，删除撤销，中文输入法组合输入保护。
- 本地 JSON 存储、原子写入与备份、错误提示和重试；文件冲突时保留未保存内容后重新加载。
- 停靠侧、收起、置顶及完成区展开状态持久化。

## 目录

- `dist/`：直接嵌入应用的前端源码，**不是应忽略的构建产物**。
- `src-tauri/`：Rust 后端、Windows AppBar、持久化、Tauri 配置及应用图标。
- `icon_pngs/`：构建后修复透明图标所需的 PNG 资源。
- `tests/`：前端、浏览器、原生回归测试及构建/图标处理脚本。
- `docs/reliability-upgrade.md`：可靠性改进的范围与验收标准。

## 构建（Windows）

需要 Rust MSVC 工具链、Visual Studio C++ Build Tools、Windows SDK、WebView2 Runtime；图标处理脚本需要 Windows Python 3。

在项目根目录执行：

```powershell
cargo build --manifest-path src-tauri/Cargo.toml --release --locked
python tests/package_icons.py src-tauri/target/release/sideline.exe
```

生成文件：`src-tauri/target/release/sideline.exe`。图标处理时目标程序不能正在运行。

已有依赖缓存时，也可运行 `python tests/build_windows.py production`（该脚本使用 `--offline`），再对 `tests/artifacts/Sideline.exe` 运行 `tests/package_icons.py`。

## 测试

Rust 单元测试（Windows）：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

前端 DOM 测试需要 Node.js 与 `jsdom`：

```powershell
npm install --no-save --package-lock=false jsdom
node --test tests/frontend.test.cjs
```

浏览器测试需要 Python `playwright` 与 Microsoft Edge；测试使用合成数据和模拟 Tauri IPC，不使用个人浏览器配置或真实待办。

```powershell
python -m venv tests/.venv
tests/.venv/Scripts/python.exe -m pip install playwright
tests/.venv/Scripts/python.exe tests/browser_smoke.py
tests/.venv/Scripts/python.exe tests/browser_features.py
tests/.venv/Scripts/python.exe tests/browser_recovery.py
tests/.venv/Scripts/python.exe tests/browser_edge_cases.py
```

原生测试须先运行 `python tests/build_windows.py qa`，使用独立的 QA 应用标识；这类测试会启动窗口、操作 AppBar，执行前请阅读相应脚本。`tests/deploy_verified.py` 会备份并替换桌面程序，不能作为普通测试随意运行。

## 数据与注意事项

生产数据位于 `%APPDATA%/com.yan.sideline/`。源码仓库不包含真实待办、备份数据、测试浏览器配置、虚拟环境或编译产物。

AppBar 注册/注销可能触发 Windows 桌面布局重排。当前原生实现主要针对 Windows；本仓库未提供 macOS/Linux 适配承诺。
