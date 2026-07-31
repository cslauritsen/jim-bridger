pub mod lda;
pub mod ses;

/// Outcome of attempting to process one email message end-to-end.
#[derive(Debug)]
pub enum ProcessOutcome {
    Success,
    /// Worth retrying (e.g. transient network/service errors).
    TransientFailure(String),
    /// Retrying would not help (e.g. malformed request, rejected message).
    PermanentFailure(String),
}
