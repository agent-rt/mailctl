//! 瞬时错误重试（指数退避）。
//!
//! 只用于**幂等**环节——连接建立（TLS + 登录/认证）与 OAuth token 请求。
//! 写操作（发信/移动/标志）不走重试，避免响应丢失导致的重复执行。

use crate::error::Result;
use std::time::Duration;

const MAX_ATTEMPTS: u32 = 3;

/// 运行 `f`；瞬时错误按指数退避重试（500ms、1000ms），最多 `MAX_ATTEMPTS` 次。
/// 非瞬时错误立即返回。
pub fn with_retry<T>(f: impl Fn() -> Result<T>) -> Result<T> {
    let mut attempt: u32 = 1;
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(e) if attempt < MAX_ATTEMPTS && e.is_transient() => {
                let backoff = Duration::from_millis(250 * 2u64.pow(attempt));
                // 提示走 stderr，不污染 stdout 的 JSON。
                eprintln!(
                    "[retry] 瞬时错误，{}ms 后重试（第 {}/{} 次）: {e}",
                    backoff.as_millis(),
                    attempt + 1,
                    MAX_ATTEMPTS
                );
                std::thread::sleep(backoff);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}
