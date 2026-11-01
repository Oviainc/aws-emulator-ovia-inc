// SPDX-License-Identifier: CC0-1.0

//! The other side of the multiview composition's one locking rule — the channels a
//! multiview configuration NAMES, and what a stored locking mode then decides for an
//! origin endpoint — driven end to end over the same router the emulator boots.
//!
//! The rule under test, asserted over a SET of compositions rather than over one recorded
//! request: a value restriction the service states about a composition holds for every
//! channel in it, so a channel referenced as a tile source is refused at the value the
//! composition does not admit exactly as the composite channel itself is, with the
//! `ValidationExceptionType` the model declares for a source
//! (`MULTIVIEW_SOURCE_NON_EPOCH_LOCKED`, beside
//! `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` for the composite — the channel's own side
//! is the sibling suite `tests/all/output_locking_mode_input_type.rs`). Every writer that
//! can put a configuration in place is held to it, so the create and the update that
//! REPLACES the configuration answer the same refusal and store nothing.
//!
//! And because a composition admits one locking mode only, no channel of one has the
//! stored mode `Segment.OutputTimestampMode` needs ("only configurable on channels with
//! `OutputLockingMode` set to `NON_EPOCH_LOCKED`"), so no endpoint under a multiview
//! channel carries that member while an endpoint under a channel that does have the mode
//! still does — what every channel configuration reports for the endpoint member is the
//! suite `tests/all/segment_output_timestamp_mode.rs`.
//!
//! The catalogues are the model's own enums
//! (`smithy_query(query="shape", shape="InputType,OutputLockingMode,OutputTimestampMode")`):
//! `InputType` is HLS | CMAF | MULTIVIEW, `OutputLockingMode` is EPOCH_LOCKED |
//! NON_EPOCH_LOCKED and `OutputTimestampMode` is PASSTHROUGH | REBASED_TO_CHANNEL_START.

use serde_json::{json, Value};

use crate::channel_lifecycle::{at, minimal_endpoint, Emulator};
// The sibling suite seeds the same group and checks a refused create the same way; the
// client itself is the crate's one emulator harness.
use crate::output_locking_mode_input_type::{
    assert_refused_create, multiview_over, seeded_group, EPOCH, GROUP, NON_EPOCH, TILE_SOURCE,
};

/// The `ValidationExceptionType` the model declares for a tile source locked the way a
/// composition does not admit.
const SOURCE_NON_EPOCH: &str = "MULTIVIEW_SOURCE_NON_EPOCH_LOCKED";

/// The `ValidationExceptionType` the model declares for an endpoint naming a segment
/// output-timestamp mode under a channel whose locking mode does not carry one.
const ONLY_NON_EPOCH_LOCKED: &str = "ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE";

/// The second seeded CMAF source, created with the mode a composition admits, so a row
/// can name an acceptable source beside the one under test.
const EPOCH_SOURCE: &str = "epoch-source";

/// The member path a refusal about the named sources is stated at.
const SOURCES_MEMBER: &str = "multiviewConfiguration.availableSources";

/// One row: the mode the seeded tile source is created with (`None` leaves the member out,
/// so the service applies its own default), the sources a configuration names, and the
/// `ValidationExceptionType` naming them is refused with (`None` when it is accepted).
type SourceRow = (
    &'static str,
    Option<&'static str>,
    &'static [&'static str],
    Option<&'static str>,
);

/// Every value of the source's own locking mode, plus the absent one, named alone and
/// beside a source the composition admits.
const SOURCE_ROWS: &[SourceRow] = &[
    ("a source that named no mode", None, &[TILE_SOURCE], None),
    ("an epoch-locked source", Some(EPOCH), &[TILE_SOURCE], None),
    (
        "a source that is not epoch-locked",
        Some(NON_EPOCH),
        &[TILE_SOURCE],
        Some(SOURCE_NON_EPOCH),
    ),
    (
        "a source the composition admits, beside one it does not",
        Some(NON_EPOCH),
        &[EPOCH_SOURCE, TILE_SOURCE],
        Some(SOURCE_NON_EPOCH),
    ),
    (
        "two sources the composition admits",
        Some(EPOCH),
        &[EPOCH_SOURCE, TILE_SOURCE],
        None,
    ),
];

/// RULE, the composition's tile sources — a channel NAMED as a source is refused at the
/// value the composition does not admit, on the create and on the update that replaces
/// the configuration, and a refused write leaves the store untouched.
#[tokio::test]
async fn a_multiview_configuration_is_refused_a_source_the_composition_rejects() {
    for (label, source_mode, sources, refused_as) in SOURCE_ROWS {
        let emulator = Emulator::new();
        seeded_group(&emulator, label, *source_mode).await;
        // The acceptable source a mixed row names, and the one the standing multiview
        // channel below is created over.
        let (status, body) = emulator
            .create_channel(
                GROUP,
                EPOCH_SOURCE,
                json!({"InputType": "CMAF", "OutputLockingMode": EPOCH}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: seeding the epoch-locked source expected 200, got \
             {status} {body}"
        );

        let channel = "composite";
        let mut create = multiview_over(sources, &["LAYOUT_2EH"]);
        create["InputType"] = json!("MULTIVIEW");
        let answer = emulator.create_channel(GROUP, channel, create).await;

        if let Some(refused_as) = refused_as {
            assert_refused_create(&emulator, label, answer, refused_as, SOURCES_MEMBER, channel)
                .await;
        } else {
            let (status, created) = answer;
            assert_eq!(
                status, 200,
                "case {label:?}: CreateChannel expected 200, got {status} {created}"
            );
            let actual = at(&created, "/MultiviewConfiguration/AvailableSources");
            assert_eq!(
                actual,
                json!(sources),
                "case {label:?}: the channel must report the sources it was created with, \
                 got {actual}"
            );
        }

        // The update REPLACES the whole configuration, so it is held to the same rule: a
        // multiview channel over the acceptable source is created first, and the row's own
        // source list is then put in its place.
        let standing = "standing";
        let mut standing_create = multiview_over(&[EPOCH_SOURCE], &["LAYOUT_4E"]);
        standing_create["InputType"] = json!("MULTIVIEW");
        let (status, body) = emulator
            .create_channel(GROUP, standing, standing_create)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: creating the standing multiview channel expected 200, got \
             {status} {body}"
        );
        let (status, updated) = emulator
            .update_channel(GROUP, standing, multiview_over(sources, &["LAYOUT_2EH"]), &[])
            .await;
        match refused_as {
            Some(refused_as) => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: UpdateChannel expected 400, got {status} {updated}"
                );
                assert_eq!(
                    updated["ValidationExceptionType"],
                    json!(refused_as),
                    "case {label:?}: UpdateChannel expected {refused_as}, got {updated}"
                );
                let (_, read) = emulator.get_channel(GROUP, standing).await;
                let actual = at(&read, "/MultiviewConfiguration/AvailableSources");
                assert_eq!(
                    actual,
                    json!([EPOCH_SOURCE]),
                    "case {label:?}: a refused update must leave the stored sources \
                     alone, got {actual}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
                );
                let actual = at(&updated, "/MultiviewConfiguration/AvailableSources");
                assert_eq!(
                    actual,
                    json!(sources),
                    "case {label:?}: the update must report the sources it committed, got \
                     {actual}"
                );
            }
        }
    }
}

/// One row of the endpoint table: the channel's create body and whether its endpoints
/// carry a segment output-timestamp mode.
type EndpointRow = (&'static str, fn() -> Value, bool);

/// RULE, what the stored mode then decides — `Segment.OutputTimestampMode` is eligible
/// only under a channel whose stored locking mode is the non-epoch one, so no endpoint of
/// a multiview channel carries it (a composition admits no such channel) while an endpoint
/// of a channel that does have the mode does.
#[tokio::test]
async fn an_endpoint_carries_an_output_timestamp_mode_only_where_the_stored_mode_allows_it() {
    let rows: &[EndpointRow] = &[
        (
            "a multiview channel, which a composition only admits epoch-locked",
            || {
                let mut body = multiview_over(&[TILE_SOURCE], &["LAYOUT_2EH"]);
                body["InputType"] = json!("MULTIVIEW");
                body
            },
            false,
        ),
        (
            "a multiview channel naming the mode a composition admits",
            || {
                let mut body = multiview_over(&[TILE_SOURCE], &["LAYOUT_4E"]);
                body["InputType"] = json!("MULTIVIEW");
                body["OutputLockingMode"] = json!(EPOCH);
                body
            },
            false,
        ),
        (
            "a CMAF channel that is not epoch-locked",
            || json!({"InputType": "CMAF", "OutputLockingMode": NON_EPOCH}),
            true,
        ),
    ];

    for (label, body, carries) in rows {
        let emulator = Emulator::new();
        seeded_group(&emulator, label, None).await;
        let channel = "subject";
        let (status, created) = emulator.create_channel(GROUP, channel, body()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );

        // Each value of the model's `OutputTimestampMode` enum.
        for mode in ["PASSTHROUGH", "REBASED_TO_CHANNEL_START"] {
            let endpoint = format!("ep-{}", mode.to_lowercase());
            let mut endpoint_body = minimal_endpoint("CMAF");
            endpoint_body["Segment"] = json!({"OutputTimestampMode": mode});
            let (status, answer) = emulator
                .create_endpoint(GROUP, channel, &endpoint, endpoint_body)
                .await;
            if *carries {
                assert_eq!(
                    status, 200,
                    "case {label:?} / mode {mode:?}: CreateOriginEndpoint expected 200, \
                     got {status} {answer}"
                );
                let actual = at(&answer, "/Segment/OutputTimestampMode");
                assert_eq!(
                    actual,
                    json!(mode),
                    "case {label:?} / mode {mode:?}: the endpoint must report the mode it \
                     was created with, got {actual}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {label:?} / mode {mode:?}: CreateOriginEndpoint expected 400, got \
                 {status} {answer}"
            );
            assert_eq!(
                answer["ValidationExceptionType"],
                json!(ONLY_NON_EPOCH_LOCKED),
                "case {label:?} / mode {mode:?}: expected {ONLY_NON_EPOCH_LOCKED}, got \
                 {answer}"
            );
            let (status, read) = emulator.get_endpoint(GROUP, channel, &endpoint).await;
            assert_eq!(
                status, 404,
                "case {label:?} / mode {mode:?}: a refused create must mint no endpoint, \
                 got {status} {read}"
            );
        }

        // An endpoint that names no mode: the member is materialised exactly where the
        // channel's stored locking mode carries it, and left absent everywhere else.
        let (status, quiet) = emulator
            .create_endpoint(GROUP, channel, "quiet", minimal_endpoint("CMAF"))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {quiet}"
        );
        let expected = if *carries {
            json!("PASSTHROUGH")
        } else {
            Value::Null
        };
        let actual = at(&quiet, "/Segment/OutputTimestampMode");
        assert_eq!(
            actual, expected,
            "case {label:?}: an endpoint naming no mode expected {expected}, got {actual}"
        );
    }
}
