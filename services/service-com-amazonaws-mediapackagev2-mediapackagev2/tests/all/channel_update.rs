// SPDX-License-Identifier: CC0-1.0

//! What `ListChannels` reports after a channel is UPDATED, driven over HTTP through the
//! same router the emulator boots, reusing the harness in [`crate::channel_lifecycle`].
//!
//! The rule under test is the listing rule this crate's `ListChannels` implements: the
//! list is a projection of the STORE, so whatever a committed `UpdateChannel` wrote is
//! what the list and `GetChannel` both report, and whatever a refused one did not write
//! is absent from both. The rows vary which editable members the update carries, since
//! that is what the rule generalises over.
//!
//! `UpdateChannel`'s own contract (which members are editable, how the
//! `x-amzn-update-if-match` precondition resolves) is covered by that operation's
//! suites; here it is the writer whose effect the list has to show.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// `PUT /channelGroup/{group}/channel/{channel}/` — the `UpdateChannel` binding from the
/// model's `http` trait. `if_match` travels in `x-amzn-update-if-match`, the header the
/// model binds `ETag` to.
async fn update_channel(
    emulator: &Emulator,
    group: &str,
    channel: &str,
    body: Value,
    if_match: Option<&str>,
) -> (u16, Value) {
    let headers: Vec<(&str, &str)> = if_match
        .map(|tag| vec![("x-amzn-update-if-match", tag)])
        .unwrap_or_default();
    emulator
        .call_with_headers(
            "PUT",
            &format!("/channelGroup/{group}/channel/{channel}/"),
            Some(body),
            &headers,
        )
        .await
}

/// One `ListChannels` item, by channel name.
async fn listed(emulator: &Emulator, group: &str, channel: &str) -> Value {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/channelGroup/{group}/channel?maxResults=100"),
            None,
        )
        .await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status} {body}");
    body["Items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["ChannelName"] == json!(channel))
                .cloned()
        })
        .unwrap_or_else(|| panic!("channel {channel:?} is missing from {body}"))
}

/// One row of the update table: the body the channel is created with and the body the
/// update carries.
struct UpdateCase {
    label: &'static str,
    create: fn() -> Value,
    update: fn() -> Value,
}

/// The rows. They differ in which editable members the create and the update carry, so
/// between them they cover setting a member, changing it, clearing it by omission and
/// leaving a member that was never set absent. The member names and the `InputType` /
/// `OutputLockingMode` values come from the model's `UpdateChannelRequest`,
/// `InputSwitchConfiguration`, `OutputHeaderConfiguration`, `MultiviewConfiguration`,
/// `InputType` and `OutputLockingMode` shapes.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "a description is added to a bare channel",
            create: || json!({}),
            update: || json!({"Description": "now described"}),
        },
        UpdateCase {
            label: "a description is replaced",
            create: || json!({"Description": "the first"}),
            update: || json!({"Description": "the second"}),
        },
        UpdateCase {
            label: "an omitted description is cleared by the PUT",
            create: || json!({"Description": "about to go"}),
            update: || json!({}),
        },
        UpdateCase {
            label: "an empty description is a value, not an absence",
            create: || json!({"Description": "about to be empty"}),
            update: || json!({"Description": ""}),
        },
        UpdateCase {
            label: "input switching and output headers are set on a CMAF channel",
            create: || json!({"InputType": "CMAF"}),
            update: || {
                json!({
                    "InputSwitchConfiguration": {
                        "MQCSInputSwitching": true,
                        "PreferredInput": 2,
                    },
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
        },
        UpdateCase {
            label: "input switching is changed and output headers are cleared",
            create: || {
                json!({
                    "InputType": "CMAF",
                    "OutputLockingMode": "EPOCH_LOCKED",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            update: || json!({"InputSwitchConfiguration": {"MQCSInputSwitching": false}}),
        },
        UpdateCase {
            label: "a multiview channel's sources and layouts are changed",
            create: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["src-a"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            update: || {
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": ["src-a", "src-b"],
                        "AvailableLayouts": ["LAYOUT_3EL", "LAYOUT_4PL"],
                    },
                })
            },
        },
        UpdateCase {
            label: "an update carrying nothing clears every editable member",
            create: || {
                json!({
                    "InputType": "CMAF",
                    "Description": "everything set",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true},
                    "OutputHeaderConfiguration": {"PublishMQCS": false},
                    "tags": {"team": "video"},
                })
            },
            update: || json!({}),
        },
    ]
}

/// A committed update is what the list reports: every member the two shapes share equals
/// what `GetChannel` reads back, and the channel's identity is unchanged by the write.
#[tokio::test]
async fn a_committed_update_is_what_the_list_reports() {
    /// The members `ChannelListConfiguration` shares with `GetChannelResponse`, from the
    /// model's two shapes.
    const SHARED_MEMBERS: &[&str] = &[
        "Arn",
        "ChannelName",
        "ChannelGroupName",
        "CreatedAt",
        "ModifiedAt",
        "Description",
        "InputType",
        "OutputLockingMode",
        "MultiviewConfiguration",
    ];

    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.label;
        let channel = format!("chan-{index}");
        let emulator = Emulator::new();
        assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
        // The sources a multiview row names have to be CMAF channels of the same group,
        // which the model's `MultiviewConfiguration` doc requires of a tile source. They
        // are seeded for every row: the rows the list assertions address are named
        // `chan-<index>`, so these never stand in for one.
        for source in ["src-a", "src-b"] {
            assert_eq!(
                emulator
                    .create_channel("grp", source, json!({"InputType": "CMAF"}))
                    .await
                    .0,
                200,
                "case {label:?}: seeding the CMAF source {source} must succeed"
            );
        }
        let (status, created) = emulator
            .create_channel("grp", &channel, (case.create)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );

        let (status, updated) =
            update_channel(&emulator, "grp", &channel, (case.update)(), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
        );

        // What the writer answered is what the readers report.
        let (status, read) = emulator.get_channel("grp", &channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        let item = listed(&emulator, "grp", &channel).await;
        for member in SHARED_MEMBERS {
            assert_eq!(
                item[member], read[member],
                "case {label:?}: the listed {member} must equal what GetChannel reports \
                 after the update"
            );
            assert_eq!(
                read[member], updated[member],
                "case {label:?}: GetChannel {member} must equal what the update stored"
            );
        }
        // The list form reports no ingest endpoints, ETag or tags: those are a `Get`'s.
        for member in ["IngestEndpoints", "ETag", "Tags"] {
            assert_eq!(
                item[member],
                Value::Null,
                "case {label:?}: the list projection must not carry {member}, got {item}"
            );
        }
        // The identity the create derived survives the write.
        for member in ["Arn", "ChannelName", "ChannelGroupName", "CreatedAt"] {
            assert_eq!(
                item[member], created[member],
                "case {label:?}: {member} must be unchanged by an update"
            );
        }
        assert!(
            item["ModifiedAt"].as_f64() >= created["ModifiedAt"].as_f64(),
            "case {label:?}: ModifiedAt must not go backwards: {} then {}",
            created["ModifiedAt"],
            item["ModifiedAt"]
        );
    }
}

/// An update the service refuses writes nothing, so the list keeps reporting the channel
/// as it was: the rows are the three ways a presented entity tag can fail to be the
/// committed one.
#[tokio::test]
async fn a_refused_update_leaves_the_list_as_it_was() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    let (_, created) = emulator
        .create_channel("grp", "chan", json!({"Description": "original"}))
        .await;
    let first_tag = created["ETag"]
        .as_str()
        .expect("the create reports an ETag")
        .to_string();

    // The committed tag is accepted, and the commit mints a new one.
    let (status, updated) = update_channel(
        &emulator,
        "grp",
        "chan",
        json!({"Description": "conditioned"}),
        Some(&first_tag),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update carrying the committed ETag must be accepted, got {status} {updated}"
    );
    let current_tag = updated["ETag"]
        .as_str()
        .expect("the update reports an ETag")
        .to_string();
    assert_ne!(
        current_tag, first_tag,
        "a commit must move the entity tag on, so the previous one is stale"
    );
    let committed = listed(&emulator, "grp", "chan").await;
    assert_eq!(
        committed["Description"],
        json!("conditioned"),
        "the list must report the committed update: {committed}"
    );

    for (label, tag) in [
        ("the tag from before the commit", first_tag.as_str()),
        ("a tag this service never issued", "not-a-real-etag"),
        ("a tag of the right shape that is not this channel's", "deadbeef"),
    ] {
        let (status, body) = update_channel(
            &emulator,
            "grp",
            "chan",
            json!({"Description": "must not land"}),
            Some(tag),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected 409, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {body}"
        );
        assert_eq!(
            listed(&emulator, "grp", "chan").await,
            committed,
            "case {label:?}: a refused update must leave the listed channel as it was"
        );
    }
}
