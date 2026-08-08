use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::ProcessOutcome;

struct SmtpResponse {
    code: u16,
    text: String,
}

pub async fn forward_via_smtp(
    raw_message: Vec<u8>,
    envelope_sender: &str,
    recipients: &[String],
    relay_host: &str,
    relay_port: u16,
) -> Result<(), ProcessOutcome> {
    tracing::info!(
        "Forwarding message via SMTP relay: {}:{}, envelope-from={}, recipients={recipients:?}",
        relay_host,
        relay_port,
        envelope_sender
    );

    let stream = TcpStream::connect((relay_host, relay_port))
        .await
        .map_err(|e| {
            ProcessOutcome::TransientFailure(format!(
                "Failed to connect to SMTP relay {relay_host}:{relay_port}: {e}"
            ))
        })?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    expect_response("banner", &read_response(&mut reader).await?, &[220])?;

    write_half
        .write_all(b"EHLO localhost\r\n")
        .await
        .map_err(|e| {
            ProcessOutcome::TransientFailure(format!("Failed to send EHLO to SMTP relay: {e}"))
        })?;
    expect_response("EHLO", &read_response(&mut reader).await?, &[250])?;

    let mail_from = format!("MAIL FROM:<{envelope_sender}>\r\n");
    write_half
        .write_all(mail_from.as_bytes())
        .await
        .map_err(|e| {
            ProcessOutcome::TransientFailure(format!("Failed to send MAIL FROM to SMTP relay: {e}"))
        })?;
    expect_response("MAIL FROM", &read_response(&mut reader).await?, &[250])?;

    for recipient in recipients {
        let rcpt_to = format!("RCPT TO:<{recipient}>\r\n");
        write_half
            .write_all(rcpt_to.as_bytes())
            .await
            .map_err(|e| {
                ProcessOutcome::TransientFailure(format!(
                    "Failed to send RCPT TO for {recipient}: {e}"
                ))
            })?;
        expect_response("RCPT TO", &read_response(&mut reader).await?, &[250, 251])?;
    }

    write_half.write_all(b"DATA\r\n").await.map_err(|e| {
        ProcessOutcome::TransientFailure(format!("Failed to send DATA command: {e}"))
    })?;
    expect_response("DATA", &read_response(&mut reader).await?, &[354])?;

    let data = dot_stuff_and_normalize_line_endings(&raw_message);
    write_half.write_all(&data).await.map_err(|e| {
        ProcessOutcome::TransientFailure(format!("Failed to write SMTP DATA body: {e}"))
    })?;
    write_half.write_all(b".\r\n").await.map_err(|e| {
        ProcessOutcome::TransientFailure(format!("Failed to terminate SMTP DATA body: {e}"))
    })?;
    expect_response("DATA end", &read_response(&mut reader).await?, &[250])?;

    write_half.write_all(b"QUIT\r\n").await.map_err(|e| {
        ProcessOutcome::TransientFailure(format!("Failed to send QUIT to SMTP relay: {e}"))
    })?;
    expect_response("QUIT", &read_response(&mut reader).await?, &[221])?;

    tracing::debug!("forwarded message via SMTP relay to {recipients:?}");
    Ok(())
}

fn expect_response(
    stage: &str,
    response: &SmtpResponse,
    expected_codes: &[u16],
) -> Result<(), ProcessOutcome> {
    if expected_codes.contains(&response.code) {
        return Ok(());
    }

    let reason = format!(
        "Unexpected SMTP response during {stage}: code={}, response={}",
        response.code,
        response.text.trim()
    );
    match response.code / 100 {
        5 => Err(ProcessOutcome::PermanentFailure(reason)),
        _ => Err(ProcessOutcome::TransientFailure(reason)),
    }
}

async fn read_response(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<SmtpResponse, ProcessOutcome> {
    let mut response = String::new();
    let mut code: Option<u16> = None;

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.map_err(|e| {
            ProcessOutcome::TransientFailure(format!("Failed reading SMTP response: {e}"))
        })?;
        if n == 0 {
            return Err(ProcessOutcome::TransientFailure(
                "SMTP relay closed connection while awaiting response".to_string(),
            ));
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.len() < 3 {
            return Err(ProcessOutcome::TransientFailure(format!(
                "Malformed SMTP response line (too short): {trimmed}"
            )));
        }
        let line_code = trimmed[0..3].parse::<u16>().map_err(|_| {
            ProcessOutcome::TransientFailure(format!("Malformed SMTP response code: {trimmed}"))
        })?;

        if let Some(existing) = code {
            if existing != line_code {
                return Err(ProcessOutcome::TransientFailure(format!(
                    "Inconsistent SMTP multiline response codes: {existing} then {line_code}"
                )));
            }
        } else {
            code = Some(line_code);
        }

        response.push_str(&line);
        let continuation = trimmed.as_bytes().get(3).copied() == Some(b'-');
        if !continuation {
            return Ok(SmtpResponse {
                code: line_code,
                text: response,
            });
        }
    }
}

fn dot_stuff_and_normalize_line_endings(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 64);
    let mut line = Vec::new();

    for &byte in raw {
        if byte == b'\n' {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            write_data_line(&mut out, &line);
            line.clear();
        } else {
            line.push(byte);
        }
    }

    if !line.is_empty() {
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        write_data_line(&mut out, &line);
    }

    out
}

fn write_data_line(out: &mut Vec<u8>, line: &[u8]) {
    if line.first() == Some(&b'.') {
        out.push(b'.');
    }
    out.extend_from_slice(line);
    out.extend_from_slice(b"\r\n");
}

#[cfg(test)]
mod tests {
    use super::dot_stuff_and_normalize_line_endings;

    #[test]
    fn dot_stuffs_lines_that_start_with_period() {
        let raw = b"Subject: hello\r\n\r\n.line one\r\n..line two\r\n";
        let out = dot_stuff_and_normalize_line_endings(raw);
        assert!(String::from_utf8_lossy(&out).contains("\r\n..line one\r\n...line two\r\n"));
    }

    #[test]
    fn normalizes_lf_line_endings_to_crlf() {
        let raw = b"Header: value\n\nbody\n";
        let out = dot_stuff_and_normalize_line_endings(raw);
        assert_eq!(out, b"Header: value\r\n\r\nbody\r\n");
    }
}
