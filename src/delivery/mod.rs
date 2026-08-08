pub mod lda;
pub mod ses;
pub mod smtp;

/// Outcome of attempting to process one email message end-to-end.
#[derive(Debug)]
pub enum ProcessOutcome {
    Success,
    /// Worth retrying (e.g. transient network/service errors).
    TransientFailure(String),
    /// Retrying would not help (e.g. malformed request, rejected message).
    PermanentFailure(String),
    /// The raw email could not be parsed. The S3 object is left in place for
    /// inspection; the SQS message is moved directly to the DLQ.
    ParsingFailure(String),
}
