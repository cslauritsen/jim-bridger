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

/// The display name and email address extracted from a message's `From` header.
#[derive(Debug, PartialEq)]
pub struct SenderInfo {
    pub name: Option<String>,
    pub email: String,
}

/// Returns the sender's display name and email address from the `From` header, if any.
pub fn original_sender(msg: &Message) -> Option<SenderInfo> {
    let addr = msg.from()?.first()?;
    let email = addr.address()?.to_string();
    let name = addr.name().map(|s| s.to_string());
    Some(SenderInfo { name, email })
}

/// Parses raw email bytes using the same permissive policy behavior as the
/// Python code's `email.message_from_bytes(raw, policy=SMTP_POLICY)`.
pub fn parse_message(raw: &[u8]) -> Option<Message<'_>> {
    MessageParser::default().parse(raw)
}

/// Rewrites the raw message's header block for SES forwarding:
/// - `From` is always set to `"Sender Name (via domain) <forwarder_address>"` so
///   SES sees a verified sending identity while the original sender remains visible.
/// - `Reply-To` is set to the original sender's email address if the message
///   doesn't already carry one, so replies reach the real sender.
///
/// The body and all other headers are preserved byte-for-byte.
pub fn rewrite_sender_headers(raw: &[u8], sender: Option<&SenderInfo>, forwarder_address: &str) -> Vec<u8> {
    let (header_block, sep, body) = match split_header_block(raw) {
        Some(parts) => parts,
        None => return raw.to_vec(),
    };

    let header_text = String::from_utf8_lossy(header_block);
    let mut lines = fold_header_lines(&header_text);

    // Build a From value that shows the original sender's identity while using
    // the verified forwarder address as the actual mailbox SES will send from.
    let from_value = build_from_value(sender, forwarder_address);
    set_header(&mut lines, "From", &from_value);

    // SES also checks the Sender header if present. Strip it so SES only sees
    // the rewritten From address.
    remove_header(&mut lines, "Sender");

    // Set Reply-To to the original sender's email so replies reach them,
    // but only if the message doesn't already have its own Reply-To.
    if let Some(s) = sender {
        if !has_header(&lines, "Reply-To") {
            set_header(&mut lines, "Reply-To", &s.email);
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

/// Builds the `From` header value, incorporating the original sender's name/address
/// so the recipient can see who originally sent the message.
///
/// Examples:
/// - name + email  → `"Chad Lauritsen (via example.com)" <forwarder@example.com>`
/// - email only    → `"chad@other.com (via example.com)" <forwarder@example.com>`
/// - no sender     → `forwarder@example.com`
fn build_from_value(sender: Option<&SenderInfo>, forwarder_address: &str) -> String {
    let via_domain = forwarder_address
        .rsplit_once('@')
        .map(|(_, domain)| domain)
        .unwrap_or(forwarder_address);

    match sender {
        None => forwarder_address.to_string(),
        Some(s) => {
            let label = s.name.as_deref().unwrap_or(&s.email);
            // Parentheses in display names require RFC 5322 quoting.
            format!("\"{label} (via {via_domain})\" <{forwarder_address}>")
        }
    }
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

/// Returns true if any header line matching `name` (case-insensitive) is present.
fn has_header(lines: &[String], name: &str) -> bool {
    let prefix = format!("{name}:");
    lines.iter().any(|l| l.len() >= prefix.len() && l[..prefix.len()].eq_ignore_ascii_case(&prefix))
}

/// Removes all header lines matching `name` (case-insensitive).
fn remove_header(lines: &mut Vec<String>, name: &str) {
    let prefix = format!("{name}:");
    lines.retain(|l| l.len() < prefix.len() || !l[..prefix.len()].eq_ignore_ascii_case(&prefix));
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
        let s = original_sender(&msg).unwrap();
        assert_eq!(s.email, "alice@example.com");
        assert_eq!(s.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn rewrites_from_with_display_name_and_preserves_existing_reply_to() {
        let msg = parse_message(SAMPLE_WITH_REPLY_TO).unwrap();
        let sender = original_sender(&msg);
        let rewritten = rewrite_sender_headers(SAMPLE_WITH_REPLY_TO, sender.as_ref(), "forwarder@example.com");
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("\"Alice (via example.com)\" <forwarder@example.com>"));
        // Existing Reply-To is preserved; original sender is NOT injected over it.
        assert!(text.contains("Reply-To: old@example.com\r\n"));
        assert!(text.ends_with("Body text.\r\n"));
    }

    #[test]
    fn inserts_reply_to_when_missing() {
        let msg = parse_message(SAMPLE_TO_CC).unwrap();
        let sender = original_sender(&msg);
        let rewritten = rewrite_sender_headers(SAMPLE_TO_CC, sender.as_ref(), "forwarder@example.com");
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("\"Alice (via example.com)\" <forwarder@example.com>"));
        assert!(text.contains("Reply-To: alice@example.com\r\n"));
    }

    #[test]
    fn replaces_from_when_no_original_sender() {
        let rewritten = rewrite_sender_headers(SAMPLE_EMPTY_FROM, None, "forwarder@example.com");
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("From: forwarder@example.com\r\n"));
        assert!(!text.contains("Reply-To"));
    }
}

#[cfg(test)]
mod real_sample_tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"))
    }

    /// Real inbound spam with `To: undisclosed-recipients:;` (an address
    /// group with no members) and no usable `From` address — should not
    /// panic, and should fall back to the default recipient since there are
    /// no concrete addresses to route on.
    #[test]
    fn undisclosed_recipients_group_falls_back_to_default() {
        let raw = fixture("34hkfr3mrk2pgrbvmcqlbi3d0vj4bekh6d9cts01");
        let msg = parse_message(&raw).expect("should parse malformed real-world message");
        assert_eq!(extract_recipients(&msg, "csl"), vec!["csl"]);
        assert_eq!(original_sender(&msg), None);
    }

    /// Real inbound spam with non-RFC5322 `To: ME` / `From: Paul Forster`
    /// (no `@`, no angle brackets) — must not panic and must fall back to
    /// the default recipient.
    #[test]
    fn non_rfc_addresses_fall_back_to_default() {
        let raw = fixture("74116765a80dd58r31hh1coqts7apoktnl7d5r01");
        let msg = parse_message(&raw).expect("should parse malformed real-world message");
        assert_eq!(extract_recipients(&msg, "csl"), vec!["csl"]);
        assert!(original_sender(&msg).is_none());
    }

    /// Real message carrying an `X-Forwarded-To` header alongside a
    /// different `To` address — `X-Forwarded-To` must take precedence, and
    /// the original sender must still be extracted from `From`.
    #[test]
    fn x_forwarded_to_takes_precedence_on_real_message() {
        let raw = fixture("atq3c8qh13hbclfkcnckqg329imgus5bcqlje281");
        let msg = parse_message(&raw).expect("should parse real message");
        assert_eq!(extract_recipients(&msg, "csl"), vec!["chad@planetlauritsen.com"]);
        assert_eq!(original_sender(&msg).map(|s| s.email).as_deref(), Some("noreply@civicplus.com"));
    }

    /// Plain real message with a single `To` address and normal `From`.
    #[test]
    fn plain_message_extracts_to_and_sender() {
        let raw = fixture("s9om2v2rqef3o1of9sdb76tpoojo8banlhachtg1");
        let msg = parse_message(&raw).expect("should parse real message");
        assert_eq!(extract_recipients(&msg, "csl"), vec!["sherlink@planetlauritsen.com"]);
        assert_eq!(original_sender(&msg).map(|s| s.email).as_deref(), Some("google-noreply@google.com"));
    }

    /// Ensures the header-rewrite pass round-trips real, large,
    /// multi-part/MIME messages without corrupting the body: byte length
    /// changes should be limited to the header block, and the body bytes
    /// must be preserved exactly.
    #[test]
    fn rewrite_preserves_body_on_all_real_fixtures() {
        let names = [
            "0sl4ra9q4belnr5gde9gn0s4gliceo7gulachtg1",
            "34hkfr3mrk2pgrbvmcqlbi3d0vj4bekh6d9cts01",
            "74116765a80dd58r31hh1coqts7apoktnl7d5r01",
            "7epml1jp15oj8nv8qo07iofas00q7ui6npkf9ig1",
            "atq3c8qh13hbclfkcnckqg329imgus5bcqlje281",
            "b3r55m93n3mckvhelfitk6k3ts7aqf36q3j7kh01",
            "eb6emvij2fj9vjh8lfmbo3vbaep130653qqii5o1",
            "pme2o3jsue7lc37h7b56jj703ft5jom21hfc5v01",
            "s9om2v2rqef3o1of9sdb76tpoojo8banlhachtg1",
        ];
        for name in names {
            let raw = fixture(name);
            let msg = parse_message(&raw).unwrap_or_else(|| panic!("failed to parse {name}"));
            let sender = original_sender(&msg);
            let rewritten = rewrite_sender_headers(&raw, sender.as_ref(), "ses-forwarder@planetlauritsen.com");

            let original_body = split_header_block(&raw).map(|(_, _, body)| body);
            let rewritten_body = split_header_block(&rewritten).map(|(_, _, body)| body);
            assert_eq!(
                original_body, rewritten_body,
                "body bytes must be unchanged for {name}"
            );
        }
    }
}

