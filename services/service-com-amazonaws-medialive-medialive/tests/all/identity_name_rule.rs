// SPDX-License-Identifier: CC0-1.0

//! The rule this crate applies to a caller-chosen name that IS the identity of
//! the resource a create stores — the key the resource is kept at, and the path
//! segment the operations that address one resource carry — driven end to end
//! over the restJson1 wire through every operation that applies it.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES spanning what the rule generalises over: which
//! OPERATION keys a resource by a name its caller supplied, and the FORM of the
//! name supplied (its length, its character classes and whether its characters
//! survive a URI path segment unencoded). The empty name is one row, and the
//! recorded `CreateMultiplexProgram` call is one more.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_lifecycle::{describe_multiplex, MULTIPLEXES};
use crate::multiplex_programs::{create_program, multiplex, program_settings};
use crate::support::{channel_encoder_settings, percent_encoded};
use crate::wire::{call, error_code, error_message, medialive};

/// Names of the one form the rule refuses and of the forms it serves.
///
/// `ProgramName` targets bare `__string` with no `@length` and no `@pattern`
/// (`smithy_query(query="operation", operation="CreateMultiplexProgram")`), so
/// every name here but the empty one is a name the model admits: one character,
/// digits, mixed case, each punctuation character a client reaches for, a name
/// whose characters need percent-encoding to travel a path segment, a non-ASCII
/// name and a long one.
const SERVED_NAMES: [(&str, &str); 9] = [
    ("one character", "a"),
    ("digits only", "407"),
    ("mixed case", "NewsAtTen"),
    ("hyphenated", "news-at-ten"),
    ("underscored", "news_at_ten"),
    ("dotted", "news.at.ten"),
    ("a space inside the name", "news at ten"),
    ("non-ASCII characters", "nouvelles-à-dix"),
    (
        "a long name",
        "news-at-ten-news-at-ten-news-at-ten-news-at-ten-news-at-ten-news-at-ten-\
         news-at-ten-news-at-ten-news-at-ten-news-at-ten-news-at-ten-news-at-ten",
    ),
];

/// The wording the one copy of the rule carries
/// (`crate::state::non_empty_identity`), per member it is applied to.
fn empty_name_message(member: &str) -> String {
    format!("{member} must not be empty")
}

/// `DescribeMultiplexProgram` on a program whose name is percent-encoded into
/// the path segment the operation carries it as (`ProgramName` is an
/// `@httpLabel` of `/prod/multiplexes/{MultiplexId}/programs/{ProgramName}`).
async fn describe_by_label(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    program_name: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "{MULTIPLEXES}/{multiplex_id}/programs/{}",
            percent_encoded(program_name)
        ),
        None,
    )
    .await
}

/// How many programs the multiplex reports holding (`ProgramCount` is "The
/// number of programs in the multiplex", derived from what the store holds).
async fn program_count(router: &MultiServiceRouter, case: &str, multiplex_id: &str) -> Value {
    let (status, multiplex) = describe_multiplex(router, multiplex_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {multiplex}"
    );
    multiplex["programCount"].clone()
}

/// Rule: a create whose caller-supplied name identifies the resource it stores
/// refuses an EMPTY name with the operation's own modelled client error and
/// stores nothing, while any non-empty name the model admits is stored and is
/// addressable afterwards by that name.
///
/// The rows span the form of the name — its length, its character classes and
/// whether it needs percent-encoding to travel the path segment that addresses
/// the program — because the rule generalises over every name but the empty one.
#[tokio::test]
async fn create_multiplex_program_refuses_an_empty_program_name() {
    // (case, the name the request carries, the status it is answered with)
    let mut rows: Vec<(&str, &str, u16)> = vec![("the empty name", "", 400)];
    for (case, name) in SERVED_NAMES {
        rows.push((case, name, 201));
    }

    for (case, program_name, expected) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "named").await;

        let (status, body) = create_program(
            &router,
            &multiplex_id,
            program_name,
            "token-named",
            program_settings(7),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: CreateMultiplexProgram: expected {expected}, got {status}: {body}"
        );

        if expected == 201 {
            assert_eq!(
                body["programName"],
                json!(program_name),
                "case {case:?}: expected the program stored under the requested name, got {:?}",
                body["programName"]
            );
            // The name is the identity: the operation that addresses ONE program
            // reads this program back by it.
            let (status, described) = describe_by_label(&router, &multiplex_id, program_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described, body,
                "case {case:?}: expected the created program read back by its name, got {described}"
            );
            assert_eq!(
                program_count(&router, case, &multiplex_id).await,
                json!(1),
                "case {case:?}: expected the multiplex to count its one program"
            );
            continue;
        }

        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected the operation's modelled client error, got {body}"
        );
        assert_eq!(
            error_message(&body),
            empty_name_message("programName"),
            "case {case:?}: expected the shared refusal wording, got {:?}",
            error_message(&body)
        );
        // And it stored nothing: the multiplex holds no program, so no program
        // slot and no PID block of it was taken by a name nothing can address.
        assert_eq!(
            program_count(&router, case, &multiplex_id).await,
            json!(0),
            "case {case:?}: expected the refused create to store nothing"
        );
        let (status, listed) = call(
            &router,
            "GET",
            &format!("{MULTIPLEXES}/{multiplex_id}/programs"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMultiplexPrograms: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed["multiplexPrograms"],
            json!([]),
            "case {case:?}: expected the listing to report no program, got {:?}",
            listed["multiplexPrograms"]
        );
    }
}

/// Rule: a refused create takes nothing from the resource it would have been
/// stored in, so everything the create would have consumed is still available to
/// the next one — whatever the request that follows the refusal asks for.
///
/// The rows span what the follow-up create reuses: the program number the
/// refused request named, a different number, and the empty name again (which is
/// refused again rather than answered as a name already in use).
#[tokio::test]
async fn a_refused_empty_name_leaves_the_multiplex_untouched() {
    // (case, the name the follow-up create carries, its program number, status)
    let rows: [(&str, &str, i64, u16); 3] = [
        ("the same program number, under a name", "news", 7, 201),
        ("another program number", "sports", 11, 201),
        ("the empty name once more", "", 7, 400),
    ];

    for (case, program_name, program_number, expected) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "untouched").await;

        let (status, refusal) = create_program(
            &router,
            &multiplex_id,
            "",
            "token-empty",
            program_settings(7),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the empty name to be refused, got {status}: {refusal}"
        );

        let (status, body) = create_program(
            &router,
            &multiplex_id,
            program_name,
            "token-after",
            program_settings(program_number),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: the create after the refusal: expected {expected}, got {status}: {body}"
        );

        if expected != 201 {
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected the empty name refused as a bad request, got {body}"
            );
            assert_eq!(
                program_count(&router, case, &multiplex_id).await,
                json!(0),
                "case {case:?}: expected the multiplex to still hold no program"
            );
            continue;
        }

        assert_eq!(
            program_count(&router, case, &multiplex_id).await,
            json!(1),
            "case {case:?}: expected the multiplex to count the one program it now holds"
        );
        let (status, described) = describe_by_label(&router, &multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["multiplexProgramSettings"]["programNumber"],
            json!(program_number),
            "case {case:?}: expected the program number the request asked for, got {:?}",
            described["multiplexProgramSettings"]
        );
        // The PID block the refused create would have allocated is the one this
        // program got: nothing was held back by the refusal.
        assert!(
            described["packetIdentifiersMap"].is_object(),
            "case {case:?}: expected the stored program to carry its allocated identifiers, got {described}"
        );
    }
}

/// Rule: the SAME rule, with the same wording, is what the other operation that
/// keys a resource by a caller-chosen name applies — `BatchUpdateSchedule`,
/// whose `ActionName` "provides the main reference to an action once it is added
/// to the schedule" — so one empty identity name is refused the same way in both
/// places, through each operation's own error channel.
#[tokio::test]
async fn batch_update_schedule_refuses_an_empty_action_name_by_the_same_rule() {
    let router = medialive();
    let (status, created) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "identity-named",
            "encoderSettings": channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateChannel to answer 201, got {status}: {created}"
    );
    let channel_id = created["channel"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created channel carries no id: {created}"))
        .to_string();
    let schedule = format!("/prod/channels/{channel_id}/schedule");

    // (case, the action name the request creates, the status it is answered with)
    let rows: [(&str, &str, u16); 2] = [
        ("the empty name", "", 422),
        ("a name of one character", "a", 200),
    ];

    for (case, action_name, expected) in rows {
        let (status, body) = call(
            &router,
            "PUT",
            &schedule,
            Some(json!({
                "creates": {
                    "scheduleActions": [{
                        "actionName": action_name,
                        "scheduleActionSettings": { "timedMetadataSettings": { "id3": "QUJD" } },
                        "scheduleActionStartSettings": {
                            "immediateModeScheduleActionStartSettings": {}
                        },
                    }]
                }
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: BatchUpdateSchedule: expected {expected}, got {status}: {body}"
        );
        if expected == 200 {
            continue;
        }
        assert_eq!(
            error_code(status, &body),
            "UnprocessableEntityException",
            "case {case:?}: expected this operation's own modelled client error, got {body}"
        );
        assert_eq!(
            body["validationErrors"][0]["elementPath"],
            json!("creates/scheduleActions/0/actionName"),
            "case {case:?}: expected the refusal to name the member, got {:?}",
            body["validationErrors"]
        );
        assert_eq!(
            body["validationErrors"][0]["errorMessage"],
            json!(empty_name_message("actionName")),
            "case {case:?}: expected the shared refusal wording, got {:?}",
            body["validationErrors"]
        );
    }
}
