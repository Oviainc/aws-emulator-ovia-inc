// SPDX-License-Identifier: CC0-1.0

//! Every URL MediaPackage derives for one OriginEndpoint sits under ONE playback path.
//!
//! RULE under test: the playback path of an OriginEndpoint is the egress host for this
//! account and Region — the account's own label, then `mediapackage.<region>.amazonaws.com`,
//! the same host the channel's ingest URLs name — followed by `/out/v1/<endpoint label>`;
//! the endpoint's own `url` is that path plus the manifest its packaging type serves, and
//! the `url` of every child HLS manifest inside its `cmafPackage` is that SAME path plus
//! the child's own label and name. So an endpoint and its children can never be served
//! from different hosts or different playback paths.
//!
//! This is the behaviour both former copies of the derivation implemented, now stated
//! once in `state::media_hostname` / `state::playback_url_prefix`; the test drives it
//! through an operation reaching each former copy: `CreateChannel` (the ingest host),
//! and `CreateOriginEndpoint`, whose response carries both the endpoint's own `url` and,
//! for a CMAF endpoint, its children's. Every case is read back through
//! `DescribeOriginEndpoint`, so the URLs are pinned as PERSISTED.
//!
//! The rows span what the rule generalises over: the packaging type (and so the manifest
//! tail), the Region, the channel and endpoint ids, whether the request names a
//! `manifestName`, and how many child manifests a CMAF package carries and which of them
//! are named.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

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
/// in `region`; the signature is never verified here.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/mediapackage/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One request/response round trip in `region`: the HTTP status and the parsed JSON body
/// (`Null` for an empty body).
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://mediapackage.{region}.amazonaws.com{uri}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

/// One case: the Region, channel and endpoint it writes, the endpoint-level
/// `manifestName` it sends (`None` when it omits it), the single packaging member it
/// sends, and the tail the endpoint's own `url` must end in after its effective manifest
/// name.
struct Case {
    name: &'static str,
    region: &'static str,
    channel: &'static str,
    endpoint: &'static str,
    manifest_name: Option<&'static str>,
    package_member: &'static str,
    package: Value,
    /// Appended to the effective manifest name to form the last path segment(s). The
    /// manifest formats each packager emits, per `state::PackagingKind::manifest_path`:
    /// HLS and CMAF an `.m3u8` playlist, DASH an `.mpd`, Microsoft Smooth an
    /// `.ism/Manifest`.
    manifest_suffix: &'static str,
}

/// The cases, one row per point of the class the rule generalises over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "hls, no manifestName",
            region: "us-west-2",
            channel: "hls-channel",
            endpoint: "hls-default",
            manifest_name: None,
            package_member: "hlsPackage",
            package: json!({}),
            manifest_suffix: ".m3u8",
        },
        Case {
            name: "hls, a manifestName of its own",
            region: "us-west-2",
            channel: "hls-named-channel",
            endpoint: "hls-named",
            manifest_name: Some("stream"),
            package_member: "hlsPackage",
            package: json!({}),
            manifest_suffix: ".m3u8",
        },
        Case {
            name: "dash, a manifestName of its own",
            region: "us-west-2",
            channel: "dash-channel",
            endpoint: "dash-named",
            manifest_name: Some("live"),
            package_member: "dashPackage",
            package: json!({}),
            manifest_suffix: ".mpd",
        },
        Case {
            name: "microsoft smooth, no manifestName",
            region: "us-west-2",
            channel: "mss-channel",
            endpoint: "mss-default",
            manifest_name: None,
            package_member: "mssPackage",
            package: json!({}),
            manifest_suffix: ".ism/Manifest",
        },
        Case {
            name: "cmaf, one unnamed child",
            region: "us-west-2",
            channel: "cmaf-channel",
            endpoint: "cmaf-one-child",
            manifest_name: Some("stream"),
            package_member: "cmafPackage",
            package: json!({ "hlsManifests": [{ "id": "a" }] }),
            manifest_suffix: "/index.m3u8",
        },
        Case {
            name: "cmaf, three children named and unnamed, endpoint unnamed",
            region: "us-west-2",
            channel: "cmaf-mixed-channel",
            endpoint: "cmaf-mixed",
            manifest_name: None,
            package_member: "cmafPackage",
            package: json!({ "hlsManifests": [
                { "id": "a" },
                { "id": "b", "manifestName": "master" },
                { "id": "c" }
            ] }),
            manifest_suffix: "/index.m3u8",
        },
        // Another Region: the host and the endpoint label both change with it, and the
        // endpoint's URLs still sit under one path with its children's.
        Case {
            name: "cmaf in another region",
            region: "eu-west-1",
            channel: "eu-cmaf-channel",
            endpoint: "eu-cmaf",
            manifest_name: Some("replay"),
            package_member: "cmafPackage",
            package: json!({ "hlsManifests": [{ "id": "a" }, { "id": "b" }] }),
            manifest_suffix: "/index.m3u8",
        },
        Case {
            name: "dash in another region, with a packaging setting",
            region: "eu-west-1",
            channel: "eu-dash-channel",
            endpoint: "eu-dash",
            manifest_name: None,
            package_member: "dashPackage",
            package: json!({ "segmentDurationSeconds": 4 }),
            manifest_suffix: ".mpd",
        },
    ]
}

/// Whether `label` is one of this service's opaque path labels: 32 lowercase hex
/// characters (`state::stable_label`).
fn is_label(label: &str) -> bool {
    label.len() == 32 && label.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// The host of an `https://host/path` URL, or `""` when `url` is not one.
fn host_of(url: &str) -> &str {
    url.strip_prefix("https://")
        .map(|rest| rest.split('/').next().unwrap_or(""))
        .unwrap_or("")
}

/// The egress host every ingest URL of the channel `created` names, asserted to be the
/// account-and-Region host form and the same across the redundant ingest endpoints.
fn ingest_host(case: &str, region: &str, created: &Value) -> String {
    let endpoints = created
        .pointer("/hlsIngest/ingestEndpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !endpoints.is_empty(),
        "case {case:?}: the created channel must carry ingest endpoints, got {created}"
    );
    let mut hosts = endpoints.iter().map(|endpoint| {
        let url = endpoint.get("url").and_then(Value::as_str).unwrap_or("");
        assert!(
            url.contains("/in/v2/"),
            "case {case:?}: expected an ingest url under /in/v2/, got {url:?}"
        );
        host_of(url).to_string()
    });
    let host = hosts.next().expect("a first ingest endpoint");
    for other in hosts {
        assert_eq!(
            other, host,
            "case {case:?}: every ingest url must name one host, got {other:?} beside {host:?}"
        );
    }
    let (label, domain) = host.split_once('.').unwrap_or((host.as_str(), ""));
    let expected_domain = format!("mediapackage.{region}.amazonaws.com");
    assert_eq!(
        domain, expected_domain,
        "case {case:?}: expected the host domain {expected_domain:?}, got {host:?}"
    );
    assert!(
        is_label(label),
        "case {case:?}: expected a 32-hex account label in the host, got {host:?}"
    );
    host
}

/// Assert `endpoint`'s own `url` ends in the manifest `case` expects and return the
/// playback path it sits under — everything before that last manifest tail.
fn playback_prefix(case: &Case, projection: &str, endpoint: &Value) -> String {
    let effective = endpoint
        .get("manifestName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expected_name = case.manifest_name.unwrap_or(state::DEFAULT_MANIFEST_NAME);
    assert_eq!(
        effective, expected_name,
        "case {:?}: {projection} must report manifestName {expected_name:?}, got {endpoint}",
        case.name
    );
    let url = endpoint.get("url").and_then(Value::as_str).unwrap_or("");
    let tail = format!("/{effective}{}", case.manifest_suffix);
    assert!(
        url.ends_with(&tail),
        "case {:?}: {projection} must carry a url ending in {tail:?}, got {url:?}",
        case.name
    );
    url[..url.len() - tail.len()].to_string()
}

/// Assert `prefix` is the playback path form — `https://<host>/out/v1/<32-hex label>` on
/// the host `host` — and nothing else.
fn assert_prefix_form(case: &str, host: &str, prefix: &str) {
    let expected_start = format!("https://{host}/out/v1/");
    let label = prefix.strip_prefix(&expected_start).unwrap_or_default();
    assert!(
        !label.is_empty() && is_label(label),
        "case {case:?}: expected a playback path {expected_start:?} plus a 32-hex endpoint \
         label, got {prefix:?}"
    );
}

/// Assert every child HLS manifest of `endpoint`'s CMAF package sits under `prefix`, with
/// its own label and the name it reports.
fn assert_children_share_prefix(case: &Case, projection: &str, prefix: &str, endpoint: &Value) {
    let sent = case
        .package
        .get("hlsManifests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let stored = endpoint
        .pointer("/cmafPackage/hlsManifests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        stored.len(),
        sent.len(),
        "case {:?}: {projection} must carry {} child manifests, got {endpoint}",
        case.name,
        sent.len()
    );
    for child in &stored {
        let name = child
            .get("manifestName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = child.get("url").and_then(Value::as_str).unwrap_or("");
        let under = format!("{prefix}/");
        let tail = format!("/{name}/index.m3u8");
        assert!(
            url.starts_with(&under) && url.ends_with(&tail),
            "case {:?}: {projection} child {child} must carry a url under {under:?} ending in \
             {tail:?}, got {url:?}",
            case.name
        );
        let label = &url[under.len()..url.len() - tail.len()];
        assert!(
            is_label(label),
            "case {:?}: {projection} child {child} must sit under a 32-hex manifest label, \
             got {label:?} in {url:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_endpoint_and_its_cmaf_children_share_one_playback_path() {
    let router = emulator();

    for case in cases() {
        let (status, channel) = call(
            &router,
            case.region,
            "POST",
            "/channels",
            Some(&json!({ "id": case.channel })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateChannel expected 200, got {status} {channel}",
            case.name
        );
        let host = ingest_host(case.name, case.region, &channel);

        let mut request = json!({
            "channelId": case.channel,
            "id": case.endpoint,
        });
        request[case.package_member] = case.package.clone();
        if let Some(name) = case.manifest_name {
            request["manifestName"] = json!(name);
        }
        let (status, created) = call(
            &router,
            case.region,
            "POST",
            "/origin_endpoints",
            Some(&request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateOriginEndpoint expected 200, got {status} {created}",
            case.name
        );

        // The endpoint's own url: the playback path for this account, Region and endpoint,
        // then its manifest — and that path is on the host the ingest urls named.
        let prefix = playback_prefix(&case, "the create response", &created);
        assert_prefix_form(case.name, &host, &prefix);
        // Each CMAF child's url: the SAME playback path, then the child's own label and
        // name. (Non-CMAF packages carry no children, so this asserts there are none.)
        assert_children_share_prefix(&case, "the create response", &prefix, &created);

        // Persisted, not an artifact of the writing call: the same bytes come back on a
        // read, so the read derives the one prefix too.
        let (status, described) = call(
            &router,
            case.region,
            "GET",
            &format!("/origin_endpoints/{}", case.endpoint),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeOriginEndpoint expected 200, got {status} {described}",
            case.name
        );
        let read_prefix = playback_prefix(&case, "DescribeOriginEndpoint", &described);
        assert_eq!(
            read_prefix, prefix,
            "case {:?}: DescribeOriginEndpoint must report the playback path the create \
             returned, got {read_prefix:?}",
            case.name
        );
        assert_prefix_form(case.name, &host, &read_prefix);
        assert_children_share_prefix(&case, "DescribeOriginEndpoint", &read_prefix, &described);
    }
}
