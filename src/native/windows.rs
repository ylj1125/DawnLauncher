// Windows 系统原生功能封装
// 移植自原项目 rust/windows.rs，去除 napi 依赖，改为纯 Rust 函数
//
// 当前已迁移功能：
// - get_file_icon: 获取文件/文件夹图标 (Base64 PNG)
// - get_shortcut_file_info: 解析 .lnk 快捷方式
// - open_file_location: 在资源管理器中定位文件
// - search_path: 在 PATH 环境变量中搜索可执行文件
// - open_path: 用系统默认程序打开文件/网址
//
// 待迁移（后续迭代）：
// - explorer_context_menu: 资源管理器右键菜单 (COM IContextMenu)
// - create_mouse_hook / enable_mouse_hook: 全局鼠标 HOOK
// - get_appx_list: Appx 应用列表
// - is_fullscreen / switch_english: 全屏检测/输入法切换

use std::collections::HashMap;
use std::process::Command;
use windows::core::{ComInterface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::UI::Shell::{
    IShellLinkW, ShellLink, SLGP_UNCPRIORITY,
};
use windows::Win32::Foundation::MAX_PATH;

/// 获取文件/文件夹图标，返回 Base64 编码的 PNG data URL
/// 如果失败返回 None
pub fn get_file_icon(path: &str) -> Option<String> {
    // 此函数完整实现需要 IShellItemImageFactory + GDI 位图处理
    // MVP 阶段使用简化版本: 调用 PowerShell 通过 SHGetFileInfo 间接获取
    // 完整版本将在后续迭代中迁移原 rust/windows.rs 的实现
    get_file_icon_simple(path)
}

/// 简化版图标获取: 通过 PowerShell 调用 ExtractAssociatedIcon
/// 性能不如原生 IShellItemImageFactory，但 MVP 阶段够用
fn get_file_icon_simple(path: &str) -> Option<String> {
    // 检查路径是否存在
    if !std::path::Path::new(path).exists() {
        return None;
    }
    // 使用 PowerShell 提取图标
    let ps_script = format!(
        r#"
Add-Type -AssemblyName System.Drawing
$icon = [System.Drawing.Icon]::ExtractAssociatedIcon('{}')
if ($icon) {{
    $ms = New-Object System.IO.MemoryStream
    $icon.ToBitmap().Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $ms.ToArray()
    [Convert]::ToBase64String($bytes)
}}
"#,
        path.replace('\'', "''")
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(format!("data:image/png;base64,{}", stdout))
    }
}

/// 解析 .lnk 快捷方式文件，返回 target 和 arguments
pub fn get_shortcut_file_info(path: &str) -> Option<HashMap<String, String>> {
    let path_h = HSTRING::from(path);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let shell_link_result: Result<IShellLinkW, windows::core::Error> =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) };
    let result = if let Ok(shell_link) = shell_link_result {
        let persist_file: Result<IPersistFile, windows::core::Error> = shell_link.cast();
        if let Ok(persist_file) = persist_file {
            let load_result =
                unsafe { persist_file.Load(PCWSTR(path_h.as_ptr()), STGM_READ) };
            if load_result.is_ok() {
                let mut target_buffer = [0u16; MAX_PATH as usize];
                let mut fd: WIN32_FIND_DATAW = Default::default();
                let _ = unsafe {
                    shell_link.GetPath(
                        &mut target_buffer,
                        &mut fd,
                        SLGP_UNCPRIORITY.0 as u32,
                    )
                };
                let mut arguments_buffer = [0u16; MAX_PATH as usize];
                let _ = unsafe { shell_link.GetArguments(&mut arguments_buffer) };
                let mut map = HashMap::with_capacity(2);
                map.insert(String::from("target"), u16_to_string(&target_buffer));
                map.insert(
                    String::from("arguments"),
                    u16_to_string(&arguments_buffer),
                );
                Some(map)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    unsafe {
        CoUninitialize();
    }
    result
}

/// u16 数组转 String（截断到第一个 0）
fn u16_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 在资源管理器中打开文件所在位置并选中
pub fn open_file_location(path: &str) {
    let _ = Command::new("explorer")
        .args(["/select,", path])
        .spawn();
}

/// 在 PATH 环境变量中搜索可执行文件
pub fn search_path(file: &str) -> Option<String> {
    let path_env = std::env::var("PATH").ok()?;
    for dir in path_env.split(';') {
        let full = std::path::Path::new(dir).join(file);
        if full.exists() {
            return Some(full.to_string_lossy().to_string());
        }
        // 尝试 .exe 后缀
        let with_exe = std::path::Path::new(dir).join(format!("{}.exe", file));
        if with_exe.exists() {
            return Some(with_exe.to_string_lossy().to_string());
        }
    }
    None
}

/// 用系统默认程序打开文件或网址
pub fn open_path(path: &str) -> std::io::Result<()> {
    Command::new("cmd").args(["/C", "start", "", path]).spawn()?;
    Ok(())
}

/// 判断当前前台窗口是否全屏 (简化版，后续完整实现)
pub fn is_fullscreen() -> bool {
    // TODO: 完整实现需要枚举所有窗口并判断是否覆盖屏幕
    false
}
