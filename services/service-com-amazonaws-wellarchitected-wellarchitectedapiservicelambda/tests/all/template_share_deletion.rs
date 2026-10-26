// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for revoking a share of a review template:
//! `DeleteTemplateShare`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The delete REVOKES the grant it names: the share stops being reported by
//!    `ListTemplateShares` and stops resolving (the same request under a fresh token is a
//!    404), and the principal it was offered to can be granted a fresh share, which the 409
//!    a second grant gets while the share stands proves was impossible before. Rows differ
//!    along the FORM of the principal the revoked share was for — an account id, an
//!    organization id, an OU id and a value at each published `SharedWith` length bound.
//! 2. A delete removes ONE share: the template's other shares are still reported, unchanged,
//!    another template's shares are untouched, and the template itself still resolves. Rows
//!    differ along which of several shares is revoked.
//! 3. A delete that resolves nothing removes nothing and is a 404 NAMING the resource that
//!    was missing: the REVIEW TEMPLATE where the ARN names no stored template (even when the
//!    `ShareId` names a share of another template, because the template is resolved first),
//!    and the SHARE where the template resolves and holds no share under the id. A `ShareId`
//!    outside the modelled pattern is refused at the boundary with a 400.
//! 4. The delete forgets the `ClientRequestToken` the revoked share was created under, so
//!    that token grants a FRESH share rather than replaying one that is gone.
//! 5. A retry of the delete is keyed to its own `ClientRequestToken`: the same token with
//!    the same parameters replays the empty success without touching the store, the same
//!    token with different parameters is a 409 naming the share it was spent on, and a fresh
//!    token naming a share that is gone is the 404 of rule 3. Rows differ along which
//!    parameter the reused token changes.
//! 6. Revoking a share withdraws the invitation it raised: `ListShareInvitations` stops
//!    reporting it.
//! 7. A share does not outlive the review template it shares: deleting the template takes
//!    its shares with it, and a delete of one of them afterwards reports the TEMPLATE as
//!    missing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the review-template builder and the lookup-failure assertion
//! are the shared `super::workload_harness`'s; the shares this suite revokes are granted and
//! read back through `super::template_shares`'s own `CreateTemplateShare` /
//! `ListTemplateShares` helpers — one spelling of each request for both suites.
//!
//! What a revocation left behind is read over HTTP through `ListTemplateShares`. The store is
//! read only for rule 7, where the template the list would be scoped to is itself gone, and
//! for the share a RULE-6 invitation stands for, which `record_template_share` seeds for the
//! reason its doc gives: an invitation is offered to the principal a template was shared
//! WITH, and a share whose recipient is the caller has no HTTP path in a single-account
//! emulator.

use serde_json::Value;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::share_invitations::{invitations, member as summary_member};
use super::template_shares::{
    create_share, created_share_id, list_shares, record_template_share, stored_share_count,
    summaries, NEW_SHARE_STATUS, OTHER_PRINCIPALS,
};
use super::workload_harness::{
    assert_not_found, encode_path_segment, member, review_template, Answer, Client,
};

/// A `ShareId` that is well-formed (`^[0-9a-f]{32}$`) and names no share.
const ABSENT_SHARE_ID: &str = "0123456789abcdef0123456789abcdef";

/// The lens every template in this suite is created under: the AWS-official lens alias the
/// rest of this crate's review-template suites seed with.
const SEED_LENS: &str = "wellarchitected";

/// Principals a revoked share may have been granted to: the three kinds `SharedWith`
/// publishes (`super::template_shares::OTHER_PRINCIPALS`) plus one value at each of its
/// published `@length(12, 2048)` bounds, because the grant is of one review template to one
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

/// `DELETE /templates/shares/{TemplateArn}/{ShareId}?ClientRequestToken=…`, the wire form of
/// `DeleteTemplateShare`: both labels are non-greedy, so each travels percent-encoded in one
/// path segment, and the `@idempotencyToken` is bound to the query string.
async fn delete_share(client: &Client, template_arn: &str, share_id: &str, token: &str) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/templates/shares/{}/{}?ClientRequestToken={}",
                encode_path_segment(template_arn),
                encode_path_segment(share_id),
                encode_path_segment(token)
            ),
            None,
        )
        .await
}

/// The `TemplateShareSummary` of every share `ListTemplateShares` reports for
/// `template_arn`, in the order it reports them.
async fn listed_shares(client: &Client, template_arn: &str, case: &str) -> Vec<Value> {
    summaries(&list_shares(client, template_arn, "").await, case)
}

/// The `ShareId`s `ListTemplateShares` reports for `template_arn`, in the order it reports
/// them.
async fn listed_share_ids(client: &Client, template_arn: &str, case: &str) -> Vec<String> {
    listed_shares(client, template_arn, case)
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

/// The refusal a `DeleteTemplateShare` naming a share the template does not hold gets.
fn assert_share_missing(case: &str, answer: &Answer, template_arn: &str, share_id: &str) {
    assert_not_found(
        case,
        answer,
        state::TEMPLATE_SHARE_RESOURCE_TYPE,
        share_id,
        &format!("share with ID {share_id} of a review template with ARN {template_arn}"),
    );
}

/// The refusal a `DeleteTemplateShare` naming a template the store does not hold gets.
fn assert_template_missing(case: &str, answer: &Answer, template_arn: &str) {
    assert_not_found(
        case,
        answer,
        state::REVIEW_TEMPLATE_RESOURCE_TYPE,
        template_arn,
        &format!("review template with ARN {template_arn}"),
    );
}

/// Rule 1.
#[tokio::test]
async fn a_granted_share_is_revoked_by_the_delete() {
    let client = Client::new();
    let mut minted: Vec<String> = Vec::new();

    for (case, shared_with) in revoked_principals() {
        let case = case.as_str();
        let template_arn =
            review_template(&client, &format!("revoked-{case}"), &[SEED_LENS]).await;

        let granted =
            create_share(&client, &template_arn, &shared_with, &format!("grant-{case}")).await;
        let share_id = created_share_id(&granted, case);
        assert!(
            !minted.contains(&share_id),
            "case {case:?}: every share must get its own ShareId, but {share_id:?} repeated"
        );
        minted.push(share_id.clone());
        assert_eq!(
            listed_share_ids(&client, &template_arn, case).await,
            vec![share_id.clone()],
            "case {case:?}: the grant must be the template's one share before it is revoked"
        );

        // While the share stands, a second grant to the same principal is refused: that is
        // what the delete below makes possible again.
        let blocked = create_share(
            &client,
            &template_arn,
            &shared_with,
            &format!("regrant-blocked-{case}"),
        )
        .await;
        assert_eq!(
            (blocked.status, blocked.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a principal holds at most one share of one template, got {blocked:?}"
        );
        assert_eq!(
            (
                member(&blocked, "ResourceId"),
                member(&blocked, "ResourceType")
            ),
            (share_id.as_str(), state::TEMPLATE_SHARE_RESOURCE_TYPE),
            "case {case:?}: the conflict must name the share that is in the way, got {blocked:?}"
        );

        // The delete revokes the grant.
        let revoked =
            delete_share(&client, &template_arn, &share_id, &format!("revoke-{case}")).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteTemplateShare 200, got {revoked:?}"
        );
        assert_eq!(
            revoked.body,
            Value::Null,
            "case {case:?}: DeleteTemplateShare answers no members, got {revoked:?}"
        );
        assert!(
            listed_shares(&client, &template_arn, case).await.is_empty(),
            "case {case:?}: the revoked share must stop being reported, got {:?}",
            listed_shares(&client, &template_arn, case).await
        );

        // The share stops resolving: the same delete under a FRESH token is a 404.
        let again = delete_share(
            &client,
            &template_arn,
            &share_id,
            &format!("revoke-again-{case}"),
        )
        .await;
        assert_share_missing(case, &again, &template_arn, &share_id);

        // And the principal can be granted a fresh share, which the 409 above proves was
        // impossible while the revoked one stood.
        let regranted = create_share(
            &client,
            &template_arn,
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
            listed_share_ids(&client, &template_arn, case).await,
            vec![fresh_id.clone()],
            "case {case:?}: the re-granted share must be the template's one share"
        );
        minted.push(fresh_id);

        // The template itself is untouched by either the share or the revocation.
        let template = client.get_review_template(&template_arn).await;
        assert_eq!(
            template.status, 200,
            "case {case:?}: the review template must still resolve after its share was \
             revoked, got {template:?}"
        );
    }
}

/// Rule 2. The rows differ along WHICH of a template's shares is revoked, including its
/// first and its last in the order they are reported, because a share is resolved by its own
/// id and not by its position.
#[tokio::test]
async fn a_delete_revokes_one_share_and_leaves_the_others_standing() {
    for revoked_index in 0..OTHER_PRINCIPALS.len() {
        let case = format!("revoke-share-{revoked_index}");
        let case = case.as_str();
        let client = Client::new();
        let template_arn =
            review_template(&client, "template-with-several-shares", &[SEED_LENS]).await;
        // A second template, shared with the same principals, so the delete is also shown
        // not to reach a share of ANOTHER template.
        let other_arn =
            review_template(&client, "another-template-with-shares", &[SEED_LENS]).await;

        let mut granted: Vec<(String, &str)> = Vec::new();
        for (principal_case, principal) in OTHER_PRINCIPALS {
            let answer = create_share(
                &client,
                &template_arn,
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

        let before = listed_shares(&client, &template_arn, case).await;
        let other_before = listed_shares(&client, &other_arn, case).await;
        let (revoked_id, revoked_principal) = granted[revoked_index].clone();

        let answer =
            delete_share(&client, &template_arn, &revoked_id, &format!("tok-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteTemplateShare 200, got {answer:?}"
        );

        let expected: Vec<Value> = before
            .iter()
            .filter(|summary| {
                summary.get("ShareId").and_then(Value::as_str) != Some(revoked_id.as_str())
            })
            .cloned()
            .collect();
        assert_eq!(
            listed_shares(&client, &template_arn, case).await,
            expected,
            "case {case:?}: only the revoked share may be removed, and the others must be \
             reported unchanged"
        );
        assert_eq!(
            listed_shares(&client, &other_arn, case).await,
            other_before,
            "case {case:?}: a delete must not reach another template's shares"
        );

        // The revoked principal can be granted afresh; the others still collide.
        let regranted = create_share(
            &client,
            &template_arn,
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
                &template_arn,
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

/// Rule 3. The rows differ along WHAT does not resolve — the template or the share — and
/// along the form of the identifier that names it, because the 404 has to name the resource
/// that was actually missing.
#[tokio::test]
async fn a_delete_that_resolves_nothing_removes_nothing() {
    let client = Client::new();
    let template_arn = review_template(&client, "template-under-refusals", &[SEED_LENS]).await;
    let other_arn = review_template(&client, "template-holding-the-share", &[SEED_LENS]).await;
    let unshared_arn = review_template(&client, "template-holding-no-share", &[SEED_LENS]).await;

    let kept = create_share(&client, &template_arn, "111122223333", "grant-kept").await;
    let kept_id = created_share_id(&kept, "kept");
    let elsewhere = create_share(&client, &other_arn, "444455556666", "grant-elsewhere").await;
    let elsewhere_id = created_share_id(&elsewhere, "elsewhere");

    // ARNs of the modelled form (`^arn:aws…:review-template/[a-f0-9]{32}$`, 50..250
    // characters) that name no stored review template: the rows differ along the resource
    // part, the account and the Region the ARN names.
    let absent_templates: Vec<(&str, String)> = vec![
        (
            "an-arn-naming-no-stored-template",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:review-template/{}",
                engine::emulator_account_id(),
                "f".repeat(32)
            ),
        ),
        (
            "an-arn-of-another-account",
            format!(
                "arn:aws:wellarchitected:us-west-2:111122223333:review-template/{}",
                "a".repeat(32)
            ),
        ),
        (
            "an-arn-of-another-region",
            format!(
                "arn:aws:wellarchitected:eu-central-1:{}:review-template/{}",
                engine::emulator_account_id(),
                "0".repeat(32)
            ),
        ),
    ];
    for (case, arn) in &absent_templates {
        // Even the id of a share that EXISTS elsewhere is a missing template here: the
        // template is resolved first, and one that does not resolve holds no shares.
        for share_id in [ABSENT_SHARE_ID, kept_id.as_str()] {
            let answer =
                delete_share(&client, arn, share_id, &format!("tok-{case}-{share_id}")).await;
            assert_template_missing(case, &answer, arn);
        }
    }

    // A template that resolves and holds no such share: the SHARE is what is missing.
    let missing_shares: Vec<(&str, &String, String)> = vec![
        (
            "a-share-id-no-share-carries",
            &template_arn,
            ABSENT_SHARE_ID.to_string(),
        ),
        (
            "a-share-id-of-another-template",
            &template_arn,
            elsewhere_id.clone(),
        ),
        (
            "a-template-that-holds-no-shares",
            &unshared_arn,
            ABSENT_SHARE_ID.to_string(),
        ),
        (
            "a-share-of-another-template-on-an-unshared-template",
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
        let answer = delete_share(&client, &template_arn, &share_id, &format!("tok-{case}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a ShareId outside the modelled pattern must be refused, got {answer:?}"
        );
    }

    // Nothing above removed anything.
    assert_eq!(
        listed_share_ids(&client, &template_arn, "refused").await,
        vec![kept_id.clone()],
        "a refused delete must leave the template's shares exactly as they were"
    );
    assert_eq!(
        listed_share_ids(&client, &other_arn, "refused").await,
        vec![elsewhere_id],
        "a refused delete must leave another template's shares alone"
    );
    assert!(
        listed_shares(&client, &unshared_arn, "refused")
            .await
            .is_empty(),
        "a refused delete must record nothing against a template that holds no shares"
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
        ("another-share-of-the-same-template", false),
        ("the-same-share-id-under-another-template", false),
    ];

    for (case, same_request) in cases {
        let client = Client::new();
        let template_arn = review_template(&client, "retried-share-template", &[SEED_LENS]).await;
        let other_arn =
            review_template(&client, "retried-share-other-template", &[SEED_LENS]).await;

        let grant_token = format!("grant-{case}");
        let granted = create_share(&client, &template_arn, "111122223333", &grant_token).await;
        let share_id = created_share_id(&granted, case);
        let kept =
            create_share(&client, &template_arn, "444455556666", &format!("kept-{case}")).await;
        let kept_id = created_share_id(&kept, case);

        let delete_token = format!("revoke-{case}");
        let revoked = delete_share(&client, &template_arn, &share_id, &delete_token).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteTemplateShare 200, got {revoked:?}"
        );
        let after = listed_shares(&client, &template_arn, case).await;

        let retry = if same_request {
            delete_share(&client, &template_arn, &share_id, &delete_token).await
        } else if case == "another-share-of-the-same-template" {
            delete_share(&client, &template_arn, &kept_id, &delete_token).await
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
                (share_id.as_str(), state::TEMPLATE_SHARE_RESOURCE_TYPE),
                "case {case:?}: the conflict must name the share the token was spent on, got \
                 {retry:?}"
            );
        }
        assert_eq!(
            listed_shares(&client, &template_arn, case).await,
            after,
            "case {case:?}: a retry under a spent token must not touch the store"
        );

        // A FRESH token naming the revoked share is the 404 of rule 3, not a replay.
        let fresh =
            delete_share(&client, &template_arn, &share_id, &format!("fresh-{case}")).await;
        assert_share_missing(case, &fresh, &template_arn, &share_id);

        // Rule 4: the token the revoked share was GRANTED under is free again, so it grants
        // a fresh share rather than replaying the one that is gone.
        let regranted = create_share(&client, &template_arn, "111122223333", &grant_token).await;
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
    let template_name = "invited-template";
    let template_arn = review_template(&client, template_name, &[SEED_LENS]).await;
    let (share_id, invitation_id) =
        record_template_share(&client, &template_arn, caller, NEW_SHARE_STATUS);

    let offered = invitations(&client.list_share_invitations("").await, "offered");
    let invitation = offered
        .iter()
        .find(|summary| summary_member(summary, "ShareInvitationId") == invitation_id)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case \"offered\": the template share's invitation must be reported, got \
                 {offered:?}"
            )
        });
    assert_eq!(
        (
            summary_member(&invitation, "ShareResourceType"),
            summary_member(&invitation, "TemplateArn"),
            summary_member(&invitation, "TemplateName"),
        ),
        (
            state::TEMPLATE_SHARE_RESOURCE_TYPE,
            template_arn.as_str(),
            template_name
        ),
        "case \"offered\": a template share's invitation must name the template it shares, got \
         {invitation:?}"
    );

    let revoked = delete_share(&client, &template_arn, &share_id, "revoke-invited").await;
    assert_eq!(
        revoked.status, 200,
        "case \"revoked\": expected DeleteTemplateShare 200, got {revoked:?}"
    );
    let remaining = invitations(&client.list_share_invitations("").await, "revoked");
    assert!(
        remaining
            .iter()
            .all(|summary| summary_member(summary, "ShareInvitationId") != invitation_id),
        "case \"revoked\": the withdrawn invitation must stop being reported, got {remaining:?}"
    );
}

/// Rule 7. The rows differ along how many shares the deleted template held. The store is
/// read here because the template the list would be scoped to is itself gone.
#[tokio::test]
async fn a_share_does_not_outlive_the_template_it_shares() {
    for (case, principals) in [
        ("one-share", vec!["111122223333"]),
        ("several-shares", vec!["111122223333", "o-abcd1234efgh"]),
    ] {
        let client = Client::new();
        let template_arn =
            review_template(&client, &format!("deleted-template-{case}"), &[SEED_LENS]).await;
        let mut share_ids: Vec<String> = Vec::new();
        for principal in &principals {
            let answer = create_share(
                &client,
                &template_arn,
                principal,
                &format!("grant-{case}-{principal}"),
            )
            .await;
            share_ids.push(created_share_id(&answer, case));
        }
        assert_eq!(
            stored_share_count(&client, &template_arn),
            principals.len(),
            "case {case:?}: every grant must have been recorded before the template is deleted"
        );

        let deleted = client
            .delete_review_template(&template_arn, &format!("delete-{case}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteReviewTemplate 200, got {deleted:?}"
        );
        assert_eq!(
            stored_share_count(&client, &template_arn),
            0,
            "case {case:?}: a deleted template's shares must go with it"
        );

        // The template is what is missing now, for every share it used to hold.
        for share_id in &share_ids {
            let answer = delete_share(
                &client,
                &template_arn,
                share_id,
                &format!("revoke-{case}-{share_id}"),
            )
            .await;
            assert_template_missing(case, &answer, &template_arn);
        }
    }
}
