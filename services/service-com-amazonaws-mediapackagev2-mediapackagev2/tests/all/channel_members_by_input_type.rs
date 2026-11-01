// SPDX-License-Identifier: CC0-1.0

//! Which server-applied members a channel carries, across the model's whole `InputType`
//! enum, driven end to end over the same router the emulator boots.
//!
//! The rule under test, asserted over a SET of channels rather than over one recorded
//! request: a member the service applies itself is materialised once, at create time, and
//! every projection of the channel reports the same value — and WHICH members a channel
//! carries follows the member's own documentation, never the shape of one request:
//!
//! * `InputSwitchConfiguration$MQCSInputSwitching` and
//!   `OutputHeaderConfiguration$PublishMQCS` are applied to a channel of EVERY input
//!   type, off unless the caller turned them on. The recorded service reports both
//!   booleans as `false` on a channel created with neither block, including a channel
//!   that is not CMAF (the `mediapackagev2-channel-lifecycle` and
//!   `mediapackagev2-multiple-origin-endpoints-isolation` wire fixtures, whose
//!   `CreateChannel` step expects `$.InputSwitchConfiguration.MQCSInputSwitching` and
//!   `$.OutputHeaderConfiguration.PublishMQCS` to be `false`), and the model states the
//!   default for the first of them ("Default is false").
//! * `OutputLockingMode`, documented with both a default ("If you don't specify a value,
//!   the default is `EPOCH_LOCKED`") and a restriction on which input types may carry it
//!   (the refusal the service answers for it names them: "only be configured when the
//!   channel InputType is CMAF or MULTIVIEW"), is applied
//!   only where the channel may carry one, and no projection of a channel that may not
//!   reports one — which is also what the recorded service reports for such a channel.
//!   The two sides of that restriction are pinned together in
//!   `tests/all/restricted_channel_members.rs`.
//! * `InputSwitchConfiguration$PreferredInput` has no documented default and the service
//!   reports none, so it is absent on every input type until a request names it.
//!
//! The rows span the model's whole `InputType` enum (HLS | CMAF | MULTIVIEW) plus the
//! absent case, which `CreateChannelRequest$InputType` defaults to HLS
//! (`smithy_query(query="shape", shape="InputType,InputSwitchConfiguration,OutputHeaderConfiguration,OutputLockingMode")`).

use serde_json::{json, Value};

// The crate's one emulator client, declared in the channel-lifecycle suite `pub(crate)`
// for exactly this reuse: every suite in this test target drives the emulator through it.
use crate::channel_lifecycle::{at, Emulator};

/// The multiview configuration a MULTIVIEW channel's own doc calls required on that input
/// type, over the CMAF source a multiview case seeds ("Each source channel must be in the
/// same channel group as the multiview channel, and must have an `InputType` of `CMAF`").
pub(crate) fn multiview_members() -> Value {
    json!({
        "MultiviewConfiguration": {
            "AvailableSources": ["tile-source"],
            "AvailableLayouts": ["LAYOUT_2EH"],
        },
    })
}

/// One row: a channel described by its input type and the members its create names, plus
/// what every projection of it must report for each server-applied member.
struct Case {
    label: &'static str,
    /// The `InputType` the create names, or `None` for the absent case.
    input_type: Option<&'static str>,
    /// CMAF channels to seed in the group first: a multiview tile source has to be one.
    sources: &'static [&'static str],
    /// The rest of the create body — the members an input type requires, and any the row
    /// names explicitly.
    members: fn() -> Value,
    /// `(pointer, value)` per server-applied member, `Value::Null` for one the channel
    /// does not carry.
    expected: fn() -> Vec<(&'static str, Value)>,
    /// The same, after an `UpdateChannel` that names none of them: the update REPLACES
    /// the two blocks, so their members fall back to the state the service applies,
    /// while `OutputLockingMode` — not an update's input at all — stays as it was.
    after_empty_update: fn() -> Vec<(&'static str, Value)>,
}

/// The state the service applies to a channel that named neither block, which is what an
/// update naming neither leaves behind.
fn applied_switching_state() -> Vec<(&'static str, Value)> {
    vec![
        ("/InputSwitchConfiguration/MQCSInputSwitching", json!(false)),
        ("/InputSwitchConfiguration/PreferredInput", Value::Null),
        ("/OutputHeaderConfiguration/PublishMQCS", json!(false)),
    ]
}

/// [`applied_switching_state`] plus the locking mode the channel keeps.
fn applied_state_with_locking_mode(mode: Value) -> Vec<(&'static str, Value)> {
    let mut expected = applied_switching_state();
    expected.push(("/OutputLockingMode", mode));
    expected
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no input type at all, so the channel is the default HLS",
            input_type: None,
            sources: &[],
            members: || json!({}),
            expected: || applied_state_with_locking_mode(Value::Null),
            after_empty_update: || applied_state_with_locking_mode(Value::Null),
        },
        Case {
            label: "an HLS channel naming the default input type explicitly",
            input_type: Some("HLS"),
            sources: &[],
            members: || json!({"Description": "hls ingest"}),
            expected: || applied_state_with_locking_mode(Value::Null),
            after_empty_update: || applied_state_with_locking_mode(Value::Null),
        },
        Case {
            label: "a CMAF channel naming none of the members",
            input_type: Some("CMAF"),
            sources: &[],
            members: || json!({}),
            expected: || applied_state_with_locking_mode(json!("EPOCH_LOCKED")),
            after_empty_update: || applied_state_with_locking_mode(json!("EPOCH_LOCKED")),
        },
        Case {
            label: "a CMAF channel turning both MQCS members on and preferring input 2",
            input_type: Some("CMAF"),
            sources: &[],
            members: || {
                json!({
                    "OutputLockingMode": "NON_EPOCH_LOCKED",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 2},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            expected: || {
                vec![
                    ("/InputSwitchConfiguration/MQCSInputSwitching", json!(true)),
                    ("/InputSwitchConfiguration/PreferredInput", json!(2)),
                    ("/OutputHeaderConfiguration/PublishMQCS", json!(true)),
                    ("/OutputLockingMode", json!("NON_EPOCH_LOCKED")),
                ]
            },
            // The update replaced the blocks it was given none of, and the locking mode
            // it cannot be given at all is the one the create stored.
            after_empty_update: || applied_state_with_locking_mode(json!("NON_EPOCH_LOCKED")),
        },
        Case {
            label: "a multiview channel, which receives no ingest of its own",
            input_type: Some("MULTIVIEW"),
            sources: &["tile-source"],
            members: multiview_members,
            // MULTIVIEW is the second input type that carries the locking mode, at the
            // one value a composition admits.
            expected: || applied_state_with_locking_mode(json!("EPOCH_LOCKED")),
            after_empty_update: || applied_state_with_locking_mode(json!("EPOCH_LOCKED")),
        },
        Case {
            label: "a multiview channel over two tile sources and two layouts",
            input_type: Some("MULTIVIEW"),
            sources: &["tile-source", "tile-source-b"],
            members: || {
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": ["tile-source", "tile-source-b"],
                        "AvailableLayouts": ["LAYOUT_2EH", "LAYOUT_4E"],
                    },
                })
            },
            expected: || applied_state_with_locking_mode(json!("EPOCH_LOCKED")),
            after_empty_update: || applied_state_with_locking_mode(json!("EPOCH_LOCKED")),
        },
    ]
}

/// A group holding this row's seeded tile sources, plus the create body for its channel.
async fn seeded_group(emulator: &Emulator, case: &Case, group: &str) -> Value {
    let (status, body) = emulator.create_group(group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateChannelGroup expected 200, got {status} {body}",
        case.label
    );
    for source in case.sources {
        let (status, body) = emulator
            .create_channel(group, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: seeding the CMAF source {source} expected 200, got {status} {body}",
            case.label
        );
    }
    let mut create = (case.members)();
    if let Some(input_type) = case.input_type {
        create["InputType"] = json!(input_type);
    }
    create
}

/// The `ListChannels` item for one channel of a group, or `Value::Null` when the list
/// does not carry it.
async fn listed_channel(emulator: &Emulator, group: &str, channel: &str) -> Value {
    let (status, body) = emulator.list_channels(group, "?maxResults=100").await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status} {body}");
    body["Items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["ChannelName"] == json!(channel))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// RULE — every projection of a channel (the create response, a read, and an update's
/// response) reports the members the service applied, and which of them the channel
/// carries follows the member's own documentation rather than the request: the two MQCS
/// booleans on a channel of any input type, the locking mode only where the input type
/// carries one. An update REPLACES the two blocks, so one that names neither leaves the
/// channel with the state the service applies — never with nothing, and never with a
/// member the channel does not carry.
#[tokio::test]
async fn a_channel_reports_the_server_applied_members_its_input_type_has() {
    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let emulator = Emulator::new();
        let group = format!("applied-group-{index}");
        let channel = "subject";
        let create = seeded_group(&emulator, &case, &group).await;

        let (status, created) = emulator.create_channel(&group, channel, create).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );
        let (status, read) = emulator.get_channel(&group, channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        // An update rewrites the whole configuration, so the members the service applies
        // are applied again: one that names none of them leaves the channel with the
        // state the service applies rather than with nothing, and may not introduce a
        // member the channel does not carry. `OutputLockingMode` is not an update's
        // input at all, so its response reports the stored one.
        let (status, updated) = emulator
            .update_channel(&group, channel, json!({}), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
        );

        for (projection, body, expected) in [
            ("CreateChannel", &created, (case.expected)()),
            ("GetChannel", &read, (case.expected)()),
            ("UpdateChannel", &updated, (case.after_empty_update)()),
        ] {
            for (pointer, expected) in expected {
                let actual = at(body, pointer);
                assert_eq!(
                    actual, expected,
                    "case {label:?}: {projection} {pointer} expected {expected}, got {actual}"
                );
            }
        }

        // What the update stored is what the next read reports.
        let (status, re_read) = emulator.get_channel(&group, channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel after the update expected 200, got {status} {re_read}"
        );
        for (pointer, expected) in (case.after_empty_update)() {
            let actual = at(&re_read, pointer);
            assert_eq!(
                actual, expected,
                "case {label:?}: GetChannel after the update {pointer} expected \
                 {expected}, got {actual}"
            );
        }

        // `ChannelListConfiguration` carries `OutputLockingMode` but neither block, so
        // the list item is held to the same expectation the read is: the mode this input
        // type carries, which for an input type that carries none is no member at all.
        let listed = listed_channel(&emulator, &group, channel).await;
        let expected = (case.after_empty_update)()
            .into_iter()
            .find(|(pointer, _)| *pointer == "/OutputLockingMode")
            .map(|(_, expected)| expected)
            .expect("every row states what OutputLockingMode it expects");
        let actual = at(&listed, "/OutputLockingMode");
        assert_eq!(
            actual, expected,
            "case {label:?}: the ListChannels item's OutputLockingMode expected \
             {expected}, got {actual}"
        );
        assert!(
            listed["ChannelName"] == json!(channel),
            "case {label:?}: the ListChannels item must be the channel under test, \
             got {listed}"
        );
    }
}
