// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule every lens-keyed `@idempotencyToken` operation of
//! this service follows: a retry is compared on the request's parameters AS THE SERVICE
//! RESOLVES THEM, so the lens counts as the lens its `LensAlias` names and not as the
//! spelling it was named by.
//!
//! "If you retry a request with the same client request token and the same parameters after
//! the original request has completed successfully, the result of the original request is
//! returned" (`smithy_query(query="doc", shape="DeleteLensShareInput",
//! member="ClientRequestToken")`), and "For custom lenses, this is the lens ARN… Each lens
//! is identified by its `LensSummary$LensAlias`" (the same query for `LensAlias`). A lens is
//! therefore ONE lens however a request spells it, and a retry that spells it the other way
//! carries the same parameters: it replays the original request's result rather than being
//! answered as a different request under a spent token (409).
//!
//! The rules pinned here, each over a TABLE of spellings rather than one recorded request:
//!
//! 1. `DeleteLensShare`: every pair of spellings — the one the delete landed under and the
//!    one the retry names the same lens by — replays the empty success and removes nothing
//!    else.
//! 2. Taking the lens as the lens it names does not flatten the fingerprint: under every
//!    spelling, the same token over a DIFFERENT share or a DIFFERENT lens is still another
//!    request and the modelled 409.
//! 3. `DeleteLens` follows the same rule for the lens it removes.
//! 4. `CreateLensShare` follows it too: the retry answers the `ShareId` the original grant
//!    minted instead of a second share or a conflict.
//! 5. `CreateLensVersion` follows it too: the retry answers the version the original publish
//!    recorded instead of a second version or a conflict.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import, publish,
//! share, share-revoke and lens-delete wire helpers are the ones `super::custom_lens_import`,
//! `super::custom_lens_versions`, `super::lens_shares`, `super::lens_share_deletion` and
//! `super::custom_lens_deletion` already spell — one spelling of each request for every
//! suite. What a replay left behind is read through the store for the reason
//! `super::lens_shares` gives: a replayed `DeleteLensShare` answers no members at all.

use super::custom_lens_deletion::delete_lens;
use super::custom_lens_import::{
    import, imported_arn, lens_document, member, wait_for_the_import_to_land,
};
use super::custom_lens_versions::create_version;
use super::lens_share_deletion::delete_share;
use super::lens_shares::{create_share, created_share_id, stored_shares};
use super::workload_harness::Client;

use serde_json::Value;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The version every lens in this suite is published under, so a version-pinned spelling of
/// its ARN can be written without reading the store back for it.
const PUBLISHED_VERSION: &str = "1.0";

/// The `LensStatus` of a `DeleteLens` that removes the whole lens.
const DELETE_EVERYTHING: &str = state::LENS_STATUS_TYPE_ALL;

/// The spellings that all name ONE custom lens.
///
/// SOURCE: the `LensAlias` doc — "For custom lenses, this is the lens ARN, such as
/// `arn:aws:wellarchitected:us-west-2:123456789012:lens/0123…`. Each lens is identified by
/// its `LensSummary$LensAlias`" (`smithy_query(query="doc", shape="DeleteLensShareInput",
/// member="LensAlias")`) — plus the two further forms this crate resolves a lens by: that
/// ARN with a version pinned after it (the form `super::lens_share_deletion`'s rule 3 pins
/// as naming the same lens) and the bare lens id.
const LENS_SPELLINGS: [&str; 3] = ["plain-arn", "version-pinned-arn", "bare-lens-id"];

/// The spellings of a custom lens that an operation refusing an Amazon Web Services official
/// lens accepts: the ARN forms only, because "for Amazon Web Services official lenses, this
/// is either the lens alias … or the lens ARN" (`LensAlias` doc) makes a NON-ARN alias an
/// official lens's spelling, which `DeleteLens`, `CreateLensShare` and `CreateLensVersion`
/// refuse with a 400 (rules 3 and 4 assert that refusal). `DeleteLensShare` takes all three
/// spellings of [`LENS_SPELLINGS`], because it resolves the SHARE the lens holds.
const ARN_SPELLINGS: [&str; 2] = ["plain-arn", "version-pinned-arn"];

/// The lens `arn` written the way `spelling` names it.
fn spelled(arn: &str, spelling: &str) -> String {
    match spelling {
        "plain-arn" => arn.to_string(),
        "version-pinned-arn" => format!("{arn}/{PUBLISHED_VERSION}"),
        "bare-lens-id" => state::lens_alias(arn).to_string(),
        other => panic!("unknown lens spelling {other:?}"),
    }
}

/// Every ordered pair of spellings: the one the original request named the lens by, and the
/// one its retry names the same lens by — so neither spelling is only ever used with itself
/// and each row's identical pair still pins the plain case.
fn spelling_pairs(spellings: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    spellings
        .iter()
        .flat_map(|original| {
            spellings
                .iter()
                .map(move |retried| (*original, *retried))
        })
        .collect()
}

/// A principal to share with, one per row: 12 characters as `SharedWith` requires at
/// minimum, and never this emulator's own account, because a share with the account doing
/// the sharing is refused.
fn principal(index: usize) -> String {
    let id = format!("9{:011}", index + 1);
    assert_ne!(
        id,
        engine::emulator_account_id(),
        "a row's principal must not be the account doing the sharing, got {id:?}"
    );
    id
}

/// `count` published custom lenses, each with its own document and its own
/// [`PUBLISHED_VERSION`].
///
/// Every import is issued before the first publish, so a table of any size waits the
/// service's own import span ONCE rather than per row.
async fn published_lenses(client: &Client, label: &str, count: usize) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    for index in 0..count {
        let case = format!("{label}-{index}");
        let document = lens_document(
            &format!("Spelling-{case}-lens"),
            None,
            &[("governance", 1, 2)],
        );
        let answer = import(
            client,
            &document.to_string(),
            None,
            &format!("import-{case}"),
            None,
        )
        .await;
        arns.push(imported_arn(&answer, &case));
    }
    wait_for_the_import_to_land();
    for (index, arn) in arns.iter().enumerate() {
        let case = format!("{label}-{index}");
        let published = create_version(
            client,
            arn,
            PUBLISHED_VERSION,
            Some(true),
            &format!("publish-{case}"),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the lens under test must publish a version first, got {published:?}"
        );
    }
    arns
}

/// The `ShareId`s the store holds for the lens `alias` names, in `ShareId` order.
fn stored_share_ids(client: &Client, alias: &str) -> Vec<String> {
    stored_shares(client, alias)
        .into_iter()
        .map(|share| share.share_id)
        .collect()
}

/// Whether the store still holds a lens under `alias`.
fn lens_is_stored(client: &Client, alias: &str) -> bool {
    let handle = client.store();
    let store = handle.lock();
    store.lens_by_alias(alias).is_some()
}

/// The versions the stored lens under `alias` holds, by name.
fn stored_versions(client: &Client, alias: &str) -> Vec<String> {
    let handle = client.store();
    let store = handle.lock();
    store
        .lens_by_alias(alias)
        .map(|record| record.versions.keys().cloned().collect())
        .unwrap_or_default()
}

/// Rule 1. However the landed `DeleteLensShare` spelled its lens, and however the retry
/// under the SAME token spells the same lens, the retry replays the empty success — and
/// removes nothing else.
#[tokio::test]
async fn a_share_delete_retry_replays_under_every_spelling_of_the_same_lens() {
    let client = Client::new();
    let arn = published_lenses(&client, "share-replay", 1)
        .await
        .pop()
        .expect("published_lenses answers one lens per row");

    // A share nothing in this test deletes: a replay must leave it standing, so a replayed
    // success cannot be a second removal in disguise.
    let bystander = create_share(&client, &arn, "111122223333", "share-bystander").await;
    let bystander_id = created_share_id(&bystander, "bystander");

    for (index, (deleted_as, retried_as)) in spelling_pairs(&LENS_SPELLINGS)
        .into_iter()
        .enumerate()
    {
        let case = format!("deleted-{deleted_as}-retried-{retried_as}");
        let shared_with = principal(index);
        let answer = create_share(&client, &arn, &shared_with, &format!("share-{case}")).await;
        let share_id = created_share_id(&answer, &case);

        let token = format!("revoke-{case}");
        let landed = delete_share(&client, &spelled(&arn, deleted_as), &share_id, &token).await;
        assert_eq!(
            landed.status, 200,
            "case {case:?}: the delete under test must land first, got {landed:?}"
        );

        let retry = delete_share(&client, &spelled(&arn, retried_as), &share_id, &token).await;
        assert_eq!(
            (retry.status, retry.error_code.as_str()),
            (200, ""),
            "case {case:?}: both spellings name the same lens, so the retry must replay the \
             landed delete, got {retry:?}"
        );
        assert_eq!(
            retry.body,
            Value::Null,
            "case {case:?}: the replayed success answers no members, got {retry:?}"
        );
        assert_eq!(
            stored_share_ids(&client, &arn),
            vec![bystander_id.clone()],
            "case {case:?}: the replay must leave the lens's shares where the delete left them"
        );
    }
}

/// One row of the discrimination table: what the retry changes about the landed delete
/// besides the spelling of the lens, and the answer it must get.
struct Changed {
    case: &'static str,
    /// Whether the retry names the lens the revoked share belonged to; `false` names another
    /// lens of the same account.
    same_lens: bool,
    /// Whether the retry names the revoked share; `false` names another share of the same
    /// lens.
    same_share: bool,
    status: u16,
    /// The wire error code, `""` for a success.
    error_code: &'static str,
}

/// Rule 2. Taking the lens as the lens it NAMES does not flatten the fingerprint: under
/// every spelling, the same token over a different share or a different lens is still
/// another request and the modelled 409.
#[tokio::test]
async fn the_share_delete_token_still_discriminates_different_parameters_under_every_spelling() {
    let rows: [Changed; 3] = [
        Changed {
            case: "the-same-request",
            same_lens: true,
            same_share: true,
            status: 200,
            error_code: "",
        },
        Changed {
            case: "the-token-over-another-share",
            same_lens: true,
            same_share: false,
            status: 409,
            error_code: "ConflictException",
        },
        Changed {
            case: "the-token-over-another-lens",
            same_lens: false,
            same_share: true,
            status: 409,
            error_code: "ConflictException",
        },
    ];

    let client = Client::new();
    let lenses = published_lenses(&client, "discriminate", 2).await;
    let (arn, other_lens) = (&lenses[0], &lenses[1]);

    let cases: Vec<(&Changed, &'static str)> = rows
        .iter()
        .flat_map(|row| LENS_SPELLINGS.iter().map(move |spelling| (row, *spelling)))
        .collect();

    for (index, (row, spelling)) in cases.into_iter().enumerate() {
        let case = format!("{}-{spelling}", row.case);
        // Two shares of the lens: the one the delete revokes, and one the retry can name
        // instead of it.
        let answer = create_share(&client, arn, &principal(index), &format!("share-{case}")).await;
        let revoked_id = created_share_id(&answer, &case);
        let answer = create_share(
            &client,
            arn,
            &principal(index + 100),
            &format!("other-{case}"),
        )
        .await;
        let surviving_id = created_share_id(&answer, &case);

        let token = format!("revoke-{case}");
        let landed = delete_share(&client, arn, &revoked_id, &token).await;
        assert_eq!(
            landed.status, 200,
            "case {case:?}: the delete under test must land first, got {landed:?}"
        );

        // The retry spells the lens its own way whichever lens it names, so the only thing a
        // row changes about the landed request is the parameter it says it changes.
        let retry_lens = if row.same_lens {
            spelled(arn, spelling)
        } else {
            spelled(other_lens, spelling)
        };
        let retry_share = if row.same_share {
            &revoked_id
        } else {
            &surviving_id
        };
        let retry = delete_share(&client, &retry_lens, retry_share, &token).await;
        assert_eq!(
            (retry.status, retry.error_code.as_str()),
            (row.status, row.error_code),
            "case {case:?}: expected {} {:?}, got {retry:?}",
            row.status,
            row.error_code
        );
        if row.error_code == "ConflictException" {
            assert_eq!(
                (member(&retry, "ResourceType"), member(&retry, "ResourceId")),
                (state::LENS_SHARE_RESOURCE_TYPE, revoked_id.as_str()),
                "case {case:?}: the 409 must name the share the token was spent on, got {retry:?}"
            );
        }
        // Whatever the retry answered, it changed nothing: the revoked share is still gone
        // and the lens's other share is still there.
        assert_eq!(
            stored_share_ids(&client, arn),
            vec![surviving_id.clone()],
            "case {case:?}: the retry must leave the lens's shares where the delete left them"
        );
        let cleanup = delete_share(&client, arn, &surviving_id, &format!("cleanup-{case}")).await;
        assert_eq!(
            cleanup.status, 200,
            "case {case:?}: expected DeleteLensShare 200, got {cleanup:?}"
        );
    }
}

/// Rule 3. `DeleteLens` keys its retry the same way: however the landed delete spelled the
/// lens it removed, a retry under the same token naming that lens the other way replays the
/// empty success, and the lens stays gone.
#[tokio::test]
async fn a_lens_delete_retry_replays_under_every_spelling_of_the_same_lens() {
    let pairs = spelling_pairs(&ARN_SPELLINGS);

    let client = Client::new();
    // One lens per row, because the row deletes it, plus one the suite never deletes: a
    // replay must leave every other lens standing.
    let mut arns = published_lenses(&client, "lens-replay", pairs.len() + 1).await;
    let bystander = arns.pop().expect("one lens per row and one bystander");

    // A bare lens id is an official lens's spelling, and `DeleteLens` refuses an official
    // lens at the boundary rather than resolving the custom lens whose id it is — which is
    // why this table spans the ARN spellings only.
    let refused = delete_lens(
        &client,
        &spelled(&bystander, "bare-lens-id"),
        DELETE_EVERYTHING,
        "delete-bare-id",
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "a bare lens id is not a spelling DeleteLens accepts, got {refused:?}"
    );

    for ((deleted_as, retried_as), arn) in pairs.into_iter().zip(&arns) {
        let case = format!("deleted-{deleted_as}-retried-{retried_as}");

        let token = format!("delete-{case}");
        let landed = delete_lens(
            &client,
            &spelled(arn, deleted_as),
            DELETE_EVERYTHING,
            &token,
        )
        .await;
        assert_eq!(
            landed.status, 200,
            "case {case:?}: the delete under test must land first, got {landed:?}"
        );

        let retry = delete_lens(
            &client,
            &spelled(arn, retried_as),
            DELETE_EVERYTHING,
            &token,
        )
        .await;
        assert_eq!(
            (retry.status, retry.error_code.as_str()),
            (200, ""),
            "case {case:?}: both spellings name the same lens, so the retry must replay the \
             landed delete, got {retry:?}"
        );
        assert!(
            !lens_is_stored(&client, arn),
            "case {case:?}: the deleted lens must stay gone, got it stored again"
        );
        assert!(
            lens_is_stored(&client, &bystander),
            "case {case:?}: the replay must leave every other lens standing"
        );
    }
}

/// Rule 4. `CreateLensShare` keys its retry the same way: the retry naming the lens the
/// other way answers the `ShareId` the original grant minted rather than minting a second
/// share or reporting a conflict.
#[tokio::test]
async fn a_share_create_retry_replays_under_every_spelling_of_the_same_lens() {
    let client = Client::new();
    let arn = published_lenses(&client, "share-create", 1)
        .await
        .pop()
        .expect("published_lenses answers one lens per row");

    // A bare lens id is an official lens's spelling, and an official lens "cannot be shared"
    // (`CreateLensShare` doc), so a create is refused at the boundary for it rather than
    // resolving the custom lens whose id it is — which is why this table spans the ARN
    // spellings only.
    let bare_id = spelled(&arn, "bare-lens-id");
    let refused = create_share(&client, &bare_id, "111122223333", "share-bare-id").await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "a bare lens id is not a spelling CreateLensShare accepts, got {refused:?}"
    );

    for (index, (created_as, retried_as)) in spelling_pairs(&ARN_SPELLINGS).into_iter().enumerate()
    {
        let case = format!("created-{created_as}-retried-{retried_as}");
        let shared_with = principal(index);
        let token = format!("share-{case}");
        let created = create_share(
            &client,
            &spelled(&arn, created_as),
            &shared_with,
            &token,
        )
        .await;
        let share_id = created_share_id(&created, &case);

        let retry = create_share(
            &client,
            &spelled(&arn, retried_as),
            &shared_with,
            &token,
        )
        .await;
        assert_eq!(
            (retry.status, retry.error_code.as_str()),
            (200, ""),
            "case {case:?}: both spellings name the same lens, so the retry must replay the \
             original grant, got {retry:?}"
        );
        assert_eq!(
            member(&retry, "ShareId"),
            share_id,
            "case {case:?}: the retry must answer the share the original request minted, got \
             {retry:?}"
        );
        let stored = stored_share_ids(&client, &arn);
        assert!(
            stored.contains(&share_id),
            "case {case:?}: the granted share must still be stored, got {stored:?}"
        );
        assert_eq!(
            stored.len(),
            index + 1,
            "case {case:?}: the replay must mint no second share, got {stored:?}"
        );
    }
}

/// Rule 5. `CreateLensVersion` keys its retry the same way: the retry naming the lens the
/// other way answers the version the original publish recorded rather than publishing a
/// second one or reporting a conflict.
#[tokio::test]
async fn a_version_create_retry_replays_under_every_spelling_of_the_same_lens() {
    // The version each row publishes on top of the one the version-pinned spelling pins.
    let next_version = "2.0";
    let pairs = spelling_pairs(&ARN_SPELLINGS);

    let client = Client::new();
    let arns = published_lenses(&client, "version-create", pairs.len()).await;

    for ((created_as, retried_as), arn) in pairs.into_iter().zip(&arns) {
        let case = format!("created-{created_as}-retried-{retried_as}");

        let token = format!("publish-{case}");
        let published = create_version(
            &client,
            &spelled(arn, created_as),
            next_version,
            Some(false),
            &token,
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the publish under test must land first, got {published:?}"
        );

        let retry = create_version(
            &client,
            &spelled(arn, retried_as),
            next_version,
            Some(false),
            &token,
        )
        .await;
        assert_eq!(
            (retry.status, retry.error_code.as_str()),
            (200, ""),
            "case {case:?}: both spellings name the same lens, so the retry must replay the \
             original publish, got {retry:?}"
        );
        assert_eq!(
            (member(&retry, "LensArn"), member(&retry, "LensVersion")),
            (arn.as_str(), next_version),
            "case {case:?}: the retry must answer the version the original request published, \
             got {retry:?}"
        );
        assert_eq!(
            stored_versions(&client, arn),
            vec![PUBLISHED_VERSION.to_string(), next_version.to_string()],
            "case {case:?}: the replay must publish no second version"
        );
    }
}
