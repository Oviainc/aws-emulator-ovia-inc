// SPDX-License-Identifier: CC0-1.0

//! What follows ONE delivery attempt of an accepted asynchronous invocation: whether the
//! event is handed to the function again, and how long after.
//!
//! "When an error occurs, your function may be invoked multiple times. Retry behavior varies
//! by error type, client, event source, and invocation type. For example, if you invoke a
//! function asynchronously and it returns an error, Lambda executes the function up to two
//! more times" (`Invoke`). So a delivery attempt whose handler ERRORED is not the end of the
//! event: Lambda makes further attempts, up to [`MAX_ASYNC_DELIVERY_ATTEMPTS`] in all, and
//! only then does the entry keep what the last of them came to
//! ([`super::AsyncInvocationOutcome`]).
//!
//! What does NOT get another attempt is an event the function could not be RUN for at all —
//! a package this emulator embeds no engine for, one it cannot read, a configuration naming
//! no handler. The retry above is for a function that ran and errored; a run that never
//! started fails the same way however often it is attempted, so the entry records that
//! instead of spending attempts on it.
//!
//! This module is the policy and nothing else: the attempt itself, and the entry the outcome
//! lands on, are [`super::async_invocation`]'s.

use std::time::Duration;

/// How many delivery attempts one accepted asynchronous event has.
///
/// "If you invoke a function asynchronously and it returns an error, Lambda executes the
/// function up to two more times" (`Invoke`): the first attempt plus those two.
pub const MAX_ASYNC_DELIVERY_ATTEMPTS: u32 = 3;

/// How long after a delivery attempt whose handler errored the next attempt is made.
///
/// An EMULATOR-CHOSEN span, inside the range this crate chooses for every span of its own
/// ([`super::ACTIVATION_DELAY`]). AWS publishes a minute-scale wait between the attempts it
/// makes on an asynchronous invocation; a wait of that order would outlast any caller's
/// observation of its own event, so what is kept faithful is the part a client can observe —
/// how many attempts the event has ([`MAX_ASYNC_DELIVERY_ATTEMPTS`]), and that they are
/// separated in time rather than run back to back.
pub const ASYNC_RETRY_DELAY: Duration = Duration::from_secs(2);

/// How long until the next delivery attempt of an event whose attempts so far number
/// `attempts_made`, or `None` when the event has no further attempt.
///
/// `function_error` is the `FunctionError` the attempt's run reported — "If present,
/// indicates that an error occurred during function execution" — and `None` for a handler
/// that returned, which is an event Lambda is done with. A run that never started is not an
/// errored handler and reaches here as `ran: false`: no attempt can change what stopped it,
/// so the event has no further attempt either.
#[must_use]
pub fn next_attempt_in(
    ran: bool,
    function_error: Option<&str>,
    attempts_made: u32,
) -> Option<Duration> {
    let errored = ran && function_error.is_some();
    (errored && attempts_made < MAX_ASYNC_DELIVERY_ATTEMPTS).then_some(ASYNC_RETRY_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below: the case, whether the run happened, the `FunctionError`
    /// it reported, how many attempts the event has had, and when the next one is due.
    type Row = (&'static str, bool, Option<&'static str>, u32, Option<Duration>);

    /// One row per combination the policy decides over: whether the run happened at all,
    /// whether its handler errored, and how many attempts the event has already had —
    /// including at the published bound and one past it.
    #[test]
    fn only_an_errored_handler_with_an_attempt_left_is_delivered_again() {
        let rows: &[Row] = &[
            ("the handler returned on its first attempt", true, None, 1, None),
            (
                "the handler errored on its first attempt",
                true,
                Some("Unhandled"),
                1,
                Some(ASYNC_RETRY_DELAY),
            ),
            (
                "the handler errored on its second attempt",
                true,
                Some("Unhandled"),
                2,
                Some(ASYNC_RETRY_DELAY),
            ),
            (
                "the handler errored on the last attempt the event has",
                true,
                Some("Unhandled"),
                MAX_ASYNC_DELIVERY_ATTEMPTS,
                None,
            ),
            (
                "the handler errored past the attempts the event has",
                true,
                Some("Unhandled"),
                MAX_ASYNC_DELIVERY_ATTEMPTS + 1,
                None,
            ),
            (
                "the handler errored with another function error",
                true,
                Some("Unhandled.OutOfMemory"),
                1,
                Some(ASYNC_RETRY_DELAY),
            ),
            ("the run never started", false, None, 1, None),
            (
                "the run never started, with an error reported for it",
                false,
                Some("Unhandled"),
                1,
                None,
            ),
            (
                "the handler returned on its last attempt",
                true,
                None,
                MAX_ASYNC_DELIVERY_ATTEMPTS,
                None,
            ),
        ];
        for (case, ran, function_error, attempts_made, expected) in rows {
            let answered = next_attempt_in(*ran, *function_error, *attempts_made);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected the next attempt in {expected:?}, got {answered:?}"
            );
        }
    }
}
