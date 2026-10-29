// SPDX-License-Identifier: CC0-1.0

//! Which modelled error each refusal of the command-execution surface carries, and
//! where this plane's 404 actually belongs.
//!
//! RULE: `StartCommandExecution` refuses every request it declines on the caller's
//! side with the modelled `ValidationException` (HTTP 400) and a message built from
//! that refusal's own rule — including a start whose `commandArn` names no command
//! that exists, whose values are otherwise all well formed. The 404
//! `ResourceNotFoundException` this plane also models answers the operations that
//! address ONE execution: a `jobId`/`thingName` pair the service holds no execution
//! for.
//!
//! The evidence for the start's 400 is the recorded service, not the declared error
//! set: the wire recording `iotjobsdata-command-execution-validation-refusals`
//! answers a start naming an unregistered command 400, and the behavioural recording
//! `iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected`
//! states that such a start is rejected `ValidationException` and not with the
//! declared not-found error. A declared error is a bound on what an operation MAY
//! answer, not a trigger; this suite pins which of the two it does answer, so a later
//! change cannot silently move a start's refusal to 404 or a single-execution read's
//! to 400.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a client sees, and every refusal is followed
//! by a read of the target's queue: a refused start persists nothing whichever rule
//! refused it.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::iot_commands::CommandFacts;

use crate::support::{error_code, error_message, Emulator};

/// A command that exists and admits any start: created with a payload template and
/// never deprecated.
const TEMPLATED: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// A command that exists in another partition and region — a command is identified by
/// its WHOLE ARN, so this is a different command from [`TEMPLATED`].
const TEMPLATED_ELSEWHERE: &str = "arn:aws-cn:iot:cn-north-1:444455556666:command/install_firmware";

/// A command that exists with a STATIC payload: it admits a start, but none carrying
/// `parameters`.
const STATIC_PAYLOAD: &str = "arn:aws:iot:us-east-1:111122223333:command/static-report";

/// A command that exists and has been DEPRECATED: still readable, but no new execution
/// may be created from it.
const DEPRECATED: &str = "arn:aws:iot:us-east-1:111122223333:command/retired-dim";

/// A thing ARN for `thing_name`, the one target form this operation accepts.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}")
}

/// Why a row's start is refused — each refusal has its own rule and so its own
/// message, which is what makes two rejections distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The `commandArn` names no command that exists.
    CommandMissing,
    /// The `commandArn` names no command at all.
    NotACommandArn,
    /// The `targetArn` names no device this plane can target.
    NotATarget,
    /// The command exists and the control plane deprecated it.
    Deprecated,
    /// The command exists and its payload is static, so it admits no `parameters`.
    StaticPayload,
}

/// One start, and the answer the rule says it gets.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The thing the start targets; a distinct one per row, so the queue read after a
    /// refusal sees only this row's writes.
    thing: &'static str,
    /// Sent verbatim as `targetArn` when the row tests a target the service cannot
    /// read, instead of the thing ARN built from `thing`.
    raw_target_arn: Option<&'static str>,
    command_arn: &'static str,
    /// The `parameters` the start carries, as JSON; empty when it carries none.
    /// (`CommandExecutionParameterMap` is modelled `len 1..`, so an empty map is
    /// refused by the constraint before the handler and is not one of these rows.)
    parameters: &'static str,
    /// The rule that refuses the row, or `None` for a start the service serves.
    refusal: Option<Refusal>,
}

/// RULE: every caller-side refusal of a start is the modelled `ValidationException`
/// (HTTP 400) — a `commandArn` naming no existing command as much as one naming no
/// command at all, an untargetable `targetArn`, a deprecated command or `parameters`
/// against a static payload — each with the message its own rule builds, and none of
/// them answers the 404 the operation also declares.
///
/// The rows span what the rule generalises over: for the absent command, every way an
/// ARN can fail to name a registered one (an id nobody created, a registered id in
/// another region, account or partition, an id that only prefixes one, and an id that
/// is deprecated or static-payload SOMEWHERE ELSE — absence is decided before either
/// fact); for the other refusals, the kind of fault (a value naming no command or no
/// target, and a command that admits no such start); and both accept forms, with and
/// without parameters. The messages of all refused rows must be pairwise distinct.
#[tokio::test]
async fn every_refused_start_is_the_modelled_validation_error() {
    let emulator = Emulator::with_commands(&[
        (TEMPLATED, CommandFacts::default()),
        (TEMPLATED_ELSEWHERE, CommandFacts::default()),
        (
            STATIC_PAYLOAD,
            CommandFacts {
                static_payload: true,
                deprecated: false,
            },
        ),
        (
            DEPRECATED,
            CommandFacts {
                static_payload: false,
                deprecated: true,
            },
        ),
    ]);

    let rows: [Row; 15] = [
        Row {
            case: "a command id nobody created",
            thing: "laser-missing-1",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/no-such-command",
            parameters: "",
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a registered id in another region",
            thing: "laser-missing-2",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:eu-west-1:111122223333:command/reboot",
            parameters: "",
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a registered id in another account",
            thing: "laser-missing-3",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:us-east-1:999988887777:command/reboot",
            parameters: "",
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a registered id in another partition",
            thing: "laser-missing-4",
            raw_target_arn: None,
            command_arn: "arn:aws-cn:iot:cn-north-1:444455556666:command/reboot",
            parameters: "",
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "an id that only prefixes a registered one",
            thing: "laser-missing-5",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/reboo",
            parameters: "",
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a deprecated command's id in another region",
            thing: "laser-missing-6",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:eu-west-1:111122223333:command/retired-dim",
            parameters: "",
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a static-payload command's id in another account, started with parameters",
            thing: "laser-missing-7",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:us-east-1:999988887777:command/static-report",
            parameters: r#"{ "level": { "S": "high" } }"#,
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a missing command, started with parameters of several types",
            thing: "laser-missing-8",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/absent_42",
            parameters: r#"{ "level": { "S": "high" }, "retries": { "I": 2 }, "dry": { "B": true } }"#,
            refusal: Some(Refusal::CommandMissing),
        },
        Row {
            case: "a commandArn that is not an ARN at all",
            thing: "laser-invalid-1",
            raw_target_arn: None,
            command_arn: "reboot",
            parameters: "",
            refusal: Some(Refusal::NotACommandArn),
        },
        Row {
            case: "a commandArn naming another IoT resource type",
            thing: "laser-invalid-2",
            raw_target_arn: None,
            command_arn: "arn:aws:iot:us-east-1:111122223333:job/upgrade-42",
            parameters: "",
            refusal: Some(Refusal::NotACommandArn),
        },
        Row {
            case: "a targetArn naming another IoT resource type",
            thing: "laser-invalid-3",
            raw_target_arn: Some("arn:aws:iot:us-east-1:111122223333:job/upgrade-42"),
            command_arn: TEMPLATED,
            parameters: "",
            refusal: Some(Refusal::NotATarget),
        },
        Row {
            case: "a command the control plane deprecated",
            thing: "laser-invalid-4",
            raw_target_arn: None,
            command_arn: DEPRECATED,
            parameters: "",
            refusal: Some(Refusal::Deprecated),
        },
        Row {
            case: "parameters against a command whose payload is static",
            thing: "laser-invalid-5",
            raw_target_arn: None,
            command_arn: STATIC_PAYLOAD,
            parameters: r#"{ "level": { "S": "high" } }"#,
            refusal: Some(Refusal::StaticPayload),
        },
        Row {
            case: "a registered command, started with no parameters",
            thing: "laser-served-1",
            raw_target_arn: None,
            command_arn: TEMPLATED,
            parameters: "",
            refusal: None,
        },
        Row {
            case: "a registered command in another partition and region, started with parameters",
            thing: "laser-served-2",
            raw_target_arn: None,
            command_arn: TEMPLATED_ELSEWHERE,
            parameters: r#"{ "level": { "S": "high" } }"#,
            refusal: None,
        },
    ];

    // One message per refusal RULE, to check the five rules are distinguishable. Two
    // rows of the SAME rule may share a message (the message names the command, and
    // two ARNs can name the same command id in two regions), so the representative is
    // the first row of each rule.
    let mut by_rule: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    for row in rows {
        let Row {
            case,
            thing,
            raw_target_arn,
            command_arn,
            parameters,
            refusal,
        } = row;
        let target_arn = raw_target_arn.map_or_else(|| thing_arn(thing), ToString::to_string);
        let command_id = command_arn.rsplit('/').next().unwrap_or_default();
        let parameters: Option<Value> = (!parameters.is_empty()).then(|| {
            serde_json::from_str(parameters).unwrap_or_else(|why| {
                panic!("case {case:?}: the row's parameters must be JSON: {why}")
            })
        });
        let (status, body) = emulator
            .start_command(&target_arn, command_arn, parameters.as_ref())
            .await;

        let Some(refusal) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the start to be served, got {status} {body}"
            );
            let execution_id = body["executionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an executionId, got {body}"))
                .to_string();
            // Round trip: the execution the accept minted is readable back, and its
            // job document names the command it was started for.
            let (status, described) = emulator
                .describe(thing, &execution_id, "?includeJobDocument=true")
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the started execution to be readable, got \
                 {status} {described}"
            );
            let document: Value = serde_json::from_str(
                described["execution"]["jobDocument"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no jobDocument in {described}")),
            )
            .unwrap_or_else(|why| panic!("case {case:?}: the job document must be JSON: {why}"));
            assert_eq!(
                document["commandArn"],
                json!(command_arn),
                "case {case:?}: the job document names the command, got {document}"
            );
            continue;
        };

        // EVERY caller-side refusal of a start is the 400, whichever rule refused it:
        // the operation's declared 404 is not an answer this operation gives.
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: every refused start is the modelled validation error, got \
             {status} {body}"
        );
        // …each with the message its own rule builds, from the request where the rule
        // has anything of the request to read back.
        let expected = match refusal {
            Refusal::CommandMissing => format!("The command {command_id} was not found."),
            Refusal::NotACommandArn => "commandArn must name a command, as \
                 arn:<partition>:iot:<region>:<account>:command/<name>"
                .to_string(),
            Refusal::NotATarget => {
                let resource_type = target_arn
                    .split(':')
                    .next_back()
                    .and_then(|resource| resource.split('/').next())
                    .unwrap_or_default();
                format!("Resource type {resource_type} not supported as a target")
            }
            Refusal::Deprecated => format!("The command {command_id} has been deprecated."),
            Refusal::StaticPayload => {
                "The parameters option is unsupported for commands with static payloads."
                    .to_string()
            }
        };
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the message this refusal's own rule builds, got {body}"
        );
        by_rule
            .entry(format!("{refusal:?}"))
            .or_insert_with(|| error_message(&body).to_string());

        // A refused start persists NOTHING: the target keeps an empty queue.
        let (status, queue) = emulator.pending(thing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the queue read, got {status} {queue}"
        );
        assert_eq!(
            queue["queuedJobs"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused start must queue nothing, got {queue}"
        );
    }

    let distinct: std::collections::BTreeSet<&String> = by_rule.values().collect();
    assert_eq!(
        distinct.len(),
        by_rule.len(),
        "each refusal rule of this operation must be distinguishable by its message, got \
         {by_rule:?}"
    );

    // The OTHER half of the boundary, on the same emulator: the 404 this plane models
    // is the answer to a lookup of ONE execution the service holds nothing for, so the
    // 400s above are a property of the start and not of a service that never answers
    // 404. The class of that lookup is asserted in `job_execution_lifecycle.rs`
    // (`unknown_executions_are_404_but_a_thing_with_none_still_reports`) and in rule 4
    // of `command_execution_validation.rs`; this is only the contrast.
    let (status, body) = emulator.describe("laser-missing-1", "no-such-execution", "").await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a single-execution read with no match\": expected the modelled not-found \
         error, got {status} {body}"
    );
}
