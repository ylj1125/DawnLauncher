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
        let mut stmt = match conn.prepare(
            "SELECT id, classification_id, name, type, data, shortcut_key, global_shortcut_key, `order` FROM item WHERE classification_id = ? ORDER BY `order` ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![classification_id], row_to_item)
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
