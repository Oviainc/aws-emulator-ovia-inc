// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule `CreateLensShare` keys its `@idempotencyToken`
//! retries by: the parameters a retry is compared on are the ones the service RESOLVES, so
//! the lens counts as the lens its `LensAlias` NAMES and not as the spelling it was named
//! by.
//!
//! "If you retry a request with the same client request token and the same parameters after
//! the original request has completed successfully, the result of the original request is
//! returned" (`smithy_query(query="shape", shape="ClientRequestToken")`), and "for custom
//! lenses, this is the lens ARN … Each lens is identified by its
//! `LensSummary$LensAlias`" (`smithy_query(query="shape", shape="LensAlias")`). One lens is
//! therefore one lens however a request spells it, and a retry spelling it the other way
//! carries the same parameters: it replays the original grant rather than being answered as
//! a different request under a spent token.
//!
//! This suite widens `super::lens_token_alias_spelling`'s rule 4 along the dimension that
//! rule's table holds fixed — WHICH version a version-pinned ARN pins — and adds the two
//! refusals that show the identity does not flatten this operation's fingerprint:
//!
//! 1. Replay: over every ordered pair of spellings drawn from the plain lens ARN and that
//!    ARN with each of several published versions pinned after it, the retry under the same
//!    token and the same `SharedWith` answers the `ShareId` the original grant minted and
//!    mints no second share.
//! 2. Taking the lens as the lens it names does not flatten the fingerprint: under EVERY
//!    spelling, the same token over a different `SharedWith`, and over a different lens, is
//!    still another request — the modelled `409 ConflictException` naming the share the
//!    token already minted — and records nothing.
//! 3. The grant a second share collides with is resolved the same way: a share of the same
//!    lens with the same principal under a FRESH token is the modelled
//!    `409 ConflictException` naming the share already in the way, HOWEVER either request
//!    spells the lens.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import, publish
//! and share wire helpers are the ones `super::custom_lens_import`,
//! `super::custom_lens_versions` and `super::lens_shares` already spell — one spelling of
//! each request for every suite. What a replay left behind is read through the store for the
//! reason `super::lens_shares` gives: a share's own members are not in this operation's
//! answer.

use super::custom_lens_import::{
    import, imported_arn, lens_document, member, wait_for_the_import_to_land,
};
use super::custom_lens_versions::create_version;
use super::lens_shares::{create_share, created_share_id, stored_shares};
use super::workload_harness::{Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The versions every lens in this suite publishes, so a spelling can pin each of them.
///
/// The rows differ along the FORM of the version name, because `LensVersion` is bounded
/// only by length — `@length(1, 32)` and no pattern
/// (`smithy_query(query="shape", shape="LensVersion")`) — so all three are version names,
/// the same spread `super::custom_lens_versions`' rule 1 publishes a lens under.
const PUBLISHED_VERSIONS: [&str; 3] = ["1.0", "2.0", "2024-10-03"];

/// The share state this suite reads back: `(ShareId, SharedWith)` per stored share, in
/// `ShareId` order.
fn stored_grants(client: &Client, alias: &str) -> Vec<(String, String)> {
    stored_shares(client, alias)
        .into_iter()
        .map(|share| (share.share_id, share.shared_with))
        .collect()
}

/// Every spelling that names ONE custom lens, as `(case, LensAlias)`: its plain ARN, and
/// that ARN with each published version pinned after it.
///
/// SOURCE: the `LensAlias` doc quoted in this module's header for the ARN itself, plus the
/// version-pinned form this crate resolves to the same lens (the form
/// `super::lens_share_deletion`'s rule 3 pins). A bare lens id is left out on purpose: "for
/// Amazon Web Services official lenses, this is either the lens alias … or the lens ARN"
/// makes a non-ARN alias an official lens's spelling, and an official lens "cannot be
/// shared" (the `CreateLensShare` doc), which `super::lens_token_alias_spelling`'s rule 4
/// pins as a 400.
fn spellings(arn: &str) -> Vec<(String, String)> {
    let mut spellings = vec![("plain-arn".to_string(), arn.to_string())];
    for version in PUBLISHED_VERSIONS {
        spellings.push((format!("pinned-{version}"), format!("{arn}/{version}")));
    }
    spellings
}

/// Every ordered pair of spellings: the one the original grant named the lens by and the one
/// its retry names the same lens by — so no spelling is only ever used with itself, and each
/// row's identical pair still pins the plain case.
fn spelling_pairs(arn: &str) -> Vec<((String, String), (String, String))> {
    let spellings = spellings(arn);
    spellings
        .iter()
        .flat_map(|original| {
            spellings
                .iter()
                .map(move |retried| (original.clone(), retried.clone()))
        })
        .collect()
}

/// A principal to share with, one per row: 12 characters as `SharedWith` requires at
/// minimum, and never this emulator's own account, because a share with the account doing
/// the sharing is refused.
fn principal(index: usize) -> String {
    let id = format!("8{:011}", index + 1);
    assert_ne!(
        id,
        engine::emulator_account_id(),
        "a row's principal must not be the account doing the sharing, got {id:?}"
    );
    id
}

/// One imported custom lens with every [`PUBLISHED_VERSIONS`] entry published from it, so a
/// version-pinned spelling of its ARN pins a version the lens really holds.
async fn published_lens(client: &Client, label: &str) -> String {
    let answer = import(
        client,
        &lens_document(
            &format!("Share-identity-{label}-lens"),
            None,
            &[("governance", 1, 2)],
        )
        .to_string(),
        None,
        &format!("import-{label}"),
        None,
    )
    .await;
    let arn = imported_arn(&answer, label);
    wait_for_the_import_to_land();
    for version in PUBLISHED_VERSIONS {
        let published = create_version(
            client,
            &arn,
            version,
            Some(true),
            &format!("publish-{label}-{version}"),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {label:?}: the lens under test must publish version {version}, got \
             {published:?}"
        );
    }
    arn
}

/// The modelled `409 ConflictException` a request that is NOT a replay is answered with,
/// naming the share the resolution identified.
fn assert_share_conflict(answer: &Answer, case: &str, share_id: &str) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: expected the modelled 409 ConflictException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        state::LENS_SHARE_RESOURCE_TYPE,
        "case {case:?}: the conflict is about a lens share, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        share_id,
        "case {case:?}: the conflict must name the share in the way, got {answer:?}"
    );
}

/// Rule 1. However the original grant spelled its lens, and however the retry under the
/// SAME token spells the same lens, the retry replays that grant's `ShareId` and mints no
/// second share.
#[tokio::test]
async fn a_share_create_retry_replays_under_every_version_pinned_spelling_of_one_lens() {
    let client = Client::new();
    let arn = published_lens(&client, "replay").await;

    for (index, ((created_as, created_alias), (retried_as, retried_alias))) in
        spelling_pairs(&arn).into_iter().enumerate()
    {
        let case = format!("created-{created_as}-retried-{retried_as}");
        let shared_with = principal(index);
        let token = format!("tok-{case}");

        let created = create_share(&client, &created_alias, &shared_with, &token).await;
        let share_id = created_share_id(&created, &case);

        let retry = create_share(&client, &retried_alias, &shared_with, &token).await;
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

        let stored = stored_grants(&client, &retried_alias);
        assert_eq!(
            stored.len(),
            index + 1,
            "case {case:?}: the replay must mint no second share, got {stored:?}"
        );
        assert!(
            stored
                .iter()
                .any(|(id, with)| id == &share_id && with == &shared_with),
            "case {case:?}: the granted share must still be stored for {shared_with}, got \
             {stored:?}"
        );
    }
}

/// Rule 2. Taking the lens as the lens it names does not flatten the fingerprint: under
/// every spelling, the same token over a different principal, or over a different lens, is
/// another request and the modelled 409 — and records nothing.
#[tokio::test]
async fn the_share_create_token_still_discriminates_different_parameters_under_every_spelling()
{
    let client = Client::new();
    let arn = published_lens(&client, "discriminates").await;
    let other_arn = published_lens(&client, "another").await;

    let rows = spellings(&arn);
    for (index, (spelling, alias)) in rows.iter().enumerate() {
        let shared_with = principal(index);
        let token = format!("tok-{spelling}");
        let granted = create_share(&client, alias, &shared_with, &token).await;
        let share_id = created_share_id(&granted, spelling);
        let before = stored_grants(&client, alias);

        // Same token, same lens, a DIFFERENT principal.
        let elsewhere = principal(index + rows.len());
        let changed_principal = create_share(&client, alias, &elsewhere, &token).await;
        assert_share_conflict(
            &changed_principal,
            &format!("{spelling}/another-principal"),
            &share_id,
        );

        // Same token, same principal, a DIFFERENT lens — under every spelling of that one.
        for (other_spelling, other_alias) in spellings(&other_arn) {
            let changed_lens = create_share(&client, &other_alias, &shared_with, &token).await;
            assert_share_conflict(
                &changed_lens,
                &format!("{spelling}/another-lens-{other_spelling}"),
                &share_id,
            );
        }

        let after = stored_grants(&client, alias);
        assert_eq!(
            after, before,
            "case {spelling:?}: a refused request must record nothing, got {after:?}"
        );
        assert!(
            !after.iter().any(|(_, with)| with == &elsewhere),
            "case {spelling:?}: the refused principal must hold no share, got {after:?}"
        );
        let other_shares = stored_grants(&client, &other_arn);
        assert!(
            other_shares.is_empty(),
            "case {spelling:?}: the other lens must hold no share, got {other_shares:?}"
        );
    }
}

/// Rule 3. The grant a second share collides with is resolved as the grant of the LENS the
/// request names: a fresh token cannot grant the same principal a second share of one lens,
/// however either request spells it.
#[tokio::test]
async fn a_second_share_of_one_lens_collides_under_every_spelling_of_that_lens() {
    let client = Client::new();
    let arn = published_lens(&client, "collides").await;

    for (index, ((granted_as, granted_alias), (again_as, again_alias))) in
        spelling_pairs(&arn).into_iter().enumerate()
    {
        let case = format!("granted-{granted_as}-again-{again_as}");
        let shared_with = principal(index);
        let granted = create_share(
            &client,
            &granted_alias,
            &shared_with,
            &format!("tok-{case}"),
        )
        .await;
        let share_id = created_share_id(&granted, &case);

        let again = create_share(
            &client,
            &again_alias,
            &shared_with,
            &format!("tok-{case}-again"),
        )
        .await;
        assert_share_conflict(&again, &case, &share_id);

        let stored = stored_grants(&client, &granted_alias);
        assert_eq!(
            stored.len(),
            index + 1,
            "case {case:?}: the refused second share must record nothing, got {stored:?}"
        );
    }
}
