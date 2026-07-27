mod config;
mod delivery;
mod email_util;
mod routing;
mod sqs_poller;

use std::sync::Arc;

use config::Config;
use routing::RoutingConfig;

#[tokio::main]
async fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL_ROOT")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).compact().init();

    let config = Arc::new(Config::from_env());
    let routing = Arc::new(RoutingConfig::new(&config.alias_config_path));

    sqs_poller::run(config, routing).await;
}
