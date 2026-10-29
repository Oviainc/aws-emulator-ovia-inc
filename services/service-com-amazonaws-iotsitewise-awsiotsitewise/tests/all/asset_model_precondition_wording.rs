// SPDX-License-Identifier: CC0-1.0

//! The refusal-WORDING rule for a failed asset-model precondition, pinned over
//! a SET of inputs rather than over the one request an issue happened to
//! record.
//!
//! The rule: every operation that carries the `If-Match` / `If-None-Match` /
//! `Match-For-Version-Type` conditional headers of an asset-model write states
//! a precondition that evaluated to FALSE with the sentence the service answers
//! that condition with, naming the conditional header whose precondition failed
//! — the same sentence for every operation that answers from the same wording,
//! whatever the request's own values were. Nothing the REQUEST supplied goes
//! into it: not the entity tag the caller sent, not the spelling it sent the
//! header in, not the asset model's own id.
//!
//! The service answers two such sentences, one per family of write, and this
//! suite pins both: the operations that WRITE the model's definition (the
//! composite-model create and the two updates) name the version the headers
//! targeted, and the operations that REMOVE part of it (the two deletes) name
//! no version.
//!
//! The expected sentences below are written out BY HAND from those rules, as an
//! independent oracle: this suite never calls the crate's message helpers, so a
//! handler that drifts back to a template of its own cannot agree with itself.
//! `If-Match` and `If-None-Match` are the names the model binds those members
//! to (`smithy_query(query="operation",
//! operation="CreateAssetModelCompositeModel")` reports
//! `ifMatch`/`ifNoneMatch` as `@httpHeader("If-Match")` /
//! `@httpHeader("If-None-Match")`), and `LATEST | ACTIVE` is the whole
//! `AssetModelVersionType` catalog `matchForVersionType` admits.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{error_code, router};
use super::asset_model_composite_model::{
    call_with_headers, create_composite, describe_composite, model_etag, plain_model,
    PAST_THE_OWED_TRANSITION,
};
use super::hierarchies_etags_and_refusals::{error_message, FOREIGN_E_TAG};

/// One asset-model write that carries the conditional headers: how the request
/// is addressed, the body it needs beside them, and whether it is a REMOVAL,
/// whose refusal is worded without the version.
struct Write {
    /// The operation the row drives; every assertion carries it.
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// `true` for the two deletes: the operations that remove part of the model.
    removal: bool,
}

/// The sentence the rule produces for `write`, written out by hand from the
/// rule itself.
///
/// `header` is the conditional header the model binds, `version` the
/// asset-model version the headers targeted as the writes' wording names it —
/// which the removals' wording does not carry at all.
fn expected_message(write: &Write, header: &str, version: &str) -> String {
    if write.removal {
        format!("Precondition failed for provided {header} value.")
    } else {
        format!("Provided {header} value does not match the current {version} version ETag.")
    }
}

/// The five writes that carry the conditional headers, addressed at
/// `model_id` and — for the two that name one — at `composite_id`.
fn writes(model_id: &str, composite_id: &str) -> Vec<Write> {
    vec![
        Write {
            operation: "UpdateAssetModel",
            method: "PUT",
            uri: format!("/asset-models/{model_id}"),
            body: Some(json!({ "assetModelName": "Boiler" })),
            removal: false,
        },
        Write {
            operation: "DeleteAssetModel",
            method: "DELETE",
            uri: format!("/asset-models/{model_id}"),
            body: None,
            removal: true,
        },
        Write {
            operation: "CreateAssetModelCompositeModel",
            method: "POST",
            uri: format!("/asset-models/{model_id}/composite-models"),
            body: Some(json!({
                "assetModelCompositeModelName": "Added",
                "assetModelCompositeModelType": "CUSTOM",
            })),
            removal: false,
        },
        Write {
            operation: "UpdateAssetModelCompositeModel",
            method: "PUT",
            uri: format!("/asset-models/{model_id}/composite-models/{composite_id}"),
            body: Some(json!({ "assetModelCompositeModelName": "Custom1" })),
            removal: false,
        },
        Write {
            operation: "DeleteAssetModelCompositeModel",
            method: "DELETE",
            uri: format!("/asset-models/{model_id}/composite-models/{composite_id}"),
            body: None,
            removal: true,
        },
    ]
}

/// The `AssetModelVersionType` values `matchForVersionType` admits, plus its
/// absence, each with the version the writes' wording names for it. Source:
/// `smithy_query(query="shape", shape="AssetModelVersionType")` — `LATEST |
/// ACTIVE`, with `LATEST` the documented default of an absent selector.
fn version_targets() -> Vec<(&'static str, Option<&'static str>, &'static str)> {
    vec![
        ("no selector, so the documented LATEST default", None, "latest"),
        ("Match-For-Version-Type: LATEST", Some("LATEST"), "latest"),
        ("Match-For-Version-Type: ACTIVE", Some("ACTIVE"), "active"),
    ]
}

/// Drive `write` with the conditional headers `headers` and assert the refusal
/// is the modelled 412 naming `model_id`, worded `expected`.
async fn assert_refusal(
    router: &MultiServiceRouter,
    write: &Write,
    case: &str,
    headers: &[(&str, &str)],
    model_id: &str,
    expected: &str,
) {
    let (status, answered, _) = call_with_headers(
        router,
        write.method,
        &write.uri,
        write.body.clone(),
        headers,
    )
    .await;
    let case = format!("{}, {case}", write.operation);
    assert_eq!(
        status, 412,
        "case {case:?}: a failed precondition is a 412, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "PreconditionFailedException",
        "case {case:?}: expected PreconditionFailedException, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(&answered)
    );
    assert_eq!(
        answered["resourceId"].as_str().unwrap_or(""),
        model_id,
        "case {case:?}: the 412 names the asset model it was evaluated on, got {answered}"
    );
}

#[tokio::test]
async fn every_asset_model_write_words_a_failed_precondition_by_its_family() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    assert_eq!(status, 202, "create Custom1: got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create Custom1: no id in {created}"))
        .to_string();
    // Past the transition the two creates owe, so the model has both a LATEST
    // and an ACTIVE version and every row's target version exists.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // A tag the service DID issue, for a version a later write superseded: the
    // second composite model carries the model to a new definition, so the tag
    // read before it is stale from then on.
    let stale_tag = model_etag(&router, &model_id).await;
    let (status, second) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom2",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    assert_eq!(status, 202, "create Custom2: got {status} {second}");
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let current_tag = model_etag(&router, &model_id).await;
    assert_ne!(
        stale_tag, current_tag,
        "a write to the model must carry it to a new entity tag, both read {current_tag:?}"
    );

    // The rows span every operation that carries the headers, both conditional
    // headers, every value of the version selector, and — for `If-Match` — both
    // kinds of non-matching tag a caller can send: one the service never issued
    // and one it issued for a version since superseded. The header names are
    // sent lowercase, so a message that named the header as the request spelled
    // it could not pass.
    for write in writes(&model_id, &composite_id) {
        for (selector, version_type, named) in version_targets() {
            for (tag_case, tag) in [
                ("a tag the service never issued", FOREIGN_E_TAG),
                ("the tag of a superseded version", stale_tag.as_str()),
            ] {
                let mut headers = vec![("if-match", tag)];
                if let Some(version_type) = version_type {
                    headers.push(("match-for-version-type", version_type));
                }
                assert_refusal(
                    &router,
                    &write,
                    &format!("If-Match with {tag_case}, {selector}"),
                    &headers,
                    &model_id,
                    &expected_message(&write, "If-Match", named),
                )
                .await;
            }

            // `If-None-Match: *` asks that the targeted version NOT exist, and
            // every target version of this settled model does.
            let mut headers = vec![("if-none-match", "*")];
            if let Some(version_type) = version_type {
                headers.push(("match-for-version-type", version_type));
            }
            assert_refusal(
                &router,
                &write,
                &format!("If-None-Match *, {selector}"),
                &headers,
                &model_id,
                &expected_message(&write, "If-None-Match", named),
            )
            .await;
        }
    }

    // A precondition that fails changes nothing, so the whole table above left
    // the model and its composite models exactly as they were.
    assert_eq!(
        model_etag(&router, &model_id).await,
        current_tag,
        "a refused precondition must leave the model's entity tag where it was"
    );
    let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(
        status, 200,
        "the composite model must survive every refused precondition, got {status} {described}"
    );
}

#[tokio::test]
async fn a_precondition_on_a_version_that_does_not_exist_is_worded_the_same_way() {
    let router = router();
    let model_id = plain_model(&router, "Chiller").await;
    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    assert_eq!(status, 202, "create Custom1: got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create Custom1: no id in {created}"))
        .to_string();

    // No wait here: the model has not settled, so it has no ACTIVE version at
    // all and an `If-Match` targeting that version can only evaluate to FALSE.
    // The refusal is the sentence the operation's family words it with, naming
    // the version the headers targeted where that wording carries one — a
    // target version that is missing is not a different refusal.
    for write in writes(&model_id, &composite_id) {
        let expected = expected_message(&write, "If-Match", "active");
        assert_refusal(
            &router,
            &write,
            "If-Match on the ACTIVE version of a model that has none yet",
            &[
                ("if-match", FOREIGN_E_TAG),
                ("match-for-version-type", "ACTIVE"),
            ],
            &model_id,
            &expected,
        )
        .await;
    }
}
