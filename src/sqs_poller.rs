use std::sync::Arc;
use std::time::Duration;

use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::Client as SesClient;
use aws_sdk_sqs::Client as SqsClient;

use crate::config::Config;
use crate::delivery::{lda, ses, ProcessOutcome};
use crate::email_util;
use crate::routing::RoutingConfig;

/// Starts the SQS long-polling loop that watches for S3-object-created
/// notifications (as delivered by SES -> S3 -> SQS), fetches the raw email
/// from S3, routes it to local (Dovecot LDA) and/or remote (SES) recipients
/// per the alias routing map, and deletes the S3 object / SQS message on
/// success. Mirrors `start_sqs_poller` in the original Python service.
pub async fn run(config: Arc<Config>, routing: Arc<RoutingConfig>) {
    let queue_url = config.sqs_queue_url.clone();

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.aws_region.clone()))
        .load()
        .await;

    let sqs = SqsClient::new(&aws_config);
    let s3 = S3Client::new(&aws_config);
    let sesc = SesClient::new(&aws_config);

    tracing::info!("Starting SQS polling loop");
    loop {
        if let Err(e) = poll_once(&config, &routing, &sqs, &s3, &sesc, &queue_url).await {
            tracing::error!("SQS polling loop error: {e}");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}

async fn poll_once(
    config: &Config,
    routing: &RoutingConfig,
    sqs: &SqsClient,
    s3: &S3Client,
    sesc: &SesClient,
    queue_url: &str,
) -> Result<(), String> {
    let resp = sqs
        .receive_message()
        .queue_url(queue_url)
        .max_number_of_messages(1)
        .wait_time_seconds(config.sqs_poll_wait)
        .visibility_timeout(config.sqs_visibility_timeout)
        .message_attribute_names("All")
        .message_system_attribute_names(aws_sdk_sqs::types::MessageSystemAttributeName::All)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let messages = resp.messages.unwrap_or_default();
    if messages.is_empty() {
        tracing::debug!("No messages found in SQS response");
        return Ok(());
    }

    for msg in messages {
        process_sqs_message(config, routing, sqs, s3, sesc, queue_url, &msg).await;
    }

    Ok(())
}

async fn process_sqs_message(
    config: &Config,
    routing: &RoutingConfig,
    sqs: &SqsClient,
    s3: &S3Client,
    sesc: &SesClient,
    queue_url: &str,
    msg: &aws_sdk_sqs::types::Message,
) {
    let Some(receipt_handle) = msg.receipt_handle.clone() else {
        return;
    };
    let body = msg.body.clone().unwrap_or_default();
    let retry_count: u32 = msg
        .attributes
        .as_ref()
        .and_then(|a| a.get(&aws_sdk_sqs::types::MessageSystemAttributeName::ApproximateReceiveCount))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    let event: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Error processing SQS message: {e}");
            move_to_dlq_if_exhausted(config, sqs, queue_url, &receipt_handle, &body, retry_count).await;
            return;
        }
    };

    let records = event
        .get("Records")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    if records.is_empty() {
        tracing::error!("No Records found in SQS message body: {body}");
        delete_message(sqs, queue_url, &receipt_handle).await;
        return;
    }

    let mut all_success = true;

    for rec in &records {
        let s3_info = rec.get("s3").cloned().unwrap_or_default();
        let s3_bucket = s3_info
            .get("bucket")
            .and_then(|b| b.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());
        let s3_key = s3_info
            .get("object")
            .and_then(|o| o.get("key"))
            .and_then(|k| k.as_str())
            .map(|s| s.to_string());

        let (Some(s3_bucket), Some(s3_key)) = (s3_bucket, s3_key) else {
            tracing::error!("Missing s3 bucket or key in SQS record: {rec}");
            all_success = false;
            continue;
        };

        let s3url = format!("s3://{s3_bucket}/{s3_key}");
        tracing::info!("Processing SQS record for S3 object: {s3url}");

        match s3.get_object().bucket(&s3_bucket).key(&s3_key).send().await {
            Ok(obj) => {
                let raw_email = match obj.body.collect().await {
                    Ok(bytes) => bytes.into_bytes().to_vec(),
                    Err(e) => {
                        tracing::error!("Error reading S3 object body {s3url}: {e}");
                        all_success = false;
                        continue;
                    }
                };

                match process_email_message(config, routing, sesc, &raw_email).await {
                    ProcessOutcome::Success => {
                        if let Err(e) = s3.delete_object().bucket(&s3_bucket).key(&s3_key).send().await {
                            tracing::error!("Failed to delete S3 object {s3url}: {e}");
                        } else {
                            tracing::info!("Successfully processed and deleted S3 object: {s3url}");
                        }
                    }
                    ProcessOutcome::ParsingFailure(err) => {
                        tracing::error!("{s3url} Could not parse email: {err} — S3 object preserved for inspection, moving SQS message to DLQ immediately");
                        // Don't delete the S3 object — leave it for manual inspection.
                        // Don't retry — a corrupt message will never parse. Go straight to DLQ
                        // and delete the original SQS message.
                        move_to_dlq(config, sqs, &body).await;
                        delete_message(sqs, queue_url, &receipt_handle).await;
                    }
                    ProcessOutcome::PermanentFailure(err) => {
                        tracing::error!("{s3url} Permanent failure processing email: {err}");
                        // Retrying will never succeed; delete the S3 object to avoid leaking it.
                        if let Err(e) = s3.delete_object().bucket(&s3_bucket).key(&s3_key).send().await {
                            tracing::error!("Failed to delete S3 object {s3url} after permanent failure: {e}");
                        } else {
                            tracing::warn!("Deleted S3 object {s3url} after permanent failure (email not delivered)");
                        }
                    }
                    ProcessOutcome::TransientFailure(err) => {
                        tracing::error!("{s3url} Failed to process email from SQS record: {err}");
                        all_success = false;
                    }
                }
            }
            Err(e) => {
                if is_no_such_key(&e) {
                    tracing::warn!("S3 object already deleted: {s3url}. Treating as success.");
                    continue;
                }
                tracing::error!("Error processing SQS record: {e}");
                all_success = false;
            }
        }
    }

    if all_success {
        delete_message(sqs, queue_url, &receipt_handle).await;
        tracing::debug!(
            "Successfully processed and deleted SQS message: {receipt_handle} with {} records",
            records.len()
        );
    } else {
        move_to_dlq_if_exhausted(config, sqs, queue_url, &receipt_handle, &body, retry_count).await;
    }
}

fn is_no_such_key(err: &aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::get_object::GetObjectError>) -> bool {
    matches!(
        err,
        aws_sdk_s3::error::SdkError::ServiceError(se)
            if matches!(se.err(), aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey(_))
    )
}

async fn delete_message(sqs: &SqsClient, queue_url: &str, receipt_handle: &str) {
    if let Err(e) = sqs
        .delete_message()
        .queue_url(queue_url)
        .receipt_handle(receipt_handle)
        .send()
        .await
    {
        tracing::error!("Failed to delete SQS message: {e}");
    }
}

async fn move_to_dlq(config: &Config, sqs: &SqsClient, body: &str) {
    if let Err(e) = sqs
        .send_message()
        .queue_url(config.sqs_dlq_url.as_str())
        .message_body(body)
        .send()
        .await
    {
        tracing::error!("Failed to send message to DLQ: {e}");
    }
}

async fn move_to_dlq_if_exhausted(
    config: &Config,
    sqs: &SqsClient,
    queue_url: &str,
    receipt_handle: &str,
    body: &str,
    retry_count: u32,
) {
    if retry_count >= config.sqs_max_retries {
        tracing::warn!("Moving message to DLQ after {retry_count} attempts");
        move_to_dlq(config, sqs, body).await;
        delete_message(sqs, queue_url, receipt_handle).await;
    }
}

/// Parses the raw email, resolves each recipient against the alias routing
/// map, delivers to Dovecot LDA for `lda` targets, and forwards via SES for
/// `smtp` targets. Mirrors `process_email_message` in the Python service.
///
/// CAUTION (not fixed here, needs a design decision): if delivery to one
/// target fails after earlier targets in the same message already succeeded
/// (e.g. LDA delivery to `alice` succeeds, then SES forwarding fails), this
/// function returns a failure for the *whole* record. The SQS message is
/// then retried, redelivering to `alice` a second time. Dovecot's `sieve
/// duplicate` check or an idempotency store keyed on `Message-ID` +
/// recipient would be needed to make retries safe.
async fn process_email_message(
    config: &Config,
    routing: &RoutingConfig,
    sesc: &SesClient,
    raw_email: &[u8],
) -> ProcessOutcome {
    let Some(parsed) = email_util::parse_message(raw_email) else {
        return ProcessOutcome::ParsingFailure("Failed to parse raw email bytes".to_string());
    };

    let recipients = email_util::extract_recipients(&parsed, &config.default_recipient);
    let sender = email_util::original_sender(&parsed);
    let routing_map = routing.get().await;

    let mut smtp_recipients = Vec::new();

    for r in &recipients {
        let norm_r = r.to_lowercase();
        let base_addr = strip_plus_address(&norm_r);
        let entry = routing_map.get(&norm_r)
            .or_else(|| base_addr.as_deref().and_then(|b| routing_map.get(b)));
        let Some(entry) = entry else {
            tracing::warn!("No routing entry found for recipient: {norm_r} — message dropped");
            continue;
        };
        for rule in &entry.targets {
            match rule.target_type.as_str() {
                "lda" => {
                    tracing::info!("local delivery {norm_r} -> {}", rule.target);
                    if let Err(e) = lda::deliver_to_dovecot(raw_email, &rule.target, &config.lda_path).await {
                        return ProcessOutcome::TransientFailure(format!("Error processing email: {e}"));
                    }
                }
                "smtp" => {
                    tracing::info!("smtp forward {norm_r} -> {}", rule.target);
                    smtp_recipients.push(rule.target.clone());
                }
                other => {
                    // A bad `type` value is a static configuration error, not
                    // a transient one; retrying without a human fixing the
                    // alias JSON will never succeed, so treat it as
                    // permanent to avoid an infinite redelivery loop.
                    return ProcessOutcome::PermanentFailure(format!(
                        "Unknown routing rule: {other} for {norm_r}"
                    ));
                }
            }
        }
    }

    if !smtp_recipients.is_empty() {
        let rewritten = email_util::rewrite_sender_headers(raw_email, sender.as_deref(), &config.forwarder_address);
        if let Err(outcome) = ses::forward_via_ses(sesc, rewritten, &config.forwarder_address, &smtp_recipients).await
        {
            return outcome;
        }
    }

    ProcessOutcome::Success
}

/// Strips the plus extension from an email address, returning the base address.
/// Returns `None` if the address has no plus extension or no `@`.
/// e.g. `"chad+blah@example.com"` → `Some("chad@example.com")`
fn strip_plus_address(addr: &str) -> Option<String> {
    let at = addr.rfind('@')?;
    let local = &addr[..at];
    let domain = &addr[at..]; // includes the '@'
    let plus = local.find('+')?;
    Some(format!("{}{}", &local[..plus], domain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_plus_extension() {
        assert_eq!(strip_plus_address("chad+blah@example.com").as_deref(), Some("chad@example.com"));
        assert_eq!(strip_plus_address("user+tag+extra@example.com").as_deref(), Some("user@example.com"));
    }

    #[test]
    fn no_plus_returns_none() {
        assert_eq!(strip_plus_address("chad@example.com"), None);
    }

    #[test]
    fn no_at_returns_none() {
        assert_eq!(strip_plus_address("notanemail"), None);
    }
}
