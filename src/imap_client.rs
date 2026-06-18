//! IMAP 协议层。短生命周期 CLI（每次调用一进程）下采用同步阻塞实现，
//! 避免 async 运行时的启动开销与复杂度——见文末性能/安全报告。

use crate::auth;
use crate::config::Account;
use crate::error::{Error, Result};
use crate::model::{FolderInfo, MessageMeta};
use crate::oauth;
use imap::types::{Flag, NameAttribute};
use imap_proto::types::Address;
use native_tls::TlsStream;
use std::net::TcpStream;

/// XOAUTH2 SASL 认证器：返回 RFC 7628 格式的初始响应，imap crate 负责 base64 编码。
struct XOAuth2 {
    user: String,
    access_token: String,
}

impl imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&self, _challenge: &[u8]) -> Self::Response {
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}

/// 单条 UID 命令（MOVE/STORE）的最大 UID 数：大集合分批，避免巨型命令拖慢/被拒。
const UID_CHUNK: usize = 50;

pub struct ImapClient {
    session: imap::Session<TlsStream<TcpStream>>,
}

impl ImapClient {
    /// 建立 TLS 会话。认证方式由是否配置 `client_id` 决定（与 provider 无关）：
    /// 有 client_id → OAuth2 XOAUTH2（Gmail 或 Hotmail）；无 → App Password LOGIN（Gmail）。
    pub fn connect(account: &Account) -> Result<Self> {
        let host = account.provider.imap_host();
        let port = account.provider.imap_port();
        let tls = native_tls::TlsConnector::builder().build()?;
        let client = imap::connect((host, port), host, &tls)?;

        let session = if account.client_id.is_some() {
            let access_token = oauth::access_token_for(account)?;
            let authenticator = XOAuth2 {
                user: account.email.clone(),
                access_token,
            };
            client
                .authenticate("XOAUTH2", &authenticator)
                .map_err(|(e, _client)| Error::Imap(e))?
        } else {
            let password = auth::load_password(account)?;
            client
                .login(&account.email, &password)
                .map_err(|(e, _client)| Error::Imap(e))?
        };
        Ok(Self { session })
    }

    /// SELECT 文件夹并（可选）校验 UIDVALIDITY。返回当前 UIDVALIDITY。
    /// `expect` 为 Agent 上次 search 看到的值——不一致说明邮箱已重建、UID 失效，立即中止。
    fn select_checked(&mut self, folder: &str, expect: Option<u32>) -> Result<u32> {
        let mailbox = self.session.select(wire(folder))?;
        let actual = mailbox.uid_validity.unwrap_or(0);
        if let Some(expected) = expect
            && expected != actual
        {
            return Err(Error::UidValidityMismatch { expected, actual });
        }
        Ok(actual)
    }

    /// 搜索并返回 (UIDVALIDITY, 元数据)。结果按 UID 降序（新邮件在前）截断到 `limit`。
    pub fn search(
        &mut self,
        folder: &str,
        criteria: &str,
        limit: usize,
        expect: Option<u32>,
    ) -> Result<(u32, Vec<MessageMeta>)> {
        let uidvalidity = self.select_checked(folder, expect)?;
        let mut uids: Vec<u32> = self.session.uid_search(criteria)?.into_iter().collect();
        uids.sort_unstable_by_key(|&u| std::cmp::Reverse(u));
        uids.truncate(limit);
        Ok((uidvalidity, self.fetch_metas(&uids)?))
    }

    /// 取指定 UID 集合的 (UIDVALIDITY, 元数据)。trash 预览复用，让用户/Agent 删除前按真实主题二次确认。
    pub fn meta(
        &mut self,
        folder: &str,
        uids: &[u32],
        expect: Option<u32>,
    ) -> Result<(u32, Vec<MessageMeta>)> {
        let uidvalidity = self.select_checked(folder, expect)?;
        Ok((uidvalidity, self.fetch_metas(uids)?))
    }

    /// SELECT 文件夹并返回 (UIDVALIDITY, UIDNEXT)。sync 用。
    pub fn select_state(&mut self, folder: &str) -> Result<(u32, u32)> {
        let mailbox = self.session.select(wire(folder))?;
        Ok((
            mailbox.uid_validity.unwrap_or(0),
            mailbox.uid_next.unwrap_or(0),
        ))
    }

    /// 当前已选文件夹的全部 UID（UID SEARCH ALL）。便宜，只回 UID。
    pub fn uid_search_all(&mut self) -> Result<Vec<u32>> {
        Ok(self.session.uid_search("ALL")?.into_iter().collect())
    }

    /// 拉取一批 UID 的 unread 标志（仅 FLAGS，轻量）。sync 刷新已缓存邮件用。
    pub fn fetch_unread(&mut self, uids: &[u32]) -> Result<Vec<(u32, bool)>> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let fetches = self.session.uid_fetch(&set, "(UID FLAGS)")?;
        let mut out = Vec::with_capacity(fetches.len());
        for f in fetches.iter() {
            let unread = !f.flags().iter().any(|fl| matches!(fl, Flag::Seen));
            out.push((f.uid.unwrap_or(0), unread));
        }
        Ok(out)
    }

    /// 取指定 UID 集合的完整元数据（文件夹须已 SELECT）。sync 拉新邮件用。
    pub fn fetch_metas(&mut self, uids: &[u32]) -> Result<Vec<MessageMeta>> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // 连 HEADER 一起 PEEK（不设 \Seen），用于 List-Unsubscribe 检测。
        let fetches = self
            .session
            .uid_fetch(&set, "(UID ENVELOPE FLAGS RFC822.SIZE BODY.PEEK[HEADER])")?;

        let mut out = Vec::with_capacity(fetches.len());
        for f in fetches.iter() {
            let envelope = f.envelope();
            let subject = envelope
                .and_then(|e| e.subject)
                .map(decode_word)
                .unwrap_or_default();
            let from = envelope
                .and_then(|e| e.from.as_ref())
                .and_then(|addrs| addrs.first())
                .map(format_address)
                .unwrap_or_default();
            let date = envelope
                .and_then(|e| e.date.as_ref())
                .map(|d| String::from_utf8_lossy(d).into_owned());
            let unread = !f.flags().iter().any(|fl| matches!(fl, Flag::Seen));
            let is_bulk = f.header().is_some_and(detect_bulk);

            out.push(MessageMeta {
                uid: f.uid.unwrap_or(0),
                from,
                subject,
                date,
                unread,
                size: f.size,
                is_bulk,
            });
        }
        // fetch 不保证按请求顺序返回，这里恢复 UID 降序。
        out.sort_unstable_by_key(|m| std::cmp::Reverse(m.uid));
        Ok(out)
    }

    /// 把一批 UID 从 `source` 移动到 `dest`（IMAP MOVE）。trash/restore 共用。
    /// 返回 source 的 UIDVALIDITY，供审计与未来的一致性校验。
    /// 大集合自动分批（同一会话多条 `UID MOVE`），避免单条巨型命令被服务器拒绝/拖慢。
    pub fn move_messages(
        &mut self,
        source: &str,
        uids: &[u32],
        dest: &str,
        expect: Option<u32>,
    ) -> Result<u32> {
        let uidvalidity = self.select_checked(source, expect)?;
        let dest_wire = wire(dest);
        for chunk in uids.chunks(UID_CHUNK) {
            let set = chunk
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            self.session.uid_mv(&set, &dest_wire)?;
        }
        Ok(uidvalidity)
    }

    /// SELECT 文件夹并返回其 UIDVALIDITY（正文缓存用它作 key 的一部分）。
    pub fn select_folder(&mut self, folder: &str) -> Result<u32> {
        self.select_checked(folder, None)
    }

    /// 拉取单封邮件原文字节（文件夹须已 SELECT）。
    /// 用 `BODY.PEEK[]` 而非 `RFC822`，避免读取副作用——不自动打 `\Seen`。
    pub fn fetch_body(&mut self, uid: u32) -> Result<Vec<u8>> {
        let fetches = self
            .session
            .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")?;
        let fetch = fetches.iter().next().ok_or(Error::MessageNotFound(uid))?;
        let raw = fetch.body().ok_or(Error::MessageNotFound(uid))?;
        Ok(raw.to_vec())
    }

    /// 批量给邮件加 flag（如 `\\Seen`、`\\Flagged`）。大集合自动分批。
    pub fn add_flags(
        &mut self,
        folder: &str,
        uids: &[u32],
        flags: &[&str],
        expect: Option<u32>,
    ) -> Result<()> {
        self.select_checked(folder, expect)?;
        let spec = format!("+FLAGS ({})", flags.join(" "));
        for chunk in uids.chunks(UID_CHUNK) {
            let set = chunk
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            self.session.uid_store(&set, &spec)?;
        }
        Ok(())
    }

    /// 列出所有文件夹/标签（IMAP LIST）。
    pub fn list_folders(&mut self) -> Result<Vec<FolderInfo>> {
        let names = self.session.list(Some(""), Some("*"))?;
        let mut out = Vec::with_capacity(names.len());
        for n in names.iter() {
            let selectable = !n
                .attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::NoSelect));
            out.push(FolderInfo {
                name: human(n.name()),
                selectable,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// 创建文件夹/标签；已存在等错误一律忽略（最终以 move 是否成功为准）。
    pub fn ensure_folder(&mut self, name: &str) {
        let _ = self.session.create(wire(name));
    }

    /// 增删 Gmail 标签（X-GM-LABELS，经 UID STORE）。一封邮件可有多个标签，
    /// 与 move 不同：加标签不会把邮件移出当前位置。
    pub fn modify_labels(
        &mut self,
        folder: &str,
        uids: &[u32],
        add: &[String],
        remove: &[String],
        expect: Option<u32>,
    ) -> Result<()> {
        self.select_checked(folder, expect)?;
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // 用原始命令 + `.SILENT`：X-GM-LABELS 的 FETCH 回显 imap_proto 解析不了，
        // SILENT 抑制回显，只留 tagged OK。
        for label in add {
            self.session.run_command_and_check_ok(format!(
                "UID STORE {set} +X-GM-LABELS.SILENT ({})",
                quote_label(label)
            ))?;
        }
        for label in remove {
            self.session.run_command_and_check_ok(format!(
                "UID STORE {set} -X-GM-LABELS.SILENT ({})",
                quote_label(label)
            ))?;
        }
        Ok(())
    }

    /// APPEND 原始邮件到草稿箱。
    pub fn append_draft(&mut self, drafts_folder: &str, raw: &[u8]) -> Result<()> {
        self.session.append(wire(drafts_folder), raw)?;
        Ok(())
    }

    pub fn logout(mut self) -> Result<()> {
        self.session.logout()?;
        Ok(())
    }
}

/// 把 IMAP ENVELOPE 地址格式化为 `Name <mailbox@host>`（无名时退化为纯地址）。
/// 显示名可能是 RFC 2047 编码词，需解码；mailbox/host 为 ASCII，直接转。
fn format_address(addr: &Address) -> String {
    let ascii = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mailbox = addr.mailbox.map(ascii).unwrap_or_default();
    let host = addr.host.map(ascii).unwrap_or_default();
    let email = if host.is_empty() {
        mailbox
    } else {
        format!("{mailbox}@{host}")
    };
    match addr.name {
        Some(name) => format!("{} <{}>", decode_word(name), email),
        None => email,
    }
}

/// 用户态 UTF-8 文件夹名 → IMAP 线缆格式（modified UTF-7，RFC 3501）。ASCII 名基本不变。
fn wire(name: &str) -> String {
    utf7_imap::encode_utf7_imap(name.to_string())
}

/// 反向：线缆名 → 显示用 UTF-8。
fn human(name: &str) -> String {
    utf7_imap::decode_utf7_imap(name.to_string())
}

/// 把 Gmail 标签名包成 IMAP 带引号字符串（含空格/层级也安全）。
fn quote_label(label: &str) -> String {
    let escaped = label.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// 头部是否为群发/营销邮件。多判据（任一命中即真）：
/// `List-Unsubscribe`、`List-Id`（邮件列表）、`Precedence: bulk|list|junk`。
/// 头部基本是 ASCII，lossy 小写后子串匹配即可。
fn detect_bulk(header: &[u8]) -> bool {
    let h = String::from_utf8_lossy(header).to_ascii_lowercase();
    h.contains("list-unsubscribe")
        || h.contains("list-id:")
        || h.contains("precedence: bulk")
        || h.contains("precedence:bulk")
        || h.contains("precedence: list")
        || h.contains("precedence: junk")
}

/// 解码 RFC 2047 编码词（如 `=?UTF-8?B?...?=`）；失败回退 UTF-8 lossy。
/// 用 `RecoverStrategy::Decode`：现实邮件常违反 75 字符/编码词上限（尤其营销邮），
/// 默认的 Abort 策略会整条报错，这里宽松解码。
fn decode_word(bytes: &[u8]) -> String {
    rfc2047_decoder::Decoder::new()
        .too_long_encoded_word_strategy(rfc2047_decoder::RecoverStrategy::Decode)
        .decode(bytes)
        .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::decode_word;

    #[test]
    fn decodes_base64_utf8_word() {
        // "=?UTF-8?B?5L2g5aW9?=" => "你好"
        assert_eq!(decode_word(b"=?UTF-8?B?5L2g5aW9?="), "你好");
    }

    #[test]
    fn decodes_quoted_printable_word() {
        // "=?ISO-8859-1?Q?=A1Hola!?=" => "¡Hola!"
        assert_eq!(decode_word(b"=?ISO-8859-1?Q?=A1Hola!?="), "¡Hola!");
    }

    #[test]
    fn decodes_overlong_encoded_word() {
        // 真机遇到的超 75 字符单编码词（违反 RFC 2047 上限），需宽松解码而非整条回退。
        let s = decode_word(
            b"=?UTF-8?B?44CQ5YWo5ZOh44KC44KJ44GI44KL44CR6ZmQ5a6aTkZU77yGNTAwTklEVOOCkuODl+ODrOOCvOODs+ODiO+8iFBS77yJ?=",
        );
        assert!(!s.contains("=?"), "应已解码，而非保留编码词: {s}");
        assert!(s.starts_with("【全員もらえる】"), "解码内容异常: {s}");
    }

    #[test]
    fn detects_bulk_via_list_unsubscribe() {
        use super::detect_bulk;
        let marketing =
            b"From: shop@x.com\r\nList-Unsubscribe: <https://x.com/u>\r\nSubject: sale\r\n";
        let personal = b"From: alice@x.com\r\nTo: bob@y.com\r\nSubject: lunch?\r\n";
        // 字段名大小写不敏感
        let lower_case = b"from: a@b\r\nlist-unsubscribe: <mailto:u@b>\r\n";
        let precedence = b"From: n@x.com\r\nPrecedence: bulk\r\nSubject: promo\r\n";
        let list_id = b"From: n@x.com\r\nList-Id: <dev.example.com>\r\nSubject: digest\r\n";
        assert!(detect_bulk(marketing));
        assert!(!detect_bulk(personal));
        assert!(detect_bulk(lower_case));
        assert!(detect_bulk(precedence));
        assert!(detect_bulk(list_id));
    }

    #[test]
    fn passes_through_plain_ascii() {
        assert_eq!(decode_word(b"Hello World"), "Hello World");
    }

    #[test]
    fn falls_back_on_invalid_utf8() {
        // 非编码词、非法 UTF-8：不 panic，lossy 回退。
        assert_eq!(decode_word(&[0xff, 0xfe]), "\u{fffd}\u{fffd}");
    }
}
