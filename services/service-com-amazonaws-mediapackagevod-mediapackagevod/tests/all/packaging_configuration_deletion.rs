// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeletePackagingConfiguration`.
//!
//! Every case drives real HTTP through the router the production binary builds, so
//! what is asserted is the wire behaviour a `mediapackage-vod` client sees. Each test
//! asserts a RULE over a TABLE of inputs; the recorded call is one row.
//!
//! The rules under test:
//!
//! * `DeletePackagingConfiguration` removes the addressed PackagingConfiguration from
//!   state and answers `204 No Content` — the RECORDED success status of a MediaPackage
//!   VOD delete, which this model's `@http` code 202 disagrees with;
//! * removal is complete: the stored record and the tags keyed by its ARN are gone,
//!   `DescribePackagingConfiguration` answers 404 for it, the id is free to be created
//!   again, and every asset in that configuration's PackagingGroup stops deriving an
//!   egress endpoint for it on the next read;
//! * the delete touches NOTHING else: the other configurations in the group, the group
//!   itself and the assets in it all still read back;
//! * an `Id` this service holds no PackagingConfiguration under is the ADDRESSED
//!   resource missing — 404 `NotFoundException`, worded in the service's one
//!   request-derived form — whatever the id's form, and whether or not a PackagingGroup
//!   or an Asset happens to be stored under that same id.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

/// The resource-type label this service words a message about a PackagingConfiguration
/// with. Not the modelled resource name: the recorded AWS not-found message is
/// `PackagingConfig with id=<id> not found`, so the abbreviated label is asserted here
/// as a literal rather than read back from the code that produces it.
const CONFIG_LABEL: &str = "PackagingConfig";

/// The region the test signs for. It appears in every ARN the state assertions build.
const REGION: &str = "us-west-2";

/// The recorded success status of a MediaPackage VOD delete.
const DELETE_SUCCESS: u16 = 204;

/// The wire error code of a missing addressed resource and its modelled `httpError`
/// (`smithy_query(query="operation",operation="DeletePackagingConfiguration")`:
/// `NotFoundException`, `errorKind client`, `httpError 404`).
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// What one answer says about itself: status, `x-amzn-errortype` and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// The `message` member the modelled error shapes spell `Message` with
    /// `jsonName: message`, or `"<unset>"` when the body carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }
}

/// A router carrying exactly this service over the engine the caller holds, so a case
/// can inspect the SAME store the handlers wrote to.
fn router_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(engine),
    );
    router
}

/// Send one request and report what it answered.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// A package block of each modelled packaging format, so a case can vary the format
/// without varying anything else (`CreatePackagingConfiguration` takes exactly one of
/// `cmafPackage` / `dashPackage` / `hlsPackage` / `mssPackage`).
fn package_block(format: &str) -> Value {
    match format {
        "hlsPackage" => json!({ "hlsPackage": { "hlsManifests": [{}] } }),
        "dashPackage" => json!({ "dashPackage": { "dashManifests": [{}] } }),
        "cmafPackage" => json!({ "cmafPackage": { "hlsManifests": [{}] } }),
        "mssPackage" => json!({ "mssPackage": { "mssManifests": [{}] } }),
        other => panic!("no such packaging format in this model: {other:?}"),
    }
}

/// Create a packaging group, asserting the setup succeeded.
async fn create_group(router: &MultiServiceRouter, id: &str) {
    let answer = call(router, "POST", "/packaging_groups", Some(json!({ "id": id }))).await;
    assert_eq!(
        answer.status, 200,
        "setup: CreatePackagingGroup {id:?}: expected 200, got {} body {}",
        answer.status, answer.body
    );
}

/// Create a tagged packaging configuration of `format` in `group`, asserting it
/// succeeded. The tag is what makes the ARN-keyed tag store observable after a delete.
async fn create_config(router: &MultiServiceRouter, id: &str, group: &str, format: &str) {
    let mut body = json!({ "id": id, "packagingGroupId": group, "tags": { "suite": "deletion" } });
    let Value::Object(members) = package_block(format) else {
        panic!("a package block must be a JSON object");
    };
    for (key, value) in members {
        body[key] = value;
    }
    let answer = call(router, "POST", "/packaging_configurations", Some(body)).await;
    // 202 Accepted: the recorded status of a successful create (the model declares
    // 200) — see `src/ops/create_packaging_configuration.rs`.
    assert_eq!(
        answer.status, 202,
        "setup: CreatePackagingConfiguration {id:?} ({format}): expected 202, got {} body {}",
        answer.status, answer.body
    );
}

/// Create an asset in `group`, asserting it succeeded.
async fn create_asset(router: &MultiServiceRouter, id: &str, group: &str) {
    let body = json!({
        "id": id,
        "packagingGroupId": group,
        "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
        "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", engine::emulator_account_id()),
    });
    let answer = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateAsset {id:?}: expected 200, got {} body {}",
        answer.status, answer.body
    );
}

/// The configuration ids `DescribeAsset` derives an egress endpoint for, in the order
/// they are reported.
async fn endpoint_config_ids(router: &MultiServiceRouter, asset_id: &str) -> Vec<String> {
    let answer = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "DescribeAsset {asset_id:?}: expected 200, got {} body {}",
        answer.status, answer.body
    );
    answer.body["egressEndpoints"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["packagingConfigurationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The not-found message this service words for a PackagingConfiguration the request
/// addressed and the store holds nothing under: the id the REQUEST asked for, bound in
/// the service's one form (`<label> with id=<id> not found`, no trailing period).
fn not_found_message(id: &str) -> String {
    format!("{CONFIG_LABEL} with id={id} not found")
}

/// `DescribePackagingConfiguration` for `id` — the reader this suite proves the delete
/// through.
async fn describe_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/packaging_configurations/{id}"), None).await
}

/// The PackagingConfiguration ids the store holds, read from the same state the
/// handlers wrote.
fn stored_config_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = vod::service_state(engine).expect("the service state store");
    // The store is keyed by (Region, `Id`) — this suite signs for one Region, so the
    // `Id` half is what it compares.
    let ids = store
        .lock()
        .packaging_configurations
        .keys()
        .map(|(_, id)| id.clone())
        .collect();
    ids
}

/// Whether the store holds a tag entry keyed by `arn`.
fn holds_tags_for(engine: &engine::InMemoryEngine, arn: &str) -> bool {
    let store = vod::service_state(engine).expect("the service state store");
    let held = store.lock().tags_by_arn.contains_key(arn);
    held
}

/// One row of the deletion table: the configurations stored in the group (id and
/// packaging format) and which of them the case deletes.
struct Row {
    /// What a failing assertion names this case.
    case: &'static str,
    /// `(id, packaging format)` per configuration created in the group, in order.
    configs: Vec<(String, &'static str)>,
    /// Index into [`Self::configs`] of the configuration the case deletes.
    delete: usize,
}

/// The deletion table: rows differ in the id's form, the packaging format, how many
/// configurations share the group and which position in that set is deleted.
fn deletion_rows() -> Vec<Row> {
    let formats = ["hlsPackage", "dashPackage", "cmafPackage", "mssPackage"];
    let mut rows = vec![
        Row {
            case: "the only configuration in the group",
            configs: vec![("cfg".to_string(), "hlsPackage")],
            delete: 0,
        },
        Row {
            case: "a single-character id",
            configs: vec![("c".to_string(), "dashPackage")],
            delete: 0,
        },
        Row {
            case: "an id with mixed case, digits and an underscore",
            configs: vec![("Mixed_Case-9".to_string(), "mssPackage")],
            delete: 0,
        },
        Row {
            case: "a dotted 20-character id",
            configs: vec![("dotted.id-0123456789".to_string(), "cmafPackage")],
            delete: 0,
        },
        Row {
            case: "a 100-character id",
            configs: vec![("x".repeat(100), "hlsPackage")],
            delete: 0,
        },
    ];
    // One row per position in a four-configuration group — first, middle and last — so
    // the delete cannot depend on where the record sits in the stored order, and each
    // group covers every packaging format at once.
    let names = [
        "the first of four configurations",
        "the second of four configurations",
        "the third of four configurations",
        "the last of four configurations",
    ];
    for (index, case) in names.into_iter().enumerate() {
        rows.push(Row {
            case,
            configs: formats
                .iter()
                .map(|format| (format!("cfg-{format}"), *format))
                .collect(),
            delete: index,
        });
    }
    rows
}

/// A delete removes the addressed configuration and nothing else: 204, the record and
/// its tags are gone, the id is free again, the derived egress endpoints shrink by
/// exactly that configuration, and the group, the asset and the sibling
/// configurations survive.
#[tokio::test]
async fn deleting_a_packaging_configuration_removes_only_that_configuration() {
    for row in deletion_rows() {
        let case = row.case;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));

        create_group(&router, "vod-group").await;
        for (id, format) in &row.configs {
            create_config(&router, id, "vod-group", format).await;
        }
        create_asset(&router, "asset-1", "vod-group").await;

        let (deleted_id, _) = row.configs[row.delete].clone();
        let deleted_arn = vod::resource_arn("packaging-configurations", REGION, &deleted_id);
        let mut survivors: Vec<String> = row
            .configs
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|id| id != &deleted_id)
            .collect();
        survivors.sort();

        // Before: the asset derives one endpoint per configuration in the group, and
        // every configuration reads back through `DescribePackagingConfiguration`.
        for (id, format) in &row.configs {
            let described = describe_config(&router, id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribePackagingConfiguration {id:?} before the delete: \
                 expected 200, got {} body {}",
                described.status, described.body
            );
            assert_eq!(
                described.body["id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: the read must report the stored id, got {}",
                described.body["id"]
            );
            assert_eq!(
                described.body["packagingGroupId"].as_str(),
                Some("vod-group"),
                "case {case:?}: the read must report the stored group, got {}",
                described.body["packagingGroupId"]
            );
            assert_eq!(
                described.body["arn"].as_str(),
                Some(vod::resource_arn("packaging-configurations", REGION, id).as_str()),
                "case {case:?}: the read must report the stored ARN, got {}",
                described.body["arn"]
            );
            assert!(
                described.body[format].is_object(),
                "case {case:?}: the read must carry the {format} block it was created \
                 with, got {}",
                described.body
            );
            assert_eq!(
                described.body["tags"],
                json!({ "suite": "deletion" }),
                "case {case:?}: the read must report the tags stored under the ARN, got {}",
                described.body["tags"]
            );
        }
        let mut before = endpoint_config_ids(&router, "asset-1").await;
        before.sort();
        let mut all: Vec<String> = row.configs.iter().map(|(id, _)| id.clone()).collect();
        all.sort();
        assert_eq!(
            before, all,
            "case {case:?}: before the delete the asset must derive one endpoint per \
             stored configuration, got {before:?}"
        );

        let answer = call(
            &router,
            "DELETE",
            &format!("/packaging_configurations/{deleted_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, DELETE_SUCCESS,
            "case {case:?}: expected the recorded HTTP {DELETE_SUCCESS}, got {} body {}",
            answer.status, answer.body
        );

        // The stored record is gone, and only it.
        let mut stored = stored_config_ids(&engine);
        stored.sort();
        assert_eq!(
            stored, survivors,
            "case {case:?}: the store must hold exactly the configurations that were not \
             deleted, got {stored:?}"
        );
        assert!(
            !holds_tags_for(&engine, &deleted_arn),
            "case {case:?}: the tags keyed by the deleted configuration's ARN must be \
             forgotten, got the entry for {deleted_arn:?}"
        );
        for id in &survivors {
            let arn = vod::resource_arn("packaging-configurations", REGION, id);
            assert!(
                holds_tags_for(&engine, &arn),
                "case {case:?}: a sibling configuration's tags must survive, lost {arn:?}"
            );
        }

        // The read answers the missing addressed resource for it, and still answers the
        // survivors.
        let described = describe_config(&router, &deleted_id).await;
        assert_eq!(
            described.status, NOT_FOUND.1,
            "case {case:?}: DescribePackagingConfiguration after the delete: expected HTTP \
             {}, got {} body {}",
            NOT_FOUND.1, described.status, described.body
        );
        assert_eq!(
            described.message(),
            not_found_message(&deleted_id),
            "case {case:?}: expected the service's request-derived not-found form, got {:?}",
            described.message()
        );
        for id in &survivors {
            let described = describe_config(&router, id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: a sibling configuration must still read back, {id:?} got {} \
                 body {}",
                described.status, described.body
            );
        }

        // The derived egress endpoints shrink by exactly the deleted configuration.
        let mut after = endpoint_config_ids(&router, "asset-1").await;
        after.sort();
        assert_eq!(
            after, survivors,
            "case {case:?}: after the delete the asset must derive an endpoint for every \
             surviving configuration and none for the deleted one, got {after:?}"
        );

        // A second delete on the same id is the addressed resource missing.
        let answer = call(
            &router,
            "DELETE",
            &format!("/packaging_configurations/{deleted_id}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, NOT_FOUND.1,
            "case {case:?}: re-deleting must answer HTTP {}, got {} body {}",
            NOT_FOUND.1, answer.status, answer.body
        );
        assert_eq!(
            answer.errortype, NOT_FOUND.0,
            "case {case:?}: expected x-amzn-errortype {}, got {:?}",
            NOT_FOUND.0, answer.errortype
        );

        // The group and the asset are untouched by a configuration delete.
        let group = call(&router, "GET", "/packaging_groups/vod-group", None).await;
        assert_eq!(
            group.status, 200,
            "case {case:?}: the PackagingGroup must survive a configuration delete, got {} \
             body {}",
            group.status, group.body
        );
        let asset = call(&router, "GET", "/assets/asset-1", None).await;
        assert_eq!(
            asset.status, 200,
            "case {case:?}: the Asset must survive a configuration delete, got {} body {}",
            asset.status, asset.body
        );

        // The record is really gone, not flagged: the id can be created again, it reads
        // back, and the asset derives its endpoint once more.
        create_config(&router, &deleted_id, "vod-group", "hlsPackage").await;
        let described = describe_config(&router, &deleted_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the re-created configuration must read back, got {} body {}",
            described.status, described.body
        );
        let mut reborn = endpoint_config_ids(&router, "asset-1").await;
        reborn.sort();
        assert_eq!(
            reborn, all,
            "case {case:?}: re-creating the deleted id must restore its endpoint, got \
             {reborn:?}"
        );
    }
}

/// An id this service holds no PackagingConfiguration under is a missing ADDRESSED
/// resource on BOTH operations that address one by id: 404 `NotFoundException`, worded
/// in the service's one request-derived form, whatever the id's form — and a
/// PackagingGroup or Asset stored under the same id is not a PackagingConfiguration.
#[tokio::test]
async fn addressing_an_unknown_packaging_configuration_is_not_found() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));

    // Ids differing in length, separators, case and digits; none names a stored
    // configuration.
    let unknown = [
        "nope".to_string(),
        "c".to_string(),
        "Config_42".to_string(),
        "dotted.id-0123456789".to_string(),
        "y".repeat(100),
        // Stored below as a PackagingGroup and as an Asset, never as a configuration.
        "shared-id".to_string(),
    ];
    create_group(&router, "shared-id").await;
    create_asset(&router, "shared-id", "shared-id").await;
    // One stored configuration, so the 404s below are not a blanket answer.
    create_config(&router, "stored-cfg", "shared-id", "hlsPackage").await;

    for id in &unknown {
        // The SAME rule on both operations that address a configuration by id.
        for (op, method) in [
            ("DeletePackagingConfiguration", "DELETE"),
            ("DescribePackagingConfiguration", "GET"),
        ] {
            let case = format!("{op} {id:?}");
            let answer = call(
                &router,
                method,
                &format!("/packaging_configurations/{id}"),
                None,
            )
            .await;
            assert_eq!(
                answer.status, NOT_FOUND.1,
                "case {case:?}: expected HTTP {}, got {} body {}",
                NOT_FOUND.1, answer.status, answer.body
            );
            assert_eq!(
                answer.errortype, NOT_FOUND.0,
                "case {case:?}: expected x-amzn-errortype {}, got {:?}",
                NOT_FOUND.0, answer.errortype
            );
            assert_eq!(
                answer.message(),
                not_found_message(id),
                "case {case:?}: expected the service's request-derived not-found form, got \
                 {:?}",
                answer.message()
            );
        }
    }

    // The refused deletes changed nothing: the stored configuration, the group and the
    // asset that share an id with one of them all still stand.
    assert_eq!(
        stored_config_ids(&engine),
        vec!["stored-cfg".to_string()],
        "the refused deletes must leave the stored configuration in place, got {:?}",
        stored_config_ids(&engine)
    );
    let answer = call(
        &router,
        "DELETE",
        "/packaging_configurations/stored-cfg",
        None,
    )
    .await;
    assert_eq!(
        answer.status, DELETE_SUCCESS,
        "the stored configuration must still be deletable, got {} body {}",
        answer.status, answer.body
    );
    let group = call(&router, "GET", "/packaging_groups/shared-id", None).await;
    assert_eq!(
        group.status, 200,
        "the PackagingGroup sharing the id must be untouched, got {} body {}",
        group.status, group.body
    );
    let asset = call(&router, "GET", "/assets/shared-id", None).await;
    assert_eq!(
        asset.status, 200,
        "the Asset sharing the id must be untouched, got {} body {}",
        asset.status, asset.body
    );
}
