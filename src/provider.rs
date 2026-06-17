//! 邮件服务商抽象。用 `enum` 建模而非多个 bool/字符串，保证「非法状态不可达」。
//! 新增 provider 只需在此扩一个分支，协议层 (imap/smtp) 完全复用。

use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gmail,
    Hotmail,
}

impl Provider {
    /// IMAP over implicit TLS (993)。
    pub const fn imap_host(self) -> &'static str {
        match self {
            Provider::Gmail => "imap.gmail.com",
            Provider::Hotmail => "outlook.office365.com",
        }
    }

    pub const fn imap_port(self) -> u16 {
        993
    }

    /// SMTP submission（lettre `relay` 默认走 587 STARTTLS）。
    pub const fn smtp_host(self) -> &'static str {
        match self {
            Provider::Gmail => "smtp.gmail.com",
            Provider::Hotmail => "smtp-mail.outlook.com",
        }
    }

    /// 回收站文件夹（IMAP MOVE 目标），各家命名不同。
    pub const fn trash_folder(self) -> &'static str {
        match self {
            Provider::Gmail => "[Gmail]/Trash",
            Provider::Hotmail => "Deleted",
        }
    }

    /// 草稿箱文件夹（IMAP APPEND 目标）。
    pub const fn drafts_folder(self) -> &'static str {
        match self {
            Provider::Gmail => "[Gmail]/Drafts",
            Provider::Hotmail => "Drafts",
        }
    }
}

impl FromStr for Provider {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "gmail" => Ok(Provider::Gmail),
            "hotmail" | "outlook" => Ok(Provider::Hotmail),
            other => Err(Error::UnknownProvider(other.to_string())),
        }
    }
}
