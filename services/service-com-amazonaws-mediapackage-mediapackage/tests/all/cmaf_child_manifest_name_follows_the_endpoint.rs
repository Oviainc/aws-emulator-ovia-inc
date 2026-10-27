// SPDX-License-Identifier: CC0-1.0

//! What a CMAF child HLS manifest is called AFTER the endpoint it belongs to is updated.
//!
//! RULE under test: the members of a stored CMAF child manifest that the SERVICE derives
//! — the `manifestName` a child sent without one of its own takes, and the playback `url`
//! built from whichever name the child carries — are derived from the endpoint's
//! EFFECTIVE `manifestName`, so every write of the endpoint re-derives them from the
//! parameters the package was stored with: a child that sent no name reports whatever the
//! endpoint reports after the write (the name that write sent, or the endpoint-level
//! default when it sent none), and a child that sent a name of its own keeps exactly that.
//! Source: the modelled doc of `HlsManifest$ManifestName` — the shape projected back on
//! every read — and of `HlsManifestCreateOrUpdateParameters$ManifestName`, "An optional
//! short string appended to the end of the OriginEndpoint URL. If not specified, defaults
//! to the manifestName for the OriginEndpoint"
//! (`smithy_query(query="doc",shape="HlsManifest",member="ManifestName",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`).
//!
//! `tests/all/cmaf_child_manifest_name.rs` pins the rule for the write that SENDS the
//! package; these rows span the other axis — what the update sends about the endpoint's
//! own name (a new one, the same one, none at all), whether the child's name was sent or
//! inherited (including a child whose sent name happens to equal the endpoint's old name
//! or the endpoint-level default, which is a sent value and not an inherited one), how
//! many children the package carries, and whether the update resends a packaging
//! configuration at all. Every case is read back through `DescribeOriginEndpoint` and
//! `ListOriginEndpoints`, so the value is pinned as PERSISTED.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "cmaf-update-channel";

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

/// One case: how the endpoint was created, what the update sends about it, and what the
/// stored endpoint and its children must report afterwards.
struct Case {
    name: &'static str,
    /// The endpoint-level `manifestName` the CREATE sends (`None` for a create that omits
    /// it, so the endpoint takes the endpoint-level default).
    created_manifest_name: Option<&'static str>,
    /// The `cmafPackage.hlsManifests` the create sends.
    children: Value,
    /// The update body, without the `id` the route carries.
    update: Value,
    /// The effective endpoint `manifestName` after the update.
    expected_endpoint: &'static str,
    /// The name each stored child must carry after the update, in order.
    expected_children: Vec<&'static str>,
}

/// The cases, one row per point of the class the rule generalises over.
fn cases() -> Vec<Case> {
    vec![
        // An update that renames the endpoint and resends no packaging: the child that
        // sent no name of its own is the endpoint's manifest name, so it moves with it.
        Case {
            name: "rename, one inherited child",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            update: json!({ "manifestName": "replay" }),
            expected_endpoint: "replay",
            expected_children: vec!["replay"],
        },
        // A DIFFERENT new name, so what the child follows is the value and not one
        // particular string.
        Case {
            name: "rename to another value, one inherited child",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            update: json!({ "manifestName": "archive" }),
            expected_endpoint: "archive",
            expected_children: vec!["archive"],
        },
        // An update that sends no `manifestName` at all resets the endpoint to the
        // endpoint-level default, and an inherited child follows it there too.
        Case {
            name: "update omits the name, one inherited child",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            update: json!({ "description": "configuration only" }),
            expected_endpoint: state::DEFAULT_MANIFEST_NAME,
            expected_children: vec![state::DEFAULT_MANIFEST_NAME],
        },
        // The endpoint keeps its name: nothing the children are derived from moved.
        Case {
            name: "update resends the same name",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            update: json!({ "manifestName": "stream" }),
            expected_endpoint: "stream",
            expected_children: vec!["stream"],
        },
        // A child that sent a name of its own keeps it: it inherits nothing.
        Case {
            name: "rename, one child with its own name",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a", "manifestName": "master" }]),
            update: json!({ "manifestName": "replay" }),
            expected_endpoint: "replay",
            expected_children: vec!["master"],
        },
        // A child whose SENT name happens to equal the endpoint's name is still a sent
        // value, so the rename leaves it where the caller put it.
        Case {
            name: "child that sent the endpoint's old name keeps it across a rename",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a", "manifestName": "stream" }]),
            update: json!({ "manifestName": "replay" }),
            expected_endpoint: "replay",
            expected_children: vec!["stream"],
        },
        // The same distinction against the endpoint-level default: a child that sent it
        // is not a child that inherited it.
        Case {
            name: "child that sent the endpoint-level default keeps it across a rename",
            created_manifest_name: None,
            children: json!([{ "id": "a", "manifestName": state::DEFAULT_MANIFEST_NAME }]),
            update: json!({ "manifestName": "replay" }),
            expected_endpoint: "replay",
            expected_children: vec![state::DEFAULT_MANIFEST_NAME],
        },
        // Several children, named and unnamed mixed: the rule is per child.
        Case {
            name: "three children, named and inherited mixed, renamed",
            created_manifest_name: Some("stream"),
            children: json!([
                { "id": "a" },
                { "id": "b", "manifestName": "master" },
                { "id": "c" }
            ]),
            update: json!({ "manifestName": "replay" }),
            expected_endpoint: "replay",
            expected_children: vec!["replay", "master", "replay"],
        },
        // An update that sends neither a name nor a packaging configuration, over several
        // children: only the inherited ones move, and they move to the default.
        Case {
            name: "two children, named and inherited, update omits the name",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }, { "id": "b", "manifestName": "master" }]),
            update: json!({ "description": "configuration only" }),
            expected_endpoint: state::DEFAULT_MANIFEST_NAME,
            expected_children: vec![state::DEFAULT_MANIFEST_NAME, "master"],
        },
        // The endpoint was created unnamed, so the children started on the default and
        // follow the name the update gives the endpoint.
        Case {
            name: "endpoint created unnamed, update names it",
            created_manifest_name: None,
            children: json!([{ "id": "a" }]),
            update: json!({ "manifestName": "replay" }),
            expected_endpoint: "replay",
            expected_children: vec!["replay"],
        },
        // The other write path: an update that RESENDS the packaging configuration stores
        // those parameters and derives the new children from the new name.
        Case {
            name: "update resends the packaging and a new name",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            update: json!({
                "manifestName": "replay",
                "cmafPackage": { "hlsManifests": [
                    { "id": "b" },
                    { "id": "c", "manifestName": "master" }
                ] }
            }),
            expected_endpoint: "replay",
            expected_children: vec!["replay", "master"],
        },
        // And the same resend WITHOUT a name: the resent package's inherited children take
        // the endpoint-level default the update reset the endpoint to.
        Case {
            name: "update resends the packaging and omits the name",
            created_manifest_name: Some("stream"),
            children: json!([{ "id": "a" }]),
            update: json!({ "cmafPackage": { "hlsManifests": [{ "id": "a" }] } }),
            expected_endpoint: state::DEFAULT_MANIFEST_NAME,
            expected_children: vec![state::DEFAULT_MANIFEST_NAME],
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

/// Assert the endpoint's OWN effective name and the playback URL derived from it, which
/// is what an inherited child has to agree with.
fn assert_endpoint_name(case: &str, projection: &str, expected: &str, endpoint: &Value) {
    let actual = endpoint.get("manifestName").and_then(Value::as_str);
    assert_eq!(
        actual,
        Some(expected),
        "case {case:?}: {projection} must report manifestName {expected:?}, got {actual:?}"
    );
    let url = endpoint.get("url").and_then(Value::as_str).unwrap_or("");
    // A CMAF endpoint's playback URL ends in the HLS master playlist under its manifest
    // name (`state::PackagingKind::manifest_path`).
    let suffix = format!("/{expected}/index.m3u8");
    assert!(
        url.ends_with(&suffix),
        "case {case:?}: {projection} must carry a url ending in {suffix:?}, got {url:?}"
    );
}

/// Read `id` back through `DescribeOriginEndpoint` and `ListOriginEndpoints` and assert
/// both render the same stored record `written` did, children included.
async fn assert_reads_agree(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected_endpoint: &str,
    expected_children: &[&str],
    written: &Value,
) {
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described}"
    );
    assert_endpoint_name(case, "DescribeOriginEndpoint", expected_endpoint, &described);
    assert_children(case, "DescribeOriginEndpoint", expected_children, &described);
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
    assert_endpoint_name(
        case,
        "the ListOriginEndpoints entry",
        expected_endpoint,
        &entry,
    );
    assert_children(
        case,
        "the ListOriginEndpoints entry",
        expected_children,
        &entry,
    );
}

#[tokio::test]
async fn an_updated_endpoint_re_derives_its_cmaf_child_manifest_names() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "creating the fixture channel expected 200, got {status} {body}"
    );

    for (index, case) in cases().into_iter().enumerate() {
        let id = format!("endpoint-{index}");
        let mut create = json!({
            "channelId": CHANNEL,
            "id": id,
            "cmafPackage": { "hlsManifests": case.children }
        });
        if let Some(name) = case.created_manifest_name {
            create["manifestName"] = json!(name);
        }
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&create)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture CreateOriginEndpoint expected 200, got {status} {created}",
            case.name
        );

        let mut request = case.update.clone();
        request["id"] = json!(id);
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
        assert_endpoint_name(
            case.name,
            "the update response",
            case.expected_endpoint,
            &updated,
        );
        assert_children(
            case.name,
            "the update response",
            &case.expected_children,
            &updated,
        );
        assert_reads_agree(
            &router,
            case.name,
            &id,
            case.expected_endpoint,
            &case.expected_children,
            &updated,
        )
        .await;

        // Re-deriving the children is a function of the stored parameters and the
        // endpoint's effective name, so sending the SAME update again is a no-op: the
        // record cannot drift by being written twice.
        let (status, again) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the repeated UpdateOriginEndpoint expected 200, got {status} {again}",
            case.name
        );
        assert_eq!(
            again, updated,
            "case {:?}: repeating the same update must render the same stored record",
            case.name
        );
    }
}

#[tokio::test]
async fn an_endpoint_repackaged_away_from_cmaf_keeps_no_child_manifests() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "creating the fixture channel expected 200, got {status} {body}"
    );

    let id = "repackaged";
    let create = json!({
        "channelId": CHANNEL,
        "id": id,
        "manifestName": "stream",
        "cmafPackage": { "hlsManifests": [{ "id": "a" }] }
    });
    let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&create)).await;
    assert_eq!(
        status, 200,
        "the fixture CreateOriginEndpoint expected 200, got {status} {created}"
    );
    assert_children("repackaged", "the create response", &["stream"], &created);

    // An endpoint packages in exactly one format, so an update that names another one
    // replaces the CMAF package outright.
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/origin_endpoints/{id}"),
        Some(&json!({ "id": id, "manifestName": "mid", "hlsPackage": {} })),
    )
    .await;
    assert_eq!(
        status, 200,
        "repackaging to HLS expected 200, got {status} {updated}"
    );
    assert!(
        updated.get("cmafPackage").is_none(),
        "the repackaged endpoint must carry no cmafPackage, got {updated}"
    );

    // A later rename re-derives what the endpoint stores NOW, so it cannot resurrect the
    // children of the package the endpoint no longer carries.
    let (status, renamed) = call(
        &router,
        "PUT",
        &format!("/origin_endpoints/{id}"),
        Some(&json!({ "id": id, "manifestName": "after", "hlsPackage": {} })),
    )
    .await;
    assert_eq!(status, 200, "renaming expected 200, got {status} {renamed}");
    assert!(
        renamed.get("cmafPackage").is_none(),
        "the renamed endpoint must still carry no cmafPackage, got {renamed}"
    );
    let url = renamed.get("url").and_then(Value::as_str).unwrap_or("");
    // An HLS endpoint's playback URL ends in the master playlist named for its manifest
    // name (`state::PackagingKind::manifest_path`).
    assert!(
        url.ends_with("/after.m3u8"),
        "the renamed HLS endpoint must carry a url ending in \"/after.m3u8\", got {url:?}"
    );

    let (status, described) = call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeOriginEndpoint expected 200, got {status} {described}"
    );
    assert_eq!(
        described, renamed,
        "DescribeOriginEndpoint must render the same stored record the update returned"
    );
}
