// SPDX-License-Identifier: CC0-1.0

//! How a start whose command does not exist is READ BACK in its refusal.
//!
//! RULE: the refusal of a start whose `commandArn` names no command that exists is
//! built FROM THE REQUEST — it names the command the supplied `commandArn` named —
//! so two rejections of two absent commands are distinguishable rather than one fixed
//! template, and nothing is persisted for the target device. The status and error
//! class are the recorded service's (400 `ValidationException`, pinned as a class in
//! `command_execution_refusal_errors.rs`); this suite is about the message.
//!
//! It is the counterpart of `command_execution_validation.rs`'s deprecation rule: the
//! two refusals a command's own state produces each read their command back, and
//! neither depends on the FORM of the command id.

use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::iot_commands::CommandFacts;

use crate::support::{error_code, error_message, Emulator};

/// A thing ARN for `thing_name`, the one target form this operation accepts.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}")
}

/// RULE: a `commandArn` naming no existing command is refused with a message that
/// names that command, and the refusal persists nothing; the same id, registered, is
/// served — so the refusal is about the command's ABSENCE and not about the form of
/// its id or of its ARN.
///
/// The rows differ in what the rule generalises over: the form of the command id
/// (hyphens, underscores, digits, mixed case, a single character and a 64-character
/// id — `CommandId` is modelled `len 1..64, ^[a-zA-Z0-9_-]+$`), and the partition,
/// region and account of the ARN the id sits in. None of these changes the rule, and
/// each must yield its own message.
#[tokio::test]
async fn a_missing_command_is_read_back_in_the_refusal() {
    /// One absent command, and the start the table makes against it.
    struct Row {
        case: &'static str,
        thing: &'static str,
        command_arn: &'static str,
    }

    let rows: [Row; 7] = [
        Row {
            case: "a hyphenated id",
            thing: "laser-absent-1",
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/reboot-now",
        },
        Row {
            case: "an id with underscores and digits",
            thing: "laser-absent-2",
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/install_firmware_42",
        },
        Row {
            case: "a mixed-case id",
            thing: "laser-absent-3",
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/RebootLaser",
        },
        Row {
            case: "a single-character id",
            thing: "laser-absent-4",
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/r",
        },
        Row {
            case: "a 64-character id, the modelled maximum",
            thing: "laser-absent-5",
            command_arn: "arn:aws:iot:us-east-1:111122223333:command/\
                          c0123456789012345678901234567890123456789012345678901234567890123",
        },
        Row {
            case: "an id in another partition and region",
            thing: "laser-absent-6",
            command_arn: "arn:aws-cn:iot:cn-north-1:444455556666:command/reboot-cn",
        },
        Row {
            case: "an id in another account",
            thing: "laser-absent-7",
            command_arn: "arn:aws-us-gov:iot:us-gov-west-1:999988887777:command/reboot-gov",
        },
    ];

    /// A command that EXISTS, whose id has the same form as the first row's: the
    /// refusals above are about absence, so this one is served.
    const REGISTERED: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot-later";

    let emulator = Emulator::with_commands(&[(REGISTERED, CommandFacts::default())]);

    let mut reported: Vec<String> = Vec::new();
    for Row {
        case,
        thing,
        command_arn,
    } in rows
    {
        let command_id = command_arn.rsplit('/').next().unwrap_or_default();
        let (status, body) = emulator
            .start_command(&thing_arn(thing), command_arn, None)
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: expected the refusal the recorded service answers a start naming \
             no existing command, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The command {command_id} was not found."),
            "case {case:?}: the refusal must name the command the request named, got {body}"
        );
        assert!(
            reported.iter().all(|seen| seen != error_message(&body)),
            "case {case:?}: two refusals of two different commands must be distinguishable, \
             got {:?} again among {reported:?}",
            error_message(&body)
        );
        reported.push(error_message(&body).to_string());

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

    let (status, body) = emulator
        .start_command(&thing_arn("laser-absent-registered"), REGISTERED, None)
        .await;
    assert_eq!(
        status, 200,
        "case \"the command that exists\": expected the start to be served, got {status} {body}"
    );
}
