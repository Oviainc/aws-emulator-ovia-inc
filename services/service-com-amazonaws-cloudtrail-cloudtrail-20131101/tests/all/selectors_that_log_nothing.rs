// SPDX-License-Identifier: CC0-1.0

//! A basic event selector that would log NOTHING is refused, and the trail keeps what it
//! had.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees: the HTTP status, the error
//! code and the message.
//!
//! The rule under test: a basic event selector logs management events, data events for the
//! resources it names, or both — so one that EXCLUDES management events and names no data
//! resource selects no event at all, and `PutEventSelectors` refuses it with
//! `InvalidEventSelectorsException` (HTTP 400), whose own documentation names this case
//! ("the combination of event selectors or advanced event selectors and data resources is
//! not valid"). The refusal writes nothing, so the selector set the trail already carried
//! is the set every later read reports.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    BASIC_DATA_RESOURCE_TYPES, EVENT_SELECTORS_PER_TRAIL_LIMIT,
};

use crate::emulator::Emulator;

/// The status CloudTrail answers its client errors with.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error the model declares for a selector combination the service will not accept
/// (`smithy_query(query="operation", operation="PutEventSelectors")`).
const INVALID_EVENT_SELECTORS: &str = "InvalidEventSelectorsException";

/// How CloudTrail words this refusal: fixed text for the error class, carrying no part of
/// the request.
const NOTHING_LOGGED_MESSAGE: &str = "Invalid combination of parameters in EventSelectors. \
     When includeManagementEvents is false, DataResources must not be empty";

/// One basic data resource of the first type the model's catalog for `DataResource$Type`
/// carries.
fn one_data_resource() -> Value {
    json!({
        "Type": BASIC_DATA_RESOURCE_TYPES[0],
        "Values": ["arn:aws:dynamodb:us-east-1:000000000000:table/orders"],
    })
}

/// The selectors `GetEventSelectors` / `PutEventSelectors` report for a trail.
fn reported(body: &Value) -> Value {
    body.get("EventSelectors").cloned().unwrap_or(Value::Null)
}

/// RULE: a selector that logs nothing is refused, with the fixed sentence for that error
/// class, and the trail's previously applied selector set is left untouched.
///
/// The rows differ along the SHAPE of the nothing-logging selector — the member absent
/// against an empty list, the `ReadWriteType` it carries, and where in a longer selector
/// list the offending entry sits — and are checked against rows carrying the SAME
/// `IncludeManagementEvents` with a data resource, which log something and are served.
#[tokio::test]
async fn a_selector_that_would_log_nothing_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let trail = "logs-nothing";
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": "my-bucket"}),
        )
        .await
        .expect_ok("CreateTrail");

    // The baseline the refusals must not disturb: a non-default set, applied through the
    // API so what is read back is what the service stored.
    let baseline_request = json!({
        "TrailName": trail,
        "EventSelectors": [{"ReadWriteType": "WriteOnly", "IncludeManagementEvents": true}],
    });
    let baseline = reported(
        emulator
            .call("PutEventSelectors", &baseline_request)
            .await
            .expect_ok("PutEventSelectors: the baseline"),
    );
    assert_eq!(
        baseline,
        json!([{
            "ReadWriteType": "WriteOnly",
            "IncludeManagementEvents": true,
            "DataResources": [],
            "ExcludeManagementEventSources": [],
        }]),
        "case the baseline: expected the set that was applied, got {baseline}"
    );

    // (case, the EventSelectors the request carries, whether the call is served)
    let mut rows: Vec<(String, Value, bool)> = vec![
        (
            "management events excluded, DataResources an empty list".to_string(),
            json!([{"ReadWriteType": "All", "IncludeManagementEvents": false, "DataResources": []}]),
            false,
        ),
        (
            "management events excluded, DataResources absent".to_string(),
            json!([{"IncludeManagementEvents": false}]),
            false,
        ),
        (
            "management events excluded for one data resource".to_string(),
            json!([{"IncludeManagementEvents": false, "DataResources": [one_data_resource()]}]),
            true,
        ),
        (
            "management events included and no data resource".to_string(),
            json!([{"IncludeManagementEvents": true}]),
            true,
        ),
        (
            "an empty selector list, which configures nothing to log".to_string(),
            json!([]),
            true,
        ),
    ];
    // One row per `ReadWriteType` the model declares: the combination is refused whatever
    // read/write scope it carries, because the scope narrows what is logged rather than
    // adding to it.
    for read_write_type in
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::READ_WRITE_TYPE_VALUES
    {
        rows.push((
            format!("management events excluded at ReadWriteType {read_write_type}"),
            json!([{
                "ReadWriteType": read_write_type,
                "IncludeManagementEvents": false,
                "DataResources": [],
            }]),
            false,
        ));
    }
    // The offending entry at each position of a selector list at the documented maximum:
    // one selector that logs nothing is enough to refuse the whole set.
    for position in 0..EVENT_SELECTORS_PER_TRAIL_LIMIT {
        let selectors: Vec<Value> = (0..EVENT_SELECTORS_PER_TRAIL_LIMIT)
            .map(|index| {
                if index == position {
                    json!({"IncludeManagementEvents": false, "DataResources": []})
                } else {
                    json!({"IncludeManagementEvents": false, "DataResources": [one_data_resource()]})
                }
            })
            .collect();
        rows.push((
            format!(
                "one selector logging nothing at position {position} of \
                 {EVENT_SELECTORS_PER_TRAIL_LIMIT}"
            ),
            Value::Array(selectors),
            false,
        ));
    }

    for (case, selectors, served) in rows {
        let reply = emulator
            .call(
                "PutEventSelectors",
                &json!({"TrailName": trail, "EventSelectors": selectors}),
            )
            .await;
        if served {
            reply.expect_ok(&case);
            // A served write replaces the baseline, so put it back for the rows that
            // follow.
            emulator
                .call("PutEventSelectors", &baseline_request)
                .await
                .expect_ok(&format!("{case}: restoring the baseline"));
            continue;
        }

        reply.expect_error(&case, CLIENT_ERROR_STATUS, INVALID_EVENT_SELECTORS);
        assert_eq!(
            reply.message(),
            NOTHING_LOGGED_MESSAGE,
            "case {case:?}: expected the fixed sentence for this error class, got {:?}",
            reply.body
        );

        let got = emulator
            .call("GetEventSelectors", &json!({"TrailName": trail}))
            .await;
        let got = got.expect_ok(&format!("GetEventSelectors after {case}")).clone();
        assert_eq!(
            reported(&got),
            baseline,
            "case {case:?}: a refused write must leave the trail's selectors untouched, \
             got {got:?}"
        );
    }
}
