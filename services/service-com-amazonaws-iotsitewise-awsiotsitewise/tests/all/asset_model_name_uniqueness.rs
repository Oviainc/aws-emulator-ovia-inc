// SPDX-License-Identifier: CC0-1.0

//! One asset-model name is held by at most one asset model.
//!
//! The rule these suites pin: a name member this service documents as a unique
//! name for its resource is part of that resource's identity in the account, so a
//! create that takes a name another resource of the same kind already holds — and
//! a rename onto such a name — is refused with the modelled
//! `ResourceAlreadyExistsException` (HTTP 409) naming the holder, and stores
//! nothing. `assetModelName` is one of those members
//! (`smithy_query(query="doc", shape="CreateAssetModelRequest",
//! member="assetModelName")`: "A unique name for the asset model."), beside
//! `gatewayName`, whose refusal `gateway_lifecycle` pins; `assetName` and
//! `portalName` are documented "A friendly name", so they carry no such rule.
//!
//! The rows span what the rule generalises over: the VALUE and SIZE of the name
//! (both published length bounds of the `Name` shape, punctuation, non-ASCII), and
//! what else the colliding request carries (another description, other properties,
//! another model type, tags, an external id, a caller-supplied id) — the name
//! alone is the conflict. The wire harness lives in `super::asset_lifecycle`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};

/// Enough of a page for every model a row of these suites creates.
const MODELS: &str = "/asset-models?maxResults=250";

/// `POST /asset-models` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/asset-models", Some(body)).await
}

/// `PUT /asset-models/{model_id}` renaming the model to `name`.
async fn rename(router: &MultiServiceRouter, model_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(json!({ "assetModelName": name })),
    )
    .await
}

/// The `assetModelName` `DescribeAssetModel` reports for `model_id`.
async fn described_name(router: &MultiServiceRouter, model_id: &str) -> String {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel {model_id}: got {status} {body}"
    );
    body["assetModelName"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeAssetModel {model_id}: no assetModelName in {body}"))
        .to_string()
}

/// How many asset models the account holds.
async fn model_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", MODELS, None).await;
    assert_eq!(status, 200, "ListAssetModels: got {status} {body}");
    body["assetModelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListAssetModels: no assetModelSummaries in {body}"))
        .len()
}

/// Create the model holding `name`, answering `(id, arn)`.
async fn hold_name(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, created) = create(router, json!({ "assetModelName": name })).await;
    assert_eq!(
        status, 202,
        "holding the name {name:?}: expected 202, got {status} {created}"
    );
    (
        created["assetModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("holding {name:?}: no assetModelId in {created}"))
            .to_string(),
        created["assetModelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("holding {name:?}: no assetModelArn in {created}"))
            .to_string(),
    )
}

/// Assert that `answered` is the modelled 409 naming the model `(id, arn)`.
fn assert_name_conflict(case: &str, status: u16, answered: &Value, id: &str, arn: &str) {
    assert_eq!(
        status, 409,
        "case {case:?}: a name already held expected 409, got {status} {answered}"
    );
    assert_eq!(
        error_code(answered),
        "ResourceAlreadyExistsException",
        "case {case:?}: unexpected error code in {answered}"
    );
    assert_eq!(
        answered["resourceId"].as_str().unwrap_or(""),
        id,
        "case {case:?}: the refusal must name the model holding the name, got {answered}"
    );
    assert_eq!(
        answered["resourceArn"].as_str().unwrap_or(""),
        arn,
        "case {case:?}: the refusal must carry that model's own ARN, got {answered}"
    );
    assert_eq!(
        answered["message"].as_str().unwrap_or(""),
        "Another asset model is already using the same name",
        "case {case:?}: the refusal must name the collided dimension, got {answered}"
    );
}

/// The `Name` shape bounds the member at 1..256 characters
/// (`smithy_query(query="doc", shape="CreateAssetModelRequest",
/// member="assetModelName")` reports `len 1..256, pattern
/// ^[^\u0000-\u001F\u007F]+$`), so the table below carries both bounds as well as
/// values inside them.
fn name_rows() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "Boiler".to_string()),
        ("the shortest name the shape admits", "a".to_string()),
        ("the longest name the shape admits", "L".repeat(256)),
        ("spaces and punctuation", "Line 3 — boiler.unit:2_B".to_string()),
        ("non-ASCII characters", "Kesselanlage-Ünit-Ⅲ-蒸気".to_string()),
    ]
}

#[tokio::test]
async fn a_name_is_held_by_at_most_one_asset_model() {
    let router = router();

    for (case, name) in name_rows() {
        let (holder_id, holder_arn) = hold_name(&router, &name).await;
        let before = model_count(&router).await;

        let (status, answered) = create(&router, json!({ "assetModelName": name })).await;
        assert_name_conflict(case, status, &answered, &holder_id, &holder_arn);
        let after = model_count(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must store nothing, {before} -> {after}"
        );
        assert_eq!(
            described_name(&router, &holder_id).await,
            name,
            "case {case:?}: the holder must keep its name"
        );

        // A name no model holds is free, whatever its shape: the refusal above is
        // about the name being taken, not about the value.
        let free = format!("{case} (a free name)");
        let (status, created) = create(&router, json!({ "assetModelName": free })).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a free name expected 202, got {status} {created}"
        );
    }
}

#[tokio::test]
async fn the_name_alone_is_the_conflict_whatever_else_the_request_carries() {
    let router = router();
    let name = "Boiler";
    let (holder_id, holder_arn) = hold_name(&router, name).await;

    // Rows differ along what the second request varies BESIDES the name: the
    // description, the property list, the model type, the tags, an external id and
    // a caller-supplied id. None of them frees the name.
    let rows: Vec<(&str, Value)> = vec![
        ("the bare required member", json!({})),
        (
            "another description",
            json!({ "assetModelDescription": "a boiler of its own" }),
        ),
        (
            "other properties",
            json!({
                "assetModelProperties": [
                    {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
            }),
        ),
        (
            "another model type",
            json!({ "assetModelType": "COMPONENT_MODEL" }),
        ),
        (
            "an interface rather than an asset model",
            json!({ "assetModelType": "INTERFACE" }),
        ),
        ("tags", json!({ "tags": {"line": "A"} })),
        (
            "a free external id",
            json!({ "assetModelExternalId": "boiler.second" }),
        ),
        (
            "a free caller-supplied id",
            json!({ "assetModelId": "7c9e6679-7425-40de-944b-e07fc1f90ae7" }),
        ),
    ];

    let before = model_count(&router).await;
    for (case, extra) in &rows {
        let mut body = json!({ "assetModelName": name });
        for (member, value) in extra.as_object().expect("a member map") {
            body[member.as_str()] = value.clone();
        }

        let (status, answered) = create(&router, body.clone()).await;
        assert_name_conflict(case, status, &answered, &holder_id, &holder_arn);
        let after = model_count(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must store nothing, {before} -> {after}"
        );
    }

    // Nothing the refused requests carried was consumed: the external id and the
    // id they named are still free for a create under a name of its own.
    let (status, created) = create(
        &router,
        json!({
            "assetModelName": "Boiler of its own",
            "assetModelExternalId": "boiler.second",
            "assetModelId": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "the members of a refused create must stay free, got {status} {created}"
    );
    assert_eq!(
        created["assetModelId"].as_str(),
        Some("7c9e6679-7425-40de-944b-e07fc1f90ae7"),
        "the create must mint the id it asked for, got {created}"
    );
}

#[tokio::test]
async fn a_rename_may_not_take_a_name_another_asset_model_holds() {
    let router = router();
    let (held_id, held_arn) = hold_name(&router, "Boiler").await;
    let (moving_id, moving_arn) = hold_name(&router, "Chiller").await;

    // Rows differ along what the rename asks for: another model's name, the name
    // the model already carries, and a name nothing holds.
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        ("onto the name another model holds", "Boiler", 409, "Chiller"),
        (
            "onto the name it already carries",
            "Chiller",
            202,
            "Chiller",
        ),
        ("onto a free name", "Condenser", 202, "Condenser"),
    ];

    for (case, requested, expected, expected_name) in rows {
        let (status, answered) = rename(&router, &moving_id, requested).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );
        if expected == 409 {
            assert_name_conflict(case, status, &answered, &held_id, &held_arn);
        }
        assert_eq!(
            described_name(&router, &moving_id).await,
            expected_name,
            "case {case:?}: unexpected name on the renamed model"
        );
        assert_eq!(
            described_name(&router, &held_id).await,
            "Boiler",
            "case {case:?}: the other model must keep its own name"
        );
    }

    // The rule follows the STATE, not the name a model was created with: the name
    // the rename left behind is free, and the one it moved onto is taken.
    let (status, created) = create(&router, json!({ "assetModelName": "Chiller" })).await;
    assert_eq!(
        status, 202,
        "a name a rename left behind must be free, got {status} {created}"
    );
    let (status, answered) = create(&router, json!({ "assetModelName": "Condenser" })).await;
    assert_name_conflict(
        "the name a rename moved onto",
        status,
        &answered,
        &moving_id,
        &moving_arn,
    );
}

#[tokio::test]
async fn deleting_the_holder_frees_its_name() {
    let router = router();
    let (holder_id, holder_arn) = hold_name(&router, "Boiler").await;

    let (status, answered) = create(&router, json!({ "assetModelName": "Boiler" })).await;
    assert_name_conflict(
        "while the holder is stored",
        status,
        &answered,
        &holder_id,
        &holder_arn,
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/asset-models/{holder_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "DeleteAssetModel {holder_id}: got {status} {deleted}"
    );

    let (status, created) = create(&router, json!({ "assetModelName": "Boiler" })).await;
    assert_eq!(
        status, 202,
        "a name its holder no longer holds must be free, got {status} {created}"
    );
    assert_ne!(
        created["assetModelId"].as_str().unwrap_or(""),
        holder_id,
        "the create must mint a model of its own, got {created}"
    );
}
