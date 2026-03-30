use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::client::HeraldClient;
use crate::config::{parse_duration, Config, FailureAction};
use crate::error::CliError;
use crate::handler;

/// Run the poll loop — periodically fetch messages and dispatch to handlers.
pub async fn run(config: &Config, client: &HeraldClient) -> Result<(), CliError> {
    let poll_interval = parse_duration(&config.poll_interval);
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent));

    tracing::info!(
        interval = ?poll_interval,
        max_concurrent = config.max_concurrent,
        endpoints = config.handlers.len(),
        "starting poll loop"
    );

    loop {
        for (endpoint, handler_config) in &config.handlers {
            // Respect concurrency limit
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tracing::debug!("max concurrent handlers reached, skipping poll cycle");
                    break;
                }
            };

            let messages = match client.poll(endpoint, 1, 300).await {
                Ok(msgs) => msgs,
                Err(e) => {
                    tracing::error!(endpoint = %endpoint, error = %e, "poll failed");
                    drop(permit);
                    continue;
                }
            };

            for msg in messages {
                let handler_cfg = handler_config.clone();
                let client_base = client.base_url().to_string();
                let client_key = client.api_key().to_string();
                let endpoint = endpoint.clone();
                let permit = permit;

                tokio::spawn(async move {
                    let result = handler::run_handler(&handler_cfg, &msg).await;
                    let ack_client = HeraldClient::new(&client_base, &client_key);

                    match result {
                        Ok(hr) if hr.success => {
                            if let Err(e) = ack_client.ack(&endpoint, &msg.message_id).await {
                                tracing::error!(
                                    message_id = %msg.message_id,
                                    error = %e,
                                    "ACK failed"
                                );
                            } else {
                                tracing::info!(message_id = %msg.message_id, "ACKed");
                            }
                        }
                        Ok(hr) => {
                            let permanent = hr.permanent_failure;
                            if let Err(e) =
                                ack_client.nack(&endpoint, &msg.message_id, permanent).await
                            {
                                tracing::error!(
                                    message_id = %msg.message_id,
                                    error = %e,
                                    "NACK failed"
                                );
                            } else {
                                tracing::warn!(
                                    message_id = %msg.message_id,
                                    permanent,
                                    "NACKed"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                message_id = %msg.message_id,
                                error = %e,
                                "handler error"
                            );
                            let permanent =
                                handler_cfg.on_failure == FailureAction::NackPermanent;
                            let _ =
                                ack_client.nack(&endpoint, &msg.message_id, permanent).await;
                        }
                    }

                    drop(permit);
                });

                // Only one permit was acquired above; break to re-check on next cycle
                break;
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}
