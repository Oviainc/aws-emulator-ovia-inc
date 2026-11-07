// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's views.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * a view IS the SQL query it is defined by, so `CreateView` admits a `definition` a
//!   SQL parser reads as ONE query — whatever form that query takes — and refuses one it
//!   does not with `ValidationException` (400), persisting nothing for the refusal;
//! * a created view is persisted and `GetView` answers from that same store, reporting
//!   the definition, the description, the `USER` type, the ARN and the timestamps the
//!   create reported;
//! * a view's name is its identity within the account and region, so a second view under
//!   a live name is a `ConflictException` (409) and leaves the first one as it was;
//! * an unknown name resolves to nothing: `ResourceNotFoundException` (404);
//! * `@idempotencyToken` replays return the original view rather than minting a second.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_view::{CreateView, CreateViewError};
use omni::ops::get_view::{GetView, GetViewError};
use omni::ops::types::{
    CreateViewInput, CreateViewOutput, GetViewInput, GetViewOutput, CREATE_VIEW_INPUT_SCHEMA,
    GET_VIEW_INPUT_SCHEMA, VIEW_TYPE_VALUES,
};
use omni::state::{
    view_definition_rejection, NOT_A_QUERY, NOT_A_QUERY_STATEMENT, VIEW_ARN_RESOURCE_TYPE,
    VIEW_TYPE_USER,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite signs with.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record and `ACCESS_KEY_ID`.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    async fn create_view(
        &self,
        input: &CreateViewInput,
    ) -> Result<CreateViewOutput, CreateViewError> {
        CreateView::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_view(&self, name: &str) -> Result<GetViewOutput, GetViewError> {
        GetView::new(Arc::clone(&self.engine))
            .handle(
                GetViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                expected_status,
                "case {case:?}: expected HTTP {expected_status} for {expected_code}, got {}",
                error.http_status()
            );
        }
    }
}

/// The smallest view request the model admits, for `name` and `definition`.
fn create_input(name: &str, definition: &str) -> CreateViewInput {
    CreateViewInput {
        name: name.to_string(),
        definition: definition.to_string(),
        description: None,
        tags: None,
        client_token: None,
    }
}

/// Definitions a SQL parser reads as ONE query, so `CreateView` persists them.
///
/// The rows differ along the FORM of the query — projection, predicate, grouping,
/// set operation, common table expression, join and ordering, a leading comment, a
/// trailing semicolon — because the rule is about the definition being one query, not
/// about the text it is written as.
const ONE_QUERY: &[(&str, &str)] = &[
    ("select all", "SELECT * FROM logs"),
    (
        "projection and predicate",
        "SELECT service, message FROM logs WHERE level = 'ERROR'",
    ),
    (
        "aggregate and grouping",
        "SELECT service, count(*) AS errors FROM logs GROUP BY service HAVING count(*) > 10",
    ),
    (
        "set operation",
        "SELECT id FROM logs UNION SELECT id FROM spans",
    ),
    (
        "common table expression",
        "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM errors",
    ),
    (
        "join, ordering and a bound",
        "SELECT l.id FROM logs l JOIN spans s ON s.trace_id = l.trace_id ORDER BY l.id LIMIT 10",
    ),
    (
        "a leading comment",
        "-- the day's errors\nSELECT count(*) FROM logs WHERE level = 'ERROR'",
    ),
    ("a trailing semicolon", "SELECT 1 AS one;"),
    (
        "a nested query",
        "SELECT id FROM logs WHERE service IN (SELECT service FROM spans)",
    ),
];

/// Which of the three refusals a definition earns, so a row asserts the message its
/// class carries rather than only the error code.
enum Refusal {
    /// Nothing a SQL parser reads as a query.
    NotAQuery,
    /// One statement, which is not a query.
    NotAQueryStatement,
    /// More than one statement, and how many.
    Statements(usize),
}

impl Refusal {
    /// The message this class of refusal carries.
    fn message(&self) -> String {
        match self {
            Self::NotAQuery => NOT_A_QUERY.to_string(),
            Self::NotAQueryStatement => NOT_A_QUERY_STATEMENT.to_string(),
            Self::Statements(count) => format!(
                "The view definition must be a single SQL query, not {count} statements."
            ),
        }
    }
}

/// Definitions a SQL parser does NOT read as one query, so `CreateView` refuses them.
///
/// The rows differ along WHY: the text is not SQL at all, it carries no statement, it is
/// a statement that is not a query, or it is more than one statement.
const NOT_ONE_QUERY: &[(&str, &str, Refusal)] = &[
    ("prose", "the daily error count", Refusal::NotAQuery),
    (
        "a JSON document",
        "{\"query\": \"SELECT 1\"}",
        Refusal::NotAQuery,
    ),
    ("a truncated query", "SELECT * FROM", Refusal::NotAQuery),
    (
        "an unclosed string",
        "SELECT * FROM logs WHERE level = 'ER",
        Refusal::NotAQuery,
    ),
    (
        "an unbalanced parenthesis",
        "SELECT count(* FROM logs",
        Refusal::NotAQuery,
    ),
    ("whitespace only", "   ", Refusal::NotAQuery),
    (
        "a comment only",
        "-- nothing but a comment",
        Refusal::NotAQuery,
    ),
    (
        "a delete",
        "DELETE FROM logs WHERE level = 'ERROR'",
        Refusal::NotAQueryStatement,
    ),
    (
        "an insert",
        "INSERT INTO logs (id) VALUES (1)",
        Refusal::NotAQueryStatement,
    ),
    (
        "an update",
        "UPDATE logs SET level = 'INFO'",
        Refusal::NotAQueryStatement,
    ),
    (
        "a DDL statement",
        "CREATE TABLE logs (id INT)",
        Refusal::NotAQueryStatement,
    ),
    ("two queries", "SELECT 1; SELECT 2", Refusal::Statements(2)),
    (
        "a query followed by a mutation",
        "SELECT * FROM logs; DROP TABLE logs",
        Refusal::Statements(2),
    ),
    (
        "three statements",
        "SELECT 1; SELECT 2; SELECT 3",
        Refusal::Statements(3),
    ),
];

/// A view name satisfying `ViewName`'s `^view\.[a-z0-9][a-z0-9_-]{0,250}$` with its
/// `@length(6..256)`, from `smithy_query(query="shape",shape="ViewName")`.
fn view_name(suffix: &str) -> String {
    format!("view.{suffix}")
}

#[tokio::test]
async fn a_definition_that_is_one_sql_query_is_persisted_and_read_back() {
    let omni = Omni::new();
    for (case, definition) in ONE_QUERY {
        let name = view_name(&case.replace(' ', "-").replace(',', ""));
        assert_eq!(
            view_definition_rejection(definition),
            None,
            "case {case:?}: the definition rule must admit {definition:?}"
        );
        let created = omni
            .create_view(&CreateViewInput {
                description: Some(format!("the {case} view")),
                ..create_input(&name, definition)
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));

        assert_eq!(
            created.definition, *definition,
            "case {case:?}: the created view must carry the definition it was given, got {:?}",
            created.definition
        );
        assert_eq!(
            created.name, name,
            "case {case:?}: the created view must carry the name it was given, got {:?}",
            created.name
        );
        assert_eq!(
            created.r#type, VIEW_TYPE_USER,
            "case {case:?}: a view created through the API is a {VIEW_TYPE_USER} view, got {:?}",
            created.r#type
        );
        assert!(
            VIEW_TYPE_VALUES.contains(&created.r#type.as_str()),
            "case {case:?}: the reported type must be a modelled ViewType, got {:?}",
            created.r#type
        );
        assert_eq!(
            created.arn,
            format!(
                "arn:aws:cloudwatch:{REGION}:{}:{VIEW_ARN_RESOURCE_TYPE}/{name}",
                engine::emulator_account_id()
            ),
            "case {case:?}: the ARN must name this region, account and view, got {:?}",
            created.arn
        );
        assert_eq!(
            created.created_at, created.updated_at,
            "case {case:?}: a view nothing has updated was last updated when it was created, got {:?} and {:?}",
            created.created_at, created.updated_at
        );

        // The round trip: the reader answers from the store the create wrote.
        let read = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"));
        assert_eq!(
            (
                read.name.as_str(),
                read.definition.as_str(),
                read.description.as_deref(),
                read.r#type.as_str(),
                read.arn.as_str(),
                read.created_at,
                read.updated_at
            ),
            (
                created.name.as_str(),
                created.definition.as_str(),
                created.description.as_deref(),
                created.r#type.as_str(),
                created.arn.as_str(),
                created.created_at,
                created.updated_at
            ),
            "case {case:?}: GetView must answer with the persisted view, got {read:?}"
        );

        // A read reports: reading twice reports the same view.
        let read_again = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the second GetView failed: {error:?}"));
        assert_eq!(
            read_again.updated_at, read.updated_at,
            "case {case:?}: reading a view must not change it, got {:?} after {:?}",
            read_again.updated_at, read.updated_at
        );
    }
}

#[tokio::test]
async fn a_definition_that_is_not_one_sql_query_is_refused_and_persists_nothing() {
    let omni = Omni::new();
    for (index, (case, definition, refusal)) in NOT_ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("refused-{index}"));
        let result = omni.create_view(&create_input(&name, definition)).await;
        // The refusal says which of the three things is wrong, so a caller learns what
        // to fix from the message alone.
        let rendered = format!("{result:?}");
        let expected = refusal.message();
        assert!(
            rendered.contains(&expected),
            "case {case:?}: expected the refusal to carry {expected:?}, got {rendered}"
        );
        assert_eq!(
            view_definition_rejection(definition),
            Some(expected.clone()),
            "case {case:?}: expected the definition rule to answer {expected:?}"
        );
        expect_modelled_error(case, result, 400, "ValidationException");
        // Nothing was persisted for the refused request, so the name resolves to
        // nothing.
        expect_modelled_error(
            case,
            omni.get_view(&name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn a_views_name_is_its_identity_within_the_account_and_region() {
    let omni = Omni::new();
    let name = view_name("errors-by-service");
    let original = omni
        .create_view(&create_input(&name, "SELECT service FROM logs"))
        .await
        .expect("the first view is created");

    for (case, definition) in [
        ("the same definition", "SELECT service FROM logs"),
        ("another definition", "SELECT count(*) FROM spans"),
    ] {
        expect_modelled_error(
            case,
            omni.create_view(&create_input(&name, definition)).await,
            409,
            "ConflictException",
        );
    }

    // The conflict left the first view as it was rather than replacing it.
    let read = omni.get_view(&name).await.expect("the first view is read");
    assert_eq!(
        (read.definition.as_str(), read.updated_at),
        (original.definition.as_str(), original.updated_at),
        "a refused duplicate must leave the live view unchanged, got {read:?}"
    );

    // Another name is another view, definition and all.
    let other = view_name("spans-by-service");
    let second = omni
        .create_view(&create_input(&other, "SELECT count(*) FROM spans"))
        .await
        .expect("a second name is a second view");
    assert_ne!(
        second.arn, original.arn,
        "two views must carry two ARNs, got {:?} twice",
        second.arn
    );
}

#[tokio::test]
async fn an_unknown_view_name_resolves_to_nothing() {
    let omni = Omni::new();
    for (case, name) in [
        ("never created", view_name("no-such-view")),
        ("the shortest admitted name", view_name("a")),
    ] {
        expect_modelled_error(
            case,
            omni.get_view(&name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_view() {
    let omni = Omni::new();
    let name = view_name("token-replayed");
    let request = CreateViewInput {
        client_token: Some("a-client-token".to_string()),
        ..create_input(&name, "SELECT * FROM logs")
    };
    let original = omni
        .create_view(&request)
        .await
        .expect("the first call creates the view");

    for (case, replay) in [
        ("the same request", request.clone()),
        (
            "the same token with another definition",
            CreateViewInput {
                definition: "SELECT count(*) FROM spans".to_string(),
                ..request.clone()
            },
        ),
    ] {
        let replayed = omni
            .create_view(&replay)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the replay failed: {error:?}"));
        assert_eq!(
            (
                replayed.name.as_str(),
                replayed.definition.as_str(),
                replayed.arn.as_str(),
                replayed.created_at
            ),
            (
                original.name.as_str(),
                original.definition.as_str(),
                original.arn.as_str(),
                original.created_at
            ),
            "case {case:?}: a token replay must return the original view, got {replayed:?}"
        );
    }

    // Another token under another name is another view.
    let fresh_name = view_name("token-fresh");
    let fresh = omni
        .create_view(&CreateViewInput {
            client_token: Some("another-client-token".to_string()),
            ..create_input(&fresh_name, "SELECT * FROM logs")
        })
        .await
        .expect("another token creates another view");
    assert_ne!(
        fresh.arn, original.arn,
        "a different token must create a different view, got {:?} twice",
        fresh.arn
    );
}

#[tokio::test]
async fn the_view_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let name = view_name("over-the-wire");

    let (status, body) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&name, "SELECT count(*) FROM logs"),
        )
        .await;
    assert_eq!(status, 200, "CreateView over the wire: expected 200");
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let created = CreateViewOutput::deserialize(&mut deserializer)
        .expect("the CreateView response deserializes");

    let (status, body) = omni
        .over_the_wire(
            "GetView",
            GET_VIEW_INPUT_SCHEMA,
            &GetViewInput { name: name.clone() },
        )
        .await;
    assert_eq!(status, 200, "GetView over the wire: expected 200");
    let mut deserializer = codec.create_deserializer(&body);
    let read =
        GetViewOutput::deserialize(&mut deserializer).expect("the GetView response deserializes");
    assert_eq!(
        (read.name.as_str(), read.definition.as_str()),
        (created.name.as_str(), created.definition.as_str()),
        "GetView over the wire must answer with the persisted view, got {read:?}"
    );

    // The refusal travels as the modelled 400 over the same wire.
    let (status, _) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&view_name("refused-over-the-wire"), "not a sql query"),
        )
        .await;
    assert_eq!(
        status, 400,
        "a definition that is not one SQL query must be refused with 400"
    );

    // And the duplicate name travels as the modelled 409.
    let (status, _) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&name, "SELECT count(*) FROM logs"),
        )
        .await;
    assert_eq!(
        status, 409,
        "a second view under a live name must be refused with 409"
    );
}
