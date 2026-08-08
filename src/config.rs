use std::env;

/// Runtime configuration, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub sqs_queue_url: String,
    pub sqs_dlq_url: String,
    /// Region for SQS and S3 (where the queue and inbound email bucket live).
    pub aws_region: String,
    /// Region for SES sending. SES identities are region-specific and may be
    /// in a different region than SQS/S3 (e.g. us-east-1 for SES,
    /// us-east-2 for SQS/S3).
    pub ses_region: String,
    pub sqs_max_retries: u32,
    pub sqs_poll_wait: i32,
    /// How long a received message is hidden from other consumers while we
    /// fetch the S3 object and run local/remote delivery. Must comfortably
    /// exceed worst-case delivery latency (dovecot-lda + SES calls) or the
    /// message can become visible again mid-processing, triggering a
    /// concurrent duplicate delivery attempt.
    pub sqs_visibility_timeout: i32,
    pub alias_config_path: String,
    pub default_recipient: String,
    pub forwarder_address: String,
    pub smtp_relay_host: String,
    pub smtp_relay_port: u16,
    /// Path to the Dovecot LDA binary for local mail delivery.
    pub lda_path: String,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            sqs_queue_url: env::var("SQS_QUEUE_URL")
                .expect("SQS_QUEUE_URL environment variable must be set"),
            sqs_dlq_url: env::var("SQS_DLQ_URL")
                .expect("SQS_DLQ_URL environment variable must be set"),
            aws_region: env::var("AWS_REGION").unwrap_or_else(|_| "us-east-2".to_string()),
            ses_region: env::var("SES_REGION").unwrap_or_else(|_| "us-east-2".to_string()),
            sqs_max_retries: env::var("SQS_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            sqs_poll_wait: env::var("SQS_POLL_WAIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            sqs_visibility_timeout: env::var("SQS_VISIBILITY_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            alias_config_path: env::var("ALIAS_CONFIG_PATH")
                .unwrap_or_else(|_| "/etc/jim-bridger/aliases.json".to_string()),
            default_recipient: env::var("DEFAULT_RECIPIENT").unwrap_or_else(|_| "csl".to_string()),
            forwarder_address: env::var("FORWARDER_ADDRESS")
                .unwrap_or_else(|_| "ses-forwarder@planetlauritsen.com".to_string()),
            smtp_relay_host: env::var("SMTP_RELAY_HOST")
                .unwrap_or_else(|_| "localhost".to_string()),
            smtp_relay_port: env::var("SMTP_RELAY_PORT")
                .map(|v| {
                    v.parse::<u16>()
                        .expect("SMTP_RELAY_PORT must be a valid u16 integer")
                })
                .unwrap_or(25),
            lda_path: env::var("LDA_PATH")
                .unwrap_or_else(|_| "/usr/lib/dovecot/dovecot-lda".to_string()),
        }
    }
}
