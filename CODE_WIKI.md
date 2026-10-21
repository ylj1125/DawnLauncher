# Dawn Launcher - Code Wiki

> 本文档为 Dawn Launcher 项目仓库的结构化代码 Wiki，涵盖项目整体架构、主要模块职责、关键类与函数说明、依赖关系以及项目运行方式等关键信息。

---

## 目录

1. [项目概述](#1-项目概述)
2. [技术栈与支持平台](#2-技术栈与支持平台)
3. [项目目录结构](#3-项目目录结构)
4. [整体架构](#4-整体架构)
5. [主要模块职责](#5-主要模块职责)
6. [关键类与函数说明](#6-关键类与函数说明)
7. [依赖关系](#7-依赖关系)
8. [项目运行方式](#8-项目运行方式)
9. [构建与打包](#9-构建与打包)
10. [数据存储与备份](#10-数据存储与备份)
11. [附录](#11-附录)

---

## 1. 项目概述

**Dawn Launcher** 是一款 `Windows` 快捷启动工具，用于整理杂乱无章的桌面，分门别类管理桌面快捷方式，使桌面保持干净整洁。

核心功能：

- 分类与子分类管理桌面快捷方式
- 关联文件夹（实时同步文件夹内容）
- 快速搜索（全局快捷键唤起）
- 相对路径（便携路径）支持
- 扫描本机开始菜单
- 本地扫描本机 Appx 应用列表
- 添加网址并一键获取网址信息（标题、图标）
- 自定义主题、背景、外观
- 数据备份与恢复

- **作者**: FanChenIO
- **版本**: 1.5.2
- **License**: MIT
- **官网**: [dawnlauncher.com](https://dawnlauncher.com/)

---

## 2. 技术栈与支持平台

| 类别 | 技术 |
| --- | --- |
| 桌面框架 | Electron 28 |
| 构建工具 | Vite 4 + vite-plugin-electron |
| 前端框架 | Vue 3 + TypeScript |
| 路由 | Vue Router 4 |
| 状态管理 | Pinia 2 |
| UI 库 | Naive UI |
| 样式 | Tailwind CSS + Less |
| 数据库 | better-sqlite3-multiple-ciphers (SQLite，支持加密) |
| 原生模块 | Rust + napi-rs (N-API) |
| 打包 | electron-builder (NSIS) |

**支持平台**: `Windows 10 / Windows 11`

---

## 3. 项目目录结构

```
.
├── electron/                    # Electron 主进程与预加载脚本
│   ├── commons/                 # 主进程公共模块
│   │   ├── betterSqlite3.ts     # SQLite 数据库连接
│   │   ├── constants.ts         # 常量定义
│   │   ├── logger.ts            # 日志
│   │   ├── utilityProcessUtils.ts # 后台 Utility 进程管理
│   │   └── utils.ts             # 主进程工具函数
│   ├── main/                    # 主进程业务模块
│   │   ├── about/               # 关于窗口
│   │   ├── classification/      # 分类管理
│   │   ├── commons/             # 主进程公共 IPC
│   │   ├── data/                # 数据备份/恢复
│   │   ├── item/                # 项目管理
│   │   │   └── commons/         # 项目子模块
│   │   ├── main/                # 主窗口
│   │   ├── search/              # 快速搜索窗口
│   │   ├── setting/             # 设置
│   │   ├── index.ts             # 主进程入口
│   │   └── worker.ts            # Utility 后台工作进程
│   ├── preload/                 # 预加载脚本
│   │   └── index.ts             # contextBridge 暴露 API
│   ├── types/
│   │   └── global.d.ts          # 全局类型声明
│   └── electron-env.d.ts
├── src/                         # Vue 渲染进程源码
│   ├── components/              # 通用组件
│   ├── pages/                   # 页面
│   │   ├── about/               # 关于页
│   │   ├── classification/      # 分类相关页
│   │   ├── data/                # 数据备份/恢复页
│   │   ├── index/               # 主页
│   │   ├── item/                # 项目相关页
│   │   ├── search/              # 快速搜索页
│   │   └── setting/             # 设置页
│   ├── router/
│   │   └── index.ts             # Vue Router 路由表
│   ├── store/
│   │   └── index.ts             # Pinia 全局状态
│   ├── styles/                  # 样式
│   ├── utils/                   # 渲染进程工具
│   ├── App.vue                  # 根组件
│   ├── main.ts                  # 渲染进程入口
│   └── index.d.ts
├── rust/                        # Rust 原生模块
│   ├── lib.rs                   # N-API 导出入口
│   ├── windows/                 # Windows API 封装
│   ├── Cargo.toml
│   └── build.rs
├── commons/                     # 主进程与渲染进程共用代码
│   ├── data/                    # 主题、搜索源等静态数据
│   ├── utils/                   # 通用工具函数
│   │   ├── common.ts            # 通用工具
│   │   └── setting.ts           # 设置默认值生成
│   └── language/                # 多语言资源
├── types/                       # 全局类型定义
│   ├── classification.d.ts      # 分类类型
│   ├── item.d.ts                # 项目类型
│   ├── setting.d.ts             # 设置类型
│   └── common.d.ts              # 通用类型
├── native/                      # 编译产物（Rust .node + SQLite .node）
├── public/                      # 静态资源
├── .env.production              # 生产环境变量
├── rebuild.js                   # 重建原生模块脚本
├── vite.config.ts               # Vite 配置
├── electron-builder.json5       # electron-builder 配置
└── package.json
```

---

## 4. 整体架构

Dawn Launcher 采用 **Electron 主进程 / 渲染进程 / 原生模块 / 后台工作进程** 四层架构：

```
┌──────────────────────────────────────────────────────────────┐
│                       渲染进程 (Vue 3)                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │  Index 页面 │  │ Setting 页面│  │ QuickSearch 等子页面│  │
│  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
│         │   Pinia Store / Vue Router            │             │
│         └────────────────┬──────────────────────┘             │
└──────────────────────────┼───────────────────────────────────┘
                           │ contextBridge 暴露的 API (window.*)
                           │ ipcRenderer.sendSync / ipcRenderer.on
┌──────────────────────────┼───────────────────────────────────┐
│                       预加载脚本                              │
│   electron/preload/index.ts                                  │
└──────────────────────────┼───────────────────────────────────┘
                           │ ipcMain.on
┌──────────────────────────┼───────────────────────────────────┐
│                    主进程 (Electron Main)                     │
│  ┌────────────┐ ┌──────────────┐ ┌─────────────────────────┐ │
│  │ Window 管理│ │ IPC 事件分发 │ │ 业务模块                │ │
│  │ (main/...)│ │ (ipcEvent)   │ │ classification / item / │ │
│  └────────────┘ └──────────────┘ │ setting / search / data │ │
│                                  └────────────┬────────────┘ │
│  ┌─────────────────────────────────────────────┴──────────┐  │
│  │             SQLite (better-sqlite3)                    │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────┬───────────────────────────────────┘
                           │ Utility Process (MessagePort)
┌──────────────────────────┼───────────────────────────────────┐
│              后台工作进程 (worker.ts)                          │
│  开始菜单扫描 / Appx 列表 / 刷新图标 / 文件夹关联 / 有效性校验│
└──────────────────────────┬───────────────────────────────────┘
                           │ require("../../native/addon.node")
┌──────────────────────────┼───────────────────────────────────┐
│              原生模块 (Rust + napi-rs → addon.node)           │
│  文件图标 / 快捷方式解析 / 资源管理器菜单 / 鼠标 HOOK / 全屏 │
└──────────────────────────────────────────────────────────────┘
```

### 4.1 进程职责划分

| 进程 | 职责 |
| --- | --- |
| **主进程** (`electron/main/index.ts`) | 应用生命周期、窗口管理、IPC 事件分发、SQLite 数据库、全局快捷键、托盘 |
| **预加载脚本** (`electron/preload/index.ts`) | 通过 `contextBridge` 安全地向渲染进程暴露主进程能力 |
| **渲染进程** (`src/`) | Vue 3 单页应用，负责 UI 与用户交互 |
| **Utility 工作进程** (`electron/main/worker.ts`) | 后台耗时任务：开始菜单扫描、Appx 列表、文件夹关联刷新、图标刷新、无效项目校验 |
| **原生模块** (`rust/` → `native/addon.node`) | 通过 N-API 调用 Windows 系统 API，提供文件图标、快捷方式解析、资源管理器上下文菜单、鼠标 HOOK、输入法切换等能力 |

### 4.2 IPC 通信机制

渲染进程通过预加载脚本暴露的全局对象（如 `window.classification`、`window.item`、`window.setting` 等）调用主进程能力：

- **同步请求**: `ipcRenderer.sendSync(channel, payload)` → 主进程 `ipcMain.on(channel, ...)` 同步返回。
- **异步事件**: 主进程通过 `webContents.send(channel, data)` 推送事件到渲染进程，渲染进程通过 `ipcRenderer.on(channel, callback)` 监听。
- **Utility 进程通信**: 主进程通过 `MessagePort` 与 `utilityProcess` 双向通信，使用临时文件传递大数据序列化结果。

---

## 5. 主要模块职责

### 5.1 Electron 主进程模块

#### 5.1.1 入口模块 `electron/main/index.ts`

- 应用启动入口，`app.whenReady()` 触发主流程
- 加载原生模块 `global.addon = require("../../native/addon.node")`
- 初始化数据：`classificationDataInit()`、`itemDataInit()`、`initSystemItem()`
- 注册所有 IPC 事件：`indexIpcEvent`、`commonIpcEvent`、`classificationIpcEvent`、`itemIpcEvent`、`settingIpcEvent`、`searchIpcEvent`、`aboutIpcEvent`、`dataIpcEvent`
- 创建主窗口、按需创建快速搜索窗口
- 注册全局快捷键 `setShortcutKey()`
- 处理应用激活、窗口关闭、单实例锁、系统电源事件等

#### 5.1.2 主窗口模块 `electron/main/main/`

- `index.ts`: 创建主窗口、显示/隐藏、边缘吸附、边缘自动隐藏、鼠标滚轮切换、双击任务栏显隐等行为
- `ipcEvent.ts`: 主窗口相关 IPC 事件（最小化、关闭、置顶、显隐切换、托盘菜单等）

#### 5.1.3 分类模块 `electron/main/classification/`

- `data.ts`: 分类的数据库 CRUD、排序、初始化默认分类
- `index.ts`: 分类业务逻辑（图标设置、关联文件夹、聚合、子分类树等）
- `ipcEvent.ts`: 分类相关 IPC 事件注册

#### 5.1.4 项目模块 `electron/main/item/`

- `data.ts`: 项目的数据库 CRUD、批量添加、排序更新、关联文件夹同步
- `index.ts`: 项目业务逻辑（打开、定位、刷新图标、添加网址、开始菜单扫描、Appx 列表、系统项目等）
- `commons/`: 项目子模块（公共数据操作）
- `ipcEvent.ts`: 项目相关 IPC 事件注册

#### 5.1.5 设置模块 `electron/main/setting/`

- `data.ts`: 设置数据持久化（electron-store）与读取
- `index.ts`: 设置变更应用（主题、外观、快捷键、开机自启、托盘、语言等）
- `ipcEvent.ts`: 设置相关 IPC 事件注册

#### 5.1.6 搜索模块 `electron/main/search/`

- `index.ts`: 快速搜索窗口创建与显示/隐藏
- `ipcEvent.ts`: 搜索相关 IPC 事件（搜索项目、执行搜索结果等）

#### 5.1.7 关于模块 `electron/main/about/`

- `index.ts`: 关于窗口创建，禁用标题栏右键菜单
- `ipcEvent.ts`: 关于窗口 IPC 事件

#### 5.1.8 数据模块 `electron/main/data/`

- `data.ts`: 数据备份与恢复（导出/导入 SQLite 数据库文件）
- `index.ts`: 备份/恢复业务逻辑
- `ipcEvent.ts`: 数据相关 IPC 事件

#### 5.1.9 公共模块 `electron/main/commons/`

- `index.ts`: 主进程通用工具函数集合（窗口获取、IPC 广播、对话框、路径转换、网址信息抓取、图片下载、关闭子进程、重启等）
- `cacheData.ts`: 缓存数据
- `ipcEvent.ts`: 公共 IPC 事件（如下载图片、获取网址信息、选择文件对话框等）

### 5.2 主进程公共模块 `electron/commons/`

- `betterSqlite3.ts`: 初始化 SQLite 数据库连接，从环境变量 `VITE_BETTER_SQLITE3_BINDING` 读取 native binding 路径
- `constants.ts`: 全局常量（语言、应用名、数据库表名、默认值、User-Agent 列表、图标后缀等）
- `logger.ts`: 基于 `electron-log` 的日志模块
- `utilityProcessUtils.ts`: Utility 进程封装，提供 `execUtilityProcess(name, data)` 接口，通过 MessagePort 与 worker.ts 通信
- `utils.ts`: 主进程工具函数（路径解析、文件图标获取、随机 UA、图标后缀等）

### 5.3 预加载模块 `electron/preload/index.ts`

通过 `contextBridge.exposeInMainWorld` 安全暴露以下 API 到渲染进程（挂载到 `window`）：

| 命名空间 | 主要方法 |
| --- | --- |
| `classification` | `list`、`selectById`、`add`、`update`、`remove`、`updateOrder`、`setIcon`、`associateFolder`、`aggregate` |
| `item` | `list`、`selectById`、`add`、`batchAdd`、`update`、`remove`、`updateOrder`、`open`、`openFileLocation`、`refreshIcon`、`addURL`、`getURLInfo`、`getStartMenuItemList`、`getAppxItemList`、`getSystemItemList` |
| `setting` | `get`、`update`、`reset` |
| `common` | `downloadImage`、`getURLInfo`、`showOpenDialog`、`showSaveDialog`、`showMessageBox`、`relaunch` |
| `main` | `hide`、`show`、`minimize`、`close`、`setAlwaysTop`、`switchDevTools` |
| `search` | `hide`、`show`、`search` |
| `data` | `backup`、`restore` |
| `about` | `show` |
| `quickSearch` | 全局快速搜索相关事件监听 |

### 5.4 渲染进程模块 `src/`

#### 5.4.1 入口与路由

- `main.ts`: 创建 Vue 应用，挂载 Pinia 与 Router
- `router/index.ts`: 路由表（生产环境使用 hash 路由，开发环境使用 history 路由）

#### 5.4.2 路由表（页面）

| 路径 | 组件 | 说明 |
| --- | --- | --- |
| `/` | `pages/index/Index.vue` | 主界面（分类 + 项目展示） |
| `/Setting/Index` | `pages/setting/Index.vue` | 设置窗口 |
| `/Classification/AddEdit` | `pages/classification/AddEdit.vue` | 分类新增/编辑 |
| `/Classification/SetIcon` | `pages/classification/SetIcon.vue` | 分类图标设置 |
| `/Classification/AssociateFolder` | `pages/classification/AssociateFolder.vue` | 关联文件夹配置 |
| `/Classification/Aggregate` | `pages/classification/Aggregate.vue` | 分类聚合 |
| `/Item/AddEdit` | `pages/item/AddEdit.vue` | 项目新增/编辑 |
| `/Item/NetworkIcon` | `pages/item/NetworkIcon.vue` | 网络图标选择 |
| `/Item/SVGIcon` | `pages/item/SVGIcon.vue` | SVG 图标选择 |
| `/Search/QuickSearch` | `pages/search/QuickSearch.vue` | 快速搜索窗口 |
| `/About` | `pages/about/Index.vue` | 关于窗口 |
| `/Data/BackupRestore` | `pages/data/BackupRestore.vue` | 数据备份/恢复 |

#### 5.4.3 状态管理 `src/store/index.ts`

`useMainStore`（Pinia）维护全局状态：

- `setting`: 应用设置（常规、外观、分类、项目、快速搜索、网络等）
- `classificationList`: 分类树
- `itemMap`: 按分类 ID 索引的项目映射
- `selectedClassificationParentId` / `selectedClassificationChildId`: 当前选中的父/子分类
- 其他 UI 状态（拖拽、搜索结果、加载中等）

#### 5.4.4 渲染进程工具 `src/utils/`

- `common.ts`: 通用工具（事件绑定、防抖等）
- `localSetting.ts`: 本地设置读写
- `shortcutKey.ts`: 快捷键解析与展示
- `style.ts`: 主题/外观样式动态生成

#### 5.4.5 通用组件 `src/components/`

- `ItemIcon.vue`: 项目图标渲染
- `CustomItemIcon.vue`: 自定义图标渲染
- `Desc.vue`: 描述文本组件
- `KeyText.vue`: 快捷键展示组件

### 5.5 共用代码模块 `commons/`

主进程与渲染进程共享：

- `utils/common.ts`: 通用工具函数（`newItem`、`newItemData`、`newCommonItem`、`newCommonItemData`、`getFileName`、`deleteExtname`、`isAbsolutePath`、`convert` 等）
- `utils/setting.ts`: 设置默认值工厂（`getSetting`、`getGeneral`、`getAppearance`、`getClassification`、`getItem`、`getQuickSearch`、`getNetwork`、`getProxy`、`getWebSearch` 等）
- `data/theme.ts`: 内置主题列表
- `data/webSearchSource.ts`: 内置网络搜索引擎源列表
- `language/`: 多语言资源文件（简体中文、English 等）

### 5.6 原生模块 `rust/`

通过 `napi-rs` 编译为 `native/addon.node`，向 Node.js / Electron 提供 Windows 系统 API 能力。

`rust/lib.rs` 导出的 N-API 函数：

| 函数 | 说明 |
| --- | --- |
| `get_file_icon(path)` | 获取文件/文件夹图标（Base64） |
| `search_path(path)` | 在 PATH 环境变量中搜索可执行文件 |
| `get_shortcut_file_info(path)` | 解析 `.lnk` 快捷方式（目标路径、参数、工作目录、图标） |
| `open_file_location(path)` | 在资源管理器中打开文件所在位置并选中 |
| `explorer_context_menu(window, path, x, y)` | 弹出资源管理器右键菜单 |
| `get_env_by_name(name)` | 获取系统环境变量 |
| `is_fullscreen()` | 判断前台窗口是否全屏 |
| `switch_english(window)` | 切换为英文输入法 |
| `create_mouse_hook(callback)` | 创建全局鼠标 HOOK |
| `enable_mouse_hook()` / `disable_mouse_hook()` | 启用/禁用鼠标 HOOK |
| `get_appx_list()` | 获取本机已安装 Appx 应用列表 |
| 其他 | 系统项识别、屏幕取色等 Windows API 封装 |

底层实现位于 `rust/windows/`，调用 Win32 API（如 `SHGetFileInfo`、`IShellLink`、`IContextMenu`、`SetWindowsHookEx` 等）。

### 5.7 类型定义 `types/`

- `classification.d.ts`: 分类（`Classification`、`SubClassification`）类型
- `item.d.ts`: 项目（`Item`、`CommonItem`、`ItemData`、`CommonItemData`）类型
- `setting.d.ts`: 设置（`Setting`、`General`、`Appearance`、`Theme`、`QuickSearch`、`Network`、`Proxy`、`WebSearch`、`WebSearchSource` 等）类型
- `common.d.ts`: 通用类型（`Result`、`ShortcutInfo` 等）

---

## 6. 关键类与函数说明

### 6.1 主进程入口关键函数

#### `electron/main/index.ts`

- `app.whenReady().then(...)`: 应用就绪后初始化原生模块、数据、IPC、窗口、快捷键
- `createMainWindow()`: 创建主窗口，配置透明、无边框、预加载脚本
- `createQuickSearchWindow()`: 创建快速搜索窗口（隐藏、置顶、无边框）
- `setShortcutKey()`: 注册全局快捷键（显示/隐藏主窗口、快速搜索等）
- `app.on('window-all-closed' | 'activate' | 'before-quit' ...)`: 应用生命周期处理

### 6.2 分类模块关键函数

#### `electron/main/classification/data.ts`

- `init()`: 创建 `classification` 表（id、parent_id、name、type、data、shortcut_key、global_shortcut_key、order），若无数据则插入默认分类
- `list(parentId?)`: 查询分类列表，按 `order` 升序
- `add(parentId, name, type, data, shortcutKey, globalShortcutKey)`: 新增分类
- `update(id, ...)`: 更新分类
- `remove(id)`: 递归删除分类及其子分类与项目
- `updateOrder(idList)`: 批量更新排序
- `getMaxOrder(parentId)`: 获取最大排序值

#### `electron/main/classification/index.ts`

- `classificationDataInit()`: 调用 `data.init()` 并初始化默认分类
- `setIcon(...)`: 设置分类图标
- `associateFolder(...)`: 关联文件夹配置
- `aggregate(...)`: 分类聚合

### 6.3 项目模块关键函数

#### `electron/main/item/data.ts`

- `init()`: 创建 `item` 表（id、classification_id、name、type、data、shortcut_key、global_shortcut_key、order）
- `list(classificationId)`: 按分类查询项目
- `add(classificationId, item)`: 新增项目
- `batchAdd(classificationId, itemList, reuseId?)`: 事务批量新增项目，自动递增 `order`
- `update(id, ...)`: 更新项目
- `remove(id | idList)`: 删除单个或批量项目
- `updateOrder(idList)`: 批量更新排序
- `getMaxOrder(classificationId)`: 获取最大排序值

#### `electron/main/item/index.ts`

- `itemDataInit()`: 初始化项目表
- `open(item, type)`: 打开项目（文件/文件夹/网址/Appx），支持参数、工作目录、管理员模式
- `openFileLocation(item)`: 在资源管理器中定位
- `refreshIcon(itemList)`: 调用 Utility 进程刷新图标
- `addURL(url)`: 添加网址项目
- `getStartMenuItemList()`: 通过 Utility 进程扫描开始菜单
- `getAppxItemList()`: 通过 Utility 进程获取 Appx 应用列表
- `getSystemItemList()`: 获取系统项（此电脑、回收站等）
- `getDirectoryItemList(...)`: 关联文件夹实时同步

### 6.4 设置模块关键函数

#### `electron/main/setting/data.ts`

- `get()`: 读取设置（electron-store）
- `update(setting)`: 更新并持久化设置
- `reset()`: 重置为默认设置

#### `electron/main/setting/index.ts`

- `applySetting(setting)`: 应用设置变更（主题、透明度、背景、快捷键重注册、开机自启、托盘等）
- `setStartup(startup)`: 开机自启开关
- `setShortcutKey()`: 重新注册全局快捷键

### 6.5 公共工具函数

#### `electron/main/commons/index.ts`

- `getWindow(name)`: 按名称获取窗口实例（`mainWindow`、`quickSearchWindow`、`settingWindow` 等）
- `closeWindow(window)`: 安全关闭窗口
- `sendToWebContent(windowName, listener, params)`: 向指定窗口发送 IPC 事件
- `sendAllWindows(channel, data)`: 向所有窗口广播 IPC
- `downloadImage(windowName, url)`: 下载图片（支持代理、重试、UA 轮换）
- `getURLInfo(windowName, url, redirect)`: 抓取网址标题与图标（cheerio 解析 HTML，处理 meta refresh 跳转）
- `convertPath(path)`: 相对路径与绝对路径互转（便携版/安装版区分基准路径）
- `closeAllChildProcess()`: 关闭所有 Utility 子进程
- `openAfterHideWindow(type)`: 打开项目后按配置隐藏主窗口/搜索窗口
- `relaunch()`: 重启应用
- `getUserDataPath()`: 获取用户数据目录（便携版使用程序同级 `data/` 目录）
- `getMainBackgorunColor()`: 获取主窗口背景色（用于非透明窗口）
- `showMessageBoxSync` / `showErrorMessageBox` / `showSaveDialogSync` / `showOpenDialogSync`: 通用对话框封装

### 6.6 Utility 工作进程函数

#### `electron/main/worker.ts`

通过 `process.parentPort` 接收主进程消息，根据 `params.name` 分发：

- `getStartMenuItemList(cacheList)`: 扫描 `%AppData%\Microsoft\Windows\Start Menu\Programs` 与 `%ProgramData%\Microsoft\Windows\Start Menu\Programs`，解析 `.lnk` 快捷方式
- `getAppxItemList()`: 通过 `global.addon.getAppxList()` 获取 Appx 列表，解析 `AppxManifest.xml` 提取图标与名称
- `refreshItemIcon(itemList)`: 批量刷新文件图标
- `getDirectoryItemList(classificationId, dir, hiddenItems, oldList)`: 关联文件夹实时同步，复用旧数据图标
- `checkInvalidItem(itemList)`: 校验文件/文件夹是否存在，返回无效项目 ID 列表

辅助函数：

- `getFiles(dir)`: 递归读取目录下所有文件
- `getAppxInfo(appx)`: 解析单个 Appx 的 `AppxManifest.xml`，智能选择最高分辨率图标（targetsize / scale / Theme-Dark_Scale）
- `getMaxIconSize(list, name, type)`: 从文件名提取并返回最大图标尺寸
- `getPropertiesIcon(installLocation, result)`: 从 Appx Properties 获取 Logo 作为兜底
- `xml2jsSync(xml)`: 同步解析 XML

### 6.7 渲染进程关键函数

#### `src/store/index.ts`

- `useMainStore`: Pinia store，提供 `setting`、`classificationList`、`itemMap` 等响应式状态及操作方法

#### `commons/utils/common.ts`

- `newItem(partial)`: 构造 `Item` 实例
- `newItemData(partial)`: 构造 `ItemData` 实例
- `newCommonItem(partial)`: 构造 `CommonItem` 实例（用于开始菜单/Appx 列表）
- `newCommonItemData(partial)`: 构造 `CommonItemData` 实例
- `getFileName(path)`: 从路径提取文件名
- `deleteExtname(name)`: 去除文件扩展名
- `isAbsolutePath(path)`: 判断是否绝对路径
- `convert<T, U>(src)`: 类型转换工具

#### `commons/utils/setting.ts`

- `getSetting(raw)`: 合并默认值生成完整 `Setting` 对象
- `getGeneral(...)` / `getAppearance(...)` / `getClassification(...)` / `getItem(...)` / `getQuickSearch(...)` / `getNetwork(...)` / `getProxy(...)` / `getWebSearch(...)`: 各设置子项默认值工厂

---

## 7. 依赖关系

### 7.1 运行时依赖 (`dependencies`)

| 依赖 | 用途 |
| --- | --- |
| `pinia` | 渲染进程状态管理 |
| `vue-router` | 渲染进程路由 |
| `electron-log` | 主进程日志 |
| `electron-store` | 设置持久化（JSON 文件） |
| `better-sqlite3-multiple-ciphers` | SQLite 数据库（支持加密） |
| `cheerio` | 网址信息抓取（HTML 解析） |
| `request` | HTTP 请求（下载图标、抓取网址） |
| `retry` | 请求重试策略 |
| `mime` | MIME 类型识别 |
| `icojs` | ICO 图标解析 |
| `pinyin-pro` | 拼音搜索支持 |
| `sortablejs` | 拖拽排序 |
| `simplebar` | 自定义滚动条 |
| `urijs` | URL 解析（代理地址等） |
| `xml2js` | AppxManifest.xml 解析 |
| `dompurify` | HTML 消毒 |
| `@types/*` | 类型声明 |

### 7.2 开发依赖 (`devDependencies`)

| 依赖 | 用途 |
| --- | --- |
| `electron` | 桌面应用框架 |
| `electron-builder` | 打包（NSIS 安装包） |
| `vite` + `vite-plugin-electron` | 构建工具链 |
| `@vitejs/plugin-vue` | Vue SFC 支持 |
| `vue` + `vue-tsc` + `typescript` | Vue 3 + TS 类型检查 |
| `naive-ui` | UI 组件库 |
| `tailwindcss` + `autoprefixer` + `postcss` | 原子化 CSS |
| `less` + `less-loader` | 样式预处理 |
| `@napi-rs/cli` | Rust N-API 编译 |
| `@vicons/*` | 图标库 |

### 7.3 内部模块依赖关系

```
src/ ──(调用 window.* API)──► electron/preload/index.ts
                                      │
                                      ▼
                            electron/main/* (ipcEvent)
                                      │
                ┌─────────────────────┼─────────────────────┐
                ▼                     ▼                     ▼
      electron/main/*/data.ts   electron/main/commons   electron/commons/*
                │                     │                     │
                ▼                     ▼                     ▼
            SQLite (DB)        utilityProcessUtils      utils / setting
                                      │
                                      ▼
                              electron/main/worker.ts
                                      │
                                      ▼
                          native/addon.node (Rust)
```

### 7.4 原生模块依赖

- `native/addon.node`: 由 `rust/` 编译产出，需 Rust + Cargo 环境
- `native/better_sqlite3.node`: 由 `better-sqlite3-multiple-ciphers` 编译产出，需 `node-gyp` + Python + VS Build Tools

`rebuild.js` 用于针对 Electron 版本重新编译 `better-sqlite3` 原生模块。

---

## 8. 项目运行方式

### 8.1 环境准备

1. **Node.js**: 推荐使用项目要求的 Node 版本
2. **node-gyp**: 编译 SQLite3 原生模块所需
   - 依赖 Python 3 与 Visual Studio Build Tools（C++ 工作负载）
3. **Rust + Cargo**: 编译 Rust 原生模块所需
   - 安装 `rustup`，添加 `stable-x86_64-pc-windows-msvc` 工具链
4. **Yarn**: 包管理器（项目使用 `yarn` 脚本）

### 8.2 安装依赖

```bash
yarn install
```

> `postinstall` 钩子会自动执行：
> - `yarn run rebuild`: 重新编译 `better-sqlite3` 针对 Electron 的原生模块
> - `yarn run rsbuild`: 编译 Rust 代码生成 `native/addon.node`
>
> 若修改了 `rust/` 下的代码，需要重新运行 `yarn install` 或 `yarn run rsbuild`。

### 8.3 本地开发

```bash
yarn run dev
```

- 启动 Vite 开发服务器（`http://127.0.0.1:3344/`）
- 通过 `vite-plugin-electron` 启动 Electron 主进程，加载开发服务器 URL
- 支持热重载（主进程改动触发重启，渲染进程改动触发刷新）

### 8.4 预览构建产物

```bash
yarn run preview
```

### 8.5 环境变量

- `.env.production`:
  - `VITE_INSTALL`: `true` 为安装版，`false` 为便携版（影响用户数据目录位置）
  - `VITE_BETTER_SQLITE3_BINDING`: SQLite 原生模块路径（由 `vite.config.ts` 的 `bindingSqlite3` 插件自动写入 `.env`）
- `package.json#debug.env.VITE_DEV_SERVER_URL`: 开发服务器地址

---

## 9. 构建与打包

### 9.1 构建命令

```bash
yarn run build
```

执行流程：

1. `vue-tsc --noEmit`: TypeScript 类型检查
2. `vite build`: 构建渲染进程（`dist/`）与主进程（`dist-electron/main`、`dist-electron/preload`）
3. `electron-builder`: 按 `electron-builder.json5` 打包

### 9.2 打包配置 `electron-builder.json5`

- `appId`: `com.dawnlauncher.application`
- `productName`: `Dawn Launcher`
- `output`: `release/${version}`
- `target`: `nsis`（Windows NSIS 安装包），架构 `x64`
- `asar`: `true`，`asarUnpack: ["**/*.node"]`（原生模块解包）
- `npmRebuild`: `false`（已在 `postinstall` 阶段完成重建）
- `files`: `["dist", "dist-electron", "native", "!node_modules/**/*"]`
- `icon`: `public/logo.ico`
- NSIS 选项：非一键安装、允许提升权限、允许更改安装目录、创建桌面快捷方式、创建开始菜单快捷方式

### 9.3 便携版与安装版

通过修改 `.env.production` 中的 `VITE_INSTALL`：

- `VITE_INSTALL=true`: 安装版，用户数据存于 `%APPDATA%\Dawn Launcher`
- `VITE_INSTALL=false`: 便携版，用户数据存于程序同级 `data/` 目录

便携版与安装版需分别打包两次。

### 9.4 Vite 配置要点 `vite.config.ts`

- 三个 Electron 入口：`electron/main/index.ts`、`electron/preload/index.ts`、`electron/main/worker.ts`
- 开发模式使用 `notBundle()` 插件（不打包主进程，便于调试）
- 构建前清空 `dist-electron` 目录
- 自定义插件 `bindingSqlite3`: 将 `better-sqlite3` 的 `.node` 文件拷贝到 `native/` 目录，并写入 `.env` 的 `VITE_BETTER_SQLITE3_BINDING`

### 9.5 原生模块重建

```bash
yarn run rebuild
```

执行 `rebuild.js`，针对当前 Electron 版本重新编译 `better-sqlite3-multiple-ciphers`。

```bash
yarn run rsbuild
```

执行 `napi build --release --strip ./rust`，编译 Rust 代码生成 `native/addon.node`。

---

## 10. 数据存储与备份

### 10.1 数据库

- **引擎**: `better-sqlite3-multiple-ciphers`（同步 API，支持加密）
- **连接初始化**: `electron/commons/betterSqlite3.ts`
- **位置**: 用户数据目录下（便携版为程序同级 `data/`）

主要表：

- `classification`: 分类（含子分类，通过 `parent_id` 建立树形关系）
- `item`: 项目（关联 `classification_id`）

### 10.2 设置存储

- **引擎**: `electron-store`（JSON 文件）
- 由 `electron/main/setting/data.ts` 读写

### 10.3 备份与恢复

- `electron/main/data/`: 提供数据库文件导出（备份）与导入（恢复）能力
- 通过 `data/BackupRestore.vue` 页面触发

---

## 11. 附录

### 11.1 关键脚本一览

| 脚本 | 命令 | 说明 |
| --- | --- | --- |
| `dev` | `vite` | 本地开发 |
| `build` | `vue-tsc --noEmit && vite build && electron-builder` | 类型检查 + 构建 + 打包 |
| `preview` | `vite preview` | 预览构建产物 |
| `rsbuild` | `napi build --release --strip ./native` | 编译 Rust |
| `rebuild` | `electron rebuild.js` | 重建 SQLite 原生模块 |
| `postinstall` | `yarn run rebuild && yarn run rsbuild` | 安装后自动重建原生模块 |

### 11.2 全局对象

主进程通过 `global` 暴露的关键对象（见 `electron/types/global.d.ts`）：

- `global.addon`: Rust 原生模块实例
- `global.setting`: 当前应用设置
- `global.language`: 当前语言资源
- `global.mainWindow` / `global.quickSearchWindow` / `global.settingWindow` / `global.aboutWindow` / `global.classificationAddEditWindow` / ...: 各窗口实例
- `global.childProcessMap`: Utility 子进程映射
- `global.mainWindowShowDialog`: 主窗口是否正在显示对话框（用于失焦判断）

### 11.3 多语言

- 资源位于 `commons/language/`
- 通过 `global.setting.general.language` 切换（`SimplifiedChinese` / `English` 等）

### 11.4 主题

- 内置主题列表位于 `commons/data/theme.ts`
- 主题包含主背景色、文字色、强调色等
- 支持自定义透明度、背景图片（重复模式、位置）、字体阴影、窗口圆角

### 11.5 快捷键体系

- **全局快捷键**: 通过 Electron `globalShortcut` 注册，用于显隐主窗口、唤起快速搜索
- **分类/项目快捷键**: 可配置是否全局生效，存储于数据库 `shortcut_key` 与 `global_shortcut_key` 字段
- **快速搜索快捷键**: 默认 `TAB`，可配置

### 11.6 关联文件夹机制

- 分类可关联一个本地文件夹
- 主进程通过 Utility 工作进程调用 `getDirectoryItemList` 实时读取文件夹内容
- 支持隐藏项配置（`hiddenItems`，逗号分隔）
- 复用旧数据图标，避免重复获取
- 文件夹变化时自动同步

### 11.7 Appx 应用扫描机制

1. `global.addon.getAppxList()` 获取已安装 Appx 包列表（路径、familyName、displayName、logo）
2. 解析每个包的 `AppxManifest.xml`（`xml2js`）
3. 智能选择最高分辨率图标（优先 `targetsize`，其次 `scale`，再次 `Theme-Dark_Scale`，兜底 `Properties/Logo`）
4. 图标转 Base64 内嵌
5. 目标路径格式化为 `Shell:AppsFolder\<familyName>!<appId>`

### 11.8 开始菜单扫描机制

1. 递归读取 `%AppData%\Microsoft\Windows\Start Menu\Programs` 与 `%ProgramData%\Microsoft\Windows\Start Menu\Programs`
2. 筛选 `.lnk` 快捷方式（`application/x-ms-shortcut`）
3. 通过 `global.addon.getShortcutFileInfo()` 解析真实目标与参数
4. 去重（按目标路径）
5. 复用缓存图标，避免重复获取

---

> 本文档基于项目源码静态分析生成，反映仓库当前状态。如代码发生变更，请同步更新本文档。
