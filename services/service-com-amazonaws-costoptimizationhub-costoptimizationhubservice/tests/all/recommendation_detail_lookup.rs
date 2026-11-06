// SPDX-License-Identifier: CC0-1.0

//! Which recommendations a read of ONE recommendation resolves its id over,
//! driven end-to-end over the wire (`awsJson1_0`) through the same
//! multi-service router the emulator boots.
//!
//! The rule under test: `GetRecommendation` resolves `recommendationId` over
//! the recommendations the CALLER may read — the rows owned by the accounts its
//! reads reach, each judged by that account's own enrollment — which is the set
//! the list readers select from, so an id a list reader handed this caller reads
//! back (`smithy_query(query="doc", shape="GetRecommendation")`: "To retrieve
//! the recommendationId, use the ListRecommendations API"). A read by id names
//! no account, so the accounts it reaches are the caller's own plus, for the
//! organization's management account, the organization's — the same widening
//! `filter.accountIds` gets on the list readers (`account_scoped_reads`), and
//! nothing more: a caller that is not the management account still resolves its
//! own account's ids alone.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, catalog_rows_for, error_code, ids, management_account_router_with_store, ok,
    record_enrollment, Store, ANALYSIS_WAIT,
};

/// Member accounts of the organization the store records. Twelve digits, which
/// is what the `AccountId` shape's `^[0-9]{12}$` pattern admits, and none of
/// them the caller.
const MEMBER_ACTIVE: &str = "111122223333";
const MEMBER_INACTIVE: &str = "444455556666";
/// A member that has opted in but whose analysis is not due yet, so it holds no
/// readable recommendation even though it owns catalog rows.
const MEMBER_NOT_DUE: &str = "777788889999";
/// An account the organization holds no enrollment record for at all.
const UNRECORDED: &str = "222233334444";

/// The members the list entry and the detail view of one recommendation share,
/// so a row read by id is the row the list served rather than another one.
const SHARED_MEMBERS: [&str; 19] = [
    "recommendationId",
    "accountId",
    "region",
    "resourceId",
    "resourceArn",
    "currentResourceType",
    "recommendedResourceType",
    "actionType",
    "implementationEffort",
    "restartNeeded",
    "rollbackPossible",
    "currencyCode",
    "source",
    "estimatedMonthlyCost",
    "estimatedMonthlySavings",
    "estimatedSavingsPercentage",
    "recommendationLookbackPeriodInDays",
    "lastRefreshTimestamp",
    "tags",
];

/// What every case below needs: the organization's management account enrolled
/// over the wire, and member accounts — which this emulator has no
/// Organizations control plane to create — recorded in the store with their own
/// catalog rows.
///
/// Answers the rows created for each member, in catalog order.
struct Organization {
    router: MultiServiceRouter,
    store: Store,
    member_rows: Vec<String>,
    inactive_rows: Vec<String>,
    not_due_rows: Vec<String>,
    unrecorded_rows: Vec<String>,
}

async fn organization() -> Organization {
    let (router, store) = management_account_router_with_store();

    // The caller enrolls over the wire, as a client would; the members are
    // recorded in the store afterwards.
    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": true}),
    )
    .await;
    record_enrollment(&store, MEMBER_ACTIVE, "Active", true);
    record_enrollment(&store, MEMBER_INACTIVE, "Inactive", false);
    record_enrollment(&store, MEMBER_NOT_DUE, "Active", false);

    let organization = Organization {
        member_rows: catalog_rows_for(&store, MEMBER_ACTIVE, 2),
        inactive_rows: catalog_rows_for(&store, MEMBER_INACTIVE, 2),
        not_due_rows: catalog_rows_for(&store, MEMBER_NOT_DUE, 1),
        unrecorded_rows: catalog_rows_for(&store, UNRECORDED, 1),
        router,
        store,
    };
    // The caller's own analysis has to have come due before its rows read.
    std::thread::sleep(ANALYSIS_WAIT);
    organization
}

/// Every recommendation `ListRecommendations` serves for `filter`, as whole
/// entries.
async fn listed(router: &MultiServiceRouter, filter: Option<&Value>) -> Vec<Value> {
    let mut request = json!({"includeAllRecommendations": true});
    if let Some(filter) = filter {
        request["filter"] = filter.clone();
    }
    let payload = ok(router, "ListRecommendations", &request).await;
    payload["items"].as_array().cloned().unwrap_or_default()
}

/// Every id a list reader hands this caller reads back by id, and the row it
/// reads back is the row the list served.
///
/// The table spans what the rule generalises over: whether the request names
/// accounts at all, whose it names, how many, and whether another filter member
/// narrows the selection beside them — each row a selection the list reader
/// serves this caller, so each id in it must resolve.
#[tokio::test]
async fn every_id_a_list_reader_serves_reads_back_by_id() {
    let organization = organization().await;
    let router = &organization.router;
    let caller = engine::emulator_account_id();

    // (case, the filter the list read sends)
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("no filter at all", None),
        ("the caller's own account", Some(json!({"accountIds": [caller]}))),
        (
            "one enrolled member",
            Some(json!({"accountIds": [MEMBER_ACTIVE]})),
        ),
        (
            "the caller and a member",
            Some(json!({"accountIds": [caller, MEMBER_ACTIVE]})),
        ),
        (
            "a member named twice",
            Some(json!({"accountIds": [MEMBER_ACTIVE, MEMBER_ACTIVE]})),
        ),
        (
            "a member beside another filter member",
            Some(json!({"accountIds": [MEMBER_ACTIVE], "regions": ["us-east-1"]})),
        ),
        (
            "a region across the whole selection",
            Some(json!({"regions": ["us-east-1"]})),
        ),
    ];

    for (case, filter) in &cases {
        let items = listed(router, filter.as_ref()).await;
        assert!(
            !items.is_empty(),
            "case {case:?}: expected the list to serve rows to read back, got {items:?}"
        );
        for item in &items {
            let recommendation_id = item["recommendationId"].as_str().unwrap_or_default();
            let (status, detail) = call(
                router,
                "GetRecommendation",
                &json!({"recommendationId": recommendation_id}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} id {recommendation_id:?}: the list served this id, so expected \
                 HTTP 200, got {status} with {detail}"
            );
            for member in SHARED_MEMBERS {
                assert_eq!(
                    detail[member], item[member],
                    "case {case:?} id {recommendation_id:?}: {member} differs between the list \
                     and the detail view, expected {} got {}",
                    item[member], detail[member]
                );
            }
        }
    }
}

/// The set a read by id resolves over is bounded by the same per-account
/// enrollment the list readers apply: an id of an account that is enrolled and
/// due resolves, and an id of any other account is one this caller holds no
/// recommendation for.
///
/// The table spans the owning account's state: the caller itself, a member that
/// is enrolled and due, one that opted out, one that opted in but whose
/// analysis is not due yet, one the organization holds no record for, and an id
/// no catalog row carries.
#[tokio::test]
async fn an_id_resolves_only_while_its_owner_may_be_read() {
    let organization = organization().await;
    let router = &organization.router;
    let caller = engine::emulator_account_id();
    let own_rows = ids(&ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await);
    assert!(
        !own_rows.is_empty(),
        "case setup: expected the enrolled caller's own rows to be readable"
    );

    // (case, the id read, the owner the answer must name — `None` is a 404)
    let mut cases: Vec<(String, String, Option<String>)> = Vec::new();
    for id in &own_rows {
        cases.push((
            format!("the caller's own row {id}"),
            id.clone(),
            Some(caller.to_string()),
        ));
    }
    for id in &organization.member_rows {
        cases.push((
            format!("an enrolled member's row {id}"),
            id.clone(),
            Some(MEMBER_ACTIVE.to_string()),
        ));
    }
    for id in &organization.inactive_rows {
        cases.push((
            format!("a row of a member that opted out {id}"),
            id.clone(),
            None,
        ));
    }
    for id in &organization.not_due_rows {
        cases.push((
            format!("a row of a member whose analysis is not due {id}"),
            id.clone(),
            None,
        ));
    }
    for id in &organization.unrecorded_rows {
        cases.push((
            format!("a row of an account with no enrollment record {id}"),
            id.clone(),
            None,
        ));
    }
    cases.push((
        "an id no catalog row carries".to_string(),
        "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c9999".to_string(),
        None,
    ));

    for (case, recommendation_id, owner) in &cases {
        let (status, payload) = call(
            router,
            "GetRecommendation",
            &json!({"recommendationId": recommendation_id}),
        )
        .await;
        match owner {
            Some(owner) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                assert_eq!(
                    payload["accountId"].as_str(),
                    Some(owner.as_str()),
                    "case {case:?}: expected the row of {owner}, got {payload}"
                );
                assert_eq!(
                    payload["recommendationId"].as_str(),
                    Some(recommendation_id.as_str()),
                    "case {case:?}: expected the id that was asked for, got {payload}"
                );
            }
            None => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404, got {status} with {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {payload}"
                );
                assert_eq!(
                    payload["resourceId"].as_str(),
                    Some(recommendation_id.as_str()),
                    "case {case:?}: expected the requested id in resourceId, got {payload}"
                );
            }
        }
    }

    // Reading by id changes nothing a later caller sees: the same list answers
    // the same rows after every read above.
    let again = ids(&ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await);
    assert_eq!(
        again, own_rows,
        "case repeated read: the readable catalog changed across the reads above"
    );
}

/// Reading by id widens nobody's reach: with no organization recorded the
/// caller is a plain member account, so the ids it resolves are its own
/// account's — exactly the selection its own list reads may serve it — and
/// another account's id is a 404.
#[tokio::test]
async fn a_caller_that_is_not_the_management_account_resolves_its_own_ids_only() {
    let organization = organization().await;
    let router = &organization.router;
    let own_rows = ids(&ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await);

    // Which accounts an organization has is an Organizations fact; with none
    // recorded, no caller has the organization's view.
    organization.store.lock().organization_management_account_id = None;

    for id in &own_rows {
        let (status, payload) = call(
            router,
            "GetRecommendation",
            &json!({"recommendationId": id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case own row {id}: expected HTTP 200, got {status} with {payload}"
        );
    }

    for id in organization
        .member_rows
        .iter()
        .chain(organization.inactive_rows.iter())
    {
        let (status, payload) = call(
            router,
            "GetRecommendation",
            &json!({"recommendationId": id}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case another account's row {id}: expected HTTP 404 for a caller with no \
             organization view, got {status} with {payload}"
        );
    }
}
