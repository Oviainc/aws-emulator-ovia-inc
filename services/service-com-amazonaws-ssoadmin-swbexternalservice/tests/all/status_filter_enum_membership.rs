// SPDX-License-Identifier: CC0-1.0

//! The status-filtering enumerations of this service, driven end to end over the
//! `awsJson1_1` wire against ONE rule about the filter value they read.
//!
//! THE RULE: a status filter whose member the model types as an `enum` admits exactly the
//! values that enum declares — anything else is refused `ValidationException` (HTTP 400)
//! before the operation runs, and a declared value nothing is in selects no row, which is an
//! empty page rather than a refusal. The refusal is answered ONCE, by the walk of the
//! modelled constraints every request takes, so no enumeration can answer it differently
//! from its siblings.
//!
//! The five enumerations the rule covers are asserted TOGETHER, as a table whose rows span
//! both shapes the filter travels in — nested in `OperationStatusFilter$Status`
//! (`ListAccountAssignmentCreationStatus`, `ListAccountAssignmentDeletionStatus`,
//! `ListPermissionSetProvisioningStatus`) and as the request's own `ProvisioningStatus`
//! member (`ListAccountsForProvisionedPermissionSet`,
//! `ListPermissionSetsProvisionedToAccount`) — and both enums those members target
//! (`StatusValues`, `ProvisioningStatus`). The filter values span the ways a caller can miss
//! an enum value: the case, the separators, a suffix, surrounding space, a value of the
//! OTHER status enum of this same model, and the empty string; and, on the accepted side,
//! every value each enum declares.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints are exercised rather than bypassed. The
//! harness is the one `ssoadmin_permission_sets.rs` owns, and the calls run against an
//! ORGANIZATION instance because the permission-set surface is withdrawn on an account
//! instance. Where the test needs the tracked workflows in a settled status it moves their
//! stored due instants into the past — the clock's business, not the wire's — so the statuses
//! a filtered page is compared against cannot change under the comparison.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, organization_engine, router_over, with_store,
};

/// Every value the model's `StatusValues` enum declares — the enum
/// `OperationStatusFilter$Status` targets.
///
/// SOURCE: `smithy_query(query="shape",shape="StatusValues")` — `FAILED | IN_PROGRESS |
/// SUCCEEDED`.
const STATUS_VALUES: [&str; 3] = ["FAILED", "IN_PROGRESS", "SUCCEEDED"];

/// Every value the model's `ProvisioningStatus` enum declares — the enum the
/// `ProvisioningStatus` input member of the two provisioned-copy enumerations targets.
///
/// SOURCE: `smithy_query(query="shape",shape="ProvisioningStatus")` —
/// `LATEST_PERMISSION_SET_NOT_PROVISIONED | LATEST_PERMISSION_SET_PROVISIONED`.
const PROVISIONING_STATUS_VALUES: [&str; 2] = [
    "LATEST_PERMISSION_SET_NOT_PROVISIONED",
    "LATEST_PERMISSION_SET_PROVISIONED",
];

/// The account the assignments are granted on: `AccountId` is `length 12..12`, `^\d{12}$`.
const ACCOUNT: &str = "123456789012";

/// The principals the assignments are granted to; `PrincipalId` takes an identity-store id,
/// which the model's pattern gives as a hyphenated UUID. Two of them, so withdrawing one
/// assignment leaves the account still holding a copy of the permission set.
const PRINCIPAL_KEPT: &str = "11111111-2222-3333-4444-555555555555";
const PRINCIPAL_WITHDRAWN: &str = "66666666-7777-8888-9999-000000000000";

/// How the status filter travels in the request.
#[derive(Debug)]
enum FilterMember {
    /// Nested in the modelled `OperationStatusFilter` structure, as its `Status` member.
    FilterStatus,
    /// The request's own `ProvisioningStatus` member.
    ProvisioningStatus,
}

/// The enum a status-filter member targets — the two of them this model declares.
#[derive(Clone, Copy, Debug)]
enum TargetEnum {
    StatusValues,
    ProvisioningStatus,
}

impl TargetEnum {
    /// Every value this enum declares.
    fn values(self) -> &'static [&'static str] {
        match self {
            Self::StatusValues => &STATUS_VALUES,
            Self::ProvisioningStatus => &PROVISIONING_STATUS_VALUES,
        }
    }

    /// The OTHER status enum of this model, whose values are outside this one.
    fn other(self) -> Self {
        match self {
            Self::StatusValues => Self::ProvisioningStatus,
            Self::ProvisioningStatus => Self::StatusValues,
        }
    }
}

/// One status-filtering enumeration: the operation, where its filter travels, the enum that
/// member targets, the output member its rows arrive in, and the request members it needs
/// beside `InstanceArn`.
struct Enumeration {
    operation: &'static str,
    filter_member: FilterMember,
    target_enum: TargetEnum,
    rows_member: &'static str,
    subject: Value,
}

/// How many spellings [`miss_spellings`] builds for one declared value.
const MISS_SPELLINGS: usize = 4;

/// The ways a caller can MISS a declared enum value, each with the label it is asserted
/// under: the case, the separators, a suffix, and surrounding space.
fn miss_spellings(value: &str) -> [(&'static str, String); MISS_SPELLINGS] {
    [
        ("in lower case", value.to_lowercase()),
        ("without its separators", value.replace('_', "")),
        ("with a suffix", format!("{value}_V2")),
        ("surrounded by spaces", format!(" {value} ")),
    ]
}

impl Enumeration {
    /// The request a client sends, carrying `filter` when the caller filters by a status.
    fn request(&self, instance_arn: &str, filter: Option<&str>) -> Value {
        let mut request = json!({"InstanceArn": instance_arn});
        for (member, value) in self
            .subject
            .as_object()
            .expect("the subject members of an enumeration are an object")
        {
            request[member] = value.clone();
        }
        if let Some(filter) = filter {
            match self.filter_member {
                FilterMember::FilterStatus => request["Filter"] = json!({"Status": filter}),
                FilterMember::ProvisioningStatus => request["ProvisioningStatus"] = json!(filter),
            }
        }
        request
    }

    /// The values OUTSIDE the enum this enumeration's filter member targets, each with the
    /// case label it is asserted under: the ways a caller can miss an enum value, plus every
    /// value of the OTHER status enum of this model.
    fn outside_its_enum(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> =
            vec![("the empty string".to_string(), String::new())];
        for value in self.target_enum.other().values() {
            rows.push((
                format!("{value}, a value of this model's other status enum"),
                (*value).to_string(),
            ));
        }
        let accepted = self.target_enum.values();
        for value in accepted {
            for (miss, candidate) in miss_spellings(value) {
                rows.push((format!("{value} {miss}"), candidate));
            }
        }
        // A candidate that happens to BE a value the enum declares would assert the opposite
        // rule, so the table is checked against the enum it was built from: a declared value
        // with no separator to drop spells ITSELF, which is the only spelling that can.
        rows.retain(|(_, candidate)| !accepted.contains(&candidate.as_str()));
        let spells_itself = accepted.iter().filter(|value| !value.contains('_')).count();
        assert_eq!(
            rows.len(),
            1 + self.target_enum.other().values().len() + MISS_SPELLINGS * accepted.len()
                - spells_itself,
            "case {:?}: the table must hold the empty string, every value of the other enum \
             and every miss spelling of every value of this one, got {rows:?}",
            self.operation
        );
        rows
    }
}

/// The five enumerations under test, over the fixture's permission set.
fn enumerations(permission_set_arn: &str) -> Vec<Enumeration> {
    vec![
        Enumeration {
            operation: "ListAccountAssignmentCreationStatus",
            filter_member: FilterMember::FilterStatus,
            target_enum: TargetEnum::StatusValues,
            rows_member: "AccountAssignmentsCreationStatus",
            subject: json!({}),
        },
        Enumeration {
            operation: "ListAccountAssignmentDeletionStatus",
            filter_member: FilterMember::FilterStatus,
            target_enum: TargetEnum::StatusValues,
            rows_member: "AccountAssignmentsDeletionStatus",
            subject: json!({}),
        },
        Enumeration {
            operation: "ListPermissionSetProvisioningStatus",
            filter_member: FilterMember::FilterStatus,
            target_enum: TargetEnum::StatusValues,
            rows_member: "PermissionSetsProvisioningStatus",
            subject: json!({}),
        },
        Enumeration {
            operation: "ListAccountsForProvisionedPermissionSet",
            filter_member: FilterMember::ProvisioningStatus,
            target_enum: TargetEnum::ProvisioningStatus,
            rows_member: "AccountIds",
            subject: json!({"PermissionSetArn": permission_set_arn}),
        },
        Enumeration {
            operation: "ListPermissionSetsProvisionedToAccount",
            filter_member: FilterMember::ProvisioningStatus,
            target_enum: TargetEnum::ProvisioningStatus,
            rows_member: "PermissionSets",
            subject: json!({"AccountId": ACCOUNT}),
        },
    ]
}

/// A router over an organization instance that has something for every one of the five
/// enumerations to report: a permission set provisioned into one account by a granted
/// assignment, a second assignment granted and withdrawn again (the deletion history), and a
/// provisioning of the permission set into that account (the provisioning history).
///
/// Returns the router, the instance ARN, the permission set ARN and the engine behind them.
async fn fixture() -> (
    MultiServiceRouter,
    String,
    String,
    Arc<engine::InMemoryEngine>,
) {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));

    let (status, body) = call(
        &router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": "FilteredSet"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet: expected 200, got {status} {body}"
    );
    let permission_set_arn = body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet minted no ARN: {body}"))
        .to_string();

    for principal_id in [PRINCIPAL_KEPT, PRINCIPAL_WITHDRAWN] {
        let (status, body) = call(
            &router,
            "CreateAccountAssignment",
            &assignment(&instance_arn, &permission_set_arn, principal_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateAccountAssignment for {principal_id}: expected 200, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "DeleteAccountAssignment",
        &assignment(&instance_arn, &permission_set_arn, PRINCIPAL_WITHDRAWN),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAccountAssignment: expected 200, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "ProvisionPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "TargetType": "AWS_ACCOUNT",
            "TargetId": ACCOUNT,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ProvisionPermissionSet: expected 200, got {status} {body}"
    );

    settle_tracked_workflows(&engine);
    (router, instance_arn, permission_set_arn, engine)
}

/// One account assignment, as a client names it on the grant and on the withdrawal.
fn assignment(instance_arn: &str, permission_set_arn: &str, principal_id: &str) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "TargetId": ACCOUNT,
        "TargetType": "AWS_ACCOUNT",
        "PermissionSetArn": permission_set_arn,
        "PrincipalType": "USER",
        "PrincipalId": principal_id,
    })
}

/// Move the stored due instant of every tracked workflow into the past, so each one is in the
/// status the clock owes it at a TERMINAL instant and the statuses a filtered page is
/// compared against cannot change between two calls of the comparison.
///
/// Only the clock's instants move; each status is still derived by the service itself,
/// exactly as in production.
fn settle_tracked_workflows(engine: &Arc<engine::InMemoryEngine>) {
    let settled = SystemTime::now() - Duration::from_secs(60);
    with_store(engine, |store| {
        for record in store.assignment_operations.values_mut() {
            record.succeeded_due = settled;
        }
        for record in &mut store.assignment_deletions {
            record.succeeded_due = settled;
        }
        for record in &mut store.permission_set_provisionings {
            record.succeeded_due = settled;
        }
    });
}

/// The rows one enumeration answers, as the JSON texts of its items in the order it reported
/// them. Asserts the call succeeded and carried its list member.
async fn rows(
    router: &MultiServiceRouter,
    enumeration: &Enumeration,
    instance_arn: &str,
    filter: Option<&str>,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        enumeration.operation,
        &enumeration.request(instance_arn, filter),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} {body}"
    );
    body[enumeration.rows_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} list, got {body}",
                enumeration.rows_member
            )
        })
        .iter()
        .map(|item| item.to_string())
        .collect()
}

/// THE RULE, refusing half — a status filter outside the enum its member targets is refused
/// `ValidationException` by every one of the five enumerations, and the refused request
/// reaches no handler, so every page each of them answers afterwards is the page it answered
/// before.
#[tokio::test]
async fn a_status_filter_outside_the_modelled_enum_is_refused_by_every_enumeration() {
    let (router, instance_arn, permission_set_arn, _engine) = fixture().await;
    let enumerations = enumerations(&permission_set_arn);

    let mut before: Vec<Vec<String>> = Vec::new();
    for enumeration in &enumerations {
        let case = format!("{} unfiltered, before the refusals", enumeration.operation);
        let reported = rows(&router, enumeration, &instance_arn, None, &case).await;
        assert!(
            !reported.is_empty(),
            "case {case:?}: the fixture must give this enumeration something to report, got \
             {reported:?}"
        );
        before.push(reported);
    }

    for enumeration in &enumerations {
        for (miss, filter) in enumeration.outside_its_enum() {
            let case = format!("{} filtered by {miss}", enumeration.operation);
            let (status, body) = call(
                &router,
                enumeration.operation,
                &enumeration.request(&instance_arn, Some(&filter)),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: {filter:?} is outside the enum, so expected HTTP 400, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }

    for (enumeration, expected) in enumerations.iter().zip(before) {
        let case = format!("{} unfiltered, after the refusals", enumeration.operation);
        let reported = rows(&router, enumeration, &instance_arn, None, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: a refused filter reached no handler, so expected {expected:?}, got \
             {reported:?}"
        );
    }
}

/// THE RULE, answering half — every value the enum declares is ANSWERED rather than refused,
/// and the pages those values answer partition the unfiltered page: each row appears under
/// exactly the one value it is in, and a value nothing is in answers an empty page.
#[tokio::test]
async fn every_value_the_modelled_enum_declares_is_answered_and_partitions_the_page() {
    let (router, instance_arn, permission_set_arn, _engine) = fixture().await;

    for enumeration in &enumerations(&permission_set_arn) {
        let unfiltered_case = format!("{} unfiltered", enumeration.operation);
        let unfiltered = rows(&router, enumeration, &instance_arn, None, &unfiltered_case).await;

        let mut selected: Vec<String> = Vec::new();
        let mut empty_values: Vec<&str> = Vec::new();
        for value in enumeration.target_enum.values() {
            let case = format!("{} filtered by {value}", enumeration.operation);
            let reported = rows(&router, enumeration, &instance_arn, Some(value), &case).await;
            for row in &reported {
                assert!(
                    unfiltered.contains(row),
                    "case {case:?}: a filtered page may only hold rows the unfiltered page \
                     holds, got {row}"
                );
            }
            if reported.is_empty() {
                empty_values.push(value);
            }
            selected.extend(reported);
        }

        let mut selected_sorted = selected.clone();
        selected_sorted.sort();
        let mut unfiltered_sorted = unfiltered.clone();
        unfiltered_sorted.sort();
        assert_eq!(
            selected_sorted, unfiltered_sorted,
            "case {unfiltered_case:?}: the values the enum declares must partition the \
             unfiltered page, expected {unfiltered_sorted:?}, got {selected_sorted:?}"
        );
        assert!(
            !empty_values.is_empty(),
            "case {unfiltered_case:?}: a value no row is in must answer an empty page rather \
             than refuse, and this fixture leaves at least one of {:?} unused, got none empty",
            enumeration.target_enum.values()
        );
    }
}
