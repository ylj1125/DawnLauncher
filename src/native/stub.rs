// 非 Windows 平台的占位实现，仅用于跨平台编译

use std::collections::HashMap;

pub fn get_file_icon(_path: &str) -> Option<String> {
    None
}

pub fn get_shortcut_file_info(_path: &str) -> Option<HashMap<String, String>> {
    None
}

pub fn open_file_location(_path: &str) {}

pub fn explorer_context_menu(_path: &str, _x: i32, _y: i32) -> bool {
    false
}

pub fn explorer_context_menu_at_cursor(_path: &str) -> bool {
    false
}

pub fn focus_window_by_title(_window_title: &str) {}

pub fn search_path(_file: &str) -> Option<String> {
    None
}

pub fn open_path(_path: &str) -> std::io::Result<()> {
    Ok(())
}

pub fn is_fullscreen() -> bool {
    false
}

pub fn register_global_hotkey(_notify: std::sync::mpsc::Sender<()>) {}

/// 扫描得到的启动项条目 (非 Windows 平台占位)
#[derive(Debug, Clone)]
pub struct LauncherEntry {
    pub name: String,
    pub target: String,
    pub kind: i64,
    pub params: Option<String>,
    pub icon: Option<String>,
}

pub fn scan_start_menu() -> Vec<LauncherEntry> {
    Vec::new()
}

pub fn scan_appx() -> Vec<LauncherEntry> {
    Vec::new()
}

pub fn open_appx(_aumid: &str) -> std::io::Result<()> {
    Ok(())
}

pub fn scan_folder(_path: &str) -> Vec<LauncherEntry> {
    Vec::new()
}

pub fn get_installed_fonts() -> Vec<String> {
    vec!["Microsoft YaHei UI".to_string()]
}

/// 网页信息抓取结果 (非 Windows 平台占位)
#[derive(Debug, Clone, Default)]
pub struct UrlInfo {
    pub title: String,
    pub icon: Option<String>,
}

pub fn fetch_url_info(_url: &str) -> Option<UrlInfo> {
    None
}

/// 单实例检测（非 Windows 平台始终返回 true）
pub fn check_single_instance() -> bool {
    true
}

pub fn release_single_instance() {}

pub fn create_tray_icon(_window_title: &str) -> bool {
    false
}

pub fn remove_tray_icon() {}

pub fn show_main_window(_window_title: &str, _visible: bool) {}

pub fn set_mouse_hook_enabled(_enabled: bool) {}
