// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the lens versions `CreateLensVersion` publishes, and the
//! read that proves what it recorded, `ExportLens`.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. `CreateLensVersion` PUBLISHES the lens's draft content under the `LensVersion` the
//!    request names, answers that version and the lens's ARN, and makes the content
//!    readable by `ExportLens?LensVersion=…`. Rows differ along the version NAME (its
//!    form and its length, up to the modelled bound), whether `IsMajorVersion` is sent
//!    and what it says, and the shape of the document being published.
//! 2. A published version is IMMUTABLE: a later import that replaces the lens's draft
//!    does not change what the published version exports, and the name of a published
//!    version cannot be reused — that is the modelled `ConflictException`. Rows differ
//!    along the shape of the replacement document.
//! 3. A lens that cannot carry a version refuses the publish and records nothing: an
//!    Amazon Web Services official lens is a 400 and a `LensAlias` naming no stored
//!    custom lens is a 404. The modelled `LensVersion` length bounds are refused at the
//!    boundary.
//! 4. The publish is idempotent per `ClientRequestToken`: the same token with the same
//!    parameters reports the version the original request published, and with different
//!    parameters is a 409 that publishes nothing.
//! 5. "A lens can have up to 100 versions" (the `CreateLensVersion` doc): a publish past
//!    that quota is the modelled `ServiceQuotaExceededException`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`; the import
//! and export wire helpers are the ones `super::custom_lens_import` already spells.

use std::time::SystemTime;

use serde_json::{json, Value};

use super::custom_lens_import::{
    export, import, imported_arn, lens_document, member, wait_for_the_import_to_land,
    ABSENT_LENS_ARN,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// `POST /lenses/{LensAlias}/versions`, the wire form of `CreateLensVersion`. The alias
/// is percent-encoded because a custom lens is named by its ARN, which carries `:` and
/// `/` inside ONE path segment.
pub(crate) async fn create_version(
    client: &Client,
    alias: &str,
    version: &str,
    is_major: Option<bool>,
    token: &str,
) -> Answer {
    let mut body = json!({ "LensVersion": version, "ClientRequestToken": token });
    if let Some(is_major) = is_major {
        body["IsMajorVersion"] = json!(is_major);
    }
    client
        .send(
            "POST",
            &format!("/lenses/{}/versions", encode_path_segment(alias)),
            Some(body),
        )
        .await
}

/// Import `document` as a fresh custom lens and answer its ARN.
async fn import_lens(client: &Client, case: &str, document: &str) -> String {
    let answer = import(client, document, None, &format!("import-{case}"), None).await;
    imported_arn(&answer, case)
}

/// The versions a stored lens holds, by name, straight from the shared state — the lens
/// record is where a published version lives, and nothing over the wire enumerates every
/// version of a lens yet (`GetLens` reports ONE version of it, and `ListLensVersions` is
/// not an operation of this service).
fn stored_versions(client: &Client, alias: &str) -> Vec<String> {
    let store = client.store();
    let store = store.lock();
    store
        .lens_by_alias(alias)
        .map(|record| record.versions.keys().cloned().collect())
        .unwrap_or_default()
}

/// Rule 1. Every form of version name publishes the lens's draft content under that
/// name, and `ExportLens` answers the published document back for it.
#[tokio::test]
async fn publishing_a_version_records_the_draft_and_exports_it_back() {
    // `LensVersion` is `@length(min: 1, max: 32)`
    // (`smithy_query(query="operation", operation="CreateLensVersion")`), so the rows
    // span a one-character name and a name AT the 32-character ceiling, beside the
    // semantic and dated forms a caller uses. `IsMajorVersion` is sent true, sent false
    // and omitted.
    let max_length_version = "9".repeat(32);
    let cases: Vec<(&str, String, Option<bool>, Value)> = vec![
        (
            "single-character",
            "1".to_string(),
            Some(true),
            lens_document("Single char lens", None, &[("governance", 1, 2)]),
        ),
        (
            "semantic-minor",
            "1.0.1".to_string(),
            Some(false),
            lens_document(
                "Semantic lens",
                Some("published from a described draft"),
                &[("security", 2, 3), ("cost", 1, 2)],
            ),
        ),
        (
            "dated",
            "2024-01-01".to_string(),
            None,
            lens_document("Dated lens", None, &[("reliability", 12, 2)]),
        ),
        (
            "at-the-length-ceiling",
            max_length_version.clone(),
            Some(true),
            lens_document("Long version lens", None, &[("durabilité", 1, 4)]),
        ),
    ];

    let client = Client::new();
    let mut published: Vec<(&str, String, String, Option<bool>, String)> = Vec::new();
    for (case, version, is_major, document) in &cases {
        let document = document.to_string();
        let arn = import_lens(&client, case, &document).await;
        published.push((case, arn, version.clone(), *is_major, document));
    }
    wait_for_the_import_to_land();

    for (case, arn, version, is_major, document) in &published {
        let answer =
            create_version(&client, arn, version, *is_major, &format!("pub-{case}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateLensVersion 200, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "LensArn"),
            arn.as_str(),
            "case {case:?}: the publish must name the lens it published, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "LensVersion"),
            version.as_str(),
            "case {case:?}: the publish must answer the version it published, got {answer:?}"
        );

        // The round trip: the published version is readable, and it carries the document
        // the draft held.
        let exported = export(&client, arn, Some(version)).await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: a published version must export, got {exported:?}"
        );
        assert_eq!(
            member(&exported, "LensJSON"),
            document.as_str(),
            "case {case:?}: the published version must carry the draft document, got {exported:?}"
        );

        // Only the version that was published exists: no other name resolves.
        let absent = export(&client, arn, Some("0.0-never-published")).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: an unpublished version must not resolve, got {absent:?}"
        );
        assert_eq!(
            absent.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {absent:?}"
        );

        assert_eq!(
            stored_versions(&client, arn),
            vec![version.clone()],
            "case {case:?}: the lens must hold exactly the published version"
        );

        // `IsMajorVersion` is recorded as the request stated it, and an omitted member
        // is not a major version.
        let store = client.store();
        let store = store.lock();
        let record = store
            .lens_by_alias(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the published lens must be stored"));
        let published = record
            .versions
            .get(version.as_str())
            .unwrap_or_else(|| panic!("case {case:?}: the published version must be stored"));
        assert_eq!(
            published.is_major_version,
            is_major.unwrap_or(false),
            "case {case:?}: IsMajorVersion must be recorded as sent, got {published:?}"
        );
    }
}

/// Rule 1, continued: a lens carries every version published from it, and each exports
/// the content its own publish captured. The rows are the number of versions published
/// in a row, each from a different draft.
#[tokio::test]
async fn each_published_version_keeps_the_draft_it_was_published_from() {
    let cases: Vec<(&str, usize)> = vec![("two-versions", 2), ("five-versions", 5)];

    for (case, count) in cases {
        let client = Client::new();
        let first = lens_document("Versioned lens", None, &[("governance", 1, 2)]).to_string();
        let arn = import_lens(&client, case, &first).await;
        wait_for_the_import_to_land();

        let mut documents: Vec<(String, String)> = Vec::new();
        for index in 0..count {
            // Each publish takes the draft as it stands, so the draft is replaced
            // between publishes: a lens's questions grow as its content does.
            let document = lens_document(
                &format!("Versioned lens {index}"),
                Some("one draft per version"),
                &[("governance", index + 1, 2)],
            )
            .to_string();
            let update = import(
                &client,
                &document,
                Some(&arn),
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
            let answer = create_version(
                &client,
                &arn,
                &version,
                Some(index == 0),
                &format!("pub-{case}-{index}"),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: publishing {version} must succeed, got {answer:?}"
            );
            documents.push((version, document));
        }

        for (version, document) in &documents {
            let exported = export(&client, &arn, Some(version)).await;
            assert_eq!(
                member(&exported, "LensJSON"),
                document.as_str(),
                "case {case:?}: version {version} must export the draft it captured, got \
                 {exported:?}"
            );
        }
        let held = stored_versions(&client, &arn);
        let expected: Vec<String> = documents
            .iter()
            .map(|(version, _)| version.clone())
            .collect();
        assert_eq!(
            held, expected,
            "case {case:?}: the lens must hold every published version"
        );
    }
}

/// Rule 2. A later import replaces the lens's DRAFT and leaves a published version
/// untouched, and the name of a published version cannot be reused. Rows differ along
/// the shape of the replacement document.
#[tokio::test]
async fn a_published_version_is_immutable_and_its_name_cannot_be_reused() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "same-shape",
            lens_document("Replaced lens", None, &[("governance", 1, 2)]),
        ),
        (
            "more-pillars",
            lens_document(
                "Grown lens",
                Some("now with more pillars"),
                &[("governance", 1, 2), ("security", 3, 3)],
            ),
        ),
        (
            "fewer-questions",
            lens_document("Shrunk lens", None, &[("governance", 1, 1)]),
        ),
    ];

    for (case, replacement) in cases {
        let client = Client::new();
        let original = lens_document(
            "Original lens",
            Some("the content version 1.0 captures"),
            &[("governance", 2, 2)],
        )
        .to_string();
        let arn = import_lens(&client, case, &original).await;
        wait_for_the_import_to_land();

        let published =
            create_version(&client, &arn, "1.0", Some(true), &format!("pub-{case}")).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: the first publish must succeed, got {published:?}"
        );

        let replacement = replacement.to_string();
        let update = import(
            &client,
            &replacement,
            Some(&arn),
            &format!("reimport-{case}"),
            None,
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: the update import must be accepted, got {update:?}"
        );
        wait_for_the_import_to_land();

        let versioned = export(&client, &arn, Some("1.0")).await;
        assert_eq!(
            member(&versioned, "LensJSON"),
            original.as_str(),
            "case {case:?}: a published version must not follow the draft, got {versioned:?}"
        );
        let current = export(&client, &arn, None).await;
        assert_eq!(
            member(&current, "LensJSON"),
            replacement.as_str(),
            "case {case:?}: the lens's current content is the new draft, got {current:?}"
        );

        // Republishing the SAME version name collides with the version already published.
        let reused = create_version(&client, &arn, "1.0", None, &format!("reuse-{case}")).await;
        assert_eq!(
            reused.status, 409,
            "case {case:?}: a published version name cannot be reused, got {reused:?}"
        );
        assert_eq!(
            reused.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {reused:?}"
        );
        assert_eq!(
            member(&reused, "ResourceType"),
            "LENS",
            "case {case:?}: expected ResourceType LENS, got {reused:?}"
        );
        let unchanged = export(&client, &arn, Some("1.0")).await;
        assert_eq!(
            unchanged.body, versioned.body,
            "case {case:?}: the refused publish must not change version 1.0, got {unchanged:?}"
        );

        // A NEW name publishes the draft as it now stands.
        let next = create_version(&client, &arn, "2.0", Some(false), &format!("next-{case}")).await;
        assert_eq!(
            next.status, 200,
            "case {case:?}: a new version name must publish, got {next:?}"
        );
        let second = export(&client, &arn, Some("2.0")).await;
        assert_eq!(
            member(&second, "LensJSON"),
            replacement.as_str(),
            "case {case:?}: version 2.0 must carry the current draft, got {second:?}"
        );
        assert_eq!(
            stored_versions(&client, &arn),
            vec!["1.0".to_string(), "2.0".to_string()],
            "case {case:?}: both versions must be held"
        );
    }
}

/// Rule 3. A lens that cannot carry a version refuses the publish. Rows differ along HOW
/// the lens is named and why it cannot be published.
#[tokio::test]
async fn publishing_is_refused_for_a_lens_that_cannot_carry_a_version() {
    let client = Client::new();
    let document = lens_document("Refusal lens", None, &[("governance", 1, 2)]).to_string();

    // A lens this account really holds, which every refusal below must leave alone.
    let stored = import_lens(&client, "stored", &document).await;
    // A custom ARN that agrees on the lens id and disagrees on the owning account names
    // a different lens, so it resolves to nothing.
    let other_account = stored.replacen(
        &format!(":{}:", engine::emulator_account_id()),
        ":210987654321:",
        1,
    );

    // (case, alias, expected status, expected error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "official-bare-alias",
            state::FRAMEWORK_LENS_ALIAS.to_string(),
            400,
            "ValidationException",
        ),
        (
            "official-arn",
            "arn:aws:wellarchitected:us-west-2::lens/serverless".to_string(),
            400,
            "ValidationException",
        ),
        (
            "unknown-custom-arn",
            ABSENT_LENS_ARN.to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another-account",
            other_account,
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, alias, status, error_code) in &cases {
        let answer =
            create_version(&client, alias, "1.0", Some(true), &format!("pub-{case}")).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, *error_code,
            "case {case:?}: expected {error_code}, got {answer:?}"
        );
    }

    // The refused publishes recorded nothing: the lens this account holds still has no
    // version, and a request naming IT publishes.
    assert!(
        stored_versions(&client, &stored).is_empty(),
        "a refused publish must record no version, got {:?}",
        stored_versions(&client, &stored)
    );
    let accepted = create_version(&client, &stored, "1.0", Some(true), "pub-stored").await;
    assert_eq!(
        accepted.status, 200,
        "the lens this account holds must publish, got {accepted:?}"
    );

    // The modelled `LensVersion` bounds — `@length(min: 1, max: 32)` — are refused at the
    // boundary, before the handler: one character past the ceiling, and the empty name.
    let too_long = "9".repeat(33);
    for (case, version) in [("one-past-the-ceiling", too_long.as_str()), ("empty", "")] {
        let answer = create_version(&client, &stored, version, None, &format!("len-{case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a LensVersion outside 1..32 must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// Rule 4. The same `ClientRequestToken` with the same parameters reports the version the
/// original request published; with different parameters it is a 409 that publishes
/// nothing. Rows differ along WHICH parameter the second request changes.
#[tokio::test]
async fn a_publish_is_idempotent_per_client_request_token() {
    // (case, version, IsMajorVersion, whether the second request is the same request)
    let cases: Vec<(&str, &str, Option<bool>, bool)> = vec![
        ("same-parameters", "1.0", Some(true), true),
        ("different-version", "2.0", Some(true), false),
        ("different-is-major", "1.0", Some(false), false),
        ("is-major-omitted", "1.0", None, false),
    ];

    for (case, version, is_major, replay) in cases {
        let client = Client::new();
        let document = lens_document("Token lens", None, &[("governance", 1, 2)]).to_string();
        let arn = import_lens(&client, case, &document).await;
        wait_for_the_import_to_land();

        let first = create_version(&client, &arn, "1.0", Some(true), "tok-publish").await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first publish must succeed, got {first:?}"
        );

        let second = create_version(&client, &arn, version, is_major, "tok-publish").await;
        if replay {
            assert_eq!(
                second.status, 200,
                "case {case:?}: a replay must succeed, got {second:?}"
            );
            assert_eq!(
                second.body, first.body,
                "case {case:?}: a replay must report the original publish, got {second:?}"
            );
        } else {
            assert_eq!(
                second.status, 409,
                "case {case:?}: a token reused with other parameters must be refused, got \
                 {second:?}"
            );
            assert_eq!(
                second.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {second:?}"
            );
        }

        // Either way exactly ONE version was published under that token.
        assert_eq!(
            stored_versions(&client, &arn),
            vec!["1.0".to_string()],
            "case {case:?}: one token publishes one version"
        );
    }
}

/// Rule 5. "A lens can have up to 100 versions" (the `CreateLensVersion` doc): the
/// publish that would be the 101st is the modelled `ServiceQuotaExceededException`, and
/// the one that fills the quota still succeeds.
#[tokio::test]
async fn a_lens_holds_at_most_the_published_version_quota() {
    let client = Client::new();
    let document = lens_document("Quota lens", None, &[("governance", 1, 2)]).to_string();
    let arn = import_lens(&client, "quota", &document).await;
    wait_for_the_import_to_land();

    // Filled through the shared state: reaching the quota over HTTP would publish a
    // hundred versions for one assertion. The ceiling is the state's, and the publish
    // under test still crosses the HTTP boundary.
    {
        let definition = state::parse_lens_json(&document).expect("the seeded document is a lens");
        let store = client.store();
        let mut store = store.lock();
        let record = store
            .lens_by_alias_mut(&arn)
            .expect("the imported lens must be stored");
        for index in 0..state::MAX_LENS_VERSIONS_PER_LENS - 1 {
            record.versions.insert(
                format!("seeded-{index:04}"),
                state::PublishedLensVersion {
                    content: state::LensContent {
                        json: document.clone(),
                        definition: definition.clone(),
                        imported_at: SystemTime::now(),
                    },
                    is_major_version: false,
                    published_at: SystemTime::now(),
                },
            );
        }
    }

    let last = create_version(&client, &arn, "the-hundredth", Some(true), "tok-last").await;
    assert_eq!(
        last.status, 200,
        "the publish that fills the quota must succeed, got {last:?}"
    );
    assert_eq!(
        stored_versions(&client, &arn).len(),
        state::MAX_LENS_VERSIONS_PER_LENS,
        "the lens must hold exactly the quota"
    );

    let past =
        create_version(&client, &arn, "the-hundred-and-first", Some(true), "tok-past").await;
    assert_eq!(
        past.status, 402,
        "a publish past the quota must be refused, got {past:?}"
    );
    assert_eq!(
        past.error_code, "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {past:?}"
    );
    assert_eq!(
        member(&past, "ResourceType"),
        "LENS",
        "expected ResourceType LENS, got {past:?}"
    );
    assert!(
        !member(&past, "QuotaCode").is_empty() && !member(&past, "ServiceCode").is_empty(),
        "the modelled QuotaCode and ServiceCode must be reported, got {past:?}"
    );
    assert_eq!(
        stored_versions(&client, &arn).len(),
        state::MAX_LENS_VERSIONS_PER_LENS,
        "a refused publish must record no version"
    );
}
