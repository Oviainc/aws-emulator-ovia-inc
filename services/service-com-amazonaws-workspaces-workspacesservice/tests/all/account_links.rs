// SPDX-License-Identifier: CC0-1.0

//! The account-link lifecycle, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine (the harness `workspace_lifecycle` already owns), so each assertion is about
//! what a client would see: the request is serialized, routed, validated, handled and
//! serialized back.
//!
//! The rules under test, all of them about links this emulator persisted rather than
//! about any one recorded request:
//!
//! * `CreateAccountLinkInvitation` mints an `AccountLinkId` matching the model's
//!   `^link-.{8,24}$`, reports the emulator's account as `SourceAccountId` and the
//!   requested account as `TargetAccountId`, and persists the link awaiting the target
//!   account's answer. Every case that creates one first states that its target account
//!   EXISTS in this emulator's organization, which is the gate
//!   `account_link_eligibility` is about.
//! * `GetAccountLink` reports the stored link a `LinkId` or a `LinkedAccountId` names.
//! * `ListAccountLinks` reports the stored links whose status the request's
//!   `LinkStatusFilter` names, paginated over a stable creation order.
//! * `AcceptAccountLinkInvitation`, `RejectAccountLinkInvitation` and
//!   `DeleteAccountLinkInvitation` move or remove that persisted link, and refuse a
//!   link whose current status forbids the change.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    service_state, MAX_STORED_ACCOUNT_LINKS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_lifecycle::{call, emulator_with_engine};

/// Every value of the model's `AccountLinkStatusEnum`, from
/// `smithy_query(query="shape",shape="AccountLinkStatusEnum")`.
const MODELLED_STATUSES: &[&str] = &[
    "LINKED",
    "LINKING_FAILED",
    "LINK_NOT_FOUND",
    "PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT",
    "REJECTED",
];

/// The status a link carries while it waits for the target account to answer.
const PENDING: &str = "PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT";

/// State that each of `accounts` EXISTS in this emulator's organization, which is
/// what makes it an account an invitation may name.
///
/// WorkSpaces creates no accounts — AWS Organizations does — so there is no wire
/// call for this precondition; it is stated through the same state API the
/// handler resolves a `TargetAccountId` against, the way the store-bound case
/// below fills the link store directly.
///
/// `pub(crate)`: `tests/all.rs` compiles every suite into ONE test crate, so
/// `account_link_eligibility` shares this rather than copying it.
pub(crate) fn accounts_exist<S: AsRef<str>>(engine: &Arc<engine::InMemoryEngine>, accounts: &[S]) {
    let state = service_state(&**engine).expect("the WorkSpaces state store");
    let mut state = state.lock();
    for account in accounts {
        let account = account.as_ref();
        state.register_organization_account(account);
        assert!(
            state.is_organization_account(account),
            "case {account}: the account must exist in the organization once registered"
        );
    }
}

/// Target accounts spanning the `AwsAccount` pattern `^\d{12}$`: the lowest and
/// highest twelve-digit values, a leading-zero one, and two arbitrary ones. None
/// of them is the emulator's own account, which is what a self-invitation case
/// uses instead.
fn target_account_table() -> Vec<&'static str> {
    vec![
        "000000000001",
        "999999999999",
        "000000012345",
        "123456789012",
        "210987654321",
    ]
}

/// Whether `candidate` satisfies the model's `LinkId` pattern `^link-.{8,24}$`.
fn is_modelled_link_id(candidate: &str) -> bool {
    candidate
        .strip_prefix("link-")
        .is_some_and(|rest| (8..=24).contains(&rest.chars().count()))
}

/// Invite `target_account_id`, asserting the call was accepted, and return the
/// `AccountLink` it reported.
async fn invite(router: &MultiServiceRouter, target_account_id: &str) -> Value {
    let request = json!({ "TargetAccountId": target_account_id });
    let (status, body) = call(router, "CreateAccountLinkInvitation", &request).await;
    assert_eq!(
        status, 200,
        "case {target_account_id}: CreateAccountLinkInvitation must answer 200, body {body:?}"
    );
    let link = body["AccountLink"].clone();
    assert!(
        link.is_object(),
        "case {target_account_id}: an AccountLink must be reported, got {body:?}"
    );
    link
}

/// Invite `target_account_id` and return the minted `AccountLinkId`.
async fn invite_id(router: &MultiServiceRouter, target_account_id: &str) -> String {
    invite(router, target_account_id).await["AccountLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {target_account_id}: a minted AccountLinkId"))
        .to_string()
}

/// Call `operation` with one `LinkId` and return `(status, body)`.
async fn call_with_link(
    router: &MultiServiceRouter,
    operation: &str,
    link_id: &str,
) -> (u16, Value) {
    call(router, operation, &json!({ "LinkId": link_id })).await
}

/// `ListAccountLinks` with `request`, returning the reported `AccountLinkId`s in
/// the order reported together with the `NextToken`, if any.
async fn list_page(
    router: &MultiServiceRouter,
    request: &Value,
) -> (Vec<String>, Option<String>) {
    let (status, body) = call(router, "ListAccountLinks", request).await;
    assert_eq!(
        status, 200,
        "case {request:?}: ListAccountLinks must answer 200, body {body:?}"
    );
    let links = body["AccountLinks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {request:?}: AccountLinks must be a list, got {body:?}"))
        .clone();
    let ids = links
        .iter()
        .map(|link| {
            link["AccountLinkId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {request:?}: every link carries an id, got {link:?}"))
                .to_string()
        })
        .collect();
    let next_token = body["NextToken"].as_str().map(str::to_string);
    (ids, next_token)
}

/// Every `AccountLinkId` `ListAccountLinks` reports for `request`, following
/// `NextToken` to the last page.
///
/// `pub(crate)`: see [`accounts_exist`].
pub(crate) async fn list_all(router: &MultiServiceRouter, request: &Value) -> Vec<String> {
    let mut page_request = request.clone();
    let mut seen = Vec::new();
    loop {
        let (ids, next_token) = list_page(router, &page_request).await;
        seen.extend(ids);
        match next_token {
            Some(token) => {
                page_request = request.clone();
                page_request["NextToken"] = json!(token);
            }
            None => return seen,
        }
    }
}

/// The wire error code of a refused call: the `__type` the JSON protocols carry.
///
/// `pub(crate)`: see [`accounts_exist`].
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn created_invitations_are_persisted_and_read_back() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(&engine, &target_account_table());
    let source = engine::emulator_account_id();

    let mut created: Vec<(String, String)> = Vec::new();
    for target in target_account_table() {
        let link = invite(&router, target).await;
        let link_id = link["AccountLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {target}: a minted AccountLinkId, got {link:?}"));
        assert!(
            is_modelled_link_id(link_id),
            "case {target}: expected an id matching ^link-.{{8,24}}$, got {link_id:?}"
        );
        assert_eq!(
            link["AccountLinkStatus"].as_str(),
            Some(PENDING),
            "case {target}: a new invitation awaits the target account, got {link:?}"
        );
        assert_eq!(
            link["SourceAccountId"].as_str(),
            Some(source),
            "case {target}: the inviting account is the source, got {link:?}"
        );
        assert_eq!(
            link["TargetAccountId"].as_str(),
            Some(target),
            "case {target}: the invited account is the target, got {link:?}"
        );
        created.push((target.to_string(), link_id.to_string()));
    }

    // Round trip: every created link reads back through GetAccountLink, both by
    // its own identifier and by the account on the other side of it.
    for (target, link_id) in &created {
        for selector in [
            json!({ "LinkId": link_id }),
            json!({ "LinkedAccountId": target }),
            json!({ "LinkId": link_id, "LinkedAccountId": target }),
        ] {
            let (status, body) = call(&router, "GetAccountLink", &selector).await;
            assert_eq!(
                status, 200,
                "case {selector:?}: GetAccountLink must answer 200, body {body:?}"
            );
            assert_eq!(
                body["AccountLink"]["AccountLinkId"].as_str(),
                Some(link_id.as_str()),
                "case {selector:?}: expected link {link_id}, got {body:?}"
            );
            assert_eq!(
                body["AccountLink"]["AccountLinkStatus"].as_str(),
                Some(PENDING),
                "case {selector:?}: the stored status must be reported, got {body:?}"
            );
        }
    }

    // And every one of them is listed, in the order they were created.
    let listed = list_all(&router, &json!({})).await;
    let expected: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(
        listed, expected,
        "ListAccountLinks must report every stored link in creation order"
    );
}

#[tokio::test]
async fn list_filters_by_status_over_the_modelled_catalog() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(
        &engine,
        &[
            "000000000001",
            "000000000002",
            "000000000003",
            "000000000004",
        ],
    );
    let accepted = invite_id(&router, "000000000001").await;
    let rejected = invite_id(&router, "000000000002").await;
    let pending_one = invite_id(&router, "000000000003").await;
    let pending_two = invite_id(&router, "000000000004").await;

    let (status, body) = call_with_link(&router, "AcceptAccountLinkInvitation", &accepted).await;
    assert_eq!(
        status, 200,
        "case accept {accepted}: must answer 200, body {body:?}"
    );
    assert_eq!(
        body["AccountLink"]["AccountLinkStatus"].as_str(),
        Some("LINKED"),
        "case accept {accepted}: an accepted invitation is linked, got {body:?}"
    );
    let (status, body) = call_with_link(&router, "RejectAccountLinkInvitation", &rejected).await;
    assert_eq!(
        status, 200,
        "case reject {rejected}: must answer 200, body {body:?}"
    );
    assert_eq!(
        body["AccountLink"]["AccountLinkStatus"].as_str(),
        Some("REJECTED"),
        "case reject {rejected}: a rejected invitation is rejected, got {body:?}"
    );

    // One row per value of the modelled status enum, including the two no stored
    // link can carry.
    let expected: Vec<(&str, Vec<String>)> = vec![
        ("LINKED", vec![accepted.clone()]),
        ("LINKING_FAILED", vec![]),
        ("LINK_NOT_FOUND", vec![]),
        (PENDING, vec![pending_one.clone(), pending_two.clone()]),
        ("REJECTED", vec![rejected.clone()]),
    ];
    assert_eq!(
        expected.iter().map(|(status, _)| *status).collect::<Vec<_>>(),
        MODELLED_STATUSES.to_vec(),
        "the table must span every value of the modelled status enum"
    );
    for (status, wanted) in &expected {
        let request = json!({ "LinkStatusFilter": [status] });
        let reported = list_all(&router, &request).await;
        assert_eq!(
            &reported, wanted,
            "case {request:?}: expected {wanted:?}, got {reported:?}"
        );
    }

    // A filter naming several statuses selects their union, still in creation
    // order; one naming none selects nothing; no filter at all selects every link.
    let union = list_all(
        &router,
        &json!({ "LinkStatusFilter": ["REJECTED", "LINKED"] }),
    )
    .await;
    assert_eq!(
        union,
        vec![accepted.clone(), rejected.clone()],
        "a multi-status filter selects the union of the statuses it names"
    );
    let none = list_all(&router, &json!({ "LinkStatusFilter": [] })).await;
    assert!(
        none.is_empty(),
        "a filter naming no status selects no link, got {none:?}"
    );
    let all = list_all(&router, &json!({})).await;
    assert_eq!(
        all,
        vec![accepted, rejected, pending_one, pending_two],
        "no filter reports every stored link in creation order"
    );
}

#[tokio::test]
async fn list_paginates_over_a_stable_creation_order() {
    let (router, engine) = emulator_with_engine();
    let targets: Vec<String> = (1..=7u32).map(|index| format!("{index:012}")).collect();
    accounts_exist(&engine, &targets);
    let mut expected = Vec::new();
    for target in &targets {
        expected.push(invite_id(&router, target).await);
    }

    // Each row is a page size; the pages it produces must concatenate back to the
    // one creation order, with no repeat and nothing skipped.
    for max_results in [1usize, 2, 3, 7, 25] {
        let request = json!({ "MaxResults": max_results });
        let mut page_request = request.clone();
        let mut seen: Vec<String> = Vec::new();
        loop {
            let (ids, next_token) = list_page(&router, &page_request).await;
            assert!(
                ids.len() <= max_results,
                "case MaxResults={max_results}: a page may not exceed the page size, got {ids:?}"
            );
            seen.extend(ids);
            let Some(token) = next_token else { break };
            assert!(
                seen.len() < expected.len(),
                "case MaxResults={max_results}: a token must only follow an incomplete walk, seen {seen:?}"
            );
            page_request = request.clone();
            page_request["NextToken"] = json!(token);
        }
        assert_eq!(
            seen, expected,
            "case MaxResults={max_results}: paging must report the whole creation order once"
        );
    }
}

#[tokio::test]
async fn list_accepts_the_published_max_results_bounds_and_refuses_the_rest() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(&engine, &["000000000001"]);
    invite_id(&router, "000000000001").await;

    // `ListAccountLinksRequest$MaxResults` is `range 1..25`: each published bound
    // is accepted and the value just past it is refused.
    for (max_results, expected_status) in [(1, 200), (25, 200), (0, 400), (26, 400)] {
        let request = json!({ "MaxResults": max_results });
        let (status, body) = call(&router, "ListAccountLinks", &request).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case MaxResults={max_results}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_invitation_is_answered_once_and_a_linked_account_stays_linked() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(&engine, &["000000000001", "000000000002"]);
    let accepted = invite_id(&router, "000000000001").await;
    let rejected = invite_id(&router, "000000000002").await;
    let (status, _body) = call_with_link(&router, "AcceptAccountLinkInvitation", &accepted).await;
    assert_eq!(status, 200, "the first accept must be served");
    let (status, _body) = call_with_link(&router, "RejectAccountLinkInvitation", &rejected).await;
    assert_eq!(status, 200, "the first reject must be served");

    // Each row is an answer a link's current status forbids: an invitation is
    // answered once, and an established link cannot be deleted.
    let cases = [
        ("accept an accepted link", "AcceptAccountLinkInvitation", &accepted),
        ("reject an accepted link", "RejectAccountLinkInvitation", &accepted),
        ("delete an accepted link", "DeleteAccountLinkInvitation", &accepted),
        ("accept a rejected link", "AcceptAccountLinkInvitation", &rejected),
        ("reject a rejected link", "RejectAccountLinkInvitation", &rejected),
    ];
    for (case, operation, link_id) in cases {
        let (status, body) = call_with_link(&router, operation, link_id).await;
        assert_eq!(
            status, 400,
            "case {case}: the request must be refused, body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case}: expected ConflictException, got {body:?}"
        );
    }

    // The refusals changed nothing: both links still carry the answer they got.
    for (link_id, expected) in [(&accepted, "LINKED"), (&rejected, "REJECTED")] {
        let (status, body) = call(&router, "GetAccountLink", &json!({ "LinkId": link_id })).await;
        assert_eq!(status, 200, "case {link_id}: the link must still be readable, body {body:?}");
        assert_eq!(
            body["AccountLink"]["AccountLinkStatus"].as_str(),
            Some(expected),
            "case {link_id}: expected status {expected}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn deleting_an_invitation_removes_it_from_the_store() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(
        &engine,
        &["000000000001", "000000000002", "000000000003"],
    );
    let kept = invite_id(&router, "000000000001").await;
    let rejected = invite_id(&router, "000000000002").await;
    let (status, _body) = call_with_link(&router, "RejectAccountLinkInvitation", &rejected).await;
    assert_eq!(status, 200, "the reject must be served");
    let pending = invite_id(&router, "000000000003").await;

    // A pending invitation and an answered-then-rejected one are both deletable.
    for link_id in [&pending, &rejected] {
        let (status, body) = call_with_link(&router, "DeleteAccountLinkInvitation", link_id).await;
        assert_eq!(
            status, 200,
            "case {link_id}: DeleteAccountLinkInvitation must answer 200, body {body:?}"
        );
        assert_eq!(
            body["AccountLink"]["AccountLinkId"].as_str(),
            Some(link_id.as_str()),
            "case {link_id}: the deleted link must be reported, got {body:?}"
        );
        assert_eq!(
            body["AccountLink"]["AccountLinkStatus"].as_str(),
            Some("LINK_NOT_FOUND"),
            "case {link_id}: a deleted link no longer exists, got {body:?}"
        );

        // And it is gone: neither readable nor listed, and not deletable twice.
        let (status, body) = call(&router, "GetAccountLink", &json!({ "LinkId": link_id })).await;
        assert_eq!(
            status, 400,
            "case {link_id}: a deleted link must not be readable, body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {link_id}: expected ResourceNotFoundException, got {body:?}"
        );
        let (status, body) = call_with_link(&router, "DeleteAccountLinkInvitation", link_id).await;
        assert_eq!(
            status, 400,
            "case {link_id}: a second delete must be refused, body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {link_id}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    let listed = list_all(&router, &json!({})).await;
    assert_eq!(
        listed,
        vec![kept],
        "only the links that were not deleted may be listed"
    );
}

#[tokio::test]
async fn a_target_account_is_invited_once_while_the_invitation_holds() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(
        &engine,
        &["000000000001", "000000000002", "000000000003"],
    );

    // Each row is a target invited twice: the second invitation is refused while
    // the first holds the account, and allowed once it no longer does.
    let cases = [
        ("still pending", None, 400),
        ("already linked", Some("AcceptAccountLinkInvitation"), 400),
        ("rejected", Some("RejectAccountLinkInvitation"), 200),
    ];
    for (index, (case, answer, expected_status)) in cases.into_iter().enumerate() {
        let target = format!("{:012}", index + 1);
        let link_id = invite_id(&router, &target).await;
        if let Some(operation) = answer {
            let (status, body) = call_with_link(&router, operation, &link_id).await;
            assert_eq!(status, 200, "case {case}: {operation} must be served, body {body:?}");
        }
        let (status, body) = call(
            &router,
            "CreateAccountLinkInvitation",
            &json!({ "TargetAccountId": target }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case}: expected ConflictException, got {body:?}"
            );
        } else {
            assert_eq!(
                body["AccountLink"]["AccountLinkStatus"].as_str(),
                Some(PENDING),
                "case {case}: a re-invitation awaits the target account again, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_client_token_makes_creation_idempotent() {
    let (router, engine) = emulator_with_engine();
    accounts_exist(
        &engine,
        &[
            "000000000001",
            "000000000002",
            "000000000003",
            "999999999999",
        ],
    );

    // Each row is a client token repeated for the same target: the repeat reports
    // the link the token already created rather than a second one.
    for (case, token, target) in [
        ("short token", "t", "000000000001"),
        (
            "64-character token",
            "0123456789012345678901234567890123456789012345678901234567890123",
            "000000000002",
        ),
        ("punctuated token", "fix/issue-42 retry", "000000000003"),
    ] {
        let request = json!({ "TargetAccountId": target, "ClientToken": token });
        let (status, first) = call(&router, "CreateAccountLinkInvitation", &request).await;
        assert_eq!(status, 200, "case {case}: the first create must be served, body {first:?}");
        let (status, second) = call(&router, "CreateAccountLinkInvitation", &request).await;
        assert_eq!(status, 200, "case {case}: the repeat must be served, body {second:?}");
        assert_eq!(
            second["AccountLink"], first["AccountLink"],
            "case {case}: the repeat must report the same link, got {second:?}"
        );

        // The same token for a DIFFERENT target is not a repeat of that request.
        let (status, body) = call(
            &router,
            "CreateAccountLinkInvitation",
            &json!({ "TargetAccountId": "999999999999", "ClientToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a token reused for another target must be refused, body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body:?}"
        );
    }

    // Three tokens, three links: no repeat created a fourth.
    let listed = list_all(&router, &json!({})).await;
    assert_eq!(
        listed.len(),
        3,
        "an idempotent repeat must not create a second link, got {listed:?}"
    );
}

#[tokio::test]
async fn invalid_account_link_requests_are_refused() {
    let (router, _engine) = emulator_with_engine();
    let unknown = "link-ffffffffffffffff";

    // Each row is a request the service refuses, with the error code it owes.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a target that is the source account",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": engine::emulator_account_id() }),
            "ValidationException",
        ),
        (
            "a target that is too short for ^\\d{12}$",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "12345" }),
            "ValidationException",
        ),
        (
            "a target that is not digits",
            "CreateAccountLinkInvitation",
            json!({ "TargetAccountId": "abcdefghijkl" }),
            "ValidationException",
        ),
        (
            "an accept of a link that does not exist",
            "AcceptAccountLinkInvitation",
            json!({ "LinkId": unknown }),
            "ResourceNotFoundException",
        ),
        (
            "a reject of a link that does not exist",
            "RejectAccountLinkInvitation",
            json!({ "LinkId": unknown }),
            "ResourceNotFoundException",
        ),
        (
            "a delete of a link that does not exist",
            "DeleteAccountLinkInvitation",
            json!({ "LinkId": unknown }),
            "ResourceNotFoundException",
        ),
        (
            "a get of a link that does not exist",
            "GetAccountLink",
            json!({ "LinkId": unknown }),
            "ResourceNotFoundException",
        ),
        (
            "a get of an account that is not linked",
            "GetAccountLink",
            json!({ "LinkedAccountId": "123456789012" }),
            "ResourceNotFoundException",
        ),
        (
            "a get naming neither a link nor an account",
            "GetAccountLink",
            json!({}),
            "ValidationException",
        ),
        (
            "a status filter the model does not declare",
            "ListAccountLinks",
            json!({ "LinkStatusFilter": ["NOT_A_STATUS"] }),
            "ValidationException",
        ),
        (
            "a next token this service never minted",
            "ListAccountLinks",
            json!({ "NextToken": "not-a-token" }),
            "ValidationException",
        ),
    ];
    for (case, operation, request, expected) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: the request must be refused, body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case}: expected {expected}, got {body:?}"
        );
    }

    // A refused request persisted nothing.
    let listed = list_all(&router, &json!({})).await;
    assert!(
        listed.is_empty(),
        "no refused request may persist a link, got {listed:?}"
    );
}

#[tokio::test]
async fn the_account_link_store_refuses_to_grow_past_its_bound() {
    let (router, engine) = emulator_with_engine();
    // Fill the store to its bound through the same state API the handler uses, so
    // the case costs one wire call rather than MAX_STORED_ACCOUNT_LINKS of them.
    {
        let state = service_state(&*engine).expect("the WorkSpaces state store");
        let mut state = state.lock();
        state.register_organization_account("999999999999");
        for index in 1..=MAX_STORED_ACCOUNT_LINKS {
            let target = format!("{index:012}");
            state.register_organization_account(&target);
            state
                .create_account_link_invitation(engine::emulator_account_id(), &target, None)
                .unwrap_or_else(|failure| {
                    panic!("case {index}: filling must succeed, got {failure:?}")
                });
        }
        assert_eq!(
            state.account_link_count(),
            MAX_STORED_ACCOUNT_LINKS,
            "the store must hold exactly its bound before the refused create"
        );
    }

    let (status, body) = call(
        &router,
        "CreateAccountLinkInvitation",
        &json!({ "TargetAccountId": "999999999999" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create past the bound must be refused, body {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException past the bound, got {body:?}"
    );
    let state = service_state(&*engine).expect("the WorkSpaces state store");
    assert_eq!(
        state.lock().account_link_count(),
        MAX_STORED_ACCOUNT_LINKS,
        "a refused create must not grow the store"
    );
}
