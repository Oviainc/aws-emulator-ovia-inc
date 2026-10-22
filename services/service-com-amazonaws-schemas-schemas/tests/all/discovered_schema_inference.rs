// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetDiscoveredSchema`.
//!
//! RULE under test: the returned `Content` is a schema document of the requested
//! `Type`, inferred from the sampled events — each `Events` entry parsed as JSON and
//! the sample unified into one description (a member every event carries is
//! `required`, a position seen as several types widens or falls back to the empty
//! schema, a position also seen as `null` is nullable in the dialect's spelling).
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! recorded corpus case (one event, `OpenApi3`) is ONE row of the tables below.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs with.
const REGION: &str = "us-west-2";

/// `GetDiscoveredSchema`'s URI, from the model's `@http` binding.
const DISCOVER_URI: &str = "/v1/discover";

/// The name the inferred schema carries in the document the service returns.
const ROOT_SCHEMA_NAME: &str = "Event";

/// One emulator with its own store, reached over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the signature check is the
        // framework's and is covered where it lives.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `schemas`
            // service.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20240101/{REGION}/schemas/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} answered {status} with a body that is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        Answer { status, body }
    }

    /// `GetDiscoveredSchema` over `events`, each sent as the JSON text of one event.
    async fn discover(&self, events: &[Value], schema_type: &str) -> Answer {
        let events: Vec<String> = events.iter().map(Value::to_string).collect();
        self.discover_raw(&events, schema_type).await
    }

    /// `GetDiscoveredSchema` over event strings that need not be JSON at all.
    async fn discover_raw(&self, events: &[String], schema_type: &str) -> Answer {
        self.call(
            "POST",
            DISCOVER_URI,
            Some(json!({"Events": events, "Type": schema_type})),
        )
        .await
    }
}

/// A response, reduced to what these tests assert on.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code a client branches on, as restJson1 renders it.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn str_at(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a string `{member}` in {:?}", self.body))
            .to_string()
    }

    /// The `Content` of a successful discovery, parsed. `Content` is schema SOURCE —
    /// a string on the wire — so a client parses it exactly like this.
    fn document(&self, case: &str) -> Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected 200 from GetDiscoveredSchema, got {} {:?}",
            self.status, self.body
        );
        let content = self.str_at("Content");
        serde_json::from_str(&content).unwrap_or_else(|error| {
            panic!("case {case}: Content must be a JSON document ({error}): {content}")
        })
    }
}

/// The inferred schema inside a document, whichever dialect carries it.
///
/// OpenAPI keeps a schema under `components/schemas`; a JSON Schema document IS the
/// schema, so its envelope keywords are what has to come off.
fn inferred(document: &Value, dialect: &str) -> Value {
    match dialect {
        "OpenApi3" => document["components"]["schemas"][ROOT_SCHEMA_NAME].clone(),
        "JSONSchemaDraft4" => {
            let mut node: Map<String, Value> = document
                .as_object()
                .unwrap_or_else(|| panic!("a draft-04 document is an object: {document:?}"))
                .clone();
            node.remove("$schema");
            node.remove("title");
            Value::Object(node)
        }
        other => panic!("no such Type: {other}"),
    }
}

/// The schema the sample gave the single member `m` of a one-member event.
fn member_schema(document: &Value, dialect: &str) -> Value {
    inferred(document, dialect)["properties"]["m"].clone()
}

/// RULE: the envelope is the one the requested `Type` defines, and it carries the
/// schema inferred from the sample.
#[tokio::test]
async fn the_document_envelope_is_the_one_the_requested_type_defines() {
    let emulator = Emulator::new();
    let events = [json!({"id": "a1", "size": 3})];

    let openapi = emulator
        .discover(&events, "OpenApi3")
        .await
        .document("OpenApi3 envelope");
    assert_eq!(
        openapi["openapi"], json!("3.0.0"),
        "case OpenApi3 envelope: the document declares the OpenAPI dialect, got {openapi:?}"
    );
    assert_eq!(
        openapi["info"],
        json!({"version": "1.0.0", "title": ROOT_SCHEMA_NAME}),
        "case OpenApi3 envelope: expected an info block, got {:?}",
        openapi["info"]
    );
    assert_eq!(
        openapi["paths"], json!({}),
        "case OpenApi3 envelope: an inferred event schema describes no operations, got {:?}",
        openapi["paths"]
    );

    let draft4 = emulator
        .discover(&events, "JSONSchemaDraft4")
        .await
        .document("JSONSchemaDraft4 envelope");
    assert_eq!(
        draft4["$schema"],
        json!("http://json-schema.org/draft-04/schema#"),
        "case JSONSchemaDraft4 envelope: the document declares the draft-04 dialect, got {draft4:?}"
    );
    assert_eq!(
        draft4["title"], json!(ROOT_SCHEMA_NAME),
        "case JSONSchemaDraft4 envelope: expected the schema to be titled, got {:?}",
        draft4["title"]
    );

    // Whichever envelope carries it, the inferred schema is the same: the dialect
    // decides the wrapping, the sample decides the shape.
    let expected = json!({
        "type": "object",
        "required": ["id", "size"],
        "properties": {"id": {"type": "string"}, "size": {"type": "integer"}},
    });
    for dialect in TYPE_VALUES {
        let document = emulator
            .discover(&events, dialect)
            .await
            .document(dialect);
        let schema = inferred(&document, dialect);
        assert_eq!(
            schema, expected,
            "case {dialect}: expected {expected:?}, got {schema:?}"
        );
    }
}

/// RULE: every JSON type in the sample is described by its own schema type, and a
/// position the sample shows no typed value at constrains nothing.
#[tokio::test]
async fn each_sampled_json_value_becomes_its_schema_type() {
    let emulator = Emulator::new();

    // Rows: one per JSON type the sample can hold, plus the inference rules that do
    // not depend on the dialect — an integral number vs a fractional one, an
    // RFC-3339 string in both its `Z` and offset spellings, an array that unifies
    // its elements, an empty array, a nested array, a nested object, and a position
    // where only `null` was ever seen.
    let cases: Vec<(&str, Value, Value)> = vec![
        ("plain string", json!("free form"), json!({"type": "string"})),
        (
            "RFC-3339 string, Z",
            json!("2024-05-17T01:02:03Z"),
            json!({"type": "string", "format": "date-time"}),
        ),
        (
            "RFC-3339 string, numeric offset",
            json!("2024-05-17T01:02:03+02:00"),
            json!({"type": "string", "format": "date-time"}),
        ),
        ("integer", json!(7), json!({"type": "integer"})),
        ("negative integer", json!(-7), json!({"type": "integer"})),
        ("zero", json!(0), json!({"type": "integer"})),
        ("fractional number", json!(1.5), json!({"type": "number"})),
        ("boolean", json!(true), json!({"type": "boolean"})),
        (
            "array of one type",
            json!(["a", "b"]),
            json!({"type": "array", "items": {"type": "string"}}),
        ),
        (
            "array mixing integer and fractional elements",
            json!([1, 2.5]),
            json!({"type": "array", "items": {"type": "number"}}),
        ),
        (
            "array whose elements disagree",
            json!([1, "two"]),
            json!({"type": "array", "items": {}}),
        ),
        (
            "empty array",
            json!([]),
            json!({"type": "array", "items": {}}),
        ),
        (
            "nested array",
            json!([[1]]),
            json!({"type": "array", "items": {"type": "array", "items": {"type": "integer"}}}),
        ),
        (
            "nested object",
            json!({"inner": "v"}),
            json!({
                "type": "object",
                "required": ["inner"],
                "properties": {"inner": {"type": "string"}},
            }),
        ),
        (
            "empty object",
            json!({}),
            json!({"type": "object", "properties": {}}),
        ),
        ("only null", json!(null), json!({})),
    ];

    for dialect in TYPE_VALUES {
        for (case, value, expected) in &cases {
            let document = emulator
                .discover(&[json!({"m": value})], dialect)
                .await
                .document(&format!("{dialect}/{case}"));
            let schema = member_schema(&document, dialect);
            assert_eq!(
                &schema, expected,
                "case {dialect}/{case}: expected {expected:?}, got {schema:?}"
            );
        }
    }
}

/// RULE: a member is `required` only when EVERY sampled event carries it.
#[tokio::test]
async fn a_member_is_required_only_when_every_event_carries_it() {
    let emulator = Emulator::new();

    // Rows: one event (everything it carries is required), a member one event omits,
    // samples with nothing in common, and a 10-event sample — the published maximum
    // of `Events` (`@length(min: 1, max: 10)`) — where only the shared member is
    // required.
    let ten: Vec<Value> = (0..10)
        .map(|i| json!({"always": i, format!("only-{i}"): i}))
        .collect();
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "one event",
            vec![json!({"a": 1, "b": "x"})],
            json!(["a", "b"]),
        ),
        (
            "a member only one event carries",
            vec![json!({"a": 1, "b": "x"}), json!({"a": 2})],
            json!(["a"]),
        ),
        (
            "ten events sharing one member",
            ten,
            json!(["always"]),
        ),
    ];

    for dialect in TYPE_VALUES {
        for (case, events, expected) in &cases {
            let document = emulator
                .discover(events, dialect)
                .await
                .document(&format!("{dialect}/{case}"));
            let schema = inferred(&document, dialect);
            assert_eq!(
                &schema["required"], expected,
                "case {dialect}/{case}: expected required {expected:?}, got {:?}",
                schema["required"]
            );
            // Optional or not, every member the sample showed is a property.
            let properties = schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("case {dialect}/{case}: expected properties"))
                .len();
            let members: usize = {
                let mut names: Vec<&String> = Vec::new();
                for event in events {
                    for name in event.as_object().expect("an event object").keys() {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
                names.len()
            };
            assert_eq!(
                properties, members,
                "case {dialect}/{case}: every sampled member must be a property"
            );
        }
    }

    // Nothing common to the whole sample: neither dialect allows an empty `required`
    // list, so the schema carries none at all.
    for dialect in TYPE_VALUES {
        let document = emulator
            .discover(&[json!({"a": 1}), json!({"b": 2})], dialect)
            .await
            .document(&format!("{dialect}/nothing in common"));
        let schema = inferred(&document, dialect);
        assert!(
            schema.get("required").is_none(),
            "case {dialect}/nothing in common: expected no required list, got {:?}",
            schema["required"]
        );
        assert_eq!(
            schema["properties"],
            json!({"a": {"type": "integer"}, "b": {"type": "integer"}}),
            "case {dialect}/nothing in common: both members are still properties, got {:?}",
            schema["properties"]
        );
    }
}

/// RULE: a position several events disagree about is unified — widened where one type
/// contains the other, and left unconstrained where neither does.
#[tokio::test]
async fn disagreeing_events_are_unified_into_one_schema() {
    let emulator = Emulator::new();

    // Rows: integer widened by a fractional number (every integer is a number), a
    // timestamp string beside a free-form one (the format is claimed only when every
    // string has it), two irreconcilable types, an array whose elements come from
    // different events, and two objects whose members merge.
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "integer then fractional",
            vec![json!({"m": 1}), json!({"m": 2.5})],
            json!({"type": "number"}),
        ),
        (
            "fractional then integer",
            vec![json!({"m": 2.5}), json!({"m": 1})],
            json!({"type": "number"}),
        ),
        (
            "timestamp then free-form string",
            vec![json!({"m": "2024-05-17T01:02:03Z"}), json!({"m": "later"})],
            json!({"type": "string"}),
        ),
        (
            "two timestamps",
            vec![
                json!({"m": "2024-05-17T01:02:03Z"}),
                json!({"m": "2025-01-01T00:00:00Z"}),
            ],
            json!({"type": "string", "format": "date-time"}),
        ),
        (
            "string then object",
            vec![json!({"m": "x"}), json!({"m": {"inner": 1}})],
            json!({}),
        ),
        (
            "string then boolean",
            vec![json!({"m": "x"}), json!({"m": false})],
            json!({}),
        ),
        (
            "empty array then a populated one",
            vec![json!({"m": []}), json!({"m": ["a"]})],
            json!({"type": "array", "items": {"type": "string"}}),
        ),
        (
            "objects whose members merge",
            vec![json!({"m": {"a": 1}}), json!({"m": {"a": 2, "b": "x"}})],
            json!({
                "type": "object",
                "required": ["a"],
                "properties": {"a": {"type": "integer"}, "b": {"type": "string"}},
            }),
        ),
    ];

    for dialect in TYPE_VALUES {
        for (case, events, expected) in &cases {
            let document = emulator
                .discover(events, dialect)
                .await
                .document(&format!("{dialect}/{case}"));
            let schema = member_schema(&document, dialect);
            assert_eq!(
                &schema, expected,
                "case {dialect}/{case}: expected {expected:?}, got {schema:?}"
            );
        }
    }
}

/// RULE: a position the sample also showed `null` at is nullable, in the spelling the
/// requested dialect has for it — OpenAPI 3.0's `nullable` keyword, draft-04's union
/// with the `null` type.
#[tokio::test]
async fn a_nullable_position_is_spelled_the_dialects_way() {
    let emulator = Emulator::new();

    // Rows: each scalar and each container type, seen beside a `null`.
    let cases: Vec<(&str, Value, &str, Value)> = vec![
        ("string", json!("x"), "string", json!({"type": "string"})),
        ("integer", json!(1), "integer", json!({"type": "integer"})),
        ("number", json!(1.5), "number", json!({"type": "number"})),
        ("boolean", json!(true), "boolean", json!({"type": "boolean"})),
        (
            "array",
            json!([1]),
            "array",
            json!({"items": {"type": "integer"}}),
        ),
        (
            "object",
            json!({"a": 1}),
            "object",
            json!({"required": ["a"], "properties": {"a": {"type": "integer"}}}),
        ),
    ];

    for (case, value, type_name, rest) in &cases {
        for dialect in TYPE_VALUES {
            let document = emulator
                .discover(&[json!({"m": value}), json!({"m": null})], dialect)
                .await
                .document(&format!("{dialect}/{case}"));
            let schema = member_schema(&document, dialect);

            let mut expected = rest.as_object().expect("a row's extra keywords").clone();
            match *dialect {
                // OpenAPI 3.0 has no `null` type.
                "OpenApi3" => {
                    expected.insert("type".to_string(), json!(type_name));
                    expected.insert("nullable".to_string(), json!(true));
                }
                // draft-04 has no `nullable` keyword.
                _ => {
                    expected.insert("type".to_string(), json!([type_name, "null"]));
                }
            }
            let expected = Value::Object(expected);
            assert_eq!(
                schema, expected,
                "case {dialect}/{case}: expected {expected:?}, got {schema:?}"
            );

            // A member that is sometimes null is still a member every event carried,
            // so it stays required: `null` is a value, not an absence.
            assert_eq!(
                inferred(&document, dialect)["required"],
                json!(["m"]),
                "case {dialect}/{case}: a null-valued member is still required"
            );
        }
    }
}

/// RULE: an `Events` entry that is not a JSON event is refused as a client error —
/// 400 `BadRequestException` — and nothing partial is returned.
#[tokio::test]
async fn an_event_that_is_not_a_json_object_is_refused() {
    let emulator = Emulator::new();

    // Rows: text that is not JSON at all, truncated JSON, and each JSON type that is
    // not an object. The modelled `@length(min: 1)` on an entry refuses the empty
    // string at the boundary, so that row asserts only the status.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("not JSON", "not json at all", Some("BadRequestException")),
        ("truncated object", "{\"a\":", Some("BadRequestException")),
        ("trailing garbage", "{} {}", Some("BadRequestException")),
        ("a JSON string", "\"an event\"", Some("BadRequestException")),
        ("a JSON array", "[{\"a\": 1}]", Some("BadRequestException")),
        ("a JSON number", "42", Some("BadRequestException")),
        ("JSON null", "null", Some("BadRequestException")),
        ("JSON true", "true", Some("BadRequestException")),
        ("the empty string", "", None),
    ];

    for dialect in TYPE_VALUES {
        for (case, raw, expected_code) in &cases {
            // Once in a sample of its own, and once beside an event that IS valid:
            // one bad entry refuses the whole call.
            for (position, events) in [
                ("alone", vec![(*raw).to_string()]),
                (
                    "beside a valid event",
                    vec![json!({"a": 1}).to_string(), (*raw).to_string()],
                ),
            ] {
                let answer = emulator.discover_raw(&events, dialect).await;
                assert_eq!(
                    answer.status, 400,
                    "case {dialect}/{case}/{position}: expected 400, got {} {:?}",
                    answer.status, answer.body
                );
                if let Some(expected_code) = expected_code {
                    assert_eq!(
                        answer.error_code(),
                        *expected_code,
                        "case {dialect}/{case}/{position}: expected {expected_code}, got {:?}",
                        answer.body
                    );
                }
                assert!(
                    answer.body.get("Content").is_none(),
                    "case {dialect}/{case}/{position}: a refusal carries no Content, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE: `Type` is one of the modelled values; anything else is refused as a client
/// error.
#[tokio::test]
async fn a_type_outside_the_modelled_enum_is_refused() {
    let emulator = Emulator::new();
    // Rows: a dialect this service does not model, a case-shifted spelling of one it
    // does, and the empty string. The error CODE depends on whether the boundary or
    // the handler refuses it; the status is 400 either way.
    for case in ["OpenApi4", "openapi3", "JSONSchemaDraft7", ""] {
        let answer = emulator.discover(&[json!({"a": 1})], case).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
    }

    // And every value the model DOES declare is served.
    for dialect in TYPE_VALUES {
        let answer = emulator.discover(&[json!({"a": 1})], dialect).await;
        assert_eq!(
            answer.status, 200,
            "case {dialect}: a modelled Type must be served, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: discovery reports what the sample says and nothing else — the same sample
/// always yields the same document, the order of the events does not matter, and the
/// call leaves the service's state exactly as it found it.
#[tokio::test]
async fn discovery_is_deterministic_and_changes_nothing() {
    let emulator = Emulator::new();
    let events = [
        json!({"a": 1, "b": "x", "c": [1, 2]}),
        json!({"a": 2, "c": [], "d": null}),
    ];
    let reversed = [events[1].clone(), events[0].clone()];

    for dialect in TYPE_VALUES {
        let first = emulator.discover(&events, dialect).await.str_at("Content");
        let again = emulator.discover(&events, dialect).await.str_at("Content");
        assert_eq!(
            first, again,
            "case {dialect}: two calls on one sample must return the same document"
        );
        let swapped = emulator
            .discover(&reversed, dialect)
            .await
            .str_at("Content");
        assert_eq!(
            first, swapped,
            "case {dialect}: the sample's order must not change the inferred schema"
        );
    }

    // Discovery names no resource and creates none: the store is untouched, so the
    // caller's registry listing is as empty after the calls as it was before them.
    // Scoped to `Local`: an unscoped listing also reports the service-owned
    // catalogue, which is there whether or not anybody calls discovery.
    let listed = emulator
        .call("GET", "/v1/registries?scope=Local", None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListRegistries, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["Registries"],
        json!([]),
        "discovery must not have created anything, got {:?}",
        listed.body
    );
}

/// RULE: the discovered document is schema SOURCE — the same thing `CreateSchema`
/// takes as `Content` — so a discovered schema round-trips through the registry
/// unchanged.
#[tokio::test]
async fn a_discovered_schema_round_trips_through_create_and_describe() {
    let emulator = Emulator::new();
    let registry = "discovered";
    let created = emulator
        .call(
            "POST",
            &format!("/v1/registries/name/{registry}"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateRegistry, got {} {:?}",
        created.status, created.body
    );

    for dialect in TYPE_VALUES {
        let discovered = emulator
            .discover(
                &[json!({"detail": {"id": "7"}, "time": "2024-05-17T01:02:03Z"})],
                dialect,
            )
            .await
            .str_at("Content");

        let schema_uri = format!("/v1/registries/name/{registry}/schemas/name/{dialect}");
        let stored = emulator
            .call(
                "POST",
                &schema_uri,
                Some(json!({"Content": discovered, "Type": dialect})),
            )
            .await;
        assert_eq!(
            stored.status, 201,
            "case {dialect}: expected 201 from CreateSchema, got {} {:?}",
            stored.status, stored.body
        );

        let described = emulator.call("GET", &schema_uri, None).await;
        assert_eq!(
            described.status, 200,
            "case {dialect}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("Content"),
            discovered,
            "case {dialect}: the registry must hold the discovered document verbatim"
        );
        assert_eq!(
            described.str_at("Type"),
            *dialect,
            "case {dialect}: the stored schema keeps the discovered dialect"
        );
    }
}
