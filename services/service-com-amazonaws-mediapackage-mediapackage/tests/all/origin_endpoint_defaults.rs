// SPDX-License-Identifier: CC0-1.0

//! The server-filled members of an origin endpoint.
//!
//! RULE under test: `CreateOriginEndpoint` stores a service default for every optional
//! origin-endpoint configuration member the request leaves unset, never overwrites a
//! member the request did supply, and every projection of the endpoint — the create
//! response, `DescribeOriginEndpoint` and each `ListOriginEndpoints` entry — renders that
//! one stored record, so all three carry the same member set.
//!
//! The defaults are keyed by MEMBER, so the same member takes the same default wherever it
//! appears (`HlsPackage`, a CMAF child `HlsManifest`, `DashPackage`); the table below is
//! the one in `crate::state`, with its sources. Every case is driven as real HTTP through
//! the `MultiServiceRouter`, the same dispatch the served emulator uses.
//!
//! The rows span the class the rule generalises over: which packaging configuration the
//! endpoint carries (HLS, DASH, CMAF, MSS), how many of the server-filled members the
//! request supplies (none, some, all), the form of a supplied list (empty versus
//! populated) and the number of CMAF child manifests.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_mediapackage_mediapackage::ops::types::shared_types as types;
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for.
const REGION: &str = "eu-west-1";

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "defaults-channel";

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

// ---------------------------------------------------------------------------
// The default table, as the service publishes it
// ---------------------------------------------------------------------------

/// The default `AdTriggers` list, as JSON.
fn default_ad_triggers() -> Value {
    Value::Array(
        state::DEFAULT_AD_TRIGGERS
            .iter()
            .map(|trigger| json!(trigger))
            .collect(),
    )
}

/// The endpoint-level members MediaPackage server-fills, with the default each takes.
fn endpoint_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("origination", json!("ALLOW")),
        ("startoverWindowSeconds", json!(0)),
        ("timeDelaySeconds", json!(0)),
    ]
}

/// The `hlsPackage` members MediaPackage server-fills, with the default each takes.
fn hls_package_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("adMarkers", json!("NONE")),
        ("adTriggers", default_ad_triggers()),
        ("adsOnDeliveryRestrictions", json!("RESTRICTED")),
        ("includeDvbSubtitles", json!(false)),
        ("includeIframeOnlyStream", json!(false)),
        ("playlistType", json!("EVENT")),
        ("playlistWindowSeconds", json!(60)),
        ("programDateTimeIntervalSeconds", json!(0)),
        ("useAudioRenditionGroup", json!(false)),
    ]
}

/// The members a CMAF child `hlsManifests[]` entry takes: the manifest members it shares
/// with `hlsPackage`, with the same defaults. `includeDvbSubtitles` and
/// `useAudioRenditionGroup` are not modelled on `HlsManifest`, so they are not here.
fn hls_manifest_defaults() -> Vec<(&'static str, Value)> {
    hls_package_defaults()
        .into_iter()
        .filter(|(member, _)| {
            !matches!(*member, "includeDvbSubtitles" | "useAudioRenditionGroup")
        })
        .collect()
}

/// The `dashPackage` members MediaPackage server-fills, with the default each takes.
fn dash_package_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("adTriggers", default_ad_triggers()),
        ("adsOnDeliveryRestrictions", json!("RESTRICTED")),
        ("includeIframeOnlyStream", json!(false)),
        ("minBufferTimeSeconds", json!(30)),
        ("periodTriggers", json!([])),
        ("profile", json!("NONE")),
        ("segmentTemplateFormat", json!("NUMBER_WITH_TIMELINE")),
    ]
}

/// What every projection of the endpoint created by `request` must carry, as JSON-pointer
/// / value pairs: the value the request supplied where it supplied one, else that member's
/// default. Derived from the request and the table above, so one row asserts BOTH halves
/// of the rule — nothing sent is overwritten and nothing unset is left absent.
fn expected_members(request: &Value) -> Vec<(String, Value)> {
    let mut expected = Vec::new();
    let mut push = |pointer: String, default: Value| {
        let value = request.pointer(&pointer).cloned().unwrap_or(default);
        expected.push((pointer, value));
    };
    for (member, default) in endpoint_defaults() {
        push(format!("/{member}"), default);
    }
    if request.get("hlsPackage").is_some() {
        for (member, default) in hls_package_defaults() {
            push(format!("/hlsPackage/{member}"), default);
        }
    }
    if request.get("dashPackage").is_some() {
        for (member, default) in dash_package_defaults() {
            push(format!("/dashPackage/{member}"), default);
        }
    }
    let children = request
        .pointer("/cmafPackage/hlsManifests")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    for index in 0..children {
        for (member, default) in hls_manifest_defaults() {
            push(format!("/cmafPackage/hlsManifests/{index}/{member}"), default);
        }
    }
    expected
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// The create bodies under test (merged onto `channelId`/`id`), one row per point of the
/// class: the packaging configuration, how much of it the request fills in, and the form
/// of the values it sends.
fn cases() -> Vec<(&'static str, Value)> {
    vec![
        // Nothing optional at all: every server-filled member must appear.
        ("hls, nothing supplied", json!({ "hlsPackage": {} })),
        // Some supplied, some not, at both levels.
        (
            "hls, partly supplied",
            json!({
                "origination": "DENY",
                "timeDelaySeconds": 7,
                "hlsPackage": {
                    "adMarkers": "PASSTHROUGH",
                    "playlistType": "VOD",
                    "playlistWindowSeconds": 120,
                    "includeIframeOnlyStream": true,
                    "adTriggers": ["BREAK", "PROVIDER_OVERLAY_PLACEMENT_OPPORTUNITY"]
                }
            }),
        ),
        // An EXPLICITLY empty list is a supplied value, not an unset member: "no ad
        // triggers" must survive instead of being replaced by the default list.
        (
            "hls, empty adTriggers",
            json!({ "hlsPackage": { "adTriggers": [] } }),
        ),
        // Every server-filled member supplied, each with a non-default value: the service
        // must overwrite none of them.
        (
            "hls, all supplied",
            json!({
                "origination": "DENY",
                "startoverWindowSeconds": 1_209_600,
                "timeDelaySeconds": 86_400,
                "hlsPackage": {
                    "adMarkers": "DATERANGE",
                    "adTriggers": ["SPLICE_INSERT"],
                    "adsOnDeliveryRestrictions": "BOTH",
                    "includeDvbSubtitles": true,
                    "includeIframeOnlyStream": true,
                    "playlistType": "NONE",
                    "playlistWindowSeconds": 1,
                    "programDateTimeIntervalSeconds": 60,
                    "useAudioRenditionGroup": true
                }
            }),
        ),
        // A different packaging type: the shared members take the same defaults, and the
        // DASH-only ones take theirs.
        ("dash, nothing supplied", json!({ "dashPackage": {} })),
        (
            "dash, partly supplied",
            json!({
                "startoverWindowSeconds": 60,
                "dashPackage": {
                    "profile": "HBBTV_1_5",
                    "periodTriggers": ["ADS"],
                    "minBufferTimeSeconds": 4
                }
            }),
        ),
        // CMAF: the defaults land on EACH child manifest, whatever its own members.
        (
            "cmaf, one child manifest",
            json!({ "cmafPackage": { "hlsManifests": [{ "id": "only" }] } }),
        ),
        (
            "cmaf, three child manifests partly supplied",
            json!({
                "cmafPackage": {
                    "hlsManifests": [
                        { "id": "a" },
                        { "id": "b", "adMarkers": "SCTE35_ENHANCED", "playlistWindowSeconds": 30 },
                        { "id": "c", "adTriggers": [], "playlistType": "VOD" }
                    ]
                }
            }),
        ),
        // MSS: the endpoint-level members are filled, and no member is invented inside a
        // package whose defaults neither the model nor a recording publishes.
        ("mss, nothing supplied", json!({ "mssPackage": {} })),
    ]
}

/// Assert `projection` carries every pair of `expected`.
fn assert_members(case: &str, projection: &str, expected: &[(String, Value)], body: &Value) {
    for (pointer, value) in expected {
        let actual = body.pointer(pointer);
        assert_eq!(
            actual,
            Some(value),
            "case {case:?}: {projection} must carry {pointer} = {value}, got {actual:?} \
             in {body}"
        );
    }
}

#[tokio::test]
async fn every_origin_endpoint_projection_carries_the_server_filled_members() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "creating the fixture channel expected 200, got {status} {body}"
    );

    for (index, (case, extras)) in cases().into_iter().enumerate() {
        let id = format!("endpoint-{index}");
        let mut request = json!({ "channelId": CHANNEL, "id": id });
        for (member, value) in extras.as_object().expect("an object of request members") {
            request[member] = value.clone();
        }
        let expected = expected_members(&request);
        assert!(
            !expected.is_empty(),
            "case {case:?}: the case must assert at least one server-filled member"
        );

        // 1. The create response.
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&request)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        assert_members(case, "the create response", &expected, &created);

        // 2. The read path, read back through DescribeOriginEndpoint: the SAME record.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described}"
        );
        assert_members(case, "DescribeOriginEndpoint", &expected, &described);
        assert_eq!(
            described, created,
            "case {case:?}: DescribeOriginEndpoint must render the same stored record as \
             the create response"
        );

        // 3. The listing, which must reuse that same projection.
        let (status, listed) = call(
            &router,
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
                    .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id.as_str()))
                    .cloned()
            })
            .unwrap_or(Value::Null);
        assert_members(case, "the ListOriginEndpoints entry", &expected, &entry);
        assert_eq!(
            entry, created,
            "case {case:?}: the listed endpoint must be the same stored record"
        );
    }
}

#[tokio::test]
async fn a_package_with_no_published_defaults_gains_no_invented_member() {
    let router = emulator();
    let (status, _) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(status, 200, "creating the fixture channel expected 200");

    // `MssPackage` publishes no default for any of its members, and neither does
    // `HlsPackage.StreamSelection`/`SegmentDurationSeconds`, so a create must store those
    // blocks exactly as sent: a default table may not grow a member AWS leaves absent.
    // One row per such block, the rows differing in whether the block is empty or carries
    // a member of its own.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("empty mss", "mssPackage", json!({})),
        (
            "mss with a window",
            "mssPackage",
            json!({ "manifestWindowSeconds": 30 }),
        ),
        (
            "mss with a stream selection",
            "mssPackage",
            json!({ "streamSelection": { "streamOrder": "ORIGINAL" } }),
        ),
    ];
    for (index, (case, member, package)) in rows.into_iter().enumerate() {
        let id = format!("unpublished-{index}");
        let request = json!({ "channelId": CHANNEL, "id": id, member: package.clone() });
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&request)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        let stored = created
            .get(member)
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        assert_eq!(
            &stored,
            &package,
            "case {case:?}: {member} must be stored exactly as sent, got {stored}"
        );
    }
}

#[tokio::test]
async fn every_server_filled_enum_default_is_a_value_the_model_accepts() {
    // The default values this service fills in are not free text: each one must be a
    // member of the enum the model declares for that member. Sources: the `*_VALUES`
    // tables generated from the Smithy model
    // (`smithy_query(query="shape",shape="AdMarkers,AdsOnDeliveryRestrictions,PlaylistType,Origination,Profile,SegmentTemplateFormat,__AdTriggersElement,__PeriodTriggersElement",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`).
    let singles: Vec<(&str, &str, &[&str])> = vec![
        ("Origination", state::DEFAULT_ORIGINATION, types::ORIGINATION_VALUES),
        ("AdMarkers", state::DEFAULT_AD_MARKERS, types::AD_MARKERS_VALUES),
        (
            "AdsOnDeliveryRestrictions",
            state::DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS,
            types::ADS_ON_DELIVERY_RESTRICTIONS_VALUES,
        ),
        ("PlaylistType", state::DEFAULT_PLAYLIST_TYPE, types::PLAYLIST_TYPE_VALUES),
        ("Profile", state::DEFAULT_DASH_PROFILE, types::PROFILE_VALUES),
        (
            "SegmentTemplateFormat",
            state::DEFAULT_DASH_SEGMENT_TEMPLATE_FORMAT,
            types::SEGMENT_TEMPLATE_FORMAT_VALUES,
        ),
    ];
    for (member, default, accepted) in singles {
        assert!(
            accepted.contains(&default),
            "case {member:?}: the default {default:?} must be one of {accepted:?}"
        );
    }

    let lists: Vec<(&str, &[&str], &[&str])> = vec![
        (
            "AdTriggers",
            state::DEFAULT_AD_TRIGGERS,
            types::__AD_TRIGGERS_ELEMENT_VALUES,
        ),
        (
            "PeriodTriggers",
            state::DEFAULT_DASH_PERIOD_TRIGGERS,
            types::__PERIOD_TRIGGERS_ELEMENT_VALUES,
        ),
    ];
    for (member, defaults, accepted) in lists {
        for entry in defaults {
            assert!(
                accepted.contains(entry),
                "case {member:?}: the default entry {entry:?} must be one of {accepted:?}"
            );
        }
        let mut sorted = defaults.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            defaults.len(),
            "case {member:?}: the default list must not repeat an entry, got {defaults:?}"
        );
    }
}
