use crate::admin::{AdminState, JsonPayload, api_error, json_ok, require_json};
use crate::admin_auth::AdminAuth;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
pub(crate) async fn admin_guard(
    State(state): State<AdminState>,
    mut request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let secure_cookie = request_uses_https(request.headers());
    let Some(session) = state.inner.admin_auth.authenticate(request.headers()) else {
        return unauthorized_response(secure_cookie);
    };
    let bodyless_runtime_reset = request.method() == Method::POST
        && matches!(
            request.uri().path(),
            "/runtime/reset"
                | "/admin/api/runtime/reset"
                | "/runtime/recreate"
                | "/admin/api/runtime/recreate"
        );
    if is_write_method(request.method())
        && !bodyless_runtime_reset
        && !is_json_content_type(request.headers())
    {
        return api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
            "写请求必须使用 Content-Type: application/json",
        );
    }
    if is_write_method(request.method())
        && !state
            .inner
            .admin_auth
            .accepts_csrf(request.headers(), &session)
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "csrf_required",
            "登录会话校验失败，请刷新页面后重试",
        );
    }
    request.extensions_mut().insert(session);
    next.run(request).await
}

pub(crate) fn unauthorized_response(secure_cookie: bool) -> Response {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "admin_auth_required",
        "登录会话不存在或已过期",
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        header::HeaderValue::from_str(&AdminAuth::clear_cookie(secure_cookie))
            .expect("固定 Cookie 属性有效"),
    );
    response
}

pub(crate) async fn admin_security_headers(
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    let is_api = path == "/admin/api" || path.starts_with("/admin/api/");
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("content-security-policy"),
        header::HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-frame-options"),
        header::HeaderValue::from_static("DENY"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("referrer-policy"),
        header::HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("permissions-policy"),
        header::HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if is_api || path == "/admin" || path.starts_with("/admin/") {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
    }
    response
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginBody {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangeCredentialsBody {
    current_password: String,
    username: String,
    new_password: String,
}

pub(crate) async fn auth_session(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    let secure_cookie = request_uses_https(&headers);
    match state.inner.admin_auth.authenticate(&headers) {
        Some(session) => {
            auth_session_response(&session.username, &session.csrf_token, session.expires_at)
        }
        None => {
            let mut response = json_ok(&json!({"authenticated": false}));
            set_cookie_header(&mut response, &AdminAuth::clear_cookie(secure_cookie));
            response
        }
    }
}

pub(crate) async fn auth_login(
    State(state): State<AdminState>,
    headers: HeaderMap,
    payload: JsonPayload<LoginBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.username.chars().count() > 128 || body.password.chars().count() > 1024 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_credentials",
            "用户名或密码格式无效",
        );
    }
    let secure_cookie = request_uses_https(&headers);
    let Some(grant) = state
        .inner
        .admin_auth
        .login(&body.username, &body.password, secure_cookie)
    else {
        tokio::time::sleep(Duration::from_millis(250)).await;
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "用户名或密码不正确",
        );
    };
    let mut response = auth_session_response(
        &grant.session.username,
        &grant.session.csrf_token,
        grant.session.expires_at,
    );
    set_cookie_header(&mut response, &grant.set_cookie);
    response
}

pub(crate) async fn auth_logout(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    let secure_cookie = request_uses_https(&headers);
    state.inner.admin_auth.logout(&headers);
    let mut response = json_ok(&json!({"authenticated": false}));
    set_cookie_header(&mut response, &AdminAuth::clear_cookie(secure_cookie));
    response
}

pub(crate) async fn change_admin_credentials(
    State(state): State<AdminState>,
    headers: HeaderMap,
    payload: JsonPayload<ChangeCredentialsBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let secure_cookie = request_uses_https(&headers);
    match state.inner.admin_auth.change_credentials(
        &body.current_password,
        &body.username,
        &body.new_password,
        secure_cookie,
    ) {
        Ok(update) => {
            let mut response = auth_session_response(
                &update.grant.session.username,
                &update.grant.session.csrf_token,
                update.grant.session.expires_at,
            );
            if let Some(warning) = update.durability_warning {
                let mut value = json!({
                    "authenticated": true,
                    "username": update.grant.session.username,
                    "csrfToken": update.grant.session.csrf_token,
                    "expiresAt": update.grant.session.expires_at,
                    "warning": warning,
                });
                response = json_ok(&value.take());
            }
            set_cookie_header(&mut response, &update.grant.set_cookie);
            response
        }
        Err(message) if message == "当前密码不正确" => api_error(
            StatusCode::BAD_REQUEST,
            "invalid_current_password",
            &message,
        ),
        Err(message) if message.starts_with("用户名") || message.starts_with("新密码") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_credentials", &message)
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "credentials_write_failed",
            &message,
        ),
    }
}

pub(crate) fn auth_session_response(username: &str, csrf_token: &str, expires_at: u64) -> Response {
    json_ok(&json!({
        "authenticated": true,
        "username": username,
        "csrfToken": csrf_token,
        "expiresAt": expires_at,
    }))
}

pub(crate) fn set_cookie_header(response: &mut Response, value: &str) {
    if let Ok(value) = header::HeaderValue::from_str(value) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
}

pub(crate) fn request_uses_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
        || headers
            .get("forwarded")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .any(|part| part.trim().eq_ignore_ascii_case("proto=https"))
            })
}

pub(crate) fn is_write_method(method: &Method) -> bool {
    method == Method::POST
        || method == Method::PUT
        || method == Method::PATCH
        || method == Method::DELETE
}

pub(crate) fn is_json_content_type(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
}
