// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `clientToken` the mutating operations of the
//! asset slice take, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rule these suites pin: a mutating request that carries a `clientToken`
//! is served ONCE. A repeat of the SAME request under the SAME token is a RETRY
//! of that call: it is answered from what the first call left behind and
//! changes nothing further — it mints no second resource, restarts no owed
//! transition and mints no second version. A DIFFERENT request reusing a token
//! already spent is refused `ConflictingOperationException` (409) naming the
//! resource the first call acted on, and changes nothing. A request carrying no
//! token is a new call every time, and a token is scoped to the operation it
//! was sent to.
//!
//! The rows span what the rule generalises over: the operation (create, update,
//! property write, association, disassociation), the resource kind (asset model
//! and asset), the members the request carries (required only, with
//! descriptions, with tags, with a caller-supplied id or external id) and which
//! member a mismatched replay differs in (a name, a description, a tag value, a
//! chosen id, the resource addressed).
//!
//! The harness (`router`, `call`, `error_code`, `create_model`, …) is the one
//! `asset_lifecycle` defines, reused rather than copied.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::{call, create_model, error_code, router, router_with_engine};

/// Comfortably past [`ACTIVATION_DELAY`], so the transition an accepted call
/// owes has fallen due by the time the next call lands.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// A `ClientToken`-shaped value built from `seed`.
///
/// Source: the `clientToken` member every mutating operation of this model
/// declares — `smithy_query(query="operation", operation="CreateAsset")` reports
/// target `ClientToken`, `length 36..64`, `pattern ^\S{36,64}$` — so a short,
/// readable seed is padded with a non-whitespace filler up to the minimum.
fn client_token(seed: &str) -> String {
    let token = format!("{seed:-<36}");
    assert!(
        (36..=64).contains(&token.len()),
        "case {seed:?}: a ClientToken is 36..64 characters, built {token:?}"
    );
    token
}

/// `body` with `clientToken` set to `token`.
fn with_token(body: &Value, token: &str) -> Value {
    let mut carried = body.clone();
    carried["clientToken"] = json!(token);
    carried
}

/// How many resources `list_path` reports under `summaries`.
async fn count(router: &MultiServiceRouter, list_path: &str, summaries: &str) -> usize {
    let (status, body) = call(router, "GET", list_path, None).await;
    assert_eq!(status, 200, "list {list_path}: got {status} {body}");
    body[summaries]
        .as_array()
        .unwrap_or_else(|| panic!("list {list_path}: no {summaries} in {body}"))
        .len()
}

/// One row of the create table: a request that mints a resource, the same
/// request with one member changed, and how its answer reads back.
struct CreateRow {
    case: &'static str,
    /// The seed the row's token is built from — non-whitespace, as the shape
    /// requires.
    token_seed: &'static str,
    path: &'static str,
    body: Value,
    /// The same request with ONE member different, which is a DIFFERENT request
    /// and so must not replay.
    changed: Value,
    id_member: &'static str,
    arn_member: &'static str,
    status_member: &'static str,
    /// The listing that counts resources of this kind, and the member it reports
    /// them under.
    list_path: String,
    summaries: &'static str,
}

/// A `CustomID`-shaped id for the caller-supplied-id row, and a second one for
/// the request that differs only in it.
const CHOSEN_ID: &str = "3f2a1b4c-5d6e-4f70-8182-93a4b5c6d7e8";
const OTHER_CHOSEN_ID: &str = "4a3b2c1d-6e5f-4071-8293-a4b5c6d7e8f9";

/// The create rows, over both resource kinds this slice mints and over the
/// member sets their requests carry. `model_id` is the stored asset model the
/// asset rows instantiate.
fn create_rows(model_id: &str) -> Vec<CreateRow> {
    let models = "/asset-models?maxResults=250".to_string();
    let assets = format!("/assets?filter=ALL&assetModelId={model_id}&maxResults=250");
    vec![
        CreateRow {
            case: "CreateAssetModel, required members only",
            token_seed: "create-asset-model-minimal",
            path: "/asset-models",
            body: json!({"assetModelName": "IdempotentModel"}),
            changed: json!({"assetModelName": "IdempotentModel renamed"}),
            id_member: "assetModelId",
            arn_member: "assetModelArn",
            status_member: "assetModelStatus",
            list_path: models.clone(),
            summaries: "assetModelSummaries",
        },
        CreateRow {
            case: "CreateAssetModel, properties and tags, differing only in a tag value",
            token_seed: "create-asset-model-tagged",
            path: "/asset-models",
            body: json!({
                "assetModelName": "IdempotentTaggedModel",
                "assetModelDescription": "the first definition",
                "assetModelProperties": [
                    {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
                "tags": {"line": "A"},
            }),
            changed: json!({
                "assetModelName": "IdempotentTaggedModel",
                "assetModelDescription": "the first definition",
                "assetModelProperties": [
                    {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
                "tags": {"line": "B"},
            }),
            id_member: "assetModelId",
            arn_member: "assetModelArn",
            status_member: "assetModelStatus",
            list_path: models,
            summaries: "assetModelSummaries",
        },
        CreateRow {
            case: "CreateAsset, required members only",
            token_seed: "create-asset-minimal",
            path: "/assets",
            body: json!({"assetName": "IdempotentAsset", "assetModelId": model_id}),
            changed: json!({"assetName": "IdempotentAsset renamed", "assetModelId": model_id}),
            id_member: "assetId",
            arn_member: "assetArn",
            status_member: "assetStatus",
            list_path: assets.clone(),
            summaries: "assetSummaries",
        },
        CreateRow {
            case: "CreateAsset, description, external id and tags",
            token_seed: "create-asset-described",
            path: "/assets",
            body: json!({
                "assetName": "IdempotentDescribedAsset",
                "assetModelId": model_id,
                "assetDescription": "the first description",
                "assetExternalId": "idempotent-asset",
                "tags": {"line": "A"},
            }),
            // The external id too: it is unique per account, so the request that
            // differs in its description has to name an external id of its own
            // for a FRESH token to be able to serve it.
            changed: json!({
                "assetName": "IdempotentDescribedAsset",
                "assetModelId": model_id,
                "assetDescription": "the second description",
                "assetExternalId": "idempotent-asset-other",
                "tags": {"line": "A"},
            }),
            id_member: "assetId",
            arn_member: "assetArn",
            status_member: "assetStatus",
            list_path: assets.clone(),
            summaries: "assetSummaries",
        },
        CreateRow {
            case: "CreateAsset, caller-supplied assetId",
            token_seed: "create-asset-chosen-id",
            path: "/assets",
            body: json!({
                "assetName": "IdempotentChosenIdAsset",
                "assetModelId": model_id,
                "assetId": CHOSEN_ID,
            }),
            changed: json!({
                "assetName": "IdempotentChosenIdAsset",
                "assetModelId": model_id,
                "assetId": OTHER_CHOSEN_ID,
            }),
            id_member: "assetId",
            arn_member: "assetArn",
            status_member: "assetStatus",
            list_path: assets,
            summaries: "assetSummaries",
        },
    ]
}

#[tokio::test]
async fn a_retried_create_replays_the_first_answer_and_mints_nothing_more() {
    let router = router();
    let model_id = create_model(&router, "IdempotencyFixture", json!([])).await;

    for row in create_rows(&model_id) {
        let CreateRow {
            case,
            token_seed,
            path,
            body,
            changed,
            id_member,
            arn_member,
            status_member,
            list_path,
            summaries,
        } = row;
        let token = client_token(token_seed);

        let before = count(&router, &list_path, summaries).await;
        let (status, first) = call(&router, "POST", path, Some(with_token(&body, &token))).await;
        assert_eq!(status, 202, "case {case:?}: the create got {status} {first}");
        let after_first = count(&router, &list_path, summaries).await;
        assert_eq!(
            after_first,
            before + 1,
            "case {case:?}: the create must mint exactly one resource, {before} -> {after_first}"
        );

        // The retry: the same request under the same token.
        let (replay_status, replay) =
            call(&router, "POST", path, Some(with_token(&body, &token))).await;
        assert_eq!(
            replay_status, status,
            "case {case:?}: a retry must be answered like the call it repeats, got \
             {replay_status} {replay}"
        );
        for member in [id_member, arn_member] {
            assert_eq!(
                replay[member], first[member],
                "case {case:?}: a retry must replay {member}, got {replay} after {first}"
            );
        }
        assert_eq!(
            replay[status_member]["state"], first[status_member]["state"],
            "case {case:?}: a retry must report the resource the first call minted, got \
             {replay} after {first}"
        );
        let after_replay = count(&router, &list_path, summaries).await;
        assert_eq!(
            after_replay, after_first,
            "case {case:?}: a retry must mint nothing, {after_first} -> {after_replay}"
        );

        // A DIFFERENT request reusing the spent token.
        let (conflict_status, conflict) =
            call(&router, "POST", path, Some(with_token(&changed, &token))).await;
        assert_eq!(
            conflict_status, 409,
            "case {case:?}: a token spent on another request must be refused, got \
             {conflict_status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictingOperationException",
            "case {case:?}: got {conflict}"
        );
        assert_eq!(
            conflict["resourceId"], first[id_member],
            "case {case:?}: the refusal must name the resource the token was spent on, got \
             {conflict} after {first}"
        );
        assert_eq!(
            conflict["resourceArn"], first[arn_member],
            "case {case:?}: the refusal must carry that resource's ARN, got {conflict} after \
             {first}"
        );
        let after_conflict = count(&router, &list_path, summaries).await;
        assert_eq!(
            after_conflict, after_first,
            "case {case:?}: a refused request must mint nothing, {after_first} -> \
             {after_conflict}"
        );

        // The same changed request under a FRESH token is simply a new call: the
        // refusal above is about the TOKEN, not about the body.
        let fresh = client_token(&format!("{token_seed}-fresh"));
        let (fresh_status, created) =
            call(&router, "POST", path, Some(with_token(&changed, &fresh))).await;
        assert_eq!(
            fresh_status, status,
            "case {case:?}: a fresh token must serve the changed request, got {fresh_status} \
             {created}"
        );
        assert_ne!(
            created[id_member], first[id_member],
            "case {case:?}: a fresh token must mint a new resource, got {created} after {first}"
        );
        let after_fresh = count(&router, &list_path, summaries).await;
        assert_eq!(
            after_fresh,
            after_first + 1,
            "case {case:?}: the fresh-token call must mint one resource, {after_first} -> \
             {after_fresh}"
        );
    }
}

#[tokio::test]
async fn a_retried_asset_update_replays_the_answer_and_restarts_no_transition() {
    let router = router();
    let model_id = create_model(&router, "UpdateIdempotency", json!([])).await;
    let (status, created) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({"assetName": "Boiler1", "assetModelId": model_id})),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created}");
    let asset_id = created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset: no assetId in {created}"))
        .to_string();
    // The create's own owed transition falls due, so the UPDATING below cannot be
    // the leftover CREATING.
    tokio::time::sleep(settled()).await;

    let token = client_token("update-asset-retry");
    let body = json!({"assetName": "Boiler1 renamed", "assetDescription": "renamed once"});
    let uri = format!("/assets/{asset_id}");
    let (status, first) = call(&router, "PUT", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(status, 202, "UpdateAsset: got {status} {first}");
    assert_eq!(
        first["assetStatus"]["state"], "UPDATING",
        "the update's own transition is owed, got {first}"
    );

    // The transition that update owes falls due, and only THEN does the retry
    // land: a retry that re-applied the update would put the asset back into
    // UPDATING.
    tokio::time::sleep(settled()).await;
    let (replay_status, replay) = call(&router, "PUT", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(
        replay_status, status,
        "a retry must be answered like the call it repeats, got {replay_status} {replay}"
    );
    assert_eq!(
        replay["assetId"], first["assetId"],
        "a retry must name the same asset, got {replay} after {first}"
    );
    assert_eq!(
        replay["assetStatus"]["state"], "ACTIVE",
        "a retry must not restart the transition the first call owed, got {replay}"
    );

    let (status, described) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    assert_eq!(
        described["assetStatus"]["state"], "ACTIVE",
        "the retry must leave the settled asset settled, got {described}"
    );
    assert_eq!(
        described["assetName"], "Boiler1 renamed",
        "the retry must leave the first update's write in place, got {described}"
    );

    // A DIFFERENT update reusing the spent token changes nothing.
    let (conflict_status, conflict) = call(
        &router,
        "PUT",
        &uri,
        Some(with_token(
            &json!({"assetName": "Boiler1 renamed twice"}),
            &token,
        )),
    )
    .await;
    assert_eq!(
        conflict_status, 409,
        "a token spent on another request must be refused, got {conflict_status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "got {conflict}"
    );
    assert_eq!(
        conflict["resourceId"], json!(asset_id),
        "the refusal must name the asset the token was spent on, got {conflict}"
    );
    let (_, described) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        described["assetName"], "Boiler1 renamed",
        "a refused update must write nothing, got {described}"
    );
}

#[tokio::test]
async fn a_retried_asset_model_update_mints_no_second_version() {
    let router = router();
    let model_id = create_model(
        &router,
        "ModelUpdateIdempotency",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    tokio::time::sleep(settled()).await;

    let uri = format!("/asset-models/{model_id}");
    let (status, before) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {before}");
    let created_version = before["assetModelVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let token = client_token("update-asset-model-retry");
    let body = json!({
        "assetModelName": "ModelUpdateIdempotency renamed",
        "assetModelProperties": [],
    });
    let (status, first) = call(&router, "PUT", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {first}");
    tokio::time::sleep(settled()).await;
    let (_, updated) = call(&router, "GET", &uri, None).await;
    let updated_version = updated["assetModelVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        updated_version, created_version,
        "the accepted update must carry the model to a new version, got {updated}"
    );

    let (replay_status, replay) = call(&router, "PUT", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(
        replay_status, status,
        "a retry must be answered like the call it repeats, got {replay_status} {replay}"
    );
    assert_eq!(
        replay["assetModelStatus"]["state"], "ACTIVE",
        "a retry must not restart the transition the first call owed, got {replay}"
    );
    let (_, after_replay) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        after_replay["assetModelVersion"],
        json!(updated_version),
        "a retry must mint no second version, got {after_replay}"
    );
    assert_eq!(
        after_replay["assetModelName"], "ModelUpdateIdempotency renamed",
        "a retry must leave the first update's write in place, got {after_replay}"
    );

    let (conflict_status, conflict) = call(
        &router,
        "PUT",
        &uri,
        Some(with_token(
            &json!({"assetModelName": "ModelUpdateIdempotency renamed twice"}),
            &token,
        )),
    )
    .await;
    assert_eq!(
        conflict_status, 409,
        "a token spent on another request must be refused, got {conflict_status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "got {conflict}"
    );
    let (_, described) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        described["assetModelName"], "ModelUpdateIdempotency renamed",
        "a refused update must write nothing, got {described}"
    );
    assert_eq!(
        described["assetModelVersion"],
        json!(updated_version),
        "a refused update must mint no version, got {described}"
    );
}

/// A model with two measurement properties of one data type, one asset of it,
/// and the two property ids, read back through `DescribeAsset`.
async fn property_fixture() -> (MultiServiceRouter, String, String, String) {
    let (router, _engine) = router_with_engine();
    let model_id = create_model(
        &router,
        "PropertyIdempotency",
        json!([
            {"name": "Inlet", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Outlet", "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let (status, created) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({"assetName": "Pump1", "assetModelId": model_id})),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created}");
    let asset_id = created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset: no assetId in {created}"))
        .to_string();
    let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    let property = |name: &str| {
        described["assetProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("DescribeAsset: no assetProperties in {described}"))
            .iter()
            .find(|property| property["name"] == name)
            .and_then(|property| property["id"].as_str())
            .unwrap_or_else(|| panic!("DescribeAsset: no property {name:?} in {described}"))
            .to_string()
    };
    let inlet = property("Inlet");
    let outlet = property("Outlet");
    (router, asset_id, inlet, outlet)
}

/// The property named `name` in a `DescribeAsset` body.
fn described_property(described: &Value, name: &str) -> Value {
    described["assetProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeAsset: no assetProperties in {described}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("DescribeAsset: no property {name:?} in {described}"))
        .clone()
}

#[tokio::test]
async fn a_retried_property_update_is_served_and_a_reused_token_is_refused() {
    let (router, asset_id, inlet, outlet) = property_fixture().await;
    let uri = format!("/assets/{asset_id}/properties/{inlet}");
    let token = client_token("update-asset-property-retry");
    let body = json!({"propertyAlias": "/line/a/inlet", "propertyNotificationState": "ENABLED"});

    let (status, first) = call(&router, "PUT", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(status, 200, "UpdateAssetProperty: got {status} {first}");

    let (replay_status, replay) = call(&router, "PUT", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(
        replay_status, status,
        "a retry must be answered like the call it repeats, got {replay_status} {replay}"
    );

    let (_, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let property = described_property(&described, "Inlet");
    assert_eq!(
        property["alias"], "/line/a/inlet",
        "the retry must leave the first write in place, got {property}"
    );
    assert_eq!(
        property["notification"]["state"], "ENABLED",
        "the retry must leave the first write in place, got {property}"
    );

    // A DIFFERENT request under the spent token — here one addressing the OTHER
    // property, which is what makes it a different request.
    let (conflict_status, conflict) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{outlet}"),
        Some(with_token(
            &json!({"propertyAlias": "/line/a/outlet"}),
            &token,
        )),
    )
    .await;
    assert_eq!(
        conflict_status, 409,
        "a token spent on another request must be refused, got {conflict_status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "got {conflict}"
    );
    let (_, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let untouched = described_property(&described, "Outlet");
    assert!(
        untouched.get("alias").is_none_or(Value::is_null),
        "a refused property write must bind nothing, got {untouched}"
    );
}

#[tokio::test]
async fn a_retried_time_series_association_and_disassociation_are_served() {
    let (router, asset_id, inlet, outlet) = property_fixture().await;
    let alias = "/line/a/inlet";

    // Bind the alias, then break the binding: that leaves the stream standing as
    // a disassociated time series the association call below can bind again.
    let (status, bound) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{inlet}"),
        Some(json!({"propertyAlias": alias})),
    )
    .await;
    assert_eq!(status, 200, "UpdateAssetProperty: got {status} {bound}");

    let disassociate =
        format!("/timeseries/disassociate?alias={alias}&assetId={asset_id}&propertyId={inlet}");
    let break_token = client_token("disassociate-time-series-retry");
    let (status, broken) = call(
        &router,
        "POST",
        &disassociate,
        Some(json!({"clientToken": break_token})),
    )
    .await;
    assert_eq!(
        status, 200,
        "DisassociateTimeSeriesFromAssetProperty: got {status} {broken}"
    );
    // The retry names an association the store no longer holds, which any OTHER
    // caller is refused — the retry of the call that broke it is served.
    let (replay_status, replay) = call(
        &router,
        "POST",
        &disassociate,
        Some(json!({"clientToken": break_token})),
    )
    .await;
    assert_eq!(
        replay_status, 200,
        "a retried disassociation must succeed, got {replay_status} {replay}"
    );
    let (refused_status, refused) = call(&router, "POST", &disassociate, None).await;
    assert_eq!(
        refused_status, 404,
        "a caller with no token naming an absent association must be refused, got \
         {refused_status} {refused}"
    );

    let associate =
        format!("/timeseries/associate?alias={alias}&assetId={asset_id}&propertyId={inlet}");
    let bind_token = client_token("associate-time-series-retry");
    let (status, associated) = call(
        &router,
        "POST",
        &associate,
        Some(json!({"clientToken": bind_token})),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateTimeSeriesToAssetProperty: got {status} {associated}"
    );
    let (replay_status, replay) = call(
        &router,
        "POST",
        &associate,
        Some(json!({"clientToken": bind_token})),
    )
    .await;
    assert_eq!(
        replay_status, 200,
        "a retried association must succeed, got {replay_status} {replay}"
    );
    let (_, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let property = described_property(&described, "Inlet");
    assert_eq!(
        property["alias"], alias,
        "the association must leave the alias bound to the property, got {property}"
    );

    // A DIFFERENT association request reusing the token that bound this one.
    let (conflict_status, conflict) = call(
        &router,
        "POST",
        &format!("/timeseries/associate?alias={alias}&assetId={asset_id}&propertyId={outlet}"),
        Some(json!({"clientToken": bind_token})),
    )
    .await;
    assert_eq!(
        conflict_status, 409,
        "a token spent on another request must be refused, got {conflict_status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "got {conflict}"
    );
    let (_, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let untouched = described_property(&described, "Outlet");
    assert!(
        untouched.get("alias").is_none_or(Value::is_null),
        "a refused association must bind nothing, got {untouched}"
    );
}

#[tokio::test]
async fn a_retried_asset_association_records_one_edge() {
    let router = router();
    let (status, turbine_model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "Turbine"})),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {turbine_model}");
    let turbine_model_id = turbine_model["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {turbine_model}"))
        .to_string();
    let (status, farm_model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Farm",
            "assetModelHierarchies": [
                {"name": "turbines", "childAssetModelId": turbine_model_id},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {farm_model}");
    let farm_model_id = farm_model["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {farm_model}"))
        .to_string();
    let (status, described) = call(
        &router,
        "GET",
        &format!("/asset-models/{farm_model_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");
    let hierarchy_id = described["assetModelHierarchies"]
        .as_array()
        .and_then(|hierarchies| hierarchies.first())
        .and_then(|hierarchy| hierarchy["id"].as_str())
        .unwrap_or_else(|| panic!("DescribeAssetModel: no hierarchy in {described}"))
        .to_string();

    let farm = create_one_asset(&router, "Farm1", &farm_model_id).await;
    let first_turbine = create_one_asset(&router, "Turbine1", &turbine_model_id).await;
    let second_turbine = create_one_asset(&router, "Turbine2", &turbine_model_id).await;

    let uri = format!("/assets/{farm}/associate");
    let token = client_token("associate-assets-retry");
    let body = json!({"hierarchyId": hierarchy_id, "childAssetId": first_turbine});
    let (status, associated) = call(&router, "POST", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(status, 200, "AssociateAssets: got {status} {associated}");

    // The retry lands while the association the first call recorded is still
    // settling: a call judged on that state would collide with it.
    let (replay_status, replay) = call(&router, "POST", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(
        replay_status, status,
        "a retried association must succeed, got {replay_status} {replay}"
    );
    // And once it has settled, a retry is still the same answer.
    tokio::time::sleep(settled()).await;
    let (late_status, late) = call(&router, "POST", &uri, Some(with_token(&body, &token))).await;
    assert_eq!(
        late_status, status,
        "a retry taken after the association settled must succeed, got {late_status} {late}"
    );

    let children = associated_ids(&router, &farm, &hierarchy_id).await;
    assert_eq!(
        children,
        vec![first_turbine.clone()],
        "the retries must record exactly one edge, got {children:?}"
    );

    // A DIFFERENT association reusing the spent token.
    let (conflict_status, conflict) = call(
        &router,
        "POST",
        &uri,
        Some(with_token(
            &json!({"hierarchyId": hierarchy_id, "childAssetId": second_turbine}),
            &token,
        )),
    )
    .await;
    assert_eq!(
        conflict_status, 409,
        "a token spent on another request must be refused, got {conflict_status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "got {conflict}"
    );
    assert_eq!(
        conflict["resourceId"], json!(farm),
        "the refusal must name the asset the token was spent on, got {conflict}"
    );
    let children = associated_ids(&router, &farm, &hierarchy_id).await;
    assert_eq!(
        children,
        vec![first_turbine.clone()],
        "a refused association must record nothing, got {children:?}"
    );

    // `DisassociateAssets`, the mirror call, follows the same rule: a retry of it
    // succeeds where any other caller naming the association it has already
    // removed is refused.
    let disassociate_uri = format!("/assets/{farm}/disassociate");
    let remove_token = client_token("disassociate-assets-retry");
    let remove = json!({"hierarchyId": hierarchy_id, "childAssetId": first_turbine});
    let (status, removed) = call(
        &router,
        "POST",
        &disassociate_uri,
        Some(with_token(&remove, &remove_token)),
    )
    .await;
    assert_eq!(status, 200, "DisassociateAssets: got {status} {removed}");
    let (replay_status, replay) = call(
        &router,
        "POST",
        &disassociate_uri,
        Some(with_token(&remove, &remove_token)),
    )
    .await;
    assert_eq!(
        replay_status, status,
        "a retried disassociation must succeed, got {replay_status} {replay}"
    );
    let (refused_status, refused) =
        call(&router, "POST", &disassociate_uri, Some(remove.clone())).await;
    assert_eq!(
        refused_status, 404,
        "a caller with no token naming an absent association must be refused, got \
         {refused_status} {refused}"
    );
    let children = associated_ids(&router, &farm, &hierarchy_id).await;
    assert!(
        children.is_empty(),
        "the disassociation and its retry must leave no edge, got {children:?}"
    );
}

#[tokio::test]
async fn a_retry_whose_resource_is_gone_is_served_as_a_new_call() {
    let router = router();
    let model_id = create_model(&router, "GoneResourceIdempotency", json!([])).await;
    let token = client_token("create-asset-then-delete");
    let body = json!({"assetName": "Transient", "assetModelId": model_id});

    let (status, first) = call(&router, "POST", "/assets", Some(with_token(&body, &token))).await;
    assert_eq!(status, 202, "CreateAsset: got {status} {first}");
    let asset_id = first["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset: no assetId in {first}"))
        .to_string();

    let (status, deleted) = call(&router, "DELETE", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 202, "DeleteAsset: got {status} {deleted}");

    // The token was spent on an asset the store no longer holds, so there is
    // nothing to replay and the call is served as the new create it now is.
    let (status, again) = call(&router, "POST", "/assets", Some(with_token(&body, &token))).await;
    assert_eq!(status, 202, "CreateAsset: got {status} {again}");
    assert_ne!(
        again["assetId"], first["assetId"],
        "a retry with nothing left to replay must mint a new asset, got {again} after {first}"
    );
}

/// `POST /assets` for one asset of `model_id`, returning its id.
async fn create_one_asset(router: &MultiServiceRouter, name: &str, model_id: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/assets",
        Some(json!({"assetName": name, "assetModelId": model_id})),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset {name}: got {status} {created}");
    created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset {name}: no assetId in {created}"))
        .to_string()
}

/// The child asset ids `ListAssociatedAssets` reports under `hierarchy_id`.
async fn associated_ids(
    router: &MultiServiceRouter,
    asset_id: &str,
    hierarchy_id: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/hierarchies?hierarchyId={hierarchy_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssociatedAssets: got {status} {body}");
    body["assetSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListAssociatedAssets: no assetSummaries in {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_token_is_scoped_to_its_operation_and_a_request_without_one_is_a_new_call() {
    let router = router();
    // ONE token value, sent to two different operations: each spends its own.
    let shared = client_token("one-token-two-operations");
    let (status, model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "SharedToken", "clientToken": shared})),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {model}");
    let model_id = model["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {model}"))
        .to_string();
    let (status, asset) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "SharedTokenAsset",
            "assetModelId": model_id,
            "clientToken": shared,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "a token spent on another operation must not refuse this one, got {status} {asset}"
    );

    // No token at all: every call is a new one, so two identical requests mint
    // two assets.
    let body = json!({"assetName": "Untokened", "assetModelId": model_id});
    let (first_status, first) = call(&router, "POST", "/assets", Some(body.clone())).await;
    let (second_status, second) = call(&router, "POST", "/assets", Some(body)).await;
    assert_eq!(first_status, 202, "CreateAsset: got {first_status} {first}");
    assert_eq!(
        second_status, 202,
        "CreateAsset: got {second_status} {second}"
    );
    assert_ne!(
        first["assetId"], second["assetId"],
        "a request carrying no clientToken is a new call every time, got {second} after {first}"
    );
}
