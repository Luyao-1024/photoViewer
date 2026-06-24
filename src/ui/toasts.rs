//! AdwToastOverlay 助手
//!
//! 在已有 `AdwToastOverlay` 的页面（当前为窗口级 / 后续可挂到各 NavigationPage）
//! 上通过 `success` / `error` / `info` 三档显示用户反馈 toast：
//!
//! - `success`：普通优先级，3 秒超时 — 成功操作（保存、还原等）
//! - `error`：高优先级，5 秒超时 — 错误反馈（操作失败、I/O 失败等）
//! - `info`：普通优先级，2 秒超时 — 状态提示（如扫描完成、撤销倒计时等）
//!
//! 注意：当前 EditorPanel / TrashPage 等 NavigationPage 自身尚未嵌入
//! `AdwToastOverlay`，其内部 `show_toast` 仍走 tracing 日志回退；这些 helper
//! 供未来接入页面级 / 窗口级 overlay 后使用。
//!
//! libadwaita 0.6 (`v1_5`) 的 `ToastPriority` 只暴露 `Normal` / `High` 两档，
//! 没有 `Low`。`info` 因此沿用 `Normal` 但缩短超时以视觉上轻量化。
use libadwaita as adw;

/// Show a success toast (normal priority, 3 second timeout).
pub fn success(overlay: &adw::ToastOverlay, msg: &str) {
    let toast = adw::Toast::new(msg);
    toast.set_priority(adw::ToastPriority::Normal);
    toast.set_timeout(3);
    overlay.add_toast(toast);
}

/// Show an error toast (high priority, 5 second timeout).
pub fn error(overlay: &adw::ToastOverlay, msg: &str) {
    let toast = adw::Toast::new(msg);
    toast.set_priority(adw::ToastPriority::High);
    toast.set_timeout(5);
    overlay.add_toast(toast);
}

/// Show an info toast (normal priority, 2 second timeout).
pub fn info(overlay: &adw::ToastOverlay, msg: &str) {
    let toast = adw::Toast::new(msg);
    toast.set_priority(adw::ToastPriority::Normal);
    toast.set_timeout(2);
    overlay.add_toast(toast);
}
