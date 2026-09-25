// 应用控制器
// 连接 SQLite 数据层、原生模块与 Slint UI

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString};

use crate::db::Database;
use crate::models::{Classification, Item};
use crate::native;
// MainWindow, ClassificationInfo, ItemInfo, MenuItem 由 slint::include_modules!() 生成在 crate 根
use crate::{ClassificationInfo, ItemInfo, MenuItem, MainWindow, QuickSearchWindow};

pub struct App {
    main_window: MainWindow,
    quick_search_window: QuickSearchWindow,
    db: Arc<Database>,
    current_classification_id: Arc<Mutex<i64>>,
    expanded_classifications: Arc<Mutex<std::collections::HashSet<i64>>>,
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

        // 3.1 创建快速搜索窗口（初始隐藏）
        let quick_search_window = QuickSearchWindow::new()?;
        quick_search_window.window().hide().ok();

        // 4. 加载分类列表
        let classifications = db.list_all_classifications();
        // 选中第一个父分类
        let first_id = classifications
            .iter()
            .find(|c| c.parent_id.is_none())
            .map(|c| c.id)
            .unwrap_or_else(|| classifications.first().map(|c| c.id).unwrap_or(0));

        // 4.1 初始化展开状态：默认所有父分类展开
        let expanded_classifications = Arc::new(Mutex::new(
            classifications
                .iter()
                .filter(|c| c.parent_id.is_none())
                .map(|c| c.id)
                .collect(),
        ));

        // 5. 绑定数据到 UI
        let class_model = to_classification_model(&classifications, first_id, &expanded_classifications.lock().unwrap());
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

        // 7.1 分类展开/折叠切换
        {
            let db_for_expand = db.clone();
            let window_for_expand = main_window.as_weak();
            let expanded_for_toggle = expanded_classifications.clone();
            let current_id_for_expand = current_id.clone();
            main_window.on_classification_expand_toggled(move |id| {
                let class_id = id as i64;
                let mut expanded = expanded_for_toggle.lock().unwrap();
                if expanded.contains(&class_id) {
                    expanded.remove(&class_id);
                } else {
                    expanded.insert(class_id);
                }
                drop(expanded);
                // 刷新分类列表
                let classifications = db_for_expand.list_all_classifications();
                let cur = *current_id_for_expand.lock().unwrap();
                if let Some(w) = window_for_expand.upgrade() {
                    w.set_classifications(to_classification_model(&classifications, cur, &expanded_for_toggle.lock().unwrap()));
                }
            });
        }

        // 8. 项目单击 - 批量模式下切换选中，非批量模式下打开项目
        let db_for_open = db.clone();
        let window_for_open = main_window.as_weak();
        let current_id_for_open = current_id.clone();
        main_window.on_item_clicked(move |item_id| {
            log::info!("项目单击回调触发, item_id={}", item_id);
            let class_id = current_id_for_open.lock().unwrap().clone();

            // 批量模式: 切换选中状态
            if let Some(w) = window_for_open.upgrade() {
                if w.get_batch_mode() {
                    // 切换该项目选中状态
                    let current_items: Vec<ItemInfo> = w.get_items().iter().collect::<Vec<_>>();
                    let mut new_items = current_items;
                    for it in new_items.iter_mut() {
                        if it.id == item_id {
                            it.selected = !it.selected;
                        }
                    }
                    let new_model = slint::VecModel::from(new_items);
                    w.set_items(ModelRc::from(Rc::new(new_model)));
                    return;
                }
            }

            // 非批量模式: 打开项目
            let items = db_for_open.list_items(class_id);
            if let Some(item) = items.iter().find(|i| i.id == item_id as i64) {
                open_item(item);
                db_for_open.record_item_open(item_id.into());
                // 打开后隐藏主窗口
                if let Some(w) = window_for_open.upgrade() {
                    if w.get_hide_after_open() {
                        w.window().hide().ok();
                    }
                }
            } else {
                log::warn!("未找到 item_id={} 的项目", item_id);
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

        // 10. 项目右键菜单: 弹出 Slint 右键菜单
        // 注意: 右键坐标从 UI 事件获取，这里 item_id 用于记录当前右键的项目
        let _window_for_item_menu = main_window.as_weak();
        let _current_id_for_item_menu = current_id.clone();
        // 记录当前右键的项目 id（供菜单项选中时使用）
        let right_clicked_item_id = Arc::new(Mutex::new(0i64));
        let right_clicked_item_id_for_menu = right_clicked_item_id.clone();
        main_window.on_item_right_clicked(move |item_id| {
            *right_clicked_item_id_for_menu.lock().unwrap() = item_id as i64;
            // 右键菜单位置: Slint 1.5 的 item-right-clicked 不传坐标，
            // 用内容区近似位置（后续可通过扩展回调传坐标）
            if let Some(w) = _window_for_item_menu.upgrade() {
                show_item_context_menu(&w, 300, 200);
            }
        });

        // 11. 分类项右键菜单
        let window_for_class_menu = main_window.as_weak();
        let right_clicked_class_id = Arc::new(Mutex::new(0i64));
        let right_clicked_class_id_for_menu = right_clicked_class_id.clone();
        main_window.on_classification_right_clicked(move |id| {
            *right_clicked_class_id_for_menu.lock().unwrap() = id as i64;
            if let Some(w) = window_for_class_menu.upgrade() {
                show_classification_item_menu(&w, 160, 200);
            }
        });

        // 11.1 项目区空白右键
        let window_for_item_area = main_window.as_weak();
        main_window.on_item_area_right_clicked(move |x, y| {
            let x = x as i32;
            let y = y as i32;
            if let Some(w) = window_for_item_area.upgrade() {
                let batch = w.get_batch_mode();
                if batch {
                    show_batch_menu(&w, x, y);
                } else {
                    show_item_area_menu(&w, x, y);
                }
            }
        });

        // 11.2 分类区空白右键
        let window_for_class_area = main_window.as_weak();
        main_window.on_classification_area_right_clicked(move |x, y| {
            let x = x as i32;
            let y = y as i32;
            if let Some(w) = window_for_class_area.upgrade() {
                show_classification_area_menu(&w, x, y);
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
        let expanded_for_new_class = expanded_classifications.clone();
        main_window.on_add_classification_clicked(move || {
            if let Some(name) = show_input_dialog("新建分类", "请输入分类名称") {
                if let Ok(id) = db_for_new_class.insert_parent_classification(&name) {
                    let classifications = db_for_new_class.list_all_classifications();
                    // 新建的父分类默认展开
                    expanded_for_new_class.lock().unwrap().insert(id);
                    if let Some(w) = window_for_new_class.upgrade() {
                        w.set_classifications(to_classification_model(&classifications, id, &expanded_for_new_class.lock().unwrap()));
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
                // 兼容旧版本: "dark" -> "classic-dark"
                let raw_theme = db_for_settings.get_setting("theme_mode", "light");
                let theme = if raw_theme == "dark" {
                    "classic-dark".to_string()
                } else {
                    raw_theme
                };
                let auto_start = db_for_settings.get_setting_bool("auto_start", false);
                let topmost = db_for_settings.get_setting_bool("window_topmost", false);
                let columns = db_for_settings.get_setting_i64("item_columns", 8) as i32;
                let icon_size = db_for_settings.get_setting_i64("item_icon_size", 48) as i32;
                let sidebar_w = db_for_settings.get_setting_i64("sidebar_width", 140) as i32;
                let hide_name = db_for_settings.get_setting_bool("hide_name", false);
                let hide_after_open = db_for_settings.get_setting_bool("hide_after_open", false);
                let qs_enabled = db_for_settings.get_setting_bool("quick_search_enabled", false);
                let qs_width = db_for_settings.get_setting_i64("quick_search_width", 600) as i32;
                log::info!(
                    "打开设置: theme={}, auto_start={}, topmost={}, columns={}",
                    theme, auto_start, topmost, columns
                );
                w.set_theme_mode(SharedString::from(theme));
                w.set_auto_start(auto_start);
                w.set_window_topmost(topmost);
                w.set_item_columns(columns);
                w.set_item_icon_size(icon_size);
                w.set_sidebar_width(sidebar_w);
                w.set_hide_name(hide_name);
                w.set_hide_after_open(hide_after_open);
                w.set_quick_search_enabled(qs_enabled);
                w.set_quick_search_width(qs_width);
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
            let qs_for_theme = quick_search_window.as_weak();
            main_window.on_theme_mode_changed(move |mode| {
                let mode_str = mode.to_string();
                log::info!("主题切换: {}", mode_str);
                let _ = db_for_theme.set_setting("theme_mode", &mode_str);
                if let Some(w) = window_for_theme.upgrade() {
                    w.set_theme_mode(mode.clone());
                    apply_theme(&w);
                }
                if let Some(qs) = qs_for_theme.upgrade() {
                    qs.set_theme_mode(mode.clone());
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

        // 15.5 图标大小调整
        {
            let db_for_icon = db.clone();
            let window_for_icon = main_window.as_weak();
            main_window.on_item_icon_size_changed(move |size| {
                log::info!("图标大小调整: {}", size);
                let _ = db_for_icon.set_setting_i64("item_icon_size", size as i64);
                if let Some(w) = window_for_icon.upgrade() {
                    w.set_item_icon_size(size);
                }
            });
        }

        // 15.6 分类栏宽度
        {
            let db_for_sw = db.clone();
            let window_for_sw = main_window.as_weak();
            main_window.on_sidebar_width_changed(move |w_val| {
                log::info!("分类栏宽度: {}", w_val);
                let _ = db_for_sw.set_setting_i64("sidebar_width", w_val as i64);
                if let Some(w) = window_for_sw.upgrade() {
                    w.set_sidebar_width(w_val);
                }
            });
        }

        // 15.7 隐藏名称
        {
            let db_for_hn = db.clone();
            let window_for_hn = main_window.as_weak();
            main_window.on_hide_name_changed(move |v| {
                log::info!("隐藏名称: {}", v);
                let _ = db_for_hn.set_setting_bool("hide_name", v);
                if let Some(w) = window_for_hn.upgrade() {
                    w.set_hide_name(v);
                }
            });
        }

        // 15.8 打开后隐藏
        {
            let db_for_hao = db.clone();
            let window_for_hao = main_window.as_weak();
            main_window.on_hide_after_open_changed(move |v| {
                log::info!("打开后隐藏: {}", v);
                let _ = db_for_hao.set_setting_bool("hide_after_open", v);
                if let Some(w) = window_for_hao.upgrade() {
                    w.set_hide_after_open(v);
                }
            });
        }

        // 15.9 快速搜索启用
        {
            let db_for_qse = db.clone();
            let window_for_qse = main_window.as_weak();
            main_window.on_quick_search_enabled_changed(move |v| {
                log::info!("快速搜索启用: {}", v);
                let _ = db_for_qse.set_setting_bool("quick_search_enabled", v);
                if let Some(w) = window_for_qse.upgrade() {
                    w.set_quick_search_enabled(v);
                }
            });
        }

        // 15.10 快速搜索窗口宽度
        {
            let db_for_qsw = db.clone();
            let window_for_qsw = main_window.as_weak();
            main_window.on_quick_search_width_changed(move |v| {
                log::info!("快速搜索宽度: {}", v);
                let _ = db_for_qsw.set_setting_i64("quick_search_width", v as i64);
                if let Some(w) = window_for_qsw.upgrade() {
                    w.set_quick_search_width(v);
                }
            });
        }

        // 15.11 工具: 数据备份
        {
            let db_for_backup = db.clone();
            main_window.on_backup_data(move || {
                // 备份: 复制数据库文件到 data/backup_<时间戳>.db
                if let Ok(db_path) = get_database_path() {
                    let ts = chrono_now_ms_str();
                    let backup_path = db_path
                        .parent()
                        .map(|p| p.join(format!("backup_{}.db", ts)))
                        .unwrap_or_else(|| std::path::PathBuf::from(format!("backup_{}.db", ts)));
                    match std::fs::copy(&db_path, &backup_path) {
                        Ok(_) => show_info_dialog(
                            "备份成功",
                            &format!("已备份到:\n{}", backup_path.display()),
                        ),
                        Err(e) => show_error_dialog("备份失败", &format!("{}", e)),
                    }
                }
            });
        }

        // 15.12 工具: 数据还原
        {
            main_window.on_restore_data(move || {
                if let Some(backup_file) = show_open_file_dialog() {
                    if let Ok(db_path) = get_database_path() {
                        // 关闭当前连接再还原 (简化: 直接复制覆盖，当前连接会被破坏)
                        // 实际应先关闭，这里提示用户重启
                        match std::fs::copy(&backup_file, &db_path) {
                            Ok(_) => show_info_dialog(
                                "还原成功",
                                "数据已还原，请重启应用使更改生效。",
                            ),
                            Err(e) => show_error_dialog("还原失败", &format!("{}", e)),
                        }
                    }
                }
            });
        }

        // 15.13 工具: 检查无效项目
        {
            let db_for_check = db.clone();
            let window_for_check = main_window.as_weak();
            let current_id_for_check = current_id.clone();
            main_window.on_check_invalid_items(move || {
                let class_id = current_id_for_check.lock().unwrap().clone();
                let items = db_for_check.list_items(class_id);
                let mut invalid_count = 0;
                for item in &items {
                    if (item.is_file() || item.is_folder()) {
                        if let Some(target) = &item.data.target {
                            if !std::path::Path::new(target).exists() {
                                invalid_count += 1;
                            }
                        }
                    }
                }
                if invalid_count > 0 {
                    show_info_dialog(
                        "检查完成",
                        &format!("发现 {} 个无效项目（目标路径不存在）", invalid_count),
                    );
                    // 刷新显示，标记无效
                    if let Some(w) = window_for_check.upgrade() {
                        w.set_items(to_item_model(&items, 0));
                    }
                } else {
                    show_info_dialog("检查完成", "所有项目均有效");
                }
            });
        }

        // 16. 窗口关闭
        main_window.on_window_close(move || {
            std::process::exit(0);
        });

        // 16.1 快速搜索窗口: 搜索文本变更 -> 实时过滤项目
        {
            let db_for_qs = db.clone();
            let qs_weak = quick_search_window.as_weak();
            quick_search_window.on_search_changed(move |text| {
                let query = text.to_string();
                let results = db_for_qs.search_items(&query);
                if let Some(qs) = qs_weak.upgrade() {
                    qs.set_search_text(text.clone());
                    qs.set_search_results(to_item_model(&results, 0));
                }
            });
        }

        // 16.2 快速搜索窗口: 双击结果 -> 打开项目
        {
            let db_for_qs_open = db.clone();
            let qs_weak_open = quick_search_window.as_weak();
            let main_weak_open = main_window.as_weak();
            quick_search_window.on_result_double_clicked(move |item_id| {
                let id = item_id as i64;
                // 从全量项目中查找
                let all_items = db_for_qs_open.list_all_items();
                if let Some(item) = all_items.iter().find(|i| i.id == id) {
                    open_item(item);
                    db_for_qs_open.record_item_open(id);
                    // 打开后隐藏快速搜索窗口
                    if let Some(qs) = qs_weak_open.upgrade() {
                        qs.window().hide().ok();
                    }
                    // 若设置了"打开后隐藏主窗口"也同时隐藏
                    if let Some(w) = main_weak_open.upgrade() {
                        if w.get_hide_after_open() {
                            w.window().hide().ok();
                        }
                    }
                }
            });
        }

        // 16.3 快速搜索窗口: 关闭请求 -> 隐藏窗口
        {
            let qs_weak_close = quick_search_window.as_weak();
            quick_search_window.on_close_requested(move || {
                if let Some(qs) = qs_weak_close.upgrade() {
                    qs.window().hide().ok();
                }
            });
        }

        // 16.4 主窗口: 触发快速搜索 -> 显示并居中快速搜索窗口
        {
            let qs_weak_show = quick_search_window.as_weak();
            let main_weak_show = main_window.as_weak();
            main_window.on_quick_search_triggered(move || {
                if let Some(qs) = qs_weak_show.upgrade() {
                    // 清空搜索文本和结果
                    qs.set_search_text(SharedString::from(""));
                    qs.set_search_results(ModelRc::from(Rc::new(slint::VecModel::default())));
                    // 同步主题
                    if let Some(w) = main_weak_show.upgrade() {
                        qs.set_theme_mode(w.get_theme_mode());
                    }
                    // 显示并居中到屏幕
                    qs.window().show().ok();
                    center_window_on_screen(&qs);
                    log::info!("快速搜索窗口已显示");
                }
            });
        }

        // 18. 右键菜单项选中处理
        {
            let db_for_ctx = db.clone();
            let window_for_ctx = main_window.as_weak();
            let current_id_for_ctx = current_id.clone();
            let right_item_id = right_clicked_item_id.clone();
            let right_class_id = right_clicked_class_id.clone();
            let expanded_for_ctx = expanded_classifications.clone();
            main_window.on_context_menu_item_selected(move |source, action| {
                let source_str = source.to_string();
                let action_str = action.to_string();
                log::info!("右键菜单选中: source={}, action={}", source_str, action_str);

                let w = match window_for_ctx.upgrade() {
                    Some(w) => w,
                    None => return,
                };
                let class_id = current_id_for_ctx.lock().unwrap().clone();

                match (source_str.as_str(), action_str.as_str()) {
                    // ===== 项目区空白右键 =====
                    ("item-area", "new-item") => {
                        // 触发添加项目
                        drop(w);
                        if let Some(path) = show_open_file_dialog() {
                            let order = db_for_ctx.max_item_order(class_id) + 1;
                            let item = build_item_from_path(&path, class_id, order);
                            if db_for_ctx.insert_item(&item).is_ok() {
                                let items = db_for_ctx.list_items(class_id);
                                if let Some(w) = window_for_ctx.upgrade() {
                                    w.set_items(to_item_model(&items, 0));
                                    w.set_status_text(SharedString::from(format!(
                                        "共 {} 个项目",
                                        items.len()
                                    )));
                                }
                            }
                        }
                    }
                    ("item-area", "batch-mode") => {
                        w.set_batch_mode(true);
                        w.set_context_menu_visible(false);
                        log::info!("进入批量操作模式");
                    }
                    ("item-area", "lock-item-order") => {
                        // 切换当前分类的项目锁定
                        let locked = db_for_ctx.is_classification_locked(class_id);
                        let _ = db_for_ctx.set_classification_locked(class_id, !locked);
                        show_info_dialog(
                            "锁定状态",
                            if !locked { "已锁定项目顺序" } else { "已解锁项目顺序" },
                        );
                    }
                    ("item-area", "item-settings") => {
                        show_info_dialog("项目设置", "项目设置弹窗（待实现）");
                    }
                    // ===== 项目排序 =====
                    ("item-area", "sort-default") => {
                        let _ = db_for_ctx.set_classification_item_sort(class_id, "default");
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                            w.set_context_menu_visible(false);
                        }
                    }
                    ("item-area", "sort-initial") => {
                        let _ = db_for_ctx.set_classification_item_sort(class_id, "initial");
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                            w.set_context_menu_visible(false);
                        }
                    }
                    ("item-area", "sort-open-number") => {
                        let _ = db_for_ctx.set_classification_item_sort(class_id, "openNumber");
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                            w.set_context_menu_visible(false);
                        }
                    }
                    ("item-area", "sort-last-open") => {
                        let _ = db_for_ctx.set_classification_item_sort(class_id, "lastOpen");
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                            w.set_context_menu_visible(false);
                        }
                    }

                    // ===== 分类区空白右键 =====
                    ("classification-area", "new-classification") => {
                        drop(w);
                        if let Some(name) = show_input_dialog("新建分类", "请输入分类名称") {
                            if let Ok(id) = db_for_ctx.insert_parent_classification(&name) {
                                let classifications = db_for_ctx.list_all_classifications();
                                expanded_for_ctx.lock().unwrap().insert(id);
                                if let Some(w) = window_for_ctx.upgrade() {
                                    w.set_classifications(to_classification_model(&classifications, id, &expanded_for_ctx.lock().unwrap()));
                                    w.set_items(to_item_model(&[], 0));
                                    w.set_status_text(SharedString::from("0 个项目"));
                                }
                            }
                        }
                    }
                    ("classification-area", "lock-order") => {
                        // 锁定所有分类顺序（简化: 切换第一个分类的 locked 作为全局标识）
                        show_info_dialog("锁定分类顺序", "分类顺序锁定功能（待完善）");
                    }

                    // ===== 分类项右键 =====
                    ("classification-item", "rename") => {
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        if let Some(new_name) = show_input_dialog("重命名分类", "请输入新名称") {
                            let _ = db_for_ctx.rename_classification(cid, &new_name);
                            let classifications = db_for_ctx.list_all_classifications();
                            if let Some(w) = window_for_ctx.upgrade() {
                                w.set_classifications(to_classification_model(&classifications, cid, &expanded_for_ctx.lock().unwrap()));
                            }
                        }
                    }
                    ("classification-item", "add-child") => {
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        if let Some(name) = show_input_dialog("新建子分类", "请输入子分类名称") {
                            if let Ok(_id) = db_for_ctx.insert_child_classification(cid, &name) {
                                // 确保父分类展开以显示新子分类
                                expanded_for_ctx.lock().unwrap().insert(cid);
                                let classifications = db_for_ctx.list_all_classifications();
                                if let Some(w) = window_for_ctx.upgrade() {
                                    w.set_classifications(to_classification_model(&classifications, cid, &expanded_for_ctx.lock().unwrap()));
                                }
                                show_info_dialog("成功", "子分类已创建");
                            }
                        }
                    }
                    ("classification-item", "delete") => {
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        let confirm = show_confirm_dialog(
                            "删除分类",
                            "删除分类将同时删除其下所有项目，确定继续吗？",
                        );
                        if confirm {
                            let _ = db_for_ctx.delete_classification(cid);
                            let classifications = db_for_ctx.list_all_classifications();
                            let first_id = classifications.first().map(|c| c.id).unwrap_or(0);
                            let items = db_for_ctx.list_items(first_id);
                            if let Some(w) = window_for_ctx.upgrade() {
                                w.set_classifications(to_classification_model(&classifications, first_id, &expanded_for_ctx.lock().unwrap()));
                                w.set_items(to_item_model(&items, 0));
                                w.set_status_text(SharedString::from(format!(
                                    "共 {} 个分类，{} 个项目",
                                    classifications.len(),
                                    items.len()
                                )));
                            }
                        }
                    }

                    // ===== 项目右键 =====
                    ("item", "open") => {
                        let item_id = *right_item_id.lock().unwrap();
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(item) = items.iter().find(|i| i.id == item_id) {
                            open_item(item);
                            db_for_ctx.record_item_open(item_id);
                        }
                    }
                    ("item", "open-location") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(target) = db_for_ctx.get_item_target(item_id) {
                            #[cfg(target_os = "windows")]
                            native::open_file_location(&target);
                        }
                    }
                    ("item", "rename") => {
                        let item_id = *right_item_id.lock().unwrap();
                        drop(w);
                        if let Some(new_name) = show_input_dialog("重命名项目", "请输入新名称") {
                            let _ = db_for_ctx.rename_item(item_id, &new_name);
                            let items = db_for_ctx.list_items(class_id);
                            if let Some(w) = window_for_ctx.upgrade() {
                                w.set_items(to_item_model(&items, 0));
                            }
                        }
                    }
                    ("item", "copy-path") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(target) = db_for_ctx.get_item_target(item_id) {
                            #[cfg(target_os = "windows")]
                            {
                                let _ = copy_to_clipboard(&target);
                                show_info_dialog("已复制", "路径已复制到剪贴板");
                            }
                        }
                    }
                    ("item", "refresh-icon") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(target) = db_for_ctx.get_item_target(item_id) {
                            if let Some(icon) = native::get_file_icon(&target) {
                                let _ = db_for_ctx.update_item_icon(item_id, &icon);
                                let items = db_for_ctx.list_items(class_id);
                                if let Some(w) = window_for_ctx.upgrade() {
                                    w.set_items(to_item_model(&items, 0));
                                }
                            }
                        }
                    }
                    ("item", "to-relative") => {
                        let item_id = *right_item_id.lock().unwrap();
                        let base_dir = get_data_dir_string();
                        match db_for_ctx.batch_to_relative_paths(&[item_id], &base_dir) {
                            Ok(n) => show_info_dialog("完成", &format!("已转换 {} 个项目", n)),
                            Err(e) => show_error_dialog("错误", &format!("转换失败: {}", e)),
                        }
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                        }
                    }
                    ("item", "to-absolute") => {
                        let item_id = *right_item_id.lock().unwrap();
                        let base_dir = get_data_dir_string();
                        match db_for_ctx.batch_to_absolute_paths(&[item_id], &base_dir) {
                            Ok(n) => show_info_dialog("完成", &format!("已转换 {} 个项目", n)),
                            Err(e) => show_error_dialog("错误", &format!("转换失败: {}", e)),
                        }
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                        }
                    }
                    ("item", "delete") => {
                        let item_id = *right_item_id.lock().unwrap();
                        drop(w);
                        let confirm = show_confirm_dialog("删除项目", "确定删除该项目吗？");
                        if confirm {
                            let _ = db_for_ctx.delete_item(item_id);
                            let items = db_for_ctx.list_items(class_id);
                            if let Some(w) = window_for_ctx.upgrade() {
                                w.set_items(to_item_model(&items, 0));
                                w.set_status_text(SharedString::from(format!(
                                    "共 {} 个项目",
                                    items.len()
                                )));
                            }
                        }
                    }

                    // ===== 批量操作右键 =====
                    ("batch", "select-all") => {
                        // 全选当前分类所有项目
                        let items = db_for_ctx.list_items(class_id);
                        let new_model = slint::VecModel::from(
                            items
                                .iter()
                                .map(|item| {
                                    let icon = item
                                        .data
                                        .icon
                                        .as_deref()
                                        .and_then(parse_icon_data_url)
                                        .unwrap_or_default();
                                    ItemInfo {
                                        id: item.id as i32,
                                        name: SharedString::from(item.name.as_str()),
                                        icon,
                                        invalid: false,
                                        selected: true,
                                    }
                                })
                                .collect::<Vec<_>>(),
                        );
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(ModelRc::from(Rc::new(new_model)));
                        }
                    }
                    ("batch", "batch-delete") => {
                        drop(w);
                        let items = db_for_ctx.list_items(class_id);
                        let selected_ids: Vec<i64> = get_selected_item_ids(&window_for_ctx, &items);
                        if selected_ids.is_empty() {
                            show_info_dialog("提示", "请先选择项目");
                            return;
                        }
                        let confirm = show_confirm_dialog(
                            "批量删除",
                            &format!("确定删除选中的 {} 个项目吗？", selected_ids.len()),
                        );
                        if confirm {
                            let _ = db_for_ctx.batch_delete_items(&selected_ids);
                            let items = db_for_ctx.list_items(class_id);
                            if let Some(w) = window_for_ctx.upgrade() {
                                w.set_items(to_item_model(&items, 0));
                                w.set_batch_mode(false);
                            }
                        }
                    }
                    ("batch", "batch-rel-path") => {
                        let items = db_for_ctx.list_items(class_id);
                        let selected_ids: Vec<i64> = get_selected_item_ids(&window_for_ctx, &items);
                        if selected_ids.is_empty() {
                            show_info_dialog("提示", "请先选择项目");
                            return;
                        }
                        let base_dir = get_data_dir_string();
                        match db_for_ctx.batch_to_relative_paths(&selected_ids, &base_dir) {
                            Ok(n) => show_info_dialog("完成", &format!("已转换 {} 个项目为相对路径", n)),
                            Err(e) => show_error_dialog("错误", &format!("转换失败: {}", e)),
                        }
                    }
                    ("batch", "batch-abs-path") => {
                        let items = db_for_ctx.list_items(class_id);
                        let selected_ids: Vec<i64> = get_selected_item_ids(&window_for_ctx, &items);
                        if selected_ids.is_empty() {
                            show_info_dialog("提示", "请先选择项目");
                            return;
                        }
                        let base_dir = get_data_dir_string();
                        match db_for_ctx.batch_to_absolute_paths(&selected_ids, &base_dir) {
                            Ok(n) => show_info_dialog("完成", &format!("已转换 {} 个项目为绝对路径", n)),
                            Err(e) => show_error_dialog("错误", &format!("转换失败: {}", e)),
                        }
                    }
                    ("batch", "batch-refresh-icon") => {
                        let items = db_for_ctx.list_items(class_id);
                        let selected_ids: Vec<i64> = get_selected_item_ids(&window_for_ctx, &items);
                        if selected_ids.is_empty() {
                            show_info_dialog("提示", "请先选择项目");
                            return;
                        }
                        let mut refreshed = 0;
                        for id in &selected_ids {
                            if let Some(target) = db_for_ctx.get_item_target(*id) {
                                if let Some(icon) = native::get_file_icon(&target) {
                                    let _ = db_for_ctx.update_item_icon(*id, &icon);
                                    refreshed += 1;
                                }
                            }
                        }
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                        }
                        show_info_dialog("完成", &format!("已刷新 {} 个项目图标", refreshed));
                    }
                    ("batch", "batch-move") | ("batch", "batch-copy") => {
                        // 弹出分类选择对话框（简化: 用输入框输入目标分类名）
                        let items = db_for_ctx.list_items(class_id);
                        let selected_ids: Vec<i64> = get_selected_item_ids(&window_for_ctx, &items);
                        if selected_ids.is_empty() {
                            show_info_dialog("提示", "请先选择项目");
                            return;
                        }
                        let all_classes = db_for_ctx.list_all_classifications();
                        let class_names: Vec<String> = all_classes
                            .iter()
                            .map(|c| format!("[{}] {}", c.id, c.name))
                            .collect();
                        let prompt = format!(
                            "请输入目标分类ID:\n{}",
                            class_names.join("\n")
                        );
                        drop(w);
                        if let Some(input) = show_input_dialog("选择目标分类", &prompt) {
                            if let Ok(target_id) = input.trim().parse::<i64>() {
                                let result = if action_str == "batch-move" {
                                    db_for_ctx.batch_move_items(&selected_ids, target_id)
                                } else {
                                    db_for_ctx.batch_copy_items(&selected_ids, target_id)
                                };
                                match result {
                                    Ok(_) => {
                                        let items = db_for_ctx.list_items(class_id);
                                        if let Some(w) = window_for_ctx.upgrade() {
                                            w.set_items(to_item_model(&items, 0));
                                            w.set_batch_mode(false);
                                        }
                                        show_info_dialog("完成", &format!("{} 操作成功", action_str));
                                    }
                                    Err(e) => show_error_dialog("错误", &format!("操作失败: {}", e)),
                                }
                            }
                        }
                    }
                    ("batch", "batch-cancel") => {
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_batch_mode(false);
                            // 清除选中状态
                            let items = db_for_ctx.list_items(class_id);
                            w.set_items(to_item_model(&items, 0));
                        }
                    }

                    _ => {
                        log::warn!("未处理的菜单项: source={}, action={}", source_str, action_str);
                    }
                }
            });
        }

        // 17. 应用启动时加载已保存的设置
        {
            let raw_theme = db.get_setting("theme_mode", "light");
            let theme = if raw_theme == "dark" {
                "classic-dark".to_string()
            } else {
                raw_theme
            };
            let topmost = db.get_setting_bool("window_topmost", false);
            let icon_size = db.get_setting_i64("item_icon_size", 48) as i32;
            let sidebar_w = db.get_setting_i64("sidebar_width", 140) as i32;
            let hide_name = db.get_setting_bool("hide_name", false);
            let hide_after_open = db.get_setting_bool("hide_after_open", false);
            main_window.set_theme_mode(SharedString::from(theme.clone()));
            main_window.set_window_topmost(topmost);
            main_window.set_item_icon_size(icon_size);
            main_window.set_sidebar_width(sidebar_w);
            main_window.set_hide_name(hide_name);
            main_window.set_hide_after_open(hide_after_open);
            // 同步快速搜索窗口主题
            quick_search_window.set_theme_mode(SharedString::from(theme.clone()));
            apply_theme(&main_window);
            // 启动时应用置顶
            if topmost {
                #[cfg(target_os = "windows")]
                {
                    set_window_topmost(&main_window, true);
                }
            }
            log::info!(
                "启动设置已应用: theme={}, topmost={}, icon_size={}, sidebar_width={}",
                theme, topmost, icon_size, sidebar_w
            );
        }

        Ok(Self {
            main_window,
            quick_search_window,
            db,
            current_classification_id: current_id,
            expanded_classifications,
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

/// 将窗口居中到主屏幕
/// 使用 Slint Window API 获取窗口逻辑尺寸，结合 Windows GetSystemMetrics 获取屏幕尺寸
fn center_window_on_screen<C: ComponentHandle>(window: &C) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as f32;
        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32;
        let size = window.window().size();
        let win_w = size.width as f32;
        let win_h = size.height as f32;
        let x = (screen_w - win_w) / 2.0;
        let y = (screen_h - win_h) / 3.0; // 偏上 1/3 处，更符合搜索框视觉习惯
        window.window().set_position(slint::LogicalPosition::new(x, y));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
    }
}

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

/// 将分类列表转为 Slint 模型 (包含父子层级)
/// list 应为已排序的完整分类列表（父+子）
/// expanded: 当前展开的父分类 id 集合，子分类仅在父分类展开时显示
fn to_classification_model(
    list: &[Classification],
    selected_id: i64,
    expanded: &std::collections::HashSet<i64>,
) -> ModelRc<ClassificationInfo> {
    // 计算每个分类是否有子分类
    let mut has_children_set = std::collections::HashSet::new();
    for c in list {
        if let Some(pid) = c.parent_id {
            has_children_set.insert(pid);
        }
    }

    // 构建树: 先放父分类，紧跟其子分类（仅当父分类展开时）
    let mut ordered: Vec<&Classification> = Vec::new();
    let parents: Vec<&Classification> = list.iter().filter(|c| c.parent_id.is_none()).collect();
    let children_map: std::collections::HashMap<i64, Vec<&Classification>> = {
        let mut m: std::collections::HashMap<i64, Vec<&Classification>> = std::collections::HashMap::new();
        for c in list.iter().filter(|c| c.parent_id.is_some()) {
            m.entry(c.parent_id.unwrap()).or_default().push(c);
        }
        m
    };

    for p in &parents {
        ordered.push(p);
        // 仅当父分类展开时才显示子分类
        if expanded.contains(&p.id) {
            if let Some(children) = children_map.get(&p.id) {
                for child in children {
                    ordered.push(child);
                }
            }
        }
    }

    let model = slint::VecModel::from(
        ordered
            .iter()
            .map(|c| {
                let depth = if c.parent_id.is_some() { 1 } else { 0 };
                ClassificationInfo {
                    id: c.id as i32,
                    name: SharedString::from(c.name.as_str()),
                    selected: c.id == selected_id,
                    parent_id: c.parent_id.unwrap_or(0) as i32,
                    depth,
                    has_children: has_children_set.contains(&c.id),
                    expanded: expanded.contains(&c.id),
                }
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

// ==================== 右键菜单辅助函数 ====================

/// 构建菜单项 Slint 模型
fn build_menu_items(items: &[(&str, &str, &str, bool, bool)]) -> ModelRc<MenuItem> {
    let model = slint::VecModel::from(
        items
            .iter()
            .map(|(id, label, icon, separator, has_submenu)| MenuItem {
                id: SharedString::from(*id),
                label: SharedString::from(*label),
                icon: SharedString::from(*icon),
                separator: *separator,
                has_submenu: *has_submenu,
                enabled: true,
            })
            .collect::<Vec<_>>(),
    );
    ModelRc::from(Rc::new(model))
}

/// 显示右键菜单
fn show_context_menu(
    window: &MainWindow,
    source: &str,
    x: i32,
    y: i32,
    items: &[(&str, &str, &str, bool, bool)],
) {
    window.set_context_menu_source(SharedString::from(source));
    window.set_context_menu_x(x as f32);
    window.set_context_menu_y(y as f32);
    window.set_context_menu_items(build_menu_items(items));
    window.set_context_menu_visible(true);
}

/// 显示项目右键菜单
/// 包含: 打开/打开位置/重命名/复制路径/刷新图标/转为相对路径/转为绝对路径/删除
fn show_item_context_menu(window: &MainWindow, x: i32, y: i32) {
    let items = [
        ("open", "打开", "▶", false, false),
        ("open-location", "打开文件位置", "📁", false, false),
        ("", "", "", true, false), // 分隔线
        ("rename", "重命名", "✏", false, false),
        ("copy-path", "复制路径", "📋", false, false),
        ("refresh-icon", "刷新图标", "🔄", false, false),
        ("", "", "", true, false),
        ("to-relative", "转为相对路径", "↻", false, false),
        ("to-absolute", "转为绝对路径", "↻", false, false),
        ("", "", "", true, false),
        ("delete", "删除", "🗑", false, false),
    ];
    show_context_menu(window, "item", x, y, &items);
}

/// 显示分类项右键菜单
/// 包含: 重命名/删除/(如果有子分类: 添加子分类)
fn show_classification_item_menu(window: &MainWindow, x: i32, y: i32) {
    let items = [
        ("rename", "重命名", "✏", false, false),
        ("add-child", "新建子分类", "+", false, false),
        ("", "", "", true, false),
        ("delete", "删除", "🗑", false, false),
    ];
    show_context_menu(window, "classification-item", x, y, &items);
}

/// 显示分类区空白右键菜单
/// 包含: 新建分类/锁定分类顺序
fn show_classification_area_menu(window: &MainWindow, x: i32, y: i32) {
    let items = [
        ("new-classification", "新建分类", "+", false, false),
        ("", "", "", true, false),
        ("lock-order", "锁定分类顺序", "🔒", false, false),
    ];
    show_context_menu(window, "classification-area", x, y, &items);
}

/// 显示项目区空白右键菜单（非批量模式）
/// 包含: 新建项目/项目设置/锁定项目顺序/批量操作
fn show_item_area_menu(window: &MainWindow, x: i32, y: i32) {
    let items = [
        ("new-item", "新建项目", "+", false, false),
        ("item-settings", "项目设置", "⚙", false, false),
        ("", "", "", true, false),
        ("sort-default", "排序：默认", "≡", false, false),
        ("sort-initial", "排序：按名称", "A", false, false),
        ("sort-open-number", "排序：按打开次数", "🔢", false, false),
        ("sort-last-open", "排序：按最近打开", "⏱", false, false),
        ("", "", "", true, false),
        ("lock-item-order", "锁定项目顺序", "🔒", false, false),
        ("batch-mode", "批量操作", "☰", false, false),
    ];
    show_context_menu(window, "item-area", x, y, &items);
}

/// 显示批量操作右键菜单
fn show_batch_menu(window: &MainWindow, x: i32, y: i32) {
    let items = [
        ("select-all", "全选", "☰", false, false),
        ("", "", "", true, false),
        ("batch-move", "批量移动到", "↪", false, true),
        ("batch-copy", "批量复制到", "📋", false, true),
        ("", "", "", true, false),
        ("batch-rel-path", "批量转为相对路径", "↻", false, false),
        ("batch-abs-path", "批量转为绝对路径", "↻", false, false),
        ("", "", "", true, false),
        ("batch-refresh-icon", "批量刷新图标", "🔄", false, false),
        ("", "", "", true, false),
        ("batch-delete", "批量删除", "🗑", false, false),
        ("", "", "", true, false),
        ("batch-cancel", "取消批量操作", "✕", false, false),
    ];
    show_context_menu(window, "batch", x, y, &items);
}

/// 从当前 UI 获取选中的项目 id 列表
fn get_selected_item_ids(
    window_weak: &slint::Weak<MainWindow>,
    _db_items: &[Item],
) -> Vec<i64> {
    let mut ids = Vec::new();
    if let Some(w) = window_weak.upgrade() {
        let items: Vec<ItemInfo> = w.get_items().iter().collect::<Vec<_>>();
        for it in items {
            if it.selected {
                ids.push(it.id as i64);
            }
        }
    }
    ids
}

/// 获取数据目录路径字符串（用于相对路径转换基准）
fn get_data_dir_string() -> String {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    exe_dir.to_string_lossy().to_string()
}

/// 复制文本到剪贴板 (Windows)
#[cfg(target_os = "windows")]
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    use windows::Win32::Foundation::HANDLE;
    use windows::core::w;

    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("打开剪贴板失败".into());
        }
        let _ = EmptyClipboard();

        // 分配全局内存
        let mut utf16: Vec<u16> = text.encode_utf16().collect();
        utf16.push(0); // null terminator
        let byte_len = utf16.len() * 2;
        let h_mem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        let h_mem = match h_mem {
            Ok(h) => h,
            Err(e) => {
                let _ = CloseClipboard();
                return Err(format!("GlobalAlloc 失败: {:?}", e));
            }
        };
        let ptr = GlobalLock(h_mem);
        if !ptr.is_null() {
            std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, byte_len);
            let _ = GlobalUnlock(h_mem);
        }
        let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(h_mem.0 as isize));
        let _ = CloseClipboard();
    }
    let _ = w!(""); // 避免 unused import warning
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn copy_to_clipboard(_text: &str) -> Result<(), String> {
    Ok(())
}

/// 生成时间戳字符串（用于备份文件名）
fn chrono_now_ms_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ms.to_string()
}
