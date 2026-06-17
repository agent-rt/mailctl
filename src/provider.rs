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

/// 各 provider 的 OAuth2 端点与差异参数。
pub struct OAuthSpec {
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static str,
    /// 追加到 authorize 查询串的差异参数（含前导 `&`）。
    pub extra_auth_params: &'static str,
    /// token 请求是否需要 client_secret（Google Desktop 客户端需要）。
    pub needs_client_secret: bool,
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

    /// OAuth2 端点与差异参数。
    pub const fn oauth_spec(self) -> OAuthSpec {
        match self {
            Provider::Gmail => OAuthSpec {
                authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
                token_url: "https://oauth2.googleapis.com/token",
                scopes: "https://mail.google.com/",
                // offline + consent：拿到 refresh_token 的前提。
                extra_auth_params: "&access_type=offline&prompt=consent",
                needs_client_secret: true,
            },
            Provider::Hotmail => OAuthSpec {
                authorize_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                scopes: "offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
                extra_auth_params: "&response_mode=query",
                needs_client_secret: false,
            },
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
