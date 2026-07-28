use aws_sdk_sesv2::operation::send_email::SendEmailError;
use aws_sdk_sesv2::types::{Destination, EmailContent, RawMessage};
use aws_sdk_sesv2::Client as SesClient;
use aws_smithy_types::Blob;

use super::ProcessOutcome;

/// Forwards a (header-rewritten) raw email to one or more remote recipients
/// via the AWS SES v2 `SendEmail` API using raw message content, mirroring
/// the envelope-sender/recipient behavior of the Python implementation's
/// `smtp_forward` (which set envelope-from to `FORWARDER_ADDRESS` and
/// envelope-to to the resolved SMTP targets) but delivering via SES instead
/// of a direct SMTP/LMTP relay.
pub async fn forward_via_ses(
    ses_client: &SesClient,
    raw_message: Vec<u8>,
    envelope_sender: &str,
    recipients: &[String],
) -> Result<(), ProcessOutcome> {
    tracing::info!("Forwarding message via SES: envelope-from={envelope_sender}, recipients={recipients:?}");

    let content = EmailContent::builder()
        .raw(RawMessage::builder().data(Blob::new(raw_message)).build().map_err(|e| {
            ProcessOutcome::PermanentFailure(format!("Failed to build raw SES message: {e}"))
        })?)
        .build();

    let destination = Destination::builder()
        .set_to_addresses(Some(recipients.to_vec()))
        .build();

    let result = ses_client
        .send_email()
        .from_email_address(envelope_sender)
        .destination(destination)
        .content(content)
        .send()
        .await;

    match result {
        Ok(_) => {
            tracing::debug!("forwarded message via SES to {recipients:?}");
            Ok(())
        }
        Err(err) => {
            // Handle non-service errors (network, timeout, dispatch) before
            // calling into_service_error(), which panics on those variants.
            let service_err = match err {
                aws_sdk_sesv2::error::SdkError::ServiceError(se) => se.into_err(),
                aws_sdk_sesv2::error::SdkError::TimeoutError(e) => {
                    return Err(ProcessOutcome::TransientFailure(format!("SES request timed out: {e:?}")));
                }
                aws_sdk_sesv2::error::SdkError::DispatchFailure(e) => {
                    return Err(ProcessOutcome::TransientFailure(format!("SES dispatch failure: {e:?}")));
                }
                e => {
                    return Err(ProcessOutcome::TransientFailure(format!("SES request failed: {e}")));
                }
            };
            Err(match service_err {
                // Configuration/application errors: the email itself is fine but our
                // setup is broken. Preserve the S3 object for reprocessing after the
                // issue is fixed, and move straight to DLQ (no retries will help).
                SendEmailError::MessageRejected(e) => {
                    ProcessOutcome::ParsingFailure(format!("SES rejected message (unverified identity or sandbox restriction): {e}"))
                }
                SendEmailError::BadRequestException(e) => {
                    ProcessOutcome::ParsingFailure(format!("SES bad request (likely a code bug): {e}"))
                }
                SendEmailError::MailFromDomainNotVerifiedException(e) => {
                    ProcessOutcome::ParsingFailure(format!("SES sending domain not verified: {e}"))
                }
                SendEmailError::AccountSuspendedException(e) => {
                    ProcessOutcome::ParsingFailure(format!("SES account suspended: {e}"))
                }
                SendEmailError::SendingPausedException(e) => {
                    ProcessOutcome::ParsingFailure(format!("SES sending paused: {e}"))
                }
                SendEmailError::LimitExceededException(e) => {
                    ProcessOutcome::TransientFailure(format!("SES limit exceeded: {e}"))
                }
                SendEmailError::TooManyRequestsException(e) => {
                    ProcessOutcome::TransientFailure(format!("SES throttling: {e}"))
                }
                SendEmailError::NotFoundException(e) => {
                    ProcessOutcome::TransientFailure(format!("SES resource not found: {e}"))
                }
                e => ProcessOutcome::TransientFailure(format!("Unexpected SES error: {e}")),
            })
        }
    }
}
