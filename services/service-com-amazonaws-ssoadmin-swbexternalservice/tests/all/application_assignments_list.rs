// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationAssignments` end to end over the `awsJson1_1` wire — the operation the
//! stub issue reported as an unimplemented scaffold.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one example:
//!
//! 1. the enumeration reports EXACTLY the stored direct assignments of the application the
//!    request names, one item per stored row, each carrying the three modelled members of an
//!    `ApplicationAssignment` and no other, in one stable principal (type, id) order
//!    whatever order the grants arrived in — and an application nobody is assigned to is an
//!    empty list, not a refusal;
//! 2. an assignment belongs to the ONE application it was granted on, so no enumeration
//!    reports another application's rows, under this instance or another;
//! 3. every item it reports is a grant another reader confirms, so the list and
//!    `DescribeApplicationAssignment` cannot disagree;
//! 4. it TRACKS the store: a grant adds exactly its row and a revocation removes exactly its
//!    row, and it is a REPORT — repeating it answers identically and changes nothing a later
//!    caller sees;
//! 5. it pages: for every `MaxResults` the model's `range(min: 1, max: 100)` admits, walking
//!    the pages yields each stored row exactly once in that same order, no page carries more
//!    than the page size asked for, and the last page carries no `NextToken`; one past each
//!    published bound is refused;
//! 6. an `ApplicationArn` this account holds no application for is refused
//!    `ResourceNotFoundException` — never an empty list and never the scaffold's HTTP 501;
//! 7. the `NextToken` is this service's own cursor: one it did not issue for THIS
//!    enumeration (another application, or no token of this service at all) is refused
//!    `ValidationException`, and resuming from one it did issue yields exactly the rows that
//!    sort after the key it carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints, the
//! `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than bypassed.
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, create_application_assignment, describe_application_assignment,
    router_over, seeded_instance_arn, with_store, TEST_REGION,
};

/// The AWS-owned provider a customer managed application is created under, an entry of the
/// catalog this service publishes (`state::BUILT_IN_APPLICATION_PROVIDERS`, reachable over
/// the wire through `ListApplicationProviders`).
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The id of the SECOND instance the cross-application rows are asserted against:
/// `ssoins-` + 16 of `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const SECOND_INSTANCE_ID: &str = "ssoins-5d4c3b2a19087766";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const SECOND_IDENTITY_STORE_ID: &str = "d-5d4c3b2a19";

/// Every member the model declares for an item of the answer, and no other.
///
/// SOURCE: `smithy_query(query="shape",shape="ApplicationAssignment",
/// model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`.
const MODELLED_ITEM_MEMBERS: [&str; 3] = ["ApplicationArn", "PrincipalId", "PrincipalType"];

/// A pattern-valid `ApplicationArn` no create ever minted, for the not-found rule. Shaped by
/// the model's `ApplicationArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
const UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-0000000000000000/apl-0000000000000000";

/// One principal the table grants assignments to.
///
/// The rows differ along what the rule generalises over: both values of `PrincipalType`
/// (`USER | GROUP`), and every form the model's `PrincipalId` pattern admits — a bare GUID in
/// lower and in upper hex, the `[0-9a-f]{10}-` prefixed form including one at the
/// 47-character length limit, and an all-`f` id that sorts last. Row 4 repeats row 0's id
/// under the other type, which is what pins a principal to the whole (type, id) pair.
///
/// SOURCE: `smithy_query(query="shape",shape="ApplicationAssignment")` for
/// `PrincipalId`/`PrincipalType`.
#[derive(Debug, Clone, Copy)]
struct Principal {
    label: &'static str,
    principal_type: &'static str,
    principal_id: &'static str,
}

const PRINCIPALS: &[Principal] = &[
    Principal {
        label: "an uppercase-hex USER GUID",
        principal_type: "USER",
        principal_id: "0F1E2D3C-4B5A-4697-8869-7A6B5C4D3E2F",
    },
    Principal {
        label: "a lowercase-hex GROUP GUID",
        principal_type: "GROUP",
        principal_id: "b7c4d1e0-2a39-4f58-9671-0c1d2e3f4a5b",
    },
    Principal {
        label: "a prefixed GROUP id at the 47-character limit",
        principal_type: "GROUP",
        principal_id: "13579bdf02-fedc1234-5678-4abc-8def-0123456789ab",
    },
    Principal {
        label: "a prefixed USER id",
        principal_type: "USER",
        principal_id: "02468ace13-0a1b2c3d-4e5f-4061-8273-8495a6b7c8d9",
    },
    Principal {
        label: "row 0's id under the other principal type",
        principal_type: "GROUP",
        principal_id: "0F1E2D3C-4B5A-4697-8869-7A6B5C4D3E2F",
    },
    Principal {
        label: "an all-f USER GUID",
        principal_type: "USER",
        principal_id: "ffffffff-ffff-4fff-8fff-ffffffffffff",
    },
];

/// One application of the table, and the principals (by index into [`PRINCIPALS`]) assigned
/// to it. The rows differ along the assignment COUNT — none, one, two, three and every
/// principal of the table — so the same rule is asserted on an empty, a single-row and a
/// multi-row enumeration.
#[derive(Debug, Clone, Copy)]
struct ApplicationRow {
    label: &'static str,
    assigned: &'static [usize],
}

const APPLICATIONS: &[ApplicationRow] = &[
    ApplicationRow {
        label: "an application nobody is assigned to",
        assigned: &[],
    },
    ApplicationRow {
        label: "an application with one USER assigned",
        assigned: &[0],
    },
    ApplicationRow {
        label: "an application with the same id under both principal types",
        assigned: &[0, 4],
    },
    ApplicationRow {
        label: "an application with three principals assigned",
        assigned: &[1, 2, 5],
    },
    ApplicationRow {
        label: "an application with every principal of the table assigned",
        assigned: &[0, 1, 2, 3, 4, 5],
    },
];

/// The store every rule is asserted against: a router, the two instances its account holds,
/// the applications created under the first and the one application of the second.
struct Fixture {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    /// The applications of the first instance, one per [`APPLICATIONS`] row, in table order.
    applications: Vec<String>,
    /// The one application of the second instance, assigned to every principal.
    second_application: String,
}

/// Create one application under `instance_arn` and return the ARN the create minted.
async fn minted_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateApplication must answer 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateApplication minted no ARN: {body}"))
        .to_string()
}

/// Grant one direct assignment, the way a client does.
async fn grant(router: &MultiServiceRouter, application_arn: &str, principal: &Principal) {
    let (status, body) = create_application_assignment(
        router,
        application_arn,
        principal.principal_type,
        principal.principal_id,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the grant on {application_arn} must answer 200, got {status} {body}",
        principal.label
    );
}

/// Revoke one direct assignment, the way a client does.
async fn revoke(router: &MultiServiceRouter, application_arn: &str, principal: &Principal) {
    let (status, body) = call(
        router,
        "DeleteApplicationAssignment",
        &json!({
            "ApplicationArn": application_arn,
            "PrincipalType": principal.principal_type,
            "PrincipalId": principal.principal_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the revocation on {application_arn} must answer 200, got {status} {body}",
        principal.label
    );
}

/// The store the rules are asserted against.
///
/// Two instances in ONE account is what lets "the assignments of the application named" be
/// asserted against a store where another instance's application carries the same principals.
/// An organization instance is enabled through AWS Organizations rather than through this API,
/// so the second one is seeded through the SAME `state::Instance` constructor the handlers
/// use — exactly as the harness's own organization fixture does — and every call a test then
/// makes goes over the wire.
///
/// The grants are made in a REVERSED principal order, over applications whose ARNs are
/// minted, so the order the answer reports cannot be the order the grants arrived in.
async fn fixture() -> Fixture {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let first_instance = seeded_instance_arn(&router).await;
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

    let mut applications = Vec::new();
    for index in 0..APPLICATIONS.len() {
        applications
            .push(minted_application(&router, &first_instance, &format!("app-{index}")).await);
    }
    let second_application =
        minted_application(&router, &second_instance, "second-instance-app").await;

    for (index, row) in APPLICATIONS.iter().enumerate() {
        for principal_index in row.assigned.iter().rev() {
            grant(&router, &applications[index], &PRINCIPALS[*principal_index]).await;
        }
    }
    // Every principal of the table is assigned to the second instance's application too, so
    // the per-application rule is asserted against a store where another application carries
    // the very rows an enumeration must not report.
    for principal in PRINCIPALS.iter().rev() {
        grant(&router, &second_application, principal).await;
    }

    Fixture {
        engine,
        router,
        applications,
        second_application,
    }
}

/// One `ListApplicationAssignments` call.
async fn list_page(
    router: &MultiServiceRouter,
    application_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"ApplicationArn": application_arn});
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListApplicationAssignments", &request).await
}

/// The items one answer carries, in the order it listed them.
fn listed_items(case: &str, body: &Value) -> Vec<Value> {
    body["ApplicationAssignments"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListApplicationAssignmentsResponse$ApplicationAssignments is \
                 missing from {body}"
            )
        })
        .clone()
}

/// The (`PrincipalType`, `PrincipalId`) of each item, in the order they were listed.
fn listed_principals(case: &str, items: &[Value]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|item| {
            let principal_type = item["PrincipalType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an item carries no PrincipalType: {item}"));
            let principal_id = item["PrincipalId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an item carries no PrincipalId: {item}"));
            (principal_type.to_string(), principal_id.to_string())
        })
        .collect()
}

/// The principals a row's assignments must be reported under, in the order the enumeration
/// must report them: ascending by the (type, id) pair, which is the part of an assignment's
/// identity the request has not already fixed.
fn expected_principals(row: &ApplicationRow) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = row
        .assigned
        .iter()
        .map(|index| {
            (
                PRINCIPALS[*index].principal_type.to_string(),
                PRINCIPALS[*index].principal_id.to_string(),
            )
        })
        .collect();
    expected.sort();
    expected
}

/// Every page of one enumeration, walked the way a client walks it.
///
/// Asserts the page-size and termination invariants on the way: no page carries more than the
/// page size asked for, and only the last page comes back without a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    application_arn: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    // An absent `MaxResults` is this service's own page ceiling, the same figure the shape's
    // `range(min: 1, max: 100)` tops out at.
    let limit = max_results.unwrap_or(i64::from(state::MAX_PAGE_SIZE)) as usize;
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, body) = list_page(router, application_arn, max_results, next_token.as_deref())
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

/// Assert every item of an answer carries exactly the modelled members, reporting the
/// application the request named.
fn assert_items_describe_the_application(case: &str, items: &[Value], application_arn: &str) {
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
                 ApplicationAssignment, got {item}"
            );
        }
        for member in MODELLED_ITEM_MEMBERS {
            assert!(
                item.get(member).and_then(Value::as_str).is_some(),
                "case {case:?}: an item must carry {member}, got {item}"
            );
        }
        assert_eq!(
            item["ApplicationArn"].as_str(),
            Some(application_arn),
            "case {case:?}: an item must report the application asked for, got {item}"
        );
    }
}

/// Rules 1, 2, 3 and 4 — the enumeration reports exactly the application's stored rows, in
/// principal order, every one of them confirmed by `DescribeApplicationAssignment`, and
/// repeating it answers identically and stores nothing.
#[tokio::test]
async fn lists_exactly_the_stored_assignments_of_the_application_named() {
    let fixture = fixture().await;
    let stored_before = with_store(&fixture.engine, |store| store.application_assignments.len());

    for (index, row) in APPLICATIONS.iter().enumerate() {
        let case = format!("application {:?}", row.label);
        let application_arn = fixture.applications[index].clone();
        let items = walk(&fixture.router, &application_arn, None, &case).await;
        assert_eq!(
            listed_principals(&case, &items),
            expected_principals(row),
            "case {case:?}: the enumeration must report exactly this application's stored \
             assignments in principal order, got {items:?}"
        );
        assert_items_describe_the_application(&case, &items, &application_arn);

        // Rule 3: every item is a grant the single-assignment reader confirms.
        for (principal_type, principal_id) in listed_principals(&case, &items) {
            let (status, body) = describe_application_assignment(
                &fixture.router,
                &application_arn,
                &principal_type,
                &principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: every listed assignment must be readable back, got {status} {body}"
            );
        }

        // Rule 4: a report — repeating the call answers identically.
        let (status, first_body) = list_page(&fixture.router, &application_arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the enumeration must answer 200, got {status} {first_body}"
        );
        let (_, repeated) = list_page(&fixture.router, &application_arn, None, None).await;
        assert_eq!(
            first_body, repeated,
            "case {case:?}: repeating the enumeration must answer identically, got {repeated}"
        );
    }

    // Rule 2: the other instance's application carries every principal of the table, and its
    // own enumeration reports exactly those and nothing of the first instance's applications.
    let case = "the second instance's application";
    let items = walk(&fixture.router, &fixture.second_application, None, case).await;
    let mut every_principal: Vec<(String, String)> = PRINCIPALS
        .iter()
        .map(|principal| {
            (
                principal.principal_type.to_string(),
                principal.principal_id.to_string(),
            )
        })
        .collect();
    every_principal.sort();
    assert_eq!(
        listed_principals(case, &items),
        every_principal,
        "case {case:?}: an assignment belongs to the one application it was granted on, got \
         {items:?}"
    );
    assert_items_describe_the_application(case, &items, &fixture.second_application);

    // Rule 4: nothing the reads did changed the store.
    let stored_after = with_store(&fixture.engine, |store| store.application_assignments.len());
    assert_eq!(
        stored_after, stored_before,
        "the reads must store nothing: expected {stored_before} rows, got {stored_after}"
    );
}

/// Rule 4 — the enumeration tracks the store: a revocation removes exactly its row and a
/// grant adds exactly its row, on the application it names and on no other.
#[tokio::test]
async fn the_enumeration_tracks_every_grant_and_revocation() {
    let fixture = fixture().await;
    // The row every principal of the table is assigned to, so each revocation can be observed
    // against the rows that remain.
    let index = APPLICATIONS.len() - 1;
    let application_arn = fixture.applications[index].clone();
    let mut remaining = expected_principals(&APPLICATIONS[index]);

    for principal in PRINCIPALS {
        revoke(&fixture.router, &application_arn, principal).await;
        remaining.retain(|(principal_type, principal_id)| {
            (principal_type.as_str(), principal_id.as_str())
                != (principal.principal_type, principal.principal_id)
        });
        let case = format!("after revoking {:?}", principal.label);
        let items = walk(&fixture.router, &application_arn, None, &case).await;
        assert_eq!(
            listed_principals(&case, &items),
            remaining,
            "case {case:?}: a revocation must remove exactly its row, got {items:?}"
        );
        // And it removed nothing from the other application that holds the same principals.
        let other = walk(&fixture.router, &fixture.second_application, None, &case).await;
        assert_eq!(
            other.len(),
            PRINCIPALS.len(),
            "case {case:?}: another application's rows must be untouched, got {other:?}"
        );
    }

    for principal in PRINCIPALS {
        grant(&fixture.router, &application_arn, principal).await;
        remaining.push((
            principal.principal_type.to_string(),
            principal.principal_id.to_string(),
        ));
        remaining.sort();
        let case = format!("after granting {:?} again", principal.label);
        let items = walk(&fixture.router, &application_arn, None, &case).await;
        assert_eq!(
            listed_principals(&case, &items),
            remaining,
            "case {case:?}: a grant must add exactly its row, got {items:?}"
        );
    }
}

/// Rule 5 — every page size the model admits walks the application's assignments exactly
/// once, and a `MaxResults` outside the modelled range is refused.
#[tokio::test]
async fn every_page_size_the_model_admits_walks_the_assignments_once() {
    let fixture = fixture().await;
    // The application every principal of the table is assigned to, so a page size below the
    // row count truncates.
    let index = APPLICATIONS.len() - 1;
    let application_arn = fixture.applications[index].clone();
    let expected = expected_principals(&APPLICATIONS[index]);

    // Absent, both published bounds of `range(min: 1, max: 100)`, every size around the
    // collection size, and one between.
    for max_results in [None, Some(1), Some(2), Some(3), Some(5), Some(6), Some(7), Some(43), Some(100)]
    {
        let case = format!("MaxResults {max_results:?}");
        let items = walk(&fixture.router, &application_arn, max_results, &case).await;
        assert_eq!(
            listed_principals(&case, &items),
            expected,
            "case {case:?}: every page size must walk the application's assignments once in \
             principal order, got {} items",
            items.len()
        );
    }

    // One past each published bound is refused by the boundary validator, never answered with
    // a page.
    for max_results in [0, 101] {
        let case = format!("MaxResults {max_results}");
        let (status, body) =
            list_page(&fixture.router, &application_arn, Some(max_results), None).await;
        assert_refused(
            &case,
            "a MaxResults outside range 1..100",
            "ValidationException",
            status,
            &body,
        );
    }
}

/// The same ARN with the last character of its `apl-` id flipped: another application of
/// the very instance the stored one lives under, which no create ever minted. Stays inside
/// the model's `ApplicationArn` pattern, whose id tail is `apl-[a-zA-Z0-9]{16}`.
fn other_application_id(application_arn: &str) -> String {
    let (head, last) = application_arn.split_at(application_arn.len() - 1);
    let flipped = if last == "a" { "b" } else { "a" };
    format!("{head}{flipped}")
}

/// Rule 6 — an `ApplicationArn` this account holds no application for is refused
/// `ResourceNotFoundException`, with the message every other reader of an unknown application
/// answers with.
#[tokio::test]
async fn an_unknown_application_is_refused_not_found() {
    let fixture = fixture().await;

    // The rows span the parts of the ARN that identify an application: one nothing minted,
    // the stored application in another partition, and one whose account segment is another
    // account's.
    let stored = fixture.applications[1].clone();
    let cases: Vec<(&str, String)> = vec![
        (
            "an application ARN no create minted",
            UNKNOWN_APPLICATION_ARN.to_string(),
        ),
        (
            "the stored application in another partition",
            stored.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "another application id under the stored application's own instance",
            other_application_id(&stored),
        ),
    ];
    for (case, application_arn) in cases {
        assert_ne!(
            application_arn, stored,
            "case {case:?}: the row must name an application the account does not hold"
        );
        let (status, body) = list_page(&fixture.router, &application_arn, None, None).await;
        assert_refused(
            case,
            "an enumeration of an unknown application",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let expected = state::application_not_found_message(&application_arn);
        let text = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default();
        assert_eq!(
            text, expected,
            "case {case:?}: the refusal must carry the service's own not-found wording, got {body}"
        );
    }
}

/// Rule 7 — the `NextToken` is this service's own cursor for THIS enumeration.
#[tokio::test]
async fn a_next_token_this_enumeration_did_not_issue_is_refused() {
    let fixture = fixture().await;
    let index = APPLICATIONS.len() - 1;
    let application_arn = fixture.applications[index].clone();
    let expected = expected_principals(&APPLICATIONS[index]);

    let (status, body) = list_page(&fixture.router, &application_arn, Some(1), None).await;
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
    let (status, resumed) =
        list_page(&fixture.router, &application_arn, Some(100), Some(&token)).await;
    assert_eq!(
        status, 200,
        "resuming from this service's own cursor must answer 200, got {status} {resumed}"
    );
    let items = listed_items("resumed", &resumed);
    assert_eq!(
        listed_principals("resumed", &items),
        expected[1..].to_vec(),
        "resuming must report exactly the rows after the cursor's key, got {items:?}"
    );

    // A token of ANOTHER enumeration, or none of this service's at all, is a bad request.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a token issued for another application's enumeration",
            fixture.applications[2].clone(),
            token.clone(),
        ),
        (
            "a token issued for an application of the other instance",
            fixture.second_application.clone(),
            token.clone(),
        ),
        (
            "a token this service never issued",
            application_arn.clone(),
            "0123456789abcdef".to_string(),
        ),
        (
            "a token that is not even hex",
            application_arn.clone(),
            "not-a-token".to_string(),
        ),
    ];
    for (case, sent_application, sent_token) in cases {
        let (status, body) =
            list_page(&fixture.router, &sent_application, None, Some(&sent_token)).await;
        assert_refused(
            case,
            "a foreign NextToken",
            "ValidationException",
            status,
            &body,
        );
    }
}
