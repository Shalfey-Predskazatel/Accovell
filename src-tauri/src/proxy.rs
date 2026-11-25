use crate::{
    codex,
    core::{now_timestamp, AppCore},
    model::{AccountProfile, UsageSnapshot, UsageWindow},
};
use axum::{
    body::{to_bytes, Body},
    extract::{
        ws::{Message as ClientMessage, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    routing::{any, get},
    Router,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as UpstreamMessage},
    MaybeTlsStream, WebSocketStream,
};

const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const BODY_LIMIT: usize = 128 * 1024 * 1024;
type UpstreamSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
struct Handshake {
    headers: HeaderMap,
    query: Option<String>,
}

pub async fn start(core: Arc<AppCore>) -> Result<(), String> {
    let address = SocketAddr::from(([127, 0, 0, 1], core.proxy_port));
    let path_prefix = format!("/{}/backend-api/codex", core.proxy_access_key);
    let responses_path = format!("{path_prefix}/responses");
    let wildcard_path = format!("{path_prefix}/{{*path}}");
    let mut shutdown = core.subscribe_shutdown();
    let listener = TcpListener::bind(address).await
        .map_err(|_| format!("Не удалось запустить локальный прокси на 127.0.0.1:{}. Закройте другое приложение на этом порту и перезапустите Codex Accounts.", core.proxy_port))?;
    let app = Router::new()
        .route(&responses_path, get(websocket_handler).post(http_handler))
        .route(&path_prefix, any(http_handler))
        .route(&wildcard_path, any(http_handler))
        .with_state(core.clone());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await
        {
            *core.proxy_error.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("Локальный маршрут Codex остановился: {error}"));
            core.emit_changed();
        }
    });
    Ok(())
}

async fn http_handler(State(core): State<Arc<AppCore>>, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, BODY_LIMIT).await {
        Ok(body) => body,
        Err(_) => {
            return message_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Запрос Codex превышает 128 МБ. Сократите контекст или создайте новый чат.",
            )
        }
    };
    let path_prefix = format!("/{}/backend-api/codex", core.proxy_access_key);
    let path = parts.uri.path().strip_prefix(&path_prefix).unwrap_or("");
    let query = parts
        .uri
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    let url = format!("{CHATGPT_CODEX_BASE_URL}{path}{query}");
    let original_headers = parts.headers;
    let method = parts.method;
    // Preserve Codex's request exactly. Requests are never replayed on a
    // different account after an upstream quota response.
    let request_body = body.clone();
    let anchor = body_previous_response_id(&request_body).or_else(|| response_id_from_path(path));
    let account = if let Some(response_id) = anchor.as_deref() {
        let Some(owner) = core.response_owner(response_id) else {
            return message_response(
                StatusCode::CONFLICT,
                "Этот запрос продолжает ответ, владельца которого прокси не знает. Чтобы не отправить чат с другого аккаунта, откройте новый чат или повторите запрос после переподключения Codex.",
            );
        };
        let vault = core.vault.read().await;
        let Some(account) = vault
            .accounts
            .iter()
            .find(|account| account.id == owner)
            .cloned()
        else {
            return message_response(
                StatusCode::CONFLICT,
                "Аккаунт, на котором начат этот чат, удалён. Восстановите его или начните новый чат.",
            );
        };
        if account.status.kind.is_permanently_unavailable() {
            let detail = account
                .status
                .detail
                .as_deref()
                .unwrap_or("Аккаунт требует повторного входа.");
            return message_response(StatusCode::UNAUTHORIZED, detail);
        }
        account
    } else {
        match core.select_active().await {
            Ok(account) => account,
            Err(error) => return message_response(StatusCode::SERVICE_UNAVAILABLE, &error),
        }
    };

    let (access_token, account_id) = match core.access_token(&account).await {
        Ok(tokens) => tokens,
        Err(error) => {
            let kind = codex::classify_account_error(&error);
            let _ = core
                .mark_account_status(&account.id, kind, error.clone())
                .await;
            return message_response(StatusCode::UNAUTHORIZED, &error);
        }
    };
    let mut builder = core
        .http
        .request(method.clone(), &url)
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header("chatgpt-account-id", account_id);
    for (name, value) in &original_headers {
        if should_forward_header(name) {
            builder = builder.header(name, value);
        }
    }
    let upstream = match builder.body(request_body).send().await {
        Ok(response) => response,
        Err(_) => return message_response(
            StatusCode::BAD_GATEWAY,
            "Не удалось связаться с chatgpt.com. Проверьте интернет, VPN или прокси-соединение.",
        ),
    };
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    update_usage_from_headers(&core, &account.id, &response_headers).await;
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let response_body = match upstream.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return message_response(
                    StatusCode::BAD_GATEWAY,
                    "Сервис оборвал ответ проверки аккаунта.",
                )
            }
        };
        let body_text = String::from_utf8_lossy(&response_body);
        let detail = friendly_upstream_error(status, &body_text);
        let kind =
            codex::classify_account_error(&format!("HTTP {} {}", status.as_u16(), body_text));
        let _ = core.mark_account_status(&account.id, kind, detail).await;
        return bytes_response(status, &response_headers, response_body);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let response_body = match upstream.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return message_response(
                    StatusCode::BAD_GATEWAY,
                    "Сервис оборвал ответ с лимитом. Повторите запрос вручную.",
                )
            }
        };
        let body_text = String::from_utf8_lossy(&response_body);
        if is_usage_limit(status.as_u16(), &body_text) {
            let reset_at = refusal_reset_at(&body_text);
            let _ = core.mark_usage_limit(&account.id, reset_at).await;
        }
        return bytes_response(status, &response_headers, response_body);
    }
    let mut response = Response::builder().status(status);
    for (name, value) in response_headers.iter() {
        if should_forward_response_header(name) {
            response = response.header(name, value);
        }
    }
    let account_id = account.id.clone();
    let mut event_buffer = Vec::new();
    let stream = upstream.bytes_stream().map(move |chunk| {
        if let Ok(bytes) = &chunk {
            observe_sse_response_ids(&core, &account_id, &mut event_buffer, bytes);
        }
        chunk
    });
    response
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            message_response(
                StatusCode::BAD_GATEWAY,
                "Не удалось передать потоковый ответ Codex.",
            )
        })
}

async fn websocket_handler(
    State(core): State<Arc<AppCore>>,
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response<Body> {
    let handshake = Handshake {
        headers,
        query: uri.query().map(str::to_string),
    };
    let account = match core.select_active().await {
        Ok(account) => account,
        Err(error) => return message_response(StatusCode::SERVICE_UNAVAILABLE, &error),
    };
    let upstream = match connect_upstream(&core, &account, &handshake).await {
        Ok(socket) => socket,
        Err(failure) => {
            if failure.usage_limit {
                let _ = core.mark_usage_limit(&account.id, failure.reset_at).await;
            }
            return message_response(failure.status, &failure.message);
        }
    };
    let response =
        websocket.on_upgrade(move |client| relay(core, client, upstream, account, handshake));
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, body)
}

struct ConnectFailure {
    status: StatusCode,
    message: String,
    usage_limit: bool,
    reset_at: Option<i64>,
}

async fn connect_upstream(
    core: &AppCore,
    account: &AccountProfile,
    handshake: &Handshake,
) -> Result<UpstreamSocket, ConnectFailure> {
    let (access, account_id) = match core.access_token(account).await {
        Ok(tokens) => tokens,
        Err(message) => {
            let kind = codex::classify_account_error(&message);
            let _ = core
                .mark_account_status(&account.id, kind, message.clone())
                .await;
            return Err(ConnectFailure {
                status: StatusCode::UNAUTHORIZED,
                message,
                usage_limit: false,
                reset_at: None,
            });
        }
    };
    let query = handshake
        .query
        .as_ref()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    let mut request = format!("{CHATGPT_CODEX_BASE_URL}/responses{query}")
        .into_client_request()
        .map_err(|_| ConnectFailure {
            status: StatusCode::BAD_GATEWAY,
            message: "Не удалось подготовить соединение Codex.".to_string(),
            usage_limit: false,
            reset_at: None,
        })?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access}")).map_err(|_| ConnectFailure {
            status: StatusCode::UNAUTHORIZED,
            message: "Не удалось подготовить авторизацию аккаунта.".to_string(),
            usage_limit: false,
            reset_at: None,
        })?,
    );
    request.headers_mut().insert(
        HeaderName::from_static("chatgpt-account-id"),
        HeaderValue::from_str(&account_id).map_err(|_| ConnectFailure {
            status: StatusCode::UNAUTHORIZED,
            message: "Не удалось прочитать id аккаунта.".to_string(),
            usage_limit: false,
            reset_at: None,
        })?,
    );
    for (name, value) in &handshake.headers {
        if should_forward_header(name) {
            request.headers_mut().insert(name.clone(), value.clone());
        }
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        connect_async(request),
    )
    .await
    {
        Err(_) => Err(ConnectFailure {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "Codex не ответил на WebSocket-подключение за 20 секунд. Проверьте интернет, VPN или прокси.".to_string(),
            usage_limit: false,
            reset_at: None,
        }),
        Ok(Ok((socket, response))) => {
            update_usage_from_headers(core, &account.id, response.headers()).await;
            Ok(socket)
        }
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
            update_usage_from_headers(core, &account.id, response.headers()).await;
            let status = response.status();
            let body = response
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or(std::borrow::Cow::Borrowed(""));
            let limit = is_usage_limit(status.as_u16(), &body);
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                let detail = friendly_upstream_error(status, &body);
                let kind =
                    codex::classify_account_error(&format!("HTTP {} {}", status.as_u16(), body));
                let _ = core.mark_account_status(&account.id, kind, detail).await;
            }
            Err(ConnectFailure {
                status,
                message: friendly_upstream_error(status, &body),
                usage_limit: limit,
                reset_at: refusal_reset_at(&body),
            })
        }
        Ok(Err(_)) => Err(ConnectFailure {
            status: StatusCode::BAD_GATEWAY,
            message: "Codex не смог открыть WebSocket к chatgpt.com. Проверьте интернет и VPN."
                .to_string(),
            usage_limit: false,
            reset_at: None,
        }),
    }
}

async fn relay(
    core: Arc<AppCore>,
    mut client: WebSocket,
    mut upstream: UpstreamSocket,
    mut account: AccountProfile,
    handshake: Handshake,
) {
    loop {
        tokio::select! {
            incoming = client.next() => {
                let Some(Ok(message)) = incoming else { break; };
                if matches!(message, ClientMessage::Ping(_) | ClientMessage::Pong(_)) { continue; }
                if let ClientMessage::Text(text) = &message {
                    if event_type(text.as_str()).as_deref() == Some("response.create") {
                        let target = if let Some(response_id) = previous_response_id(text.as_str()) {
                            let Some(owner) = core.response_owner(&response_id) else {
                                let detail = "Не удалось определить аккаунт, на котором начат этот чат. Запрос остановлен, чтобы не отправить продолжение с другого профиля. Переподключите Codex или начните новый чат.";
                                let _ = client.send(ClientMessage::Text(error_event(StatusCode::CONFLICT, detail).into())).await;
                                continue;
                            };
                            let owner_account = {
                                let vault = core.vault.read().await;
                                vault.accounts.iter().find(|entry| entry.id == owner).cloned()
                            };
                            let Some(owner_account) = owner_account else {
                                let detail = "Аккаунт, на котором начат этот чат, больше не сохранён. Восстановите профиль или начните новый чат.";
                                let _ = client.send(ClientMessage::Text(error_event(StatusCode::CONFLICT, detail).into())).await;
                                continue;
                            };
                            if owner_account.status.kind.is_permanently_unavailable() {
                                let detail = owner_account
                                    .status
                                    .detail
                                    .as_deref()
                                    .unwrap_or("Аккаунт требует повторного входа.");
                                let _ = client.send(ClientMessage::Text(error_event(StatusCode::UNAUTHORIZED, detail).into())).await;
                                continue;
                            }
                            owner_account
                        } else {
                            match core.select_active().await {
                                Ok(value) => value,
                                Err(error) => {
                                    let _ = client.send(ClientMessage::Text(error_event(StatusCode::SERVICE_UNAVAILABLE, &error).into())).await;
                                    continue;
                                }
                            }
                        };

                        // Existing response chains stay with their owner. The
                        // selected profile only affects a new conversation.
                        if target.id != account.id {
                            match connect_upstream(&core, &target, &handshake).await {
                                Ok(next_socket) => {
                                    let _ = upstream.close(None).await;
                                    upstream = next_socket;
                                    account = target;
                                }
                                Err(failure) => {
                                    if failure.usage_limit {
                                        let _ = core.mark_usage_limit(&target.id, failure.reset_at).await;
                                    }
                                    let _ = client.send(ClientMessage::Text(error_event(failure.status, &failure.message).into())).await;
                                    continue;
                                }
                            }
                        }
                    }
                }
                if send_to_upstream(&mut upstream, message).await.is_err() { break; }
            }
            incoming = upstream.next() => {
                let Some(Ok(message)) = incoming else { break; };
                if matches!(message, UpstreamMessage::Ping(_) | UpstreamMessage::Pong(_)) { continue; }
                if let UpstreamMessage::Text(text) = &message {
                    if let Some(response_id) = response_id(text.as_str()) {
                        core.remember_response_owner(&response_id, &account.id);
                    }
                    if event_type(text.as_str()).as_deref() == Some("error") {
                        if is_parallel_tool_calls_requirement(text.as_str()) {
                            let detail = friendly_upstream_error(StatusCode::BAD_REQUEST, text.as_str());
                            let _ = client.send(ClientMessage::Text(error_event(StatusCode::BAD_REQUEST, &detail).into())).await;
                            continue;
                        }
                        if let Some((status, _headers)) = error_event_details(text.as_str()) {
                            if is_usage_limit(status, text.as_str()) {
                                let _ = core.mark_usage_limit(&account.id, refusal_reset_at(text.as_str())).await;
                            } else if status == StatusCode::UNAUTHORIZED.as_u16()
                                || status == StatusCode::FORBIDDEN.as_u16()
                            {
                                let kind = codex::classify_account_error(text.as_str());
                                let http_status = StatusCode::from_u16(status)
                                    .unwrap_or(StatusCode::BAD_GATEWAY);
                                let detail = friendly_upstream_error(http_status, text.as_str());
                                let _ = core.mark_account_status(&account.id, kind, detail).await;
                            }
                        }
                    }
                }
                if client.send(convert_upstream_message(message)).await.is_err() { break; }
            }
        }
    }
    let _ = upstream.close(None).await;
}
fn body_previous_response_id(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|payload| {
            payload
                .get("previous_response_id")?
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn response_id_from_path(path: &str) -> Option<String> {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    segments.windows(2).find_map(|pair| {
        if pair[0] != "responses"
            || matches!(pair[1], "compact" | "input_items" | "count" | "tokens")
        {
            return None;
        }
        (!pair[1].is_empty()).then(|| pair[1].to_string())
    })
}

async fn send_to_upstream(socket: &mut UpstreamSocket, message: ClientMessage) -> Result<(), ()> {
    let upstream = match message {
        ClientMessage::Text(text) => UpstreamMessage::Text(text.to_string().into()),
        ClientMessage::Binary(data) => UpstreamMessage::Binary(data),
        ClientMessage::Ping(data) => UpstreamMessage::Ping(data),
        ClientMessage::Pong(data) => UpstreamMessage::Pong(data),
        ClientMessage::Close(_) => UpstreamMessage::Close(None),
    };
    socket.send(upstream).await.map_err(|_| ())
}

fn convert_upstream_message(message: UpstreamMessage) -> ClientMessage {
    match message {
        UpstreamMessage::Text(text) => ClientMessage::Text(text.to_string().into()),
        UpstreamMessage::Binary(data) => ClientMessage::Binary(data),
        UpstreamMessage::Ping(data) => ClientMessage::Ping(data),
        UpstreamMessage::Pong(data) => ClientMessage::Pong(data),
        UpstreamMessage::Close(_) => ClientMessage::Close(None),
        UpstreamMessage::Frame(_) => ClientMessage::Close(None),
    }
}

fn should_forward_header(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "host"
            | "connection"
            | "upgrade"
            | "content-length"
            | "authorization"
            | "chatgpt-account-id"
            | "cookie"
            | "proxy-authorization"
            | "sec-websocket-key"
            | "sec-websocket-version"
            | "sec-websocket-accept"
            | "sec-websocket-extensions"
    )
}

fn should_forward_response_header(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "connection" | "transfer-encoding" | "content-length" | "set-cookie"
    ) && !name.as_str().starts_with("cf-")
}

async fn update_usage_from_headers(core: &AppCore, account_id: &str, headers: &HeaderMap) {
    let now = now_timestamp();
    let mut vault = core.vault.write().await;
    let Some(account) = vault
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
    else {
        return;
    };
    let mut usage = account.usage.clone().unwrap_or(UsageSnapshot {
        primary: None,
        secondary: None,
        updated_at: now,
        error: None,
    });
    let mut changed = false;
    for (name, slot) in [
        ("primary", &mut usage.primary),
        ("secondary", &mut usage.secondary),
    ] {
        let used = headers
            .get(format!("x-codex-{name}-used-percent"))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok())
            .map(|value| value.round().clamp(0.0, 100.0) as i32);
        let duration = headers
            .get(format!("x-codex-{name}-window-minutes"))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok());
        let reset = headers
            .get(format!("x-codex-{name}-reset-at"))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok());
        if used.is_some() || duration.is_some() || reset.is_some() {
            changed = true;
            let window = slot.get_or_insert(UsageWindow {
                used_percent: 0,
                window_minutes: None,
                resets_at: None,
            });
            if let Some(value) = used {
                window.used_percent = value;
            }
            if duration.is_some() {
                window.window_minutes = duration;
            }
            if reset.is_some() {
                window.resets_at = reset;
            }
        }
    }
    if changed {
        usage.updated_at = now;
        usage.error = None;
        account.usage = Some(usage);
    }
    drop(vault);
    if changed {
        let _ = core.save_vault().await;
        core.emit_changed();
    }
}

fn is_usage_limit(status: u16, body: &str) -> bool {
    if status != 429 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let error = value.get("error").unwrap_or(&value);
    let kind = error
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or_default();
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| value.get("code").and_then(Value::as_str))
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let reset_at = error
        .get("resets_at")
        .or_else(|| error.get("reset_at"))
        .or_else(|| value.get("resets_at"))
        .or_else(|| value.get("reset_at"))
        .and_then(Value::as_i64)
        .filter(|timestamp| *timestamp > 0);
    let explicit_quota_type = kind == "usage_limit_reached" || code == "usage_limit_reached";
    let explicit_quota_message = message.contains("usage limit")
        || message.contains("weekly limit")
        || message.contains("plan limit");

    // A reset timestamp alone is not enough: 429 responses can also represent
    // short-lived request throttling. Require an explicit quota message, or a
    // usage-limit type/code together with a valid reset time before routing a
    // new attempt to another profile.
    explicit_quota_message || (explicit_quota_type && reset_at.is_some())
}

fn refusal_reset_at(body: &str) -> Option<i64> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    value
        .pointer("/error/resets_at")
        .and_then(Value::as_i64)
        .or_else(|| value.pointer("/error/reset_at").and_then(Value::as_i64))
        .or_else(|| value.get("resets_at").and_then(Value::as_i64))
        .or_else(|| value.get("reset_at").and_then(Value::as_i64))
}

fn event_type(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("type")?
        .as_str()
        .map(str::to_string)
}

fn previous_response_id(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text).ok().and_then(|value| {
        value
            .get("previous_response_id")?
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn response_id(text: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    value
        .pointer("/response/id")
        .and_then(Value::as_str)
        .or_else(|| value.get("response_id").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn observe_sse_response_ids(
    core: &AppCore,
    account_id: &str,
    pending: &mut Vec<u8>,
    chunk: &Bytes,
) {
    const MAX_SSE_LINE_BYTES: usize = 8 * 1024 * 1024;

    pending.extend_from_slice(chunk);
    let mut cursor = 0;
    while let Some(line_length) = pending[cursor..].iter().position(|byte| *byte == b'\n') {
        let line_end = cursor + line_length;
        let mut line = &pending[cursor..line_end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").unwrap_or(data);
            if let Ok(text) = std::str::from_utf8(data) {
                if let Some(response_id) = response_id(text) {
                    core.remember_response_owner(&response_id, account_id);
                }
            }
        }
        cursor = line_end + 1;
    }
    if cursor > 0 {
        pending.drain(..cursor);
    }
    if pending.len() > MAX_SSE_LINE_BYTES {
        pending.clear();
    }
}

fn error_event_details(text: &str) -> Option<(u16, HeaderMap)> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    if value.get("type")?.as_str()? != "error" {
        return None;
    }
    let status = value
        .get("status")
        .or_else(|| value.get("status_code"))
        .and_then(Value::as_u64)
        .map(|value| value as u16)
        .or_else(|| is_usage_limit(429, text).then_some(429))?;
    let mut headers = HeaderMap::new();
    if let Some(values) = value.get("headers").and_then(Value::as_object) {
        for (name, value) in values {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value.as_str().unwrap_or_default()),
            ) {
                headers.insert(name, value);
            }
        }
    }
    Some((status, headers))
}

fn error_event(status: StatusCode, message: &str) -> String {
    serde_json::json!({"type":"error","status":status.as_u16(),"error":{"type":"proxy_error","message":message}}).to_string()
}

fn friendly_upstream_error(status: StatusCode, body: &str) -> String {
    if is_parallel_tool_calls_requirement(body) {
        return "Сервер Codex отклонил параллельные вызовы инструментов и требует последовательный режим. Приложение передало запрос без изменения; это ограничение endpoint. Для параллельных вызовов нужен маршрут, который их поддерживает.".to_string();
    }
    if is_usage_limit(status.as_u16(), body) {
        return "Аккаунт достиг лимита; автоматическое переключение не нашло доступный профиль."
            .to_string();
    }
    match status {
        StatusCode::UNAUTHORIZED => {
            "OpenAI отклонил вход. Обновите аккаунт или войдите в него заново.".to_string()
        }
        StatusCode::FORBIDDEN => {
            "Доступ к Codex для этого аккаунта запрещён. Проверьте план и настройки аккаунта."
                .to_string()
        }
        StatusCode::TOO_MANY_REQUESTS => {
            "Сервис временно ограничил запросы. Подождите и повторите.".to_string()
        }
        _ => format!(
            "Не удалось открыть соединение Codex (HTTP {}). Проверьте интернет и попробуйте снова.",
            status.as_u16()
        ),
    }
}

fn is_parallel_tool_calls_requirement(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("parallel_tool_calls")
        && (lower.contains("requires") || lower.contains("must be false"))
}

fn message_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(
            serde_json::json!({"error":{"type":"proxy_error","message":message}}).to_string(),
        ))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn bytes_response(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Response<Body> {
    let mut response = Response::builder().status(status);
    for (name, value) in headers.iter() {
        if should_forward_response_header(name) {
            response = response.header(name, value);
        }
    }
    response.body(Body::from(body)).unwrap_or_else(|_| {
        message_response(StatusCode::BAD_GATEWAY, "Не удалось передать ответ Codex.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_limit_requires_a_confirmed_quota_response() {
        assert!(is_usage_limit(
            429,
            r#"{"error":{"type":"usage_limit_reached","reset_at":1794662400}}"#,
        ));
        assert!(is_usage_limit(
            429,
            r#"{"error":{"message":"Weekly usage limit reached"}}"#,
        ));
        assert!(!is_usage_limit(
            429,
            r#"{"error":{"type":"rate_limit_error","message":"Too many requests"}}"#,
        ));
        assert!(!is_usage_limit(
            429,
            r#"{"error":{"type":"usage_limit_reached"}}"#,
        ));
        assert!(!is_usage_limit(
            403,
            r#"{"error":{"type":"usage_limit_reached"}}"#,
        ));
    }

    #[test]
    fn parallel_tool_requirement_error_is_descriptive() {
        let detail = friendly_upstream_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Codex Responses-Lite requires parallel_tool_calls to be false"}}"#,
        );
        assert!(detail.contains("передало запрос без изменения"));
        assert!(detail.contains("endpoint"));
    }

    #[test]
    fn response_ids_in_resource_paths_are_detected_for_account_pinning() {
        assert_eq!(
            response_id_from_path("/responses/resp_123"),
            Some("resp_123".to_string())
        );
        assert_eq!(
            response_id_from_path("/v1/responses/resp_456/input_items"),
            Some("resp_456".to_string())
        );
        assert_eq!(response_id_from_path("/responses"), None);
        assert_eq!(response_id_from_path("/responses/compact"), None);
    }

    #[test]
    fn response_chain_ids_are_read_without_rewriting_the_request() {
        let body = Bytes::from_static(br#"{"previous_response_id":"resp_789","input":"next"}"#);
        assert_eq!(
            body_previous_response_id(&body),
            Some("resp_789".to_string())
        );
        assert_eq!(
            previous_response_id(r#"{"type":"response.create","previous_response_id":"resp_789"}"#),
            Some("resp_789".to_string())
        );
    }
}
