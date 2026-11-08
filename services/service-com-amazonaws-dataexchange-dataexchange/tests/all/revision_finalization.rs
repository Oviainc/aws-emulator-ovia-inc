// SPDX-License-Identifier: CC0-1.0

//! `UpdateRevision`: what finalizing a revision means, and what it refuses.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `harness`, `call`, `create_data_set`, `seconds`) is the
//! one `data_sets.rs` already uses.
//!
//! Two rules are under test.
//!
//! 1. `UpdateRevision` applies the `Comment` and `Finalized` the request carries to
//!    the stored revision and answers the record as it now stands; an absent
//!    `Comment` keeps the stored one, and `UpdatedAt` moves only when the record
//!    really changed.
//! 2. Finalizing publishes the revision's assets, so a request that sets
//!    `Finalized=true` on a revision holding NO assets is refused with the modelled
//!    `ValidationException` (400) and changes nothing; one on a revision holding at
//!    least one asset succeeds, whatever else the revision carries. An id naming no
//!    data set, or no revision of it, is the modelled `ResourceNotFoundException`
//!    (404).
//!
//! The rows span what the rules generalise over: how many assets the revision holds
//! (zero, one, several), whether the request carries a comment and whether that
//! comment is a change, and which named resource is absent.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, emulator, harness, seconds, Answer, REGION};
use crate::revisions::create_revision;

/// Well-formed per the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` (32 characters)
/// but never created.
const NEVER_CREATED: &str = "dddddddddddddddddddddddddddddddd";

/// The refusal AWS answers a finalize of an empty revision with, recorded in
/// `dataexchange.finalize-revision-without-assets-rejected` and stated by
/// `crate::state::revision_has_no_assets_to_finalize`.
const NO_ASSETS: &str = "Cannot set Finalized to true for a Revision which has 0 Assets.";

/// Send `UpdateRevision` for one revision.
async fn update_revision(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        Some(body),
    )
    .await
}

/// Read one revision back through `GetRevision`, the single-revision reader.
async fn get_revision(router: &MultiServiceRouter, data_set_id: &str, revision_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        None,
    )
    .await
}

/// The refusal message an answer carries, or the empty string when it carries none.
fn message(answer: &Answer) -> String {
    answer
        .member("Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Put `count` assets into a revision — the state finalizing a revision requires.
///
/// Assets arrive in a revision through an import job (`StartJob`, still a scaffold),
/// so the test writes that state through the shared engine, the way
/// `data_set_notifications.rs` writes a data set's Marketplace product. It is also
/// what proves the handler's answer comes from the stored record rather than from the
/// request.
pub(crate) fn import_assets(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
    count: usize,
) {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    let data_set = store
        .data_sets
        .get_mut(data_set_id)
        .expect("the data set the test just created");
    let asset_type = data_set.asset_type.clone();
    let imported_at = dataexchange::state::now();
    let revision = data_set
        .revisions
        .get_mut(revision_id)
        .expect("the revision the test just created");
    for index in 0..count {
        let asset_id = dataexchange::state::mint_id();
        revision.assets.push(dataexchange::state::Asset {
            arn: dataexchange::state::asset_arn(REGION, data_set_id, revision_id, &asset_id),
            id: asset_id,
            name: format!("feed-{index}.csv"),
            asset_type: asset_type.clone(),
            // These assets are written straight into the store, so no import staged
            // content for them, none fulfils an API and none is a data access.
            content: Vec::new(),
            api: None,
            s3_data_access: None,
            created_at: imported_at,
            updated_at: imported_at,
        });
    }
}

/// `Finalized` and `Comment` as state holds them, read straight off the store.
fn stored_revision(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
) -> Option<(bool, Option<String>)> {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    store
        .data_sets
        .get(data_set_id)?
        .revisions
        .get(revision_id)
        .map(|revision| (revision.finalized, revision.comment.clone()))
}

/// RULE: a request that sets `Finalized=true` on a revision holding no assets is
/// refused with a 400 `ValidationException` naming the asset count, and leaves the
/// revision exactly as it was.
#[tokio::test]
async fn finalizing_a_revision_with_no_assets_is_refused() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "empty-finalize", "e", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Rows differ along what else the request and the revision carry: whether the
    // revision was created with a comment, and whether the refused call also tried to
    // set one. None of that makes an empty revision finalizable.
    let rows: Vec<(&str, Option<&str>, Value)> = vec![
        ("no comment stored, none sent", None, json!({ "Finalized": true })),
        (
            "no comment stored, one sent",
            None,
            json!({ "Finalized": true, "Comment": "ready to publish" }),
        ),
        (
            "comment stored, none sent",
            Some("first cut"),
            json!({ "Finalized": true }),
        ),
        (
            "comment stored, another sent",
            Some("first cut"),
            json!({ "Finalized": true, "Comment": "second cut" }),
        ),
        (
            "empty comment sent",
            Some("first cut"),
            json!({ "Finalized": true, "Comment": "" }),
        ),
    ];

    for (case, stored_comment, body) in rows {
        let created = create_revision(&router, &data_set_id, stored_comment, None, case).await;
        let revision_id = created.string("Id");

        let refused = update_revision(&router, &data_set_id, &revision_id, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400 from UpdateRevision(Finalized=true) on an empty \
             revision, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            refused.error
        );
        assert_eq!(
            message(&refused),
            NO_ASSETS,
            "case {case}: expected the recorded refusal wording, got {:?}",
            message(&refused)
        );

        // Nothing moved: not the flag, not the comment, not UpdatedAt.
        assert_eq!(
            stored_revision(&engine, &data_set_id, &revision_id),
            Some((false, stored_comment.map(str::to_string))),
            "case {case}: expected the refused finalize to change nothing in state"
        );
        let fetched = get_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            fetched.member("Finalized"),
            &json!(false),
            "case {case}: expected Finalized to stay false, got {:?}",
            fetched.member("Finalized")
        );
        assert_eq!(
            seconds(fetched.member("UpdatedAt"), case),
            seconds(created.member("UpdatedAt"), case),
            "case {case}: expected UpdatedAt to stay where the create left it"
        );
    }
}

/// RULE: a revision holding at least one asset is finalized — 200, `Finalized=true`
/// in the answer and for every later reader — whatever else it carries and however
/// many assets it holds.
#[tokio::test]
async fn finalizing_a_revision_that_holds_assets_succeeds() {
    let (router, engine) = harness();

    // Rows differ along how many assets the revision holds (one, two, many), the
    // `AssetType` of the data set they belong to, and whether the finalizing request
    // also carries a comment.
    let rows: Vec<(&str, &str, usize, Option<&str>)> = vec![
        ("one asset, no comment", "S3_SNAPSHOT", 1, None),
        ("one asset, a comment", "S3_SNAPSHOT", 1, Some("published")),
        ("two assets", "API_GATEWAY_API", 2, None),
        ("several assets", "REDSHIFT_DATA_SHARE", 7, Some("monthly feed")),
    ];

    for (case, asset_type, assets, comment) in rows {
        let data_set_id = create_data_set(&router, case, "f", asset_type, None)
            .await
            .string("Id");
        let created = create_revision(&router, &data_set_id, None, None, case).await;
        let revision_id = created.string("Id");
        import_assets(&engine, &data_set_id, &revision_id, assets);

        let mut body = json!({ "Finalized": true });
        if let Some(comment) = comment {
            body["Comment"] = json!(comment);
        }
        let updated = update_revision(&router, &data_set_id, &revision_id, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateRevision, got {} body {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.member("Finalized"),
            &json!(true),
            "case {case}: expected Finalized=true, got {:?}",
            updated.member("Finalized")
        );
        assert_eq!(
            updated.string("Id"),
            revision_id,
            "case {case}: expected the answer to report the updated revision, got {:?}",
            updated.member("Id")
        );
        let expected_comment = comment.map_or(Value::Null, |text| json!(text));
        assert_eq!(
            updated.member("Comment"),
            &expected_comment,
            "case {case}: expected the comment the request carried, got {:?}",
            updated.member("Comment")
        );
        assert!(
            seconds(updated.member("UpdatedAt"), case)
                >= seconds(created.member("UpdatedAt"), case),
            "case {case}: expected UpdatedAt to move forward, got {} for {}",
            seconds(updated.member("UpdatedAt"), case),
            seconds(created.member("UpdatedAt"), case)
        );

        // Round trip: the single-revision reader reports what the update persisted.
        let fetched = get_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetRevision, got {} body {:?}",
            fetched.status, fetched.body
        );
        for member in ["Arn", "Comment", "CreatedAt", "Finalized", "Id", "UpdatedAt"] {
            assert_eq!(
                fetched.member(member),
                updated.member(member),
                "case {case}: expected GetRevision's {member} to equal what the update \
                 answered, got {:?} for {:?}",
                fetched.member(member),
                updated.member(member)
            );
        }

        // And a finalized revision stays finalized across a repeat of the same call.
        let again = update_revision(&router, &data_set_id, &revision_id, json!({ "Finalized": true }))
            .await;
        assert_eq!(
            again.member("Finalized"),
            &json!(true),
            "case {case}: expected a repeated finalize to answer Finalized=true, got {:?}",
            again.member("Finalized")
        );
    }
}

/// RULE: `UpdateRevision` applies the members the request carries — an absent
/// `Comment` keeps the stored one, `Finalized=false` leaves the revision open — and
/// `UpdatedAt` moves only when the record really changed.
#[tokio::test]
async fn an_update_applies_what_it_carries_and_nothing_else() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "comment-updates", "c", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Rows differ along the shape of the request body: a comment where there was
    // none, a different comment, the SAME comment (no change), the empty comment the
    // model's `@length(min: 0)` admits, a non-ASCII one, and a body carrying no
    // comment at all.
    let rows: Vec<(&str, Value, Option<&str>, bool)> = vec![
        (
            "a comment where there was none",
            json!({ "Finalized": false, "Comment": "first" }),
            Some("first"),
            true,
        ),
        (
            "a different comment",
            json!({ "Finalized": false, "Comment": "second" }),
            Some("second"),
            true,
        ),
        (
            "the same comment again",
            json!({ "Finalized": false, "Comment": "second" }),
            Some("second"),
            false,
        ),
        (
            "no comment member at all",
            json!({ "Finalized": false }),
            Some("second"),
            false,
        ),
        (
            "the empty comment",
            json!({ "Finalized": false, "Comment": "" }),
            Some(""),
            true,
        ),
        (
            "a non-ascii comment",
            json!({ "Finalized": false, "Comment": "révision — 改訂 ✅" }),
            Some("révision — 改訂 ✅"),
            true,
        ),
    ];

    let revision_id = create_revision(&router, &data_set_id, None, None, "setup")
        .await
        .string("Id");

    for (case, body, expected_comment, expect_moved) in rows {
        let before = get_revision(&router, &data_set_id, &revision_id).await;
        let before_updated_at = seconds(before.member("UpdatedAt"), case);

        let updated = update_revision(&router, &data_set_id, &revision_id, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200, got {} body {:?}",
            updated.status, updated.body
        );
        let expected = expected_comment.map_or(Value::Null, |text| json!(text));
        assert_eq!(
            updated.member("Comment"),
            &expected,
            "case {case}: expected Comment {expected:?}, got {:?}",
            updated.member("Comment")
        );
        assert_eq!(
            updated.member("Finalized"),
            &json!(false),
            "case {case}: expected the revision to stay open, got {:?}",
            updated.member("Finalized")
        );
        assert_eq!(
            stored_revision(&engine, &data_set_id, &revision_id),
            Some((false, expected_comment.map(str::to_string))),
            "case {case}: expected state to hold what the update applied"
        );

        let after_updated_at = seconds(updated.member("UpdatedAt"), case);
        if expect_moved {
            assert!(
                after_updated_at >= before_updated_at,
                "case {case}: expected UpdatedAt to move forward, got {after_updated_at} \
                 for {before_updated_at}"
            );
        } else {
            assert_eq!(
                after_updated_at, before_updated_at,
                "case {case}: expected UpdatedAt to stay put when nothing changed, got \
                 {after_updated_at} for {before_updated_at}"
            );
        }
    }
}

/// RULE: an id naming no data set, or no revision of the named data set, is a 404
/// `ResourceNotFoundException` whose `ResourceType` names the absent resource —
/// including a revision whose data set has since been deleted.
#[tokio::test]
async fn updating_an_absent_revision_is_a_404() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "absent-updates", "a", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let revision_id = create_revision(&router, &data_set_id, None, None, "setup")
        .await
        .string("Id");

    // Rows differ along WHICH named resource is absent, and along the `Finalized` the
    // request asks for: the answer is about the resource either way, so the zero-asset
    // guard never gets to speak for a revision that does not exist.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "unknown data set",
            NEVER_CREATED.to_string(),
            revision_id.clone(),
            "DATA_SET",
        ),
        (
            "unknown revision of a known data set",
            data_set_id.clone(),
            NEVER_CREATED.to_string(),
            "REVISION",
        ),
        (
            "a data set id used as a revision id",
            data_set_id.clone(),
            data_set_id.clone(),
            "REVISION",
        ),
    ];

    for (case, data_set, revision, resource_type) in &rows {
        for finalized in [true, false] {
            let label = format!("{case}, Finalized={finalized}");
            let answer = update_revision(
                &router,
                data_set,
                revision,
                json!({ "Finalized": finalized }),
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {label}: expected 404, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label}: expected ResourceNotFoundException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.member("ResourceType"),
                &json!(resource_type),
                "case {label}: expected ResourceType {resource_type}, got {:?}",
                answer.member("ResourceType")
            );
        }
    }

    // A revision whose data set was deleted is gone with it: the data set is now the
    // absent resource.
    let deleted = call(&router, "DELETE", &format!("/v1/data-sets/{data_set_id}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case delete setup: expected 204 from DeleteDataSet, got {} body {:?}",
        deleted.status, deleted.body
    );
    let after_delete =
        update_revision(&router, &data_set_id, &revision_id, json!({ "Finalized": true })).await;
    assert_eq!(
        after_delete.status, 404,
        "case update after the data set was deleted: expected 404, got {} body {:?}",
        after_delete.status, after_delete.body
    );
    assert_eq!(
        after_delete.member("ResourceType"),
        &json!("DATA_SET"),
        "case update after the data set was deleted: expected the data set to be the \
         absent resource, got {:?}",
        after_delete.member("ResourceType")
    );
}

/// RULE: a malformed id — one outside the modelled `^[a-zA-Z0-9]{30,40}$` — is a 400
/// `ValidationException` in either label position, not a 404 and not a refusal about
/// the body.
#[tokio::test]
async fn a_malformed_id_is_a_400() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "malformed-updates", "m", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Rows differ along HOW the id breaks the pattern: one short of the published
    // minimum, one past the published maximum, and a character outside the set.
    let ids = [
        ("one short of the minimum length", "a".repeat(29)),
        ("one past the maximum length", "a".repeat(41)),
        (
            "a hyphen is not alphanumeric",
            format!("{}-{}", "a".repeat(16), "b".repeat(15)),
        ),
    ];

    for (how, id) in ids {
        for position in ["DataSetId", "RevisionId"] {
            let case = format!("{position} {how}");
            let answer = match position {
                "DataSetId" => {
                    update_revision(&router, &id, NEVER_CREATED, json!({ "Finalized": true })).await
                }
                _ => {
                    update_revision(&router, &data_set_id, &id, json!({ "Finalized": true })).await
                }
            };
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 for {id:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException for {id:?}, got {:?}",
                answer.error
            );
        }
    }
}
