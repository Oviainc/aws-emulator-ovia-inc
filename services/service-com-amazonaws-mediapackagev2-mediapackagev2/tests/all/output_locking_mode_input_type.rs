// SPDX-License-Identifier: CC0-1.0

//! A channel's `OutputLockingMode` against the model's whole `InputType` enum and its own,
//! driven end to end over the same router the emulator boots.
//!
//! The rule under test, asserted over a SET of channels rather than over one recorded
//! request: a member an input type DOES carry can still be restricted to some of its
//! VALUES, and such a restriction is refused on the value — with the
//! `ValidationExceptionType` the model declares for that combination, storing nothing —
//! rather than by withholding the member, so an input type that carries the member keeps
//! reporting it at every value it may have, on every projection of the channel.
//!
//! Which input types carry `OutputLockingMode` at all is the sibling restriction
//! (`tests/all/restricted_channel_members.rs`), named by the refusal the service answers
//! for it: "OutputLockingMode can only be configured when the channel InputType is CMAF
//! or MULTIVIEW", with `ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE` naming it. The
//! VALUE restriction is the multiview one: a multiview channel composites several streams
//! into one, which only lines up while each is locked to the epoch, and the model names
//! that refusal `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` for the channel itself and
//! `MULTIVIEW_SOURCE_NON_EPOCH_LOCKED` for a tile source it names — the source side, and
//! what a stored mode then decides for an origin endpoint, are the sibling suite
//! `tests/all/multiview_epoch_locking.rs`.
//!
//! The catalogues are the model's own enums
//! (`smithy_query(query="shape", shape="InputType,OutputLockingMode,ValidationExceptionType")`):
//! `InputType` is HLS | CMAF | MULTIVIEW and `OutputLockingMode` is EPOCH_LOCKED |
//! NON_EPOCH_LOCKED. The rows span both, plus the absent case of each, which
//! `CreateChannelRequest` documents a service-applied value for ("If unprovided, the value
//! defaults to HLS"; "If you don't specify a value, the default is `EPOCH_LOCKED`").

use serde_json::{json, Value};

use crate::channel_lifecycle::{at, error_type, Emulator};

/// The locking mode no channel of a multiview composition may have.
pub(crate) const NON_EPOCH: &str = "NON_EPOCH_LOCKED";

/// The locking mode a composition admits, which is also the member's documented default.
pub(crate) const EPOCH: &str = "EPOCH_LOCKED";

/// The `ValidationExceptionType` the model declares for a multiview channel asking for
/// the mode a composition does not admit.
const MULTIVIEW_WITH_NON_EPOCH: &str = "MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED";

/// The `ValidationExceptionType` the model declares for the member on an input type that
/// carries no locking mode at all.
const ONLY_CMAF: &str = "ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE";

/// The group every case of this suite and its sibling works in.
pub(crate) const GROUP: &str = "composition";

/// The CMAF tile source a multiview create names.
pub(crate) const TILE_SOURCE: &str = "tile-source";

/// A multiview configuration over `sources`, with the layouts the row names.
///
/// `pub(crate)` for the sibling suite, which drives the same configuration from the
/// source side rather than from the channel's own member.
pub(crate) fn multiview_over(sources: &[&str], layouts: &[&str]) -> Value {
    json!({
        "MultiviewConfiguration": {
            "AvailableSources": sources,
            "AvailableLayouts": layouts,
        },
    })
}

/// Seed the group, plus one CMAF tile source created with `source_mode` (`None` leaves
/// the member out, so the service applies its own default).
pub(crate) async fn seeded_group(emulator: &Emulator, label: &str, source_mode: Option<&str>) {
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let mut source = json!({"InputType": "CMAF"});
    if let Some(mode) = source_mode {
        source["OutputLockingMode"] = json!(mode);
    }
    let (status, body) = emulator.create_channel(GROUP, TILE_SOURCE, source).await;
    assert_eq!(
        status, 200,
        "case {label:?}: seeding the CMAF tile source expected 200, got {status} {body}"
    );
}

/// The `ListChannels` item for one channel of the group, or `Value::Null` when the list
/// does not carry it.
async fn listed_channel(emulator: &Emulator, label: &str, channel: &str) -> Value {
    let (status, body) = emulator.list_channels(GROUP, "?maxResults=100").await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListChannels expected 200, got {status} {body}"
    );
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

/// Assert a refused create: the modelled 400, its discriminator, a message pathed at
/// `member`, and nothing stored under `channel`.
///
/// `pub(crate)` for the sibling suite: one refused create is checked the same way
/// whichever side of the composition rule refused it.
pub(crate) async fn assert_refused_create(
    emulator: &Emulator,
    label: &str,
    answer: (u16, Value),
    expected_type: &str,
    member: &str,
    channel: &str,
) {
    let (status, body) = answer;
    assert_eq!(
        status, 400,
        "case {label:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ValidationException",
        "case {label:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["ValidationExceptionType"],
        json!(expected_type),
        "case {label:?}: expected {expected_type}, got {body}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!("request.{member}")),
        "case {label:?}: the message must name the member the refusal is at, got \
         {message:?}"
    );
    let (status, read) = emulator.get_channel(GROUP, channel).await;
    assert_eq!(
        status, 404,
        "case {label:?}: a refused create must store nothing, got {status} {read}"
    );
}

/// One row: the `InputType` the create names (`None` is the absent case), the mode it asks
/// for (`None` leaves the member out), the `ValidationExceptionType` the combination is
/// refused with (`None` when it is accepted) and the mode every projection of an accepted
/// channel must report (`None` for a channel that carries no mode).
type ChannelRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// Every input type against every value of the member, plus the absent value of each: the
/// two restrictions on the member meet in this table, and only one cell is the value rule.
const CHANNEL_ROWS: &[ChannelRow] = &[
    ("no input type and no mode", None, None, None, None),
    (
        "no input type, the default mode",
        None,
        Some(EPOCH),
        Some(ONLY_CMAF),
        None,
    ),
    ("HLS and no mode", Some("HLS"), None, None, None),
    (
        "HLS asking for the non-epoch mode",
        Some("HLS"),
        Some(NON_EPOCH),
        Some(ONLY_CMAF),
        None,
    ),
    ("CMAF and no mode", Some("CMAF"), None, None, Some(EPOCH)),
    (
        "CMAF epoch-locked",
        Some("CMAF"),
        Some(EPOCH),
        None,
        Some(EPOCH),
    ),
    (
        "CMAF not epoch-locked",
        Some("CMAF"),
        Some(NON_EPOCH),
        None,
        Some(NON_EPOCH),
    ),
    (
        "MULTIVIEW and no mode",
        Some("MULTIVIEW"),
        None,
        None,
        Some(EPOCH),
    ),
    (
        "MULTIVIEW epoch-locked",
        Some("MULTIVIEW"),
        Some(EPOCH),
        None,
        Some(EPOCH),
    ),
    (
        "MULTIVIEW asking for the non-epoch mode",
        Some("MULTIVIEW"),
        Some(NON_EPOCH),
        Some(MULTIVIEW_WITH_NON_EPOCH),
        None,
    ),
];

/// RULE — the one value a multiview channel may not have is refused as a VALUE, with its
/// own discriminator, while every other value of the member is accepted and reported by
/// every projection of the channel.
#[tokio::test]
async fn a_value_an_input_type_may_not_have_is_refused_and_every_other_is_reported() {
    for (label, input_type, mode, refused_as, reported) in CHANNEL_ROWS {
        let emulator = Emulator::new();
        seeded_group(&emulator, label, None).await;

        let mut create = if *input_type == Some("MULTIVIEW") {
            multiview_over(&[TILE_SOURCE], &["LAYOUT_2EH"])
        } else {
            json!({})
        };
        if let Some(input_type) = input_type {
            create["InputType"] = json!(input_type);
        }
        if let Some(mode) = mode {
            create["OutputLockingMode"] = json!(mode);
        }
        let channel = "subject";
        let answer = emulator.create_channel(GROUP, channel, create).await;

        if let Some(refused_as) = refused_as {
            assert_refused_create(
                &emulator,
                label,
                answer,
                refused_as,
                "outputLockingMode",
                channel,
            )
            .await;
            continue;
        }

        let (status, created) = answer;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );
        let expected = reported.map_or(Value::Null, |mode| json!(mode));
        let (status, read) = emulator.get_channel(GROUP, channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        let listed = listed_channel(&emulator, label, channel).await;
        for (projection, body) in [
            ("CreateChannel", &created),
            ("GetChannel", &read),
            ("ListChannels", &listed),
        ] {
            let actual = at(body, "/OutputLockingMode");
            assert_eq!(
                actual, expected,
                "case {label:?}: {projection} OutputLockingMode expected {expected}, got \
                 {actual}"
            );
        }
    }
}
