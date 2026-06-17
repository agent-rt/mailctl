//! 写操作审计日志（JSONL，追加写）。
//! 每次变更前**先记录意图再执行**——即使执行中途崩溃，也留有「试图改动了什么」的痕迹，
//! 可据此人工或批量回滚。记录失败视为致命：不留痕不动手。

use crate::error::{Error, Result};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "mailctl")
        .ok_or_else(|| Error::Config("无法定位配置目录".to_string()))?;
    Ok(dirs.config_dir().join("audit.log"))
}

/// 追加一条审计记录。`dest` 仅 move 类操作（trash/restore）有意义。
pub fn record(
    account: &str,
    action: &str,
    folder: &str,
    dest: Option<&str>,
    uids: &[u32],
) -> Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = serde_json::json!({
        "ts": ts,
        "account": account,
        "action": action,
        "folder": folder,
        "dest": dest,
        "uids": uids,
    });

    let path = path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{entry}")?;
    Ok(())
}
