// 原生模块入口
// 在 Windows 平台使用 windows 子模块，其他平台提供空实现以便跨平台编译

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(target_os = "windows"))]
pub mod stub;

#[cfg(not(target_os = "windows"))]
pub use stub::*;
