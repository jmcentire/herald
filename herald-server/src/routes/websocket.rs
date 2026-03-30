use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::auth;
use crate::crypto;
use crate::queue;
use crate::state::AppState;

const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum ClientMessage {
    Auth {
        api_key: String,
    },
    Ack {
        message_id: String,
    },
    Nack {
        message_id: String,
        #[serde(default)]
        permanent: bool,
    },
    Heartbeat {
        message_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum ServerMessage {
    AuthOk,
    AuthError {
        reason: String,
    },
    Message {
        message_id: String,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<serde_json::Value>,
        received_at: String,
        deliver_count: u32,
    },
    AckOk {
        message_id: String,
    },
    Error {
        reason: String,
    },
}

/// WS /stream/:endpoint_name — WebSocket streaming endpoint
pub async fn websocket_handler(
    State(state): State<AppState>,
    Path(endpoint_name): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, endpoint_name))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, endpoint_name: String) {
    // C006: First-message authentication — no API keys in query params
    let account = match authenticate(&mut socket, &state).await {
        Ok(Some(account)) => account,
        Ok(None) => return, // Socket closed during auth
        Err(reason) => {
            let msg = ServerMessage::AuthError { reason };
            let _ = send_json(&mut socket, &msg).await;
            return;
        }
    };

    // Check WebSocket permission
    if !account.tier.limits().websocket_allowed {
        let msg = ServerMessage::AuthError {
            reason: "WebSocket not available on your tier".into(),
        };
        let _ = send_json(&mut socket, &msg).await;
        return;
    }

    // Send auth confirmation
    if send_json(&mut socket, &ServerMessage::AuthOk)
        .await
        .is_err()
    {
        return;
    }

    tracing::info!(
        customer_id = %account.customer_id,
        endpoint = %endpoint_name,
        "WebSocket connected"
    );

    // Main message loop: poll queue and push to client
    let mut conn = state.redis.clone();
    loop {
        tokio::select! {
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Ok(msg) = serde_json::from_str::<ClientMessage>(&text) {
                            handle_client_message(
                                &mut conn, &mut socket, &account, &endpoint_name, msg,
                            ).await;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        tracing::info!(customer_id = %account.customer_id, "WebSocket disconnected");
                        break;
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                let messages = match queue::fetch(
                    &mut conn, &account.customer_id, &endpoint_name, 1, 300,
                ).await {
                    Ok(msgs) => msgs,
                    Err(e) => {
                        tracing::error!(error = %e, "fetch error in WebSocket loop");
                        continue;
                    }
                };

                for msg in messages {
                    let decrypted_body = match crypto::decrypt(
                        &state.config.service_encryption_key, &msg.body,
                    ) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!(error = %e, "decrypt error");
                            continue;
                        }
                    };

                    let body_b64 = base64::engine::general_purpose::STANDARD.encode(&decrypted_body);

                    let headers = if account.tier.limits().headers_included {
                        msg.headers.as_ref()
                            .and_then(|h| base64::engine::general_purpose::STANDARD.decode(h).ok())
                            .and_then(|encrypted| crypto::decrypt(&state.config.service_encryption_key, &encrypted).ok())
                            .and_then(|decrypted| String::from_utf8(decrypted).ok())
                            .and_then(|json_str| serde_json::from_str(&json_str).ok())
                    } else {
                        None
                    };

                    let server_msg = ServerMessage::Message {
                        message_id: msg.message_id,
                        body: body_b64,
                        headers,
                        received_at: msg.received_at.to_string(),
                        deliver_count: msg.deliver_count,
                    };

                    if send_json(&mut socket, &server_msg).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// First-message authentication with Redis lookup.
async fn authenticate(
    socket: &mut WebSocket,
    state: &AppState,
) -> Result<Option<auth::Account>, String> {
    let auth_result = timeout(AUTH_TIMEOUT, socket.recv()).await;

    match auth_result {
        Ok(Some(Ok(WsMessage::Text(text)))) => {
            let msg: ClientMessage =
                serde_json::from_str(&text).map_err(|_| "expected JSON auth message".to_string())?;

            match msg {
                ClientMessage::Auth { api_key } => {
                    let mut conn = state.redis.clone();
                    match auth::lookup_account(&mut conn, &api_key).await {
                        Ok(account) => Ok(Some(account)),
                        Err(_) => Err("invalid API key".to_string()),
                    }
                }
                _ => Err("first message must be type: auth".to_string()),
            }
        }
        Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) => Ok(None),
        Ok(Some(Err(e))) => Err(format!("WebSocket error: {e}")),
        Err(_) => Err("authentication timeout (5 seconds)".to_string()),
        _ => Err("unexpected message type".to_string()),
    }
}

async fn handle_client_message(
    conn: &mut redis::aio::MultiplexedConnection,
    socket: &mut WebSocket,
    account: &auth::Account,
    endpoint_name: &str,
    msg: ClientMessage,
) {
    match msg {
        ClientMessage::Ack { message_id } => {
            match queue::ack(conn, &account.customer_id, endpoint_name, &message_id).await {
                Ok(true) => {
                    let _ = send_json(socket, &ServerMessage::AckOk { message_id }).await;
                }
                Ok(false) => {
                    let _ = send_json(
                        socket,
                        &ServerMessage::Error {
                            reason: format!("message {message_id} not in flight"),
                        },
                    )
                    .await;
                }
                Err(e) => {
                    let _ = send_json(
                        socket,
                        &ServerMessage::Error {
                            reason: e.to_string(),
                        },
                    )
                    .await;
                }
            }
        }
        ClientMessage::Nack {
            message_id,
            permanent,
        } => {
            let _ = queue::nack(
                conn,
                &account.customer_id,
                endpoint_name,
                &message_id,
                permanent,
                3,
            )
            .await;
        }
        ClientMessage::Heartbeat { message_id } => {
            let _ = queue::heartbeat(conn, &message_id, 300).await;
        }
        ClientMessage::Auth { .. } => {
            let _ = send_json(
                socket,
                &ServerMessage::Error {
                    reason: "already authenticated".into(),
                },
            )
            .await;
        }
    }
}

async fn send_json<T: Serialize>(socket: &mut WebSocket, msg: &T) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|_| ())?;
    socket
        .send(WsMessage::Text(text.into()))
        .await
        .map_err(|_| ())
}
