// SPDX-License-Identifier: CC0-1.0

//! `ProvisionPermissionSet` and `DescribePermissionSetProvisioningStatus`, end to end over
//! the `awsJson1_1` wire.
//!
//! THE RULE under test: provisioning a permission set COPIES the permission set's
//! definition, as it stands when the call is accepted, into the accounts the request's
//! target names — the one account an `AWS_ACCOUNT` target names, or every account that
//! already holds the permission set for an `ALL_PROVISIONED_ACCOUNTS` one — so each of
//! those accounts then holds the permission set's current definition and the accounts
//! outside the target keep the copy they had. The work is ASYNCHRONOUS: the call answers
//! with the tracking record of the workflow it started (a minted `RequestId`, the
//! non-terminal `IN_PROGRESS` status and the date it was accepted), that record is stored
//! under its `RequestId` for `DescribePermissionSetProvisioningStatus` to read ("The
//! identifier that is provided by the ProvisionPermissionSet call to retrieve the current
//! status of the provisioning workflow"), and both the terminal `SUCCEEDED` status and the
//! copy itself fall due one span after the call.
//!
//! A target that resolves to nothing is refused rather than answered: an `InstanceArn` or
//! `PermissionSetArn` nothing is held under is a `ResourceNotFoundException`, a target of
//! one account that names no account is a `ValidationException`, a `RequestId` this
//! instance tracks no provisioning under is a `ResourceNotFoundException`, and the whole
//! permission-set surface stays withdrawn on an account instance.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: both `ProvisionTargetType` values the model declares, the account ids at both
//! ends of `TargetId`'s own `^\d{12}$` class, the number of accounts holding the
//! permission set, and every KIND of change to a permission set's definition the
//! implemented API can make (an AWS managed policy, a customer managed policy reference,
//! an inline policy, a permissions boundary).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, is_uuid, organization_engine, router_over,
    with_store, UNKNOWN_INSTANCE_ARN, UNKNOWN_PERMISSION_SET_ARN,
};

/// `ProvisioningStatus::LATEST_PERMISSION_SET_PROVISIONED`, as the model's
/// `ProvisioningStatus` enum (`LATEST_PERMISSION_SET_NOT_PROVISIONED |
/// LATEST_PERMISSION_SET_PROVISIONED`) spells it.
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";

/// `ProvisioningStatus::LATEST_PERMISSION_SET_NOT_PROVISIONED`, from the same enum.
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// The two values `ProvisionTargetType` accepts on the wire, as the model declares them
/// (`smithy_query(query="operation",operation="ProvisionPermissionSet")`: `TargetType`
/// targets `ProvisionTargetType`, one of `ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// `StatusValues` as the model spells the two statuses a provisioning walks.
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// Account ids spanning `TargetId`'s own class (`length 12..12`, `^\d{12}$`): both ends of
/// the 12-digit range and one in between.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_HIGH: &str = "999999999999";

/// An account the fixtures never assign anything on, so the permission set is not
/// provisioned there.
const ACCOUNT_UNASSIGNED: &str = "555555555555";

/// Principal ids the assignments are granted to; `PrincipalId` takes the identity-store
/// user or group id, which the model's pattern gives as a hyphenated UUID.
const PRINCIPAL_A: &str = "11111111-2222-3333-4444-555555555555";

/// A pattern-valid `UUId` ("length 36..36",
/// `^\b[0-9a-f]{8}\b-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-\b[0-9a-f]{12}\b$`) nothing was
/// ever minted under, used to pin the not-found rule of the describe.
const UNMINTED_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A router over an organization instance, that instance's ARN, and the engine behind it.
fn organization_fixture() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// Move the stored completion instant of every provisioning the store holds to `due_at`,
/// so both sides of that instant can be observed without the test sleeping.
///
/// The transition itself — the status reaching `SUCCEEDED` and the copy reaching the target
/// accounts — is still applied by the service's own `materialize_due`, exactly as in
/// production: this moves only the CLOCK's instant, which is the clock's business and not
/// the wire's.
pub(crate) fn hold_provisionings(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for record in &mut store.permission_set_provisionings {
            record.succeeded_due = due_at;
        }
    });
}

/// An instant far enough ahead that every held provisioning is still in flight.
fn in_flight() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// An instant far enough behind that every held provisioning has completed.
fn completed() -> SystemTime {
    SystemTime::now() - Duration::from_secs(60)
}

/// Create a permission set over the wire and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {created}"
    );
    created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {created}"))
        .to_string()
}

/// Grant one account assignment over the wire, which is what first PROVISIONS the
/// permission set into `account_id`.
async fn assign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "CreateAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": PRINCIPAL_A,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Call `ProvisionPermissionSet`, with `TargetId` only when the row carries one.
async fn provision(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    target_type: &str,
    target_id: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "TargetType": target_type,
    });
    if let Some(target_id) = target_id {
        request["TargetId"] = json!(target_id);
    }
    call(router, "ProvisionPermissionSet", &request).await
}

/// The provisioning status a successful `ProvisionPermissionSet` answers with.
async fn provision_ok(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    target_type: &str,
    target_id: Option<&str>,
    case: &str,
) -> Value {
    let (status, body) =
        provision(router, instance_arn, permission_set_arn, target_type, target_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ProvisionPermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSetProvisioningStatus"].clone()
}

/// Read one provisioning's status back by the `RequestId` the provisioning answered with.
async fn describe_status(
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

/// The `RequestId` of a provisioning status object.
fn request_id_of(status: &Value, case: &str) -> String {
    status["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the status carries no RequestId: {status}"))
        .to_string()
}

/// Every account the permission set is provisioned into under `provisioning_status`, in
/// the order the enumeration reports them.
async fn list_accounts(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    provisioning_status: Option<&str>,
    case: &str,
) -> Vec<String> {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    if let Some(provisioning_status) = provisioning_status {
        request["ProvisioningStatus"] = json!(provisioning_status);
    }
    let (status, body) = call(router, "ListAccountsForProvisionedPermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAccountsForProvisionedPermissionSet expected 200, got {status} {body}"
    );
    body["AccountIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: AccountIds must be a list, got {body}"))
        .iter()
        .map(|account| {
            account
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is not a string: {body}"))
                .to_string()
        })
        .collect()
}

/// Assert an enumeration reported exactly `expected`, whatever order either is in.
fn assert_accounts(listed: &[String], expected: &[&str], case: &str) {
    let mut listed: Vec<String> = listed.to_vec();
    listed.sort();
    let mut expected: Vec<String> = expected.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case {case:?}: expected the accounts {expected:?}, got {listed:?}"
    );
}

/// One way of changing a permission set's definition, as the request that makes it.
struct ChangeCase {
    label: &'static str,
    operation: &'static str,
    /// The members the change carries beside `InstanceArn` and `PermissionSetArn`.
    body: Value,
}

/// Every kind of change to a permission set's definition the implemented API can make:
/// attaching an AWS managed policy, attaching a customer managed policy reference, putting
/// an inline policy and putting a permissions boundary. Each of them leaves the copy a
/// provisioned account holds behind, so each of them is a definition a later provisioning
/// has to carry into those accounts.
fn change_cases() -> Vec<ChangeCase> {
    vec![
        ChangeCase {
            label: "an AWS managed policy attached",
            operation: "AttachManagedPolicyToPermissionSet",
            body: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess"}),
        },
        ChangeCase {
            label: "a customer managed policy reference attached",
            operation: "AttachCustomerManagedPolicyReferenceToPermissionSet",
            body: json!({
                "CustomerManagedPolicyReference": {"Name": "ReadOnlyPolicy", "Path": "/"}
            }),
        },
        ChangeCase {
            label: "an inline policy put",
            operation: "PutInlinePolicyToPermissionSet",
            body: json!({
                "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
            }),
        },
        ChangeCase {
            label: "a permissions boundary put",
            operation: "PutPermissionsBoundaryToPermissionSet",
            body: json!({
                "PermissionsBoundary": {
                    "ManagedPolicyArn": "arn:aws:iam::aws:policy/PowerUserAccess"
                }
            }),
        },
        ChangeCase {
            label: "the permission set updated",
            operation: "UpdatePermissionSet",
            body: json!({"Description": "Reprovisioned", "SessionDuration": "PT4H"}),
        },
    ]
}

/// Apply one change to a permission set over the wire.
async fn apply_change(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    change: &ChangeCase,
    case: &str,
) {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    for (member, value) in change.body.as_object().expect("the change carries members") {
        request[member] = value.clone();
    }
    let (status, body) = call(router, change.operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} expected 200, got {status} {body}",
        change.operation
    );
}

/// THE RULE: a provisioning carries the permission set's CURRENT definition into the
/// accounts its target names, and into no others — asserted over both target types and
/// over every kind of change to a permission set the API can make.
#[tokio::test]
async fn provisioning_copies_the_current_definition_into_the_accounts_the_target_names() {
    // (the target type, the account the target names, the accounts the copy reaches)
    let targets: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        // One account out of the three that hold the permission set.
        (
            TARGET_TYPE_ONE_ACCOUNT,
            Some(ACCOUNT_MID),
            vec![ACCOUNT_MID],
        ),
        // The lowest and the highest account of `TargetId`'s 12-digit class.
        (
            TARGET_TYPE_ONE_ACCOUNT,
            Some(ACCOUNT_LOW),
            vec![ACCOUNT_LOW],
        ),
        (
            TARGET_TYPE_ONE_ACCOUNT,
            Some(ACCOUNT_HIGH),
            vec![ACCOUNT_HIGH],
        ),
        // Every account the permission set is already provisioned into.
        (
            TARGET_TYPE_ALL_PROVISIONED,
            None,
            vec![ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH],
        ),
    ];
    let provisioned = [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH];

    for change in change_cases() {
        for (target_type, target_id, reached) in &targets {
            let case = format!("{} provisioned to {target_type} {target_id:?}", change.label);
            let (router, instance_arn, engine) = organization_fixture();
            let permission_set_arn =
                create_permission_set(&router, &instance_arn, "Provisioned").await;
            for account_id in provisioned {
                assign(&router, &instance_arn, &permission_set_arn, account_id, &case).await;
            }
            // The change leaves every account holding a copy of an earlier definition.
            apply_change(&router, &instance_arn, &permission_set_arn, &change, &case).await;
            let stale = list_accounts(
                &router,
                &instance_arn,
                &permission_set_arn,
                Some(LATEST_NOT_PROVISIONED),
                &case,
            )
            .await;
            assert_accounts(&stale, &provisioned, &format!("{case}: before provisioning"));

            let status = provision_ok(
                &router,
                &instance_arn,
                &permission_set_arn,
                target_type,
                *target_id,
                &case,
            )
            .await;
            assert_eq!(
                status["Status"].as_str(),
                Some(STATUS_IN_PROGRESS),
                "case {case:?}: a workflow just accepted is in flight, got {status}"
            );

            // While the workflow is in flight the copy has not landed: every account still
            // holds the definition it held before the call.
            hold_provisionings(&engine, in_flight());
            let latest = list_accounts(
                &router,
                &instance_arn,
                &permission_set_arn,
                Some(LATEST_PROVISIONED),
                &case,
            )
            .await;
            assert_accounts(&latest, &[], &format!("{case}: in flight"));

            // Once the clock makes the workflow due, the copy has reached exactly the
            // accounts the target named.
            hold_provisionings(&engine, completed());
            let latest = list_accounts(
                &router,
                &instance_arn,
                &permission_set_arn,
                Some(LATEST_PROVISIONED),
                &case,
            )
            .await;
            assert_accounts(&latest, reached, &format!("{case}: completed, latest"));
            let untouched: Vec<&str> = provisioned
                .iter()
                .copied()
                .filter(|account| !reached.contains(account))
                .collect();
            let stale = list_accounts(
                &router,
                &instance_arn,
                &permission_set_arn,
                Some(LATEST_NOT_PROVISIONED),
                &case,
            )
            .await;
            assert_accounts(&stale, &untouched, &format!("{case}: completed, stale"));
            // The accounts holding the permission set are the same ones either way: a
            // provisioning refreshes copies, it neither adds nor removes an account.
            let all = list_accounts(&router, &instance_arn, &permission_set_arn, None, &case).await;
            assert_accounts(&all, &provisioned, &format!("{case}: completed, unfiltered"));
        }
    }
}

/// A provisioning of a permission set the target account does not hold has no copy to
/// refresh there: an account holds a permission set through the assignments granted on it,
/// so the call is accepted and tracked, and the accounts reported as holding the permission
/// set are unchanged.
#[tokio::test]
async fn provisioning_an_account_that_holds_no_assignment_adds_no_account() {
    let case = "an unassigned target account";
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Unassigned").await;
    assign(&router, &instance_arn, &permission_set_arn, ACCOUNT_MID, case).await;

    let status = provision_ok(
        &router,
        &instance_arn,
        &permission_set_arn,
        TARGET_TYPE_ONE_ACCOUNT,
        Some(ACCOUNT_UNASSIGNED),
        case,
    )
    .await;
    let request_id = request_id_of(&status, case);
    hold_provisionings(&engine, completed());

    let (described_status, described) = describe_status(&router, &instance_arn, &request_id).await;
    assert_eq!(
        described_status, 200,
        "case {case:?}: the describe expected 200, got {described_status} {described}"
    );
    assert_eq!(
        described["PermissionSetProvisioningStatus"]["Status"].as_str(),
        Some(STATUS_SUCCEEDED),
        "case {case:?}: a completed workflow is SUCCEEDED, got {described}"
    );
    let all = list_accounts(&router, &instance_arn, &permission_set_arn, None, case).await;
    assert_accounts(&all, &[ACCOUNT_MID], &format!("{case}: unfiltered"));
}

/// THE TRACKING RULE: the call answers the record of the workflow it started, stored under
/// a freshly minted `RequestId`, and the describe of that id answers the same record —
/// reporting the terminal status once the clock has made the workflow due. Asserted over
/// both target types and over the ends of `TargetId`'s class.
#[tokio::test]
async fn the_call_answers_a_tracking_record_the_describe_reads_back() {
    let targets: Vec<(&str, Option<&str>)> = vec![
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_LOW)),
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_MID)),
        (TARGET_TYPE_ONE_ACCOUNT, Some(ACCOUNT_HIGH)),
        (TARGET_TYPE_ALL_PROVISIONED, None),
    ];

    for (target_type, target_id) in targets {
        let case = format!("tracked {target_type} {target_id:?}");
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Tracked").await;
        for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
            assign(&router, &instance_arn, &permission_set_arn, account_id, &case).await;
        }

        let status = provision_ok(
            &router,
            &instance_arn,
            &permission_set_arn,
            target_type,
            target_id,
            &case,
        )
        .await;
        let request_id = request_id_of(&status, &case);
        assert!(
            is_uuid(&request_id),
            "case {case:?}: RequestId must be a hyphenated UUID, got {request_id:?}"
        );
        assert_eq!(
            status["Status"].as_str(),
            Some(STATUS_IN_PROGRESS),
            "case {case:?}: a workflow just accepted is in flight, got {status}"
        );
        assert_eq!(
            status["PermissionSetArn"].as_str(),
            Some(permission_set_arn.as_str()),
            "case {case:?}: expected the permission set provisioned, got {status}"
        );
        assert_eq!(
            status["AccountId"].as_str(),
            target_id,
            "case {case:?}: AccountId is the account the target named, got {status}"
        );
        assert!(
            status["CreatedDate"].is_number(),
            "case {case:?}: expected the date the workflow was accepted, got {status}"
        );
        assert!(
            status["FailureReason"].is_null(),
            "case {case:?}: nothing failed, so no FailureReason, got {status}"
        );

        // The describe answers the record the call stored, and answers it the same way
        // twice: reading a workflow's status neither advances it nor changes it.
        hold_provisionings(&engine, in_flight());
        let (described_status, first) = describe_status(&router, &instance_arn, &request_id).await;
        assert_eq!(
            described_status, 200,
            "case {case:?}: the describe expected 200, got {described_status} {first}"
        );
        assert_eq!(
            first["PermissionSetProvisioningStatus"], status,
            "case {case:?}: the describe must answer the record the call reported, got {first}"
        );
        let (_, again) = describe_status(&router, &instance_arn, &request_id).await;
        assert_eq!(
            again, first,
            "case {case:?}: a report must answer the same twice, got {first} then {again}"
        );

        // Once the workflow is due, the SAME record reports the terminal status, and
        // nothing else about it moved.
        hold_provisionings(&engine, completed());
        let (_, finished) = describe_status(&router, &instance_arn, &request_id).await;
        let finished = finished["PermissionSetProvisioningStatus"].clone();
        assert_eq!(
            finished["Status"].as_str(),
            Some(STATUS_SUCCEEDED),
            "case {case:?}: a completed workflow is SUCCEEDED, got {finished}"
        );
        for member in ["RequestId", "PermissionSetArn", "AccountId", "CreatedDate"] {
            assert_eq!(
                finished[member], status[member],
                "case {case:?}: {member} must not move, got {finished}"
            );
        }

        // Each call starts its OWN workflow, tracked under its own id: a second
        // provisioning is a second record, and the first one is still readable.
        let second = provision_ok(
            &router,
            &instance_arn,
            &permission_set_arn,
            target_type,
            target_id,
            &case,
        )
        .await;
        let second_id = request_id_of(&second, &case);
        assert_ne!(
            second_id, request_id,
            "case {case:?}: each provisioning mints its own RequestId, got {second_id} twice"
        );
        let (status_code, still_there) = describe_status(&router, &instance_arn, &request_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the first workflow is still tracked, got {status_code} {still_there}"
        );
        let (status_code, second_described) =
            describe_status(&router, &instance_arn, &second_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the second workflow is tracked, got {status_code} {second_described}"
        );
        assert_eq!(
            second_described["PermissionSetProvisioningStatus"]["RequestId"].as_str(),
            Some(second_id.as_str()),
            "case {case:?}: the describe answers the id it was asked for, got {second_described}"
        );
    }
}

/// A target or a request id that resolves to nothing is REFUSED, each with the error its
/// kind of failure earns, and never with a 200 or the scaffold's 501.
///
/// The STATUS is the 400 this service answers every refusal of a request with, whatever
/// 4xx the error shape's `@httpError` declares (`crate::scaffolded::modeled_error_status`),
/// so the error CODE is what tells a missing resource from the other refusals.
#[tokio::test]
async fn a_target_or_request_id_that_resolves_to_nothing_is_refused() {
    let (router, instance_arn, _engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Refused").await;
    assign(
        &router,
        &instance_arn,
        &permission_set_arn,
        ACCOUNT_MID,
        "refused",
    )
    .await;
    let status = provision_ok(
        &router,
        &instance_arn,
        &permission_set_arn,
        TARGET_TYPE_ONE_ACCOUNT,
        Some(ACCOUNT_MID),
        "refused",
    )
    .await;
    let minted_request_id = request_id_of(&status, "refused");

    // (case, operation, request, expected status, expected error code)
    let rows: Vec<(String, &str, Value, u16, &str)> = vec![
        (
            "an instance nothing is held under".to_string(),
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": UNKNOWN_INSTANCE_ARN,
                "PermissionSetArn": permission_set_arn,
                "TargetId": ACCOUNT_MID,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a permission set nothing is held under".to_string(),
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "TargetId": ACCOUNT_MID,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a target of one account that names none".to_string(),
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            }),
            400,
            "ValidationException",
        ),
        (
            "a describe under an instance nothing is held under".to_string(),
            "DescribePermissionSetProvisioningStatus",
            json!({
                "InstanceArn": UNKNOWN_INSTANCE_ARN,
                "ProvisionPermissionSetRequestId": minted_request_id,
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a describe of a request id nothing was minted under".to_string(),
            "DescribePermissionSetProvisioningStatus",
            json!({
                "InstanceArn": instance_arn,
                "ProvisionPermissionSetRequestId": UNMINTED_REQUEST_ID,
            }),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A provisioning status belongs to the permission set it reports on: once that
    // permission set is gone, the instance tracks no provisioning under its id.
    let (deleted, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        deleted, 200,
        "DeletePermissionSet expected 200, got {deleted} {body}"
    );
    let (status, body) = describe_status(&router, &instance_arn, &minted_request_id).await;
    assert_eq!(
        status, 400,
        "a provisioning of a deleted permission set expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a provisioning of a deleted permission set expected ResourceNotFoundException, got {body}"
    );
}

/// The permission-set surface stays WITHDRAWN on an account instance: both operations are
/// refused `ValidationException` (HTTP 400) there, which is the restriction and no longer
/// the scaffold's 501.
#[tokio::test]
async fn both_operations_are_withdrawn_on_an_account_instance() {
    let (router, instance_arn) = account_instance_router().await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a provisioning of one account",
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "TargetId": ACCOUNT_MID,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            }),
        ),
        (
            "a provisioning of every provisioned account",
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
                "TargetType": TARGET_TYPE_ALL_PROVISIONED,
            }),
        ),
        (
            "a describe of a provisioning",
            "DescribePermissionSetProvisioningStatus",
            json!({
                "InstanceArn": instance_arn,
                "ProvisionPermissionSetRequestId": UNMINTED_REQUEST_ID,
            }),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 on an account instance, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}
