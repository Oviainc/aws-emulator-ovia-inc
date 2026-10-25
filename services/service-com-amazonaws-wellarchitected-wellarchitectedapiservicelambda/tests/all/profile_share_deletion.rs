// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for revoking a share of a Well-Architected Tool profile:
//! `DeleteProfileShare`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The delete REVOKES the grant it names: the share stops being reported by
//!    `ListProfileShares` and stops resolving (the same request again is a 404), and the
//!    principal it was offered to can be granted a fresh share, which the 409 a second
//!    grant gets while the share stands proves was impossible before. Rows differ along the
//!    FORM of the principal the revoked share was for — an account id, an organization id,
//!    an OU id and a value at each published `SharedWith` length bound.
//! 2. A delete removes ONE share: the profile's other shares are still reported, unchanged,
//!    another profile's shares are untouched, and the profile itself still resolves. Rows
//!    differ along which of several shares is revoked.
//! 3. A delete that resolves nothing removes nothing and is a 404 NAMING the resource that
//!    was missing: the PROFILE where the ARN names no stored profile (even when the
//!    `ShareId` names a share of another profile, because the profile is resolved first),
//!    and the SHARE where the profile resolves and holds no share under the id. A `ShareId`
//!    outside the modelled pattern is refused at the boundary with a 400.
//! 4. The delete forgets the `ClientRequestToken` the revoked share was created under, so
//!    that token grants a FRESH share rather than replaying one that is gone.
//! 5. A retry of the delete is keyed to its own `ClientRequestToken`: the same token with
//!    the same parameters replays the empty success without touching the store, the same
//!    token with different parameters is a 409, and a fresh token naming a share that is
//!    gone is the 404 of rule 3. Rows differ along which parameter the reused token
//!    changes.
//! 6. Revoking a share withdraws the invitation it raised: `ListShareInvitations` stops
//!    reporting it.
//! 7. A share does not outlive the profile it shares: deleting the profile takes its shares
//!    with it, and a delete of one of them afterwards reports the PROFILE as missing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the profile-minting helper and the lookup-failure assertion
//! are the shared `super::workload_harness`'s; the shares this suite revokes are granted and
//! read back through `super::profile_shares`'s own `CreateProfileShare` /
//! `ListProfileShares` helpers — one spelling of each request for both suites.
//!
//! What a revocation left behind is read over HTTP through `ListProfileShares`. The store is
//! read only for rule 7, where the profile the list would be scoped to is itself gone, and
//! for the share a RULE-6 invitation stands for, which `record_profile_share` seeds for the
//! reason its doc gives: an invitation is offered to the principal a profile was shared
//! WITH, and a share whose recipient is the caller has no HTTP path in a single-account
//! emulator.

use serde_json::Value;

use super::profile_shares::{
    create_share, created_share_id, list_shares, record_profile_share, stored_share_count,
    summaries, NEW_SHARE_STATUS, OTHER_PRINCIPALS,
};
use super::share_invitations::{invitations, member as summary_member};
use super::workload_harness::{
    assert_not_found, created_profile, encode_path_segment, member, Answer, Client,
};

/// A `ShareId` that is well-formed (`^[0-9a-f]{32}$`) and names no share.
const ABSENT_SHARE_ID: &str = "0123456789abcdef0123456789abcdef";

/// Principals a revoked share may have been granted to: the three kinds `SharedWith`
/// publishes (`super::profile_shares::OTHER_PRINCIPALS`) plus one value at each of its
/// published `@length(12, 2048)` bounds, because the grant is of one profile to one
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

/// `DELETE /profiles/{ProfileArn}/shares/{ShareId}?ClientRequestToken=…`, the wire form of
/// `DeleteProfileShare`: both labels are non-greedy, so each travels percent-encoded in one
/// path segment, and the `@idempotencyToken` is bound to the query string.
async fn delete_share(client: &Client, profile_arn: &str, share_id: &str, token: &str) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/profiles/{}/shares/{}?ClientRequestToken={}",
                encode_path_segment(profile_arn),
                encode_path_segment(share_id),
                encode_path_segment(token)
            ),
            None,
        )
        .await
}

/// The `ProfileShareSummary` of every share `ListProfileShares` reports for `profile_arn`,
/// in the order it reports them.
async fn listed_shares(client: &Client, profile_arn: &str, case: &str) -> Vec<Value> {
    summaries(&list_shares(client, profile_arn, "").await, case)
}

/// The `ShareId`s `ListProfileShares` reports for `profile_arn`, in the order it reports
/// them.
async fn listed_share_ids(client: &Client, profile_arn: &str, case: &str) -> Vec<String> {
    listed_shares(client, profile_arn, case)
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

/// The refusal a `DeleteProfileShare` naming a share the profile does not hold gets.
fn assert_share_missing(case: &str, answer: &Answer, profile_arn: &str, share_id: &str) {
    assert_not_found(
        case,
        answer,
        "PROFILE_SHARE",
        share_id,
        &format!("share with ID {share_id} of a profile with ARN {profile_arn}"),
    );
}

/// The refusal a `DeleteProfileShare` naming a profile the store does not hold gets.
fn assert_profile_missing(case: &str, answer: &Answer, profile_arn: &str) {
    assert_not_found(
        case,
        answer,
        "PROFILE",
        profile_arn,
        &format!("profile with ARN {profile_arn}"),
    );
}

/// Rule 1.
#[tokio::test]
async fn a_granted_share_is_revoked_by_the_delete() {
    let client = Client::new();
    let mut minted: Vec<String> = Vec::new();

    for (case, shared_with) in revoked_principals() {
        let case = case.as_str();
        let (profile_arn, _) = created_profile(&client, &format!("revoked-{case}")).await;

        let granted =
            create_share(&client, &profile_arn, &shared_with, &format!("grant-{case}")).await;
        let share_id = created_share_id(&granted, case);
        assert!(
            !minted.contains(&share_id),
            "case {case:?}: every share must get its own ShareId, but {share_id:?} repeated"
        );
        minted.push(share_id.clone());
        assert_eq!(
            listed_share_ids(&client, &profile_arn, case).await,
            vec![share_id.clone()],
            "case {case:?}: the grant must be the profile's one share before it is revoked"
        );

        // While the share stands, a second grant to the same principal is refused: that is
        // what the delete below makes possible again.
        let blocked = create_share(
            &client,
            &profile_arn,
            &shared_with,
            &format!("regrant-blocked-{case}"),
        )
        .await;
        assert_eq!(
            (blocked.status, blocked.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a principal holds at most one share of one profile, got {blocked:?}"
        );
        assert_eq!(
            (
                member(&blocked, "ResourceId"),
                member(&blocked, "ResourceType")
            ),
            (share_id.as_str(), "PROFILE_SHARE"),
            "case {case:?}: the conflict must name the share that is in the way, got {blocked:?}"
        );

        // The delete revokes the grant.
        let revoked =
            delete_share(&client, &profile_arn, &share_id, &format!("revoke-{case}")).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteProfileShare 200, got {revoked:?}"
        );
        assert_eq!(
            revoked.body,
            Value::Null,
            "case {case:?}: DeleteProfileShare answers no members, got {revoked:?}"
        );
        assert!(
            listed_shares(&client, &profile_arn, case).await.is_empty(),
            "case {case:?}: the revoked share must stop being reported, got {:?}",
            listed_shares(&client, &profile_arn, case).await
        );

        // The share stops resolving: the same delete under a FRESH token is a 404.
        let again = delete_share(
            &client,
            &profile_arn,
            &share_id,
            &format!("revoke-again-{case}"),
        )
        .await;
        assert_share_missing(case, &again, &profile_arn, &share_id);

        // And the principal can be granted a fresh share, which the 409 above proves was
        // impossible while the revoked one stood.
        let regranted = create_share(
            &client,
            &profile_arn,
            &shared_with,
            &format!("regrant-{case}"),
        )
        .await;
        let fresh_id = created_share_id(&regranted, case);
        assert_ne!(
            fresh_id, share_id,
            "case {case:?}: a fresh grant must mint a new ShareId, got {regranted:?}"
        );
        assert_eq!(
            listed_share_ids(&client, &profile_arn, case).await,
            vec![fresh_id.clone()],
            "case {case:?}: the re-granted share must be the profile's one share"
        );
        minted.push(fresh_id);

        // The profile itself is untouched by either the share or the revocation.
        let profile = client.get_profile(&profile_arn, None).await;
        assert_eq!(
            profile.status, 200,
            "case {case:?}: the profile must still resolve after its share was revoked, got \
             {profile:?}"
        );
    }
}

/// Rule 2. The rows differ along WHICH of a profile's shares is revoked, including its
/// first and its last in the order they are reported, because a share is resolved by its
/// own id and not by its position.
#[tokio::test]
async fn a_delete_revokes_one_share_and_leaves_the_others_standing() {
    for revoked_index in 0..OTHER_PRINCIPALS.len() {
        let case = format!("revoke-share-{revoked_index}");
        let case = case.as_str();
        let client = Client::new();
        let (profile_arn, _) = created_profile(&client, "profile-with-several-shares").await;
        // A second profile, shared with the same principals, so the delete is also shown
        // not to reach a share of ANOTHER profile.
        let (other_arn, _) = created_profile(&client, "another-profile-with-shares").await;

        let mut granted: Vec<(String, &str)> = Vec::new();
        for (principal_case, principal) in OTHER_PRINCIPALS {
            let answer = create_share(
                &client,
                &profile_arn,
                principal,
                &format!("grant-{case}-{principal_case}"),
            )
            .await;
            granted.push((created_share_id(&answer, case), principal));
            let elsewhere = create_share(
                &client,
                &other_arn,
                principal,
                &format!("grant-other-{case}-{principal_case}"),
            )
            .await;
            created_share_id(&elsewhere, case);
        }

        let before = listed_shares(&client, &profile_arn, case).await;
        let other_before = listed_shares(&client, &other_arn, case).await;
        let (revoked_id, revoked_principal) = granted[revoked_index].clone();

        let answer = delete_share(&client, &profile_arn, &revoked_id, &format!("tok-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteProfileShare 200, got {answer:?}"
        );

        let expected: Vec<Value> = before
            .iter()
            .filter(|summary| {
                summary.get("ShareId").and_then(Value::as_str) != Some(revoked_id.as_str())
            })
            .cloned()
            .collect();
        assert_eq!(
            listed_shares(&client, &profile_arn, case).await,
            expected,
            "case {case:?}: only the revoked share may be removed, and the others must be \
             reported unchanged"
        );
        assert_eq!(
            listed_shares(&client, &other_arn, case).await,
            other_before,
            "case {case:?}: a delete must not reach another profile's shares"
        );

        // The revoked principal can be granted afresh; the others still collide.
        let regranted = create_share(
            &client,
            &profile_arn,
            revoked_principal,
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
                &profile_arn,
                principal,
                &format!("still-blocked-{case}-{principal}"),
            )
            .await;
            assert_eq!(
                (collided.status, member(&collided, "ResourceId")),
                (409, share_id.as_str()),
                "case {case:?}: the share of {principal:?} must still stand, got {collided:?}"
            );
        }
    }
}

/// Rule 3. The rows differ along WHAT does not resolve — the profile or the share — and
/// along the form of the identifier that names it, because the 404 has to name the resource
/// that was actually missing.
#[tokio::test]
async fn a_delete_that_resolves_nothing_removes_nothing() {
    let client = Client::new();
    let (profile_arn, _) = created_profile(&client, "profile-under-refusals").await;
    let (other_arn, _) = created_profile(&client, "profile-holding-the-share").await;
    let (unshared_arn, _) = created_profile(&client, "profile-holding-no-share").await;

    let kept = create_share(&client, &profile_arn, "111122223333", "grant-kept").await;
    let kept_id = created_share_id(&kept, "kept");
    let elsewhere = create_share(&client, &other_arn, "444455556666", "grant-elsewhere").await;
    let elsewhere_id = created_share_id(&elsewhere, "elsewhere");

    // ARNs of the modelled form that name no stored profile: the rows differ along the
    // resource part and along the account the ARN names.
    let absent_profiles: Vec<(&str, String)> = vec![
        (
            "an-arn-naming-no-stored-profile",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:profile/0123456789abcdef0123456789abcdef",
                engine::emulator_account_id()
            ),
        ),
        (
            "an-arn-whose-resource-id-is-one-character",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:profile/0",
                engine::emulator_account_id()
            ),
        ),
        (
            "an-arn-of-another-account",
            "arn:aws:wellarchitected:us-west-2:111122223333:profile/abc123".to_string(),
        ),
    ];
    for (case, arn) in &absent_profiles {
        // Even the id of a share that EXISTS elsewhere is a missing profile here: the
        // profile is resolved first, and one that does not resolve holds no shares.
        for share_id in [ABSENT_SHARE_ID, kept_id.as_str()] {
            let answer =
                delete_share(&client, arn, share_id, &format!("tok-{case}-{share_id}")).await;
            assert_profile_missing(case, &answer, arn);
        }
    }

    // A profile that resolves and holds no such share: the SHARE is what is missing.
    let missing_shares: Vec<(&str, &String, String)> = vec![
        (
            "a-share-id-no-share-carries",
            &profile_arn,
            ABSENT_SHARE_ID.to_string(),
        ),
        (
            "a-share-id-of-another-profile",
            &profile_arn,
            elsewhere_id.clone(),
        ),
        (
            "a-profile-that-holds-no-shares",
            &unshared_arn,
            ABSENT_SHARE_ID.to_string(),
        ),
        (
            "a-share-of-another-profile-on-an-unshared-profile",
            &unshared_arn,
            kept_id.clone(),
        ),
    ];
    for (case, arn, share_id) in &missing_shares {
        let answer = delete_share(&client, arn, share_id, &format!("tok-{case}")).await;
        assert_share_missing(case, &answer, arn, share_id);
    }

    // A `ShareId` outside the modelled `^[0-9a-f]{32}$` never reaches the handler.
    for (case, share_id) in [
        ("not-hex", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string()),
        ("upper-case-hex", "0123456789ABCDEF0123456789ABCDEF".to_string()),
        ("one-character-short", "0".repeat(31)),
        ("one-character-long", "0".repeat(33)),
    ] {
        let answer = delete_share(&client, &profile_arn, &share_id, &format!("tok-{case}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a ShareId outside the modelled pattern must be refused, got {answer:?}"
        );
    }

    // Nothing above removed anything.
    assert_eq!(
        listed_share_ids(&client, &profile_arn, "refused").await,
        vec![kept_id.clone()],
        "a refused delete must leave the profile's shares exactly as they were"
    );
    assert_eq!(
        listed_share_ids(&client, &other_arn, "refused").await,
        vec![elsewhere_id],
        "a refused delete must leave another profile's shares alone"
    );
    assert!(
        listed_shares(&client, &unshared_arn, "refused")
            .await
            .is_empty(),
        "a refused delete must record nothing against a profile that holds no shares"
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
        ("another-share-of-the-same-profile", false),
        ("the-same-share-id-under-another-profile", false),
    ];

    for (case, same_request) in cases {
        let client = Client::new();
        let (profile_arn, _) = created_profile(&client, "retried-share-profile").await;
        let (other_arn, _) = created_profile(&client, "retried-share-other-profile").await;

        let grant_token = format!("grant-{case}");
        let granted = create_share(&client, &profile_arn, "111122223333", &grant_token).await;
        let share_id = created_share_id(&granted, case);
        let kept =
            create_share(&client, &profile_arn, "444455556666", &format!("kept-{case}")).await;
        let kept_id = created_share_id(&kept, case);

        let delete_token = format!("revoke-{case}");
        let revoked = delete_share(&client, &profile_arn, &share_id, &delete_token).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteProfileShare 200, got {revoked:?}"
        );
        let after = listed_shares(&client, &profile_arn, case).await;

        let retry = if same_request {
            delete_share(&client, &profile_arn, &share_id, &delete_token).await
        } else if case == "another-share-of-the-same-profile" {
            delete_share(&client, &profile_arn, &kept_id, &delete_token).await
        } else {
            delete_share(&client, &other_arn, &share_id, &delete_token).await
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
                (share_id.as_str(), "PROFILE_SHARE"),
                "case {case:?}: the conflict must name the share the token was spent on, got \
                 {retry:?}"
            );
        }
        assert_eq!(
            listed_shares(&client, &profile_arn, case).await,
            after,
            "case {case:?}: a retry under a spent token must not touch the store"
        );

        // A FRESH token naming the revoked share is the 404 of rule 3, not a replay.
        let fresh = delete_share(&client, &profile_arn, &share_id, &format!("fresh-{case}")).await;
        assert_share_missing(case, &fresh, &profile_arn, &share_id);

        // Rule 4: the token the revoked share was GRANTED under is free again, so it grants
        // a fresh share rather than replaying the one that is gone.
        let regranted = create_share(&client, &profile_arn, "111122223333", &grant_token).await;
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
    let profile_name = "invited-profile";
    let (profile_arn, _) = created_profile(&client, profile_name).await;
    let (share_id, invitation_id) =
        record_profile_share(&client, &profile_arn, caller, NEW_SHARE_STATUS);

    let offered = invitations(&client.list_share_invitations("").await, "offered");
    let invitation = offered
        .iter()
        .find(|summary| summary_member(summary, "ShareInvitationId") == invitation_id)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case \"offered\": the profile share's invitation must be reported, got {offered:?}"
            )
        });
    assert_eq!(
        (
            summary_member(&invitation, "ShareResourceType"),
            summary_member(&invitation, "ProfileArn"),
            summary_member(&invitation, "ProfileName"),
        ),
        ("PROFILE", profile_arn.as_str(), profile_name),
        "case \"offered\": a profile share's invitation must name the profile it shares, got \
         {invitation:?}"
    );

    let revoked = delete_share(&client, &profile_arn, &share_id, "revoke-invited").await;
    assert_eq!(
        revoked.status, 200,
        "case \"revoked\": expected DeleteProfileShare 200, got {revoked:?}"
    );
    let remaining = invitations(&client.list_share_invitations("").await, "revoked");
    assert!(
        remaining
            .iter()
            .all(|summary| summary_member(summary, "ShareInvitationId") != invitation_id),
        "case \"revoked\": the withdrawn invitation must stop being reported, got {remaining:?}"
    );
}

/// Rule 7. The rows differ along how many shares the deleted profile held. The store is
/// read here because the profile the list would be scoped to is itself gone.
#[tokio::test]
async fn a_share_does_not_outlive_the_profile_it_shares() {
    for (case, principals) in [
        ("one-share", vec!["111122223333"]),
        ("several-shares", vec!["111122223333", "o-abcd1234efgh"]),
    ] {
        let client = Client::new();
        let (profile_arn, _) = created_profile(&client, &format!("deleted-profile-{case}")).await;
        let mut share_ids: Vec<String> = Vec::new();
        for principal in &principals {
            let answer = create_share(
                &client,
                &profile_arn,
                principal,
                &format!("grant-{case}-{principal}"),
            )
            .await;
            share_ids.push(created_share_id(&answer, case));
        }
        assert_eq!(
            stored_share_count(&client, &profile_arn),
            principals.len(),
            "case {case:?}: every grant must have been recorded before the profile is deleted"
        );

        let deleted = client
            .delete_profile(&profile_arn, &format!("delete-{case}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteProfile 200, got {deleted:?}"
        );
        assert_eq!(
            stored_share_count(&client, &profile_arn),
            0,
            "case {case:?}: a deleted profile's shares must go with it"
        );

        // The profile is what is missing now, for every share it used to hold.
        for share_id in &share_ids {
            let answer = delete_share(
                &client,
                &profile_arn,
                share_id,
                &format!("revoke-{case}-{share_id}"),
            )
            .await;
            assert_profile_missing(case, &answer, &profile_arn);
        }
    }
}
