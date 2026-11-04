// SPDX-License-Identifier: CC0-1.0

//! @op StartLiveTail
//!
//! `POST / -> 200` (`X-Amz-Target: Logs_20140328.StartLiveTail`). Starts a Live Tail
//! session over one to ten log groups and answers with the modelled
//! `StartLiveTailResponseStream` as a `vnd.amazon.eventstream` body.
//!
//! This operation is hand-authored rather than generated: its output member
//! `responseStream` targets a `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim refuses the
//! request. The framing and the routing seam are `service_common::event_stream` and
//! `src/service.rs`; this file holds the behaviour, and the shapes are in
//! `src/ops/types/start_live_tail_types.rs`.
//!
//! # What a session answers
//!
//! The model documents the stream's shape, and each part of it is served from the log
//! groups the request names:
//!
//! * "A single LiveTailSessionStart object is sent at the start of the session" — one
//!   `sessionStart` event naming the session and echoing the selection it was started
//!   with.
//! * "Every second, a LiveTailSessionUpdate object is sent. Each of these objects
//!   contains an array of the actual log events [...] If no new log events were ingested
//!   in the past second, the LiveTailSessionUpdate object will contain an empty array."
//!   The session this emulator serves is one response rather than a connection held
//!   open, so it carries ONE such update, over the events the selected log groups hold
//!   that the session's selection admits — read out of the store, never synthesised —
//!   and an empty array when the selection admits none.
//! * "The array of log events contained in a LiveTailSessionUpdate can include as many
//!   as 500 log events. If the number of log events matching the request exceeds 500 per
//!   second, the log events are sampled down to 500 log events to be included in each
//!   LiveTailSessionUpdate object" — [`MAX_EVENTS_PER_UPDATE`], with
//!   `sessionMetadata.sampled` set exactly when the match exceeded it, as that member
//!   documents ("more than 500 log events matched the request for this update, and the
//!   sessionResults includes a sample of 500 of those events").
//!
//! One update is bounded by [`MAX_UPDATE_BYTES`] as well, so a request this small cannot
//! be answered with every byte the tailed groups hold; an update that left a matching
//! event out for either ceiling reports itself sampled.
//!
//! The two remaining members of the union, `SessionTimeoutException` and
//! `SessionStreamingException`, are in-band events of a session held open — the first
//! "when the session times out, after it has been kept open for three hours", the second
//! "if an unknown error occurs on the server side" — so neither belongs to a session
//! that is answered and closed in one response.
//!
//! Nothing here mutates the store: a session reports the log events a group has
//! ingested, so a tail leaves the groups it tailed exactly as a later reader finds them,
//! and the session itself is observable through no operation of this service.
//!
//! ## Input
//!
//! * `logGroupIdentifiers` (StartLiveTailLogGroupIdentifiers, len 1..10) **required** —
//!   each item a log group to include, by name or ARN. The modelled `LogGroupIdentifier`
//!   pattern (`^[\w#+=/:,.@-]*$`) is what refuses the asterisk-suffixed ARN form the
//!   member's documentation rules out ("If you specify an ARN, the ARN can't end with an
//!   asterisk (*)").
//! * `logStreamNames` (InputLogStreamNames, len 1..100) — only events of these streams
//!   are included. "If you specify this field, you can't also specify the
//!   logStreamNamePrefixes field", and "You can specify this parameter only if you
//!   specify only one log group in logGroupIdentifiers".
//! * `logStreamNamePrefixes` (InputLogStreamNames, len 1..100) — only events of the
//!   streams whose names start with one of these prefixes, under the same two rules.
//! * `logEventFilterPattern` (FilterPattern, len 0..1024) — "An optional pattern to use
//!   to filter the results to include only log events that match the pattern", in the
//!   service's own filter-and-pattern syntax
//!   ([`crate::state::filter_pattern::FilterPattern`]).
//!
//! ## Output
//!
//! * `responseStream` (StartLiveTailResponseStream, `@streaming`) — the body: the
//!   `sessionStart` event, then one `sessionUpdate`.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 400) — a `logGroupIdentifiers` item names a log
//!   group this account does not hold.
//! * `InvalidParameterException` (HTTP 400) — a parameter is specified incorrectly: the
//!   two stream selectors together, a stream selector across several log groups, a
//!   filter pattern that does not parse, or a value outside a modelled constraint.
//! * `AccessDeniedException`, `InvalidOperationException`, `LimitExceededException`
//!   (HTTP 400) — the operation's remaining modelled errors; no condition of this store
//!   raises them.

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::constraints::{validate, Violation, ViolationKind};
use aws_smithy_schema_server::DeserializableStruct;
use service_common::event_stream::{error_response, event_stream_only_response, frame_event};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it, and every one of them is a client error at HTTP 400.
#[smithy_macros::operation_error("com.amazonaws.cloudwatchlogs#StartLiveTail")]
#[derive(Debug)]
pub enum StartLiveTailErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidOperationException(InvalidOperationException),
    InvalidParameterException(InvalidParameterException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// The `StartLiveTailResponseStream` member each event is framed as, which the frame
/// carries in its `:event-type` header (source: `smithy_query(query="shape",
/// shape="StartLiveTailResponseStream")`).
const SESSION_START_EVENT: &str = "sessionStart";
/// See [`SESSION_START_EVENT`].
const SESSION_UPDATE_EVENT: &str = "sessionUpdate";

/// The most log events one `sessionUpdate` carries: "The array of log events contained
/// in a LiveTailSessionUpdate can include as many as 500 log events. If the number of
/// log events matching the request exceeds 500 per second, the log events are sampled
/// down to 500 log events to be included in each LiveTailSessionUpdate object"
/// (`StartLiveTail`). Past it the update carries a sample of this many and reports
/// itself sampled.
const MAX_EVENTS_PER_UPDATE: usize = 500;

/// The most bytes of log-event messages one `sessionUpdate` carries.
///
/// An EMULATOR bound rather than an AWS one, on the size of the answer a session
/// produces: one event's message may be a megabyte ([`crate::state::MAX_LOG_EVENT_BYTES`]),
/// so the event ceiling above alone would let a request this small be answered with
/// hundreds of megabytes. Accounted from the messages themselves, as this service's other
/// bounded read of stored events accounts its page, and an update that leaves a matching
/// event out for either ceiling reports itself sampled. The newest event is always
/// carried, so a single large event is delivered rather than dropped.
const MAX_UPDATE_BYTES: usize = 1_048_576;

/// The refusal for a request that names both log-stream selectors: "If you specify this
/// field, you can't also specify the logStreamNamePrefixes field"
/// (`StartLiveTailRequest$logStreamNames`). Worded like the service's other
/// exclusive-pair sentences ([`crate::state::LOG_GROUP_SELECTORS_EXCLUSIVE`]), naming the
/// two members the request spells.
const STREAM_SELECTORS_EXCLUSIVE: &str =
    "logStreamNames and logStreamNamePrefixes are mutually exclusive parameters.";

/// The refusal for a request that selects streams while tailing more than one log group:
/// "You can specify this parameter only if you specify only one log group in
/// logGroupIdentifiers" (`StartLiveTailRequest$logStreamNames` and
/// `$logStreamNamePrefixes`). Worded like the sentence above, naming the members the rule
/// holds between.
const STREAM_SELECTOR_NEEDS_ONE_LOG_GROUP: &str =
    "logStreamNames and logStreamNamePrefixes can be specified only with a single log \
     group in logGroupIdentifiers.";

/// The refusal CloudWatch Logs answers a Live Tail session that names a log group it does
/// not hold. Fixed text for the class: it states the resource the session could not be
/// started over and carries nothing of the request.
const LOG_GROUP_MISSING_MESSAGE: &str = "Provided LogGroup doesn't exist.";

/// One log event a session delivers, with the group identifier the request named its log
/// group by and the stream it was ingested into — the two positions the modelled
/// `LiveTailSessionLogEvent` reports beside the event itself.
struct Delivered<'store> {
    at: crate::state::EventKey,
    log_group_identifier: &'store str,
    log_stream_name: &'store str,
    event: &'store crate::state::StoredEvent,
}

/// The `StartLiveTail` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds one
/// of these and `service.rs` holds another wrapped in an `EventStreamRoute`; both reach
/// the same state, because the state lives in the shared engine, not on the operation.
pub struct StartLiveTail<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartLiveTail<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> StartLiveTail<Eng> {
    /// The whole request: resolve the log groups, select their events, and frame the
    /// session's `sessionStart` and `sessionUpdate` events.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, StartLiveTailErrorModeled> {
        let input = self.read_input(protocol, req)?;

        // "If you specify this field, you can't also specify the logStreamNamePrefixes
        // field", and either selector needs a single log group.
        if input.log_stream_names.is_some() && input.log_stream_name_prefixes.is_some() {
            return Err(invalid(STREAM_SELECTORS_EXCLUSIVE.to_string()));
        }
        let selects_streams =
            input.log_stream_names.is_some() || input.log_stream_name_prefixes.is_some();
        if selects_streams && input.log_group_identifiers.len() > 1 {
            return Err(invalid(STREAM_SELECTOR_NEEDS_ONE_LOG_GROUP.to_string()));
        }

        // The pattern is parsed before anything is looked up: a filter pattern that does
        // not parse is a bad request, not a session that delivers nothing.
        let pattern = crate::state::filter_pattern::FilterPattern::parse(
            input.log_event_filter_pattern.as_deref().unwrap_or(""),
        )
        .map_err(invalid)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartLiveTailErrorModeled::InvalidOperationException(InvalidOperationException {
                message: Some("the log store is unavailable".to_string()),
            })
        })?;
        let store = state.lock();

        // A session is started over the log groups it names, so every identifier has to
        // resolve before any of them is tailed: a session that could never deliver one
        // group's events is refused rather than silently narrowed to the others.
        let mut groups = Vec::with_capacity(input.log_group_identifiers.len());
        for identifier in &input.log_group_identifiers {
            let name = crate::state::log_group_name_from_identifier(identifier);
            let Some(group) = store.log_groups.get(name) else {
                return Err(StartLiveTailErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(LOG_GROUP_MISSING_MESSAGE.to_string()),
                    },
                ));
            };
            groups.push((identifier.as_str(), group));
        }

        // The events the session's selection admits, in the store's own event order (the
        // timestamp, then the ingestion sequence), so events of different streams and of
        // different groups interleave in one chronological run.
        let mut matched: Vec<Delivered<'_>> = Vec::new();
        for (log_group_identifier, group) in &groups {
            for (log_stream_name, stream) in &group.streams {
                if !covers_stream(
                    log_stream_name,
                    input.log_stream_names.as_deref(),
                    input.log_stream_name_prefixes.as_deref(),
                ) {
                    continue;
                }
                for event in &stream.events {
                    if pattern.match_message(&event.message).is_none() {
                        continue;
                    }
                    matched.push(Delivered {
                        at: event.key(),
                        log_group_identifier,
                        log_stream_name,
                        event,
                    });
                }
            }
        }
        matched.sort_by_key(|delivered| delivered.at);

        // Past either ceiling the update carries a sample — the most recent events, which
        // is what a tail of a stream is watching — and says so through `sampled`. The
        // newest event is kept whatever its size, so one large event is delivered rather
        // than dropped.
        let mut kept = 0usize;
        let mut bytes = 0usize;
        for candidate in matched.iter().rev() {
            if kept >= MAX_EVENTS_PER_UPDATE {
                break;
            }
            let cost = candidate.event.message.len();
            if kept > 0 && bytes.saturating_add(cost) > MAX_UPDATE_BYTES {
                break;
            }
            bytes = bytes.saturating_add(cost);
            kept += 1;
        }
        let sampled = kept < matched.len();
        let delivered = &matched[matched.len().saturating_sub(kept)..];

        let session_start = LiveTailSessionStart {
            // Both ids are minted per session: nothing in the request carries either,
            // and a session is named only in the response that starts it.
            request_id: Some(::service_common::query_error_envelope::new_request_id()),
            session_id: Some(::service_common::query_error_envelope::new_request_id()),
            log_group_identifiers: Some(input.log_group_identifiers.clone()),
            log_stream_names: input.log_stream_names.clone(),
            log_stream_name_prefixes: input.log_stream_name_prefixes.clone(),
            log_event_filter_pattern: input.log_event_filter_pattern.clone(),
        };
        let session_update = LiveTailSessionUpdate {
            session_metadata: Some(LiveTailSessionMetadata { sampled }),
            session_results: Some(
                delivered
                    .iter()
                    .map(|delivered| LiveTailSessionLogEvent {
                        log_stream_name: Some(delivered.log_stream_name.to_string()),
                        log_group_identifier: Some(delivered.log_group_identifier.to_string()),
                        message: Some(delivered.event.message.clone()),
                        timestamp: Some(delivered.event.timestamp),
                        ingestion_time: Some(delivered.event.ingestion_time),
                    })
                    .collect(),
            ),
        };
        drop(store);

        let mut frames = frame_event(
            protocol,
            SESSION_START_EVENT,
            false,
            &session_start,
            &LiveTailSessionStart::SCHEMA,
        )
        .map_err(|error| internal(format!("the sessionStart event could not be framed: {error}")))?;
        frames.extend(
            frame_event(
                protocol,
                SESSION_UPDATE_EVENT,
                false,
                &session_update,
                &LiveTailSessionUpdate::SCHEMA,
            )
            .map_err(|error| {
                internal(format!("the sessionUpdate event could not be framed: {error}"))
            })?,
        );
        Ok(event_stream_only_response(frames))
    }

    /// Read the request into the modelled input shape and check it against the model.
    ///
    /// Driven through the service's own protocol, so the body codec is the protocol's;
    /// the refusal is worded by [`crate::service::constraint_envelope`] rather than by
    /// the framework's default envelope, so a bad request to this hand-authored
    /// operation reads exactly as the framework's boundary words the same violation for
    /// this service's generated operations.
    fn read_input(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<StartLiveTailInput, StartLiveTailErrorModeled> {
        let mut deserializer = protocol
            .deserialize_input(req, &StartLiveTailInput::SCHEMA)
            .map_err(|error| invalid(format!("the request could not be read: {error}")))?;
        let input = StartLiveTailInput::deserialize(&mut *deserializer)
            .map_err(|error| invalid(serde_refusal(error)));
        drop(deserializer);
        let input = input?;
        let violations = validate(&input, &StartLiveTailInput::SCHEMA);
        match refusal_message(&violations) {
            Some(message) => Err(invalid(message)),
            None => Ok(input),
        }
    }
}

/// Whether one of the group's streams is in the session's selection: the named streams
/// when the request named any, the streams whose name starts with one of the prefixes
/// when it gave prefixes, and every stream of the group when it gave neither. The two
/// selectors are refused together before this is reached.
fn covers_stream(
    log_stream_name: &str,
    names: Option<&[String]>,
    prefixes: Option<&[String]>,
) -> bool {
    if let Some(names) = names {
        return names.iter().any(|name| name == log_stream_name);
    }
    if let Some(prefixes) = prefixes {
        return prefixes
            .iter()
            .any(|prefix| log_stream_name.starts_with(prefix.as_str()));
    }
    true
}

/// This service's wording for the violations a request failed, falling back to the
/// framework's envelope for a class the service words no clause for. `None` when the
/// request broke nothing.
fn refusal_message(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    crate::service::constraint_envelope(violations).or_else(|| {
        aws_smithy_schema_server::constraints::envelope(violations)
    })
}

/// A member the request did not carry, worded as the service words an absent
/// `@required` member — the same clause the framework's own boundary renders for one.
fn serde_refusal(error: ::aws_smithy_schema::serde::SerdeError) -> String {
    match error {
        ::aws_smithy_schema::serde::SerdeError::MissingMember { member_name } => {
            let violation = Violation {
                path: aws_smithy_schema_server::constraints::path::member(&member_name),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            };
            refusal_message(&[violation]).unwrap_or_else(|| member_name.clone())
        }
        other => other.to_string(),
    }
}

/// "A parameter is specified incorrectly" — this operation's refusal for a request it
/// will not accept.
fn invalid(message: String) -> StartLiveTailErrorModeled {
    StartLiveTailErrorModeled::InvalidParameterException(InvalidParameterException {
        message: Some(message),
    })
}

/// A failure of this emulator rather than of the request. `InvalidOperationException` is
/// the only modelled error of this operation that is not about a parameter the caller
/// chose, and every modelled error here is a client error at 400: the operation declares
/// no server fault to answer with.
fn internal(message: String) -> StartLiveTailErrorModeled {
    StartLiveTailErrorModeled::InvalidOperationException(InvalidOperationException {
        message: Some(message),
    })
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for StartLiveTail<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused session is diagnosable. No
            // request payload and no credential reaches it.
            tracing::debug!(
                operation = "StartLiveTail",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}
