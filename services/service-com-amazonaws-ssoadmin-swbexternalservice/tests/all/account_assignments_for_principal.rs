// SPDX-License-Identifier: CC0-1.0

//! `ListAccountAssignmentsForPrincipal` end to end over the `awsJson1_1` wire — the
//! operation the stub issue reported as an unimplemented scaffold.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. the enumeration reports EXACTLY the stored account assignments of the principal the
//!    request names, one item per stored row, each carrying the four modelled members of an
//!    `AccountAssignmentForPrincipal` and no other, in one stable (account, permission set)
//!    order whatever order the assignments were granted in — and a principal that holds
//!    none is an empty list, not a refusal;
//! 2. the principal is the whole (type, id) pair: the same id under the other
//!    `PrincipalType` is a DIFFERENT principal and its assignments are never reported;
//! 3. `ListAccountAssignmentsFilter$AccountId` NARROWS the enumeration to the assignments
//!    on that one account, an omitted filter narrows nothing, and an account the principal
//!    holds nothing on is an empty list (an account is not a resource of this service);
//! 4. an assignment belongs to the instance it was granted under, so no enumeration of one
//!    instance reports another instance's assignments;
//! 5. it TRACKS the store: a grant adds exactly its row and a withdrawal removes exactly
//!    its row, and it is a REPORT — repeating it answers identically and changes nothing a
//!    later caller sees;
//! 6. it pages: for every `MaxResults` the model's `range(min: 1, max: 100)` admits,
//!    walking the pages yields each stored row exactly once in that same order, no page
//!    carries more than the page size asked for, and the last page carries no `NextToken`;
//!    one past each published bound is refused;
//! 7. an `InstanceArn` this account holds no instance for is refused
//!    `ResourceNotFoundException`, and the whole account-assignment surface stays withdrawn
//!    on an ACCOUNT instance (`ValidationException`) — never an empty list and never the
//!    scaffold's HTTP 501;
//! 8. the `NextToken` is this service's own cursor: one it did not issue for THIS
//!    enumeration (another principal, another filter, another instance, or no token of this
//!    service at all) is refused `ValidationException`, and resuming from one it did issue
//!    yields exactly the rows that sort after the key it carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints, the
//! `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than bypassed.
//! The harness is the one `ssoadmin_permission_sets.rs` owns, and the calls run against an
//! ORGANIZATION instance because the account-assignment surface is withdrawn on an account
//! instance (asserted here too).

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_engine, router_over, with_store,
    TEST_REGION, UNKNOWN_INSTANCE_ARN,
};

/// Account ids spanning `AccountId`'s own class (`length 12..12`, `^\d{12}$`): both ends of
/// the 12-digit range and one in between.
///
/// SOURCE: `smithy_query(query="shape",shape="AccountAssignmentForPrincipal",
/// model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`.
const ACCOUNTS: [&str; 3] = ["000000000000", "123456789012", "999999999999"];

/// A pattern-valid `AccountId` no assignment of the table is granted on, for the rule that
/// a filter selecting no row is an empty page rather than a refusal.
const UNUSED_ACCOUNT: &str = "555555555555";

/// How many permission sets the table grants assignments through, under the first instance.
const PERMISSION_SETS: usize = 2;

/// The id of the SECOND instance the instance-scoping rows are asserted against:
/// `ssoins-` + 16 of `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const SECOND_INSTANCE_ID: &str = "ssoins-3c2b1a0998877665";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const SECOND_IDENTITY_STORE_ID: &str = "d-3c2b1a0998";

/// Every member the model declares for an item of the answer, and no other.
///
/// SOURCE: `smithy_query(query="shape",shape="AccountAssignmentForPrincipal",
/// model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`.
const MODELLED_ITEM_MEMBERS: [&str; 4] =
    ["AccountId", "PermissionSetArn", "PrincipalId", "PrincipalType"];

/// One principal of the table, and the (account, permission set) pairs its assignments are
/// granted on — each given as an index into [`ACCOUNTS`] and into the created permission
/// sets.
///
/// The rows differ along what the rule generalises over: both values of `PrincipalType`
/// (`USER | GROUP`), both forms the model's `PrincipalId` pattern admits — a bare GUID in
/// lower and in upper hex and the `[0-9a-f]{10}-` prefixed form, including one at the
/// 47-character length limit — and every assignment count from none through one to all of
/// the pairs. The last row repeats the first row's id under the other type, which is what
/// pins the principal to the whole (type, id) pair.
///
/// SOURCE: `smithy_query(query="operation",
/// operation="ListAccountAssignmentsForPrincipal")` for `PrincipalId`/`PrincipalType`.
#[derive(Debug, Clone)]
struct PrincipalRow {
    label: &'static str,
    principal_type: &'static str,
    principal_id: &'static str,
    assigned: &'static [(usize, usize)],
}

const PRINCIPALS: &[PrincipalRow] = &[
    PrincipalRow {
        label: "an uppercase-hex USER GUID assigned on three pairs",
        principal_type: "USER",
        principal_id: "0F1E2D3C-4B5A-4697-8869-7A6B5C4D3E2F",
        assigned: &[(0, 0), (1, 1), (2, 0)],
    },
    PrincipalRow {
        label: "a lowercase-hex GROUP GUID assigned on one pair",
        principal_type: "GROUP",
        principal_id: "b7c4d1e0-2a39-4f58-9671-0c1d2e3f4a5b",
        assigned: &[(1, 0)],
    },
    PrincipalRow {
        label: "a prefixed GROUP id at the 47-character limit assigned on every pair",
        principal_type: "GROUP",
        principal_id: "13579bdf02-fedc1234-5678-4abc-8def-0123456789ab",
        assigned: &[(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)],
    },
    PrincipalRow {
        label: "a prefixed USER id assigned on none",
        principal_type: "USER",
        principal_id: "02468ace13-0a1b2c3d-4e5f-4061-8273-8495a6b7c8d9",
        assigned: &[],
    },
    PrincipalRow {
        label: "the first row's id under the other principal type",
        principal_type: "GROUP",
        principal_id: "0F1E2D3C-4B5A-4697-8869-7A6B5C4D3E2F",
        assigned: &[(2, 1)],
    },
];

/// The store every rule is asserted against: a router, the two instances its account holds,
/// the permission sets created under each, and the engine behind them.
struct Fixture {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    first_instance: String,
    second_instance: String,
    /// The permission sets of the first instance, in the order they were created.
    permission_sets: Vec<String>,
    /// The one permission set of the second instance.
    second_permission_set: String,
}

/// Create one permission set under `instance_arn` and return the ARN the create minted.
async fn minted_permission_set(
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
        "case {name:?}: CreatePermissionSet must answer 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreatePermissionSet minted no ARN: {body}"))
        .to_string()
}

/// Grant one account assignment, the way a client does.
async fn grant(
    router: &MultiServiceRouter,
    instance_arn: &str,
    account_id: &str,
    permission_set_arn: &str,
    principal_type: &str,
    principal_id: &str,
) {
    let (status, body) = call(
        router,
        "CreateAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": principal_type,
            "PrincipalId": principal_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the grant of {principal_type} {principal_id} on {account_id} must answer 200, got \
         {status} {body}"
    );
}

/// Withdraw one account assignment, the way a client does.
async fn withdraw(
    router: &MultiServiceRouter,
    instance_arn: &str,
    account_id: &str,
    permission_set_arn: &str,
    principal_type: &str,
    principal_id: &str,
) {
    let (status, body) = call(
        router,
        "DeleteAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": principal_type,
            "PrincipalId": principal_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the withdrawal of {principal_type} {principal_id} on {account_id} must answer 200, \
         got {status} {body}"
    );
}

/// The store every rule is asserted against.
///
/// Two instances in ONE account is what lets "under the instance the request names" be
/// asserted rather than read off the only instance that exists. An organization instance is
/// enabled through AWS Organizations rather than through this API, so the second one is
/// seeded through the SAME `state::Instance` constructor the handlers use — exactly as the
/// harness's own organization fixture does — and every call a test then makes goes over the
/// wire.
///
/// The grants are made in a REVERSED order, through permission sets whose ARNs are minted,
/// so the order the answer reports cannot be the order the grants arrived in.
async fn fixture() -> Fixture {
    let (engine, first_instance) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let second_instance = with_store(&engine, |store| {
        let instance = state::Instance::active(
            SECOND_INSTANCE_ID,
            SECOND_IDENTITY_STORE_ID,
            state::InstanceType::Organization,
            Some("Second".to_string()),
            aws_smithy_types::DateTime::from(SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });
    assert_ne!(
        first_instance, second_instance,
        "the fixture must hold two instances of different ids, got {first_instance}"
    );

    let mut permission_sets = Vec::new();
    for index in 0..PERMISSION_SETS {
        permission_sets
            .push(minted_permission_set(&router, &first_instance, &format!("Assigned{index}")).await);
    }
    let second_permission_set =
        minted_permission_set(&router, &second_instance, "SecondInstanceSet").await;

    for row in PRINCIPALS.iter().rev() {
        for (account, permission_set) in row.assigned.iter().rev() {
            grant(
                &router,
                &first_instance,
                ACCOUNTS[*account],
                &permission_sets[*permission_set],
                row.principal_type,
                row.principal_id,
            )
            .await;
        }
        // Every principal the table assigns anything to is assigned under the second
        // instance as well, so the instance-scoping rule is asserted against a store where
        // both instances hold assignments of the same principal.
        if !row.assigned.is_empty() {
            grant(
                &router,
                &second_instance,
                ACCOUNTS[1],
                &second_permission_set,
                row.principal_type,
                row.principal_id,
            )
            .await;
        }
    }

    Fixture {
        engine,
        router,
        first_instance,
        second_instance,
        permission_sets,
        second_permission_set,
    }
}

/// One `ListAccountAssignmentsForPrincipal` call.
async fn list_page(
    router: &MultiServiceRouter,
    instance_arn: &str,
    principal_type: &str,
    principal_id: &str,
    filter_account: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PrincipalType": principal_type,
        "PrincipalId": principal_id,
    });
    if let Some(account_id) = filter_account {
        request["Filter"] = json!({"AccountId": account_id});
    }
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListAccountAssignmentsForPrincipal", &request).await
}

/// The items one answer carries, in the order it listed them.
fn listed_items(case: &str, body: &Value) -> Vec<Value> {
    body["AccountAssignments"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListAccountAssignmentsForPrincipalResponse$AccountAssignments \
                 is missing from {body}"
            )
        })
        .clone()
}

/// The (`AccountId`, `PermissionSetArn`) of each item, in the order they were listed.
fn listed_pairs(case: &str, items: &[Value]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|item| {
            let account = item["AccountId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an item carries no AccountId: {item}"));
            let permission_set = item["PermissionSetArn"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: an item carries no PermissionSetArn: {item}")
            });
            (account.to_string(), permission_set.to_string())
        })
        .collect()
}

/// Every page of one enumeration, walked the way a client walks it.
///
/// Asserts the page-size and termination invariants on the way: no page carries more than
/// the page size asked for, and only the last page comes back without a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    instance_arn: &str,
    principal_type: &str,
    principal_id: &str,
    filter_account: Option<&str>,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    // Absent `MaxResults` is the modelled `default: 100`, the same ceiling the shape's
    // `range(min: 1, max: 100)` tops out at.
    let limit = max_results.unwrap_or(100) as usize;
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, body) = list_page(
            router,
            instance_arn,
            principal_type,
            principal_id,
            filter_account,
            max_results,
            next_token.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the enumeration must answer 200, got {status} {body}"
        );
        let items = listed_items(case, &body);
        assert!(
            items.len() <= limit,
            "case {case:?}: a page must carry at most {limit} items, got {}",
            items.len()
        );
        seen.extend(items.clone());
        pages += 1;
        assert!(
            pages <= state::MAX_RECORDS_PER_STORE,
            "case {case:?}: the enumeration did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(token) => {
                assert_eq!(
                    items.len(),
                    limit,
                    "case {case:?}: a page that carries a NextToken must be full, got {} items",
                    items.len()
                );
                next_token = Some(token.to_string());
            }
            None => break,
        }
    }
    seen
}

/// The (account, permission set) pairs a row's assignments are expected to be reported
/// under, in the order the enumeration must report them: ascending by account id, then by
/// permission-set ARN.
fn expected_pairs(fixture: &Fixture, row: &PrincipalRow) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = row
        .assigned
        .iter()
        .map(|(account, permission_set)| {
            (
                ACCOUNTS[*account].to_string(),
                fixture.permission_sets[*permission_set].clone(),
            )
        })
        .collect();
    expected.sort();
    expected
}

/// Assert every item of an answer carries exactly the modelled members, with the principal
/// the request named.
fn assert_items_describe_the_principal(case: &str, items: &[Value], row: &PrincipalRow) {
    for item in items {
        let members: Vec<String> = item
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: an item must be a structure: {item}"))
            .keys()
            .cloned()
            .collect();
        for member in &members {
            assert!(
                MODELLED_ITEM_MEMBERS.contains(&member.as_str()),
                "case {case:?}: {member:?} is not a member the model declares for an \
                 AccountAssignmentForPrincipal, got {item}"
            );
        }
        for member in MODELLED_ITEM_MEMBERS {
            assert!(
                item.get(member).and_then(Value::as_str).is_some(),
                "case {case:?}: an item must carry {member}, got {item}"
            );
        }
        assert_eq!(
            item["PrincipalType"].as_str(),
            Some(row.principal_type),
            "case {case:?}: an item must report the principal type asked for, got {item}"
        );
        assert_eq!(
            item["PrincipalId"].as_str(),
            Some(row.principal_id),
            "case {case:?}: an item must report the principal id asked for, got {item}"
        );
    }
}

/// Rules 1, 2, 4 and 5 — the enumeration reports exactly the stored rows of the principal
/// named, under the instance named, in (account, permission set) order, and repeating it
/// answers identically.
#[tokio::test]
async fn lists_exactly_the_stored_assignments_of_the_principal_under_the_instance() {
    let fixture = fixture().await;

    for row in PRINCIPALS {
        let case = format!("principal {:?}", row.label);
        let items = walk(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            None,
            &case,
        )
        .await;
        assert_eq!(
            listed_pairs(&case, &items),
            expected_pairs(&fixture, row),
            "case {case:?}: the enumeration must report exactly the principal's stored \
             assignments of this instance in (account, permission set) order, got {items:?}"
        );
        assert_items_describe_the_principal(&case, &items, row);

        // Rule 4: the second instance holds ONE assignment per assigned principal, so its
        // enumeration reports that row and nothing of the first instance.
        let second = walk(
            &fixture.router,
            &fixture.second_instance,
            row.principal_type,
            row.principal_id,
            None,
            None,
            &case,
        )
        .await;
        let expected_second = if row.assigned.is_empty() {
            Vec::new()
        } else {
            vec![(
                ACCOUNTS[1].to_string(),
                fixture.second_permission_set.clone(),
            )]
        };
        assert_eq!(
            listed_pairs(&case, &second),
            expected_second,
            "case {case:?}: an assignment belongs to the instance it was granted under, got \
             {second:?}"
        );

        // Rule 5: a report — repeating the call answers identically and changes nothing.
        let (status, first_body) = list_page(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the enumeration must answer 200, got {status} {first_body}"
        );
        let (_, repeated) = list_page(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(
            first_body, repeated,
            "case {case:?}: repeating the enumeration must answer identically, got {repeated}"
        );
    }

    // Rule 5: nothing the reads did changed the store.
    let stored = with_store(&fixture.engine, |store| store.account_assignments.len());
    let granted: usize = PRINCIPALS
        .iter()
        .map(|row| row.assigned.len() + usize::from(!row.assigned.is_empty()))
        .sum();
    assert_eq!(
        stored, granted,
        "the reads must store nothing: expected {granted} rows, got {stored}"
    );
}

/// Rule 3 — the filter narrows the enumeration to the assignments on the one account it
/// names, for every account of the table and every principal of it, and an account the
/// principal holds nothing on is an empty page.
#[tokio::test]
async fn the_filter_narrows_the_enumeration_to_one_account() {
    let fixture = fixture().await;

    for row in PRINCIPALS {
        for account_id in ACCOUNTS.iter().chain(std::iter::once(&UNUSED_ACCOUNT)) {
            let case = format!("principal {:?} filtered to account {account_id}", row.label);
            let items = walk(
                &fixture.router,
                &fixture.first_instance,
                row.principal_type,
                row.principal_id,
                Some(account_id),
                None,
                &case,
            )
            .await;
            let expected: Vec<(String, String)> = expected_pairs(&fixture, row)
                .into_iter()
                .filter(|(account, _)| account == account_id)
                .collect();
            assert_eq!(
                listed_pairs(&case, &items),
                expected,
                "case {case:?}: the filter must report only that account's assignments, got \
                 {items:?}"
            );
            assert_items_describe_the_principal(&case, &items, row);
        }
    }
}

/// Rule 5 — the enumeration tracks the store: a withdrawal removes exactly its row and a
/// grant adds exactly its row.
#[tokio::test]
async fn the_enumeration_tracks_every_grant_and_withdrawal() {
    let fixture = fixture().await;
    // The row assigned on every pair, so each withdrawal can be observed against the rows
    // that remain.
    let row = &PRINCIPALS[2];

    let mut remaining = expected_pairs(&fixture, row);
    for (account, permission_set) in row.assigned {
        let account_id = ACCOUNTS[*account];
        let permission_set_arn = fixture.permission_sets[*permission_set].clone();
        withdraw(
            &fixture.router,
            &fixture.first_instance,
            account_id,
            &permission_set_arn,
            row.principal_type,
            row.principal_id,
        )
        .await;
        remaining.retain(|pair| pair != &(account_id.to_string(), permission_set_arn.clone()));
        let case = format!("after withdrawing {account_id} / {permission_set_arn}");
        let items = walk(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            None,
            &case,
        )
        .await;
        assert_eq!(
            listed_pairs(&case, &items),
            remaining,
            "case {case:?}: a withdrawal must remove exactly its row, got {items:?}"
        );
    }

    // And a fresh grant of a tuple that is not granted adds exactly that row back.
    for (account, permission_set) in row.assigned {
        let account_id = ACCOUNTS[*account];
        let permission_set_arn = fixture.permission_sets[*permission_set].clone();
        grant(
            &fixture.router,
            &fixture.first_instance,
            account_id,
            &permission_set_arn,
            row.principal_type,
            row.principal_id,
        )
        .await;
        remaining.push((account_id.to_string(), permission_set_arn.clone()));
        remaining.sort();
        let case = format!("after granting {account_id} / {permission_set_arn} again");
        let items = walk(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            None,
            &case,
        )
        .await;
        assert_eq!(
            listed_pairs(&case, &items),
            remaining,
            "case {case:?}: a grant must add exactly its row, got {items:?}"
        );
    }
}

/// Rule 6 — every page size the model admits walks the principal's assignments exactly
/// once, and a `MaxResults` outside the modelled range is refused.
#[tokio::test]
async fn every_page_size_the_model_admits_walks_the_assignments_once() {
    let fixture = fixture().await;
    // The row assigned on every pair, so a page size below the row count truncates.
    let row = &PRINCIPALS[2];
    let expected = expected_pairs(&fixture, row);

    // Absent, both published bounds of `range(min: 1, max: 100)`, every size around the
    // collection size, and one between.
    for max_results in [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(5),
        Some(6),
        Some(7),
        Some(41),
        Some(100),
    ] {
        let case = format!("MaxResults {max_results:?}");
        let items = walk(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            max_results,
            &case,
        )
        .await;
        assert_eq!(
            listed_pairs(&case, &items),
            expected,
            "case {case:?}: every page size must walk the principal's assignments once in \
             (account, permission set) order, got {} items",
            items.len()
        );
    }

    // One past each published bound is refused by the boundary validator, never answered
    // with a page.
    for max_results in [0, 101] {
        let case = format!("MaxResults {max_results}");
        let (status, body) = list_page(
            &fixture.router,
            &fixture.first_instance,
            row.principal_type,
            row.principal_id,
            None,
            Some(max_results),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside range 1..100 must be refused 400, got {status} \
             {body}"
        );
    }
}

/// Rule 7 — an instance this account holds none of is refused `ResourceNotFoundException`,
/// and the surface stays withdrawn on an ACCOUNT instance.
#[tokio::test]
async fn an_unknown_instance_is_not_found_and_an_account_instance_is_refused() {
    let fixture = fixture().await;
    let row = &PRINCIPALS[0];

    // The instance rows span the parts of the ARN that identify an instance — an id that
    // was never enabled, the `ins-` spelling the pattern admits, and the stored instance in
    // another partition — none of which this account holds an instance under.
    let instance_cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid instance id that was never enabled",
            UNKNOWN_INSTANCE_ARN.to_string(),
        ),
        (
            "the ins- spelling of an id nothing holds",
            "arn:aws:sso:::instance/ins-1234567890abcdef".to_string(),
        ),
        (
            "the stored instance in another partition",
            fixture
                .first_instance
                .replace("arn:aws:", "arn:aws-us-gov:"),
        ),
    ];
    for (case, instance_arn) in instance_cases {
        assert_ne!(
            instance_arn, fixture.first_instance,
            "case {case:?}: the fixture must name an instance the account does not hold"
        );
        let (status, body) = list_page(
            &fixture.router,
            &instance_arn,
            row.principal_type,
            row.principal_id,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown instance must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // An ACCOUNT instance serves no permission sets, so no account assignment can ever
    // exist on one and the enumeration is refused rather than answered with an empty page.
    let (account_router, account_instance) = account_instance_router().await;
    let (status, body) = list_page(
        &account_router,
        &account_instance,
        row.principal_type,
        row.principal_id,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "an account instance must be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "an account instance must be refused ValidationException, got {body}"
    );
    assert_eq!(
        body["message"].as_str().or(body["Message"].as_str()),
        Some(state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE),
        "an account instance must be refused with the restriction's own wording, got {body}"
    );
}

/// One foreign-`NextToken` row: the enumeration it is sent to, and the token sent.
struct TokenCase {
    case: &'static str,
    instance_arn: String,
    token: String,
    principal_type: &'static str,
    principal_id: &'static str,
    filter: Option<String>,
}

/// Rule 8 — the `NextToken` is this service's own cursor for THIS enumeration.
#[tokio::test]
async fn a_next_token_this_enumeration_did_not_issue_is_refused() {
    let fixture = fixture().await;
    // The row assigned on every pair, so a one-item page leaves a cursor behind.
    let row = &PRINCIPALS[2];
    let expected = expected_pairs(&fixture, row);

    let (status, body) = list_page(
        &fixture.router,
        &fixture.first_instance,
        row.principal_type,
        row.principal_id,
        None,
        Some(1),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the first page must answer 200, got {status} {body}"
    );
    let token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a NextToken: {body}"))
        .to_string();

    // Resuming from a token this enumeration issued yields exactly the rows that sort after
    // the key it carries.
    let (status, resumed) = list_page(
        &fixture.router,
        &fixture.first_instance,
        row.principal_type,
        row.principal_id,
        None,
        Some(100),
        Some(&token),
    )
    .await;
    assert_eq!(
        status, 200,
        "resuming from this service's own cursor must answer 200, got {status} {resumed}"
    );
    let items = listed_items("resumed", &resumed);
    assert_eq!(
        listed_pairs("resumed", &items),
        expected[1..].to_vec(),
        "resuming must report exactly the rows after the cursor's key, got {items:?}"
    );

    // A token of ANOTHER enumeration, or none of this service's at all, is a bad request.
    let other_principal = &PRINCIPALS[0];
    let cases = vec![
        TokenCase {
            case: "a token issued for another principal's enumeration",
            instance_arn: fixture.first_instance.clone(),
            token: token.clone(),
            principal_type: other_principal.principal_type,
            principal_id: other_principal.principal_id,
            filter: None,
        },
        TokenCase {
            case: "a token issued for the same principal under another instance",
            instance_arn: fixture.second_instance.clone(),
            token: token.clone(),
            principal_type: row.principal_type,
            principal_id: row.principal_id,
            filter: None,
        },
        TokenCase {
            case: "a token issued for the unfiltered enumeration, sent with a filter",
            instance_arn: fixture.first_instance.clone(),
            token: token.clone(),
            principal_type: row.principal_type,
            principal_id: row.principal_id,
            filter: Some(ACCOUNTS[0].to_string()),
        },
        TokenCase {
            case: "a token this service never issued",
            instance_arn: fixture.first_instance.clone(),
            token: "0123456789abcdef".to_string(),
            principal_type: row.principal_type,
            principal_id: row.principal_id,
            filter: None,
        },
        TokenCase {
            case: "a token that is not even hex",
            instance_arn: fixture.first_instance.clone(),
            token: "not-a-token".to_string(),
            principal_type: row.principal_type,
            principal_id: row.principal_id,
            filter: None,
        },
    ];
    for TokenCase {
        case,
        instance_arn,
        token: sent_token,
        principal_type,
        principal_id,
        filter,
    } in cases
    {
        let (status, body) = list_page(
            &fixture.router,
            &instance_arn,
            principal_type,
            principal_id,
            filter.as_deref(),
            None,
            Some(&sent_token),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign NextToken must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}
