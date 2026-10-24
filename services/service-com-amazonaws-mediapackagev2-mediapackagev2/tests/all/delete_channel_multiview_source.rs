// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rule the model states on
//! `GetChannelResponse$AttachedMultiviewChannels`, driven over HTTP through the same
//! router the emulator boots, reusing the harness in [`crate::channel_lifecycle`].
//!
//! The rule under test: `DeleteChannel` refuses with `ConflictException` (409,
//! `ConflictExceptionType` `RESOURCE_IN_USE`) and leaves the channel stored for exactly
//! as long as some OTHER channel of the SAME channel group still lists it in
//! `MultiviewConfiguration.AvailableSources`; once nothing does, the delete removes it.
//! So the refusal and the derived `AttachedMultiviewChannels` member are two readings of
//! one fact, and the store never holds a multiview channel whose available sources name
//! a channel that is gone.
//!
//! Each test asserts that over a SET of inputs: the rows vary how many channels name the
//! target, how long their source lists are and where in those lists the target sits,
//! whether the namer is in the target's own group, and which operation releases the
//! reference.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The group the table operates in.
const GROUP: &str = "grp";

/// A second channel group, seeded with the SAME channel names and a multiview channel
/// naming all of them. The model scopes the rule to "the multiview channels, in the same
/// channel group", so nothing this group holds may ever block a delete in [`GROUP`].
const OTHER_GROUP: &str = "other";

/// A layout value, from the model's `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL |
/// LAYOUT_3EL | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL). `AvailableLayouts` is required and
/// has a minimum length of 1; which value it carries is irrelevant to this rule, so every
/// row uses one that the published enum lists and the issue did not cite.
const LAYOUT: &str = "LAYOUT_3PL";

/// One row: a channel group's contents and the channel the row deletes.
struct Case {
    /// What this row is, for every assertion message.
    label: &'static str,
    /// The CMAF channels created in [`GROUP`] (and mirrored into [`OTHER_GROUP`]).
    sources: &'static [&'static str],
    /// The multiview channels created in [`GROUP`]: a name and its `AvailableSources`.
    multiview: &'static [(&'static str, &'static [&'static str])],
    /// The channel `DeleteChannel` is called on.
    target: &'static str,
    /// The multiview channels expected to block that delete, in name order. Empty means
    /// the delete must succeed.
    blocked_by: &'static [&'static str],
}

/// The rows. They span the class the rule generalises over: the number of channels naming
/// the target (none, one, two), the length of the naming source list (1, 2, 3), the
/// target's position in it (only, first, middle, last), and channels that are named by
/// nothing in their own group — including a multiview channel, which is itself deletable.
const CASES: &[Case] = &[
    Case {
        label: "a channel no multiview channel exists to name",
        sources: &["alpha"],
        multiview: &[],
        target: "alpha",
        blocked_by: &[],
    },
    Case {
        label: "the only source of one multiview channel",
        sources: &["alpha"],
        multiview: &[("tiled", &["alpha"])],
        target: "alpha",
        blocked_by: &["tiled"],
    },
    Case {
        label: "the first of three sources",
        sources: &["alpha", "bravo", "charlie"],
        multiview: &[("tiled", &["alpha", "bravo", "charlie"])],
        target: "alpha",
        blocked_by: &["tiled"],
    },
    Case {
        label: "the middle of three sources",
        sources: &["alpha", "bravo", "charlie"],
        multiview: &[("tiled", &["alpha", "bravo", "charlie"])],
        target: "bravo",
        blocked_by: &["tiled"],
    },
    Case {
        label: "the last of three sources",
        sources: &["alpha", "bravo", "charlie"],
        multiview: &[("tiled", &["alpha", "bravo", "charlie"])],
        target: "charlie",
        blocked_by: &["tiled"],
    },
    Case {
        label: "a source that two multiview channels name",
        sources: &["alpha", "bravo"],
        multiview: &[("tiled-one", &["alpha", "bravo"]), ("tiled-two", &["alpha"])],
        target: "alpha",
        blocked_by: &["tiled-one", "tiled-two"],
    },
    Case {
        label: "a source that only one of two multiview channels names",
        sources: &["alpha", "bravo"],
        multiview: &[("tiled-one", &["alpha", "bravo"]), ("tiled-two", &["alpha"])],
        target: "bravo",
        blocked_by: &["tiled-one"],
    },
    Case {
        label: "an unnamed channel beside a named one",
        sources: &["alpha", "bravo"],
        multiview: &[("tiled", &["alpha"])],
        target: "bravo",
        blocked_by: &[],
    },
    Case {
        label: "the multiview channel itself, which nothing names",
        sources: &["alpha"],
        multiview: &[("tiled", &["alpha"])],
        target: "tiled",
        blocked_by: &[],
    },
];

/// Create `GROUP` with the row's channels, and `OTHER_GROUP` with the same channel names
/// plus a multiview channel naming every one of them.
async fn seed(emulator: &Emulator, case: &Case) {
    for group in [GROUP, OTHER_GROUP] {
        let (status, body) = emulator.create_group(group, json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateChannelGroup {group} expected 200, got {status} {body}",
            case.label
        );
        for source in case.sources {
            let (status, body) = emulator
                .create_channel(group, source, json!({"InputType": "CMAF"}))
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: CreateChannel {group}/{source} expected 200, got {status} {body}",
                case.label
            );
        }
    }
    for (name, sources) in case.multiview {
        let (status, body) = emulator
            .create_channel(GROUP, name, multiview_body(sources))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateChannel {GROUP}/{name} expected 200, got {status} {body}",
            case.label
        );
    }
    // The other group names every one of its own channels from a multiview channel, so a
    // delete in `GROUP` that succeeds proves the rule is scoped to one channel group.
    if !case.sources.is_empty() {
        let (status, body) = emulator
            .create_channel(OTHER_GROUP, "tiled-elsewhere", multiview_body(case.sources))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateChannel {OTHER_GROUP}/tiled-elsewhere expected 200, got {status} {body}",
            case.label
        );
    }
}

/// A `CreateChannel`/`UpdateChannel` body for a multiview channel over `sources`.
fn multiview_body(sources: &[&str]) -> Value {
    json!({
        "InputType": "MULTIVIEW",
        "MultiviewConfiguration": {
            "AvailableSources": sources,
            "AvailableLayouts": [LAYOUT],
        },
    })
}

/// The `Message` of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// Assert one answer is the in-use refusal: 409, `ConflictException`, `RESOURCE_IN_USE`,
/// and a sentence that names what has to be updated first while echoing none of the
/// names the caller sent and no count of how many of them there are.
fn assert_in_use_refusal(label: &str, answer: &(u16, Value), identifiers: &[&str]) {
    let (status, body) = answer;
    assert_eq!(
        *status, 409,
        "case {label:?}: expected 409, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ConflictException",
        "case {label:?}: expected a ConflictException envelope, got {body}"
    );
    assert_eq!(
        body.get("ConflictExceptionType")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "RESOURCE_IN_USE",
        "case {label:?}: expected ConflictExceptionType RESOURCE_IN_USE, got {body}"
    );
    assert!(
        message(body).contains("multiview"),
        "case {label:?}: expected the message to name the multiview channels that must be \
         updated first, got {:?}",
        message(body)
    );
    assert!(
        !message(body).chars().any(char::is_numeric),
        "case {label:?}: the message must not report a referrer count, got {:?}",
        message(body)
    );
    for identifier in identifiers {
        assert!(
            !message(body).contains(identifier),
            "case {label:?}: the message must not echo {identifier:?}, got {:?}",
            message(body)
        );
    }
}

/// The channels of one group, as `ListChannels` reports them.
async fn channel_names(emulator: &Emulator, group: &str, label: &str) -> Vec<String> {
    let (status, body) = emulator
        .call("GET", &format!("/channelGroup/{group}/channel?maxResults=100"), None)
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListChannels {group} expected 200, got {status} {body}"
    );
    body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["ChannelName"].as_str().map(str::to_string))
        .collect()
}

/// The rule, over the whole table: the delete is refused exactly while the target is
/// named, the refused call changes nothing, and `AttachedMultiviewChannels` reports the
/// very channels the refusal is about.
#[tokio::test]
async fn delete_channel_is_refused_exactly_while_a_multiview_channel_names_it() {
    for case in CASES {
        let emulator = Emulator::new();
        seed(&emulator, case).await;

        // Every name this case put on the wire: none of them may appear in the refusal.
        let mut identifiers: Vec<&str> = vec![GROUP, OTHER_GROUP, case.target];
        identifiers.extend(case.sources.iter().copied());
        identifiers.extend(case.multiview.iter().map(|(name, _)| *name));

        // The read the model points the caller at, before the delete.
        let (status, read) = emulator.get_channel(GROUP, case.target).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetChannel expected 200, got {status} {read}",
            case.label
        );
        let expected_attached = if case.blocked_by.is_empty() {
            Value::Null
        } else {
            json!(case.blocked_by)
        };
        assert_eq!(
            read["AttachedMultiviewChannels"], expected_attached,
            "case {:?}: expected AttachedMultiviewChannels {expected_attached}, got {read}",
            case.label
        );

        let before = channel_names(&emulator, GROUP, case.label).await;
        let answer = emulator.delete_channel(GROUP, case.target).await;

        if case.blocked_by.is_empty() {
            assert_eq!(
                answer.0, 200,
                "case {:?}: expected the delete to succeed, got {} {}",
                case.label, answer.0, answer.1
            );
            let (status, body) = emulator.get_channel(GROUP, case.target).await;
            assert_eq!(
                status, 404,
                "case {:?}: expected the deleted channel to be gone, got {status} {body}",
                case.label
            );
        } else {
            assert_in_use_refusal(case.label, &answer, &identifiers);
            // The refused delete stored nothing: the channel is still readable, and the
            // group still holds exactly what it held.
            let (status, body) = emulator.get_channel(GROUP, case.target).await;
            assert_eq!(
                status, 200,
                "case {:?}: the refused delete must leave the channel readable, got {status} {body}",
                case.label
            );
            assert_eq!(
                channel_names(&emulator, GROUP, case.label).await,
                before,
                "case {:?}: the refused delete must leave the group's channels alone",
                case.label
            );

            // Releasing every reference — here by deleting the multiview channels that
            // hold them — lets the same delete through.
            for namer in case.blocked_by {
                let (status, body) = emulator.delete_channel(GROUP, namer).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: DeleteChannel {namer} expected 200, got {status} {body}",
                    case.label
                );
            }
            let (status, body) = emulator.delete_channel(GROUP, case.target).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the delete to succeed once nothing names the channel, \
                 got {status} {body}",
                case.label
            );
            let (status, body) = emulator.get_channel(GROUP, case.target).await;
            assert_eq!(
                status, 404,
                "case {:?}: expected the deleted channel to be gone, got {status} {body}",
                case.label
            );
        }

        // Whatever happened in `GROUP`, the other group's channels are untouched.
        let elsewhere = channel_names(&emulator, OTHER_GROUP, case.label).await;
        for source in case.sources {
            assert!(
                elsewhere.iter().any(|name| name == source),
                "case {:?}: {OTHER_GROUP}/{source} must survive, got {elsewhere:?}",
                case.label
            );
        }
    }
}

/// The other release path the model names: UPDATING the multiview channel so it stops
/// listing the source. `AvailableSources` has a minimum length of 1, so a release rewrites
/// the list rather than emptying it; the rows vary which of the namer's other sources it
/// keeps and how many.
#[tokio::test]
async fn updating_a_multiview_channel_releases_the_sources_it_drops() {
    // (what the namer is updated to list, the source then released, the ones still held)
    let rows: &[(&str, &[&str], &str, &[&str])] = &[
        (
            "the namer keeps one of its three sources",
            &["bravo"],
            "alpha",
            &["bravo"],
        ),
        (
            "the namer keeps two of its three sources",
            &["bravo", "charlie"],
            "alpha",
            &["bravo", "charlie"],
        ),
        (
            "the namer keeps a different single source",
            &["charlie"],
            "alpha",
            &["charlie"],
        ),
    ];

    for (label, updated_sources, released, still_held) in rows {
        let emulator = Emulator::new();
        assert_eq!(emulator.create_group(GROUP, json!({})).await.0, 200);
        for source in ["alpha", "bravo", "charlie"] {
            let (status, body) = emulator
                .create_channel(GROUP, source, json!({"InputType": "CMAF"}))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: CreateChannel {source} expected 200, got {status} {body}"
            );
        }
        let (status, body) = emulator
            .create_channel(GROUP, "tiled", multiview_body(&["alpha", "bravo", "charlie"]))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel tiled expected 200, got {status} {body}"
        );

        // While the namer still lists it, the delete is refused.
        assert_in_use_refusal(
            label,
            &emulator.delete_channel(GROUP, released).await,
            &[GROUP, "tiled", released],
        );

        let (status, body) = emulator
            .update_channel(GROUP, "tiled", multiview_body(updated_sources), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {body}"
        );

        // The dropped source is now deletable...
        let (status, body) = emulator.delete_channel(GROUP, released).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the released channel to delete, got {status} {body}"
        );
        // ...and the ones the namer still lists are not.
        for held in *still_held {
            assert_in_use_refusal(
                label,
                &emulator.delete_channel(GROUP, held).await,
                &[GROUP, "tiled", held],
            );
            let (status, body) = emulator.get_channel(GROUP, held).await;
            assert_eq!(
                status, 200,
                "case {label:?}: {held} must survive its refused delete, got {status} {body}"
            );
            assert_eq!(
                body["AttachedMultiviewChannels"], json!(["tiled"]),
                "case {label:?}: expected {held} to report tiled as attached, got {body}"
            );
        }
    }
}

/// The emulator does not yet enforce the model's requirement that a multiview channel's
/// sources be OTHER, CMAF channels of the same group, so a channel that lists its own
/// name is storable. A self-reference must not make it undeletable: deleting the channel
/// removes the listing along with the channel, so no dangling reference is left behind.
#[tokio::test]
async fn a_multiview_channel_that_lists_its_own_name_stays_deletable() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group(GROUP, json!({})).await.0, 200);
    for (label, name, sources) in [
        ("only its own name", "self-only", vec!["self-only"]),
        ("its own name beside another", "self-plus", vec!["alpha", "self-plus"]),
    ] {
        let (status, body) = emulator
            .create_channel(GROUP, name, multiview_body(&sources))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel {name} expected 200, got {status} {body}"
        );
        let (status, body) = emulator.delete_channel(GROUP, name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a self-listing channel must stay deletable, got {status} {body}"
        );
        let (status, body) = emulator.get_channel(GROUP, name).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the deleted channel to be gone, got {status} {body}"
        );
    }
}
