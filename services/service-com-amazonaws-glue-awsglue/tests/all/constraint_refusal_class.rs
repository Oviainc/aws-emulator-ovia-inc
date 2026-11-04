// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `GetColumnStatisticsTaskRun` classifies the identifier it is
//! given: a malformed one against one that names no run.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a value that violates the modelled constraints of the member
//! addressing a resource is a MALFORMED REQUEST — refused with HTTP 400 and the error code
//! `ValidationException`, in the constraint-violation wording AWS words such a refusal
//! with, before anything is looked up. A value that satisfies those constraints is a
//! well-formed address, so it reaches the lookup: one no run is stored under is the
//! modelled `EntityNotFoundException`, whose sentence names the member the request
//! addressed the run by, and the id a start reported serves its run.
//!
//! The rows differ along what the rule generalises over: which character outside the
//! member's pattern the value carries and where it carries it, and — on the other side of
//! the rule — which characters INSIDE the pattern a value carries, including the ones a
//! reader might take for control characters.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok, REGION};

/// The sentence a read refuses an id no run is stored under with: fixed text naming the
/// member the request addressed the run by, with the value withheld. Written here
/// independently of the handler, so the suite pins the wording and not the code.
const RUN_NOT_FOUND: &str =
    "For given input ColumnStatisticsTaskRunId, associated ColumnStatisticsTaskRun does not \
     exist";

/// `ColumnStatisticsTaskRunId`'s modelled `@pattern`, as the model writes it
/// (`smithy_query operation=GetColumnStatisticsTaskRun`). A constraint refusal quotes the
/// expression the member declares, so the expectation below quotes it the same way.
const RUN_ID_PATTERN: &str = r"^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$";

/// The member path a constraint clause names the member by.
const RUN_ID_MEMBER_PATH: &str = "columnStatisticsTaskRunId";

/// The IAM role a start names, so the run the well-formed rows miss really could exist.
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-column-stats-role";

/// The refusal a value outside [`RUN_ID_PATTERN`] must carry: AWS's validation-error
/// envelope over the one clause that fired, naming the value submitted, the member path it
/// was sent at and the expression it had to satisfy.
fn expected_clause(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{RUN_ID_MEMBER_PATH}' failed to \
         satisfy constraint: Member must satisfy regular expression pattern: {}",
        // A per-member clause quotes the expression without the anchoring pair, the
        // rendering `service_common::validation::unanchored` owns for every service here.
        ::service_common::validation::unanchored(RUN_ID_PATTERN)
    )
}

/// The wording of a constraint refusal, which the awsJson protocols carry in the error
/// shape's lowercase `message` member — the member the boundary validator's own refusals
/// come back in.
fn constraint_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Read one run id and answer the HTTP status and body.
async fn read_run(
    router: &smithy_server_runtime::MultiServiceRouter,
    run_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GetColumnStatisticsTaskRun",
        &json!({ "ColumnStatisticsTaskRunId": run_id }),
    )
    .await
}

// ---------------------------------------------------------------------------
// A value the member's pattern does not admit
// ---------------------------------------------------------------------------

/// An id carrying a character the member's pattern does not admit is refused as a
/// malformed request — `ValidationException`, in AWS's constraint wording — and not as a
/// run that could not be found.
#[tokio::test]
async fn a_run_id_outside_the_members_pattern_is_a_malformed_request() {
    let router = emulator();

    // (case, the id the request carries). Every row carries a character below U+0020 that
    // is not a tab, or one of the two non-characters the pattern's ranges stop short of.
    let rows: Vec<(&str, String)> = vec![
        ("newlines inside the id", "bad\nrun\nid".to_string()),
        ("a trailing newline", "run-id\n".to_string()),
        ("a leading newline", "\nrun-id".to_string()),
        ("a carriage return", "run\rid".to_string()),
        ("a bare newline alone", "\n".to_string()),
        ("a vertical tab", "run\u{000B}id".to_string()),
        ("a form feed", "run\u{000C}id".to_string()),
        ("a NUL", "run\u{0000}id".to_string()),
        ("a bell", "run\u{0007}id".to_string()),
        ("an escape", "run\u{001B}id".to_string()),
        ("a unit separator", "run\u{001F}id".to_string()),
        (
            "the non-character just past the pattern's last admitted character",
            "run\u{FFFE}id".to_string(),
        ),
        ("the last code point of the plane", "run\u{FFFF}id".to_string()),
    ];

    for (case, run_id) in &rows {
        let (status, body) = read_run(&router, run_id).await;
        assert_eq!(
            status, 400,
            "case {case}: a malformed id must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected ValidationException for {run_id:?}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            constraint_message(&body),
            expected_clause(run_id),
            "case {case}: unexpected wording for {run_id:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A value the pattern admits
// ---------------------------------------------------------------------------

/// An id the member's pattern admits is a well-formed address, so it reaches the lookup:
/// one no run is stored under is the modelled `EntityNotFoundException`, while the id a
/// start reported serves that run.
#[tokio::test]
async fn a_run_id_the_pattern_admits_reaches_the_lookup() {
    let router = emulator();

    // (case, the id the request carries). Each row is inside the pattern: a tab is the one
    // control character it admits, and its ranges cover every printable character of the
    // basic plane plus the astral planes it spells as a surrogate pair.
    let rows: Vec<(&str, String)> = vec![
        ("a plain id nothing minted", "no-such-run".to_string()),
        ("a tab, the one control character the pattern admits", "run\tid".to_string()),
        ("a single space, which the pattern's first range opens at", " ".to_string()),
        ("characters outside ASCII", "corrida-señal".to_string()),
        ("a character from the astral planes", "run-😀".to_string()),
        (
            "the last character of the pattern's first range",
            "run\u{D7FF}id".to_string(),
        ),
        (
            "the first character of its second range",
            "run\u{E000}id".to_string(),
        ),
        (
            "the last character of its second range",
            "run\u{FFFD}id".to_string(),
        ),
        ("an id of the longest length the member admits", "r".repeat(255)),
    ];

    for (case, run_id) in &rows {
        let (status, body) = read_run(&router, run_id).await;
        assert_eq!(
            status, 400,
            "case {case}: an id naming no run must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {run_id:?}, got {} in {body}",
            error_code(&body)
        );
        // The miss names the MEMBER the request addressed the run by and withholds the
        // value, so the sentence is the same whatever well-formed id was sent.
        assert_eq!(
            error_message(&body),
            RUN_NOT_FOUND,
            "case {case}: unexpected wording for the miss of {run_id:?}"
        );
    }

    // The id a start reported is served, so the misses above are misses and not a lookup
    // that refuses everything.
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "stats_class_db" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "stats_class_db",
            "TableInput": {
                "Name": "events",
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://lake/{REGION}/events/"),
                    "Columns": [ { "Name": "id", "Type": "bigint" } ],
                },
            },
        }),
    )
    .await;
    let started = ok(
        &router,
        "StartColumnStatisticsTaskRun",
        &json!({
            "DatabaseName": "stats_class_db",
            "TableName": "events",
            "Role": ROLE,
        }),
    )
    .await;
    let run_id = started
        .get("ColumnStatisticsTaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the start must report a run id, got {started}"))
        .to_string();
    let (status, body) = read_run(&router, &run_id).await;
    assert_eq!(
        status, 200,
        "the id the start reported must serve its run, got {status} with {body}"
    );
    assert_eq!(
        body["ColumnStatisticsTaskRun"]["ColumnStatisticsTaskRunId"]
            .as_str()
            .unwrap_or_default(),
        run_id,
        "the served run must be the one the start reported, got {body}"
    );
}
