// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `EntityNotFound` refusal every connection function operation answers
//! with, end to end over HTTP.
//!
//! The rule under test: a CloudFront operation that cannot find the connection function a
//! request addresses refuses it with the fixed per-code sentence for that resource kind —
//! the whole message is service text, so no value taken from the request (the identifier,
//! the stage, the version token) and no value taken from the store appears in it, and the
//! terminating period is part of it.
//!
//! The rows differ along everything that rule generalises over:
//!
//! * the OPERATION: the describe, the code read, the delete, the publish and the listing of
//!   the distributions a connection function serves — every operation of this surface whose
//!   model names `EntityNotFound` and whose handler resolves a connection function;
//! * the IDENTIFIER NAMESPACE the request addressed the resource in: a minted-shaped id
//!   that was never allocated, a name nothing was created under, and the id, the name and —
//!   where the operation takes the identifier as a query parameter rather than as a path
//!   label, which no ARN can travel in — the ARN of a connection function that really
//!   existed and was deleted;
//! * the shape and the size of the identifier: one character, the longest name the model
//!   admits, and a value in no namespace this service mints;
//! * the STAGE: none at all, `DEVELOPMENT`, `LIVE`, and a stage a connection function this
//!   service DOES hold was never published to — the one row where the resource is there and
//!   the stage is not;
//! * the version token a mutation carried: the one the dead connection function really
//!   stood at, and a well-formed token nothing ever held.
//!
//! One row per operation reads a connection function that IS there, so the table cannot
//! pass by refusing everything.

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_code::get_connection_function_refusal;
use crate::connection_function_delete::delete_connection_function;
use crate::connection_function_lifecycle::{
    create_connection_function, describe_connection_function, member, Create,
};
use crate::connection_function_publish::publish_connection_function;
use crate::distribution_lifecycle::{emulator, Answer};
use crate::distributions_by_connection_function::list_by_function;

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The wire error code every row expects (`EntityNotFound`, HTTP 404 — the code the model
/// names for this condition on each operation below).
const ENTITY_NOT_FOUND: &str = "EntityNotFound";

/// The whole sentence CloudFront refuses a connection function it does not hold with,
/// spelled out here so a wrong constant in the crate cannot make these assertions pass.
const NOT_FOUND_SENTENCE: &str = "The specified connection function does not exist.";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no connection function in a fresh emulator holds.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

/// A connection function body, small enough that the create is about the identity it mints
/// rather than about the code.
const CODE: &str = "function handler(event) { return event.request; }\n";

/// Which operation a row drives, with the stage the reads name.
#[derive(Debug, Clone, Copy)]
enum Op {
    /// `DescribeConnectionFunction`, naming `Stage` when it is `Some`.
    Describe(Option<&'static str>),
    /// `GetConnectionFunction` — the code read — naming `Stage` when it is `Some`.
    ReadCode(Option<&'static str>),
    /// `DeleteConnectionFunction`, carrying the version token the row names.
    Delete,
    /// `PublishConnectionFunction`, carrying the version token the row names.
    Publish,
    /// `ListDistributionsByConnectionFunction`.
    ListDistributions,
}

/// The stage name a row's request carries, or `""` when it names none: the token an
/// assertion checks the refusal does not echo.
fn named_stage(op: Op) -> &'static str {
    match op {
        Op::Describe(stage) | Op::ReadCode(stage) => stage.unwrap_or_default(),
        _ => "",
    }
}

/// Drive `op` against `identifier`, carrying `token` as the `If-Match` of the mutations.
async fn refuse(
    router: &MultiServiceRouter,
    op: Op,
    identifier: &str,
    token: &str,
) -> Answer {
    match op {
        Op::Describe(stage) => describe_connection_function(router, identifier, stage).await,
        Op::ReadCode(stage) => get_connection_function_refusal(router, identifier, stage).await,
        Op::Delete => delete_connection_function(router, identifier, token).await,
        Op::Publish => publish_connection_function(router, identifier, token).await,
        Op::ListDistributions => list_by_function(router, identifier, "").await,
    }
}

/// The create every row of this suite starts from, under the name it is given.
fn create_named(case: &'static str, name: &str) -> Create {
    Create {
        case,
        name: name.to_string(),
        comment: "refusal wording".to_string(),
        runtime: "cloudfront-js-2.0",
        code: CODE.as_bytes().to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    }
}

/// What one create minted.
struct Created {
    id: String,
    name: String,
    arn: String,
    etag: String,
}

/// Create one connection function and answer the identity and the version token it minted.
async fn create_one(router: &MultiServiceRouter, create: &Create) -> Created {
    let answer = create_connection_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        create.case, answer.status, answer.raw
    );
    let root = answer.root();
    Created {
        id: member(root, "Id", create.case, &answer.raw),
        name: create.name.clone(),
        arn: member(root, "ConnectionFunctionArn", create.case, &answer.raw),
        etag: answer.header("etag").to_string(),
    }
}

/// Every operation of this surface refuses a connection function it cannot find with the
/// SAME fixed sentence, whatever identifier namespace, identifier shape, stage or version
/// token the request carried — and the sentence echoes none of them.
#[tokio::test]
async fn every_connection_function_refusal_carries_the_fixed_sentence() {
    let router = emulator();

    // One connection function that stays: the rows that name its LIVE stage are refused for
    // the stage rather than for the identifier, and the control reads are served from it.
    let held = create_one(&router, &create_named("the connection function that stays", "held_fn")).await;

    // One connection function that really existed and was then deleted, so its id, its name
    // and its ARN are identifiers the store once answered and no longer does.
    let dead = create_one(&router, &create_named("the connection function that is deleted", "dead_fn")).await;
    let deleted = delete_connection_function(&router, &dead.id, &dead.etag).await;
    assert_eq!(
        deleted.status, 204,
        "the delete that makes the identifiers dead must answer HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );

    // (case, operation, identifier, If-Match token)
    let rows: Vec<(&str, Op, String, &str)> = vec![
        (
            "describe, a minted-shaped id never allocated, no stage named",
            Op::Describe(None),
            "cf_0000000000000".to_string(),
            "",
        ),
        (
            "describe, the dead id, the DEVELOPMENT stage",
            Op::Describe(Some(STAGE_DEVELOPMENT)),
            dead.id.clone(),
            "",
        ),
        (
            "describe, the dead name, the LIVE stage",
            Op::Describe(Some(STAGE_LIVE)),
            dead.name.clone(),
            "",
        ),
        (
            "describe, the dead name, no stage named",
            Op::Describe(None),
            dead.name.clone(),
            "",
        ),
        (
            "describe, a held connection function in a stage it was never published to",
            Op::Describe(Some(STAGE_LIVE)),
            held.id.clone(),
            "",
        ),
        (
            "describe, the longest name the model admits, never created",
            Op::Describe(Some(STAGE_DEVELOPMENT)),
            "z".repeat(64),
            "",
        ),
        (
            "describe, a one-character name never created",
            Op::Describe(None),
            "q".to_string(),
            "",
        ),
        (
            "the code read, a name never created, no stage named",
            Op::ReadCode(None),
            "absent_fn".to_string(),
            "",
        ),
        (
            "the code read, the dead id, the LIVE stage",
            Op::ReadCode(Some(STAGE_LIVE)),
            dead.id.clone(),
            "",
        ),
        (
            "the code read, a value in no namespace this service mints",
            Op::ReadCode(Some(STAGE_DEVELOPMENT)),
            "arn-shaped-nothing-holds".to_string(),
            "",
        ),
        (
            "the code read, a held connection function in a stage it was never published to",
            Op::ReadCode(Some(STAGE_LIVE)),
            held.name.clone(),
            "",
        ),
        (
            "the delete, the dead id, the version token it really stood at",
            Op::Delete,
            dead.id.clone(),
            dead.etag.as_str(),
        ),
        (
            "the delete, a minted-shaped id never allocated, a token nothing holds",
            Op::Delete,
            "cf_1111111111111".to_string(),
            UNHELD_VERSION_TOKEN,
        ),
        (
            "the publish, the dead id, the version token it really stood at",
            Op::Publish,
            dead.id.clone(),
            dead.etag.as_str(),
        ),
        (
            "the publish, a name never created, a token nothing holds",
            Op::Publish,
            "absent_fn".to_string(),
            UNHELD_VERSION_TOKEN,
        ),
        (
            "the listing of served distributions, the dead ARN",
            Op::ListDistributions,
            dead.arn.clone(),
            "",
        ),
        (
            "the listing of served distributions, a name never created",
            Op::ListDistributions,
            "absent_fn".to_string(),
            "",
        ),
    ];

    for (case, op, identifier, token) in &rows {
        let answer = refuse(&router, *op, identifier, token).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            ENTITY_NOT_FOUND,
            "case {case}: expected {ENTITY_NOT_FOUND}, got {:?} with {}",
            answer.error_code(),
            answer.raw
        );
        let message = answer
            .root()
            .find_text("Message")
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message, NOT_FOUND_SENTENCE,
            "case {case}: expected the fixed sentence, got {message:?}"
        );
        // Nothing the request carried is any part of it: not the identifier, not the stage,
        // not the version token.
        assert!(
            !message.contains(identifier.as_str()),
            "case {case}: the refusal must not echo the identifier {identifier:?}, got \
             {message:?}"
        );
        let stage = named_stage(*op);
        assert!(
            stage.is_empty() || !message.contains(stage),
            "case {case}: the refusal must not echo the stage {stage:?}, got {message:?}"
        );
        assert!(
            token.is_empty() || !message.contains(token),
            "case {case}: the refusal must not echo the version token {token:?}, got \
             {message:?}"
        );
    }

    // The control: the connection function that IS there is still served by each read, so
    // the rows above refuse what is absent rather than everything.
    for (case, op) in [
        ("describe, no stage named", Op::Describe(None)),
        (
            "describe, the DEVELOPMENT stage",
            Op::Describe(Some(STAGE_DEVELOPMENT)),
        ),
        ("the code read, no stage named", Op::ReadCode(None)),
    ] {
        let answer = refuse(&router, op, &held.id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: the held connection function must still be served, got {} with {}",
            answer.status, answer.raw
        );
    }

    // And the mutation that names it succeeds under its own version token, so the refusals
    // above are about the resource being absent rather than about the token.
    let published = publish_connection_function(&router, &held.id, &held.etag).await;
    assert_eq!(
        published.status, 200,
        "the held connection function must publish under its own version token, got {} with {}",
        published.status, published.raw
    );
}
