// SPDX-License-Identifier: CC0-1.0

//! What the two device operations that accept `statusDetails` admit into a job
//! execution's details map.
//!
//! The rule under test: a map's KEY and its VALUE each carry their own declared
//! constraints, and each side of a supplied pair is admitted only when it satisfies
//! ITS OWN — a name within `DetailsKey`'s `length 1..128` and `^[a-zA-Z0-9:_-]+$`, a
//! value within `DetailsValue`'s `length min 1` and `^[^\p{C}]+$` plus the published
//! 1,024-character ceiling on a value (which the value shape itself does not
//! declare). Those are the constraints of the shapes the member targets, read with
//! `smithy_query(query="shape", shape="DetailsMap,DetailsKey,DetailsValue")`. A pair
//! outside them is refused 400 and changes nothing; an admitted map is persisted and
//! read back.
//!
//! Which 400 a row gets is a consequence of WHERE the rule can be applied, and the
//! rows state it: the constraint walk the framework runs ahead of every handler
//! treats a map's key as a path segment rather than as a constrained leaf, so it
//! reports a bad VALUE in its own `ValidationException` envelope, while a bad NAME
//! reaches the handler and is refused with the operation's modelled
//! `InvalidRequestException` — as is the published ceiling on a value, which no
//! declared constraint carries.
//!
//! The table spans both sides of a pair and, on each side, the dimensions the rule
//! generalises over: length (the one-character minimum, the declared maximum, one
//! character past it), the character classes each pattern admits and classes it does
//! not (a space, punctuation outside the set, a non-ASCII letter, a control
//! character, a format character), and the number of pairs a map carries. The
//! recorded case — a 1,024-character name accepted — is one row of the length
//! dimension, now refused.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a device agent sees.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    MAX_STATUS_DETAIL_NAME_LEN, MAX_STATUS_DETAIL_VALUE_LEN,
};

use crate::support::{error_code, Emulator};

/// The operation's own modelled client error, which a handler refusal carries.
const MODELED_REFUSAL: &str = "InvalidRequestException";
/// The envelope the framework's pre-handler constraint walk refuses in.
const WALKED_REFUSAL: &str = "ValidationException";

/// One row: the `statusDetails` map a request carries, and either the wire error code
/// the pair is refused with or `None` for a pair the details admit.
struct Row {
    case: String,
    details: Value,
    refusal: Option<&'static str>,
}

impl Row {
    fn admitted(case: &str, details: Value) -> Self {
        Self {
            case: case.to_string(),
            details,
            refusal: None,
        }
    }

    fn refused(case: &str, details: Value, refusal: &'static str) -> Self {
        Self {
            case: case.to_string(),
            details,
            refusal: Some(refusal),
        }
    }
}

/// A map of one pair, so a row can carry a name or a value it computed.
fn pair(name: &str, value: &str) -> Value {
    json!({ name: value })
}

/// The rows, spanning the name side and the value side of the rule.
fn rows() -> Vec<Row> {
    let longest_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN);
    let over_long_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN + 1);
    let longest_value = "v".repeat(MAX_STATUS_DETAIL_VALUE_LEN);
    let over_long_value = "v".repeat(MAX_STATUS_DETAIL_VALUE_LEN + 1);

    vec![
        // The name side, admitted.
        Row::admitted(
            "a one-character name and a one-character value, both at the declared minimum",
            pair("s", "v"),
        ),
        Row::admitted(
            "a name at the declared maximum length",
            pair(&longest_name, "v"),
        ),
        Row::admitted(
            "a name using every character class the name pattern admits",
            pair("Step:1_a-B9", "v"),
        ),
        // The value side, admitted: the value pattern excludes only Unicode's
        // "Other" category, so spaces, punctuation, letters of any script and
        // symbols are all admitted.
        Row::admitted(
            "a value carrying spaces and punctuation",
            pair("step", "downloading 50% (2/3)"),
        ),
        Row::admitted(
            "a value of non-ASCII letters and a symbol",
            pair("step", "téléchargement ✅"),
        ),
        Row::admitted(
            "a value at the published maximum length",
            pair("step", &longest_value),
        ),
        Row::admitted(
            "several admissible pairs at once",
            json!({"stage": "download", "attempt-2": "retrying", "host:name": "laser"}),
        ),
        // The name side, refused: the handler's, since the walk reports no key.
        Row::refused(
            "a name one character past the declared maximum",
            pair(&over_long_name, "v"),
            MODELED_REFUSAL,
        ),
        Row::refused(
            "a name carrying a space",
            pair("step name", "v"),
            MODELED_REFUSAL,
        ),
        Row::refused(
            "a name carrying punctuation outside the name pattern",
            pair("step.name", "v"),
            MODELED_REFUSAL,
        ),
        Row::refused(
            "a name carrying a non-ASCII letter",
            pair("étape", "v"),
            MODELED_REFUSAL,
        ),
        Row::refused("an empty name", pair("", "v"), MODELED_REFUSAL),
        Row::refused(
            "one over-long name among admissible pairs",
            json!({"stage": "download", over_long_name.clone(): "v"}),
            MODELED_REFUSAL,
        ),
        // The value side, refused: the value shape's own declared length and
        // pattern, which the framework's walk reports.
        Row::refused("an empty value", pair("step", ""), WALKED_REFUSAL),
        Row::refused(
            "a value carrying a control character",
            pair("step", "down\nload"),
            WALKED_REFUSAL,
        ),
        Row::refused(
            "a value carrying a zero-width space, a format character of the same excluded category",
            pair("step", "down\u{200b}load"),
            WALKED_REFUSAL,
        ),
        // The published ceiling on a value carries no declared constraint, so it is
        // the handler that holds it.
        Row::refused(
            "a value one character past the published maximum",
            pair("step", &over_long_value),
            MODELED_REFUSAL,
        ),
    ]
}

/// RULE: a `statusDetails` pair is admitted only when its NAME satisfies the
/// constraints declared for the map's key (`^[a-zA-Z0-9:_-]+$`, within
/// `MAX_STATUS_DETAIL_NAME_LEN` characters) and its VALUE those declared for the
/// map's value (`^[^\p{C}]+$`, non-empty, within the published
/// `MAX_STATUS_DETAIL_VALUE_LEN` characters); a map carrying any other pair is
/// refused 400 and the execution is left exactly as it was.
///
/// Asserted on BOTH operations that accept the member —
/// `StartNextPendingJobExecution` and `UpdateJobExecution` — since the map a request
/// carries is the map the record would hold either way.
#[tokio::test]
async fn a_status_details_pair_is_admitted_only_within_its_own_declared_constraints() {
    let emulator = Emulator::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            details,
            refusal,
        } = row;
        // One thing per row, so a row's own queue holds exactly its own execution.
        let thing = format!("laser-details-{index}");
        let id = emulator.start_command_on(&thing, None).await;

        // `StartNextPendingJobExecution`: the map is applied as the queued execution
        // is started.
        let (status, started) = emulator
            .start_next(&thing, json!({"statusDetails": details}))
            .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from $next, got {status} {started}"
                );
                assert_eq!(
                    started["execution"]["statusDetails"], details,
                    "case {case:?}: expected $next to hand back the details it stored, got {started}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 from $next, got {status} {started}"
                );
                assert_eq!(
                    error_code(&started), expected,
                    "case {case:?}: expected {expected} from $next, got {started}"
                );
                assert!(
                    started["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("statusDetails")),
                    "case {case:?}: expected the refusal to name the member at fault, got {started}"
                );
            }
        }

        // `UpdateJobExecution`: the same map through the device's own update.
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
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from the update, got {status} {updated}"
                );
                assert_eq!(
                    updated["executionState"]["statusDetails"], details,
                    "case {case:?}: expected the update to report the details it stored, got {updated}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 from the update, got {status} {updated}"
                );
                assert_eq!(
                    error_code(&updated), expected,
                    "case {case:?}: expected {expected} from the update, got {updated}"
                );
            }
        }

        // What the execution now holds: an admitted map is persisted and read back,
        // and a refused one left the execution exactly as the start found it.
        let (status, described) = emulator.describe(&thing, &id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to be able to read the execution back, got {status} {described}"
        );
        let execution = &described["execution"];
        if refusal.is_none() {
            assert_eq!(
                execution["statusDetails"], details,
                "case {case:?}: expected the stored details read back, got {described}"
            );
            assert_eq!(
                execution["status"], "IN_PROGRESS",
                "case {case:?}: expected the admitted calls to have started the execution, got {described}"
            );
            continue;
        }

        assert!(
            execution["statusDetails"].is_null(),
            "case {case:?}: expected the refused calls to store no details, got {described}"
        );
        assert_eq!(
            execution["status"], "QUEUED",
            "case {case:?}: expected the refused calls to change nothing, got {described}"
        );
        assert_eq!(
            execution["versionNumber"], 1,
            "case {case:?}: expected the refused calls to change nothing, got {described}"
        );

        // A refusal leaves the execution usable: the same request with an admissible
        // map is accepted afterwards.
        let (status, recovered) = emulator
            .update(
                &thing,
                &id,
                json!({
                    "status": "IN_PROGRESS",
                    "statusDetails": {"stage": "download"},
                    "includeJobExecutionState": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an admissible map accepted after the refusal, got {status} {recovered}"
        );
        assert_eq!(
            recovered["executionState"]["statusDetails"],
            json!({"stage": "download"}),
            "case {case:?}: expected the admissible map stored after the refusal, got {recovered}"
        );
    }
}

/// RULE: the bound on a NAME is the one declared for the map's KEY, so a name at
/// `MAX_STATUS_DETAIL_NAME_LEN` is admitted and every longer name is refused —
/// including the names that the ceiling published for a VALUE
/// (`MAX_STATUS_DETAIL_VALUE_LEN`) would admit, which is the eightfold gap between
/// the two bounds this test pins shut.
///
/// The rows walk the name's length to the boundary and then past it, up to the
/// ceiling published for a value and beyond.
#[tokio::test]
async fn a_status_details_name_is_bounded_by_the_key_constraint_and_not_by_the_value_ceiling() {
    let emulator = Emulator::new();

    // (case, the name's length, admitted)
    let lengths: &[(&str, usize, bool)] = &[
        ("the one-character minimum", 1, true),
        (
            "one character inside the maximum",
            MAX_STATUS_DETAIL_NAME_LEN - 1,
            true,
        ),
        ("the declared maximum", MAX_STATUS_DETAIL_NAME_LEN, true),
        (
            "one character past the maximum",
            MAX_STATUS_DETAIL_NAME_LEN + 1,
            false,
        ),
        ("twice the maximum", MAX_STATUS_DETAIL_NAME_LEN * 2, false),
        (
            "the ceiling published for a value",
            MAX_STATUS_DETAIL_VALUE_LEN,
            false,
        ),
        (
            "past the ceiling published for a value",
            MAX_STATUS_DETAIL_VALUE_LEN + 1,
            false,
        ),
    ];

    for (case, length, admitted) in lengths {
        let thing = format!("laser-name-{length}");
        let id = emulator.start_command_on(&thing, None).await;
        let details = json!({"n".repeat(*length): "v"});
        let (status, answered) = emulator
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
        let expected = if *admitted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?} (a name of {length} characters): expected {expected}, got {status} {answered}"
        );
        if *admitted {
            assert_eq!(
                answered["executionState"]["statusDetails"], details,
                "case {case:?} (a name of {length} characters): expected the details stored, got {answered}"
            );
        } else {
            assert_eq!(
                error_code(&answered),
                MODELED_REFUSAL,
                "case {case:?} (a name of {length} characters): expected {MODELED_REFUSAL}, got {answered}"
            );
        }
    }
}
