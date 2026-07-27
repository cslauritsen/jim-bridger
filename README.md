# Jim Bridger

A Rust email routing and delivery service that processes email notifications from AWS SES via SQS and S3, routing them to local Dovecot mailboxes or remote SMTP recipients based on configurable routing rules.

## Behavior

Jim Bridger operates as an SQS long-polling consumer that:

1. **Receives notifications**: Monitors an AWS SQS queue for S3 object-created events
2. **Fetches email**: Retrieves raw email messages from S3
3. **Routes email**: Looks up recipient addresses in a JSON routing configuration file and applies routing rules
4. **Delivers locally**: Forwards mail to local Unix user mailboxes via Dovecot LDA (Local Delivery Agent)
5. **Delivers remotely**: Forwards mail to external recipients via AWS SES
6. **Cleans up**: Deletes processed messages from SQS and email objects from S3

### Routing Rules

Email recipients are matched against a JSON routing configuration (`aliases.json` by default). Each recipient can have multiple delivery targets:

- **`lda` targets**: Deliver to a local Unix user mailbox via Dovecot LDA
- **`smtp` targets**: Forward to an external email address via AWS SES

Example routing configuration:

```json
{
  "user@example.com": {
    "targets": [
      { "target": "localuser", "type": "lda" }
    ]
  },
  "admin@example.com": {
    "targets": [
      { "target": "admin", "type": "lda" },
      { "target": "external@otherdomain.com", "type": "smtp" }
    ]
  }
}
```

## Configuration

Jim Bridger is configured entirely through environment variables.

### Required Variables

- **`SQS_QUEUE_URL`** (required): URL of the SQS queue to monitor. The service will panic if unset.
- **`SQS_DLQ_URL`** (required): URL of the dead-letter queue used after retry exhaustion. The service will panic if unset.

### AWS Configuration

- **`AWS_REGION`** (default: `us-east-2`): AWS region for S3, SQS, and SES services
- **`SQS_QUEUE_URL`** (required): URL of the SQS queue to monitor
- **`SQS_DLQ_URL`** (required): URL of the dead-letter queue for failed messages

### SQS Polling Configuration

- **`SQS_MAX_RETRIES`** (default: `5`): Maximum number of retry attempts for failed message deliveries
- **`SQS_POLL_WAIT`** (default: `20`): Long-poll wait time in seconds
- **`SQS_VISIBILITY_TIMEOUT`** (default: `300`): Message visibility timeout in seconds. Must exceed worst-case delivery latency (dovecot-lda + SES calls combined) or messages may be reprocessed concurrently

### Routing Configuration

- **`ALIAS_CONFIG_PATH`** (default: `/etc/jim-bridger/aliases.json`): Path to the routing configuration JSON file
- **`DEFAULT_RECIPIENT`** (default: `csl`): Default Unix user for mail with no matching routing rule
- **`FORWARDER_ADDRESS`** (default: `ses-forwarder@planetlauritsen.com`): "From" address for SES-forwarded mail

### Local Delivery Configuration

- **`LDA_PATH`** (default: `/usr/lib/dovecot/dovecot-lda`): Path to the Dovecot LDA binary. Override if your installation uses a different location or if you need to use a wrapper script

## Logging

Logging is controlled by the `LOG_LEVEL_ROOT` environment variable. If unset, defaults to `info` level.

## Example Deployment

### Minimal Configuration

```bash
export SQS_QUEUE_URL=https://sqs.us-east-1.amazonaws.com/123456789/jim-bridger-queue
export SQS_DLQ_URL=https://sqs.us-east-1.amazonaws.com/123456789/jim-bridger-dlq
export ALIAS_CONFIG_PATH=/etc/jim-bridger/aliases.json
cargo run --release
```

### Full SQS Configuration

```bash
export AWS_REGION=us-east-1
export SQS_QUEUE_URL=https://sqs.us-east-1.amazonaws.com/123456789/jim-bridger-queue
export SQS_DLQ_URL=https://sqs.us-east-1.amazonaws.com/123456789/jim-bridger-dlq
export SQS_MAX_RETRIES=5
export SQS_POLL_WAIT=20
export SQS_VISIBILITY_TIMEOUT=300
export ALIAS_CONFIG_PATH=/etc/jim-bridger/aliases.json
export DEFAULT_RECIPIENT=csl
export FORWARDER_ADDRESS=ses-forwarder@example.com
export LDA_PATH=/usr/lib/dovecot/dovecot-lda
export LOG_LEVEL_ROOT=info
cargo run --release
```

### Custom LDA Path Example

If your Dovecot installation is in a non-standard location or you need to use a wrapper script:

```bash
export LDA_PATH=/opt/custom-dovecot/lda-wrapper
# ... other configuration
cargo run --release
```

## Building

```bash
cargo build --release
```

The binary will be available at `target/release/jim_bridger`.

## Testing

```bash
cargo test
```

## systemd service

A production unit file is included at `deploy/systemd/jim-bridger.service`.

1. Install the binary to `/usr/local/bin/jim_bridger`.
2. Create `/etc/jim-bridger/jim-bridger.env` with the required environment variables (`SQS_QUEUE_URL`, `SQS_DLQ_URL`) and any optional overrides.
3. Copy the unit file to `/etc/systemd/system/jim-bridger.service`.
4. Create service user and working directory:

```bash
sudo useradd --system --home /var/lib/jim-bridger --shell /usr/sbin/nologin jim-bridger
sudo mkdir -p /var/lib/jim-bridger /etc/jim-bridger
sudo chown -R jim-bridger:jim-bridger /var/lib/jim-bridger /etc/jim-bridger
```

5. Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now jim-bridger
```

The unit uses `Restart=on-failure` with `RestartSec=5s`, so crashes and non-zero exits are automatically restarted.

## Development

This service runs as an SQS-driven mail processor: it reads S3 object notifications from SQS, fetches the raw message from S3, and routes each message according to alias configuration.
