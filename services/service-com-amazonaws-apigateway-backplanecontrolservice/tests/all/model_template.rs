// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetModelTemplate`.
//!
//! The RULE these cases assert: **`GetModelTemplate` GENERATES the sample mapping template
//! of the addressed model — the `#set($inputRoot = $input.path('$'))` preamble followed by
//! an EXAMPLE of the payload the model's JSON Schema declares**, in which every declared
//! property carries a value of the JSON type the schema declares for it (a `string` an
//! example string, an `integer` an integer, a `number` a fractional number, a `boolean` a
//! boolean, an `object` an object of its own declared properties, an `array` one example
//! element), and where the schema itself states the value (an `enum` entry, a `default`)
//! that value is the example. The generation is a function of the STORED record, so it
//! reads the store and writes nothing: two calls on an unchanged model answer the same
//! template and the model survives them. A model the API does not hold is the modelled 404;
//! a model that declares no JSON Schema to describe, and one whose schema does not fit the
//! bounds a template is generated within, are the modelled 400.
//!
//! The rows vary what the rule generalises over: the JSON type the schema declares
//! (`object`, `array`, `string`, `integer`, `number`, `boolean`, `null`, a union of types,
//! a type implied by `properties`/`items`/`enum`, none at all), how deeply it nests, how
//! many properties it declares, how a property is named, and whether a schema is reached
//! directly, through an in-document `$ref` or through a reference to another model of the
//! API. The recorded cases that filed the issue — a `GetModelTemplate` on a JSON Schema
//! model, asserting each property's example carries the declared type — are rows of the
//! first two tables.

use serde_json::{json, Value};

use crate::models::{create_model, created_model, get_model};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// The line a generated template opens with: it binds `$inputRoot` to the root of the
/// incoming payload.
const PREAMBLE: &str = "#set($inputRoot = $input.path('$'))";

/// The output bound a template is generated within, read FROM the service rather than
/// copied, so this suite cannot drift from the bound it asserts.
const MAX_TEMPLATE_BYTES: usize = service_com_amazonaws_apigateway_backplanecontrolservice::ops::get_model_template::MAX_TEMPLATE_BYTES;

/// The step bound a template is generated within, read from the service for the same
/// reason.
const MAX_STEPS: usize =
    service_com_amazonaws_apigateway_backplanecontrolservice::ops::get_model_template::MAX_STEPS;

/// How deep a generation descends before it renders the value as one of unknown shape.
const MAX_DEPTH: usize =
    service_com_amazonaws_apigateway_backplanecontrolservice::ops::get_model_template::MAX_DEPTH;

/// A whole template: the preamble, then `body`.
fn template(body: &str) -> String {
    format!("{PREAMBLE}\n{body}")
}

/// `GetModelTemplate` for the model `name` of `api`.
async fn get_model_template(gateway: &Gateway, api: &str, name: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/models/{name}/default_template"),
            None,
        )
        .await
}

/// `GetModelTemplate`, asserting the modelled 200 and answering the template it reported.
async fn generated_template(gateway: &Gateway, api: &str, name: &str, case: &str) -> String {
    let answer = get_model_template(gateway, api, name).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetModelTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["value"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: expected a template value, got {:?}", answer.body)
        })
        .to_owned()
}

/// The generated template's body — everything after the preamble — parsed as the JSON
/// document the sample payload is.
fn parsed_sample(value: &str, case: &str) -> Value {
    let body = value
        .split_once('\n')
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("case {case}: expected a preamble and a body, got {value:?}"));
    serde_json::from_str(body)
        .unwrap_or_else(|_| panic!("case {case}: expected the sample to be JSON, got {body:?}"))
}

/// Create a model of `schema` under `api` and answer the name it was created with.
async fn model_of(gateway: &Gateway, api: &str, case: &str, name: &str, schema: &Value) -> String {
    created_model(
        gateway,
        api,
        case,
        json!({
            "name": name,
            "contentType": "application/json",
            "schema": schema.to_string(),
        }),
    )
    .await;
    name.to_owned()
}

/// One row: the content type the model is typed with, its schema, and the sample payload
/// `GetModelTemplate` must generate from that schema.
struct Row {
    case: &'static str,
    content_type: &'static str,
    schema: Value,
    expected_body: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an object declaring one property of each JSON scalar type",
            content_type: "application/json",
            schema: json!({
                "$schema": "http://json-schema.org/draft-04/schema#",
                "title": "Order",
                "required": ["id"],
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "paid": { "type": "boolean" },
                    "quantity": { "type": "integer" },
                    "total": { "type": "number" }
                }
            }),
            expected_body: [
                "{",
                "  \"id\" : \"foo\",",
                "  \"paid\" : true,",
                "  \"quantity\" : 0,",
                "  \"total\" : 0.0",
                "}",
            ]
            .join("\n"),
        },
        Row {
            // An object declaring no property constrains no member, so there is nothing to
            // describe — the schema of the Empty model an API is given.
            case: "a draft-04 object declaring no properties",
            content_type: "application/json",
            schema: json!({
                "$schema": "http://json-schema.org/draft-04/schema#",
                "title": "Empty Schema",
                "type": "object"
            }),
            expected_body: "{ }".to_string(),
        },
        Row {
            case: "an object nesting an object",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": {
                    "customer": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } }
                    },
                    "total": { "type": "number" }
                }
            }),
            expected_body: [
                "{",
                "  \"customer\" : {",
                "    \"name\" : \"foo\"",
                "  },",
                "  \"total\" : 0.0",
                "}",
            ]
            .join("\n"),
        },
        Row {
            case: "an array of objects",
            content_type: "application/json",
            schema: json!({
                "$schema": "http://json-schema.org/draft-04/schema#",
                "title": "Pets",
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "class": { "type": "string" },
                        "number": { "type": "integer" }
                    }
                }
            }),
            expected_body: [
                "[ {",
                "  \"class\" : \"foo\",",
                "  \"number\" : 0",
                "} ]",
            ]
            .join("\n"),
        },
        Row {
            case: "an array of strings",
            content_type: "application/json",
            schema: json!({ "type": "array", "items": { "type": "string" } }),
            expected_body: "[ \"foo\" ]".to_string(),
        },
        Row {
            case: "an array of arrays",
            content_type: "application/json",
            schema: json!({
                "type": "array",
                "items": { "type": "array", "items": { "type": "integer" } }
            }),
            expected_body: "[ [ 0 ] ]".to_string(),
        },
        Row {
            case: "an array declaring no items, whose elements are unconstrained",
            content_type: "application/json",
            schema: json!({ "type": "array" }),
            expected_body: "[ ]".to_string(),
        },
        Row {
            case: "an object whose property is an array of objects",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": { "sku": { "type": "string" } }
                        }
                    }
                }
            }),
            expected_body: [
                "{",
                "  \"items\" : [ {",
                "    \"sku\" : \"foo\"",
                "  } ]",
                "}",
            ]
            .join("\n"),
        },
        Row {
            // JSON Schema draft-04 section 7: a reference into the same document is a JSON
            // Pointer fragment, which resolves to the schema it names.
            case: "a property whose schema is a $ref into the document's definitions",
            content_type: "application/json",
            schema: json!({
                "$schema": "http://json-schema.org/draft-04/schema#",
                "type": "object",
                "properties": { "pet": { "$ref": "#/definitions/Pet" } },
                "definitions": {
                    "Pet": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } }
                    }
                }
            }),
            expected_body: [
                "{",
                "  \"pet\" : {",
                "    \"name\" : \"foo\"",
                "  }",
                "}",
            ]
            .join("\n"),
        },
        Row {
            // Draft-04 section 5.5.2 allows a union of types; the first that is not `null`
            // is the type the example carries.
            case: "properties declaring a union of types",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": {
                    "count": { "type": ["integer", "null"] },
                    "note": { "type": ["string", "null"] }
                }
            }),
            expected_body: ["{", "  \"count\" : 0,", "  \"note\" : \"foo\"", "}"].join("\n"),
        },
        Row {
            // A schema that RESTRICTS its instance to a set of values (section 5.5.1) has
            // already stated the value it means, so the example is the first of them —
            // whatever JSON type the entries carry.
            case: "properties restricted to enum values",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": {
                    "retries": { "enum": [2, 3] },
                    "status": { "type": "string", "enum": ["OPEN", "CLOSED"] }
                }
            }),
            expected_body: ["{", "  \"retries\" : 2,", "  \"status\" : \"OPEN\"", "}"].join("\n"),
        },
        Row {
            // And a schema that supplies a default for its instance (section 6.2) has
            // stated one too.
            case: "a property carrying a default",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": { "region": { "type": "string", "default": "us-east-1" } }
            }),
            expected_body: ["{", "  \"region\" : \"us-east-1\"", "}"].join("\n"),
        },
        Row {
            case: "a property declared null",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": { "deletedAt": { "type": "null" } }
            }),
            expected_body: ["{", "  \"deletedAt\" : null", "}"].join("\n"),
        },
        Row {
            // An object is what `properties` applies to (section 5.4.4), even where no
            // `type` says so.
            case: "an object typed only by declaring properties",
            content_type: "application/json",
            schema: json!({ "properties": { "name": { "type": "string" } } }),
            expected_body: ["{", "  \"name\" : \"foo\"", "}"].join("\n"),
        },
        Row {
            // And an array is what `items` applies to (section 5.3.1).
            case: "an array typed only by declaring items",
            content_type: "application/json",
            schema: json!({ "items": { "type": "number" } }),
            expected_body: "[ 0.0 ]".to_string(),
        },
        Row {
            // A value the schema declares nothing about is rendered as the JSON value that
            // says nothing about it.
            case: "a property whose schema declares no type at all",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": { "payload": { "description": "anything at all" } }
            }),
            expected_body: ["{", "  \"payload\" : null", "}"].join("\n"),
        },
        Row {
            case: "the empty schema, which matches every document",
            content_type: "application/json",
            schema: json!({}),
            expected_body: "null".to_string(),
        },
        Row {
            // A property name is a JSON member name whatever it carries, so the structure
            // of the generated document is the generator's and never a name's.
            case: "property names carrying a space and quotes",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": {
                    "order id": { "type": "string" },
                    "o'rder \"x\"": { "type": "integer" }
                }
            }),
            expected_body: [
                "{",
                r#"  "o'rder \"x\"" : 0,"#,
                r#"  "order id" : "foo""#,
                "}",
            ]
            .join("\n"),
        },
        Row {
            // draft-04 §5.5.3: an instance satisfying `allOf` satisfies EVERY subschema, so
            // the example describes what all of them declare together — here a named
            // definition composed with a property of the schema's own.
            case: "an object composed with allOf",
            content_type: "application/json",
            schema: json!({
                "$schema": "http://json-schema.org/draft-04/schema#",
                "type": "object",
                "allOf": [
                    { "$ref": "#/definitions/Audited" },
                    {
                        "type": "object",
                        "properties": { "quantity": { "type": "integer" } }
                    }
                ],
                "properties": { "sku": { "type": "string" } },
                "definitions": {
                    "Audited": {
                        "type": "object",
                        "properties": { "createdBy": { "type": "string" } }
                    }
                }
            }),
            expected_body: [
                "{",
                "  \"createdBy\" : \"foo\",",
                "  \"quantity\" : 0,",
                "  \"sku\" : \"foo\"",
                "}",
            ]
            .join("\n"),
        },
        Row {
            // §§5.5.4–5.5.5: an instance satisfying `anyOf`/`oneOf` satisfies one branch, so
            // the example describes the first of them — which is also what types the value
            // where the outer schema states no type at all.
            case: "a property composed with oneOf",
            content_type: "application/json",
            schema: json!({
                "type": "object",
                "properties": {
                    "amount": { "oneOf": [{ "type": "number" }, { "type": "string" }] },
                    "tag": { "anyOf": [{ "type": "string" }, { "type": "null" }] }
                }
            }),
            expected_body: [
                "{",
                "  \"amount\" : 0.0,",
                "  \"tag\" : \"foo\"",
                "}",
            ]
            .join("\n"),
        },
        Row {
            // A `+json` media type (RFC 6839) names JSON Schema as its schema language
            // too, so a template is generated for it — the same rule one level more
            // general than `application/json`.
            case: "a +json suffixed media type carrying a parameter",
            content_type: "application/vnd.example.order+json; charset=utf-8",
            schema: json!({
                "type": "object",
                "properties": { "sku": { "type": "string" } }
            }),
            expected_body: ["{", "  \"sku\" : \"foo\"", "}"].join("\n"),
        },
    ]
}

#[tokio::test]
async fn get_model_template_generates_the_sample_the_models_schema_declares() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template").await;

    // Every row is created on ONE API, so each generation runs against a store holding
    // models of every other shape beside the one it names.
    let mut created: Vec<(&'static str, String, String)> = Vec::new();
    for (index, row) in rows().into_iter().enumerate() {
        let name = format!("Template{index}");
        created_model(
            &gateway,
            &api,
            row.case,
            json!({
                "name": name.clone(),
                "contentType": row.content_type,
                "schema": row.schema.to_string(),
            }),
        )
        .await;
        created.push((row.case, name, template(&row.expected_body)));
    }

    for (case, name, expected) in &created {
        let value = generated_template(&gateway, &api, name, case).await;
        assert_eq!(
            &value, expected,
            "case {case:?}: expected the sample its schema declares {expected:?}, got \
             {value:?}"
        );
        assert!(
            value.starts_with(PREAMBLE),
            "case {case:?}: expected the template to open with {PREAMBLE:?}, got {value:?}"
        );
        // Whatever the schema declares, the generated sample is a JSON document.
        parsed_sample(&value, case);
    }
}

#[tokio::test]
async fn every_example_carries_the_json_type_its_schema_declares() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template-types").await;

    // One property per declared type, each checked against what the parsed sample must
    // carry for it. The rows span every type a JSON Schema can declare for a value, plus
    // the two forms that state a value outright.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, Value, fn(&Value) -> bool)> = vec![
        ("a string", json!({ "type": "string" }), |value| value.is_string()),
        ("an integer", json!({ "type": "integer" }), |value| value.is_i64()),
        ("a number", json!({ "type": "number" }), |value| {
            value.is_f64() && !value.is_i64()
        }),
        ("a boolean", json!({ "type": "boolean" }), |value| value.is_boolean()),
        ("a null", json!({ "type": "null" }), |value| value.is_null()),
        (
            "an object",
            json!({ "type": "object", "properties": { "inner": { "type": "string" } } }),
            |value| value.is_object(),
        ),
        (
            "an array",
            json!({ "type": "array", "items": { "type": "integer" } }),
            |value| value.is_array(),
        ),
        ("an integer enum", json!({ "enum": [7, 8] }), |value| value.is_i64()),
        (
            "a string default",
            json!({ "type": "string", "default": "set" }),
            |value| value.is_string(),
        ),
        (
            "a union of a number and null",
            json!({ "type": ["number", "null"] }),
            |value| value.is_f64(),
        ),
    ];

    let properties: serde_json::Map<String, Value> = rows
        .iter()
        .enumerate()
        .map(|(index, (_, schema, _))| (format!("member{index}"), schema.clone()))
        .collect();
    let name = model_of(
        &gateway,
        &api,
        "one model declaring every type",
        "EveryType",
        &json!({ "type": "object", "properties": properties }),
    )
    .await;

    let value = generated_template(&gateway, &api, &name, "one model declaring every type").await;
    let sample = parsed_sample(&value, "one model declaring every type");
    for (index, (case, _, carries)) in rows.iter().enumerate() {
        let member = format!("member{index}");
        let example = &sample[&member];
        assert!(
            carries(example),
            "case {case}: expected {member} to carry the declared type, got {example:?}"
        );
    }
}

#[tokio::test]
async fn a_generated_sample_resolves_the_model_references_the_schema_carries() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template-references").await;

    // The reference form API Gateway addresses another model of the same API by: the
    // model's own URL under the API that holds it.
    let reference =
        |model: &str| format!("https://apigateway.amazonaws.com/restapis/{api}/models/{model}");

    model_of(
        &gateway,
        &api,
        "the referenced model",
        "Leg",
        &json!({ "type": "object", "properties": { "length": { "type": "number" } } }),
    )
    .await;

    // Rows differ along WHERE the reference sits in the document: a referenced property, a
    // reference inside a list, and a reference this API cannot resolve, whose value is then
    // one the schema declares nothing about.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a referenced property",
            json!({
                "type": "object",
                "properties": { "front": { "$ref": reference("Leg") } }
            }),
            ["{", "  \"front\" : {", "    \"length\" : 0.0", "  }", "}"].join("\n"),
        ),
        (
            "a reference inside a list",
            json!({
                "type": "object",
                "properties": {
                    "legs": { "type": "array", "items": { "$ref": reference("Leg") } }
                }
            }),
            [
                "{",
                "  \"legs\" : [ {",
                "    \"length\" : 0.0",
                "  } ]",
                "}",
            ]
            .join("\n"),
        ),
        (
            "a reference to a model this API does not hold",
            json!({
                "type": "object",
                "properties": { "front": { "$ref": reference("Wing") } }
            }),
            ["{", "  \"front\" : null", "}"].join("\n"),
        ),
    ];

    for (index, (case, schema, expected_body)) in rows.into_iter().enumerate() {
        let name = model_of(&gateway, &api, case, &format!("Referencing{index}"), &schema).await;
        let value = generated_template(&gateway, &api, &name, case).await;
        assert_eq!(
            value,
            template(&expected_body),
            "case {case:?}: expected {:?}, got {value:?}",
            template(&expected_body)
        );
    }
}

#[tokio::test]
async fn generating_a_template_changes_nothing_a_later_caller_sees() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template-read").await;

    let schema = json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "type": "object",
        "properties": { "id": { "type": "string" } }
    })
    .to_string();
    let stored = created_model(
        &gateway,
        &api,
        "a read model",
        json!({
            "name": "ReadOnly",
            "contentType": "application/json",
            "description": "the model the template is generated from",
            "schema": schema.clone(),
        }),
    )
    .await;

    let mut answers: Vec<String> = Vec::new();
    for attempt in 0..3 {
        answers.push(
            generated_template(&gateway, &api, "ReadOnly", &format!("attempt {attempt}")).await,
        );
    }
    for (attempt, answer) in answers.iter().enumerate() {
        assert_eq!(
            answer, &answers[0],
            "case attempt {attempt}: expected the same template every call, got {answer:?} \
             after {:?}",
            answers[0]
        );
    }

    // The record itself survived the generations, member for member.
    let read = get_model(&gateway, &api, "ReadOnly", false).await;
    assert_eq!(
        read.status, 200,
        "case GetModel after the generations: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body, stored,
        "case GetModel after the generations: expected the stored model {stored:?}, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_template_is_asked_of_the_one_api_that_holds_the_model() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template-owner").await;
    let (other, _other_root) = create_api(&gateway, "model-template-other").await;

    model_of(
        &gateway,
        &api,
        "the held model",
        "Held",
        &json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
    )
    .await;

    // One unknown identifier at a time, and the name of a model the OTHER API holds: a
    // model is addressed inside the API that holds it, never account-wide.
    let missing = "zzzzzzzzzz";
    for (case, target_api, model, named) in [
        ("a model of another API", other.as_str(), "Held", "Held"),
        ("a model name nothing created", api.as_str(), "Nope", "Nope"),
        ("an API that does not exist", missing, "Held", missing),
        ("an unknown API and model", missing, "Nope", missing),
    ] {
        let answer = get_model_template(&gateway, target_api, model).await;
        assert_failure(&answer, 404, NOT_FOUND, case);
        // The refusal names the identifier the REQUEST supplied, which is what the caller
        // can act on.
        assert!(
            answer.body["message"]
                .as_str()
                .is_some_and(|message| message.contains(named)),
            "case {case}: expected the refusal to name {named:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_model_declaring_no_json_schema_has_no_generated_template() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template-languages").await;

    // Rows differ along WHY the model declares no JSON Schema to describe a payload from: a
    // schema written in another content type's language, a document that parses as JSON
    // under a content type that is not JSON (so it was never declared to be a JSON Schema),
    // and a model created with no schema at all, under either kind of content type.
    for (index, (case, content_type, schema)) in [
        (
            "an XML schema under an XML content type",
            "application/xml",
            Some("<xs:schema><xs:element name=\"order\"/></xs:schema>"),
        ),
        (
            "a JSON document under an XML content type",
            "application/xml",
            Some("{\"type\":\"object\",\"properties\":{\"id\":{\"type\":\"string\"}}}"),
        ),
        ("a plain-text model", "text/plain", Some("one order per line")),
        ("a JSON model with no schema at all", "application/json", None),
        ("an XML model with no schema at all", "application/xml", None),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("Lang{index}");
        let mut request = json!({ "name": name.clone(), "contentType": content_type });
        if let Some(schema) = schema {
            request["schema"] = json!(schema);
        }
        created_model(&gateway, &api, case, request).await;

        let answer = get_model_template(&gateway, &api, &name).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&name),
            "case {case}: expected the refusal to name the model {name:?}, got {message:?}"
        );
    }

    // The refusals cost the API nothing: a model that does declare a JSON Schema is still
    // answered on the same API.
    let name = model_of(
        &gateway,
        &api,
        "a JSON model beside them",
        "Fits",
        &json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
    )
    .await;
    let value = generated_template(&gateway, &api, &name, "a JSON model beside them").await;
    assert_eq!(
        value,
        template(&["{", "  \"id\" : \"foo\"", "}"].join("\n")),
        "case a JSON model beside them: expected the sample its schema declares, got {value:?}"
    );
}

#[tokio::test]
async fn a_schema_whose_sample_would_not_fit_the_bounds_is_refused_not_expanded() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-template-bounded").await;

    // Each row is a small schema whose sample, expanded without a bound, would be
    // enormous: the rows vary HOW it grows — a branching self-reference (exponential in the
    // branching factor), a branching reference into a definition, and arrays whose items
    // refer back to the whole document.
    let mut branching = serde_json::Map::new();
    for index in 0..10 {
        branching.insert(format!("child{index}"), json!({ "$ref": "#" }));
    }
    let mut fanning = serde_json::Map::new();
    for index in 0..8 {
        fanning.insert(format!("node{index}"), json!({ "$ref": "#/definitions/Node" }));
    }
    let mut nesting = serde_json::Map::new();
    for index in 0..10 {
        nesting.insert(
            format!("list{index}"),
            json!({ "type": "array", "items": { "$ref": "#" } }),
        );
    }
    let rows: Vec<(&str, Value)> = vec![
        (
            "ten properties that each refer back to the whole document",
            json!({ "type": "object", "properties": Value::Object(branching) }),
        ),
        (
            "a definition whose properties refer back to that definition",
            json!({
                "type": "object",
                "properties": { "root": { "$ref": "#/definitions/Node" } },
                "definitions": {
                    "Node": { "type": "object", "properties": Value::Object(fanning) }
                }
            }),
        ),
        (
            // The same growth through arrays: each element is one example, so an array
            // whose items refer back to the document re-expands it once per property.
            "ten arrays whose items refer back to the whole document",
            json!({ "type": "object", "properties": Value::Object(nesting) }),
        ),
    ];

    for (index, (case, schema)) in rows.into_iter().enumerate() {
        let name = model_of(&gateway, &api, case, &format!("Bounded{index}"), &schema).await;
        let answer = get_model_template(&gateway, &api, &name).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        // The refusal names the bound it hit, so the caller can see which one.
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&MAX_STEPS.to_string())
                || message.contains(&MAX_TEMPLATE_BYTES.to_string()),
            "case {case}: expected the refusal to name the bound it hit, got {message:?}"
        );
    }

    // The budget covers the work of READING the schema, not only of rendering a value: a
    // flat object declaring more property names than the step bound is refused on that
    // bound, while one well inside it is answered. A wide object re-read per render is how
    // a small request would otherwise buy unbounded work.
    for (case, properties, refused) in [
        ("a flat object inside the step bound", 500, false),
        ("a flat object past the step bound", MAX_STEPS + 1, true),
    ] {
        // `{}` as each member's schema keeps the document inside the published 400 KB
        // maximum a model's schema may carry, so what the row varies is the property COUNT.
        let members: serde_json::Map<String, Value> = (0..properties)
            .map(|index| (format!("m{index}"), json!({})))
            .collect();
        let name = model_of(
            &gateway,
            &api,
            case,
            &format!("Wide{properties}"),
            &json!({ "type": "object", "properties": members }),
        )
        .await;
        let answer = get_model_template(&gateway, &api, &name).await;
        if refused {
            assert_failure(&answer, 400, BAD_REQUEST, case);
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&MAX_STEPS.to_string()),
                "case {case}: expected the refusal to name the step bound, got {message:?}"
            );
        } else {
            let value = generated_template(&gateway, &api, &name, case).await;
            let sample = parsed_sample(&value, case);
            assert_eq!(
                sample.as_object().map(serde_json::Map::len),
                Some(properties),
                "case {case}: expected every declared member in the sample, got {sample:?}"
            );
        }
    }

    // A composition is read WHOLE or refused: no subschema is silently dropped. The rows
    // vary the number of subschemas an `allOf` names — well past the handful a schema
    // usually composes — so a sample missing a composed member would fail here.
    for (case, subschemas) in [
        ("a composition of three subschemas", 3usize),
        ("a composition of forty subschemas", 40usize),
    ] {
        let composition: Vec<Value> = (0..subschemas)
            .map(|index| {
                json!({
                    "type": "object",
                    "properties": { format!("part{index}"): { "type": "integer" } }
                })
            })
            .collect();
        let name = model_of(
            &gateway,
            &api,
            case,
            &format!("Composed{subschemas}"),
            &json!({ "type": "object", "allOf": composition }),
        )
        .await;
        let value = generated_template(&gateway, &api, &name, case).await;
        let sample = parsed_sample(&value, case);
        for index in 0..subschemas {
            let member = format!("part{index}");
            assert_eq!(
                sample.get(&member),
                Some(&json!(0)),
                "case {case}: expected {member} from the composed subschema, got {sample:?}"
            );
        }
    }

    // A schema that refers to ITSELF but does not branch is answered: the descent is
    // bounded, so the generation terminates and the deepest value is the one nothing more
    // is known about.
    let name = model_of(
        &gateway,
        &api,
        "a recursive model",
        "Recursive",
        &json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "child": { "$ref": "#" }
            }
        }),
    )
    .await;
    let value = generated_template(&gateway, &api, &name, "a recursive model").await;
    parsed_sample(&value, "a recursive model");
    let depth = value.matches("\"child\" : ").count();
    assert!(
        (1..=MAX_DEPTH).contains(&depth),
        "case a recursive model: expected the descent to be bounded by {MAX_DEPTH}, got \
         {depth} levels in {value:?}"
    );
    assert!(
        value.len() <= MAX_TEMPLATE_BYTES,
        "case a recursive model: expected the template to fit {MAX_TEMPLATE_BYTES} bytes, \
         got {} bytes",
        value.len()
    );

    // The API is still serving after the refusals: a model created on it afterwards is
    // stored and answered.
    let answer = create_model(
        &gateway,
        &api,
        json!({
            "name": "AfterTheRefusals",
            "contentType": "application/json",
            "schema": "{\"type\":\"object\",\"properties\":{\"id\":{\"type\":\"string\"}}}",
        }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case after the refusals: expected 201 from CreateModel, got {} with {:?}",
        answer.status, answer.body
    );
    let value = generated_template(&gateway, &api, "AfterTheRefusals", "after the refusals").await;
    assert_eq!(
        value,
        template(&["{", "  \"id\" : \"foo\"", "}"].join("\n")),
        "case after the refusals: expected the sample its schema declares, got {value:?}"
    );
}
