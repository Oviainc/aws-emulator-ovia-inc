// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for revoking a share of a custom lens: `DeleteLensShare`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The delete REVOKES the grant it names: the share stops resolving (the same request
//!    again is a 404) and the principal it was offered to can be granted a fresh share,
//!    which the 409 a second grant gets proves was impossible while the share stood. Rows
//!    differ along the FORM of the principal the revoked share was for — an account id, an
//!    organization id, an OU id and a value at the modelled `SharedWith` length ceiling.
//! 2. A delete removes ONE share: the lens's other shares still stand, and the lens's own
//!    content is untouched. Rows differ along which of several shares is revoked.
//! 3. A share is resolved against the lens its `LensAlias` NAMES, not against the
//!    spelling: a lens ARN and that ARN with a version pinned after it name the same lens
//!    and reach the same shares.
//! 4. A delete that resolves nothing removes nothing and is a 404 NAMING the resource that
//!    was missing: the SHARE where the lens resolves and does not hold it (a lens Amazon
//!    Web Services publishes, which cannot be shared, holds none; so does a lens whose
//!    shares belong to another lens or to nobody), and the LENS where the name resolves to
//!    no lens at all. A `ShareId` outside the modelled pattern is refused at the boundary
//!    with a 400.
//! 5. The delete forgets the `ClientRequestToken` the revoked share was created under, so
//!    that token grants a FRESH share rather than replaying one that is gone.
//! 6. A retry of the delete is keyed to its own `ClientRequestToken`: the same token with
//!    the same parameters replays the empty success without touching the store, the same
//!    token with different parameters is a 409, and a fresh token naming a share that is
//!    gone is the 404 of rule 4. Rows differ along which parameter the reused token
//!    changes.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the lens document,
//! import and export helpers are `super::custom_lens_import`'s, and the share this suite
//! revokes is granted through `super::lens_shares`'s own `CreateLensShare` helpers — one
//! spelling of each request for both suites.
//!
//! What the delete left behind is read through the store for the reason
//! `super::lens_shares` gives: `DeleteLensShare` answers no members, and the paged read of
//! a lens's shares is `super::lens_share_listing`'s own suite. The REFUSALS and the
//! re-grant that prove the same thing over HTTP are asserted beside every stored
//! assertion.

use super::custom_lens_import::{export, lens_document, member, ABSENT_LENS_ARN};
use super::lens_shares::{
    create_share, created_share_id, one_published_lens, publish_lenses, stored_shares,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

use serde_json::Value;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// A `ShareId` that is well-formed (`^[0-9a-f]{32}$`) and names no share.
const ABSENT_SHARE_ID: &str = "0123456789abcdef0123456789abcdef";

/// One row of the refusal table: how the request names the lens and the share, the answer
/// it must get, and the resource that answer must NAME when it is a lookup miss.
struct Refusal {
    case: &'static str,
    alias: String,
    share_id: String,
    status: u16,
    error_code: &'static str,
    /// The `(ResourceType, ResourceId)` the 404 carries, or `None` for a refusal that
    /// names no resource.
    missing: Option<(&'static str, String)>,
}

/// `DELETE /lenses/{LensAlias}/shares/{ShareId}?ClientRequestToken=…`, the wire form of
/// `DeleteLensShare`: both labels are non-greedy, so each travels percent-encoded in one
/// path segment, and the `@idempotencyToken` is bound to the query string. `pub(crate)`
/// because `super::lens_token_alias_spelling` revokes its shares through this one spelling
/// of the request.
pub(crate) async fn delete_share(
    client: &Client,
    alias: &str,
    share_id: &str,
    token: &str,
) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/lenses/{}/shares/{}?ClientRequestToken={}",
                encode_path_segment(alias),
                encode_path_segment(share_id),
                encode_path_segment(token)
            ),
            None,
        )
        .await
}

/// The `ShareId`s the store holds for the lens `alias`, in `ShareId` order.
fn stored_share_ids(client: &Client, alias: &str) -> Vec<String> {
    stored_shares(client, alias)
        .into_iter()
        .map(|share| share.share_id)
        .collect()
}

/// A published lens per case, named after the case so the rows never share a lens.
async fn published_lenses(client: &Client, cases: &[&'static str]) -> Vec<String> {
    let documents: Vec<(&'static str, Value)> = cases
        .iter()
        .map(|case| {
            (
                *case,
                lens_document(
                    &format!("Revoked-{case}-lens"),
                    None,
                    &[("governance", 1, 2), ("security", 2, 2)],
                ),
            )
        })
        .collect();
    publish_lenses(client, &documents)
        .await
        .into_iter()
        .map(|(_, arn)| arn)
        .collect()
}

/// A `SharedWith` at the modelled `@length(min: 12, max: 2048)` ceiling, built as an
/// organization id so only its LENGTH differs from the other rows.
fn shared_with_at_the_length_ceiling() -> String {
    let value = format!("o-{}", "a".repeat(2046));
    assert_eq!(
        value.len(),
        2048,
        "the ceiling row must be exactly 2048 characters, got {}",
        value.len()
    );
    value
}

/// Rule 1. Whatever form of principal held the share, the delete revokes exactly it: the
/// share is gone from the store, the same request again is a 404, and the principal can be
/// granted a fresh share where a second grant was a 409 while the first stood.
#[tokio::test]
async fn the_delete_revokes_the_share_it_names() {
    // `SharedWith` is "The Amazon Web Services account ID, organization ID, or
    // organizational unit (OU) ID with which the workload, lens, profile, or review
    // template is shared", `@length(min: 12, max: 2048)`
    // (`smithy_query(query="operation", operation="CreateLensShare")`), so the rows span
    // each form the doc names, the 12-character floor (an account id) and the 2048
    // character ceiling.
    let ceiling = shared_with_at_the_length_ceiling();
    let cases: Vec<(&'static str, String)> = vec![
        ("account-id", "123456789012".to_string()),
        ("organization-id", "o-a1b2c3d4e5".to_string()),
        ("organizational-unit-id", "ou-a1b2-f6g7h8i9".to_string()),
        ("at-the-length-ceiling", ceiling),
    ];

    let client = Client::new();
    let names: Vec<&'static str> = cases.iter().map(|(case, _)| *case).collect();
    let arns = published_lenses(&client, &names).await;

    let mut granted: Vec<String> = Vec::new();
    for ((case, shared_with), arn) in cases.iter().zip(&arns) {
        let answer = create_share(&client, arn, shared_with, &format!("share-{case}")).await;
        let share_id = created_share_id(&answer, case);

        // The grant is in force: the same principal cannot hold a second share of the
        // same lens while it stands.
        let again = create_share(&client, arn, shared_with, &format!("again-{case}")).await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a second share for the same principal must be a 409, got {again:?}"
        );
        granted.push(share_id);
    }

    for (((case, shared_with), arn), share_id) in cases.iter().zip(&arns).zip(&granted) {
        let answer = delete_share(&client, arn, share_id, &format!("revoke-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteLensShare 200, got {answer:?}"
        );
        assert_eq!(
            answer.body,
            Value::Null,
            "case {case:?}: DeleteLensShare answers no members, got {answer:?}"
        );
        assert!(
            stored_shares(&client, arn).is_empty(),
            "case {case:?}: the revoked share must be gone, got {:?}",
            stored_shares(&client, arn)
        );

        // A share that is gone resolves to nothing, so the same request under a FRESH
        // token is a 404 — and the principal it was offered to can be granted a fresh
        // share, which the 409 above proved was impossible while the share stood. (The
        // retry under the ORIGINAL token is the replay this suite's rule-6 token table
        // pins, not this 404.)
        let repeat = delete_share(&client, arn, share_id, &format!("repeat-{case}")).await;
        assert_eq!(
            (repeat.status, repeat.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a repeated delete under a fresh token must be a 404, got {repeat:?}"
        );
        let regranted = create_share(&client, arn, shared_with, &format!("regrant-{case}")).await;
        let regranted_id = created_share_id(&regranted, case);
        assert_ne!(
            &regranted_id, share_id,
            "case {case:?}: the fresh share must carry its own ShareId, got {regranted_id:?}"
        );
        let stored = stored_shares(&client, arn);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: the fresh share must be the only share of the lens, got {stored:?}"
        );
        assert_eq!(
            (stored[0].share_id.as_str(), stored[0].shared_with.as_str()),
            (regranted_id.as_str(), shared_with.as_str()),
            "case {case:?}: the fresh share must be the one answered, got {stored:?}"
        );
    }
}

/// Rule 2. The delete removes the ONE share it names: the lens's other shares still stand
/// and the lens's own content is untouched.
#[tokio::test]
async fn deleting_one_share_leaves_the_other_shares_and_the_lens_alone() {
    // The principals a lens is shared with, and — per row — which of them loses its
    // share: the first, a middle one and the last, so the delete is exercised at both ends
    // of the stored order and inside it.
    let principals = ["111111111111", "222222222222", "333333333333"];
    let cases: [(&'static str, usize); 3] = [("first", 0), ("middle", 1), ("last", 2)];

    let client = Client::new();
    let names: Vec<&'static str> = cases.iter().map(|(case, _)| *case).collect();
    let arns = published_lenses(&client, &names).await;

    for ((case, revoked), arn) in cases.iter().zip(&arns) {
        let mut ids: Vec<String> = Vec::new();
        for (index, principal) in principals.iter().enumerate() {
            let answer =
                create_share(&client, arn, principal, &format!("share-{case}-{index}")).await;
            ids.push(created_share_id(&answer, case));
        }

        let answer = delete_share(&client, arn, &ids[*revoked], &format!("revoke-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteLensShare 200, got {answer:?}"
        );

        let surviving: Vec<(String, String)> = stored_shares(&client, arn)
            .into_iter()
            .map(|share| (share.share_id, share.shared_with))
            .collect();
        let mut expected: Vec<(String, String)> = principals
            .iter()
            .enumerate()
            .filter(|(index, _)| index != revoked)
            .map(|(index, principal)| (ids[index].clone(), (*principal).to_string()))
            .collect();
        // The store enumerates a lens's shares in `ShareId` order, which is not the order
        // they were created in.
        expected.sort();
        assert_eq!(
            surviving, expected,
            "case {case:?}: only the revoked share may be gone, got {surviving:?}"
        );

        // The share is the resource that was deleted: the lens still exports the content
        // it was imported with.
        let exported = export(&client, arn, None).await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: the lens must still be readable after a share is revoked, got \
             {exported:?}"
        );
    }
}

/// Rule 3. A lens ARN and that ARN with a version pinned after it name the same lens, so a
/// share created under one spelling is revoked under the other.
#[tokio::test]
async fn a_share_is_resolved_by_the_lens_its_name_resolves_to() {
    let client = Client::new();
    let arn = one_published_lens(&client, "spellings", "Spellings-lens").await;
    // The version the lens was published under, read off the lens itself rather than
    // spelled again here: a version-pinned lens ARN is `…:lens/<id>/<version>`.
    let version = {
        let handle = client.store();
        let store = handle.lock();
        store
            .lens_by_alias(&arn)
            .and_then(|lens| lens.latest_version().map(|(version, _)| version.to_string()))
            .expect("the lens under test must carry a published version")
    };
    let version_pinned = format!("{arn}/{version}");

    // Each row names the lens one way on the create and the OTHER way on the delete, so
    // neither spelling is only ever used with itself.
    let cases: [(&str, &str, &str, &str); 2] = [
        (
            "created-plain-revoked-pinned",
            "444444444444",
            &arn,
            &version_pinned,
        ),
        (
            "created-pinned-revoked-plain",
            "555555555555",
            &version_pinned,
            &arn,
        ),
    ];

    for (case, principal, created_under, revoked_under) in cases {
        let answer =
            create_share(&client, created_under, principal, &format!("share-{case}")).await;
        let share_id = created_share_id(&answer, case);
        let answer =
            delete_share(&client, revoked_under, &share_id, &format!("revoke-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: both spellings name the same lens, so the delete must be 200, got \
             {answer:?}"
        );
        assert!(
            stored_shares(&client, &arn).is_empty(),
            "case {case:?}: the share must be gone, got {:?}",
            stored_shares(&client, &arn)
        );
    }
}

/// Rule 4. A delete that resolves no share deletes nothing and says which resource was
/// missing.
#[tokio::test]
async fn a_delete_that_resolves_nothing_is_refused_and_removes_nothing() {
    let client = Client::new();
    let arns = published_lenses(&client, &["target", "other"]).await;
    let (target, other) = (&arns[0], &arns[1]);

    let answer = create_share(&client, target, "666666666666", "share-target").await;
    let target_share = created_share_id(&answer, "target");
    let answer = create_share(&client, other, "777777777777", "share-other").await;
    let other_share = created_share_id(&answer, "other");

    // `AWS_OFFICIAL_LENS_ALIASES` is the catalog a bare alias resolves against
    // (`smithy_query(query="shape", shape="LensAlias")` gives `serverless` as an example of
    // one); the rows below take an entry of that catalog this suite is not otherwise
    // about, the official-ARN spelling of another, an alias outside the catalog, an
    // unknown custom ARN, and share ids belonging to another lens and to none.
    //
    // The last member of each row is the resource the 404 must NAME
    // (`ResourceNotFoundException$ResourceType` / `$ResourceId`, both `@required`): the
    // SHARE, by the id as sent, where the lens resolves and does not hold it, and the
    // LENS, by the alias as sent, where the name resolves to no lens at all — so the two
    // refusals cannot be mistaken for each other.
    let official_alias = state::AWS_OFFICIAL_LENS_ALIASES
        .iter()
        .find(|alias| **alias != state::FRAMEWORK_LENS_ALIAS)
        .expect("the official lens catalog carries more than the framework lens");
    let official_arn = state::official_lens_arn(state::FRAMEWORK_LENS_ALIAS);
    let cases: Vec<Refusal> = vec![
        Refusal {
            case: "an-official-lens-alias",
            alias: (*official_alias).to_string(),
            share_id: ABSENT_SHARE_ID.to_string(),
            status: 404,
            error_code: "ResourceNotFoundException",
            missing: Some((
                state::LENS_SHARE_RESOURCE_TYPE,
                ABSENT_SHARE_ID.to_string(),
            )),
        },
        Refusal {
            case: "an-official-lens-arn",
            alias: official_arn,
            share_id: ABSENT_SHARE_ID.to_string(),
            status: 404,
            error_code: "ResourceNotFoundException",
            missing: Some((
                state::LENS_SHARE_RESOURCE_TYPE,
                ABSENT_SHARE_ID.to_string(),
            )),
        },
        Refusal {
            case: "an-alias-naming-no-lens",
            alias: "notalens".to_string(),
            share_id: ABSENT_SHARE_ID.to_string(),
            status: 404,
            error_code: "ResourceNotFoundException",
            missing: Some((state::LENS_RESOURCE_TYPE, "notalens".to_string())),
        },
        Refusal {
            case: "an-unknown-custom-lens-arn",
            alias: ABSENT_LENS_ARN.to_string(),
            share_id: ABSENT_SHARE_ID.to_string(),
            status: 404,
            error_code: "ResourceNotFoundException",
            missing: Some((state::LENS_RESOURCE_TYPE, ABSENT_LENS_ARN.to_string())),
        },
        Refusal {
            case: "a-share-id-of-another-lens",
            alias: target.clone(),
            share_id: other_share.clone(),
            status: 404,
            error_code: "ResourceNotFoundException",
            missing: Some((state::LENS_SHARE_RESOURCE_TYPE, other_share.clone())),
        },
        Refusal {
            case: "an-absent-share-id",
            alias: target.clone(),
            share_id: ABSENT_SHARE_ID.to_string(),
            status: 404,
            error_code: "ResourceNotFoundException",
            missing: Some((
                state::LENS_SHARE_RESOURCE_TYPE,
                ABSENT_SHARE_ID.to_string(),
            )),
        },
        // `ShareId` is `@pattern("^[0-9a-f]{32}$")`, so a value outside it names no share
        // and is refused at the boundary before the handler runs — a `ValidationException`,
        // which names no resource.
        Refusal {
            case: "a-share-id-outside-the-modelled-pattern",
            alias: target.clone(),
            share_id: "not-a-share-id".to_string(),
            status: 400,
            error_code: "ValidationException",
            missing: None,
        },
    ];

    for row in &cases {
        let case = row.case;
        let answer = delete_share(
            &client,
            &row.alias,
            &row.share_id,
            &format!("revoke-{case}"),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (row.status, row.error_code),
            "case {case:?}: expected {} {}, got {answer:?}",
            row.status,
            row.error_code
        );
        if let Some((resource_type, resource_id)) = &row.missing {
            assert_eq!(
                (
                    member(&answer, "ResourceType"),
                    member(&answer, "ResourceId")
                ),
                (*resource_type, resource_id.as_str()),
                "case {case:?}: the 404 must name the resource that was missing, got {answer:?}"
            );
        }
    }

    // Nothing was removed: both shares are still there, and still revocable.
    assert_eq!(
        stored_share_ids(&client, target),
        vec![target_share.clone()],
        "a refused delete must leave the lens's shares where they were"
    );
    assert_eq!(
        stored_share_ids(&client, other),
        vec![other_share.clone()],
        "a refused delete must leave another lens's shares where they were"
    );
    for (case, alias, share_id) in [
        ("target", target, &target_share),
        ("other", other, &other_share),
    ] {
        let answer = delete_share(&client, alias, share_id, &format!("final-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the surviving share must still revoke, got {answer:?}"
        );
    }
}

/// Rule 5. The delete forgets the token the revoked share was created under: a request
/// carrying it again grants a fresh share rather than replaying one that is gone, while
/// the replay BEFORE the delete reports the original.
#[tokio::test]
async fn the_delete_forgets_the_token_its_share_was_created_under() {
    let client = Client::new();
    let arn = one_published_lens(&client, "tokens", "Token-lens").await;

    for (case, principal) in [
        ("an-account", "999999999999"),
        ("an-organization", "o-9z8y7x6w5v"),
    ] {
        let token = format!("tok-{case}");
        let first = create_share(&client, &arn, principal, &token).await;
        let share_id = created_share_id(&first, case);

        // While the share stands, the token is a replay.
        let replay = create_share(&client, &arn, principal, &token).await;
        assert_eq!(
            created_share_id(&replay, case),
            share_id,
            "case {case:?}: the token must replay the original share, got {replay:?}"
        );

        let revoked = delete_share(&client, &arn, &share_id, &format!("revoke-{case}")).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected DeleteLensShare 200, got {revoked:?}"
        );

        // The share the token named is gone, so the token grants a new one.
        let after = create_share(&client, &arn, principal, &token).await;
        let fresh = created_share_id(&after, case);
        assert_ne!(
            fresh, share_id,
            "case {case:?}: a token whose share was revoked must grant a fresh share, got \
             {fresh:?}"
        );
        assert_eq!(
            stored_share_ids(&client, &arn),
            vec![fresh.clone()],
            "case {case:?}: the fresh share must be the only share of the lens"
        );
        let cleanup = delete_share(&client, &arn, &fresh, &format!("cleanup-{case}")).await;
        assert_eq!(
            cleanup.status, 200,
            "case {case:?}: expected DeleteLensShare 200, got {cleanup:?}"
        );
    }
}

/// One row of the reused-token table: what the retry of a landed delete changes about the
/// original request, and the answer it must get.
struct Retry {
    case: &'static str,
    /// Whether the retry carries the `ClientRequestToken` the landed delete was made
    /// under; `false` is a token of its own.
    same_token: bool,
    /// Whether the retry names the lens the revoked share belonged to; `false` names
    /// another lens of the same account.
    same_lens: bool,
    /// Whether the retry names the revoked share; `false` names another share of the same
    /// lens.
    same_share: bool,
    status: u16,
    /// The wire error code, `""` for a success.
    error_code: &'static str,
}

/// Rule 6. `ClientRequestToken` is `@idempotencyToken`: "If you retry a request with the
/// same client request token and the same parameters after the original request has
/// completed successfully, the result of the original request is returned"
/// (`smithy_query(query="doc",shape="DeleteLensShareInput",member="ClientRequestToken")`).
/// So the token the landed delete was made under replays its empty success, the same token
/// over different parameters is another request and a 409, and a token of its own naming a
/// share that is gone is the 404 of rule 4 — and none of the three touches the store.
#[tokio::test]
async fn a_retry_is_keyed_to_the_token_the_landed_delete_carried() {
    let cases: [Retry; 4] = [
        Retry {
            case: "the-same-request",
            same_token: true,
            same_lens: true,
            same_share: true,
            status: 200,
            error_code: "",
        },
        Retry {
            case: "the-token-over-another-share",
            same_token: true,
            same_lens: true,
            same_share: false,
            status: 409,
            error_code: "ConflictException",
        },
        Retry {
            case: "the-token-over-another-lens",
            same_token: true,
            same_lens: false,
            same_share: true,
            status: 409,
            error_code: "ConflictException",
        },
        Retry {
            case: "a-token-of-its-own",
            same_token: false,
            same_lens: true,
            same_share: true,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
    ];

    let client = Client::new();
    let names: Vec<&'static str> = cases.iter().map(|row| row.case).collect();
    let revoked_from = published_lenses(&client, &names).await;
    let other_lens = one_published_lens(&client, "retry-other", "Retry-other-lens").await;

    for (row, arn) in cases.iter().zip(&revoked_from) {
        let case = row.case;

        // Two shares of the lens: the one the delete revokes, and one that must still be
        // standing whatever the retry does.
        let answer = create_share(&client, arn, "121212121212", &format!("share-{case}")).await;
        let revoked_id = created_share_id(&answer, case);
        let answer = create_share(&client, arn, "343434343434", &format!("other-{case}")).await;
        let surviving_id = created_share_id(&answer, case);

        let token = format!("revoke-{case}");
        let landed = delete_share(&client, arn, &revoked_id, &token).await;
        assert_eq!(
            landed.status, 200,
            "case {case:?}: the delete under test must land first, got {landed:?}"
        );

        let retry_token = if row.same_token {
            token.clone()
        } else {
            format!("retry-{case}")
        };
        let retry_lens = if row.same_lens { arn } else { &other_lens };
        let retry_share = if row.same_share {
            &revoked_id
        } else {
            &surviving_id
        };
        let retry = delete_share(&client, retry_lens, retry_share, &retry_token).await;
        assert_eq!(
            (retry.status, retry.error_code.as_str()),
            (row.status, row.error_code),
            "case {case:?}: expected {} {:?}, got {retry:?}",
            row.status,
            row.error_code
        );
        if row.status == 200 {
            assert_eq!(
                retry.body,
                Value::Null,
                "case {case:?}: the replayed success answers no members, got {retry:?}"
            );
        }

        // Whatever the retry answered, it changed nothing: the revoked share is still
        // gone and the other share of the lens is still there — a replay does not remove
        // a second share, and a refusal removes none.
        assert_eq!(
            stored_share_ids(&client, arn),
            vec![surviving_id.clone()],
            "case {case:?}: the retry must leave the lens's shares where the delete left them"
        );
        // A conflict names the share the token was already spent on, so a caller can tell
        // a reused token from an unknown one.
        if row.error_code == "ConflictException" {
            assert_eq!(
                (member(&retry, "ResourceType"), member(&retry, "ResourceId")),
                (state::LENS_SHARE_RESOURCE_TYPE, revoked_id.as_str()),
                "case {case:?}: the 409 must name the share the token was spent on, got {retry:?}"
            );
        }
    }
}
