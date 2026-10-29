// SPDX-License-Identifier: CC0-1.0

//! What `StartCommandExecution` admits as the NAME of a `parameters` pair.
//!
//! The rule under test: a map's KEY carries its own declared constraints, and a pair
//! is admitted only when its name satisfies them — here `CommandParameterName`'s
//! `length 1..192` and `^[.$a-zA-Z0-9_-]+$`, the key shape of the
//! `CommandExecutionParameterMap` the member targets
//! (`smithy_query(query="shape", shape="CommandExecutionParameterMap,CommandParameterName")`).
//! A name outside them is refused 400 `ValidationException` and nothing is persisted;
//! an admitted name is persisted and read back out of the job document the device
//! later fetches.
//!
//! The refusal is the HANDLER's rather than the framework's for the same reason the
//! `statusDetails` name's is (`status_details_modeled_constraints.rs`): the constraint
//! walk ahead of a handler treats a map's key as a path segment and not as a
//! constrained leaf, so it reports nothing about a name.
//!
//! The table spans the dimensions the rule generalises over: the name's LENGTH (the
//! one-character minimum, inside the declared maximum, the maximum itself, one
//! character past it and far past it), the character CLASSES the pattern admits
//! (letters, digits, `.`, `$`, `_`, `-`) and classes it does not (a space, a slash, a
//! colon — which the sibling name shapes of this family DO admit — other punctuation,
//! a newline, a non-ASCII character), and the COUNT of pairs a map carries (one, or
//! several of which one is inadmissible). The reported case — a name carrying a space
//! accepted — is one row of the class dimension.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    COMMAND_PARAMETER_NAME_PATTERN, MAX_COMMAND_PARAMETER_NAME_LEN,
};

use crate::support::{error_code, Emulator};

/// The modelled client error this operation refuses a request it cannot serve with —
/// the same one the framework's pre-handler walk answers a reported member's constraint
/// violation with, which is why the refusal below reads like one of those.
const MODELED_REFUSAL: &str = "ValidationException";

/// The message a start carrying a name the map does not admit is refused with: the
/// envelope over a clause naming the MEMBER, stating no value (a map has no scalar
/// rendering) and listing what a KEY of this map has to satisfy. Spelled out here, from
/// the constants the key shape declares, so the suite pins the wire text a client reads
/// rather than re-asserting whatever the service built.
fn expected_refusal() -> String {
    format!(
        "1 validation error detected: Value at 'parameters' failed to satisfy constraint: Map \
         keys must satisfy constraint: [Member must have length less than or equal to \
         {MAX_COMMAND_PARAMETER_NAME_LEN}, Member must have length greater than or equal to 1, \
         Member must satisfy regular expression pattern: {COMMAND_PARAMETER_NAME_PATTERN}]"
    )
}

/// One row: the case, the single parameter name the start carries, and whether the
/// map admits it.
struct Row {
    case: String,
    name: String,
    admitted: bool,
}

impl Row {
    fn new(case: &str, name: &str, admitted: bool) -> Self {
        Self {
            case: case.to_string(),
            name: name.to_string(),
            admitted,
        }
    }
}

/// The rows, spanning the name's length and the character classes the pattern decides.
fn rows() -> Vec<Row> {
    let inside_maximum = "n".repeat(MAX_COMMAND_PARAMETER_NAME_LEN - 1);
    let at_maximum = "n".repeat(MAX_COMMAND_PARAMETER_NAME_LEN);
    let past_maximum = "n".repeat(MAX_COMMAND_PARAMETER_NAME_LEN + 1);
    let far_past_maximum = "n".repeat(MAX_COMMAND_PARAMETER_NAME_LEN * 2);

    vec![
        // The length dimension.
        Row::new("one character, the declared minimum", "s", true),
        Row::new("one character inside the maximum", &inside_maximum, true),
        Row::new("the declared maximum length", &at_maximum, true),
        Row::new("one character past the maximum", &past_maximum, false),
        Row::new("twice the maximum", &far_past_maximum, false),
        Row::new("the empty name, under the declared minimum", "", false),
        // The character-class dimension: what the pattern admits.
        Row::new("letters and digits", "level2", true),
        Row::new("a dotted placeholder path", "config.level.max", true),
        Row::new("a dollar-prefixed name", "$version", true),
        Row::new("an underscore and a hyphen", "dry_run-mode", true),
        Row::new(
            "every character class the pattern admits at once",
            "A.b$1_c-9",
            true,
        ),
        // The character-class dimension: what it does not.
        Row::new("a space", "bad name", false),
        Row::new("a slash", "config/level", false),
        Row::new(
            "a colon, which the thing-name and statusDetails shapes admit and this one does not",
            "config:level",
            false,
        ),
        Row::new("punctuation outside the set", "level!", false),
        Row::new("a newline", "level\nmore", false),
        Row::new("a non-ASCII letter", "niveau\u{e9}", false),
        Row::new("a non-ASCII symbol", "level\u{2713}", false),
    ]
}

/// The `parameters` value every row carries: the name is what varies, and a value's
/// own declared constraints are a different rule (the framework's walk reports those).
fn value() -> Value {
    json!({ "S": "high" })
}

/// RULE: a `parameters` pair is admitted only when its NAME satisfies the constraints
/// declared for the map's KEY (`^[.$a-zA-Z0-9_-]+$`, 1..`MAX_COMMAND_PARAMETER_NAME_LEN`
/// characters); a start carrying any other name is refused 400 `ValidationException`,
/// mints no `executionId` and persists nothing, while an admitted name is persisted
/// into the job document the device fetches.
#[tokio::test]
async fn a_command_parameter_name_is_admitted_only_within_its_declared_key_constraint() {
    let emulator = Emulator::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            name,
            admitted,
        } = row;
        // One thing per row, so a row's own queue holds exactly what its own start
        // created.
        let thing = format!("laser-parameter-{index}");
        let parameters = json!({ name.clone(): value() });
        let (status, body) = emulator
            .start_command_with_parameters(&thing, &parameters)
            .await;

        if !admitted {
            assert_eq!(
                status, 400,
                "case {case:?} (a name of {} characters): expected the start refused 400, got \
                 {status} {body}",
                name.chars().count()
            );
            assert_eq!(
                error_code(&body),
                MODELED_REFUSAL,
                "case {case:?}: expected {MODELED_REFUSAL}, got {body}"
            );
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                expected_refusal(),
                "case {case:?}: expected the refusal to state what a key of this map must \
                 satisfy, got {body}"
            );
            assert!(
                body["executionId"].is_null(),
                "case {case:?}: a refused start mints no executionId, got {body}"
            );
            // Nothing was persisted: the device's queue for this thing is empty.
            let (status, queue) = emulator.pending(&thing).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected to be able to poll the queue, got {status} {queue}"
            );
            assert_eq!(
                queue["queuedJobs"],
                json!([]),
                "case {case:?}: a refused start enqueues nothing, got {queue}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?} (a name of {} characters): expected the start served, got {status} \
             {body}",
            name.chars().count()
        );
        let execution_id = body["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a served start mints an executionId, got {body}"))
            .to_string();

        // Read the execution back the way the device does: the admitted name is in the
        // persisted job document, carrying the value it was started with.
        let (status, described) = emulator
            .describe(&thing, &execution_id, "?includeJobDocument=true")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to read the execution back, got {status} {described}"
        );
        let document: Value = described["execution"]["jobDocument"]
            .as_str()
            .map(|document| {
                serde_json::from_str(document).unwrap_or_else(|why| {
                    panic!("case {case:?}: the job document must be JSON: {why}")
                })
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected a job document, got {described}"));
        assert_eq!(
            document["parameters"], parameters,
            "case {case:?}: expected the admitted parameters in the stored document, got \
             {document}"
        );
    }
}

/// RULE, over the COUNT dimension: the names of a map are each admitted on their own,
/// so a map carrying several admissible names is served whole, and ONE name the key
/// shape does not admit refuses the start whatever else the map carries — nothing is
/// persisted and no pair of that map reaches the job document.
#[tokio::test]
async fn one_inadmissible_name_refuses_a_start_whatever_else_the_map_carries() {
    let emulator = Emulator::new();

    let over_long = "n".repeat(MAX_COMMAND_PARAMETER_NAME_LEN + 1);

    // (case, the map the start carries, admitted)
    let rows: Vec<(String, Value, bool)> = vec![
        (
            "three admissible names of three modelled value types".to_string(),
            json!({
                "level": { "S": "high" },
                "retries": { "I": 2 },
                "dry-run": { "B": true },
            }),
            true,
        ),
        (
            "an inadmissible name beside two admissible ones".to_string(),
            json!({
                "level": { "S": "high" },
                "bad name": { "S": "x" },
                "retries": { "I": 2 },
            }),
            false,
        ),
        (
            "an over-long name beside an admissible one".to_string(),
            json!({ "level": { "S": "high" }, over_long.clone(): { "S": "x" } }),
            false,
        ),
        (
            "two inadmissible names and nothing else".to_string(),
            json!({ "bad name": { "S": "x" }, "worse/name": { "S": "y" } }),
            false,
        ),
    ];

    for (index, (case, parameters, admitted)) in rows.into_iter().enumerate() {
        let thing = format!("laser-parameter-map-{index}");
        let (status, body) = emulator
            .start_command_with_parameters(&thing, &parameters)
            .await;
        let expected = if admitted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );

        if !admitted {
            assert_eq!(
                (error_code(&body), body["message"].as_str().unwrap_or_default()),
                (MODELED_REFUSAL, expected_refusal().as_str()),
                "case {case:?}: expected the map-key refusal, got {body}"
            );
            let (status, queue) = emulator.pending(&thing).await;
            assert_eq!(
                (status, &queue["queuedJobs"]),
                (200, &json!([])),
                "case {case:?}: a refused start enqueues nothing, got {queue}"
            );
            continue;
        }

        let execution_id = body["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a served start mints an executionId, got {body}"))
            .to_string();
        let (status, described) = emulator
            .describe(&thing, &execution_id, "?includeJobDocument=true")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected to read the execution back, got {status} {described}"
        );
        let document: Value = described["execution"]["jobDocument"]
            .as_str()
            .map(|document| {
                serde_json::from_str(document).unwrap_or_else(|why| {
                    panic!("case {case:?}: the job document must be JSON: {why}")
                })
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected a job document, got {described}"));
        assert_eq!(
            document["parameters"], parameters,
            "case {case:?}: expected every admitted pair in the stored document, got {document}"
        );
    }
}
