// SPDX-License-Identifier: CC0-1.0

//! How EventBridge REFUSES an `InvocationRateLimitPerSecond` past the maximum the
//! service enforces for an API destination, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter`.
//!
//! The RULE asserted here: `InvocationRateLimitPerSecond` is modelled `range min 1`
//! with NO maximum
//! (`smithy_query(query="operation",operation="CreateApiDestination")` /
//! `UpdateApiDestination`), so the ceiling is a service-side rule — and a request
//! past it is refused with the modelled `LimitExceededException` (HTTP 400) whose
//! message is the api-destination clause naming the CALL that failed followed by the
//! rate-limit clause naming the destination the request asked for, the rate it asked
//! for and the maximum in force, each quoted. Both the create and the update path
//! produce that one sentence, differing only in the verb of the call that failed; a
//! value AT the maximum is accepted by both, so the refusal is about passing the
//! ceiling rather than about reaching it.
//!
//! The table spans the class rather than the one recorded case: the rows differ along
//! the CALL being refused (create and update), along how far past the ceiling the rate
//! is (one past it, a multiple of it, the largest value the modelled `integer`
//! carries) and along the destination's name, which is what the sentence has to name
//! back. Every expected sentence is composed in the test from the request's own
//! values and the maximum, never imported from the implementation.
//!
//! The harness (the router with EventBridge alone, and the connection and destination
//! fixtures) is `tests/all/api_destination_update.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_destination_update::{
    emulator, error_code, events, given_connection, given_destination, message, update,
};

/// The modelled error a rate past the ceiling is refused with
/// (`CreateApiDestination`/`UpdateApiDestination` both list `LimitExceededException`,
/// `errorKind client`, `httpError 400`).
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The maximum the service enforces, stated HERE as the value the message has to
/// quote rather than read from the implementation.
const ENFORCED_MAXIMUM: i32 = 300;

/// The sentence a refused api-destination call must carry, composed from the verb of
/// the call that failed, the `Name` the request carried and its
/// `InvocationRateLimitPerSecond`.
fn expected_sentence(verb: &str, name: &str, rate: i32) -> String {
    format!(
        "Failed to {verb} the api-destination(s). Invalid Invocation Rate Limit: Rate \
         specified '{rate}' for '{name}' exceeds maximum allowed of '{ENFORCED_MAXIMUM}'."
    )
}

/// Which call a row refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Create,
    Update,
}

impl Call {
    /// The verb the refusal of this call opens with.
    fn verb(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
        }
    }
}

/// One row: the case, which call is made, the destination name the request carries
/// and the rate it asks for.
struct Row {
    case: &'static str,
    call: Call,
    name: &'static str,
    rate: i32,
}

/// Make `row`'s call against a destination that already exists, and answer its
/// status and body.
async fn refuse(router: &MultiServiceRouter, row: &Row, connection: &str) -> (u16, Value) {
    match row.call {
        Call::Create => {
            events(
                router,
                "CreateApiDestination",
                &json!({
                    "Name": row.name,
                    "ConnectionArn": connection,
                    "InvocationEndpoint": "https://api.example.com/events",
                    "HttpMethod": "POST",
                    "InvocationRateLimitPerSecond": row.rate,
                }),
            )
            .await
        }
        Call::Update => {
            update(
                router,
                row.name,
                &json!({ "InvocationRateLimitPerSecond": row.rate }),
            )
            .await
        }
    }
}

/// A rate past the enforced maximum is refused with AWS's api-destination sentence,
/// over a table of calls, rates and destination names.
#[tokio::test]
async fn a_rate_past_the_maximum_is_refused_naming_the_destination_the_rate_and_the_maximum() {
    let rows = [
        Row {
            case: "an update one past the maximum",
            call: Call::Update,
            name: "nudged-destination",
            rate: ENFORCED_MAXIMUM + 1,
        },
        Row {
            case: "an update far past the maximum",
            call: Call::Update,
            name: "racing-destination",
            rate: ENFORCED_MAXIMUM * 10,
        },
        Row {
            // The largest value the modelled `integer` carries, so nothing about the
            // sentence depends on the magnitude of the rate.
            case: "an update at the largest value the member's type holds",
            call: Call::Update,
            name: "unbounded-destination",
            rate: i32::MAX,
        },
        Row {
            case: "a create one past the maximum",
            call: Call::Create,
            name: "eager-destination",
            rate: ENFORCED_MAXIMUM + 1,
        },
        Row {
            case: "a create far past the maximum",
            call: Call::Create,
            name: "very-eager-destination",
            rate: ENFORCED_MAXIMUM * 7,
        },
        Row {
            // A different name, to pin that the name in the sentence is the
            // request's own rather than anything fixed.
            case: "a create under a name of its own",
            call: Call::Create,
            name: "a.name_with-every.admitted-char",
            rate: ENFORCED_MAXIMUM + 2,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = emulator();
        let connection = given_connection(&router, "rate-limit-connection", true).await;
        // The update rows need the destination to exist first; the create rows need
        // the name to be FREE, so each row runs on its own emulator and only the
        // update rows create it.
        if row.call == Call::Update {
            given_destination(&router, row.name, &connection).await;
        }

        let (status, body) = refuse(&router, row, &connection).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            LIMIT_EXCEEDED,
            "case {case:?}: expected {LIMIT_EXCEEDED}, got {body}"
        );
        let expected = expected_sentence(row.call.verb(), row.name, row.rate);
        let reported = message(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the message {expected:?}, got {reported:?}"
        );
    }
}

/// The ceiling is INCLUSIVE: a rate AT the enforced maximum is stored by both calls
/// and read back, so the refusal above is about passing the maximum rather than
/// reaching it.
#[tokio::test]
async fn a_rate_at_the_maximum_is_accepted_by_both_calls() {
    let case = "a create and an update at the enforced maximum";
    let router = emulator();
    let connection = given_connection(&router, "ceiling-connection", true).await;

    let (status, created) = events(
        &router,
        "CreateApiDestination",
        &json!({
            "Name": "ceiling-destination",
            "ConnectionArn": connection,
            "InvocationEndpoint": "https://api.example.com/events",
            "HttpMethod": "POST",
            "InvocationRateLimitPerSecond": ENFORCED_MAXIMUM,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a create at the maximum must be accepted, got {status} with {created}"
    );

    // Back down and up again through the update path, to the same ceiling.
    let (status, updated) = update(
        &router,
        "ceiling-destination",
        &json!({ "InvocationRateLimitPerSecond": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an update to the smallest admitted rate must be accepted, got \
         {status} with {updated}"
    );
    let (status, updated) = update(
        &router,
        "ceiling-destination",
        &json!({ "InvocationRateLimitPerSecond": ENFORCED_MAXIMUM }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an update at the maximum must be accepted, got {status} with {updated}"
    );

    let (status, described) = events(
        &router,
        "DescribeApiDestination",
        &json!({ "Name": "ceiling-destination" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApiDestination answered {status} with {described}"
    );
    assert_eq!(
        described["InvocationRateLimitPerSecond"],
        json!(ENFORCED_MAXIMUM),
        "case {case:?}: the destination must hold the rate it was updated to, got {described}"
    );
}

/// The maximum the emulator enforces IS the one the sentence quotes: a refusal at
/// one past the published ceiling names that same ceiling, so the message and the
/// rule cannot drift apart.
#[tokio::test]
async fn the_maximum_the_message_quotes_is_the_one_the_service_enforces() {
    let case = "the enforced maximum and the quoted maximum agree";
    assert_eq!(
        state::MAX_API_DESTINATION_RATE_LIMIT, ENFORCED_MAXIMUM,
        "case {case:?}: the service enforces {}, while the sentence this suite expects quotes \
         {ENFORCED_MAXIMUM}",
        state::MAX_API_DESTINATION_RATE_LIMIT
    );
}
