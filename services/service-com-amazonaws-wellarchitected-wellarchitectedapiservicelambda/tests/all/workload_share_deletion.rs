// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for revoking a share of a workload: `DeleteWorkloadShare`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The delete REVOKES the grant it names: the share stops being reported by
//!    `ListWorkloadShares` and stops resolving (the same request under a fresh token is a
//!    404), and the principal it was offered to can be granted a fresh share, which the 409
//!    a second grant gets while the share stands proves was impossible before. Rows differ
//!    along the FORM of the principal the revoked share was for — an account id, an
//!    organization id, an OU id and a value at each published `SharedWith` length bound —
//!    crossed with the permission the revoked share granted.
//! 2. A delete removes ONE share: the workload's other shares are still reported, unchanged,
//!    another workload's shares are untouched, and the workload itself still resolves. Rows
//!    differ along which of several shares is revoked.
//! 3. A delete that resolves nothing removes nothing and is a 404 NAMING the resource that
//!    was missing: the WORKLOAD where the id names no stored workload (even when the
//!    `ShareId` names a share of another workload, because the workload is resolved first),
//!    and the SHARE where the workload resolves and holds no share under the id. A `ShareId`
//!    outside the modelled pattern is refused at the boundary with a 400.
//! 4. The delete forgets the `ClientRequestToken` the revoked share was created under, so
//!    that token grants a FRESH share rather than replaying one that is gone.
//! 5. A retry of the delete is keyed to its own `ClientRequestToken`: the same token with
//!    the same parameters replays the empty success without touching the store, the same
//!    token with different parameters is a 409, and a fresh token naming a share that is
//!    gone is the 404 of rule 3. Rows differ along which parameter the reused token changes.
//! 6. Revoking a share withdraws the invitation it raised: `ListShareInvitations` stops
//!    reporting it.
//! 7. A share does not outlive the workload it shares: deleting the workload takes its
//!    shares with it, and a delete of one of them afterwards reports the WORKLOAD as
//!    missing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the workload builders, the `CreateWorkloadShare` /
//! `ListWorkloadShares` requests and the lookup-failure assertion are the shared
//! `super::workload_harness`'s; the `SharedWith` catalog and the `PENDING` literal are
//! `super::profile_shares`'s, because `SharedWith` is one member shared by every resource
//! kind this service shares.
//!
//! What a revocation left behind is read over HTTP through `ListWorkloadShares`. The store is
//! read only for rule 7, where the workload the list would be scoped to is itself gone, and
//! for the share a RULE-6 invitation stands for, which `record_workload_share` seeds for the
//! reason its doc gives: an invitation is offered to the principal a workload was shared
//! WITH, and a share whose recipient is the caller has no HTTP path in a single-account
//! emulator.

use serde_json::{json, Value};

use super::profile_shares::{NEW_SHARE_STATUS, OTHER_PRINCIPALS};
use super::share_invitations::{invitations, member as summary_member};
use super::workload_harness::{
    assert_not_found, created_id, encode_path_segment, member, minimal_create, Answer, Client,
};

/// A `ShareId` that is well-formed (`^[0-9a-f]{32}$`) and names no share.
const ABSENT_SHARE_ID: &str = "0123456789abcdef0123456789abcdef";

/// The `PermissionType` members the model publishes —
/// `smithy_query(query="shape", shape="PermissionType")` lists exactly these two.
const PERMISSION_TYPES: [&str; 2] = ["READONLY", "CONTRIBUTOR"];

/// Principals a revoked share may have been granted to: the three kinds `SharedWith`
/// publishes (`super::profile_shares::OTHER_PRINCIPALS`) plus one value at each of its
/// published `@length(12, 2048)` bounds, because the grant is of one workload to one
/// principal and nothing about revoking it depends on how that principal is spelled.
fn revoked_principals() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = OTHER_PRINCIPALS
        .iter()
        .map(|(case, principal)| ((*case).to_string(), (*principal).to_string()))
        .collect();
    rows.push((
        "a-principal-at-the-minimum-length".to_string(),
        format!("o-{}", "a".repeat(10)),
    ));
    rows.push((
        "a-principal-at-the-maximum-length".to_string(),
        format!("o-{}", "a".repeat(2046)),
    ));
    rows
}

/// `DELETE /workloads/{WorkloadId}/shares/{ShareId}?ClientRequestToken=…`, the wire form of
/// `DeleteWorkloadShare`: both labels are non-greedy, so each travels percent-encoded in one
/// path segment, and the `@idempotencyToken` is bound to the query string.
async fn delete_share(client: &Client, workload_id: &str, share_id: &str, token: &str) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/workloads/{}/shares/{}?ClientRequestToken={}",
                encode_path_segment(workload_id),
                encode_path_segment(share_id),
                encode_path_segment(token)
            ),
            None,
        )
        .await
}

/// Create a workload over HTTP and answer its `WorkloadId`.
async fn workload(client: &Client, name: &str) -> String {
    let created = client
        .create(minimal_create(name, &format!("create-{name}")))
        .await;
    created_id(&created, name)
}

/// `CreateWorkloadShare` for one (principal, permission) grant.
async fn create_share(
    client: &Client,
    workload_id: &str,
    shared_with: &str,
    permission_type: &str,
    token: &str,
) -> Answer {
    client
        .create_share(
            workload_id,
            json!({
                "SharedWith": shared_with,
                "PermissionType": permission_type,
                "ClientRequestToken": token,
            }),
        )
        .await
}

/// The `ShareId` a successful `CreateWorkloadShare` answered with, checked against the
/// modelled `^[0-9a-f]{32}$` pattern.
fn created_share_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkloadShare 200, got {answer:?}"
    );
    let id = member(answer, "ShareId").to_string();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}

/// The `WorkloadShareSummary` of every share `ListWorkloadShares` reports for
/// `workload_id`, in the order it reports them.
async fn listed_shares(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    let answer = client.list_shares(workload_id, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListWorkloadShares 200, got {answer:?}"
    );
    answer
        .body
        .get("WorkloadShareSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListWorkloadShares must report WorkloadShareSummaries, got \
                 {answer:?}"
            )
        })
}

/// The `ShareId`s `ListWorkloadShares` reports for `workload_id`, in the order it reports
/// them.
async fn listed_share_ids(client: &Client, workload_id: &str, case: &str) -> Vec<String> {
    listed_shares(client, workload_id, case)
        .await
        .iter()
        .map(|summary| {
            summary
                .get("ShareId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// How many shares the store holds against `workload_id` — read only where the workload the
/// list would be scoped to is itself gone.
fn stored_share_count(client: &Client, workload_id: &str) -> usize {
    let state = client.store();
    let store = state.lock();
    store.workload_shares_of(workload_id).len()
}

/// The refusal a `DeleteWorkloadShare` naming a share the workload does not hold gets.
fn assert_share_missing(case: &str, answer: &Answer, workload_id: &str, share_id: &str) {
    assert_not_found(
        case,
        answer,
        "WORKLOAD_SHARE",
        share_id,
        &format!("share with ID {share_id} of a workload with ID {workload_id}"),
    );
}

/// The refusal a `DeleteWorkloadShare` naming a workload the store does not hold gets.
fn assert_workload_missing(case: &str, answer: &Answer, workload_id: &str) {
    assert_not_found(
        case,
        answer,
        "WORKLOAD",
        workload_id,
        &format!("workload with ID {workload_id}"),
    );
}

/// Rule 1.
#[tokio::test]
async fn a_granted_share_is_revoked_by_the_delete() {
    let client = Client::new();
    let mut minted: Vec<String> = Vec::new();

    for (index, (case, shared_with)) in revoked_principals().into_iter().enumerate() {
        // The permission a share grants is irrelevant to revoking it, so the rows walk both
        // published members rather than holding one fixed.
        let permission_type = PERMISSION_TYPES[index % PERMISSION_TYPES.len()];
        let case = format!("{case}-{permission_type}");
        let case = case.as_str();
        let workload_id = workload(&client, &format!("revoked-{index}")).await;

        let granted = create_share(
            &client,
            &workload_id,
            &shared_with,
            permission_type,
            &format!("grant-{case}"),
        )
        .await;
        let share_id = created_share_id(&granted, case);
        assert!(
            !minted.contains(&share_id),
            "case {case:?}: every share must get its own ShareId, but {share_id:?} repeated"
        );
        minted.push(share_id.clone());
        assert_eq!(
            listed_share_ids(&client, &workload_id, case).await,
            vec![share_id.clone()],
            "case {case:?}: the grant must be the workload's one share before it is revoked"
        );

        // While the share stands, a second grant to the same principal is refused: that is
        // what the delete below makes possible again.
        let blocked = create_share(
            &client,
            &workload_id,
            &shared_with,
            permission_type,
            &format!("regrant-blocked-{case}"),
        )
        .await;
        assert_eq!(
            (blocked.status, blocked.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a principal holds at most one share of one workload, got {blocked:?}"
        );
        assert_eq!(
            (
                member(&blocked, "ResourceId"),
                member(&blocked, "ResourceType")
            ),
            (share_id.as_str(), "WORKLOAD_SHARE"),
            "case {case:?}: the conflict must name the share that is in the way, got {blocked:?}"
        );

        // The delete revokes the grant.
        let revoked =
            delete_share(&client, &workload_id, &share_id, &format!("revoke-{case}")).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteWorkloadShare 200, got {revoked:?}"
        );
        assert_eq!(
            revoked.body,
            Value::Null,
            "case {case:?}: DeleteWorkloadShare answers no members, got {revoked:?}"
        );
        let after = listed_shares(&client, &workload_id, case).await;
        assert!(
            after.is_empty(),
            "case {case:?}: the revoked share must stop being reported, got {after:?}"
        );

        // The share stops resolving: the same delete under a FRESH token is a 404.
        let again = delete_share(
            &client,
            &workload_id,
            &share_id,
            &format!("revoke-again-{case}"),
        )
        .await;
        assert_share_missing(case, &again, &workload_id, &share_id);

        // And the principal can be granted a fresh share, which the 409 above proves was
        // impossible while the revoked one stood.
        let regranted = create_share(
            &client,
            &workload_id,
            &shared_with,
            permission_type,
            &format!("regrant-{case}"),
        )
        .await;
        let fresh_id = created_share_id(&regranted, case);
        assert_ne!(
            fresh_id, share_id,
            "case {case:?}: a fresh grant must mint a new ShareId, got {regranted:?}"
        );
        assert_eq!(
            listed_share_ids(&client, &workload_id, case).await,
            vec![fresh_id.clone()],
            "case {case:?}: the re-granted share must be the workload's one share"
        );
        minted.push(fresh_id);

        // The workload itself is untouched by either the share or the revocation.
        let read_back = client.get(&workload_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the workload must still resolve after its share was revoked, got \
             {read_back:?}"
        );
    }
}

/// Rule 2. The rows differ along WHICH of a workload's shares is revoked, including its
/// first and its last in the order they are reported, because a share is resolved by its own
/// id and not by its position.
#[tokio::test]
async fn a_delete_revokes_one_share_and_leaves_the_others_standing() {
    for revoked_index in 0..OTHER_PRINCIPALS.len() {
        let case = format!("revoke-share-{revoked_index}");
        let case = case.as_str();
        let client = Client::new();
        let workload_id = workload(&client, "workload-with-several-shares").await;
        // A second workload, shared with the same principals, so the delete is also shown
        // not to reach a share of ANOTHER workload.
        let other_id = workload(&client, "another-workload-with-shares").await;

        let mut granted: Vec<(String, &str)> = Vec::new();
        for (principal_case, principal) in OTHER_PRINCIPALS {
            let answer = create_share(
                &client,
                &workload_id,
                principal,
                "READONLY",
                &format!("grant-{case}-{principal_case}"),
            )
            .await;
            granted.push((created_share_id(&answer, case), principal));
            let elsewhere = create_share(
                &client,
                &other_id,
                principal,
                "CONTRIBUTOR",
                &format!("grant-other-{case}-{principal_case}"),
            )
            .await;
            created_share_id(&elsewhere, case);
        }

        let before = listed_shares(&client, &workload_id, case).await;
        let other_before = listed_shares(&client, &other_id, case).await;
        let (revoked_id, revoked_principal) = granted[revoked_index].clone();

        let answer = delete_share(&client, &workload_id, &revoked_id, &format!("tok-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteWorkloadShare 200, got {answer:?}"
        );

        let expected: Vec<Value> = before
            .iter()
            .filter(|summary| {
                summary.get("ShareId").and_then(Value::as_str) != Some(revoked_id.as_str())
            })
            .cloned()
            .collect();
        assert_eq!(
            listed_shares(&client, &workload_id, case).await,
            expected,
            "case {case:?}: only the revoked share may be removed, and the others must be \
             reported unchanged"
        );
        assert_eq!(
            listed_shares(&client, &other_id, case).await,
            other_before,
            "case {case:?}: a delete must not reach another workload's shares"
        );

        // The revoked principal can be granted afresh; the others still collide.
        let regranted = create_share(
            &client,
            &workload_id,
            revoked_principal,
            "READONLY",
            &format!("regrant-{case}"),
        )
        .await;
        assert_eq!(
            regranted.status, 200,
            "case {case:?}: the revoked principal must be grantable again, got {regranted:?}"
        );
        for (share_id, principal) in &granted {
            if share_id == &revoked_id {
                continue;
            }
            let collided = create_share(
                &client,
                &workload_id,
                principal,
                "READONLY",
                &format!("still-blocked-{case}-{principal}"),
            )
            .await;
            assert_eq!(
                (collided.status, member(&collided, "ResourceId")),
                (409, share_id.as_str()),
                "case {case:?}: the share of {principal:?} must still stand, got {collided:?}"
            );
        }

        // The workload the delete named still resolves.
        let read_back = client.get(&workload_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: revoking a share must leave the workload itself alone, got \
             {read_back:?}"
        );
    }
}

/// Rule 3. The rows differ along WHAT does not resolve — the workload or the share — and
/// along the form of the identifier that names it, because the 404 has to name the resource
/// that was actually missing.
#[tokio::test]
async fn a_delete_that_resolves_nothing_removes_nothing() {
    let client = Client::new();
    let workload_id = workload(&client, "workload-under-refusals").await;
    let other_id = workload(&client, "workload-holding-the-share").await;
    let unshared_id = workload(&client, "workload-holding-no-share").await;

    let kept = create_share(
        &client,
        &workload_id,
        "111122223333",
        "READONLY",
        "grant-kept",
    )
    .await;
    let kept_id = created_share_id(&kept, "kept");
    let elsewhere = create_share(
        &client,
        &other_id,
        "444455556666",
        "CONTRIBUTOR",
        "grant-elsewhere",
    )
    .await;
    let elsewhere_id = created_share_id(&elsewhere, "elsewhere");

    // `WorkloadId`s of the modelled form (32 lowercase hex) that name no stored workload:
    // the rows differ along the digits the id is spelled with.
    let absent_workloads: [(&str, &str); 3] = [
        ("an-id-of-zeroes", "00000000000000000000000000000000"),
        ("an-id-of-f", "ffffffffffffffffffffffffffffffff"),
        ("a-mixed-id", "0123456789abcdef0123456789abcdef"),
    ];
    for (case, absent) in absent_workloads {
        // Even the id of a share that EXISTS elsewhere is a missing workload here: the
        // workload is resolved first, and one that does not resolve holds no shares.
        for share_id in [ABSENT_SHARE_ID, kept_id.as_str()] {
            let answer =
                delete_share(&client, absent, share_id, &format!("tok-{case}-{share_id}")).await;
            assert_workload_missing(case, &answer, absent);
        }
    }

    // A workload that resolves and holds no such share: the SHARE is what is missing.
    let missing_shares: Vec<(&str, &String, String)> = vec![
        (
            "a-share-id-no-share-carries",
            &workload_id,
            ABSENT_SHARE_ID.to_string(),
        ),
        (
            "a-share-id-of-another-workload",
            &workload_id,
            elsewhere_id.clone(),
        ),
        (
            "a-workload-that-holds-no-shares",
            &unshared_id,
            ABSENT_SHARE_ID.to_string(),
        ),
        (
            "a-share-of-another-workload-on-an-unshared-workload",
            &unshared_id,
            kept_id.clone(),
        ),
    ];
    for (case, id, share_id) in &missing_shares {
        let answer = delete_share(&client, id, share_id, &format!("tok-{case}")).await;
        assert_share_missing(case, &answer, id, share_id);
    }

    // A `ShareId` outside the modelled `^[0-9a-f]{32}$` never reaches the handler.
    for (case, share_id) in [
        ("not-hex", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string()),
        ("upper-case-hex", "0123456789ABCDEF0123456789ABCDEF".to_string()),
        ("one-character-short", "0".repeat(31)),
        ("one-character-long", "0".repeat(33)),
    ] {
        let answer = delete_share(&client, &workload_id, &share_id, &format!("tok-{case}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a ShareId outside the modelled pattern must be refused, got {answer:?}"
        );
    }

    // Nothing above removed anything.
    assert_eq!(
        listed_share_ids(&client, &workload_id, "refused").await,
        vec![kept_id.clone()],
        "a refused delete must leave the workload's shares exactly as they were"
    );
    assert_eq!(
        listed_share_ids(&client, &other_id, "refused").await,
        vec![elsewhere_id],
        "a refused delete must leave another workload's shares alone"
    );
    assert!(
        listed_shares(&client, &unshared_id, "refused")
            .await
            .is_empty(),
        "a refused delete must record nothing against a workload that holds no shares"
    );
}

/// Rules 4 and 5. The rows differ along WHICH parameter a reused delete token changes,
/// because the token identifies one request and both of the delete's parameters are part of
/// it.
#[tokio::test]
async fn the_client_request_token_decides_what_a_retry_of_the_delete_means() {
    // (case, whether the retry names the same parameters as the original delete)
    let cases: Vec<(&str, bool)> = vec![
        ("identical", true),
        ("another-share-of-the-same-workload", false),
        ("the-same-share-id-under-another-workload", false),
    ];

    for (case, same_request) in cases {
        let client = Client::new();
        let workload_id = workload(&client, "retried-share-workload").await;
        let other_id = workload(&client, "retried-share-other-workload").await;

        let grant_token = format!("grant-{case}");
        let granted = create_share(
            &client,
            &workload_id,
            "111122223333",
            "READONLY",
            &grant_token,
        )
        .await;
        let share_id = created_share_id(&granted, case);
        let kept = create_share(
            &client,
            &workload_id,
            "444455556666",
            "CONTRIBUTOR",
            &format!("kept-{case}"),
        )
        .await;
        let kept_id = created_share_id(&kept, case);

        let delete_token = format!("revoke-{case}");
        let revoked = delete_share(&client, &workload_id, &share_id, &delete_token).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteWorkloadShare 200, got {revoked:?}"
        );
        let after = listed_shares(&client, &workload_id, case).await;

        let retry = if same_request {
            delete_share(&client, &workload_id, &share_id, &delete_token).await
        } else if case == "another-share-of-the-same-workload" {
            delete_share(&client, &workload_id, &kept_id, &delete_token).await
        } else {
            delete_share(&client, &other_id, &share_id, &delete_token).await
        };

        if same_request {
            assert_eq!(
                (retry.status, &retry.body),
                (200, &Value::Null),
                "case {case:?}: a retry of the same delete must replay its empty success, got \
                 {retry:?}"
            );
        } else {
            assert_eq!(
                (retry.status, retry.error_code.as_str()),
                (409, "ConflictException"),
                "case {case:?}: the same token with different parameters must conflict, got \
                 {retry:?}"
            );
            assert_eq!(
                (member(&retry, "ResourceId"), member(&retry, "ResourceType")),
                (share_id.as_str(), "WORKLOAD_SHARE"),
                "case {case:?}: the conflict must name the share the token was spent on, got \
                 {retry:?}"
            );
        }
        assert_eq!(
            listed_shares(&client, &workload_id, case).await,
            after,
            "case {case:?}: a retry under a spent token must not touch the store"
        );

        // A FRESH token naming the revoked share is the 404 of rule 3, not a replay.
        let fresh = delete_share(&client, &workload_id, &share_id, &format!("fresh-{case}")).await;
        assert_share_missing(case, &fresh, &workload_id, &share_id);

        // Rule 4: the token the revoked share was GRANTED under is free again, so it grants
        // a fresh share rather than replaying the one that is gone.
        let regranted = create_share(
            &client,
            &workload_id,
            "111122223333",
            "READONLY",
            &grant_token,
        )
        .await;
        let fresh_id = created_share_id(&regranted, case);
        assert_ne!(
            fresh_id, share_id,
            "case {case:?}: the create token of a revoked share must grant afresh, got \
             {regranted:?}"
        );
    }
}

/// Rule 6. The invitation is read over HTTP for the principal the share was offered to,
/// which is why the share it stands for is seeded: the rows are the invitation before and
/// after the revocation.
#[tokio::test]
async fn revoking_a_share_withdraws_the_invitation_it_raised() {
    let client = Client::new();
    let caller = engine::emulator_account_id();
    let workload_name = "invited-workload";
    let workload_id = workload(&client, workload_name).await;
    let (share_id, invitation_id) =
        client.record_workload_share(&workload_id, caller, "READONLY", NEW_SHARE_STATUS);

    let offered = invitations(&client.list_share_invitations("").await, "offered");
    let invitation = offered
        .iter()
        .find(|summary| summary_member(summary, "ShareInvitationId") == invitation_id)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case \"offered\": the workload share's invitation must be reported, got \
                 {offered:?}"
            )
        });
    assert_eq!(
        (
            summary_member(&invitation, "ShareResourceType"),
            summary_member(&invitation, "WorkloadId"),
            summary_member(&invitation, "WorkloadName"),
        ),
        ("WORKLOAD", workload_id.as_str(), workload_name),
        "case \"offered\": a workload share's invitation must name the workload it shares, got \
         {invitation:?}"
    );

    let revoked = delete_share(&client, &workload_id, &share_id, "revoke-invited").await;
    assert_eq!(
        revoked.status, 200,
        "case \"revoked\": expected DeleteWorkloadShare 200, got {revoked:?}"
    );
    let remaining = invitations(&client.list_share_invitations("").await, "revoked");
    assert!(
        remaining
            .iter()
            .all(|summary| summary_member(summary, "ShareInvitationId") != invitation_id),
        "case \"revoked\": the withdrawn invitation must stop being reported, got {remaining:?}"
    );
}

/// Rule 7. The rows differ along how many shares the deleted workload held. The store is
/// read here because the workload the list would be scoped to is itself gone.
#[tokio::test]
async fn a_share_does_not_outlive_the_workload_it_shares() {
    for (case, principals) in [
        ("one-share", vec!["111122223333"]),
        ("several-shares", vec!["111122223333", "o-abcd1234efgh"]),
    ] {
        let client = Client::new();
        let workload_id = workload(&client, &format!("deleted-workload-{case}")).await;
        let mut share_ids: Vec<String> = Vec::new();
        for principal in &principals {
            let answer = create_share(
                &client,
                &workload_id,
                principal,
                "READONLY",
                &format!("grant-{case}-{principal}"),
            )
            .await;
            share_ids.push(created_share_id(&answer, case));
        }
        assert_eq!(
            stored_share_count(&client, &workload_id),
            principals.len(),
            "case {case:?}: every grant must have been recorded before the workload is deleted"
        );

        let deleted = client.delete(&workload_id, &format!("delete-{case}")).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteWorkload 200, got {deleted:?}"
        );
        assert_eq!(
            stored_share_count(&client, &workload_id),
            0,
            "case {case:?}: a deleted workload's shares must go with it"
        );

        // The workload is what is missing now, for every share it used to hold.
        for share_id in &share_ids {
            let answer = delete_share(
                &client,
                &workload_id,
                share_id,
                &format!("revoke-{case}-{share_id}"),
            )
            .await;
            assert_workload_missing(case, &answer, &workload_id);
        }
    }
}
