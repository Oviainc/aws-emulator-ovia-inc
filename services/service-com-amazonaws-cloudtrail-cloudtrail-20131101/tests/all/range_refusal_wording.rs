// SPDX-License-Identifier: CC0-1.0

//! What a boundary refusal of a value outside a modelled `@range` SAYS.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees: the HTTP status, the
//! error code and the `Message`.
//!
//! The rule under test: CloudTrail refuses a value outside a modelled `@range` with
//! `400 ValidationException` whose clause names the MEMBER PATH and the BOUND it broke
//! and quotes NOTHING of the rejected value — `1 validation error detected: Value at
//! '<member path>' failed to satisfy constraint: Member must have value {greater,less}
//! than or equal to <bound>` — the wording the recorded response to an
//! `UpdateEventDataStore` whose `RetentionPeriod` is above the modelled maximum carries.
//! It is a request-derived message, so the rule is the requirement rather than the one
//! recorded sentence; the rows below hold it over both bounds of two different range
//! members, on three operations, including one the recording never addressed.
//!
//! The control row pins the other half of the rule: a class this service has NO recorded
//! value-free wording for — a `@length` violation — keeps the framework's own sentence,
//! value and all. See `EmulatedService::override_refusal` in `src/service.rs`.

use serde_json::json;

use super::emulator::Emulator;

/// The code and status AWS answers an input-constraint violation with on the JSON
/// protocols.
const VALIDATION_EXCEPTION: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION`].
const VALIDATION_STATUS: u16 = 400;

/// The clause an out-of-range `member` whose broken bound is `bound` states, in the
/// envelope the framework renders around one violation.
fn clause(member: &str, bound: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have value {bound}"
    )
}

/// A value outside a modelled `@range` is refused naming the member and the bound,
/// without the value.
///
/// The ranges come from the model, not from this service's own code:
/// `smithy_query(query="operation", operation="UpdateEventDataStore")` gives
/// `RetentionPeriod` `@range{min:7,max:3653}`, and
/// `smithy_query(query="operation", operation="ListEventDataStores")` gives `MaxResults`
/// `@range{min:1,max:1000}`. Each member is exercised on BOTH published bounds, one past
/// it in each direction.
#[tokio::test]
async fn a_value_outside_a_modelled_range_is_refused_without_quoting_it() {
    let emulator = Emulator::new();

    // (case, operation, request, the clause the refusal must state)
    let rows = [
        (
            "retention period one below the minimum, on create",
            "CreateEventDataStore",
            json!({"Name": "range-low", "RetentionPeriod": 6}),
            clause("retentionPeriod", "greater than or equal to 7"),
        ),
        (
            "retention period one above the maximum, on create",
            "CreateEventDataStore",
            json!({"Name": "range-high", "RetentionPeriod": 3654}),
            clause("retentionPeriod", "less than or equal to 3653"),
        ),
        (
            "retention period far above the maximum, on update",
            "UpdateEventDataStore",
            json!({"EventDataStore": "no-such-store", "RetentionPeriod": 999_999}),
            clause("retentionPeriod", "less than or equal to 3653"),
        ),
        (
            "retention period one below the minimum, on update",
            "UpdateEventDataStore",
            json!({"EventDataStore": "no-such-store", "RetentionPeriod": 6}),
            clause("retentionPeriod", "greater than or equal to 7"),
        ),
        (
            "max results one below the minimum",
            "ListEventDataStores",
            json!({"MaxResults": 0}),
            clause("maxResults", "greater than or equal to 1"),
        ),
        (
            "max results one above the maximum",
            "ListEventDataStores",
            json!({"MaxResults": 1001}),
            clause("maxResults", "less than or equal to 1000"),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(case, VALIDATION_STATUS, VALIDATION_EXCEPTION);
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected the member and the bound without the value, got {:?}",
            reply.body
        );
    }

    // A value INSIDE the range is not refused by the boundary at all: the published
    // bounds themselves are accepted, so the rows above are about the values past them.
    for (case, retention) in [("the minimum itself", 7), ("the maximum itself", 3653)] {
        let reply = emulator
            .call(
                "CreateEventDataStore",
                &json!({"Name": format!("range-ok-{retention}"), "RetentionPeriod": retention}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected a published bound to be accepted, got {:?}",
            reply.body
        );
    }
}

/// A class this service has no recorded value-free wording for keeps the framework's own
/// sentence, which quotes the value — so the rewording above is the recorded classes and
/// nothing wider.
#[tokio::test]
async fn a_length_violation_keeps_the_frameworks_own_sentence() {
    let emulator = Emulator::new();
    // `Name` is `EventDataStoreName` @length 3..128 (model).
    let reply = emulator
        .call("CreateEventDataStore", &json!({"Name": "ab"}))
        .await;
    reply.expect_error(
        "a name below the modelled minimum length",
        VALIDATION_STATUS,
        VALIDATION_EXCEPTION,
    );
    assert_eq!(
        reply.message(),
        "1 validation error detected: Value 'ab' at 'name' failed to satisfy constraint: \
         Member must have length greater than or equal to 3",
        "case a name below the modelled minimum length: expected the framework's own \
         sentence, got {:?}",
        reply.body
    );
}
