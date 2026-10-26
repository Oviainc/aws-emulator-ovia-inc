// SPDX-License-Identifier: CC0-1.0

//! `GetConnectionFunction` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the URI label
//! and query binding, the handler, the persisted state and the blob payload serialization —
//! the same path a client takes. The rules under test are:
//!
//! * the code read SERVES the bytes the create of the addressed stage stored, byte for byte,
//!   together with that stage's version token and the media type a blob payload travels
//!   under — so the answer comes from the one stored record and is the same whether the
//!   resource is addressed by the id its create minted or by the name it was created under;
//! * a read changes nothing a later caller sees: repeating it, and describing the connection
//!   function afterwards, report the same bytes and the same version token;
//! * a stage carries its OWN code and version token, and the `LIVE` stage exists only once a
//!   publish has copied `DEVELOPMENT` into it — so reading the code of the `LIVE` stage of a
//!   connection function that was only created is an entity that was not found (HTTP 404),
//!   and after a publish that read answers with the copied bytes under a version token of
//!   its own while the `DEVELOPMENT` stage keeps the token it had;
//! * an identifier no connection function answers to is the same refusal, whatever stage it
//!   names, and reports which identifier and stage was asked for.
//!
//! The table rows differ along everything these rules generalise over: the name's length
//! (both published bounds of `FunctionName`) and the character classes its pattern admits,
//! the runtime (every value the modelled `FunctionRuntime` enum admits), the code's size
//! (the modelled minimum, a real script, and the published function-code quota) and whether
//! the code is text at all (a `FunctionBlob` is a blob, so arbitrary bytes have to survive
//! the round trip), and whether the configuration associates a key value store — the create
//! table this crate already shares, so the code read is asserted over the same class of
//! connection functions the rest of the surface is.

use std::collections::BTreeSet;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_lifecycle::{
    create_connection_function, create_table, describe_connection_function, key_value_store_arn,
    member, Create,
};
// The publish call path, from the suite that owns the publish surface: one copy of it rather
// than a second here.
use crate::connection_function_publish::publish_connection_function;
use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, API_VERSION, AUTHORIZATION, HOST,
};

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The media type a response whose payload is the `FunctionBlob` itself travels under
/// (`GetConnectionFunction$ContentType`, bound to the `Content-Type` header), spelled out
/// here for the same reason the stages are.
const BLOB_MEDIA_TYPE: &str = "application/octet-stream";

/// The wire error code of the refusal this operation models for a connection function stage
/// that is not there (`EntityNotFound`, HTTP 404).
const ENTITY_NOT_FOUND: &str = "EntityNotFound";

// ---------------------------------------------------------------------------
// Calling the code read
// ---------------------------------------------------------------------------

/// What a `GetConnectionFunction` answered: the status, the `ETag` and `Content-Type`
/// headers it bound, and the response payload as BYTES.
///
/// The payload is the `FunctionBlob` itself rather than a document, so it is read as bytes:
/// code that is not valid UTF-8 (a blob may be anything) has to come back unchanged, which
/// reading it as a string would not show.
struct CodeAnswer {
    status: u16,
    etag: String,
    content_type: String,
    body: Vec<u8>,
}

/// `GET /2020-05-31/connection-function/{Identifier}`, read as raw bytes, naming `stage`
/// when it is `Some` and leaving the optional query parameter out entirely when it is
/// `None`.
async fn get_connection_function(
    router: &MultiServiceRouter,
    identifier: &str,
    stage: Option<&str>,
) -> CodeAnswer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(code_uri(identifier, stage))
        .header("host", HOST)
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let etag = header("etag");
    let content_type = header("content-type");
    CodeAnswer {
        status,
        etag,
        content_type,
        body: response.into_body().to_vec(),
    }
}

/// The same read, through the shared XML-parsing call path, for the cases whose answer is
/// the restXml error envelope rather than a blob.
async fn get_connection_function_refusal(
    router: &MultiServiceRouter,
    identifier: &str,
    stage: Option<&str>,
) -> Answer {
    call(router, "GET", &code_uri(identifier, stage), None, None).await
}

/// The URI the code read is addressed at.
fn code_uri(identifier: &str, stage: Option<&str>) -> String {
    match stage {
        Some(stage) => format!("/{API_VERSION}/connection-function/{identifier}?Stage={stage}"),
        None => format!("/{API_VERSION}/connection-function/{identifier}"),
    }
}

/// Create the connection function `create` asks for and answer the `(Id, ETag)` the create
/// minted for its `DEVELOPMENT` stage.
async fn create_one(router: &MultiServiceRouter, create: &Create) -> (String, String) {
    let case = create.case;
    let answer = create_connection_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected HTTP 201 from the create, got {} with {}",
        answer.status, answer.raw
    );
    (
        member(answer.root(), "Id", case, &answer.raw),
        answer.header("etag").to_string(),
    )
}

/// Assert `answer` served `code` under `etag`, naming `case` and `context`.
fn assert_code(answer: &CodeAnswer, code: &[u8], etag: &str, case: &str, context: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case} ({context}): expected HTTP 200 from the code read, got {} with {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.body,
        code,
        "case {case} ({context}): expected the stored code ({} bytes), got {} bytes",
        code.len(),
        answer.body.len()
    );
    assert_eq!(
        answer.etag, etag,
        "case {case} ({context}): expected the stage's version token {etag}, got {:?}",
        answer.etag
    );
    assert_eq!(
        answer.content_type, BLOB_MEDIA_TYPE,
        "case {case} ({context}): expected the blob media type, got {:?}",
        answer.content_type
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The code read answers from the one stored record: the bytes the create stored, byte for
/// byte, under the version token that create handed back and the media type a blob payload
/// travels under — addressed by the minted id and by the name the create was given alike,
/// and with the stage named explicitly or left out. Across the table no two connection
/// functions serve the same version token.
#[tokio::test]
async fn the_code_read_answers_the_bytes_the_create_stored() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_code_store").await;
    let mut etags = BTreeSet::new();

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let (id, etag) = create_one(&router, &create).await;
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token from the create, got {etag:?}"
        );
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // By the minted id, with no stage named.
        let by_id = get_connection_function(&router, &id, None).await;
        assert_code(&by_id, &create.code, &etag, case, "by the minted id");

        // By the name the create was given: the SAME record.
        let by_name = get_connection_function(&router, &create.name, None).await;
        assert_code(&by_name, &create.code, &etag, case, "by the name");

        // Naming the stage a connection function is created in is the same read as naming
        // no stage at all.
        let explicit = get_connection_function(&router, &id, Some(STAGE_DEVELOPMENT)).await;
        assert_code(
            &explicit,
            &create.code,
            &etag,
            case,
            "naming DEVELOPMENT explicitly",
        );
    }
}

/// A read changes nothing a later caller sees: repeating the code read, and describing the
/// connection function after it, report the same bytes and the same version token the create
/// stored.
#[tokio::test]
async fn the_code_read_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_read_store").await;

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let (id, etag) = create_one(&router, &create).await;

        let first = get_connection_function(&router, &id, None).await;
        assert_code(&first, &create.code, &etag, case, "the first read");
        let again = get_connection_function(&router, &id, None).await;
        assert_code(&again, &create.code, &etag, case, "the repeated read");

        // The describe still reports the stage the create stored, under the token the
        // create handed back: the code read advanced nothing.
        let described = describe_connection_function(&router, &id, None).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected HTTP 200 from the describe after the read, got {} with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {case}: expected the create's version token from the describe after the \
             read, got {:?}",
            described.header("etag")
        );
        let stage = member(described.root(), "Stage", case, &described.raw);
        assert_eq!(
            stage, STAGE_DEVELOPMENT,
            "case {case}: expected the stage the create stored after the read, got {stage:?}"
        );
    }
}

/// Each stage carries its OWN code and version token, and the `LIVE` stage exists only once
/// a publish has copied `DEVELOPMENT` into it: reading the code of the `LIVE` stage of a
/// connection function that was only created is an entity that was not found, and after a
/// publish that read serves the copied bytes under a token of its own while the
/// `DEVELOPMENT` stage keeps the token it had.
#[tokio::test]
async fn the_live_stage_serves_code_only_after_a_publish() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_live_store").await;

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let (id, development_etag) = create_one(&router, &create).await;

        // Before the publish there is no LIVE stage to read code from.
        let before = get_connection_function_refusal(&router, &id, Some(STAGE_LIVE)).await;
        assert_eq!(
            before.status, 404,
            "case {case}: expected HTTP 404 for the LIVE stage of a connection function that \
             was only created, got {} with {}",
            before.status, before.raw
        );
        assert_eq!(
            before.error_code(),
            ENTITY_NOT_FOUND,
            "case {case}: expected {ENTITY_NOT_FOUND}, got {:?} with {}",
            before.error_code(),
            before.raw
        );

        let published = publish_connection_function(&router, &id, &development_etag).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected HTTP 200 from the publish, got {} with {}",
            published.status, published.raw
        );

        // The publish binds no `ETag` header (its only output member is the summary), so
        // the token the published stage carries is read through the describe of that stage.
        let described_live = describe_connection_function(&router, &id, Some(STAGE_LIVE)).await;
        assert_eq!(
            described_live.status, 200,
            "case {case}: expected HTTP 200 from the describe of the published stage, got {} \
             with {}",
            described_live.status, described_live.raw
        );
        let live_etag = described_live.header("etag").to_string();
        assert!(
            is_version_token(&live_etag),
            "case {case}: expected a version token on the published stage, got {live_etag:?}"
        );
        assert_ne!(
            live_etag, development_etag,
            "case {case}: the published stage must carry a version token of its own"
        );

        // The published stage serves the bytes the publish copied, under its own token.
        let live = get_connection_function(&router, &id, Some(STAGE_LIVE)).await;
        assert_code(&live, &create.code, &live_etag, case, "the LIVE stage");

        // The stage the publish READ is untouched: same bytes, and the token it had.
        let development = get_connection_function(&router, &id, Some(STAGE_DEVELOPMENT)).await;
        assert_code(
            &development,
            &create.code,
            &development_etag,
            case,
            "the DEVELOPMENT stage after the publish",
        );
    }
}

/// A connection function stage this service does not hold is the modelled `EntityNotFound`
/// (HTTP 404), whatever identifier namespace the request addressed it in and whatever stage
/// it named — and the sentence names the identifier and the stage that was asked for, so a
/// caller can tell which of the two it got wrong.
#[tokio::test]
async fn a_stage_the_service_does_not_hold_is_not_found() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_missing_store").await;

    // One connection function the service DOES hold, so the rows that name a stage it has
    // not been published to are refused for the stage rather than for the identifier.
    let create = create_table(&kvs_arn)
        .into_iter()
        .next()
        .expect("the shared create table is not empty");
    let (held_id, _) = create_one(&router, &create).await;

    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "an id no connection function answers to",
            "cf_0000000000000".to_string(),
            None,
        ),
        (
            "a name no connection function was created under",
            "no_such_connection_function".to_string(),
            Some(STAGE_DEVELOPMENT),
        ),
        (
            "an identifier in no namespace this service mints",
            "arn-shaped-nothing-holds".to_string(),
            Some(STAGE_LIVE),
        ),
        (
            "a held connection function in a stage it was never published to",
            held_id,
            Some(STAGE_LIVE),
        ),
        (
            "a held connection function's name in a stage it was never published to",
            create.name.clone(),
            Some(STAGE_LIVE),
        ),
    ];

    for (case, identifier, stage) in rows {
        let answer = get_connection_function_refusal(&router, &identifier, stage).await;
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
        assert!(
            message.starts_with("The entity was not found"),
            "case {case}: expected the sentence the model states on the shape, got {message:?}"
        );
        assert!(
            message.contains(&identifier) && message.contains(stage.unwrap_or(STAGE_DEVELOPMENT)),
            "case {case}: expected the identifier and the stage that was asked for in the \
             message, got {message:?}"
        );
    }
}
