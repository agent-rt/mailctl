//! 凭据存储。两种后端，按账户的 `secret_ref` 自动探测：
//!
//! - **env（secretctl）**：账户配了 `secret_ref` 且对应环境变量存在（通常由
//!   `secretctl exec --only <ref> -- ...` 注入）时，主密钥从环境变量读，**完全不碰 keychain**。
//! - **keychain（默认）**：主密钥存系统钥匙串（App Password / refresh_token）。
//!
//! access_token 缓存一律落 **0600 文件**（不进 keychain）——命中缓存的命令零钥匙串访问，
//! 这是减少弹窗的关键。

use crate::config::Account;
use crate::error::{Error, Result};
use std::io::Write;
use std::path::PathBuf;

const SERVICE: &str = "mailctl";

fn entry(account: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, account)?)
}

fn refresh_key(email: &str) -> String {
    format!("{email}#refresh")
}

fn oauth_secret_key(email: &str) -> String {
    format!("{email}#oauth-secret")
}

/// 账户配了 secret_ref 且环境变量存在时返回其值（即「env 后端生效」）。
fn env_secret(account: &Account) -> Option<String> {
    account
        .secret_ref
        .as_ref()
        .and_then(|name| std::env::var(name).ok())
}

// ---- 主密钥：Gmail = App Password / Hotmail = refresh_token 种子 ----

pub fn load_password(account: &Account) -> Result<String> {
    if let Some(v) = env_secret(account) {
        return Ok(v);
    }
    Ok(entry(&account.email)?.get_password()?)
}

pub fn store_password(email: &str, password: &str) -> Result<()> {
    entry(email)?.set_password(password)?;
    Ok(())
}

pub fn load_refresh_token(account: &Account) -> Result<String> {
    if let Some(v) = env_secret(account) {
        return Ok(v);
    }
    Ok(entry(&refresh_key(&account.email))?.get_password()?)
}

pub fn store_refresh_token(email: &str, token: &str) -> Result<()> {
    entry(&refresh_key(email))?.set_password(token)?;
    Ok(())
}

/// OAuth client_secret（Gmail Desktop 客户端需要；Google 视其为非机密但必传）。存钥匙串。
pub fn store_oauth_secret(email: &str, secret: &str) -> Result<()> {
    entry(&oauth_secret_key(email))?.set_password(secret)?;
    Ok(())
}

pub fn load_oauth_secret(account: &Account) -> Result<String> {
    Ok(entry(&oauth_secret_key(&account.email))?.get_password()?)
}

/// 轮换后的 refresh_token 持久化。env 模式下无法写回 secretctl，跳过——
/// 微软 refresh_token 90 天有效，到期重新 `auth login` 即可。
pub fn persist_rotated_refresh(account: &Account, token: &str) -> Result<()> {
    if env_secret(account).is_some() {
        return Ok(());
    }
    store_refresh_token(&account.email, token)
}

// ---- access_token 缓存：0600 文件，不进 keychain ----

fn cache_file(email: &str) -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "mailctl")
        .ok_or_else(|| Error::Config("无法定位缓存目录".to_string()))?;
    Ok(dirs.cache_dir().join(format!("{email}.token")))
}

pub fn store_access_cache(email: &str, blob: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = cache_file(email)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600) // 仅属主可读写
        .open(&path)?;
    file.write_all(blob.as_bytes())?;
    Ok(())
}

pub fn load_access_cache(email: &str) -> Result<String> {
    Ok(std::fs::read_to_string(cache_file(email)?)?)
}

/// 注销时清理全部凭据（钥匙串主密钥 + 缓存文件），任一不存在都不算失败（幂等）。
pub fn delete_all(email: &str) -> Result<()> {
    for key in [
        email.to_string(),
        refresh_key(email),
        oauth_secret_key(email),
    ] {
        if let Ok(e) = entry(&key) {
            let _ = e.delete_credential();
        }
    }
    if let Ok(path) = cache_file(email) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}
