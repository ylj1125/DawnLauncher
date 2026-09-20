// 非 Windows 平台的占位实现，仅用于跨平台编译

use std::collections::HashMap;

pub fn get_file_icon(_path: &str) -> Option<String> {
    None
}

pub fn get_shortcut_file_info(_path: &str) -> Option<HashMap<String, String>> {
    None
}

pub fn open_file_location(_path: &str) {}

pub fn search_path(_file: &str) -> Option<String> {
    None
}

pub fn open_path(_path: &str) -> std::io::Result<()> {
    Ok(())
}

pub fn is_fullscreen() -> bool {
    false
}
