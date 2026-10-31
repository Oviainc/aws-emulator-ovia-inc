// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `UpdateOriginEndpoint` WRITES, driven over HTTP through the
//! same router the emulator boots and reusing the restJson1 client in `channel_lifecycle`.
//!
//! `control_plane_validation` covers what the operation REFUSES (the immutable members,
//! the manifest matrix, the manifest ceiling, the ETag precondition, the 404s). This
//! suite covers the accepted path, over a SET of inputs rather than one request:
//!
//! 1. REPLACE — an update carries the endpoint's whole desired configuration, so a
//!    mutable member the request omits is NOT preserved: it is cleared, or resolved to
//!    the service's own default where it has one (`StartoverWindowSeconds`,
//!    `Segment`'s members), exactly as a create with that member absent resolves it. The
//!    one documented exception is `UriSeparator` ("If you don't specify a value in the
//!    update request, the current value is preserved"). The endpoint's identity (`Arn`,
//!    the three names, `CreatedAt`) and its tags survive, `ModifiedAt` is refreshed, and
//!    the following `GetOriginEndpoint` reports exactly what the update stored.
//! 2. CONTENT-ADDRESSED ETAG — the tag is a function of the endpoint's content, so an
//!    update that stores different content rotates it and one that re-submits the stored
//!    content reports the tag the endpoint already had.

use serde_json::{json, Value};

use crate::channel_lifecycle::Emulator;

/// The parents every case in this suite hangs its endpoints under.
const GROUP: &str = "upd-group";
const CHANNEL: &str = "upd-channel";

/// An emulator with the group and channel this suite's endpoints live in, plus the
/// group's egress domain — the host every manifest URL beneath it is served from.
///
/// The channel's `InputType` is CMAF because rows below carry
/// `ForceEndpointErrorConfiguration`, which the model restricts to the endpoints of a
/// CMAF-input channel (`ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION`);
/// `endpoint_member_input_type_restriction` covers that restriction itself.
async fn emulator_with_parents() -> (Emulator, String) {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "CreateChannelGroup expected 200, got {body}");
    let egress_domain = body["EgressDomain"]
        .as_str()
        .expect("the created group's egress domain")
        .to_string();
    let (status, body) = emulator
        .create_channel(GROUP, CHANNEL, json!({"InputType": "CMAF"}))
        .await;
    assert_eq!(status, 200, "CreateChannel expected 200, got {body}");
    (emulator, egress_domain)
}

/// A response timestamp, as the epoch seconds restJson1 renders it with.
fn instant(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be a timestamp, got {}", body[member]))
}

/// The manifest names one family carries, in the order the response lists them.
fn manifest_names(body: &Value, member: &str) -> Vec<String> {
    body[member].as_array().map_or_else(Vec::new, |manifests| {
        manifests
            .iter()
            .map(|manifest| {
                manifest["ManifestName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    })
}

/// Every manifest family an origin endpoint can carry, with the URL suffix of its
/// container format (`ManifestKind` in `src/state.rs`). Named `URL_SUFFIX_BY_FAMILY`
/// rather than `MANIFEST_FAMILIES` so it cannot be confused with the same-named table in
/// `control_plane_validation`, which pairs a family with a sample manifest name.
const URL_SUFFIX_BY_FAMILY: &[(&str, &str)] = &[
    ("HlsManifests", ".m3u8"),
    ("LowLatencyHlsManifests", ".m3u8"),
    ("DashManifests", ".mpd"),
    ("MssManifests", ".ism/Manifest"),
];

/// One row of the replace table: an endpoint to create, the update to apply, and what
/// the endpoint must then report.
struct UpdateCase {
    label: &'static str,
    endpoint: &'static str,
    create: fn() -> Value,
    update: fn() -> Value,
    /// Members whose value after the update is pinned, named by a dotted path into the
    /// response (`Segment.SegmentName`) so a row can pin one member of a structure
    /// without pinning the defaults the service fills the others with. `Value::Null`
    /// means the update cleared the member.
    expected: fn() -> Vec<(&'static str, Value)>,
    /// `(family member, manifest name)` for every manifest the endpoint carries after
    /// the update; a family not listed here must carry none.
    manifests: &'static [(&'static str, &'static str)],
    /// The tag map the endpoint must still report after the update, asserted literally
    /// so a dropped map cannot pass. The empty map for a row created without tags.
    expected_tags: fn() -> Value,
}

/// The rows, differing in which members the update supplies and which it omits, in the
/// manifest families and counts it stores, and in container type. `ContainerType` values
/// come from the model's `ContainerType` enum (TS, CMAF, ISM), `UriSeparator` from
/// `UriSeparator` (UNDERSCORE, HYPHEN) and `EndpointErrorConditions` from
/// `EndpointErrorCondition` (STALE_MANIFEST, INCOMPLETE_MANIFEST, MISSING_DRM_KEY,
/// SLATE_INPUT); every row carries the one manifest an endpoint must have, in a family
/// its container type serves.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "an update does not preserve the members it omits",
            endpoint: "ep-cleared",
            create: || {
                json!({
                    "ContainerType": "TS",
                    "Description": "the original",
                    "StartoverWindowSeconds": 300,
                    "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                    "UriSeparator": "UNDERSCORE",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "ForceEndpointErrorConfiguration": {
                        "EndpointErrorConditions": ["STALE_MANIFEST"]
                    },
                    "Tags": {"team": "video"},
                })
            },
            update: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            expected: || {
                vec![
                    ("Description", Value::Null),
                    ("ForceEndpointErrorConfiguration", Value::Null),
                    // Not preserved either: the window the create named is gone and the
                    // member resolves to the service's own default, the value a create
                    // that named none resolves to
                    // (`state::DEFAULT_STARTOVER_WINDOW_SECONDS`).
                    ("StartoverWindowSeconds", json!(900)),
                    // Documented as preserved when the update omits it.
                    ("UriSeparator", json!("UNDERSCORE")),
                ]
            },
            manifests: &[("HlsManifests", "index")],
            expected_tags: || json!({"team": "video"}),
        },
        UpdateCase {
            label: "the manifests are replaced wholesale, across families",
            endpoint: "ep-manifests",
            create: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "hls-a"}, {"ManifestName": "hls-b"}],
                    "DashManifests": [{"ManifestName": "dash-a"}],
                })
            },
            update: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "hls-c"}],
                    "LowLatencyHlsManifests": [{"ManifestName": "ll-hls"}],
                })
            },
            expected: || vec![("Description", Value::Null)],
            manifests: &[("HlsManifests", "hls-c"), ("LowLatencyHlsManifests", "ll-hls")],
            expected_tags: || json!({}),
        },
        UpdateCase {
            label: "startover window at the published minimum, description emptied",
            endpoint: "ep-min-window",
            create: || {
                json!({
                    "ContainerType": "TS",
                    "StartoverWindowSeconds": 300,
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            update: || {
                json!({
                    "ContainerType": "TS",
                    "StartoverWindowSeconds": 0,
                    "Description": "",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            expected: || {
                vec![
                    ("StartoverWindowSeconds", json!(0)),
                    ("Description", json!("")),
                ]
            },
            manifests: &[("HlsManifests", "index")],
            expected_tags: || json!({}),
        },
        UpdateCase {
            label: "startover window at the published maximum, uri separator replaced",
            endpoint: "ep-max-window",
            create: || {
                json!({
                    "ContainerType": "TS",
                    "UriSeparator": "UNDERSCORE",
                    "Description": "before",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            update: || {
                json!({
                    "ContainerType": "TS",
                    "StartoverWindowSeconds": 1_209_600,
                    "UriSeparator": "HYPHEN",
                    "Description": "after",
                    "LowLatencyHlsManifests": [{"ManifestName": "ll"}],
                })
            },
            expected: || {
                vec![
                    ("StartoverWindowSeconds", json!(1_209_600)),
                    ("UriSeparator", json!("HYPHEN")),
                    ("Description", json!("after")),
                ]
            },
            manifests: &[("LowLatencyHlsManifests", "ll")],
            expected_tags: || json!({}),
        },
        UpdateCase {
            label: "an ISM endpoint's MSS manifest and segment configuration are replaced",
            endpoint: "ep-ism",
            create: || {
                json!({
                    "ContainerType": "ISM",
                    "MssManifests": [{"ManifestName": "smooth"}],
                    "Tags": {"a": "1", "b": "2"},
                })
            },
            update: || {
                json!({
                    "ContainerType": "ISM",
                    "MssManifests": [{"ManifestName": "smoother"}],
                    "Segment": {"SegmentDurationSeconds": 4, "SegmentName": "s"},
                })
            },
            expected: || {
                vec![
                    ("Segment.SegmentDurationSeconds", json!(4)),
                    ("Segment.SegmentName", json!("s")),
                ]
            },
            manifests: &[("MssManifests", "smoother")],
            expected_tags: || json!({"a": "1", "b": "2"}),
        },
        UpdateCase {
            label: "the failover settings are replaced with a different condition set",
            endpoint: "ep-failover",
            create: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "Tags": {"owner": "video"},
                    "ForceEndpointErrorConfiguration": {
                        "EndpointErrorConditions": ["STALE_MANIFEST"]
                    },
                })
            },
            update: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "ForceEndpointErrorConfiguration": {
                        "EndpointErrorConditions": ["INCOMPLETE_MANIFEST", "SLATE_INPUT"]
                    },
                })
            },
            expected: || {
                vec![(
                    "ForceEndpointErrorConfiguration",
                    json!({"EndpointErrorConditions": ["INCOMPLETE_MANIFEST", "SLATE_INPUT"]}),
                )]
            },
            manifests: &[("HlsManifests", "index")],
            expected_tags: || json!({"owner": "video"}),
        },
    ]
}

/// RULES 1 and 2 — an update replaces the endpoint's mutable configuration, keeps its
/// identity and tags, rotates the ETag, and is reported unchanged by the following read.
#[tokio::test]
async fn an_update_replaces_the_endpoint_and_the_read_reports_what_it_stored() {
    let (emulator, egress_domain) = emulator_with_parents().await;

    for case in update_cases() {
        let label = case.label;
        let (status, created) = emulator
            .create_endpoint(GROUP, CHANNEL, case.endpoint, (case.create)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );

        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, case.endpoint, (case.update)(), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );

        // Identity and creation time are not the update's to change.
        for member in [
            "Arn",
            "ChannelGroupName",
            "ChannelName",
            "OriginEndpointName",
            "ContainerType",
            "CreatedAt",
        ] {
            assert_eq!(
                updated[member], created[member],
                "case {label:?}: {member} must survive the update, got {updated}"
            );
        }
        // `UpdateOriginEndpointResponse` carries `@jsonName("tags")` on the tag map the
        // create response renders as `Tags`, so the two differ in the member NAME and
        // must agree on the value: only the tagging operations change tags. The expected
        // map is asserted LITERALLY as well, so a dropped tag map cannot pass as two
        // absent maps.
        let expected_tags = (case.expected_tags)();
        assert_eq!(
            updated["tags"], expected_tags,
            "case {label:?}: expected the tags to still be {expected_tags}, got {updated}"
        );
        assert_eq!(
            updated["tags"], created["Tags"],
            "case {label:?}: the endpoint's tags must survive the update, got {updated}"
        );
        // `ModifiedAt` is refreshed by the update, so it is at or after the creation
        // instant rather than merely present.
        assert!(
            instant(&updated, "ModifiedAt") >= instant(&created, "CreatedAt"),
            "case {label:?}: ModifiedAt {} must be at or after CreatedAt {}",
            updated["ModifiedAt"],
            created["CreatedAt"]
        );
        // `Segment` is required in the response, so it is reported whether or not the
        // update supplied one (a row that supplies one pins its value below).
        assert!(
            updated["Segment"].is_object(),
            "case {label:?}: Segment must be reported, got {updated}"
        );

        for (path, expected) in (case.expected)() {
            let actual = path.split('.').fold(&updated, |value, member| &value[member]);
            assert_eq!(
                *actual, expected,
                "case {label:?}: expected {path} to be {expected}, got {actual}"
            );
        }

        // Every family carries exactly the manifests the update submitted, each with the
        // URL computed from the group's egress domain and the resource path.
        for (family, suffix) in URL_SUFFIX_BY_FAMILY {
            let expected: Vec<String> = case
                .manifests
                .iter()
                .filter(|(member, _)| member == family)
                .map(|(_, name)| (*name).to_string())
                .collect();
            assert_eq!(
                manifest_names(&updated, family),
                expected,
                "case {label:?}: {family} must carry exactly {expected:?}, got {}",
                updated[family]
            );
            for manifest in updated[family].as_array().into_iter().flatten() {
                let name = manifest["ManifestName"].as_str().unwrap_or_default();
                let url = manifest["Url"].as_str().unwrap_or_default();
                let expected_url = format!(
                    "https://{egress_domain}/out/v1/{GROUP}/{CHANNEL}/{}/{name}{suffix}",
                    case.endpoint
                );
                assert_eq!(
                    url, expected_url,
                    "case {label:?}: {family} {name:?} url must be the group's egress \
                     domain plus this resource path"
                );
            }
        }

        // RULE 2 — these rows all store different content, so the tag must rotate.
        assert_ne!(
            updated["ETag"], created["ETag"],
            "case {label:?}: an update that changes the endpoint's content must rotate the \
             ETag, got {updated}"
        );

        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, case.endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
        );
        for member in [
            "Arn",
            "ContainerType",
            "Segment",
            "Description",
            "StartoverWindowSeconds",
            "HlsManifests",
            "LowLatencyHlsManifests",
            "DashManifests",
            "MssManifests",
            "ForceEndpointErrorConfiguration",
            "UriSeparator",
            "StreamNameOutputMode",
            "CreatedAt",
            "ModifiedAt",
            "ETag",
        ] {
            assert_eq!(
                read[member], updated[member],
                "case {label:?}: GetOriginEndpoint {member} must equal what the update stored"
            );
        }
    }
}

/// RULE 2 — the ETag is a function of the content, so re-submitting the stored
/// configuration reports the tag the endpoint already had.
///
/// AWS documents the ETag's PURPOSE (it is what `x-amzn-update-if-match` is matched
/// against) but not how it is computed, so this invariant is the emulator's own contract
/// for that precondition — a tag that rotated on every write would refuse a conditional
/// update the caller is entitled to make — not a value read off the model.
#[tokio::test]
async fn an_update_that_stores_the_same_content_reports_the_same_etag() {
    let (emulator, _egress_domain) = emulator_with_parents().await;

    // One row per shape of configuration: the invariant holds for an endpoint with
    // nothing optional set beyond its one manifest, and for one with every family of
    // member set.
    let bodies = [
        (
            "minimal",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
            }),
        ),
        (
            "fully configured",
            json!({
                "ContainerType": "CMAF",
                "Description": "stable",
                "StartoverWindowSeconds": 60,
                "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                "UriSeparator": "HYPHEN",
                "HlsManifests": [{"ManifestName": "index"}],
                "DashManifests": [{"ManifestName": "dash"}],
                "ForceEndpointErrorConfiguration": {
                    "EndpointErrorConditions": ["MISSING_DRM_KEY"]
                },
            }),
        ),
    ];

    for (label, body) in bodies {
        let name = format!("ep-same-{}", label.replace(' ', "-"));
        let (status, created) = emulator
            .create_endpoint(GROUP, CHANNEL, &name, body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );

        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, &name, body.clone(), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["ETag"], created["ETag"],
            "case {label:?}: an update storing the same content must report the same ETag, \
             got {} after {}",
            updated["ETag"], created["ETag"]
        );
    }
}
