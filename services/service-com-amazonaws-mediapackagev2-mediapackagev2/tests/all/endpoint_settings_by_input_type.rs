// SPDX-License-Identifier: CC0-1.0

//! Which of an origin endpoint's settings its PARENT channel's `InputType` lets it carry,
//! driven end-to-end over HTTP through the same router the emulator boots and reusing the
//! restJson1 client in `channel_lifecycle`.
//!
//! The rule under test, asserted over a TABLE of inputs rather than over one recorded
//! request: a setting the model restricts to the endpoints of particular channel
//! `InputType`s is refused — with the `ValidationExceptionType` the model names for that
//! combination, and with nothing stored — under a channel whose input type is not one of
//! them, and accepted under one whose is. A restricted setting with a service-applied
//! default is materialised only where it may be carried, so an endpoint whose parent may
//! not carry it reports none.
//!
//! The settings and their discriminators are the model's own
//! (`smithy_query(query="shape", shape="ValidationExceptionType,InputType")`):
//! `ForceEndpointErrorConfiguration` on the endpoints of a CMAF-input channel only
//! (`ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION`), and — on a channel
//! that composites its tiles from other channels rather than ingesting a stream of its own
//! — no ISM container (`MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER`), no low-latency HLS
//! manifest (`MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST`), no MSS manifest
//! (`MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST`) and no startover window
//! (`MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED`). `StreamNameOutputMode`, the sixth setting
//! restricted this way, is covered by `endpoint_member_input_type_restriction`.
//!
//! The rows span the model's whole `InputType` enum (HLS | CMAF | MULTIVIEW) plus the
//! absent case its doc defaults to HLS, and vary the endpoint's container type, the
//! manifest families it publishes and the value of the restricted setting.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    origin_endpoint_input_type_violation, SubmittedOriginEndpointMembers,
};

use crate::channel_lifecycle::{error_type, Emulator};
use crate::multiview_configuration_validation::multiview;

/// The CMAF channels a multiview row lists as tile sources:
/// `MultiviewConfiguration$AvailableSources` requires each source to be a channel of the
/// same group with an `InputType` of `CMAF`.
const SOURCES: [&str; 2] = ["tile-one", "tile-two"];

/// A member a row pins on every projection of the created endpoint: the response member
/// and the value it must report. A `fn` pointer rather than a `Value`, so a row stays a
/// `const`-friendly table entry.
type PinnedMember = (&'static str, fn() -> Value);

/// What a create carrying the row's settings must answer.
enum Expected {
    /// 200, with these members reported by every projection. `Value::Null` means the
    /// endpoint carries none.
    Stored(&'static [PinnedMember]),
    /// 400 `ValidationException` with this `ValidationExceptionType`, naming this member
    /// path, with nothing stored.
    Refused(&'static str, &'static str),
}

/// One row: the parent channel's input type, the endpoint the create asks for, and what
/// the service answers.
struct Case {
    label: &'static str,
    input_type: Option<&'static str>,
    /// The `CreateOriginEndpoint` body, container type and manifests included.
    body: fn() -> Value,
    expected: Expected,
}

/// The failover settings a row submits, from the model's `EndpointErrorCondition` enum.
fn force_endpoint_error() -> Value {
    json!({"EndpointErrorConditions": ["STALE_MANIFEST", "MISSING_DRM_KEY"]})
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "failover settings under a CMAF-input channel",
            input_type: Some("CMAF"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "ForceEndpointErrorConfiguration": force_endpoint_error(),
                })
            },
            expected: Expected::Stored(&[(
                "ForceEndpointErrorConfiguration",
                force_endpoint_error,
            )]),
        },
        Case {
            label: "failover settings under an HLS-input channel",
            input_type: Some("HLS"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "ForceEndpointErrorConfiguration": force_endpoint_error(),
                })
            },
            expected: Expected::Refused(
                "ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION",
                "forceEndpointErrorConfiguration",
            ),
        },
        Case {
            label: "failover settings under a channel naming no input type",
            input_type: None,
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "ForceEndpointErrorConfiguration": force_endpoint_error(),
                })
            },
            expected: Expected::Refused(
                "ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION",
                "forceEndpointErrorConfiguration",
            ),
        },
        Case {
            label: "failover settings under a multiview channel",
            input_type: Some("MULTIVIEW"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "ForceEndpointErrorConfiguration": force_endpoint_error(),
                })
            },
            expected: Expected::Refused(
                "ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION",
                "forceEndpointErrorConfiguration",
            ),
        },
        Case {
            label: "an ISM container under an HLS-input channel",
            input_type: Some("HLS"),
            body: || {
                json!({
                    "ContainerType": "ISM",
                    "MssManifests": [{"ManifestName": "smooth"}],
                })
            },
            expected: Expected::Stored(&[("ContainerType", || json!("ISM"))]),
        },
        Case {
            label: "an ISM container under a multiview channel",
            input_type: Some("MULTIVIEW"),
            body: || {
                json!({
                    "ContainerType": "ISM",
                    "MssManifests": [{"ManifestName": "smooth"}],
                })
            },
            expected: Expected::Refused("MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER", "containerType"),
        },
        Case {
            label: "a low-latency HLS manifest under a CMAF-input channel",
            input_type: Some("CMAF"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "LowLatencyHlsManifests": [{"ManifestName": "low-latency"}],
                })
            },
            expected: Expected::Stored(&[]),
        },
        Case {
            label: "a low-latency HLS manifest under a multiview channel",
            input_type: Some("MULTIVIEW"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "LowLatencyHlsManifests": [{"ManifestName": "low-latency"}],
                })
            },
            expected: Expected::Refused(
                "MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST",
                "lowLatencyHlsManifests",
            ),
        },
        Case {
            label: "a startover window under an HLS-input channel",
            input_type: Some("HLS"),
            body: || {
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "StartoverWindowSeconds": 300,
                })
            },
            expected: Expected::Stored(&[("StartoverWindowSeconds", || json!(300))]),
        },
        Case {
            label: "no startover window under a CMAF-input channel: the service applies one",
            input_type: Some("CMAF"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            // `state::DEFAULT_STARTOVER_WINDOW_SECONDS`, the value every endpoint whose
            // parent carries the member reports when the create names none.
            expected: Expected::Stored(&[("StartoverWindowSeconds", || json!(900))]),
        },
        Case {
            label: "a startover window under a multiview channel",
            input_type: Some("MULTIVIEW"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                    "StartoverWindowSeconds": 300,
                })
            },
            expected: Expected::Refused(
                "MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED",
                "startoverWindowSeconds",
            ),
        },
        Case {
            label: "no startover window under a multiview channel: none is applied either",
            input_type: Some("MULTIVIEW"),
            body: || {
                json!({
                    "ContainerType": "CMAF",
                    "HlsManifests": [{"ManifestName": "index"}],
                })
            },
            expected: Expected::Stored(&[("StartoverWindowSeconds", || Value::Null)]),
        },
    ]
}

/// A channel body for one input type: `None` names no input type at all, and a multiview
/// channel carries the configuration its input type requires.
fn channel_body(input_type: Option<&str>) -> Value {
    match input_type {
        None => json!({}),
        Some("MULTIVIEW") => json!({
            "InputType": "MULTIVIEW",
            "MultiviewConfiguration": multiview(&SOURCES, &["LAYOUT_2EH"]),
        }),
        Some(input_type) => json!({"InputType": input_type}),
    }
}

/// A channel group holding one channel of the given shape, with the tile sources a
/// multiview shape names seeded as CMAF channels of the same group first.
async fn parent_chain(emulator: &Emulator, case: &str, group: &str, channel: &str, body: Value) {
    let (status, answer) = emulator.create_group(group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannelGroup expected 200, got {status} {answer}"
    );
    if body.get("InputType") == Some(&json!("MULTIVIEW")) {
        for source in SOURCES {
            let (status, answer) = emulator
                .create_channel(group, source, json!({"InputType": "CMAF"}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: seeding source {source} expected 200, got {status} {answer}"
            );
        }
    }
    let (status, answer) = emulator.create_channel(group, channel, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannel expected 200, got {status} {answer}"
    );
}

/// Assert that `body` is this service's `ValidationException` carrying `expected_type` and
/// naming `member`.
fn assert_refused(case: &str, status: u16, body: &Value, expected_type: &str, member: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body.get("ValidationExceptionType")
            .and_then(Value::as_str)
            .unwrap_or(""),
        expected_type,
        "case {case:?}: expected ValidationExceptionType {expected_type}, got {body}"
    );
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.starts_with(&format!("Validation error encountered at request.{member}:")),
        "case {case:?}: the refusal must name {member} in this service's message form, \
         got {body}"
    );
}

/// RULE, on the create — a setting restricted to particular parent input types is carried
/// only under a channel that has one of them: elsewhere it is refused with the model's own
/// discriminator and nothing is stored, and a restricted setting with a default is
/// materialised only where it may be carried.
#[tokio::test]
async fn an_endpoints_settings_follow_the_parent_channels_input_type() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let group = format!("input-group-{index}");
        let channel = format!("input-channel-{index}");
        let endpoint = format!("input-endpoint-{index}");
        parent_chain(
            &emulator,
            label,
            &group,
            &channel,
            channel_body(case.input_type),
        )
        .await;

        let (status, created) = emulator
            .create_endpoint(&group, &channel, &endpoint, (case.body)())
            .await;

        match case.expected {
            Expected::Refused(expected_type, member) => {
                assert_refused(label, status, &created, expected_type, member);
                let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must mint nothing, got {status} {read}"
                );
            }
            Expected::Stored(expected) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
                );
                let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
                );
                for (member, value) in expected {
                    let value = value();
                    for (projection, body) in [("the create", &created), ("the read", &read)] {
                        let actual = body.get(member).cloned().unwrap_or(Value::Null);
                        assert_eq!(
                            actual, value,
                            "case {label:?}: {projection} must report {member} as {value}, \
                             got {body}"
                        );
                    }
                }
            }
        }
    }
}

/// RULE, through the update — `UpdateOriginEndpoint` writes the same settings, so it reads
/// the same table: a setting its parent channel may not carry is refused and the endpoint
/// is left as the create stored it.
#[tokio::test]
async fn an_update_reads_the_same_input_type_restrictions() {
    let emulator = Emulator::new();
    let group = "input-update-group";
    let channel = "input-update-channel";
    let endpoint = "input-update-endpoint";
    parent_chain(
        &emulator,
        "the update's parents",
        group,
        channel,
        channel_body(Some("MULTIVIEW")),
    )
    .await;
    let stored = json!({
        "ContainerType": "CMAF",
        "HlsManifests": [{"ManifestName": "index"}],
    });
    let (status, created) = emulator
        .create_endpoint(group, channel, endpoint, stored.clone())
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint expected 200, got {status} {created}"
    );

    // (label, the update body, the discriminator and member the refusal must carry)
    let rows: [(&str, Value, &str, &str); 3] = [
        (
            "an update adding a startover window",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "index"}],
                "StartoverWindowSeconds": 60,
            }),
            "MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED",
            "startoverWindowSeconds",
        ),
        (
            "an update adding a low-latency HLS manifest",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "index"}],
                "LowLatencyHlsManifests": [{"ManifestName": "low-latency"}],
            }),
            "MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST",
            "lowLatencyHlsManifests",
        ),
        (
            "an update adding failover settings",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "index"}],
                "ForceEndpointErrorConfiguration": force_endpoint_error(),
            }),
            "ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION",
            "forceEndpointErrorConfiguration",
        ),
    ];

    for (label, body, expected_type, member) in rows {
        let (status, answer) = emulator
            .update_endpoint(group, channel, endpoint, body, &[])
            .await;
        assert_refused(label, status, &answer, expected_type, member);
        let (status, read) = emulator.get_endpoint(group, channel, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the endpoint must still be readable, got {status} {read}"
        );
        assert_eq!(
            read["ETag"], created["ETag"],
            "case {label:?}: a refused update must not rotate the entity tag, got {read}"
        );
        assert_eq!(
            read.get("StartoverWindowSeconds").cloned(),
            None,
            "case {label:?}: the endpoint must still carry no startover window, got {read}"
        );
    }
}

/// RULE, at the one combination no request can reach end to end — an MSS manifest is
/// served by the ISM container alone (`CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST` and
/// `TS_CONTAINER_TYPE_WITH_MSS_MANIFEST` refuse the others), and under a multiview channel
/// the ISM container is itself refused first, so the MSS restriction is asserted on the
/// shared rule the handlers read.
#[test]
fn a_multiview_channels_endpoint_carries_no_mss_manifest() {
    // (label, the input type, whether the set is refused)
    let rows: [(&str, &str, bool); 3] = [
        ("under a multiview channel", "MULTIVIEW", true),
        ("under an HLS-input channel", "HLS", false),
        ("under a CMAF-input channel", "CMAF", false),
    ];

    for (label, input_type, refused) in rows {
        let violation = origin_endpoint_input_type_violation(
            SubmittedOriginEndpointMembers {
                mss_manifests: true,
                ..SubmittedOriginEndpointMembers::default()
            },
            input_type,
        );
        match refused {
            true => {
                let violation = violation.unwrap_or_else(|| {
                    panic!("case {label:?}: expected a refusal, got none")
                });
                assert_eq!(
                    violation.validation_exception_type.as_deref(),
                    Some("MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST"),
                    "case {label:?}: expected the MSS discriminator, got {violation:?}"
                );
            }
            false => assert!(
                violation.is_none(),
                "case {label:?}: expected no refusal, got {violation:?}"
            ),
        }
    }
}
