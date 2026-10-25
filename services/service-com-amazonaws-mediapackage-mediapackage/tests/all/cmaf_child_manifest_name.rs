// SPDX-License-Identifier: CC0-1.0

//! The name of a CMAF child HLS manifest.
//!
//! RULE under test: a child `cmafPackage.hlsManifests[]` entry that sends no
//! `manifestName` is stored with the OriginEndpoint's own EFFECTIVE `manifestName` — the
//! one the endpoint reports, which is the value its request sent or, when it sent none,
//! the endpoint-level default — and its derived playback `url` names that same value; a
//! child that sends a `manifestName` keeps exactly that. Source: the modelled doc of
//! `HlsManifestCreateOrUpdateParameters$ManifestName`, "An optional short string appended
//! to the end of the OriginEndpoint URL. If not specified, defaults to the manifestName
//! for the OriginEndpoint"
//! (`smithy_query(query="shape",shape="HlsManifestCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`).
//!
//! The rows span what the rule generalises over: whether the ENDPOINT sends a
//! `manifestName` (so the inherited value is the sent one or the endpoint's own default),
//! whether the CHILD sends one, how many children the package carries and in what mix,
//! and which write path stored the record (`CreateOriginEndpoint` or
//! `UpdateOriginEndpoint`). Every case is read back through `DescribeOriginEndpoint` and
//! `ListOriginEndpoints`, so the value is pinned as PERSISTED and not as a serialization
//! artifact of the writing call.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "cmaf-channel";

/// A fresh emulator holding only this service, with request authentication off (this
/// suite exercises behaviour, not signing).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` whose credential scope routes the request to `mediapackage`
/// in [`REGION`]; the signature is never verified here.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackage/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One request/response round trip: the HTTP status and the parsed JSON body (`Null` for
/// an empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://mediapackage.{REGION}.amazonaws.com{uri}"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

/// One case: the endpoint-level `manifestName` the request sends (`None` for a request
/// that omits it), the `hlsManifests` list it sends, and the name each stored child must
/// carry, in order.
struct Case {
    name: &'static str,
    endpoint_manifest_name: Option<&'static str>,
    children: Value,
    expected: Vec<&'static str>,
}

/// The cases, one row per point of the class the rule generalises over.
fn cases() -> Vec<Case> {
    vec![
        // The combination the rule is about: the endpoint names a manifest and the child
        // does not, so the child inherits the endpoint's name.
        Case {
            name: "endpoint names a manifest, one unnamed child",
            endpoint_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            expected: vec!["stream"],
        },
        // A DIFFERENT endpoint-level name, so what is inherited is the value and not one
        // particular string.
        Case {
            name: "a different endpoint manifest name, one unnamed child",
            endpoint_manifest_name: Some("replay"),
            children: json!([{ "id": "a" }]),
            expected: vec!["replay"],
        },
        // The endpoint sends none, so it takes the endpoint-level default and the child
        // inherits THAT — the only way the service-wide default reaches a child.
        Case {
            name: "endpoint names no manifest, one unnamed child",
            endpoint_manifest_name: None,
            children: json!([{ "id": "a" }]),
            expected: vec![state::DEFAULT_MANIFEST_NAME],
        },
        // A child with a name of its own keeps it, whatever the endpoint is called.
        Case {
            name: "endpoint names a manifest, one named child",
            endpoint_manifest_name: Some("stream"),
            children: json!([{ "id": "a", "manifestName": "master" }]),
            expected: vec!["master"],
        },
        Case {
            name: "endpoint names no manifest, one named child",
            endpoint_manifest_name: None,
            children: json!([{ "id": "a", "manifestName": "master" }]),
            expected: vec!["master"],
        },
        // A child whose own name HAPPENS to be the endpoint-level default is a supplied
        // value, not an inherited one: it survives an endpoint named something else.
        Case {
            name: "child named for the endpoint-level default",
            endpoint_manifest_name: Some("stream"),
            children: json!([{ "id": "a", "manifestName": state::DEFAULT_MANIFEST_NAME }]),
            expected: vec![state::DEFAULT_MANIFEST_NAME],
        },
        // Several children, a mix of named and unnamed: the rule is per child.
        Case {
            name: "three children, named and unnamed mixed",
            endpoint_manifest_name: Some("stream"),
            children: json!([
                { "id": "a" },
                { "id": "b", "manifestName": "master" },
                { "id": "c" }
            ]),
            expected: vec!["stream", "master", "stream"],
        },
        Case {
            name: "two children, both unnamed, endpoint unnamed too",
            endpoint_manifest_name: None,
            children: json!([{ "id": "a" }, { "id": "b" }]),
            expected: vec![state::DEFAULT_MANIFEST_NAME, state::DEFAULT_MANIFEST_NAME],
        },
    ]
}

/// Assert `endpoint` reports `expected` as the name of each stored child, in order, and
/// that each child's derived `url` names the same value.
fn assert_children(case: &str, projection: &str, expected: &[&str], endpoint: &Value) {
    let children = endpoint
        .pointer("/cmafPackage/hlsManifests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        children.len(),
        expected.len(),
        "case {case:?}: {projection} must carry {} child manifests, got {endpoint}",
        expected.len()
    );
    for (child, want) in children.iter().zip(expected) {
        let actual = child.get("manifestName").and_then(Value::as_str);
        assert_eq!(
            actual,
            Some(*want),
            "case {case:?}: {projection} child {child} must report manifestName {want:?}, \
             got {actual:?}"
        );
        let url = child.get("url").and_then(Value::as_str).unwrap_or("");
        let suffix = format!("/{want}/index.m3u8");
        assert!(
            url.ends_with(&suffix),
            "case {case:?}: {projection} child {child} must carry a url ending in \
             {suffix:?}, got {url:?}"
        );
    }
}

/// Read `id` back through `DescribeOriginEndpoint` and `ListOriginEndpoints` and assert
/// both render the same stored record `written` did, children included.
async fn assert_reads_agree(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected: &[&str],
    written: &Value,
) {
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described}"
    );
    assert_children(case, "DescribeOriginEndpoint", expected, &described);
    assert_eq!(
        &described, written,
        "case {case:?}: DescribeOriginEndpoint must render the same stored record the write \
         returned"
    );

    let (status, listed) = call(
        router,
        "GET",
        &format!("/origin_endpoints?channelId={CHANNEL}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListOriginEndpoints expected 200, got {status} {listed}"
    );
    let entry = listed
        .get("originEndpoints")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
                .cloned()
        })
        .unwrap_or(Value::Null);
    assert_children(case, "the ListOriginEndpoints entry", expected, &entry);
}

#[tokio::test]
async fn a_created_cmaf_child_manifest_inherits_the_endpoint_manifest_name() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "creating the fixture channel expected 200, got {status} {body}"
    );

    for (index, case) in cases().into_iter().enumerate() {
        let id = format!("created-{index}");
        let mut request = json!({
            "channelId": CHANNEL,
            "id": id,
            "cmafPackage": { "hlsManifests": case.children }
        });
        if let Some(name) = case.endpoint_manifest_name {
            request["manifestName"] = json!(name);
        }

        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&request)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateOriginEndpoint expected 200, got {status} {created}",
            case.name
        );
        // The endpoint's own effective name is what the children inherit, so pin it too:
        // every expected child name is either the child's own or this one.
        let effective = case
            .endpoint_manifest_name
            .unwrap_or(state::DEFAULT_MANIFEST_NAME);
        assert_eq!(
            created.get("manifestName").and_then(Value::as_str),
            Some(effective),
            "case {:?}: the endpoint must report manifestName {effective:?}, got {created}",
            case.name
        );
        assert_children(case.name, "the create response", &case.expected, &created);
        assert_reads_agree(&router, case.name, &id, &case.expected, &created).await;
    }
}

#[tokio::test]
async fn an_updated_cmaf_child_manifest_inherits_the_endpoint_manifest_name() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "creating the fixture channel expected 200, got {status} {body}"
    );

    for (index, case) in cases().into_iter().enumerate() {
        let id = format!("updated-{index}");
        // Created with an HLS package and an endpoint name the update does NOT keep, so
        // the children the update materialises can only inherit the UPDATE's value.
        let create = json!({
            "channelId": CHANNEL,
            "id": id,
            "manifestName": "before",
            "hlsPackage": {}
        });
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&create)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture CreateOriginEndpoint expected 200, got {status} {created}",
            case.name
        );

        let mut request = json!({
            "id": id,
            "cmafPackage": { "hlsManifests": case.children }
        });
        if let Some(name) = case.endpoint_manifest_name {
            request["manifestName"] = json!(name);
        }
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateOriginEndpoint expected 200, got {status} {updated}",
            case.name
        );
        let effective = case
            .endpoint_manifest_name
            .unwrap_or(state::DEFAULT_MANIFEST_NAME);
        assert_eq!(
            updated.get("manifestName").and_then(Value::as_str),
            Some(effective),
            "case {:?}: the updated endpoint must report manifestName {effective:?}, \
             got {updated}",
            case.name
        );
        assert_children(case.name, "the update response", &case.expected, &updated);
        assert_reads_agree(&router, case.name, &id, &case.expected, &updated).await;
    }
}
