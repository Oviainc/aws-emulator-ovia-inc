// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the chain a custom lens goes through after `ImportLens`:
//! `ExportLens`, `CreateLensVersion` and `GetLens`, each against the lens the import
//! minted.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. The document `ImportLens` accepts is the lens's content from that moment on, so the
//!    operations that resolve the lens resolve it straight away: `ExportLens` answers the
//!    document and `CreateLensVersion` publishes it, answering the lens's own ARN and the
//!    `LensVersion` the request named. Rows differ along the shape of the document and
//!    along the form of the version name, because the rule is about the lens, not about
//!    one document or one version name.
//! 2. `GetLens` retrieves a custom lens only through a version PUBLISHED from it — "The
//!    new or updated lens will have a status of DRAFT. The lens cannot be applied to
//!    workloads or shared with other Amazon Web Services accounts until it's published
//!    with CreateLensVersion" (`ImportLens` doc) — so the same lens is a 404
//!    `ResourceNotFoundException` naming the LENS and the lens's id before the publish and
//!    is reported at that version afterwards, with every other member taken from the lens's
//!    own record.
//! 3. A `LensVersion` the lens has already published is a 409 `ConflictException` naming
//!    the lens and the version, while a `LensAlias` naming no stored lens stays the 404 of
//!    the lookup: the lens is resolved BEFORE the version name is judged, so "already
//!    published" and "no such lens" are never collapsed into one answer. The rows are the
//!    version names rule 1 published, each republished, beside the names of no lens.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import, export
//! and publish wire helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell.

use serde_json::Value;

use super::custom_lens_import::{
    export, import, imported_arn, lens_document, member, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `GET /lenses/{LensAlias}`, the wire form of `GetLens`. The alias is percent-encoded
/// because a custom lens is named by its ARN, which carries `:` and `/` inside ONE path
/// segment.
async fn get_lens(client: &Client, alias: &str) -> Answer {
    client
        .send(
            "GET",
            &format!("/lenses/{}", encode_path_segment(alias)),
            None,
        )
        .await
}

/// One row of the chain table: a case name, the lens document to import and the
/// `LensVersion` to publish it under.
struct Row {
    case: &'static str,
    document: Value,
    version: String,
}

/// The rows of the chain: the document shape varies (how many pillars and questions, and
/// whether it carries a description) and so does the form of the version name. `LensVersion`
/// is `@length(min: 1, max: 32)`
/// (`smithy_query(query="operation", operation="CreateLensVersion")`), so the names span a
/// single character and a name AT the ceiling beside the semantic and dated forms a caller
/// uses.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "semantic-version",
            document: lens_document("Chain lens", None, &[("governance", 1, 2)]),
            version: "1.0".to_string(),
        },
        Row {
            case: "single-character-version",
            document: lens_document(
                "Described chain lens",
                Some("the questions company policy adds to a review"),
                &[("security", 2, 3)],
            ),
            version: "1".to_string(),
        },
        Row {
            case: "dated-version",
            document: lens_document(
                "Wide chain lens",
                Some("several pillars of questions"),
                &[("security", 1, 2), ("costOptimization", 3, 2)],
            ),
            version: "2024-01-01".to_string(),
        },
        Row {
            case: "version-at-the-ceiling",
            document: lens_document("Lentille d'évaluation", None, &[("durabilité", 2, 2)]),
            version: "9".repeat(32),
        },
    ]
}

/// The message a read of a lens no version has been published from is refused with, as the
/// lens's own id names the lens.
fn not_published_message(lens_id: &str) -> String {
    format!(
        "[ResourceNotFound] {} with id {lens_id} not found. Lens is not published.",
        state::LENS_RESOURCE_TYPE
    )
}

/// Rules 1 and 2. The accepted import is exportable and publishable at once, and the
/// `GetLens` of the lens turns from the not-published 404 into the published version.
#[tokio::test]
async fn an_accepted_import_is_exportable_publishable_and_then_retrievable() {
    for row in rows() {
        let client = Client::new();
        let case = row.case;
        let document = row.document.to_string();
        let arn = imported_arn(
            &import(&client, &document, None, &format!("chain-{case}"), None).await,
            case,
        );
        // The id the lens's ARN ends in, read off the ARN through the crate's own
        // reduction of a lens name rather than spelled again here.
        let lens_id = state::lens_alias(&arn).to_string();

        // Rule 1: the document the import accepted is the lens's content.
        let exported = export(&client, &arn, None).await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: an accepted import must be exportable, got {exported:?}"
        );
        assert_eq!(
            member(&exported, "LensJSON"),
            document.as_str(),
            "case {case:?}: ExportLens must answer the document as imported, got {exported:?}"
        );

        // Rule 2, before the publish: the lens is not retrievable through `GetLens`.
        let unpublished = get_lens(&client, &arn).await;
        assert_eq!(
            (unpublished.status, unpublished.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unpublished lens must not be retrievable, got {unpublished:?}"
        );
        assert_eq!(
            unpublished.body.get("ResourceType").and_then(Value::as_str),
            Some(state::LENS_RESOURCE_TYPE),
            "case {case:?}: the refusal must name the LENS resource type, got {unpublished:?}"
        );
        assert_eq!(
            unpublished.body.get("ResourceId").and_then(Value::as_str),
            Some(lens_id.as_str()),
            "case {case:?}: the refusal must name the lens by its id, got {unpublished:?}"
        );
        assert_eq!(
            unpublished.body.get("Message").and_then(Value::as_str),
            Some(not_published_message(&lens_id).as_str()),
            "case {case:?}: the refusal must say the lens is not published, got {unpublished:?}"
        );

        // Rule 1: the publish answers the lens's ARN and the version it was asked for.
        let published = create_version(
            &client,
            &arn,
            &row.version,
            Some(true),
            &format!("publish-{case}"),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the imported lens must publish, got {published:?}"
        );
        assert_eq!(
            member(&published, "LensArn"),
            arn.as_str(),
            "case {case:?}: the publish must answer the lens's own ARN, got {published:?}"
        );
        assert_eq!(
            member(&published, "LensVersion"),
            row.version.as_str(),
            "case {case:?}: the publish must answer the version it was asked for, got \
             {published:?}"
        );

        // Rule 2, after the publish: the lens is reported at that version, from its own
        // record and the published document.
        let answer = get_lens(&client, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a published lens must be retrievable, got {answer:?}"
        );
        let lens = answer
            .body
            .get("Lens")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the answer must carry a Lens, got {answer:?}"));
        for (name, expected) in [
            ("LensArn", arn.clone()),
            ("LensVersion", row.version.clone()),
            (
                "Name",
                row.document["name"].as_str().unwrap_or_default().to_string(),
            ),
            ("Owner", engine::emulator_account_id().to_string()),
            // "The ID assigned to the share invitation": the member is reported for a lens
            // the caller did not reach through an invitation too, carrying no id.
            ("ShareInvitationId", String::new()),
        ] {
            assert_eq!(
                lens.get(name).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: expected {name} {expected:?}, got {lens:?}"
            );
        }
        assert_eq!(
            lens.get("Description").and_then(Value::as_str),
            row.document.get("description").and_then(Value::as_str),
            "case {case:?}: the description comes from the published document, got {lens:?}"
        );

        // And the published version exports the same document the import carried.
        let pinned = export(&client, &arn, Some(&row.version)).await;
        assert_eq!(
            member(&pinned, "LensJSON"),
            document.as_str(),
            "case {case:?}: the published version must carry the imported document, got \
             {pinned:?}"
        );
    }
}

/// Rule 3. Republishing a version name the lens already carries is the 409
/// `ConflictException`, while a lens that is not stored at all stays the lookup's 404.
#[tokio::test]
async fn republishing_a_version_name_conflicts_while_an_unknown_lens_is_not_found() {
    let client = Client::new();

    for row in rows() {
        let case = row.case;
        let document = row.document.to_string();
        let arn = imported_arn(
            &import(&client, &document, None, &format!("conflict-{case}"), None).await,
            case,
        );
        let published = create_version(
            &client,
            &arn,
            &row.version,
            Some(true),
            &format!("first-{case}"),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the first publish must succeed, got {published:?}"
        );

        // A second publish under that name, under a token of its own so the refusal is
        // about the version name and not about an idempotency replay.
        let again = create_version(
            &client,
            &arn,
            &row.version,
            Some(true),
            &format!("second-{case}"),
        )
        .await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: a version name already published must conflict, got {again:?}"
        );
        assert_eq!(
            again.body.get("ResourceType").and_then(Value::as_str),
            Some(state::LENS_RESOURCE_TYPE),
            "case {case:?}: the conflict must name the LENS resource type, got {again:?}"
        );
        assert_eq!(
            again.body.get("ResourceId").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the conflict must name the lens it collided with, got {again:?}"
        );
        assert_eq!(
            again.body.get("Message").and_then(Value::as_str),
            Some(
                format!(
                    "[Conflict] A lens {arn} with version name {} already exists.",
                    row.version
                )
                .as_str()
            ),
            "case {case:?}: the conflict must name the lens and the version, got {again:?}"
        );

        // A name the lens does NOT carry publishes, so the conflict was about that one
        // version name rather than about the lens. The name is well within `LensVersion`'s
        // 32-character ceiling, which rule 1's rows already span.
        let other = create_version(
            &client,
            &arn,
            "0.1",
            None,
            &format!("next-{case}"),
        )
        .await;
        assert_eq!(
            other.status, 200,
            "case {case:?}: another version name must still publish, got {other:?}"
        );
    }

    // A lens nothing is stored under is the lookup's 404 for the very same request: the
    // version name is judged only once the lens has resolved.
    for (case, alias) in [
        ("unknown-custom-arn", ABSENT_LENS_ARN.to_string()),
        (
            "another-account",
            ABSENT_LENS_ARN.replacen(
                &format!(":{}:", engine::emulator_account_id()),
                ":210987654321:",
                1,
            ),
        ),
    ] {
        let answer = create_version(&client, &alias, "1.0", Some(true), &format!("miss-{case}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a lens nothing is stored under must not be found, got {answer:?}"
        );
    }
}
