// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteLens`: the delete of a custom lens's content, and the
//! reads that prove what it removed — `GetLens`, `ExportLens` and `ListLenses`.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. `LensStatus` is "the status of the lens to be deleted", so it selects WHICH content
//!    of the lens the request removes, by the publication state that content is in — the
//!    same publication vocabulary `ListLenses` filters its own `LensStatus` on. `ALL`
//!    removes every version and the draft, so the lens itself is gone; `DRAFT` removes
//!    only the content no version has been published from, leaving the lens at its latest
//!    published version; `PUBLISHED` removes only the published versions, leaving the
//!    content the lens carries as a draft again. A lens with nothing left is gone from the
//!    store. Rows differ along the publication state the lens is in when the delete
//!    arrives — a draft nothing was published from, one version, several versions, and a
//!    draft imported over published versions — because the rule is about that state and
//!    not about one lens.
//! 2. Content the request's status selects nothing of is the lookup's 404
//!    `ResourceNotFoundException`, and the lens is left exactly as it stood: a lens whose
//!    content is published has no draft to delete, and a lens no version was published
//!    from has no published version to delete.
//! 3. A lens the caller cannot delete refuses the request and removes nothing: an Amazon
//!    Web Services official lens is a 400 `ValidationException` (AWS publishes its own
//!    lenses), a `LensAlias` naming no stored custom lens is a 404, and a `LensStatus`
//!    outside the three members `LensStatusType` publishes is a 400.
//! 4. The delete is idempotent per `ClientRequestToken`: the same token naming the same
//!    lens and status replays the original request's empty 200 without resurrecting what
//!    it removed, the same token naming a different lens or a different status is a 409
//!    `ConflictException` that removes nothing, and a REFUSED delete spends no token.
//! 5. A share grants access TO a lens, so it cannot outlive the lens: a delete that
//!    removes the lens drops every share recorded against it, the create token that
//!    resolved into each and the invitations those shares raised, while a delete that
//!    leaves the lens standing leaves its shares alone.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import,
//! export, publish and share wire helpers are the ones `super::custom_lens_import`,
//! `super::custom_lens_versions` and `super::lens_shares` already spell. The store is read
//! only for the records the HTTP surface can no longer reach once a lens is gone (its
//! shares answer a 404 with it), exactly as `super::shared_resource_deletion` does.

use serde_json::Value;

use super::custom_lens_import::{
    export, import, imported_arn, lens_document, member, wait_for_the_import_to_land,
    ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::lens_shares::{create_share, record_lens_share, stored_shares, PRINCIPALS};
use super::share_invitations::{invitation_ids, invitations};
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `DELETE /lenses/{LensAlias}`, the wire form of `DeleteLens`, with both required query
/// members bound to the query string. The alias is percent-encoded because a custom lens
/// is named by its ARN, which carries `:` and `/` inside ONE path segment. `pub(crate)`
/// because `super::lens_token_alias_spelling` deletes its lenses through this one spelling
/// of the request.
pub(crate) async fn delete_lens(
    client: &Client,
    alias: &str,
    status: &str,
    token: &str,
) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/lenses/{}?ClientRequestToken={}&LensStatus={}",
                encode_path_segment(alias),
                encode_path_segment(token),
                encode_path_segment(status)
            ),
            None,
        )
        .await
}

/// `GET /lenses/{LensAlias}`, the wire form of `GetLens` — the read that proves what a
/// delete left. `pub(crate)` because `super::bare_lens_name_resolution` reads a lens back
/// through this one spelling of the request.
pub(crate) async fn get_lens(client: &Client, alias: &str, version: Option<&str>) -> Answer {
    let path = format!("/lenses/{}", encode_path_segment(alias));
    let uri = match version {
        Some(version) => format!("{path}?LensVersion={}", encode_path_segment(version)),
        None => path,
    };
    client.send("GET", &uri, None).await
}

/// `GET /lenses`, the wire form of `ListLenses`, under one status filter.
async fn listed_arns(client: &Client, status: &str, case: &str) -> Vec<String> {
    let answer = client
        .send(
            "GET",
            &format!("/lenses?LensStatus={status}&MaxResults=50"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLenses 200, got {answer:?}"
    );
    answer
        .body
        .get("LensSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListLenses must report LensSummaries, got {answer:?}")
        })
        .iter()
        .map(|summary| {
            summary
                .get("LensArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `LensVersion` a `GetLens` reports the lens at — its latest published version.
fn reported_version(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLens 200, got {answer:?}"
    );
    answer
        .body
        .get("Lens")
        .and_then(|lens| lens.get("LensVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every assertion the result of a `DeleteLens` has to satisfy: the modelled empty 200 it
/// answers (`http {method: DELETE, code: 200}` over a `Unit` output).
fn assert_deleted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteLens 200, got {answer:?}"
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case:?}: DeleteLens models no output members, so the body is empty, got \
         {answer:?}"
    );
    assert!(
        answer.error_code.is_empty(),
        "case {case:?}: a successful DeleteLens carries no error code, got {answer:?}"
    );
}

/// Whether the store still holds the lens `alias` names — the one thing no read can report
/// once the lens is gone.
fn is_stored(client: &Client, alias: &str) -> bool {
    let store = client.store();
    let store = store.lock();
    store.lens_by_alias(alias).is_some()
}

/// The publication state a lens under test is in when the delete arrives.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// An import landed and no version was published from it: the lens carries a draft
    /// only.
    DraftOnly,
    /// `n` versions published, the last from the content the lens now carries: the lens is
    /// published and carries no draft.
    Published(usize),
    /// `n` versions published and then a further document imported over them: the lens
    /// carries a draft AND published versions at once.
    Redrafted(usize),
}

impl Shape {
    /// How many versions this shape publishes.
    fn versions(self) -> usize {
        match self {
            Self::DraftOnly => 0,
            Self::Published(count) | Self::Redrafted(count) => count,
        }
    }
}

/// One lens built to a [`Shape`], with the documents its content came from.
#[derive(Debug)]
struct Prepared {
    /// The lens ARN its import minted — the `LensAlias` every request below names it by.
    arn: String,
    /// `(LensVersion, the document that version published)`, in publish order.
    versions: Vec<(String, String)>,
    /// The document the lens carries that no version was published from, if it carries
    /// one.
    draft: Option<String>,
}

impl Prepared {
    /// The document the lens's CURRENT content is, whatever publication state it is in.
    fn current(&self) -> &str {
        self.draft.as_deref().unwrap_or_else(|| {
            self.versions
                .last()
                .map(|(_, document)| document.as_str())
                .expect("a prepared lens carries content")
        })
    }
}

/// Import a lens, publish `shape.versions()` versions of it — each from its own draft, so
/// no two versions carry the same document — and leave it in `shape`'s publication state.
async fn prepare(client: &Client, case: &str, shape: Shape) -> Prepared {
    let first = lens_document(
        &format!("Deletable lens {case}"),
        None,
        &[("governance", 1, 2)],
    )
    .to_string();
    let imported = import(client, &first, None, &format!("import-{case}"), None).await;
    let arn = imported_arn(&imported, case);
    wait_for_the_import_to_land();

    let mut prepared = Prepared {
        arn,
        versions: Vec::new(),
        draft: Some(first),
    };
    for index in 0..shape.versions() {
        // Each publish takes the draft as it stands, so the draft is replaced between
        // publishes and every version carries a document of its own.
        let document = lens_document(
            &format!("Deletable lens {case} v{index}"),
            Some("the content one version published"),
            &[("governance", index + 1, 2)],
        )
        .to_string();
        let update = import(
            client,
            &document,
            Some(&prepared.arn),
            &format!("import-{case}-{index}"),
            None,
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: the update import must be accepted, got {update:?}"
        );
        wait_for_the_import_to_land();

        let version = format!("{}.0", index + 1);
        let published = create_version(
            client,
            &prepared.arn,
            &version,
            Some(index == 0),
            &format!("pub-{case}-{index}"),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: publishing {version} must succeed, got {published:?}"
        );
        prepared.versions.push((version, document));
        // The lens's content is now that published version's, so it carries no draft.
        prepared.draft = None;
    }

    if let Shape::Redrafted(_) = shape {
        let document = lens_document(
            &format!("Deletable lens {case} redraft"),
            None,
            &[("security", 2, 3)],
        )
        .to_string();
        let update = import(
            client,
            &document,
            Some(&prepared.arn),
            &format!("redraft-{case}"),
            None,
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: the redraft import must be accepted, got {update:?}"
        );
        wait_for_the_import_to_land();
        prepared.draft = Some(document);
    }

    prepared
}

/// Rule 1, `ALL`: every version of the lens and its draft go, so the lens itself is gone —
/// no read resolves it and the catalog no longer lists it. The rows are the publication
/// states a lens can be in when the delete arrives.
#[tokio::test]
async fn deleting_all_of_a_lens_removes_the_lens_itself() {
    let shapes: [(&str, Shape); 4] = [
        ("draft-only", Shape::DraftOnly),
        ("one-version", Shape::Published(1)),
        ("two-versions", Shape::Published(2)),
        ("redrafted-over-one-version", Shape::Redrafted(1)),
    ];

    for (case, shape) in shapes {
        let client = Client::new();
        let lens = prepare(&client, case, shape).await;

        // Before the delete the lens's content is readable and the lens is listed. The read
        // that proves it is `ExportLens`, which answers the content of a lens in EVERY
        // publication state; `GetLens` answers only a published one, so it is asked here
        // for the shapes that have published a version.
        let before = export(&client, &lens.arn, None).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the prepared lens's content must be readable, got {before:?}"
        );
        if !lens.versions.is_empty() {
            let read = get_lens(&client, &lens.arn, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: a published lens must be retrievable, got {read:?}"
            );
        }
        assert!(
            listed_arns(&client, state::LENS_STATUS_TYPE_ALL, case)
                .await
                .contains(&lens.arn),
            "case {case:?}: the prepared lens must be listed before the delete"
        );

        let deleted = delete_lens(
            &client,
            &lens.arn,
            state::LENS_STATUS_TYPE_ALL,
            &format!("del-{case}"),
        )
        .await;
        assert_deleted(&deleted, case);

        let gone = get_lens(&client, &lens.arn, None).await;
        assert_eq!(
            (gone.status, gone.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted lens must not resolve, got {gone:?}"
        );
        let exported = export(&client, &lens.arn, None).await;
        assert_eq!(
            exported.status, 404,
            "case {case:?}: a deleted lens has no content to export, got {exported:?}"
        );
        for (version, _) in &lens.versions {
            let pinned = get_lens(&client, &lens.arn, Some(version)).await;
            assert_eq!(
                pinned.status, 404,
                "case {case:?}: version {version} must go with the lens, got {pinned:?}"
            );
            let pinned_export = export(&client, &lens.arn, Some(version)).await;
            assert_eq!(
                pinned_export.status, 404,
                "case {case:?}: version {version} must export nothing, got {pinned_export:?}"
            );
        }
        assert!(
            !listed_arns(&client, state::LENS_STATUS_TYPE_ALL, case)
                .await
                .contains(&lens.arn),
            "case {case:?}: a deleted lens must leave the catalog"
        );
        assert!(
            !is_stored(&client, &lens.arn),
            "case {case:?}: the store must hold no record of a deleted lens"
        );
    }
}

/// Rule 1, `DRAFT`: only the content no version was published from goes. A lens that has
/// published a version stays, reporting its LATEST published version and that version's
/// content; a lens that has published none has nothing left, so it is gone. The rows are
/// how many versions the lens had published when its draft was deleted.
#[tokio::test]
async fn deleting_the_draft_leaves_the_lens_at_its_latest_published_version() {
    let shapes: [(&str, Shape); 3] = [
        ("draft-only", Shape::DraftOnly),
        ("redrafted-over-one-version", Shape::Redrafted(1)),
        ("redrafted-over-two-versions", Shape::Redrafted(2)),
    ];

    for (case, shape) in shapes {
        let client = Client::new();
        let lens = prepare(&client, case, shape).await;
        let discarded = lens.current().to_string();

        let deleted = delete_lens(
            &client,
            &lens.arn,
            state::LENS_STATUS_TYPE_DRAFT,
            &format!("del-{case}"),
        )
        .await;
        assert_deleted(&deleted, case);

        match lens.versions.last() {
            // No version was ever published, so the draft was everything the lens had.
            None => {
                let gone = get_lens(&client, &lens.arn, None).await;
                assert_eq!(
                    (gone.status, gone.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: a lens whose only content was its draft must be gone, \
                     got {gone:?}"
                );
                assert!(
                    !is_stored(&client, &lens.arn),
                    "case {case:?}: the store must hold no record of a lens with nothing left"
                );
            }
            Some((version, document)) => {
                // The lens stands, at the version it last published.
                let read = get_lens(&client, &lens.arn, None).await;
                assert_eq!(
                    reported_version(&read, case),
                    version.as_str(),
                    "case {case:?}: the lens must report its latest published version, got \
                     {read:?}"
                );
                let current = export(&client, &lens.arn, None).await;
                assert_eq!(
                    member(&current, "LensJSON"),
                    document.as_str(),
                    "case {case:?}: the lens's content must be that version's, got {current:?}"
                );
                assert_ne!(
                    member(&current, "LensJSON"),
                    discarded.as_str(),
                    "case {case:?}: the deleted draft must not be the lens's content any more"
                );
                // Every published version is untouched, and the lens is listed as a
                // published one.
                for (held, published) in &lens.versions {
                    let pinned = export(&client, &lens.arn, Some(held)).await;
                    assert_eq!(
                        member(&pinned, "LensJSON"),
                        published.as_str(),
                        "case {case:?}: version {held} must survive the draft's delete, got \
                         {pinned:?}"
                    );
                }
                let published_lenses =
                    listed_arns(&client, state::LENS_STATUS_TYPE_PUBLISHED, case).await;
                assert!(
                    published_lenses.contains(&lens.arn),
                    "case {case:?}: the lens must be listed as published, got \
                     {published_lenses:?}"
                );
            }
        }
    }
}

/// Rule 1, `PUBLISHED`: only the versions published from the lens go. The lens stands,
/// carrying the content it had as a draft again — so a `GetLens` no longer retrieves it, no
/// version of it resolves any more, and the status filter finds it under `DRAFT`. The rows
/// are how many versions the lens had published, and whether its content was one of them.
#[tokio::test]
async fn deleting_the_published_versions_leaves_the_lens_as_a_draft() {
    let shapes: [(&str, Shape); 3] = [
        ("one-version", Shape::Published(1)),
        ("three-versions", Shape::Published(3)),
        ("redrafted-over-one-version", Shape::Redrafted(1)),
    ];

    for (case, shape) in shapes {
        let client = Client::new();
        let lens = prepare(&client, case, shape).await;
        let content = lens.current().to_string();

        let deleted = delete_lens(
            &client,
            &lens.arn,
            state::LENS_STATUS_TYPE_PUBLISHED,
            &format!("del-{case}"),
        )
        .await;
        assert_deleted(&deleted, case);

        // The lens stands as a draft, so it is no longer retrievable by `GetLens` — a lens
        // "cannot be applied to workloads or shared … until it's published with
        // CreateLensVersion" (`ImportLens` doc) — while `ExportLens` still answers the
        // content it kept.
        let read = get_lens(&client, &lens.arn, None).await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a lens with no published version left is a draft, got {read:?}"
        );
        let current = export(&client, &lens.arn, None).await;
        assert_eq!(
            member(&current, "LensJSON"),
            content.as_str(),
            "case {case:?}: the lens must keep the content it carried, got {current:?}"
        );
        for (version, _) in &lens.versions {
            let pinned = get_lens(&client, &lens.arn, Some(version)).await;
            assert_eq!(
                (pinned.status, pinned.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: version {version} must not resolve after the delete, got \
                 {pinned:?}"
            );
            let pinned_export = export(&client, &lens.arn, Some(version)).await;
            assert_eq!(
                pinned_export.status, 404,
                "case {case:?}: version {version} must export nothing, got {pinned_export:?}"
            );
        }
        let drafts = listed_arns(&client, state::LENS_STATUS_TYPE_DRAFT, case).await;
        assert!(
            drafts.contains(&lens.arn),
            "case {case:?}: the lens must be listed as a draft, got {drafts:?}"
        );
        let published = listed_arns(&client, state::LENS_STATUS_TYPE_PUBLISHED, case).await;
        assert!(
            !published.contains(&lens.arn),
            "case {case:?}: the lens must no longer be listed as published, got {published:?}"
        );

        // The lens can publish again, which proves the version names were really freed
        // rather than merely hidden.
        let republished = create_version(
            &client,
            &lens.arn,
            &lens
                .versions
                .first()
                .map_or_else(|| "1.0".to_string(), |(version, _)| version.clone()),
            Some(true),
            &format!("republish-{case}"),
        )
        .await;
        assert_eq!(
            republished.status, 200,
            "case {case:?}: a version name the delete freed must publish again, got \
             {republished:?}"
        );
    }
}

/// Rule 2. A status selecting content the lens does not carry is the lookup's 404, and the
/// lens is left exactly as it stood — including the token the refused request carried,
/// which is still free to name a delete that completes. The rows are the two ways a status
/// can select nothing.
#[tokio::test]
async fn a_status_the_lens_carries_nothing_in_is_a_not_found() {
    // (case, the lens's state, the status that selects nothing of it)
    let cases: [(&str, Shape, &str); 3] = [
        (
            "no-draft-to-delete",
            Shape::Published(1),
            state::LENS_STATUS_TYPE_DRAFT,
        ),
        (
            "no-draft-to-delete-two-versions",
            Shape::Published(2),
            state::LENS_STATUS_TYPE_DRAFT,
        ),
        (
            "no-published-version-to-delete",
            Shape::DraftOnly,
            state::LENS_STATUS_TYPE_PUBLISHED,
        ),
    ];

    for (case, shape, status) in cases {
        let client = Client::new();
        let lens = prepare(&client, case, shape).await;
        let before = get_lens(&client, &lens.arn, None).await;
        let content_before = export(&client, &lens.arn, None).await;
        let token = format!("del-{case}");

        let refused = delete_lens(&client, &lens.arn, status, &token).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: deleting {status} content the lens has none of must be refused, \
             got {refused:?}"
        );
        assert_eq!(
            refused.body.get("ResourceType").and_then(Value::as_str),
            Some(state::LENS_RESOURCE_TYPE),
            "case {case:?}: the refusal must name the resource type LENS, got {refused:?}"
        );
        assert_eq!(
            refused.body.get("ResourceId").and_then(Value::as_str),
            Some(lens.arn.as_str()),
            "case {case:?}: the refusal must name the lens as sent, got {refused:?}"
        );

        // Nothing moved: the lens reads back exactly as it did, with its content and every
        // version it published.
        let after = get_lens(&client, &lens.arn, None).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused delete must leave the lens as it stood, got {after:?}"
        );
        let content_after = export(&client, &lens.arn, None).await;
        assert_eq!(
            content_after.body, content_before.body,
            "case {case:?}: a refused delete must leave the lens's content, got \
             {content_after:?}"
        );
        for (version, document) in &lens.versions {
            let pinned = export(&client, &lens.arn, Some(version)).await;
            assert_eq!(
                member(&pinned, "LensJSON"),
                document.as_str(),
                "case {case:?}: a refused delete must leave version {version}, got {pinned:?}"
            );
        }

        // The refused request completed nothing, so its token still names a request that
        // does complete.
        let accepted = delete_lens(&client, &lens.arn, state::LENS_STATUS_TYPE_ALL, &token).await;
        assert_deleted(&accepted, &format!("{case}/token-reused"));
        assert!(
            !is_stored(&client, &lens.arn),
            "case {case:?}: the reused token must have deleted the lens"
        );
    }
}

/// Rule 3. A lens the caller cannot delete, or a status outside the published member set,
/// refuses the request and removes nothing. The rows differ along HOW the request cannot
/// be served.
#[tokio::test]
async fn a_lens_or_status_that_cannot_be_deleted_is_refused() {
    let client = Client::new();
    let lens = prepare(&client, "refusals", Shape::Published(1)).await;
    // A custom ARN that agrees on the lens id and disagrees on the owning account names a
    // different lens, so it resolves to nothing.
    let other_account = lens.arn.replacen(
        &format!(":{}:", engine::emulator_account_id()),
        ":210987654321:",
        1,
    );

    // (case, alias, status, expected status, expected error code)
    let cases: Vec<(&str, String, &str, u16, &str)> = vec![
        (
            "official-bare-alias",
            state::FRAMEWORK_LENS_ALIAS.to_string(),
            state::LENS_STATUS_TYPE_ALL,
            400,
            "ValidationException",
        ),
        (
            "official-arn",
            "arn:aws:wellarchitected:us-west-2::lens/serverless".to_string(),
            state::LENS_STATUS_TYPE_ALL,
            400,
            "ValidationException",
        ),
        (
            "unknown-custom-arn",
            ABSENT_LENS_ARN.to_string(),
            state::LENS_STATUS_TYPE_ALL,
            404,
            "ResourceNotFoundException",
        ),
        (
            "another-account",
            other_account,
            state::LENS_STATUS_TYPE_ALL,
            404,
            "ResourceNotFoundException",
        ),
        // `LensStatus` is a `LensStatusType`, which publishes exactly `ALL`, `DRAFT` and
        // `PUBLISHED`; `CURRENT` is a member of the OTHER lens status enum (`LensStatus`,
        // the version currency a lens REVIEW reports), so it names no publication state
        // here. The lowercase row is the same rule: the members are case-sensitive.
        (
            "status-of-another-enum",
            lens.arn.clone(),
            "CURRENT",
            400,
            "ValidationException",
        ),
        (
            "status-in-the-wrong-case",
            lens.arn.clone(),
            "all",
            400,
            "ValidationException",
        ),
    ];

    for (case, alias, status, status_code, error_code) in &cases {
        let answer = delete_lens(&client, alias, status, &format!("del-{case}")).await;
        assert_eq!(
            answer.status, *status_code,
            "case {case:?}: expected HTTP {status_code}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, *error_code,
            "case {case:?}: expected {error_code}, got {answer:?}"
        );
    }

    // Every refusal left the lens under test exactly where it was, published version and
    // all.
    let read = get_lens(&client, &lens.arn, None).await;
    assert_eq!(
        reported_version(&read, "refusals"),
        lens.versions[0].0.as_str(),
        "a refused delete must leave the lens at its published version, got {read:?}"
    );
    let exported = export(&client, &lens.arn, Some(&lens.versions[0].0)).await;
    assert_eq!(
        member(&exported, "LensJSON"),
        lens.versions[0].1.as_str(),
        "a refused delete must leave the published version's content, got {exported:?}"
    );
    assert!(
        is_stored(&client, &lens.arn),
        "a refused delete must leave the lens stored"
    );
}

/// Rule 4. The `@idempotencyToken` rule on a delete: a retry under the same token and the
/// same parameters replays the empty 200, and a token already spent on another request is
/// a 409 that removes nothing. The rows differ along how the second request differs from
/// the first — not at all, in the status it names, and in the lens it names.
#[tokio::test]
async fn the_delete_is_idempotent_per_client_request_token() {
    // (case, the status the retry names, the lens the retry names: true for the lens the
    // original request deleted, false for another lens, whether it is a replay)
    let cases: [(&str, &str, bool, bool); 4] = [
        ("same-request", state::LENS_STATUS_TYPE_ALL, true, true),
        (
            "different-status",
            state::LENS_STATUS_TYPE_DRAFT,
            true,
            false,
        ),
        ("different-lens", state::LENS_STATUS_TYPE_ALL, false, false),
        (
            "different-lens-and-status",
            state::LENS_STATUS_TYPE_PUBLISHED,
            false,
            false,
        ),
    ];

    for (case, status, same_lens, replay) in cases {
        let client = Client::new();
        let deleted = prepare(&client, case, Shape::Published(1)).await;
        let other = prepare(&client, &format!("{case}-other"), Shape::Published(1)).await;
        let token = format!("tok-{case}");

        let first = delete_lens(&client, &deleted.arn, state::LENS_STATUS_TYPE_ALL, &token).await;
        assert_deleted(&first, case);

        let target = if same_lens { &deleted.arn } else { &other.arn };
        let second = delete_lens(&client, target, status, &token).await;
        if replay {
            assert_deleted(&second, &format!("{case}/retry"));
            assert_eq!(
                second.body, first.body,
                "case {case:?}: a replay must answer the original result, got {second:?}"
            );
            // A replay answers a result; it does not bring the lens back.
            let still_gone = get_lens(&client, &deleted.arn, None).await;
            assert_eq!(
                still_gone.status, 404,
                "case {case:?}: a replay must not resurrect the lens, got {still_gone:?}"
            );
        } else {
            assert_eq!(
                second.status, 409,
                "case {case:?}: a token spent on another request must be refused, got \
                 {second:?}"
            );
            assert_eq!(
                second.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {second:?}"
            );
            assert_eq!(
                second.body.get("ResourceId").and_then(Value::as_str),
                Some(deleted.arn.as_str()),
                "case {case:?}: the conflict must name what the token already deleted, got \
                 {second:?}"
            );
        }

        // The lens the original request did NOT name is untouched either way.
        assert!(
            is_stored(&client, &other.arn),
            "case {case:?}: a lens the token's request never named must stand"
        );
        let other_read = get_lens(&client, &other.arn, None).await;
        assert_eq!(
            reported_version(&other_read, case),
            other.versions[0].0.as_str(),
            "case {case:?}: the other lens must keep its published version, got {other_read:?}"
        );
    }
}

/// Rule 5. A share grants access TO the lens, so it cannot outlive it: the delete that
/// removes the lens drops every share recorded against it, the create token that resolved
/// into each and the invitations those shares raised, while a delete that leaves the lens
/// standing leaves its shares alone. The rows are the deletes of a published lens, by what
/// they leave behind.
#[tokio::test]
async fn deleting_a_lens_drops_its_shares_while_a_partial_delete_keeps_them() {
    /// Shares granted over the wire per row, well below the per-lens share quota.
    const GRANTS: usize = 2;

    // (case, the status the delete names, whether the lens survives it)
    let cases: [(&str, &str, bool); 2] = [
        ("all", state::LENS_STATUS_TYPE_ALL, false),
        ("published", state::LENS_STATUS_TYPE_PUBLISHED, true),
    ];

    for (case, status, survives) in cases {
        let client = Client::new();
        let lens = prepare(&client, case, Shape::Published(1)).await;
        let kept = prepare(&client, &format!("{case}-kept"), Shape::Published(1)).await;

        let mut grant_tokens: Vec<String> = Vec::new();
        for (label, principal) in PRINCIPALS.iter().take(GRANTS) {
            let token = format!("grant-{case}-{label}");
            let granted = create_share(&client, &lens.arn, principal, &token).await;
            assert_eq!(
                granted.status, 200,
                "case {case:?}: expected CreateLensShare 200, got {granted:?}"
            );
            grant_tokens.push(token);
        }
        // The one share whose invitation this caller can be offered, so the invitation side
        // of the rule is observable over HTTP.
        let (_, invitation) = record_lens_share(&client, &lens.arn, engine::emulator_account_id());
        let (_, kept_invitation) =
            record_lens_share(&client, &kept.arn, engine::emulator_account_id());
        let kept_token = format!("grant-{case}-kept");
        let kept_grant = create_share(&client, &kept.arn, PRINCIPALS[0].1, &kept_token).await;
        assert_eq!(
            kept_grant.status, 200,
            "case {case:?}: expected CreateLensShare 200 for the kept lens, got {kept_grant:?}"
        );

        assert_eq!(
            stored_shares(&client, &lens.arn).len(),
            GRANTS + 1,
            "case {case:?}: every share must be recorded before the delete"
        );
        let before = invitation_ids(&invitations(
            &client.list_share_invitations("").await,
            case,
        ));
        assert!(
            before.contains(&invitation) && before.contains(&kept_invitation),
            "case {case:?}: both invitations must be outstanding before the delete, got \
             {before:?}"
        );

        assert_deleted(
            &delete_lens(&client, &lens.arn, status, &format!("del-{case}")).await,
            case,
        );

        let held = stored_shares(&client, &lens.arn);
        let outstanding =
            invitation_ids(&invitations(&client.list_share_invitations("").await, case));
        if survives {
            assert_eq!(
                held.len(),
                GRANTS + 1,
                "case {case:?}: a delete that leaves the lens standing leaves its shares, \
                 got {held:?}"
            );
            assert!(
                outstanding.contains(&invitation),
                "case {case:?}: the invitation of a surviving lens's share must still be \
                 reported, got {outstanding:?}"
            );
        } else {
            assert!(
                held.is_empty(),
                "case {case:?}: a deleted lens's shares must go with it, got {held:?}"
            );
            assert!(
                !outstanding.contains(&invitation),
                "case {case:?}: the invitation of a deleted lens's share must stop being \
                 reported, got {outstanding:?}"
            );
            let store = client.store();
            let store = store.lock();
            for token in &grant_tokens {
                assert!(
                    store.create_lens_share_tokens.get(token).is_none(),
                    "case {case:?}: the create token of a dropped share must go with it \
                     ({token})"
                );
            }
        }

        // The lens the delete did not name keeps its share, its create token and its
        // invitation, whichever status the delete named.
        assert_eq!(
            stored_shares(&client, &kept.arn).len(),
            2,
            "case {case:?}: a lens the delete did not name keeps every share of its own"
        );
        assert!(
            outstanding.contains(&kept_invitation),
            "case {case:?}: another lens's invitation must still be reported, got \
             {outstanding:?}"
        );
        let store = client.store();
        let store = store.lock();
        assert!(
            store.create_lens_share_tokens.get(&kept_token).is_some(),
            "case {case:?}: another lens's create token must survive"
        );
    }
}
