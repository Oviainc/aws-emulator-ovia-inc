// SPDX-License-Identifier: CC0-1.0

//! How long a `CreateMultiplexProgram` `RequestId` (`@idempotencyToken`) stays a
//! replay, end to end over the restJson1 wire.
//!
//! RULE: a create's token binds only to the resource it minted, so deleting that
//! resource ends the token's window and a create carrying it again is answered
//! exactly as the same request under a `RequestId` nothing ever spent would be.
//! A multiplex program is identified by the multiplex it lives in together with
//! its caller-chosen `ProgramName` (`smithy_query(query="operation",
//! operation="DescribeMultiplexProgram")`: `DELETE
//! /prod/multiplexes/{MultiplexId}/programs/{ProgramName}`) and it has no
//! lifecycle state of its own, so `DeleteMultiplexProgram` takes the program
//! right out of the store rather than keeping a tombstone: a token that outlived
//! it would not merely go stale, it would resolve to whichever program held the
//! name NEXT — a program a different request created, with settings the
//! replaying request never asked for.
//!
//! The same rule, on the resources whose delete keeps a tombstone record, is the
//! table of [`crate::idempotency_token_after_terminal_delete`]; the window a
//! program token loses when its MULTIPLEX is deleted is pinned there too. What
//! this suite adds is the window the PROGRAM's own delete ends.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suite is a TABLE: the reported create-delete-recreate-retry sequence is
//! ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_program_deletion::{delete_program, listed_names};
use crate::multiplex_programs::{create_program, describe_program, multiplex, program_settings};
use crate::wire::{call, error_code, error_message, medialive};

/// The token every row spends on the program it then deletes, and retries with.
const TOKEN: &str = "token-a";

/// The token the bystander program of the same multiplex is created under, so
/// the delete can be shown to end ONE token's window rather than the map's.
const BYSTANDER_TOKEN: &str = "token-b";

/// The name and the `ProgramNumber` the deleted program is created with.
const DELETED_NAME: &str = "news";
const DELETED_NUMBER: i64 = 7;

/// The bystander's name and number, which no row's retry asks for.
const BYSTANDER_NAME: &str = "sports";
const BYSTANDER_NUMBER: i64 = 9;

/// What the retry past the delete must be answered with: the answer the SAME
/// request gets under a `RequestId` nothing ever spent, which is what the
/// forgotten token leaves it as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retried {
    /// `201`, with a program the retry itself stored under the name and the
    /// settings IT carried.
    Stored,
    /// `409`, because another program of the multiplex now holds the name the
    /// retry asks for.
    NameInUse,
    /// `409`, because another program of the multiplex now holds the
    /// `ProgramNumber` the retry's settings carry.
    NumberInUse { held_by: &'static str },
}

/// One row: the case, the programs created (under tokens of their own) after the
/// delete, the name and the `ProgramNumber` the retry carries, what the retry
/// must be answered with, whether a bystander program of the same multiplex
/// holds a token of its own, and whether a SECOND multiplex holds a program
/// created under the very same `RequestId`.
type Row = (
    &'static str,
    &'static [(&'static str, i64)],
    &'static str,
    i64,
    Retried,
    bool,
    bool,
);

/// The programs of one multiplex, by name, with each one's `ProgramNumber`, as
/// the readers report them — what a create either added to or was refused
/// against.
async fn held(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
) -> Vec<(String, Value)> {
    let mut programs = Vec::new();
    for name in listed_names(router, case, multiplex_id).await {
        let (status, described) = describe_program(router, multiplex_id, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram({name}): expected 200, got {status}: \
             {described}"
        );
        programs.push((name, described["multiplexProgramSettings"].clone()));
    }
    programs
}

/// One create asserted to have been served, as the program it answered with.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    name: &str,
    token: &str,
    number: i64,
) -> Value {
    let (status, program) =
        create_program(router, multiplex_id, name, token, program_settings(number)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateMultiplexProgram({name}): expected 201, got {status}: {program}"
    );
    program
}

/// Rule: deleting a multiplex program ends its create token's window — the
/// program is gone from the store, so a create presenting that `RequestId` again
/// is judged on the multiplex's state exactly as a first attempt is: it STORES
/// the program its own request describes when the name and the number are free,
/// and is refused with the modelled `ConflictException` when another program now
/// holds one of them. In particular a later program that reuses the deleted
/// name is never what the spent token answers with.
///
/// The rows differ along what the rule generalises over: whether the retry asks
/// for the deleted program's own name or a new one, whether another program has
/// since taken that name or that number, which of the two uniqueness rules the
/// retry then collides with, and whether other tokens — a bystander program of
/// the same multiplex, and the same `RequestId` spent on another multiplex — are
/// in the store beside it.
#[tokio::test]
async fn deleting_a_program_forgets_the_token_that_created_it() {
    const ROWS: [Row; 7] = [
        (
            "the retry asks for the deleted program's own name and number",
            &[],
            DELETED_NAME,
            DELETED_NUMBER,
            Retried::Stored,
            false,
            false,
        ),
        (
            "the retry asks for a free name and a free number",
            &[],
            "late-news",
            8,
            Retried::Stored,
            true,
            false,
        ),
        (
            "another program reused the deleted name",
            &[(DELETED_NAME, 2)],
            DELETED_NAME,
            DELETED_NUMBER,
            Retried::NameInUse,
            false,
            false,
        ),
        (
            "another program reused the deleted name, beside other tokens",
            &[(DELETED_NAME, 2)],
            DELETED_NAME,
            DELETED_NUMBER,
            Retried::NameInUse,
            true,
            true,
        ),
        (
            "another program took the deleted number",
            &[("weather", DELETED_NUMBER)],
            "late-news",
            DELETED_NUMBER,
            Retried::NumberInUse { held_by: "weather" },
            false,
            false,
        ),
        (
            "the deleted name is taken and the retry asks for a free one",
            &[(DELETED_NAME, 2)],
            "late-news",
            8,
            Retried::Stored,
            true,
            true,
        ),
        (
            "two programs were created after the delete",
            &[(DELETED_NAME, 2), ("weather", 3)],
            DELETED_NAME,
            65_535,
            Retried::NameInUse,
            true,
            false,
        ),
    ];

    for (case, recreated, retried_name, retried_number, expected, with_bystander, with_other) in
        ROWS
    {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "programmed").await;
        let first = created(
            &router,
            case,
            &multiplex_id,
            DELETED_NAME,
            TOKEN,
            DELETED_NUMBER,
        )
        .await;
        if with_bystander {
            created(
                &router,
                case,
                &multiplex_id,
                BYSTANDER_NAME,
                BYSTANDER_TOKEN,
                BYSTANDER_NUMBER,
            )
            .await;
        }
        // A second multiplex holding a program created under the SAME
        // `RequestId`: its own window is none of this delete's business, because
        // a token names the multiplex it was presented to.
        let other_multiplex = if with_other {
            let other = multiplex(&router, case, "neighbour").await;
            created(
                &router,
                case,
                &other,
                DELETED_NAME,
                TOKEN,
                DELETED_NUMBER,
            )
            .await;
            Some(other)
        } else {
            None
        };

        // While the program is there the token IS a replay: the same create
        // answers with the program it minted, whatever name and number it now
        // asks for, and stores nothing.
        let before = held(&router, case, &multiplex_id).await;
        let (status, replayed) = create_program(
            &router,
            &multiplex_id,
            retried_name,
            TOKEN,
            program_settings(retried_number),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the live replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            replayed, first,
            "case {case:?}: expected a live program's token to replay it, got {replayed}"
        );
        assert_eq!(
            held(&router, case, &multiplex_id).await,
            before,
            "case {case:?}: expected the live replay to have stored nothing"
        );

        let (status, deleted) = delete_program(&router, &multiplex_id, DELETED_NAME).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMultiplexProgram: expected 200, got {status}: {deleted}"
        );

        // The programs the row creates after the delete, each under a token of
        // its own — among them, where the row asks for it, one that reuses the
        // deleted program's name.
        for (name, number) in recreated {
            created(
                &router,
                case,
                &multiplex_id,
                name,
                &format!("token-{name}-{number}"),
                *number,
            )
            .await;
        }

        let before = held(&router, case, &multiplex_id).await;
        let (status, retried) = create_program(
            &router,
            &multiplex_id,
            retried_name,
            TOKEN,
            program_settings(retried_number),
        )
        .await;
        match expected {
            Retried::Stored => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected the retry to be served as a first attempt, got \
                     {status}: {retried}"
                );
                assert_eq!(
                    retried["programName"],
                    json!(retried_name),
                    "case {case:?}: expected the name the retry asked for, got {retried}"
                );
                assert_eq!(
                    retried["multiplexProgramSettings"],
                    program_settings(retried_number),
                    "case {case:?}: expected the settings the retry asked for, got {retried}"
                );
                // It really STORED that program: the reader reports it and the
                // multiplex now holds it beside what was already there.
                let (status, read) =
                    describe_program(&router, &multiplex_id, retried_name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DescribeMultiplexProgram after the retry: expected 200, got \
                     {status}: {read}"
                );
                assert_eq!(
                    read, retried,
                    "case {case:?}: expected the reader to report the program the retry stored, \
                     got {read}"
                );
                let mut expected_held = before.clone();
                expected_held.push((
                    retried_name.to_string(),
                    program_settings(retried_number),
                ));
                expected_held.sort_by(|left, right| left.0.cmp(&right.0));
                assert_eq!(
                    held(&router, case, &multiplex_id).await,
                    expected_held,
                    "case {case:?}: expected the multiplex to hold the retry's own program too"
                );

                // The token now names the program the retry stored: one more
                // retry replays THAT one and stores nothing.
                let (status, again) = create_program(
                    &router,
                    &multiplex_id,
                    "another-name",
                    TOKEN,
                    program_settings(42),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected the third create to answer 201, got {status}: {again}"
                );
                assert_eq!(
                    again, retried,
                    "case {case:?}: expected the token to replay the program it just minted, got \
                     {again}"
                );
                assert_eq!(
                    held(&router, case, &multiplex_id).await,
                    expected_held,
                    "case {case:?}: expected the second replay to have stored nothing"
                );
            }
            Retried::NameInUse | Retried::NumberInUse { .. } => {
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected the retry to be refused as a first attempt is, got \
                     {status}: {retried}"
                );
                assert_eq!(
                    error_code(status, &retried),
                    "ConflictException",
                    "case {case:?}: expected the modelled ConflictException, got {retried}"
                );
                let expected_message = match expected {
                    Retried::NameInUse => format!(
                        "Multiplex {multiplex_id} already has a program named {retried_name}"
                    ),
                    Retried::NumberInUse { held_by } => format!(
                        "multiplexProgramSettings.programNumber {retried_number} is already used \
                         by the program {held_by} of multiplex {multiplex_id}"
                    ),
                    Retried::Stored => unreachable!("a stored retry is handled above"),
                };
                let message = error_message(&retried);
                assert_eq!(
                    message, expected_message,
                    "case {case:?}: expected the refusal a first attempt gets, got {message:?}"
                );
                assert_eq!(
                    held(&router, case, &multiplex_id).await,
                    before,
                    "case {case:?}: expected the refused retry to have written nothing"
                );
            }
        }

        // The bystander's own token was never this delete's business.
        if with_bystander {
            let (status, replayed) = create_program(
                &router,
                &multiplex_id,
                "yet-another-name",
                BYSTANDER_TOKEN,
                program_settings(43),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the bystander's replay to answer 201, got {status}: \
                 {replayed}"
            );
            assert_eq!(
                replayed["programName"],
                json!(BYSTANDER_NAME),
                "case {case:?}: expected the surviving token to replay its own program, got \
                 {replayed}"
            );
            assert_eq!(
                replayed["multiplexProgramSettings"],
                program_settings(BYSTANDER_NUMBER),
                "case {case:?}: expected the bystander's own settings, got {replayed}"
            );
        }

        // And the same `RequestId`, spent on ANOTHER multiplex, still replays
        // that multiplex's program: the delete ended one program's window.
        if let Some(other) = other_multiplex {
            let (status, elsewhere) = create_program(
                &router,
                &other,
                "a-new-name",
                TOKEN,
                program_settings(44),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the other multiplex's replay to answer 201, got \
                 {status}: {elsewhere}"
            );
            assert_eq!(
                elsewhere["programName"],
                json!(DELETED_NAME),
                "case {case:?}: expected the other multiplex's token to replay its own program, \
                 got {elsewhere}"
            );
            assert_eq!(
                listed_names(&router, case, &other).await,
                vec![DELETED_NAME.to_string()],
                "case {case:?}: expected the other multiplex to hold only its own program"
            );
        }
    }
}

/// Rule: the window a program's delete ends is that program's alone, and it ends
/// for good — a token spent again after the program it minted was deleted names
/// the program THAT create stored, and deleting that one in turn ends the window
/// again, however many rounds a caller runs.
///
/// The rows differ along what the rule generalises over: how many
/// create-delete-retry rounds one token runs, and whether each round reuses the
/// one name or asks for a new one.
#[tokio::test]
async fn one_token_over_repeated_create_and_delete_rounds() {
    // (case, how many rounds, whether every round reuses one name)
    let rows: [(&str, usize, bool); 4] = [
        ("two rounds under one name", 2, true),
        ("two rounds under a new name each", 2, false),
        ("three rounds under one name", 3, true),
        ("three rounds under a new name each", 3, false),
    ];

    for (case, rounds, same_name) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "rounds").await;

        for round in 1..=rounds {
            let case = format!("{case}, round {round}");
            let name = if same_name {
                DELETED_NAME.to_string()
            } else {
                format!("{DELETED_NAME}-{round}")
            };
            let number = DELETED_NUMBER + round as i64;

            // The round's own create: the token replays nothing, because every
            // program it ever minted has been deleted.
            let program = created(&router, &case, &multiplex_id, &name, TOKEN, number).await;
            assert_eq!(
                program["programName"],
                json!(name),
                "case {case:?}: expected the create to store the name it asked for, got {program}"
            );
            assert_eq!(
                program["multiplexProgramSettings"],
                program_settings(number),
                "case {case:?}: expected the create to store its own settings, got {program}"
            );
            assert_eq!(
                listed_names(&router, &case, &multiplex_id).await,
                vec![name.clone()],
                "case {case:?}: expected the multiplex to hold exactly this round's program"
            );

            // While it is there the token replays it.
            let (status, replayed) = create_program(
                &router,
                &multiplex_id,
                "replay-asks-for-this",
                TOKEN,
                program_settings(1),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the replay to answer 201, got {status}: {replayed}"
            );
            assert_eq!(
                replayed, program,
                "case {case:?}: expected the token to replay this round's program, got {replayed}"
            );

            let (status, deleted) = delete_program(&router, &multiplex_id, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteMultiplexProgram: expected 200, got {status}: {deleted}"
            );
            assert!(
                listed_names(&router, &case, &multiplex_id).await.is_empty(),
                "case {case:?}: expected the multiplex to hold no program after the delete, got \
                 {:?}",
                listed_names(&router, &case, &multiplex_id).await
            );
        }

        // Every program the token minted is gone, so the store holds none of
        // them and a read of the last name is a not-found — nothing the spent
        // token could have resolved to.
        let (status, described) = describe_program(&router, &multiplex_id, DELETED_NAME).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted program to be a not-found, got {status}: \
             {described}"
        );
        let (status, listing) = call(
            &router,
            "GET",
            &format!("/prod/multiplexes/{multiplex_id}/programs"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMultiplexPrograms: expected 200, got {status}: {listing}"
        );
        assert_eq!(
            listing["multiplexPrograms"],
            json!([]),
            "case {case:?}: expected the multiplex to hold no program at all, got {listing}"
        );
    }
}
