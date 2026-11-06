// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeletePackagingGroup`'s referential integrity.
//!
//! Every case drives real HTTP through the router the production binary builds, so what
//! is asserted is the wire behaviour a `mediapackage-vod` client sees. Each test asserts
//! a RULE over a TABLE of inputs; the recorded call is one row.
//!
//! The rules under test:
//!
//! * a PackagingGroup that any stored child resource still names in its
//!   `PackagingGroupId` — a PackagingConfiguration or an Asset — cannot be deleted: the
//!   delete is refused with the modelled `UnprocessableEntityException` (HTTP 422,
//!   `errorKind client`) and the refusal has NO effect — the group still reads back
//!   byte-for-byte, keeps its tags, and every configuration and asset it holds is still
//!   stored;
//! * the protection is keyed on the children stored AT DELETE TIME and on the group the
//!   request addresses: deleting the dependent children first lets the identical delete
//!   succeed (`204 No Content`, then `DescribePackagingGroup` 404 and the tags gone),
//!   and a child attached to a DIFFERENT group protects nothing;
//! * an `Id` this service holds no PackagingGroup under is the ADDRESSED resource
//!   missing — 404 `NotFoundException` — whatever children the service holds.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The recorded success status of a MediaPackage VOD delete (the model declares 202 —
/// see `src/ops/delete_packaging_group.rs`).
const DELETE_SUCCESS: u16 = 204;

/// The wire error code of the dependency refusal and its modelled `httpError`
/// (`smithy_query(query="operation",operation="DeletePackagingGroup")`:
/// `UnprocessableEntityException`, `errorKind client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// The wire error code of a missing addressed resource and its modelled `httpError`
/// (same query: `NotFoundException`, `errorKind client`, `httpError 404`).
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// What one answer says about itself: status, `x-amzn-errortype` and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
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

/// A package block of each modelled packaging format, so a row can vary the format of
/// the dependent configuration without varying anything else
/// (`CreatePackagingConfiguration` takes exactly one of `cmafPackage` / `dashPackage` /
/// `hlsPackage` / `mssPackage`).
fn package_block(format: &str) -> Value {
    match format {
        "hlsPackage" => json!({ "hlsPackage": { "hlsManifests": [{}] } }),
        "dashPackage" => json!({ "dashPackage": { "dashManifests": [{}] } }),
        "cmafPackage" => json!({ "cmafPackage": { "hlsManifests": [{}] } }),
        "mssPackage" => json!({ "mssPackage": { "mssManifests": [{}] } }),
        other => panic!("no such packaging format in this model: {other:?}"),
    }
}

/// Create a tagged packaging group, asserting the setup succeeded. The tag is what
/// makes the ARN-keyed tag store observable across a refused and an accepted delete.
async fn create_group(router: &MultiServiceRouter, id: &str) {
    let body = json!({ "id": id, "tags": { "suite": "group-deletion" } });
    let answer = call(router, "POST", "/packaging_groups", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "setup: CreatePackagingGroup {id:?}: expected 200, got {} body {}",
        answer.status, answer.body
    );
}

/// Create a packaging configuration of `format` in `group`, asserting it succeeded.
async fn create_config(router: &MultiServiceRouter, id: &str, group: &str, format: &str) {
    let mut body = json!({ "id": id, "packagingGroupId": group });
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
        "sourceRoleArn": format!(
            "arn:aws:iam::{}:role/MediaPackageVod",
            engine::emulator_account_id()
        ),
    });
    let answer = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateAsset {id:?}: expected 200, got {} body {}",
        answer.status, answer.body
    );
}

/// `DescribePackagingGroup`'s answer for `id`.
async fn describe_group(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/packaging_groups/{id}"), None).await
}

/// `DeletePackagingGroup`'s answer for `id`.
async fn delete_group(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "DELETE", &format!("/packaging_groups/{id}"), None).await
}

/// `DeletePackagingConfiguration`'s answer for `id`, asserting the teardown succeeded.
async fn delete_config(router: &MultiServiceRouter, id: &str) {
    let answer = call(
        router,
        "DELETE",
        &format!("/packaging_configurations/{id}"),
        None,
    )
    .await;
    // Every delete of this service answers 204 No Content.
    assert_eq!(
        answer.status, DELETE_SUCCESS,
        "DeletePackagingConfiguration {id:?}: expected {DELETE_SUCCESS}, got {} body {}",
        answer.status, answer.body
    );
}

/// `DeleteAsset`'s answer for `id`, asserting the teardown succeeded.
async fn delete_asset(router: &MultiServiceRouter, id: &str) {
    let answer = call(router, "DELETE", &format!("/assets/{id}"), None).await;
    assert_eq!(
        answer.status, DELETE_SUCCESS,
        "DeleteAsset {id:?}: expected {DELETE_SUCCESS}, got {} body {}",
        answer.status, answer.body
    );
}

/// The id of the `index`th asset a row populates its group with.
fn asset_id(index: usize) -> String {
    format!("asset-{index}")
}

/// One row of the dependency table: how the group under test was populated.
#[derive(Debug, Clone)]
struct Row {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// `(configuration id, packaging format)` of each configuration created IN the
    /// group under test — one of the two dependent classes the refusal keys off.
    configs: Vec<(&'static str, &'static str)>,
    /// How many assets the group holds — the other dependent class.
    assets: usize,
}

/// The group every row populates and then tries to delete.
const GROUP: &str = "protected-group";

/// The rows span the dependent CLASS (configurations only, assets only, both), the
/// dependent COUNT (one, two, three, four), the packaging FORMAT of each dependent
/// configuration (each of the four the model declares) and the FORM of its id — because
/// the rule is about whether ANY child still names the group, not about any of those.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one hls dependent",
            configs: vec![("cfg-hls", "hlsPackage")],
            assets: 0,
        },
        Row {
            case: "one dash dependent, two assets",
            configs: vec![("cfg.dash.1", "dashPackage")],
            assets: 2,
        },
        Row {
            case: "two dependents, cmaf and mss",
            configs: vec![("cfg-cmaf", "cmafPackage"), ("CFG_MSS", "mssPackage")],
            assets: 0,
        },
        Row {
            case: "four dependents, every modelled format, one asset",
            configs: vec![
                ("a", "hlsPackage"),
                ("cfg-2", "dashPackage"),
                ("cfg_3", "cmafPackage"),
                ("cfg-4-mss", "mssPackage"),
            ],
            assets: 1,
        },
        Row {
            case: "one asset, no configuration",
            configs: Vec::new(),
            assets: 1,
        },
        Row {
            case: "three assets, no configuration",
            configs: Vec::new(),
            assets: 3,
        },
    ]
}

/// Populate a fresh engine with `row`'s group, configurations and assets.
async fn populate(row: &Row) -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    create_group(&router, GROUP).await;
    for (id, format) in &row.configs {
        create_config(&router, id, GROUP, format).await;
    }
    for index in 0..row.assets {
        create_asset(&router, &asset_id(index), GROUP).await;
    }
    (engine, router)
}

/// How many configurations and assets the store holds, and how many tag entries.
fn stored_counts(engine: &Arc<engine::InMemoryEngine>) -> (usize, usize, usize) {
    let store = engine.state::<vod::ComAmazonawsMediapackagevodMediapackagevodState>();
    let state = store.lock();
    (
        state.packaging_configurations.len(),
        state.assets.len(),
        state.tags_by_arn.len(),
    )
}

/// The message a refused delete carries for `row`: the per-member line this service
/// composes, naming the dependency class the caller has to clear first. Assets are
/// reported first, in the order a caller has to remove them.
fn expected_refusal_message(row: &Row) -> &'static str {
    if row.assets > 0 {
        "id cannot be deleted because it has attached assets"
    } else {
        "id cannot be deleted because it has attached packaging configurations"
    }
}

#[tokio::test]
async fn a_group_a_child_resource_references_cannot_be_deleted() {
    for row in rows() {
        let (engine, router) = populate(&row).await;
        let case = row.case;
        let before = describe_group(&router, GROUP).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: setup DescribePackagingGroup: expected 200, got {} body {}",
            before.status, before.body
        );
        let counts_before = stored_counts(&engine);

        // Twice: a refusal that had a partial effect would answer differently the
        // second time.
        for attempt in 1..=2 {
            let refused = delete_group(&router, GROUP).await;
            assert_eq!(
                (refused.status, refused.errortype.as_str()),
                (UNPROCESSABLE.1, UNPROCESSABLE.0),
                "case {case:?} attempt {attempt}: DeletePackagingGroup with {} \
                 configuration(s) and {} asset(s) still stored: expected HTTP {} / {}, \
                 got HTTP {} / {:?} body {}",
                row.configs.len(),
                row.assets,
                UNPROCESSABLE.1,
                UNPROCESSABLE.0,
                refused.status,
                refused.errortype,
                refused.body
            );
            assert_eq!(
                refused.body["message"].as_str(),
                Some(expected_refusal_message(&row)),
                "case {case:?} attempt {attempt}: expected the refusal to name the \
                 dependency class, got {}",
                refused.body
            );
        }

        // NOTHING changed: the group reads back exactly as before (ARN, CreatedAt,
        // tags, derived counts included), and the store holds every child it held,
        // each still naming the group.
        let after = describe_group(&router, GROUP).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: DescribePackagingGroup after the refused delete: expected 200, \
             got {} body {}",
            after.status, after.body
        );
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused delete must not change the group, before {} after {}",
            before.body, after.body
        );
        for index in 0..row.assets {
            let id = asset_id(index);
            let asset = call(&router, "GET", &format!("/assets/{id}"), None).await;
            assert_eq!(
                (asset.status, asset.body["packagingGroupId"].as_str()),
                (200, Some(GROUP)),
                "case {case:?}: DescribeAsset {id:?} after the refused delete must still \
                 name the group, got HTTP {} body {}",
                asset.status,
                asset.body
            );
        }
        let counts_after = stored_counts(&engine);
        assert_eq!(
            counts_after, counts_before,
            "case {case:?}: a refused delete must store exactly what it found \
             (configurations, assets, tag entries), before {counts_before:?} after \
             {counts_after:?}"
        );
    }
}

#[tokio::test]
async fn the_identical_delete_succeeds_once_the_last_dependent_is_gone() {
    for row in rows() {
        let (engine, router) = populate(&row).await;
        let case = row.case;

        // Each dependent removed in turn: while ANY is still stored the delete is
        // refused, so the protection is about the set of children being non-empty, not
        // about a particular child or class. Each child carries its CLASS, so the
        // teardown reaches the right delete even for an asset and a configuration
        // sharing an id.
        let mut remaining = row.assets + row.configs.len();
        let children: Vec<(bool, String)> = (0..row.assets)
            .map(|index| (true, asset_id(index)))
            .chain(
                row.configs
                    .iter()
                    .map(|(id, _)| (false, (*id).to_string())),
            )
            .collect();
        for (is_asset, child) in &children {
            let refused = delete_group(&router, GROUP).await;
            assert_eq!(
                (refused.status, refused.errortype.as_str()),
                (UNPROCESSABLE.1, UNPROCESSABLE.0),
                "case {case:?}: with {remaining} dependent(s) left, DeletePackagingGroup: \
                 expected HTTP {} / {}, got HTTP {} / {:?} body {}",
                UNPROCESSABLE.1,
                UNPROCESSABLE.0,
                refused.status,
                refused.errortype,
                refused.body
            );
            if *is_asset {
                delete_asset(&router, child).await;
            } else {
                delete_config(&router, child).await;
            }
            remaining -= 1;
        }

        let accepted = delete_group(&router, GROUP).await;
        assert_eq!(
            accepted.status, DELETE_SUCCESS,
            "case {case:?}: with no dependent left, DeletePackagingGroup: expected \
             {DELETE_SUCCESS}, got {} body {}",
            accepted.status, accepted.body
        );

        let gone = describe_group(&router, GROUP).await;
        assert_eq!(
            (gone.status, gone.errortype.as_str()),
            (NOT_FOUND.1, NOT_FOUND.0),
            "case {case:?}: DescribePackagingGroup after the accepted delete: expected \
             HTTP {} / {}, got HTTP {} / {:?} body {}",
            NOT_FOUND.1,
            NOT_FOUND.0,
            gone.status,
            gone.errortype,
            gone.body
        );
        let again = delete_group(&router, GROUP).await;
        assert_eq!(
            (again.status, again.errortype.as_str()),
            (NOT_FOUND.1, NOT_FOUND.0),
            "case {case:?}: a second DeletePackagingGroup: expected HTTP {} / {}, got \
             HTTP {} / {:?} body {}",
            NOT_FOUND.1,
            NOT_FOUND.0,
            again.status,
            again.errortype,
            again.body
        );

        // The accepted delete forgot the group's tags, so a group later created with
        // the same id inherits none — and every child was removed by its own delete,
        // never cascaded by the group's.
        let (configs, assets, _) = stored_counts(&engine);
        assert_eq!(
            configs, 0,
            "case {case:?}: every dependent was deleted before the group, {configs} \
             configuration(s) left"
        );
        assert_eq!(
            assets, 0,
            "case {case:?}: every dependent was deleted before the group, {assets} \
             asset(s) left"
        );
        let recreated = call(
            &router,
            "POST",
            "/packaging_groups",
            Some(json!({ "id": GROUP })),
        )
        .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the id must be free again, got {} body {}",
            recreated.status, recreated.body
        );
        assert_eq!(
            recreated.body["tags"],
            json!({}),
            "case {case:?}: a re-created group must not inherit the deleted group's tags, \
             got {}",
            recreated.body["tags"]
        );
    }
}

#[tokio::test]
async fn only_children_of_the_addressed_group_protect_it() {
    // (case, how many assets ANOTHER group holds — the group under test holds no child
    // of its own in any row, while that other group holds one configuration of every
    // modelled format plus this many assets)
    let cases: [(&str, usize); 3] = [
        ("another group with configurations only", 0),
        ("another group with configurations and one asset", 1),
        ("another group with configurations and three assets", 3),
    ];

    for (case, assets) in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(engine.clone());
        create_group(&router, GROUP).await;
        create_group(&router, "other-group").await;
        // The other group's children name THEIR group, so they must not protect this
        // one whatever their class, format or count.
        for (index, format) in ["hlsPackage", "dashPackage", "cmafPackage", "mssPackage"]
            .into_iter()
            .enumerate()
        {
            create_config(&router, &format!("other-cfg-{index}"), "other-group", format).await;
        }
        for index in 0..assets {
            create_asset(&router, &asset_id(index), "other-group").await;
        }

        let accepted = delete_group(&router, GROUP).await;
        assert_eq!(
            accepted.status, DELETE_SUCCESS,
            "case {case:?}: a group no child of its own references must delete: expected \
             {DELETE_SUCCESS}, got {} body {}",
            accepted.status, accepted.body
        );
        let gone = describe_group(&router, GROUP).await;
        assert_eq!(
            gone.status, NOT_FOUND.1,
            "case {case:?}: DescribePackagingGroup after the delete: expected {}, got {} \
             body {}",
            NOT_FOUND.1, gone.status, gone.body
        );
        // The other group and its children are untouched.
        let other = describe_group(&router, "other-group").await;
        assert_eq!(
            other.status, 200,
            "case {case:?}: the other group must be untouched, got {} body {}",
            other.status, other.body
        );
        let (configs, stored_assets, _) = stored_counts(&engine);
        assert_eq!(
            (configs, stored_assets),
            (4, assets),
            "case {case:?}: the other group's four configurations and {assets} asset(s) \
             must still be stored, got {configs} and {stored_assets}"
        );
    }
}

#[tokio::test]
async fn an_id_no_group_is_stored_under_is_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    create_group(&router, GROUP).await;
    create_config(&router, "cfg-hls", GROUP, "hlsPackage").await;

    // The ids differ in form — never created, a suffixed near-miss of the stored one,
    // and the same id in another case or with another separator — because a missing
    // ADDRESSED resource is 404 whatever the id looks like, and the dependency refusal
    // must not shadow it even while a configuration is stored.
    for id in [
        "absent-group",
        "protected-group-",
        "PROTECTED-GROUP",
        "protected_group",
        "cfg-hls",
    ] {
        let answer = delete_group(&router, id).await;
        assert_eq!(
            (answer.status, answer.errortype.as_str()),
            (NOT_FOUND.1, NOT_FOUND.0),
            "case {id:?}: DeletePackagingGroup for an id no group is stored under: \
             expected HTTP {} / {}, got HTTP {} / {:?} body {}",
            NOT_FOUND.1,
            NOT_FOUND.0,
            answer.status,
            answer.errortype,
            answer.body
        );
    }

    // None of those refusals touched the stored group or its configuration.
    let still = describe_group(&router, GROUP).await;
    assert_eq!(
        still.status, 200,
        "the stored group must survive every 404, got {} body {}",
        still.status, still.body
    );
    let (configs, _, _) = stored_counts(&engine);
    assert_eq!(
        configs, 1,
        "the stored configuration must survive every 404, {configs} left"
    );
}
