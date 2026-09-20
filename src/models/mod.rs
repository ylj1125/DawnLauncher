// 数据模型定义
// 对应原项目 types/classification.d.ts 和 types/item.d.ts

use serde::{Deserialize, Serialize};

/// 分类类型
/// 0: 普通分类  1: 关联文件夹  2: 聚合分类
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Classification {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: i64,
    pub data: ClassificationData,
    pub shortcut_key: Option<String>,
    pub global_shortcut_key: bool,
    pub order: i64,
    pub child_list: Vec<Classification>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClassificationData {
    pub icon: Option<String>,
    pub associate_folder_path: Option<String>,
    pub associate_folder_hidden_items: Option<String>,
    pub item_layout: String,        // default | tile | list
    pub item_sort: String,          // default | initial | openNumber | lastOpen
    pub item_column_number: Option<i64>,
    pub item_icon_size: Option<i64>,
    pub item_show_only: String,     // default | file | folder
    pub fixed: bool,
    pub aggregate_item_count: i64,
    pub exclude_search: bool,
}

/// 项目类型
/// 0: 文件  1: 文件夹  2: 网址  3: 系统  4: Appx  5: 多项目
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Item {
    pub id: i64,
    pub classification_id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: i64,
    pub data: ItemData,
    pub shortcut_key: Option<String>,
    pub global_shortcut_key: bool,
    pub order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ItemData {
    pub start_location: Option<String>,
    pub target: Option<String>,
    pub params: Option<String>,
    pub run_as_admin: bool,
    pub icon: Option<String>,
    pub html_icon: Option<String>,
    pub remark: Option<String>,
    pub icon_background_color: bool,
    pub fixed_icon: bool,
    pub open_number: i64,
    pub last_open: i64,
    pub quick_search_last_open: i64,
    pub multi_items_time_interval: i64,
}

impl Classification {
    /// 是否为父分类 (无 parent_id)
    pub fn is_parent(&self) -> bool {
        self.parent_id.is_none()
    }
}

impl Item {
    /// 是否为文件类项目
    pub fn is_file(&self) -> bool {
        self.kind == 0
    }
    /// 是否为文件夹类项目
    pub fn is_folder(&self) -> bool {
        self.kind == 1
    }
    /// 是否为网址类项目
    pub fn is_url(&self) -> bool {
        self.kind == 2
    }
    /// 是否为系统项
    pub fn is_system(&self) -> bool {
        self.kind == 3
    }
    /// 是否为 Appx
    pub fn is_appx(&self) -> bool {
        self.kind == 4
    }
}
