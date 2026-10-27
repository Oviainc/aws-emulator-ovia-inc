// SPDX-License-Identifier: CC0-1.0

//! The PACKET IDENTIFIERS a multiplex program carries, end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The rule: a program's `PacketIdentifiersMap` is supplied by the SERVICE, not
//! by the caller — the PIDs the program's streams and its own tables occupy in
//! its multiplex's transport stream are allocated to it when it is created, and
//! every read of that program reports them. Within one multiplex the allocation
//! gives each stream an identifier of its own, inside the range the service
//! publishes for a transport-stream PID, and no identifier of one program is an
//! identifier of another.
//!
//! The suites are tables: one program of one multiplex is ONE row of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_program_deletion::delete_program;
use crate::multiplex_program_update::update_program;
use crate::multiplex_programs::{
    create_program, describe_program, multiplex, program_settings, with_member,
    PREFERRED_CHANNEL_PIPELINES, PROGRAM_NUMBER, STATMUX_PRIORITY, VIDEO_BITRATE,
};
use crate::wire::medialive;

/// The inclusive range a transport-stream packet identifier of this service is
/// published within: "Valid values are 32 (or 0x20)..8182 (or 0x1ff6)"
/// (`smithy_query(query="doc", shape="M2tsSettings", member="VideoPid")`, the
/// same sentence on `PmtPid`, `Scte35Pid`, `AudioPids` and every other PID
/// member the model documents a range for).
const PACKET_IDENTIFIER_RANGE: (i64, i64) = (32, 8_182);

/// Every single-valued member of a `MultiplexProgramPacketIdentifiersMap`
/// (`smithy_query(query="shape",
/// shape="MultiplexProgramPacketIdentifiersMap")`), so the table spans the
/// shape rather than the members the issue happened to name.
const SINGLE_PACKET_IDENTIFIERS: [&str; 12] = [
    "aribCaptionsPid",
    "dvbTeletextPid",
    "ecmPid",
    "etvPlatformPid",
    "etvSignalPid",
    "pcrPid",
    "pmtPid",
    "privateMetadataPid",
    "scte35Pid",
    "smpte2038Pid",
    "timedMetadataPid",
    "videoPid",
];

/// Every multi-valued member of that same shape.
const PACKET_IDENTIFIER_LISTS: [&str; 5] = [
    "audioPids",
    "dvbSubPids",
    "dvbTeletextPids",
    "klvDataPids",
    "scte27Pids",
];

/// The one member nothing is allocated for: the model documents it as "This
/// field is unused and deprecated" (`smithy_query(query="doc",
/// shape="M2tsSettings", member="EcmPid")`).
const DEPRECATED_PACKET_IDENTIFIER: &str = "ecmPid";

/// How many of the reported identifiers name a stream another member already
/// named, so they are the same number rather than one of their own: `PcrPid`
/// ("When no value is given, the encoder will assign the same value as the Video
/// PID") and `DvbTeletextPids`, which names the same DVB teletext data as
/// `DvbTeletextPid`.
const ALIASED_PACKET_IDENTIFIERS: usize = 2;

/// Every packet identifier the program's map reports, asserted along the way to
/// be an allocation rather than an omission: each member of the shape populated
/// with an integer (bar the deprecated one), each identifier inside the
/// published range, the PCR carried on the program's video identifier, and every
/// other stream of the program on an identifier of its own.
fn allocated_identifiers(case: &str, program: &Value) -> Vec<i64> {
    let map = &program["packetIdentifiersMap"];
    assert!(
        map.is_object(),
        "case {case:?}: expected the program to report a packetIdentifiersMap, got {program}"
    );

    let mut identifiers: Vec<i64> = Vec::new();
    for member in SINGLE_PACKET_IDENTIFIERS {
        if member == DEPRECATED_PACKET_IDENTIFIER {
            assert_eq!(
                map[member],
                Value::Null,
                "case {case:?}: expected nothing allocated for the deprecated {member}, got {:?}",
                map[member]
            );
            continue;
        }
        identifiers.push(map[member].as_i64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an allocated {member}, got {:?}",
                map[member]
            )
        }));
    }
    for member in PACKET_IDENTIFIER_LISTS {
        let listed = map[member].as_array().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an allocated {member}, got {:?}",
                map[member]
            )
        });
        assert!(
            !listed.is_empty(),
            "case {case:?}: expected {member} to carry an identifier, got {:?}",
            map[member]
        );
        for entry in listed {
            identifiers.push(entry.as_i64().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected {member} to carry identifiers, got {:?}",
                    map[member]
                )
            }));
        }
    }

    for identifier in &identifiers {
        assert!(
            (PACKET_IDENTIFIER_RANGE.0..=PACKET_IDENTIFIER_RANGE.1).contains(identifier),
            "case {case:?}: expected every identifier inside {PACKET_IDENTIFIER_RANGE:?}, \
             got {identifier} in {map}"
        );
    }
    assert_eq!(
        map["pcrPid"], map["videoPid"],
        "case {case:?}: expected the PCR on the program's video identifier, got {map}"
    );
    assert_eq!(
        map["dvbTeletextPids"],
        json!([map["dvbTeletextPid"]]),
        "case {case:?}: expected the teletext list to name the program's teletext \
         identifier, got {map}"
    );

    let mut distinct = identifiers.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        identifiers.len() - ALIASED_PACKET_IDENTIFIERS,
        "case {case:?}: expected every stream of the program on an identifier of its own, \
         got {map}"
    );
    identifiers
}

/// One row of the allocation table: the case, the program's name and the
/// `MultiplexProgramSettings` the create carries.
type ProgramRow = (&'static str, &'static str, Value);

/// The rows the allocation table spans: settings differing along which optional
/// members they name, along the enum value they pick and along each published
/// bound of `ProgramNumber` — none of which the identifiers a program is
/// allocated depend on, which is what this table is for.
fn allocation_rows() -> Vec<ProgramRow> {
    let mut rows: Vec<ProgramRow> = vec![
        (
            "the required member only, at its lower bound",
            "news",
            program_settings(PROGRAM_NUMBER.0),
        ),
        (
            "a program number at its upper bound",
            "sports",
            program_settings(PROGRAM_NUMBER.1),
        ),
        (
            "a constant bitrate",
            "cbr",
            with_member(
                program_settings(11),
                "videoSettings.constantBitrate",
                VIDEO_BITRATE.1,
            ),
        ),
        (
            "every statmux member",
            "statmux",
            json!({
                "programNumber": 12,
                "videoSettings": {
                    "statmuxSettings": {
                        "maximumBitrate": VIDEO_BITRATE.1,
                        "minimumBitrate": VIDEO_BITRATE.0,
                        "priority": STATMUX_PRIORITY.0,
                    }
                }
            }),
        ),
        (
            "a service descriptor",
            "described",
            json!({
                "programNumber": 13,
                "serviceDescriptor": {
                    "providerName": "a-provider",
                    "serviceName": "a-service",
                },
            }),
        ),
    ];
    // One row per value of the one enum a `MultiplexProgramSettings` reaches.
    for (index, pipeline) in PREFERRED_CHANNEL_PIPELINES.iter().enumerate() {
        rows.push((
            pipeline,
            pipeline,
            json!({
                "programNumber": 100 + index as i64,
                "preferredChannelPipeline": pipeline,
            }),
        ));
    }
    rows
}

/// Rule: the service allocates a program its transport-stream packet
/// identifiers when it is created, so `CreateMultiplexProgram` answers with a
/// populated `PacketIdentifiersMap` — every member of the shape carrying an
/// identifier inside the published PID range — and every later read of that
/// program reports the SAME identifiers: a describe, a repeated describe, and a
/// describe after an update that replaced the program's settings.
#[tokio::test]
async fn a_created_program_is_allocated_identifiers_every_read_reports() {
    for (case, program_name, settings) in allocation_rows() {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "programmed").await;

        let (status, created) = create_program(
            &router,
            &multiplex_id,
            program_name,
            &format!("token-{program_name}"),
            settings,
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram: expected 201, got {status}: {created}"
        );
        let identifiers = allocated_identifiers(case, &created);
        let allocated = created["packetIdentifiersMap"].clone();

        // Two reads in a row: a read reports the allocation, it does not make
        // one, so the second read answers exactly as the first did.
        for read in ["the first read", "the second read"] {
            let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{read}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["packetIdentifiersMap"], allocated,
                "case {case:?}/{read}: expected the identifiers allocated at creation \
                 ({allocated}), got {:?}",
                described["packetIdentifiersMap"]
            );
            assert_eq!(
                allocated_identifiers(case, &described),
                identifiers,
                "case {case:?}/{read}: expected the same identifiers the create reported, \
                 got {described}"
            );
        }

        // The identifiers are the service's, not a function of the settings: an
        // update that replaces those settings whole leaves them alone.
        let (status, updated) = update_program(
            &router,
            &multiplex_id,
            program_name,
            Some(program_settings(PROGRAM_NUMBER.1 - 1)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateMultiplexProgram: expected 200, got {status}: {updated}"
        );
        assert_eq!(
            updated["packetIdentifiersMap"], allocated,
            "case {case:?}: expected the update to report the allocated identifiers \
             ({allocated}), got {:?}",
            updated["packetIdentifiersMap"]
        );
        let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram after the update: expected 200, \
             got {status}: {described}"
        );
        assert_eq!(
            described["packetIdentifiersMap"], allocated,
            "case {case:?}: expected the update to leave the allocated identifiers alone, \
             got {described}"
        );
    }
}

/// The identifiers of every program named, as (program name, identifiers) pairs,
/// asserted to have been allocated.
async fn identifiers_of(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    program_names: &[String],
) -> Vec<(String, Vec<i64>)> {
    let mut allocated = Vec::new();
    for program_name in program_names {
        let (status, described) = describe_program(router, multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}/{program_name}: DescribeMultiplexProgram: expected 200, \
             got {status}: {described}"
        );
        allocated.push((
            program_name.clone(),
            allocated_identifiers(case, &described),
        ));
    }
    allocated
}

/// Assert that no identifier of one of these programs is an identifier of
/// another: the programs of one multiplex share one transport stream, so each
/// stream of each of them needs a PID nothing else is using.
fn identifiers_are_disjoint(case: &str, allocated: &[(String, Vec<i64>)]) {
    for (index, (program_name, identifiers)) in allocated.iter().enumerate() {
        for (other_name, other) in &allocated[index + 1..] {
            let shared: Vec<i64> = identifiers
                .iter()
                .copied()
                .filter(|identifier| other.contains(identifier))
                .collect();
            assert!(
                shared.is_empty(),
                "case {case:?}: expected the identifiers of {program_name} and {other_name} \
                 to be disjoint, both carry {shared:?}"
            );
        }
    }
}

/// Rule: the identifiers of the programs of ONE multiplex are disjoint — a
/// transport stream carries each stream on a PID of its own — and that holds for
/// a program created after another was deleted, whose identifiers may not
/// collide with any program the multiplex still holds.
///
/// The rows differ along how many programs the multiplex holds, which is what
/// the rule generalises over.
#[tokio::test]
async fn the_programs_of_one_multiplex_are_allocated_disjoint_identifiers() {
    for count in [1usize, 2, 3, 7] {
        let case = format!("{count} programs");
        let router = medialive();
        let multiplex_id = multiplex(&router, &case, "crowded").await;

        let mut program_names: Vec<String> = Vec::new();
        for index in 0..count {
            let program_name = format!("program-{index}");
            let (status, created) = create_program(
                &router,
                &multiplex_id,
                &program_name,
                &format!("token-{program_name}"),
                program_settings(index as i64 + 1),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}/{program_name}: CreateMultiplexProgram: expected 201, \
                 got {status}: {created}"
            );
            program_names.push(program_name);
        }

        let allocated = identifiers_of(&router, &case, &multiplex_id, &program_names).await;
        identifiers_are_disjoint(&case, &allocated);

        // The first program goes away and another takes its place: whatever the
        // allocation hands the newcomer, it is not an identifier one of the
        // surviving programs is still using.
        let removed = program_names.remove(0);
        let (status, deleted) = delete_program(&router, &multiplex_id, &removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMultiplexProgram: expected 200, got {status}: {deleted}"
        );
        let (status, created) = create_program(
            &router,
            &multiplex_id,
            "newcomer",
            "token-newcomer",
            program_settings(count as i64 + 1),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram for the newcomer: expected 201, \
             got {status}: {created}"
        );
        program_names.push("newcomer".to_string());

        let allocated = identifiers_of(&router, &case, &multiplex_id, &program_names).await;
        identifiers_are_disjoint(&case, &allocated);
    }
}

/// Rule: the identifiers are allocated within ONE multiplex's transport stream,
/// so what a program is allocated depends only on the programs of the multiplex
/// that holds it: the same program of a second, independent multiplex is
/// allocated the same identifiers.
///
/// The rows differ along how many programs each multiplex already holds when the
/// program under test is created.
#[tokio::test]
async fn identifiers_are_allocated_per_multiplex() {
    for before in [0usize, 1, 4] {
        let case = format!("after {before} programs");
        let router = medialive();
        let mut allocated: Vec<Value> = Vec::new();

        for multiplex_name in ["first", "second"] {
            let multiplex_id = multiplex(&router, &case, multiplex_name).await;
            for index in 0..before {
                let program_name = format!("{multiplex_name}-filler-{index}");
                let (status, created) = create_program(
                    &router,
                    &multiplex_id,
                    &program_name,
                    &format!("token-{program_name}"),
                    program_settings(index as i64 + 1),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}/{program_name}: CreateMultiplexProgram: expected 201, \
                     got {status}: {created}"
                );
            }

            let program_name = format!("{multiplex_name}-program");
            let (status, created) = create_program(
                &router,
                &multiplex_id,
                &program_name,
                &format!("token-{program_name}"),
                program_settings(before as i64 + 1),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}/{program_name}: CreateMultiplexProgram: expected 201, \
                 got {status}: {created}"
            );
            allocated_identifiers(&case, &created);
            allocated.push(created["packetIdentifiersMap"].clone());
        }

        assert_eq!(
            allocated[0], allocated[1],
            "case {case:?}: expected a program of a second multiplex to be allocated the \
             same identifiers ({}), got {}",
            allocated[0], allocated[1]
        );
    }
}
