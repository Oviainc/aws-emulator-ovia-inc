// SPDX-License-Identifier: CC0-1.0

//! `ListDelegationRequests`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rules being asserted:
//!
//! * a delegation request is listed only if it has an OWNER — "if a delegation request
//!   has no owner, even if it is assigned to a specific account, it will not be part of
//!   the ListDelegationRequests output for that account";
//! * `OwnerId` narrows the listing to the requests held under exactly that owner;
//! * the selection is paged by `MaxItems` and `Marker` the way every IAM collection is:
//!   each request reported exactly once across the pages, in the store's own key order,
//!   with a `Marker` naming a POSITION rather than an entity;
//! * a `Marker` this service did not issue is refused as an invalid input parameter;
//! * the listing is a report: it leaves the account holding what it held.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: which
//! ownership members a stored record carries, the form and length of an owner id, the
//! page size (both published bounds and the absent-parameter default), and whether the
//! caller's own policies permit the call.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::DelegationRequest;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::delegation_request_reads::{held_delegation_requests, seed};
use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The owners the suite spans, each as a `(case, owner id)` row.
///
/// Source: `ownerIdType` on `smithy_query(query="operation",
/// operation="ListDelegationRequests")` — length 20..2048, pattern
/// `^[a-zA-Z0-9:/+=,.@_-]+$` — and `DelegationRequest$OwnerId` ("ARN of the owner of this
/// delegation request"), so the rows carry the ARN forms an owner takes, the published
/// minimum length, and the whole published alphabet.
fn owners() -> Vec<(&'static str, String)> {
    vec![
        (
            "an IAM user's ARN",
            format!("arn:aws:iam::{}:user/approver", account_id()),
        ),
        (
            "an IAM role's ARN",
            format!("arn:aws:iam::{}:role/partner-approver", account_id()),
        ),
        (
            "an assumed-role session's ARN",
            format!(
                "arn:aws:sts::{}:assumed-role/partner-approver/review",
                account_id()
            ),
        ),
        // The published minimum length (20 characters), which a unique id rather than an
        // ARN is the shortest owner handle at.
        ("an owner id at the published minimum length", "AIDAOWNEREXAMPLE1234".to_string()),
        // Every character class the published pattern admits.
        (
            "the whole published alphabet",
            "arn:aws:iam::123456789012:user/Dev_Ops+Lead=1,2.3@x-y/z".to_string(),
        ),
    ]
}

/// How many requests each owner of [`owners`] holds, so a per-owner listing is asserted
/// over more than one request.
const REQUESTS_PER_OWNER: usize = 2;

/// The delegation requests the account holds in every suite below: [`REQUESTS_PER_OWNER`]
/// owned by each owner of [`owners`], plus the three shapes an UNOWNED record takes — one
/// carrying nothing but its identifier, one "assigned to a specific account" through
/// `OwnerAccountId`, and one carrying an approver but still no owner.
///
/// Identifiers are 17 characters of `^[\w-]+$`, inside `delegationRequestIdType`'s
/// published 16..128.
fn fixture() -> Vec<DelegationRequest> {
    let mut held = Vec::new();
    for (index, (_, owner)) in owners().into_iter().enumerate() {
        for sequence in 0..REQUESTS_PER_OWNER {
            held.push(DelegationRequest {
                delegation_request_id: Some(format!("owned-{index}-{sequence}-0000000")),
                owner_id: Some(owner.clone()),
                owner_account_id: Some(account_id().to_string()),
                description: Some("temporary access for a partner engagement".to_string()),
                ..DelegationRequest::default()
            });
        }
    }
    held.push(DelegationRequest {
        delegation_request_id: Some("unowned-0-0000000".to_string()),
        ..DelegationRequest::default()
    });
    held.push(DelegationRequest {
        delegation_request_id: Some("unowned-1-0000000".to_string()),
        owner_account_id: Some(account_id().to_string()),
        ..DelegationRequest::default()
    });
    held.push(DelegationRequest {
        delegation_request_id: Some("unowned-2-0000000".to_string()),
        owner_account_id: Some(account_id().to_string()),
        approver_id: Some(format!("arn:aws:iam::{}:user/approver", account_id())),
        ..DelegationRequest::default()
    });
    held
}

/// The identifier of `held`, which every fixture record carries.
fn id_of(held: &DelegationRequest) -> String {
    held.delegation_request_id
        .clone()
        .expect("a fixture delegation request carries its own identifier")
}

/// A router holding [`fixture`] in the service's own store, and the engine that store
/// lives in.
fn router_with_fixture() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    for held in fixture() {
        seed(&engine, &held);
    }
    (router, engine)
}

/// The identifiers a listing filtered by `owner_id` must report, in the store's own key
/// order — which for a `BTreeMap` keyed by the identifier is the identifiers sorted.
fn expected_ids(owner_id: Option<&str>) -> Vec<String> {
    let mut expected: Vec<String> = fixture()
        .iter()
        .filter(|held| {
            held.owner_id
                .as_deref()
                .is_some_and(|owner| owner_id.is_none_or(|asked| asked == owner))
        })
        .map(id_of)
        .collect();
    expected.sort();
    expected
}

/// One `ListDelegationRequests` call, presented as signed by `akid` when a row names one.
async fn list(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_signed_by(router, akid, "ListDelegationRequests", params).await
}

/// The name the response reports the truncation flag under: `isTruncated`, as
/// `ListDelegationRequestsResponse` declares that member (`smithy_query(query="operation",
/// operation="ListDelegationRequests")`) — lower-cased, unlike IAM's other list
/// responses, which is the model's spelling rather than a choice of this emulator.
const IS_TRUNCATED: &str = "isTruncated";

/// RULE: a delegation request is part of a listing only if it has an owner, and a request
/// carrying no `OwnerId` has none however it is assigned — so an unowned request is
/// reported to no caller, while every owned one is reported with the members the account
/// holds on it.
#[tokio::test]
async fn only_a_request_that_has_an_owner_is_listed() {
    let (router, engine) = router_with_fixture();
    let expected = expected_ids(None);

    let (status, body) = list(&router, None, &[("MaxItems", "1000")]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "DelegationRequestId"),
        expected,
        "expected every OWNED request in the store's key order, got {:?}: {body}",
        elements(&body, "DelegationRequestId")
    );
    assert_eq!(
        element(&body, IS_TRUNCATED),
        "false",
        "a page holding every selected request is not truncated: {body}"
    );
    assert!(
        elements(&body, "Marker").is_empty(),
        "a final page carries no Marker, got {:?}",
        elements(&body, "Marker")
    );

    // Each unowned record is held by the account and reported by none of the listing.
    for held in fixture().iter().filter(|held| held.owner_id.is_none()) {
        let id = id_of(held);
        assert!(
            !elements(&body, "DelegationRequestId").contains(&id),
            "case {id:?}: a request with no owner is not part of the listing: {body}"
        );
    }

    // A listed request carries the members the account holds on it.
    assert_eq!(
        elements(&body, "Description").len(),
        expected.len(),
        "every listed request reports its stored Description, got {:?}",
        elements(&body, "Description")
    );

    // The listing wrote nothing: the account holds exactly what it was seeded with.
    let mut seeded: Vec<String> = fixture().iter().map(id_of).collect();
    seeded.sort();
    assert_eq!(
        held_delegation_requests(&engine),
        seeded,
        "a listing persists nothing, got {:?}",
        held_delegation_requests(&engine)
    );
}

/// RULE: `OwnerId` is "the owner ID to filter delegation requests by", so a listing that
/// names one reports exactly the requests held under that owner, compared verbatim — an
/// owner nothing is held under, including one differing from a held owner only in case,
/// selects nothing.
#[tokio::test]
async fn the_owner_id_filter_selects_exactly_the_requests_held_under_that_owner() {
    let (router, _engine) = router_with_fixture();

    for (case, owner) in owners() {
        let expected = expected_ids(Some(&owner));
        assert_eq!(
            expected.len(),
            REQUESTS_PER_OWNER,
            "case {case:?}: the fixture holds {REQUESTS_PER_OWNER} requests per owner, \
             got {expected:?}"
        );

        let (status, body) = list(&router, None, &[("OwnerId", owner.as_str())]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            elements(&body, "DelegationRequestId"),
            expected,
            "case {case:?}: expected only this owner's requests, got {:?}: {body}",
            elements(&body, "DelegationRequestId")
        );
        for reported in elements(&body, "OwnerId") {
            assert_eq!(
                reported, owner,
                "case {case:?}: every listed request is owned by the owner asked about, \
                 got {reported:?}"
            );
        }
    }

    // An owner the account holds no request under, and a held owner's id in the other
    // case: both select nothing, because an owner id is a handle compared verbatim.
    let held_owner = owners()
        .into_iter()
        .map(|(_, owner)| owner)
        .next()
        .expect("the suite spans at least one owner");
    for (case, owner) in [
        (
            "an owner nothing is held under",
            format!("arn:aws:iam::{}:user/nobody-at-all", account_id()),
        ),
        ("a held owner in the other case", held_owner.to_uppercase()),
    ] {
        let (status, body) = list(&router, None, &[("OwnerId", owner.as_str())]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert!(
            elements(&body, "DelegationRequestId").is_empty(),
            "case {case:?}: expected no request, got {:?}: {body}",
            elements(&body, "DelegationRequestId")
        );
        assert_eq!(
            element(&body, IS_TRUNCATED),
            "false",
            "case {case:?}: a page of nothing is not truncated: {body}"
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {case:?}: a page of nothing carries no Marker, got {:?}",
            elements(&body, "Marker")
        );
    }
}

/// RULE: `MaxItems` and `Marker` page the selection — every selected request is reported
/// exactly once across the pages, in the store's own key order, no page holds more than
/// `MaxItems` of them, and only a truncated page carries the `Marker` the next one
/// resumes from.
#[tokio::test]
async fn the_listing_pages_through_every_selected_request_exactly_once() {
    let (router, _engine) = router_with_fixture();

    // Rows span the page size: the published minimum, sizes that do and do not divide the
    // selection, a size equal to it, the published maximum, and the absent-parameter
    // default. `maxItemsType` is range 1..1000.
    let selections: Vec<(String, Option<String>)> = std::iter::once(("every owner".to_string(), None))
        .chain(
            owners()
                .into_iter()
                .map(|(case, owner)| (case.to_string(), Some(owner))),
        )
        .collect();

    for (selection, owner) in selections {
        let expected = expected_ids(owner.as_deref());
        for page_size in [
            Some(1),
            Some(2),
            Some(3),
            Some(expected.len().max(1) as i32),
            Some(1000),
            None,
        ] {
            let case = format!("{selection} MaxItems={page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            let mut pages = 0usize;

            loop {
                let mut params: Vec<(String, String)> = Vec::new();
                if let Some(owner) = &owner {
                    params.push(("OwnerId".to_string(), owner.clone()));
                }
                if let Some(size) = page_size {
                    params.push(("MaxItems".to_string(), size.to_string()));
                }
                if let Some(token) = &marker {
                    params.push(("Marker".to_string(), token.clone()));
                }
                let borrowed: Vec<(&str, &str)> = params
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();

                let (status, body) = list(&router, None, &borrowed).await;
                assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");

                let page = elements(&body, "DelegationRequestId");
                if let Some(size) = page_size {
                    assert!(
                        page.len() <= size as usize,
                        "case {case:?}: expected at most {size} requests on a page, got {}",
                        page.len()
                    );
                }
                seen.extend(page);

                let truncated = element(&body, IS_TRUNCATED);
                let page_marker = elements(&body, "Marker");
                if truncated == "true" {
                    assert_eq!(
                        page_marker.len(),
                        1,
                        "case {case:?}: a truncated page must carry a Marker, got {page_marker:?}"
                    );
                    marker = Some(page_marker.into_iter().next().expect("a marker"));
                } else {
                    assert_eq!(
                        truncated, "false",
                        "case {case:?}: expected isTruncated true or false, got {truncated:?}"
                    );
                    assert!(
                        page_marker.is_empty(),
                        "case {case:?}: a final page carries no Marker, got {page_marker:?}"
                    );
                    break;
                }

                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
                );
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: expected every selected request exactly once in key order, \
                 got {seen:?}"
            );
        }
    }
}

/// RULE: a `Marker` this service did not issue for this collection names no position in
/// it, so the call is refused as an invalid input parameter (HTTP 400, awsQuery code
/// `InvalidInput`) rather than silently answered from the beginning; a token that DOES
/// decode names a position, which stays usable even once the request it was cut at is
/// gone.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_for_this_collection_is_refused() {
    let (router, engine) = router_with_fixture();

    // Rows span the ways a token can fail to be one of this collection's: text that is no
    // encoded token at all, text that decodes to nothing this service wrote, and a token
    // this service DID issue for another collection.
    for (case, bogus) in [
        ("text that is not an encoded token", "not-a-marker".to_string()),
        ("a token that decodes to foreign bytes", "zz".to_string()),
        (
            "a token issued for another collection",
            state::encode_marker(state::MARKER_SCOPE_USERS, "approver"),
        ),
    ] {
        let (status, body) = list(&router, None, &[("Marker", bogus.as_str())]).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            "Invalid Marker.",
            "case {case:?}: the refusal must be IAM's one sentence for a marker it did \
             not issue, got {:?}",
            element(&body, "Message")
        );
    }

    // A marker cut at a request that is then gone still resumes from that POSITION: the
    // rest of the order follows it, minus the departed request. The account's store is
    // what the request is removed from, because the operation that retires one is still a
    // scaffolded 501 of its own.
    let (status, body) = list(&router, None, &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected the first page, got {status}: {body}");
    let marker = element(&body, "Marker");
    let first = element(&body, "DelegationRequestId");
    state::service_state(engine.as_ref())
        .expect("the IAM store")
        .lock()
        .delegation_requests
        .remove(&first);

    let (status, body) = list(&router, None, &[("Marker", marker.as_str()), ("MaxItems", "1000")]).await;
    assert_eq!(
        status, 200,
        "expected a stale marker to still resume, got {status}: {body}"
    );
    let expected: Vec<String> = expected_ids(None)
        .into_iter()
        .filter(|id| *id != first)
        .collect();
    assert_eq!(
        elements(&body, "DelegationRequestId"),
        expected,
        "expected the order after the marker's position, got {:?}: {body}",
        elements(&body, "DelegationRequestId")
    );
}

/// RULE: `MaxItems` is range 1..1000, and the boundary validator refuses a value outside
/// it before the handler runs — so each published bound is served and one past each is
/// refused.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _engine) = router_with_fixture();

    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = list(&router, None, &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    for in_range in ["1", "1000"] {
        let (status, body) = list(&router, None, &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: IAM decides the call against the caller's own identity-based policies before any
/// request is reported — a principal whose documents do not allow the action is refused
/// HTTP 403 `AccessDenied`, naming the delegation-request collection the decision was
/// made on, and learns nothing about what the account holds.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_action_is_refused() {
    let (router, _engine) = router_with_fixture();
    let collection = format!("arn:aws:iam::{}:delegation-request/", account_id());

    for (row, (case, policies, reason)) in [
        (
            "an explicit deny about this call",
            vec![
                (
                    "allowed".to_string(),
                    statement("Allow", "iam:ListDelegationRequests", "*"),
                ),
                (
                    "denied".to_string(),
                    statement("Deny", "iam:ListDelegationRequests", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:ListUsers", "*"),
            )],
            "because no identity-based policy allows the iam:ListDelegationRequests action"
                .to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let user_name = format!("listrefused{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;

        let (status, body) = list(&router, Some(&access_key_id), &[]).await;
        assert_eq!(status, 403, "case {case:?}: expected 403, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}: {body}",
            error_code(&body)
        );
        assert_access_denied_message(
            case,
            &element(&body, "Message"),
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            "iam:ListDelegationRequests",
            &collection,
            &reason,
        );
        assert!(
            elements(&body, "DelegationRequestId").is_empty(),
            "case {case:?}: a refused listing reports no request, got {:?}",
            elements(&body, "DelegationRequestId")
        );
    }

    // A principal whose documents DO allow the action is served the same selection an
    // unsigned caller is.
    let allowed = principal(
        &router,
        "allowed to list",
        "listallowed",
        &[(
            "allowed".to_string(),
            statement("Allow", "iam:ListDelegationRequests", "*"),
        )],
    )
    .await;
    let (status, body) = list(&router, Some(&allowed), &[("MaxItems", "1000")]).await;
    let case = "allowed to list";
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "DelegationRequestId"),
        expected_ids(None),
        "case {case:?}: expected every owned request, got {:?}: {body}",
        elements(&body, "DelegationRequestId")
    );
}

/// RULE: a reporting operation changes nothing a later caller sees — the second identical
/// listing answers exactly what the first did, and the account holds what it held.
#[tokio::test]
async fn listing_delegation_requests_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_fixture();
    let before = held_delegation_requests(&engine);

    let (first_status, first) = list(&router, None, &[("MaxItems", "1000")]).await;
    let (second_status, second) = list(&router, None, &[("MaxItems", "1000")]).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "expected both listings to be served, got {first_status} then {second_status}"
    );
    assert_eq!(
        elements(&first, "DelegationRequestId"),
        elements(&second, "DelegationRequestId"),
        "case {:?}: a repeated listing answers the same, got {:?} then {:?}",
        "repeated listing",
        elements(&first, "DelegationRequestId"),
        elements(&second, "DelegationRequestId")
    );
    assert_eq!(
        held_delegation_requests(&engine),
        before,
        "case {:?}: a listing persists nothing, got {:?}",
        "repeated listing",
        held_delegation_requests(&engine)
    );
}
