// SPDX-License-Identifier: CC0-1.0

//! An origin-endpoint member the model restricts to particular PARENT-CHANNEL input
//! types.
//!
//! The rule under test, asserted over a SET of inputs rather than over one recorded
//! request: a member whose model doc both documents a service-applied default and names
//! the channel `InputType`s the setting is valid for is carried only by an endpoint whose
//! parent channel has one of those input types — the default is materialised and reported
//! there and nowhere else, and a value submitted for a channel that may not carry the
//! member is refused with the `ValidationExceptionType` the model names for that
//! combination instead of being stored.
//!
//! `StreamNameOutputMode` is the endpoint member documented that way ("The output mode
//! for stream names in egress manifests. This setting is valid only when the associated
//! channel's `InputType` is `HLS`. ... If you don't specify a value, the default is
//! `INDEX`." —
//! `smithy_query(query="doc", shape="CreateOriginEndpointRequest", member="StreamNameOutputMode")`),
//! and `ValidationExceptionType` names
//! `ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE` for a submitted one.
//!
//! Every row differs along what the rule generalises over: the parent channel's input
//! type (the model's whole `InputType` enum — HLS | CMAF | MULTIVIEW — plus the absent
//! case its doc defaults to HLS), whether the request names the member and which of the
//! `StreamNameOutputMode` enum's values it names (INDEX | PASSTHROUGH_NAME), and the
//! endpoint's own container type and manifest family.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// The CMAF channels a multiview row lists as tile sources:
/// `MultiviewConfiguration$AvailableSources`' doc requires each source to be in the same
/// channel group and to have an `InputType` of `CMAF`.
const SOURCES: [&str; 2] = ["tile-source-one", "tile-source-two"];

/// The member this suite reads back on every projection of an endpoint.
const MEMBER: &str = "StreamNameOutputMode";

/// The `ValidationExceptionType` the model names for a stream-name output mode submitted
/// for a channel whose input type may not carry the member.
const ONLY_HLS: &str = "ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE";

/// The `ValidationExceptionType` for changing the mode of an endpoint that HAS one.
const IMMUTABLE: &str = "STREAM_NAME_OUTPUT_MODE_IMMUTABLE";

/// The member of a body, or `Value::Null` when it is absent — an ABSENT member is half of
/// what this suite asserts, so a missing one must compare unequal to a reported value.
fn member(body: &Value) -> Value {
    body.get(MEMBER).cloned().unwrap_or(Value::Null)
}

/// Assert that `body` is this service's own `ValidationException` carrying
/// `expected_type`.
fn assert_validation(case: &str, status: u16, body: &Value, expected_type: &str) {
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
    assert!(
        body.get("Message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .starts_with("Validation error encountered at request"),
        "case {case:?}: expected this service's validation message form, got {body}"
    );
}

/// A channel group holding one channel of the given shape, with any tile sources the
/// shape names seeded as CMAF channels of the same group first.
async fn parent_chain(emulator: &Emulator, case: &str, group: &str, channel: &str, body: Value) {
    let (status, answer) = emulator.create_group(group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannelGroup expected 200, got {status} {answer}"
    );
    let sources = body
        .pointer("/MultiviewConfiguration/AvailableSources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for source in &sources {
        let source = source.as_str().unwrap_or_default();
        let (status, answer) = emulator
            .create_channel(group, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding source {source} expected 200, got {status} {answer}"
        );
    }
    let (status, answer) = emulator.create_channel(group, channel, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannel expected 200, got {status} {answer}"
    );
}

/// A channel body for one input type: `None` names no input type at all, and a multiview
/// channel carries the configuration its input type requires.
fn channel_body(input_type: Option<&str>) -> Value {
    match input_type {
        None => json!({}),
        Some("MULTIVIEW") => json!({
            "InputType": "MULTIVIEW",
            "MultiviewConfiguration": {
                "AvailableSources": SOURCES,
                "AvailableLayouts": ["LAYOUT_2EH"],
            },
        }),
        Some(input_type) => json!({"InputType": input_type}),
    }
}

/// What a create is expected to answer.
enum Expected {
    /// 200, with every projection reporting this mode — `None` for a member no projection
    /// carries at all.
    Stored(Option<&'static str>),
    /// 400 `ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE`, with nothing stored.
    Refused,
}

/// One row: the parent channel's input type, the endpoint the create asks for, and what
/// the service answers.
struct Case {
    label: &'static str,
    input_type: Option<&'static str>,
    container: &'static str,
    /// The `StreamNameOutputMode` the create names, if it names one.
    submitted: Option<&'static str>,
    expected: Expected,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no input type at all (the doc's default is HLS), no mode named",
            input_type: None,
            container: "TS",
            submitted: None,
            expected: Expected::Stored(Some("INDEX")),
        },
        Case {
            label: "HLS named, no mode named: the documented default is applied",
            input_type: Some("HLS"),
            container: "TS",
            submitted: None,
            expected: Expected::Stored(Some("INDEX")),
        },
        Case {
            label: "HLS, an ISM container with an MSS manifest and no mode named",
            input_type: Some("HLS"),
            container: "ISM",
            submitted: None,
            expected: Expected::Stored(Some("INDEX")),
        },
        Case {
            label: "HLS, the default mode named explicitly",
            input_type: Some("HLS"),
            container: "CMAF",
            submitted: Some("INDEX"),
            expected: Expected::Stored(Some("INDEX")),
        },
        Case {
            label: "HLS, the non-default mode named",
            input_type: Some("HLS"),
            container: "TS",
            submitted: Some("PASSTHROUGH_NAME"),
            expected: Expected::Stored(Some("PASSTHROUGH_NAME")),
        },
        Case {
            label: "CMAF, no mode named: no mode to default to, so none is reported",
            input_type: Some("CMAF"),
            container: "CMAF",
            submitted: None,
            expected: Expected::Stored(None),
        },
        Case {
            label: "CMAF, the default mode named explicitly",
            input_type: Some("CMAF"),
            container: "CMAF",
            submitted: Some("INDEX"),
            expected: Expected::Refused,
        },
        Case {
            label: "CMAF, the non-default mode named",
            input_type: Some("CMAF"),
            container: "TS",
            submitted: Some("PASSTHROUGH_NAME"),
            expected: Expected::Refused,
        },
        Case {
            label: "MULTIVIEW, no mode named",
            input_type: Some("MULTIVIEW"),
            container: "CMAF",
            submitted: None,
            expected: Expected::Stored(None),
        },
        Case {
            label: "MULTIVIEW, the non-default mode named",
            input_type: Some("MULTIVIEW"),
            container: "CMAF",
            submitted: Some("PASSTHROUGH_NAME"),
            expected: Expected::Refused,
        },
    ]
}

/// RULE — an endpoint carries the restricted member only under a parent channel whose
/// input type may carry it: there the documented default is materialised and every
/// projection reports it, elsewhere no projection reports one and a submitted value is
/// refused with the model's own discriminator.
#[tokio::test]
async fn a_restricted_endpoint_member_follows_the_parent_channels_input_type() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let group = format!("restricted-group-{index}");
        let channel = format!("restricted-channel-{index}");
        let endpoint = format!("restricted-endpoint-{index}");
        parent_chain(
            &emulator,
            label,
            &group,
            &channel,
            channel_body(case.input_type),
        )
        .await;

        let mut body = minimal_endpoint(case.container);
        if let Some(submitted) = case.submitted {
            body[MEMBER] = json!(submitted);
        }
        let (status, created) = emulator
            .create_endpoint(&group, &channel, &endpoint, body)
            .await;

        match case.expected {
            Expected::Refused => {
                assert_validation(label, status, &created, ONLY_HLS);
                let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must store no endpoint, got {status} {read}"
                );
            }
            Expected::Stored(expected) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
                );
                let expected = expected.map_or(Value::Null, |mode| json!(mode));
                let actual = member(&created);
                assert_eq!(
                    actual, expected,
                    "case {label:?}: CreateOriginEndpoint {MEMBER} expected {expected}, \
                     got {created}"
                );

                let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
                );
                let actual = member(&read);
                assert_eq!(
                    actual, expected,
                    "case {label:?}: GetOriginEndpoint {MEMBER} expected {expected}, got {read}"
                );

                let (status, listed) = emulator.list_endpoints(&group, &channel, "").await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: ListOriginEndpoints expected 200, got {status} {listed}"
                );
                let item = listed
                    .pointer("/Items/0")
                    .cloned()
                    .unwrap_or(Value::Null);
                let actual = member(&item);
                assert_eq!(
                    actual, expected,
                    "case {label:?}: ListOriginEndpoints item {MEMBER} expected {expected}, \
                     got {listed}"
                );
            }
        }
    }
}

/// RULE, through the update — which refusal a submitted mode earns follows the parent
/// channel's input type too: under a channel that may carry the member the endpoint HAS a
/// mode, so naming another one changes an immutable member; under a channel that may not,
/// the endpoint has no mode and a submitted one is refused for the input type. An update
/// that names no mode is accepted either way and leaves the stored state as it was.
#[tokio::test]
async fn which_refusal_an_update_earns_follows_the_parent_channels_input_type() {
    // (input type, the mode the created endpoint reports, the mode an update names, the
    // discriminator that update is refused with)
    let rows: [(Option<&str>, Option<&str>, &str, &str); 4] = [
        (None, Some("INDEX"), "PASSTHROUGH_NAME", IMMUTABLE),
        (Some("HLS"), Some("INDEX"), "PASSTHROUGH_NAME", IMMUTABLE),
        (Some("CMAF"), None, "INDEX", ONLY_HLS),
        (Some("MULTIVIEW"), None, "PASSTHROUGH_NAME", ONLY_HLS),
    ];

    for (index, (input_type, stored, requested, expected_type)) in rows.into_iter().enumerate() {
        let label = format!("{input_type:?} updated with {requested}");
        let emulator = Emulator::new();
        let group = format!("update-group-{index}");
        let channel = format!("update-channel-{index}");
        let endpoint = "update-endpoint";
        parent_chain(
            &emulator,
            &label,
            &group,
            &channel,
            channel_body(input_type),
        )
        .await;
        let base = minimal_endpoint("CMAF");
        let (status, created) = emulator
            .create_endpoint(&group, &channel, endpoint, base.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        let stored_value = stored.map_or(Value::Null, |mode| json!(mode));
        assert_eq!(
            member(&created), stored_value,
            "case {label:?}: the create must report {stored_value}, got {created}"
        );

        // An update that names no mode keeps the endpoint exactly as it was.
        let (status, updated) = emulator
            .update_endpoint(&group, &channel, endpoint, base.clone(), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update naming no mode expected 200, got {status} {updated}"
        );
        assert_eq!(
            member(&updated), stored_value,
            "case {label:?}: an update naming no mode must preserve the member, got {updated}"
        );

        // An update that names a mode the endpoint does not hold is refused — with the
        // discriminator the parent channel's input type decides.
        let mut body = base.clone();
        body[MEMBER] = json!(requested);
        let (status, refused) = emulator
            .update_endpoint(&group, &channel, endpoint, body, &[])
            .await;
        assert_validation(&label, status, &refused, expected_type);
        let (_, read) = emulator.get_endpoint(&group, &channel, endpoint).await;
        assert_eq!(
            member(&read), stored_value,
            "case {label:?}: a refused update must leave the member as it was, got {read}"
        );

        // And an update that names the mode the endpoint DOES hold is accepted, where
        // there is one to name.
        if let Some(stored) = stored {
            let mut body = base.clone();
            body[MEMBER] = json!(stored);
            let (status, accepted) = emulator
                .update_endpoint(&group, &channel, endpoint, body, &[])
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: an update repeating the stored mode expected 200, \
                 got {status} {accepted}"
            );
            assert_eq!(
                member(&accepted), stored_value,
                "case {label:?}: the repeated mode must be kept, got {accepted}"
            );
        }
    }
}
