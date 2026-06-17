//! mailctl —— Agent 友好的邮件 CLI。
//! 顶层只做：解析命令 → 分发 → 统一以 JSON 输出结果或错误（稳定退出码）。

mod audit;
mod auth;
mod cli;
mod config;
mod error;
mod imap_client;
mod mime;
mod model;
mod oauth;
mod provider;
mod smtp_client;

use clap::Parser;
use cli::{AuthAction, Cli, Command};
use config::{Account, Config};
use error::{Error, Result};
use imap_client::ImapClient;
use model::{ActionResult, SearchResult, print_json};
use provider::Provider;
use serde_json::json;
use std::str::FromStr;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        // 错误也走机器可读 JSON（stderr），退出码非零，便于 Agent 判定。
        eprintln!("{}", json!({ "ok": false, "error": e.to_string() }));
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Auth { action } => run_auth(action),
        Command::Search {
            query,
            limit,
            expect_uidvalidity,
        } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            let criteria = translate_query(query.as_deref());
            let mut client = ImapClient::connect(account)?;
            let (uidvalidity, messages) =
                client.search(&cli.folder, &criteria, limit, expect_uidvalidity)?;
            client.logout()?;
            print_json(&SearchResult {
                folder: cli.folder,
                uidvalidity,
                messages,
            })
        }
        Command::Read { uid } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            let mut client = ImapClient::connect(account)?;
            let body = client.read(&cli.folder, uid)?;
            client.logout()?;
            print_json(&body)
        }
        Command::Flag { uid, read, star } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            let mut client = ImapClient::connect(account)?;
            let mut applied = Vec::new();
            if read {
                client.add_flag(&cli.folder, uid, "\\Seen")?;
                applied.push("read");
            }
            if star {
                client.add_flag(&cli.folder, uid, "\\Flagged")?;
                applied.push("star");
            }
            client.logout()?;
            print_json(&ActionResult {
                ok: true,
                action: "flag",
                uid: Some(uid),
                detail: format!("已应用: {}", applied.join(", ")),
            })
        }
        Command::Trash {
            uids,
            confirm,
            expect_uidvalidity,
        } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            if !confirm {
                // 预览：拉真实主题给用户/Agent 二次确认，不改动任何邮件。
                let mut client = ImapClient::connect(account)?;
                let (uidvalidity, metas) = client.meta(&cli.folder, &uids, expect_uidvalidity)?;
                client.logout()?;
                print_json(&json!({
                    "action": "trash",
                    "executed": false,
                    "folder": cli.folder,
                    "uidvalidity": uidvalidity,
                    "would_trash": metas,
                    "hint": "这是预览，未改动任何邮件。确认时加 --confirm，并带 --expect-uidvalidity <上面的 uidvalidity> 防 UID 失效。",
                }))
            } else {
                let trash = account.provider.trash_folder();
                let n = uids.len();
                // 先记审计意图，再动手。
                audit::record(&account.email, "trash", &cli.folder, Some(trash), &uids)?;
                let mut client = ImapClient::connect(account)?;
                client.move_messages(&cli.folder, &uids, trash, expect_uidvalidity)?;
                client.logout()?;
                print_json(&json!({
                    "ok": true,
                    "action": "trash",
                    "executed": true,
                    "trashed": uids,
                    "dest": trash,
                    "detail": format!("已移动 {n} 封到 {trash}（30 天内可 restore 找回）"),
                }))
            }
        }
        Command::Restore {
            uids,
            to,
            expect_uidvalidity,
        } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            let trash = account.provider.trash_folder();
            let n = uids.len();
            audit::record(&account.email, "restore", trash, Some(&to), &uids)?;
            let mut client = ImapClient::connect(account)?;
            client.move_messages(trash, &uids, &to, expect_uidvalidity)?;
            client.logout()?;
            print_json(&json!({
                "ok": true,
                "action": "restore",
                "restored": uids,
                "from": trash,
                "to": to,
                "detail": format!("已从 {trash} 恢复 {n} 封到 {to}"),
            }))
        }
        Command::Folders => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            let mut client = ImapClient::connect(account)?;
            let folders = client.list_folders()?;
            client.logout()?;
            print_json(&folders)
        }
        Command::Move {
            uids,
            to,
            create,
            expect_uidvalidity,
        } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            let n = uids.len();
            audit::record(&account.email, "move", &cli.folder, Some(&to), &uids)?;
            let mut client = ImapClient::connect(account)?;
            if create {
                client.ensure_folder(&to);
            }
            client.move_messages(&cli.folder, &uids, &to, expect_uidvalidity)?;
            client.logout()?;
            print_json(&json!({
                "ok": true,
                "action": "move",
                "moved": uids,
                "from": cli.folder,
                "to": to,
                "detail": format!("已移动 {n} 封：{} -> {to}", cli.folder),
            }))
        }
        Command::Label {
            uids,
            add,
            remove,
            expect_uidvalidity,
        } => {
            let config = Config::load()?;
            let account = config.resolve(cli.account.as_deref())?;
            if account.provider != Provider::Gmail {
                return Err(Error::Other(
                    "标签仅 Gmail 支持；Hotmail 请用 `move` 移动到文件夹".to_string(),
                ));
            }
            if add.is_empty() && remove.is_empty() {
                return Err(Error::Other("请用 --add 或 --remove 指定标签".to_string()));
            }
            let n = uids.len();
            audit::record(&account.email, "label", &cli.folder, None, &uids)?;
            let mut client = ImapClient::connect(account)?;
            client.modify_labels(&cli.folder, &uids, &add, &remove, expect_uidvalidity)?;
            client.logout()?;
            print_json(&json!({
                "ok": true,
                "action": "label",
                "uids": uids,
                "added": add,
                "removed": remove,
                "detail": format!("已更新 {n} 封的标签"),
            }))
        }
        Command::Send {
            to,
            subject,
            body_file,
            body,
            confirm,
        } => run_send(
            cli.account.as_deref(),
            to,
            subject,
            body_file,
            body,
            confirm,
        ),
    }
}

fn run_auth(action: AuthAction) -> Result<()> {
    match action {
        AuthAction::Login {
            provider,
            email,
            password,
            client_id,
            secret_ref,
        } => {
            let provider = Provider::from_str(&provider)?;
            let stored_client_id = match provider {
                Provider::Gmail => {
                    // secret_ref 模式：主密钥在 secretctl，运行时经 env 注入，不写 keychain。
                    if secret_ref.is_none() {
                        let password = match password {
                            Some(p) => p,
                            None => rpassword::prompt_password("App Password: ")?,
                        };
                        auth::store_password(&email, &password)?;
                    }
                    None
                }
                Provider::Hotmail => {
                    let client_id = client_id.ok_or_else(|| {
                        Error::OAuth(
                            "Hotmail 需要 --client-id（Azure 应用注册的 public client id）".to_string(),
                        )
                    })?;
                    let refresh_token = oauth::interactive_login(&client_id)?;
                    // refresh_token 存 keychain（每小时刷新时读一次）；access 缓存走 0600 文件。
                    auth::store_refresh_token(&email, &refresh_token)?;
                    Some(client_id)
                }
            };

            let mode = if secret_ref.is_some() {
                "secretctl/env"
            } else {
                "keychain"
            };
            let mut config = Config::load()?;
            config.upsert(Account {
                email: email.clone(),
                provider,
                client_id: stored_client_id,
                secret_ref,
            });
            config.save()?;

            print_json(&ActionResult {
                ok: true,
                action: "login",
                uid: None,
                detail: format!("已保存账户 {email}（{provider:?}，主密钥后端: {mode}）"),
            })
        }
        AuthAction::List => {
            let config = Config::load()?;
            print_json(&json!({
                "default_account": config.default_account,
                "accounts": config.accounts,
            }))
        }
        AuthAction::Logout { email } => {
            // 即使钥匙串中无此项也继续清理配置，保证幂等。
            auth::delete_all(&email)?;
            let mut config = Config::load()?;
            config.remove(&email);
            config.save()?;
            print_json(&ActionResult {
                ok: true,
                action: "logout",
                uid: None,
                detail: format!("已注销 {email}"),
            })
        }
    }
}

fn run_send(
    account_sel: Option<&str>,
    to: Vec<String>,
    subject: String,
    body_file: Option<std::path::PathBuf>,
    body: Option<String>,
    confirm: bool,
) -> Result<()> {
    let config = Config::load()?;
    let account = config.resolve(account_sel)?;

    let body_text = match (body, body_file) {
        (Some(b), _) => b,
        (None, Some(path)) => std::fs::read_to_string(path)?,
        (None, None) => String::new(),
    };

    let message = smtp_client::build(account, &to, &subject, &body_text)?;

    if confirm {
        smtp_client::send(account, &message)?;
        print_json(&ActionResult {
            ok: true,
            action: "send",
            uid: None,
            detail: format!("已发送给 {}", to.join(", ")),
        })
    } else {
        // 草稿优先：未确认则 APPEND 到草稿箱，并以错误退出码提示需要确认。
        let drafts = account.provider.drafts_folder();
        let raw = message.formatted();
        let mut client = ImapClient::connect(account)?;
        client.append_draft(drafts, &raw)?;
        client.logout()?;
        Err(Error::SendNotConfirmed)
    }
}

/// 把简易查询语法翻译为 IMAP SEARCH 条件。
fn translate_query(query: Option<&str>) -> String {
    let Some(query) = query else {
        return "ALL".to_string();
    };
    let mut parts = Vec::new();
    for token in query.split_whitespace() {
        if let Some(rest) = token.strip_prefix("from:") {
            parts.push(format!("FROM \"{rest}\""));
        } else if let Some(rest) = token.strip_prefix("to:") {
            parts.push(format!("TO \"{rest}\""));
        } else if let Some(rest) = token.strip_prefix("subject:") {
            parts.push(format!("SUBJECT \"{rest}\""));
        } else if token == "is:unread" {
            parts.push("UNSEEN".to_string());
        } else if token == "is:read" {
            parts.push("SEEN".to_string());
        } else if token == "is:starred" {
            parts.push("FLAGGED".to_string());
        } else {
            parts.push(format!("TEXT \"{token}\""));
        }
    }
    if parts.is_empty() {
        "ALL".to_string()
    } else {
        parts.join(" ")
    }
}
