//! 本地 SQLite 缓存（Phase 1：邮件正文）。
//!
//! 正文按 `(account, uidvalidity, uid)` 缓存——这三元组下邮件内容**不可变**，
//! 所以缓存命中永远正确，无陈旧风险。`read` 仍会 SELECT 拿当前 uidvalidity，
//! 命中即跳过最重的正文 FETCH。
//!
//! 缓存是「优化」而非「真相」：上层把所有缓存操作当 best-effort，
//! 打开/读写失败都不应阻断核心功能。

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: i64 = 1;

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
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
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
    }
    // 未来版本：在此按 SCHEMA_VERSION 递增迁移。
    let _ = SCHEMA_VERSION;
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

/// (条数, 总字节)。
pub fn info(conn: &Connection) -> Result<(i64, i64)> {
    let row = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(LENGTH(raw)), 0) FROM bodies",
        [],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    Ok(row)
}

/// 清空所有缓存正文。
pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute_batch("DELETE FROM bodies;")?;
    Ok(())
}
