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

        // 8. 项目点击 (单击选中)
        let window_for_item = main_window.as_weak();
        main_window.on_item_clicked(move |item_id| {
            if let Some(w) = window_for_item.upgrade() {
                let items: Vec<ItemInfo> = w
                    .get_items()
                    .iter()
                    .collect::<Vec<_>>();
                let mut new_items = items;
                for it in new_items.iter_mut() {
                    it.selected = it.id == item_id;
                }
                let new_model = slint::VecModel::from(new_items);
                w.set_items(ModelRc::from(Rc::new(new_model)));
            }
        });

        // 9. 项目双击 (打开)
        let db_for_open = db.clone();
        let window_for_open = main_window.as_weak();
        let current_id_for_open = current_id.clone();
        main_window.on_item_double_clicked(move |item_id| {
            let items = db_for_open.list_items(current_id_for_open.lock().unwrap().clone());
            if let Some(item) = items.iter().find(|i| i.id == item_id as i64) {
                open_item(item);
                db_for_open.record_item_open(item_id.into());
            }
            if let Some(w) = window_for_open.upgrade() {
                let _ = w.window();
            }
        });

        // 10. 项目右键菜单 (MVP: 暂未实现)
        main_window.on_item_right_clicked(move |_item_id| {
            // TODO: 完整右键菜单(打开/打开位置/编辑/删除)在后续迭代实现
        });

        // 11. 分类右键菜单 (MVP: 暂未实现)
        main_window.on_classification_right_clicked(move |_id| {
            // TODO: 完整右键菜单(添加/编辑/删除/设置图标)在后续迭代实现
        });

        // 12. 窗口关闭
        main_window.on_window_close(move || {
            std::process::exit(0);
        });

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

/// 获取数据库路径
/// 便携版: 程序同级 data/dawn-launcher.db
/// 安装版: %APPDATA%/Dawn Launcher/dawn-launcher.db
fn get_database_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let exe_dir = std::env::current_exe()?
        .parent()
        .ok_or("无法获取程序目录")?
        .to_path_buf();

    // 优先使用便携版路径 (程序同级 data/)
    let portable_path = exe_dir.join("data");
    if portable_path.exists() || exe_dir.join(".portable").exists() {
        std::fs::create_dir_all(&portable_path)?;
        return Ok(portable_path.join("dawn-launcher.db"));
    }

    // 回退到用户目录 (安装版)
    if let Some(app_data) = dirs::data_dir() {
        let app_dir = app_data.join("Dawn Launcher");
        std::fs::create_dir_all(&app_dir)?;
        return Ok(app_dir.join("dawn-launcher.db"));
    }

    // 最终回退
    std::fs::create_dir_all(&exe_dir.join("data"))?;
    Ok(exe_dir.join("data").join("dawn-launcher.db"))
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
    // data:image/png;base64,xxxx
    let (format, base64_part) = parse_data_url(data_url)?;
    let bytes = base64_decode(&base64_part)?;
    // Slint 1.18 提供的 load_from_data 会自动识别格式
    slint::Image::load_from_data(&bytes, Some(&format)).ok()
        .or_else(|| slint::Image::load_from_data(&bytes, None).ok())
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
fn open_item(item: &Item) {
    let target = match &item.data.target {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return,
    };

    log::info!("打开项目: {} ({})", item.name, target);

    // 网址直接用系统默认浏览器打开
    if item.is_url() {
        let _ = native::open_path(&target);
        return;
    }

    // 文件 / 文件夹 / Appx
    let mut cmd = std::process::Command::new(&target);
    if let Some(args) = &item.data.params {
        if !args.is_empty() {
            cmd = std::process::Command::new("cmd");
            cmd.args(["/C", "start", "", &target]);
            if let Some(start_loc) = &item.data.start_location {
                if !start_loc.is_empty() {
                    cmd.current_dir(start_loc);
                }
            }
            let _ = cmd.spawn();
            return;
        }
    }
    if let Some(start_loc) = &item.data.start_location {
        if !start_loc.is_empty() {
            cmd.current_dir(start_loc);
        }
    }
    let _ = cmd.spawn();
}
