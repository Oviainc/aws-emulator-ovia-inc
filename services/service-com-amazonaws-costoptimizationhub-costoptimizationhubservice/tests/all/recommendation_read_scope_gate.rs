// SPDX-License-Identifier: CC0-1.0

//! The SCOPE a recommendation read resolves to, the permission that scope
//! needs, and the one sentence the refusal is worded with — driven end-to-end
//! over the wire (`awsJson1_0`) through the same multi-service router the
//! emulator boots.
//!
//! The rules under test:
//!
//! * A recommendation read is refused as the organization's view, which only
//!   the organization's management account has, exactly when the accounts it
//!   addresses reach an account OTHER than the caller's — the same question the
//!   enrollment reader's scope is gated by (`enrollment_read_scope_gate`). A
//!   `filter.accountIds` naming only the caller is a read of that account's own
//!   record (`smithy_query(query="doc", shape="Filter", member="accountIds")`:
//!   "The account to which the recommendation applies"), so it is served
//!   whatever else the filter sets, and it answers the rows that account may be
//!   read for.
//! * Every read this service refuses for that missing view is refused with one
//!   fixed sentence, whichever data the request asked for, beside the 403
//!   `AccessDeniedException` the operation declares
//!   (`smithy_query(query="operation",
//!   operation="ListRecommendations,ListRecommendationSummaries,ListEnrollmentStatuses")`).
//!
//! Each rule is asserted over a table whose rows differ in which accounts the
//! scope names, how many, whether the caller is among them, and what else the
//! filter narrows by.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, catalog_rows_for, enrolled_router, error_code, ids, management_account_router_with_store,
    ok, record_enrollment, ANALYSIS_WAIT, ORGANIZATION_VIEW_DENIAL,
};

/// Accounts of the organization that are not the caller: twelve digits, which
/// is what the `AccountId` shape's `^[0-9]{12}$` pattern admits.
const OTHER_ACCOUNT: &str = "111122223333";
const SECOND_OTHER_ACCOUNT: &str = "444455556666";

/// Stands in for the calling account in a row, whose id is the emulator's and
/// so not a constant this file can write.
const CALLER: &str = "<caller>";

/// One row of the scope table: the `filter` the read sends, and whether the
/// accounts it addresses reach past the caller's own.
struct Case {
    /// What the case is called in every assertion of this row.
    name: &'static str,
    /// The ids `filter.accountIds` names, with [`CALLER`] standing in for the
    /// calling account; `None` leaves the member out entirely.
    account_ids: Option<&'static [&'static str]>,
    /// Another filter member sent beside them, so the gate is shown to be
    /// about the account scope and not about the rest of the filter.
    beside: Option<&'static str>,
    /// Whether the scope reaches an account other than the caller's — so a
    /// member account is refused this read and only the management account is
    /// served it.
    reaches_past_caller: bool,
}

/// The `filter` one row sends, with [`CALLER`] resolved.
fn filter(case: &Case, caller: &str) -> Value {
    let mut filter = json!({});
    if let Some(account_ids) = case.account_ids {
        let named: Vec<String> = account_ids
            .iter()
            .map(|account| {
                if *account == CALLER {
                    caller.to_string()
                } else {
                    (*account).to_string()
                }
            })
            .collect();
        filter["accountIds"] = json!(named);
    }
    if let Some(region) = case.beside {
        filter["regions"] = json!([region]);
    }
    filter
}

/// Every scope the `accountIds` member can resolve to: the member omitted, the
/// caller alone (once, twice, beside another member), and one, two or a mixed
/// list of accounts that are not the caller.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no account named",
            account_ids: None,
            beside: None,
            reaches_past_caller: false,
        },
        Case {
            name: "no account named, beside another filter member",
            account_ids: None,
            beside: Some("us-east-1"),
            reaches_past_caller: false,
        },
        Case {
            name: "the caller's own account",
            account_ids: Some(&[CALLER]),
            beside: None,
            reaches_past_caller: false,
        },
        Case {
            name: "the caller's own account, named twice",
            account_ids: Some(&[CALLER, CALLER]),
            beside: None,
            reaches_past_caller: false,
        },
        Case {
            name: "the caller's own account, beside another filter member",
            account_ids: Some(&[CALLER]),
            beside: Some("us-east-1"),
            reaches_past_caller: false,
        },
        Case {
            name: "one other account",
            account_ids: Some(&[OTHER_ACCOUNT]),
            beside: None,
            reaches_past_caller: true,
        },
        Case {
            name: "two other accounts",
            account_ids: Some(&[OTHER_ACCOUNT, SECOND_OTHER_ACCOUNT]),
            beside: None,
            reaches_past_caller: true,
        },
        Case {
            name: "the caller's own account beside another",
            account_ids: Some(&[CALLER, OTHER_ACCOUNT]),
            beside: None,
            reaches_past_caller: true,
        },
        Case {
            name: "one other account, beside another filter member",
            account_ids: Some(&[OTHER_ACCOUNT]),
            beside: Some("us-east-1"),
            reaches_past_caller: true,
        },
    ]
}

/// The two list readers' requests for one row's filter, as
/// `(operation, body)` — the gate is one rule, so every row is driven through
/// both.
fn reads(filter: &Value) -> Vec<(&'static str, Value)> {
    vec![
        (
            "ListRecommendations",
            json!({"includeAllRecommendations": true, "filter": filter}),
        ),
        (
            "ListRecommendationSummaries",
            json!({"groupBy": "AccountId", "filter": filter}),
        ),
    ]
}

/// The accounts a `ListRecommendations` answer's rows belong to, sorted.
fn owners(payload: &Value) -> Vec<String> {
    let mut accounts: Vec<String> = payload["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["accountId"].as_str().unwrap_or_default().to_string())
        .collect();
    accounts.sort();
    accounts.dedup();
    accounts
}

/// The group labels a summaries answer carries, sorted.
fn groups(payload: &Value) -> Vec<String> {
    let mut labels: Vec<String> = payload["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["group"].as_str().unwrap_or_default().to_string())
        .collect();
    labels.sort();
    labels
}

/// Assert one refused read: the modelled 403 `AccessDeniedException`, worded
/// with this service's one sentence for the view the caller does not have.
fn assert_refused(case: &str, operation: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 403,
        "case {case:?} {operation}: expected HTTP 403, got {status} with {payload}"
    );
    let code = error_code(payload);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?} {operation}: expected AccessDeniedException, got {code:?} in {payload}"
    );
    assert_eq!(
        payload["message"].as_str(),
        Some(ORGANIZATION_VIEW_DENIAL),
        "case {case:?} {operation}: expected the refusal {ORGANIZATION_VIEW_DENIAL:?}, got \
         {payload}"
    );
}

/// A caller that is not the organization's management account is refused
/// exactly the recommendation reads whose scope reaches past its own account,
/// and served every read confined to it — with the rows of its own account,
/// which is what naming its own account selects.
#[tokio::test]
async fn a_member_account_is_refused_exactly_the_recommendation_reads_that_reach_past_it() {
    let router = enrolled_router().await;
    let caller = engine::emulator_account_id();

    for case in cases() {
        let name = case.name;
        let filter = filter(&case, caller);
        for (operation, body) in reads(&filter) {
            let (status, payload) = call(&router, operation, &body).await;
            if case.reaches_past_caller {
                assert_refused(name, operation, status, &payload);
                continue;
            }
            assert_eq!(
                status, 200,
                "case {name:?} {operation}: expected HTTP 200 for {body}, got {status} with \
                 {payload}"
            );
            // The admitted read answers the caller's OWN rows, not an empty
            // list and nobody else's: grouped by account there is exactly the
            // caller's group.
            let answered = if operation == "ListRecommendations" {
                owners(&payload)
            } else {
                groups(&payload)
            };
            assert_eq!(
                answered,
                vec![caller.to_string()],
                "case {name:?} {operation}: expected the caller's own rows, got {answered:?} in \
                 {payload}"
            );
        }
    }

    // A self-named scope selects exactly what naming no account selects: it is
    // the same one record either way.
    let unnamed = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await;
    let named = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "filter": {"accountIds": [caller]}}),
    )
    .await;
    assert_eq!(
        ids(&named),
        ids(&unnamed),
        "case the caller's own account: expected the rows the unnamed read serves, got {named}"
    );

    // And every id that read serves reads back through the detail reader, so
    // the admitted scope answers rows that really exist for this caller.
    for recommendation_id in ids(&named) {
        let (status, detail) = call(
            &router,
            "GetRecommendation",
            &json!({"recommendationId": recommendation_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case id {recommendation_id:?}: the list served this id, so expected HTTP 200, got \
             {status} with {detail}"
        );
        assert_eq!(
            detail["accountId"].as_str(),
            Some(caller),
            "case id {recommendation_id:?}: expected the caller's own row, got {detail}"
        );
    }
}

/// The other arm of the same gate: the organization's management account is
/// served every one of those scopes, with the rows of the accounts the scope
/// names.
#[tokio::test]
async fn the_management_account_is_served_every_recommendation_scope() {
    let (router, store) = management_account_router_with_store();
    let caller = engine::emulator_account_id();

    // The caller enrolls over the wire as a client would; the members are
    // recorded in the store, because this emulator has no Organizations
    // control plane to create another account's record or its catalog rows.
    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": true}),
    )
    .await;
    record_enrollment(&store, OTHER_ACCOUNT, "Active", true);
    let other_rows = catalog_rows_for(&store, OTHER_ACCOUNT, 2);
    assert!(
        !other_rows.is_empty(),
        "case setup: expected the member to own catalog rows, got {other_rows:?}"
    );
    std::thread::sleep(ANALYSIS_WAIT);

    for case in cases() {
        let name = case.name;
        let filter = filter(&case, caller);
        for (operation, body) in reads(&filter) {
            let payload = ok(&router, operation, &body).await;
            let answered = if operation == "ListRecommendations" {
                owners(&payload)
            } else {
                groups(&payload)
            };
            // Whatever the scope names, no row of an account outside it is
            // answered.
            let named: Vec<String> = filter["accountIds"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|account| account.as_str().unwrap_or_default().to_string())
                .collect();
            if !named.is_empty() {
                for account in &answered {
                    assert!(
                        named.contains(account),
                        "case {name:?} {operation}: a row of {account} was answered for \
                         {named:?}, got {payload}"
                    );
                }
            }
            assert!(
                !answered.is_empty(),
                "case {name:?} {operation}: expected the named accounts' rows, got {payload}"
            );
        }
    }
}

/// One wording for the whole refusal class: every read this service refuses
/// for the organization view the caller does not have answers the same fixed
/// sentence, whichever data was asked for and whichever scope asked for it.
#[tokio::test]
async fn every_organization_view_refusal_is_worded_the_same() {
    let router = enrolled_router().await;
    let caller = engine::emulator_account_id();

    // (case, operation, request) — the reads of all three kinds of data this
    // service refuses for that missing view.
    let refused: Vec<(String, &str, Value)> = vec![
        (
            "recommendations of one other account".to_string(),
            "ListRecommendations",
            json!({
                "includeAllRecommendations": true,
                "filter": {"accountIds": [OTHER_ACCOUNT]},
            }),
        ),
        (
            "recommendations of two other accounts, paged".to_string(),
            "ListRecommendations",
            json!({
                "includeAllRecommendations": false,
                "filter": {"accountIds": [OTHER_ACCOUNT, SECOND_OTHER_ACCOUNT]},
                "maxResults": 5,
            }),
        ),
        (
            "summaries of one other account".to_string(),
            "ListRecommendationSummaries",
            json!({"groupBy": "Region", "filter": {"accountIds": [OTHER_ACCOUNT]}}),
        ),
        (
            "summaries of another account beside the caller".to_string(),
            "ListRecommendationSummaries",
            json!({
                "groupBy": "ResourceType",
                "filter": {"accountIds": [caller, OTHER_ACCOUNT]},
                "metrics": ["SavingsPercentage"],
            }),
        ),
        (
            "the enrollment of one other account".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": OTHER_ACCOUNT}),
        ),
        (
            "the enrollment of the whole organization".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": true}),
        ),
    ];

    for (case, operation, body) in &refused {
        let (status, payload) = call(&router, operation, body).await;
        assert_refused(case, operation, status, &payload);
    }
}

/// What the admitted self-named scope unlocks: the summaries of the caller's
/// own account, with the additional metric the request asks for reported beside
/// them.
///
/// The table spans the groupings the summaries are taken over and whether the
/// own-account scope is sent beside another filter member, because the metric
/// is reported for the view the request selects.
#[tokio::test]
async fn the_additional_metric_is_reported_for_an_own_account_scope() {
    let router: MultiServiceRouter = enrolled_router().await;
    let caller = engine::emulator_account_id();

    // (case, groupBy, the filter sent)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "grouped by resource type",
            "ResourceType",
            json!({"accountIds": [caller]}),
        ),
        (
            "grouped by region",
            "Region",
            json!({"accountIds": [caller]}),
        ),
        (
            "grouped by account",
            "AccountId",
            json!({"accountIds": [caller]}),
        ),
        (
            "grouped by region, narrowed by region too",
            "Region",
            json!({"accountIds": [caller], "regions": ["us-east-1"]}),
        ),
    ];

    for (case, group_by, filter) in &cases {
        let payload = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({
                "groupBy": group_by,
                "filter": filter,
                "metrics": ["SavingsPercentage"],
            }),
        )
        .await;
        assert_eq!(
            payload["groupBy"].as_str(),
            Some(*group_by),
            "case {case:?}: expected the grouping asked for, got {payload}"
        );
        assert!(
            payload["items"].is_array(),
            "case {case:?}: expected an items list, got {payload}"
        );
        assert!(
            payload["currencyCode"]
                .as_str()
                .is_some_and(|code| !code.is_empty()),
            "case {case:?}: expected the currency the figures are in, got {payload}"
        );
        let metric = payload["metrics"]["savingsPercentage"].as_str();
        assert!(
            metric.is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected the requested metric reported, got {payload}"
        );
    }
}
