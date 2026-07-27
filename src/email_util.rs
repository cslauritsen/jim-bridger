use mail_parser::{Message, MessageParser};

/// Extracts the list of recipients to evaluate against the routing map,
/// mirroring the logic in `process_email_message` in the Python service:
/// prefer a single `X-Forwarded-To` header if present, otherwise combine
/// To/Cc/Bcc addresses, falling back to a default recipient if none found.
pub fn extract_recipients(msg: &Message, default_recipient: &str) -> Vec<String> {
    let forwarded_to: Vec<String> = msg
        .header_values("X-Forwarded-To")
        .filter_map(|v| v.as_text().map(|s| s.to_string()))
        .collect();

    if !forwarded_to.is_empty() {
        tracing::info!("X-Forwarded-For detected: {:?}", forwarded_to);
        return vec![forwarded_to[0].clone()];
    }

    let mut recipients = Vec::new();
    for addr in [msg.to(), msg.cc(), msg.bcc()].into_iter().flatten() {
        for a in addr.iter() {
            if let Some(email) = a.address() {
                recipients.push(email.to_string());
            }
        }
    }

    if recipients.is_empty() {
        recipients.push(default_recipient.to_string());
    }

    recipients
}

/// Returns the bare email address from the message's `From` header, if any.
pub fn original_sender(msg: &Message) -> Option<String> {
    msg.from()?.first()?.address().map(|s| s.to_string())
}

/// Parses raw email bytes using the same permissive policy behavior as the
/// Python code's `email.message_from_bytes(raw, policy=SMTP_POLICY)`.
pub fn parse_message(raw: &[u8]) -> Option<Message<'_>> {
    MessageParser::default().parse(raw)
}

/// Rewrites the raw message's header block so that:
/// - if the original message had a non-empty `From` address, the `From`
///   header is left untouched and `Reply-To` is set/replaced with that
///   original sender address (so replies go back to the real sender);
/// - otherwise, the `From` header is replaced with `forwarder_address`.
///
/// This mirrors the header manipulation performed before SMTP-forwarding in
/// the Python implementation. The body and all other headers are preserved
/// byte-for-byte.
pub fn rewrite_sender_headers(raw: &[u8], original_sender: Option<&str>, forwarder_address: &str) -> Vec<u8> {
    let (header_block, sep, body) = match split_header_block(raw) {
        Some(parts) => parts,
        None => return raw.to_vec(),
    };

    let header_text = String::from_utf8_lossy(header_block);
    let mut lines = fold_header_lines(&header_text);

    match original_sender {
        Some(sender) if !sender.is_empty() => {
            set_header(&mut lines, "Reply-To", sender);
        }
        _ => {
            set_header(&mut lines, "From", forwarder_address);
        }
    }

    let mut result = Vec::with_capacity(raw.len());
    for line in lines {
        result.extend_from_slice(line.as_bytes());
    }
    result.extend_from_slice(sep);
    result.extend_from_slice(body);
    result
}

/// Splits raw message bytes into (header_block, separator, body), where the
/// separator is whichever blank-line sequence (`\r\n\r\n` or `\n\n`) appears
/// first, and the header_block includes trailing newlines for each header
/// line but not the separator itself.
fn split_header_block(raw: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    if let Some(pos) = find_subslice(raw, b"\r\n\r\n") {
        return Some((&raw[..pos + 2], &raw[pos + 2..pos + 4], &raw[pos + 4..]));
    }
    if let Some(pos) = find_subslice(raw, b"\n\n") {
        return Some((&raw[..pos + 1], &raw[pos + 1..pos + 2], &raw[pos + 2..]));
    }
    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Splits a header block into logical header lines, joining folded
/// (continuation) lines that start with whitespace onto the previous line.
/// Each returned line retains its original trailing line ending.
fn fold_header_lines(header_text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw_line in split_keep_ending(header_text) {
        let starts_folded = raw_line
            .chars()
            .next()
            .map(|c| c == ' ' || c == '\t')
            .unwrap_or(false);
        if starts_folded
            && let Some(last) = lines.last_mut()
        {
            last.push_str(raw_line);
            continue;
        }
        lines.push(raw_line.to_string());
    }
    lines
}

/// Splits text into lines, keeping the `\r\n` or `\n` line ending attached to
/// each returned line.
fn split_keep_ending(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            result.push(&text[start..=i]);
            start = i + 1;
        }
        i += 1;
    }
    if start < bytes.len() {
        result.push(&text[start..]);
    }
    result
}

/// Replaces the first header line matching `name` (case-insensitive) with
/// `name: value`, preserving that line's original line ending. If no such
/// header exists, appends a new one (using `\r\n`).
fn set_header(lines: &mut Vec<String>, name: &str, value: &str) {
    let prefix = format!("{name}:");
    for line in lines.iter_mut() {
        if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(&prefix) {
            let ending = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            *line = format!("{name}: {value}{ending}");
            return;
        }
    }
    // Not found: insert before the end, using the same ending style as the
    // rest of the header block (default to \r\n).
    let ending = lines
        .last()
        .map(|l| if l.ends_with("\r\n") { "\r\n" } else { "\n" })
        .unwrap_or("\r\n");
    lines.push(format!("{name}: {value}{ending}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TO_CC: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: bob@example.com\r\n\
Cc: carol@example.com\r\n\
Subject: Hi\r\n\
\r\n\
Body text.\r\n";

    const SAMPLE_XFWD: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: bob@example.com\r\n\
X-Forwarded-To: override@example.com\r\n\
Subject: Hi\r\n\
\r\n\
Body text.\r\n";

    const SAMPLE_NO_RECIPIENTS: &[u8] = b"From: Alice <alice@example.com>\r\n\
Subject: Hi\r\n\
\r\n\
Body text.\r\n";

    const SAMPLE_WITH_REPLY_TO: &[u8] = b"From: Alice <alice@example.com>\r\n\
Reply-To: old@example.com\r\n\
To: bob@example.com\r\n\
Subject: Hi\r\n\
\r\n\
Body text.\r\n";

    const SAMPLE_EMPTY_FROM: &[u8] = b"To: bob@example.com\r\n\
Subject: Hi\r\n\
\r\n\
Body text.\r\n";

    #[test]
    fn extracts_to_and_cc_recipients() {
        let msg = parse_message(SAMPLE_TO_CC).unwrap();
        let recipients = extract_recipients(&msg, "csl");
        assert_eq!(recipients, vec!["bob@example.com", "carol@example.com"]);
    }

    #[test]
    fn x_forwarded_to_overrides_recipients() {
        let msg = parse_message(SAMPLE_XFWD).unwrap();
        let recipients = extract_recipients(&msg, "csl");
        assert_eq!(recipients, vec!["override@example.com"]);
    }

    #[test]
    fn falls_back_to_default_recipient() {
        let msg = parse_message(SAMPLE_NO_RECIPIENTS).unwrap();
        let recipients = extract_recipients(&msg, "csl");
        assert_eq!(recipients, vec!["csl"]);
    }

    #[test]
    fn extracts_original_sender() {
        let msg = parse_message(SAMPLE_TO_CC).unwrap();
        assert_eq!(original_sender(&msg).as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn rewrites_existing_reply_to_and_preserves_from() {
        let rewritten = rewrite_sender_headers(SAMPLE_WITH_REPLY_TO, Some("alice@example.com"), "forwarder@example.com");
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("From: Alice <alice@example.com>\r\n"));
        assert!(text.contains("Reply-To: alice@example.com\r\n"));
        assert!(!text.contains("old@example.com"));
        assert!(text.ends_with("Body text.\r\n"));
    }

    #[test]
    fn inserts_reply_to_when_missing() {
        let rewritten = rewrite_sender_headers(SAMPLE_TO_CC, Some("alice@example.com"), "forwarder@example.com");
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("From: Alice <alice@example.com>\r\n"));
        assert!(text.contains("Reply-To: alice@example.com\r\n"));
    }

    #[test]
    fn replaces_from_when_no_original_sender() {
        let rewritten = rewrite_sender_headers(SAMPLE_EMPTY_FROM, None, "forwarder@example.com");
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("From: forwarder@example.com\r\n"));
    }
}
