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
//! value and all. A page-size member is the one range member that does NOT keep the
//! generic CODE: its operation's model declares `InvalidMaxResultsException` for a limit
//! that is not valid, so that is the code it carries — the same value-free clause, naming
//! the member and the bound. See `EmulatedService::override_refusal` in `src/service.rs`.

use serde_json::json;

use super::emulator::Emulator;

/// The code and status AWS answers an input-constraint violation with on the JSON
/// protocols.
const VALIDATION_EXCEPTION: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION`].
const VALIDATION_STATUS: u16 = 400;

/// The code a paged operation whose model declares it answers a limit outside its
/// modelled range with, in place of [`VALIDATION_EXCEPTION`].
const INVALID_MAX_RESULTS_EXCEPTION: &str = "InvalidMaxResultsException";

/// The clause an out-of-range `member` whose broken bound is `bound` states, in the
/// envelope the framework renders around one violation.
fn clause(member: &str, bound: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have value {bound}"
    )
}

/// The same clause for a PAGE-SIZE member, which the modelled
/// [`INVALID_MAX_RESULTS_EXCEPTION`] words as the limit it is: the member and the bound
/// it broke, and none of the rejected value.
fn limit_clause(member: &str, bound: &str) -> String {
    format!("The limit at '{member}' is not valid: it must have value {bound}.")
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

/// A page-size member outside its modelled `@range` keeps the value-free clause but
/// carries the code its OPERATION models for a limit that is not valid.
///
/// `InvalidMaxResultsException` is "thrown if the limit specified is not valid"
/// (`smithy_query(query="shape", shape="InvalidMaxResultsException")`), and every
/// operation below declares it while declaring no `ValidationException`
/// (`smithy_query(query="operation",
/// operation="ListEventDataStores,ListQueries,LookupEvents,GetQueryResults")`) — so a
/// client catching the modelled error would miss a generic refusal altogether. The rows
/// differ along the member's SPELLING (`MaxResults` / `MaxQueryResults`), the operation
/// and which published bound is broken; each bound comes from the model, not from this
/// service's code.
#[tokio::test]
async fn a_page_size_outside_its_range_carries_the_code_its_operation_models() {
    let emulator = Emulator::new();

    // (case, operation, request, the clause the refusal must state)
    let rows = [
        (
            "max results one below the minimum",
            "ListEventDataStores",
            json!({"MaxResults": 0}),
            limit_clause("maxResults", "greater than or equal to 1"),
        ),
        (
            "max results one above the maximum",
            "ListEventDataStores",
            json!({"MaxResults": 1001}),
            limit_clause("maxResults", "less than or equal to 1000"),
        ),
        (
            "a lookup's max results above its own, narrower maximum",
            "LookupEvents",
            json!({"MaxResults": 51}),
            limit_clause("maxResults", "less than or equal to 50"),
        ),
        (
            "a query listing's max results below the minimum",
            "ListQueries",
            json!({"EventDataStore": "00000000-0000-4000-8000-000000000000", "MaxResults": 0}),
            limit_clause("maxResults", "greater than or equal to 1"),
        ),
        (
            "the other spelling of the member, above its maximum",
            "GetQueryResults",
            json!({"QueryId": "00000000-0000-4000-8000-000000000000", "MaxQueryResults": 1001}),
            limit_clause("maxQueryResults", "less than or equal to 1000"),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(case, VALIDATION_STATUS, INVALID_MAX_RESULTS_EXCEPTION);
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected the member and the bound without the value, got {:?}",
            reply.body
        );
    }

    // The published bounds themselves are served, so the rows above are about the
    // values past them.
    for (case, max_results) in [("the lowest page size", 1), ("the largest", 1000)] {
        let reply = emulator
            .call("ListEventDataStores", &json!({"MaxResults": max_results}))
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
