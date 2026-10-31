// SPDX-License-Identifier: CC0-1.0

//! Which offender a `statusDetails` refusal names when a request carries SEVERAL.
//!
//! The rule under test: one request has one answer — a refusal that quotes part of
//! the request picks what it quotes by a function of the request's CONTENT, so the
//! same request is always refused with the same message. For a map carrying more
//! than one inadmissible pair this plane answers for the first of them in NAME
//! order, which is the same normalisation the rest of the surface applies to a map
//! before it reaches the wire (a command execution's `parameters` are rendered in
//! sorted name order, and a stored details map is kept ordered).
//!
//! The table spans the dimensions the rule generalises over: how MANY pairs offend,
//! which KIND of bound each breaks (a name past the key shape's length, a name
//! outside its pattern, a value past the published ceiling), where the offenders sit
//! among admissible pairs, and how the names order against each other (case, digits,
//! punctuation, one name a prefix of another). Each row is sent repeatedly, because
//! a map's iteration order is a function of its hasher's seed rather than of the
//! keys it holds: a single answer proves nothing, a run of identical answers does.
//!
//! Asserted on BOTH operations that accept the member —
//! `StartNextPendingJobExecution` and `UpdateJobExecution` — and over the wire
//! (restJson1 through the multi-service router), so what is pinned is the HTTP
//! message a device agent reads.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    MAX_STATUS_DETAIL_NAME_LEN, MAX_STATUS_DETAIL_VALUE_LEN,
};
use service_common::iot_job_executions::STATUS_DETAIL_NAME_PATTERN;

use crate::support::{error_code, error_message, Emulator};

/// The operation's own modelled client error, which both handlers refuse a map the
/// details cannot hold with.
const MODELED_REFUSAL: &str = "InvalidRequestException";

/// How many times each row's request is sent. A map's iteration order is decided per
/// instance, so a row carrying `n` offenders would agree with itself by chance with
/// probability `n.pow(1 - REPEATS)` if the answer were read off that order.
const REPEATS: usize = 12;

/// One row: the `statusDetails` pairs a request carries and the one message every
/// send of it must be refused with.
struct Row {
    case: &'static str,
    details: Value,
    expected: String,
}

/// The refusal a VALUE past the published ceiling is reported with, naming the pair
/// it belongs to.
fn over_bound_value(name: &str, length: usize) -> String {
    format!(
        "the statusDetails value for {name} is {length} characters, over the \
         {MAX_STATUS_DETAIL_VALUE_LEN} character maximum"
    )
}

/// The refusal a NAME past the key shape's declared length is reported with.
fn over_bound_name(length: usize) -> String {
    format!(
        "a statusDetails name of {length} characters is over the \
         {MAX_STATUS_DETAIL_NAME_LEN} character maximum"
    )
}

/// The refusal a NAME outside the key shape's declared pattern is reported with.
fn inadmissible_name(name: &str) -> String {
    format!(
        "the statusDetails name {name} is not 1..{MAX_STATUS_DETAIL_NAME_LEN} characters \
         matching {STATUS_DETAIL_NAME_PATTERN}"
    )
}

/// The rows, spanning the offender count, the kinds of bound broken and the orderings
/// the names can stand in.
fn rows() -> Vec<Row> {
    let over_long_value = "v".repeat(MAX_STATUS_DETAIL_VALUE_LEN + 1);
    let over_value_len = MAX_STATUS_DETAIL_VALUE_LEN + 1;
    let just_over_long_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN + 1);
    let far_over_long_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN * 2);

    vec![
        Row {
            case: "two over-bound values",
            details: json!({"alpha": over_long_value, "beta": over_long_value}),
            expected: over_bound_value("alpha", over_value_len),
        },
        Row {
            case: "five over-bound values",
            details: json!({
                "gamma": over_long_value,
                "epsilon": over_long_value,
                "beta": over_long_value,
                "delta": over_long_value,
                "alpha": over_long_value,
            }),
            expected: over_bound_value("alpha", over_value_len),
        },
        Row {
            case: "over-bound values whose names differ only in case, ordered by byte",
            details: json!({"alpha": over_long_value, "Zebra": over_long_value}),
            expected: over_bound_value("Zebra", over_value_len),
        },
        Row {
            case: "over-bound values whose names differ in a digit, one a prefix of another",
            details: json!({
                "step-2": over_long_value,
                "step-10": over_long_value,
                "step-1": over_long_value,
            }),
            expected: over_bound_value("step-1", over_value_len),
        },
        Row {
            case: "one over-bound value among admissible pairs",
            details: json!({"aaa": "v", "mmm": over_long_value, "zzz": "v"}),
            expected: over_bound_value("mmm", over_value_len),
        },
        Row {
            case: "a name outside the pattern beside an over-bound value, the name sorting first",
            details: json!({"bad name": "v", "zz": over_long_value}),
            expected: inadmissible_name("bad name"),
        },
        Row {
            case: "an over-bound name beside an over-bound value, the value's name sorting first",
            details: json!({just_over_long_name.clone(): "v", "alpha": over_long_value}),
            expected: over_bound_value("alpha", over_value_len),
        },
        Row {
            case: "two over-bound names of different lengths",
            details: json!({
                far_over_long_name.clone(): "v",
                just_over_long_name.clone(): "v",
            }),
            expected: over_bound_name(MAX_STATUS_DETAIL_NAME_LEN + 1),
        },
        Row {
            case: "two names outside the pattern",
            details: json!({"step.name": "v", "étape": "v"}),
            expected: inadmissible_name("step.name"),
        },
    ]
}

/// RULE: a `statusDetails` map carrying more than one inadmissible pair is refused
/// with ONE message, decided from the request's own content — the first offending
/// pair in name order — so two identical requests, and the same request in two
/// processes, are always answered alike.
#[tokio::test]
async fn a_status_details_refusal_names_one_offender_whatever_the_map_order() {
    let emulator = Emulator::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            details,
            expected,
        } = row;
        // One thing per row, so a row's own queue holds exactly its own execution —
        // which every send below leaves exactly as it found it.
        let thing = format!("laser-offender-{index}");
        let id = emulator.start_command_on(&thing, None).await;

        for send in 1..=REPEATS {
            let (status, started) = emulator
                .start_next(&thing, json!({"statusDetails": details}))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} (send {send} of $next): expected 400, got {status} {started}"
            );
            assert_eq!(
                error_code(&started),
                MODELED_REFUSAL,
                "case {case:?} (send {send} of $next): expected {MODELED_REFUSAL}, got {started}"
            );
            assert_eq!(
                error_message(&started),
                expected,
                "case {case:?} (send {send} of $next): expected {expected:?}, got {:?}",
                error_message(&started)
            );

            let (status, updated) = emulator
                .update(
                    &thing,
                    &id,
                    json!({
                        "status": "IN_PROGRESS",
                        "statusDetails": details,
                        "includeJobExecutionState": true,
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} (send {send} of the update): expected 400, got {status} {updated}"
            );
            assert_eq!(
                error_code(&updated),
                MODELED_REFUSAL,
                "case {case:?} (send {send} of the update): expected {MODELED_REFUSAL}, got \
                 {updated}"
            );
            assert_eq!(
                error_message(&updated),
                expected,
                "case {case:?} (send {send} of the update): expected {expected:?}, got {:?}",
                error_message(&updated)
            );
        }

        // Every send was refused before anything was written, so the execution is
        // still the queued one the start left.
        let (status, described) = emulator.describe(&thing, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to be able to read the execution back, got {status} \
             {described}"
        );
        let execution = &described["execution"];
        assert_eq!(
            execution["status"], "QUEUED",
            "case {case:?}: expected the refused sends to change nothing, got {described}"
        );
        assert_eq!(
            execution["versionNumber"], 1,
            "case {case:?}: expected the refused sends to change nothing, got {described}"
        );
        assert!(
            execution["statusDetails"].is_null(),
            "case {case:?}: expected the refused sends to store no details, got {described}"
        );
    }
}

/// RULE: the offender a refusal names is a function of the request alone, so a map
/// holding the same pairs is refused alike however it was built — asserted here by
/// sending one row's pairs as several requests whose maps the service builds anew
/// each time, and requiring every answer to equal the answer to the first.
///
/// This is the invariant the row table above pins message by message, asserted once
/// more without naming an expected string: whatever the message is, there is only
/// one of it.
#[tokio::test]
async fn repeated_identical_requests_are_refused_with_one_message() {
    let emulator = Emulator::new();
    let thing = "laser-offender-repeat";
    let id = emulator.start_command_on(thing, None).await;

    // Ten offenders, so a map-order answer would agree with itself by chance about
    // once in 10^(REPEATS - 1) runs.
    let over_long_value = "v".repeat(MAX_STATUS_DETAIL_VALUE_LEN + 1);
    let details: Value = (0..10)
        .map(|index| (format!("detail-{index}"), Value::from(over_long_value.clone())))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    let mut first: Option<String> = None;
    for send in 1..=REPEATS {
        let (status, refused) = emulator
            .start_next(thing, json!({"statusDetails": details}))
            .await;
        assert_eq!(
            status, 400,
            "send {send}: expected 400, got {status} {refused}"
        );
        let message = error_message(&refused).to_string();
        match &first {
            None => first = Some(message),
            Some(expected) => assert_eq!(
                &message, expected,
                "send {send}: expected the same refusal as the first send ({expected:?}), got \
                 {message:?}"
            ),
        }
    }

    let (status, described) = emulator.describe(thing, &id, "").await;
    assert_eq!(
        status, 200,
        "expected to be able to read the execution back, got {status} {described}"
    );
    assert_eq!(
        described["execution"]["status"], "QUEUED",
        "expected the refused sends to change nothing, got {described}"
    );
}
