// SPDX-License-Identifier: CC0-1.0

//! Daemon task-definition references: what the FAMILY part of a reference may spell, and
//! what an operation that resolves one answers when it spells something else.
//!
//! The rule under test, asserted over a table that spans the class rather than over one
//! request: every form a reference takes — "The family for the latest ACTIVE revision,
//! family and revision (family:revision) for a specific revision in the family, or full
//! Amazon Resource Name (ARN) of the daemon task definition" — carries a FAMILY, and a
//! family is "Up to 255 letters (uppercase and lowercase), numbers, underscores, and
//! hyphens". A reference whose family part is outside that character set names no family
//! at all, so the operation resolving it refuses it as INVALID INPUT — the modelled
//! invalid-parameter error, in that operation's own wording — and leaves the store
//! untouched.
//!
//! A reference that IS well-formed but names no stored revision is a different answer, and
//! the suites of each operation (`daemon_task_definitions`,
//! `daemon_task_definition_deletion`) assert that one; here the control is only that the
//! stored revision survives every refused call.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::json;

use crate::ecs_wire::{ok, refusal, service, string, ACCOUNT, SIGNED_REGION};

/// How the READ states the refusal: the pattern the family part has to match.
const READ_MESSAGE: &str =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::DAEMON_TASK_DEFINITION_FAMILY_PATTERN_MESSAGE;

/// How the DELETE states it: the identifier the call acts on is not one.
const DELETE_MESSAGE: &str =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::DAEMON_TASK_DEFINITION_INVALID_MESSAGE;

/// The family the one stored revision is registered under, and so the family part every
/// row below breaks in some way.
const FAMILY: &str = "daemon-reference-family";

/// The image the stored revision's container starts from; irrelevant to the rule.
const IMAGE: &str = "public.ecr.aws/amazonlinux/amazonlinux:latest";

/// The references whose FAMILY part is outside the documented character set: a character
/// the set does not hold, in each position a family can carry one; a value with no family
/// at all; and the same breakages inside the `family:revision` form and inside a full ARN,
/// so the rule is shown to hold whichever form the reference takes.
fn malformed_family_references() -> Vec<(&'static str, String)> {
    let arn = |family: &str| {
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-task-definition/{family}:1")
    };
    vec![
        (
            "a family with a space in it",
            "not a valid family".to_string(),
        ),
        ("a family of punctuation", "@@@".to_string()),
        (
            "a family carrying a character the set does not hold",
            "daemon@family".to_string(),
        ),
        ("a family with a leading dot", ".daemonfamily".to_string()),
        ("a family with a trailing slash", "daemonfamily/".to_string()),
        ("a family that is only a space", " ".to_string()),
        ("no family at all", String::new()),
        (
            "a malformed family naming a revision",
            "daemon family:1".to_string(),
        ),
        (
            "a malformed family naming every revision",
            "daemon family:*".to_string(),
        ),
        ("a malformed family inside an ARN", arn("daemon family")),
        ("an empty family inside an ARN", arn("")),
    ]
}

#[tokio::test]
async fn a_reference_whose_family_is_no_family_name_is_refused_as_invalid_input() {
    let router = service();

    // One stored revision, so every refusal below is shown to be about the reference and
    // not about an empty store — and so the control at the end has something to read.
    let case = "the stored revision";
    let registered = ok(
        &router,
        "RegisterDaemonTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [{ "name": "agent", "image": IMAGE, "memory": 128 }],
        }),
        case,
    )
    .await;
    let stored_arn = string(&registered, "daemonTaskDefinitionArn", case);

    for (case, reference) in malformed_family_references() {
        // Both operations that take the reference as the resource they act on: the read
        // and the delete, each with the wording it states the refusal in.
        for (operation, expected) in [
            ("DescribeDaemonTaskDefinition", READ_MESSAGE),
            ("DeleteDaemonTaskDefinition", DELETE_MESSAGE),
        ] {
            let (code, message) = refusal(
                &router,
                operation,
                json!({ "daemonTaskDefinition": reference.clone() }),
                case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected {operation} to refuse {reference:?} as invalid \
                 input, got {code} with {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {operation} to answer {expected:?} for \
                 {reference:?}, got {message:?}"
            );
        }
    }

    // The control: every refused call left the store alone, so the one stored revision is
    // still readable and still ACTIVE.
    let case = "the control read";
    let described = ok(
        &router,
        "DescribeDaemonTaskDefinition",
        json!({ "daemonTaskDefinition": FAMILY }),
        case,
    )
    .await;
    let view = &described["daemonTaskDefinition"];
    assert_eq!(
        string(view, "daemonTaskDefinitionArn", case),
        stored_arn,
        "expected the stored revision to be untouched, got {view}"
    );
    assert_eq!(
        string(view, "status", case),
        "ACTIVE",
        "expected the stored revision to be untouched, got {view}"
    );
}
