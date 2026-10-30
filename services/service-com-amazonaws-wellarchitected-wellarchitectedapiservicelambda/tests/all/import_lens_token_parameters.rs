// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the parameters `ImportLens` compares a retry under an
//! already-spent `ClientRequestToken` against.
//!
//! "A unique case-sensitive string used to ensure that this request is idempotent
//! (executes only once) … If you retry a request with the same client request token and
//! the same parameters after the original request has completed successfully, the result
//! of the original request is returned" (`smithy_query(query="doc", shape="ImportLensInput",
//! member="ClientRequestToken")`).
//!
//! The ONE rule pinned here, over a TABLE of retries rather than one request: a retry is
//! the SAME request exactly when every parameter it carries NAMES what the landed request
//! named — and the lens counts as the lens it names rather than as the spelling it was
//! named by, because "for custom lenses, this is the lens ARN… Each lens is identified by
//! its `LensSummary$LensAlias`" (`smithy_query(query="doc", shape="ImportLensInput",
//! member="LensAlias")`) and this crate resolves that ARN with a version pinned after it as
//! naming the same lens. So a retry that spells the lens the other way replays the original
//! import, while a retry that asks for something else — another document, another tag map,
//! another lens, or no ARN at all, which asks for a NEW lens ("to update an existing custom
//! lens, specify its ARN as the `LensAlias`. If no ARN is specified, a new custom lens is
//! created", the `ImportLens` doc) — is another request and the modelled 409.
//!
//! `super::lens_token_alias_spelling` pins the SPELLING half of that rule across every
//! lens-keyed idempotent operation of this service; what is pinned here is the whole
//! parameter set of ONE of them, from both sides — so taking two spellings as one lens
//! cannot flatten the rest of the request, and cannot blur the create the absence of an ARN
//! asks for into the update an ARN asks for.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import, export,
//! publish and tag-read wire helpers are the ones `super::custom_lens_import`,
//! `super::custom_lens_versions` and `super::resource_tags` already spell — one spelling of
//! each request for every suite.

use serde_json::{json, Value};

use super::custom_lens_import::{
    export, import, imported_arn, lens_document, member, wait_for_the_import_to_land,
};
use super::custom_lens_versions::create_version;
use super::resource_tags::list_tags;
use super::workload_harness::{stored_lens_count, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The version the lens under test publishes before the update, so a row can pin a REAL
/// published version after that lens's ARN.
const PUBLISHED_VERSION: &str = "1.0";

/// The statuses an accepted import answers. "The status of the imported lens" is an
/// `ImportLensStatus`, whose members are `COMPLETE`, `ERROR` and `IN_PROGRESS`
/// (`smithy_query(query="shape", shape="ImportLensStatus")`): an accepted import is in
/// flight until the service's own import span has passed and then complete, and is never
/// `ERROR`. A replay's status is asserted against BOTH only because that span can elapse
/// between the landed import and its retry on a loaded machine; the import the retry replays
/// is the same one either way.
const ACCEPTED_STATUSES: [&str; 2] = ["IN_PROGRESS", "COMPLETE"];

/// How a row's retry names the lens, relative to the lens the landed update imported over.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// The same lens, by the ARN the import answered.
    SameLensPlainArn,
    /// The same lens, by that ARN with the published version pinned after it — the other
    /// ARN spelling this crate's lens identity resolves to that one lens.
    SameLensVersionPinnedArn,
    /// The same lens's bare id, which is NO ARN: "if no ARN is specified, a new custom lens
    /// is created", so this asks for a new lens rather than for that one.
    SameLensBareId,
    /// Another stored custom lens, by its own ARN.
    AnotherLens,
    /// No `LensAlias` member at all — the create its absence asks for.
    NoLens,
}

/// One retry under the landed update's token: what it changes about that request's
/// parameters, and whether that leaves it the SAME request.
struct Retry {
    case: &'static str,
    names: Names,
    /// Whether the retry carries the document the landed update imported.
    same_document: bool,
    /// The `Tags` the retry carries, where the landed update carried none.
    tags: Option<Value>,
    /// Whether the retry is the same request, so the original import's result is replayed.
    replays: bool,
}

/// The table: the rows differ along WHICH parameter the retry changes (the lens's spelling,
/// the lens itself, the presence of a `LensAlias` at all, the document, the tags), and so
/// along both outcomes the token's rule has.
fn retries() -> Vec<Retry> {
    vec![
        Retry {
            case: "identical",
            names: Names::SameLensPlainArn,
            same_document: true,
            tags: None,
            replays: true,
        },
        Retry {
            case: "version-pinned-arn",
            names: Names::SameLensVersionPinnedArn,
            same_document: true,
            tags: None,
            replays: true,
        },
        Retry {
            case: "bare-lens-id",
            names: Names::SameLensBareId,
            same_document: true,
            tags: None,
            replays: false,
        },
        Retry {
            case: "no-lens-alias",
            names: Names::NoLens,
            same_document: true,
            tags: None,
            replays: false,
        },
        Retry {
            case: "another-lens",
            names: Names::AnotherLens,
            same_document: true,
            tags: None,
            replays: false,
        },
        Retry {
            case: "version-pinned-arn-other-document",
            names: Names::SameLensVersionPinnedArn,
            same_document: false,
            tags: None,
            replays: false,
        },
        Retry {
            case: "version-pinned-arn-added-tags",
            names: Names::SameLensVersionPinnedArn,
            same_document: true,
            tags: Some(json!({ "team": "platform" })),
            replays: false,
        },
    ]
}

/// The `Tags` the lens carries, read back through `ListTagsForResource` — the read that
/// tells whether a request associated any ("Tags to associate to a lens", the `ImportLens`
/// doc). `Value::Null` for a lens carrying none.
async fn stored_tags(client: &Client, lens_arn: &str) -> Value {
    let answer = list_tags(client, lens_arn).await;
    assert_eq!(
        answer.status, 200,
        "a stored lens's tags must be readable, got {answer:?}"
    );
    answer.body.get("Tags").cloned().unwrap_or(Value::Null)
}

/// A fresh custom lens holding `document`, by the ARN the import answered. The caller waits
/// for the import to land itself, so a table imports every row's lenses before waiting once.
async fn imported(client: &Client, case: &str, document: &str) -> String {
    let answer = import(client, document, None, &format!("seed-{case}"), None).await;
    imported_arn(&answer, case)
}

/// The modelled 409 a retry that asks for something else answers, as the service words it.
fn assert_conflict(answer: &Answer, case: &str, token: &str, lens_id: &str) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: the retry asks for something the landed request did not, so it must \
         conflict, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        "LENS",
        "case {case:?}: expected ResourceType LENS, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        lens_id,
        "case {case:?}: the conflict must name the lens the token already imported, got \
         {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[Conflict] A request with ClientRequestToken {token} was already submitted with \
             different parameters."
        ),
        "case {case:?}: the conflict must name the token, got {answer:?}"
    );
}

/// The rule over an UPDATE: however the landed update spelled the lens it imported over, a
/// retry under its token replays it exactly when every parameter still names the same thing
/// — and otherwise is the modelled 409 that imports nothing, over no lens.
#[tokio::test]
async fn an_update_retry_replays_exactly_when_its_parameters_name_the_same_import() {
    let retries = retries();
    let client = Client::new();

    // Each row gets its own lens to update and its own bystander lens (the one a row's
    // retry names instead), and every import is issued before the single wait, so a table
    // of any size waits the service's import span once.
    let mut fixtures: Vec<(String, String, String)> = Vec::new();
    for retry in &retries {
        let case = retry.case;
        let updated = lens_document(&format!("Token-{case}-lens"), None, &[("governance", 1, 2)])
            .to_string();
        let bystander =
            lens_document(&format!("Bystander-{case}-lens"), None, &[("cost", 1, 2)]).to_string();
        let updated_arn = imported(&client, case, &updated).await;
        let bystander_arn = imported(&client, &format!("{case}-bystander"), &bystander).await;
        fixtures.push((updated_arn, bystander_arn, bystander));
    }
    wait_for_the_import_to_land();

    // The lens under test carries a published version, so a row can pin that version after
    // its ARN the way the `LensAlias` identity rule allows.
    for (retry, (updated_arn, _, _)) in retries.iter().zip(&fixtures) {
        let case = retry.case;
        let published = create_version(
            &client,
            updated_arn,
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

    let mut landed_documents: Vec<String> = Vec::new();
    for (retry, (updated_arn, bystander_arn, _)) in retries.iter().zip(&fixtures) {
        let case = retry.case;
        let token = format!("tok-{case}");
        let lens_id = state::lens_alias(updated_arn).to_string();
        let landed_document = lens_document(
            &format!("Updated-{case}-lens"),
            Some("the document the landed update imports"),
            &[("security", 2, 3)],
        )
        .to_string();
        let other_document =
            lens_document(&format!("Other-{case}-lens"), None, &[("sustainability", 1, 2)])
                .to_string();

        let landed = import(&client, &landed_document, Some(updated_arn), &token, None).await;
        assert_eq!(
            (landed.status, member(&landed, "LensArn")),
            (200, updated_arn.as_str()),
            "case {case:?}: the update under test must land on the lens it names, got {landed:?}"
        );
        assert_eq!(
            member(&landed, "Status"),
            "IN_PROGRESS",
            "case {case:?}: an accepted import is still in progress, got {landed:?}"
        );

        let lenses_before = stored_lens_count(&client);
        let tags_before = stored_tags(&client, updated_arn).await;

        let alias = match retry.names {
            Names::SameLensPlainArn => Some(updated_arn.clone()),
            Names::SameLensVersionPinnedArn => Some(format!("{updated_arn}/{PUBLISHED_VERSION}")),
            Names::SameLensBareId => Some(lens_id.clone()),
            Names::AnotherLens => Some(bystander_arn.clone()),
            Names::NoLens => None,
        };
        let document = if retry.same_document {
            landed_document.as_str()
        } else {
            other_document.as_str()
        };
        let answer = import(
            &client,
            document,
            alias.as_deref(),
            &token,
            retry.tags.clone(),
        )
        .await;

        if retry.replays {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (200, ""),
                "case {case:?}: every parameter still names what the landed update named, so \
                 the retry must replay it, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "LensArn"),
                updated_arn.as_str(),
                "case {case:?}: the replay must answer the lens the landed update imported \
                 over, got {answer:?}"
            );
            let status = member(&answer, "Status");
            assert!(
                ACCEPTED_STATUSES.contains(&status),
                "case {case:?}: the replay must report the import's own status, one of \
                 {ACCEPTED_STATUSES:?}, got {answer:?}"
            );
        } else {
            assert_conflict(&answer, case, &token, &lens_id);
        }

        assert_eq!(
            stored_lens_count(&client),
            lenses_before,
            "case {case:?}: neither a replay nor a refusal imports a second lens"
        );
        let tags_after = stored_tags(&client, updated_arn).await;
        assert_eq!(
            tags_after, tags_before,
            "case {case:?}: neither a replay nor a refusal associates tags, got {tags_after:?}"
        );
        landed_documents.push(landed_document);
    }

    // What each lens holds once the imports have landed: the document its landed update
    // imported, and — for the lens a refused retry named instead — the document of its own
    // import, untouched.
    wait_for_the_import_to_land();
    for ((retry, (updated_arn, bystander_arn, bystander_document)), landed_document) in
        retries.iter().zip(&fixtures).zip(&landed_documents)
    {
        let case = retry.case;
        let updated = export(&client, updated_arn, None).await;
        assert_eq!(
            member(&updated, "LensJSON"),
            landed_document.as_str(),
            "case {case:?}: the lens must still hold the document the landed update imported, \
             got {updated:?}"
        );
        let bystander = export(&client, bystander_arn, None).await;
        assert_eq!(
            member(&bystander, "LensJSON"),
            bystander_document.as_str(),
            "case {case:?}: a retry naming another lens must import nothing over it, got \
             {bystander:?}"
        );
    }
}

/// The same rule from the CREATE side. "If no ARN is specified, a new custom lens is
/// created", so every retry that specifies NO ARN asks for what the landed create asked for
/// and replays its lens, while a retry that specifies an ARN asks to update that lens
/// instead and is another request — whatever version it pins after it.
#[tokio::test]
async fn a_create_retry_replays_for_every_retry_that_specifies_no_arn() {
    let client = Client::new();
    let document =
        lens_document("Create-token-lens", None, &[("governance", 1, 2)]).to_string();
    let token = "tok-create-replay";

    let created = import(&client, &document, None, token, None).await;
    let arn = imported_arn(&created, "create");
    let lens_id = state::lens_alias(&arn).to_string();

    // (case, the `LensAlias` the retry carries, whether that leaves it the same request)
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("no-lens-alias", None, true),
        ("bare-lens-id", Some(lens_id.clone()), true),
        ("plain-arn", Some(arn.clone()), false),
        (
            "version-pinned-arn",
            Some(format!("{arn}/{PUBLISHED_VERSION}")),
            false,
        ),
    ];

    for (case, alias, replays) in rows {
        let answer = import(&client, &document, alias.as_deref(), token, None).await;
        if replays {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (200, ""),
                "case {case:?}: a retry specifying no ARN asks for the create the token \
                 already made, so it must replay it, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "LensArn"),
                arn.as_str(),
                "case {case:?}: the replay must answer the lens the create minted, got \
                 {answer:?}"
            );
        } else {
            assert_conflict(&answer, case, token, &lens_id);
        }
        assert_eq!(
            stored_lens_count(&client),
            1,
            "case {case:?}: the token minted ONE lens, so no retry under it may mint another"
        );
    }

    // And the lens the create minted still holds the document it was created with.
    wait_for_the_import_to_land();
    let exported = export(&client, &arn, None).await;
    assert_eq!(
        member(&exported, "LensJSON"),
        document.as_str(),
        "the created lens must still hold the document the create imported, got {exported:?}"
    );
}
