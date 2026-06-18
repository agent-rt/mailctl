//! OAuth2（Gmail 与 Hotmail/Outlook.com）。
//! authorization-code + PKCE + loopback 回调。端点/scope 由 `Provider::oauth_spec()` 提供：
//! Gmail（Desktop 客户端，需 client_secret）、Hotmail（public client，免 secret）。
//! refresh_token 落钥匙串；每次连接前用 refresh grant 换短期 access_token（带本地缓存）。

use crate::auth;
use crate::config::Account;
use crate::error::{Error, Result};
use crate::provider::Provider;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{SystemTime, UNIX_EPOCH};

/// access_token 视为「即将过期」的安全余量（秒），避免临界点用到失效令牌。
const EXPIRY_MARGIN_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    /// 有效期（秒）；缺省按 1 小时保守估计。
    expires_in: Option<u64>,
}

/// 钥匙串中缓存的 access_token，附绝对过期时间戳（unix 秒）。
#[derive(Debug, Serialize, Deserialize)]
struct CachedToken {
    access_token: String,
    expires_at: u64,
}

/// 交互式登录：本地起回调服务 → 浏览器授权 → 换取 token。返回 refresh_token。
/// `email` 用作 login_hint 预填账户；`client_secret` 仅 Gmail 需要。
pub fn interactive_login(
    provider: Provider,
    client_id: &str,
    client_secret: Option<&str>,
    email: &str,
) -> Result<String> {
    let spec = provider.oauth_spec();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://localhost:{port}");

    let verifier = generate_verifier()?;
    let challenge = code_challenge(&verifier);
    let state = generate_verifier()?; // 复用随机源做 CSRF state

    let auth_url = format!(
        "{authorize}?client_id={cid}&response_type=code&redirect_uri={redir}\
         &scope={scope}&state={state}&login_hint={hint}\
         &code_challenge={challenge}&code_challenge_method=S256{extra}",
        authorize = spec.authorize_url,
        cid = enc(client_id),
        redir = enc(&redirect_uri),
        scope = enc(spec.scopes),
        state = enc(&state),
        hint = enc(email),
        challenge = enc(&challenge),
        extra = spec.extra_auth_params,
    );

    // 尽力自动打开浏览器；失败则提示用户手动访问。
    let _ = std::process::Command::new("open").arg(&auth_url).spawn();
    eprintln!("请在浏览器中完成授权；若未自动打开，请访问:\n{auth_url}\n");

    let (code, returned_state) = wait_for_redirect(&listener)?;
    if returned_state != state {
        return Err(Error::OAuth("state 不匹配，疑似 CSRF，已中止".to_string()));
    }

    let mut form = vec![
        ("client_id", client_id),
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
        ("scope", spec.scopes),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    let resp = post_token(spec.token_url, &form)?;
    resp.refresh_token
        .ok_or_else(|| Error::OAuth("授权响应缺少 refresh_token（Gmail 需 access_type=offline+prompt=consent；MS 需 offline_access scope）".to_string()))
}

/// 取可用 access_token。优先复用钥匙串里未过期的缓存（省去每次刷新的网络往返）；
/// 否则用 refresh_token 刷新，回写新缓存，并在服务端轮换 refresh_token 时一并更新。
pub fn access_token_for(account: &Account) -> Result<String> {
    let spec = account.provider.oauth_spec();
    let client_id = account.client_id.as_deref().ok_or_else(|| {
        Error::OAuth(format!(
            "账户 {} 缺少 client_id，请重新 login",
            account.email
        ))
    })?;

    // 1. 缓存命中（且距过期还有余量）则直接用。
    if let Some(token) = cached_access_token(&account.email)? {
        return Ok(token);
    }

    // 2. 刷新。
    let refresh = auth::load_refresh_token(account)?;
    let secret = if spec.needs_client_secret {
        Some(auth::load_oauth_secret(account)?)
    } else {
        None
    };
    let mut form = vec![
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.as_str()),
        ("scope", spec.scopes),
    ];
    if let Some(s) = &secret {
        form.push(("client_secret", s.as_str()));
    }
    let resp = post_token(spec.token_url, &form)?;

    if let Some(new_refresh) = resp.refresh_token
        && new_refresh != refresh
    {
        auth::persist_rotated_refresh(account, &new_refresh)?;
    }

    // 3. 写缓存（失败不致命，下次重新刷新即可）。
    let expires_at = now_unix()? + resp.expires_in.unwrap_or(3600);
    let cached = CachedToken {
        access_token: resp.access_token.clone(),
        expires_at,
    };
    if let Ok(blob) = serde_json::to_string(&cached) {
        let _ = auth::store_access_cache(&account.email, &blob);
    }
    Ok(resp.access_token)
}

/// 读缓存：仅当存在、可解析且距过期 > 余量时返回令牌，否则 `None`（触发刷新）。
fn cached_access_token(email: &str) -> Result<Option<String>> {
    let Ok(blob) = auth::load_access_cache(email) else {
        return Ok(None);
    };
    let Ok(cached) = serde_json::from_str::<CachedToken>(&blob) else {
        return Ok(None);
    };
    if now_unix()? + EXPIRY_MARGIN_SECS < cached.expires_at {
        Ok(Some(cached.access_token))
    } else {
        Ok(None)
    }
}

fn now_unix() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| Error::OAuth(format!("系统时间异常: {e}")))
}

fn post_token(token_url: &str, form: &[(&str, &str)]) -> Result<TokenResponse> {
    // 传输层错误（→ Error::Http）瞬时可重试；端点 4xx（→ Error::OAuth，如 invalid_grant）永久不重试。
    crate::retry::with_retry(|| {
        let response = ureq::post(token_url).send_form(form).map_err(|e| match e {
            // 4xx 时 body 含 error_description，透传给用户便于排查。
            ureq::Error::Status(_, resp) => Error::OAuth(
                resp.into_string()
                    .unwrap_or_else(|_| "token 端点返回错误".to_string()),
            ),
            other => Error::Http(other.to_string()),
        })?;
        response
            .into_json::<TokenResponse>()
            .map_err(|e| Error::OAuth(format!("解析 token 响应失败: {e}")))
    })
}

/// 阻塞等待一次回调请求，解析出 ?code= 与 ?state=。
fn wait_for_redirect(listener: &TcpListener) -> Result<(String, String)> {
    let (mut stream, _) = listener.accept()?;
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // 形如: "GET /?code=XXX&state=YYY HTTP/1.1"
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| Error::OAuth("回调请求格式异常".to_string()))?;
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", v)) => code = Some(decode(v)),
            Some(("state", v)) => state = Some(decode(v)),
            Some(("error", v)) => error = Some(decode(v)),
            _ => {}
        }
    }

    let body = "<html><body>授权完成，可关闭此窗口返回终端。</body></html>";
    let http = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(http.as_bytes());

    if let Some(err) = error {
        return Err(Error::OAuth(format!("授权被拒绝: {err}")));
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state)),
        _ => Err(Error::OAuth("回调缺少 code/state".to_string())),
    }
}

/// 从 OS 熵源取 32 字节生成 PKCE verifier（base64url，无填充）。
fn generate_verifier() -> Result<String> {
    let mut bytes = [0u8; 32];
    let mut f = std::fs::File::open("/dev/urandom")?;
    f.read_exact(&mut bytes)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn decode(s: &str) -> String {
    urlencoding::decode(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}
