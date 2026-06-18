//! 本地 SQLite 缓存（Phase 1：邮件正文）。
//!
//! 正文按 `(account, uidvalidity, uid)` 缓存——这三元组下邮件内容**不可变**，
//! 所以缓存命中永远正确，无陈旧风险。`read` 仍会 SELECT 拿当前 uidvalidity，
//! 命中即跳过最重的正文 FETCH。
//!
//! 缓存是「优化」而非「真相」：上层把所有缓存操作当 best-effort，
//! 打开/读写失败都不应阻断核心功能。

use crate::error::{Error, Result};
use crate::model::MessageMeta;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: i64 = 2;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn db_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "mailctl")
        .ok_or_else(|| Error::Config("无法定位缓存目录".to_string()))?;
    Ok(dirs.cache_dir().join("cache.db"))
}

/// 打开（必要时创建）缓存库，启用 WAL，并跑迁移。
pub fn open() -> Result<Connection> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)?;
    // WAL 支持并发读（未来 daemon 也安全）；busy_timeout 避免瞬时锁失败。
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bodies (
                 account     TEXT    NOT NULL,
                 uidvalidity INTEGER NOT NULL,
                 uid         INTEGER NOT NULL,
                 raw         BLOB    NOT NULL,
                 cached_at   INTEGER NOT NULL,
                 PRIMARY KEY (account, uidvalidity, uid)
             ) WITHOUT ROWID;
             PRAGMA user_version = 1;",
        )?;
        version = 1;
    }
    if version < 2 {
        // Phase 2：元数据缓存。folders 记每个文件夹的同步状态；
        // messages 存元数据（含 unread/is_bulk），供 `search --cached`。
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folders (
                 account     TEXT    NOT NULL,
                 name        TEXT    NOT NULL,
                 uidvalidity INTEGER NOT NULL,
                 uidnext     INTEGER,
                 last_sync   INTEGER NOT NULL,
                 PRIMARY KEY (account, name)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS messages (
                 account     TEXT    NOT NULL,
                 folder      TEXT    NOT NULL,
                 uidvalidity INTEGER NOT NULL,
                 uid         INTEGER NOT NULL,
                 from_addr   TEXT    NOT NULL,
                 subject     TEXT    NOT NULL,
                 date        TEXT,
                 unread      INTEGER NOT NULL,
                 size        INTEGER,
                 is_bulk     INTEGER NOT NULL,
                 PRIMARY KEY (account, folder, uidvalidity, uid)
             ) WITHOUT ROWID;
             PRAGMA user_version = 2;",
        )?;
        version = 2;
    }
    debug_assert_eq!(version, SCHEMA_VERSION);
    let _ = version;
    Ok(())
}

/// 取缓存正文；未命中返回 `None`。
pub fn get_body(
    conn: &Connection,
    account: &str,
    uidvalidity: u32,
    uid: u32,
) -> Result<Option<Vec<u8>>> {
    let raw = conn
        .query_row(
            "SELECT raw FROM bodies WHERE account = ?1 AND uidvalidity = ?2 AND uid = ?3",
            params![account, uidvalidity as i64, uid as i64],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    Ok(raw)
}

/// 写入缓存正文（已存在则覆盖）。
pub fn put_body(
    conn: &Connection,
    account: &str,
    uidvalidity: u32,
    uid: u32,
    raw: &[u8],
) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT OR REPLACE INTO bodies (account, uidvalidity, uid, raw, cached_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account, uidvalidity as i64, uid as i64, raw, now],
    )?;
    Ok(())
}

/// (正文条数, 正文总字节, 元数据条数)。
pub fn info(conn: &Connection) -> Result<(i64, i64, i64)> {
    let (bodies, bytes) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(LENGTH(raw)), 0) FROM bodies",
        [],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    let messages: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    Ok((bodies, bytes, messages))
}

/// 清空全部缓存（正文 + 元数据 + 同步状态）。
pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute_batch("DELETE FROM bodies; DELETE FROM messages; DELETE FROM folders;")?;
    Ok(())
}

// ---- Phase 2：元数据缓存 ----

/// 文件夹同步状态：(uidvalidity, last_sync)。
pub fn folder_state(conn: &Connection, account: &str, folder: &str) -> Result<Option<(u32, i64)>> {
    let row = conn
        .query_row(
            "SELECT uidvalidity, last_sync FROM folders WHERE account = ?1 AND name = ?2",
            params![account, folder],
            |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(row)
}

pub fn set_folder_state(
    conn: &Connection,
    account: &str,
    folder: &str,
    uidvalidity: u32,
    uidnext: Option<u32>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO folders (account, name, uidvalidity, uidnext, last_sync)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            account,
            folder,
            uidvalidity as i64,
            uidnext.map(|x| x as i64),
            now_unix()
        ],
    )?;
    Ok(())
}

/// 丢弃某文件夹全部缓存元数据（UIDVALIDITY 变更时调用）。
pub fn clear_folder(conn: &Connection, account: &str, folder: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM messages WHERE account = ?1 AND folder = ?2",
        params![account, folder],
    )?;
    Ok(())
}

/// 当前已缓存的 UID 集合。
pub fn cached_uids(
    conn: &Connection,
    account: &str,
    folder: &str,
    uidvalidity: u32,
) -> Result<Vec<u32>> {
    let mut stmt = conn.prepare(
        "SELECT uid FROM messages WHERE account = ?1 AND folder = ?2 AND uidvalidity = ?3",
    )?;
    let rows = stmt.query_map(params![account, folder, uidvalidity as i64], |r| {
        r.get::<_, i64>(0)
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r? as u32);
    }
    Ok(out)
}

pub fn upsert_messages(
    conn: &mut Connection,
    account: &str,
    folder: &str,
    uidvalidity: u32,
    metas: &[MessageMeta],
) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO messages
             (account, folder, uidvalidity, uid, from_addr, subject, date, unread, size, is_bulk)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for m in metas {
            stmt.execute(params![
                account,
                folder,
                uidvalidity as i64,
                m.uid as i64,
                m.from,
                m.subject,
                m.date,
                m.unread as i64,
                m.size.map(|s| s as i64),
                m.is_bulk as i64,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_messages(
    conn: &mut Connection,
    account: &str,
    folder: &str,
    uidvalidity: u32,
    uids: &[u32],
) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "DELETE FROM messages WHERE account = ?1 AND folder = ?2 AND uidvalidity = ?3 AND uid = ?4",
        )?;
        for &uid in uids {
            stmt.execute(params![account, folder, uidvalidity as i64, uid as i64])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 刷新已缓存邮件的 unread 标志（sync 时从服务器重新拉 FLAGS）。
pub fn update_unread(
    conn: &mut Connection,
    account: &str,
    folder: &str,
    uidvalidity: u32,
    items: &[(u32, bool)],
) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "UPDATE messages SET unread = ?5
             WHERE account = ?1 AND folder = ?2 AND uidvalidity = ?3 AND uid = ?4",
        )?;
        for &(uid, unread) in items {
            stmt.execute(params![
                account,
                folder,
                uidvalidity as i64,
                uid as i64,
                unread as i64
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 读出某文件夹全部缓存元数据，按 UID 降序（新邮件在前）。`search --cached` 用。
pub fn all_messages(
    conn: &Connection,
    account: &str,
    folder: &str,
    uidvalidity: u32,
) -> Result<Vec<MessageMeta>> {
    let mut stmt = conn.prepare(
        "SELECT uid, from_addr, subject, date, unread, size, is_bulk
         FROM messages WHERE account = ?1 AND folder = ?2 AND uidvalidity = ?3
         ORDER BY uid DESC",
    )?;
    let rows = stmt.query_map(params![account, folder, uidvalidity as i64], |r| {
        Ok(MessageMeta {
            uid: r.get::<_, i64>(0)? as u32,
            from: r.get(1)?,
            subject: r.get(2)?,
            date: r.get(3)?,
            unread: r.get::<_, i64>(4)? != 0,
            size: r.get::<_, Option<i64>>(5)?.map(|x| x as u32),
            is_bulk: r.get::<_, i64>(6)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
