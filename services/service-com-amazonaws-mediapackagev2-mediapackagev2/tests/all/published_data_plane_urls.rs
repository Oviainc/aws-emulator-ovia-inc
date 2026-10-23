// SPDX-License-Identifier: CC0-1.0

//! Authorization on EVERY data-plane URL the control plane publishes — not only the one
//! HLS manifest and the one ingest endpoint a recorded run happens to use.
//!
//! The rule: a request addressed to a hostname this service publishes is answered by this
//! service with an AUTHORIZATION decision, never by a routing failure — an ingest push
//! carrying no SigV4 credential is `403` with an access-denied body (MediaPackage v2 vends
//! no ingest credential), and an egress fetch is `403` until the addressed origin
//! endpoint's resource policy allows the action the request's method needs to the `*`
//! principal, falling through to the object lookup (`404`, since nothing is packaged) only
//! once it does.
//!
//! `rejection_rule_and_negative_paths.rs` (rule 6) pins that rule in its ONE-row form: the
//! HLS manifest of a TS endpoint, the first ingest endpoint, and the shapes of resource
//! policy that do and do not grant. This suite is the same rule along the axes that row
//! fixes — the manifest FAMILY an endpoint publishes a URL for (and so its container type
//! and URL suffix), the OBJECT under the endpoint's path (the published manifest and a
//! segment beside it), the request METHOD with the action it needs, the channel's input
//! type, and the INDEX of the ingest endpoint within a channel's redundant pair. It does
//! not vary the resource names, which the rule does not generalise over, and it leaves the
//! credential axis to `data_plane_credentials.rs`, which runs with signature verification
//! on.
//!
//! The client, the policy writer and the URL splitter are the ones
//! `rejection_rule_and_negative_paths` already owns, reached through `pub(crate)`, so there
//! is no second copy of them here.

use serde_json::{json, Value};

use crate::rejection_rule_and_negative_paths::{grant_policy, split_url, Emulator};

/// The channel group every fixture hangs under. Nothing in the rule depends on it.
const GROUP: &str = "pub-group";

/// The channel the egress rows' endpoints hang under. Its input type is `CMAF`, which the
/// container types these rows use all accept.
const EGRESS_CHANNEL: &str = "pub-channel";

/// One egress row: a manifest family, a container type that serves it, and the suffix the
/// family's URL ends in.
struct EgressRow {
    label: &'static str,
    endpoint: &'static str,
    container: &'static str,
    family: &'static str,
    manifest: &'static str,
    suffix: &'static str,
}

/// Every manifest family an origin endpoint publishes a URL for, each on a container type
/// that serves it.
///
/// Source: the `CreateOriginEndpoint` input members `HlsManifests`,
/// `LowLatencyHlsManifests`, `DashManifests` and `MssManifests`
/// (`smithy_query query="operation" operation="CreateOriginEndpoint"`), the `ContainerType`
/// enum (`CMAF | ISM | TS`), and the container/family pairs the model's
/// `ValidationExceptionType` refuses — `TS_CONTAINER_TYPE_WITH_DASH_MANIFEST`,
/// `TS_CONTAINER_TYPE_WITH_MSS_MANIFEST`, `CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST`,
/// `ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST`, `ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST` and
/// `ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST` — which leave exactly these pairings. The HLS
/// row is the one a recording covers; the other three are what the rule also has to hold
/// for.
const EGRESS_ROWS: &[EgressRow] = &[
    EgressRow {
        label: "HLS on a TS endpoint",
        endpoint: "ep-hls",
        container: "TS",
        family: "HlsManifests",
        manifest: "index",
        suffix: ".m3u8",
    },
    EgressRow {
        label: "low-latency HLS on a CMAF endpoint",
        endpoint: "ep-llhls",
        container: "CMAF",
        family: "LowLatencyHlsManifests",
        manifest: "low",
        suffix: ".m3u8",
    },
    EgressRow {
        label: "DASH on a CMAF endpoint",
        endpoint: "ep-dash",
        container: "CMAF",
        family: "DashManifests",
        manifest: "dash",
        suffix: ".mpd",
    },
    EgressRow {
        label: "MSS on an ISM endpoint",
        endpoint: "ep-mss",
        container: "ISM",
        family: "MssManifests",
        manifest: "smooth",
        suffix: ".ism/Manifest",
    },
];

/// The methods the egress data plane serves, each with the action it is authorized against:
/// `mediapackagev2:GetObject` for a `GET` and `mediapackagev2:GetHeadObject` for a `HEAD`,
/// the actions MediaPackage's own endpoint-policy examples grant.
const EGRESS_METHODS: &[(&str, &str)] = &[
    ("GET", "mediapackagev2:GetObject"),
    ("HEAD", "mediapackagev2:GetHeadObject"),
];

/// The published URL of the one manifest a response's `family` carries.
fn manifest_url(created: &Value, family: &str) -> String {
    created
        .get(family)
        .and_then(Value::as_array)
        .and_then(|manifests| manifests.first())
        .and_then(|manifest| manifest.get("Url"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{family} must publish a manifest URL, got {created}"))
        .to_string()
}

/// The path of a sibling object under the same origin endpoint as `manifest_path` — a
/// segment beside the manifest, so the gate cannot be keyed to the manifest's own name.
fn sibling_object(manifest_path: &str) -> String {
    let parent = manifest_path
        .rsplit_once('/')
        .map(|(parent, _object)| parent)
        .unwrap_or_default();
    format!("{parent}/segment_1.ts")
}

/// Create the channel group and the channel the egress rows hang under, and report the
/// group's published egress domain.
async fn egress_fixtures(emulator: &Emulator) -> String {
    let group = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        group.status, 200,
        "the channel group must be created, got {} with body {}",
        group.status, group.body
    );
    let egress_domain = group.string("EgressDomain").to_string();
    assert!(
        egress_domain.contains(".egress."),
        "the group must publish an egress domain, got {}",
        group.body
    );
    let channel = emulator
        .create_channel(GROUP, EGRESS_CHANNEL, json!({"InputType": "CMAF"}))
        .await;
    assert_eq!(
        channel.status, 200,
        "the channel must be created, got {} with body {}",
        channel.status, channel.body
    );
    egress_domain
}

/// Every manifest family's published URL is gated by the endpoint's resource policy: `403`
/// for an anonymous fetch of any object under the endpoint until the policy grants the
/// action the method needs, `404` once it does.
#[tokio::test]
async fn every_published_manifest_url_is_gated_by_the_endpoints_resource_policy() {
    let emulator = Emulator::new();
    let egress_domain = egress_fixtures(&emulator).await;

    for row in EGRESS_ROWS {
        let label = row.label;
        // The family is a row's value, so the member it names is set on the body rather
        // than written as a literal key.
        let mut body = json!({"ContainerType": row.container});
        body[row.family] = json!([{"ManifestName": row.manifest}]);
        let created = emulator
            .create_endpoint(GROUP, EGRESS_CHANNEL, row.endpoint, body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the origin endpoint must be created, got {} with body {}",
            created.status, created.body
        );
        let url = manifest_url(&created.json, row.family);
        assert_eq!(
            url,
            format!(
                "https://{egress_domain}/out/v1/{GROUP}/{EGRESS_CHANNEL}/{}/{}{}",
                row.endpoint, row.manifest, row.suffix
            ),
            "case {label:?}: the published URL must be the group's egress domain plus this \
             resource path, got {url}"
        );
        let (host, manifest_path) = split_url(&url);
        let segment_path = sibling_object(&manifest_path);
        let objects = [
            ("the published manifest", manifest_path.as_str()),
            ("a segment beside it", segment_path.as_str()),
        ];
        let policy_path = format!(
            "/channelGroup/{GROUP}/channel/{EGRESS_CHANNEL}/originEndpoint/{}/policy",
            row.endpoint
        );

        // Before any grant: refused, whichever object and whichever method, and the refusal
        // says access is denied rather than reporting the endpoint as empty.
        for (object, path) in objects {
            for (method, _action) in EGRESS_METHODS {
                let answer = emulator
                    .send(method, path, None, &host, false, &[])
                    .await;
                assert_eq!(
                    answer.status, 403,
                    "case {label:?} / {object} / {method} with no policy attached: expected \
                     HTTP 403, got {} with body {}",
                    answer.status, answer.body
                );
                assert!(
                    answer.body.to_lowercase().contains("access denied"),
                    "case {label:?} / {object} / {method} with no policy attached: the refusal \
                     must say access denied, got {}",
                    answer.body
                );
            }
        }

        // With the action a method needs granted to everyone: authorized, so the object
        // lookup answers — nothing is packaged, which is a 404 and no longer a 403.
        for (method, action) in EGRESS_METHODS {
            let granted = emulator
                .call(
                    "POST",
                    &policy_path,
                    Some(json!({"Policy": grant_policy(action)})),
                )
                .await;
            assert_eq!(
                granted.status, 200,
                "case {label:?} / {method}: the grant of {action} must be stored, got {} with \
                 body {}",
                granted.status, granted.body
            );
            for (object, path) in objects {
                let answer = emulator
                    .send(method, path, None, &host, false, &[])
                    .await;
                assert_eq!(
                    answer.status, 404,
                    "case {label:?} / {object} / {method} once the policy grants {action}: \
                     expected HTTP 404, got {} with body {}",
                    answer.status, answer.body
                );
            }
        }
    }
}

/// The channels the ingest rows push to, each with an input type from the model's
/// `InputType` enum (`CMAF | HLS | MULTIVIEW`).
const INGEST_CHANNELS: &[(&str, &str)] = &[("in-cmaf", "CMAF"), ("in-hls", "HLS")];

/// The methods the ingest data plane serves: a contributing encoder writes a segment with
/// either of them.
const INGEST_METHODS: &[&str] = &["PUT", "POST"];

/// Every ingest URL a channel publishes answers an unauthenticated push with `403` and an
/// access-denied body — for every channel, every endpoint index in its redundant pair, and
/// either write method.
#[tokio::test]
async fn every_published_ingest_url_refuses_an_unauthenticated_push() {
    let emulator = Emulator::new();
    let group = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        group.status, 200,
        "the channel group must be created, got {} with body {}",
        group.status, group.body
    );

    let mut seen: Vec<String> = Vec::new();
    for (channel, input_type) in INGEST_CHANNELS {
        let created = emulator
            .create_channel(GROUP, channel, json!({"InputType": input_type}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {channel:?}: the channel must be created, got {} with body {}",
            created.status, created.body
        );
        let endpoints = created
            .json
            .get("IngestEndpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            endpoints.len() >= 2,
            "case {channel:?}: a channel publishes a redundant pair of ingest endpoints, got {}",
            created.body
        );
        for (index, endpoint) in endpoints.iter().enumerate() {
            let url = endpoint
                .get("Url")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {channel:?}: ingest endpoint {index} has no Url"));
            assert!(
                !seen.iter().any(|already| already == url),
                "case {channel:?} / ingest endpoint {index}: every published ingest URL must be \
                 its own, got {url} twice"
            );
            seen.push(url.to_string());
            let (host, path) = split_url(url);
            let object = format!("{path}/0.mp4");
            for method in INGEST_METHODS {
                let answer = emulator
                    .send(
                        method,
                        &object,
                        Some(json!({"segment": "bytes"})),
                        &host,
                        false,
                        &[],
                    )
                    .await;
                assert_eq!(
                    answer.status, 403,
                    "case {channel:?} / ingest endpoint {index} / {method} with no credential: \
                     expected HTTP 403, got {} with body {}",
                    answer.status, answer.body
                );
                assert!(
                    answer.body.to_lowercase().contains("access denied"),
                    "case {channel:?} / ingest endpoint {index} / {method} with no credential: \
                     the refusal must say access denied, got {}",
                    answer.body
                );
            }
        }
    }
}
