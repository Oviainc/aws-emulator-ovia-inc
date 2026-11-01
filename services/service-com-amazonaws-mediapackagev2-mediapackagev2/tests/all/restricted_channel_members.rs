// SPDX-License-Identifier: CC0-1.0

//! The two sides of ONE restriction, for a channel member the model restricts to
//! particular `InputType`s, driven end to end over the same router the emulator boots.
//!
//! The rule under test, asserted over a SET of channels rather than over one recorded
//! request: for a member whose documentation both restricts which input types may carry
//! it and gives the service's own default, the input types that CARRY it are exactly the
//! input types that ACCEPT it — so a channel of an input type that may not carry the
//! member reports none, and a create that names the member on such a channel is refused
//! with the `ValidationExceptionType` the model declares for that combination and stores
//! nothing. A stored member and a refused one can never disagree.
//!
//! `OutputLockingMode` is the channel member documented that way: "If you don't specify a
//! value, the default is `EPOCH_LOCKED`", with the input types that may configure it named
//! by the refusal the service answers for it ("OutputLockingMode can only be configured
//! when the channel InputType is CMAF or MULTIVIEW") and
//! `ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE` naming that refusal
//! (`smithy_query(query="shape", shape="OutputLockingMode,ValidationExceptionType")`).
//! The rows span the model's whole `InputType` enum (HLS | CMAF | MULTIVIEW) plus the
//! absent case, which `CreateChannelRequest$InputType` defaults to HLS, against each value
//! of the member's own enum — including the one VALUE an input type that carries the member
//! may not take, which is its own refusal and the subject of
//! `tests/all/multiview_epoch_locking.rs`. What every projection of a channel reports for
//! the members the service applies is the sibling suite,
//! `tests/all/channel_members_by_input_type.rs`.

use serde_json::json;

// The crate's one emulator client, and the two helpers the sibling suite exposes, so
// neither the client nor the multiview configuration a MULTIVIEW row needs is written
// twice.
use crate::channel_lifecycle::{at, error_type, Emulator};
use crate::channel_members_by_input_type::multiview_members;

/// RULE — the input types that carry the locking mode are the input types that accept a
/// submitted one.
#[tokio::test]
async fn the_input_types_that_carry_the_locking_mode_are_the_ones_that_accept_it() {
    // (label, the InputType the create names, whether that input type carries the member,
    // the one value of the member that input type may not take)
    let rows: &[(&str, Option<&str>, bool, Option<&str>)] = &[
        ("no input type at all, so the default HLS", None, false, None),
        ("an HLS channel", Some("HLS"), false, None),
        ("a CMAF channel", Some("CMAF"), true, None),
        (
            "a multiview channel",
            Some("MULTIVIEW"),
            true,
            Some("NON_EPOCH_LOCKED"),
        ),
    ];

    for (label, input_type, carries, refused_value) in rows {
        let emulator = Emulator::new();
        let group = "locking-group";
        assert_eq!(
            emulator.create_group(group, json!({})).await.0,
            200,
            "case {label:?}: CreateChannelGroup must succeed"
        );
        // A multiview row's tile source must be a CMAF channel of the same group, so it
        // is seeded for every row; the rows' own channels are named separately.
        assert_eq!(
            emulator
                .create_channel(group, "tile-source", json!({"InputType": "CMAF"}))
                .await
                .0,
            200,
            "case {label:?}: seeding the CMAF tile source must succeed"
        );
        let base = || {
            let mut body = if *input_type == Some("MULTIVIEW") {
                multiview_members()
            } else {
                json!({})
            };
            if let Some(input_type) = input_type {
                body["InputType"] = json!(input_type);
            }
            body
        };

        // The reporting side: a channel created without the member carries the service's
        // default exactly where its input type may carry the member at all.
        let (status, created) = emulator.create_channel(group, "quiet", base()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );
        let reported = at(&created, "/OutputLockingMode");
        assert_eq!(
            reported.is_string(),
            *carries,
            "case {label:?}: OutputLockingMode reported as {reported}, while this input \
             type carrying one is {carries}"
        );
        let (status, read) = emulator.get_channel(group, "quiet").await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        let actual = at(&read, "/OutputLockingMode");
        assert_eq!(
            actual, reported,
            "case {label:?}: GetChannel must report the mode the create reported \
             ({reported}), got {actual}"
        );

        // The accepting side: the same member SUBMITTED, at each value of its own enum.
        // An input type that carries the member accepts every value but the one its own
        // discriminator names, which is refused as a value rather than as a member.
        for mode in ["EPOCH_LOCKED", "NON_EPOCH_LOCKED"] {
            let mut body = base();
            body["OutputLockingMode"] = json!(mode);
            let name = format!("named-{}", mode.to_lowercase());
            let (status, answer) = emulator.create_channel(group, &name, body).await;
            let expected_type = if !*carries {
                Some("ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE")
            } else if *refused_value == Some(mode) {
                Some("MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED")
            } else {
                None
            };
            if let Some(expected_type) = expected_type {
                assert_eq!(
                    status, 400,
                    "case {label:?}: CreateChannel naming {mode} expected 400, got \
                     {status} {answer}"
                );
                assert_eq!(
                    error_type(&answer),
                    "ValidationException",
                    "case {label:?}: naming {mode} expected ValidationException, got \
                     {answer}"
                );
                assert_eq!(
                    answer["ValidationExceptionType"],
                    json!(expected_type),
                    "case {label:?}: naming {mode} expected {expected_type}, got {answer}"
                );
                assert_eq!(
                    emulator.get_channel(group, &name).await.0,
                    404,
                    "case {label:?}: a refused create naming {mode} must store nothing"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {label:?}: CreateChannel naming {mode} expected 200, got \
                 {status} {answer}"
            );
            let actual = at(&answer, "/OutputLockingMode");
            assert_eq!(
                actual,
                json!(mode),
                "case {label:?}: the channel must report the {mode} it was created \
                 with, got {actual}"
            );
        }
    }
}
