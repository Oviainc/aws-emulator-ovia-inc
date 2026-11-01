// SPDX-License-Identifier: CC0-1.0

//! `DeleteMultiplexProgram`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The rule: `DeleteMultiplexProgram` takes the program its `MultiplexId` and
//! `ProgramName` name out of the multiplex that holds it and answers with that
//! program as it stood when the delete removed it, so the multiplex stops
//! counting, listing and reporting it and its name and `ProgramNumber` are free
//! again; a multiplex or a program the store does not hold is a not-found, and a
//! multiplex a delete has already been accepted for is a conflict.
//!
//! The suites are tables: the recorded case (one program of one multiplex,
//! deleted after its settings were updated) is ONE row of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{describe_multiplex, MULTIPLEXES};
use crate::multiplex_programs::{
    create_program, describe_program, multiplex, program_settings, with_member,
    PREFERRED_CHANNEL_PIPELINES, PROGRAM_NUMBER, STATMUX_PRIORITY, VIDEO_BITRATE,
};
use crate::wire::{call, error_code, error_message, medialive};

/// `DeleteMultiplexProgram`'s binding (`smithy_query(query="operation",
/// operation="DeleteMultiplexProgram")`: `DELETE
/// /prod/multiplexes/{MultiplexId}/programs/{ProgramName}`, success 200).
pub(crate) async fn delete_program(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    program_name: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("{MULTIPLEXES}/{multiplex_id}/programs/{program_name}"),
        None,
    )
    .await
}

/// `ListMultiplexPrograms` on one multiplex, as the `ProgramName`s it reports in
/// the order it reports them (`smithy_query(query="operation",
/// operation="ListMultiplexPrograms")`: GET
/// `/prod/multiplexes/{MultiplexId}/programs`).
pub(crate) async fn listed_names(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("{MULTIPLEXES}/{multiplex_id}/programs"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMultiplexPrograms: expected 200, got {status}: {body}"
    );
    body["multiplexPrograms"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a multiplexPrograms list, got {body}"))
        .iter()
        .map(|program| program["programName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every program of `programs` created in `multiplex_id`, with each create
/// asserted to have succeeded, as the program each one answered with keyed by
/// its name.
async fn stored(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    programs: &[(&str, Value)],
) -> Vec<(String, Value)> {
    let mut created = Vec::new();
    for (program_name, settings) in programs {
        let (status, program) = create_program(
            router,
            multiplex_id,
            program_name,
            &format!("token-{multiplex_id}-{program_name}"),
            settings.clone(),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram({program_name}): expected 201, \
             got {status}: {program}"
        );
        created.push(((*program_name).to_string(), program));
    }
    created
}

/// `UpdateMultiplex` carrying nothing but a `PacketIdentifiersMapping`, which is
/// the one thing that sets a program's `PacketIdentifiersMap`, asserted to have
/// succeeded (`PUT /prod/multiplexes/{MultiplexId}`).
async fn map_packet_identifiers(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    mapping: Value,
) {
    let (status, body) = call(
        router,
        "PUT",
        &format!("{MULTIPLEXES}/{multiplex_id}"),
        Some(json!({ "packetIdentifiersMapping": mapping })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateMultiplex: expected 200, got {status}: {body}"
    );
}

/// One row of the deletion table: the case, the programs the multiplex holds
/// (name and the `MultiplexProgramSettings` the create carries), the name of the
/// one the delete names, and a `PacketIdentifiersMapping` applied before the
/// delete (empty when the row sets no identifiers).
type DeletionRow = (
    &'static str,
    Vec<(&'static str, Value)>,
    &'static str,
    Option<Value>,
);

/// The rows of the deletion table: they differ along the settings the deleted
/// program carries (which optional members it names, each published bound of the
/// ranged ones, each value of the one enum those settings reach), along how many
/// programs the multiplex holds, along WHICH of them the delete names, and along
/// whether the program carries packet identifiers a caller mapped onto it — all
/// of which is what the rule generalises over.
fn deletion_rows() -> Vec<DeletionRow> {
    let mut rows: Vec<DeletionRow> = vec![
        (
            "the only program, required member only",
            vec![("news", program_settings(PROGRAM_NUMBER.0))],
            "news",
            None,
        ),
        (
            "the only program, program number at its upper bound",
            vec![("sports", program_settings(PROGRAM_NUMBER.1))],
            "sports",
            None,
        ),
        (
            "the first of three, by name order",
            vec![
                ("alpha", program_settings(1)),
                ("beta", program_settings(2)),
                ("gamma", program_settings(3)),
            ],
            "alpha",
            None,
        ),
        (
            "the middle of three",
            vec![
                ("alpha", program_settings(1)),
                ("beta", program_settings(2)),
                ("gamma", program_settings(3)),
            ],
            "beta",
            None,
        ),
        (
            "the last of three",
            vec![
                ("alpha", program_settings(1)),
                ("beta", program_settings(2)),
                ("gamma", program_settings(3)),
            ],
            "gamma",
            None,
        ),
        (
            "a program carrying a constant bitrate at its lower bound",
            vec![(
                "cbr-low",
                with_member(
                    program_settings(10),
                    "videoSettings.constantBitrate",
                    VIDEO_BITRATE.0,
                ),
            )],
            "cbr-low",
            None,
        ),
        (
            "a program carrying a constant bitrate at its upper bound",
            vec![(
                "cbr-high",
                with_member(
                    program_settings(11),
                    "videoSettings.constantBitrate",
                    VIDEO_BITRATE.1,
                ),
            )],
            "cbr-high",
            None,
        ),
        (
            "a program carrying every statmux member, at its bounds",
            vec![(
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
            )],
            "statmux",
            None,
        ),
        (
            "a program carrying a service descriptor",
            vec![(
                "described",
                json!({
                    "programNumber": 13,
                    "serviceDescriptor": {
                        "providerName": "a-provider",
                        "serviceName": "a-service",
                    },
                }),
            )],
            "described",
            None,
        ),
        (
            "a program a caller mapped packet identifiers onto",
            vec![
                ("mapped", program_settings(21)),
                ("unmapped", program_settings(22)),
            ],
            "mapped",
            Some(json!({ "mapped": { "videoPid": 481, "audioPids": [482, 483] } })),
        ),
    ];
    // One row per value of the one enum a `MultiplexProgramSettings` reaches, so
    // the table spans that catalog rather than the value the recording carried.
    for (index, pipeline) in PREFERRED_CHANNEL_PIPELINES.iter().enumerate() {
        rows.push((
            pipeline,
            vec![(
                pipeline,
                json!({
                    "programNumber": 100 + index as i64,
                    "preferredChannelPipeline": pipeline,
                }),
            )],
            pipeline,
            None,
        ));
    }
    rows
}

/// Rule: `DeleteMultiplexProgram` removes the named program from the named
/// multiplex and answers with that program exactly as a read of it answered just
/// before — so afterwards the multiplex's derived `ProgramCount` no longer counts
/// it, `ListMultiplexPrograms` no longer reports it, a describe of it and a
/// second delete of it are both not-founds, and every OTHER program of the same
/// multiplex is left exactly as it was.
#[tokio::test]
async fn a_deleted_program_leaves_the_multiplex_and_is_answered_with() {
    for (case, programs, target, mapping) in deletion_rows() {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "programmed").await;
        let created = stored(&router, case, &multiplex_id, &programs).await;
        if let Some(mapping) = mapping {
            map_packet_identifiers(&router, case, &multiplex_id, mapping).await;
        }

        // What a reader sees for every program just before the delete, so the
        // response and the survivors are judged against the stored state rather
        // than against the requests that made it.
        let mut before: Vec<(String, Value)> = Vec::new();
        for (program_name, _) in &created {
            let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplexProgram({program_name}): expected 200, \
                 got {status}: {described}"
            );
            before.push((program_name.clone(), described));
        }

        let (status, deleted) = delete_program(&router, &multiplex_id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMultiplexProgram: expected 200, got {status}: {deleted}"
        );
        let expected = before
            .iter()
            .find(|(program_name, _)| program_name == target)
            .map(|(_, described)| described.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            deleted, expected,
            "case {case:?}: expected the delete to answer with the program it removed \
             ({expected}), got {deleted}"
        );

        // The program is gone: the reader that reported it a moment ago answers
        // the modelled not-found, naming the program that is no longer there.
        let (status, described) = describe_program(&router, &multiplex_id, target).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeMultiplexProgram after the delete: expected 404, \
             got {status}: {described}"
        );
        assert_eq!(
            error_code(status, &described),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {described}"
        );
        let message = error_message(&described);
        assert_eq!(
            message,
            format!("Multiplex program {multiplex_id}/{target} not found"),
            "case {case:?}: expected the refusal to name the deleted program, got {message:?}"
        );

        // A second delete of the same program is the same not-found: a delete
        // that removed nothing never answers as though it had.
        let (status, again) = delete_program(&router, &multiplex_id, target).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the second DeleteMultiplexProgram: expected 404, got {status}: {again}"
        );
        assert_eq!(
            error_code(status, &again),
            "NotFoundException",
            "case {case:?}: expected the second delete to answer the modelled \
             NotFoundException, got {again}"
        );

        // The survivors are untouched, and the multiplex counts and lists
        // exactly them.
        let survivors: Vec<&(String, Value)> = before
            .iter()
            .filter(|(program_name, _)| program_name != target)
            .collect();
        for (program_name, was) in &survivors {
            let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplexProgram({program_name}) after the delete: \
                 expected 200, got {status}: {described}"
            );
            assert_eq!(
                &described, was,
                "case {case:?}: expected the delete to leave {program_name} alone, got {described}"
            );
        }

        let mut expected_names: Vec<String> = survivors
            .iter()
            .map(|(program_name, _)| program_name.clone())
            .collect();
        expected_names.sort();
        let names = listed_names(&router, case, &multiplex_id).await;
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected the listing to report only the surviving programs \
             ({expected_names:?}), got {names:?}"
        );

        let (status, described) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["programCount"],
            json!(survivors.len()),
            "case {case:?}: expected the multiplex to count {} programs, got {:?}",
            survivors.len(),
            described["programCount"]
        );
    }
}

/// Rule: a deleted program holds neither its `ProgramName` nor its
/// `ProgramNumber` any more, so a create the multiplex refused while the program
/// was there — one asking for that name, or that number — is accepted once it has
/// been deleted.
///
/// The rows differ along WHICH of the two uniqueness rules the second create
/// collides with: the name, the number, or both at once.
#[tokio::test]
async fn deleting_a_program_frees_its_name_and_its_number() {
    // (case, the name and the program number the second create asks for)
    let rows: [(&str, &str, i64); 3] = [
        ("the same name and the same number", "news", 7),
        ("the same name, a free number", "news", 8),
        ("a free name, the same number", "late-news", 7),
    ];

    for (case, name, program_number) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "recycled").await;
        stored(
            &router,
            case,
            &multiplex_id,
            &[("news", program_settings(7))],
        )
        .await;

        // The control: while the first program is there, the second create
        // collides with it, so the acceptance below is the delete's doing.
        let (status, refusal) = create_program(
            &router,
            &multiplex_id,
            name,
            &format!("token-collide-{name}-{program_number}"),
            program_settings(program_number),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the colliding create to be refused while the program \
             is there, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refusal}"
        );

        let (status, deleted) = delete_program(&router, &multiplex_id, "news").await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMultiplexProgram: expected 200, got {status}: {deleted}"
        );

        let (status, recreated) = create_program(
            &router,
            &multiplex_id,
            name,
            &format!("token-again-{name}-{program_number}"),
            program_settings(program_number),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the same create to be accepted once the program was \
             deleted, got {status}: {recreated}"
        );
        assert_eq!(
            recreated["multiplexProgramSettings"],
            program_settings(program_number),
            "case {case:?}: expected the settings the second create supplied, got {:?}",
            recreated["multiplexProgramSettings"]
        );

        // And the multiplex holds exactly the new program, never both.
        let names = listed_names(&router, case, &multiplex_id).await;
        assert_eq!(
            names,
            vec![name.to_string()],
            "case {case:?}: expected the multiplex to hold only the recreated program, \
             got {names:?}"
        );
    }
}

/// Rule: a `DeleteMultiplexProgram` whose `MultiplexId` names no stored
/// multiplex, or whose `ProgramName` names no program of the multiplex it does
/// name, is answered with the modelled `NotFoundException` naming the resource
/// that is missing, and removes nothing.
///
/// The rows differ along which of the two identifiers is the unknown one, and
/// along the FORM of the unknown value (a numeric id no multiplex was minted, a
/// non-numeric one, a program name another multiplex holds, a name no multiplex
/// holds).
#[tokio::test]
async fn a_delete_naming_no_stored_resource_is_refused() {
    // (case, whether the multiplex under test is the one named, the id or name
    // the request carries, the resource type the refusal must name)
    let rows: [(&str, bool, &str, &str); 5] = [
        (
            "a multiplex id no multiplex was minted",
            false,
            "999999",
            "Multiplex",
        ),
        (
            "a multiplex id that is not even numeric",
            false,
            "not-a-multiplex",
            "Multiplex",
        ),
        (
            "a program name that multiplex never held",
            true,
            "never-created",
            "Multiplex program",
        ),
        (
            "a program name only the other multiplex holds",
            true,
            "elsewhere",
            "Multiplex program",
        ),
        (
            "a program name differing from the stored one only in case",
            true,
            "NEWS",
            "Multiplex program",
        ),
    ];

    for (case, names_the_multiplex, identifier, resource_type) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "holder").await;
        stored(
            &router,
            case,
            &multiplex_id,
            &[("news", program_settings(1))],
        )
        .await;
        // A second multiplex holds a program of its own, so a request that asks
        // the first multiplex for THAT name is refused rather than served from
        // the wrong multiplex.
        let other_id = multiplex(&router, case, "neighbour").await;
        stored(
            &router,
            case,
            &other_id,
            &[("elsewhere", program_settings(1))],
        )
        .await;

        let (target_multiplex, program_name) = if names_the_multiplex {
            (multiplex_id.as_str(), identifier)
        } else {
            (identifier, "news")
        };
        // The identifier the refusal names: a multiplex by its id, a program by
        // the multiplex it would live in together with its own name, since a
        // program has no identifier of its own.
        let named = if resource_type == "Multiplex program" {
            format!("{target_multiplex}/{program_name}")
        } else {
            target_multiplex.to_string()
        };
        let (status, refusal) = delete_program(&router, target_multiplex, program_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {refusal}"
        );
        let message = error_message(&refusal);
        assert_eq!(
            message,
            format!("{resource_type} {named} not found"),
            "case {case:?}: expected the refusal to name the missing resource, got {message:?}"
        );

        // Nothing was removed: both multiplexes still hold the program each was
        // created with.
        for (holder, expected) in [(&multiplex_id, "news"), (&other_id, "elsewhere")] {
            let names = listed_names(&router, case, holder).await;
            assert_eq!(
                names,
                vec![expected.to_string()],
                "case {case:?}: expected the refused delete to remove nothing, got {names:?}"
            );
        }
    }
}

/// Rule: once a delete has been accepted for a multiplex, nothing it holds is a
/// caller's to change, so deleting one of its programs is answered with the
/// modelled `ConflictException` naming the multiplex and its state — in the
/// `DELETING` state the delete opens and in the terminal `DELETED` the clock owes
/// it alike — and the program stays readable.
#[tokio::test]
async fn deleting_a_program_of_a_multiplex_whose_delete_was_accepted_conflicts() {
    let case = "accepted multiplex delete";
    let router = medialive();
    let multiplex_id = multiplex(&router, case, "doomed").await;
    stored(
        &router,
        case,
        &multiplex_id,
        &[("news", program_settings(1))],
    )
    .await;
    // A multiplex is deletable only once it has stopped, which the clock owes
    // the create.
    wait_for_the_clock();

    let (status, deleting) = call(
        &router,
        "DELETE",
        &format!("{MULTIPLEXES}/{multiplex_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
    );

    for phase in ["DELETING", "DELETED"] {
        if phase == "DELETED" {
            wait_for_the_clock();
        }
        let (status, described) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["state"],
            json!(phase),
            "case {case:?}: the test did not reach {phase}, it reached {:?}",
            described["state"]
        );

        let (status, refusal) = delete_program(&router, &multiplex_id, "news").await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 while the multiplex is {phase}, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException while the multiplex is \
             {phase}, got {refusal}"
        );
        let message = error_message(&refusal);
        assert!(
            message.contains(&multiplex_id) && message.contains(phase),
            "case {case:?}: expected the refusal to name the multiplex and its state, \
             got {message:?}"
        );

        // The refusal wrote nothing: the program is still there to read, and the
        // multiplex — re-read AFTER the refusal — still counts it.
        let (status, program) = describe_program(&router, &multiplex_id, "news").await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram while {phase}: expected 200, \
             got {status}: {program}"
        );
        let (status, after) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the refusal: expected 200, \
             got {status}: {after}"
        );
        assert_eq!(
            after, described,
            "case {case:?}: expected the refused delete to leave the {phase} multiplex \
             exactly as it was, got {after}"
        );
        assert_eq!(
            after["programCount"],
            json!(1),
            "case {case:?}: expected the {phase} multiplex to still count its program, got {:?}",
            after["programCount"]
        );
    }
}
