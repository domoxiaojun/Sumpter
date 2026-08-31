//! Admin 内置登录凭据、会话 Cookie 与 CSRF 状态。

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::http::{HeaderMap, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_ADMIN_USERNAME: &str = "kkl";

const CREDENTIAL_FILE_VERSION: u32 = 1;
const MAX_CREDENTIAL_FILE_BYTES: u64 = 16 * 1024;
const MAX_SESSIONS: usize = 64;
const SESSION_COOKIE_NAME: &str = "kekulv_admin_session";
const SESSION_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone)]
pub struct AdminAuth {
    inner: Arc<AdminAuthInner>,
}

struct AdminAuthInner {
    credentials: RwLock<AdminCredentials>,
    credential_updates: Mutex<()>,
    sessions: Mutex<HashMap<String, StoredSession>>,
    credential_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdminCredentials {
    username: String,
    password: StoredPassword,
    revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredPassword {
    LegacySha256([u8; 32]),
    Argon2(String),
}

#[derive(Debug, Clone)]
struct StoredSession {
    username: String,
    csrf_token: String,
    created_at: u64,
    expires_at: u64,
    credential_revision: u64,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub username: String,
    pub csrf_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct SessionGrant {
    pub session: AuthenticatedSession,
    pub set_cookie: String,
}

#[derive(Debug, Clone)]
pub struct CredentialUpdate {
    pub grant: SessionGrant,
    pub durability_warning: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialFile {
    version: u32,
    username: String,
    password_hash: String,
}

impl std::fmt::Debug for AdminAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminAuth")
            .field("mode", &self.mode())
            .field("username", &self.username())
            .field("persistent", &self.inner.credential_path.is_some())
            .finish_non_exhaustive()
    }
}

impl PartialEq for AdminAuth {
    fn eq(&self, other: &Self) -> bool {
        self.credentials_snapshot() == other.credentials_snapshot()
    }
}

impl Eq for AdminAuth {}

impl AdminAuth {
    /// 测试与嵌入调用的内存凭据；生产 daemon 使用 `from_password_file`。
    pub fn password(password: &[u8]) -> Result<Self, String> {
        if password.is_empty() {
            return Err("Admin 密码不能为空".into());
        }
        Ok(Self::new(
            AdminCredentials {
                username: DEFAULT_ADMIN_USERNAME.into(),
                password: StoredPassword::LegacySha256(Sha256::digest(password).into()),
                revision: 1,
            },
            None,
        ))
    }

    /// 兼容旧的单行明文 `admin-password`，也接受修改凭据后写入的 v1 JSON/Argon2 文件。
    pub fn from_password_file(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("读取 Admin 凭据文件 {} 失败: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("Admin 凭据文件不是普通文件: {}", path.display()));
        }
        if metadata.len() > MAX_CREDENTIAL_FILE_BYTES {
            return Err(format!(
                "Admin 凭据文件超过 {} 字节: {}",
                MAX_CREDENTIAL_FILE_BYTES,
                path.display()
            ));
        }
        let mut bytes = std::fs::read(path)
            .map_err(|error| format!("读取 Admin 凭据文件 {} 失败: {error}", path.display()))?;
        if bytes.len() as u64 > MAX_CREDENTIAL_FILE_BYTES {
            return Err(format!(
                "Admin 凭据文件超过 {} 字节: {}",
                MAX_CREDENTIAL_FILE_BYTES,
                path.display()
            ));
        }
        remove_one_trailing_newline(&mut bytes);
        if bytes.is_empty() {
            return Err(format!("Admin 凭据文件为空: {}", path.display()));
        }

        let credentials = if bytes.first() == Some(&b'{') {
            match serde_json::from_slice::<CredentialFile>(&bytes) {
                Ok(file) => {
                    if file.version != CREDENTIAL_FILE_VERSION {
                        return Err(format!(
                            "Admin 凭据文件版本不支持: {}（需要 {}）",
                            file.version, CREDENTIAL_FILE_VERSION
                        ));
                    }
                    validate_username(&file.username)?;
                    PasswordHash::new(&file.password_hash)
                        .map_err(|error| format!("Admin 密码哈希无效: {error}"))?;
                    AdminCredentials {
                        username: file.username,
                        password: StoredPassword::Argon2(file.password_hash),
                        revision: 1,
                    }
                }
                // 旧版密码本身可以以 `{` 开头；只有能解析成凭据结构时才按 JSON 处理。
                Err(_) => legacy_credentials(&bytes, path)?,
            }
        } else {
            legacy_credentials(&bytes, path)?
        };
        Ok(Self::new(credentials, Some(path.to_path_buf())))
    }

    fn new(credentials: AdminCredentials, credential_path: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(AdminAuthInner {
                credentials: RwLock::new(credentials),
                credential_updates: Mutex::new(()),
                sessions: Mutex::new(HashMap::new()),
                credential_path,
            }),
        }
    }

    pub const fn mode(&self) -> &'static str {
        "session-cookie"
    }

    pub fn username(&self) -> String {
        self.credentials_snapshot().username
    }

    pub fn login(
        &self,
        username: &str,
        password: &str,
        secure_cookie: bool,
    ) -> Option<SessionGrant> {
        let credentials = self.credentials_snapshot();
        let username_ok = constant_time_eq(username.as_bytes(), credentials.username.as_bytes());
        let password_ok = verify_password(&credentials.password, password.as_bytes());
        if !(username_ok & password_ok) {
            return None;
        }
        Some(self.create_session(&credentials, secure_cookie))
    }

    pub fn authenticate(&self, headers: &HeaderMap) -> Option<AuthenticatedSession> {
        let raw_token = cookie_value(headers, SESSION_COOKIE_NAME)?;
        let token_digest = token_digest(&raw_token);
        let now = unix_seconds();
        let credentials = self.credentials_snapshot();
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        sessions.retain(|_, session| {
            session.expires_at > now && session.credential_revision == credentials.revision
        });
        let session = sessions.get(&token_digest)?;
        if session.username != credentials.username {
            return None;
        }
        Some(AuthenticatedSession {
            username: session.username.clone(),
            csrf_token: session.csrf_token.clone(),
            expires_at: session.expires_at,
        })
    }

    pub fn accepts_csrf(&self, headers: &HeaderMap, session: &AuthenticatedSession) -> bool {
        headers
            .get("x-kekulv-csrf")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| constant_time_eq(value.as_bytes(), session.csrf_token.as_bytes()))
    }

    pub fn logout(&self, headers: &HeaderMap) {
        let Some(raw_token) = cookie_value(headers, SESSION_COOKIE_NAME) else {
            return;
        };
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(&token_digest(&raw_token));
    }

    pub fn change_credentials(
        &self,
        current_password: &str,
        new_username: &str,
        new_password: &str,
        secure_cookie: bool,
    ) -> Result<CredentialUpdate, String> {
        validate_username(new_username)?;
        validate_new_password(new_password)?;

        let _update_guard = self
            .inner
            .credential_updates
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        let current = self.credentials_snapshot();
        if !verify_password(&current.password, current_password.as_bytes()) {
            return Err("当前密码不正确".into());
        }
        let password_hash = hash_password(new_password)?;
        let next = AdminCredentials {
            username: new_username.to_string(),
            password: StoredPassword::Argon2(password_hash.clone()),
            revision: current.revision.saturating_add(1),
        };
        let durability_warning = match &self.inner.credential_path {
            Some(path) => persist_credentials(path, new_username, &password_hash)?,
            None => None,
        };
        *self
            .inner
            .credentials
            .write()
            .unwrap_or_else(|lock| lock.into_inner()) = next.clone();
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .clear();
        Ok(CredentialUpdate {
            grant: self.create_session(&next, secure_cookie),
            durability_warning,
        })
    }

    pub fn clear_cookie(secure_cookie: bool) -> String {
        format!(
            "{SESSION_COOKIE_NAME}=; Path=/admin; Max-Age=0; HttpOnly; SameSite=Strict{}",
            if secure_cookie { "; Secure" } else { "" }
        )
    }

    fn create_session(&self, credentials: &AdminCredentials, secure_cookie: bool) -> SessionGrant {
        let raw_token = random_token();
        let csrf_token = random_token();
        let now = unix_seconds();
        let expires_at = now.saturating_add(SESSION_TTL_SECONDS);
        let stored = StoredSession {
            username: credentials.username.clone(),
            csrf_token: csrf_token.clone(),
            created_at: now,
            expires_at,
            credential_revision: credentials.revision,
        };
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        sessions.retain(|_, session| {
            session.expires_at > now && session.credential_revision == credentials.revision
        });
        if sessions.len() >= MAX_SESSIONS
            && let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, session)| session.created_at)
                .map(|(key, _)| key.clone())
        {
            sessions.remove(&oldest);
        }
        sessions.insert(token_digest(&raw_token), stored);
        SessionGrant {
            session: AuthenticatedSession {
                username: credentials.username.clone(),
                csrf_token,
                expires_at,
            },
            set_cookie: format!(
                "{SESSION_COOKIE_NAME}={raw_token}; Path=/admin; Max-Age={SESSION_TTL_SECONDS}; HttpOnly; SameSite=Strict{}",
                if secure_cookie { "; Secure" } else { "" }
            ),
        }
    }

    fn credentials_snapshot(&self) -> AdminCredentials {
        self.inner
            .credentials
            .read()
            .unwrap_or_else(|lock| lock.into_inner())
            .clone()
    }
}

fn legacy_credentials(bytes: &[u8], path: &Path) -> Result<AdminCredentials, String> {
    if bytes
        .iter()
        .any(|byte| matches!(*byte, b'\r' | b'\n' | b'\0'))
    {
        return Err(format!(
            "旧版 Admin 密码文件只能包含单行密码且不能包含 NUL: {}",
            path.display()
        ));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| format!("旧版 Admin 密码文件必须是 UTF-8: {}", path.display()))?;
    Ok(AdminCredentials {
        username: DEFAULT_ADMIN_USERNAME.into(),
        password: StoredPassword::LegacySha256(Sha256::digest(bytes).into()),
        revision: 1,
    })
}

fn remove_one_trailing_newline(bytes: &mut Vec<u8>) {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
}

fn validate_username(username: &str) -> Result<(), String> {
    let length = username.chars().count();
    if username.trim() != username || length == 0 || length > 64 {
        return Err("用户名必须为 1-64 个字符，且首尾不能有空格".into());
    }
    if username.chars().any(char::is_control) {
        return Err("用户名不能包含控制字符".into());
    }
    Ok(())
}

fn validate_new_password(password: &str) -> Result<(), String> {
    let length = password.chars().count();
    if !(12..=512).contains(&length) {
        return Err("新密码必须为 12-512 个字符".into());
    }
    if password.chars().any(char::is_control) {
        return Err("新密码不能包含换行或其他控制字符".into());
    }
    Ok(())
}

fn verify_password(password: &StoredPassword, candidate: &[u8]) -> bool {
    match password {
        StoredPassword::LegacySha256(digest) => {
            let candidate: [u8; 32] = Sha256::digest(candidate).into();
            constant_time_eq(digest, &candidate)
        }
        StoredPassword::Argon2(encoded) => PasswordHash::new(encoded)
            .ok()
            .is_some_and(|hash| Argon2::default().verify_password(candidate, &hash).is_ok()),
    }
}

fn hash_password(password: &str) -> Result<String, String> {
    let mut salt_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|error| error.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| format!("生成 Admin 密码哈希失败: {error}"))
}

fn persist_credentials(
    path: &Path,
    username: &str,
    password_hash: &str,
) -> Result<Option<String>, String> {
    let file = CredentialFile {
        version: CREDENTIAL_FILE_VERSION,
        username: username.to_string(),
        password_hash: password_hash.to_string(),
    };
    let mut data = serde_json::to_vec_pretty(&file)
        .map_err(|error| format!("序列化 Admin 凭据失败: {error}"))?;
    data.push(b'\n');

    let parent = path
        .parent()
        .ok_or_else(|| "Admin 凭据路径缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建 Admin 凭据目录失败: {error}"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("admin-credentials");
    let temp_path = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let prepare_result = (|| -> std::io::Result<std::fs::File> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(&temp_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600))?;
        }
        output.write_all(&data)?;
        output.sync_all()?;
        drop(output);
        std::fs::File::open(parent)
    })();
    let directory = match prepare_result {
        Ok(directory) => directory,
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(format!("写入 Admin 凭据失败: {error}"));
        }
    };
    if let Err(error) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("提交 Admin 凭据失败: {error}"));
    }
    Ok(directory.sync_all().err().map(|error| {
        format!(
            "{} 已更新，但父目录同步失败，断电耐久性不确定: {error}",
            path.display()
        )
    }))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_string()))
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn token_digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let compared_len = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..compared_len {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kekulv-admin-auth-{label}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ))
    }

    #[test]
    fn legacy_password_and_hashed_credentials_round_trip() {
        let path = temp_path("round-trip");
        std::fs::write(&path, b"legacy-password\r\n").unwrap();
        let auth = AdminAuth::from_password_file(&path).unwrap();
        assert!(
            auth.login(DEFAULT_ADMIN_USERNAME, "legacy-password", false)
                .is_some()
        );

        let update = auth
            .change_credentials("legacy-password", "new-admin", "a-new-password-1234", false)
            .unwrap();
        assert!(update.durability_warning.is_none());
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("a-new-password-1234"));
        assert!(persisted.contains("$argon2"));

        let reloaded = AdminAuth::from_password_file(&path).unwrap();
        assert!(
            reloaded
                .login("new-admin", "a-new-password-1234", false)
                .is_some()
        );
        assert!(
            reloaded
                .login(DEFAULT_ADMIN_USERNAME, "legacy-password", false)
                .is_none()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn credential_validation_rejects_weak_or_ambiguous_values() {
        assert!(validate_username("").is_err());
        assert!(validate_username(" admin").is_err());
        assert!(validate_new_password("short").is_err());
        assert!(validate_new_password("valid-password\n123").is_err());
    }

    #[test]
    fn legacy_password_starting_with_brace_is_not_forced_into_json() {
        let path = temp_path("brace-password");
        std::fs::write(&path, b"{legacy-password\n").unwrap();
        let auth = AdminAuth::from_password_file(&path).unwrap();
        assert!(
            auth.login(DEFAULT_ADMIN_USERNAME, "{legacy-password", false)
                .is_some()
        );
        let _ = std::fs::remove_file(path);
    }
}
