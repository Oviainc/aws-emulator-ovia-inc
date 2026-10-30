// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateModel`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the PATCH
//! wrappers and the `Model` create/read helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `patch_updates` and `models`.
//!
//! The RULE under test: `UpdateModel` applies every entry of `patchOperations` to the
//! stored `Model` record of the named API, in request order, and reports the patched
//! record — which a later `GetModel` reads back member for member — while a patch the
//! service cannot apply is refused with a modelled error and leaves the stored record
//! exactly as it was. A patched `schema` is held to the same rule the create holds one
//! to: it must describe the payload the model's content type names.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! of members, ops, content-type families and schema forms; the paths and ops the
//! resource does not support; the published 400 KB schema maximum and the description
//! character bound, each on the bound and one past it; repeated application of one list;
//! and every identifier the operation resolves.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_STRING_CHARS;

use crate::models::{
    create_model, created_model, get_model, json_schema, schema_of_length, MAX_SCHEMA_CHARS,
};
use crate::patch_updates::{op, patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// The ONE sentence API Gateway answers a `schema` that is not a schema document with,
/// whichever way the document failed to read. Written out here rather than read from the
/// service, so the test checks the text instead of restating it.
const INVALID_MODEL_SCHEMA: &str = "Invalid model specified: Validation Result: \
     warnings : [], errors : [Invalid model schema specified]";

/// The `UpdateModel`/`GetModel` URI for one model of one API.
fn model_uri(api: &str, name: &str) -> String {
    format!("/restapis/{api}/models/{name}")
}

/// `UpdateModel` on one model of `api`.
async fn update(gateway: &Gateway, api: &str, name: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &model_uri(api, name), operations).await
}

/// Create the model a row patches: `request` with `name` filled in, so every row of a
/// table gets a record of its own under the one API.
async fn baseline(gateway: &Gateway, api: &str, case: &str, name: &str, request: Value) -> Value {
    let mut request = request;
    request["name"] = json!(name);
    created_model(gateway, api, case, request).await
}

/// The model every refusal row starts from: both patchable members set to known values,
/// so a refused patch has something to have left alone.
fn baseline_request() -> Value {
    json!({
        "contentType": "application/json",
        "description": "initial description",
        "schema": json_schema("Pet", 1),
    })
}

/// Assert every `(member, expected)` pair holds in `body`; a `Value::Null` expectation is
/// the member being ABSENT, which is what a cleared optional member looks like on the wire.
fn assert_members(body: &Value, expectations: &[(&str, Value)], case: &str, source: &str) {
    for (member, expected) in expectations {
        assert_eq!(
            body[*member], *expected,
            "case {case}: {source} must report {member} as {expected:?}, got {:?}",
            body[*member]
        );
    }
}

#[tokio::test]
async fn a_patched_model_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which member the path names
    // (`description`, `schema` — the two members of a `Model` that describe the payload
    // rather than identify the record), which op the row uses, whether the record started
    // with that member set, how many ops one request carries, whether two ops target one
    // member, the content-type family the model is typed with (JSON, a `+json` structured
    // syntax suffix, XML, plain text), the form the schema document takes, and each
    // published bound.
    type Row = (
        &'static str,
        Value,
        Vec<Value>,
        Vec<(&'static str, Value)>,
    );
    let nested = json!({
        "type": "object",
        "properties": { "pet": { "type": "object", "properties": { "leg": { "type": "string" } } } },
    })
    .to_string();
    let rows: Vec<Row> = vec![
        (
            "replace the description",
            baseline_request(),
            vec![replace("/description", "changed")],
            vec![
                ("description", json!("changed")),
                ("schema", json!(json_schema("Pet", 1))),
                ("contentType", json!("application/json")),
            ],
        ),
        (
            "add a description the model had none of",
            json!({ "contentType": "application/json" }),
            vec![op("add", "/description", Some("first description"))],
            vec![
                ("description", json!("first description")),
                ("schema", Value::Null),
            ],
        ),
        (
            "remove the description of an optional member",
            baseline_request(),
            vec![op("remove", "/description", None)],
            vec![
                ("description", Value::Null),
                ("schema", json!(json_schema("Pet", 1))),
            ],
        ),
        (
            "replace the schema with another document",
            baseline_request(),
            vec![replace("/schema", &json_schema("Pet", 3))],
            vec![
                ("schema", json!(json_schema("Pet", 3))),
                ("description", json!("initial description")),
            ],
        ),
        (
            "add a schema the model had none of",
            json!({ "contentType": "application/json" }),
            vec![op("add", "/schema", Some(&json_schema("Added", 1)))],
            vec![("schema", json!(json_schema("Added", 1)))],
        ),
        (
            "remove the schema",
            baseline_request(),
            vec![op("remove", "/schema", None)],
            vec![
                ("schema", Value::Null),
                ("description", json!("initial description")),
            ],
        ),
        (
            "a nested schema document",
            baseline_request(),
            vec![replace("/schema", &nested)],
            vec![("schema", json!(nested.clone()))],
        ),
        (
            "both members in one request",
            baseline_request(),
            vec![
                replace("/description", "both"),
                replace("/schema", &json_schema("Both", 2)),
            ],
            vec![
                ("description", json!("both")),
                ("schema", json!(json_schema("Both", 2))),
            ],
        ),
        (
            "two ops on one member apply in request order",
            baseline_request(),
            vec![
                replace("/description", "first"),
                replace("/description", "second"),
            ],
            vec![("description", json!("second"))],
        ),
        (
            "a schema cleared and then set again",
            baseline_request(),
            vec![
                op("remove", "/schema", None),
                replace("/schema", &json_schema("Again", 1)),
            ],
            vec![("schema", json!(json_schema("Again", 1)))],
        ),
        (
            "a schema at the published maximum",
            baseline_request(),
            vec![replace("/schema", &schema_of_length(MAX_SCHEMA_CHARS))],
            vec![("schema", json!(schema_of_length(MAX_SCHEMA_CHARS)))],
        ),
        (
            "a description at the character bound",
            baseline_request(),
            vec![replace("/description", &"d".repeat(MAX_STRING_CHARS))],
            vec![("description", json!("d".repeat(MAX_STRING_CHARS)))],
        ),
        (
            "a description outside ASCII",
            baseline_request(),
            vec![replace("/description", "模型 ✓")],
            vec![("description", json!("模型 ✓"))],
        ),
        (
            // The JSON Schema sentence is scoped to models typed as JSON, so a model of
            // another content type takes the body it is given whatever its syntax.
            "an XML model takes a schema that is not JSON",
            json!({ "contentType": "application/xml", "schema": "<xs:schema/>" }),
            vec![replace(
                "/schema",
                "<xs:schema><xs:element name=\"pet\"/></xs:schema>",
            )],
            vec![
                (
                    "schema",
                    json!("<xs:schema><xs:element name=\"pet\"/></xs:schema>"),
                ),
                ("contentType", json!("application/xml")),
            ],
        ),
        (
            "a plain-text model takes any body as its schema",
            json!({ "contentType": "text/plain" }),
            vec![replace("/schema", "an opaque payload")],
            vec![
                ("schema", json!("an opaque payload")),
                ("contentType", json!("text/plain")),
            ],
        ),
        (
            // The same schema rule one level more general: a content type that is JSON by
            // its structured syntax suffix rather than by its subtype.
            "a +json model takes a JSON schema document",
            json!({ "contentType": "application/vnd.pet+json" }),
            vec![replace("/schema", &json_schema("Suffixed", 1))],
            vec![
                ("schema", json!(json_schema("Suffixed", 1))),
                ("contentType", json!("application/vnd.pet+json")),
            ],
        ),
        (
            // A model may carry no schema at all, so a blank document is the absent
            // member rather than a schema of whitespace — the create's own rule.
            "a blank schema clears the member",
            baseline_request(),
            vec![replace("/schema", "   ")],
            vec![("schema", Value::Null)],
        ),
        (
            "an empty list reports the record unchanged",
            baseline_request(),
            vec![],
            vec![
                ("description", json!("initial description")),
                ("schema", json!(json_schema("Pet", 1))),
                ("contentType", json!("application/json")),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-model-rows").await;

    for (index, (case, request, operations, expected)) in rows.into_iter().enumerate() {
        let name = format!("Row{index}");
        let created = baseline(&gateway, &api, case, &name, request).await;
        let updated = patch_ok(&gateway, case, &model_uri(&api, &name), operations).await;
        assert_members(&updated, &expected, case, "UpdateModel");

        // The members that identify the record are the ones the create minted and stored:
        // no patch path names them.
        for member in ["id", "name", "contentType"] {
            assert_eq!(
                updated[member], created[member],
                "case {case}: the update must report the record's own {member} {:?}, got {:?}",
                created[member], updated[member]
            );
        }

        // The patched record is what is STORED, not just what the update answered.
        let read = get_model(&gateway, &api, &name, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetModel, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, updated,
            "case {case}: GetModel must read back the patched record, got {:?} after {:?}",
            read.body, updated
        );
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    // Rows differ along the KIND of refusal: an op no single-valued member supports, an op
    // missing its value, an entry missing a member, a path naming a member that identifies
    // the record rather than describing the payload, a path naming no member at all, a
    // path that is not a JSON Pointer, a path reaching into a single-valued member, a
    // schema that does not describe a JSON payload (truncated, not JSON, a JSON value that
    // is not an object), each published bound one character past it, and a list whose LAST
    // op is refused (so the earlier ops must not be kept either).
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "move is not supported",
            vec![op("move", "/description", None)],
        ),
        (
            "copy is not supported",
            vec![json!({ "op": "copy", "path": "/description", "from": "/description" })],
        ),
        (
            "test is not supported",
            vec![op("test", "/description", None)],
        ),
        (
            "a replace with no value",
            vec![op("replace", "/description", None)],
        ),
        ("an entry with no path", vec![json!({ "op": "replace" })]),
        ("an entry with no op", vec![json!({ "path": "/description" })]),
        (
            "the name is reported but not patchable",
            vec![replace("/name", "Renamed")],
        ),
        (
            "the id is reported but not patchable",
            vec![replace("/id", "zzzzzzzzzz")],
        ),
        (
            "the content type is reported but not patchable",
            vec![replace("/contentType", "text/plain")],
        ),
        (
            "a path naming no updateable member",
            vec![replace("/nosuchmember", "x")],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("description", "x")],
        ),
        ("an empty path", vec![replace("", "x")]),
        (
            "a path reaching into a single-valued member",
            vec![replace("/description/first", "x")],
        ),
        (
            "a truncated JSON schema",
            vec![replace("/schema", "{")],
        ),
        (
            "a schema that is not JSON at all",
            vec![replace("/schema", "not a schema")],
        ),
        (
            "a schema that is a JSON array",
            vec![replace("/schema", "[{\"type\":\"object\"}]")],
        ),
        (
            "a schema that is a JSON string",
            vec![replace("/schema", "\"object\"")],
        ),
        (
            "a schema one character past the published maximum",
            vec![replace("/schema", &schema_of_length(MAX_SCHEMA_CHARS + 1))],
        ),
        (
            "a description one character past the bound",
            vec![replace("/description", &"d".repeat(MAX_STRING_CHARS + 1))],
        ),
        (
            "a list whose last op names no updateable member",
            vec![
                replace("/description", "never stored"),
                replace("/nosuchmember", "x"),
            ],
        ),
        (
            "a list whose last op carries a schema that is not a document",
            vec![
                replace("/description", "never stored"),
                replace("/schema", "not a schema"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-model-refusals").await;
    let baseline_members = vec![
        ("description", json!("initial description")),
        ("schema", json!(json_schema("Pet", 1))),
        ("contentType", json!("application/json")),
    ];

    for (index, (case, operations)) in cases.iter().enumerate() {
        let name = format!("Refused{index}");
        baseline(&gateway, &api, case, &name, baseline_request()).await;
        let answer = update(&gateway, &api, &name, operations.clone()).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        assert!(
            answer.body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message naming what to fix, got {:?}",
            answer.body
        );

        // Nothing was applied: the stored record is the one the create left behind.
        let read = get_model(&gateway, &api, &name, false).await;
        assert_members(&read.body, &baseline_members, case, "GetModel");
    }

    // A path the resource has no updateable member for is refused with the path quoted,
    // the `op` of the same entry and the paths a model publishes — the wording the
    // `patch_path_refusal_templates` suite pins for every operation that enumerates them.
    let case = "the message of an unsupported path";
    baseline(&gateway, &api, case, "Wording", baseline_request()).await;
    let answer = update(&gateway, &api, "Wording", vec![replace("/name", "Renamed")]).await;
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message,
        "Invalid patch path  '/name' specified for op 'replace'. \
         Must be one of: [/description, /schema]",
        "case {case}: expected the path, the op and the model's own paths, got {message:?}"
    );
}

#[tokio::test]
async fn every_identifier_the_update_resolves_is_a_not_found_when_it_names_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-model-notfound").await;
    let missing = "zzzzzzzzzz";
    baseline(
        &gateway,
        &api,
        "the model to patch",
        "Pet",
        baseline_request(),
    )
    .await;
    let operations = vec![replace("/description", "never applied")];

    // Rows span each identifier the operation resolves, one unknown at a time.
    assert_failure(
        &update(&gateway, missing, "Pet", operations.clone()).await,
        404,
        NOT_FOUND,
        "UpdateModel on an unknown API",
    );
    assert_failure(
        &update(&gateway, &api, "Nope", operations.clone()).await,
        404,
        NOT_FOUND,
        "UpdateModel for an unknown model",
    );

    // A model belongs to ONE API: it cannot be patched through another one, even by a name
    // that API holds no model under.
    let (other, _other_root) = create_api(&gateway, "update-model-notfound-other").await;
    assert_failure(
        &update(&gateway, &other, "Pet", operations.clone()).await,
        404,
        NOT_FOUND,
        "UpdateModel through another API",
    );

    // None of those refusals touched the record.
    let read = get_model(&gateway, &api, "Pet", false).await;
    assert_members(
        &read.body,
        &[("description", json!("initial description"))],
        "after the refusals",
        "GetModel",
    );

    // The models of a deleted API go with it, patch included.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &update(&gateway, &api, "Pet", operations).await,
        404,
        NOT_FOUND,
        "UpdateModel after DeleteRestApi",
    );
}

#[tokio::test]
async fn a_schema_that_is_not_a_document_is_refused_with_the_one_fixed_sentence() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-model-schema-message").await;

    // Rows span HOW the document fails to be a JSON Schema — unparseable JSON, text that
    // is not JSON at all, and each JSON value that is not an object — and WHICH JSON
    // content type carried it (the `application/json` subtype and the `+json` structured
    // syntax suffix). The sentence is the same for every one of them, and the same whether
    // the create or the patch wrote the member.
    let documents = [
        ("a truncated document", "{"),
        ("text that is not JSON", "not a schema"),
        ("a JSON array", "[{\"type\":\"object\"}]"),
        ("a JSON string", "\"object\""),
        ("a JSON number", "123"),
        ("a JSON null", "null"),
    ];

    for (index, content_type) in ["application/json", "application/vnd.pet+json"]
        .into_iter()
        .enumerate()
    {
        for (document_index, (document_case, schema)) in documents.iter().enumerate() {
            let case = format!("{document_case} under {content_type}");

            // The create writer of the member.
            let created = create_model(
                &gateway,
                &api,
                json!({
                    "name": format!("Created{index}x{document_index}"),
                    "contentType": content_type,
                    "schema": schema,
                }),
            )
            .await;
            assert_failure(&created, 400, BAD_REQUEST, &format!("CreateModel({case})"));
            assert_eq!(
                created.body["message"].as_str(),
                Some(INVALID_MODEL_SCHEMA),
                "case CreateModel({case}): expected {INVALID_MODEL_SCHEMA:?}, got {:?}",
                created.body["message"]
            );

            // The patch writer of the same member, on a model whose stored schema is a
            // document the create accepted.
            let name = format!("Patched{index}x{document_index}");
            baseline(
                &gateway,
                &api,
                &case,
                &name,
                json!({ "contentType": content_type, "schema": json_schema("Pet", 1) }),
            )
            .await;
            let patched = update(&gateway, &api, &name, vec![replace("/schema", schema)]).await;
            assert_failure(&patched, 400, BAD_REQUEST, &format!("UpdateModel({case})"));
            assert_eq!(
                patched.body["message"].as_str(),
                Some(INVALID_MODEL_SCHEMA),
                "case UpdateModel({case}): expected {INVALID_MODEL_SCHEMA:?}, got {:?}",
                patched.body["message"]
            );

            // The refused patch left the stored schema as the create wrote it.
            let read = get_model(&gateway, &api, &name, false).await;
            assert_members(
                &read.body,
                &[("schema", json!(json_schema("Pet", 1)))],
                &case,
                "GetModel",
            );
        }
    }
}

#[tokio::test]
async fn one_model_is_patched_and_its_siblings_are_left_alone() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-model-isolation").await;
    let (other, _other_root) = create_api(&gateway, "update-model-isolation-other").await;

    baseline(
        &gateway,
        &api,
        "the model to patch",
        "Pet",
        baseline_request(),
    )
    .await;
    let sibling = baseline(
        &gateway,
        &api,
        "the sibling model",
        "Leg",
        json!({ "contentType": "application/xml", "description": "a leg" }),
    )
    .await;
    // A model of the SAME name under another API is another record entirely.
    let elsewhere = baseline(
        &gateway,
        &other,
        "the other API's model of the same name",
        "Pet",
        json!({ "contentType": "text/plain", "description": "elsewhere" }),
    )
    .await;

    // Applying one list repeatedly lands on the same record: the patch sets members, it
    // does not accumulate.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("UpdateModel(round {round})");
        seen.push(
            patch_ok(
                &gateway,
                &case,
                &model_uri(&api, "Pet"),
                vec![
                    replace("/description", "settled"),
                    replace("/schema", &json_schema("Settled", 2)),
                ],
            )
            .await,
        );
    }
    assert_eq!(
        seen[0], seen[2],
        "case UpdateModel repeated: the same list must land on the same record, got {:?} \
         then {:?}",
        seen[0], seen[2]
    );
    assert_eq!(
        string_member(&seen[0], "name", "UpdateModel repeated"),
        "Pet",
        "case UpdateModel repeated: expected the record's own name, got {:?}",
        seen[0]["name"]
    );

    // The sibling, and the other API's model of the same name, are the records their own
    // creates left behind.
    for (case, target_api, expected) in [
        ("the sibling model", api.as_str(), &sibling),
        ("the other API's model of the same name", other.as_str(), &elsewhere),
    ] {
        let name = string_member(expected, "name", case);
        let read = get_model(&gateway, target_api, &name, false).await;
        assert_eq!(
            &read.body, expected,
            "case {case}: expected the record its create left behind {expected:?}, got {:?}",
            read.body
        );
    }
}
