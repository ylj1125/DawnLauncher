// 应用控制器
// 连接 SQLite 数据层、原生模块与 Slint UI

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString};

use crate::db::Database;
use crate::models::{Classification, Item};
use crate::native;
// MainWindow, ClassificationInfo, ItemInfo 由 slint::include_modules!() 生成在 crate 根
use crate::{ClassificationInfo, ItemInfo, MainWindow};

pub struct App {
    main_window: MainWindow,
    db: Arc<Database>,
    current_classification_id: Arc<Mutex<i64>>,
}

impl App {
    /// 初始化应用
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // 1. 获取数据库路径
        let db_path = get_database_path()?;
        log::info!("数据库路径: {}", db_path.display());

        // 2. 打开并初始化数据库
        let db = Arc::new(Database::open(db_path.to_str().unwrap())?);
        db.init_schema()?;
        db.ensure_default_data()?;

        // 3. 创建主窗口
        let main_window = MainWindow::new()?;

        // 4. 加载分类列表
        let classifications = db.list_parent_classifications();
        let first_id = classifications.first().map(|c| c.id).unwrap_or(0);

        // 5. 绑定数据到 UI
        let class_model = to_classification_model(&classifications, first_id);
        main_window.set_classifications(class_model);

        // 6. 加载首个分类的项目
        let items = db.list_items(first_id);
        let item_model = to_item_model(&items, 0);
        main_window.set_items(item_model);
        main_window.set_status_text(SharedString::from(format!(
            "共 {} 个分类，{} 个项目",
            classifications.len(),
            items.len()
        )));

        // 7. 绑定 UI 回调
        let db_for_class = db.clone();
        let window_for_class = main_window.as_weak();
        let current_id = Arc::new(Mutex::new(first_id));
        let current_id_for_class = current_id.clone();
        main_window.on_classification_clicked(move |id| {
            let items = db_for_class.list_items(id.into());
            let item_model = to_item_model(&items, 0);
            if let Some(w) = window_for_class.upgrade() {
                // 更新分类选中状态
                let classes: Vec<ClassificationInfo> = w
                    .get_classifications()
                    .iter()
                    .collect::<Vec<_>>();
                let mut new_classes = classes;
                for c in new_classes.iter_mut() {
                    c.selected = c.id == id;
                }
                let new_model = slint::VecModel::from(new_classes);
                w.set_classifications(ModelRc::from(Rc::new(new_model)));
                w.set_items(item_model);
                w.set_status_text(SharedString::from(format!("{} 项", items.len())));
            }
            *current_id_for_class.lock().unwrap() = id.into();
        });

        // 8. 项目单击 (直接打开项目 - 最可靠方式，避免双击事件不触发)
        let db_for_open = db.clone();
        let window_for_open = main_window.as_weak();
        let current_id_for_open = current_id.clone();
        main_window.on_item_clicked(move |item_id| {
            log::info!("项目单击回调触发, item_id={}", item_id);
            let items = db_for_open.list_items(current_id_for_open.lock().unwrap().clone());
            if let Some(item) = items.iter().find(|i| i.id == item_id as i64) {
                open_item(item);
                db_for_open.record_item_open(item_id.into());
            } else {
                log::warn!("未找到 item_id={} 的项目", item_id);
            }
            if let Some(w) = window_for_open.upgrade() {
                let _ = w.window();
            }
        });

        // 9. 项目双击 (同样打开，作为备用)
        let db_for_open2 = db.clone();
        let current_id_for_open2 = current_id.clone();
        main_window.on_item_double_clicked(move |item_id| {
            log::info!("项目双击回调触发, item_id={}", item_id);
            let items = db_for_open2.list_items(current_id_for_open2.lock().unwrap().clone());
            if let Some(item) = items.iter().find(|i| i.id == item_id as i64) {
                open_item(item);
                db_for_open2.record_item_open(item_id.into());
            }
        });

        // 10. 项目右键菜单: 弹出原生菜单 (打开位置/删除)
        let db_for_item_menu = db.clone();
        let window_for_item_menu = main_window.as_weak();
        let current_id_for_item_menu = current_id.clone();
        main_window.on_item_right_clicked(move |item_id| {
            let class_id = current_id_for_item_menu.lock().unwrap().clone();
            // 简化版: 直接弹出确认框删除 (后续可改为完整右键菜单)
            let confirm = show_confirm_dialog(
                "删除项目",
                "确定删除该项目吗？",
            );
            if confirm {
                let _ = db_for_item_menu.delete_item(item_id as i64);
                // 刷新当前分类的项目列表
                let items = db_for_item_menu.list_items(class_id);
                if let Some(w) = window_for_item_menu.upgrade() {
                    w.set_items(to_item_model(&items, 0));
                    w.set_status_text(SharedString::from(format!(
                        "共 {} 个项目",
                        items.len()
                    )));
                }
            }
        });

        // 11. 分类右键菜单: 删除/重命名
        let db_for_class_menu = db.clone();
        let window_for_class_menu = main_window.as_weak();
        main_window.on_classification_right_clicked(move |id| {
            // 简化版: 弹出菜单选项 (删除 / 重命名)
            let action = show_action_dialog(
                "分类操作",
                &["重命名", "删除"],
            );
            match action.as_deref() {
                Some("删除") => {
                    let confirm = show_confirm_dialog(
                        "删除分类",
                        "删除分类将同时删除其下所有项目，确定继续吗？",
                    );
                    if confirm {
                        let _ = db_for_class_menu.delete_classification(id as i64);
                        // 刷新分类列表
                        let classifications = db_for_class_menu.list_parent_classifications();
                        let first_id = classifications.first().map(|c| c.id).unwrap_or(0);
                        let items = db_for_class_menu.list_items(first_id);
                        if let Some(w) = window_for_class_menu.upgrade() {
                            w.set_classifications(to_classification_model(&classifications, first_id));
                            w.set_items(to_item_model(&items, 0));
                            w.set_status_text(SharedString::from(format!(
                                "共 {} 个分类，{} 个项目",
                                classifications.len(),
                                items.len()
                            )));
                        }
                    }
                }
                Some("重命名") => {
                    if let Some(new_name) = show_input_dialog("重命名分类", "请输入新名称") {
                        let _ = db_for_class_menu.rename_classification(id as i64, &new_name);
                        // 刷新分类列表
                        let classifications = db_for_class_menu.list_parent_classifications();
                        if let Some(w) = window_for_class_menu.upgrade() {
                            w.set_classifications(to_classification_model(&classifications, id as i64));
                        }
                    }
                }
                _ => {}
            }
        });

        // 12. 工具栏"添加项目"按钮
        let db_for_add = db.clone();
        let window_for_add = main_window.as_weak();
        let current_id_for_add = current_id.clone();
        main_window.on_add_item_clicked(move || {
            if let Some(path) = show_open_file_dialog() {
                let class_id = current_id_for_add.lock().unwrap().clone();
                let order = db_for_add.max_item_order(class_id) + 1;
                let item = build_item_from_path(&path, class_id, order);
                if let Ok(_id) = db_for_add.insert_item(&item) {
                    // 刷新项目列表
                    let items = db_for_add.list_items(class_id);
                    if let Some(w) = window_for_add.upgrade() {
                        w.set_items(to_item_model(&items, 0));
                        w.set_status_text(SharedString::from(format!(
                            "共 {} 个项目",
                            items.len()
                        )));
                    }
                }
            }
        });

        // 13. 左侧栏"+ 新建分类"按钮
        let db_for_new_class = db.clone();
        let window_for_new_class = main_window.as_weak();
        main_window.on_add_classification_clicked(move || {
            if let Some(name) = show_input_dialog("新建分类", "请输入分类名称") {
                if let Ok(id) = db_for_new_class.insert_parent_classification(&name) {
                    let classifications = db_for_new_class.list_parent_classifications();
                    if let Some(w) = window_for_new_class.upgrade() {
                        w.set_classifications(to_classification_model(&classifications, id));
                        w.set_items(to_item_model(&[], 0));
                        w.set_status_text(SharedString::from("0 个项目"));
                    }
                }
            }
        });

        // 14. 拖拽文件添加项目 (Slint 1.5 通过 DropArea/Window 事件)
        // Slint 1.5 的窗口级拖拽回调
        let db_for_drop = db.clone();
        let window_for_drop = main_window.as_weak();
        let current_id_for_drop = current_id.clone();
        main_window.on_item_dropped(move |path_str| {
            let class_id = current_id_for_drop.lock().unwrap().clone();
            let order = db_for_drop.max_item_order(class_id) + 1;
            let item = build_item_from_path(&path_str, class_id, order);
            if db_for_drop.insert_item(&item).is_ok() {
                let items = db_for_drop.list_items(class_id);
                if let Some(w) = window_for_drop.upgrade() {
                    w.set_items(to_item_model(&items, 0));
                    w.set_status_text(SharedString::from(format!(
                        "共 {} 个项目",
                        items.len()
                    )));
                }
            }
        });

        // 15. 设置按钮 - 切换到设置面板，加载当前设置值
        {
            let db_for_settings = db.clone();
            let window_for_settings = main_window.as_weak();
            main_window.on_settings_clicked(move || {
                let w = match window_for_settings.upgrade() {
                    Some(w) => w,
                    None => return,
                };
                // 从数据库读取设置值
                let theme = db_for_settings.get_setting("theme_mode", "light");
                let auto_start = db_for_settings.get_setting_bool("auto_start", false);
                let topmost = db_for_settings.get_setting_bool("window_topmost", false);
                let columns = db_for_settings.get_setting_i64("item_columns", 8) as i32;
                log::info!(
                    "打开设置: theme={}, auto_start={}, topmost={}, columns={}",
                    theme, auto_start, topmost, columns
                );
                w.set_theme_mode(SharedString::from(theme));
                w.set_auto_start(auto_start);
                w.set_window_topmost(topmost);
                w.set_item_columns(columns);
                w.set_view_mode(SharedString::from("settings"));
                // 同步全局主题
                slint::invoke_from_event_loop({
                    let w = w.as_weak();
                    move || {
                        if let Some(w) = w.upgrade() {
                            apply_theme(&w);
                        }
                    }
                })
                .ok();
            });
        }

        // 15.1 主题切换
        {
            let db_for_theme = db.clone();
            let window_for_theme = main_window.as_weak();
            main_window.on_theme_mode_changed(move |mode| {
                let mode_str = mode.to_string();
                log::info!("主题切换: {}", mode_str);
                let _ = db_for_theme.set_setting("theme_mode", &mode_str);
                if let Some(w) = window_for_theme.upgrade() {
                    w.set_theme_mode(mode.clone());
                    apply_theme(&w);
                }
            });
        }

        // 15.2 开机自启切换
        {
            let db_for_autostart = db.clone();
            let window_for_autostart = main_window.as_weak();
            main_window.on_auto_start_changed(move |enabled| {
                log::info!("开机自启切换: {}", enabled);
                let _ = db_for_autostart.set_setting_bool("auto_start", enabled);
                // 写注册表
                #[cfg(target_os = "windows")]
                {
                    let exe_path = std::env::current_exe()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !exe_path.is_empty() {
                        let result = set_auto_start(enabled, &exe_path);
                        log::info!("注册表写入结果: {:?}", result);
                    }
                }
                if let Some(w) = window_for_autostart.upgrade() {
                    w.set_auto_start(enabled);
                }
            });
        }

        // 15.3 窗口置顶切换
        {
            let db_for_topmost = db.clone();
            let window_for_topmost = main_window.as_weak();
            main_window.on_window_topmost_changed(move |enabled| {
                log::info!("窗口置顶切换: {}", enabled);
                let _ = db_for_topmost.set_setting_bool("window_topmost", enabled);
                #[cfg(target_os = "windows")]
                {
                    if let Some(w) = window_for_topmost.upgrade() {
                        set_window_topmost(&w, enabled);
                    }
                }
                if let Some(w) = window_for_topmost.upgrade() {
                    w.set_window_topmost(enabled);
                }
            });
        }

        // 15.4 项目列数调整
        {
            let db_for_cols = db.clone();
            let window_for_cols = main_window.as_weak();
            main_window.on_item_columns_changed(move |cols| {
                log::info!("项目列数调整: {}", cols);
                let _ = db_for_cols.set_setting_i64("item_columns", cols as i64);
                if let Some(w) = window_for_cols.upgrade() {
                    w.set_item_columns(cols);
                }
            });
        }

        // 16. 窗口关闭
        main_window.on_window_close(move || {
            std::process::exit(0);
        });

        // 17. 应用启动时加载已保存的设置
        {
            let theme = db.get_setting("theme_mode", "light");
            let topmost = db.get_setting_bool("window_topmost", false);
            main_window.set_theme_mode(SharedString::from(theme.clone()));
            main_window.set_window_topmost(topmost);
            apply_theme(&main_window);
            // 启动时应用置顶
            if topmost {
                #[cfg(target_os = "windows")]
                {
                    set_window_topmost(&main_window, true);
                }
            }
            log::info!("启动设置已应用: theme={}, topmost={}", theme, topmost);
        }

        Ok(Self {
            main_window,
            db,
            current_classification_id: current_id,
        })
    }

    /// 运行应用
    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.main_window.run()
    }
}

/// 应用主题
/// Slint 1.5 的 global in-property 无法从 Rust 端直接 set
/// 但我们改成了通过 MainWindow.theme-mode 驱动颜色函数
/// 所以只需设置 window.theme_mode，颜色会自动通过函数计算更新
fn apply_theme(window: &MainWindow) {
    let mode = window.get_theme_mode().to_string();
    log::info!("主题已设置: {}", mode);
    // 颜色绑定在 Slint 中通过 bg-color(root.theme-mode) 等函数实现
    // set_theme_mode 会自动触发所有依赖 root.theme-mode 的颜色重新计算
}

/// 设置窗口置顶 (Windows)
/// 通过 FindWindowW 按标题查找窗口句柄，再用 SetWindowPos 设置置顶
#[cfg(target_os = "windows")]
fn set_window_topmost(_window: &MainWindow, topmost: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, HWND_TOPMOST, HWND_NOTOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
        SetWindowPos,
    };
    use windows::core::{HSTRING, PCWSTR};

    let title = HSTRING::from("Dawn Launcher");
    unsafe {
        let hwnd = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr()));
        if hwnd.0 as usize != 0 {
            let insert_after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
            let _ = SetWindowPos(
                hwnd,
                insert_after,
                0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            log::info!("窗口置顶设置完成: topmost={}", topmost);
        } else {
            log::warn!("未找到 Dawn Launcher 窗口");
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn set_window_topmost(_window: &MainWindow, _topmost: bool) {}

/// 设置/取消开机自启 (Windows 注册表)
#[cfg(target_os = "windows")]
fn set_auto_start(enable: bool, exe_path: &str) -> Result<(), String> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegSetValueExW, RegDeleteValueW,
        HKEY_CURRENT_USER, KEY_SET_VALUE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
        HKEY,
    };
    use windows::core::w;

    // 注册表路径: HKCU\Software\Microsoft\Windows\CurrentVersion\Run
    let sub_key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let value_name = w!("DawnLauncher");

    unsafe {
        if enable {
            // 创建/打开键
            let mut hkey = HKEY::default();
            let result = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_key,
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            );
            if result.is_err() {
                return Err(format!("RegCreateKeyExW 失败: {:?}", result));
            }
            // 设置值
            let value_data: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
            let result = RegSetValueExW(
                hkey,
                value_name,
                0,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    value_data.as_ptr() as *const u8,
                    value_data.len() * 2,
                )),
            );
            let _ = RegCloseKey(hkey);
            if result.is_err() {
                return Err(format!("RegSetValueExW 失败: {:?}", result));
            }
        } else {
            // 打开键并删除值
            let mut hkey = HKEY::default();
            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                sub_key,
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if result.is_err() {
                // 键不存在视为成功
                return Ok(());
            }
            let _ = RegDeleteValueW(hkey, value_name);
            let _ = RegCloseKey(hkey);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_auto_start(_enable: bool, _exe_path: &str) -> Result<(), String> {
    Ok(())
}

/// 获取数据库路径
/// 始终使用程序同级 data/dawn-launcher.db（便携模式）
fn get_database_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let exe_dir = std::env::current_exe()?
        .parent()
        .ok_or("无法获取程序目录")?
        .to_path_buf();

    let data_dir = exe_dir.join("data");
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir.join("dawn-launcher.db"))
}

/// 将分类列表转为 Slint 模型
fn to_classification_model(
    list: &[Classification],
    selected_id: i64,
) -> ModelRc<ClassificationInfo> {
    let model = slint::VecModel::from(
        list.iter()
            .map(|c| ClassificationInfo {
                id: c.id as i32,
                name: SharedString::from(c.name.as_str()),
                selected: c.id == selected_id,
            })
            .collect::<Vec<_>>(),
    );
    ModelRc::from(Rc::new(model))
}

/// 将项目列表转为 Slint 模型
fn to_item_model(list: &[Item], _selected_id: i64) -> ModelRc<ItemInfo> {
    let model = slint::VecModel::from(
        list.iter()
            .map(|item| {
                let icon = item
                    .data
                    .icon
                    .as_deref()
                    .and_then(parse_icon_data_url)
                    .unwrap_or_default();
                let invalid = (item.is_file() || item.is_folder())
                    && item
                        .data
                        .target
                        .as_ref()
                        .map(|t| !std::path::Path::new(t).exists())
                        .unwrap_or(true);
                ItemInfo {
                    id: item.id as i32,
                    name: SharedString::from(item.name.as_str()),
                    icon,
                    invalid,
                    selected: false,
                }
            })
            .collect::<Vec<_>>(),
    );
    ModelRc::from(Rc::new(model))
}

/// 解析 data:image/...;base64,... 格式的图标为 Slint Image
fn parse_icon_data_url(data_url: &str) -> Option<slint::Image> {
    let (_format, base64_part) = parse_data_url(data_url)?;
    let bytes = base64_decode(&base64_part)?;
    // Slint 1.18 (compat-1-18 模式): load_from_data(bytes, format)
    // format 设为 None 让 Slint 自动识别
    slint::Image::load_from_data(&bytes, None).ok()
}

/// 从 data URL 中提取格式和 Base64 内容
fn parse_data_url(data_url: &str) -> Option<(String, String)> {
    // 格式: data:image/png;base64,xxxxx
    let after_data = data_url.strip_prefix("data:")?;
    let semi = after_data.find(';')?;
    let format_part = &after_data[..semi]; // image/png
    let after_semi = &after_data[semi + 1..];
    let comma = after_semi.find(',')?;
    let base64 = &after_semi[comma + 1..];
    Some((format_part.to_string(), base64.to_string()))
}

/// 简易 Base64 解码 (避免引入额外依赖)
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let mut buf = Vec::with_capacity(input.len() * 3 / 4);
    let lookup = |c: u8| -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut state = 0u32;
    let mut count = 0u32;
    for &b in input.as_bytes() {
        if b == b'=' {
            break;
        }
        let v = lookup(b)?;
        state = (state << 6) | (v as u32);
        count += 6;
        if count >= 8 {
            count -= 8;
            buf.push((state >> count) as u8);
            state &= (1 << count) - 1;
        }
    }
    Some(buf)
}

/// 打开项目 (文件/文件夹/网址/Appx)
/// 统一使用 native::open_path (cmd /C start) 打开，可靠处理空格/中文路径
fn open_item(item: &Item) {
    let target = match &item.data.target {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            log::warn!("项目 [{}] 无目标路径，无法打开", item.name);
            return;
        }
    };

    log::info!("准备打开项目: name={}, target={}, type={}", item.name, target, item.kind);

    // 检查目标是否存在 (网址不检查)
    if !item.is_url() && !std::path::Path::new(&target).exists() {
        log::error!("目标路径不存在: {}", target);
        // 弹出错误提示
        #[cfg(target_os = "windows")]
        {
            show_error_dialog(
                "打开失败",
                &format!("目标路径不存在:\n{}", target),
            );
        }
        return;
    }

    // 统一用 native::open_path 打开 (内部用 cmd /C start "" path)
    // 这能正确处理 exe/文件夹/网址/带空格路径
    match native::open_path(&target) {
        Ok(_) => log::info!("打开成功: {}", target),
        Err(e) => {
            log::error!("打开失败: {}, error: {}", target, e);
            #[cfg(target_os = "windows")]
            {
                show_error_dialog(
                    "打开失败",
                    &format!("无法打开:\n{}\n\n错误: {}", target, e),
                );
            }
        }
    }
}

/// 根据路径构建新项目 (自动判断类型)
/// 支持: 文件 / 文件夹 / .lnk 快捷方式 / 网址
fn build_item_from_path(path: &str, classification_id: i64, order: i64) -> Item {
    // 去除两端引号
    let path = path.trim_matches('"').to_string();

    // 判断类型
    let (kind, name, target, params, icon) = if path.starts_with("http://")
        || path.starts_with("https://")
    {
        // 网址
        (
            2,
            path.clone(),
            Some(path.clone()),
            None,
            None,
        )
    } else if path.to_lowercase().ends_with(".lnk") {
        // 快捷方式: 解析 target
        if let Some(info) = native::get_shortcut_file_info(&path) {
            let target = info.get("target").cloned().unwrap_or(path.clone());
            let arguments = info.get("arguments").cloned().unwrap_or_default();
            let name = std::path::Path::new(&path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| target.clone());
            let icon = native::get_file_icon(&target);
            (
                0, // 快捷方式目标通常是文件
                name,
                Some(target),
                if arguments.is_empty() { None } else { Some(arguments) },
                icon,
            )
        } else {
            // 解析失败按普通文件处理
            let name = std::path::Path::new(&path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let icon = native::get_file_icon(&path);
            (0, name, Some(path.clone()), None, icon)
        }
    } else if std::path::Path::new(&path).is_dir() {
        // 文件夹
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let icon = native::get_file_icon(&path);
        (1, name, Some(path.clone()), None, icon)
    } else {
        // 文件
        let name = std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let icon = native::get_file_icon(&path);
        (0, name, Some(path.clone()), None, icon)
    };

    Item {
        id: 0,
        classification_id,
        name,
        kind,
        data: crate::models::ItemData {
            target,
            params,
            icon,
            ..Default::default()
        },
        shortcut_key: None,
        global_shortcut_key: false,
        order,
    }
}

/// 弹出文件选择对话框 (Windows)
/// 返回选中的文件路径，用户取消时返回 None
#[cfg(target_os = "windows")]
fn show_open_file_dialog() -> Option<String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FOS_FILEMUSTEXIST, FOS_PATHMUSTEXIST, IFileOpenDialog, SIGDN_FILESYSPATH,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let result = (|| -> Option<String> {
        unsafe {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            dialog.SetOptions(FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST).ok()?;
            if dialog.Show(None).is_err() {
                return None; // 用户取消
            }
            let result = dialog.GetResult().ok()?;
            let display_name = result.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            Some(display_name.to_string().ok()?)
        }
    })();
    unsafe {
        CoUninitialize();
    }
    result
}

#[cfg(not(target_os = "windows"))]
fn show_open_file_dialog() -> Option<String> {
    None
}

/// 显示确认对话框，返回 true 表示用户点击"是"
#[cfg(target_os = "windows")]
fn show_confirm_dialog(title: &str, message: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONQUESTION, MB_NOFOCUS, MB_YESNO,
    };
    use windows::core::{HSTRING, PCWSTR};

    let title_h = HSTRING::from(title);
    let msg_h = HSTRING::from(message);
    unsafe {
        let result = MessageBoxW(
            None,
            PCWSTR(msg_h.as_ptr()),
            PCWSTR(title_h.as_ptr()),
            MB_YESNO | MB_ICONQUESTION | MB_NOFOCUS,
        );
        // IDYES = 6
        result.0 == 6
    }
}

#[cfg(not(target_os = "windows"))]
fn show_confirm_dialog(_title: &str, _message: &str) -> bool {
    false
}

/// 显示错误对话框
#[cfg(target_os = "windows")]
fn show_error_dialog(title: &str, message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK,
    };
    use windows::core::{HSTRING, PCWSTR};

    let title_h = HSTRING::from(title);
    let msg_h = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(msg_h.as_ptr()),
            PCWSTR(title_h.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn show_error_dialog(_title: &str, _message: &str) {}

/// 显示信息对话框
#[cfg(target_os = "windows")]
fn show_info_dialog(title: &str, message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK,
    };
    use windows::core::{HSTRING, PCWSTR};

    let title_h = HSTRING::from(title);
    let msg_h = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(msg_h.as_ptr()),
            PCWSTR(title_h.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn show_info_dialog(_title: &str, _message: &str) {}

/// 显示输入对话框 (Windows 上通过 PowerShell 实现 InputBox)
/// 返回 Some(输入内容) 表示用户确认，None 表示取消
#[cfg(target_os = "windows")]
fn show_input_dialog(title: &str, prompt: &str) -> Option<String> {
    // 通过 PowerShell 的 [Microsoft.VisualBasic.Interaction]::InputBox 实现
    // 使用 CREATE_NO_WINDOW 标志隐藏 PowerShell 控制台窗口
    let script = format!(
        r#"
Add-Type -AssemblyName Microsoft.VisualBasic
$result = [Microsoft.VisualBasic.Interaction]::InputBox('{}', '{}', '')
if ($result -ne '') {{ $result }} else {{ '' }}
"#,
        prompt.replace('\'', "''"),
        title.replace('\'', "''")
    );
    let output = run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script])?;
    let stdout = String::from_utf8_lossy(&output).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

#[cfg(not(target_os = "windows"))]
fn show_input_dialog(_title: &str, _prompt: &str) -> Option<String> {
    None
}

/// 显示动作选择对话框，返回用户选择的动作名称
/// 返回 None 表示取消
#[cfg(target_os = "windows")]
fn show_action_dialog(title: &str, actions: &[&str]) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    if actions.len() == 1 {
        return Some(actions[0].to_string());
    }

    // 构建 PowerShell 菜单
    let menu_items: Vec<String> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| format!("{} - {}", i + 1, a))
        .collect();
    let menu_text = menu_items.join("\n");
    let script = format!(
        r#"
Add-Type -AssemblyName Microsoft.VisualBasic
$result = [Microsoft.VisualBasic.Interaction]::InputBox('请选择操作编号:\n{}', '{}', '1')
Write-Output $result
"#,
        menu_text.replace('\'', "''"),
        title.replace('\'', "''")
    );
    let output = run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script])?;
    let stdout = String::from_utf8_lossy(&output).trim().to_string();
    if stdout.is_empty() {
        return None;
    }
    let idx: usize = stdout.parse().ok()?;
    if idx >= 1 && idx <= actions.len() {
        Some(actions[idx - 1].to_string())
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
fn show_action_dialog(_title: &str, _actions: &[&str]) -> Option<String> {
    None
}

/// 隐藏窗口执行命令的统一入口在 native 模块
/// (避免重复实现，统一使用 CREATE_NO_WINDOW 标志)
#[cfg(target_os = "windows")]
fn run_hidden(cmd: &str, args: &[&str]) -> Option<Vec<u8>> {
    crate::native::run_hidden(cmd, args)
}

#[cfg(not(target_os = "windows"))]
fn run_hidden(_cmd: &str, _args: &[&str]) -> Option<Vec<u8>> {
    None
}
