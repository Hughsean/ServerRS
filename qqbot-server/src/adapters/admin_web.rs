//! 本机管理员页面：登录后读取 NapCat 群目录并更新群观察白名单。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use qqbot::napcat::{NapCatDirectoryReadT, NapCatReadOnlyClient};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::watch;

use crate::config::AdminConfig;
use crate::group_whitelist::GroupWhitelist;
use crate::worker_lifecycle::WorkerHandle;

const SESSION_COOKIE: &str = "qqbot_admin_session";
const MAX_GROUPS: usize = 5_000;

pub(crate) struct AdminWebHandle {
    shutdown: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

impl AdminWebHandle {
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("admin_web", self.join)
    }
}

#[derive(Clone)]
struct AdminState {
    password: Arc<str>,
    session_token: Arc<str>,
    csrf_token: Arc<str>,
    session_ttl_secs: u64,
    whitelist: Arc<GroupWhitelist>,
    directory: Arc<dyn NapCatDirectoryReadT>,
    login_guard: Arc<Mutex<LoginGuard>>,
}

#[derive(Default)]
struct LoginGuard {
    failures: u32,
    blocked_until: Option<Instant>,
}

pub(crate) async fn spawn_admin_web(
    config: AdminConfig,
    whitelist: Arc<GroupWhitelist>,
    napcat_http_base_url: String,
) -> Result<AdminWebHandle, String> {
    let address = format!("{}:{}", config.bind, config.port)
        .parse::<SocketAddr>()
        .map_err(|_| "invalid admin listen address".to_string())?;
    let password = config.password().map_err(|error| error.to_string())?;
    let state = AdminState {
        password: password.into(),
        session_token: random_token()?.into(),
        csrf_token: random_token()?.into(),
        session_ttl_secs: config.session_ttl_secs,
        whitelist,
        directory: Arc::new(NapCatReadOnlyClient::new(napcat_http_base_url)),
        login_guard: Arc::new(Mutex::new(LoginGuard::default())),
    };
    let router = Router::new()
        .route("/", get(index))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/session", get(session))
        .route("/api/groups", get(groups))
        .route(
            "/api/whitelist/{group_id}",
            put(enable_group).delete(disable_group),
        )
        .fallback(not_found)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| "cannot bind admin listen address".to_string())?;
    let (shutdown, mut receiver) = watch::channel(false);
    let join = tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                loop {
                    if *receiver.borrow() || receiver.changed().await.is_err() {
                        return;
                    }
                }
            })
            .await;
        if result.is_err() {
            tracing::error!(
                error_code = "admin_web_serve_failed",
                "管理员页面服务异常退出"
            );
        }
    });
    Ok(AdminWebHandle { shutdown, join })
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "cannot create admin session token".to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

async fn index() -> Response {
    let mut response = Html(ADMIN_HTML).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    response
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
struct SessionResponse<'a> {
    authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    csrf_token: Option<&'a str>,
}

async fn login(State(state): State<AdminState>, Json(input): Json<LoginRequest>) -> Response {
    {
        let guard = state.login_guard.lock().expect("login guard poisoned");
        if guard
            .blocked_until
            .is_some_and(|until| until > Instant::now())
        {
            return api_error(StatusCode::TOO_MANY_REQUESTS, "登录尝试过于频繁");
        }
    }
    let valid = input.password.len() <= 256
        && bool::from(input.password.as_bytes().ct_eq(state.password.as_bytes()));
    if !valid {
        let mut guard = state.login_guard.lock().expect("login guard poisoned");
        guard.failures = guard.failures.saturating_add(1);
        if guard.failures >= 5 {
            guard.blocked_until = Some(Instant::now() + Duration::from_secs(30));
            guard.failures = 0;
        }
        return api_error(StatusCode::UNAUTHORIZED, "账号或密码错误");
    }
    *state.login_guard.lock().expect("login guard poisoned") = LoginGuard::default();
    let cookie = format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        state.session_token, state.session_ttl_secs
    );
    let mut response = Json(SessionResponse {
        authenticated: true,
        csrf_token: Some(&state.csrf_token),
    })
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(SET_COOKIE, value);
    }
    response
}

async fn logout(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    if let Err(error) = require_mutation_auth(&headers, &state) {
        return error.into_response();
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static(
            "qqbot_admin_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        ),
    );
    response
}

async fn session(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    let authenticated = is_authenticated(&headers, &state);
    Json(SessionResponse {
        authenticated,
        csrf_token: authenticated.then_some(state.csrf_token.as_ref()),
    })
    .into_response()
}

#[derive(Serialize)]
struct GroupItem {
    group_id: i64,
    group_name: String,
    observed: bool,
}

async fn groups(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    if !is_authenticated(&headers, &state) {
        return api_error(StatusCode::UNAUTHORIZED, "请先登录");
    }
    let mut groups = match state.directory.get_group_list().await {
        Ok(groups) => groups,
        Err(_) => return api_error(StatusCode::BAD_GATEWAY, "NapCat 群列表暂不可用"),
    };
    groups.sort_by(|left, right| left.group_name.cmp(&right.group_name));
    let response = groups
        .into_iter()
        .take(MAX_GROUPS)
        .map(|group| GroupItem {
            group_id: group.group_id,
            group_name: group.group_name,
            observed: state.whitelist.contains(group.group_id),
        })
        .collect::<Vec<_>>();
    Json(response).into_response()
}

async fn enable_group(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(group_id): Path<i64>,
) -> Response {
    update_group(state, headers, group_id, true)
}

async fn disable_group(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(group_id): Path<i64>,
) -> Response {
    update_group(state, headers, group_id, false)
}

fn update_group(state: AdminState, headers: HeaderMap, group_id: i64, enabled: bool) -> Response {
    if let Err(error) = require_mutation_auth(&headers, &state) {
        return error.into_response();
    }
    match state.whitelist.set(group_id, enabled) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "白名单保存失败"),
    }
}

enum MutationAuthError {
    Unauthorized,
    Forbidden,
}

impl MutationAuthError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized => api_error(StatusCode::UNAUTHORIZED, "请先登录"),
            Self::Forbidden => api_error(StatusCode::FORBIDDEN, "请求校验失败"),
        }
    }
}

fn require_mutation_auth(headers: &HeaderMap, state: &AdminState) -> Result<(), MutationAuthError> {
    if !is_authenticated(headers, state) {
        return Err(MutationAuthError::Unauthorized);
    }
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !bool::from(csrf.as_bytes().ct_eq(state.csrf_token.as_bytes())) {
        return Err(MutationAuthError::Forbidden);
    }
    Ok(())
}

fn is_authenticated(headers: &HeaderMap, state: &AdminState) -> bool {
    let Some(cookie) = headers.get(COOKIE).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    cookie.split(';').any(|entry| {
        let Some((name, value)) = entry.trim().split_once('=') else {
            return false;
        };
        name == SESSION_COOKIE && bool::from(value.as_bytes().ct_eq(state.session_token.as_bytes()))
    })
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    error: &'a str,
}

fn api_error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(ErrorResponse { error: message })).into_response()
}

const ADMIN_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>QQBot 管理</title>
  <style>
    :root{color-scheme:light;--bg:#f4f6f8;--panel:#fff;--line:#d8dee6;--text:#17202a;--muted:#66717d;--accent:#087f5b;--danger:#b42318}
    *{box-sizing:border-box;letter-spacing:0}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.5 system-ui,"Microsoft YaHei",sans-serif}
    header{height:56px;background:#18212b;color:#fff;display:flex;align-items:center;padding:0 24px;font-weight:650;font-size:17px}
    main{width:min(960px,calc(100% - 28px));margin:24px auto}section{background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:20px}
    h1{font-size:19px;margin:0 0 18px}label{display:block;font-weight:600;margin-bottom:7px}input{width:100%;height:40px;border:1px solid #aeb8c4;border-radius:4px;padding:0 11px;font:inherit}
    button{height:36px;border:1px solid #9aa6b2;border-radius:4px;background:#fff;padding:0 14px;font:inherit;cursor:pointer}button.primary{background:var(--accent);border-color:var(--accent);color:#fff}button:disabled{opacity:.55;cursor:not-allowed}
    .login{max-width:420px;margin:72px auto}.row{display:flex;gap:10px;align-items:center}.toolbar{display:grid;grid-template-columns:1fr auto auto;gap:10px;margin-bottom:14px}
    .summary{color:var(--muted);margin:0 0 14px}.list{border:1px solid var(--line);border-radius:5px;overflow:hidden}.item{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:16px;align-items:center;padding:12px 14px;border-bottom:1px solid var(--line)}.item:last-child{border-bottom:0}
    .name{font-weight:600;overflow-wrap:anywhere}.id{color:var(--muted);font-variant-numeric:tabular-nums}.status{min-width:92px}.enabled{color:var(--accent)}.disabled{color:var(--muted)}
    .error{color:var(--danger);min-height:22px;margin-top:10px}.hidden{display:none!important}.empty{padding:32px;text-align:center;color:var(--muted)}
    @media(max-width:640px){header{padding:0 14px}main{margin:14px auto}.toolbar{grid-template-columns:1fr 1fr}.toolbar input{grid-column:1/-1}.item{grid-template-columns:1fr}.row{align-items:stretch}.item .row{justify-content:space-between}}
  </style>
</head>
<body>
<header>QQBot 管理</header>
<main>
  <section id="login" class="login">
    <h1>管理员登录</h1>
    <form id="login-form">
      <label for="password">密码</label>
      <input id="password" type="password" required minlength="12" autocomplete="current-password">
      <div style="margin-top:14px"><button class="primary" type="submit">登录</button></div>
      <div id="login-error" class="error" role="alert"></div>
    </form>
  </section>
  <section id="dashboard" class="hidden">
    <div class="row" style="justify-content:space-between;margin-bottom:18px"><h1 style="margin:0">群聊观察白名单</h1><button id="logout">退出</button></div>
    <div class="toolbar"><input id="search" type="search" placeholder="搜索群名称或群号"><button id="refresh">刷新</button><button id="observe-all">全部观察</button></div>
    <p id="summary" class="summary"></p><div id="groups" class="list"></div><div id="dashboard-error" class="error" role="alert"></div>
  </section>
</main>
<script>
let csrf='',groups=[];
const el=id=>document.getElementById(id);
async function request(url,options={}){const headers={...(options.headers||{})};if(options.method&&options.method!=='GET')headers['X-CSRF-Token']=csrf;const response=await fetch(url,{...options,headers});if(response.status===204)return null;const data=await response.json().catch(()=>({error:'请求失败'}));if(!response.ok)throw new Error(data.error||'请求失败');return data}
function showDashboard(){el('login').classList.add('hidden');el('dashboard').classList.remove('hidden');loadGroups()}
async function boot(){const session=await request('/api/session');if(session.authenticated){csrf=session.csrf_token;showDashboard()}}
el('login-form').addEventListener('submit',async event=>{event.preventDefault();el('login-error').textContent='';try{const data=await request('/api/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({password:el('password').value})});csrf=data.csrf_token;el('password').value='';showDashboard()}catch(error){el('login-error').textContent=error.message}});
async function loadGroups(){el('dashboard-error').textContent='';el('groups').innerHTML='<div class="empty">加载中</div>';try{groups=await request('/api/groups');render()}catch(error){el('groups').innerHTML='';el('dashboard-error').textContent=error.message}}
function render(){const query=el('search').value.trim().toLowerCase();const filtered=groups.filter(group=>group.group_name.toLowerCase().includes(query)||String(group.group_id).includes(query));el('summary').textContent=`已观察 ${groups.filter(group=>group.observed).length} / ${groups.length} 个群`;const list=el('groups');list.innerHTML='';if(!filtered.length){list.innerHTML='<div class="empty">没有匹配的群聊</div>';return}for(const group of filtered){const row=document.createElement('div');row.className='item';const info=document.createElement('div');const name=document.createElement('div');name.className='name';name.textContent=group.group_name||'未命名群聊';const id=document.createElement('div');id.className='id';id.textContent=String(group.group_id);info.append(name,id);const controls=document.createElement('div');controls.className='row';const status=document.createElement('span');status.className='status '+(group.observed?'enabled':'disabled');status.textContent=group.observed?'正在观察':'未观察';const button=document.createElement('button');button.textContent=group.observed?'移除':'加入';button.addEventListener('click',()=>toggle(group,button));controls.append(status,button);row.append(info,controls);list.append(row)}}
async function toggle(group,button){button.disabled=true;try{await request(`/api/whitelist/${group.group_id}`,{method:group.observed?'DELETE':'PUT'});group.observed=!group.observed;render()}catch(error){el('dashboard-error').textContent=error.message;button.disabled=false}}
el('search').addEventListener('input',render);el('refresh').addEventListener('click',loadGroups);el('observe-all').addEventListener('click',async()=>{for(const group of groups.filter(group=>!group.observed)){await request(`/api/whitelist/${group.group_id}`,{method:'PUT'});group.observed=true}render()});
el('logout').addEventListener('click',async()=>{await request('/api/logout',{method:'POST'});location.reload()});boot().catch(()=>{});
</script>
</body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use qqbot::napcat::{FriendInfoData, GroupInfoData, NapCatError, RecentContactData};

    struct FakeDirectory;

    #[async_trait::async_trait]
    impl NapCatDirectoryReadT for FakeDirectory {
        async fn get_friend_list(&self) -> Result<Vec<FriendInfoData>, NapCatError> {
            Ok(Vec::new())
        }

        async fn get_group_list(&self) -> Result<Vec<GroupInfoData>, NapCatError> {
            Ok(vec![GroupInfoData {
                group_id: 42,
                group_name: "工作群".into(),
                member_count: None,
                max_member_count: None,
            }])
        }

        async fn get_recent_contact(&self) -> Result<Vec<RecentContactData>, NapCatError> {
            Ok(Vec::new())
        }
    }

    fn state(path: std::path::PathBuf) -> AdminState {
        AdminState {
            password: "correct-password".into(),
            session_token: "session".into(),
            csrf_token: "csrf".into(),
            session_ttl_secs: 300,
            whitelist: Arc::new(GroupWhitelist::new(
                Some(path),
                std::collections::HashSet::new(),
            )),
            directory: Arc::new(FakeDirectory),
            login_guard: Arc::new(Mutex::new(LoginGuard::default())),
        }
    }

    #[test]
    fn generated_tokens_are_bounded_and_distinct() {
        let first = random_token().unwrap();
        let second = random_token().unwrap();
        assert_eq!(first.len(), 43);
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn login_and_whitelist_mutation_require_session_and_csrf() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path().join("groups.json"));
        let rejected = login(
            State(state.clone()),
            Json(LoginRequest {
                password: "wrong-password".into(),
            }),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        let accepted = login(
            State(state.clone()),
            Json(LoginRequest {
                password: "correct-password".into(),
            }),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::OK);
        assert!(accepted.headers().contains_key(SET_COOKIE));

        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("qqbot_admin_session=session"),
        );
        assert_eq!(
            update_group(state.clone(), headers.clone(), 42, true).status(),
            StatusCode::FORBIDDEN
        );
        headers.insert("x-csrf-token", HeaderValue::from_static("csrf"));
        assert_eq!(
            update_group(state.clone(), headers, 42, true).status(),
            StatusCode::NO_CONTENT
        );
        assert!(state.whitelist.contains(42));
    }
}
