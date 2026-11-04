// SPDX-License-Identifier: CC0-1.0

//! Trace retrievals: the records `StartTraceRetrieval` mints, the statuses the
//! clock owes them, the traces one retrieval holds, and the `RetrievalToken`
//! this service issues and checks.
//!
//! One concern, one file: every surface that starts, reads, lists or cancels a
//! retrieval answers from this module, so the token form, the status spans and
//! the refusal wording of a damaged token are stated once.

use super::{
    ComAmazonawsXrayAwsxrayState, TraceRecord, MALFORMED_RETRIEVAL_TOKEN_CAUSE,
    MAX_TRACE_RETRIEVALS, RETRIEVAL_STATUS_CANCELLED, RETRIEVAL_STATUS_COMPLETE,
    RETRIEVAL_STATUS_RUNNING, RETRIEVAL_STATUS_SCHEDULED, TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS,
    TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS, TRACE_RETRIEVAL_TIMEOUT_SECONDS,
    UNDECODABLE_EXECUTION_ID_CAUSE,
};

/// One started trace retrieval: what `StartTraceRetrieval` was asked for, under
/// the `RetrievalToken` it minted for it, plus the instants the clock owes its
/// `RetrievalStatus` transitions at.
#[derive(Debug, Clone, Default)]
pub struct TraceRetrievalRecord {
    /// The minted, opaque `RetrievalToken`.
    pub retrieval_token: String,
    /// The trace ids the retrieval was started for.
    pub trace_ids: Vec<String>,
    /// The requested window's start, in seconds of epoch time.
    pub start_time: f64,
    /// The requested window's end, in seconds of epoch time.
    pub end_time: f64,
    /// When the retrieval was started, in seconds of epoch time.
    pub started_at: f64,
    /// When the retrieval stops reporting `SCHEDULED` and starts reporting
    /// `RUNNING`, in seconds of epoch time.
    pub running_at: f64,
    /// When the retrieval's results become readable and it starts reporting
    /// `COMPLETE`, in seconds of epoch time.
    pub complete_at: f64,
    /// When `CancelTraceRetrieval` stopped this retrieval, in seconds of epoch
    /// time; `None` while it was never cancelled.
    pub cancelled_at: Option<f64>,
    /// When the retrieval times out, [`TRACE_RETRIEVAL_TIMEOUT_SECONDS`] after
    /// it started.
    pub expires_at: f64,
}

impl TraceRetrievalRecord {
    /// This retrieval's `RetrievalStatus` at `now`, computed from the instants
    /// stored on it — never from a status written at the time of some earlier
    /// call, so two readers at the same instant always agree.
    ///
    /// `FAILED` and `TIMEOUT` are never reported: no retrieval here can fail
    /// (the results are derived from traces already stored), and every one
    /// becomes readable within [`TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS`] of
    /// running — far inside the documented 60-minute timeout, so nothing ever
    /// runs out of it.
    pub fn status(&self, now: f64) -> &'static str {
        if self.cancelled_at.is_some() {
            RETRIEVAL_STATUS_CANCELLED
        } else if now >= self.complete_at {
            RETRIEVAL_STATUS_COMPLETE
        } else if now >= self.running_at {
            RETRIEVAL_STATUS_RUNNING
        } else {
            RETRIEVAL_STATUS_SCHEDULED
        }
    }

    /// Whether this retrieval is still the "ongoing trace retrieval job"
    /// `CancelTraceRetrieval` cancels: one that has not reached a status it can
    /// no longer leave.
    pub fn is_ongoing(&self, now: f64) -> bool {
        matches!(
            self.status(now),
            RETRIEVAL_STATUS_SCHEDULED | RETRIEVAL_STATUS_RUNNING
        )
    }
}

impl ComAmazonawsXrayAwsxrayState {
    /// Start a trace retrieval and return the stored record, whose
    /// `retrieval_token` is what the caller gets back.
    ///
    /// Bounded: retrievals the clock has already timed out are dropped first,
    /// and if the store is still at [`MAX_TRACE_RETRIEVALS`] the one that started
    /// longest ago makes room.
    pub fn start_trace_retrieval(
        &mut self,
        trace_ids: Vec<String>,
        start_time: f64,
        end_time: f64,
        now: f64,
    ) -> &TraceRetrievalRecord {
        self.expire_trace_retrievals(now);
        if self.retrievals.len() >= MAX_TRACE_RETRIEVALS {
            let oldest = self
                .retrievals
                .iter()
                .min_by(|(_, left), (_, right)| left.started_at.total_cmp(&right.started_at))
                .map(|(token, _)| token.clone());
            if let Some(token) = oldest {
                self.retrievals.remove(&token);
            }
        }
        self.retrieval_seq += 1;
        let retrieval_token = self.mint_retrieval_token(&trace_ids, start_time, end_time);
        let running_at = now + TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS;
        self.retrievals.insert(
            retrieval_token.clone(),
            TraceRetrievalRecord {
                retrieval_token: retrieval_token.clone(),
                trace_ids,
                start_time,
                end_time,
                started_at: now,
                running_at,
                complete_at: running_at + TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS,
                cancelled_at: None,
                expires_at: now + TRACE_RETRIEVAL_TIMEOUT_SECONDS,
            },
        );
        &self.retrievals[&retrieval_token]
    }

    /// The retrieval `retrieval_token` names, once the retrievals the clock has
    /// already timed out are gone.
    ///
    /// `None` is a token this service never issued, or one whose retrieval has
    /// timed out — which is what a reader answers `ResourceNotFoundException`
    /// for. The expiry it materialises was owed at `now` whether or not anybody
    /// called, so a reader may do it too.
    pub fn trace_retrieval(
        &mut self,
        retrieval_token: &str,
        now: f64,
    ) -> Option<&TraceRetrievalRecord> {
        self.expire_trace_retrievals(now);
        self.retrievals.get(retrieval_token)
    }

    /// Stop the retrieval a token names, so every later read of that token
    /// reports `RETRIEVAL_STATUS_CANCELLED`.
    ///
    /// IDEMPOTENT in the statuses a retrieval can no longer leave: a retrieval
    /// that already reached a terminal status is left exactly as it stands —
    /// cancelling asks for a state it is already in or past, so there is nothing
    /// to do and nothing to refuse. `Some` is the stored retrieval as it stands
    /// after the call; `None` is a token that names no retrieval at all, which is
    /// the one thing a caller is refused for.
    pub fn cancel_trace_retrieval(
        &mut self,
        retrieval_token: &str,
        now: f64,
    ) -> Option<&TraceRetrievalRecord> {
        self.expire_trace_retrievals(now);
        let retrieval = self.retrievals.get_mut(retrieval_token)?;
        if retrieval.is_ongoing(now) {
            retrieval.cancelled_at = Some(now);
        }
        Some(retrieval)
    }

    /// The traces one retrieval holds: every requested trace id that names a
    /// stored trace, each trimmed to the segments whose `start_time` falls
    /// inside the retrieval's window, in the order the retrieval asked for them.
    ///
    /// The window is INCLUSIVE at both ends — "The range is inclusive, so the
    /// specified start time is included in the query" /"the specified end time
    /// is included", `StartTraceRetrieval`'s own documentation for `StartTime`
    /// and `EndTime` — unlike the half-open interval `GetTraceSummaries`
    /// selects on.
    ///
    /// A requested id that names nothing stored, a repeat of one already
    /// answered, and a trace whose every segment started outside the window each
    /// contribute no trace: a retrieval reports the traffic that was recorded,
    /// never the ids that were asked about.
    pub fn retrieved_traces(&self, retrieval: &TraceRetrievalRecord) -> Vec<TraceRecord> {
        let mut answered: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut traces = Vec::new();
        for trace_id in &retrieval.trace_ids {
            if !answered.insert(trace_id.as_str()) {
                continue;
            }
            let Some(trace) = self.traces.get(trace_id) else {
                continue;
            };
            let trimmed = trace.within(retrieval.start_time, retrieval.end_time);
            if !trimmed.segments.is_empty() {
                traces.push(trimmed);
            }
        }
        traces
    }

    /// Drop every retrieval the clock has already timed out.
    ///
    /// This is work the clock owed at `now` whether or not anybody called, which
    /// is why a reader may materialise it too.
    pub fn expire_trace_retrievals(&mut self, now: f64) {
        self.retrievals
            .retain(|_, retrieval| retrieval.expires_at > now);
    }

    /// Mint an opaque `RetrievalToken`: [`RETRIEVAL_TOKEN_HEX_DIGITS`]
    /// hexadecimal digits, of which the first half identifies the retrieval and
    /// the second half is a CHECK over that first half
    /// ([`retrieval_token_check`]), so a token that was damaged in transit can be
    /// told from one this service never issued.
    ///
    /// The identifying half mixes the never-reset
    /// [`retrieval_seq`](ComAmazonawsXrayAwsxrayState::retrieval_seq) with what
    /// the retrieval asked for, so no two retrievals share a token even when they
    /// ask for the same traces and no clock enters the value; a mixing that
    /// happened to land on a token already held is re-mixed rather than returned,
    /// so the token a caller gets names its own retrieval and no other.
    fn mint_retrieval_token(&self, trace_ids: &[String], start_time: f64, end_time: f64) -> String {
        let mut token = String::new();
        for attempt in 0..=MAX_TRACE_RETRIEVALS {
            let identity = ::service_common::stable_hex_id(&[
                &self.retrieval_seq.to_string(),
                &attempt.to_string(),
                &trace_ids.join(","),
                &start_time.to_bits().to_string(),
                &end_time.to_bits().to_string(),
            ]);
            token = format!("{identity}{}", retrieval_token_check(&identity));
            if !self.retrievals.contains_key(&token) {
                break;
            }
        }
        token
    }
}

/// The CHECK half of a `RetrievalToken` whose identifying half is `identity`:
/// the same deterministic mixing every derived id of this crate uses, so the
/// check can be recomputed from the token alone
/// ([`retrieval_token_defect`]).
///
/// This makes the token integrity-checked, not unguessable: it tells a damaged
/// token from an intact one, and nothing authenticates on it.
pub fn retrieval_token_check(identity: &str) -> String {
    ::service_common::stable_hex_id(&["retrieval-token", identity])
}

/// How many hexadecimal digits a `RetrievalToken` this service mints carries
/// ([`ComAmazonawsXrayAwsxrayState::mint_retrieval_token`]).
pub const RETRIEVAL_TOKEN_HEX_DIGITS: usize = 32;

/// Why a submitted `RetrievalToken` is not a value this service ISSUED.
///
/// Both classes are a malformed parameter — `InvalidRequestException` at HTTP
/// 400, decided BEFORE any lookup — and they differ only in what the
/// execution-id decoder the value was handed can say about it
/// ([`malformed_retrieval_token_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalTokenDefect {
    /// The value is not of the form a `RetrievalToken` has at all: a width other
    /// than [`RETRIEVAL_TOKEN_HEX_DIGITS`], or a digit outside the hexadecimal
    /// alphabet. The decoder was handed something that is not an execution id,
    /// so it can say only that the execution id was invalid.
    NotTokenForm,
    /// The value has the minted width and the minted alphabet, but its check
    /// half does not derive from its identifying half
    /// ([`retrieval_token_check`]) — a token damaged after it was issued, by one
    /// character or by many, the case of a digit included. The decoder read an
    /// execution id of the right shape and could not decode it, so it names the
    /// value it could not decode.
    UndecodableExecutionId,
}

/// How `token` misses the `RetrievalToken` values this service mints, or `None`
/// when it is one this service could have ISSUED: the right number of
/// hexadecimal digits, and a check half that matches its identifying half
/// ([`retrieval_token_check`]).
///
/// The only retrieval tokens that exist are the ones `StartTraceRetrieval`
/// minted, and every one of them carries that check, so a value whose check does
/// not match could never have named a retrieval however much it looks like one:
/// it is a malformed parameter rather than a retrieval that is gone — which is
/// what a token damaged in transit, by one character or by many, is. That
/// distinction is what lets the retrieval readers answer such a token with
/// `InvalidRequestException` (HTTP 400) BEFORE any existence lookup, and reserve
/// `ResourceNotFoundException` (HTTP 404) for an intact token whose retrieval was
/// never started, has been evicted, or has timed out.
///
/// The check is recomputed over the EXACT characters the request submitted, and
/// nothing about them is folded: a retrieval token is an opaque string whose
/// characters are each significant, so a value that differs from an issued one in
/// the CASE of a digit is a value this service never issued — damaged in transit
/// exactly like one whose digit changed to another value — and it is refused as a
/// malformed parameter rather than resolved. Folding case here would also let a
/// value pass this gate and then miss the store that resolves it, which is keyed
/// by the exact string the mint emitted, so the caller would be told its
/// retrieval is gone when what is wrong is the value it submitted.
pub fn retrieval_token_defect(token: &str) -> Option<RetrievalTokenDefect> {
    if token.len() != RETRIEVAL_TOKEN_HEX_DIGITS
        || !token.chars().all(|digit| digit.is_ascii_hexdigit())
    {
        return Some(RetrievalTokenDefect::NotTokenForm);
    }
    let (identity, check) = token.split_at(RETRIEVAL_TOKEN_HEX_DIGITS / 2);
    if retrieval_token_check(identity) == check {
        None
    } else {
        Some(RetrievalTokenDefect::UndecodableExecutionId)
    }
}

/// What a `RetrievalToken` carrying `defect` ([`retrieval_token_defect`]) is
/// refused with, before any lookup: the name the operation gives the member, the
/// value the request submitted, and what the execution-id decoder made of it.
///
/// X-Ray words this refusal as TWO clauses: an outer one naming the parameter and
/// the value the request submitted — "Invalid <member> <value> passed: " — and
/// then the clause of the execution-id decoder the value was handed to. The
/// request's value is the sentence's ONLY variable; how often it appears is what
/// the decoder's clause decides:
///
/// * a value that is not of the token form at all got no further than the
///   decoder's own entry check, which states that the execution id was invalid
///   and names nothing ([`MALFORMED_RETRIEVAL_TOKEN_CAUSE`]) — so the value
///   appears once, in the outer clause; and
/// * a value of the token form whose check does not verify was DECODED and
///   rejected, and the decoder names the value it could not decode
///   ([`UNDECODABLE_EXECUTION_ID_CAUSE`]) — so the value appears twice, once per
///   clause.
///
/// The sentence is built here once and each reader passes the member name it uses
/// (`retrieval token`, `execution id`), so the two readers of one class never
/// disagree about its wording.
pub fn malformed_retrieval_token_message(
    member: &str,
    token: &str,
    defect: RetrievalTokenDefect,
) -> String {
    match defect {
        RetrievalTokenDefect::NotTokenForm => {
            format!("Invalid {member} {token} passed: {MALFORMED_RETRIEVAL_TOKEN_CAUSE}")
        }
        RetrievalTokenDefect::UndecodableExecutionId => {
            format!("Invalid {member} {token} passed: {UNDECODABLE_EXECUTION_ID_CAUSE}{token}")
        }
    }
}
