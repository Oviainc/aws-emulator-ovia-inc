// SPDX-License-Identifier: CC0-1.0

//! Invitations, end to end over HTTP: `ListInvitations` and `RejectInvitation`.
//!
//! An invitation enters this service's store when an approved `InviteAction` is carried
//! out — "The `InviteAction` is carried out when a `Proposal` is `APPROVED`"
//! (smithy_query: shape `InviteAction`) — which is
//! `state::record_invitation`. `VoteOnProposal`, the call that drives it once the votes
//! approve a proposal, is exercised end to end in `vote_on_proposal.rs`; these cases
//! record the invitations through that one state entry point, so each row arranges the
//! listing it asserts on directly, and then drive every assertion through the router the
//! production emulator mounts.
//!
//! The tables span the classes the rules generalise over: how many invitations are held,
//! page sizes from one to past the item count plus both published `MaxResults` bounds and
//! one past each, the proposal durations an `ExpirationDate` is computed from (both
//! published bounds and an omitted policy), the account an invitation names, and every
//! status a reject can be attempted from.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    service_state, INVITATION_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::network_and_node_lifecycle::{
    call, create_network, create_network_body, is_date_time, is_resource_id, router_over, ACCOUNT,
    REGION,
};

/// An account that is not the one this emulator answers for, so an invitation naming it is
/// not an invitation "for the current Amazon Web Services account".
const OTHER_ACCOUNT: &str = "210987654321";

/// A router and the store its handlers read and write — the SAME engine, so a test can
/// record the invitations an approved `InviteAction` would have minted.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// Record one invitation to `principal` onto `network_id`, created at `created`, and
/// answer the invitation id the service minted for it.
fn invite(
    engine: &engine::InMemoryEngine,
    network_id: &str,
    principal: &str,
    created: SystemTime,
) -> String {
    service_state(engine)
        .expect("this service's own store")
        .lock()
        .record_invitation(network_id, principal, REGION, created)
        .expect("the network exists and the store has room for another invitation")
}

/// `GET /invitations`, with the query string `query` appended when it is not empty.
async fn list(router: &MultiServiceRouter, query: &str) -> Value {
    let path = if query.is_empty() {
        "/invitations".to_string()
    } else {
        format!("/invitations?{query}")
    };
    let answer = call(router, "GET", &path, None).await;
    assert_eq!(
        answer.status, 200,
        "case {path}: expected ListInvitations 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `Invitations` array of a listing body.
fn items(body: &Value) -> Vec<Value> {
    body["Invitations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case listing: expected an Invitations array, got {body:?}"))
}

/// The invitation whose `InvitationId` is `id`, from a listing body.
fn find(body: &Value, id: &str) -> Value {
    items(body)
        .into_iter()
        .find(|item| item["InvitationId"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {id}: expected the listing to hold it, got {body:?}"))
}

/// A `Timestamp` member of a listing item, in seconds since the epoch.
fn seconds(item: &Value, member: &str) -> f64 {
    let text = item[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {member}: expected a date-time string, got {item:?}"));
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("case {member}: {text} does not parse: {error}"))
        .as_secs_f64()
}

/// A `CreateNetwork` body whose voting policy names `duration` proposal hours, or none at
/// all when `duration` is `None`.
fn network_body(token: &str, name: &str, duration: Option<i32>) -> Value {
    let mut body = create_network_body(token, name);
    match duration {
        Some(hours) => body["VotingPolicy"]["ApprovalThresholdPolicy"]
            ["ProposalDurationInHours"] = json!(hours),
        None => {
            body["VotingPolicy"] = json!({
                "ApprovalThresholdPolicy": {
                    "ThresholdPercentage": 50,
                    "ThresholdComparator": "GREATER_THAN"
                }
            });
        }
    }
    body
}

/// `ListInvitations` answers from the stored records: an empty store lists nothing, and
/// every invitation this account holds is reported with its stored id, ARN, dates, status
/// and the summary of the network it is onto.
#[tokio::test]
async fn every_stored_invitation_for_this_account_is_reported() {
    let (router, engine) = router_and_engine();

    // Nothing recorded yet: a real listing of an empty store, not a refusal.
    let empty = list(&router, "").await;
    assert!(
        items(&empty).is_empty(),
        "case empty store: expected no invitations, got {empty:?}"
    );

    let (network_id, _member_id) =
        create_network(&router, network_body("inv-tok", "inv-net", Some(24))).await;

    // Rows: a growing number of invitations for this account, plus one naming another
    // account, which is not this caller's to see.
    let now = SystemTime::now();
    let mine: Vec<String> = (0..3)
        .map(|_| invite(&engine, &network_id, ACCOUNT, now))
        .collect();
    let theirs = invite(&engine, &network_id, OTHER_ACCOUNT, now);

    let body = list(&router, "").await;
    let mut listed: Vec<String> = items(&body)
        .iter()
        .map(|item| item["InvitationId"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected = mine.clone();
    listed.sort();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case three invitations for this account: expected exactly them, got {body:?}"
    );
    assert!(
        !listed.contains(&theirs),
        "case another account's invitation: it must not be listed, got {body:?}"
    );

    let network_arn = format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:networks/{network_id}");
    for id in &mine {
        let item = find(&body, id);
        assert!(
            is_resource_id(id, "i"),
            "case {id}: expected a minted invitation id, got {item:?}"
        );
        for (member, expected) in [
            ("Status", "PENDING".to_string()),
            (
                "Arn",
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:invitations/{id}"),
            ),
        ] {
            assert_eq!(
                item[member].as_str().unwrap_or_default(),
                expected,
                "case {id}: expected {member} {expected}, got {item:?}"
            );
        }
        for member in ["CreationDate", "ExpirationDate"] {
            assert!(
                is_date_time(&item[member]),
                "case {id}: expected {member} as a date-time, got {item:?}"
            );
        }
        let summary = &item["NetworkSummary"];
        for (member, expected) in [
            ("Id", network_id.clone()),
            ("Name", "inv-net".to_string()),
            ("Framework", "HYPERLEDGER_FABRIC".to_string()),
            ("Arn", network_arn.clone()),
        ] {
            assert_eq!(
                summary[member].as_str().unwrap_or_default(),
                expected,
                "case {id}: expected NetworkSummary.{member} {expected}, got {summary:?}"
            );
        }
    }

    // A read changes nothing a later caller sees: the same listing twice over.
    let again = list(&router, "").await;
    assert_eq!(
        items(&again).len(),
        mine.len(),
        "case a repeated listing: expected {} invitations, got {again:?}",
        mine.len()
    );
}

/// The listing walks every invitation exactly once, in stored-id order, at any page size,
/// and refuses a page size outside the modelled range or a token it never issued.
#[tokio::test]
async fn the_listing_pages_in_stored_id_order() {
    let (router, engine) = router_and_engine();
    let (network_id, _member_id) =
        create_network(&router, network_body("page-tok", "page-net", Some(1))).await;

    let now = SystemTime::now();
    let mut expected: Vec<String> = (0..7)
        .map(|_| invite(&engine, &network_id, ACCOUNT, now))
        .collect();
    // The listing is ordered by the stored id, which is what the page cursor keys on.
    expected.sort();

    // Page sizes from one to past the item count, including both published `MaxResults`
    // bounds (@range(1, 100)).
    for page_size in [1_usize, 2, 3, 7, 8, INVITATION_MAX_RESULTS as usize] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let body = list(&router, &query).await;
            let page = items(&body);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page holds at most MaxResults items, got {}",
                page.len()
            );
            collected.extend(
                page.iter()
                    .map(|item| item["InvitationId"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case page size {page_size}: paging must terminate, got {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case page size {page_size}: paging must walk every invitation exactly once, in id \
             order, got {collected:?}"
        );
    }

    // One page size past each published bound is refused by the modelled range.
    for (case, page_size) in [("below", 0), ("above", INVITATION_MAX_RESULTS + 1)] {
        let answer = call(
            &router,
            "GET",
            &format!("/invitations?maxResults={page_size}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {case} the range: expected 400, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A token this service never issued cannot resume a page.
    let answer = call(&router, "GET", "/invitations?nextToken=not-a-token", None).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case a forged nextToken: expected 400 InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );
}

/// `Invitation$ExpirationDate` is "the `CreationDate` plus the `ProposalDurationInHours`
/// that is specified in the `ProposalThresholdPolicy`" of the inviting network.
#[tokio::test]
async fn the_expiration_date_is_the_creation_date_plus_the_networks_proposal_duration() {
    // Rows: both published bounds of `ProposalDurationInt` (@range(1, 168)), one in
    // between, and a policy that names no duration at all — which falls back on the
    // emulator's own `DEFAULT_PROPOSAL_DURATION_IN_HOURS`.
    let cases: &[(&str, Option<i32>, i64)] = &[
        ("lower bound", Some(1), 1),
        ("in between", Some(72), 72),
        ("upper bound", Some(168), 168),
        ("omitted", None, 24),
    ];

    for (case, configured, expected_hours) in cases {
        let (router, engine) = router_and_engine();
        let (network_id, _member_id) = create_network(
            &router,
            network_body(&format!("exp-{case}"), "exp-net", *configured),
        )
        .await;
        let id = invite(&engine, &network_id, ACCOUNT, SystemTime::now());

        let item = find(&list(&router, "").await, &id);
        let span = seconds(&item, "ExpirationDate") - seconds(&item, "CreationDate");
        let expected = (*expected_hours * 3_600) as f64;
        assert_eq!(
            span, expected,
            "case {case}: expected ExpirationDate {expected} seconds after CreationDate, got \
             {span} ({item:?})"
        );
    }
}

/// `RejectInvitation` rejects the invitation it names, and only from `PENDING`: an id that
/// names nothing, or an invitation issued to another account, is not found, and one that
/// has already left `PENDING` is an illegal action.
#[tokio::test]
async fn rejecting_moves_a_pending_invitation_to_rejected_and_nothing_else() {
    let (router, engine) = router_and_engine();
    let (network_id, _member_id) =
        create_network(&router, network_body("rej-tok", "rej-net", Some(24))).await;

    let now = SystemTime::now();
    let pending = invite(&engine, &network_id, ACCOUNT, now);
    let theirs = invite(&engine, &network_id, OTHER_ACCOUNT, now);
    // Created far enough in the past that its expiry already fell due, so the listing
    // below reports the `EXPIRED` the clock owed it.
    let expired = invite(
        &engine,
        &network_id,
        ACCOUNT,
        now - Duration::from_secs(48 * 3_600),
    );

    let body = list(&router, "").await;
    for (id, expected) in [(&pending, "PENDING"), (&expired, "EXPIRED")] {
        let item = find(&body, id);
        assert_eq!(
            item["Status"].as_str().unwrap_or_default(),
            expected,
            "case {id}: expected status {expected} before any reject, got {item:?}"
        );
    }

    // Rejecting the pending invitation succeeds exactly once; the second attempt is an
    // illegal action on a resource that is no longer pending.
    for (case, expected_status, expected_error) in [
        ("first reject", 200, ""),
        ("repeated reject", 400, "IllegalActionException"),
    ] {
        let answer = call(&router, "DELETE", &format!("/invitations/{pending}"), None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (expected_status, expected_error),
            "case {case}: expected HTTP {expected_status} {expected_error}, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // Rows that name no invitation of this account's, each refused as not found: an id of
    // the right shape that nothing holds, one that belongs to another account, and the
    // network's own id.
    for (case, id) in [
        ("unknown id", "i-ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string()),
        ("another account's invitation", theirs.clone()),
        ("a network id", network_id.clone()),
    ] {
        let answer = call(&router, "DELETE", &format!("/invitations/{id}"), None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // An invitation the clock already expired cannot be rejected either.
    let answer = call(&router, "DELETE", &format!("/invitations/{expired}"), None).await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "IllegalActionException"),
        "case an expired invitation: expected 400 IllegalActionException, got {} {:?}",
        answer.status,
        answer.body
    );

    // The round trip: the listing reports exactly what the rejects did, and nothing else
    // moved.
    let body = list(&router, "").await;
    for (id, expected) in [(&pending, "REJECTED"), (&expired, "EXPIRED")] {
        let item = find(&body, id);
        assert_eq!(
            item["Status"].as_str().unwrap_or_default(),
            expected,
            "case {id}: expected status {expected} after the rejects, got {item:?}"
        );
    }
    assert_eq!(
        items(&body).len(),
        2,
        "case after the rejects: a rejected invitation is still reported, got {body:?}"
    );
    // The other account's invitation survived every refusal, untouched.
    let answer = call(&router, "GET", "/invitations", None).await;
    assert!(
        !answer.body.to_string().contains(&theirs),
        "case another account's invitation: still not listed, got {:?}",
        answer.body
    );
}
