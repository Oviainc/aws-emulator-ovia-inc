// SPDX-License-Identifier: CC0-1.0

//! `UpdateMultiplex`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The rule: `UpdateMultiplex` replaces, on the one stored multiplex its
//! `MultiplexId` names, exactly the members the request carries — `Name`, the
//! `MultiplexSettings` (whole, not per member) and the `PacketIdentifiersMap` of
//! each program the `PacketIdentifiersMapping` names — leaves every member the
//! request omits as it stands, and answers with that stored multiplex, which
//! `DescribeMultiplex` then reports member for member. It refuses, writing
//! nothing, settings outside the ranges the model publishes for them (400), a
//! `MultiplexId` the store does not hold and a `ProgramName` the multiplex does
//! not hold (404), and a multiplex a delete has already taken away (409).
//!
//! The suites are tables: one row is the case a single recorded call would be.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{
    create_body, created, describe_multiplex, settings, MULTIPLEXES, RANGED_SETTINGS,
    TRANSPORT_STREAM_BITRATE, TRANSPORT_STREAM_ID, TRANSPORT_STREAM_RESERVED_BITRATE,
    VIDEO_BUFFER_DELAY,
};
use crate::multiplex_programs::{create_program, describe_program, program_settings};
use crate::wire::{call, error_code, error_message, medialive};

/// `UpdateMultiplex`: its status and, on success, the multiplex it answers with
/// (`smithy_query(query="operation", operation="UpdateMultiplex")`:
/// `PUT /prod/multiplexes/{MultiplexId}`, success 200, the multiplex under the
/// `multiplex` member).
async fn update_multiplex(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    body: Value,
) -> (u16, Value) {
    let (status, response) = call(
        router,
        "PUT",
        &format!("{MULTIPLEXES}/{multiplex_id}"),
        Some(body),
    )
    .await;
    if status == 200 {
        (status, response["multiplex"].clone())
    } else {
        (status, response)
    }
}

/// An `UpdateMultiplex` request body carrying only the members the row names,
/// so a row that omits one is asserted to have left it alone rather than to have
/// cleared it.
fn update_body(name: Option<&str>, multiplex_settings: Option<Value>) -> Value {
    let mut body = json!({});
    if let Some(name) = name {
        body["name"] = json!(name);
    }
    if let Some(multiplex_settings) = multiplex_settings {
        body["multiplexSettings"] = multiplex_settings;
    }
    body
}

/// Every `MultiplexSettings` member, each at a value well inside its published
/// range: a multiplex created with these is one whose optional members a later
/// update can be asserted to have DROPPED when it carries them no longer.
fn full_settings() -> Value {
    json!({
        "transportStreamBitrate": 20_000_000,
        "transportStreamId": 42,
        "maximumVideoBufferDelayMilliseconds": 1_000,
        "transportStreamReservedBitrate": 1_000,
    })
}

/// A multiplex created with [`full_settings`] under `name`, as its minted id and
/// the multiplex the create answered with.
async fn multiplex_with_full_settings(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, Value) {
    let multiplex = created(
        router,
        case,
        create_body(name, &format!("token-{name}"), full_settings()),
    )
    .await;
    let id = multiplex["id"].as_str().unwrap_or_default().to_string();
    (id, multiplex)
}

/// One row of the replacement table: the case, the `Name` the update carries and
/// the `MultiplexSettings` it carries (`None` for a member it leaves out).
type UpdateRow = (&'static str, Option<&'static str>, Option<Value>);

/// Rule: `UpdateMultiplex` replaces the stored multiplex's `Name` and
/// `MultiplexSettings` with the ones the request carries — the settings whole,
/// so a member the new settings omit is omitted on the multiplex — leaves a
/// member the request does not carry exactly as it stands, and answers with that
/// stored multiplex, which `DescribeMultiplex` then reports member for member.
///
/// The rows differ along which of the two members the request names, along which
/// settings members the new settings name, along each published bound of the
/// ranged settings members, and along whether the request names anything at all
/// — which is what the rule generalises over.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_is_read_back() {
    let rows: [UpdateRow; 9] = [
        ("a new name only", Some("renamed"), None),
        (
            "new settings only",
            None,
            Some(settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0)),
        ),
        (
            "both the name and the settings",
            Some("renamed-and-retuned"),
            Some(settings(7_000_000, 300)),
        ),
        ("neither member", None, None),
        (
            "the settings at their upper bounds",
            Some("maxed"),
            Some(settings(TRANSPORT_STREAM_BITRATE.1, TRANSPORT_STREAM_ID.1)),
        ),
        (
            "every optional settings member, at its lower bound",
            None,
            Some(json!({
                "transportStreamBitrate": TRANSPORT_STREAM_BITRATE.0,
                "transportStreamId": TRANSPORT_STREAM_ID.0,
                "maximumVideoBufferDelayMilliseconds": VIDEO_BUFFER_DELAY.0,
                "transportStreamReservedBitrate": TRANSPORT_STREAM_RESERVED_BITRATE.0,
            })),
        ),
        (
            "every optional settings member, at its upper bound",
            None,
            Some(json!({
                "transportStreamBitrate": TRANSPORT_STREAM_BITRATE.1,
                "transportStreamId": TRANSPORT_STREAM_ID.1,
                "maximumVideoBufferDelayMilliseconds": VIDEO_BUFFER_DELAY.1,
                "transportStreamReservedBitrate": TRANSPORT_STREAM_RESERVED_BITRATE.1,
            })),
        ),
        (
            "the name it already has",
            Some("original"),
            None,
        ),
        (
            "the same settings it already has",
            None,
            Some(full_settings()),
        ),
    ];

    for (case, name, multiplex_settings) in rows {
        let router = medialive();
        let (id, before) = multiplex_with_full_settings(&router, case, "original").await;
        // A second multiplex, so an update is asserted to have touched only the
        // multiplex its path named.
        let (other_id, other_before) =
            multiplex_with_full_settings(&router, case, "neighbour").await;

        let (status, updated) = update_multiplex(
            &router,
            &id,
            update_body(name, multiplex_settings.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateMultiplex: expected 200, got {status}: {updated}"
        );

        // What the multiplex is expected to carry afterwards: the members the
        // request named, and the stored ones for the members it did not. `State`
        // is the one member the clock may have moved on by now.
        let mut expected = before.clone();
        if let Some(name) = name {
            expected["name"] = json!(name);
        }
        if let Some(multiplex_settings) = multiplex_settings.clone() {
            expected["multiplexSettings"] = multiplex_settings;
        }
        expected["state"] = updated["state"].clone();
        assert_eq!(
            updated, expected,
            "case {case:?}: expected the stored multiplex with only the named members replaced, got {updated}"
        );
        // An update is not a lifecycle call: it leaves the multiplex in the
        // state the clock has it in.
        assert!(
            matches!(updated["state"].as_str(), Some("CREATING" | "IDLE")),
            "case {case:?}: expected an update to leave the lifecycle alone, got {:?}",
            updated["state"]
        );

        // The round trip: the reader answers with the stored multiplex, so the
        // update wrote to the one copy of the state rather than to its own
        // response.
        let (status, described) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
        );
        let mut expected = updated.clone();
        expected["state"] = described["state"].clone();
        assert_eq!(
            described, expected,
            "case {case:?}: expected DescribeMultiplex to report the updated multiplex, got {described}"
        );

        // And the multiplex the update did not name is untouched.
        let (_, other) = describe_multiplex(&router, &other_id).await;
        let mut expected = other_before.clone();
        expected["state"] = other["state"].clone();
        assert_eq!(
            other, expected,
            "case {case:?}: expected the other multiplex to survive the update, got {other}"
        );
    }
}

/// Rule: a `MultiplexSettings` member outside the range the model publishes for
/// it is refused as a client error naming the member and the value, and writes
/// nothing — the same values `CreateMultiplex` refuses, since an update cannot
/// store settings a create could not have stored; a value on either published
/// bound is accepted.
///
/// The rows walk EVERY ranged member of the structure to each of its published
/// bounds and one step past each (the ranges in [`RANGED_SETTINGS`]).
#[tokio::test]
async fn an_update_refuses_settings_outside_their_published_range() {
    for (member, (min, max)) in RANGED_SETTINGS {
        for (variant, value, expected) in [
            ("at the minimum", min, 200u16),
            ("below the minimum", min - 1, 400),
            ("at the maximum", max, 200),
            ("past the maximum", max + 1, 400),
        ] {
            let case = format!("{member} {variant} ({value})");
            let router = medialive();
            let (id, before) = multiplex_with_full_settings(&router, &case, "original").await;
            // Only the member under test leaves its range: the two required
            // members otherwise stay well inside theirs.
            let mut multiplex_settings = settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0);
            multiplex_settings[member] = json!(value);

            let (status, body) = update_multiplex(
                &router,
                &id,
                update_body(Some("renamed"), Some(multiplex_settings.clone())),
            )
            .await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected {expected}, got {status}: {body}"
            );

            if expected == 200 {
                assert_eq!(
                    body["multiplexSettings"], multiplex_settings,
                    "case {case:?}: expected the settings the update supplied, got {:?}",
                    body["multiplexSettings"]
                );
                continue;
            }

            // Nested `MultiplexSettings` members are not range-checked at the
            // request boundary (only the operation's own input members are), so
            // the refusal is this operation's own modelled `BadRequestException`
            // rather than the boundary's `ValidationException`.
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(member) && message.contains(&value.to_string()),
                "case {case:?}: expected the refusal to name the member and the value, got {body}"
            );
            // And it wrote NOTHING: neither the settings it carried nor the
            // name it carried beside them reached the stored multiplex.
            let (status, described) = describe_multiplex(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["multiplexSettings"], before["multiplexSettings"],
                "case {case:?}: expected the refused update to write no settings, got {described}"
            );
            assert_eq!(
                described["name"], before["name"],
                "case {case:?}: expected the refused update to write no name, got {described}"
            );
        }
    }
}

/// Rule: `UpdateMultiplex` changes a multiplex that already exists, so a
/// `MultiplexId` the store does not hold is answered with the
/// `NotFoundException` the operation models, naming the id asked for, and writes
/// nothing — including the id of another MediaLive resource, since a multiplex id
/// is only ever one this service minted for a multiplex.
#[tokio::test]
async fn updating_a_multiplex_that_does_not_exist_is_refused() {
    let router = medialive();
    let (id, before) = multiplex_with_full_settings(&router, "kept", "original").await;

    // A channel exists too, so a request that passes ITS id to the multiplex
    // writer is refused rather than applied to the wrong store.
    let (status, response) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "not-a-multiplex" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateChannel: expected 201, got {status}: {response}"
    );
    let channel_id = response["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, named) in [
        ("never minted", "9999999".to_string()),
        ("a channel's id", channel_id),
        ("not a number at all", "no-such-multiplex".to_string()),
    ] {
        let (status, body) = update_multiplex(
            &router,
            &named,
            update_body(Some("renamed"), Some(settings(3_000_000, 7))),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Multiplex {named} not found"),
            "case {case:?}: expected the refusal to name the id asked for, got {body}"
        );
        // The refused update created nothing: the id it named still names no
        // multiplex.
        let (status, described) = describe_multiplex(&router, &named).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused update to store nothing, got {status}: {described}"
        );
    }

    // And none of those refusals touched the multiplex that does exist.
    let (status, described) = describe_multiplex(&router, &id).await;
    assert_eq!(
        status, 200,
        "DescribeMultiplex: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described["name"], before["name"],
        "expected the stored multiplex to keep its name, got {described}"
    );
    assert_eq!(
        described["multiplexSettings"], before["multiplexSettings"],
        "expected the stored multiplex to keep its settings, got {described}"
    );
}

/// Every member of `MultiplexProgramPacketIdentifiersMap`, by wire name, with
/// whether it is a list of PIDs rather than one
/// (`smithy_query(query="shape", shape="MultiplexProgramPacketIdentifiersMap")`:
/// audioPids, dvbSubPids, dvbTeletextPid, etvPlatformPid, etvSignalPid,
/// klvDataPids, pcrPid, pmtPid, privateMetadataPid, scte27Pids, scte35Pid,
/// timedMetadataPid, videoPid, aribCaptionsPid, dvbTeletextPids, ecmPid,
/// smpte2038Pid).
const PACKET_IDENTIFIER_MEMBERS: [(&str, bool); 17] = [
    ("audioPids", true),
    ("dvbSubPids", true),
    ("dvbTeletextPid", false),
    ("etvPlatformPid", false),
    ("etvSignalPid", false),
    ("klvDataPids", true),
    ("pcrPid", false),
    ("pmtPid", false),
    ("privateMetadataPid", false),
    ("scte27Pids", true),
    ("scte35Pid", false),
    ("timedMetadataPid", false),
    ("videoPid", false),
    ("aribCaptionsPid", false),
    ("dvbTeletextPids", true),
    ("ecmPid", false),
    ("smpte2038Pid", false),
];

/// A `MultiplexProgramPacketIdentifiersMap` naming every member of the shape,
/// each PID derived from `first` so two programs of one test carry different
/// maps.
fn every_packet_identifier(first: i64) -> Value {
    let mut map = json!({});
    for (offset, (member, is_list)) in PACKET_IDENTIFIER_MEMBERS.into_iter().enumerate() {
        let pid = first + i64::try_from(offset).expect("a small offset");
        map[member] = if is_list {
            json!([pid, pid + 100])
        } else {
            json!(pid)
        };
    }
    map
}

/// Rule: `UpdateMultiplex`'s `PacketIdentifiersMapping` is keyed by the
/// `ProgramName` the identifiers belong to ("Packet identifiers map for a given
/// Multiplex program"), so each entry replaces the `PacketIdentifiersMap` of
/// that program of the named multiplex — read back by
/// `DescribeMultiplexProgram` — and a program the mapping does not name keeps
/// the map it had.
///
/// The rows differ along which members of the identifiers map are named (one
/// PID, one list of PIDs, every member of the shape), and along how many of the
/// multiplex's programs the mapping names — which is what the rule generalises
/// over.
#[tokio::test]
async fn a_packet_identifiers_mapping_sets_the_pids_of_the_programs_it_names() {
    // (case, the mapping the update carries)
    let rows: [(&str, Value); 5] = [
        ("one PID of one program", json!({ "news": { "videoPid": 481 } })),
        (
            "one list of PIDs of one program",
            json!({ "news": { "audioPids": [482, 483] } }),
        ),
        (
            "every member of the identifiers map",
            json!({ "news": every_packet_identifier(500) }),
        ),
        (
            "both programs of the multiplex",
            json!({
                "news": every_packet_identifier(600),
                "sports": every_packet_identifier(700),
            }),
        ),
        ("no program at all", json!({})),
    ];

    for (case, mapping) in rows {
        let router = medialive();
        let (id, _) = multiplex_with_full_settings(&router, case, "original").await;
        for (program_name, program_number) in [("news", 1), ("sports", 2)] {
            let (status, created) = create_program(
                &router,
                &id,
                program_name,
                &format!("token-{program_name}"),
                program_settings(program_number),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}/{program_name}: CreateMultiplexProgram: expected 201, got {status}: {created}"
            );
            // A created program carries no packet identifiers: the real
            // service's multiplexer allocates them, and this emulator does not
            // run it, so only a caller's mapping can set them.
            assert_eq!(
                created["packetIdentifiersMap"],
                Value::Null,
                "case {case:?}/{program_name}: expected no packet identifiers before the update, got {created}"
            );
        }

        let mut body = update_body(Some("remapped"), None);
        body["packetIdentifiersMapping"] = mapping.clone();
        let (status, updated) = update_multiplex(&router, &id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateMultiplex: expected 200, got {status}: {updated}"
        );
        // The mapping is not part of the multiplex the response carries: the
        // model's `Multiplex` has no such member, and the programs are where it
        // is read back from.
        assert_eq!(
            updated["name"],
            json!("remapped"),
            "case {case:?}: expected the name the update supplied, got {:?}",
            updated["name"]
        );

        for program_name in ["news", "sports"] {
            let (status, described) = describe_program(&router, &id, program_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{program_name}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
            );
            // A program the mapping named carries exactly the identifiers it
            // carried; one it did not name carries none, as it did before.
            let expected = mapping
                .get(program_name)
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                described["packetIdentifiersMap"], expected,
                "case {case:?}/{program_name}: expected the packet identifiers the mapping supplied, got {:?}",
                described["packetIdentifiersMap"]
            );
        }
    }
}

/// Rule: a `PacketIdentifiersMapping` entry names the program its identifiers
/// belong to, so a `ProgramName` the named multiplex does not hold is answered
/// with the `NotFoundException` the operation models, naming the program asked
/// for, and the whole update writes nothing — not the identifiers of the
/// programs that do exist, and not the `Name` it carried beside them.
#[tokio::test]
async fn a_mapping_naming_a_program_the_multiplex_does_not_hold_is_refused() {
    let router = medialive();
    let (id, before) = multiplex_with_full_settings(&router, "live", "original").await;
    let (status, created) = create_program(
        &router,
        &id,
        "news",
        "token-news",
        program_settings(1),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateMultiplexProgram: expected 201, got {status}: {created}"
    );
    // Another multiplex holds a program of its own, so a mapping that asks the
    // first multiplex for THAT name is refused rather than applied to the wrong
    // multiplex's program.
    let (other_id, _) = multiplex_with_full_settings(&router, "other", "neighbour").await;
    let (status, elsewhere) = create_program(
        &router,
        &other_id,
        "elsewhere",
        "token-elsewhere",
        program_settings(1),
    )
    .await;
    assert_eq!(
        status, 201,
        "the other CreateMultiplexProgram: expected 201, got {status}: {elsewhere}"
    );

    // (case, the program names the mapping keys, the name the refusal names)
    let rows: [(&str, &[&str], &str); 4] = [
        ("a program name no program has", &["no-such-program"], "no-such-program"),
        (
            "a program name that is another multiplex's",
            &["elsewhere"],
            "elsewhere",
        ),
        (
            "one program that exists and one that does not",
            &["news", "absent"],
            "absent",
        ),
        ("the empty program name", &[""], ""),
    ];

    for (case, program_names, named) in rows {
        let mut mapping = json!({});
        for program_name in program_names {
            mapping[*program_name] = every_packet_identifier(900);
        }
        let mut body = update_body(Some("renamed"), None);
        body["packetIdentifiersMapping"] = mapping;

        let (status, refusal) = update_multiplex(&router, &id, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            format!("MultiplexProgram {named} not found"),
            "case {case:?}: expected the refusal to name the program asked for, got {refusal}"
        );

        // Nothing was written: the program that does exist still carries no
        // packet identifiers, and the multiplex still carries its own name.
        let (status, described) = describe_program(&router, &id, "news").await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["packetIdentifiersMap"],
            Value::Null,
            "case {case:?}: expected the refused update to set no packet identifiers, got {described}"
        );
        let (_, multiplex) = describe_multiplex(&router, &id).await;
        assert_eq!(
            multiplex["name"], before["name"],
            "case {case:?}: expected the refused update to write no name, got {multiplex}"
        );
    }
}

/// Rule: a multiplex a delete has already been accepted for is still READABLE —
/// `DescribeMultiplex` goes on reporting it in `DELETING` and then in the
/// terminal `DELETED` — but it names no multiplex a caller may still change, so
/// an update of it is a conflict with its state, naming that state, and writes
/// nothing.
#[tokio::test]
async fn a_multiplex_a_delete_has_taken_away_cannot_be_updated() {
    let router = medialive();
    let (id, before) = multiplex_with_full_settings(&router, "deleted", "original").await;
    // A delete is accepted only once the multiplex has stopped, which is the
    // state the clock owes a new one.
    wait_for_the_clock();
    let (status, deleting) = call(&router, "DELETE", &format!("{MULTIPLEXES}/{id}"), None).await;
    assert_eq!(
        status, 202,
        "DeleteMultiplex: expected 202, got {status}: {deleting}"
    );

    // Both states a delete leaves the record in: the `DELETING` it starts in and
    // the terminal `DELETED` the clock owes it.
    for (case, expected_state) in [("DELETING", "DELETING"), ("DELETED", "DELETED")] {
        if expected_state == "DELETED" {
            wait_for_the_clock();
        }
        let (status, described) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected the record to still be readable, got {status}: {described}"
        );
        assert_eq!(
            described["state"],
            json!(expected_state),
            "case {case:?}: the test did not reach {expected_state}, it reached {:?}",
            described["state"]
        );

        let (status, refusal) = update_multiplex(
            &router,
            &id,
            update_body(Some("renamed"), Some(settings(3_000_000, 7))),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        let message = error_message(&refusal);
        assert!(
            message.contains(&id) && message.contains(expected_state),
            "case {case:?}: expected the refusal to name the multiplex and its state, got {refusal}"
        );

        // And it wrote nothing: the record a reader still reports carries the
        // name and the settings it was created with.
        let (_, described) = describe_multiplex(&router, &id).await;
        assert_eq!(
            described["name"], before["name"],
            "case {case:?}: expected the refused update to write no name, got {described}"
        );
        assert_eq!(
            described["multiplexSettings"], before["multiplexSettings"],
            "case {case:?}: expected the refused update to write no settings, got {described}"
        );
    }
}
