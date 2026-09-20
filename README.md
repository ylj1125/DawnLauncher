# Dawn Launcher (Rust + Slint 重构版)

`Windows` 快捷启动工具，帮助您整理杂乱无章的桌面，分门别类管理您的桌面快捷方式。

本项目为原 Electron 版 Dawn Launcher 的**全面重构版本**，使用 Rust + Slint 替换 Electron + Vue3 + TypeScript 技术栈。

## 重构目标

- ✅ **大幅减小体积**：从 ~180MB 降至 ~10MB (减少 95%)
- ✅ **提升运行效率**：原生编译，无 Node.js/Chromium 开销
- ✅ **无运行时依赖**：无需 WebView2 / Node.js / .NET
- ✅ **Windows 精简版兼容**：单文件分发，零依赖

## 技术栈

`Rust + Slint + rusqlite + windows-rs`

## 支持平台

`Windows(10/11)` (含精简版)

## 仓库结构

```
.
├── src/                # Rust 源码
│   ├── main.rs         # 程序入口
│   ├── app.rs          # 应用控制器 (数据/UI 桥接)
│   ├── db/             # SQLite 数据访问层 (rusqlite)
│   ├── models/         # 数据模型 (Classification/Item)
│   └── native/         # Windows API 封装 (windows-rs)
├── ui/                 # Slint UI 声明文件
│   ├── main.slint     # 主界面
│   ├── theme.slint    # 主题
│   └── components/     # 通用组件
├── release/            # 部署目录 (含可执行文件, 供用户下载测试)
│   ├── dawn-launcher.exe
│   ├── .portable      # 便携版标记
│   └── README.md      # 部署说明
├── legacy-electron/   # 原 Electron 版源码备份
├── Cargo.toml          # Rust 项目配置
├── build.rs            # Slint 编译脚本
└── .cargo/config.toml  # 交叉编译配置 (Linux -> Windows)
```

## 当前状态

⚠️ **MVP 阶段**：仅实现了核心读取与打开功能，大量功能尚未迁移。
详见 [release/README.md](release/README.md) 的功能清单。

## 编译方法 (开发者)

### 环境要求
- Rust 1.92+
- mingw-w64 (交叉编译 Windows 目标)
- Linux 开发环境 (推荐) 或 Windows

### 命令
```bash
# 安装 Windows GNU 目标
rustup target add x86_64-pc-windows-gnu

# 交叉编译
cargo build --target x86_64-pc-windows-gnu --release

# 输出
target/x86_64-pc-windows-gnu/release/dawn-launcher.exe
```

## 与原项目的关系

- 原 Electron 版源码完整保留在 `legacy-electron/` 目录
- 数据库表结构 100% 兼容，可无缝读取原版数据
- Rust 原生模块 (`src/native/`) 由原 `rust/windows.rs` 迁移而来
- 商业逻辑由 TypeScript 重写为 Rust

## License

MIT License
