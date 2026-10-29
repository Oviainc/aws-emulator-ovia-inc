// SPDX-License-Identifier: CC0-1.0

//! `ListStackSetOperationResults` over the awsQuery wire: what one StackSet operation did in
//! each account and Region it acted on.
//!
//! The rule every case here holds the service to: **`ListStackSetOperationResults` reports
//! one summary per account-and-Region TARGET the operation the request names — of the
//! StackSet the request names, resolved within the administrator account and Region the
//! request was signed for — acted on, every member read off the stored record of that target,
//! narrowed by the modelled result-status filter and handed out one bounded page at a time; a
//! `StackSetName` that resolves to no StackSet is refused with `StackSetNotFoundException`
//! and an `OperationId` that StackSet holds no operation under with
//! `OperationNotFoundException`, rather than answered with an empty list.**
//!
//! The reported-members table below is one row per kind of target a StackSet operation can
//! leave behind: its rows span every value of `StackInstanceDetailedStatus`
//! (`smithy_query(query="shape", shape="StackInstanceDetailedStatus")`, which
//! `state::stack_sets::detailed_status` names) and so every one of the five
//! `StackSetOperationResultStatus` values (`smithy_query(query="shape",
//! shape="StackSetOperationResultStatus")`), a result with a reason and one without, a
//! service-managed target carrying an organizational unit id and a self-managed one carrying
//! none, and several accounts across several target Regions.
//!
//! Those records span target shapes no single call sequence produces, so they are seeded
//! straight into the store through
//! `state::ComAmazonawsCloudformationCloudformationState::record_stack_set` — the same device
//! `list_stack_set_operations.rs` and `list_stack_instances.rs` use for records whose writers
//! are not available. [`a_concluded_operations_results_outlive_the_instances_it_removed`]
//! drives the whole thing through real calls instead, so the read is also held to what the
//! service's own writers leave behind. Every READ goes through the real request path
//! (form-encoded body in, XML envelope out), so what is asserted is what a client sees.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, operation_action, operation_result_filter_name, operation_status,
    result_status, StoredStackInstance, StoredStackSet, StoredStackSetOperation,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{self, PAGE_SIZE};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router, router_with_engine, store, wait_for_the_transition, xml_blocks,
    xml_text, REGION,
};

/// Another administrator Region, for the cases that hold a StackSet to the Region it was
/// created in.
const OTHER_REGION: &str = "us-west-2";

/// Target accounts beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";
const ACCOUNT_C: &str = "333333333333";

/// Target Regions beside [`REGION`]: a stack instance's Region is a target the deploying
/// request chooses, so a result is reported for the pair rather than for the account alone.
const WEST: &str = "us-west-2";
const IRELAND: &str = "eu-west-1";

/// An organizational unit id and an organization root id, the two forms
/// `OrganizationalUnitId` admits
/// (`@pattern("^(ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}|r-[a-z0-9]{4,32})$")`).
const ORGANIZATIONAL_UNIT: &str = "ou-ab12-cdef3456";
const ORGANIZATION_ROOT: &str = "r-ab12";

/// The template the lifecycle case's StackSet is created from: one topic, really created in
/// Amazon SNS by each instance's stack and really deleted when that stack is deleted.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"instance-display"}}}}"#;

/// One row of the reported-members table: the target the operation acted on, the state its
/// stack instance records, and every member the summary must report for it.
struct Row {
    case: &'static str,
    account: &'static str,
    region: &'static str,
    /// The `StackInstanceStatus$DetailedStatus` the operation left on that target.
    detailed_status: &'static str,
    /// The `StackSetOperationResultStatus` that detailed status IS, for this operation.
    expected_status: &'static str,
    status_reason: Option<&'static str>,
    organizational_unit_id: Option<&'static str>,
}

/// The targets the table reads, all under ONE operation of ONE StackSet, so one response
/// carries every row.
const ROWS: &[Row] = &[
    Row {
        case: "a target the operation completed in",
        account: ACCOUNT_A,
        region: REGION,
        detailed_status: detailed_status::SUCCEEDED,
        expected_status: result_status::SUCCEEDED,
        status_reason: None,
        organizational_unit_id: None,
    },
    Row {
        case: "a target the operation is still working in",
        account: ACCOUNT_A,
        region: WEST,
        detailed_status: detailed_status::RUNNING,
        expected_status: result_status::RUNNING,
        status_reason: None,
        organizational_unit_id: Some(ORGANIZATIONAL_UNIT),
    },
    Row {
        case: "a target the operation has yet to start in",
        account: ACCOUNT_B,
        region: REGION,
        detailed_status: detailed_status::PENDING,
        expected_status: result_status::PENDING,
        status_reason: None,
        organizational_unit_id: Some(ORGANIZATION_ROOT),
    },
    Row {
        case: "a target the operation failed in, with a reason",
        account: ACCOUNT_B,
        region: WEST,
        detailed_status: detailed_status::FAILED,
        expected_status: result_status::FAILED,
        status_reason: Some("Resource handler returned message: topic already exists"),
        organizational_unit_id: None,
    },
    Row {
        case: "a target the caller's stop cancelled",
        account: ACCOUNT_B,
        region: IRELAND,
        detailed_status: detailed_status::CANCELLED,
        expected_status: result_status::CANCELLED,
        status_reason: Some("User initiated the cancellation of this operation"),
        organizational_unit_id: None,
    },
    Row {
        case: "a target a failed deletion left unstable",
        account: ACCOUNT_C,
        region: REGION,
        detailed_status: detailed_status::INOPERABLE,
        expected_status: result_status::FAILED,
        status_reason: Some("A DeleteStackInstances operation failed"),
        organizational_unit_id: Some(ORGANIZATIONAL_UNIT),
    },
    Row {
        case: "a target whose import did not complete",
        account: ACCOUNT_C,
        region: WEST,
        detailed_status: detailed_status::FAILED_IMPORT,
        expected_status: result_status::FAILED,
        status_reason: Some("The stack could not be imported into the StackSet"),
        organizational_unit_id: None,
    },
    Row {
        case: "a target whose account is suspended, so it was skipped",
        account: ACCOUNT_C,
        region: IRELAND,
        detailed_status: detailed_status::SKIPPED_SUSPENDED_ACCOUNT,
        expected_status: result_status::CANCELLED,
        status_reason: Some("The account is suspended"),
        organizational_unit_id: Some(ORGANIZATION_ROOT),
    },
];

/// One target as the stack-instance record the operation left behind.
fn instance(
    account: &str,
    region: &str,
    operation_id: &str,
    detailed: &str,
    status_reason: Option<&str>,
    organizational_unit_id: Option<&str>,
) -> StoredStackInstance {
    let mut instance = StoredStackInstance::new(account, region, operation_id);
    instance.detailed_status = detailed.to_string();
    instance.status_reason = status_reason.map(str::to_string);
    instance.organizational_unit_id = organizational_unit_id.map(str::to_string);
    instance
}

/// One row as the stack-instance record the store holds.
fn stored_instance(row: &Row, operation_id: &str) -> StoredStackInstance {
    instance(
        row.account,
        row.region,
        operation_id,
        row.detailed_status,
        row.status_reason,
        row.organizational_unit_id,
    )
}

/// An operation the store holds as already concluded, with NO frozen results of its own: the
/// results of such a record are the state of the instances it acted on, which is what the
/// seeded rows are. [`a_concluded_operations_results_outlive_the_instances_it_removed`] is
/// what covers the record a real conclusion freezes.
fn concluded(operation_id: &str, action: &str) -> StoredStackSetOperation {
    let mut operation = StoredStackSetOperation::running(operation_id, action, SystemTime::now());
    operation.pending = None;
    operation.status = operation_status::SUCCEEDED.to_string();
    operation
}

/// The `(Account, Region)` pairs one response reports, in the order it reports them.
fn reported_targets(body: &str) -> Vec<(String, String)> {
    xml_blocks(body, "member")
        .iter()
        .map(|summary| {
            (
                xml_text(summary, "Account").unwrap_or_default(),
                xml_text(summary, "Region").unwrap_or_default(),
            )
        })
        .collect()
}

/// The one summary a response carries for `(account, region)`, if it carries one.
fn summary_for(body: &str, account: &str, region: &str) -> Option<String> {
    xml_blocks(body, "member").into_iter().find(|summary| {
        xml_text(summary, "Account").as_deref() == Some(account)
            && xml_text(summary, "Region").as_deref() == Some(region)
    })
}

/// `ListStackSetOperationResults` for one operation of one StackSet, plus whatever else the
/// case names.
async fn list_results(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    operation_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("OperationId", operation_id),
    ];
    members.extend_from_slice(extra);
    call(router, "ListStackSetOperationResults", &members).await
}

#[tokio::test]
async fn every_member_is_read_off_the_targets_own_record() {
    const STACK_SET: &str = "fleet-reported-members";
    const OPERATION: &str = "operation-reported-members";

    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(concluded(OPERATION, operation_action::UPDATE));
        for row in ROWS {
            stack_set.record_instance(stored_instance(row, OPERATION));
        }
        held.record_stack_set(stack_set);
    }

    let (status, body) = list_results(&router, STACK_SET, OPERATION, &[]).await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    assert_eq!(
        reported_targets(&body).len(),
        ROWS.len(),
        "one summary per target the operation acted on, got {body}"
    );

    for row in ROWS {
        let case = row.case;
        let summary = summary_for(&body, row.account, row.region).unwrap_or_else(|| {
            panic!("case {case:?}: expected a summary for that target, got {body}")
        });
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some(row.expected_status),
            "case {case:?}: the result status that target's own state is, got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "StatusReason").as_deref(),
            row.status_reason,
            "case {case:?}: the record's own reason, absent for a result with none, got \
             {summary}"
        );
        assert_eq!(
            xml_text(&summary, "OrganizationalUnitId").as_deref(),
            row.organizational_unit_id,
            "case {case:?}: the target's organizational unit, absent for a self-managed one, \
             got {summary}"
        );
        assert_eq!(
            xml_text(&summary, "AccountGateResult"),
            None,
            "case {case:?}: no account gate function is registered, so no result carries one, \
             got {summary}"
        );
    }
}

#[tokio::test]
async fn only_the_targets_of_the_named_operation_are_reported() {
    const STACK_SET: &str = "fleet-per-operation";
    const EARLIER: &str = "operation-earlier";
    const LATER: &str = "operation-later";
    const UNTARGETED: &str = "operation-untargeted";

    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(concluded(EARLIER, operation_action::CREATE));
        stack_set.record_operation(concluded(LATER, operation_action::UPDATE));
        stack_set.record_operation(concluded(UNTARGETED, operation_action::DETECT_DRIFT));
        // The pairs the EARLIER operation last acted on, and the ones the LATER one did.
        for (account, region, operation_id) in [
            (ACCOUNT_A, REGION, EARLIER),
            (ACCOUNT_A, WEST, EARLIER),
            (ACCOUNT_B, REGION, LATER),
        ] {
            stack_set.record_instance(instance(
                account,
                region,
                operation_id,
                detailed_status::SUCCEEDED,
                None,
                None,
            ));
        }
        held.record_stack_set(stack_set);
    }

    for (case, operation_id, expected) in [
        (
            "the earlier operation's own targets",
            EARLIER,
            vec![(ACCOUNT_A, REGION), (ACCOUNT_A, WEST)],
        ),
        (
            "the later operation's own targets",
            LATER,
            vec![(ACCOUNT_B, REGION)],
        ),
        (
            "an operation that acted on no target",
            UNTARGETED,
            Vec::new(),
        ),
    ] {
        let (status, body) = list_results(&router, STACK_SET, operation_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(account, region)| (account.to_string(), region.to_string()))
            .collect();
        assert_eq!(
            reported_targets(&body),
            expected,
            "case {case:?}: exactly the targets that operation acted on, got {body}"
        );
        if expected.is_empty() {
            assert_eq!(
                xml_text(&body, "NextToken"),
                None,
                "case {case:?}: an empty collection carries no token, got {body}"
            );
        }
    }
}

/// One `Filters` entry as `Filters.member.1.{Name,Values}`, as an AWS SDK form-encodes one
/// filter. `Filters` is `@length(0..1)`, so a request names at most this one.
fn result_filter(name: &str, values: Option<&str>) -> Vec<(String, String)> {
    let mut members = vec![member("Filters.member.1.Name", name)];
    if let Some(values) = values {
        members.push(member("Filters.member.1.Values", values));
    }
    members
}

#[tokio::test]
async fn the_result_status_filter_narrows_the_collection() {
    const STACK_SET: &str = "fleet-filtered";
    const OPERATION: &str = "operation-filtered";

    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(concluded(OPERATION, operation_action::UPDATE));
        for row in ROWS {
            stack_set.record_instance(stored_instance(row, OPERATION));
        }
        held.record_stack_set(stack_set);
    }

    // Every value of the modelled `OperationResultFilterName`'s one filter: each selects
    // exactly the targets the operation left in that result status.
    for status_filter in [
        result_status::PENDING,
        result_status::RUNNING,
        result_status::SUCCEEDED,
        result_status::FAILED,
        result_status::CANCELLED,
    ] {
        let expected: Vec<(String, String)> = ROWS
            .iter()
            .filter(|row| row.expected_status == status_filter)
            .map(|row| (row.account.to_string(), row.region.to_string()))
            .collect();
        assert!(
            !expected.is_empty(),
            "case {status_filter:?}: the table must cover every result status"
        );
        let (status, body) = list_results(
            &router,
            STACK_SET,
            OPERATION,
            &result_filter(
                operation_result_filter_name::OPERATION_RESULT_STATUS,
                Some(status_filter),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {status_filter:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_targets(&body),
            expected,
            "case {status_filter:?}: exactly the targets in that result status, got {body}"
        );
    }

    // Every target, in the order the StackSet holds them: what a request that narrows
    // nothing reports.
    let every_target: Vec<(String, String)> = ROWS
        .iter()
        .map(|row| (row.account.to_string(), row.region.to_string()))
        .collect();
    for (case, extra) in [
        ("no filter at all", Vec::new()),
        (
            "a filter naming no value, which narrows nothing",
            result_filter(operation_result_filter_name::OPERATION_RESULT_STATUS, None),
        ),
    ] {
        let (status, body) = list_results(&router, STACK_SET, OPERATION, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_targets(&body),
            every_target,
            "case {case:?}: every target the operation acted on, got {body}"
        );
    }
}

#[tokio::test]
async fn a_stackset_or_an_operation_the_store_does_not_hold_is_refused() {
    const STACK_SET: &str = "fleet-refusals";
    const OPERATION: &str = "operation-refusals";
    const ELSEWHERE: &str = "fleet-refusals-elsewhere";
    const ELSEWHERE_OPERATION: &str = "operation-refusals-elsewhere";

    let (router, engine) = router_with_engine();
    let stack_set_id;
    {
        let state = store(&engine);
        let mut held = state.lock();

        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(concluded(OPERATION, operation_action::CREATE));
        stack_set.record_instance(instance(
            ACCOUNT_A,
            REGION,
            OPERATION,
            detailed_status::SUCCEEDED,
            None,
            None,
        ));
        stack_set_id = stack_set.stack_set_id.clone();
        held.record_stack_set(stack_set);

        // Another StackSet, with an operation of its own: an operation id is unique within
        // the StackSet that holds it, so it addresses no operation of any other.
        let mut elsewhere = StoredStackSet::new(ELSEWHERE, REGION);
        elsewhere.record_operation(concluded(ELSEWHERE_OPERATION, operation_action::UPDATE));
        held.record_stack_set(elsewhere);
    }

    // A `StackSetName` that resolves to no StackSet of the administrator account and Region
    // the request is signed for.
    let unresolved: Vec<(&str, String)> = vec![
        ("a name no StackSet carries", "no-such-stack-set".to_string()),
        (
            "an id no StackSet carries",
            format!("{STACK_SET}:00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an operation id, which addresses no StackSet",
            OPERATION.to_string(),
        ),
        (
            "the name of a StackSet of this Region, with its case changed",
            STACK_SET.to_uppercase(),
        ),
    ];
    for (case, stack_set_name) in &unresolved {
        let (status, body) = list_results(&router, stack_set_name, OPERATION, &[]).await;
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

    // An `OperationId` the resolved StackSet holds no operation under.
    for (case, operation_id) in [
        ("an id no operation carries", "operation-no-such"),
        ("another StackSet's operation id", ELSEWHERE_OPERATION),
        ("the StackSet's own name, which addresses no operation", STACK_SET),
    ] {
        let (status, body) = list_results(&router, STACK_SET, operation_id, &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("OperationNotFoundException"),
            "case {case:?}: expected the modelled code, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some("Operation not found"),
            "case {case:?}: expected the operation-not-found message, got {body}"
        );
    }

    // The StackSet is addressable by the id the store minted for it, and by that id ONLY in
    // the Region that holds it.
    let (status, body) = list_results(&router, &stack_set_id, OPERATION, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the StackSet addressed by its id\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_targets(&body),
        vec![(ACCOUNT_A.to_string(), REGION.to_string())],
        "case \"the StackSet addressed by its id\": its operation's own target, got {body}"
    );

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListStackSetOperationResults",
        &[
            member("StackSetName", &stack_set_id),
            member("OperationId", OPERATION),
        ],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"another Region's StackSet id\": expected 404, got {status} {body}"
    );
}

/// How many targets the paginated case seeds: more than one default page, so the default read
/// must hand out a token and the resumed read must finish the collection.
const PAGINATED_TARGETS: usize = PAGE_SIZE + 23;

/// The target account of the `index`th seeded result (`Account` is
/// `@pattern("^[0-9]{12}$")`).
fn paginated_account(index: usize) -> String {
    format!("{:012}", index + 1)
}

#[tokio::test]
async fn the_collection_is_paged_and_a_token_resumes_just_past_the_page() {
    const STACK_SET: &str = "fleet-paged-results";
    const OPERATION: &str = "operation-paged-results";

    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        stack_set.record_operation(concluded(OPERATION, operation_action::CREATE));
        for index in 0..PAGINATED_TARGETS {
            stack_set.record_instance(instance(
                &paginated_account(index),
                REGION,
                OPERATION,
                detailed_status::SUCCEEDED,
                None,
                None,
            ));
        }
        held.record_stack_set(stack_set);
    }

    let expected: Vec<(String, String)> = (0..PAGINATED_TARGETS)
        .map(|index| (paginated_account(index), REGION.to_string()))
        .collect();

    // A request naming no `MaxResults` gets this service's own page size, and a token.
    let (status, first) = list_results(&router, STACK_SET, OPERATION, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the default page\": expected 200, got {status} {first}"
    );
    assert_eq!(
        reported_targets(&first),
        &expected[..PAGE_SIZE],
        "case \"the default page\": the first {PAGE_SIZE} results, got {first}"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the default page\": expected a NextToken, got {first}")
    });

    let (status, second) =
        list_results(&router, STACK_SET, OPERATION, &[member("NextToken", &token)]).await;
    assert_eq!(
        status, 200,
        "case \"the resumed page\": expected 200, got {status} {second}"
    );
    assert_eq!(
        reported_targets(&second),
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
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut extra = vec![member("MaxResults", &max_results.to_string())];
            if let Some(token) = &next {
                extra.push(member("NextToken", token));
            }
            let (status, body) = list_results(&router, STACK_SET, OPERATION, &extra).await;
            assert_eq!(
                status, 200,
                "case \"MaxResults {max_results}\": expected 200, got {status} {body}"
            );
            let page = reported_targets(&body);
            assert!(
                page.len() <= max_results,
                "case \"MaxResults {max_results}\": a page may not exceed the maximum, got {} \
                 in {body}",
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
        state::PageOf::StackSetOperationResultSummaries,
        PAGINATED_TARGETS + 1,
    );
    let other_collection =
        state::encode_next_token(state::PageOf::StackSetOperationSummaries, 0);
    for (case, token) in [
        ("a cursor past the end of the collection", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
        ("a token this service never issued", "not-a-token".to_string()),
    ] {
        let (status, body) =
            list_results(&router, STACK_SET, OPERATION, &[member("NextToken", &token)]).await;
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
async fn an_in_flight_operations_results_move_with_the_targets_not_with_a_read() {
    const STACK_SET: &str = "fleet-in-flight-results";
    const OPERATION: &str = "operation-in-flight-results";
    /// Far enough out that no read of this case can reach it: a target the operation has not
    /// finished in must stay in progress however many times it is listed.
    const NOT_DUE_FOR: Duration = Duration::from_secs(3_600);

    let (router, engine) = router_with_engine();
    {
        let now = SystemTime::now();
        let state = store(&engine);
        let mut held = state.lock();
        let mut stack_set = StoredStackSet::new(STACK_SET, REGION);
        let mut operation =
            StoredStackSetOperation::running(OPERATION, operation_action::CREATE, now);
        if let Some(pending) = operation.pending.as_mut() {
            pending.due_at = now + NOT_DUE_FOR;
        }
        stack_set.record_operation(operation);
        stack_set.record_instance(instance(
            ACCOUNT_A,
            REGION,
            OPERATION,
            detailed_status::RUNNING,
            None,
            None,
        ));
        held.record_stack_set(stack_set);
    }

    // A read never concludes an operation whose conclusion is not due, so two reads report
    // the same result for the same target.
    for pass in ["the first read", "the second read"] {
        let (status, body) = list_results(&router, STACK_SET, OPERATION, &[]).await;
        assert_eq!(
            status, 200,
            "case {pass:?}: expected 200, got {status} {body}"
        );
        let summary = summary_for(&body, ACCOUNT_A, REGION)
            .unwrap_or_else(|| panic!("case {pass:?}: expected that target, got {body}"));
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some(result_status::RUNNING),
            "case {pass:?}: a read must not advance a target's result, got {summary}"
        );
    }
}

/// `CreateStackInstances` for one product of accounts and Regions, answered with the
/// `OperationId` of the operation it runs as.
async fn create_stack_instances(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    for (index, account) in accounts.iter().enumerate() {
        members.push(member(&format!("Accounts.member.{}", index + 1), account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    call(router, "CreateStackInstances", &members).await
}

/// The `OperationId` a deployment call answered.
fn answered_operation_id(case: &str, body: &str) -> String {
    xml_text(body, "OperationId")
        .unwrap_or_else(|| panic!("case {case:?}: expected an OperationId, got {body}"))
}

#[tokio::test]
async fn a_concluded_operations_results_outlive_the_instances_it_removed() {
    const STACK_SET: &str = "fleet-lifecycle-results";
    let router = router();

    let (status, body) = call(
        &router,
        "CreateStackSet",
        &[
            member("StackSetName", STACK_SET),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );

    // Two targets, deployed as ONE `CREATE` operation.
    let (status, body) =
        create_stack_instances(&router, STACK_SET, &[ACCOUNT_A, ACCOUNT_B], &[REGION]).await;
    assert_eq!(
        status, 200,
        "expected CreateStackInstances to succeed, got {status} {body}"
    );
    let creation = answered_operation_id("the deployment", &body);
    let deployed: Vec<(String, String)> = vec![
        (ACCOUNT_A.to_string(), REGION.to_string()),
        (ACCOUNT_B.to_string(), REGION.to_string()),
    ];

    wait_for_the_transition().await;
    let (status, body) = list_results(&router, STACK_SET, &creation, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the deployment, concluded\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_targets(&body),
        deployed,
        "case \"the deployment, concluded\": the targets it deployed to, got {body}"
    );
    for (account, region) in &deployed {
        let summary = summary_for(&body, account, region).unwrap_or_else(|| {
            panic!("case \"the deployment, concluded\": expected {account}/{region}, got {body}")
        });
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some(result_status::SUCCEEDED),
            "case \"the deployment, concluded\": a deployment that completed there, got \
             {summary}"
        );
    }

    // The deletion of ONE of those targets, as its own `DELETE` operation: its instance
    // leaves the StackSet when that operation concludes.
    let (status, body) = call(
        &router,
        "DeleteStackInstances",
        &[
            member("StackSetName", STACK_SET),
            member("Accounts.member.1", ACCOUNT_A),
            member("Regions.member.1", REGION),
            member("RetainStacks", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteStackInstances to succeed, got {status} {body}"
    );
    let deletion = answered_operation_id("the deletion", &body);
    wait_for_the_transition().await;

    // The instance really is gone...
    let (status, body) = call(
        &router,
        "ListStackInstances",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected ListStackInstances to succeed, got {status} {body}"
    );
    assert_eq!(
        reported_targets(&body),
        vec![(ACCOUNT_B.to_string(), REGION.to_string())],
        "the deleted instance has left the StackSet, got {body}"
    );

    // ...and the deletion still reports what it did to the target it removed.
    let (status, body) = list_results(&router, STACK_SET, &deletion, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the deletion, concluded\": expected 200, got {status} {body}"
    );
    let summary = summary_for(&body, ACCOUNT_A, REGION).unwrap_or_else(|| {
        panic!("case \"the deletion, concluded\": expected the removed target, got {body}")
    });
    assert_eq!(
        xml_text(&summary, "Status").as_deref(),
        Some(result_status::SUCCEEDED),
        "case \"the deletion, concluded\": a deletion carried out there, got {summary}"
    );
    assert_eq!(
        reported_targets(&body),
        vec![(ACCOUNT_A.to_string(), REGION.to_string())],
        "case \"the deletion, concluded\": only the target it acted on, got {body}"
    );

    // And the earlier deployment still reports both of ITS targets, including the one the
    // deletion has since taken over and removed.
    let (status, body) = list_results(&router, STACK_SET, &creation, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the deployment, after the deletion\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_targets(&body),
        deployed,
        "case \"the deployment, after the deletion\": the targets it acted on, whatever ran \
         after it, got {body}"
    );
    for (account, region) in &deployed {
        let summary = summary_for(&body, account, region).unwrap_or_else(|| {
            panic!(
                "case \"the deployment, after the deletion\": expected {account}/{region}, got \
                 {body}"
            )
        });
        assert_eq!(
            xml_text(&summary, "Status").as_deref(),
            Some(result_status::SUCCEEDED),
            "case \"the deployment, after the deletion\": what the deployment did there, got \
             {summary}"
        );
    }
}
