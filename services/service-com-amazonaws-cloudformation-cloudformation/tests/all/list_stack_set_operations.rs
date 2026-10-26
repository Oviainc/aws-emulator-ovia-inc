// SPDX-License-Identifier: CC0-1.0

//! `ListStackSetOperations` over the awsQuery wire: the operations performed on a StackSet.
//!
//! The rule every case here holds the service to: **`ListStackSetOperations` reports the
//! operations the StackSet named by the request PERSISTS — resolved by name or by id within
//! the administrator account and Region the request was signed for — one summary per
//! recorded operation with every member read off the operation record, most recently
//! initiated first and one bounded page at a time; a `StackSetName` that resolves to no
//! StackSet is refused with `StackSetNotFoundException` rather than answered with an empty
//! list, and a read reports the status the operation holds at that instant without moving
//! it.**
//!
//! The reported-members table below is one row per kind of operation a StackSet can hold:
//! its rows span all four `StackSetOperationAction` values and all six
//! `StackSetOperationStatus` values (`smithy_query(query="shape",
//! shape="StackSetOperationAction,StackSetOperationStatus")`, which
//! `state::stack_sets::operation_action` / `operation_status` name), an operation that has
//! ended and one that has not, one with a status reason and one without, and failed-instance
//! counts of none, zero and several.
//!
//! The store's WRITERS are `CreateStackSet`, `CreateStackInstances`, `UpdateStackSet`,
//! `DeleteStackInstances` and `DetectStackSetDrift`, each still a scaffold with its own stub
//! issue, so the records below are seeded straight into the store through
//! `state::ComAmazonawsCloudformationCloudformationState::record_stack_set` — the same device
//! `list_stack_instances.rs` and `extension_registry.rs` use for records whose writers are
//! not available. Every READ goes through the real request path (form-encoded body in, XML
//! envelope out), so what is asserted is what a client sees.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    operation_action, operation_status, StoredOperationPreferences, StoredStackSet,
    StoredStackSetOperation,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{self, PAGE_SIZE};

use crate::stack_lifecycle::{
    call, call_in, expected_status, member, router_with_engine, store, wait_for_the_transition,
    xml_blocks, xml_text, xml_texts, REGION,
};

/// Another Region, for the cases that hold a StackSet to the Region it was created in.
const OTHER_REGION: &str = "us-west-2";

/// The instant `1_700_000_000` and the instants a whole number of minutes after it, with
/// the text the modelled `Timestamp` renders each as. The rows below stamp their operations
/// with these, so a reported instant is compared against an exact string rather than against
/// a re-derivation of the formatting.
const INSTANT_0: (u64, &str) = (1_700_000_000, "2023-11-14T22:13:20Z");
const INSTANT_5: (u64, &str) = (1_700_000_300, "2023-11-14T22:18:20Z");
const INSTANT_10: (u64, &str) = (1_700_000_600, "2023-11-14T22:23:20Z");
const INSTANT_20: (u64, &str) = (1_700_001_200, "2023-11-14T22:33:20Z");
const INSTANT_30: (u64, &str) = (1_700_001_800, "2023-11-14T22:43:20Z");
const INSTANT_40: (u64, &str) = (1_700_002_400, "2023-11-14T22:53:20Z");
const INSTANT_50: (u64, &str) = (1_700_003_000, "2023-11-14T23:03:20Z");
const INSTANT_60: (u64, &str) = (1_700_003_600, "2023-11-14T23:13:20Z");
const INSTANT_70: (u64, &str) = (1_700_004_200, "2023-11-14T23:23:20Z");

/// One of those instants as a [`SystemTime`].
fn at(instant: (u64, &str)) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(instant.0)
}

/// One row of the reported-members table: the StackSet that holds the operation, and every
/// member the summary must report for it.
struct Row {
    case: &'static str,
    stack_set: &'static str,
    operation_id: &'static str,
    action: &'static str,
    status: &'static str,
    creation: (u64, &'static str),
    end: Option<(u64, &'static str)>,
    status_reason: Option<&'static str>,
    failed_instances: Option<i32>,
}

/// The operations the table reads, one per StackSet so each response carries exactly the
/// summary of the row under test.
const ROWS: &[Row] = &[
    Row {
        case: "a create that completed",
        stack_set: "fleet-create-succeeded",
        operation_id: "operation-create-succeeded",
        action: operation_action::CREATE,
        status: operation_status::SUCCEEDED,
        creation: INSTANT_0,
        end: Some(INSTANT_5),
        status_reason: None,
        failed_instances: Some(0),
    },
    Row {
        case: "an update that exceeded its failure tolerance",
        stack_set: "fleet-update-failed",
        operation_id: "operation-update-failed",
        action: operation_action::UPDATE,
        status: operation_status::FAILED,
        creation: INSTANT_10,
        end: Some(INSTANT_20),
        status_reason: Some("Stack instance operation failed in 3 accounts"),
        failed_instances: Some(3),
    },
    Row {
        case: "a delete the caller canceled",
        stack_set: "fleet-delete-stopped",
        operation_id: "operation-delete-stopped",
        action: operation_action::DELETE,
        status: operation_status::STOPPED,
        creation: INSTANT_20,
        end: Some(INSTANT_30),
        status_reason: Some("User initiated the cancellation of this operation"),
        failed_instances: Some(1),
    },
    Row {
        case: "a drift detection that completed",
        stack_set: "fleet-detect-drift",
        operation_id: "operation-detect-drift",
        action: operation_action::DETECT_DRIFT,
        status: operation_status::SUCCEEDED,
        creation: INSTANT_30,
        end: Some(INSTANT_40),
        status_reason: None,
        failed_instances: Some(0),
    },
    Row {
        case: "an operation queued behind another",
        stack_set: "fleet-queued",
        operation_id: "operation-queued",
        action: operation_action::CREATE,
        status: operation_status::QUEUED,
        creation: INSTANT_40,
        end: None,
        status_reason: None,
        failed_instances: None,
    },
    Row {
        case: "an operation still being performed",
        stack_set: "fleet-running",
        operation_id: "operation-running",
        action: operation_action::UPDATE,
        status: operation_status::RUNNING,
        creation: INSTANT_50,
        end: None,
        status_reason: None,
        failed_instances: None,
    },
    Row {
        case: "an operation in the process of stopping",
        stack_set: "fleet-stopping",
        operation_id: "operation-stopping",
        action: operation_action::DELETE,
        status: operation_status::STOPPING,
        creation: INSTANT_60,
        end: None,
        status_reason: Some("User initiated the cancellation of this operation"),
        failed_instances: None,
    },
    Row {
        case: "a create that failed in many accounts",
        stack_set: "fleet-create-failed",
        operation_id: "operation-create-failed",
        action: operation_action::CREATE,
        status: operation_status::FAILED,
        creation: INSTANT_70,
        end: Some(INSTANT_70),
        status_reason: Some("Stack instance operation failed in 17 accounts"),
        failed_instances: Some(17),
    },
];

/// One row as the record the store holds.
///
/// The record carries no pending conclusion: each row is an operation whose status the store
/// already holds, and an operation still ON the clock is what
/// [`an_operation_in_flight_concludes_on_the_clock_not_on_a_read`] covers.
fn stored(row: &Row) -> StoredStackSetOperation {
    let mut operation =
        StoredStackSetOperation::running(row.operation_id, row.action, at(row.creation));
    operation.pending = None;
    operation.status = row.status.to_string();
    operation.end = row.end.map(at);
    operation.status_reason = row.status_reason.map(str::to_string);
    operation.failed_stack_instances_count = row.failed_instances;
    operation
}

#[tokio::test]
async fn every_member_is_read_off_the_operation_record() {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in ROWS {
            let mut stack_set = StoredStackSet::new(row.stack_set, REGION);
            stack_set.record_operation(stored(row));
            held.record_stack_set(stack_set);
        }
    }

    for row in ROWS {
        let case = row.case;
        let (status, body) = call(
            &router,
            "ListStackSetOperations",
            &[member("StackSetName", row.stack_set)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "OperationId").as_deref(),
            Some(row.operation_id),
            "case {case:?}: the record's own operation id, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(row.action),
            "case {case:?}: the record's own action, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(row.status),
            "case {case:?}: the record's own status, got {body}"
        );
        assert_eq!(
            xml_text(&body, "CreationTimestamp").as_deref(),
            Some(row.creation.1),
            "case {case:?}: the instant the operation was initiated, got {body}"
        );
        assert_eq!(
            xml_text(&body, "EndTimestamp").as_deref(),
            row.end.map(|(_, text)| text),
            "case {case:?}: the instant the operation ended, absent while it has not, got \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "StatusReason").as_deref(),
            row.status_reason,
            "case {case:?}: the record's own status reason, got {body}"
        );
        assert_eq!(
            xml_text(&body, "FailedStackInstancesCount"),
            row.failed_instances.map(|failed| failed.to_string()),
            "case {case:?}: the recorded failed-instance count, got {body}"
        );
    }
}

/// The operations one response reports, in the order it reports them.
fn reported_operations(body: &str) -> Vec<String> {
    xml_texts(body, "OperationId")
}

#[tokio::test]
async fn the_history_is_reported_most_recently_initiated_first() {
    const STACK_SET: &str = "fleet-history";
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        for row in ROWS {
            stack_set.record_operation(stored(row));
        }
        held.record_stack_set(stack_set);
    }

    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");

    let mut expected: Vec<String> = ROWS
        .iter()
        .map(|row| row.operation_id.to_string())
        .collect();
    expected.reverse();
    assert_eq!(
        reported_operations(&body),
        expected,
        "the StackSet's whole history, newest first, got {body}"
    );
}

#[tokio::test]
async fn a_stackset_with_no_operation_answers_the_empty_list() {
    const UNUSED: &str = "fleet-untouched";
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.record_stack_set(StoredStackSet::new(UNUSED, REGION));
    }

    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", UNUSED)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    assert_eq!(
        reported_operations(&body),
        Vec::<String>::new(),
        "a StackSet nothing has been performed on reports no operation, got {body}"
    );
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "an empty collection carries no token, got {body}"
    );
}

#[tokio::test]
async fn the_preferences_an_operation_ran_under_are_reported_back() {
    // `StackSetOperationPreferences` admits either the COUNT form or the PERCENTAGE form of
    // its failure tolerance and its concurrency maximum, "but not both", and
    // `RegionConcurrencyType` / `ConcurrencyMode` are the model's own enums
    // (`smithy_query(query="shape",
    // shape="StackSetOperationPreferences,RegionConcurrencyType,ConcurrencyMode")`).
    const COUNTS: &str = "fleet-preferences-counts";
    const PERCENTAGES: &str = "fleet-preferences-percentages";
    const NONE: &str = "fleet-preferences-none";
    const REGION_ORDER: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();

        let mut counts = StoredStackSet::new(COUNTS, REGION);
        let mut operation = stored(&ROWS[0]);
        operation.preferences = Some(StoredOperationPreferences {
            region_concurrency_type: Some("PARALLEL".to_string()),
            region_order: REGION_ORDER.iter().map(|region| region.to_string()).collect(),
            failure_tolerance_count: Some(2),
            max_concurrent_count: Some(5),
            concurrency_mode: Some("SOFT_FAILURE_TOLERANCE".to_string()),
            ..StoredOperationPreferences::default()
        });
        counts.record_operation(operation);
        held.record_stack_set(counts);

        let mut percentages = StoredStackSet::new(PERCENTAGES, REGION);
        let mut operation = stored(&ROWS[1]);
        operation.preferences = Some(StoredOperationPreferences {
            region_concurrency_type: Some("SEQUENTIAL".to_string()),
            failure_tolerance_percentage: Some(25),
            max_concurrent_percentage: Some(50),
            concurrency_mode: Some("STRICT_FAILURE_TOLERANCE".to_string()),
            ..StoredOperationPreferences::default()
        });
        percentages.record_operation(operation);
        held.record_stack_set(percentages);

        let mut none = StoredStackSet::new(NONE, REGION);
        none.record_operation(stored(&ROWS[2]));
        held.record_stack_set(none);
    }

    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", COUNTS)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the count form\": expected 200, got {status} {body}"
    );
    for (tag, value) in [
        ("RegionConcurrencyType", "PARALLEL"),
        ("FailureToleranceCount", "2"),
        ("MaxConcurrentCount", "5"),
        ("ConcurrencyMode", "SOFT_FAILURE_TOLERANCE"),
    ] {
        assert_eq!(
            xml_text(&body, tag).as_deref(),
            Some(value),
            "case \"the count form\": {tag} as the operation ran under it, got {body}"
        );
    }
    for tag in ["FailureTolerancePercentage", "MaxConcurrentPercentage"] {
        assert_eq!(
            xml_text(&body, tag),
            None,
            "case \"the count form\": {tag} was never named, got {body}"
        );
    }
    let order = xml_blocks(&body, "RegionOrder");
    let reported_order: Vec<String> = order
        .first()
        .map(|block| xml_blocks(block, "member"))
        .unwrap_or_default();
    let expected_order: Vec<String> = REGION_ORDER
        .iter()
        .map(|region| region.to_string())
        .collect();
    assert_eq!(
        reported_order, expected_order,
        "case \"the count form\": the Region order the operation ran in, got {body}"
    );

    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", PERCENTAGES)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the percentage form\": expected 200, got {status} {body}"
    );
    for (tag, value) in [
        ("RegionConcurrencyType", "SEQUENTIAL"),
        ("FailureTolerancePercentage", "25"),
        ("MaxConcurrentPercentage", "50"),
        ("ConcurrencyMode", "STRICT_FAILURE_TOLERANCE"),
    ] {
        assert_eq!(
            xml_text(&body, tag).as_deref(),
            Some(value),
            "case \"the percentage form\": {tag} as the operation ran under it, got {body}"
        );
    }
    for tag in ["FailureToleranceCount", "MaxConcurrentCount", "RegionOrder"] {
        assert_eq!(
            xml_text(&body, tag),
            None,
            "case \"the percentage form\": {tag} was never named, got {body}"
        );
    }

    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", NONE)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"no preferences\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "OperationPreferences"),
        None,
        "case \"no preferences\": an operation whose request named none reports none, got \
         {body}"
    );
}

#[tokio::test]
async fn a_stackset_this_region_does_not_hold_is_refused() {
    const STACK_SET: &str = "fleet-regional";
    let (router, engine) = router_with_engine();
    let stack_set_id;
    {
        let state = store(&engine);
        let mut held = state.lock();

        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(stored(&ROWS[0]));
        stack_set_id = stack_set.stack_set_id.clone();
        held.record_stack_set(stack_set);

        // Another Region's StackSet of the SAME name, with an operation of its own.
        let mut elsewhere = StoredStackSet::new(STACK_SET, OTHER_REGION);
        elsewhere.record_operation(stored(&ROWS[1]));
        held.record_stack_set(elsewhere);
    }

    // Each row names a `StackSetName` that resolves to no StackSet of the administrator
    // account and Region the request is signed for.
    let cases: Vec<(&str, String)> = vec![
        ("a name no StackSet carries", "no-such-stack-set".to_string()),
        (
            "an id no StackSet carries",
            format!("{STACK_SET}:00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an operation id, which addresses no StackSet",
            ROWS[0].operation_id.to_string(),
        ),
        (
            "the name of a StackSet of this Region, with its case changed",
            STACK_SET.to_uppercase(),
        ),
    ];
    for (case, stack_set_name) in &cases {
        let (status, body) = call(
            &router,
            "ListStackSetOperations",
            &[member("StackSetName", stack_set_name)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("StackSetNotFoundException"),
            "case {case:?}: expected the modelled code, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("StackSet {stack_set_name} not found").as_str()),
            "case {case:?}: expected the StackSet-not-found message, got {body}"
        );
    }

    // The StackSet of this Region is addressable by the id the store minted for it, and by
    // that id ONLY in that Region; the NAME in the other Region resolves that Region's own
    // StackSet and its own operation history.
    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", &stack_set_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the StackSet addressed by its id\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_operations(&body),
        vec![ROWS[0].operation_id.to_string()],
        "case \"the StackSet addressed by its id\": its own operation, got {body}"
    );

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackSetOperations",
        &[member("StackSetName", &stack_set_id)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"another Region's StackSet id\": expected 404, got {status} {body}"
    );

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackSetOperations",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the same name in another Region\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_operations(&body),
        vec![ROWS[1].operation_id.to_string()],
        "case \"the same name in another Region\": that Region's own operation, got {body}"
    );
}

/// How many operations the paginated case seeds: more than one default page, so the default
/// read must hand out a token and the resumed read must finish the collection.
const PAGINATED_OPERATIONS: usize = PAGE_SIZE + 23;

/// The operation id of the `index`th seeded operation (`OperationId` is a
/// `ClientRequestToken`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`).
fn paginated_operation_id(index: usize) -> String {
    format!("operation-{index:05}")
}

#[tokio::test]
async fn the_collection_is_paged_and_a_token_resumes_just_past_the_page() {
    const STACK_SET: &str = "fleet-paged";
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        for index in 0..PAGINATED_OPERATIONS {
            let mut operation = StoredStackSetOperation::running(
                paginated_operation_id(index),
                operation_action::CREATE,
                at(INSTANT_0) + Duration::from_secs(index as u64),
            );
            operation.pending = None;
            operation.status = operation_status::SUCCEEDED.to_string();
            stack_set.record_operation(operation);
        }
        held.record_stack_set(stack_set);
    }

    // Newest first, so the walk reports the seeded operations in reverse seeding order.
    let expected: Vec<String> = (0..PAGINATED_OPERATIONS)
        .rev()
        .map(paginated_operation_id)
        .collect();

    // A request naming no `MaxResults` gets this service's own page size, and a token.
    let (status, first) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the default page\": expected 200, got {status} {first}"
    );
    assert_eq!(
        reported_operations(&first),
        &expected[..PAGE_SIZE],
        "case \"the default page\": the first {PAGE_SIZE} operations, got {first}"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the default page\": expected a NextToken, got {first}")
    });

    let (status, second) = call(
        &router,
        "ListStackSetOperations",
        &[
            member("StackSetName", STACK_SET),
            member("NextToken", &token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the resumed page\": expected 200, got {status} {second}"
    );
    assert_eq!(
        reported_operations(&second),
        &expected[PAGE_SIZE..],
        "case \"the resumed page\": the rest of the collection, got {second}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no token, got {second}"
    );

    // `MaxResults` is "the maximum number of results to be returned with a single call": a
    // walk in pages of that size reports the whole collection exactly once, in order, and a
    // caller may change the maximum between calls.
    for max_results in [1usize, 7, PAGE_SIZE] {
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("MaxResults", &max_results.to_string()),
            ];
            if let Some(token) = &next {
                members.push(member("NextToken", token));
            }
            let (status, body) = call(&router, "ListStackSetOperations", &members).await;
            assert_eq!(
                status, 200,
                "case \"MaxResults {max_results}\": expected 200, got {status} {body}"
            );
            let page = reported_operations(&body);
            assert!(
                page.len() <= max_results,
                "case \"MaxResults {max_results}\": a page may not exceed the maximum, got \
                 {} in {body}",
                page.len()
            );
            walked.extend(page);
            next = xml_text(&body, "NextToken");
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case \"MaxResults {max_results}\": the walk must report the collection once, got \
             {walked:?}"
        );
    }

    // A token this service did not issue for THIS collection is a bad request, not an empty
    // page: one past the end of the collection, and one issued for another collection.
    let beyond_the_end = state::encode_next_token(
        state::PageOf::StackSetOperationSummaries,
        PAGINATED_OPERATIONS + 1,
    );
    let other_collection = state::encode_next_token(state::PageOf::StackInstanceSummaries, 0);
    for (case, token) in [
        ("a cursor past the end of the collection", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
        ("a token this service never issued", "not-a-token".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "ListStackSetOperations",
            &[
                member("StackSetName", STACK_SET),
                member("NextToken", &token),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("{token} is not a valid NextToken").as_str()),
            "case {case:?}: expected the invalid-token refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn an_operation_in_flight_concludes_on_the_clock_not_on_a_read() {
    const IN_FLIGHT: &str = "fleet-in-flight";
    const LONG_RUNNING: &str = "fleet-long-running";
    const IN_FLIGHT_OPERATION: &str = "operation-in-flight";
    const LONG_RUNNING_OPERATION: &str = "operation-long-running";
    /// Far enough out that no read of this case can reach it: an operation whose conclusion
    /// is not yet due must stay in flight however many times it is listed.
    const NOT_DUE_FOR: Duration = Duration::from_secs(3_600);

    let (router, engine) = router_with_engine();
    let started = std::time::Instant::now();
    {
        let now = SystemTime::now();
        let state = store(&engine);
        let mut held = state.lock();

        let mut in_flight = StoredStackSet::new(IN_FLIGHT, REGION);
        in_flight.record_operation(StoredStackSetOperation::running(
            IN_FLIGHT_OPERATION,
            operation_action::CREATE,
            now,
        ));
        held.record_stack_set(in_flight);

        let mut long_running = StoredStackSet::new(LONG_RUNNING, REGION);
        let mut operation = StoredStackSetOperation::running(
            LONG_RUNNING_OPERATION,
            operation_action::UPDATE,
            now,
        );
        if let Some(pending) = operation.pending.as_mut() {
            pending.due_at = now + NOT_DUE_FOR;
        }
        long_running.record_operation(operation);
        held.record_stack_set(long_running);
    }

    // A caller listing straight after the call that started the operation sees it in flight
    // — unless the clock already made its conclusion due, which the guard allows for.
    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", IN_FLIGHT)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"straight after the call\": expected 200, got {status} {body}"
    );
    let owed = expected_status(started, operation_status::RUNNING, operation_status::SUCCEEDED);
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(owed),
        "case \"straight after the call\": expected {owed}, got {body}"
    );
    assert_eq!(
        xml_text(&body, "EndTimestamp").is_some(),
        owed == operation_status::SUCCEEDED,
        "case \"straight after the call\": an operation ends exactly when it concludes, got \
         {body}"
    );

    // Past the instant its conclusion was due, every later read reports the conclusion and
    // the instant it ended at.
    wait_for_the_transition().await;
    let (status, body) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", IN_FLIGHT)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"past the due instant\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(operation_status::SUCCEEDED),
        "case \"past the due instant\": the conclusion the clock owed, got {body}"
    );
    assert!(
        xml_text(&body, "EndTimestamp").is_some(),
        "case \"past the due instant\": a concluded operation carries its end instant, got \
         {body}"
    );
    assert_eq!(
        xml_text(&body, "FailedStackInstancesCount").as_deref(),
        Some("0"),
        "case \"past the due instant\": the details of that conclusion, got {body}"
    );

    // And a read NEVER moves an operation whose conclusion is not due: two reads of the
    // long-running operation report the same status, and no end instant.
    for pass in ["the first read", "the second read"] {
        let (status, body) = call(
            &router,
            "ListStackSetOperations",
            &[member("StackSetName", LONG_RUNNING)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {pass:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::RUNNING),
            "case {pass:?}: a read must not conclude an operation, got {body}"
        );
        assert_eq!(
            xml_text(&body, "EndTimestamp"),
            None,
            "case {pass:?}: an operation still in flight has not ended, got {body}"
        );
    }
}
