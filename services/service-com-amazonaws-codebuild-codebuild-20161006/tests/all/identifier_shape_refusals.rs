// SPDX-License-Identifier: CC0-1.0

//! Cover for the SHAPE of the identifiers CodeBuild takes as plain strings, driven over
//! the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test, for each kind of identifier: a value that cannot name a resource
//! of that kind at all is refused with the modelled `InvalidInputException` BEFORE
//! anything is looked up, while a well-formed identifier this account holds nothing for
//! is answered from the store — an empty listing, or an entry in the not-found half of a
//! batch read. The two absent-input classes have to stay distinguishable: a client that
//! sent a malformed value must be told to fix it rather than told the resource is simply
//! not there.
//!
//! Each table spans the class rather than one value: every way a name can carry a
//! character a name may not, and every way an identifier can lack the structure its kind
//! has, against the well-formed values that must still be answered.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use super::project_build_lifecycle::{call, create_project, emulator, error_type, REGION};

/// The `message` an error body carries.
///
/// `pub(crate)`: the suites that assert on a refusal's wording share this reader rather
/// than each growing a copy of it.
pub(crate) fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Every class of character the name patterns of this service exclude, each in a name of
/// its own: a path separator, a space, punctuation, an ARN separator, a dot, a plus sign,
/// a non-ASCII letter, whitespace alone, and a name that is one illegal character.
///
/// None of these is a character `FleetName` (`^[A-Za-z0-9][A-Za-z0-9\-_]{1,127}$`) or
/// `ProjectName` (`^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$`) allows — the two patterns admit
/// the same set — so ONE table serves every suite that drives an unwritable name, and the
/// rows cannot drift apart between them. `pub(crate)` for that reason;
/// `name_character_refusals` drives the same rows through the operations that CREATE a
/// resource.
pub(crate) const UNSUPPORTED_NAMES: [(&str, &str); 9] = [
    ("a slash", "slash/name"),
    ("a space", "not a name"),
    ("an exclamation mark", "name!!"),
    ("a colon", "name:one"),
    ("a dot", "name.one"),
    ("a plus sign", "name+one"),
    ("a non-ASCII letter", "náme"),
    ("whitespace alone", "\t"),
    ("a star", "*"),
];

/// RULE: a compute-fleet identifier is either an ARN or a fleet NAME, and a fleet name is
/// written with the characters the model's `FleetName` pattern allows
/// (`^[A-Za-z0-9][A-Za-z0-9\-_]{1,127}$` — ASCII letters and digits, dash, underscore);
/// a value carrying anything else names no fleet, so `BatchGetFleets` refuses it with
/// `InvalidInputException` instead of reporting an empty result. A value whose characters
/// are all allowed is an identifier whatever its length, which the store answers.
#[tokio::test]
async fn batch_get_fleets_refuses_a_name_that_is_not_a_fleet_name() {
    let router = emulator();

    // Every class of character the fleet-name pattern excludes, each tried in a name of
    // its own — the table every suite of this crate drives an unwritable name from. None
    // of these is a character CreateFleet would have accepted in a name either, which is
    // what `name_character_refusals` asserts over the same rows.
    for (case, name) in UNSUPPORTED_NAMES {
        let (status, body) = call(&router, "BatchGetFleets", json!({"names": [name]})).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} with {body}");
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            message(&body),
            state::INVALID_FLEET_NAME_MESSAGE,
            "case {case:?}: expected CodeBuild's invalid-fleet-name sentence, got {body}"
        );
        assert!(
            !message(&body).contains(name),
            "case {case:?}: the sentence must not echo the value, got {body}"
        );
    }

    // One malformed entry refuses the whole request, whatever it is mixed with.
    let (status, body) = call(
        &router,
        "BatchGetFleets",
        json!({"names": ["well-formed", "not a fleet"]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a mixed request: expected 400, got {status} with {body}"
    );

    // The values that ARE identifiers stay answered from the store: a name nothing was
    // created under, and ARNs of every shape the resolver can be handed.
    let accepted = [
        ("a name nothing was created under", "no-such-fleet".to_string()),
        ("a two-character name", "ab".to_string()),
        ("a one-character name", "a".to_string()),
        (
            "a name longer than a fleet name may be",
            "f".repeat(300),
        ),
        ("a name of dashes and underscores", "a-_0".to_string()),
        (
            "a fleet ARN nothing was created under",
            state::fleet_arn(REGION, "no-such-fleet:0001"),
        ),
        (
            "an ARN of another resource type",
            state::project_arn(REGION, "no-such-fleet"),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::bucket/key".to_string(),
        ),
    ];
    for (case, identifier) in accepted {
        let (status, body) = call(
            &router,
            "BatchGetFleets",
            json!({"names": [identifier.clone()]}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {body}");
        assert_eq!(
            body["fleets"],
            json!([]),
            "case {case:?}: nothing resolves, so fleets must be [], got {body}"
        );
    }
}

/// RULE: a sandbox identifier is either a sandbox ARN or the structured
/// `<projectName>:<id>` pair a sandbox id is, so `ListCommandExecutionsForSandbox`
/// refuses a bare, unstructured value with `InvalidInputException` before it lists
/// anything — while a well-formed id this account holds no sandbox for is a successful,
/// empty listing.
#[tokio::test]
async fn list_command_executions_for_sandbox_refuses_an_id_that_is_not_a_sandbox_id() {
    let router = emulator();
    create_project(&router, "cmd-shape", json!({})).await;

    // Every way an identifier can lack a sandbox id's structure: no separator at all, a
    // half of the pair missing, nothing but the separator, and an ARN whose resource
    // segment names something other than a sandbox.
    let refused = [
        ("a bare name", "cmd-shape".to_string()),
        ("a bare uuid", "00000000-0000-8000-8000-000000000000".to_string()),
        ("a project with no id", "cmd-shape:".to_string()),
        ("an id with no project", ":0001".to_string()),
        ("the separator alone", ":".to_string()),
        (
            "a project ARN rather than a sandbox ARN",
            state::project_arn(REGION, "cmd-shape"),
        ),
        (
            "a build ARN rather than a sandbox ARN",
            state::build_arn(REGION, "cmd-shape:0001"),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::bucket/key".to_string(),
        ),
    ];
    for (case, sandbox_id) in refused {
        let (status, body) = call(
            &router,
            "ListCommandExecutionsForSandbox",
            json!({"sandboxId": sandbox_id}),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} with {body}");
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            message(&body),
            state::INVALID_SANDBOX_ID_MESSAGE,
            "case {case:?}: expected CodeBuild's invalid-sandbox-id sentence, got {body}"
        );
        assert!(
            !message(&body).contains(&sandbox_id),
            "case {case:?}: the sentence must not echo the value, got {body}"
        );
    }

    // The opposite class: well-SHAPED identifiers no sandbox of this account answers to
    // still list nothing, successfully.
    let absent_id = "cmd-shape:00000000-0000-8000-8000-000000000000";
    let accepted = [
        ("an id nothing started", absent_id.to_string()),
        (
            "the ARN of an id nothing started",
            state::sandbox_arn(REGION, absent_id),
        ),
        (
            "an id of a project that does not exist either",
            "no-such-project:0001".to_string(),
        ),
    ];
    for (case, sandbox_id) in accepted {
        let (status, body) = call(
            &router,
            "ListCommandExecutionsForSandbox",
            json!({"sandboxId": sandbox_id}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {body}");
        assert_eq!(
            body["commandExecutions"],
            json!([]),
            "case {case:?}: a sandbox holding nothing lists nothing, got {body}"
        );
    }
}
