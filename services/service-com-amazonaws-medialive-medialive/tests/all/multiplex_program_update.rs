// SPDX-License-Identifier: CC0-1.0

//! `UpdateMultiplexProgram`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The rule: `UpdateMultiplexProgram` REPLACES the `MultiplexProgramSettings`
//! of the program its `MultiplexId` and `ProgramName` name with the ones the
//! request carries (and leaves them as they are when it carries none), answers
//! with the stored program, and refuses — storing nothing — settings outside
//! what the model publishes for them, a multiplex or program that does not
//! exist, and a `ProgramNumber` another program of the same multiplex holds.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_SERVICE_DESCRIPTOR_NAME_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_lifecycle::{describe_multiplex, MULTIPLEXES};
use crate::multiplex_programs::{
    create_program, describe_program, multiplex, program_settings, with_member,
    PREFERRED_CHANNEL_PIPELINES, PROGRAM_NUMBER, RANGED_PROGRAM_SETTINGS, STATMUX_PRIORITY,
    VIDEO_BITRATE,
};
use crate::wire::{call, error_code, error_message, medialive};

/// `UpdateMultiplexProgram`: its status and, on success, the program it answers
/// with (`PUT /prod/multiplexes/{MultiplexId}/programs/{ProgramName}`).
///
/// `settings` is `None` for the request that carries no
/// `MultiplexProgramSettings` at all, which the model makes optional.
async fn update_program(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    program_name: &str,
    settings: Option<Value>,
) -> (u16, Value) {
    let body = match settings {
        Some(settings) => json!({ "multiplexProgramSettings": settings }),
        None => json!({}),
    };
    let (status, response) = call(
        router,
        "PUT",
        &format!("{MULTIPLEXES}/{multiplex_id}/programs/{program_name}"),
        Some(body),
    )
    .await;
    if status == 200 {
        (status, response["multiplexProgram"].clone())
    } else {
        (status, response)
    }
}

/// A multiplex holding one program named `program_name` with `settings`, with
/// both creates asserted to have succeeded, as the multiplex's minted id.
///
/// The multiplex is named after the program, so two calls in one test create two
/// DIFFERENT multiplexes: `multiplex` derives the create's `@idempotencyToken`
/// from the name it is given, and a repeated token answers with the multiplex
/// the first create made.
async fn multiplex_with_program(
    router: &MultiServiceRouter,
    case: &str,
    program_name: &str,
    settings: Value,
) -> String {
    let multiplex_id = multiplex(router, case, program_name).await;
    let (status, program) = create_program(
        router,
        &multiplex_id,
        program_name,
        &format!("token-{program_name}"),
        settings,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateMultiplexProgram: expected 201, got {status}: {program}"
    );
    multiplex_id
}

/// The settings every replacement table row starts from: every optional member
/// a `MultiplexProgramSettings` reaches is named, so a row that drops one is
/// asserted to have dropped it rather than to have left it alone.
fn full_settings() -> Value {
    json!({
        "programNumber": 7,
        "preferredChannelPipeline": "PIPELINE_0",
        "serviceDescriptor": {
            "providerName": "the-original-provider",
            "serviceName": "the-original-service",
        },
        "videoSettings": {
            "statmuxSettings": {
                "maximumBitrate": VIDEO_BITRATE.1,
                "minimumBitrate": VIDEO_BITRATE.0,
                "priority": STATMUX_PRIORITY.0,
            }
        },
    })
}

/// One row of the replacement table: the case and the `MultiplexProgramSettings`
/// the update carries (`None` for an update that carries none).
type UpdateRow = (&'static str, Option<Value>);

/// Rule: `UpdateMultiplexProgram` replaces the stored program's
/// `MultiplexProgramSettings` with the ones the request carries — whole, so a
/// member the new settings omit is omitted on the program — and answers with
/// that stored program, which `DescribeMultiplexProgram` then reports member
/// for member; an update carrying no settings at all asks for no change.
///
/// The rows differ along which members the new settings name, along the enum
/// value they pick, along each published bound of the ranged members, and along
/// whether settings are carried at all — which is what the rule generalises
/// over.
#[tokio::test]
async fn an_update_replaces_the_stored_settings_and_is_read_back() {
    let mut rows: Vec<UpdateRow> = vec![
        (
            "down to the required member only",
            Some(program_settings(7)),
        ),
        (
            "the program number at its lower bound",
            Some(program_settings(PROGRAM_NUMBER.0)),
        ),
        (
            "the program number at its upper bound",
            Some(program_settings(PROGRAM_NUMBER.1)),
        ),
        (
            "statmux settings swapped for a constant bitrate",
            Some(with_member(
                program_settings(7),
                "videoSettings.constantBitrate",
                VIDEO_BITRATE.0,
            )),
        ),
        (
            "a constant bitrate at its upper bound",
            Some(with_member(
                program_settings(7),
                "videoSettings.constantBitrate",
                VIDEO_BITRATE.1,
            )),
        ),
        (
            "new statmux settings at the other bounds",
            Some(json!({
                "programNumber": 7,
                "videoSettings": {
                    "statmuxSettings": {
                        "maximumBitrate": VIDEO_BITRATE.0,
                        "minimumBitrate": VIDEO_BITRATE.0,
                        "priority": STATMUX_PRIORITY.1,
                    }
                },
            })),
        ),
        (
            "another service descriptor",
            Some(json!({
                "programNumber": 7,
                "serviceDescriptor": {
                    "providerName": "a-new-provider",
                    "serviceName": "a-new-service",
                },
            })),
        ),
        (
            "a service descriptor at the published name length",
            Some(json!({
                "programNumber": 7,
                "serviceDescriptor": {
                    "providerName": "p".repeat(MAX_SERVICE_DESCRIPTOR_NAME_LEN),
                    "serviceName": "s".repeat(MAX_SERVICE_DESCRIPTOR_NAME_LEN),
                },
            })),
        ),
        ("no settings at all", None),
    ];
    // One row per value of the one enum these settings reach, so the table
    // spans the catalog rather than the value the issue happened to carry.
    for pipeline in PREFERRED_CHANNEL_PIPELINES {
        rows.push((
            pipeline,
            Some(json!({
                "programNumber": 7,
                "preferredChannelPipeline": pipeline,
            })),
        ));
    }

    for (case, settings) in rows {
        let router = medialive();
        let multiplex_id = multiplex_with_program(&router, case, "news", full_settings()).await;
        // A second program of the same multiplex, so an update is asserted to
        // have touched only the program it named.
        let (status, untouched) = create_program(
            &router,
            &multiplex_id,
            "sports",
            "token-sports",
            program_settings(8),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the second CreateMultiplexProgram: expected 201, got {status}: {untouched}"
        );

        // An update that carries no settings asks for no change, so what the
        // program is expected to carry afterwards is what it carried before.
        let expected = settings.clone().unwrap_or_else(full_settings);
        let (status, updated) =
            update_program(&router, &multiplex_id, "news", settings.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateMultiplexProgram: expected 200, got {status}: {updated}"
        );
        assert_eq!(
            updated["programName"],
            json!("news"),
            "case {case:?}: expected the updated program's own name, got {:?}",
            updated["programName"]
        );
        assert_eq!(
            updated["multiplexProgramSettings"], expected,
            "case {case:?}: expected the settings the update supplied, got {:?}",
            updated["multiplexProgramSettings"]
        );

        // The round trip: the reader answers with the stored program, so the
        // update wrote to the one copy of the state rather than to its own
        // response.
        let (status, described) = describe_program(&router, &multiplex_id, "news").await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described, updated,
            "case {case:?}: expected DescribeMultiplexProgram to report the updated program, got {described}"
        );

        // An update is not a create: the multiplex still holds its two
        // programs, and the one the update did not name is unchanged.
        let (_, multiplex) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            multiplex["programCount"],
            json!(2),
            "case {case:?}: expected the update to store no new program, got {:?}",
            multiplex["programCount"]
        );
        let (_, other) = describe_program(&router, &multiplex_id, "sports").await;
        assert_eq!(
            other, untouched,
            "case {case:?}: expected the other program to survive the update, got {other}"
        );
    }
}

/// Rule: settings `CreateMultiplexProgram` refuses are settings
/// `UpdateMultiplexProgram` refuses — a value outside the range the model
/// publishes for it, a `MultiplexProgramServiceDescriptor` name past its
/// published `@length` max, and `MultiplexVideoSettings` defining both
/// `ConstantBitrate` and `StatmuxSettings` are each a client error that names
/// the member — and a refused update leaves the stored program exactly as it
/// was.
///
/// The rows walk EVERY ranged member those settings reach to each of its
/// published bounds and one step past each (the ranges in
/// [`RANGED_PROGRAM_SETTINGS`]), plus both descriptor names at and one past
/// their published length, plus the over-specified `VideoSettings`.
#[tokio::test]
async fn an_update_refuses_the_settings_a_create_refuses() {
    // (case, the settings the update carries, the expected status, the member
    //  a refusal is expected to name)
    let mut rows: Vec<(String, Value, u16, &'static str)> = Vec::new();
    for (member, (min, max)) in RANGED_PROGRAM_SETTINGS {
        for (variant, value, expected) in [
            ("at the minimum", min, 200u16),
            ("below the minimum", min - 1, 400),
            ("at the maximum", max, 200),
            ("past the maximum", max + 1, 400),
        ] {
            // Only the member under test leaves its range: the required
            // `ProgramNumber` otherwise stays well inside its own, and keeps
            // the number the program already holds.
            rows.push((
                format!("{member} {variant} ({value})"),
                with_member(program_settings(7), member, value),
                expected,
                member,
            ));
        }
    }
    for member in ["providerName", "serviceName"] {
        for (variant, length, expected) in [
            ("at the published length", MAX_SERVICE_DESCRIPTOR_NAME_LEN, 200u16),
            (
                "one past the published length",
                MAX_SERVICE_DESCRIPTOR_NAME_LEN + 1,
                400,
            ),
        ] {
            let mut settings = json!({
                "programNumber": 7,
                "serviceDescriptor": { "providerName": "p", "serviceName": "s" },
            });
            settings["serviceDescriptor"][member] = json!("x".repeat(length));
            rows.push((
                format!("serviceDescriptor.{member} {variant} ({length} characters)"),
                settings,
                expected,
                member,
            ));
        }
    }
    for (variant, video_settings, expected) in [
        (
            "a constant bitrate alone",
            json!({ "constantBitrate": VIDEO_BITRATE.0 }),
            200u16,
        ),
        (
            "statmux settings alone",
            json!({ "statmuxSettings": { "priority": 0 } }),
            200,
        ),
        (
            "both a constant bitrate and statmux settings",
            json!({
                "constantBitrate": VIDEO_BITRATE.0,
                "statmuxSettings": { "priority": 0 },
            }),
            400,
        ),
    ] {
        let mut settings = program_settings(7);
        settings["videoSettings"] = video_settings;
        rows.push((
            format!("videoSettings: {variant}"),
            settings,
            expected,
            "constantBitrate",
        ));
    }

    for (case, settings, expected, member) in rows {
        let router = medialive();
        let before = full_settings();
        let multiplex_id = multiplex_with_program(&router, &case, "news", before.clone()).await;

        let (status, body) =
            update_program(&router, &multiplex_id, "news", Some(settings.clone())).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );

        if expected == 200 {
            assert_eq!(
                body["multiplexProgramSettings"], settings,
                "case {case:?}: expected the settings the update supplied, got {:?}",
                body["multiplexProgramSettings"]
            );
            continue;
        }

        let code = error_code(status, &body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {case:?}: expected the update to be refused as a client error, got {code} ({body})"
        );
        let message = error_message(&body);
        assert!(
            message.contains(member),
            "case {case:?}: expected the refusal to name {member:?}, got {body}"
        );
        // And it wrote nothing: the stored program still carries the settings
        // it was created with.
        let (status, described) = describe_program(&router, &multiplex_id, "news").await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["multiplexProgramSettings"], before,
            "case {case:?}: expected the refused update to write nothing, got {described}"
        );
    }
}

/// Rule: `UpdateMultiplexProgram` updates a program that already exists, so a
/// `MultiplexId` naming no stored multiplex and a `ProgramName` the named
/// multiplex does not hold are each answered with the `NotFoundException` the
/// operation models (`smithy_query(query="operation",
/// operation="UpdateMultiplexProgram")`), naming what was asked for, and write
/// nothing.
#[tokio::test]
async fn updating_a_multiplex_or_program_that_does_not_exist_is_refused() {
    let router = medialive();
    let multiplex_id = multiplex_with_program(&router, "live", "news", full_settings()).await;
    // A second multiplex holds a program of its own, so an update that asks the
    // first multiplex for THAT name is refused rather than applied to the wrong
    // multiplex's program.
    let other_multiplex =
        multiplex_with_program(&router, "other", "other", program_settings(1)).await;

    // (case, the multiplex id the request names — empty means the one that
    //  exists —, the program name it names, the resource the refusal names and
    //  the identifier it names)
    let rows: [(&str, &str, &str, &str, &str); 4] = [
        (
            "a multiplex id never minted",
            "9999999",
            "news",
            "Multiplex",
            "9999999",
        ),
        (
            "a multiplex id that is not a number at all",
            "no-such-multiplex",
            "news",
            "Multiplex",
            "no-such-multiplex",
        ),
        (
            "a program name no program has",
            "",
            "no-such-program",
            "MultiplexProgram",
            "no-such-program",
        ),
        (
            "a program name that is another multiplex's",
            "",
            "other",
            "MultiplexProgram",
            "other",
        ),
    ];

    for (case, named_multiplex, program_name, resource_type, identifier) in rows {
        let named_multiplex = if named_multiplex.is_empty() {
            multiplex_id.as_str()
        } else {
            named_multiplex
        };

        let (status, body) = update_program(
            &router,
            named_multiplex,
            program_name,
            Some(program_settings(42)),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: UpdateMultiplexProgram: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("{resource_type} {identifier} not found"),
            "case {case:?}: expected the refusal to name what was asked for, got {body}"
        );
        // The refused update created nothing: the name it asked for still names
        // no program of that multiplex.
        if resource_type == "MultiplexProgram" {
            let (status, described) = describe_program(&router, named_multiplex, program_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused update to store nothing, got {status}: {described}"
            );
        }
    }

    // And none of those refusals touched the programs that do exist.
    for (multiplex_id, program_name, expected) in [
        (multiplex_id.as_str(), "news", full_settings()),
        (other_multiplex.as_str(), "other", program_settings(1)),
    ] {
        let (status, described) = describe_program(&router, multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {program_name:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["multiplexProgramSettings"], expected,
            "case {program_name:?}: expected the stored program to survive, got {described}"
        );
    }
}

/// Rule: a `ProgramNumber` is "Unique program number" within its multiplex, so
/// an update may keep the number its own program already holds or take one no
/// program of that multiplex holds, but taking ANOTHER program's number of the
/// same multiplex is a conflict that writes nothing; the same number in a
/// different multiplex belongs to a different transport stream and is accepted.
#[tokio::test]
async fn an_update_cannot_take_another_programs_number_in_the_same_multiplex() {
    /// The number the OTHER multiplex's program holds, so a row can ask for a
    /// number that is taken, but not in the multiplex being updated.
    const HELD_ELSEWHERE: i64 = 9;

    // (case, the new program number, the expected status)
    let rows: [(&str, i64, u16); 5] = [
        ("its own number again", 1, 200),
        ("a number no program holds", 3, 200),
        ("the other program's number", 2, 409),
        (
            "a number only the other multiplex's program holds",
            HELD_ELSEWHERE,
            200,
        ),
        ("the upper bound, which nothing holds", PROGRAM_NUMBER.1, 200),
    ];

    for (case, program_number, expected) in rows {
        let router = medialive();
        // One multiplex holding two programs, numbered 1 and 2 ...
        let one = multiplex_with_program(&router, case, "first", program_settings(1)).await;
        let (status, second) =
            create_program(&router, &one, "second", "token-second", program_settings(2)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the second CreateMultiplexProgram: expected 201, got {status}: {second}"
        );
        // ... and another multiplex whose own program holds a third number, so
        // a row can ask for a number that is taken only elsewhere.
        let two = multiplex_with_program(
            &router,
            case,
            "elsewhere",
            program_settings(HELD_ELSEWHERE),
        )
        .await;

        let settings = program_settings(program_number);
        let (status, body) = update_program(&router, &one, "first", Some(settings.clone())).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );

        if expected == 409 {
            assert_eq!(
                error_code(status, &body),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&program_number.to_string()) && message.contains("second"),
                "case {case:?}: expected the refusal to name the number and the program holding it, got {body}"
            );
            // The refused update wrote nothing: both programs keep the numbers
            // their creates gave them.
            for (name, number) in [("first", 1), ("second", 2)] {
                let (_, described) = describe_program(&router, &one, name).await;
                assert_eq!(
                    described["multiplexProgramSettings"]["programNumber"],
                    json!(number),
                    "case {case:?}/{name}: expected the refused update to write nothing, got {described}"
                );
            }
            continue;
        }

        assert_eq!(
            body["multiplexProgramSettings"], settings,
            "case {case:?}: expected the settings the update supplied, got {:?}",
            body["multiplexProgramSettings"]
        );
        let (_, described) = describe_program(&router, &one, "first").await;
        assert_eq!(
            described["multiplexProgramSettings"]["programNumber"],
            json!(program_number),
            "case {case:?}: expected the stored program to carry its new number, got {described}"
        );
        // The programs the update did not name keep their own numbers, in this
        // multiplex and in the other one.
        for (multiplex_id, name, number) in [
            (&one, "second", 2),
            (&two, "elsewhere", HELD_ELSEWHERE),
        ] {
            let (_, other) = describe_program(&router, multiplex_id, name).await;
            assert_eq!(
                other["multiplexProgramSettings"]["programNumber"],
                json!(number),
                "case {case:?}/{name}: expected the other program to keep its number, got {other}"
            );
        }
    }
}
