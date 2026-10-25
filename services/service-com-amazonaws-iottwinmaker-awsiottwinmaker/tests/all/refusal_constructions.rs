// SPDX-License-Identifier: CC0-1.0

//! How this service CONSTRUCTS its refusals, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! Two RULES, each asserted over a table rather than one recorded case:
//!
//! * a create whose id is already taken is refused 409 `ConflictException` with
//!   a message built from the request — the kind of resource and the id first,
//!   then the workspace that holds it where the resource lives inside one, then
//!   "already exists.";
//! * a `queryStatement` carrying a run of characters the query language's lexer
//!   cannot recognise as a token is refused 400 `ValidationException` naming the
//!   position of that token as a 1-BASED line and column and quoting the text
//!   the lexer consumed for it.

use serde_json::{json, Value};

use crate::harness::{workspace_arn, Emulator, Resp};

/// The workspace the cases are created in.
const WORKSPACE: &str = "conflictWs";

/// The message an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message").as_str().unwrap_or_default().to_string()
}

/// RULE: a create whose id is already taken in its scope is refused 409
/// `ConflictException`, in the service's own construction — the resource and its
/// id, the workspace that holds it where there is one, then "already exists.".
///
/// The rows differ along the resource kind and along whether the resource nests
/// under a workspace at all, because that is what the construction generalises
/// over.
#[tokio::test]
async fn a_taken_id_is_refused_in_the_services_conflict_construction() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    // The resource each row re-creates has to exist first.
    let component_type = json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}});
    let entity = json!({"entityId": "takenEntity", "entityName": "Taken"});
    let scene = json!({"sceneId": "takenScene", "contentLocation": "s3://bucket/taken.json"});
    let job = json!({
        "metadataTransferJobId": "takenJob",
        "sources": [{
            "type": "s3",
            "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/taken.json"},
        }],
        "destination": {
            "type": "iottwinmaker",
            "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
        },
    });
    for (case, status) in [
        (
            "component type",
            emulator
                .create_component_type(WORKSPACE, "com.example.taken", component_type.clone())
                .await
                .status,
        ),
        (
            "entity",
            emulator.create_entity(WORKSPACE, entity.clone()).await.status,
        ),
        (
            "scene",
            emulator.create_scene(WORKSPACE, scene.clone()).await.status,
        ),
        (
            "metadata transfer job",
            emulator.create_metadata_transfer_job(job.clone()).await.status,
        ),
    ] {
        assert_eq!(
            status, 200,
            "case {case:?}: the resource the row re-creates must be created first"
        );
    }

    // (case, the refused second create, the message AWS builds for it). A
    // second create differing from the first in its other members is still the
    // same conflict, which is why the rows change them.
    let rows: Vec<(&str, Resp, &str)> = vec![
        (
            "workspace",
            emulator.create_workspace(WORKSPACE).await,
            "Workspace conflictWs already exists.",
        ),
        (
            "component type",
            emulator
                .create_component_type(
                    WORKSPACE,
                    "com.example.taken",
                    json!({"propertyDefinitions": {"Other": {"dataType": {"type": "STRING"}}}}),
                )
                .await,
            "Component type com.example.taken in workspace conflictWs already exists.",
        ),
        (
            "entity",
            emulator
                .create_entity(
                    WORKSPACE,
                    json!({"entityId": "takenEntity", "entityName": "A different name"}),
                )
                .await,
            "Entity takenEntity in workspace conflictWs already exists.",
        ),
        (
            "scene",
            emulator
                .create_scene(
                    WORKSPACE,
                    json!({
                        "sceneId": "takenScene",
                        "contentLocation": "s3://bucket/other.json",
                        "description": "a different scene",
                    }),
                )
                .await,
            "Scene takenScene in workspace conflictWs already exists.",
        ),
        (
            "metadata transfer job",
            emulator
                .create_metadata_transfer_job(json!({
                    "metadataTransferJobId": "takenJob",
                    "sources": [{"type": "iotsitewise"}],
                    "destination": {
                        "type": "s3",
                        "s3Configuration": {
                            "location": "arn:aws:s3:::metadata-bucket-other",
                        },
                    },
                }))
                .await,
            "MetadataTransferJob takenJob already exists.",
        ),
    ];

    for (case, refused, expected) in &rows {
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected a 409 ConflictException, got {refused:?}"
        );
        assert_eq!(
            message(refused),
            *expected,
            "case {case:?}: expected the conflict construction, got {refused:?}"
        );
    }
}

/// RULE: a `queryStatement` the lexer cannot tokenize is refused 400
/// `ValidationException`, naming the offending token's position as a 1-based
/// line and column of the statement and quoting the text consumed for it.
///
/// The rows differ along where in the statement the token sits (first
/// character, mid-line, on a later line), along how wide it is, and along
/// whether the statement is one line or several — the dimensions a line and a
/// column range over.
#[tokio::test]
async fn an_unrecognisable_token_is_refused_with_its_1_based_line_and_column() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the queries are run against must exist"
    );

    // (case, statement, line, column, the quoted token).
    let rows: Vec<(&str, &str, usize, usize, &str)> = vec![
        // The recorded statement: the `!` at offset 20 is column 21, and the
        // lexer had consumed it and the character that failed to complete it.
        ("recorded", "THIS IS NOT PARTIQL !!! ###", 1, 21, "!!"),
        ("firstCharacter", "### SELECT", 1, 1, "#"),
        ("midLine", "SELECT !x FROM EntityGraph", 1, 8, "!x"),
        ("lastCharacter", "SELECT entity #", 1, 15, "#"),
        (
            "secondLineStart",
            "SELECT entity\n@ FROM EntityGraph",
            2,
            1,
            "@",
        ),
        (
            "thirdLineMidLine",
            "SELECT entity\nFROM EntityGraph\nMATCH (entity) ###",
            3,
            16,
            "#",
        ),
    ];

    for (case, statement, line, column, token) in &rows {
        let refused = emulator
            .execute_query(json!({"workspaceId": WORKSPACE, "queryStatement": statement}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "Invalid QueryStatement: at line {line}, column {column}: invalid token, \
                 token recognition error at: '{token}'"
            ),
            "case {case:?}: expected the position and the offending token, got {refused:?}"
        );
    }

    // A statement whose characters all tokenize but whose grammar does not hold
    // is the same refusal under the same prefix — the statement is what could
    // not be used — and a well-formed statement is still served.
    let unparsable = emulator
        .execute_query(json!({"workspaceId": WORKSPACE, "queryStatement": "SELECT FROM"}))
        .await;
    assert_eq!(
        (unparsable.status, unparsable.error_type.as_deref()),
        (400, Some("ValidationException")),
        "a statement the grammar rejects must be a 400 ValidationException, got {unparsable:?}"
    );
    assert!(
        message(&unparsable).starts_with("Invalid QueryStatement: "),
        "a refusal about the statement must name it, got {unparsable:?}"
    );

    let served = emulator
        .execute_query(json!({
            "workspaceId": WORKSPACE,
            "queryStatement": "SELECT entity FROM EntityGraph MATCH (entity)",
        }))
        .await;
    assert_eq!(
        served.status, 200,
        "a well-formed statement must still be served, got {served:?}"
    );
    assert_eq!(
        served.member("rows"),
        &json!([]),
        "the empty workspace selects no row, got {served:?}"
    );
}

/// The members a `CreateEntity` refusal must not have written: nothing.
///
/// Kept beside the construction suites because the conflict rows above assert
/// what a refused create ANSWERS, and this one asserts that it also stored
/// nothing — the two halves of one refusal.
#[tokio::test]
async fn a_conflicting_create_leaves_the_stored_resource_alone() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace(WORKSPACE).await.status, 200);
    let first: Value = json!({"entityId": "kept", "entityName": "The first name"});
    assert_eq!(
        emulator.create_entity(WORKSPACE, first.clone()).await.status,
        200,
        "the entity the second create conflicts with must exist"
    );
    let again = emulator
        .create_entity(
            WORKSPACE,
            json!({"entityId": "kept", "entityName": "The second name"}),
        )
        .await;
    assert_eq!(
        again.status, 409,
        "a taken entityId must conflict, got {again:?}"
    );
    let read = emulator.get_entity(WORKSPACE, "kept").await;
    assert_eq!(
        read.member("entityName").as_str(),
        first["entityName"].as_str(),
        "the refused create must not have rewritten the entity, got {read:?}"
    );
}
