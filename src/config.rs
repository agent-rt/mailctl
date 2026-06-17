//! 账户配置，存于 `~/Library/Application Support/mailctl/config.toml`（macOS）。
//! 只存非敏感元数据；密码/令牌一律走 keyring（见 `auth.rs`）。

use crate::error::{Error, Result};
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub default_account: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    pub provider: Provider,
    /// OAuth public client id（Azure 应用注册）。仅 Hotmail 需要；非敏感，可明文存配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// secretctl 密钥名（= `secretctl exec` 注入的环境变量名）。
    /// 设置后优先从环境变量取主密钥，脱离 keychain。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "mailctl")
            .ok_or_else(|| Error::Config("无法定位配置目录".to_string()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// 解析目标账户：显式 `--account` 优先，否则回退默认账户。
    pub fn resolve(&self, email: Option<&str>) -> Result<&Account> {
        let target = match email {
            Some(e) => e.to_string(),
            None => self
                .default_account
                .clone()
                .ok_or(Error::NoDefaultAccount)?,
        };
        self.accounts
            .iter()
            .find(|a| a.email == target)
            .ok_or(Error::AccountNotFound(target))
    }

    /// 新增或覆盖账户；首个账户自动设为默认。
    pub fn upsert(&mut self, account: Account) {
        if self.default_account.is_none() {
            self.default_account = Some(account.email.clone());
        }
        match self.accounts.iter_mut().find(|a| a.email == account.email) {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
    }

    pub fn remove(&mut self, email: &str) {
        self.accounts.retain(|a| a.email != email);
        if self.default_account.as_deref() == Some(email) {
            self.default_account = self.accounts.first().map(|a| a.email.clone());
        }
    }
}
