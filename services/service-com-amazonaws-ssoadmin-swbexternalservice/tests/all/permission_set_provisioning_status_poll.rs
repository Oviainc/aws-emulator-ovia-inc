// SPDX-License-Identifier: CC0-1.0

//! `DescribePermissionSetProvisioningStatus` as the POLL of a provisioning workflow, end
//! to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the operation REPORTS the stored record of the permission-set
//! provisioning its `ProvisionPermissionSetRequestId` names under the instance the request
//! names — "the identifier that is provided by the ProvisionPermissionSet call to retrieve
//! the current status of the provisioning workflow" — so
//!
//! 1. the `Status` it answers is the one the CLOCK implies at the instant of the call: the
//!    non-terminal one until the workflow's completion falls due one span after the call
//!    that started it, the terminal one after, with every other member of the record
//!    unmoved;
//! 2. polling is a READ: however often a request id is asked for, and whether it resolves
//!    or not, a later caller sees exactly what it would have seen had nobody polled — no
//!    record is minted or consumed, no workflow is advanced and no copy lands early;
//! 3. a request id this instance tracks no provisioning under is a
//!    `ResourceNotFoundException` naming that id, whether it was never minted or was
//!    minted by another history (an account-assignment operation's handle is not a
//!    provisioning's);
//! 4. a request id outside the class the model declares for it (`UUId`, "length 36..36",
//!    `^\b[0-9a-f]{8}\b-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-\b[0-9a-f]{12}\b$`) is refused
//!    by the constraint walk as a `ValidationException` naming that member, while the
//!    published length itself is inside the class and reaches the lookup.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: both `ProvisionTargetType` values the model declares, the number of accounts
//! holding the permission set, the ends of the request id's own hex class, each side of
//! its published length, and every channel a request id can come from.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance. Where a test has to observe BOTH sides of a due
//! instant it moves that instant through the clock helper
//! `permission_set_provisioning.rs` owns — the clock's business, not the wire's — except
//! where the real span is itself the thing under test.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ssoadmin_swbexternalservice::state::PERMISSION_SET_PROVISIONING_SPAN;

use crate::permission_set_provisioning::hold_provisionings;
use crate::ssoadmin_permission_sets::{call, error_code, organization_engine, router_over, with_store};

/// `ProvisionTargetType`, as the model declares its two values
/// (`ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// `StatusValues`, as the model spells the two statuses a provisioning walks.
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `ProvisioningStatus` as the model's enum spells it
/// (`LATEST_PERMISSION_SET_NOT_PROVISIONED | LATEST_PERMISSION_SET_PROVISIONED`).
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// Accounts spanning `TargetId`'s class (`length 12..12`, `^\d{12}$`): both ends of the
/// 12-digit range and one in between.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_HIGH: &str = "999999999999";

/// The principal the assignments are granted to; `PrincipalId` takes an identity-store id,
/// which the model's pattern gives as a hyphenated UUID.
const PRINCIPAL: &str = "11111111-2222-3333-4444-555555555555";

/// Both ends of the request id's own hex class, and a value from inside it — each a
/// `UUId` the model admits that nothing was ever minted under.
const UNMINTED_LOW: &str = "00000000-0000-0000-0000-000000000000";
const UNMINTED_HIGH: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";
const UNMINTED_MIXED: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

/// The member path the constraint walk names a refused request id by: the lower-camel
/// spelling of `DescribePermissionSetProvisioningStatusRequest$ProvisionPermissionSetRequestId`.
const REQUEST_ID_MEMBER_PATH: &str = "provisionPermissionSetRequestId";

/// A router over an organization instance, that instance's ARN, and the engine behind it.
fn organization_fixture() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// Create a permission set over the wire and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {body}"))
        .to_string()
}

/// Grant one account assignment over the wire — the call that first provisions the
/// permission set into `account_id` — and return the `RequestId` it is tracked under.
async fn assign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    case: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": PRINCIPAL,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
    body["AccountAssignmentCreationStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the assignment is tracked by no id: {body}"))
        .to_string()
}

/// Change the permission set's definition, so the accounts holding it hold a copy of an
/// earlier definition and a provisioning has something to carry.
async fn change_definition(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "PutInlinePolicyToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutInlinePolicyToPermissionSet expected 200, got {status} {body}"
    );
}

/// Start a provisioning over the wire and return the status object it answered with.
async fn provision(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    target_type: &str,
    target_id: Option<&str>,
    case: &str,
) -> Value {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "TargetType": target_type,
    });
    if let Some(target_id) = target_id {
        request["TargetId"] = json!(target_id);
    }
    let (status, body) = call(router, "ProvisionPermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ProvisionPermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSetProvisioningStatus"].clone()
}

/// Poll one provisioning by the id the call answered with, as `(http status, body)`.
async fn poll(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribePermissionSetProvisioningStatus",
        &json!({
            "InstanceArn": instance_arn,
            "ProvisionPermissionSetRequestId": request_id,
        }),
    )
    .await
}

/// The `PermissionSetProvisioningStatus` a successful poll reports.
async fn poll_ok(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
    case: &str,
) -> Value {
    let (status, body) = poll(router, instance_arn, request_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the poll of {request_id} expected 200, got {status} {body}"
    );
    body["PermissionSetProvisioningStatus"].clone()
}

/// The `RequestId` of a provisioning status object.
fn request_id_of(status: &Value, case: &str) -> String {
    status["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the status carries no RequestId: {status}"))
        .to_string()
}

/// The instant the store holds for one provisioning's completion.
fn completion_due(
    engine: &Arc<engine::InMemoryEngine>,
    request_id: &str,
    case: &str,
) -> SystemTime {
    with_store(engine, |store| {
        store
            .permission_set_provisionings
            .iter()
            .find(|record| record.request_id == request_id)
            .unwrap_or_else(|| panic!("case {case:?}: nothing is tracked under {request_id}"))
            .succeeded_due
    })
}

/// Everything the store holds about every tracked provisioning: the handle, whether its
/// copy has landed, the definition it carries and the instant it falls due. Two equal
/// snapshots are a store nothing moved.
fn provisioning_snapshot(
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<(String, bool, String, SystemTime)> {
    with_store(engine, |store| {
        store
            .permission_set_provisionings
            .iter()
            .map(|record| {
                (
                    record.request_id.clone(),
                    record.applied,
                    record.revision.clone(),
                    record.succeeded_due,
                )
            })
            .collect()
    })
}

/// The accounts an enumeration reports the permission set as provisioned (or not) into,
/// sorted so a comparison is order-free.
async fn accounts_with_status(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    provisioning_status: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListAccountsForProvisionedPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "ProvisioningStatus": provisioning_status,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAccountsForProvisionedPermissionSet expected 200, got {status} {body}"
    );
    let mut accounts: Vec<String> = body["AccountIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: AccountIds must be a list, got {body}"))
        .iter()
        .map(|account| {
            account
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is not a string: {body}"))
                .to_string()
        })
        .collect();
    accounts.sort();
    accounts
}

/// The accounts the fixtures assign the permission set on, sorted.
fn fixture_accounts() -> Vec<String> {
    let mut accounts: Vec<String> = [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH]
        .iter()
        .map(|account| account.to_string())
        .collect();
    accounts.sort();
    accounts
}

/// RULE 1 — the status a poll reports is the one the clock implies: the workflow's
/// completion falls due one real span after the call that started it, so a caller polling
/// straight after the call still sees the non-terminal status, and a caller that waits the
/// span out sees the terminal one and the copy landed. Nothing here touches the store:
/// the only clock is the real one.
#[tokio::test]
async fn the_status_a_poll_reports_is_the_one_the_clock_implies() {
    // (the target type, the account the target names)
    let targets: Vec<(&str, Option<&str>)> = vec![
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_LOW)),
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_HIGH)),
        (TARGET_TYPE_ALL_PROVISIONED, None),
    ];

    for (target_type, target_id) in targets {
        let case = format!("polled {target_type} {target_id:?}");
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Polled").await;
        for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
            assign(&router, &instance_arn, &permission_set_arn, account_id, &case).await;
        }
        change_definition(&router, &instance_arn, &permission_set_arn, &case).await;

        let opened_at = SystemTime::now();
        let accepted = provision(
            &router,
            &instance_arn,
            &permission_set_arn,
            target_type,
            target_id,
            &case,
        )
        .await;
        let closed_at = SystemTime::now();
        let request_id = request_id_of(&accepted, &case);

        // The span is REAL and never zero: the completion falls due one span after the
        // call was accepted, so the transient status is observable.
        let due_at = completion_due(&engine, &request_id, &case);
        assert!(
            due_at >= opened_at + PERMISSION_SET_PROVISIONING_SPAN
                && due_at <= closed_at + PERMISSION_SET_PROVISIONING_SPAN,
            "case {case:?}: the completion must fall due one span after the call, got {due_at:?}"
        );

        // Straight after the call, on the real clock: still in flight, and the poll
        // answers the record the call reported.
        let in_flight = poll_ok(&router, &instance_arn, &request_id, &case).await;
        assert_eq!(
            in_flight, accepted,
            "case {case:?}: the poll must answer the record the call reported, got {in_flight}"
        );
        assert_eq!(
            in_flight["Status"].as_str(),
            Some(STATUS_IN_PROGRESS),
            "case {case:?}: a workflow whose completion is not due yet is in flight, got \
             {in_flight}"
        );

        // Past the due instant — waited out rather than moved — the SAME record reports
        // the terminal status, and nothing else about it moved.
        std::thread::sleep(PERMISSION_SET_PROVISIONING_SPAN + Duration::from_millis(250));
        let completed = poll_ok(&router, &instance_arn, &request_id, &case).await;
        assert_eq!(
            completed["Status"].as_str(),
            Some(STATUS_SUCCEEDED),
            "case {case:?}: a workflow whose completion is due has succeeded, got {completed}"
        );
        for member in ["RequestId", "PermissionSetArn", "AccountId", "CreatedDate"] {
            assert_eq!(
                completed[member], accepted[member],
                "case {case:?}: {member} must not move with the status, got {completed}"
            );
        }
        assert!(
            completed["FailureReason"].is_null(),
            "case {case:?}: nothing failed, so no FailureReason, got {completed}"
        );

        // The copy reached exactly the accounts the target named at that same instant.
        let expected_reached: Vec<String> = match target_id {
            Some(account_id) => vec![account_id.to_string()],
            None => fixture_accounts(),
        };
        let provisioned = accounts_with_status(
            &router,
            &instance_arn,
            &permission_set_arn,
            LATEST_PROVISIONED,
            &case,
        )
        .await;
        assert_eq!(
            provisioned, expected_reached,
            "case {case:?}: expected the copy in {expected_reached:?}, got {provisioned:?}"
        );
    }
}

/// RULE 2 — polling is a READ: with the completion held open, a request id answers the
/// same object however often it is asked, a request id that resolves to nothing stays
/// nothing, and neither poll mints a record, advances a workflow or lands a copy early.
#[tokio::test]
async fn polling_changes_nothing_a_later_caller_sees() {
    let targets: Vec<(&str, Option<&str>)> = vec![
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
        (TARGET_TYPE_ALL_PROVISIONED, None),
    ];

    for (target_type, target_id) in targets {
        let case = format!("repeatedly polled {target_type} {target_id:?}");
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Repeated").await;
        for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
            assign(&router, &instance_arn, &permission_set_arn, account_id, &case).await;
        }
        change_definition(&router, &instance_arn, &permission_set_arn, &case).await;
        let accepted = provision(
            &router,
            &instance_arn,
            &permission_set_arn,
            target_type,
            target_id,
            &case,
        )
        .await;
        let request_id = request_id_of(&accepted, &case);

        // The completion is held open, so every poll below is asked of the SAME instant's
        // worth of workflow: anything that moves was moved by the poll.
        hold_provisionings(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let before = provisioning_snapshot(&engine);
        let first = poll_ok(&router, &instance_arn, &request_id, &case).await;

        for attempt in 1..=5 {
            let again = poll_ok(&router, &instance_arn, &request_id, &case).await;
            assert_eq!(
                again, first,
                "case {case:?}: poll {attempt} must answer the same object, got {again}"
            );
            assert_eq!(
                again["Status"].as_str(),
                Some(STATUS_IN_PROGRESS),
                "case {case:?}: poll {attempt} must not advance the workflow, got {again}"
            );
            // A poll that resolves to nothing is just as much a read.
            let (status, body) = poll(&router, &instance_arn, UNMINTED_LOW).await;
            assert_eq!(
                status, 400,
                "case {case:?}: poll {attempt} of an unminted id expected 400, got {status} {body}"
            );
        }

        let after = provisioning_snapshot(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: polling must leave the tracked workflows alone, got {after:?} \
             after {before:?}"
        );

        // The copy has not landed on any account: it lands when the clock makes the
        // workflow due, not when somebody polls.
        let stale = accounts_with_status(
            &router,
            &instance_arn,
            &permission_set_arn,
            LATEST_NOT_PROVISIONED,
            &case,
        )
        .await;
        assert_eq!(
            stale,
            fixture_accounts(),
            "case {case:?}: expected every account still holding the earlier definition, \
             got {stale:?}"
        );
    }
}

/// RULE 3 — a request id this instance tracks no provisioning under is refused, naming the
/// id that resolved to nothing: over both ends of the id's hex class, a value from inside
/// it, and an id minted by the OTHER history this service keeps (an account-assignment
/// operation's handle, which answers a poll of its own and never this one).
///
/// The STATUS is the 400 this service answers every refusal of a request with, whatever
/// 4xx the error shape's `@httpError` declares, so the error CODE is what tells a missing
/// record from the other refusals.
#[tokio::test]
async fn a_request_id_no_provisioning_is_tracked_under_is_refused() {
    let case = "no provisioning tracked";
    let (router, instance_arn, _engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Untracked").await;
    let assignment_request_id =
        assign(&router, &instance_arn, &permission_set_arn, ACCOUNT_MID, case).await;
    let provisioning = provision(
        &router,
        &instance_arn,
        &permission_set_arn,
        TARGET_TYPE_ONE_ACCOUNT,
        Some(ACCOUNT_MID),
        case,
    )
    .await;
    let minted = request_id_of(&provisioning, case);

    // (case, the request id polled for)
    let rows: Vec<(String, String)> = vec![
        (
            "the lowest id of the hex class".to_string(),
            UNMINTED_LOW.to_string(),
        ),
        (
            "the highest id of the hex class".to_string(),
            UNMINTED_HIGH.to_string(),
        ),
        (
            "an id from inside the hex class".to_string(),
            UNMINTED_MIXED.to_string(),
        ),
        (
            "an assignment operation's handle".to_string(),
            assignment_request_id.clone(),
        ),
    ];

    for (label, request_id) in rows {
        assert_ne!(
            request_id, minted,
            "case {label:?}: this row must name an id no provisioning holds"
        );
        let (status, body) = poll(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for {request_id}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&request_id),
            "case {label:?}: the refusal must name the id that resolved to nothing, got {body}"
        );
    }

    // The id a provisioning IS tracked under still answers, so the rows above are refused
    // for the id they named and not for the fixture.
    let tracked = poll_ok(&router, &instance_arn, &minted, case).await;
    assert_eq!(
        tracked["RequestId"].as_str(),
        Some(minted.as_str()),
        "case {case:?}: the poll answers the id it was asked for, got {tracked}"
    );
}

/// RULE 4 — a request id outside the class the model declares for it is refused by the
/// constraint walk, naming that member; the published length is INSIDE the class, so an id
/// of exactly that length reaches the lookup and earns the missing-record refusal instead.
#[tokio::test]
async fn a_request_id_outside_its_modelled_class_is_refused_as_a_validation_error() {
    let case = "request id class";
    let (router, instance_arn, _engine) = organization_fixture();

    // (case, the request id, the error code expected). `UUId` is "length 36..36" with the
    // pattern `^\b[0-9a-f]{8}\b-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-\b[0-9a-f]{12}\b$`:
    // each side of the published length, the hex alphabet's bounds, and the length itself.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "one character below the published length",
            "0000000-0000-0000-0000-000000000000",
            "ValidationException",
        ),
        (
            "one character past the published length",
            "000000000-0000-0000-0000-000000000000",
            "ValidationException",
        ),
        (
            "the published length, outside the hex alphabet",
            "gggggggg-0000-0000-0000-000000000000",
            "ValidationException",
        ),
        (
            "the published length, in upper-case hex",
            "AAAAAAAA-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "ValidationException",
        ),
        (
            "the published length, without the separators",
            "000000000000000000000000000000000000",
            "ValidationException",
        ),
        ("no id at all", "", "ValidationException"),
        (
            "exactly the published length, inside the class",
            UNMINTED_MIXED,
            "ResourceNotFoundException",
        ),
    ];

    for (label, request_id, expected_code) in rows {
        let (status, body) = poll(&router, &instance_arn, request_id).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for {request_id:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {label:?}: expected {expected_code}, got {body}"
        );
        if expected_code == "ValidationException" {
            let message = body["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains(REQUEST_ID_MEMBER_PATH),
                "case {label:?}: the refusal must name {REQUEST_ID_MEMBER_PATH}, got {body}"
            );
        }
    }

    // A request the walk refuses reaches no handler, so nothing about the instance moved:
    // the surface still answers.
    let (status, body) = call(&router, "ListPermissionSets", &json!({"InstanceArn": instance_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the instance still serves reads, got {status} {body}"
    );
}
