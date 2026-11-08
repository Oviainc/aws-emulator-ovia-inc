// SPDX-License-Identifier: CC0-1.0

//! Cover for how CodeBuild WORDS a resource name carrying a character it does not allow,
//! driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: a name whose characters fall outside the set the model's pattern
//! for that kind of name allows (`ProjectName`, `FleetName` — ASCII letters and digits,
//! dash and underscore) is refused with 400 `InvalidInputException` stating CodeBuild's
//! own sentence for that resource kind, which names the supported characters and
//! interpolates nothing: not the submitted value, not the member it came in on, not the
//! model's pattern. It holds wherever the service takes such a name — the operation that
//! creates the resource, where the model constrains the member, and the batch read, where
//! it does not — so one illegal name is refused with one sentence however it arrives.
//!
//! The pattern's OTHER clauses (how long a name may be, and its leading character) are a
//! different refusal: the sentence below does not describe them, so a name that breaks
//! only those keeps the shared constraint envelope, which is what the last table pins.
//!
//! Each table spans the class rather than one value: it drives every way a name can carry
//! a character a name may not — the rows `identifier_shape_refusals::UNSUPPORTED_NAMES`
//! holds, shared with the batch-read suite so the two cannot drift apart — against the
//! control values that must still be answered.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use super::fleet_lifecycle::create_fleet_body;
use super::identifier_shape_refusals::{message, UNSUPPORTED_NAMES};
use super::project_build_lifecycle::{call, create_project_body, emulator, error_type, REGION};

/// Assert that `body` is CodeBuild's own sentence for a name it cannot be written with,
/// and that the sentence leaks nothing of the request or of the model.
fn assert_name_sentence(case: &str, status: u16, body: &Value, expected: &str, name: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_type(body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );
    let stated = message(body);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected CodeBuild's own invalid-name sentence, got {stated:?}"
    );
    assert!(
        !stated.contains(name),
        "case {case:?}: the sentence must not echo the submitted value, got {stated:?}"
    );
    assert!(
        !stated.contains("'name'") && !stated.contains("constraint"),
        "case {case:?}: the sentence must not name the member or the constraint, got \
         {stated:?}"
    );
    assert!(
        !stated.contains("[A-Za-z0-9]"),
        "case {case:?}: the sentence must not leak the model's pattern, got {stated:?}"
    );
}

/// RULE: `CreateProject` refuses a name carrying a character a project name may not be
/// written with by stating CodeBuild's project-name sentence, and persists nothing for it.
#[tokio::test]
async fn create_project_states_codebuilds_invalid_project_name_sentence() {
    let router = emulator();

    for (case, name) in UNSUPPORTED_NAMES {
        let (status, body) =
            call(&router, "CreateProject", create_project_body(name, json!({}))).await;
        assert_name_sentence(
            case,
            status,
            &body,
            state::INVALID_PROJECT_NAME_MESSAGE,
            name,
        );

        // A refused create stores nothing, so the rejected name is in no listing.
        let (status, listed) = call(&router, "ListProjects", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListProjects expected 200, got {status} with {listed}"
        );
        assert!(
            !listed["projects"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|project| project.as_str() == Some(name)),
            "case {case:?}: the refused name must not be listed, got {listed}"
        );
    }

    // The control: the same names with every unsupported character replaced by a dash
    // are created and read back, so each row above was decided by the character and not
    // by the rest of the name.
    for (row, (case, name)) in UNSUPPORTED_NAMES.into_iter().enumerate() {
        let supported: String = name
            .chars()
            .map(|character| {
                if state::is_supported_name_character(character) {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        // Each row's control carries its own position, so two rows whose replacement
        // lands on the same text are still two projects.
        let supported = format!("ok{row}{supported}");
        let (status, body) = call(
            &router,
            "CreateProject",
            create_project_body(&supported, json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {supported:?} is a project name, got {status} with {body}"
        );
        assert_eq!(
            body["project"]["name"],
            json!(supported),
            "case {case:?}: the created project reports the name it was created under, got \
             {body}"
        );

        // Read back through the implemented reader: the create persisted a project, not
        // just an answer.
        let (status, read) = call(
            &router,
            "BatchGetProjects",
            json!({"names": [supported.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetProjects expected 200, got {status} with {read}"
        );
        assert_eq!(
            read["projects"][0]["name"],
            json!(supported),
            "case {case:?}: the stored project reads back, got {read}"
        );
        assert_eq!(
            read["projectsNotFound"],
            json!([]),
            "case {case:?}: a held project is not reported absent, got {read}"
        );
    }
}

/// RULE: the sentence is keyed to the model's PATTERN, not to one member or one
/// operation, so a project name refused on a member the model constrains with
/// `ProjectName` under another name — the `projectName` the webhook operations carry —
/// states the same sentence as the create does.
#[tokio::test]
async fn a_project_name_member_of_another_operation_states_the_same_sentence() {
    let router = emulator();

    for (case, name) in UNSUPPORTED_NAMES {
        for operation in ["CreateWebhook", "UpdateWebhook", "DeleteWebhook"] {
            let (status, body) =
                call(&router, operation, json!({"projectName": name})).await;
            let case = format!("{operation} with {case}");
            assert_name_sentence(
                &case,
                status,
                &body,
                state::INVALID_PROJECT_NAME_MESSAGE,
                name,
            );
        }
    }
}

/// RULE: `BatchGetProjects` holds a name to the same character rule and answers the
/// IDENTICAL refusal — a value that cannot be a project name is not a project this
/// account happens not to hold, so it is refused rather than reported under
/// `projectsNotFound`.
#[tokio::test]
async fn batch_get_projects_refuses_a_name_that_is_not_a_project_name() {
    let router = emulator();

    for (case, name) in UNSUPPORTED_NAMES {
        let (status, body) = call(&router, "BatchGetProjects", json!({"names": [name]})).await;
        assert_name_sentence(
            case,
            status,
            &body,
            state::INVALID_PROJECT_NAME_MESSAGE,
            name,
        );
        assert!(
            body["projectsNotFound"].is_null(),
            "case {case:?}: a refusal reports no projectsNotFound, got {body}"
        );
    }

    // One malformed entry refuses the whole request, whatever it is mixed with.
    let (status, body) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["well-formed", "not a name"]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a mixed request: expected 400, got {status} with {body}"
    );

    // The opposite class: values that CAN name a project — including the ARN form the
    // member documents, whose own separators are not project-name characters — stay
    // answered from the store, reported under `projectsNotFound` exactly as sent.
    let accepted = [
        ("a name nothing was created under", "no-such-project".to_string()),
        ("a one-character name", "a".to_string()),
        ("a name of dashes and underscores", "a-_0".to_string()),
        (
            "a name longer than a project name may be",
            "p".repeat(300),
        ),
        (
            "a project ARN nothing was created under",
            state::project_arn(REGION, "no-such-project"),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::bucket/key".to_string(),
        ),
    ];
    for (case, identifier) in accepted {
        let (status, body) = call(
            &router,
            "BatchGetProjects",
            json!({"names": [identifier.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["projects"],
            json!([]),
            "case {case:?}: nothing resolves, so projects must be [], got {body}"
        );
        assert_eq!(
            body["projectsNotFound"],
            json!([identifier]),
            "case {case:?}: the identifier is reported absent exactly as sent, got {body}"
        );
    }
}

/// RULE: the sentence is stated per RESOURCE KIND, from the pattern the model puts on
/// that kind's name — so a compute fleet's name is refused with CodeBuild's fleet-name
/// sentence by the operation that creates one, the same way the batch read already
/// refuses it.
#[tokio::test]
async fn create_fleet_states_codebuilds_invalid_fleet_name_sentence() {
    let router = emulator();

    for (case, name) in UNSUPPORTED_NAMES {
        let (status, body) =
            call(&router, "CreateFleet", create_fleet_body(name, json!({}))).await;
        assert_name_sentence(case, status, &body, state::INVALID_FLEET_NAME_MESSAGE, name);
    }

    // The control: a fleet name written with the supported characters is created.
    let (status, body) = call(
        &router,
        "CreateFleet",
        create_fleet_body("cbnv-fleet_0", json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "a supported fleet name: expected 200, got {status} with {body}"
    );
}

/// RULE: the character sentence states the character SET and nothing else, so a name that
/// breaks another clause of the same pattern — how short or long it may be, or its leading
/// character — is refused as the input-constraint violation it is, not as a name carrying
/// an unsupported character. That holds however long the name is: a refusal carries the
/// rejected value ALREADY elided for rendering, and an elision brings characters of its
/// own, so a very long name written entirely with supported characters must not be read as
/// one carrying unsupported ones.
#[tokio::test]
async fn a_name_breaking_another_clause_is_not_the_character_refusal() {
    let router = emulator();

    // Each published bound of `ProjectName`'s 2..=150 length, one past each bound, the
    // leading-character clause, and lengths far past the point where a refused value is
    // rendered whole (the framework elides a value over 256 bytes into a prefix plus a
    // byte count, which carries a dot, a space and brackets).
    let other_clauses = [
        ("one character, under the minimum", "a".to_string()),
        ("one character past the maximum", "p".repeat(151)),
        ("longer than a rendered value is kept whole", "p".repeat(257)),
        ("far longer than that", "p".repeat(4_096)),
        (
            "long, and written with every supported character",
            "aZ9-_".repeat(200),
        ),
        ("a leading dash", "-leading-dash".to_string()),
        ("a leading underscore", "_leading_underscore".to_string()),
    ];
    for (case, name) in other_clauses {
        let (status, body) = call(
            &router,
            "CreateProject",
            create_project_body(&name, json!({})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_ne!(
            message(&body),
            state::INVALID_PROJECT_NAME_MESSAGE,
            "case {case:?}: the characters are all supported, so this is not the \
             character refusal, got {body}"
        );
        assert_ne!(
            message(&body),
            state::INVALID_FLEET_NAME_MESSAGE,
            "case {case:?}: a project's refusal never states another resource's \
             sentence, got {body}"
        );
        // What it DOES state is the shared constraint envelope, which names the member
        // and the constraint that was broken.
        assert!(
            message(&body).contains("constraint"),
            "case {case:?}: expected the shared constraint envelope, got {body}"
        );
    }

    // The same clauses on the other resource the model names by pattern, including the
    // lengths whose rejected value is elided before it is rendered.
    for (case, name) in [
        ("a fleet name under the minimum", "a".to_string()),
        ("a fleet name past the maximum", "f".repeat(129)),
        (
            "a fleet name longer than a rendered value is kept whole",
            "f".repeat(257),
        ),
        ("a fleet name with a leading dash", "-leading-dash".to_string()),
    ] {
        let (status, body) =
            call(&router, "CreateFleet", create_fleet_body(&name, json!({}))).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_ne!(
            message(&body),
            state::INVALID_FLEET_NAME_MESSAGE,
            "case {case:?}: the characters are all supported, so this is not the \
             character refusal, got {body}"
        );
    }

    // Both published bounds of the same length clause ARE names: 2 characters and 150.
    for (case, name) in [
        ("the shortest name the model allows", "ab".to_string()),
        ("the longest name the model allows", "p".repeat(150)),
    ] {
        let (status, body) = call(
            &router,
            "CreateProject",
            create_project_body(&name, json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
    }
}
