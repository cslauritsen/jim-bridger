use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Delivers a raw email message locally via Dovecot's LDA binary, matching
/// the Python implementation's `deliver_to_dovecot`:
/// `<lda_path> -d <target_unix_user>` fed the raw message on stdin.
pub async fn deliver_to_dovecot(
    raw_email_bytes: &[u8],
    target_unix_user: &str,
    lda_path: &str,
) -> Result<(), String> {
    let mut child = Command::new(lda_path)
        .arg("-d")
        .arg(target_unix_user)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn dovecot-lda: {e}"))?;

    // Write stdin concurrently with draining stdout/stderr (via
    // wait_with_output) to avoid deadlocking on large messages that could
    // fill the pipe buffers in either direction.
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "dovecot-lda stdin unavailable".to_string())?;
    let raw_email_bytes = raw_email_bytes.to_vec();
    let writer = tokio::spawn(async move {
        let result = stdin.write_all(&raw_email_bytes).await;
        drop(stdin);
        result
    });

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait on dovecot-lda: {e}"))?;

    writer
        .await
        .map_err(|e| format!("stdin writer task failed: {e}"))?
        .map_err(|e| format!("Failed to write to dovecot-lda stdin: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Mailstore delivery execution failure: {stderr}"));
    }

    Ok(())
}
