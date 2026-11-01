// SPDX-License-Identifier: CC0-1.0

//! An origin endpoint's `Segment.OutputTimestampMode`: the default the service applies,
//! the channels that carry the member at all, and its immutability after creation.
//!
//! The rule under test, asserted over a SET of channel configurations and over both
//! published modes rather than over one request: a member the model documents a
//! service-applied default for AND restricts to a subset of parents is materialised at
//! CREATE time on exactly the resources whose parent may carry it — so every projection
//! of such a resource reports a concrete value and no later write can be the one that
//! first sets it — while the member stays absent, and a request naming it is refused with
//! the `ValidationExceptionType` the model declares, wherever the parent may not carry
//! it.
//!
//! The sentences the rule comes from, all three in one member doc
//! (`smithy_query(query="doc", shape="Segment", member="OutputTimestampMode")`): "This
//! setting is only configurable on channels with `OutputLockingMode` set to
//! `NON_EPOCH_LOCKED`", "This value is immutable after endpoint creation" and "If you
//! don't specify a value, the default is `PASSTHROUGH`".
//!
//! The catalogues are the model's own enums
//! (`smithy_query(query="shape", shape="OutputTimestampMode,OutputLockingMode,InputType")`):
//! `OutputTimestampMode` is PASSTHROUGH | REBASED_TO_CHANNEL_START, `OutputLockingMode`
//! is EPOCH_LOCKED | NON_EPOCH_LOCKED and `InputType` is HLS | CMAF | MULTIVIEW — and
//! because an `OutputLockingMode` is itself only carried by some input types, and only at
//! some of its values, the channel rows below cover each input type with the locking mode
//! absent, with the default named and with the non-default one wherever a channel may
//! have it.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// Every value the model's `OutputTimestampMode` enum publishes.
const MODES: [&str; 2] = ["PASSTHROUGH", "REBASED_TO_CHANNEL_START"];

/// The mode the service applies to an endpoint whose channel carries the member and whose
/// create named no mode ("If you don't specify a value, the default is `PASSTHROUGH`").
const DEFAULT_MODE: &str = "PASSTHROUGH";

/// The `ValidationExceptionType` the model declares for a mode named where the channel's
/// output locking mode does not carry the member.
const NOT_CONFIGURABLE: &str = "ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE";

/// The `ValidationExceptionType` the model declares for a change to the stored mode.
const IMMUTABLE: &str = "OUTPUT_TIMESTAMP_MODE_IMMUTABLE";

/// One channel configuration an endpoint can hang off: the create body, whether its
/// output locking mode carries a segment output-timestamp mode, and the container type
/// the row's endpoints are created with (so the rows differ in manifest family too).
struct ChannelCase {
    label: &'static str,
    body: fn() -> Value,
    carries_mode: bool,
    container: &'static str,
}

/// Each input type with no locking mode, with `EPOCH_LOCKED` and — where a channel may
/// have it — with `NON_EPOCH_LOCKED`; only the last of those carries an output-timestamp
/// mode, and a channel that carries no locking mode, or that may only be epoch-locked,
/// never does.
fn channel_cases() -> Vec<ChannelCase> {
    vec![
        ChannelCase {
            label: "an HLS channel (the service's own input type), which carries no locking mode",
            body: || json!({}),
            carries_mode: false,
            container: "TS",
        },
        ChannelCase {
            label: "an HLS channel named explicitly",
            body: || json!({"InputType": "HLS"}),
            carries_mode: false,
            container: "CMAF",
        },
        ChannelCase {
            label: "a CMAF channel naming no locking mode, so the service applies EPOCH_LOCKED",
            body: || json!({"InputType": "CMAF"}),
            carries_mode: false,
            container: "ISM",
        },
        ChannelCase {
            label: "a CMAF channel locked to the epoch explicitly",
            body: || json!({"InputType": "CMAF", "OutputLockingMode": "EPOCH_LOCKED"}),
            carries_mode: false,
            container: "TS",
        },
        ChannelCase {
            label: "a CMAF channel that is NOT epoch-locked",
            body: || json!({"InputType": "CMAF", "OutputLockingMode": "NON_EPOCH_LOCKED"}),
            carries_mode: true,
            container: "TS",
        },
        ChannelCase {
            label: "a multiview channel, which can only be epoch-locked",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["source-feed"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            carries_mode: false,
            container: "CMAF",
        },
        ChannelCase {
            label: "a multiview channel naming the one locking mode it may have",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "OutputLockingMode": "EPOCH_LOCKED",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["source-feed"],
                        "AvailableLayouts": ["LAYOUT_4E"],
                    },
                })
            },
            carries_mode: false,
            container: "TS",
        },
    ]
}

/// The mode an endpoint response reports, or `Value::Null` when it reports none — an
/// ABSENT member is exactly what this suite tells apart from a concrete one.
fn reported_mode(body: &Value) -> Value {
    body.pointer("/Segment/OutputTimestampMode")
        .cloned()
        .unwrap_or(Value::Null)
}

/// An endpoint body for `container`, carrying `mode` when the case names one.
fn endpoint_body(container: &str, mode: Option<&str>) -> Value {
    let mut body = minimal_endpoint(container);
    if let Some(mode) = mode {
        body["Segment"] = json!({"OutputTimestampMode": mode});
    }
    body
}

/// Seed the group and the channel a case's endpoints hang off, named for the case index.
/// A multiview channel's available sources have to be CMAF channels of the same group
/// before it can name them, so each named source is created first.
async fn parent_chain(emulator: &Emulator, case: &ChannelCase, index: usize) -> (String, String) {
    let label = case.label;
    let group = format!("otm-group-{index}");
    let channel = format!("otm-channel-{index}");
    let (status, body) = emulator.create_group(&group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let channel_body = (case.body)();
    let sources = channel_body
        .pointer("/MultiviewConfiguration/AvailableSources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for source in &sources {
        let source = source.as_str().unwrap_or_default();
        let (status, seeded) = emulator
            .create_channel(&group, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: seeding source channel {source:?} expected 200, got {status} {seeded}"
        );
    }
    let (status, body) = emulator
        .create_channel(&group, &channel, channel_body)
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannel expected 200, got {status} {body}"
    );
    (group, channel)
}

/// RULE, the create side — an endpoint on a channel that carries the member reports a
/// concrete mode on every projection (the submitted one, else the documented default),
/// and an endpoint on a channel that does not carry it reports none and is refused when
/// its create names one.
#[tokio::test]
async fn an_endpoint_reports_a_mode_exactly_where_its_channel_carries_one() {
    for (index, case) in channel_cases().into_iter().enumerate() {
        let label = case.label;
        let emulator = Emulator::new();
        let (group, channel) = parent_chain(&emulator, &case, index).await;

        // The submitted-mode column: no mode at all, then each value the enum publishes.
        let submitted: Vec<Option<&str>> =
            ::std::iter::once(None).chain(MODES.map(Some)).collect();
        for (attempt, mode) in submitted.into_iter().enumerate() {
            let endpoint = format!("otm-endpoint-{attempt}");
            let (status, created) = emulator
                .create_endpoint(
                    &group,
                    &channel,
                    &endpoint,
                    endpoint_body(case.container, mode),
                )
                .await;

            match (case.carries_mode, mode) {
                // The channel carries the member: the create reports the mode it named,
                // or the service's own when it named none, and the read reports the same.
                (true, _) => {
                    let expected = json!(mode.unwrap_or(DEFAULT_MODE));
                    assert_eq!(
                        status, 200,
                        "case {label:?} / mode {mode:?}: CreateOriginEndpoint expected 200, \
                         got {status} {created}"
                    );
                    assert_eq!(
                        reported_mode(&created),
                        expected,
                        "case {label:?} / mode {mode:?}: the create must report the stored \
                         mode, got {created}"
                    );
                    let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                    assert_eq!(
                        status, 200,
                        "case {label:?} / mode {mode:?}: GetOriginEndpoint expected 200, \
                         got {status} {read}"
                    );
                    assert_eq!(
                        reported_mode(&read),
                        expected,
                        "case {label:?} / mode {mode:?}: the read must report the stored \
                         mode, got {read}"
                    );
                }
                // It does not, and the create names one: refused, and nothing is minted.
                (false, Some(_)) => {
                    assert_eq!(
                        status, 400,
                        "case {label:?} / mode {mode:?}: CreateOriginEndpoint expected 400, \
                         got {status} {created}"
                    );
                    assert_eq!(
                        error_type(&created),
                        "ValidationException",
                        "case {label:?} / mode {mode:?}: expected ValidationException, \
                         got {created}"
                    );
                    assert_eq!(
                        created["ValidationExceptionType"],
                        json!(NOT_CONFIGURABLE),
                        "case {label:?} / mode {mode:?}: expected {NOT_CONFIGURABLE}, \
                         got {created}"
                    );
                    let message = created["Message"].as_str().unwrap_or_default();
                    assert!(
                        message.contains("request.segment.outputTimestampMode"),
                        "case {label:?} / mode {mode:?}: the message must name the member \
                         the refusal is at, got {message:?}"
                    );
                    let (status, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                    assert_eq!(
                        status, 404,
                        "case {label:?} / mode {mode:?}: a refused create must mint no \
                         endpoint, got {status} {read}"
                    );
                }
                // It does not, and the create names none: no mode is invented.
                (false, None) => {
                    assert_eq!(
                        status, 200,
                        "case {label:?} / mode {mode:?}: CreateOriginEndpoint expected 200, \
                         got {status} {created}"
                    );
                    assert_eq!(
                        reported_mode(&created),
                        Value::Null,
                        "case {label:?}: a channel that carries no mode must leave the \
                         member absent, got {created}"
                    );
                    let (_, read) = emulator.get_endpoint(&group, &channel, &endpoint).await;
                    assert_eq!(
                        reported_mode(&read),
                        Value::Null,
                        "case {label:?}: the read must report no mode either, got {read}"
                    );
                }
            }
        }
    }
}

/// RULE, the update side — the stored mode survives every update: one that omits the
/// member, one that repeats it, and one that names the other mode, which is refused as a
/// change to an immutable member. On a channel that carries no mode there is nothing an
/// update can set: naming one is the same refusal the create answers.
#[tokio::test]
async fn an_update_can_never_change_the_stored_mode() {
    for (index, case) in channel_cases().into_iter().enumerate() {
        let label = case.label;
        // Each create column: the mode the endpoint is created with (none means the
        // service applies its own), paired with the mode an update then tries to put in
        // its place.
        for (created_with, other) in [
            (None, MODES[0]),
            (None, MODES[1]),
            (Some(MODES[0]), MODES[1]),
            (Some(MODES[1]), MODES[0]),
        ] {
            if !case.carries_mode && created_with.is_some() {
                // Covered by the create-side table: such a create is refused outright.
                continue;
            }
            if case.carries_mode && other == created_with.unwrap_or(DEFAULT_MODE) {
                // Not a CHANGE: an update repeating the stored mode is the accepted case
                // asserted below, not the refused one.
                continue;
            }
            let emulator = Emulator::new();
            let (group, channel) = parent_chain(&emulator, &case, index).await;
            let endpoint = "otm-endpoint";
            let (status, created) = emulator
                .create_endpoint(
                    &group,
                    &channel,
                    endpoint,
                    endpoint_body(case.container, created_with),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?} / created with {created_with:?}: expected 200, \
                 got {status} {created}"
            );
            let stored = reported_mode(&created);

            // An update that names no segment at all leaves the stored mode alone.
            let (status, updated) = emulator
                .update_endpoint(
                    &group,
                    &channel,
                    endpoint,
                    minimal_endpoint(case.container),
                    &[],
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?} / created with {created_with:?}: an update omitting the \
                 segment expected 200, got {status} {updated}"
            );
            assert_eq!(
                reported_mode(&updated),
                stored,
                "case {label:?} / created with {created_with:?}: an update omitting the \
                 member must preserve it, got {updated}"
            );

            // An update repeating the stored mode changes nothing, so it is accepted —
            // and on a channel that carries none, there is no stored mode to repeat, so
            // naming one is refused as not configurable.
            if case.carries_mode {
                let repeated = stored.as_str().unwrap_or_default().to_string();
                let (status, again) = emulator
                    .update_endpoint(
                        &group,
                        &channel,
                        endpoint,
                        endpoint_body(case.container, Some(&repeated)),
                        &[],
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {label:?} / created with {created_with:?}: an update repeating \
                     {repeated:?} expected 200, got {status} {again}"
                );
                assert_eq!(
                    reported_mode(&again),
                    stored,
                    "case {label:?} / created with {created_with:?}: repeating the stored \
                     mode must keep it, got {again}"
                );
            }

            // An update naming the OTHER mode is refused, with the discriminator for the
            // rule it breaks: the member is immutable where the channel carries one, and
            // not configurable at all where it does not.
            let (status, refused) = emulator
                .update_endpoint(
                    &group,
                    &channel,
                    endpoint,
                    endpoint_body(case.container, Some(other)),
                    &[],
                )
                .await;
            let expected_type = if case.carries_mode {
                IMMUTABLE
            } else {
                NOT_CONFIGURABLE
            };
            assert_eq!(
                status, 400,
                "case {label:?} / created with {created_with:?} -> {other:?}: expected 400, \
                 got {status} {refused}"
            );
            assert_eq!(
                error_type(&refused),
                "ValidationException",
                "case {label:?} / created with {created_with:?} -> {other:?}: expected \
                 ValidationException, got {refused}"
            );
            assert_eq!(
                refused["ValidationExceptionType"],
                json!(expected_type),
                "case {label:?} / created with {created_with:?} -> {other:?}: expected \
                 {expected_type}, got {refused}"
            );

            // And it committed nothing: the endpoint still reports what it was created
            // with, through its own read.
            let (_, read) = emulator.get_endpoint(&group, &channel, endpoint).await;
            assert_eq!(
                reported_mode(&read),
                stored,
                "case {label:?} / created with {created_with:?} -> {other:?}: a refused \
                 update must leave the mode alone, got {read}"
            );
        }
    }
}
