// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the member set one `ListLayers` entry carries.
//!
//! The rule under test is that a listing entry is a projection of STORED state and
//! nothing else: the layer's two ARNs and the newest version the store still holds, with
//! that version reported exactly as it was published — a member the version carries no
//! value for is left out of the entry rather than filled in with an empty value, which is
//! how `ListLayerVersions` reports the same version too. The suite exists as a guard: a
//! wider projection (an empty description, an empty licence or an empty compatibility
//! list written into every entry) diverges from the recorded responses this behaviour was
//! pinned against, so it must not be reintroduced without a recording that asks for it.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body); everything an entry reports is checked against what
//! `PublishLayerVersion` persisted, so no suite trusts the listing alone.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
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

/// Dispatch one JSON request, the shape every operation here uses.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
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

/// Layer archive bytes of a given length.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `Content` member holding a small inline archive, for a version published only to
/// exist. restJson1 carries a blob base64-encoded in a JSON body.
fn inline_content(length: usize) -> Value {
    json!({
        "ZipFile": base64::engine::general_purpose::STANDARD.encode(archive_bytes(length)),
    })
}

/// `PublishLayerVersion` on a layer name, answering the version it stored.
async fn publish(router: &MultiServiceRouter, name: &str, body: &Value) -> Value {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    let answer = json_call(router, "POST", &uri, body).await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: the publish must answer HTTP 201, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// The `ListLayers` entries a query reports.
async fn listed(router: &MultiServiceRouter, query: &str) -> Vec<Value> {
    let answer = json_call(
        router,
        "GET",
        &format!("/2018-10-31/layers{query}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "query {query:?}: ListLayers must answer HTTP 200, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("Layers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The entry a query reports for one layer.
async fn listed_layer(router: &MultiServiceRouter, query: &str, name: &str) -> Option<Value> {
    listed(router, query)
        .await
        .into_iter()
        .find(|entry| entry.get("LayerName").and_then(Value::as_str) == Some(name))
}

/// The `ListLayerVersions` entry reported for one version of a layer.
async fn listed_version(router: &MultiServiceRouter, name: &str, version: i64) -> Option<Value> {
    let answer = json_call(
        router,
        "GET",
        &format!("/2018-10-31/layers/{name}/versions"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {name:?}: ListLayerVersions must answer HTTP 200, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("LayerVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry.get("Version").and_then(Value::as_i64) == Some(version))
}

/// The member names an object carries, for a member-set assertion.
fn members(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// The member names `expected` lists, as a set.
fn member_set(expected: &[&str]) -> BTreeSet<String> {
    expected
        .iter()
        .map(|member| (*member).to_string())
        .collect()
}

/// Assert one member of `object` is present and equal to `expected`.
fn assert_member(case: &str, object: &Value, member: &str, expected: &Value) {
    let actual = object.get(member).unwrap_or_else(|| {
        panic!("case {case:?}: expected the entry to carry {member}, got {object:?}")
    });
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {member} {expected:?}, got {actual:?}"
    );
}

/// One row of the entry table: a layer, the versions to publish under it, and the member
/// names the entry's newest version must therefore carry.
struct Case {
    /// The case name, which is also the layer name.
    name: &'static str,
    /// The `PublishLayerVersion` bodies, oldest version first.
    versions: Vec<Value>,
    /// The members `LatestMatchingVersion` must carry, beside the three every version
    /// summary carries (`LayerVersionArn`, `Version`, `CreatedDate`).
    optional_members: Vec<&'static str>,
}

/// Every `ListLayers` entry carries the layer's ARNs and the newest version the store
/// holds, with that version reported exactly as published.
#[tokio::test]
async fn a_list_layers_entry_carries_the_stored_members_and_no_others() {
    // One row per form the rule generalises over: which optional members the newest
    // version carries (none, a description, a licence, a compatibility list, all of
    // them), and how many versions the layer holds — the entry is the NEWEST version's
    // projection, never a merge of the layer's history.
    let cases = vec![
        Case {
            name: "bare",
            versions: vec![json!({ "Content": inline_content(32) })],
            optional_members: vec![],
        },
        Case {
            name: "described",
            versions: vec![json!({
                "Content": inline_content(48),
                "Description": "shared helpers",
            })],
            optional_members: vec!["Description"],
        },
        Case {
            name: "licensed",
            versions: vec![json!({ "Content": inline_content(64), "LicenseInfo": "MIT" })],
            optional_members: vec!["LicenseInfo"],
        },
        Case {
            name: "runtime-only",
            versions: vec![json!({
                "Content": inline_content(72),
                "CompatibleRuntimes": ["python3.12"],
            })],
            optional_members: vec!["CompatibleRuntimes"],
        },
        Case {
            name: "everything",
            versions: vec![json!({
                "Content": inline_content(80),
                "Description": "parsers",
                "LicenseInfo": "https://opensource.org/license/mit",
                "CompatibleRuntimes": ["python3.12", "nodejs20.x"],
                "CompatibleArchitectures": ["arm64", "x86_64"],
            })],
            optional_members: vec![
                "Description",
                "LicenseInfo",
                "CompatibleRuntimes",
                "CompatibleArchitectures",
            ],
        },
        Case {
            // Three versions where only the OLDEST carries the optional members: the
            // entry shows the newest version, so none of them appears.
            name: "newest-bare",
            versions: vec![
                json!({
                    "Content": inline_content(96),
                    "Description": "the version below",
                    "LicenseInfo": "Apache-2.0",
                    "CompatibleRuntimes": ["ruby3.3"],
                }),
                json!({ "Content": inline_content(112) }),
                json!({ "Content": inline_content(128) }),
            ],
            optional_members: vec![],
        },
    ];

    let router = router();
    for case in &cases {
        let mut newest = Value::Null;
        for body in &case.versions {
            newest = publish(&router, case.name, body).await;
        }

        let entry = listed_layer(&router, "", case.name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: expected the layer to be listed", case.name));

        assert_member(case.name, &entry, "LayerName", &json!(case.name));
        assert_member(
            case.name,
            &entry,
            "LayerArn",
            &json!(format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{}", case.name)),
        );
        assert_eq!(
            members(&entry),
            member_set(&["LatestMatchingVersion", "LayerArn", "LayerName"]),
            "case {:?}: unexpected entry member set, got {entry:?}",
            case.name
        );

        let latest = entry.get("LatestMatchingVersion").unwrap_or_else(|| {
            panic!("case {:?}: expected a LatestMatchingVersion, got {entry:?}", case.name)
        });

        // Everything the shown version reports comes out of what the publish stored.
        let version = newest
            .get("Version")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: the publish must report a Version", case.name));
        assert_member(case.name, latest, "Version", &json!(version));
        assert_member(
            case.name,
            latest,
            "LayerVersionArn",
            &json!(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{}:{version}",
                case.name
            )),
        );
        assert_member(
            case.name,
            latest,
            "CreatedDate",
            newest.get("CreatedDate").unwrap_or_else(|| {
                panic!("case {:?}: the publish must report a CreatedDate", case.name)
            }),
        );

        // Only the members the version itself carries, with the value the publish
        // supplied: nothing is defaulted into the entry.
        let mut expected_members = member_set(&["CreatedDate", "LayerVersionArn", "Version"]);
        for member in &case.optional_members {
            expected_members.insert((*member).to_string());
            let supplied = case
                .versions
                .last()
                .and_then(|body| body.get(member).cloned())
                .unwrap_or_else(|| {
                    panic!("case {:?}: the newest publish must supply {member}", case.name)
                });
            assert_member(case.name, latest, member, &supplied);
        }
        assert_eq!(
            members(latest),
            expected_members,
            "case {:?}: unexpected LatestMatchingVersion member set, got {latest:?}",
            case.name
        );

        // The VERSION listing reports the same version through the same projection, so a
        // client reading either listing sees one member set for one version.
        let listed = listed_version(&router, case.name, version)
            .await
            .unwrap_or_else(|| panic!("case {:?}: expected version {version} to be listed", case.name));
        assert_eq!(
            members(&listed),
            expected_members,
            "case {:?}: the version listing must report the same members, got {listed:?}",
            case.name
        );
    }
}

/// A filtered listing reports the same entry projection as an unfiltered one: the member
/// set is a property of the stored version, not of the request.
#[tokio::test]
async fn a_filtered_entry_carries_the_same_member_set() {
    let router = router();
    publish(
        &router,
        "filtered",
        &json!({
            "Content": inline_content(64),
            "CompatibleRuntimes": ["python3.12"],
            "CompatibleArchitectures": ["arm64"],
        }),
    )
    .await;

    for query in [
        "",
        "?CompatibleRuntime=python3.12",
        "?CompatibleArchitecture=arm64",
        "?CompatibleRuntime=python3.12&CompatibleArchitecture=arm64",
    ] {
        let entry = listed_layer(&router, query, "filtered")
            .await
            .unwrap_or_else(|| panic!("case {query:?}: expected the layer to be listed"));
        assert_eq!(
            members(&entry),
            member_set(&["LatestMatchingVersion", "LayerArn", "LayerName"]),
            "case {query:?}: unexpected entry member set, got {entry:?}"
        );
        let latest = entry
            .get("LatestMatchingVersion")
            .unwrap_or_else(|| panic!("case {query:?}: expected a LatestMatchingVersion, got {entry:?}"));
        assert_eq!(
            members(latest),
            member_set(&[
                "CompatibleArchitectures",
                "CompatibleRuntimes",
                "CreatedDate",
                "LayerVersionArn",
                "Version",
            ]),
            "case {query:?}: unexpected LatestMatchingVersion member set, got {latest:?}"
        );
    }
}

/// Deleting the newest version leaves the entry reporting the version below it, with that
/// version's own members and no others.
#[tokio::test]
async fn the_entry_follows_the_newest_version_the_store_still_holds() {
    let router = router();
    publish(
        &router,
        "rolled-back",
        &json!({
            "Content": inline_content(32),
            "Description": "first",
            "LicenseInfo": "MIT",
        }),
    )
    .await;
    publish(&router, "rolled-back", &json!({ "Content": inline_content(48) })).await;

    let before = listed_layer(&router, "", "rolled-back")
        .await
        .expect("the layer must be listed while it holds two versions");
    let latest = before
        .get("LatestMatchingVersion")
        .expect("the entry must carry a LatestMatchingVersion");
    assert_member("two versions", latest, "Version", &json!(2));
    assert_eq!(
        members(latest),
        member_set(&["CreatedDate", "LayerVersionArn", "Version"]),
        "case \"two versions\": the newest version carries no optional member, got {latest:?}"
    );

    let deleted = json_call(
        &router,
        "DELETE",
        "/2018-10-31/layers/rolled-back/versions/2",
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "the delete must answer HTTP 204, got {} {}",
        deleted.status,
        deleted.text()
    );

    let after = listed_layer(&router, "", "rolled-back")
        .await
        .expect("the layer must still be listed while it holds version 1");
    let fallen_back = after
        .get("LatestMatchingVersion")
        .expect("the entry must carry a LatestMatchingVersion");
    assert_member("one version", fallen_back, "Version", &json!(1));
    assert_member("one version", fallen_back, "Description", &json!("first"));
    assert_member("one version", fallen_back, "LicenseInfo", &json!("MIT"));
    assert_eq!(
        members(fallen_back),
        member_set(&[
            "CreatedDate",
            "Description",
            "LayerVersionArn",
            "LicenseInfo",
            "Version",
        ]),
        "case \"one version\": unexpected LatestMatchingVersion member set, got {fallen_back:?}"
    );
}
