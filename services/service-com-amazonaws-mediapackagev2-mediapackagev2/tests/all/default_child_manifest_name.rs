// SPDX-License-Identifier: CC0-1.0

//! The child manifest name MediaPackage applies to a manifest configuration that was
//! submitted without one.
//!
//! RULE under test, asserted over a SET of inputs rather than over one request: an HLS or
//! low-latency HLS manifest configuration whose create/update request named no
//! `ChildManifestName` is stored with the child manifest name the model documents the
//! service applies, so the create response, a later `GetOriginEndpoint`, the
//! `ListOriginEndpoints` projection and the same three after an `UpdateOriginEndpoint` all
//! report that one concrete value; a submitted name is reported verbatim, and the families
//! that model no such member (DASH, MSS) report none.
//!
//! Rows differ along what the rule generalises over: which manifest family carries the
//! manifest, how many manifests there are, whether each one names a child manifest name,
//! where in the member's modelled length range a submitted name sits, and whether the
//! manifest's own name is the default one.

use serde_json::{json, Value};

// The crate's one emulator client, declared in the channel-lifecycle suite `pub(crate)`
// for exactly this reuse.
use crate::channel_lifecycle::{at, Emulator};

/// The child manifest name the service applies when a manifest configuration names none.
///
/// Source: the model's `GetHlsManifestConfiguration$ChildManifestName` and
/// `GetLowLatencyHlsManifestConfiguration$ChildManifestName` docs — "If you don't enter a
/// value, MediaPackage uses the default child manifest name, index_1"
/// (`smithy_query(query="doc", shape="GetHlsManifestConfiguration",
/// member="ChildManifestName")`), the concrete form of the suffixed default the matching
/// `Create*` shapes describe.
const DEFAULT_CHILD_MANIFEST_NAME: &str = "index_1";

/// The modelled length range of a SUBMITTED child manifest name: `ManifestName` is
/// `@length(1, 256)` with `@pattern("^[a-zA-Z0-9-]+$")`
/// (`smithy_query(query="shape", shape="CreateHlsManifestConfiguration")`).
const MIN_NAME_LENGTH: usize = 1;
const MAX_NAME_LENGTH: usize = 256;

/// One row: a packaging configuration to create (and then to re-submit as an update), plus
/// the `(pointer, value)` pairs every projection of the endpoint must report.
struct Case {
    label: &'static str,
    body: fn() -> Value,
    expected: fn() -> Vec<(String, Value)>,
}

fn rows() -> Vec<Case> {
    vec![
        Case {
            label: "one bare HLS manifest",
            body: || json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}),
            expected: || {
                vec![(
                    "/HlsManifests/0/ChildManifestName".to_string(),
                    json!(DEFAULT_CHILD_MANIFEST_NAME),
                )]
            },
        },
        Case {
            // The applied default does not depend on the manifest's own name.
            label: "a bare HLS manifest whose own name is not the default manifest name",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "primary-feed"}],
                })
            },
            expected: || {
                vec![(
                    "/HlsManifests/0/ChildManifestName".to_string(),
                    json!(DEFAULT_CHILD_MANIFEST_NAME),
                )]
            },
        },
        Case {
            label: "an HLS manifest that names its own child manifest",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index", "ChildManifestName": "child"}],
                })
            },
            expected: || {
                vec![(
                    "/HlsManifests/0/ChildManifestName".to_string(),
                    json!("child"),
                )]
            },
        },
        Case {
            label: "a submitted child manifest name at the published minimum length",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{
                        "ManifestName": "index",
                        "ChildManifestName": "a".repeat(MIN_NAME_LENGTH),
                    }],
                })
            },
            expected: || {
                vec![(
                    "/HlsManifests/0/ChildManifestName".to_string(),
                    json!("a".repeat(MIN_NAME_LENGTH)),
                )]
            },
        },
        Case {
            label: "a submitted child manifest name at the published maximum length",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{
                        "ManifestName": "index",
                        "ChildManifestName": "b".repeat(MAX_NAME_LENGTH),
                    }],
                })
            },
            expected: || {
                vec![(
                    "/HlsManifests/0/ChildManifestName".to_string(),
                    json!("b".repeat(MAX_NAME_LENGTH)),
                )]
            },
        },
        Case {
            label: "three HLS manifests, only the middle one naming a child manifest",
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [
                        {"ManifestName": "hls-a"},
                        {"ManifestName": "hls-b", "ChildManifestName": "named-child"},
                        {"ManifestName": "hls-c"},
                    ],
                })
            },
            expected: || {
                vec![
                    (
                        "/HlsManifests/0/ChildManifestName".to_string(),
                        json!(DEFAULT_CHILD_MANIFEST_NAME),
                    ),
                    (
                        "/HlsManifests/1/ChildManifestName".to_string(),
                        json!("named-child"),
                    ),
                    (
                        "/HlsManifests/2/ChildManifestName".to_string(),
                        json!(DEFAULT_CHILD_MANIFEST_NAME),
                    ),
                ]
            },
        },
        Case {
            label: "one bare low-latency HLS manifest",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "LowLatencyHlsManifests": [{"ManifestName": "ll"}],
                })
            },
            expected: || {
                vec![(
                    "/LowLatencyHlsManifests/0/ChildManifestName".to_string(),
                    json!(DEFAULT_CHILD_MANIFEST_NAME),
                )]
            },
        },
        Case {
            label: "a low-latency HLS manifest that names its own child manifest",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "LowLatencyHlsManifests": [{
                        "ManifestName": "ll",
                        "ChildManifestName": "ll-child",
                    }],
                })
            },
            expected: || {
                vec![(
                    "/LowLatencyHlsManifests/0/ChildManifestName".to_string(),
                    json!("ll-child"),
                )]
            },
        },
        Case {
            label: "both HLS families bare, two manifests each",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "hls-a"}, {"ManifestName": "hls-b"}],
                    "LowLatencyHlsManifests": [
                        {"ManifestName": "ll-a"},
                        {"ManifestName": "ll-b", "ChildManifestName": "ll-b-child"},
                    ],
                })
            },
            expected: || {
                vec![
                    (
                        "/HlsManifests/0/ChildManifestName".to_string(),
                        json!(DEFAULT_CHILD_MANIFEST_NAME),
                    ),
                    (
                        "/HlsManifests/1/ChildManifestName".to_string(),
                        json!(DEFAULT_CHILD_MANIFEST_NAME),
                    ),
                    (
                        "/LowLatencyHlsManifests/0/ChildManifestName".to_string(),
                        json!(DEFAULT_CHILD_MANIFEST_NAME),
                    ),
                    (
                        "/LowLatencyHlsManifests/1/ChildManifestName".to_string(),
                        json!("ll-b-child"),
                    ),
                ]
            },
        },
        Case {
            // DASH models no child manifest name at all, so nothing is applied there.
            label: "a DASH manifest beside a bare HLS one",
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "DashManifests": [{"ManifestName": "dash"}],
                })
            },
            expected: || {
                vec![
                    (
                        "/HlsManifests/0/ChildManifestName".to_string(),
                        json!(DEFAULT_CHILD_MANIFEST_NAME),
                    ),
                    ("/DashManifests/0/ChildManifestName".to_string(), Value::Null),
                ]
            },
        },
        Case {
            // MSS models no child manifest name either, and an ISM endpoint serves only
            // that family.
            label: "an ISM endpoint carrying only an MSS manifest",
            body: || json!({"ContainerType": "ISM", "MssManifests": [{"ManifestName": "mss"}]}),
            expected: || {
                vec![("/MssManifests/0/ChildManifestName".to_string(), Value::Null)]
            },
        },
    ]
}

/// A channel group and a channel to hang the row's endpoint off.
async fn parent_chain(emulator: &Emulator, index: usize) -> (String, String) {
    let group = format!("child-group-{index}");
    let channel = format!("child-channel-{index}");
    let (status, body) = emulator.create_group(&group, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup for {group} expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel(&group, &channel, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannel for {channel} expected 200, got {status} {body}"
    );
    (group, channel)
}

/// RULE — every projection of the endpoint reports the child manifest name the service
/// materialised at write time: the submitted one, else the documented default.
#[tokio::test]
async fn every_projection_reports_the_child_manifest_name_the_service_applied() {
    let emulator = Emulator::new();

    for (index, case) in rows().into_iter().enumerate() {
        let label = case.label;
        let (group, channel) = parent_chain(&emulator, index).await;
        let endpoint = format!("endpoint-{index}");
        let expected = (case.expected)();

        let (status, created) = emulator
            .create_endpoint(&group, &channel, &endpoint, (case.body)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
        );
        let (status, listed) = emulator.list_endpoints(&group, &channel, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListOriginEndpoints expected 200, got {status} {listed}"
        );
        let item = at(&listed, "/Items/0");

        // The same packaging configuration submitted again through the update path, which
        // rebuilds the whole manifest set: a write must materialise the member too.
        let (status, updated) = emulator
            .update_endpoint(&group, &channel, &endpoint, (case.body)(), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        let (status, read_again) = emulator.get_endpoint(&group, &channel, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint after the update expected 200, got \
             {status} {read_again}"
        );

        for (projection, body) in [
            ("CreateOriginEndpoint", &created),
            ("GetOriginEndpoint", &read),
            ("ListOriginEndpoints item", &item),
            ("UpdateOriginEndpoint", &updated),
            ("GetOriginEndpoint after the update", &read_again),
        ] {
            for (pointer, want) in &expected {
                let actual = at(body, pointer);
                assert_eq!(
                    &actual, want,
                    "case {label:?}: {projection} {pointer} expected {want}, got {actual}"
                );
            }
        }
    }
}

/// RULE (the refusal half) — the default is applied only to an ABSENT name: a submitted
/// one outside the member's modelled constraints is refused rather than replaced by the
/// default. The bounds are `ManifestName`'s `@length(1, 256)` and its
/// `^[a-zA-Z0-9-]+$` pattern, which is why the applied default's `_` can only come from
/// the service.
#[tokio::test]
async fn a_submitted_child_manifest_name_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    let (group, channel) = parent_chain(&emulator, 900).await;

    let refused: Vec<(&str, Value)> = vec![
        (
            "one character past the published maximum length",
            json!("c".repeat(MAX_NAME_LENGTH + 1)),
        ),
        (
            "one character short of the published minimum length",
            json!(""),
        ),
        (
            "the applied default itself, whose separator the submitted pattern excludes",
            json!(DEFAULT_CHILD_MANIFEST_NAME),
        ),
    ];

    for (index, (label, child_name)) in refused.into_iter().enumerate() {
        let name = format!("refused-{index}");
        let body = json!({
            "ContainerType": "TS",
            "HlsManifests": [{"ManifestName": "index", "ChildManifestName": child_name}],
        });
        let (status, answer) = emulator.create_endpoint(&group, &channel, &name, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a 400 ValidationException, got {status} {answer}"
        );
        let (status, read) = emulator.get_endpoint(&group, &channel, &name).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a refused create must store nothing, got {status} {read}"
        );
    }
}
