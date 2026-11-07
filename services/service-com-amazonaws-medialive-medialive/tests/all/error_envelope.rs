// SPDX-License-Identifier: CC0-1.0

//! MediaLive's error envelope, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted is
//! the emulator's wire behaviour, not an internal function's return value.
//!
//! Rule under test: every failure this service answers carries, beside the modelled
//! `message`, the members that identify the RESPONSE — the error code (`code`), the
//! HTTP status (`statusCode`), whether the call may be retried (`retryable`), the
//! request id (`requestId`, also reported in the `x-amzn-requestid` header) and the
//! instant the response was produced (`time`) — and every response, success or
//! failure, reports a request id of its own.
//!
//! The suites are tables: the 409 in the issue is ONE row.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::service::error_envelope::MODELLED_EXCEPTIONS;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::GROUPS;
use crate::wire::{
    call_with_headers, error_code, error_message, header, medialive, DEFAULT_REGION as REGION,
};

/// The header AWS reports a response's request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The envelope members a MediaLive error body carries beside `message`.
const ENVELOPE_MEMBERS: [&str; 5] = ["code", "statusCode", "retryable", "requestId", "time"];

/// One answer the emulator gave: what was asked, and everything a client can read
/// back.
struct Answer {
    case: &'static str,
    status: u16,
    body: Value,
    headers: Vec<(String, String)>,
}

/// Dispatch one request signed for [`REGION`] and keep its whole answer.
async fn answer(
    router: &MultiServiceRouter,
    case: &'static str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let (status, body, headers) = call_with_headers(router, REGION, method, uri, body).await;
    Answer {
        case,
        status,
        body,
        headers,
    }
}

/// A wire timestamp read as seconds since the Unix epoch, whichever ISO-8601
/// rendering it arrived in.
fn epoch_seconds(case: &str, text: &str) -> f64 {
    let stamp = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: time {text:?} is not an ISO-8601 date-time: {error}")
    });
    stamp.secs() as f64 + f64::from(stamp.subsec_nanos()) / 1e9
}

/// The same reading of a [`SystemTime`] the test took itself.
fn unix_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs_f64()
}

/// Assert the whole rule on one failure: the status, the code, the retry
/// classification, the correlation handle and the response instant.
fn assert_envelope(
    observed: &Answer,
    expected_status: u16,
    expected_code: &str,
    expected_retryable: bool,
    window: (f64, f64),
) {
    let case = observed.case;
    let body = &observed.body;
    assert_eq!(
        observed.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {}: {body}",
        observed.status
    );
    assert_eq!(
        error_code(observed.status, body),
        expected_code,
        "case {case:?}: expected the modelled discriminator {expected_code:?}, got {body}"
    );
    assert_eq!(
        body["code"],
        json!(expected_code),
        "case {case:?}: expected code {expected_code:?}, got {:?}",
        body["code"]
    );
    assert_eq!(
        body["statusCode"],
        json!(expected_status),
        "case {case:?}: expected statusCode {expected_status}, got {:?}",
        body["statusCode"]
    );
    assert_eq!(
        body["retryable"],
        json!(expected_retryable),
        "case {case:?}: expected retryable {expected_retryable}, got {:?}",
        body["retryable"]
    );
    assert!(
        !error_message(body).is_empty(),
        "case {case:?}: expected the failure to state a message, got {body}"
    );

    // The request id is the caller's correlation handle, so the body and the header
    // must name ONE request.
    let reported = body["requestId"].as_str().unwrap_or_default().to_string();
    assert!(
        !reported.is_empty(),
        "case {case:?}: expected a requestId, got {body}"
    );
    assert_eq!(
        header(&observed.headers, REQUEST_ID_HEADER).as_deref(),
        Some(reported.as_str()),
        "case {case:?}: expected the {REQUEST_ID_HEADER} header to repeat the body's requestId, got {:?}",
        header(&observed.headers, REQUEST_ID_HEADER)
    );

    // `time` is the instant THIS response was produced: it falls in the window the
    // suite ran in, not at a fixed instant.
    let text = body["time"].as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected time as an ISO-8601 string, got {:?}",
            body["time"]
        )
    });
    let stamped = epoch_seconds(case, text);
    let (opened, closed) = window;
    assert!(
        (opened..=closed).contains(&stamped),
        "case {case:?}: expected time within the suite's window [{opened}, {closed}], got {stamped} ({text})"
    );
}

/// Rule: every failure MediaLive answers carries the response envelope beside its
/// modelled message.
///
/// The table spans what the rule generalises over: the modelled exception and its
/// status (409, 404, 400, 429), the retry classification each one implies (a
/// throttle may be retried, the other client failures may not), the operation that
/// refused (a create, two reads, a delete, a list) and the resource family it
/// belongs to (the monitor API and the classic channel API). The issue's
/// recorded case is the first row.
///
/// The 404 rows are families whose refusal STATES a message; that the envelope is
/// complete on a 404 that states none — the cloudwatch alarm-template hierarchy's
/// — is pinned by `crate::not_found_refusal_wording`.
#[tokio::test]
async fn every_failure_carries_the_response_envelope() {
    let opened = unix_seconds(SystemTime::now()) - 1.0;
    let router = medialive();

    // A group to conflict with, and a channel to refuse a delete.
    let taken = "envelope-group";
    let created = answer(
        &router,
        "fixture create",
        "POST",
        GROUPS,
        Some(json!({ "name": taken })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the fixture group create got {}: {}",
        created.status, created.body
    );
    let channel = answer(
        &router,
        "fixture channel",
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "envelope-channel",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(
        channel.status, 201,
        "the fixture channel create got {}: {}",
        channel.status, channel.body
    );
    // Freshly created, so still CREATING — a state a delete is refused in.
    let channel_id = channel.body["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, expected status, expected code, retryable, the answer)
    let mut rows: Vec<(u16, &str, bool, Answer)> = vec![
        (
            409,
            "ConflictException",
            false,
            answer(
                &router,
                "create reusing a taken template-group name",
                "POST",
                GROUPS,
                Some(json!({ "name": taken })),
            )
            .await,
        ),
        (
            404,
            "NotFoundException",
            false,
            answer(
                &router,
                "read of a signal map no identifier answers to",
                "GET",
                "/prod/signal-maps/absent-map",
                None,
            )
            .await,
        ),
        (
            404,
            "NotFoundException",
            false,
            answer(
                &router,
                "read of a channel no id answers to",
                "GET",
                "/prod/channels/9999999",
                None,
            )
            .await,
        ),
        (
            409,
            "ConflictException",
            false,
            answer(
                &router,
                "delete of a channel that is not idle",
                "DELETE",
                &format!("/prod/channels/{channel_id}"),
                None,
            )
            .await,
        ),
        (
            400,
            "BadRequestException",
            false,
            answer(
                &router,
                "list asking for a page of no channels",
                "GET",
                "/prod/channels?maxResults=0",
                None,
            )
            .await,
        ),
    ];

    // A throttle is the one failure the rule classifies as retryable, so the table
    // must carry one: the group store is bounded, and the create past its bound is
    // refused with the operation's modelled throttle.
    {
        use service_com_amazonaws_medialive_medialive::state::MAX_ALARM_TEMPLATE_GROUPS;
        let router = medialive();
        for index in 0..MAX_ALARM_TEMPLATE_GROUPS {
            let (status, body, _) = call_with_headers(
                &router,
                REGION,
                "POST",
                GROUPS,
                Some(json!({ "name": format!("g-{index}") })),
            )
            .await;
            assert_eq!(status, 201, "filling the store at {index} got {status}: {body}");
        }
        rows.push((
            429,
            "TooManyRequestsException",
            true,
            answer(
                &router,
                "create past the bound of the group store",
                "POST",
                GROUPS,
                Some(json!({ "name": "one-too-many" })),
            )
            .await,
        ));
    }

    let closed = unix_seconds(SystemTime::now()) + 1.0;
    let mut ids: Vec<String> = Vec::new();
    for (status, code, retryable, observed) in &rows {
        assert_envelope(observed, *status, code, *retryable, (opened, closed));
        ids.push(observed.body["requestId"].as_str().unwrap_or_default().to_string());
    }

    // A request id names ONE call, so no two of these failures share one.
    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "expected one request id per response, got {ids:?}"
    );
}

/// Rule: a successful response reports a request id of its own and carries only the
/// operation's modelled output — the envelope belongs to a failure.
///
/// The table spans a create and a read on both resource families, and a list, so the
/// claim is about every success rather than one operation's.
#[tokio::test]
async fn a_success_reports_a_request_id_and_no_envelope_members() {
    let router = medialive();

    let created = answer(
        &router,
        "create a template group",
        "POST",
        GROUPS,
        Some(json!({ "name": "success-group" })),
    )
    .await;
    let channel = answer(
        &router,
        "create a channel",
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "success-channel",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    let rows = vec![
        (201, created),
        (201, channel),
        (
            200,
            answer(
                &router,
                "read the template group",
                "GET",
                &format!("{GROUPS}/success-group"),
                None,
            )
            .await,
        ),
        (
            200,
            answer(&router, "list the channels", "GET", "/prod/channels", None).await,
        ),
    ];

    let mut ids: Vec<String> = Vec::new();
    for (expected_status, observed) in &rows {
        let case = observed.case;
        assert_eq!(
            observed.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {}: {}",
            observed.status, observed.body
        );
        for member in ENVELOPE_MEMBERS {
            assert!(
                observed.body.get(member).is_none(),
                "case {case:?}: a success carries no {member}, got {}",
                observed.body
            );
        }
        let reported = header(&observed.headers, REQUEST_ID_HEADER).unwrap_or_default();
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected a {REQUEST_ID_HEADER} header, got {:?}",
            observed.headers
        );
        ids.push(reported);
    }

    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "expected one request id per response, got {ids:?}"
    );
}

/// Rule: the envelope accompanies one of this service's MODELLED exceptions, so a
/// failure answered under a code this service's model does not declare keeps the
/// framework's own body and is not dressed up as a MediaLive exception.
///
/// Reached without depending on which operations are implemented: a URI this service
/// claims but the model binds no operation to is refused by the framework's own
/// routing-failure renderer, under a code that is not in the model's error list. The
/// gate itself is what is asserted — the envelope is there exactly when the code the
/// response names is one the model declares.
#[tokio::test]
async fn a_code_this_service_does_not_model_keeps_its_own_body() {
    let router = medialive();
    let observed = answer(
        &router,
        "a uri the model binds no operation to",
        "GET",
        "/prod/no-such-medialive-resource",
        None,
    )
    .await;
    let case = observed.case;
    assert!(
        observed.status >= 400,
        "case {case:?}: expected the unroutable request to be refused, got {}: {}",
        observed.status,
        observed.body
    );

    let code = error_code(observed.status, &observed.body);
    assert!(
        !MODELLED_EXCEPTIONS.contains(&code.as_str()),
        "case {case:?}: expected a code outside the model's error list, got {code:?}: {}",
        observed.body
    );
    for member in ENVELOPE_MEMBERS {
        assert!(
            observed.body.get(member).is_none(),
            "case {case:?}: the code {code:?} is not one this service models, so its body carries no {member}, got {}",
            observed.body
        );
    }
}

/// Rule: `time` is the instant of the response that carries it, so two failures a
/// measurable span apart report instants that far apart — it is not a constant and
/// not the moment the resource was created.
#[tokio::test]
async fn the_response_time_moves_with_the_clock() {
    let router = medialive();
    let (status, body, _) = call_with_headers(
        &router,
        REGION,
        "POST",
        GROUPS,
        Some(json!({ "name": "clock-group" })),
    )
    .await;
    assert_eq!(status, 201, "the fixture create got {status}: {body}");

    let conflict = |case: &'static str| {
        answer(
            &router,
            case,
            "POST",
            GROUPS,
            Some(json!({ "name": "clock-group" })),
        )
    };

    let first = conflict("first conflict").await;
    // The `date-time` rendering carries sub-second precision, so a gap this size is
    // measurable in the reported instants; the wait is asynchronous, so the runtime
    // thread is not blocked while the clock moves.
    let gap = Duration::from_millis(300);
    tokio::time::sleep(gap).await;
    let second = conflict("second conflict").await;

    let before = epoch_seconds(first.case, first.body["time"].as_str().unwrap_or_default());
    let after = epoch_seconds(second.case, second.body["time"].as_str().unwrap_or_default());
    let moved = after - before;
    assert!(
        moved >= gap.as_secs_f64() / 2.0,
        "expected the second failure's time to be at least {:?} later, got {before} then {after}",
        gap / 2
    );
}
