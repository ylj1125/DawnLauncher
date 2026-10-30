// 应用控制器
// 连接 SQLite 数据层、原生模块与 Slint UI

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString};

use crate::db::Database;
use crate::models::{Classification, Item};
use crate::native;
// MainWindow, ClassificationInfo, ItemInfo, MenuItem 由 slint::include_modules!() 生成在 crate 根
use crate::{ClassificationInfo, DragHelper, ItemEditWindow, ItemInfo, MenuItem, MainWindow, QuickSearchWindow, SettingsWindow};

pub struct App {
    main_window: MainWindow,
    quick_search_window: QuickSearchWindow,
    settings_window: SettingsWindow,
    item_edit_window: ItemEditWindow,
    db: Arc<Database>,
    current_classification_id: Arc<Mutex<i64>>,
    expanded_classifications: Arc<Mutex<std::collections::HashSet<i64>>>,
    hotkey_rx: std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
    tray_icon_enabled: Arc<Mutex<bool>>,
    minimize_to_tray: Arc<Mutex<bool>>,
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

        // 3.1.1 创建设置窗口（初始隐藏）
        let settings_window = SettingsWindow::new()?;
        settings_window.window().hide().ok();

        // 3.1.2 创建项目编辑窗口（初始隐藏）
        let item_edit_window = ItemEditWindow::new()?;
        item_edit_window.window().hide().ok();
        // 应用窗口背景图与不透明度设置
        let bg_path = db.get_setting("window_bg_image", "");
        if !bg_path.is_empty() {
            match slint::Image::load_from_path(std::path::Path::new(&bg_path)) {
                Ok(img) => {
                    main_window.set_window_bg_image(img);
                    main_window.set_window_bg_visible(true);
                }
                Err(e) => log::warn!("加载窗口背景图失败: {}", e),
            }
        }
        main_window.set_window_opacity(db.get_setting_i64("window_opacity", 100) as i32);
        // 初始化边缘吸附状态
        title_drag::set_edge_dock(db.get_setting_bool("edge_dock", false));
        title_drag::set_auto_hide(db.get_setting_bool("edge_auto_hide", false));
        // 初始化鼠标钩子
        native::set_mouse_hook_enabled(db.get_setting_bool("mouse_wheel_click", false));
        // 编辑窗口当前编辑的项目 id 与待保存图标
        let editing_item_id = Arc::new(Mutex::new(0i64));
        let editing_icon: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // 3.1.2 托盘状态（运行期可切换）
        let tray_icon_enabled = Arc::new(Mutex::new(db.get_setting_bool("tray_icon", true)));
        let minimize_to_tray = Arc::new(Mutex::new(db.get_setting_bool("minimize_to_tray", false)));

        // 3.2 注册全局热键 (Ctrl+Space) 唤起快速搜索
        let (hotkey_tx, hotkey_rx) = std::sync::mpsc::channel::<()>();
        crate::native::register_global_hotkey(hotkey_tx);
        let hotkey_rx = std::sync::Arc::new(std::sync::Mutex::new(hotkey_rx));

        // 4. 加载分类列表
        let classifications = db.list_all_classifications();
        // 选中默认分类（优先使用设置中保存的默认分类，否则选第一个父分类）
        let default_id = db.get_setting_i64("default_classification_id", 0);
        let first_id = if default_id > 0
            && classifications.iter().any(|c| c.id == default_id)
        {
            default_id
        } else {
            classifications
                .iter()
                .find(|c| c.parent_id.is_none())
                .map(|c| c.id)
                .unwrap_or_else(|| classifications.first().map(|c| c.id).unwrap_or(0))
        };

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
            let class_id: i64 = id.into();
            // 立即更新选中状态，保证界面响应
            if let Some(w) = window_for_class.upgrade() {
                let classes: Vec<ClassificationInfo> =
                    w.get_classifications().iter().collect::<Vec<_>>();
                let mut new_classes = classes;
                for c in new_classes.iter_mut() {
                    c.selected = c.id == id;
                }
                let new_model = slint::VecModel::from(new_classes);
                w.set_classifications(ModelRc::from(Rc::new(new_model)));
                w.set_search_text(SharedString::from(""));
            }
            *current_id_for_class.lock().unwrap() = class_id;

            // 关联文件夹分类切换前先与磁盘同步，避免列表过期
            let is_folder_class = db_for_class
                .get_classification(class_id)
                .map(|c| c.kind == 1)
                .unwrap_or(false);

            if is_folder_class {
                let db_bg = db_for_class.clone();
                let win_bg = window_for_class.clone();
                std::thread::spawn(move || {
                    sync_associate_folder(&db_bg, class_id);
                    let items = db_bg.list_items(class_id);
                    let settings = db_bg.get_classification(class_id);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win_bg.upgrade() {
                            w.set_items(to_item_model(&items, 0));
                            w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                            if let Some(c) = settings {
                                apply_class_settings(&w, &c.data);
                            }
                        }
                    });
                });
            } else {
                let items = db_for_class.list_items(class_id);
                let class_data = db_for_class.get_classification(class_id).map(|c| c.data.clone());
                if let Some(w) = window_for_class.upgrade() {
                    w.set_items(to_item_model(&items, 0));
                    w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                    if let Some(data) = class_data {
                        apply_class_settings(&w, &data);
                    }
                }
            }
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

        // 7.2 注册分类 / 项目快捷键（全局生效，修改后需重启）
        {
            let mut specs: Vec<(String, String)> = Vec::new();
            for c in db.list_all_classifications() {
                if let Some(k) = c.shortcut_key.as_ref().filter(|k| !k.trim().is_empty()) {
                    specs.push((format!("class:{}", c.id), k.clone()));
                }
                for item in db.list_items(c.id) {
                    if let Some(k) = item.shortcut_key.as_ref().filter(|k| !k.trim().is_empty()) {
                        specs.push((format!("item:{}", item.id), k.clone()));
                    }
                }
            }
            if !specs.is_empty() {
                let (hk_tx, hk_rx) = std::sync::mpsc::channel::<String>();
                crate::native::register_shortcut_hotkeys(specs, hk_tx);
                let hk_rx = Arc::new(Mutex::new(hk_rx));
                let db_for_hk = db.clone();
                let window_for_hk = main_window.as_weak();
                let current_for_hk = current_id.clone();
                let expanded_for_hk = expanded_classifications.clone();
                std::thread::spawn(move || loop {
                    let action = match hk_rx.lock().unwrap().recv() {
                        Ok(a) => a,
                        Err(_) => break,
                    };
                    let db_ctx = db_for_hk.clone();
                    let win_ctx = window_for_hk.clone();
                    let cur_ctx = current_for_hk.clone();
                    let exp_ctx = expanded_for_hk.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win_ctx.upgrade() {
                            if let Some(id) = action
                                .strip_prefix("class:")
                                .and_then(|s| s.parse::<i64>().ok())
                            {
                                *cur_ctx.lock().unwrap() = id;
                                let classes = db_ctx.list_all_classifications();
                                let expanded = exp_ctx.lock().unwrap().clone();
                                w.set_classifications(to_classification_model(&classes, id, &expanded));
                                let items = db_ctx.list_items(id);
                                w.set_items(to_item_model(&items, 0));
                                w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                                if let Some(c) = db_ctx.get_classification(id) {
                                    apply_class_settings(&w, &c.data);
                                }
                                w.set_search_text(SharedString::from(""));
                            } else if let Some(id) = action
                                .strip_prefix("item:")
                                .and_then(|s| s.parse::<i64>().ok())
                            {
                                if let Some(item) = db_ctx.get_item(id) {
                                    open_item(&item);
                                    db_ctx.record_item_open(id);
                                    if w.get_hide_after_open() {
                                        w.window().hide().ok();
                                    }
                                }
                            }
                        }
                    });
                });
            }
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

        // 10.1 拖拽排序: 构造/解析拖拽数据并处理放置
        {
            let db_for_drag = db.clone();
            let main_weak_drag = main_window.as_weak();
            let current_id_for_drag = current_id.clone();
            let expanded_for_drag = expanded_classifications.clone();
            let drag_global = main_window.global::<DragHelper>();
            drag_global.on_make_item(|id| {
                slint::DataTransfer::from(SharedString::from(format!("item:{}", id)))
            });
            drag_global.on_make_class(|id| {
                slint::DataTransfer::from(SharedString::from(format!("class:{}", id)))
            });
            drag_global.on_kind_of(|data| {
                data.plain_text()
                    .ok()
                    .and_then(|text| text.split(':').next().map(|s| s.to_string()))
                    .map(SharedString::from)
                    .unwrap_or_default()
            });
            drag_global.on_id_of(|data| {
                data.plain_text()
                    .ok()
                    .and_then(|text| {
                        text.rsplit(':').next().and_then(|n| n.parse::<i32>().ok())
                    })
                    .unwrap_or(0)
            });
            drag_global.on_drop_on(move |kind, from_id, to_id| {
                if from_id <= 0 || to_id <= 0 || from_id == to_id {
                    return;
                }
                if kind.as_str() == "class" {
                    match db_for_drag.move_classification_before(from_id as i64, to_id as i64) {
                        Ok(_) => {
                            let classes = db_for_drag.list_all_classifications();
                            let cur = *current_id_for_drag.lock().unwrap();
                            if let Some(mw) = main_weak_drag.upgrade() {
                                mw.set_classifications(to_classification_model(
                                    &classes,
                                    cur,
                                    &expanded_for_drag.lock().unwrap(),
                                ));
                            }
                            log::info!("分类拖拽排序: {} -> {}", from_id, to_id);
                        }
                        Err(e) => log::warn!("分类拖拽排序失败: {}", e),
                    }
                    return;
                }
                if kind.as_str() != "item" {
                    return;
                }
                match db_for_drag.move_item_before(from_id as i64, to_id as i64) {
                    Ok(_) => {
                        let class_id = *current_id_for_drag.lock().unwrap();
                        let items = db_for_drag.list_items(class_id);
                        if let Some(mw) = main_weak_drag.upgrade() {
                            mw.set_items(to_item_model(&items, 0));
                            mw.set_status_text(SharedString::from(format!(
                                "共 {} 个项目",
                                items.len()
                            )));
                        }
                        log::info!("项目拖拽排序: {} -> {}", from_id, to_id);
                    }
                    Err(e) => log::warn!("项目拖拽排序失败: {}", e),
                }
            });
        }

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
        let db_for_class_menu = db.clone();
        main_window.on_classification_right_clicked(move |id| {
            *right_clicked_class_id_for_menu.lock().unwrap() = id as i64;
            if let Some(w) = window_for_class_menu.upgrade() {
                let cur = db_for_class_menu.get_classification(id as i64);
                let (exclude, layout, icon_size, show_only) = match &cur {
                    Some(c) => (
                        c.data.exclude_search,
                        c.data.item_layout.clone(),
                        c.data.item_icon_size.unwrap_or(48),
                        c.data.item_show_only.clone(),
                    ),
                    None => (false, "tile".into(), 48, "default".into()),
                };
                show_classification_item_menu(
                    &w,
                    160,
                    200,
                    exclude,
                    &layout,
                    icon_size,
                    &show_only,
                );
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
            let settings_weak = settings_window.as_weak();
            let main_weak_for_settings = main_window.as_weak();
            main_window.on_settings_clicked(move || {
                let sw = match settings_weak.upgrade() {
                    Some(w) => w,
                    None => return,
                };
                // 从数据库读取所有设置值
                let raw_theme = db_for_settings.get_setting("theme_mode", "light");
                let theme = if raw_theme == "dark" { "classic-dark".to_string() } else { raw_theme };
                // 自定义主题颜色
                let custom_bg = db_for_settings.get_setting("custom_bg", "#f5f6f7");
                let custom_sidebar = db_for_settings.get_setting("custom_sidebar", "#ffffff");
                let custom_text = db_for_settings.get_setting("custom_text", "#2c2c2c");
                let custom_border = db_for_settings.get_setting("custom_border", "#e5e7eb");
                let custom_card = db_for_settings.get_setting("custom_card", "#ffffff");
                let auto_start = db_for_settings.get_setting_bool("auto_start", false);
                let topmost = db_for_settings.get_setting_bool("window_topmost", false);
                let min_to_tray = db_for_settings.get_setting_bool("minimize_to_tray", false);
                let tray_icon = db_for_settings.get_setting_bool("tray_icon", true);
                let taskbar_show = db_for_settings.get_setting_bool("taskbar_show", true);
                let follow_mouse = db_for_settings.get_setting_bool("follow_mouse", false);
                let hide_lost = db_for_settings.get_setting_bool("hide_on_lost_focus", false);
                let lock_size = db_for_settings.get_setting_bool("lock_size", false);
                let lock_pos = db_for_settings.get_setting_bool("lock_position", false);
                let edge_dock = db_for_settings.get_setting_bool("edge_dock", false);
                let edge_auto_hide =
                    db_for_settings.get_setting_bool("edge_auto_hide", false);
                let mouse_wheel_click =
                    db_for_settings.get_setting_bool("mouse_wheel_click", false);
                let always_center = db_for_settings.get_setting_bool("always_center", false);
                let columns = db_for_settings.get_setting_i64("item_columns", 8) as i32;
                let icon_size = db_for_settings.get_setting_i64("item_icon_size", 48) as i32;
                let hide_name = db_for_settings.get_setting_bool("hide_name", false);
                let hide_ellipsis = db_for_settings.get_setting_bool("hide_ellipsis", false);
                let hide_after_open = db_for_settings.get_setting_bool("hide_after_open", false);
                let record_open = db_for_settings.get_setting_bool("record_open_count", true);
                let show_tooltip = db_for_settings.get_setting_bool("show_tooltip", true);
                let show_path = db_for_settings.get_setting_bool("show_path", false);
                let sidebar_w = db_for_settings.get_setting_i64("sidebar_width", 140) as i32;
                let sidebar_pos = db_for_settings.get_setting("sidebar_position", "left");
                let hover_switch = db_for_settings.get_setting_bool("hover_switch", false);
                let wheel_switch = db_for_settings.get_setting_bool("wheel_switch", true);
                let remember_sel = db_for_settings.get_setting_bool("remember_selected", true);
                let qs_enabled = db_for_settings.get_setting_bool("quick_search_enabled", false);
                let qs_width = db_for_settings.get_setting_i64("quick_search_width", 600) as i32;
                let qs_pos = db_for_settings.get_setting("quick_search_position", "center");
                let qs_hide_lost = db_for_settings.get_setting_bool("quick_search_hide_on_lost_focus", true);
                let qs_auto_open = db_for_settings.get_setting_bool("quick_search_auto_open", true);
                let qs_hide_after = db_for_settings.get_setting_bool("quick_search_hide_after_open", true);
                let qs_history = db_for_settings.get_setting_bool("quick_search_history", true);
                let qs_match_remark = db_for_settings.get_setting_bool("quick_search_match_remark", false);
                let item_layout = db_for_settings.get_setting("item_layout", "tile");
                let win_opacity = db_for_settings.get_setting_i64("window_opacity", 100) as i32;
                let has_bg =
                    !db_for_settings.get_setting("window_bg_image", "").is_empty();

                sw.set_theme_mode(SharedString::from(theme));
                sw.set_auto_start(auto_start);
                sw.set_window_topmost(topmost);
                sw.set_minimize_to_tray(min_to_tray);
                sw.set_tray_icon(tray_icon);
                sw.set_taskbar_show(taskbar_show);
                sw.set_follow_mouse(follow_mouse);
                sw.set_hide_on_lost_focus(hide_lost);
                sw.set_lock_size(lock_size);
                sw.set_lock_position(lock_pos);
                sw.set_edge_dock(edge_dock);
                sw.set_edge_auto_hide(edge_auto_hide);
                sw.set_mouse_wheel_click(mouse_wheel_click);
                sw.set_always_center(always_center);
                sw.set_item_columns(columns);
                sw.set_item_icon_size(icon_size);
                sw.set_hide_name(hide_name);
                sw.set_hide_ellipsis(hide_ellipsis);
                sw.set_hide_after_open(hide_after_open);
                sw.set_record_open_count(record_open);
                sw.set_show_tooltip(show_tooltip);
                sw.set_show_path(show_path);
                sw.set_sidebar_width(sidebar_w);
                sw.set_sidebar_position(SharedString::from(sidebar_pos));
                sw.set_hover_switch(hover_switch);
                sw.set_wheel_switch(wheel_switch);
                sw.set_remember_selected(remember_sel);
                sw.set_quick_search_enabled(qs_enabled);
                sw.set_quick_search_width(qs_width);
                sw.set_quick_search_position(SharedString::from(qs_pos));
                sw.set_quick_search_hide_on_lost_focus(qs_hide_lost);
                sw.set_quick_search_auto_open(qs_auto_open);
                sw.set_quick_search_hide_after_open(qs_hide_after);
                sw.set_quick_search_history(qs_history);
                sw.set_quick_search_match_remark(qs_match_remark);
                sw.set_item_layout(SharedString::from(item_layout));
                sw.set_window_opacity(win_opacity);
                sw.set_has_bg_image(has_bg);

                // 加载字体列表 (后台获取，避免阻塞 UI)
                let saved_font =
                    db_for_settings.get_setting("font_family", "Microsoft YaHei UI");
                let sw_font = settings_weak.clone();
                let db_font = db_for_settings.clone();
                std::thread::spawn(move || {
                    let fonts = native::get_installed_fonts();
                    let fonts_shared: Vec<SharedString> = fonts
                        .iter()
                        .map(|f| SharedString::from(f.as_str()))
                        .collect();
                    let idx = fonts
                        .iter()
                        .position(|f| f == &saved_font)
                        .unwrap_or(0) as i32;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(sw) = sw_font.upgrade() {
                            sw.set_font_list(ModelRc::from(Rc::new(slint::VecModel::from(
                                fonts_shared,
                            ))));
                            sw.set_font_selected_index(idx);
                        }
                    });
                });
                // 让 settings window 的字体与主窗口一致
                if let Some(mw) = main_weak_for_settings.upgrade() {
                    let ff = mw.global::<crate::ThemeSettings>().get_font_family();
                    apply_font_family(&sw, &ff.to_string());
                }

                // 同步主题到设置窗口
                if let Some(mw) = main_weak_for_settings.upgrade() {
                    sw.set_theme_mode(mw.get_theme_mode());
                }

                // 设置自定义主题颜色 (输入框显示值 + ThemeSettings 全局)
                sw.set_custom_bg(SharedString::from(custom_bg.clone()));
                sw.set_custom_sidebar(SharedString::from(custom_sidebar.clone()));
                sw.set_custom_text(SharedString::from(custom_text.clone()));
                sw.set_custom_border(SharedString::from(custom_border.clone()));
                sw.set_custom_card(SharedString::from(custom_card.clone()));
                set_custom_theme_color(&sw, "bg", &custom_bg);
                set_custom_theme_color(&sw, "sidebar", &custom_sidebar);
                set_custom_theme_color(&sw, "text", &custom_text);
                set_custom_theme_color(&sw, "border", &custom_border);
                set_custom_theme_color(&sw, "card", &custom_card);

                sw.window().show().ok();
                // 定位到主窗口内部居中
                if let Some(mw) = main_weak_for_settings.upgrade() {
                    let mw_pos = mw.window().position();
                    let mw_size = mw.window().size();
                    let sw_size = sw.window().size();
                    let x = mw_pos.x as f32 + (mw_size.width as f32 - sw_size.width as f32) / 2.0;
                    let y = mw_pos.y as f32 + (mw_size.height as f32 - sw_size.height as f32) / 2.0;
                    sw.window().set_position(slint::LogicalPosition::new(x, y));
                }
                log::info!("设置窗口已打开");
            });
        }

        // 15.1 主题切换
        {
            let db_for_theme = db.clone();
            let window_for_theme = main_window.as_weak();
            let qs_for_theme = quick_search_window.as_weak();
            let sw_for_theme = settings_window.as_weak();
            settings_window.on_theme_mode_changed(move |mode| {
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
                if let Some(sw) = sw_for_theme.upgrade() {
                    sw.set_theme_mode(mode.clone());
                }
            });
        }

        // 15.0.2 自定义主题颜色变更
        {
            let db_cc = db.clone();
            let main_cc = main_window.as_weak();
            let qs_cc = quick_search_window.as_weak();
            let sw_cc = settings_window.as_weak();
            settings_window.on_custom_color_changed(move |key, val| {
                let key_str = key.to_string();
                let val_str = val.to_string();
                log::info!("自定义主题颜色变更: {}={}", key_str, val_str);
                let setting_key = format!("custom_{}", key_str);
                let _ = db_cc.set_setting(&setting_key, &val_str);
                // 同步到各窗口的 ThemeSettings 全局
                let key_clone = key_str.clone();
                let val_clone = val_str.clone();
                if let Some(w) = main_cc.upgrade() {
                    set_custom_theme_color(&w, &key_clone, &val_clone);
                }
                if let Some(qs) = qs_cc.upgrade() {
                    set_custom_theme_color(&qs, &key_clone, &val_clone);
                }
                if let Some(sw) = sw_cc.upgrade() {
                    set_custom_theme_color(&sw, &key_clone, &val_clone);
                    // 同步设置窗口自身的显示值
                    match key_str.as_str() {
                        "bg" => sw.set_custom_bg(SharedString::from(val_str.clone())),
                        "sidebar" => sw.set_custom_sidebar(SharedString::from(val_str.clone())),
                        "text" => sw.set_custom_text(SharedString::from(val_str.clone())),
                        "border" => sw.set_custom_border(SharedString::from(val_str.clone())),
                        "card" => sw.set_custom_card(SharedString::from(val_str.clone())),
                        _ => {}
                    }
                }
            });
        }

        // 15.2 开机自启切换
        {
            let db_for_autostart = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_auto_start_changed(move |enabled| {
                log::info!("开机自启切换: {}", enabled);
                let _ = db_for_autostart.set_setting_bool("auto_start", enabled);
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
                if let Some(sw) = sw_weak.upgrade() {
                    sw.set_auto_start(enabled);
                }
            });
        }

        // 15.3 窗口置顶切换
        {
            let db_for_topmost = db.clone();
            let main_weak = main_window.as_weak();
            let sw_weak = settings_window.as_weak();
            settings_window.on_window_topmost_changed(move |enabled| {
                log::info!("窗口置顶切换: {}", enabled);
                let _ = db_for_topmost.set_setting_bool("window_topmost", enabled);
                #[cfg(target_os = "windows")]
                {
                    if let Some(w) = main_weak.upgrade() {
                        set_window_topmost(&w, enabled);
                    }
                }
                if let Some(sw) = sw_weak.upgrade() {
                    sw.set_window_topmost(enabled);
                }
            });
        }

        // 15.4 启动后最小化到托盘
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            let min_to_tray_state = minimize_to_tray.clone();
            settings_window.on_minimize_to_tray_changed(move |v| {
                let _ = db_clone.set_setting_bool("minimize_to_tray", v);
                *min_to_tray_state.lock().unwrap() = v;
                if let Some(sw) = sw_weak.upgrade() { sw.set_minimize_to_tray(v); }
            });
        }

        // 15.5 托盘图标
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            let tray_state = tray_icon_enabled.clone();
            settings_window.on_tray_icon_changed(move |v| {
                let _ = db_clone.set_setting_bool("tray_icon", v);
                *tray_state.lock().unwrap() = v;
                if let Some(sw) = sw_weak.upgrade() { sw.set_tray_icon(v); }
                // 运行时创建/移除托盘图标
                #[cfg(target_os = "windows")]
                {
                    if v {
                        native::create_tray_icon("Dawn Launcher");
                    } else {
                        native::remove_tray_icon();
                    }
                }
            });
        }

        // 15.5.1 窗口不透明度
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_window_opacity_changed(move |v| {
                let val = (v as i32).clamp(30, 100);
                let _ = db_clone.set_setting_i64("window_opacity", val as i64);
                set_window_opacity(val);
                if let Some(sw) = sw_weak.upgrade() {
                    sw.set_window_opacity(val);
                }
            });
        }

        // 15.5.2 选择窗口背景图
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            let mw_weak = main_window.as_weak();
            settings_window.on_pick_bg_image(move || {
                let Some(path) = show_open_file_dialog() else {
                    return;
                };
                match slint::Image::load_from_path(std::path::Path::new(&path)) {
                    Ok(img) => {
                        let _ = db_clone.set_setting("window_bg_image", &path);
                        if let Some(w) = mw_weak.upgrade() {
                            w.set_window_bg_image(img);
                            w.set_window_bg_visible(true);
                        }
                        if let Some(sw) = sw_weak.upgrade() {
                            sw.set_has_bg_image(true);
                        }
                        log::info!("窗口背景图已设置: {}", path);
                    }
                    Err(e) => show_error_dialog("背景图", &format!("无法加载图片: {}", e)),
                }
            });
        }

        // 15.5.3 清除窗口背景图
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            let mw_weak = main_window.as_weak();
            settings_window.on_clear_bg_image(move || {
                let _ = db_clone.set_setting("window_bg_image", "");
                if let Some(w) = mw_weak.upgrade() {
                    w.set_window_bg_visible(false);
                }
                if let Some(sw) = sw_weak.upgrade() {
                    sw.set_has_bg_image(false);
                }
            });
        }

        // 15.6 任务栏显示
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_taskbar_show_changed(move |v| {
                let _ = db_clone.set_setting_bool("taskbar_show", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_taskbar_show(v); }
            });
        }

        // 15.7 显示时跟随鼠标
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_follow_mouse_changed(move |v| {
                let _ = db_clone.set_setting_bool("follow_mouse", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_follow_mouse(v); }
            });
        }

        // 15.8 失去焦点隐藏
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_hide_on_lost_focus_changed(move |v| {
                let _ = db_clone.set_setting_bool("hide_on_lost_focus", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_hide_on_lost_focus(v); }
            });
        }

        // 15.9 锁定尺寸
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_lock_size_changed(move |v| {
                let _ = db_clone.set_setting_bool("lock_size", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_lock_size(v); }
            });
        }

        // 15.10 固定位置
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_lock_position_changed(move |v| {
                let _ = db_clone.set_setting_bool("lock_position", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_lock_position(v); }
            });
        }

        // 15.10.1 边缘吸附
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_edge_dock_changed(move |v| {
                let _ = db_clone.set_setting_bool("edge_dock", v);
                title_drag::set_edge_dock(v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_edge_dock(v); }
            });
        }

        // 15.10.2 边缘自动隐藏
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_edge_auto_hide_changed(move |v| {
                let _ = db_clone.set_setting_bool("edge_auto_hide", v);
                title_drag::set_auto_hide(v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_edge_auto_hide(v); }
            });
        }

        // 15.10.3 鼠标中键单击显示/隐藏
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_mouse_wheel_click_changed(move |v| {
                let _ = db_clone.set_setting_bool("mouse_wheel_click", v);
                native::set_mouse_hook_enabled(v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_mouse_wheel_click(v); }
            });
        }

        // 15.11 永远居中
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_always_center_changed(move |v| {
                let _ = db_clone.set_setting_bool("always_center", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_always_center(v); }
            });
        }

        // 15.12 项目列数调整
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_item_columns_changed(move |cols| {
                let cols_i = cols as i32;
                let _ = db_clone.set_setting_i64("item_columns", cols_i as i64);
                if let Some(sw) = sw_weak.upgrade() { sw.set_item_columns(cols_i); }
            });
        }

        // 15.13 图标大小调整
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_item_icon_size_changed(move |size| {
                let size_i = size as i32;
                let _ = db_clone.set_setting_i64("item_icon_size", size_i as i64);
                if let Some(sw) = sw_weak.upgrade() { sw.set_item_icon_size(size_i); }
            });
        }

        // 15.13.1 字体切换
        {
            let db_clone = db.clone();
            let main_weak = main_window.as_weak();
            let qs_weak = quick_search_window.as_weak();
            let sw_weak = settings_window.as_weak();
            settings_window.on_font_changed(move |family| {
                let family_str = family.to_string();
                log::info!("字体切换: {}", family_str);
                let _ = db_clone.set_setting("font_family", &family_str);
                let ss = SharedString::from(family_str.clone());
                if let Some(w) = main_weak.upgrade() {
                    apply_font_family(&w, &family_str);
                }
                if let Some(qs) = qs_weak.upgrade() {
                    apply_font_family(&qs, &family_str);
                }
                if let Some(sw) = sw_weak.upgrade() {
                    apply_font_family(&sw, &family_str);
                    sw.set_font_selected_index(
                        sw.get_font_list().iter().position(|f| f == ss).unwrap_or(0) as i32,
                    );
                }
            });
        }

        // 15.14 隐藏名称
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_hide_name_changed(move |v| {
                let _ = db_clone.set_setting_bool("hide_name", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_hide_name(v); }
            });
        }

        // 15.15 隐藏省略号
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_hide_ellipsis_changed(move |v| {
                let _ = db_clone.set_setting_bool("hide_ellipsis", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_hide_ellipsis(v); }
            });
        }

        // 15.16 打开后隐藏
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_hide_after_open_changed(move |v| {
                let _ = db_clone.set_setting_bool("hide_after_open", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_hide_after_open(v); }
            });
        }

        // 15.17 记录打开次数
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_record_open_count_changed(move |v| {
                let _ = db_clone.set_setting_bool("record_open_count", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_record_open_count(v); }
            });
        }

        // 15.18 显示项目信息提示
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_show_tooltip_changed(move |v| {
                let _ = db_clone.set_setting_bool("show_tooltip", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_show_tooltip(v); }
            });
        }

        // 15.19 显示路径
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_show_path_changed(move |v| {
                let _ = db_clone.set_setting_bool("show_path", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_show_path(v); }
            });
        }

        // 15.20 分类栏宽度
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_sidebar_width_changed(move |w_val| {
                let w_i = w_val as i32;
                let _ = db_clone.set_setting_i64("sidebar_width", w_i as i64);
                if let Some(sw) = sw_weak.upgrade() { sw.set_sidebar_width(w_i); }
            });
        }

        // 15.21 鼠标悬停切换
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_hover_switch_changed(move |v| {
                let _ = db_clone.set_setting_bool("hover_switch", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_hover_switch(v); }
            });
        }

        // 15.22 鼠标滚轮切换
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_wheel_switch_changed(move |v| {
                let _ = db_clone.set_setting_bool("wheel_switch", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_wheel_switch(v); }
            });
        }

        // 15.23 记住选择状态
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_remember_selected_changed(move |v| {
                let _ = db_clone.set_setting_bool("remember_selected", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_remember_selected(v); }
            });
        }

        // 15.24 快速搜索启用
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_enabled_changed(move |v| {
                let _ = db_clone.set_setting_bool("quick_search_enabled", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_enabled(v); }
            });
        }

        // 15.25 快速搜索窗口宽度
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_width_changed(move |v| {
                let v_i = v as i32;
                let _ = db_clone.set_setting_i64("quick_search_width", v_i as i64);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_width(v_i); }
            });
        }

        // 15.26 快速搜索失去焦点隐藏
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_hide_on_lost_focus_changed(move |v| {
                let _ = db_clone.set_setting_bool("quick_search_hide_on_lost_focus", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_hide_on_lost_focus(v); }
            });
        }

        // 15.27 快速搜索单结果自动打开
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_auto_open_changed(move |v| {
                let _ = db_clone.set_setting_bool("quick_search_auto_open", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_auto_open(v); }
            });
        }

        // 15.28 快速搜索打开后隐藏
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_hide_after_open_changed(move |v| {
                let _ = db_clone.set_setting_bool("quick_search_hide_after_open", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_hide_after_open(v); }
            });
        }

        // 15.29 快速搜索历史记录
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_history_changed(move |v| {
                let _ = db_clone.set_setting_bool("quick_search_history", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_history(v); }
            });
        }

        // 15.30 快速搜索匹配备注
        {
            let db_clone = db.clone();
            let sw_weak = settings_window.as_weak();
            settings_window.on_quick_search_match_remark_changed(move |v| {
                let _ = db_clone.set_setting_bool("quick_search_match_remark", v);
                if let Some(sw) = sw_weak.upgrade() { sw.set_quick_search_match_remark(v); }
            });
        }

        // 15.31 设置窗口关闭
        {
            let sw_weak = settings_window.as_weak();
            settings_window.on_close_requested(move || {
                if let Some(sw) = sw_weak.upgrade() {
                    sw.window().hide().ok();
                }
            });
        }

        // 15.32 工具: 数据备份
        {
            let db_for_backup = db.clone();
            settings_window.on_backup_data(move || {
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

        // 15.33 工具: 数据还原
        {
            settings_window.on_restore_data(move || {
                if let Some(backup_file) = show_open_file_dialog() {
                    if let Ok(db_path) = get_database_path() {
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

        // 15.34 工具: 检查无效项目
        {
            let db_for_check = db.clone();
            let window_for_check = main_window.as_weak();
            let current_id_for_check = current_id.clone();
            settings_window.on_check_invalid_items(move || {
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
                    if let Some(w) = window_for_check.upgrade() {
                        w.set_items(to_item_model(&items, 0));
                    }
                } else {
                    show_info_dialog("检查完成", "所有项目均有效");
                }
            });
        }

        // 15.35 主窗口工具回调: 数据备份
        {
            main_window.on_backup_data(move || {
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

        // 15.36 主窗口工具回调: 数据还原
        {
            main_window.on_restore_data(move || {
                if let Some(backup_file) = show_open_file_dialog() {
                    if let Ok(db_path) = get_database_path() {
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

        // 15.37 主窗口工具回调: 检查无效项目
        {
            let db_for_check2 = db.clone();
            let main_weak_check = main_window.as_weak();
            let current_id_check2 = current_id.clone();
            main_window.on_check_invalid_items(move || {
                let class_id = current_id_check2.lock().unwrap().clone();
                let items = db_for_check2.list_items(class_id);
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
                    if let Some(w) = main_weak_check.upgrade() {
                        w.set_items(to_item_model(&items, 0));
                    }
                } else {
                    show_info_dialog("检查完成", "所有项目均有效");
                }
            });
        }

        // 15.38 工具: 扫描开始菜单 (导入 .lnk 应用)
        {
            let db_for_scan_menu = db.clone();
            let window_for_scan_menu = main_window.as_weak();
            let current_id_for_scan_menu = current_id.clone();
            main_window.on_scan_start_menu(move || {
                let db = db_for_scan_menu.clone();
                let win = window_for_scan_menu.clone();
                let cid = current_id_for_scan_menu.clone();
                // 扫描涉及磁盘遍历与 PowerShell，放到后台线程，避免阻塞 UI
                std::thread::spawn(move || {
                    let entries = native::scan_start_menu();
                    let class_id: i64 = *cid.lock().unwrap();
                    let (added, skipped) = import_entries(&db, class_id, entries);
                    let msg = format!(
                        "扫描开始菜单完成\n\n新增 {} 个应用\n跳过 {} 个（已存在）",
                        added, skipped
                    );
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win.upgrade() {
                            let items = db.list_items(class_id);
                            w.set_items(to_item_model(&items, 0));
                            w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                        }
                        show_info_dialog("扫描完成", &msg);
                    });
                });
            });
        }

        // 15.39 工具: 扫描商店应用 (Appx / UWP)
        {
            let db_for_scan_appx = db.clone();
            let window_for_scan_appx = main_window.as_weak();
            let current_id_for_scan_appx = current_id.clone();
            main_window.on_scan_appx(move || {
                let db = db_for_scan_appx.clone();
                let win = window_for_scan_appx.clone();
                let cid = current_id_for_scan_appx.clone();
                std::thread::spawn(move || {
                    let entries = native::scan_appx();
                    let class_id: i64 = *cid.lock().unwrap();
                    let (added, skipped) = import_entries(&db, class_id, entries);
                    let msg = if added == 0 && skipped == 0 {
                        "未扫描到商店应用（Appx）".to_string()
                    } else {
                        format!(
                            "扫描商店应用完成\n\n新增 {} 个应用\n跳过 {} 个（已存在）",
                            added, skipped
                        )
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win.upgrade() {
                            let items = db.list_items(class_id);
                            w.set_items(to_item_model(&items, 0));
                            w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                        }
                        show_info_dialog("扫描完成", &msg);
                    });
                });
            });
        }

        // 15.40 「关联文件夹」实时同步: 后台轮询文件夹结构变化，自动增删项目并刷新界面
        {
            let db_watch = db.clone();
            let win_watch = main_window.as_weak();
            let cur_watch = current_id.clone();
            std::thread::spawn(move || {
                let mut sigs: std::collections::HashMap<i64, String> =
                    std::collections::HashMap::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    let classes = db_watch.list_all_classifications();
                    for c in classes.iter().filter(|c| c.kind == 1) {
                        let path = match &c.data.associate_folder_path {
                            Some(p) if !p.is_empty() => p.clone(),
                            _ => continue,
                        };
                        let sig = match folder_signature(&path) {
                            Some(s) => s,
                            None => continue,
                        };
                        // 对比上一次的结构签名
                        match sigs.insert(c.id, sig.clone()) {
                            // 首次记录基线，不同步
                            None => continue,
                            Some(prev) if prev == sig => continue,
                            Some(_) => {}
                        }
                        // 结构发生变化 → 同步数据库
                        if !sync_associate_folder(&db_watch, c.id) {
                            continue;
                        }
                        // 若正在显示该分类，刷新界面
                        let is_current = *cur_watch.lock().unwrap() == c.id;
                        if !is_current {
                            continue;
                        }
                        let db_bg = db_watch.clone();
                        let win_bg = win_watch.clone();
                        let id = c.id;
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = win_bg.upgrade() {
                                let items = db_bg.list_items(id);
                                w.set_items(to_item_model(&items, 0));
                                w.set_status_text(SharedString::from(format!(
                                    "{} 项",
                                    items.len()
                                )));
                            }
                        });
                    }
                }
            });
        }

        // 16. 窗口关闭
        {
            let tray_enabled = tray_icon_enabled.clone();
            let min_to_tray = minimize_to_tray.clone();
            main_window.on_window_close(move || {
                let tray = *tray_enabled.lock().unwrap();
                let min = *min_to_tray.lock().unwrap();
                if tray && min {
                    // 最小化到托盘：隐藏窗口而非退出
                    #[cfg(target_os = "windows")]
                    native::show_main_window("Dawn Launcher", false);
                    log::info!("已最小化到托盘");
                } else {
                    std::process::exit(0);
                }
            });
        }

        // 16.0.1 三明治菜单动作
        {
            let main_weak = main_window.as_weak();
            main_window.on_menu_action(move |action| {
                match action.as_str() {
                    "settings" => {
                        if let Some(mw) = main_weak.upgrade() {
                            mw.invoke_settings_clicked();
                        }
                    }
                    "help" => {
                        show_info_dialog(
                            "关于",
                            "Dawn Launcher v0.7.2-20261008\n轻量级桌面快捷启动工具\n\n构建时间: 2026-10-08\n\n快捷键: Ctrl+Space 快速搜索\n\n操作:\n- 右键新建项目/分类\n- 拖拽文件添加项目\n- 分类左侧箭头折叠子分类\n- 项目区空白处右键排序",
                        );
                    }
                    "exit" => {
                        std::process::exit(0);
                    }
                    _ => {}
                }
            });
        }

        // 16.0 标题栏拖拽 (无边框窗口)
        // 采用「手动拖动」：按下记录起始点，移动时用 SetWindowPos 跟随，
        // 不进入系统模态移动循环，避免吞掉鼠标抬起事件导致关闭按钮失效。
        main_window.on_title_bar_drag_start(move || {
            #[cfg(target_os = "windows")]
            title_drag::start();
        });
        main_window.on_title_bar_drag_move(move || {
            #[cfg(target_os = "windows")]
            title_drag::move_window();
        });
        main_window.on_title_bar_drag_end(move || {
            #[cfg(target_os = "windows")]
            title_drag::end();
        });

        // 16.1 快速搜索窗口: 搜索文本变更 -> 实时过滤项目（支持网络搜索前缀）
        {
            let db_for_qs = db.clone();
            let qs_weak = quick_search_window.as_weak();
            quick_search_window.on_search_changed(move |text| {
                let query = text.to_string();
                let Some(qs) = qs_weak.upgrade() else { return };
                // 网络搜索：:g/:b/:ba/:gh/:so + 关键字
                if let Some((label, _url)) = parse_web_search_query(&query) {
                    let pseudo = vec![ItemInfo {
                        id: -1,
                        name: SharedString::from(format!("🌐 {}", label)),
                        icon: slint::Image::default(),
                        invalid: false,
                        selected: false,
                    }];
                    qs.set_search_text(text.clone());
                    qs.set_search_results(ModelRc::from(Rc::new(slint::VecModel::from(pseudo))));
                    return;
                }
                let results = db_for_qs.search_items(&query);
                qs.set_search_text(text.clone());
                qs.set_search_results(to_item_model(&results, 0));
            });
        }

        // 16.2 快速搜索窗口: 双击结果 -> 打开项目
        {
            let db_for_qs_open = db.clone();
            let qs_weak_open = quick_search_window.as_weak();
            let main_weak_open = main_window.as_weak();
            quick_search_window.on_result_double_clicked(move |item_id| {
                let id = item_id as i64;
                // 网络搜索伪结果 (id = -1)
                if id == -1 {
                    if let Some(qs) = qs_weak_open.upgrade() {
                        let text = qs.get_search_text().to_string();
                        if let Some((_, url)) = parse_web_search_query(&text) {
                            if let Err(e) = native::open_path_with_options(&url, None, None, false) {
                                log::warn!("打开网络搜索失败: {} ({})", url, e);
                            }
                        }
                        qs.window().hide().ok();
                    }
                    return;
                }
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

        // 16.2.1 快速搜索窗口: 单击结果 -> 定位到主界面对应项目
        {
            let db_for_locate = db.clone();
            let qs_weak_locate = quick_search_window.as_weak();
            let main_weak_locate = main_window.as_weak();
            let current_id_for_locate = current_id.clone();
            let expanded_for_locate = expanded_classifications.clone();
            quick_search_window.on_result_clicked(move |item_id| {
                let id = item_id as i64;
                // 网络搜索伪结果无需定位
                if id == -1 {
                    return;
                }
                let all_items = db_for_locate.list_all_items();
                let Some(item) = all_items.iter().find(|i| i.id == id) else {
                    return;
                };
                let class_id = item.classification_id;

                let classifications = db_for_locate.list_all_classifications();
                // 若目标为子分类，确保其父分类处于展开状态
                if let Some(cl) = classifications.iter().find(|c| c.id == class_id) {
                    if let Some(pid) = cl.parent_id {
                        expanded_for_locate.lock().unwrap().insert(pid);
                    }
                }
                let expanded = expanded_for_locate.lock().unwrap().clone();
                let items = db_for_locate.list_items(class_id);

                if let Some(w) = main_weak_locate.upgrade() {
                    // 切回主界面并定位到该项目所在分类，高亮目标项目
                    w.set_view_mode(SharedString::from("main"));
                    w.set_classifications(to_classification_model(&classifications, class_id, &expanded));
                    w.set_items(to_item_model(&items, id));
                    w.set_search_text(SharedString::from(""));
                    w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                    // 显示并激活主窗口
                    w.window().show().ok();
                    w.window().set_minimized(false);
                }
                *current_id_for_locate.lock().unwrap() = class_id;

                if let Some(qs) = qs_weak_locate.upgrade() {
                    qs.window().hide().ok();
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

        // 16.5 应用内搜索: 文本变更 -> 实时过滤项目
        {
            let db_for_search = db.clone();
            let main_weak_search = main_window.as_weak();
            let current_id_for_search = current_id.clone();
            main_window.on_search_text_changed(move |text| {
                let query = text.to_string();
                if let Some(w) = main_weak_search.upgrade() {
                    if query.trim().is_empty() {
                        // 清空搜索 -> 恢复当前分类项目
                        let class_id = current_id_for_search.lock().unwrap().clone();
                        let items = db_for_search.list_items(class_id);
                        w.set_items(to_item_model(&items, 0));
                        w.set_status_text(SharedString::from(format!("{} 项", items.len())));
                    } else {
                        // 搜索匹配项目（跨全部分类）
                        let results = db_for_search.search_items(&query);
                        w.set_items(to_item_model(&results, 0));
                        w.set_status_text(SharedString::from(
                            if results.is_empty() {
                                format!("未找到 \"{}\"", query)
                            } else {
                                format!("找到 {} 个匹配项目", results.len())
                            }
                        ));
                    }
                }
            });
        }

        // 16.6 应用内搜索: 回车 -> 打开第一个匹配项目
        {
            let db_for_accept = db.clone();
            let main_weak_accept = main_window.as_weak();
            main_window.on_search_accepted(move || {
                if let Some(w) = main_weak_accept.upgrade() {
                    // 网络搜索前缀优先
                    let text = w.get_search_text().to_string();
                    if let Some((_, url)) = parse_web_search_query(&text) {
                        if let Err(e) = native::open_path_with_options(&url, None, None, false) {
                            log::warn!("打开网络搜索失败: {} ({})", url, e);
                        }
                        return;
                    }
                    let items: Vec<ItemInfo> = w.get_items().iter().collect();
                    if let Some(first) = items.first() {
                        let id = first.id as i64;
                        let all_items = db_for_accept.list_all_items();
                        if let Some(item) = all_items.iter().find(|i| i.id == id) {
                            open_item(item);
                            db_for_accept.record_item_open(id);
                            // 打开后隐藏主窗口（若设置）
                            if w.get_hide_after_open() {
                                w.window().hide().ok();
                            }
                        }
                    }
                }
            });
        }

        // 16.7 应用内搜索: 清除 -> 恢复当前分类项目
        {
            let db_for_clear = db.clone();
            let main_weak_clear = main_window.as_weak();
            let current_id_for_clear = current_id.clone();
            main_window.on_search_cleared(move || {
                if let Some(w) = main_weak_clear.upgrade() {
                    let class_id = current_id_for_clear.lock().unwrap().clone();
                    let items = db_for_clear.list_items(class_id);
                    w.set_items(to_item_model(&items, 0));
                    w.set_status_text(SharedString::from(format!("{} 项", items.len())));
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
            let edit_weak_for_ctx = item_edit_window.as_weak();
            let editing_item_id_for_ctx = editing_item_id.clone();
            let editing_icon_for_ctx = editing_icon.clone();
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
                    ("item-area", "new-url") => {
                        // 新建网址: 输入地址后后台抓取标题与图标
                        drop(w);
                        if let Some(raw) =
                            show_input_dialog("新建网址", "请输入网址（如 https://www.example.com）")
                        {
                            let mut url = raw.trim().to_string();
                            if !url.is_empty() {
                                // 自动补全协议
                                if !url.starts_with("http://") && !url.starts_with("https://") {
                                    url = format!("https://{}", url);
                                }
                                let order = db_for_ctx.max_item_order(class_id) + 1;
                                let db_bg = db_for_ctx.clone();
                                let win_bg = window_for_ctx.clone();
                                // 抓取标题/图标涉及网络请求，放后台线程避免阻塞 UI
                                std::thread::spawn(move || {
                                    let (name, icon) = match native::fetch_url_info(&url) {
                                        Some(u) => {
                                            let n = if u.title.trim().is_empty() {
                                                url.clone()
                                            } else {
                                                u.title
                                            };
                                            (n, u.icon)
                                        }
                                        None => (url.clone(), None),
                                    };
                                    let item = Item {
                                        id: 0,
                                        classification_id: class_id,
                                        name,
                                        kind: 2,
                                        data: crate::models::ItemData {
                                            target: Some(url.clone()),
                                            params: None,
                                            icon,
                                            ..Default::default()
                                        },
                                        shortcut_key: None,
                                        global_shortcut_key: false,
                                        order,
                                    };
                                    let ok = db_bg.insert_item(&item).is_ok();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(w) = win_bg.upgrade() {
                                            if ok {
                                                let items = db_bg.list_items(class_id);
                                                w.set_items(to_item_model(&items, 0));
                                                w.set_status_text(SharedString::from(format!(
                                                    "共 {} 个项目",
                                                    items.len()
                                                )));
                                            }
                                        }
                                    });
                                });
                            }
                        }
                    }
                    ("item-area", "new-system") => {
                        // 新建系统项目：弹出常见系统项清单让用户选择
                        drop(w);
                        // 内置常见系统项 (名称 / target 协议)
                        let systems: &[(&str, &str)] = &[
                            ("此电脑", "shell:MyComputerFolder"),
                            ("回收站", "shell:RecycleBinFolder"),
                            ("控制面板", "shell:ControlPanelFolder"),
                            ("打印机和设备", "shell:PrintersFolder"),
                            ("网络", "shell:NetworkPlacesFolder"),
                            ("运行", "shell:RunFolder"),
                            ("Windows 设置", "ms-settings:"),
                            ("注册表编辑器", "regedit"),
                            ("设备管理器", "devmgmt.msc"),
                            ("磁盘管理", "diskmgmt.msc"),
                            ("服务", "services.msc"),
                            ("事件查看器", "eventvwr.msc"),
                            ("任务计划程序", "taskschd.msc"),
                            ("电源选项", "powercfg.cpl"),
                            ("日期和时间", "timedate.cpl"),
                            ("计算器", "calc"),
                            ("记事本", "notepad"),
                            ("命令提示符", "cmd"),
                            ("PowerShell", "powershell"),
                            ("Windows 更新", "ms-settings:windowsupdate"),
                        ];
                        let mut prompt = String::from("请输入编号选择系统项目:\n");
                        for (i, (n, _)) in systems.iter().enumerate() {
                            prompt.push_str(&format!("{}: {}\n", i + 1, n));
                        }
                        prompt.push_str("或直接输入 shell:/程序名 (手动添加)");
                        let db_ctx = db_for_ctx.clone();
                        let class_id_clone = class_id;
                        let win_ctx = window_for_ctx.clone();
                        if let Some(input) = show_input_dialog("新建系统项目", &prompt) {
                            let trimmed = input.trim();
                            let (name, target) = if let Ok(num) = trimmed.parse::<usize>() {
                                if num >= 1 && num <= systems.len() {
                                    let (n, t) = systems[num - 1];
                                    (n.to_string(), t.to_string())
                                } else {
                                    (trimmed.to_string(), trimmed.to_string())
                                }
                            } else if let Some(colon) = trimmed.find(':') {
                                // 允许 "名称=target" 格式手动输入
                                let n = trimmed[..colon].trim().to_string();
                                let t = trimmed[colon + 1..].trim().to_string();
                                (if n.is_empty() { t.clone() } else { n }, t)
                            } else {
                                (trimmed.to_string(), trimmed.to_string())
                            };
                            if !target.is_empty() {
                                let order = db_ctx.max_item_order(class_id_clone) + 1;
                                let icon = native::get_file_icon(&target);
                                let item = crate::models::Item {
                                    id: 0,
                                    classification_id: class_id_clone,
                                    name,
                                    kind: 3,
                                    data: crate::models::ItemData {
                                        target: Some(target),
                                        icon,
                                        ..Default::default()
                                    },
                                    shortcut_key: None,
                                    global_shortcut_key: false,
                                    order,
                                };
                                if db_ctx.insert_item(&item).is_ok() {
                                    let items = db_ctx.list_items(class_id_clone);
                                    if let Some(w) = win_ctx.upgrade() {
                                        w.set_items(to_item_model(&items, 0));
                                        w.set_status_text(SharedString::from(format!(
                                            "共 {} 个项目",
                                            items.len()
                                        )));
                                    }
                                }
                            }
                        }
                    }
                    ("item-area", "new-multi-item") => {
                        // 新建多项目：让用户依次输入子项路径（每行一个，可含 ;params 分隔）
                        drop(w);
                        let prompt = "新建多项目\n\n请输入名称 + 子项列表，格式:\n名称\n路径1\n路径2\n路径3...\n\n示例:\n浏览器合集\nC:\\Program Files\\Google\\Chrome\\Application\\chrome.exe\nC:\\Program Files\\Mozilla Firefox\\firefox.exe\nmsedge";
                        let db_ctx = db_for_ctx.clone();
                        let class_id_clone = class_id;
                        let win_ctx = window_for_ctx.clone();
                        if let Some(text) = show_input_dialog("新建多项目", prompt) {
                            let mut lines = text.lines();
                            let name = lines.next().unwrap_or("多项目").trim().to_string();
                            let children: Vec<crate::models::MultiItem> = lines
                                .filter(|l| !l.trim().is_empty())
                                .map(|line| {
                                    let l = line.trim();
                                    let (target, params) = if let Some(idx) = l.find(';') {
                                        (l[..idx].trim().to_string(), Some(l[idx + 1..].trim().to_string()))
                                    } else {
                                        (l.to_string(), None)
                                    };
                                    crate::models::MultiItem {
                                        target,
                                        params,
                                        start_location: None,
                                        run_as_admin: false,
                                        name: None,
                                    }
                                })
                                .collect();
                            if children.is_empty() {
                                show_info_dialog("提示", "至少需要配置一个子项");
                                return;
                            }
                            let order = db_ctx.max_item_order(class_id_clone) + 1;
                            let icon = db_ctx.get_item(class_id_clone).and_then(|_| None); // 不设图标
                            let item = crate::models::Item {
                                id: 0,
                                classification_id: class_id_clone,
                                name,
                                kind: 5,
                                data: crate::models::ItemData {
                                    multi_items: children,
                                    multi_items_time_interval: 200,
                                    icon,
                                    ..Default::default()
                                },
                                shortcut_key: None,
                                global_shortcut_key: false,
                                order,
                            };
                            if db_ctx.insert_item(&item).is_ok() {
                                let items = db_ctx.list_items(class_id_clone);
                                if let Some(w) = win_ctx.upgrade() {
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
                        // 已改为在项目右键菜单中「编辑项目」
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
                                *current_id_for_ctx.lock().unwrap() = id;
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
                    ("classification-area", "new-folder-classification") => {
                        // 新建关联文件夹分类: 选择文件夹后自动同步其内容
                        drop(w);
                        if let Some(folder) = show_pick_folder_dialog() {
                            let folder = folder.trim().to_string();
                            if !folder.is_empty() {
                                let name = std::path::Path::new(&folder)
                                    .file_name()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "文件夹".to_string());
                                if let Ok(id) = db_for_ctx
                                    .insert_associate_folder_classification(None, &name, &folder)
                                {
                                    *current_id_for_ctx.lock().unwrap() = id;
                                    let classifications = db_for_ctx.list_all_classifications();
                                    expanded_for_ctx.lock().unwrap().insert(id);
                                    if let Some(w) = window_for_ctx.upgrade() {
                                        w.set_classifications(to_classification_model(
                                            &classifications,
                                            id,
                                            &expanded_for_ctx.lock().unwrap(),
                                        ));
                                    }
                                    // 后台同步文件夹内容 (含图标提取)
                                    let db_bg = db_for_ctx.clone();
                                    let win_bg = window_for_ctx.clone();
                                    std::thread::spawn(move || {
                                        sync_associate_folder(&db_bg, id);
                                        let items = db_bg.list_items(id);
                                        let _ = slint::invoke_from_event_loop(move || {
                                            if let Some(w) = win_bg.upgrade() {
                                                w.set_items(to_item_model(&items, 0));
                                                w.set_status_text(SharedString::from(format!(
                                                    "{} 项",
                                                    items.len()
                                                )));
                                            }
                                        });
                                    });
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
                    ("classification-item", "associate-folder") => {
                        // 将分类关联到磁盘文件夹 (type=1)，随后同步内容
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        if let Some(folder) = show_pick_folder_dialog() {
                            let folder = folder.trim().to_string();
                            if !folder.is_empty() {
                                if let Err(e) =
                                    db_for_ctx.set_classification_associate_folder(cid, &folder)
                                {
                                    show_error_dialog("错误", &format!("设置关联文件夹失败: {}", e));
                                } else {
                                    *current_id_for_ctx.lock().unwrap() = cid;
                                    // 后台同步 (含图标提取，避免阻塞 UI)
                                    let db_bg = db_for_ctx.clone();
                                    let win_bg = window_for_ctx.clone();
                                    let expanded_bg = expanded_for_ctx.clone();
                                    std::thread::spawn(move || {
                                        sync_associate_folder(&db_bg, cid);
                                        let classes = db_bg.list_all_classifications();
                                        let items = db_bg.list_items(cid);
                                        let _ = slint::invoke_from_event_loop(move || {
                                            if let Some(w) = win_bg.upgrade() {
                                                w.set_classifications(to_classification_model(
                                                    &classes,
                                                    cid,
                                                    &expanded_bg.lock().unwrap(),
                                                ));
                                                w.set_items(to_item_model(&items, 0));
                                                w.set_status_text(SharedString::from(format!(
                                                    "{} 项",
                                                    items.len()
                                                )));
                                            }
                                        });
                                    });
                                    show_info_dialog(
                                        "完成",
                                        "已关联文件夹，后续文件增删将自动同步",
                                    );
                                }
                            }
                        }
                    }
                    ("classification-item", "aggregate") => {
                        // 聚合分类: 聚合所有分类下的项目，可设置排序与显示数量
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        if db_for_ctx.has_child_classification(cid) {
                            show_error_dialog("聚合分类", "当前分类是父级分类，不能设置为聚合分类。");
                        } else if let Some(choice) = show_action_dialog(
                            "聚合分类 - 排序方式",
                            &["按名称", "按打开次数", "按最近打开"],
                        ) {
                            let sort_mode = match choice.as_str() {
                                "按名称" => "initial",
                                "按打开次数" => "openNumber",
                                "按最近打开" => "lastOpen",
                                _ => "default",
                            };
                            let count_text = show_input_dialog(
                                "聚合分类",
                                "设置显示项目数量（默认 50）",
                            )
                            .unwrap_or_else(|| "50".to_string());
                            let count = count_text.trim().parse::<i64>().unwrap_or(50).max(1);
                            match db_for_ctx.set_classification_aggregate(cid, sort_mode, count) {
                                Ok(_) => {
                                    *current_id_for_ctx.lock().unwrap() = cid;
                                    let classes = db_for_ctx.list_all_classifications();
                                    let items = db_for_ctx.list_items(cid);
                                    if let Some(w) = window_for_ctx.upgrade() {
                                        w.set_classifications(to_classification_model(
                                            &classes,
                                            cid,
                                            &expanded_for_ctx.lock().unwrap(),
                                        ));
                                        w.set_items(to_item_model(&items, 0));
                                        w.set_status_text(SharedString::from(format!(
                                            "聚合 {} 个项目",
                                            items.len()
                                        )));
                                        w.set_context_menu_visible(false);
                                    }
                                }
                                Err(e) => show_error_dialog(
                                    "聚合分类",
                                    &format!("设置失败: {}", e),
                                ),
                            }
                        }
                    }
                    ("classification-item", "toggle-layout") => {
                        let cid = *right_class_id.lock().unwrap();
                        if let Some(classification) = db_for_ctx.get_classification(cid) {
                            let new_layout = if classification.data.item_layout == "list" {
                                "tile".to_string()
                            } else {
                                "list".to_string()
                            };
                            match db_for_ctx.set_classification_item_layout(cid, &new_layout) {
                                Ok(_) => {
                                    let items = db_for_ctx.list_items(cid);
                                    if let Some(w) = window_for_ctx.upgrade() {
                                        w.set_items(to_item_model(&items, 0));
                                        apply_class_settings(&w, &db_for_ctx.get_classification(cid).unwrap().data);
                                        w.set_context_menu_visible(false);
                                    }
                                    show_info_dialog("完成", &format!("布局已切换为：{}", new_layout));
                                }
                                Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                            }
                        }
                    }
                    ("classification-item", "icon-size") => {
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        let prompt = "请输入图标大小 (整数, 16~128)\n建议: 32 小 / 48 默认 / 64 大";
                        if let Some(input) = show_input_dialog("图标大小", prompt) {
                            let size = input.trim().parse::<i64>().unwrap_or(48).clamp(16, 128);
                            match db_for_ctx.set_classification_item_icon_size(cid, size) {
                                Ok(_) => {
                                    let items = db_for_ctx.list_items(cid);
                                    if let Some(w) = window_for_ctx.upgrade() {
                                        w.set_items(to_item_model(&items, 0));
                                        apply_class_settings(&w, &db_for_ctx.get_classification(cid).unwrap().data);
                                    }
                                    show_info_dialog("完成", &format!("图标大小已设为 {}", size));
                                }
                                Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                            }
                        }
                    }
                    ("classification-item", "toggle-show-only") => {
                        let cid = *right_class_id.lock().unwrap();
                        if let Some(classification) = db_for_ctx.get_classification(cid) {
                            let new_val = match classification.data.item_show_only.as_str() {
                                "default" => "file",
                                "file" => "folder",
                                _ => "default",
                            };
                            match db_for_ctx.set_classification_item_show_only(cid, new_val) {
                                Ok(_) => {
                                    let items = db_for_ctx.list_items(cid);
                                    if let Some(w) = window_for_ctx.upgrade() {
                                        w.set_items(to_item_model(&items, 0));
                                        apply_class_settings(&w, &db_for_ctx.get_classification(cid).unwrap().data);
                                        w.set_context_menu_visible(false);
                                    }
                                    let label = match new_val {
                                        "file" => "仅文件",
                                        "folder" => "仅文件夹",
                                        _ => "全部显示",
                                    };
                                    show_info_dialog("完成", &format!("显示已设为：{}", label));
                                }
                                Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                            }
                        }
                    }
                    ("classification-item", "set-shortcut") => {
                        let cid = *right_class_id.lock().unwrap();
                        let current = db_for_ctx
                            .get_classification(cid)
                            .and_then(|c| c.shortcut_key.clone())
                            .unwrap_or_default();
                        let cur_label = if current.trim().is_empty() {
                            "无".to_string()
                        } else {
                            current.clone()
                        };
                        drop(w);
                        let prompt = format!(
                            "当前快捷键：{}\n\n请输入快捷键（如 Ctrl+Alt+1），留空则清除\n修饰键: Ctrl / Alt / Shift / Win\n按键: A-Z / 0-9 / F1-F12 等",
                            cur_label
                        );
                        if let Some(input) = show_input_dialog("设置分类快捷键", &prompt) {
                            let val = input.trim().to_string();
                            let new_key = if val.is_empty() { None } else { Some(val.as_str()) };
                            match db_for_ctx.set_classification_shortcut(cid, new_key) {
                                Ok(_) => show_info_dialog("完成", "快捷键已保存，重启应用后生效"),
                                Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                            }
                        }
                    }
                    ("classification-item", "set-default") => {
                        let cid = *right_class_id.lock().unwrap();
                        match db_for_ctx.set_setting_i64("default_classification_id", cid) {
                            Ok(_) => {
                                show_info_dialog("已设置", "该分类已设为默认分类");
                                log::info!("默认分类已设为: id={}", cid);
                            }
                            Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                        }
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_context_menu_visible(false);
                        }
                    }
                    ("classification-item", "edit-icon") => {
                        let cid = *right_class_id.lock().unwrap();
                        drop(w);
                        if let Some(path) = show_open_file_dialog() {
                            match image_file_to_data_url(&path) {
                                Some(data_url) => {
                                    match db_for_ctx.set_classification_icon(cid, Some(&data_url)) {
                                        Ok(_) => {
                                            let classes = db_for_ctx.list_all_classifications();
                                            let cur = *current_id_for_ctx.lock().unwrap();
                                            if let Some(w) = window_for_ctx.upgrade() {
                                                w.set_classifications(to_classification_model(
                                                    &classes,
                                                    cur,
                                                    &expanded_for_ctx.lock().unwrap(),
                                                ));
                                            }
                                            show_info_dialog("已设置", "分类图标已更新");
                                        }
                                        Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                                    }
                                }
                                None => show_error_dialog("错误", "无法读取该图片文件"),
                            }
                        }
                    }
                    ("classification-item", "toggle-exclude-search") => {
                        let cid = *right_class_id.lock().unwrap();
                        if let Some(classification) = db_for_ctx.get_classification(cid) {
                            let new_val = !classification.data.exclude_search;
                            match db_for_ctx.set_classification_exclude_search(cid, new_val) {
                                Ok(_) => {
                                    let classes = db_for_ctx.list_all_classifications();
                                    let cur = *current_id_for_ctx.lock().unwrap();
                                    if let Some(w) = window_for_ctx.upgrade() {
                                        w.set_classifications(to_classification_model(
                                            &classes,
                                            cur,
                                            &expanded_for_ctx.lock().unwrap(),
                                        ));
                                        w.set_context_menu_visible(false);
                                    }
                                    let msg = if new_val { "已排除搜索" } else { "已取消排除搜索" };
                                    show_info_dialog("完成", msg);
                                }
                                Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
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
                    ("item", "run-as-admin") => {
                        let item_id = *right_item_id.lock().unwrap();
                        let items = db_for_ctx.list_items(class_id);
                        if let Some(item) = items.iter().find(|i| i.id == item_id) {
                            let mut admin_item = item.clone();
                            admin_item.data.run_as_admin = true;
                            open_item(&admin_item);
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
                    ("item", "explorer-menu") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(mut target) = db_for_ctx.get_item_target(item_id) {
                            // 相对路径基于程序所在目录解析
                            if target.starts_with('.') {
                                let base = get_data_dir_string();
                                target = std::path::Path::new(&base)
                                    .join(std::path::Path::new(&target))
                                    .to_string_lossy()
                                    .to_string();
                            }
                            #[cfg(target_os = "windows")]
                            {
                                if !native::explorer_context_menu_at_cursor(&target) {
                                    show_error_dialog(
                                        "无法显示菜单",
                                        "该项目不是有效的文件或文件夹路径",
                                    );
                                }
                            }
                            #[cfg(not(target_os = "windows"))]
                            {
                                let _ = &target;
                            }
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
                    ("item", "edit") => {
                        let item_id = *right_item_id.lock().unwrap();
                        *editing_item_id_for_ctx.lock().unwrap() = item_id;
                        drop(w);
                        open_item_editor(
                            &db_for_ctx,
                            &edit_weak_for_ctx,
                            &window_for_ctx,
                            item_id,
                            &editing_icon_for_ctx,
                        );
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
                    ("item", "copy-name") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(item) = db_for_ctx.get_item(item_id) {
                            #[cfg(target_os = "windows")]
                            {
                                let _ = copy_to_clipboard(&item.name);
                                show_info_dialog("已复制", "名称已复制到剪贴板");
                            }
                        }
                    }
                    ("item", "copy-remark") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(item) = db_for_ctx.get_item(item_id) {
                            let remark = item.data.remark.clone().unwrap_or_default();
                            if remark.trim().is_empty() {
                                show_info_dialog("提示", "该项目暂无备注");
                            } else {
                                #[cfg(target_os = "windows")]
                                {
                                    let _ = copy_to_clipboard(&remark);
                                    show_info_dialog("已复制", "备注已复制到剪贴板");
                                }
                            }
                        }
                    }
                    ("item", "remark-link") => {
                        let item_id = *right_item_id.lock().unwrap();
                        if let Some(item) = db_for_ctx.get_item(item_id) {
                            let remark = item.data.remark.clone().unwrap_or_default();
                            match extract_url_from_text(&remark) {
                                Some(url) => {
                                    if let Err(e) = native::open_path_with_options(&url, None, None, false) {
                                        show_error_dialog("打开失败", &format!("无法打开链接:\n{}\n\n{}", url, e));
                                    }
                                }
                                None => show_info_dialog(
                                    "提示",
                                    "备注中未找到链接（需以 http:// 或 https:// 开头）",
                                ),
                            }
                        }
                    }
                    ("item", "set-shortcut") => {
                        let item_id = *right_item_id.lock().unwrap();
                        let current = db_for_ctx
                            .get_item(item_id)
                            .and_then(|i| i.shortcut_key.clone())
                            .unwrap_or_default();
                        let cur_label = if current.trim().is_empty() {
                            "无".to_string()
                        } else {
                            current.clone()
                        };
                        drop(w);
                        let prompt = format!(
                            "当前快捷键：{}\n\n请输入快捷键（如 Ctrl+Alt+A），留空则清除\n修饰键: Ctrl / Alt / Shift / Win\n按键: A-Z / 0-9 / F1-F12 / Space / Enter 等",
                            cur_label
                        );
                        if let Some(input) = show_input_dialog("设置项目快捷键", &prompt) {
                            let val = input.trim().to_string();
                            let new_key = if val.is_empty() { None } else { Some(val.as_str()) };
                            match db_for_ctx.set_item_shortcut(item_id, new_key) {
                                Ok(_) => show_info_dialog("完成", "快捷键已保存，重启应用后生效"),
                                Err(e) => show_error_dialog("设置失败", &format!("{}", e)),
                            }
                        }
                    }
                    ("item", "move-to") | ("item", "copy-to") => {
                        let item_id = *right_item_id.lock().unwrap();
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
                                let is_move = action_str == "move-to";
                                let result = if is_move {
                                    db_for_ctx.batch_move_items(&[item_id], target_id)
                                } else {
                                    db_for_ctx.batch_copy_items(&[item_id], target_id)
                                };
                                match result {
                                    Ok(_) => {
                                        let items = db_for_ctx.list_items(class_id);
                                        if let Some(w) = window_for_ctx.upgrade() {
                                            w.set_items(to_item_model(&items, 0));
                                        }
                                        let op = if is_move { "移动" } else { "复制" };
                                        show_info_dialog("完成", &format!("{} 成功", op));
                                    }
                                    Err(e) => show_error_dialog("错误", &format!("操作失败: {}", e)),
                                }
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
                    ("batch", "batch-open") => {
                        // 批量打开: 一次性启动所有选中的项目（多项目运行）
                        let items = db_for_ctx.list_items(class_id);
                        let selected_ids: Vec<i64> = get_selected_item_ids(&window_for_ctx, &items);
                        if selected_ids.is_empty() {
                            show_info_dialog("提示", "请先选择项目");
                            return;
                        }
                        let all = db_for_ctx.list_all_items();
                        let mut opened = 0;
                        // 依次启动，间隔 150ms，避免同时启动过多进程导致卡顿
                        for id in &selected_ids {
                            if let Some(item) = all.iter().find(|i| i.id == *id) {
                                open_item(item);
                                opened += 1;
                                std::thread::sleep(std::time::Duration::from_millis(150));
                            }
                        }
                        if let Some(w) = window_for_ctx.upgrade() {
                            w.set_batch_mode(false);
                            w.set_items(to_item_model(&items, 0));
                        }
                        show_info_dialog("完成", &format!("已启动 {} 个项目", opened));
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

        // 18.1 项目编辑窗口: 保存
        {
            let db_for_edit = db.clone();
            let edit_weak_save = item_edit_window.as_weak();
            let main_weak_save = main_window.as_weak();
            let editing_id_save = editing_item_id.clone();
            let editing_icon_save = editing_icon.clone();
            item_edit_window.on_save_requested(move |name, target, params, remark, admin| {
                let item_id = *editing_id_save.lock().unwrap();
                if item_id == 0 {
                    return;
                }
                let Some(mut item) = db_for_edit.get_item(item_id) else {
                    return;
                };
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    item.name = trimmed.to_string();
                }
                let t = target.trim().to_string();
                item.data.target = if t.is_empty() { None } else { Some(t) };
                let p = params.trim().to_string();
                item.data.params = if p.is_empty() { None } else { Some(p) };
                let r = remark.trim().to_string();
                item.data.remark = if r.is_empty() { None } else { Some(r) };
                item.data.run_as_admin = admin;
                item.data.icon = editing_icon_save.lock().unwrap().clone();

                match db_for_edit.update_item(&item) {
                    Ok(_) => {
                        let items = db_for_edit.list_items(item.classification_id);
                        if let Some(mw) = main_weak_save.upgrade() {
                            mw.set_items(to_item_model(&items, 0));
                            mw.set_status_text(SharedString::from(format!(
                                "共 {} 个项目",
                                items.len()
                            )));
                        }
                        if let Some(ew) = edit_weak_save.upgrade() {
                            ew.window().hide().ok();
                        }
                        log::info!("项目已更新: id={}, name={}", item_id, item.name);
                    }
                    Err(e) => {
                        show_error_dialog("保存失败", &format!("更新项目失败: {}", e));
                    }
                }
            });
        }

        // 18.2 项目编辑窗口: 选择自定义图标
        {
            let edit_weak_pick = item_edit_window.as_weak();
            let editing_icon_pick = editing_icon.clone();
            item_edit_window.on_pick_icon_requested(move || {
                if let Some(path) = show_open_file_dialog() {
                    match image_file_to_data_url(&path) {
                        Some(url) => {
                            *editing_icon_pick.lock().unwrap() = Some(url.clone());
                            if let Some(ew) = edit_weak_pick.upgrade() {
                                ew.set_icon_preview(
                                    parse_icon_data_url(&url).unwrap_or_default(),
                                );
                            }
                        }
                        None => show_info_dialog(
                            "图标",
                            "无法读取该图片，请选择 PNG/JPG/BMP 格式的图标文件",
                        ),
                    }
                }
            });
        }

        // 18.3 项目编辑窗口: 恢复默认图标（按目标重新提取）
        {
            let edit_weak_refresh = item_edit_window.as_weak();
            let editing_icon_refresh = editing_icon.clone();
            item_edit_window.on_refresh_icon_requested(move || {
                let Some(ew) = edit_weak_refresh.upgrade() else {
                    return;
                };
                let target = ew.get_item_target().trim().to_string();
                if target.is_empty() {
                    show_info_dialog("提示", "请先填写目标路径或网址");
                    return;
                }
                let weak = edit_weak_refresh.clone();
                let icon_state = editing_icon_refresh.clone();
                std::thread::spawn(move || {
                    let icon = if target.starts_with("http://") || target.starts_with("https://") {
                        native::fetch_url_info(&target).and_then(|u| u.icon)
                    } else {
                        native::get_file_icon(&target)
                    };
                    if let Some(icon) = icon {
                        if let Ok(mut state) = icon_state.lock() {
                            *state = Some(icon.clone());
                        }
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ew) = weak.upgrade() {
                                ew.set_icon_preview(
                                    parse_icon_data_url(&icon).unwrap_or_default(),
                                );
                            }
                        });
                    }
                });
            });
        }

        // 18.4 项目编辑窗口: 取消
        {
            let edit_weak_cancel = item_edit_window.as_weak();
            item_edit_window.on_cancel_requested(move || {
                if let Some(ew) = edit_weak_cancel.upgrade() {
                    ew.window().hide().ok();
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
            let font_family = db.get_setting("font_family", "Microsoft YaHei UI");
            // 自定义主题颜色
            let custom_bg = db.get_setting("custom_bg", "#f5f6f7");
            let custom_sidebar = db.get_setting("custom_sidebar", "#ffffff");
            let custom_text = db.get_setting("custom_text", "#2c2c2c");
            let custom_border = db.get_setting("custom_border", "#e5e7eb");
            let custom_card = db.get_setting("custom_card", "#ffffff");
            main_window.set_theme_mode(SharedString::from(theme.clone()));
            main_window.set_window_topmost(topmost);
            main_window.set_item_icon_size(icon_size);
            main_window.set_sidebar_width(sidebar_w);
            main_window.set_hide_name(hide_name);
            main_window.set_hide_after_open(hide_after_open);
            // 应用字体到所有窗口
            apply_font_family(&main_window, &font_family);
            apply_font_family(&quick_search_window, &font_family);
            apply_font_family(&settings_window, &font_family);
            // 应用自定义主题颜色到所有窗口的 ThemeSettings 全局
            for (k, v) in [
                ("bg", &custom_bg),
                ("sidebar", &custom_sidebar),
                ("text", &custom_text),
                ("border", &custom_border),
                ("card", &custom_card),
            ] {
                set_custom_theme_color(&main_window, k, v);
                set_custom_theme_color(&quick_search_window, k, v);
                set_custom_theme_color(&settings_window, k, v);
            }
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
            settings_window,
            item_edit_window,
            db,
            current_classification_id: current_id,
            expanded_classifications,
            hotkey_rx,
            tray_icon_enabled,
            minimize_to_tray,
        })
    }

    /// 运行应用
    pub fn run(&self) -> Result<(), slint::PlatformError> {
        // 轮询全局热键信号，收到后触发快速搜索窗口
        let qs_weak = self.quick_search_window.as_weak();
        let main_weak = self.main_window.as_weak();
        let rx = self.hotkey_rx.clone();
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
            if let Ok(rx) = rx.try_lock() {
                if rx.try_recv().is_ok() {
                    log::info!("全局热键触发，显示快速搜索窗口");
                    if let Some(qs) = qs_weak.upgrade() {
                        qs.set_search_text(SharedString::from(""));
                        qs.set_search_results(ModelRc::from(Rc::new(slint::VecModel::default())));
                        if let Some(w) = main_weak.upgrade() {
                            qs.set_theme_mode(w.get_theme_mode());
                        }
                        qs.window().show().ok();
                        center_window_on_screen(&qs);
                    }
                }
            }
        });
        // 保持 timer 不被 drop（Slint 事件循环会持有引用）
        std::mem::forget(timer);

        // 边缘自动隐藏轮询
        let auto_hide_timer = slint::Timer::default();
        auto_hide_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(150),
            move || {
                #[cfg(target_os = "windows")]
                title_drag::poll_auto_hide();
            },
        );
        std::mem::forget(auto_hide_timer);

        // 窗口创建后应用圆角和阴影效果，并按设置初始化托盘
        let tray_enabled_run = self.tray_icon_enabled.clone();
        let min_to_tray_run = self.minimize_to_tray.clone();
        let db_run = self.db.clone();
        let frame_timer = slint::Timer::default();
        frame_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(50),
            move || {
                apply_window_frame_effects();
                // 应用窗口不透明度
                let opacity = db_run.get_setting_i64("window_opacity", 100) as i32;
                if opacity < 100 {
                    set_window_opacity(opacity);
                }
                // 启动后最小化到托盘
                let min = *min_to_tray_run.lock().unwrap();
                if min {
                    #[cfg(target_os = "windows")]
                    native::show_main_window("Dawn Launcher", false);
                }
                // 创建托盘图标
                let tray = *tray_enabled_run.lock().unwrap();
                if tray {
                    #[cfg(target_os = "windows")]
                    native::create_tray_icon("Dawn Launcher");
                }
            },
        );
        std::mem::forget(frame_timer);

        self.main_window.run()
    }
}

/// 打开项目编辑窗口并载入项目数据
fn open_item_editor(
    db: &Database,
    edit_window: &slint::Weak<ItemEditWindow>,
    main_window: &slint::Weak<MainWindow>,
    item_id: i64,
    editing_icon: &Arc<Mutex<Option<String>>>,
) {
    let Some(mw) = main_window.upgrade() else {
        return;
    };
    let Some(item) = db.get_item(item_id) else {
        log::warn!("编辑项目失败：未找到 id={}", item_id);
        return;
    };
    let Some(ew) = edit_window.upgrade() else {
        return;
    };

    *editing_icon.lock().unwrap() = item.data.icon.clone();

    ew.set_theme_mode(mw.get_theme_mode());
    // 同步字体与自定义主题颜色
    {
        let mg = mw.global::<crate::ThemeSettings>();
        let eg = ew.global::<crate::ThemeSettings>();
        eg.set_font_family(mg.get_font_family());
        eg.set_custom_bg(mg.get_custom_bg());
        eg.set_custom_sidebar(mg.get_custom_sidebar());
        eg.set_custom_text(mg.get_custom_text());
        eg.set_custom_border(mg.get_custom_border());
        eg.set_custom_card(mg.get_custom_card());
    }

    ew.set_kind_label(SharedString::from(item_kind_label(item.kind)));
    ew.set_item_name(SharedString::from(item.name.as_str()));
    ew.set_item_target(SharedString::from(
        item.data.target.clone().unwrap_or_default(),
    ));
    ew.set_item_params(SharedString::from(
        item.data.params.clone().unwrap_or_default(),
    ));
    ew.set_item_remark(SharedString::from(
        item.data.remark.clone().unwrap_or_default(),
    ));
    ew.set_run_as_admin(item.data.run_as_admin);
    ew.set_icon_preview(
        item.data
            .icon
            .as_deref()
            .and_then(parse_icon_data_url)
            .unwrap_or_default(),
    );

    ew.window().show().ok();
    center_window_on_screen(&ew);
}

/// 项目类型的中文名称
fn item_kind_label(kind: i64) -> &'static str {
    match kind {
        0 => "文件",
        1 => "文件夹",
        2 => "网址",
        3 => "系统",
        4 => "应用商店应用",
        5 => "多项目",
        _ => "项目",
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

/// 应用字体族到指定窗口的 ThemeSettings 全局
/// font-family 是 ThemeSettings 的 in-property，可通过 global 句柄设置
fn apply_font_family<W: slint::ComponentHandle>(window: &W, family: &str)
where
    for<'a> crate::ThemeSettings<'a>: slint::Global<'a, W>,
{
    let global = window.global::<crate::ThemeSettings>();
    global.set_font_family(SharedString::from(family));
}

/// 将自定义主题颜色应用到指定窗口的 ThemeSettings 全局
/// key: bg / sidebar / text / border / card
fn set_custom_theme_color<W: slint::ComponentHandle>(window: &W, key: &str, hex: &str)
where
    for<'a> crate::ThemeSettings<'a>: slint::Global<'a, W>,
{
    let global = window.global::<crate::ThemeSettings>();
    let color = parse_hex_color(hex);
    match key {
        "bg" => global.set_custom_bg(color),
        "sidebar" => global.set_custom_sidebar(color),
        "text" => global.set_custom_text(color),
        "border" => global.set_custom_border(color),
        "card" => global.set_custom_card(color),
        _ => {}
    }
}

/// 解析十六进制颜色字符串 (#RRGGBB 或 #RGB) 为 Slint Color
/// 解析失败时返回透明色
fn parse_hex_color(hex: &str) -> slint::Color {
    let hex = hex.trim().trim_start_matches('#');
    let (r, g, b) = if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        (r, g, b)
    } else if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).unwrap_or(0);
        (r, g, b)
    } else {
        (0, 0, 0)
    };
    slint::Color::from_argb_u8(255, r, g, b)
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

/// 设置主窗口不透明度 (30~100)
/// 使用 Win32 分层窗口实现整窗半透明
#[cfg(target_os = "windows")]
fn set_window_opacity(percent: i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GWL_EXSTYLE, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongW,
        WS_EX_LAYERED,
    };
    use windows::core::{HSTRING, PCWSTR};

    let title = HSTRING::from("Dawn Launcher");
    unsafe {
        let hwnd = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr()));
        if hwnd.0 as usize == 0 {
            log::warn!("未找到 Dawn Launcher 窗口，无法设置不透明度");
            return;
        }
        let percent = percent.clamp(30, 100);
        if percent >= 100 {
            // 恢复不透明: 移除分层窗口样式
            let style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(hwnd, GWL_EXSTYLE);
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, style & !(WS_EX_LAYERED.0 as i32));
        } else {
            let style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(hwnd, GWL_EXSTYLE);
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, style | (WS_EX_LAYERED.0 as i32));
            let alpha = (percent as u32 * 255 / 100) as u8;
            let _ = SetLayeredWindowAttributes(
                hwnd,
                windows::Win32::Foundation::COLORREF(0),
                alpha,
                LWA_ALPHA,
            );
        }
        log::info!("窗口不透明度: {}%", percent);
    }
}

#[cfg(not(target_os = "windows"))]
fn set_window_opacity(_percent: i32) {}

/// 标题栏手动拖动（Windows）
/// 按下时记录光标与窗口位置，移动时用 SetWindowPos 跟随光标，
/// 不使用系统模态移动循环，保证 Slint 能正常收到鼠标抬起事件。
#[cfg(target_os = "windows")]
mod title_drag {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetCursorPos, GetForegroundWindow, GetWindowRect, SetWindowPos,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };
    use windows::core::{HSTRING, PCWSTR};

    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static EDGE_DOCK: AtomicBool = AtomicBool::new(false);
    static AUTO_HIDE: AtomicBool = AtomicBool::new(false);
    static HIDDEN: AtomicBool = AtomicBool::new(false);
    static DOCK_EDGE: AtomicI32 = AtomicI32::new(0); // 0=未吸附 1=左 2=右 3=上 4=下
    static DOCK_X: AtomicI32 = AtomicI32::new(0);
    static DOCK_Y: AtomicI32 = AtomicI32::new(0);
    static START_CURSOR_X: AtomicI32 = AtomicI32::new(0);
    static START_CURSOR_Y: AtomicI32 = AtomicI32::new(0);
    static START_WIN_X: AtomicI32 = AtomicI32::new(0);
    static START_WIN_Y: AtomicI32 = AtomicI32::new(0);

    const VISIBLE_PX: i32 = 4;

    /// 设置是否启用边缘吸附
    pub fn set_edge_dock(enabled: bool) {
        EDGE_DOCK.store(enabled, Ordering::SeqCst);
        if !enabled {
            DOCK_EDGE.store(0, Ordering::SeqCst);
        }
    }

    /// 设置是否启用边缘自动隐藏
    pub fn set_auto_hide(enabled: bool) {
        AUTO_HIDE.store(enabled, Ordering::SeqCst);
        DOCK_EDGE.store(0, Ordering::SeqCst);
        if !enabled && HIDDEN.load(Ordering::SeqCst) {
            restore_window();
        }
    }

    fn find_hwnd() -> Option<HWND> {
        let title = HSTRING::from("Dawn Launcher");
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) };
        if hwnd.0 as usize != 0 {
            Some(hwnd)
        } else {
            None
        }
    }

    pub fn start() {
        let Some(hwnd) = find_hwnd() else { return };
        let mut pt = POINT { x: 0, y: 0 };
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            let _ = GetCursorPos(&mut pt);
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        START_CURSOR_X.store(pt.x, Ordering::SeqCst);
        START_CURSOR_Y.store(pt.y, Ordering::SeqCst);
        START_WIN_X.store(rect.left, Ordering::SeqCst);
        START_WIN_Y.store(rect.top, Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
    }

    pub fn move_window() {
        if !ACTIVE.load(Ordering::SeqCst) {
            return;
        }
        let Some(hwnd) = find_hwnd() else { return };
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let dx = pt.x - START_CURSOR_X.load(Ordering::SeqCst);
        let dy = pt.y - START_CURSOR_Y.load(Ordering::SeqCst);
        let nx = START_WIN_X.load(Ordering::SeqCst) + dx;
        let ny = START_WIN_Y.load(Ordering::SeqCst) + dy;
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND(0),
                nx,
                ny,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub fn end() {
        ACTIVE.store(false, Ordering::SeqCst);
        if EDGE_DOCK.load(Ordering::SeqCst) {
            snap_to_edge();
        }
    }

    /// 拖动结束后若靠近屏幕工作区边缘，则自动贴边
    fn snap_to_edge() {
        let Some(hwnd) = find_hwnd() else { return };
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return;
            }
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
                return;
            }
        }
        let work = mi.rcWork;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        const THRESHOLD: i32 = 16;

        let mut nx = rect.left;
        let mut ny = rect.top;
        if rect.left - work.left < THRESHOLD {
            nx = work.left;
        } else if work.right - rect.right < THRESHOLD {
            nx = work.right - width;
        }
        if rect.top - work.top < THRESHOLD {
            ny = work.top;
        } else if work.bottom - rect.bottom < THRESHOLD {
            ny = work.bottom - height;
        }
        if nx != rect.left || ny != rect.top {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND(0),
                    nx,
                    ny,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            log::info!("窗口边缘吸附: ({}, {})", nx, ny);
        }

        // 记录吸附边缘，供边缘自动隐藏使用
        if AUTO_HIDE.load(Ordering::SeqCst) {
            let edge = if nx == work.left {
                1
            } else if nx == work.right - width {
                2
            } else if ny == work.top {
                3
            } else if ny == work.bottom - height {
                4
            } else {
                0
            };
            DOCK_EDGE.store(edge, Ordering::SeqCst);
            DOCK_X.store(nx, Ordering::SeqCst);
            DOCK_Y.store(ny, Ordering::SeqCst);
            HIDDEN.store(false, Ordering::SeqCst);
        }
    }

    /// 将窗口移回吸附位置
    fn restore_window() {
        let Some(hwnd) = find_hwnd() else { return };
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND(0),
                DOCK_X.load(Ordering::SeqCst),
                DOCK_Y.load(Ordering::SeqCst),
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        HIDDEN.store(false, Ordering::SeqCst);
    }

    /// 将窗口移出屏幕，仅保留一条窄边
    fn hide_window(width: i32, height: i32) {
        let Some(hwnd) = find_hwnd() else { return };
        let edge = DOCK_EDGE.load(Ordering::SeqCst);
        let (x, y) = match edge {
            1 => (DOCK_X.load(Ordering::SeqCst) - width + VISIBLE_PX, DOCK_Y.load(Ordering::SeqCst)),
            2 => (DOCK_X.load(Ordering::SeqCst) + width - VISIBLE_PX, DOCK_Y.load(Ordering::SeqCst)),
            3 => (DOCK_X.load(Ordering::SeqCst), DOCK_Y.load(Ordering::SeqCst) - height + VISIBLE_PX),
            4 => (DOCK_X.load(Ordering::SeqCst), DOCK_Y.load(Ordering::SeqCst) + height - VISIBLE_PX),
            _ => return,
        };
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND(0),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        HIDDEN.store(true, Ordering::SeqCst);
        log::info!("窗口自动隐藏: edge={}", edge);
    }

    /// 轮询鼠标位置，实现吸附后的自动隐藏/唤出
    pub fn poll_auto_hide() {
        if !AUTO_HIDE.load(Ordering::SeqCst) {
            return;
        }
        let edge = DOCK_EDGE.load(Ordering::SeqCst);
        if edge == 0 {
            return;
        }
        let Some(hwnd) = find_hwnd() else { return };
        let mut pt = POINT { x: 0, y: 0 };
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            let _ = GetCursorPos(&mut pt);
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return;
            }
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        if HIDDEN.load(Ordering::SeqCst) {
            // 窗口被热键或托盘重新激活时恢复位置
            if unsafe { GetForegroundWindow() } == hwnd {
                restore_window();
                return;
            }
            // 鼠标进入屏幕边缘窄条时唤出
            let in_strip = match edge {
                1 => pt.x >= DOCK_X.load(Ordering::SeqCst)
                    && pt.x <= DOCK_X.load(Ordering::SeqCst) + VISIBLE_PX
                    && pt.y >= DOCK_Y.load(Ordering::SeqCst)
                    && pt.y <= DOCK_Y.load(Ordering::SeqCst) + height,
                2 => pt.x >= DOCK_X.load(Ordering::SeqCst) + width - VISIBLE_PX
                    && pt.x <= DOCK_X.load(Ordering::SeqCst) + width
                    && pt.y >= DOCK_Y.load(Ordering::SeqCst)
                    && pt.y <= DOCK_Y.load(Ordering::SeqCst) + height,
                3 => pt.y >= DOCK_Y.load(Ordering::SeqCst)
                    && pt.y <= DOCK_Y.load(Ordering::SeqCst) + VISIBLE_PX
                    && pt.x >= DOCK_X.load(Ordering::SeqCst)
                    && pt.x <= DOCK_X.load(Ordering::SeqCst) + width,
                4 => pt.y >= DOCK_Y.load(Ordering::SeqCst) + height - VISIBLE_PX
                    && pt.y <= DOCK_Y.load(Ordering::SeqCst) + height
                    && pt.x >= DOCK_X.load(Ordering::SeqCst)
                    && pt.x <= DOCK_X.load(Ordering::SeqCst) + width,
                _ => false,
            };
            if in_strip {
                restore_window();
            }
        } else {
            // 鼠标离开窗口后隐藏
            let inside = pt.x >= rect.left && pt.x <= rect.right && pt.y >= rect.top && pt.y <= rect.bottom;
            if !inside {
                hide_window(width, height);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod title_drag {
    pub fn start() {}
    pub fn move_window() {}
    pub fn end() {}
    pub fn set_edge_dock(_enabled: bool) {}
    pub fn set_auto_hide(_enabled: bool) {}
    pub fn poll_auto_hide() {}
}

/// 为主窗口设置圆角和阴影 (Windows DWM API)
#[cfg(target_os = "windows")]
fn apply_window_frame_effects() {
    use windows::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Controls::MARGINS;

    let title = HSTRING::from("Dawn Launcher");
    unsafe {
        let hwnd = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr()));
        if hwnd.0 as usize == 0 {
            return;
        }
        // 圆角 (Windows 11)
        let preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const _,
            std::mem::size_of_val(&preference) as u32,
        );
        // 扩展非客户区到客户区，获得阴影 (margins = -1 表示全部扩展)
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_window_frame_effects() {}

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

/// URL 查询参数编码（仅保留字母数字与 -_.~）
fn url_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 解析网络搜索触发前缀，返回 (引擎名称, 最终 URL)
/// 支持 :g 谷歌 / :b 必应 / :ba 百度 / :gh GitHub / :so StackOverflow
fn parse_web_search_query(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    if !t.starts_with(':') {
        return None;
    }
    let (prefix, keyword) = match t.find(' ') {
        Some(i) => (&t[..i], t[i + 1..].trim()),
        None => return None,
    };
    if keyword.is_empty() {
        return None;
    }
    let (label, template) = match prefix {
        ":g" => ("Google", "https://www.google.com/search?q={w}"),
        ":b" => ("Bing", "https://www.bing.com/search?q={w}"),
        ":ba" => ("百度", "https://www.baidu.com/s?wd={w}"),
        ":gh" => ("GitHub", "https://github.com/search?q={w}"),
        ":so" => ("StackOverflow", "https://stackoverflow.com/search?q={w}"),
        _ => return None,
    };
    let url = template.replace("{w}", &url_encode_component(keyword));
    Some((format!("{} 搜索: {}", label, keyword), url))
}

/// 从文本中提取第一个 http(s) 链接
fn extract_url_from_text(text: &str) -> Option<String> {
    for prefix in ["https://", "http://"] {
        if let Some(idx) = text.find(prefix) {
            let rest = &text[idx..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>')
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(|c: char| {
                c == ',' || c == '.' || c == ')' || c == ']' || c == '}' || c == '。' || c == '，'
            });
            if url.len() > prefix.len() {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// 将分类设置同步到 MainWindow 的三个 property（供 ItemCard 读取）
fn apply_class_settings(w: &MainWindow, data: &crate::models::ClassificationData) {
    let layout = match data.item_layout.as_str() {
        "list" => "list",
        _ => "tile",
    };
    w.set_current_class_layout(SharedString::from(layout));
    let icon_size = data.item_icon_size.unwrap_or(48);
    w.set_current_class_icon_size(icon_size as i32);
    let show_only = match data.item_show_only.as_str() {
        "file" | "folder" => data.item_show_only.clone(),
        _ => "default".to_string(),
    };
    w.set_current_class_show_only(SharedString::from(show_only));
}

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
                let icon = c
                    .data
                    .icon
                    .as_deref()
                    .and_then(parse_icon_data_url)
                    .unwrap_or_default();
                ClassificationInfo {
                    id: c.id as i32,
                    name: SharedString::from(c.name.as_str()),
                    selected: c.id == selected_id,
                    parent_id: c.parent_id.unwrap_or(0) as i32,
                    depth,
                    has_children: has_children_set.contains(&c.id),
                    expanded: expanded.contains(&c.id),
                    icon,
                }
            })
            .collect::<Vec<_>>(),
    );
    ModelRc::from(Rc::new(model))
}

/// 将项目列表转为 Slint 模型
fn to_item_model(list: &[Item], selected_id: i64) -> ModelRc<ItemInfo> {
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
                    selected: selected_id != 0 && item.id == selected_id,
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

/// 将本地图片文件转换为 data URL 图标
/// 仅支持 Slint 可直接渲染的位图格式
fn image_file_to_data_url(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(format!("data:{};base64,{}", mime, base64_encode(&bytes)))
}

/// 简易 Base64 编码（标准字母表 + 填充）
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
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

    // Appx (商店应用) 以 AppUserModelID 启动，不走文件路径
    if item.is_appx() {
        match native::open_appx(&target) {
            Ok(_) => log::info!("启动 Appx 成功: {}", target),
            Err(e) => {
                log::error!("启动 Appx 失败: {}, error: {}", target, e);
                #[cfg(target_os = "windows")]
                {
                    show_error_dialog("打开失败", &format!("无法启动应用:\n{}\n\n错误: {}", target, e));
                }
            }
        }
        return;
    }

    // 多项目 (kind=5)：循环打开 multi_items 子项，间隔 multi_items_time_interval 毫秒
    if item.is_multi() {
        let interval = item.data.multi_items_time_interval.max(0) as u64;
        let children = item.data.multi_items.clone();
        if children.is_empty() {
            show_info_dialog(
                "提示",
                "此多项目尚未配置子项。\n请右键 → 编辑项目 → 依次输入子项路径。",
            );
            return;
        }
        let mut opened = 0usize;
        for sub in &children {
            if sub.target.trim().is_empty() {
                continue;
            }
            let sub_opened = native::open_path_with_options(
                &sub.target,
                sub.params.as_deref(),
                sub.start_location.as_deref(),
                sub.run_as_admin,
            )
            .is_ok();
            if sub_opened {
                opened += 1;
            }
            if interval > 0 {
                std::thread::sleep(std::time::Duration::from_millis(interval));
            }
        }
        log::info!("多项目 [{}] 已打开 {}/{} 个子项", item.name, opened, children.len());
        return;
    }

    // 检查目标是否存在 (网址/系统项不检查)
    if !item.is_url() && !item.is_system() && !std::path::Path::new(&target).exists() {
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

    // 统一用 native::open_path_with_options 打开
    // 传入启动参数 / 起始位置 / 管理员运行标志（之前这三项被忽略）
    match native::open_path_with_options(
        &target,
        item.data.params.as_deref(),
        item.data.start_location.as_deref(),
        item.data.run_as_admin,
    ) {
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

/// 将扫描得到的启动项批量导入指定分类
/// 按 target 去重（忽略大小写，并与该分类内已有项目比对）
/// 返回 (新增数量, 跳过数量)
fn import_entries(
    db: &Database,
    classification_id: i64,
    entries: Vec<native::LauncherEntry>,
) -> (usize, usize) {
    if entries.is_empty() {
        return (0, 0);
    }
    // 分类内已有项目的 target 集合
    let existing = db.list_items(classification_id);
    let mut seen: std::collections::HashSet<String> = existing
        .iter()
        .filter_map(|i| i.data.target.as_deref())
        .map(|t| t.to_lowercase())
        .collect();

    let mut order = db.max_item_order(classification_id);
    let mut added = 0usize;
    let mut skipped = 0usize;
    for entry in entries {
        let key = entry.target.to_lowercase();
        if key.is_empty() || seen.contains(&key) {
            skipped += 1;
            continue;
        }
        seen.insert(key);
        order += 1;
        let item = Item {
            id: 0,
            classification_id,
            name: entry.name,
            kind: entry.kind,
            data: crate::models::ItemData {
                target: Some(entry.target),
                params: entry.params,
                icon: entry.icon,
                ..Default::default()
            },
            shortcut_key: None,
            global_shortcut_key: false,
            order,
        };
        match db.insert_item(&item) {
            Ok(_) => added += 1,
            Err(e) => {
                log::error!("导入项目失败: {}", e);
                skipped += 1;
            }
        }
    }
    (added, skipped)
}

/// 将「关联文件夹」分类与磁盘内容同步
/// 删除磁盘上已不存在的项目，插入新出现的文件/子文件夹
/// 返回是否发生了增删 (true=变化)
fn sync_associate_folder(db: &Database, classification_id: i64) -> bool {
    let path = match db.get_classification(classification_id) {
        Some(c) if c.kind == 1 => match c.data.associate_folder_path {
            Some(p) if !p.is_empty() => p,
            _ => return false,
        },
        _ => return false,
    };
    if !std::path::Path::new(&path).is_dir() {
        return false;
    }

    let entries = native::scan_folder(&path);
    let existing = db.list_items(classification_id);

    // 当前磁盘目标集合 (忽略大小写)
    let current_targets: std::collections::HashSet<String> = entries
        .iter()
        .map(|e| e.target.to_lowercase())
        .collect();
    // 已有项目目标集合
    let existing_targets: std::collections::HashSet<String> = existing
        .iter()
        .filter_map(|i| i.data.target.as_deref())
        .map(|t| t.to_lowercase())
        .collect();

    let mut changed = false;

    // 1. 删除磁盘上已不存在的项目
    for item in &existing {
        let t = match &item.data.target {
            Some(t) => t.to_lowercase(),
            None => continue,
        };
        if !current_targets.contains(&t) {
            let _ = db.delete_item(item.id);
            changed = true;
        }
    }

    // 2. 插入新出现的项目
    let mut order = db.max_item_order(classification_id);
    for entry in entries {
        let key = entry.target.to_lowercase();
        if existing_targets.contains(&key) {
            continue;
        }
        order += 1;
        let item = Item {
            id: 0,
            classification_id,
            name: entry.name,
            kind: entry.kind,
            data: crate::models::ItemData {
                target: Some(entry.target),
                params: entry.params,
                icon: entry.icon,
                ..Default::default()
            },
            shortcut_key: None,
            global_shortcut_key: false,
            order,
        };
        if db.insert_item(&item).is_ok() {
            changed = true;
        }
    }

    changed
}

/// 计算文件夹结构签名 (仅名称 + 是否目录)
/// 用于检测文件增删与重命名；忽略点文件
fn folder_signature(path: &str) -> Option<String> {
    let mut items: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(path).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.metadata().map(|m| m.is_dir()).unwrap_or(false);
        items.push(format!(
            "{}:{}",
            name.to_lowercase(),
            if is_dir { "d" } else { "f" }
        ));
    }
    items.sort();
    Some(items.join("\u{1}"))
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

/// 弹出文件夹选择对话框 (Windows)
#[cfg(target_os = "windows")]
fn show_pick_folder_dialog() -> Option<String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, IFileOpenDialog, SIGDN_FILESYSPATH,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let result = (|| -> Option<String> {
        unsafe {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            dialog.SetOptions(FOS_PICKFOLDERS | FOS_PATHMUSTEXIST).ok()?;
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
fn show_pick_folder_dialog() -> Option<String> {
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

/// 显示输入对话框 (Windows 上通过 PowerShell + WinForms 实现，控制窗口大小)
/// 返回 Some(输入内容) 表示用户确认，None 表示取消
#[cfg(target_os = "windows")]
fn show_input_dialog(title: &str, prompt: &str) -> Option<String> {
    // 使用 WinForms 创建固定大小的输入框，避免默认 InputBox 过宽
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$form = New-Object System.Windows.Forms.Form
$form.Text = '{}'
$form.Size = New-Object System.Drawing.Size(400, 210)
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'FixedDialog'
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.BackColor = '#ffffff'
$form.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9)

$label = New-Object System.Windows.Forms.Label
$label.Text = '{}'
$label.Location = New-Object System.Drawing.Point(20, 20)
$label.Size = New-Object System.Drawing.Size(345, 25)
$form.Controls.Add($label)

$textbox = New-Object System.Windows.Forms.TextBox
$textbox.Location = New-Object System.Drawing.Point(20, 52)
$textbox.Size = New-Object System.Drawing.Size(345, 25)
$form.Controls.Add($textbox)

$okBtn = New-Object System.Windows.Forms.Button
$okBtn.Text = '确定'
$okBtn.Location = New-Object System.Drawing.Point(195, 100)
$okBtn.Size = New-Object System.Drawing.Size(75, 30)
$okBtn.DialogResult = [System.Windows.Forms.DialogResult]::OK
$form.AcceptButton = $okBtn
$form.Controls.Add($okBtn)

$cancelBtn = New-Object System.Windows.Forms.Button
$cancelBtn.Text = '取消'
$cancelBtn.Location = New-Object System.Drawing.Point(285, 100)
$cancelBtn.Size = New-Object System.Drawing.Size(75, 30)
$cancelBtn.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
$form.CancelButton = $cancelBtn
$form.Controls.Add($cancelBtn)

$result = $form.ShowDialog()
if ($result -eq [System.Windows.Forms.DialogResult]::OK) {{ $textbox.Text }} else {{ '' }}
"#,
        title.replace('\'', "''"),
        prompt.replace('\'', "''")
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
                checked: false,
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
    // 计算菜单高度: 8px 内边距 + 每项高度(分隔线9px/普通项32px)
    let height: f32 = 8.0 + items.iter().map(|i| if i.3 { 9.0 } else { 32.0 }).sum::<f32>();
    window.set_context_menu_height(height);
    window.set_context_menu_items(build_menu_items(items));
    window.set_context_menu_visible(true);
}

/// 显示项目右键菜单
/// 包含: 打开/以管理员运行/打开位置/资源管理器菜单/重命名/编辑/复制名称/复制路径/移动到/复制到/刷新图标/转相对/转绝对/删除
fn show_item_context_menu(window: &MainWindow, x: i32, y: i32) {
    let items = [
        ("open", "打开", "▶", false, false),
        ("run-as-admin", "以管理员身份运行", "🛡", false, false),
        ("open-location", "打开文件位置", "📁", false, false),
        ("explorer-menu", "资源管理器菜单", "🗂", false, false),
        ("", "", "", true, false), // 分隔线
        ("rename", "重命名", "✏", false, false),
        ("edit", "编辑项目", "⚙", false, false),
        ("set-shortcut", "设置快捷键...", "⌨", false, false),
        ("copy-name", "复制名称", "🏷", false, false),
        ("copy-path", "复制路径", "📋", false, false),
        ("", "", "", true, false),
        ("move-to", "移动到...", "↪", false, false),
        ("copy-to", "复制到...", "📋", false, false),
        ("", "", "", true, false),
        ("refresh-icon", "刷新图标", "🔄", false, false),
        ("remark-link", "打开备注链接", "🔗", false, false),
        ("copy-remark", "复制备注", "📋", false, false),
        ("to-relative", "转为相对路径", "↻", false, false),
        ("to-absolute", "转为绝对路径", "↻", false, false),
        ("", "", "", true, false),
        ("delete", "删除", "🗑", false, false),
    ];
    show_context_menu(window, "item", x, y, &items);
}

/// 显示分类项右键菜单
/// 包含: 重命名/新建子分类/切换布局/图标大小/显示过滤/设为默认分类/设置图标/排除搜索/关联文件夹/聚合分类/删除
fn show_classification_item_menu(
    window: &MainWindow,
    x: i32,
    y: i32,
    exclude_search: bool,
    cur_layout: &str,
    cur_icon_size: i64,
    cur_show_only: &str,
) {
    let exclude_label = if exclude_search { "取消排除搜索" } else { "排除搜索" };
    let layout_tip = if cur_layout == "list" {
        "切换到卡片布局"
    } else {
        "切换到列表布局"
    };
    let icon_tip = format!(
        "图标大小 (当前 {})",
        if cur_icon_size == 0 { 48 } else { cur_icon_size }
    );
    let show_label = match cur_show_only {
        "file" => "显示：仅文件夹",
        "folder" => "显示：仅文件",
        _ => "显示：仅文件",
    };
    let items = [
        ("rename", "重命名", "✏", false, false),
        ("add-child", "新建子分类", "+", false, false),
        ("", "", "", true, false),
        ("toggle-layout", layout_tip, "▦", false, false),
        ("icon-size", icon_tip.as_str(), "🖼", false, false),
        ("toggle-show-only", show_label, "🗂", false, false),
        ("", "", "", true, false),
        ("set-default", "设为默认分类", "★", false, false),
        ("set-shortcut", "设置快捷键...", "⌨", false, false),
        ("edit-icon", "设置图标...", "🖼", false, false),
        ("toggle-exclude-search", exclude_label, "🚫", false, false),
        ("", "", "", true, false),
        ("associate-folder", "关联文件夹...", "📁", false, false),
        ("aggregate", "聚合分类...", "∑", false, false),
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
        ("new-folder-classification", "新建关联文件夹分类", "📁", false, false),
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
        ("new-url", "新建网址", "🔗", false, false),
        ("new-system", "新建系统项目...", "⚙", false, false),
        ("new-multi-item", "新建多项目...", "⧗", false, false),
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
        ("batch-open", "批量打开", "▶", false, false),
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
