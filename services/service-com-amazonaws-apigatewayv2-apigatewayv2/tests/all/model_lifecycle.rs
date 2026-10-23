// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Model` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateModel` persists a model under its api and answers 201 with the `ModelId`
//!   it minted**, reporting the `Name`, `ContentType`, `Description` and `Schema` the
//!   record holds; `GetModel` reports that same record, member for member, with the
//!   schema document verbatim. A model belongs to the ONE api that owns it.
//! * **A `Schema` that is not a schema document of the model's content type is refused
//!   400 `BadRequestException`**, and nothing is persisted. For a JSON model — one whose
//!   content type is a JSON media type, or that names none — the schema is a JSON Schema
//!   document ("For application/json models, this should be JSON schema draft 4 model",
//!   `CreateModel$Schema`), and "A JSON Schema is a JSON document, and that document
//!   MUST be an object" (JSON Schema draft-04, section 3.2). A model of another content
//!   type carries its schema in that content type's own language, which this service has
//!   no reader for, so such a schema is stored verbatim.
//! * **A `Name` that is not alphanumeric is refused 400** ("The name of the model. Must
//!   be alphanumeric", `CreateModel$Name`), and a name another model of the same api
//!   already carries is refused 409 `ConflictException`.
//! * **An api holds at most `MAX_MODELS_PER_API` models**, after which a create is
//!   refused 429 `TooManyRequestsException`; the bound is per api.
//!
//! The rows vary what each rule generalises over: the content type (absent, JSON, a
//! `+json` suffixed media type, one with parameters, a non-JSON one), the schema's JSON
//! type and well-formedness, which optional members the request carries, and the name's
//! alphabet. The recorded case that filed the issue — a malformed schema refused with
//! `BadRequestException` — is one row of the refusal table.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// A draft-04 document, the schema language `application/json` models use.
fn draft4_schema() -> String {
    json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": "Order",
        "type": "object",
        "required": ["id"],
        "properties": {
            "id": { "type": "string" },
            "quantity": { "type": "integer" }
        }
    })
    .to_string()
}

/// One accepted-create row: the request, and the members the stored model must report.
struct Row {
    case: &'static str,
    request: Value,
    expected: Vec<(&'static str, Value)>,
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a draft-04 model with every optional member set",
            request: json!({
                "name": "Order",
                "contentType": "application/json",
                "description": "the body of an order request",
                "schema": draft4_schema()
            }),
            expected: vec![
                ("name", json!("Order")),
                ("contentType", json!("application/json")),
                ("description", json!("the body of an order request")),
                ("schema", json!(draft4_schema())),
            ],
        },
        Row {
            case: "only the required members: no content type and no description",
            request: json!({
                "name": "Empty",
                "schema": "{}"
            }),
            expected: vec![
                ("name", json!("Empty")),
                // Nothing is invented for a member the request left out.
                ("schema", json!("{}")),
            ],
        },
        Row {
            case: "a JSON media type carrying a charset parameter",
            request: json!({
                "name": "Parameterised",
                "contentType": "application/json; charset=UTF-8",
                "schema": "{\"type\":\"object\"}"
            }),
            expected: vec![
                ("contentType", json!("application/json; charset=UTF-8")),
                ("schema", json!("{\"type\":\"object\"}")),
            ],
        },
        Row {
            case: "a +json structured-syntax-suffix media type (RFC 6839)",
            request: json!({
                "name": "SuffixedJson",
                "contentType": "application/vnd.example.order+json",
                "schema": "{\"type\":\"array\"}"
            }),
            expected: vec![
                ("contentType", json!("application/vnd.example.order+json")),
                ("schema", json!("{\"type\":\"array\"}")),
            ],
        },
        Row {
            case: "a non-JSON content type, whose schema language is not JSON Schema",
            request: json!({
                "name": "OrderXml",
                "contentType": "application/xml",
                // Not JSON at all, and not refused: an XML model's schema is XML.
                "schema": "<xs:schema><xs:element name=\"order\"/></xs:schema>"
            }),
            expected: vec![
                ("contentType", json!("application/xml")),
                (
                    "schema",
                    json!("<xs:schema><xs:element name=\"order\"/></xs:schema>"),
                ),
            ],
        },
        Row {
            case: "an alphanumeric name mixing letters and digits",
            request: json!({
                "name": "Order2Items3",
                "contentType": "application/json",
                "schema": "{\"type\":\"object\",\"properties\":{}}"
            }),
            expected: vec![("name", json!("Order2Items3"))],
        },
        Row {
            case: "an empty JSON Schema object, which matches every document",
            request: json!({
                "name": "Anything",
                "contentType": "application/json",
                "schema": "{ }"
            }),
            // Stored verbatim, not reserialized: the whitespace survives.
            expected: vec![("schema", json!("{ }"))],
        },
    ]
}

/// Create one WebSocket api — the protocol type whose routes reference models — and
/// answer its `ApiId`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

#[tokio::test]
async fn create_model_persists_the_record_and_get_model_reports_it_back() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the models api", "models-api").await;

    let mut minted = Vec::new();
    for row in accepted_rows() {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateModel, got {status} {created}",
            row.case
        );
        let model_id = as_str(&created, "modelId");
        assert_minted_id(row.case, "ModelId", &model_id);
        minted.push(model_id.clone());

        // The same record, read back through the api that owns it.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/models/{model_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetModel, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "modelId"),
            model_id,
            "case {:?}: expected the stored model's own id, got {fetched}",
            row.case
        );

        for (member, value) in &row.expected {
            assert_eq!(
                created.get(member),
                Some(value),
                "case {:?}: expected CreateModel to report {member}={value}, got {created}",
                row.case
            );
            assert_eq!(
                fetched.get(member),
                Some(value),
                "case {:?}: expected GetModel to report {member}={value}, got {fetched}",
                row.case
            );
        }

        // A member the request did not carry is reported by neither reader.
        for member in ["contentType", "description"] {
            if row.request.get(member).is_none() {
                assert!(
                    fetched.get(member).is_none_or(Value::is_null),
                    "case {:?}: expected GetModel to report no {member}, got {fetched}",
                    row.case
                );
            }
        }
    }

    // Every create minted a distinct id within the api.
    for (index, id) in minted.iter().enumerate() {
        assert_eq!(
            minted.iter().filter(|other| *other == id).count(),
            1,
            "model {index}: expected distinct ids, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn a_schema_that_is_no_schema_document_of_its_content_type_is_refused() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the refusals api", "model-refusals-api").await;

    // Each row is a `Schema` that is not a JSON Schema document, under a content type
    // whose schema language IS JSON Schema (named, suffixed, or left to the default).
    let rows: Vec<(&str, Value)> = vec![
        (
            "a truncated JSON document",
            json!({ "name": "Truncated", "contentType": "application/json",
                    "schema": "{\"type\":" }),
        ),
        (
            "a document with trailing garbage",
            json!({ "name": "Trailing", "contentType": "application/json",
                    "schema": "{} }" }),
        ),
        (
            "prose that is not JSON at all",
            json!({ "name": "Prose", "contentType": "application/json",
                    "schema": "this is not a schema" }),
        ),
        (
            "an empty schema, which parses as no document",
            json!({ "name": "Blank", "contentType": "application/json", "schema": "" }),
        ),
        (
            "a JSON array, not an object",
            json!({ "name": "Array", "contentType": "application/json", "schema": "[]" }),
        ),
        (
            "a JSON number, not an object",
            json!({ "name": "Number", "contentType": "application/json", "schema": "42" }),
        ),
        (
            "a JSON string, not an object",
            json!({ "name": "Str", "contentType": "application/json",
                    "schema": "\"an object, please\"" }),
        ),
        (
            "a JSON boolean, not an object",
            json!({ "name": "Boolean", "contentType": "application/json", "schema": "true" }),
        ),
        (
            "a JSON null, not an object",
            json!({ "name": "Null", "contentType": "application/json", "schema": "null" }),
        ),
        (
            "a malformed document under a +json media type",
            json!({ "name": "SuffixedBad", "contentType": "application/vnd.example+json",
                    "schema": "{\"type\": }" }),
        ),
        (
            "a malformed document under no content type at all",
            json!({ "name": "DefaultBad", "schema": "{oops}" }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateModel, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // Every create above was refused, so the api persisted nothing AND minted nothing:
    // the first model it accepts carries the FIRST id of its own id bijection, which is
    // the id a fresh api's first model carries too. A refused create that had minted an
    // id, or stored a record, would have moved this api's sequence on.
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/models"),
        Some(json!({
            "name": "WellFormed",
            "contentType": "application/json",
            "schema": draft4_schema()
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for a well-formed model after the refusals, got {status} {created}"
    );
    let after_refusals = as_str(&created, "modelId");

    let fresh = create_api(&emulator, "a fresh api", "model-refusals-fresh-api").await;
    let (status, first) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{fresh}/models"),
        Some(json!({
            "name": "WellFormed",
            "contentType": "application/json",
            "schema": draft4_schema()
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for the fresh api's first model, got {status} {first}"
    );
    assert_eq!(
        after_refusals,
        as_str(&first, "modelId"),
        "expected the refused creates to have minted nothing, so the api's first \
         accepted model carries the same id a fresh api's first model does, got \
         {after_refusals:?} and {first}"
    );

    // And that one model is the ONLY model the api holds: an id nothing minted is not
    // found under it.
    let (status, body) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/models/nosuchmdl0"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "expected no model under an id nothing minted, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "expected NotFoundException, got {body}"
    );
}

#[tokio::test]
async fn a_name_that_is_not_alphanumeric_or_is_already_taken_is_refused() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the names api", "model-names-api").await;

    // "The name of the model. Must be alphanumeric" (`CreateModel$Name`): the rows vary
    // the non-alphanumeric character and where in the name it falls.
    for (case, name) in [
        ("a hyphenated name", "order-body"),
        ("an underscored name", "order_body"),
        ("a name carrying a space", "order body"),
        ("a dotted name", "order.body"),
        ("a name carrying a dollar", "$default"),
        ("a name outside ASCII", "ordré"),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(json!({
                "name": name,
                "contentType": "application/json",
                "schema": "{}"
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateModel, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // A name is unique within its api.
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/models"),
        Some(json!({ "name": "Taken", "contentType": "application/json", "schema": "{}" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from the first CreateModel, got {status} {created}"
    );
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/models"),
        Some(json!({
            "name": "Taken",
            "contentType": "application/json",
            // A different schema, the same name: the NAME is what conflicts.
            "schema": draft4_schema()
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "expected 409 from a second CreateModel of the same name, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body}"
    );

    // The conflict left the first model as it was.
    let model_id = as_str(&created, "modelId");
    let (status, fetched) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/models/{model_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetModel after the conflict, got {status} {fetched}"
    );
    assert_eq!(
        fetched.get("schema"),
        Some(&json!("{}")),
        "expected the first model's schema to survive the conflict, got {fetched}"
    );

    // The same name under ANOTHER api is a different model: uniqueness is per api.
    let other = create_api(&emulator, "another api", "model-names-other-api").await;
    let (status, created_elsewhere) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{other}/models"),
        Some(json!({ "name": "Taken", "contentType": "application/json", "schema": "{}" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for the same name under another api, got {status} {created_elsewhere}"
    );
}

#[tokio::test]
async fn an_api_holds_at_most_the_models_its_quota_allows() {
    // The per-api bound the service enforces, read from the service itself rather than
    // copied: the create past it is refused with the modelled `TooManyRequestsException`
    // ("a limit has been exceeded").
    const LIMIT: usize =
        service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_MODELS_PER_API;

    let emulator = emulator();
    let api_id = create_api(&emulator, "the quota api", "model-quota-api").await;

    for index in 0..LIMIT {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(json!({
                "name": format!("Model{index}"),
                "contentType": "application/json",
                "schema": "{}"
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "model {index} of {LIMIT}: expected 201 from CreateModel, got {status} {created}"
        );
    }

    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/models"),
        Some(json!({
            "name": "OneTooMany",
            "contentType": "application/json",
            "schema": "{}"
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 from the model past the limit of {LIMIT}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // Another api is bounded on its own: the full api does not exhaust it.
    let other = create_api(&emulator, "another api", "model-quota-other-api").await;
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{other}/models"),
        Some(json!({ "name": "Room", "contentType": "application/json", "schema": "{}" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 under another api, got {status} {created}"
    );
}

#[tokio::test]
async fn a_model_belongs_to_the_one_api_that_owns_it() {
    let emulator = emulator();
    let first = create_api(&emulator, "the owning api", "model-owner-api").await;
    let second = create_api(&emulator, "another api", "model-other-api").await;

    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{first}/models"),
        Some(json!({
            "name": "Owned",
            "contentType": "application/json",
            "schema": draft4_schema()
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateModel, got {status} {created}"
    );
    let model_id = as_str(&created, "modelId");

    for (case, uri) in [
        (
            "a model of another api",
            format!("/v2/apis/{second}/models/{model_id}"),
        ),
        (
            "a model id nothing minted",
            format!("/v2/apis/{first}/models/nosuchmdl0"),
        ),
        (
            "an api that does not exist",
            format!("/v2/apis/nosuchapi00/models/{model_id}"),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetModel, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // Creating under an api that does not exist creates nothing.
    let (status, body) = call(
        &emulator,
        "POST",
        "/v2/apis/nosuchapi00/models",
        Some(json!({ "name": "Nowhere", "contentType": "application/json", "schema": "{}" })),
    )
    .await;
    assert_eq!(
        status, 404,
        "expected 404 from CreateModel on an unknown api, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "expected NotFoundException, got {body}"
    );
}
