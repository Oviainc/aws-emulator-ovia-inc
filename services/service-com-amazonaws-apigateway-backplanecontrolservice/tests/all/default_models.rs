// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `Model` records every REST API is CREATED with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `GetModel` / `CreateModel` helpers are this crate's existing harness,
//! reused from `rest_api_lifecycle` and `models`.
//!
//! The RULE under test: **a REST API is created holding API Gateway's two default models —
//! `Empty`, which types a JSON payload of no declared structure, and `Error`, which types
//! the error payload a gateway response carries — so every create path answers an API whose
//! `GetModels` already reports them and whose `GetModel` serves each by name.** They are
//! ordinary stored records from that moment: each carries the content type, description and
//! draft-04 schema API Gateway documents for it, an id of the form this service mints every
//! model id in, and the creation ranks that put them before anything a caller creates — and
//! a caller may read, generate a template from, reference, patch and delete them like any
//! other model.
//!
//! The rows vary what the rule generalises over: WHICH create path made the API
//! (`CreateRestApi` with only the required member, `CreateRestApi` setting every member it
//! takes, `CreateRestApi` cloning another API, and `ImportRestApi` from an external
//! definition), and WHICH of the two defaults is read. The recorded case that filed the
//! issue — a create followed by `GetModel` for `Empty` — is one row of the first table.

use serde_json::{json, Value};

use crate::models::{create_model, created_model, default_model_names, get_model};
use crate::rest_api_lifecycle::{create_api, page_items, string_member, Answer, Gateway};

/// The content type both defaults describe a payload of.
const JSON: &str = "application/json";

/// What API Gateway documents for one default model: its name, the description it is stored
/// with, and the `title` its draft-04 schema declares.
///
/// Source: the API Gateway developer guide's own account of the two models a new API holds
/// ("Empty model", titled `Empty Schema`, and "Error model", titled `Error Schema`, which
/// declares the one `message` string an error payload carries).
struct DefaultModel {
    name: &'static str,
    description: &'static str,
    title: &'static str,
    /// The property names the schema declares, in order.
    properties: &'static [&'static str],
}

/// The two defaults, in the order a create makes them.
const DEFAULTS: &[DefaultModel] = &[
    DefaultModel {
        name: "Empty",
        description: "This is a default empty schema model",
        title: "Empty Schema",
        properties: &[],
    },
    DefaultModel {
        name: "Error",
        description: "This is a default error schema model",
        title: "Error Schema",
        properties: &["message"],
    },
];

/// The line a generated mapping template opens with.
const PREAMBLE: &str = "#set($inputRoot = $input.path('$'))";

/// The example string a generated sample payload fills a string-typed leaf with — the one
/// rule the `model_template` suite pins, asserted here so a default model's template is
/// held to it too.
const SAMPLE_STRING: &str = "foo";

/// An external API definition declaring no model of its own, so what an imported API holds
/// is what the create seeded.
fn definition(title: &str) -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": title, "version": "2026-01-01" },
        "paths": {
            "/widgets": {
                "get": { "responses": { "200": { "description": "ok" } } }
            }
        }
    })
    .to_string()
}

/// `GetModels` for `api`, asserting the modelled 200 and answering the items it reports.
async fn listed(gateway: &Gateway, api: &str, case: &str) -> Vec<Value> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/models?limit=500"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetModels, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
}

/// `GetModelTemplate` for the model `name` of `api`.
async fn model_template(gateway: &Gateway, api: &str, name: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/models/{name}/default_template"),
            None,
        )
        .await
}

/// Every API this suite stands up, one per create path, as `(case, api id)`.
async fn apis_of_every_create_path(gateway: &Gateway) -> Vec<(&'static str, String)> {
    let mut apis: Vec<(&'static str, String)> = Vec::new();

    let (minimal, _root) = create_api(gateway, "default-models-minimal").await;
    apis.push(("CreateRestApi with only its required member", minimal.clone()));

    // A create that sets every member it takes: the defaults do not depend on what the
    // request said about the API.
    let full = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({
                "name": "default-models-full",
                "description": "an API created with every member set",
                "version": "2026-01-01",
                "apiKeySource": "AUTHORIZER",
                "binaryMediaTypes": ["image/png"],
                "minimumCompressionSize": 1024,
                "disableExecuteApiEndpoint": true,
                "tags": { "team": "gateway" },
            })),
        )
        .await;
    assert_eq!(
        full.status, 201,
        "setup CreateRestApi(every member): expected 201, got {} with {:?}",
        full.status, full.body
    );
    apis.push((
        "CreateRestApi setting every member",
        string_member(&full.body, "id", "CreateRestApi(every member)"),
    ));

    // A CLONE is a created API too, so it holds the defaults of its own.
    let cloned = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "default-models-clone", "cloneFrom": minimal })),
        )
        .await;
    assert_eq!(
        cloned.status, 201,
        "setup CreateRestApi(cloneFrom): expected 201, got {} with {:?}",
        cloned.status, cloned.body
    );
    apis.push((
        "CreateRestApi cloning another API",
        string_member(&cloned.body, "id", "CreateRestApi(cloneFrom)"),
    ));

    // And so does an API created from an external definition.
    let imported = gateway
        .call_raw(
            "POST",
            "/restapis?mode=import",
            &definition("default-models-import"),
        )
        .await;
    assert_eq!(
        imported.status, 201,
        "setup ImportRestApi: expected 201, got {} with {:?}",
        imported.status, imported.body
    );
    apis.push((
        "ImportRestApi from an external definition",
        string_member(&imported.body, "id", "ImportRestApi"),
    ));

    apis
}

#[tokio::test]
async fn every_created_api_holds_the_default_models() {
    let gateway = Gateway::new();
    let apis = apis_of_every_create_path(&gateway).await;
    let mut ids_seen: Vec<String> = Vec::new();

    for (case, api) in &apis {
        // The collection reports exactly the two defaults, in the order they were created,
        // on an API whose caller has created no model of its own.
        let items = listed(&gateway, api, case).await;
        let names: Vec<String> = items
            .iter()
            .map(|item| string_member(item, "name", case))
            .collect();
        assert_eq!(
            names,
            default_model_names(),
            "case {case}: expected the default models in creation order, got {names:?}"
        );

        for (index, expected) in DEFAULTS.iter().enumerate() {
            let row = format!("{case} / {}", expected.name);
            // The single reader serves each default BY NAME, and reports the record the
            // listing reported, member for member.
            let read = get_model(&gateway, api, expected.name, false).await;
            assert_eq!(
                read.status, 200,
                "case {row}: expected 200 from GetModel, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body, items[index],
                "case {row}: expected the listed record {:?}, got {:?}",
                items[index], read.body
            );

            assert_eq!(
                read.body["contentType"].as_str(),
                Some(JSON),
                "case {row}: expected the content type {JSON:?}, got {:?}",
                read.body["contentType"]
            );
            assert_eq!(
                read.body["description"].as_str(),
                Some(expected.description),
                "case {row}: expected the description {:?}, got {:?}",
                expected.description,
                read.body["description"]
            );

            // The schema is the draft-04 document API Gateway stores for the model: it is
            // carried as TEXT, and that text reads back as the document it describes.
            let schema = read.body["schema"]
                .as_str()
                .unwrap_or_else(|| panic!("case {row}: expected a schema, got {:?}", read.body));
            let document: Value = serde_json::from_str(schema)
                .unwrap_or_else(|_| panic!("case {row}: expected JSON Schema, got {schema:?}"));
            assert_eq!(
                document["$schema"].as_str(),
                Some("http://json-schema.org/draft-04/schema#"),
                "case {row}: expected a draft-04 document, got {document}"
            );
            assert_eq!(
                document["title"].as_str(),
                Some(expected.title),
                "case {row}: expected the title {:?}, got {document}",
                expected.title
            );
            assert_eq!(
                document["type"].as_str(),
                Some("object"),
                "case {row}: expected an object schema, got {document}"
            );
            let declared: Vec<String> = document["properties"]
                .as_object()
                .map(|members| members.keys().cloned().collect())
                .unwrap_or_default();
            assert_eq!(
                declared, expected.properties,
                "case {row}: expected the declared properties {:?}, got {declared:?}",
                expected.properties
            );

            // The id is one this service minted, of the form every model id takes, and it
            // is this API's own: no two APIs share a default model's id.
            let id = string_member(&read.body, "id", &row);
            assert!(
                !id.is_empty() && id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()),
                "case {row}: expected a lowercase alphanumeric id, got {id:?}"
            );
            assert!(
                !ids_seen.contains(&id),
                "case {row}: expected an id of this API's own, got the already-seen {id:?}"
            );
            ids_seen.push(id);
        }
    }
}

#[tokio::test]
async fn a_default_model_is_an_ordinary_model_of_the_api() {
    let gateway = Gateway::new();
    let apis = apis_of_every_create_path(&gateway).await;

    for (case, api) in &apis {
        // A template is GENERATED from a default's schema, exactly as from any other
        // model's: the `Error` schema declares one string property, so the sample payload
        // carries the placeholder string a generated template fills a string leaf with.
        for expected in DEFAULTS {
            let row = format!("{case} / {}", expected.name);
            let answer = model_template(&gateway, api, expected.name).await;
            assert_eq!(
                answer.status, 200,
                "case {row}: expected 200 from GetModelTemplate, got {} with {:?}",
                answer.status, answer.body
            );
            let value = answer.body["value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {row}: expected a template, got {:?}", answer.body));
            let (preamble, body) = value
                .split_once('\n')
                .unwrap_or_else(|| panic!("case {row}: expected a preamble and a body, got {value:?}"));
            assert_eq!(
                preamble, PREAMBLE,
                "case {row}: expected the preamble {PREAMBLE:?}, got {preamble:?}"
            );
            let sample: Value = serde_json::from_str(body)
                .unwrap_or_else(|_| panic!("case {row}: expected the sample to be JSON, got {body:?}"));
            let declared: Vec<String> = sample
                .as_object()
                .map(|members| members.keys().cloned().collect())
                .unwrap_or_default();
            assert_eq!(
                declared, expected.properties,
                "case {row}: expected an example of the declared properties {:?}, got {sample}",
                expected.properties
            );
            for property in expected.properties {
                assert_eq!(
                    sample[*property].as_str(),
                    Some(SAMPLE_STRING),
                    "case {row}: expected {property:?} to carry the example string \
                     {SAMPLE_STRING:?}, got {sample}"
                );
            }
        }

        // A second create under a default's name is the collision every model name earns:
        // the name is already held by the API.
        let collided = create_model(
            &gateway,
            api,
            json!({ "name": "Empty", "contentType": JSON }),
        )
        .await;
        assert_eq!(
            collided.status, 409,
            "case {case}: expected 409 from CreateModel(Empty), got {} with {:?}",
            collided.status, collided.body
        );

        // A default is PATCHED like any other model, and the patched record reads back.
        let patched = gateway
            .call(
                "PATCH",
                &format!("/restapis/{api}/models/Error"),
                Some(json!({
                    "patchOperations": [
                        { "op": "replace", "path": "/description", "value": "ours now" }
                    ]
                })),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateModel(Error), got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            get_model(&gateway, api, "Error", false).await.body["description"].as_str(),
            Some("ours now"),
            "case {case}: expected the patched description to read back"
        );

        // And a default is DELETED like any other model, which takes it out of the
        // collection and leaves the other default standing beside a model of the caller's.
        let created = created_model(
            &gateway,
            api,
            case,
            json!({ "name": "Mine", "contentType": JSON }),
        )
        .await;
        let mine = string_member(&created, "name", case);
        let removed = gateway
            .call("DELETE", &format!("/restapis/{api}/models/Empty"), None)
            .await;
        assert_eq!(
            removed.status, 202,
            "case {case}: expected 202 from DeleteModel(Empty), got {} with {:?}",
            removed.status, removed.body
        );
        let names: Vec<String> = listed(&gateway, api, case)
            .await
            .iter()
            .map(|item| string_member(item, "name", case))
            .collect();
        assert_eq!(
            names,
            vec!["Error".to_string(), mine],
            "case {case}: expected the surviving models in creation order, got {names:?}"
        );
        assert_eq!(
            get_model(&gateway, api, "Empty", false).await.status,
            404,
            "case {case}: expected the deleted default to be gone"
        );
    }
}
