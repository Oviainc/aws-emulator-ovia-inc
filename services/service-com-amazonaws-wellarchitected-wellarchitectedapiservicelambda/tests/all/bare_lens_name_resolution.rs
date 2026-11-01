// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule every lens-named operation of this service
//! follows about the SPELLING of a `LensAlias`: a custom lens is named by its ARN, so a name
//! that is not an ARN names an Amazon Web Services official lens and resolves to NO custom
//! lens this account holds — whatever lens id it happens to spell.
//!
//! "For Amazon Web Services official lenses, this is either the lens alias, such as
//! `serverless`, or the lens ARN … For custom lenses, this is the lens ARN, such as
//! `arn:aws:wellarchitected:us-west-2:123456789012:lens/0123…`"
//! (`smithy_query(query="shape", shape="LensAlias")`). A bare name is therefore an official
//! lens's spelling, and the operations of this crate classify it that way before they reach
//! the lens store — so the ones that resolve a lens in order to act on what it HOLDS have to
//! classify it the same way: a bare lens id must not reach the stored custom lens whose id it
//! is, in a read or in a write.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. Every operation that names a lens refuses the bare lens id of a stored, published
//!    custom lens, and the lens, its content and its shares are exactly as they were
//!    afterwards. Rows differ along the OPERATION, each with the refusal its own reading of
//!    the name owes: the 400 `ValidationException` on `LensAlias` where the operation is not
//!    permitted on an official lens, and the 404 `ResourceNotFoundException` naming the LENS
//!    where the name resolves to no lens at all.
//! 2. The two share operations that resolve a lens to reach the shares it holds —
//!    `DeleteLensShare` and `ListLensShares` — answer that 404 for EVERY non-ARN form of the
//!    name, not only the bare id: rows differ along the FORM (the bare id, the bare id with a
//!    version pinned after it, the ARN's resource part on its own, an upper-case id, an
//!    unknown id and a bare name outside the official lens catalog). The share stands
//!    through all of them and is still revocable by the lens's ARN, which is the spelling
//!    `CreateLensShare` recorded it under.
//!
//! A lens Amazon Web Services publishes is a different case and is pinned where it belongs:
//! it EXISTS and holds no shares, so `ListLensShares` reports an empty page for its alias
//! (`super::lens_share_listing` rule 5) and `DeleteLensShare` reports the missing SHARE
//! (`super::lens_share_deletion` rule 4). `ImportLens` is a different case too: a request
//! carrying no ARN is a CREATE there ("to update an existing custom lens, specify its ARN as
//! the LensAlias. If no ARN is specified, a new custom lens is created"), which
//! `super::custom_lens_import` pins.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; every request is
//! issued through the one wire helper its own suite already spells.

use super::custom_lens_deletion::{delete_lens, get_lens};
use super::custom_lens_import::export;
use super::custom_lens_versions::create_version;
use super::lens_resolution::field_names;
use super::lens_share_deletion::delete_share;
use super::lens_share_listing::list_shares;
use super::lens_shares::{create_share, created_share_id, one_published_lens, stored_shares};
use super::lens_version_difference::difference;
use super::workload_harness::{assert_not_found, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The version every lens of this suite is published under, which is what
/// `super::lens_shares::one_published_lens` publishes.
const PUBLISHED_VERSION: &str = "1.0";

/// The member every refusal about the lens NAME is reported under.
const LENS_ALIAS_MEMBER: &str = "LensAlias";

/// The operations that take a `LensAlias` and resolve the lens it names before acting.
#[derive(Debug, Clone, Copy)]
enum Named {
    GetLens,
    ExportLens,
    GetLensVersionDifference,
    CreateLensVersion,
    CreateLensShare,
    ListLensShares,
    DeleteLensShare,
    DeleteLens,
}

/// The answer a refused request owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// 400 `ValidationException` naming `LensAlias`: the operation "is not permitted on
    /// Amazon Web Services official lenses" (`LensAlias` doc), which a non-ARN name is one
    /// of, so what the request got wrong is the member value.
    NotPermitted,
    /// 404 `ResourceNotFoundException` naming the LENS, by the alias AS SENT: the name
    /// resolves to no lens at all — neither a published official alias nor a custom lens,
    /// which only its ARN names.
    NoSuchLens,
}

impl Named {
    /// Issue this operation against `alias`, with `share_id` for the two that name a share.
    async fn send(self, client: &Client, alias: &str, share_id: &str, token: &str) -> Answer {
        match self {
            Named::GetLens => get_lens(client, alias, None).await,
            Named::ExportLens => export(client, alias, None).await,
            Named::GetLensVersionDifference => difference(client, alias, None, None).await,
            Named::CreateLensVersion => {
                create_version(client, alias, "2.0", Some(true), token).await
            }
            Named::CreateLensShare => create_share(client, alias, "111122223333", token).await,
            Named::ListLensShares => list_shares(client, alias, "").await,
            Named::DeleteLensShare => delete_share(client, alias, share_id, token).await,
            Named::DeleteLens => {
                delete_lens(client, alias, state::LENS_STATUS_TYPE_ALL, token).await
            }
        }
    }
}

/// Assert `answer` is the refusal `expected` owes for the name `alias` as sent.
fn assert_refused(case: &str, answer: &Answer, alias: &str, expected: Refusal) {
    match expected {
        Refusal::NotPermitted => {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {answer:?}"
            );
            assert_eq!(
                field_names(answer),
                vec![LENS_ALIAS_MEMBER.to_string()],
                "case {case:?}: the refusal must name the lens member, got {answer:?}"
            );
        }
        Refusal::NoSuchLens => assert_not_found(
            case,
            answer,
            state::LENS_RESOURCE_TYPE,
            alias,
            &format!("lens with alias {alias}"),
        ),
    }
}

/// The `ShareId`s the store holds for the lens `alias` names, in `ShareId` order.
fn stored_share_ids(client: &Client, alias: &str) -> Vec<String> {
    stored_shares(client, alias)
        .into_iter()
        .map(|share| share.share_id)
        .collect()
}

/// The `LensVersion` a `GetLens` by the lens's ARN reports for it — the version the lens is
/// published AS, read over the wire so a refusal that published or deleted a version would
/// show up here.
async fn reported_version(client: &Client, arn: &str, case: &str) -> String {
    let answer = get_lens(client, arn, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the lens must still read back by its ARN, got {answer:?}"
    );
    answer
        .body
        .get("Lens")
        .and_then(|lens| lens.get("LensVersion"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Rule 1. Every operation that names a lens refuses the bare lens id of a stored custom
/// lens, and nothing about that lens moves.
#[tokio::test]
async fn every_lens_operation_refuses_the_bare_id_of_a_stored_custom_lens() {
    // (case, the operation, the refusal it owes a name that can only be an official lens's)
    let rows: [(&str, Named, Refusal); 8] = [
        ("GetLens", Named::GetLens, Refusal::NoSuchLens),
        ("ExportLens", Named::ExportLens, Refusal::NotPermitted),
        (
            "GetLensVersionDifference",
            Named::GetLensVersionDifference,
            Refusal::NoSuchLens,
        ),
        (
            "CreateLensVersion",
            Named::CreateLensVersion,
            Refusal::NotPermitted,
        ),
        (
            "CreateLensShare",
            Named::CreateLensShare,
            Refusal::NotPermitted,
        ),
        ("ListLensShares", Named::ListLensShares, Refusal::NoSuchLens),
        (
            "DeleteLensShare",
            Named::DeleteLensShare,
            Refusal::NoSuchLens,
        ),
        ("DeleteLens", Named::DeleteLens, Refusal::NotPermitted),
    ];

    let client = Client::new();
    let arn = one_published_lens(&client, "bare-id", "Bare-id-lens").await;
    let granted = create_share(&client, &arn, "111122223333", "share-bare-id").await;
    let share_id = created_share_id(&granted, "bare-id");

    // The lens id on its own — the name every row sends. `lens_alias` is the crate's own
    // reduction of a lens ARN to the id its resource part carries, so the id is read off the
    // lens rather than spelled again here.
    let bare_id = state::lens_alias(&arn).to_string();
    assert_ne!(
        bare_id, arn,
        "the bare lens id must differ from the ARN it was read off, got {bare_id:?}"
    );

    // What the lens is before any row runs: the share it holds and the version it was
    // published under. Both must be untouched afterwards, so a refusal cannot be a write in
    // disguise.
    assert_eq!(
        stored_share_ids(&client, &arn),
        vec![share_id.clone()],
        "the lens under test must hold the share this suite granted it"
    );
    assert_eq!(
        reported_version(&client, &arn, "before").await,
        PUBLISHED_VERSION,
        "the lens under test must be published as the version this suite publishes"
    );

    for (case, operation, expected) in rows {
        let answer = operation
            .send(&client, &bare_id, &share_id, &format!("token-{case}"))
            .await;
        assert_refused(case, &answer, &bare_id, expected);

        // Every row's refusal leaves the lens, its published version and its share exactly
        // where they were: a custom lens is reached by its ARN and only by its ARN.
        assert_eq!(
            stored_share_ids(&client, &arn),
            vec![share_id.clone()],
            "case {case:?}: the refusal must leave the lens's shares alone, got {:?}",
            stored_share_ids(&client, &arn)
        );
        // The ARN spelling still reaches the same lens at the same published version, so
        // the row was refused for the NAME it sent — not because the lens had stopped
        // existing — and neither published a version nor deleted one.
        assert_eq!(
            reported_version(&client, &arn, case).await,
            PUBLISHED_VERSION,
            "case {case:?}: the refusal must leave the lens's published version alone"
        );
    }

    let listed = list_shares(&client, &arn, "").await;
    assert_eq!(
        listed.status, 200,
        "the lens's shares must still list by its ARN, got {listed:?}"
    );
}

/// Rule 2. Every non-ARN form of a stored lens's name resolves to no lens in the two share
/// operations, and the share it holds survives all of them.
#[tokio::test]
async fn the_share_operations_resolve_no_lens_for_any_non_arn_form_of_the_name() {
    let client = Client::new();
    let arn = one_published_lens(&client, "non-arn", "Non-arn-lens").await;
    let granted = create_share(&client, &arn, "444455556666", "share-non-arn").await;
    let share_id = created_share_id(&granted, "non-arn");
    let bare_id = state::lens_alias(&arn).to_string();

    // The forms differ along HOW the name spells the lens without being an ARN: the id on
    // its own, the id with the lens's published version pinned after it, the resource part a
    // lens ARN ends in, the id in upper case, a well-formed id nothing is stored under, and
    // a bare name outside the official lens catalog
    // (`state::AWS_OFFICIAL_LENS_ALIASES`, whose entries are the only bare names that name
    // a lens at all).
    let forms: Vec<(&str, String)> = vec![
        ("the-bare-lens-id", bare_id.clone()),
        (
            "the-bare-lens-id-with-a-version-pinned",
            format!("{bare_id}/{PUBLISHED_VERSION}"),
        ),
        ("the-arn-resource-part", format!("lens/{bare_id}")),
        ("the-bare-lens-id-upper-cased", bare_id.to_uppercase()),
        (
            "an-id-naming-no-lens",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        ("a-bare-name-outside-the-catalog", "notalens".to_string()),
    ];
    for (case, form) in &forms {
        assert!(
            !state::AWS_OFFICIAL_LENS_ALIASES.contains(&form.as_str()),
            "case {case:?}: a form of this table must not be a published official alias, got \
             {form:?}"
        );
    }

    for (case, form) in &forms {
        let listed = list_shares(&client, form, "").await;
        assert_refused(
            &format!("{case}-listed"),
            &listed,
            form,
            Refusal::NoSuchLens,
        );

        let revoked = delete_share(&client, form, &share_id, &format!("revoke-{case}")).await;
        assert_refused(
            &format!("{case}-revoked"),
            &revoked,
            form,
            Refusal::NoSuchLens,
        );

        assert_eq!(
            stored_share_ids(&client, &arn),
            vec![share_id.clone()],
            "case {case:?}: a name that resolves to no lens must remove no share, got {:?}",
            stored_share_ids(&client, &arn)
        );
    }

    // The share is still the lens's own, reachable and revocable by the ARN
    // `CreateLensShare` recorded it under.
    let listed = list_shares(&client, &arn, "").await;
    assert_eq!(
        listed.status, 200,
        "the lens's shares must still list by its ARN, got {listed:?}"
    );
    let revoked = delete_share(&client, &arn, &share_id, "revoke-by-arn").await;
    assert_eq!(
        revoked.status, 200,
        "the share must still be revocable by the lens's ARN, got {revoked:?}"
    );
    assert_eq!(
        stored_share_ids(&client, &arn),
        Vec::<String>::new(),
        "the revoke under the ARN must remove the share, got {:?}",
        stored_share_ids(&client, &arn)
    );
}
