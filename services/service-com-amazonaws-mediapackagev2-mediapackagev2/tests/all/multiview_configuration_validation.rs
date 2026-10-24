// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rules the model states about a channel's
//! `MultiviewConfiguration`, driven over HTTP through the same router the emulator boots
//! and reusing the client in [`crate::channel_lifecycle`].
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. The member is stated in BOTH directions ("This setting is required when InputType is
//!    MULTIVIEW, and can't be set for any other input type"), so a channel of the input
//!    type that requires it is refused without it — `MULTIVIEW_CONFIGURATION_REQUIRED` —
//!    exactly as one of any other input type is refused with it.
//! 2. `AvailableSources` names other CHANNELS, so every name in it is resolved against the
//!    store: one no channel of the SAME channel group carries is `MULTIVIEW_SOURCE_NOT_FOUND`
//!    and one whose channel is not of the input type a tile source must have is
//!    `MULTIVIEW_SOURCE_INVALID_INPUT_TYPE` ("Each source channel must be in the same
//!    channel group as the multiview channel, and must have an `InputType` of `CMAF`").
//! 3. Each of the two lists names a set, so an entry may appear at most once ("Each layout
//!    must appear at most once"): a repeat is `MULTIVIEW_DUPLICATE_SOURCE` /
//!    `MULTIVIEW_DUPLICATE_LAYOUT`.
//!
//! Each rule is asserted on `CreateChannel` and on `UpdateChannel`, which write the same
//! member, and every refusal is asserted to have written nothing.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The group the tables operate in.
const GROUP: &str = "mv-group";

/// A second channel group, holding a CMAF channel of its own: the model scopes a source to
/// "the same channel group as the multiview channel", so this group's channel is a name
/// that exists in the service and still may not be used as a tile source in [`GROUP`].
const OTHER_GROUP: &str = "mv-other";

/// Layout values, from the model's `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL |
/// LAYOUT_3EL | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL). `AvailableLayouts` has a `@length`
/// of 1..6, so the whole catalogue is also the longest legal list.
const LAYOUTS: [&str; 6] = [
    "LAYOUT_2EH",
    "LAYOUT_2PL",
    "LAYOUT_3EL",
    "LAYOUT_3PL",
    "LAYOUT_4E",
    "LAYOUT_4PL",
];

/// The `ValidationExceptionType` of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `Message` of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// Assert one answer is the modeled refusal for a member: 400, a `ValidationException`
/// envelope, the expected discriminator, and the service's one message form — the member
/// path the rule is about, then the requirement.
fn assert_refusal(
    case: &str,
    answer: &(u16, Value),
    expected_type: &str,
    member_path: &str,
) {
    let (status, body) = answer;
    assert_eq!(
        *status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException envelope, got {body}"
    );
    assert_eq!(
        validation_type(body),
        expected_type,
        "case {case:?}: expected {expected_type}, got {body}"
    );
    let expected_prefix = format!("Validation error encountered at request.{member_path}: ");
    assert!(
        message(body).starts_with(&expected_prefix),
        "case {case:?}: expected a message naming {member_path}, got {:?}",
        message(body)
    );
}

/// A `MultiviewConfiguration` member value.
fn multiview(sources: &[&str], layouts: &[&str]) -> Value {
    json!({"AvailableSources": sources, "AvailableLayouts": layouts})
}

/// Create [`GROUP`] with a CMAF channel per name in `cmaf`, an HLS channel per name in
/// `hls`, and [`OTHER_GROUP`] with one CMAF channel called `cmaf-elsewhere`.
async fn seeded(cmaf: &[&str], hls: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    for group in [GROUP, OTHER_GROUP] {
        let (status, body) = emulator.create_group(group, json!({})).await;
        assert_eq!(
            status, 200,
            "CreateChannelGroup {group} expected 200, got {status} {body}"
        );
    }
    // `InputType` values come from the model's `InputType` enum (CMAF | HLS | MULTIVIEW).
    for name in cmaf {
        let (status, body) = emulator
            .create_channel(GROUP, name, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            status, 200,
            "CreateChannel {name} expected 200, got {status} {body}"
        );
    }
    for name in hls {
        let (status, body) = emulator
            .create_channel(GROUP, name, json!({"InputType": "HLS"}))
            .await;
        assert_eq!(
            status, 200,
            "CreateChannel {name} expected 200, got {status} {body}"
        );
    }
    let (status, body) = emulator
        .create_channel(OTHER_GROUP, "cmaf-elsewhere", json!({"InputType": "CMAF"}))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel cmaf-elsewhere expected 200, got {status} {body}"
    );
    emulator
}

/// RULE 1 — the configuration is required on the input type whose channels carry it and
/// forbidden on every other, so both directions are refusals and only the matching pair is
/// accepted.
///
/// The rows are the model's whole `InputType` enum (CMAF | HLS | MULTIVIEW) plus the absent
/// case, which the member's doc defaults to HLS, each with and without a configuration.
#[tokio::test]
async fn the_configuration_is_required_on_multiview_input_and_refused_on_every_other() {
    let emulator = seeded(&["cmaf-a"], &[]).await;

    // (label, the submitted InputType, whether a configuration is carried, the expected
    // ValidationExceptionType or None when the combination is legal)
    let rows: &[(&str, Option<&str>, bool, Option<&str>)] = &[
        (
            "MULTIVIEW without a configuration",
            Some("MULTIVIEW"),
            false,
            Some("MULTIVIEW_CONFIGURATION_REQUIRED"),
        ),
        (
            "MULTIVIEW with a configuration",
            Some("MULTIVIEW"),
            true,
            None,
        ),
        ("HLS without a configuration", Some("HLS"), false, None),
        (
            "HLS with a configuration",
            Some("HLS"),
            true,
            Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        ),
        ("CMAF without a configuration", Some("CMAF"), false, None),
        (
            "CMAF with a configuration",
            Some("CMAF"),
            true,
            Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        ),
        ("no input type at all", None, false, None),
        (
            "no input type, with a configuration",
            None,
            true,
            Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        ),
    ];

    for (index, (label, input_type, carries, expected)) in rows.iter().enumerate() {
        let name = format!("cand-{index}");
        let mut body = json!({});
        if let Some(input_type) = input_type {
            body["InputType"] = json!(input_type);
        }
        if *carries {
            body["MultiviewConfiguration"] = multiview(&["cmaf-a"], &[LAYOUTS[0]]);
        }
        let answer = emulator.create_channel(GROUP, &name, body).await;

        match expected {
            Some(expected_type) => {
                assert_refusal(label, &answer, expected_type, "multiviewConfiguration");
                let (status, read) = emulator.get_channel(GROUP, &name).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must store nothing, got {status} {read}"
                );
            }
            None => {
                let (status, created) = &answer;
                assert_eq!(
                    *status, 200,
                    "case {label:?}: expected 200, got {status} {created}"
                );
                let (status, read) = emulator.get_channel(GROUP, &name).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: an accepted create must be readable, got {status} {read}"
                );
                assert_eq!(
                    read["MultiviewConfiguration"], created["MultiviewConfiguration"],
                    "case {label:?}: the read must report the stored configuration, got {read}"
                );
                // A configuration is reported exactly on the input type that carries one.
                assert_eq!(
                    read["MultiviewConfiguration"].is_null(),
                    !*carries,
                    "case {label:?}: expected a configuration to be reported only when one \
                     was stored, got {read}"
                );
            }
        }
    }
}

/// One row of the source-resolution table.
struct SourceCase {
    label: &'static str,
    sources: &'static [&'static str],
    /// The expected `ValidationExceptionType`, or `None` when every source resolves.
    refused_as: Option<&'static str>,
}

/// The rows span what the rule generalises over: how many sources there are, where in the
/// list the offending one sits, whether it is absent from the group, present in ANOTHER
/// group, or present with an input type a tile source may not have (HLS, and MULTIVIEW —
/// which is what a multiview channel naming itself or another multiview channel amounts
/// to).
const SOURCE_CASES: &[SourceCase] = &[
    SourceCase {
        label: "one CMAF source of the same group",
        sources: &["cmaf-a"],
        refused_as: None,
    },
    SourceCase {
        label: "two CMAF sources of the same group",
        sources: &["cmaf-a", "cmaf-b"],
        refused_as: None,
    },
    SourceCase {
        label: "a name no channel of the group carries",
        sources: &["absent"],
        refused_as: Some("MULTIVIEW_SOURCE_NOT_FOUND"),
    },
    SourceCase {
        label: "an absent name after a resolvable one",
        sources: &["cmaf-a", "absent"],
        refused_as: Some("MULTIVIEW_SOURCE_NOT_FOUND"),
    },
    SourceCase {
        label: "an absent name before a resolvable one",
        sources: &["absent", "cmaf-b"],
        refused_as: Some("MULTIVIEW_SOURCE_NOT_FOUND"),
    },
    SourceCase {
        label: "an absent name between two resolvable ones",
        sources: &["cmaf-a", "absent", "cmaf-b"],
        refused_as: Some("MULTIVIEW_SOURCE_NOT_FOUND"),
    },
    SourceCase {
        label: "a channel of another channel group",
        sources: &["cmaf-elsewhere"],
        refused_as: Some("MULTIVIEW_SOURCE_NOT_FOUND"),
    },
    SourceCase {
        label: "an HLS channel of the same group",
        sources: &["hls-a"],
        refused_as: Some("MULTIVIEW_SOURCE_INVALID_INPUT_TYPE"),
    },
    SourceCase {
        label: "an HLS channel after a CMAF one",
        sources: &["cmaf-a", "hls-a"],
        refused_as: Some("MULTIVIEW_SOURCE_INVALID_INPUT_TYPE"),
    },
    SourceCase {
        label: "a MULTIVIEW channel of the same group",
        sources: &["tiled-seed"],
        refused_as: Some("MULTIVIEW_SOURCE_INVALID_INPUT_TYPE"),
    },
    SourceCase {
        label: "an absent name before a wrong-input-type one: the first failure decides",
        sources: &["absent", "hls-a"],
        refused_as: Some("MULTIVIEW_SOURCE_NOT_FOUND"),
    },
];

/// Seed the group the source table runs in: two CMAF channels, one HLS channel, and one
/// MULTIVIEW channel (itself a legal one) for the rows about a non-CMAF source.
async fn seeded_for_sources() -> Emulator {
    let emulator = seeded(&["cmaf-a", "cmaf-b"], &["hls-a"]).await;
    let (status, body) = emulator
        .create_channel(
            GROUP,
            "tiled-seed",
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": multiview(&["cmaf-a"], &[LAYOUTS[0]]),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel tiled-seed expected 200, got {status} {body}"
    );
    emulator
}

/// RULE 2, on the create — every available source is resolved against the addressed
/// channel group, and a create naming one the group cannot serve stores nothing.
#[tokio::test]
async fn a_created_multiview_channels_sources_are_resolved_against_its_own_group() {
    let emulator = seeded_for_sources().await;

    for (index, case) in SOURCE_CASES.iter().enumerate() {
        let label = case.label;
        let name = format!("tiled-{index}");
        let answer = emulator
            .create_channel(
                GROUP,
                &name,
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": multiview(case.sources, &[LAYOUTS[1]]),
                }),
            )
            .await;

        match case.refused_as {
            Some(expected_type) => {
                assert_refusal(
                    label,
                    &answer,
                    expected_type,
                    "multiviewConfiguration.availableSources",
                );
                let (status, read) = emulator.get_channel(GROUP, &name).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must store nothing, got {status} {read}"
                );
            }
            None => {
                let (status, created) = &answer;
                assert_eq!(
                    *status, 200,
                    "case {label:?}: expected 200, got {status} {created}"
                );
                let (status, read) = emulator.get_channel(GROUP, &name).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: an accepted create must be readable, got {status} {read}"
                );
                assert_eq!(
                    read["MultiviewConfiguration"]["AvailableSources"],
                    json!(case.sources),
                    "case {label:?}: expected the submitted sources to be stored, got {read}"
                );
            }
        }
    }
}

/// RULE 2, on the update — the same resolution, and a refused update leaves the stored
/// configuration exactly as it was.
#[tokio::test]
async fn an_updated_multiview_channels_sources_are_resolved_against_its_own_group() {
    let emulator = seeded_for_sources().await;
    let channel = "tiled-edit";
    let (status, created) = emulator
        .create_channel(
            GROUP,
            channel,
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": multiview(&["cmaf-a"], &[LAYOUTS[0]]),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel {channel} expected 200, got {status} {created}"
    );
    let mut stored = created["MultiviewConfiguration"].clone();

    for case in SOURCE_CASES {
        let label = case.label;
        let answer = emulator
            .update_channel(
                GROUP,
                channel,
                json!({"MultiviewConfiguration": multiview(case.sources, &[LAYOUTS[2]])}),
                &[],
            )
            .await;

        match case.refused_as {
            Some(expected_type) => {
                assert_refusal(
                    label,
                    &answer,
                    expected_type,
                    "multiviewConfiguration.availableSources",
                );
                let (status, read) = emulator.get_channel(GROUP, channel).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the channel must survive a refused update, got {status} \
                     {read}"
                );
                assert_eq!(
                    read["MultiviewConfiguration"], stored,
                    "case {label:?}: a refused update must leave the stored configuration \
                     alone, got {read}"
                );
            }
            None => {
                let (status, updated) = &answer;
                assert_eq!(
                    *status, 200,
                    "case {label:?}: expected 200, got {status} {updated}"
                );
                stored = updated["MultiviewConfiguration"].clone();
                let (status, read) = emulator.get_channel(GROUP, channel).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetChannel expected 200, got {status} {read}"
                );
                assert_eq!(
                    read["MultiviewConfiguration"], stored,
                    "case {label:?}: the read must report what the update stored, got {read}"
                );
                assert_eq!(
                    read["MultiviewConfiguration"]["AvailableSources"],
                    json!(case.sources),
                    "case {label:?}: expected the submitted sources to be stored, got {read}"
                );
            }
        }
    }
}

/// One row of the repetition table: the two lists as submitted, and the expected
/// discriminator with the member path it is reported at.
struct RepetitionCase {
    label: &'static str,
    sources: &'static [&'static str],
    layouts: &'static [&'static str],
    refused_as: Option<(&'static str, &'static str)>,
}

/// The rows span where the repeat sits (adjacent, separated), which list carries it, how
/// long the list is, and a list whose repeated entry does not even resolve — a repetition
/// is about the request alone, so it is decided without the store. The accepted rows
/// include the longest legal layout list, the model's whole `MultiviewLayoutType` enum.
const REPETITION_CASES: &[RepetitionCase] = &[
    RepetitionCase {
        label: "distinct sources and distinct layouts",
        sources: &["cmaf-a", "cmaf-b"],
        layouts: &["LAYOUT_2EH", "LAYOUT_4E"],
        refused_as: None,
    },
    RepetitionCase {
        label: "the whole layout catalogue, each layout once",
        sources: &["cmaf-a"],
        layouts: &LAYOUTS,
        refused_as: None,
    },
    RepetitionCase {
        label: "the same source twice, adjacent",
        sources: &["cmaf-a", "cmaf-a"],
        layouts: &["LAYOUT_2EH"],
        refused_as: Some((
            "MULTIVIEW_DUPLICATE_SOURCE",
            "multiviewConfiguration.availableSources",
        )),
    },
    RepetitionCase {
        label: "the same source twice, separated by another",
        sources: &["cmaf-a", "cmaf-b", "cmaf-a"],
        layouts: &["LAYOUT_2EH"],
        refused_as: Some((
            "MULTIVIEW_DUPLICATE_SOURCE",
            "multiviewConfiguration.availableSources",
        )),
    },
    RepetitionCase {
        label: "a repeated source that does not resolve either",
        sources: &["absent", "absent"],
        layouts: &["LAYOUT_2EH"],
        refused_as: Some((
            "MULTIVIEW_DUPLICATE_SOURCE",
            "multiviewConfiguration.availableSources",
        )),
    },
    RepetitionCase {
        label: "the same layout twice, adjacent",
        sources: &["cmaf-a"],
        layouts: &["LAYOUT_3PL", "LAYOUT_3PL"],
        refused_as: Some((
            "MULTIVIEW_DUPLICATE_LAYOUT",
            "multiviewConfiguration.availableLayouts",
        )),
    },
    RepetitionCase {
        label: "the same layout twice, separated by two others",
        sources: &["cmaf-a", "cmaf-b"],
        layouts: &["LAYOUT_2PL", "LAYOUT_3EL", "LAYOUT_4PL", "LAYOUT_2PL"],
        refused_as: Some((
            "MULTIVIEW_DUPLICATE_LAYOUT",
            "multiviewConfiguration.availableLayouts",
        )),
    },
    RepetitionCase {
        label: "both lists repeat an entry: the source list is reported",
        sources: &["cmaf-b", "cmaf-b"],
        layouts: &["LAYOUT_4E", "LAYOUT_4E"],
        refused_as: Some((
            "MULTIVIEW_DUPLICATE_SOURCE",
            "multiviewConfiguration.availableSources",
        )),
    },
];

/// RULE 3 — each list admits an entry at most once, on the create and on the update alike,
/// and a refused request writes nothing.
#[tokio::test]
async fn each_multiview_list_admits_an_entry_at_most_once() {
    let emulator = seeded(&["cmaf-a", "cmaf-b"], &[]).await;
    let editable = "tiled-repeat";
    let (status, created) = emulator
        .create_channel(
            GROUP,
            editable,
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": multiview(&["cmaf-a"], &[LAYOUTS[0]]),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel {editable} expected 200, got {status} {created}"
    );
    let mut stored = created["MultiviewConfiguration"].clone();

    for (index, case) in REPETITION_CASES.iter().enumerate() {
        let label = case.label;
        let configuration = multiview(case.sources, case.layouts);
        let name = format!("repeat-{index}");
        let created = emulator
            .create_channel(
                GROUP,
                &name,
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": configuration,
                }),
            )
            .await;
        let updated = emulator
            .update_channel(
                GROUP,
                editable,
                json!({"MultiviewConfiguration": multiview(case.sources, case.layouts)}),
                &[],
            )
            .await;

        match case.refused_as {
            Some((expected_type, member_path)) => {
                assert_refusal(label, &created, expected_type, member_path);
                assert_refusal(label, &updated, expected_type, member_path);
                let (status, read) = emulator.get_channel(GROUP, &name).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must store nothing, got {status} {read}"
                );
                let (status, read) = emulator.get_channel(GROUP, editable).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the edited channel must survive, got {status} {read}"
                );
                assert_eq!(
                    read["MultiviewConfiguration"], stored,
                    "case {label:?}: a refused update must leave the stored configuration \
                     alone, got {read}"
                );
            }
            None => {
                let (status, body) = &created;
                assert_eq!(
                    *status, 200,
                    "case {label:?}: CreateChannel expected 200, got {status} {body}"
                );
                let (status, body) = &updated;
                assert_eq!(
                    *status, 200,
                    "case {label:?}: UpdateChannel expected 200, got {status} {body}"
                );
                stored = body["MultiviewConfiguration"].clone();
                for (projection, channel) in [("created", name.as_str()), ("edited", editable)] {
                    let (status, read) = emulator.get_channel(GROUP, channel).await;
                    assert_eq!(
                        status, 200,
                        "case {label:?}: GetChannel {projection} expected 200, got {status} \
                         {read}"
                    );
                    assert_eq!(
                        read["MultiviewConfiguration"],
                        multiview(case.sources, case.layouts),
                        "case {label:?}: the {projection} channel must report the submitted \
                         lists, got {read}"
                    );
                }
            }
        }
    }
}
