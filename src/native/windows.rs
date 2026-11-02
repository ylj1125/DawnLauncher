// Windows 系统原生功能封装
// 移植自原项目 rust/windows.rs，去除 napi 依赖，改为纯 Rust 函数
//
// 当前已迁移功能：
// - get_file_icon: 获取文件/文件夹图标 (IShellItemImageFactory 原生提取, Base64 PNG)
// - enable_file_drop: 拖拽文件到主窗口添加项目 (WM_DROPFILES)
// - get_shortcut_file_info: 解析 .lnk 快捷方式
// - open_file_location: 在资源管理器中定位文件
// - search_path: 在 PATH 环境变量中搜索可执行文件
// - open_path: 用系统默认程序打开文件/网址
// - scan_start_menu: 扫描开始菜单快捷方式
// - scan_appx: 扫描已安装的 Appx (商店) 应用
// - open_appx: 通过 AppUserModelID 启动 Appx 应用
//
// - explorer_context_menu: 资源管理器右键菜单 (COM IContextMenu)
//
// 待迁移（后续迭代）：
// - 输入法高级控制（中英文双向切换）

use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;
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

// windows-rs 0.51 将 TrackPopupMenu 的 BOOL 返回值映射为 Result<()>，
// 会丢失 TPM_RETURNCMD 返回的命令 ID，因此这里直接绑定 user32 原始接口。
#[link(name = "user32")]
extern "system" {
    #[link_name = "TrackPopupMenu"]
    fn track_popup_menu_raw(
        hmenu: isize,
        uflags: u32,
        x: i32,
        y: i32,
        nreserved: i32,
        hwnd: isize,
        prcrect: *const core::ffi::c_void,
    ) -> i32;
}

/// 获取文件/文件夹图标，返回 Base64 编码的 PNG data URL
/// 优先使用 IShellItemImageFactory 原生提取，失败时回退到 PowerShell 方案
pub fn get_file_icon(path: &str) -> Option<String> {
    get_file_icon_native(path).or_else(|| get_file_icon_simple(path))
}

/// 原生图标提取: SHCreateItemFromParsingName -> IShellItemImageFactory::GetImage
/// 得到 32bpp HBITMAP 后转 RGBA 并编码为 PNG，无需启动外部进程
fn get_file_icon_native(path: &str) -> Option<String> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };

    // 图标按 48px 提取，兼顾清晰度与体积
    const ICON_SIZE: i32 = 48;

    if !std::path::Path::new(path).exists() {
        return None;
    }

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let path_h = HSTRING::from(path);
    let factory: IShellItemImageFactory =
        match unsafe { SHCreateItemFromParsingName(PCWSTR(path_h.as_ptr()), None) } {
            Ok(f) => f,
            Err(_) => {
                unsafe {
                    let _ = CoUninitialize();
                }
                return None;
            }
        };

    let hbitmap = match unsafe {
        factory.GetImage(
            SIZE {
                cx: ICON_SIZE,
                cy: ICON_SIZE,
            },
            SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
        )
    } {
        Ok(h) => h,
        Err(_) => {
            unsafe {
                let _ = CoUninitialize();
            }
            return None;
        }
    };

    let png = unsafe { hbitmap_to_png(hbitmap) };

    unsafe {
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(hbitmap);
        let _ = CoUninitialize();
    }

    let png = png?;
    Some(format!(
        "data:image/png;base64,{}",
        crate::app::base64_encode(&png)
    ))
}

/// 将 32bpp HBITMAP 转为 RGBA 字节并编码为 PNG
unsafe fn hbitmap_to_png(hbitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BI_RGB,
        DIB_RGB_COLORS,
    };

    let mut bm: BITMAP = std::mem::zeroed();
    if GetObjectW(
        hbitmap,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut core::ffi::c_void),
    ) == 0
    {
        return None;
    }
    let width = bm.bmWidth;
    let height = bm.bmHeight;
    if width <= 0 || height <= 0 {
        return None;
    }

    let hdc = CreateCompatibleDC(None);
    if hdc.0 == 0 {
        return None;
    }

    // 负高度表示自上而下的 DIB，省去手动翻转
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of_val(&bmi.bmiHeader) as u32;
    bmi.bmiHeader.biWidth = width;
    bmi.bmiHeader.biHeight = -height;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(
        hdc,
        hbitmap,
        0,
        height as u32,
        Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    let _ = DeleteDC(hdc);
    if lines == 0 {
        return None;
    }

    // BGRA -> RGBA；若整张图都没有 alpha（老式 24 位图标），统一按不透明处理
    let has_alpha = pixels.chunks_exact(4).any(|px| px[3] != 0);
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        if !has_alpha {
            px[3] = 255;
        }
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&pixels).ok()?;
    }
    Some(out)
}

/// 简化版图标获取: 通过 PowerShell 调用 ExtractAssociatedIcon
/// 性能不如原生 IShellItemImageFactory，但 MVP 阶段够用
/// 使用 CREATE_NO_WINDOW 标志隐藏 PowerShell 控制台窗口
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
    let output = run_hidden(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &ps_script],
    )?;
    let stdout = String::from_utf8_lossy(&output).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(format!("data:image/png;base64,{}", stdout))
    }
}

/// 隐藏窗口执行命令，返回 stdout 字节
/// 使用 CREATE_NO_WINDOW 标志，避免弹出黑色控制台窗口
#[cfg(target_os = "windows")]
pub(crate) fn run_hidden(cmd: &str, args: &[&str]) -> Option<Vec<u8>> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW = 0x08000000
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    std::process::Command::new(cmd)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .map(|o| o.stdout)
}

/// 运行 PowerShell WinForms 对话框（显示在前台，不带 CREATE_NO_WINDOW）
pub(crate) fn run_dialog(cmd: &str, args: &[&str]) -> Option<Vec<u8>> {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .map(|o| o.stdout)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn run_hidden(_cmd: &str, _args: &[&str]) -> Option<Vec<u8>> {
    None
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
/// 使用 ShellExecuteW 而非 cmd /C start，避免 mingw 下参数转义问题
pub fn open_path(path: &str) -> std::io::Result<()> {
    open_path_with_options(path, None, None, false)
}

/// 带参数 / 起始位置 / 管理员权限 打开项目
/// - params: 启动参数（可为空）
/// - start_location: 工作目录（为空则不指定）
/// - run_as_admin: true 时用 "runas" 动词提权
pub fn open_path_with_options(
    path: &str,
    params: Option<&str>,
    start_location: Option<&str>,
    run_as_admin: bool,
) -> std::io::Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let path_h = HSTRING::from(path);
    let operation = if run_as_admin {
        HSTRING::from("runas")
    } else {
        HSTRING::from("open")
    };
    let params_h = params
        .filter(|p| !p.trim().is_empty())
        .map(HSTRING::from);
    let dir_h = start_location
        .filter(|d| !d.trim().is_empty())
        .map(HSTRING::from);

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(path_h.as_ptr()),
            params_h
                .as_ref()
                .map(|h| PCWSTR(h.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            dir_h
                .as_ref()
                .map(|h| PCWSTR(h.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as usize <= 32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("ShellExecuteW 失败，错误码: {}", result.0 as usize),
        ));
    }
    Ok(())
}

/// 在资源管理器中打开文件所在位置（选中该文件）
pub fn open_file_location(path: &str) {
    // 使用 explorer.exe /select,"path" 语法
    // 为避免路径中含特殊字符，直接用 ShellExecuteW
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let select_arg = format!("/select,\"{}\"", path);
    let arg_h = HSTRING::from(select_arg);
    let exe_h = HSTRING::from("explorer.exe");
    let _ = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(exe_h.as_ptr()),
            PCWSTR(arg_h.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
}

/// 显示资源管理器原生右键菜单 (COM IContextMenu)
/// path: 文件/文件夹路径，x/y: 屏幕坐标
/// 返回 true 表示菜单已弹出（不代表用户选择了命令）
pub fn explorer_context_menu(path: &str, x: i32, y: i32) -> bool {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, IContextMenu, IShellItem, SHCreateItemFromParsingName, CMF_NORMAL,
        CMINVOKECOMMANDINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, FindWindowW, GetForegroundWindow, SetForegroundWindow,
        SW_NORMAL, TPM_NONOTIFY, TPM_RETURNCMD,
    };

    // 目标必须真实存在（网址/无效路径不弹菜单）
    if !std::path::Path::new(path).exists() {
        return false;
    }

    // 确保当前线程已初始化 COM（重复调用安全）
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let path_h = HSTRING::from(path);
    let shell_item: Result<IShellItem, _> = unsafe {
        SHCreateItemFromParsingName(PCWSTR(path_h.as_ptr()), None)
    };
    let Ok(shell_item) = shell_item else {
        return false;
    };

    // 绑定到 IContextMenu
    let context_menu: Result<IContextMenu, _> =
        unsafe { shell_item.BindToHandler(None, &BHID_SFUIObject) };
    let Ok(context_menu) = context_menu else {
        return false;
    };

    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return false;
    };

    // 把资源管理器的菜单项写入弹窗菜单
    if unsafe { context_menu.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL) }.is_err() {
        unsafe {
            let _ = DestroyMenu(menu);
        }
        return false;
    }

    // 菜单宿主窗口：优先使用前台窗口（即本应用窗口）
    let mut hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 {
        let title = HSTRING::from("Dawn Launcher");
        hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) };
    }
    if hwnd.0 == 0 {
        hwnd = HWND(0);
    }

    let cmd = unsafe {
        let _ = SetForegroundWindow(hwnd);
        track_popup_menu_raw(
            menu.0,
            (TPM_RETURNCMD | TPM_NONOTIFY).0,
            x,
            y,
            0,
            hwnd.0,
            std::ptr::null(),
        )
    };

    // 用户选择了菜单项：命令 ID 从 1 开始，lpVerb 用整数伪装成指针传入
    if cmd != 0 {
        let mut info = CMINVOKECOMMANDINFO::default();
        info.cbSize = std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32;
        info.hwnd = hwnd;
        info.lpVerb = PCSTR((cmd - 1) as *mut u8);
        info.nShow = SW_NORMAL.0 as i32;
        let _ = unsafe { context_menu.InvokeCommand(&info) };
    }

    unsafe {
        let _ = DestroyMenu(menu);
    }
    true
}

/// 在当前鼠标位置弹出资源管理器原生右键菜单
pub fn explorer_context_menu_at_cursor(path: &str) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut pt = POINT { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut pt) }.is_ok();
    let (x, y) = if ok { (pt.x, pt.y) } else { (0, 0) };
    explorer_context_menu(path, x, y)
}

/// 判断当前前台窗口是否全屏（用窗口矩形与屏幕尺寸比较）
pub fn is_fullscreen() -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        if screen_w <= 0 || screen_h <= 0 {
            return false;
        }
        rect.left <= 0
            && rect.top <= 0
            && (rect.right - rect.left) >= screen_w
            && (rect.bottom - rect.top) >= screen_h
    }
}

/// 将当前线程的键盘布局切换为英文（美式 0409），关闭 IME 中文输入状态
/// 用于快速搜索窗口打开时，保证用户可以直接输入英文字符
pub fn switch_english() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{LoadKeyboardLayoutW, KLF_ACTIVATE};
    // 00000409 = 美式英语键盘布局
    let klid = HSTRING::from("00000409");
    unsafe {
        LoadKeyboardLayoutW(PCWSTR(klid.as_ptr()), KLF_ACTIVATE).ok();
    }
    // 向前台窗口发送 IMC_SETOPENSTATUS(FALSE)，关闭输入法
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, SendMessageW, WM_IME_CONTROL,
        };
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        const IMC_SETOPENSTATUS: u32 = 0x0028;
        let hwnd = GetForegroundWindow();
        if hwnd.0 != 0 {
            SendMessageW(hwnd, WM_IME_CONTROL, WPARAM(IMC_SETOPENSTATUS as usize), LPARAM(0));
        }
    }
}

// ============================================================
// 拖拽文件到主窗口 (WM_DROPFILES)
// ============================================================

/// 拖放回调：收到被拖入的文件/文件夹路径列表
static DROP_HANDLER: OnceLock<Box<dyn Fn(Vec<String>) + Send + Sync>> = OnceLock::new();
/// 原窗口过程（子类化前的 winit 过程），用于转发未处理的消息
static OLD_WNDPROC: AtomicIsize = AtomicIsize::new(0);
/// 是否已安装（避免重复子类化）
static DROP_INSTALLED: AtomicBool = AtomicBool::new(false);

const WM_DROPFILES: u32 = 0x0233;

unsafe extern "system" fn drop_wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};

    if msg == WM_DROPFILES {
        let hdrop = HDROP(wparam.0 as isize);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            // 先取长度（不含结尾 0），再按长度读取
            let len = DragQueryFileW(hdrop, i, None);
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; (len + 1) as usize];
            let copied = DragQueryFileW(hdrop, i, Some(&mut buf[..]));
            if copied > 0 {
                buf.truncate(copied as usize);
                paths.push(String::from_utf16_lossy(&buf));
            }
        }
        DragFinish(hdrop);
        log::info!("拖入 {} 个文件/文件夹", paths.len());
        if let Some(handler) = DROP_HANDLER.get() {
            handler(paths);
        }
        return LRESULT(0);
    }

    let old = OLD_WNDPROC.load(Ordering::SeqCst);
    if old != 0 {
        let proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC =
            std::mem::transmute(old);
        return windows::Win32::UI::WindowsAndMessaging::CallWindowProcW(
            proc, hwnd, msg, wparam, lparam,
        );
    }
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 让主窗口接受文件拖放 (DragAcceptFiles + 子类化窗口过程)
/// 返回 true 表示安装成功；窗口必须先创建（show 过）才能找到 HWND
pub fn enable_file_drop<F>(window_title: &str, callback: F) -> bool
where
    F: Fn(Vec<String>) + Send + Sync + 'static,
{
    use windows::Win32::UI::Shell::DragAcceptFiles;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GWLP_WNDPROC, SetWindowLongPtrW,
    };

    if DROP_INSTALLED.load(Ordering::SeqCst) {
        return true;
    }

    unsafe {
        let title = HSTRING::from(window_title);
        let hwnd = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr()));
        if hwnd.0 == 0 {
            log::warn!("未找到主窗口，文件拖放安装失败");
            return false;
        }
        DragAcceptFiles(hwnd, true);
        let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, drop_wndproc as usize as isize);
        if old == 0 {
            log::warn!("子类化窗口过程失败，文件拖放不可用");
            return false;
        }
        OLD_WNDPROC.store(old, Ordering::SeqCst);
    }

    // 子类化成功后再注册回调（消息进入时 DROP_HANDLER 已就绪）
    if DROP_HANDLER.set(Box::new(callback)).is_err() {
        return false;
    }
    DROP_INSTALLED.store(true, Ordering::SeqCst);
    log::info!("文件拖放已启用");
    true
}

// ============================================================
// 开始菜单 / Appx (商店应用) 扫描
// ============================================================

/// 扫描得到的启动项条目
#[derive(Debug, Clone)]
pub struct LauncherEntry {
    /// 显示名称
    pub name: String,
    /// 启动目标：文件路径 或 Appx 的 AppUserModelID
    pub target: String,
    /// 项目类型: 0 文件 / 4 Appx
    pub kind: i64,
    /// 启动参数 (可为空)
    pub params: Option<String>,
    /// 图标 (data:image/png;base64,...)
    pub icon: Option<String>,
}

/// 递归收集目录下的 .lnk 文件
/// depth 用于限制目录深度，避免异常目录结构导致过深递归
fn collect_lnk_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 8 || !dir.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lnk_files(&path, out, depth + 1);
        } else if path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("lnk"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

/// 批量获取文件图标（逐项走原生 IShellItemImageFactory，无需启动 PowerShell）
/// 返回与输入等长的结果
pub fn get_icons_batch(paths: &[String]) -> Vec<Option<String>> {
    paths.iter().map(|p| get_file_icon(p)).collect()
}

/// 扫描开始菜单中的快捷方式 (用户 + 全局)，返回可导入的应用列表
/// 会解析 .lnk 得到真实目标路径与启动参数，并批量提取图标
pub fn scan_start_menu() -> Vec<LauncherEntry> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs.push(
            std::path::Path::new(&appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    if let Ok(programdata) = std::env::var("PROGRAMDATA") {
        dirs.push(
            std::path::Path::new(&programdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }

    let mut lnk_files: Vec<std::path::PathBuf> = Vec::new();
    for d in &dirs {
        collect_lnk_files(d, &mut lnk_files, 0);
    }
    lnk_files.sort();
    lnk_files.dedup();

    // 解析每个 .lnk（去重按目标路径）
    struct Raw {
        name: String,
        target: String,
        params: Option<String>,
        icon_src: String,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in &lnk_files {
        let path_str = path.to_string_lossy().to_string();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let (target, params) = match get_shortcut_file_info(&path_str) {
            Some(info) => (
                info.get("target").cloned().unwrap_or_default(),
                info.get("arguments").cloned(),
            ),
            None => (String::new(), None),
        };
        let target = target.trim().trim_matches('"').to_string();
        // 解析失败时退化为直接用 .lnk 启动
        let target = if target.is_empty() {
            path_str.clone()
        } else {
            target
        };
        let key = target.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        let params = params.filter(|p| !p.trim().is_empty());
        // 图标优先取目标文件；目标是文件夹/不存在时退化为 .lnk 自身
        let icon_src = if std::path::Path::new(&target).is_file() {
            target.clone()
        } else {
            path_str.clone()
        };
        raws.push(Raw {
            name,
            target,
            params,
            icon_src,
        });
    }

    let icon_srcs: Vec<String> = raws.iter().map(|r| r.icon_src.clone()).collect();
    let icons = get_icons_batch(&icon_srcs);

    raws.into_iter()
        .enumerate()
        .map(|(i, r)| LauncherEntry {
            name: r.name,
            target: r.target,
            kind: 0,
            params: r.params,
            icon: icons.get(i).cloned().flatten(),
        })
        .collect()
}

/// 扫描已安装的 Appx (Microsoft Store / UWP) 应用
/// 通过 shell:AppsFolder 枚举得到显示名称与 AppUserModelID，
/// 并从各包清单解析 Logo 作为图标（单次 PowerShell 调用完成）。
pub fn scan_appx() -> Vec<LauncherEntry> {
    // 结果写入临时文件（UTF-8 无 BOM），避免 CREATE_NO_WINDOW 下无控制台导致
    // [Console]::OutputEncoding 报错以及中文名称乱码
    let out_file = std::env::temp_dir().join("dawn-launcher-appx.txt");
    let out_path = out_file.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Drawing
$out = '{out}'
$lines = @()
# 建立 AUMID -> Logo 文件路径 的映射
$map = @{{}}
foreach ($pkg in Get-AppxPackage) {{
    $loc = $pkg.InstallLocation
    if (-not $loc) {{ continue }}
    $mf = $null
    try {{ $mf = Get-AppxPackageManifest $pkg }} catch {{ continue }}
    if (-not $mf) {{ continue }}
    $fam = $pkg.PackageFamilyName
    try {{
        foreach ($app in $mf.Package.Applications.Application) {{
            $logo = $app.VisualElements.Square44x44Logo
            if (-not $logo) {{ $logo = $app.VisualElements.Square150x150Logo }}
            if (-not $logo) {{ $logo = $app.VisualElements.Logo }}
            if (-not $logo) {{ continue }}
            $full = Join-Path $loc ($logo -replace '/', '\')
            $map[($fam + '!' + $app.Id)] = $full
        }}
    }} catch {{}}
}}
# 枚举 shell:AppsFolder 得到显示名称与 AUMID
$shell = New-Object -ComObject Shell.Application
$folder = $shell.NameSpace('shell:AppsFolder')
if ($folder) {{
    foreach ($item in $folder.Items()) {{
        $aumid = $item.Path
        if (-not ($aumid -and $aumid.Contains('!'))) {{ continue }}
        $b = '-'
        $lp = $map[$aumid]
        if ($lp) {{
            # Logo 基准文件可能不存在，按前缀匹配尺寸变体并取最大
            if (-not (Test-Path -LiteralPath $lp -PathType Leaf)) {{
                try {{
                    $dir = Split-Path $lp
                    $base = [System.IO.Path]::GetFileNameWithoutExtension($lp)
                    $f = Get-ChildItem -LiteralPath $dir -Filter ($base + '*') -File |
                         Sort-Object Length -Descending | Select-Object -First 1
                    if ($f) {{ $lp = $f.FullName }}
                }} catch {{}}
            }}
            try {{
                if (Test-Path -LiteralPath $lp -PathType Leaf) {{
                    $img = [System.Drawing.Image]::FromFile($lp)
                    $bmp = New-Object System.Drawing.Bitmap($img, 48, 48)
                    $ms = New-Object System.IO.MemoryStream
                    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
                    $b = [Convert]::ToBase64String($ms.ToArray())
                    $bmp.Dispose()
                    $img.Dispose()
                }}
            }} catch {{ $b = '-' }}
        }}
        $lines += ($aumid + "`t" + $item.Name + "`t" + $b)
    }}
}}
[System.IO.File]::WriteAllText($out, ($lines -join "`n"), (New-Object System.Text.UTF8Encoding($false)))
"#,
        out = out_path
    );
    let _ = run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]);
    let text = std::fs::read(&out_file)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    let mut result: Vec<LauncherEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in text.lines() {
        let mut parts = line.split('\t');
        let aumid = parts.next().unwrap_or("").trim().to_string();
        let name = parts.next().unwrap_or("").trim().to_string();
        let icon_b64 = parts.next().unwrap_or("").trim().to_string();
        if aumid.is_empty() || !aumid.contains('!') {
            continue;
        }
        let key = aumid.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        let name = if name.is_empty() { aumid.clone() } else { name };
        let icon = if icon_b64.is_empty() || icon_b64 == "-" {
            None
        } else {
            Some(format!("data:image/png;base64,{}", icon_b64))
        };
        result.push(LauncherEntry {
            name,
            target: aumid,
            kind: 4,
            params: None,
            icon,
        });
    }
    result
}

/// 枚举系统已安装的字体族名称
/// 通过 System.Drawing.FontFamily.Families 获取，结果经 UTF-8 临时文件回传
pub fn get_installed_fonts() -> Vec<String> {
    let out_file = std::env::temp_dir().join("dawn-launcher-fonts.txt");
    let out_path = out_file.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Drawing
$out = '{out}'
$names = [System.Drawing.FontFamily]::Families | ForEach-Object {{ $_.Name }}
[System.IO.File]::WriteAllLines($out, $names, (New-Object System.Text.UTF8Encoding($false)))
"#,
        out = out_path
    );
    let _ = run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]);
    let text = std::fs::read(&out_file)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 扫描一个文件夹下的直接子项 (文件 + 子文件夹)
/// 用于「关联文件夹」分类：将其内容映射为可启动的项目。
/// 跳过隐藏项，文件夹排前、其余按名称排序，并批量提取图标。
pub fn scan_folder(path: &str) -> Vec<LauncherEntry> {
    let mut result: Vec<LauncherEntry> = Vec::new();
    let dir = match std::fs::read_dir(path) {
        Ok(d) => d,
        Err(_) => return result,
    };

    struct Raw {
        name: String,
        target: String,
        kind: i64,
        is_dir: bool,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in dir.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = meta.is_dir();
        // 跳过隐藏项与点文件
        if is_hidden_entry(&p, &meta) {
            continue;
        }
        let target = p.to_string_lossy().to_string();
        let key = target.to_lowercase();
        if !seen.insert(key) {
            continue;
        }
        raws.push(Raw {
            name,
            target,
            kind: if is_dir { 1 } else { 0 },
            is_dir,
        });
    }

    // 文件夹在前，其余按名称不区分大小写排序
    raws.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });

    let icon_srcs: Vec<String> = raws.iter().map(|r| r.target.clone()).collect();
    let icons = get_icons_batch(&icon_srcs);

    for (i, r) in raws.into_iter().enumerate() {
        result.push(LauncherEntry {
            name: r.name,
            target: r.target,
            kind: r.kind,
            params: None,
            icon: icons.get(i).cloned().flatten(),
        });
    }
    result
}

/// 判断目录项是否为隐藏项 (Windows 隐藏属性 或 名称以 '.' 开头)
fn is_hidden_entry(path: &std::path::Path, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    if meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
        return true;
    }
    path.file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

/// 网页信息抓取结果
#[derive(Debug, Clone, Default)]
pub struct UrlInfo {
    /// 网页标题 (<title>)
    pub title: String,
    /// 站点图标 (data:image/png;base64,...)
    pub icon: Option<String>,
}

/// 抓取网址的标题与图标 (favicon)
/// 通过 PowerShell 下载页面并解析 <title> 与 <link rel="icon">，
/// 依次尝试声明的图标地址与 /favicon.ico，结果经 UTF-8 临时文件回传。
/// 完全失败 (无标题且无图标) 返回 None。
pub fn fetch_url_info(url: &str) -> Option<UrlInfo> {
    let out_file = std::env::temp_dir().join("dawn-launcher-urlinfo.txt");
    let out_path = out_file.to_string_lossy().replace('\'', "''");
    let url_escaped = url.replace('\'', "''");
    let script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Drawing
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$out = '{out}'
$url = '{url}'
$title = ''
$icon = '-'
try {{
    $resp = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 15 -MaximumRedirection 5
    $html = $resp.Content
    if ($html -is [byte[]]) {{ $html = [System.Text.Encoding]::UTF8.GetString($html) }}
    $m = [regex]::Match($html, '(?is)<title[^>]*>(.*?)</title>')
    if ($m.Success) {{ $title = ($m.Groups[1].Value -replace '\s+', ' ').Trim() }}
    # 基准地址 (处理重定向)
    $baseUri = New-Object System.Uri($url)
    try {{ $baseUri = $resp.BaseResponse.ResponseUri }} catch {{}}
    # 收集 <link rel=...icon...> 的 href 候选
    $cands = New-Object System.Collections.ArrayList
    foreach ($lt in [regex]::Matches($html, '(?is)<link\b[^>]*>')) {{
        $tag = $lt.Value
        if ($tag -notmatch '(?i)rel\s*=\s*["'']?([^"''>\s]+)') {{ continue }}
        if ($Matches[1] -notmatch '(?i)icon') {{ continue }}
        if ($tag -notmatch '(?i)href\s*=\s*["'']([^"'']+)') {{ continue }}
        [void]$cands.Add($Matches[1])
    }}
    [void]$cands.Add('/favicon.ico')
    foreach ($c in $cands) {{
        try {{ $iu = (New-Object System.Uri($baseUri, $c)).AbsoluteUri }} catch {{ continue }}
        try {{
            $wc = New-Object System.Net.WebClient
            $bytes = $wc.DownloadData($iu)
            if (-not $bytes -or $bytes.Length -eq 0) {{ continue }}
            $ms = New-Object System.IO.MemoryStream(,$bytes)
            $bmp = $null
            try {{ $ic = New-Object System.Drawing.Icon($ms); $bmp = $ic.ToBitmap() }} catch {{ $bmp = $null }}
            if (-not $bmp) {{
                try {{
                    $ms.Position = 0
                    $img = [System.Drawing.Image]::FromStream($ms)
                    $bmp = New-Object System.Drawing.Bitmap($img, 32, 32)
                }} catch {{ $bmp = $null }}
            }}
            if ($bmp) {{
                $osm = New-Object System.IO.MemoryStream
                $bmp.Save($osm, [System.Drawing.Imaging.ImageFormat]::Png)
                $icon = 'data:image/png;base64,' + [Convert]::ToBase64String($osm.ToArray())
                break
            }}
        }} catch {{}}
    }}
}} catch {{}}
[System.IO.File]::WriteAllText($out, ($title + "`n" + $icon), (New-Object System.Text.UTF8Encoding($false)))
"#,
        out = out_path,
        url = url_escaped
    );
    let _ = run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]);
    let text = std::fs::read(&out_file)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);

    let mut lines = text.splitn(2, '\n');
    let title = lines.next().unwrap_or("").trim().to_string();
    let icon_line = lines.next().unwrap_or("").trim();
    let icon = if icon_line.is_empty() || icon_line == "-" {
        None
    } else {
        Some(icon_line.to_string())
    };
    if title.is_empty() && icon.is_none() {
        return None;
    }
    Some(UrlInfo { title, icon })
}

/// 启动 Appx/UWP 应用 (通过 AppUserModelID)
/// 使用 explorer.exe 打开 shell:AppsFolder\<AUMID>
pub fn open_appx(aumid: &str) -> std::io::Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let arg = format!("shell:AppsFolder\\{}", aumid);
    let arg_h = HSTRING::from(arg);
    let exe_h = HSTRING::from("explorer.exe");
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(exe_h.as_ptr()),
            PCWSTR(arg_h.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as usize <= 32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("启动应用失败 (AUMID): {}", aumid),
        ));
    }
    Ok(())
}

// ============================================================
// 全局热键注册 (RegisterHotKey) - 使用直接 FFI 声明
// ============================================================

use std::sync::mpsc::Sender;

/// RegisterHotKey 的 mod 标志位
const MOD_CONTROL: u32 = 0x0002;
const MOD_ALT: u32 = 0x0001;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000;
/// VK_SPACE
const VK_SPACE: u32 = 0x20;
/// WM_HOTKEY 消息
const WM_HOTKEY: u32 = 0x0312;
/// 热键 ID
const HOTKEY_ID_QUICK_SEARCH: i32 = 0xB001;

extern "system" {
    fn RegisterHotKey(hWnd: *mut std::ffi::c_void, id: i32, fsModifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hWnd: *mut std::ffi::c_void, id: i32) -> i32;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: *mut std::ffi::c_void, wMsgFilterMin: u32, wMsgFilterMax: u32) -> std::ffi::c_int;
}

#[repr(C)]
#[derive(Default)]
struct MSG {
    hwnd: *mut std::ffi::c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

/// 在后台线程注册全局热键 (Ctrl+Space)，收到热键时通过 channel 发送通知
pub fn register_global_hotkey(notify: Sender<()>) {
    std::thread::spawn(move || {
        let result = unsafe {
            RegisterHotKey(
                std::ptr::null_mut(),
                HOTKEY_ID_QUICK_SEARCH,
                MOD_CONTROL | MOD_NOREPEAT,
                VK_SPACE,
            )
        };
        if result == 0 {
            log::warn!("RegisterHotKey 失败");
            return;
        }
        log::info!("全局热键 Ctrl+Space 注册成功");

        // 消息循环：等待 WM_HOTKEY
        let mut msg = MSG::default();
        loop {
            let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if ret <= 0 {
                break; // WM_QUIT (0) 或 错误 (-1)
            }
            if msg.message == WM_HOTKEY && msg.wparam as i32 == HOTKEY_ID_QUICK_SEARCH {
                let _ = notify.send(());
            }
        }

        // 注销热键
        let _ = unsafe { UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID_QUICK_SEARCH) };
        log::info!("全局热键已注销");
    });
}

/// 解析快捷键字符串（如 "Ctrl+Alt+A"）为 (修饰键, 虚拟键码)
fn parse_shortcut(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut vk: Option<u32> = None;
    for part in s.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" | "meta" => mods |= MOD_WIN,
            _ => {
                if vk.is_some() {
                    return None;
                }
                let up = part.to_uppercase();
                let code = if up.len() == 1 {
                    let c = up.chars().next()?;
                    if c.is_ascii_alphanumeric() {
                        c as u32
                    } else {
                        return None;
                    }
                } else if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                    if (1..=24).contains(&n) {
                        0x70 + n - 1
                    } else {
                        return None;
                    }
                } else {
                    match up.as_str() {
                        "SPACE" => 0x20,
                        "ENTER" | "RETURN" => 0x0D,
                        "ESC" | "ESCAPE" => 0x1B,
                        "TAB" => 0x09,
                        "DELETE" | "DEL" => 0x2E,
                        "BACKSPACE" => 0x08,
                        "HOME" => 0x24,
                        "END" => 0x2F,
                        "INSERT" | "INS" => 0x2D,
                        "PGUP" | "PAGEUP" => 0x21,
                        "PGDN" | "PAGEDOWN" => 0x22,
                        "UP" => 0x26,
                        "DOWN" => 0x28,
                        "LEFT" => 0x25,
                        "RIGHT" => 0x27,
                        _ => return None,
                    }
                };
                vk = Some(code);
            }
        }
    }
    vk.map(|v| (mods, v))
}

/// 批量注册分类/项目快捷键。specs: (动作标识, 快捷键字符串)
/// 收到热键时通过 channel 发送动作标识（如 "class:3" / "item:12"）
pub fn register_shortcut_hotkeys(specs: Vec<(String, String)>, notify: Sender<String>) {
    std::thread::spawn(move || {
        let mut map: Vec<(i32, String)> = Vec::new();
        for (action, shortcut) in specs {
            if let Some((mods, vk)) = parse_shortcut(&shortcut) {
                let id = 0xC000 + map.len() as i32;
                let ok = unsafe { RegisterHotKey(std::ptr::null_mut(), id, mods | MOD_NOREPEAT, vk) };
                if ok != 0 {
                    map.push((id, action));
                } else {
                    log::warn!("注册快捷键失败: {} -> {}", action, shortcut);
                }
            } else {
                log::warn!("无法解析快捷键: {}", shortcut);
            }
        }
        if map.is_empty() {
            return;
        }
        log::info!("已注册 {} 个分类/项目快捷键", map.len());

        let mut msg = MSG::default();
        loop {
            let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if ret <= 0 {
                break;
            }
            if msg.message == WM_HOTKEY {
                let id = msg.wparam as i32;
                if let Some((_, action)) = map.iter().find(|(hid, _)| *hid == id) {
                    let _ = notify.send(action.clone());
                }
            }
        }
        for (id, _) in &map {
            let _ = unsafe { UnregisterHotKey(std::ptr::null_mut(), *id) };
        }
    });
}

/// 单实例检测：创建命名互斥体，若已存在则说明已有实例在运行
/// 返回 true 表示当前是首个实例，false 表示已有实例运行
pub fn check_single_instance() -> bool {
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, BOOL};
    use windows::core::HSTRING;

    let name = HSTRING::from("DawnLauncher_SingleInstance_Mutex");
    unsafe {
        let handle = match CreateMutexW(None, BOOL(1), PCWSTR(name.as_ptr())) {
            Ok(h) => h,
            Err(_) => {
                log::warn!("CreateMutexW 失败，单实例检测不可用");
                return true; // 创建失败时允许启动，避免阻塞用户
            }
        };
        let err = GetLastError();
        if err.is_err() {
            // 已有实例运行（ERROR_ALREADY_EXISTS）
            let _ = ReleaseMutex(handle);
            let _ = CloseHandle(handle);
            return false;
        }
        // 故意不释放互斥体，保持句柄存活以维持单实例锁
        // 进程退出时系统会自动释放
        log::info!("单实例锁获取成功");
        true
    }
}

/// 释放单实例互斥体（可选，进程退出时系统自动回收）
pub fn release_single_instance() {}

// ===== 系统托盘图标 =====
// 采用独立隐藏消息窗口 + 专用消息循环线程的方案，
// 避免与 Slint/winit 的窗口过程冲突。

const WM_TRAYICON: u32 = 0x8000 + 1; // WM_APP + 1
const TRAY_ICON_ID: u32 = 1;
const IDM_TRAY_SHOW: usize = 1001;
const IDM_TRAY_EXIT: usize = 1002;
const WM_LBUTTONUP_C: u32 = 0x0202;
const WM_LBUTTONDBLCLK_C: u32 = 0x0203;
const WM_RBUTTONUP_C: u32 = 0x0205;
const WM_DESTROY_C: u32 = 0x0002;
const WM_CLOSE_C: u32 = 0x0010;
const WM_COMMAND_C: u32 = 0x0111;

/// 托盘图标是否已创建
static TRAY_ALIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 主窗口句柄 (保存为 isize 以便跨线程共享)
static TRAY_MAIN_HWND: once_cell::sync::Lazy<std::sync::Mutex<isize>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(0));
/// 托盘消息窗口句柄
static TRAY_SINK_HWND: once_cell::sync::Lazy<std::sync::Mutex<isize>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(0));

/// 切换主窗口显示/隐藏
#[cfg(target_os = "windows")]
unsafe fn tray_toggle_main_window() {
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
    };
    let main = windows::Win32::Foundation::HWND(*TRAY_MAIN_HWND.lock().unwrap());
    if main.0 == 0 {
        return;
    }
    if IsWindowVisible(main).as_bool() {
        let _ = ShowWindow(main, SW_HIDE);
    } else {
        let _ = ShowWindow(main, SW_SHOW);
        let _ = SetForegroundWindow(main);
    }
}

/// 显示托盘右键菜单
#[cfg(target_os = "windows")]
unsafe fn tray_show_menu(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow,
        TrackPopupMenu, MF_SEPARATOR, MF_STRING, TPM_RIGHTBUTTON,
    };
    use windows::Win32::Foundation::POINT;
    use windows::core::HSTRING;

    let hmenu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };
    let show_text = HSTRING::from("显示主窗口");
    let exit_text = HSTRING::from("退出");
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_TRAY_SHOW, PCWSTR(show_text.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_TRAY_EXIT, PCWSTR(exit_text.as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // 必须先置前台，否则菜单点击外部不会消失
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        hmenu,
        TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(hmenu);
}

/// 托盘消息窗口过程
#[cfg(target_os = "windows")]
unsafe extern "system" fn tray_wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, PostQuitMessage};

    match msg {
        WM_TRAYICON => {
            let event = lparam.0 as u32;
            if event == WM_LBUTTONUP_C || event == WM_LBUTTONDBLCLK_C {
                tray_toggle_main_window();
            } else if event == WM_RBUTTONUP_C {
                tray_show_menu(hwnd);
            }
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_COMMAND_C => {
            let id = (wparam.0 & 0xFFFF) as usize;
            if id == IDM_TRAY_SHOW {
                tray_toggle_main_window();
            } else if id == IDM_TRAY_EXIT {
                std::process::exit(0);
            }
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_DESTROY_C => {
            PostQuitMessage(0);
            windows::Win32::Foundation::LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 创建系统托盘图标
/// window_title: 用于通过 FindWindow 查找主窗口句柄
/// 返回 true 表示已启动托盘线程
pub fn create_tray_icon(window_title: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::HSTRING;

    let title = HSTRING::from(window_title);
    let main = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) };
    if main.0 == 0 {
        log::warn!("创建托盘图标失败：未找到主窗口");
        return false;
    }
    *TRAY_MAIN_HWND.lock().unwrap() = main.0;

    if TRAY_ALIVE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return true; // 已创建
    }

    std::thread::spawn(move || unsafe {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NOTIFYICONDATAW,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DispatchMessageW, GetMessageW, LoadIconW, LoadImageW, RegisterClassW,
            TranslateMessage, HICON, IMAGE_ICON, LR_LOADFROMFILE, WINDOW_EX_STYLE, WS_OVERLAPPED,
            HMENU, IDI_APPLICATION, MSG, WNDCLASSW,
        };
        use windows::Win32::Foundation::{HWND, HINSTANCE};

        let hinstance = match GetModuleHandleW(PCWSTR::null()) {
            Ok(h) => h,
            Err(_) => {
                TRAY_ALIVE.store(false, std::sync::atomic::Ordering::SeqCst);
                log::warn!("托盘线程：GetModuleHandleW 失败");
                return;
            }
        };

        let class_name = HSTRING::from("DawnLauncherTraySink");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(tray_wndproc),
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let sink = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND(0),
            HMENU(0),
            HINSTANCE(hinstance.0),
            None,
        );
        if sink.0 == 0 {
            TRAY_ALIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            log::warn!("托盘线程：CreateWindowExW 失败");
            return;
        }
        *TRAY_SINK_HWND.lock().unwrap() = sink.0;

        // 优先使用 data/tray.ico 自定义托盘图标，不存在时回退到系统默认图标
        let custom_icon = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("data").join("tray.ico")))
            .filter(|p| p.exists());
        let hicon = custom_icon
            .and_then(|p| {
                let wide: Vec<u16> = p
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                LoadImageW(
                    HINSTANCE(0),
                    PCWSTR(wide.as_ptr()),
                    IMAGE_ICON,
                    16,
                    16,
                    LR_LOADFROMFILE,
                )
                .ok()
                .map(|h| HICON(h.0))
            })
            .unwrap_or_else(|| LoadIconW(HINSTANCE(0), IDI_APPLICATION).unwrap_or_default());

        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = sink;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = hicon;
        let tip: Vec<u16> = HSTRING::from("Dawn Launcher").as_wide().to_vec();
        let len = tip.len().min(127);
        nid.szTip[..len].copy_from_slice(&tip[..len]);

        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            log::warn!("托盘线程：Shell_NotifyIconW(NIM_ADD) 失败");
            TRAY_ALIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        log::info!("系统托盘图标已创建");

        // 消息循环
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        log::info!("托盘消息循环已退出");
    });

    true
}

/// 删除系统托盘图标
pub fn remove_tray_icon() {
    use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
    use windows::Win32::Foundation::{HWND, WPARAM, LPARAM};

    if !TRAY_ALIVE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let sink = *TRAY_SINK_HWND.lock().unwrap();
    if sink == 0 {
        return;
    }
    unsafe {
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = HWND(sink);
        nid.uID = TRAY_ICON_ID;
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        // 通知托盘线程退出消息循环
        let _ = PostMessageW(HWND(sink), WM_CLOSE_C, WPARAM(0), LPARAM(0));
    }
    *TRAY_SINK_HWND.lock().unwrap() = 0;
    log::info!("系统托盘图标已删除");
}

/// 显示/隐藏主窗口（通过托盘图标调用）
/// visible: true=显示, false=隐藏
pub fn show_main_window(window_title: &str, visible: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
    };
    use windows::core::HSTRING;

    // 优先使用缓存的主窗口句柄
    let cached = *TRAY_MAIN_HWND.lock().unwrap();
    let hwnd = if cached != 0 {
        windows::Win32::Foundation::HWND(cached)
    } else {
        let title = HSTRING::from(window_title);
        unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) }
    };
    if hwnd.0 == 0 {
        return;
    }
    unsafe {
        if visible {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

// ===== 鼠标钩子 (鼠标中键显示/隐藏主窗口) =====

static MOUSE_HOOK_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static MOUSE_HOOK_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 切换主窗口显示/隐藏
fn toggle_main_window() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
    };
    use windows::core::{HSTRING, PCWSTR};

    let cached = *TRAY_MAIN_HWND.lock().unwrap();
    let hwnd = if cached != 0 {
        HWND(cached)
    } else {
        let title = HSTRING::from("Dawn Launcher");
        unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) }
    };
    if hwnd.0 == 0 {
        return;
    }
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    unsafe {
        if visible {
            let _ = ShowWindow(hwnd, SW_HIDE);
        } else {
            // 清除可能存在的托盘缓存隐藏标记
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

unsafe extern "system" fn mouse_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, WM_MBUTTONUP};
    if code >= 0
        && MOUSE_HOOK_ENABLED.load(std::sync::atomic::Ordering::SeqCst)
        && wparam.0 as u32 == WM_MBUTTONUP
    {
        toggle_main_window();
    }
    CallNextHookEx(
        windows::Win32::UI::WindowsAndMessaging::HHOOK(0),
        code,
        wparam,
        lparam,
    )
}

/// 启用/禁用鼠标中键显示隐藏主窗口
pub fn set_mouse_hook_enabled(enabled: bool) {
    MOUSE_HOOK_ENABLED.store(enabled, std::sync::atomic::Ordering::SeqCst);
    if !enabled {
        return;
    }
    // 钩子已安装则无需重复创建
    if MOUSE_HOOK_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, MSG, WH_MOUSE_LL,
        };
        use windows::Win32::Foundation::HINSTANCE;

        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), HINSTANCE(0), 0);
        if hook.is_err() {
            log::warn!("安装鼠标钩子失败");
            MOUSE_HOOK_STARTED.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        log::info!("鼠标钩子已启动");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, windows::Win32::Foundation::HWND(0), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
