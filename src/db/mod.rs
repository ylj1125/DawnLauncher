// 数据库访问层
// 使用 rusqlite (SQLite)，沿用原项目表结构，便于数据迁移

use rusqlite::{params, Connection};
use std::sync::Mutex;

use crate::models::{Classification, ClassificationData, Item, ItemData};

/// 全局数据库连接 (Slint 的事件循环是单线程的，使用 Mutex 即可)
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// 打开数据库文件
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 初始化表结构 (与原项目一致)
    pub fn init_schema(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        // classification 表
        conn.execute_batch(
            r#"CREATE TABLE IF NOT EXISTS classification (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_id INTEGER,
                name TEXT NOT NULL,
                type INTEGER NOT NULL,
                data TEXT NOT NULL,
                shortcut_key TEXT,
                global_shortcut_key INTEGER NOT NULL,
                `order` INTEGER NOT NULL
            )"#,
        )?;
        // item 表
        conn.execute_batch(
            r#"CREATE TABLE IF NOT EXISTS item (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                classification_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                type INTEGER NOT NULL,
                data TEXT NOT NULL,
                shortcut_key TEXT,
                global_shortcut_key INTEGER NOT NULL,
                `order` INTEGER NOT NULL
            )"#,
        )?;
        // setting 表 (键值对，存储应用设置)
        conn.execute_batch(
            r#"CREATE TABLE IF NOT EXISTS setting (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )"#,
        )?;
        Ok(())
    }

    /// 如果数据库为空，插入默认分类
    pub fn ensure_default_data(&self) -> Result<(), rusqlite::Error> {
        let count: i64 = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM classification",
            [],
            |row| row.get(0),
        )?;
        if count == 0 {
            let data = ClassificationData {
                item_layout: "default".into(),
                item_sort: "default".into(),
                item_show_only: "default".into(),
                ..Default::default()
            };
            let data_str = serde_json::to_string(&data).unwrap_or_default();
            self.conn.lock().unwrap().execute(
                "INSERT INTO classification (parent_id, name, type, data, shortcut_key, global_shortcut_key, `order`) VALUES (NULL, ?, 0, ?, NULL, 0, 1)",
                params!["常用", data_str],
            )?;
        }
        Ok(())
    }

    /// 查询所有父分类 (parent_id IS NULL)，按 order 升序
    pub fn list_parent_classifications(&self) -> Vec<Classification> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, parent_id, name, type, data, shortcut_key, global_shortcut_key, `order` FROM classification WHERE parent_id IS NULL ORDER BY `order` ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = stmt
            .query_map([], row_to_classification)
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default();
        rows
    }

    /// 查询某父分类的子分类
    pub fn list_child_classifications(&self, parent_id: i64) -> Vec<Classification> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, parent_id, name, type, data, shortcut_key, global_shortcut_key, `order` FROM classification WHERE parent_id = ? ORDER BY `order` ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![parent_id], row_to_classification)
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }

    /// 查询某分类下的项目，按 order 升序
    pub fn list_items(&self, classification_id: i64) -> Vec<Item> {
        let conn = self.conn.lock().unwrap();
        // 读取分类的 item_sort 设置（默认按 order 排序）
        let sort_mode: String = conn
            .query_row(
                "SELECT COALESCE(json_extract(data, '$.itemSort'), 'default') FROM classification WHERE id = ?",
                params![classification_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "default".to_string());

        let order_by = match sort_mode.as_str() {
            "initial" => "name ASC",
            "openNumber" => "COALESCE(json_extract(data, '$.openNumber'), 0) DESC",
            "lastOpen" => "COALESCE(json_extract(data, '$.lastOpen'), 0) DESC",
            _ => "`order` ASC",
        };

        let sql = format!(
            "SELECT id, classification_id, name, type, data, shortcut_key, global_shortcut_key, `order` FROM item WHERE classification_id = ? ORDER BY {}",
            order_by
        );
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![classification_id], row_to_item)
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }

    /// 快速搜索：按名称模糊匹配所有分类下的项目
    /// 使用 SQL LIKE 大小写不敏感匹配，按打开次数降序、名称升序排列
    pub fn search_items(&self, query: &str) -> Vec<Item> {
        if query.trim().is_empty() {
            return vec![];
        }
        let conn = self.conn.lock().unwrap();
        let like = format!("%{}%", query.trim());
        let mut stmt = match conn.prepare(
            "SELECT id, classification_id, name, type, data, shortcut_key, global_shortcut_key, `order` \
             FROM item WHERE name LIKE ? COLLATE NOCASE \
             ORDER BY COALESCE(json_extract(data, '$.openNumber'), 0) DESC, name ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![like], row_to_item)
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }

    /// 获取所有项目（不限分类），用于快速搜索的全量匹配
    pub fn list_all_items(&self) -> Vec<Item> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, classification_id, name, type, data, shortcut_key, global_shortcut_key, `order` FROM item ORDER BY name ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map([], row_to_item)
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }

    /// 增加项目打开次数 + 更新最后打开时间
    pub fn record_item_open(&self, item_id: i64) {
        let conn = self.conn.lock().unwrap();
        let now = chrono_now_ms();
        let _ = conn.execute(
            "UPDATE item SET data = json_set(data, '$.openNumber', json_extract(data, '$.openNumber') + 1, '$.lastOpen', ?) WHERE id = ?",
            params![now, item_id],
        );
    }

    /// 获取某分类下项目的最大 order
    pub fn max_item_order(&self, classification_id: i64) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(`order`), 0) FROM item WHERE classification_id = ?",
            params![classification_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// 获取父分类的最大 order
    pub fn max_parent_classification_order(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(`order`), 0) FROM classification WHERE parent_id IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// 插入新项目，返回新 id
    pub fn insert_item(&self, item: &Item) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let data_str = serde_json::to_string(&item.data).unwrap_or_default();
        conn.execute(
            "INSERT INTO item (classification_id, name, type, data, shortcut_key, global_shortcut_key, `order`) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                item.classification_id,
                item.name,
                item.kind,
                data_str,
                item.shortcut_key,
                item.global_shortcut_key as i64,
                item.order,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 插入新父分类，返回新 id
    pub fn insert_parent_classification(&self, name: &str) -> Result<i64, rusqlite::Error> {
        let order = self.max_parent_classification_order() + 1;
        let data = ClassificationData {
            item_layout: "default".into(),
            item_sort: "default".into(),
            item_show_only: "default".into(),
            ..Default::default()
        };
        let data_str = serde_json::to_string(&data).unwrap_or_default();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO classification (parent_id, name, type, data, shortcut_key, global_shortcut_key, `order`) VALUES (NULL, ?, 0, ?, NULL, 0, ?)",
            params![name, data_str, order],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 删除项目
    pub fn delete_item(&self, item_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM item WHERE id = ?", params![item_id])?;
        Ok(())
    }

    /// 删除分类 (同时删除其下所有项目)
    pub fn delete_classification(&self, classification_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM item WHERE classification_id = ?", params![classification_id])?;
        conn.execute("DELETE FROM classification WHERE id = ?", params![classification_id])?;
        Ok(())
    }

    /// 重命名分类
    pub fn rename_classification(&self, classification_id: i64, new_name: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE classification SET name = ? WHERE id = ?",
            params![new_name, classification_id],
        )?;
        Ok(())
    }

    /// 插入子分类，返回新 id
    pub fn insert_child_classification(&self, parent_id: i64, name: &str) -> Result<i64, rusqlite::Error> {
        let order = self.max_child_classification_order(parent_id) + 1;
        let data = ClassificationData {
            item_layout: "default".into(),
            item_sort: "default".into(),
            item_show_only: "default".into(),
            ..Default::default()
        };
        let data_str = serde_json::to_string(&data).unwrap_or_default();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO classification (parent_id, name, type, data, shortcut_key, global_shortcut_key, `order`) VALUES (?, ?, 0, ?, NULL, 0, ?)",
            params![parent_id, name, data_str, order],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 获取某父分类下子分类的最大 order
    pub fn max_child_classification_order(&self, parent_id: i64) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(`order`), 0) FROM classification WHERE parent_id = ?",
            params![parent_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// 列出所有分类（父+子），用于批量移动目标选择
    pub fn list_all_classifications(&self) -> Vec<Classification> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, parent_id, name, type, data, shortcut_key, global_shortcut_key, `order` FROM classification ORDER BY `order` ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map([], row_to_classification)
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }

    /// 批量移动项目到目标分类（修改 classification_id）
    pub fn batch_move_items(&self, item_ids: &[i64], target_class_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        // 重新分配 order，避免冲突
        let mut max_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(`order`), 0) FROM item WHERE classification_id = ?",
            params![target_class_id],
            |row| row.get(0),
        )?;
        for id in item_ids {
            max_order += 1;
            tx.execute(
                "UPDATE item SET classification_id = ?, `order` = ? WHERE id = ?",
                params![target_class_id, max_order, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 批量复制项目到目标分类（深拷贝，重新分配 id 和 order）
    pub fn batch_copy_items(&self, item_ids: &[i64], target_class_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let mut max_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(`order`), 0) FROM item WHERE classification_id = ?",
            params![target_class_id],
            |row| row.get(0),
        )?;
        for id in item_ids {
            max_order += 1;
            // 拷贝整行，只改 classification_id 和 order
            tx.execute(
                "INSERT INTO item (classification_id, name, type, data, shortcut_key, global_shortcut_key, `order`)
                 SELECT ?, name, type, data, shortcut_key, global_shortcut_key, ? FROM item WHERE id = ?",
                params![target_class_id, max_order, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 批量删除项目
    pub fn batch_delete_items(&self, item_ids: &[i64]) -> Result<(), rusqlite::Error> {
        if item_ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = vec!["?"; item_ids.len()].join(",");
        let sql = format!("DELETE FROM item WHERE id IN ({})", placeholders);
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = item_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        stmt.execute(params.as_slice())?;
        Ok(())
    }

    /// 批量转换项目路径为相对路径（相对数据库文件所在目录）
    pub fn batch_to_relative_paths(&self, item_ids: &[i64], base_dir: &str) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut updated = 0;
        for id in item_ids {
            let row: (String, String) = conn.query_row(
                "SELECT data, name FROM item WHERE id = ?",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?;
            let mut data_str = row.0;
            if let Ok(mut data) = serde_json::from_str::<crate::models::ItemData>(&data_str) {
                if let Some(ref target) = data.target.clone() {
                    if let Some(rel) = make_relative_path(target, base_dir) {
                        data.target = Some(rel);
                        data_str = serde_json::to_string(&data).unwrap_or(data_str);
                        conn.execute(
                            "UPDATE item SET data = ? WHERE id = ?",
                            params![data_str, id],
                        )?;
                        updated += 1;
                    }
                }
            }
        }
        Ok(updated)
    }

    /// 批量转换项目路径为绝对路径
    pub fn batch_to_absolute_paths(&self, item_ids: &[i64], base_dir: &str) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut updated = 0;
        for id in item_ids {
            let row: (String, String) = conn.query_row(
                "SELECT data, name FROM item WHERE id = ?",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?;
            let mut data_str = row.0;
            if let Ok(mut data) = serde_json::from_str::<crate::models::ItemData>(&data_str) {
                if let Some(ref target) = data.target.clone() {
                    if let Some(abs) = make_absolute_path(target, base_dir) {
                        data.target = Some(abs);
                        data_str = serde_json::to_string(&data).unwrap_or(data_str);
                        conn.execute(
                            "UPDATE item SET data = ? WHERE id = ?",
                            params![data_str, id],
                        )?;
                        updated += 1;
                    }
                }
            }
        }
        Ok(updated)
    }

    /// 更新项目图标（批量刷新图标用）
    pub fn update_item_icon(&self, item_id: i64, icon_data_url: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let data_str: String = conn.query_row(
            "SELECT data FROM item WHERE id = ?",
            params![item_id],
            |r| r.get(0),
        )?;
        if let Ok(mut data) = serde_json::from_str::<crate::models::ItemData>(&data_str) {
            data.icon = Some(icon_data_url.to_string());
            let new_data_str = serde_json::to_string(&data).unwrap_or(data_str);
            conn.execute(
                "UPDATE item SET data = ? WHERE id = ?",
                params![new_data_str, item_id],
            )?;
        }
        Ok(())
    }

    /// 获取项目（含 target）用于批量刷新图标
    pub fn get_item_target(&self, item_id: i64) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        let data_str: String = conn.query_row(
            "SELECT data FROM item WHERE id = ?",
            params![item_id],
            |r| r.get(0),
        )
        .ok()?;
        let data: crate::models::ItemData = serde_json::from_str(&data_str).ok()?;
        data.target
    }

    /// 更新分类的锁定状态（fixed 字段）
    pub fn set_classification_locked(&self, class_id: i64, locked: bool) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let data_str: String = conn.query_row(
            "SELECT data FROM classification WHERE id = ?",
            params![class_id],
            |r| r.get(0),
        )?;
        if let Ok(mut data) = serde_json::from_str::<ClassificationData>(&data_str) {
            data.fixed = locked;
            let new_data_str = serde_json::to_string(&data).unwrap_or(data_str);
            conn.execute(
                "UPDATE classification SET data = ? WHERE id = ?",
                params![new_data_str, class_id],
            )?;
        }
        Ok(())
    }

    /// 获取分类锁定状态
    pub fn is_classification_locked(&self, class_id: i64) -> bool {
        let conn = self.conn.lock().unwrap();
        let data_str: String = conn
            .query_row(
                "SELECT data FROM classification WHERE id = ?",
                params![class_id],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let data: ClassificationData = serde_json::from_str(&data_str).unwrap_or_default();
        data.fixed
    }

    /// 设置分类的项目排序方式
    /// sort_mode: "default" | "initial" | "openNumber" | "lastOpen"
    pub fn set_classification_item_sort(&self, class_id: i64, sort_mode: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let data_str: String = conn.query_row(
            "SELECT data FROM classification WHERE id = ?",
            params![class_id],
            |r| r.get(0),
        )?;
        if let Ok(mut data) = serde_json::from_str::<ClassificationData>(&data_str) {
            data.item_sort = sort_mode.to_string();
            let new_data_str = serde_json::to_string(&data).unwrap_or(data_str);
            conn.execute(
                "UPDATE classification SET data = ? WHERE id = ?",
                params![new_data_str, class_id],
            )?;
        }
        Ok(())
    }

    /// 重命名项目
    pub fn rename_item(&self, item_id: i64, new_name: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE item SET name = ? WHERE id = ?",
            params![new_name, item_id],
        )?;
        Ok(())
    }

    // ==================== 设置 (setting) ====================

    /// 读取设置项，不存在返回默认值
    pub fn get_setting(&self, key: &str, default: &str) -> String {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM setting WHERE key = ?",
            params![key],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| default.to_string())
    }

    /// 读取 bool 设置
    pub fn get_setting_bool(&self, key: &str, default: bool) -> bool {
        self.get_setting(key, if default { "1" } else { "0" }) == "1"
    }

    /// 读取 i64 设置
    pub fn get_setting_i64(&self, key: &str, default: i64) -> i64 {
        self.get_setting(key, &default.to_string())
            .parse()
            .unwrap_or(default)
    }

    /// 写入设置项 (upsert)
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO setting (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 写入 bool 设置
    pub fn set_setting_bool(&self, key: &str, value: bool) -> Result<(), rusqlite::Error> {
        self.set_setting(key, if value { "1" } else { "0" })
    }

    /// 写入 i64 设置
    pub fn set_setting_i64(&self, key: &str, value: i64) -> Result<(), rusqlite::Error> {
        self.set_setting(key, &value.to_string())
    }
}

fn row_to_classification(row: &rusqlite::Row) -> rusqlite::Result<Classification> {
    let data_str: String = row.get(4)?;
    let data: ClassificationData =
        serde_json::from_str(&data_str).unwrap_or_default();
    Ok(Classification {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        data,
        shortcut_key: row.get(5)?,
        global_shortcut_key: row.get(6)?,
        order: row.get(7)?,
        child_list: vec![],
    })
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<Item> {
    let data_str: String = row.get(4)?;
    let data: ItemData = serde_json::from_str(&data_str).unwrap_or_default();
    Ok(Item {
        id: row.get(0)?,
        classification_id: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        data,
        shortcut_key: row.get(5)?,
        global_shortcut_key: row.get(6)?,
        order: row.get(7)?,
    })
}

/// 当前时间戳 (毫秒)
fn chrono_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 将绝对路径转为相对路径（相对 base_dir）
/// 网址不转换
fn make_relative_path(target: &str, base_dir: &str) -> Option<String> {
    // 网址不转换
    if target.starts_with("http://") || target.starts_with("https://") {
        return None;
    }
    // 已经是相对路径
    if target.starts_with('.') || target.starts_with("..") {
        return None;
    }
    let target_path = std::path::Path::new(target);
    let base_path = std::path::Path::new(base_dir);
    let diff = pathdiff::diff_paths(target_path, base_path)?;
    let diff_str = diff.to_string_lossy().to_string();
    // Windows 下统一用正斜杠
    let rel = diff_str.replace('\\', "/");
    if rel.is_empty() {
        None
    } else if rel.starts_with('.') {
        Some(rel)
    } else {
        Some(format!("./{}", rel))
    }
}

/// 将相对路径转为绝对路径（基于 base_dir）
fn make_absolute_path(target: &str, base_dir: &str) -> Option<String> {
    // 网址不转换
    if target.starts_with("http://") || target.starts_with("https://") {
        return None;
    }
    // 已经是绝对路径（Windows 盘符或 UNC）
    if target.len() >= 2 && target.as_bytes()[1] == b':' {
        return None;
    }
    if target.starts_with("\\\\") {
        return None;
    }
    // 必须是相对路径才转换
    if !target.starts_with('.') && !target.starts_with("..") {
        return None;
    }
    let base_path = std::path::Path::new(base_dir);
    let target_path = std::path::Path::new(target);
    let joined = base_path.join(target_path);
    let canonical = joined.canonicalize().unwrap_or(joined);
    Some(canonical.to_string_lossy().to_string())
}
