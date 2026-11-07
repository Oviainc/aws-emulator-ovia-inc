// SPDX-License-Identifier: CC0-1.0

//! The MediaLive multiplex PROGRAM lifecycle, end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_SERVICE_DESCRIPTOR_NAME_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{create_body, created, describe_multiplex, settings, MULTIPLEXES};
use crate::wire::{call, error_code, error_message, medialive};

/// Every value of `PreferredChannelPipeline`, the one enum a
/// `MultiplexProgramSettings` reaches (`smithy_query(query="shape",
/// shape="PreferredChannelPipeline")`: CURRENTLY_ACTIVE | PIPELINE_0 |
/// PIPELINE_1).
pub(crate) const PREFERRED_CHANNEL_PIPELINES: [&str; 3] = ["CURRENTLY_ACTIVE", "PIPELINE_0", "PIPELINE_1"];

/// The published bounds of `MultiplexProgramSettings$ProgramNumber`, from its
/// target shape `__integerMin0Max65535` (`smithy_query(query="shape",
/// shape="MultiplexProgramSettings")`).
pub(crate) const PROGRAM_NUMBER: (i64, i64) = (0, 65_535);

/// The published bounds of `MultiplexVideoSettings$ConstantBitrate` and of both
/// `MultiplexStatmuxVideoSettings` bitrates, from the shape all three target,
/// `__integerMin100000Max100000000`.
pub(crate) const VIDEO_BITRATE: (i64, i64) = (100_000, 100_000_000);

/// The published bounds of `MultiplexStatmuxVideoSettings$Priority`, from its
/// target shape `__integerMinNegative5Max5`.
pub(crate) const STATMUX_PRIORITY: (i64, i64) = (-5, 5);

/// Every ranged member a `MultiplexProgramSettings` reaches, by the wire path
/// the refusal names it at, with the inclusive bounds the model publishes for
/// it — the whole class the range rule covers.
pub(crate) const RANGED_PROGRAM_SETTINGS: [(&str, (i64, i64)); 5] = [
    ("programNumber", PROGRAM_NUMBER),
    ("videoSettings.constantBitrate", VIDEO_BITRATE),
    ("videoSettings.statmuxSettings.maximumBitrate", VIDEO_BITRATE),
    ("videoSettings.statmuxSettings.minimumBitrate", VIDEO_BITRATE),
    ("videoSettings.statmuxSettings.priority", STATMUX_PRIORITY),
];

/// A `MultiplexProgramSettings` carrying only the member the structure requires
/// (`ProgramNumber`): the row's own members are what each test adds.
pub(crate) fn program_settings(program_number: i64) -> Value {
    json!({ "programNumber": program_number })
}

/// `settings` with the member at the dot-separated wire `path` set to `value`,
/// creating the nested objects that path needs — so a table row names a member
/// exactly as the refusal does.
pub(crate) fn with_member(mut settings: Value, path: &str, value: i64) -> Value {
    let segments: Vec<&str> = path.split('.').collect();
    let (last, parents) = segments.split_last().expect("a non-empty member path");
    let mut cursor = &mut settings;
    for segment in parents {
        if !cursor[*segment].is_object() {
            cursor[*segment] = json!({});
        }
        cursor = &mut cursor[*segment];
    }
    cursor[*last] = json!(value);
    settings
}

/// A multiplex to create programs in, with the create asserted to have
/// succeeded, as its minted id.
pub(crate) async fn multiplex(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let multiplex = created(
        router,
        case,
        create_body(name, &format!("token-{name}"), settings(2_000_000, 1)),
    )
    .await;
    multiplex["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `CreateMultiplexProgram`: the status and, on success, the program it answers
/// with (`POST /prod/multiplexes/{MultiplexId}/programs`).
pub(crate) async fn create_program(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    program_name: &str,
    request_id: &str,
    program_settings: Value,
) -> (u16, Value) {
    let (status, response) = call(
        router,
        "POST",
        &format!("{MULTIPLEXES}/{multiplex_id}/programs"),
        Some(json!({
            "programName": program_name,
            "requestId": request_id,
            "multiplexProgramSettings": program_settings,
        })),
    )
    .await;
    if status == 201 {
        (status, response["multiplexProgram"].clone())
    } else {
        (status, response)
    }
}

/// `DescribeMultiplexProgram`: its status and body. The description is the
/// response itself, not a wrapper member.
pub(crate) async fn describe_program(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    program_name: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{MULTIPLEXES}/{multiplex_id}/programs/{program_name}"),
        None,
    )
    .await
}

/// One row of the create-and-read-back table: the case, the program's name and
/// the `MultiplexProgramSettings` the request carries.
type ProgramRow = (&'static str, &'static str, Value);

/// Rule: `CreateMultiplexProgram` stores a program under the `ProgramName` it
/// was given in the multiplex its `MultiplexId` names, keeping the
/// `MultiplexProgramSettings` verbatim, and answers with that stored program;
/// `DescribeMultiplexProgram` reads the same program back, and the multiplex's
/// own derived `ProgramCount` counts the programs it now holds.
///
/// The rows differ along which optional settings members the request names,
/// along the enum value it picks, and along each published bound of the ranged
/// members — which is what the rule generalises over.
#[tokio::test]
async fn a_created_program_is_read_back_by_describe() {
    let mut rows: Vec<ProgramRow> = vec![
        (
            "the required member only",
            "news",
            program_settings(PROGRAM_NUMBER.0),
        ),
        (
            "program number at its upper bound",
            "sports",
            program_settings(PROGRAM_NUMBER.1),
        ),
        (
            "a constant bitrate at its lower bound",
            "cbr-low",
            with_member(
                program_settings(10),
                "videoSettings.constantBitrate",
                VIDEO_BITRATE.0,
            ),
        ),
        (
            "a constant bitrate at its upper bound",
            "cbr-high",
            with_member(
                program_settings(11),
                "videoSettings.constantBitrate",
                VIDEO_BITRATE.1,
            ),
        ),
        (
            "every statmux member, at its bounds",
            "statmux",
            json!({
                "programNumber": 12,
                "videoSettings": {
                    "statmuxSettings": {
                        "maximumBitrate": VIDEO_BITRATE.1,
                        "minimumBitrate": VIDEO_BITRATE.0,
                        "priority": STATMUX_PRIORITY.1,
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
        (
            "a service descriptor at the published name length",
            "long-descriptor",
            json!({
                "programNumber": 14,
                "serviceDescriptor": {
                    "providerName": "p".repeat(MAX_SERVICE_DESCRIPTOR_NAME_LEN),
                    "serviceName": "s".repeat(MAX_SERVICE_DESCRIPTOR_NAME_LEN),
                },
            }),
        ),
    ];
    // One row per value of the one enum these settings reach, so the table
    // spans the catalog rather than a value the issue happened to carry.
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

    for (case, program_name, settings) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "programmed").await;

        let (status, program) = create_program(
            &router,
            &multiplex_id,
            program_name,
            &format!("token-{program_name}"),
            settings.clone(),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram: expected 201, got {status}: {program}"
        );
        assert_eq!(
            program["programName"],
            json!(program_name),
            "case {case:?}: expected the requested program name, got {:?}",
            program["programName"]
        );
        assert_eq!(
            program["multiplexProgramSettings"], settings,
            "case {case:?}: expected the settings the request supplied, got {:?}",
            program["multiplexProgramSettings"]
        );
        // No channel sends its output to this program yet, so the association
        // the response derives from the store is absent rather than invented.
        assert_eq!(
            program["channelId"],
            Value::Null,
            "case {case:?}: expected no channel associated with a fresh program, got {:?}",
            program["channelId"]
        );

        // The round trip: the reader answers with the stored program, member
        // for member, not with anything derived from its own request.
        let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described, program,
            "case {case:?}: expected DescribeMultiplexProgram to report the created program, got {described}"
        );

        // And the multiplex counts it: `ProgramCount` is "The number of
        // programs in the multiplex", derived from what the store holds.
        let (status, multiplex) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {multiplex}"
        );
        assert_eq!(
            multiplex["programCount"],
            json!(1),
            "case {case:?}: expected the multiplex to count its one program, got {:?}",
            multiplex["programCount"]
        );
    }
}

/// Rule: a `MultiplexProgramSettings` member outside the range the model
/// publishes for it is refused as a client error naming the member and the
/// value, and stores nothing; a value on either published bound is accepted.
///
/// The rows walk EVERY ranged member those settings reach to each of its
/// published bounds and one step past each — the ranges in
/// [`RANGED_PROGRAM_SETTINGS`].
#[tokio::test]
async fn program_settings_outside_their_published_range_are_refused() {
    for (member, (min, max)) in RANGED_PROGRAM_SETTINGS {
        for (variant, value, expected) in [
            ("at the minimum", min, 201u16),
            ("below the minimum", min - 1, 400),
            ("at the maximum", max, 201),
            ("past the maximum", max + 1, 400),
        ] {
            let case = format!("{member} {variant} ({value})");
            let router = medialive();
            let multiplex_id = multiplex(&router, &case, "ranged").await;
            // Only the member under test leaves its range: the required
            // `ProgramNumber` otherwise stays well inside its own.
            let settings = with_member(program_settings(7), member, value);

            let (status, body) =
                create_program(&router, &multiplex_id, "ranged", "token-ranged", settings.clone())
                    .await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected {expected}, got {status}: {body}"
            );

            if expected == 201 {
                assert_eq!(
                    body["multiplexProgramSettings"], settings,
                    "case {case:?}: expected the settings the request supplied, got {:?}",
                    body["multiplexProgramSettings"]
                );
                continue;
            }

            let code = error_code(status, &body);
            assert!(
                code == "BadRequestException" || code == "ValidationException",
                "case {case:?}: expected the request to be refused as a client error, got {code} ({body})"
            );
            let message = error_message(&body);
            assert!(
                message.contains(member) && message.contains(&value.to_string()),
                "case {case:?}: expected the refusal to name the member and the value, got {body}"
            );
            // And it stored nothing: the program the refused create named does
            // not exist, and the multiplex still holds no program.
            let (status, described) = describe_program(&router, &multiplex_id, "ranged").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to store nothing, got {status}: {described}"
            );
            let (_, multiplex) = describe_multiplex(&router, &multiplex_id).await;
            assert_eq!(
                multiplex["programCount"],
                json!(0),
                "case {case:?}: expected the multiplex to hold no program, got {:?}",
                multiplex["programCount"]
            );
        }
    }
}

/// Rule: a `MultiplexProgramServiceDescriptor` name longer than the `@length`
/// max the model publishes for it (`__stringMax256`) is refused as a client
/// error and stores nothing; a name on that bound is accepted (the
/// create-and-read-back table above carries that row).
#[tokio::test]
async fn a_service_descriptor_name_past_its_published_length_is_refused() {
    for member in ["providerName", "serviceName"] {
        for (variant, length, expected) in [
            ("at the published length", MAX_SERVICE_DESCRIPTOR_NAME_LEN, 201u16),
            (
                "one past the published length",
                MAX_SERVICE_DESCRIPTOR_NAME_LEN + 1,
                400,
            ),
        ] {
            let case = format!("{member} {variant} ({length} characters)");
            let router = medialive();
            let multiplex_id = multiplex(&router, &case, "descriptive").await;
            let mut settings = json!({
                "programNumber": 3,
                "serviceDescriptor": { "providerName": "p", "serviceName": "s" },
            });
            settings["serviceDescriptor"][member] = json!("x".repeat(length));

            let (status, body) = create_program(
                &router,
                &multiplex_id,
                "descriptive",
                "token-descriptive",
                settings.clone(),
            )
            .await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected {expected}, got {status}: {body}"
            );
            if expected == 201 {
                assert_eq!(
                    body["multiplexProgramSettings"], settings,
                    "case {case:?}: expected the settings the request supplied, got {:?}",
                    body["multiplexProgramSettings"]
                );
                continue;
            }

            let code = error_code(status, &body);
            assert!(
                code == "BadRequestException" || code == "ValidationException",
                "case {case:?}: expected a client error, got {code} ({body})"
            );
            let (status, described) =
                describe_program(&router, &multiplex_id, "descriptive").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to store nothing, got {status}: {described}"
            );
        }
    }
}

/// Rule: `MultiplexVideoSettings` carries exactly one of `ConstantBitrate` and
/// `StatmuxSettings` — each member documents that the other "must be undefined"
/// when it is defined — so a request defining both is refused as a client error
/// and stores nothing, while a request defining either alone, or neither, is
/// accepted.
#[tokio::test]
async fn video_settings_define_at_most_one_rate_control() {
    // (case, the videoSettings the request carries, the expected status)
    let rows: [(&str, Value, u16); 4] = [
        ("neither", json!({}), 201),
        (
            "a constant bitrate alone",
            json!({ "constantBitrate": VIDEO_BITRATE.0 }),
            201,
        ),
        (
            "statmux settings alone",
            json!({ "statmuxSettings": { "priority": 0 } }),
            201,
        ),
        (
            "both",
            json!({
                "constantBitrate": VIDEO_BITRATE.0,
                "statmuxSettings": { "priority": 0 },
            }),
            400,
        ),
    ];

    for (case, video_settings, expected) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "video").await;
        let mut settings = program_settings(5);
        settings["videoSettings"] = video_settings;

        let (status, body) =
            create_program(&router, &multiplex_id, "video", "token-video", settings.clone()).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if expected == 201 {
            assert_eq!(
                body["multiplexProgramSettings"], settings,
                "case {case:?}: expected the settings the request supplied, got {:?}",
                body["multiplexProgramSettings"]
            );
            continue;
        }

        let code = error_code(status, &body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {case:?}: expected a client error, got {code} ({body})"
        );
        let message = error_message(&body);
        assert!(
            message.contains("constantBitrate") && message.contains("statmuxSettings"),
            "case {case:?}: expected the refusal to name both members, got {body}"
        );
        let (status, described) = describe_program(&router, &multiplex_id, "video").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused create to store nothing, got {status}: {described}"
        );
    }
}

/// Rule: within ONE multiplex a program's `ProgramName` and its `ProgramNumber`
/// ("Unique program number") each identify it, so a second create taking either
/// from a program that multiplex already holds is a conflict and stores nothing;
/// the same pair in a different multiplex is a different program and is
/// accepted.
#[tokio::test]
async fn a_program_name_and_number_are_unique_within_one_multiplex() {
    // (case, the second program's name, its number, whether the same multiplex
    //  takes it, the expected status)
    let rows: [(&str, &str, i64, bool, u16); 5] = [
        ("the same name and number", "first", 1, true, 409),
        ("the same name, another number", "first", 2, true, 409),
        ("another name, the same number", "second", 1, true, 409),
        ("another name and number", "second", 2, true, 201),
        ("the same name and number, another multiplex", "first", 1, false, 201),
    ];

    for (case, program_name, program_number, same_multiplex, expected) in rows {
        let router = medialive();
        let one = multiplex(&router, case, "one").await;
        let two = multiplex(&router, case, "two").await;

        let (status, first) =
            create_program(&router, &one, "first", "token-first", program_settings(1)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the first create: expected 201, got {status}: {first}"
        );

        let target = if same_multiplex { &one } else { &two };
        let (status, body) = create_program(
            &router,
            target,
            program_name,
            "token-second",
            program_settings(program_number),
        )
        .await;
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
            // The program that does exist is untouched: it still carries the
            // number the first create gave it.
            let (status, described) = describe_program(&router, &one, "first").await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["multiplexProgramSettings"]["programNumber"],
                json!(1),
                "case {case:?}: expected the stored program to survive the conflict, got {described}"
            );
            let (_, multiplex) = describe_multiplex(&router, &one).await;
            assert_eq!(
                multiplex["programCount"],
                json!(1),
                "case {case:?}: expected the refused create to store nothing, got {:?}",
                multiplex["programCount"]
            );
            continue;
        }

        let (status, described) = describe_program(&router, target, program_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["multiplexProgramSettings"]["programNumber"],
            json!(program_number),
            "case {case:?}: expected the second program to carry its own number, got {described}"
        );
        let (_, multiplex) = describe_multiplex(&router, target).await;
        let expected_count = if same_multiplex { 2 } else { 1 };
        assert_eq!(
            multiplex["programCount"],
            json!(expected_count),
            "case {case:?}: expected the multiplex to count its programs, got {:?}",
            multiplex["programCount"]
        );
    }
}

/// Rule: a create retried with the `RequestId` (`@idempotencyToken`) of an
/// earlier one answers with the program that one created and stores no second
/// program; two creates that do not share a token create two programs.
#[tokio::test]
async fn a_retried_create_returns_the_program_the_first_one_created() {
    // (case, first RequestId, second RequestId, whether both calls share one program)
    let rows: [(&str, &str, &str, bool); 3] = [
        ("the same token", "token-a", "token-a", true),
        ("two tokens", "token-a", "token-b", false),
        (
            "the second token is a prefix of the first",
            "token-ab",
            "token-a",
            false,
        ),
    ];

    for (case, first_token, second_token, shared) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "retried").await;

        let (status, one) = create_program(
            &router,
            &multiplex_id,
            "first",
            first_token,
            program_settings(1),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the first create: expected 201, got {status}: {one}"
        );
        let (status, two) = create_program(
            &router,
            &multiplex_id,
            "second",
            second_token,
            program_settings(2),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the second create: expected 201, got {status}: {two}"
        );

        let (_, multiplex) = describe_multiplex(&router, &multiplex_id).await;
        if shared {
            assert_eq!(
                two, one,
                "case {case:?}: expected the retry to answer with the first program, got {two}"
            );
            assert_eq!(
                multiplex["programCount"],
                json!(1),
                "case {case:?}: expected the retry to store no second program, got {:?}",
                multiplex["programCount"]
            );
            let (status, described) = describe_program(&router, &multiplex_id, "second").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the retry's own name to name no program, got {status}: {described}"
            );
            continue;
        }

        assert_ne!(
            two["programName"], one["programName"],
            "case {case:?}: expected two distinct programs, got {:?} twice",
            one["programName"]
        );
        assert_eq!(
            multiplex["programCount"],
            json!(2),
            "case {case:?}: expected both programs to be stored, got {:?}",
            multiplex["programCount"]
        );
        // Each program carries its own request's settings, and both are stored.
        for (label, number) in [("first", 1), ("second", 2)] {
            let (status, described) = describe_program(&router, &multiplex_id, label).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{label}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["multiplexProgramSettings"]["programNumber"],
                json!(number),
                "case {case:?}/{label}: expected the program to carry its own number, got {described}"
            );
        }
    }
}

/// One row of the idempotency-scope table: the case name, which of the two
/// multiplexes each create is addressed to together with the `RequestId` and the
/// `ProgramName` it carries, and whether the second create must answer with the
/// program the first one created.
type ProgramRetryRow<'a> = (
    &'a str,
    (usize, &'a str, &'a str),
    (usize, &'a str, &'a str),
    bool,
);

/// Rule: a `RequestId` (`@idempotencyToken`) is idempotent only for the endpoint
/// it was presented to, and `CreateMultiplexProgram` is addressed to one
/// multiplex (`POST /prod/multiplexes/{MultiplexId}/programs`) — so a retry
/// replays the first create's program only when it names the SAME multiplex, and
/// the same token presented for another multiplex is a create of its own, which
/// stores a program in the multiplex that request named.
///
/// The rows differ along the tokens, along the multiplexes the two creates are
/// addressed to, along the program names they carry and along the order the two
/// multiplexes are used in — which is what the rule generalises over.
#[tokio::test]
async fn a_retried_create_is_idempotent_only_for_the_multiplex_it_was_addressed_to() {
    // (case, first (multiplex, token, program name), second (likewise), replayed?)
    let rows: [ProgramRetryRow<'_>; 6] = [
        (
            "the same token and name, one multiplex",
            (0, "token-a", "news"),
            (0, "token-a", "news"),
            true,
        ),
        (
            "the same token, two names, one multiplex",
            (0, "token-a", "news"),
            (0, "token-a", "sports"),
            true,
        ),
        (
            "two tokens, one multiplex",
            (0, "token-a", "news"),
            (0, "token-b", "sports"),
            false,
        ),
        (
            "the same token and name, two multiplexes",
            (0, "token-a", "news"),
            (1, "token-a", "news"),
            false,
        ),
        (
            "the same token, two names, two multiplexes",
            (0, "token-a", "news"),
            (1, "token-a", "sports"),
            false,
        ),
        (
            "the same token, two multiplexes, in the other order",
            (1, "token-a", "news"),
            (0, "token-a", "news"),
            false,
        ),
    ];

    for (
        case,
        (first_multiplex, first_token, first_name),
        (second_multiplex, second_token, second_name),
        replayed,
    ) in rows
    {
        let router = medialive();
        let multiplexes = [
            multiplex(&router, case, "retry-scope-a").await,
            multiplex(&router, case, "retry-scope-b").await,
        ];

        let (status, one) = create_program(
            &router,
            &multiplexes[first_multiplex],
            first_name,
            first_token,
            program_settings(1),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the first create: expected 201, got {status}: {one}"
        );
        let (status, two) = create_program(
            &router,
            &multiplexes[second_multiplex],
            second_name,
            second_token,
            program_settings(2),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the second create: expected 201, got {status}: {two}"
        );

        if replayed {
            assert_eq!(
                two, one,
                "case {case:?}: expected the retry to answer with the first program, got {two}"
            );
        } else {
            assert_eq!(
                two["programName"],
                json!(second_name),
                "case {case:?}: expected the second create to answer with its own program, got {two}"
            );
            assert_eq!(
                two["multiplexProgramSettings"]["programNumber"],
                json!(2),
                "case {case:?}: expected the second create to answer with its own settings, got {two}"
            );
            let (status, described) =
                describe_program(&router, &multiplexes[second_multiplex], second_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplexProgram of the second program: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["multiplexProgramSettings"]["programNumber"],
                json!(2),
                "case {case:?}: expected the second program to be stored in the multiplex its request named, got {described}"
            );
        }

        // Each multiplex counts exactly the programs its OWN endpoint was asked
        // for: a replay stores no second program, and every other pair stores
        // one program per create.
        for (index, multiplex_id) in multiplexes.iter().enumerate() {
            let mut expected = usize::from(first_multiplex == index);
            if !replayed {
                expected += usize::from(second_multiplex == index);
            }
            let (status, described) = describe_multiplex(&router, multiplex_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplex {multiplex_id}: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["programCount"],
                json!(expected),
                "case {case:?}: multiplex {multiplex_id}: expected programCount {expected}, got {:?}",
                described["programCount"]
            );
        }
    }
}

/// Rule: a `MultiplexId` that names no stored multiplex cannot carry a program —
/// `DescribeMultiplexProgram` answers `NotFoundException` naming the PROGRAM the
/// request addressed (the multiplex and the program name its URI names it by,
/// even when it is the enclosing multiplex that is missing), and so does a
/// `ProgramName` the named multiplex does not hold, while
/// `CreateMultiplexProgram`, which addresses the multiplex's program collection
/// and models no `NotFoundException`
/// (`smithy_query(query="operation", operation="CreateMultiplexProgram")`),
/// refuses the absent multiplex as a client error naming it and stores nothing.
#[tokio::test]
async fn naming_a_multiplex_or_program_that_does_not_exist_is_refused() {
    let router = medialive();
    let multiplex_id = multiplex(&router, "live", "live").await;
    let (status, program) = create_program(
        &router,
        &multiplex_id,
        "kept",
        "token-kept",
        program_settings(1),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateMultiplexProgram: expected 201, got {status}: {program}"
    );
    // A second multiplex holds a program of its own, so a request that asks the
    // first multiplex for THAT name is refused rather than answered out of the
    // wrong multiplex.
    let other_multiplex = multiplex(&router, "other", "other").await;
    let (status, other) = create_program(
        &router,
        &other_multiplex,
        "other",
        "token-other",
        program_settings(1),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateMultiplexProgram in the other multiplex: expected 201, got {status}: {other}"
    );

    // (case, the multiplex id the request names, the program name it names,
    //  the resource the refusal names and the identifier it names)
    let rows: [(&str, &str, &str, &str, &str); 4] = [
        (
            "a multiplex id never minted",
            "9999999",
            "kept",
            "Multiplex program",
            "kept",
        ),
        (
            "a multiplex id that is not a number at all",
            "no-such-multiplex",
            "kept",
            "Multiplex program",
            "kept",
        ),
        (
            "a program name no program has",
            "",
            "no-such-program",
            "Multiplex program",
            "no-such-program",
        ),
        (
            "a program name that is another multiplex's",
            "",
            "other",
            "Multiplex program",
            "other",
        ),
    ];

    for (case, named_multiplex, program_name, resource_type, identifier) in rows {
        // An empty id in the row means "the multiplex that does exist": the row
        // is about the program name, not the multiplex.
        let named_multiplex = if named_multiplex.is_empty() {
            multiplex_id.as_str()
        } else {
            named_multiplex
        };
        // A program has no identifier of its own, so a refusal about one names
        // it by the multiplex it would live in together with its name, which is
        // also how its URI names it.
        let identifier = if resource_type == "Multiplex program" {
            format!("{named_multiplex}/{identifier}")
        } else {
            identifier.to_string()
        };

        let (status, body) = describe_program(&router, named_multiplex, program_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeMultiplexProgram: expected 404, got {status}: {body}"
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
    }

    // A create naming a multiplex that does not exist is refused as a client
    // error that names it, and stores nothing.
    let (status, body) = create_program(
        &router,
        "9999999",
        "orphan",
        "token-orphan",
        program_settings(2),
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status}: {body}");
    assert_eq!(
        error_code(status, &body),
        "BadRequestException",
        "expected BadRequestException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "Multiplex 9999999 not found",
        "expected the refusal to name the multiplex asked for, got {body}"
    );
    let (status, described) = describe_program(&router, "9999999", "orphan").await;
    assert_eq!(
        status, 404,
        "expected the refused create to store nothing, got {status}: {described}"
    );

    // And none of those refusals touched the programs that do exist.
    let (status, described) = describe_program(&router, &multiplex_id, "kept").await;
    assert_eq!(
        status, 200,
        "DescribeMultiplexProgram: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described, program,
        "expected the stored program to survive, got {described}"
    );
    let (status, described) = describe_program(&router, &other_multiplex, "other").await;
    assert_eq!(
        status, 200,
        "DescribeMultiplexProgram in the other multiplex: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described, other,
        "expected the other multiplex's program to survive, got {described}"
    );
}

/// Rule: a program's `ChannelId` ("The MediaLive channel associated with the
/// program") is the channel whose `Destinations` name that multiplex and that
/// program, derived from the channels the store holds on every read — so it
/// appears when such a channel is created, names only the program that channel's
/// destination points at, and is gone once that channel has been deleted.
#[tokio::test]
async fn a_programs_channel_is_the_channel_whose_destination_names_it() {
    let router = medialive();
    let multiplex_id = multiplex(&router, "associated", "associated").await;
    for (program_name, number) in [("news", 1), ("sports", 2)] {
        let (status, program) = create_program(
            &router,
            &multiplex_id,
            program_name,
            &format!("token-{program_name}"),
            program_settings(number),
        )
        .await;
        assert_eq!(
            status, 201,
            "CreateMultiplexProgram {program_name}: expected 201, got {status}: {program}"
        );
    }

    // No channel yet: neither program reports one.
    for program_name in ["news", "sports"] {
        let (_, described) = describe_program(&router, &multiplex_id, program_name).await;
        assert_eq!(
            described["channelId"],
            Value::Null,
            "case {program_name:?}: expected no channel before one is created, got {described}"
        );
    }

    // A channel whose output destination names ONE of the two programs.
    let (status, response) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "feeds-the-news",
            "destinations": [{
                "id": "multiplex-out",
                "multiplexSettings": {
                    "multiplexId": multiplex_id,
                    "programName": "news",
                },
            }],
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
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

    // (the program, the channel id it is expected to report)
    for (program_name, expected) in [("news", json!(channel_id)), ("sports", Value::Null)] {
        let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {program_name:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["channelId"], expected,
            "case {program_name:?}: expected the channel named by the destination, got {:?}",
            described["channelId"]
        );
    }

    // Once the channel has been deleted it is associated with nothing: the
    // program reports no channel again.
    wait_for_the_clock();
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/prod/channels/{channel_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteChannel: expected 200, got {status}: {deleted}"
    );
    wait_for_the_clock();
    let (status, described) = describe_program(&router, &multiplex_id, "news").await;
    assert_eq!(
        status, 200,
        "DescribeMultiplexProgram: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described["channelId"],
        Value::Null,
        "expected a deleted channel to be associated with nothing, got {:?}",
        described["channelId"]
    );
}
