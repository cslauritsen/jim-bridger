use std::env;

/// Runtime configuration, loaded from environment variables.
///
/// Variable names and defaults intentionally match the original Python
/// implementation so existing deployment tooling (terraform/helm) keeps
/// working without changes.
#[derive(Debug, Clone)]
pub struct Config {
    pub sqs_queue_url: Option<String>,
    pub sqs_dlq_url: Option<String>,
    pub s3_bucket_name: Option<String>,
    pub aws_region: String,
    pub sqs_max_retries: u32,
    pub sqs_poll_wait: i32,
    pub enable_sqs_poll: bool,
    pub alias_config_path: String,
    pub default_recipient: String,
    pub forwarder_address: String,
    /// Required for parity with the Python service (which reads
    /// `os.environ['PRE_SHARED_SECRET']` and panics if unset). Currently
    /// unused by the message-processing logic itself.
    #[allow(dead_code)]
    pub pre_shared_secret: String,
}

impl Config {
    pub fn from_env() -> Self {
        let pre_shared_secret = env::var("PRE_SHARED_SECRET")
            .expect("PRE_SHARED_SECRET environment variable must be set");

        Config {
            sqs_queue_url: env::var("SQS_QUEUE_URL").ok(),
            sqs_dlq_url: env::var("SQS_DLQ_URL").ok(),
            s3_bucket_name: env::var("S3_BUCKET_NAME").ok(),
            aws_region: env::var("AWS_REGION").unwrap_or_else(|_| "us-east-2".to_string()),
            sqs_max_retries: env::var("SQS_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            sqs_poll_wait: env::var("SQS_POLL_WAIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            enable_sqs_poll: env::var("ENABLE_SQS_POLL")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
            alias_config_path: env::var("ALIAS_CONFIG_PATH")
                .unwrap_or_else(|_| "/etc/jim-bridger/aliases.json".to_string()),
            default_recipient: env::var("DEFAULT_RECIPIENT").unwrap_or_else(|_| "csl".to_string()),
            forwarder_address: env::var("FORWARDER_ADDRESS")
                .unwrap_or_else(|_| "ses-forwarder@planetlauritsen.com".to_string()),
            pre_shared_secret,
        }
    }
}
