// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the queue configuration an event source mapping is accepted
//! against: `CreateEventSourceMapping` "creates a mapping between an event source and an
//! Lambda Event Function. Lambda reads items from the event source and invokes the
//! function."
//!
//! The rule under test: a mapping between an Amazon SQS queue and a function is refused
//! with the modelled `InvalidParameterValueException` (HTTP 400) when the queue's
//! effective `VisibilityTimeout` is SHORTER than the invoked function's `Timeout`, and
//! accepted whenever it is at least as long — because a queue hands a message to one
//! consumer for the length of its visibility timeout and makes it visible again once that
//! lapses, so a shorter timeout re-delivers a message the function is still working on.
//! The same comparison holds for `UpdateEventSourceMapping` when it re-targets a mapping
//! onto another function, which is the other call that states a queue-and-function pair.
//! An event source that is not a queue has no visibility timeout, so no such refusal
//! applies to it.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with
//! BOTH services on one engine — the queue is created through Amazon SQS exactly as a
//! client creates it, and the mapping through Lambda — so a comparison made against
//! anything other than the stored queue fails here. The rows differ along what the rule
//! generalises over: the two timeouts (below, equal and above, at each published bound of
//! both members and one second either side of equality), whether the queue states a
//! visibility timeout at all or carries the documented default, and which call states the
//! pair.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_mapping, create_queue, create_timed_function, get_mapping, list_mappings,
    router_with_queues, Answer, KINESIS_SOURCE, MAPPINGS_URI,
};

/// The `State` a freshly created mapping carries.
const STATE_CREATING: &str = "Creating";

/// The wire code of the modelled 400 this refusal is carried by.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// How long past a due instant this suite waits before calling: the settle out of
/// `Creating` is owed by the CLOCK, and an update inside that window is refused for the
/// mapping's availability rather than for its timeouts.
const SLACK: Duration = Duration::from_millis(100);

/// The documented default of the queue attribute under test: "Valid values: An integer
/// from 0 to 43,200 (12 hours). Default: 30."
const DEFAULT_VISIBILITY_TIMEOUT: i64 = 30;

/// The published bounds of that attribute, from the same sentence.
const MIN_VISIBILITY_TIMEOUT: i64 = 0;
/// See [`MIN_VISIBILITY_TIMEOUT`].
const MAX_VISIBILITY_TIMEOUT: i64 = 43_200;

/// The shortest `Timeout` a function may carry (`@range(min: 1)` on the member,
/// `smithy_query(query="operation", operation="CreateFunction")`).
const MIN_FUNCTION_TIMEOUT: i64 = 1;

/// The longest one it may carry: AWS's published Lambda quota "Function timeout: 900
/// seconds (15 minutes)", which is what this emulator refuses a larger value against.
const MAX_FUNCTION_TIMEOUT: i64 = 900;

/// Create a queue called `name`, stating `VisibilityTimeout` when `visibility_timeout`
/// names one and leaving the queue at the documented default otherwise, and answer the
/// ARN it reports.
///
/// The queue is created, and its ARN read back, through the shared harness every suite
/// spanning Lambda and Amazon SQS uses; only which attributes this suite states is its own.
async fn create_timed_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    visibility_timeout: Option<i64>,
) -> String {
    let attributes = visibility_timeout.map_or_else(
        || json!({}),
        |seconds| json!({ "VisibilityTimeout": seconds.to_string() }),
    );
    create_queue(router, case, name, &attributes).await.1
}

/// `UpdateEventSourceMapping` on the mapping `uuid` names.
async fn update_mapping(router: &MultiServiceRouter, uuid: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{MAPPINGS_URI}/{uuid}"), body).await
}

/// The sentence a refused pair earns, as Lambda words it for this error class.
fn refusal(visibility_timeout: i64, function_timeout: i64) -> String {
    format!(
        "Queue visibility timeout: {visibility_timeout} seconds is less than Function \
         timeout: {function_timeout} seconds"
    )
}

/// Assert `answer` is the modelled refusal naming both timeouts.
fn assert_refused(case: &str, answer: &Answer, visibility_timeout: i64, function_timeout: i64) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains(INVALID_PARAMETER),
        "case {case:?}: expected {INVALID_PARAMETER}, got {} in {}",
        answer.error_code(),
        answer.text()
    );
    let expected = refusal(visibility_timeout, function_timeout);
    let message = answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?} in {}",
        answer.text()
    );
}

/// One row: the queue's visibility timeout (stated or left at the default), the function's
/// timeout, and whether the pair is one Lambda accepts.
struct Row {
    case: &'static str,
    /// `None` leaves the queue at the documented default.
    visibility_timeout: Option<i64>,
    function_timeout: i64,
}

impl Row {
    /// The timeout the queue is effectively polled under, which is what the comparison —
    /// and the refusal — names.
    fn effective_visibility(&self) -> i64 {
        self.visibility_timeout
            .unwrap_or(DEFAULT_VISIBILITY_TIMEOUT)
    }

    /// Whether the pair is refused: the queue would re-deliver mid-invocation.
    fn refused(&self) -> bool {
        self.effective_visibility() < self.function_timeout
    }
}

/// The catalog: the two timeouts below, equal and above each other, at each published
/// bound of both members, and one second either side of equality.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queue two seconds below a five second function",
            visibility_timeout: Some(2),
            function_timeout: 5,
        },
        Row {
            case: "a queue exactly as long as the function",
            visibility_timeout: Some(5),
            function_timeout: 5,
        },
        Row {
            case: "a queue one second below the function",
            visibility_timeout: Some(29),
            function_timeout: 30,
        },
        Row {
            case: "a queue one second above the function",
            visibility_timeout: Some(31),
            function_timeout: 30,
        },
        Row {
            case: "a queue at the documented default, below the function",
            visibility_timeout: None,
            function_timeout: 60,
        },
        Row {
            case: "a queue at the documented default, equal to the function",
            visibility_timeout: None,
            function_timeout: DEFAULT_VISIBILITY_TIMEOUT,
        },
        Row {
            case: "a queue at the documented default, above the function",
            visibility_timeout: None,
            function_timeout: 5,
        },
        Row {
            case: "a queue at its published minimum, below the shortest function",
            visibility_timeout: Some(MIN_VISIBILITY_TIMEOUT),
            function_timeout: MIN_FUNCTION_TIMEOUT,
        },
        Row {
            case: "a queue one second long, equal to the shortest function",
            visibility_timeout: Some(MIN_FUNCTION_TIMEOUT),
            function_timeout: MIN_FUNCTION_TIMEOUT,
        },
        Row {
            case: "a queue one second below the longest function",
            visibility_timeout: Some(MAX_FUNCTION_TIMEOUT - 1),
            function_timeout: MAX_FUNCTION_TIMEOUT,
        },
        Row {
            case: "a queue exactly as long as the longest function",
            visibility_timeout: Some(MAX_FUNCTION_TIMEOUT),
            function_timeout: MAX_FUNCTION_TIMEOUT,
        },
        Row {
            case: "a queue at its published maximum, above the longest function",
            visibility_timeout: Some(MAX_VISIBILITY_TIMEOUT),
            function_timeout: MAX_FUNCTION_TIMEOUT,
        },
    ]
}

#[tokio::test]
async fn a_queue_mapping_is_accepted_only_where_the_queue_outlasts_the_function() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("esm-visibility-{index}");
        let queue = format!("esm-visibility-queue-{index}");
        create_timed_function(&router, case, &function, row.function_timeout).await;
        let queue_arn = create_timed_queue(&router, case, &queue, row.visibility_timeout).await;

        let answer = create_mapping(
            &router,
            &json!({ "FunctionName": function, "EventSourceArn": queue_arn }),
        )
        .await;

        if row.refused() {
            assert_refused(
                case,
                &answer,
                row.effective_visibility(),
                row.function_timeout,
            );
            // The refusal is instead of the mapping, not beside it: nothing polls the
            // queue afterwards.
            let listed = list_mappings(&router, &format!("EventSourceArn={queue_arn}")).await;
            assert_eq!(
                (listed.status, listed.listed_uuids()),
                (200, Vec::new()),
                "case {case:?}: a refused create must leave no mapping behind, got {} {}",
                listed.status,
                listed.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            202,
            "case {case:?}: expected HTTP 202, got {} {}",
            answer.status,
            answer.text()
        );
        let uuid = answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()));

        // Round trip: the accepted mapping is one this emulator holds, on the queue it
        // was made for.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (
                reading.status,
                reading.string("EventSourceArn"),
                reading.string("State"),
            ),
            (
                200,
                Some(queue_arn.clone()),
                Some(STATE_CREATING.to_string()),
            ),
            "case {case:?}: the mapping must read back on {queue_arn}, got {} {}",
            reading.status,
            reading.text()
        );
    }
}

#[tokio::test]
async fn an_event_source_without_a_visibility_timeout_is_not_held_to_one() {
    // A Kinesis data stream has no visibility timeout at all, so the longest function
    // this emulator admits still maps onto it.
    let case = "a kinesis stream and the longest function";
    let router = router_with_queues();
    let function = "esm-visibility-stream";
    create_timed_function(&router, case, function, MAX_FUNCTION_TIMEOUT).await;

    let answer = create_mapping(
        &router,
        &json!({
            "FunctionName": function,
            "EventSourceArn": KINESIS_SOURCE,
            "StartingPosition": "LATEST",
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        answer.status,
        answer.text()
    );
}

#[tokio::test]
async fn re_targeting_a_queue_mapping_is_held_to_the_same_comparison() {
    let case = "a re-target onto a longer function";
    let router = router_with_queues();
    // One queue, and three functions: the one the mapping is created on, one that outruns
    // the queue, and one that does not.
    let visibility_timeout = 5;
    let queue_arn = create_timed_queue(
        &router,
        case,
        "esm-visibility-retarget",
        Some(visibility_timeout),
    )
    .await;
    create_timed_function(&router, case, "esm-retarget-fits", visibility_timeout).await;
    create_timed_function(&router, case, "esm-retarget-too-long", 300).await;
    create_timed_function(&router, case, "esm-retarget-also-fits", MIN_FUNCTION_TIMEOUT).await;

    let created = create_mapping(
        &router,
        &json!({ "FunctionName": "esm-retarget-fits", "EventSourceArn": queue_arn }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected the mapping to be created, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    let function_arn = created
        .string("FunctionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {}", created.text()));

    // The mapping is not available to any update while it is still settling, so the wait
    // is past the instant the create made the move due.
    tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;

    let refused = update_mapping(
        &router,
        &uuid,
        &json!({ "FunctionName": "esm-retarget-too-long" }),
    )
    .await;
    assert_refused(case, &refused, visibility_timeout, 300);

    // And the refusal changed nothing: the mapping still invokes the function it was
    // created on.
    let reading = get_mapping(&router, &uuid).await;
    assert_eq!(
        (reading.status, reading.string("FunctionArn")),
        (200, Some(function_arn)),
        "case {case:?}: a refused re-target must leave the mapping on its function, got {} {}",
        reading.status,
        reading.text()
    );

    // A re-target onto a function the queue does outlast is accepted.
    let accepted = update_mapping(
        &router,
        &uuid,
        &json!({ "FunctionName": "esm-retarget-also-fits" }),
    )
    .await;
    assert_eq!(
        accepted.status,
        202,
        "case \"a re-target onto a shorter function\": expected HTTP 202, got {} {}",
        accepted.status,
        accepted.text()
    );
    assert!(
        accepted
            .string("FunctionArn")
            .is_some_and(|arn| arn.ends_with("esm-retarget-also-fits")),
        "case \"a re-target onto a shorter function\": expected the mapping to move onto it, \
         got {}",
        accepted.text()
    );
}
