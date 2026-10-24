// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListLayers`: the layers an account can enumerate in a Region,
//! the newest version each one still holds, the two compatibility filters and the
//! pagination walk.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than any handler's internals. Everything a
//! listing reports is checked against what `PublishLayerVersion` persisted (and what
//! `DeleteLayerVersion` took back out), so no suite trusts the listing alone.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region a suite that is not about Regions signs every request for.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// Where `ListLayers` is bound (`smithy_query`: `GET /2018-10-31/layers`).
const LAYERS_URI: &str = "/2018-10-31/layers";

/// The largest `MaxItems` the model admits (`MaxLayerListItems`, `@range(min: 1, max:
/// 50)`).
const MAX_ITEMS_MAX: usize = 50;

/// A SigV4 credential scope naming `lambda` in `region` — the router picks a service by
/// its service field and the handler reads the Region from it. Neither the key id nor
/// the signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one JSON request signed for `region`.
async fn json_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let host = format!("lambda.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Layer archive bytes of a given length. The content is irrelevant to the API — only
/// its length and its digest are reported — but it differs per length so two archives
/// never share a digest.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `Content.ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// A `Content` member holding a small inline archive, for a version published only to
/// exist.
fn inline_content(length: usize) -> Value {
    json!({ "ZipFile": zip_file(&archive_bytes(length)) })
}

/// `PublishLayerVersion` in `region`, answering the version it stored. Panics unless the
/// publish succeeded, because every assertion below is about what it stored.
async fn publish(router: &MultiServiceRouter, region: &str, name: &str, body: &Value) -> Value {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    let answer = json_call(router, region, "POST", &uri, body).await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: expected 201 from PublishLayerVersion, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// `DeleteLayerVersion` in `region`, which answers 204.
async fn delete_version(router: &MultiServiceRouter, region: &str, name: &str, version: i64) {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    let answer = json_call(router, region, "DELETE", &uri, &Value::Null).await;
    assert_eq!(
        answer.status,
        204,
        "case {name:?}:{version}: expected 204 from DeleteLayerVersion, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `ListLayers` in `region` with a query string (`""` for none).
async fn list_layers(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    json_call(
        router,
        region,
        "GET",
        &format!("{LAYERS_URI}{query}"),
        &Value::Null,
    )
    .await
}

/// The `Layers` a successful `ListLayers` reported. Panics unless the call answered 200
/// with the member present.
async fn listed_layers(router: &MultiServiceRouter, region: &str, query: &str) -> Vec<Value> {
    let answer = list_layers(router, region, query).await;
    assert_eq!(
        answer.status,
        200,
        "case {query:?} in {region}: expected 200 from ListLayers, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    body.get("Layers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {query:?}: expected a Layers array, got {}", answer.text()))
}

/// The `LayerName`s a successful `ListLayers` reported, in the order it reported them.
async fn listed_names(router: &MultiServiceRouter, region: &str, query: &str) -> Vec<String> {
    listed_layers(router, region, query)
        .await
        .iter()
        .map(|item| {
            item.get("LayerName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {query:?}: a listed layer must carry a LayerName, got {item:?}"))
                .to_string()
        })
        .collect()
}

/// The one listed entry for `name`, or `None` when the listing does not hold it.
async fn listed_layer(
    router: &MultiServiceRouter,
    region: &str,
    query: &str,
    name: &str,
) -> Option<Value> {
    listed_layers(router, region, query)
        .await
        .into_iter()
        .find(|item| item.get("LayerName").and_then(Value::as_str) == Some(name))
}

/// The ARN a layer of `name` published in `region` must carry.
fn layer_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:layer:{name}")
}

/// Assert one member of a listed entry: `None` means the member must be absent from the
/// answer altogether, not present and empty.
fn assert_member(case: &str, body: &Value, member: &str, expected: Option<Value>) {
    let actual = body.get(member);
    match expected {
        None => assert!(
            actual.is_none(),
            "case {case:?}: expected no {member} member, got {actual:?}"
        ),
        Some(value) => assert_eq!(
            actual,
            Some(&value),
            "case {case:?}: expected {member} {value:?}, got {actual:?}"
        ),
    }
}

/// One row of the listing table: a layer, the versions to publish under it, and the
/// metadata the newest of them must therefore be reported with.
struct ListCase {
    /// The case name, which is also the layer name.
    name: &'static str,
    /// One `PublishLayerVersion` body per version, oldest first.
    versions: Vec<Value>,
    /// The `LatestMatchingVersion.Description` the listing must report.
    description: Option<Value>,
    /// The `LatestMatchingVersion.LicenseInfo` the listing must report.
    license_info: Option<Value>,
    /// The `LatestMatchingVersion.CompatibleRuntimes` the listing must report.
    compatible_runtimes: Option<Value>,
    /// The `LatestMatchingVersion.CompatibleArchitectures` the listing must report.
    compatible_architectures: Option<Value>,
}

/// `ListLayers` reports one entry per layer the store holds, carrying that layer's own
/// ARNs and the metadata of the newest version it holds.
///
/// The rows differ along what the rule generalises over: how many versions a layer holds
/// (one, two, three), which optional members the newest of them carries (none, each of
/// them, the published maximum of 15 compatible runtimes and both architectures), and
/// whether the versions BELOW the newest carry members the newest does not — the entry
/// is the newest version's own metadata, never a merge of the layer's history. The
/// runtime and architecture values are entries of the model's own `Runtime` and
/// `Architecture` enums (`smithy_query(query="shape",shape="Runtime,Architecture")`).
#[tokio::test]
async fn list_layers_reports_the_newest_version_of_each_layer() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Fifteen entries: the `CompatibleRuntimes` length maximum the model publishes
    // (`@length(min: 0, max: 15)`), spelled with values from the `Runtime` enum.
    let fifteen_runtimes = json!([
        "nodejs18.x",
        "nodejs20.x",
        "nodejs22.x",
        "python3.9",
        "python3.10",
        "python3.11",
        "python3.12",
        "python3.13",
        "java11",
        "java17",
        "java21",
        "ruby3.2",
        "ruby3.3",
        "dotnet8",
        "provided.al2023",
    ]);

    let cases = vec![
        // The plain case: one version and no optional member at all.
        ListCase {
            name: "single-bare",
            versions: vec![json!({ "Content": inline_content(32) })],
            description: None,
            license_info: None,
            compatible_runtimes: None,
            compatible_architectures: None,
        },
        // Three versions, the newest carrying every optional member.
        ListCase {
            name: "three-described",
            versions: vec![
                json!({ "Content": inline_content(64) }),
                json!({ "Content": inline_content(96) }),
                json!({
                    "Content": inline_content(128),
                    "Description": "shared helpers",
                    "LicenseInfo": "MIT",
                    "CompatibleRuntimes": ["python3.12"],
                    "CompatibleArchitectures": ["arm64"],
                }),
            ],
            description: Some(json!("shared helpers")),
            license_info: Some(json!("MIT")),
            compatible_runtimes: Some(json!(["python3.12"])),
            compatible_architectures: Some(json!(["arm64"])),
        },
        // The newest version carries nothing while the one below it carries everything:
        // the entry reports the newest version's own metadata, so none of those members
        // appears.
        ListCase {
            name: "newest-bare",
            versions: vec![
                json!({
                    "Content": inline_content(160),
                    "Description": "the version below",
                    "LicenseInfo": "Apache-2.0",
                    "CompatibleRuntimes": ["nodejs20.x"],
                    "CompatibleArchitectures": ["x86_64"],
                }),
                json!({ "Content": inline_content(192) }),
            ],
            description: None,
            license_info: None,
            compatible_runtimes: None,
            compatible_architectures: None,
        },
        // The list bounds: fifteen compatible runtimes and both architectures, with the
        // license given as the URL form the member's documentation names.
        ListCase {
            name: "every-runtime",
            versions: vec![json!({
                "Content": inline_content(224),
                "Description": "",
                "LicenseInfo": "https://opensource.org/license/mit",
                "CompatibleRuntimes": fifteen_runtimes.clone(),
                "CompatibleArchitectures": ["arm64", "x86_64"],
            })],
            description: Some(json!("")),
            license_info: Some(json!("https://opensource.org/license/mit")),
            compatible_runtimes: Some(fifteen_runtimes.clone()),
            compatible_architectures: Some(json!(["arm64", "x86_64"])),
        },
    ];

    // The `CreatedDate` of the newest version of each layer, as the publish that stored
    // it reported — so the listing is checked against stored state rather than against
    // a value this test made up.
    let mut created: Vec<(&str, String, i64)> = Vec::new();
    for case in &cases {
        let mut last: Option<Value> = None;
        for body in &case.versions {
            last = Some(publish(&router, REGION, case.name, body).await);
        }
        let published = last.expect("every case publishes at least one version");
        let created_date = published
            .get("CreatedDate")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: the publish must report a CreatedDate", case.name))
            .to_string();
        let version = published
            .get("Version")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: the publish must report a Version", case.name));
        created.push((case.name, created_date, version));
    }

    let listed = listed_layers(&router, REGION, "").await;
    assert_eq!(
        listed.len(),
        cases.len(),
        "expected one entry per layer ({}), got {listed:?}",
        cases.len()
    );

    // The entries are reported in layer-name order, so a caller paging them sees a
    // stable sequence.
    let names = listed_names(&router, REGION, "").await;
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "expected the layers in name order, got {names:?}"
    );

    for (case, (name, created_date, version)) in cases.iter().zip(created.iter()) {
        assert_eq!(case.name, *name, "the case order must match the publish order");
        let item = listed_layer(&router, REGION, "", case.name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: expected the layer to be listed, got {listed:?}", case.name));

        assert_member(case.name, &item, "LayerName", Some(json!(case.name)));
        assert_member(
            case.name,
            &item,
            "LayerArn",
            Some(json!(layer_arn(REGION, case.name))),
        );

        let latest = item
            .get("LatestMatchingVersion")
            .unwrap_or_else(|| panic!("case {:?}: expected a LatestMatchingVersion, got {item:?}", case.name));

        // The newest version is the LAST one published, so its number is the count of
        // the versions the case published.
        let expected_version = i64::try_from(case.versions.len()).unwrap_or_default();
        assert_eq!(
            *version, expected_version,
            "case {:?}: the publishes must have numbered the newest version {expected_version}, got {version}",
            case.name
        );
        assert_member(case.name, latest, "Version", Some(json!(expected_version)));
        assert_member(
            case.name,
            latest,
            "LayerVersionArn",
            Some(json!(format!(
                "{}:{expected_version}",
                layer_arn(REGION, case.name)
            ))),
        );
        assert_member(
            case.name,
            latest,
            "CreatedDate",
            Some(json!(created_date.clone())),
        );
        assert_member(case.name, latest, "Description", case.description.clone());
        assert_member(case.name, latest, "LicenseInfo", case.license_info.clone());
        assert_member(
            case.name,
            latest,
            "CompatibleRuntimes",
            case.compatible_runtimes.clone(),
        );
        assert_member(
            case.name,
            latest,
            "CompatibleArchitectures",
            case.compatible_architectures.clone(),
        );
    }
}

/// Both compatibility filters are applied to a layer's NEWEST version alone: a layer
/// whose newest version does not declare the runtime (or architecture) asked for is not
/// listed, however many of its older versions do.
///
/// The rows differ along what the rule generalises over: which of the two filters a
/// request names (neither, each alone, both), the value asked for (several `Runtime` and
/// both `Architecture` entries, including one no layer here declares), and whether the
/// declaration sits on the newest version, on an older one, or nowhere at all.
#[tokio::test]
async fn list_layers_filters_on_the_newest_version_only() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Each layer's versions, oldest first.
    let layers: Vec<(&str, Vec<Value>)> = vec![
        // The newest version declares python; an older one declares nodejs.
        (
            "newest-python",
            vec![
                json!({ "Content": inline_content(32), "CompatibleRuntimes": ["nodejs20.x"] }),
                json!({ "Content": inline_content(48), "CompatibleRuntimes": ["python3.12"] }),
            ],
        ),
        // The mirror image: the newest declares nodejs, an older one python.
        (
            "newest-nodejs",
            vec![
                json!({ "Content": inline_content(64), "CompatibleRuntimes": ["python3.12"] }),
                json!({ "Content": inline_content(80), "CompatibleRuntimes": ["nodejs20.x"] }),
            ],
        ),
        // Declares no compatibility at all, so no filtered request matches it.
        ("declares-nothing", vec![json!({ "Content": inline_content(96) })]),
        // One runtime and one architecture each, so an architecture filter separates
        // two otherwise identical layers.
        (
            "python-arm64",
            vec![json!({
                "Content": inline_content(112),
                "CompatibleRuntimes": ["python3.12"],
                "CompatibleArchitectures": ["arm64"],
            })],
        ),
        (
            "python-x86",
            vec![json!({
                "Content": inline_content(128),
                "CompatibleRuntimes": ["python3.12"],
                "CompatibleArchitectures": ["x86_64"],
            })],
        ),
        // Both architectures and a third runtime, so it matches either architecture.
        (
            "ruby-both-arches",
            vec![json!({
                "Content": inline_content(144),
                "CompatibleRuntimes": ["ruby3.3"],
                "CompatibleArchitectures": ["arm64", "x86_64"],
            })],
        ),
    ];
    for (name, versions) in &layers {
        for body in versions {
            publish(&router, REGION, name, body).await;
        }
    }

    // (query, the layers it must report)
    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "",
            vec![
                "declares-nothing",
                "newest-nodejs",
                "newest-python",
                "python-arm64",
                "python-x86",
                "ruby-both-arches",
            ],
        ),
        (
            "?CompatibleRuntime=python3.12",
            vec!["newest-python", "python-arm64", "python-x86"],
        ),
        ("?CompatibleRuntime=nodejs20.x", vec!["newest-nodejs"]),
        ("?CompatibleRuntime=ruby3.3", vec!["ruby-both-arches"]),
        // A `Runtime` entry no layer here declares at all.
        ("?CompatibleRuntime=java21", vec![]),
        (
            "?CompatibleArchitecture=arm64",
            vec!["python-arm64", "ruby-both-arches"],
        ),
        (
            "?CompatibleArchitecture=x86_64",
            vec!["python-x86", "ruby-both-arches"],
        ),
        (
            "?CompatibleRuntime=python3.12&CompatibleArchitecture=arm64",
            vec!["python-arm64"],
        ),
        (
            "?CompatibleRuntime=ruby3.3&CompatibleArchitecture=x86_64",
            vec!["ruby-both-arches"],
        ),
        // Both filters named, each matching a different layer: no layer matches both.
        (
            "?CompatibleRuntime=nodejs20.x&CompatibleArchitecture=arm64",
            vec![],
        ),
    ];

    for (query, expected) in cases {
        let names = listed_names(&router, REGION, query).await;
        assert_eq!(
            names, expected,
            "case {query:?}: expected {expected:?}, got {names:?}"
        );
    }

    // The entry a filtered request reports still carries the layer's newest version,
    // which is the version the filter matched.
    let item = listed_layer(&router, REGION, "?CompatibleRuntime=python3.12", "newest-python")
        .await
        .expect("the filtered listing must report newest-python");
    let latest = item
        .get("LatestMatchingVersion")
        .expect("the filtered entry must carry a LatestMatchingVersion");
    assert_member("newest-python filtered", latest, "Version", Some(json!(2)));
    assert_member(
        "newest-python filtered",
        latest,
        "CompatibleRuntimes",
        Some(json!(["python3.12"])),
    );
}

/// One row of the deletion table: the version to delete, and the newest version each
/// layer must report once it is gone.
struct DeleteCase {
    /// The layer the version is deleted from, which is also the case name.
    layer: &'static str,
    /// The version to delete, or `None` for the row that asserts the listing before
    /// anything has been deleted.
    version: Option<i64>,
    /// The newest version the listing must then report per layer; `None` means the
    /// layer must not be listed at all.
    newest: Vec<(&'static str, Option<i64>)>,
}

/// Deleting a layer's newest version leaves the highest version the store still holds as
/// the one the listing reports, and a layer the store holds no version of at all is not
/// listed.
///
/// The rows differ along which version goes: the newest one (repeatedly, down to the
/// last), one in the middle (which leaves the newest alone), and every version of a
/// layer (which takes the layer out of the listing). A layer nothing is deleted from is
/// asserted unchanged throughout, so the fallback is not a listing that forgot a layer.
#[tokio::test]
async fn list_layers_falls_back_when_the_newest_version_is_deleted() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for name in ["falls-back", "middle-gap", "emptied"] {
        for size in [32_usize, 48, 64] {
            publish(
                &router,
                REGION,
                name,
                &json!({ "Content": inline_content(size) }),
            )
            .await;
        }
    }
    publish(
        &router,
        REGION,
        "untouched",
        &json!({ "Content": inline_content(80) }),
    )
    .await;

    /// The newest version the listing reports for `name`, or `None` when the layer is
    /// not listed at all.
    async fn newest_listed(router: &MultiServiceRouter, name: &str) -> Option<i64> {
        let item = listed_layer(router, REGION, "", name).await?;
        item.get("LatestMatchingVersion")
            .and_then(|latest| latest.get("Version"))
            .and_then(Value::as_i64)
    }

    let cases = vec![
        // Nothing deleted yet: every layer reports the highest version published.
        DeleteCase {
            layer: "before any delete",
            version: None,
            newest: vec![
                ("falls-back", Some(3)),
                ("middle-gap", Some(3)),
                ("emptied", Some(3)),
                ("untouched", Some(1)),
            ],
        },
        // The newest version of one layer goes: the one below it becomes the newest.
        DeleteCase {
            layer: "falls-back",
            version: Some(3),
            newest: vec![
                ("falls-back", Some(2)),
                ("middle-gap", Some(3)),
                ("emptied", Some(3)),
                ("untouched", Some(1)),
            ],
        },
        // And again, down to the last version the layer holds.
        DeleteCase {
            layer: "falls-back",
            version: Some(2),
            newest: vec![
                ("falls-back", Some(1)),
                ("middle-gap", Some(3)),
                ("emptied", Some(3)),
                ("untouched", Some(1)),
            ],
        },
        // A version in the middle goes: the newest is unaffected.
        DeleteCase {
            layer: "middle-gap",
            version: Some(2),
            newest: vec![
                ("falls-back", Some(1)),
                ("middle-gap", Some(3)),
                ("emptied", Some(3)),
                ("untouched", Some(1)),
            ],
        },
        // Every version of a layer goes, newest first, until the layer is no longer
        // listed at all.
        DeleteCase {
            layer: "emptied",
            version: Some(3),
            newest: vec![
                ("falls-back", Some(1)),
                ("middle-gap", Some(3)),
                ("emptied", Some(2)),
                ("untouched", Some(1)),
            ],
        },
        DeleteCase {
            layer: "emptied",
            version: Some(2),
            newest: vec![
                ("falls-back", Some(1)),
                ("middle-gap", Some(3)),
                ("emptied", Some(1)),
                ("untouched", Some(1)),
            ],
        },
        DeleteCase {
            layer: "emptied",
            version: Some(1),
            newest: vec![
                ("falls-back", Some(1)),
                ("middle-gap", Some(3)),
                ("emptied", None),
                ("untouched", Some(1)),
            ],
        },
        // The last version of the layer that fell back goes too.
        DeleteCase {
            layer: "falls-back",
            version: Some(1),
            newest: vec![
                ("falls-back", None),
                ("middle-gap", Some(3)),
                ("emptied", None),
                ("untouched", Some(1)),
            ],
        },
    ];

    for case in cases {
        if let Some(version) = case.version {
            delete_version(&router, REGION, case.layer, version).await;
        }
        for (name, newest) in case.newest {
            let actual = newest_listed(&router, name).await;
            assert_eq!(
                actual, newest,
                "case delete {:?}:{:?}: expected {name} to report newest version {newest:?}, \
                 got {actual:?}",
                case.layer, case.version
            );
        }
    }
}

/// A paged walk reports every layer exactly once, in name order, and the marker of the
/// last page is absent.
///
/// The rows differ along the page size: one layer at a time, several sizes that do not
/// divide the total, each published `MaxItems` bound (1 and 50), and a size larger than
/// the number of layers. A size outside the published range is refused rather than
/// clamped.
#[tokio::test]
async fn list_layers_pages_through_every_layer_in_name_order() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // `layer-1` is a prefix of `layer-10`, so the names do not come back in publish
    // order: the listing is ordered by name, which is what a marker continues after.
    let names = ["layer-1", "layer-10", "layer-2", "layer-3", "layer-21"];
    for name in names {
        publish(
            &router,
            REGION,
            name,
            &json!({ "Content": inline_content(16 + name.len()) }),
        )
        .await;
    }
    let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    expected.sort();

    for page_size in [1_usize, 2, 3, 5, MAX_ITEMS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &marker {
                Some(marker) => format!("?MaxItems={page_size}&Marker={marker}"),
                None => format!("?MaxItems={page_size}"),
            };
            let answer = list_layers(&router, REGION, &query).await;
            assert_eq!(
                answer.status,
                200,
                "case page size {page_size}: expected 200, got {} {}",
                answer.status,
                answer.text()
            );
            let body = answer.json();
            let page = body
                .get("Layers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("case page size {page_size}: expected a Layers array, got {}", answer.text()));
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page holds at most {page_size} layers, got {}",
                page.len()
            );
            for item in &page {
                let name = item
                    .get("LayerName")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case page size {page_size}: a listed layer must carry a LayerName, got {item:?}"));
                seen.push(name.to_string());
            }
            match body.get("NextMarker").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case page size {page_size}: a marker was returned for an empty page"
                    );
                    marker = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every layer once in name order, got {seen:?}"
        );
    }

    // `MaxItems` is modelled `@range(min: 1, max: 50)`: a page size outside it is
    // refused rather than paged over.
    for (case, max_items) in [("zero", "0"), ("negative", "-1"), ("past the maximum", "51")] {
        let answer = list_layers(&router, REGION, &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {case}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// A layer belongs to the Region it was published in: a listing reports the layers of
/// the Region the call was signed for and no other, and each entry's ARNs carry that
/// Region.
///
/// The rows differ along the Region: four of them, in three partitions' worth of name
/// shapes, each publishing its own layer, and every listing is checked against every
/// Region's layers rather than only its own.
#[tokio::test]
async fn a_layer_is_invisible_from_another_region() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // (the Region the layer is published in, the layer's name)
    let cases = [
        ("us-east-1", "layer-use1"),
        ("us-west-2", "layer-usw2"),
        ("eu-central-1", "layer-euc1"),
        ("ap-southeast-2", "layer-apse2"),
    ];
    for (region, name) in cases {
        publish(
            &router,
            region,
            name,
            &json!({
                "Content": inline_content(32 + name.len()),
                "CompatibleRuntimes": ["python3.12"],
            }),
        )
        .await;
    }

    for (region, own) in cases {
        let names = listed_names(&router, region, "").await;
        assert_eq!(
            names,
            vec![own.to_string()],
            "case {region}: expected only {own:?} to be listed, got {names:?}"
        );

        let item = listed_layer(&router, region, "", own)
            .await
            .unwrap_or_else(|| panic!("case {region}: expected {own:?} to be listed"));
        assert_member(region, &item, "LayerArn", Some(json!(layer_arn(region, own))));
        let latest = item
            .get("LatestMatchingVersion")
            .unwrap_or_else(|| panic!("case {region}: expected a LatestMatchingVersion, got {item:?}"));
        assert_member(
            region,
            latest,
            "LayerVersionArn",
            Some(json!(format!("{}:1", layer_arn(region, own)))),
        );

        // A filter does not make another Region's layer visible either, even though
        // every layer here declares the runtime asked for.
        let filtered = listed_names(&router, region, "?CompatibleRuntime=python3.12").await;
        assert_eq!(
            filtered,
            vec![own.to_string()],
            "case {region}: expected only {own:?} under a runtime filter, got {filtered:?}"
        );
    }
}
