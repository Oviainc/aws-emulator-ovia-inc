// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteModel`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `CreateRestApi` helper and the
//! `CreateModel`/`GetModel` helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle` and `models`.
//!
//! The RULE under test: a delete removes exactly the one stored model its
//! `restApiId`/`modelName` pair names and leaves every other record standing, so a later
//! read of that name finds nothing, the name is free for a new create and a flattened read
//! no longer inlines that schema, while every sibling still reads back member for member;
//! an identifier that names no stored record is the modelled 404 `NotFoundException`,
//! stating the sentence that identifier earns.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! spanning the content-type families a payload can carry, which optional members the
//! model was created with and the forms a schema document takes (including one whose text
//! is outside ASCII); every identifier the operation resolves, one unknown at a time (an
//! unknown API, an unknown model, a model of another API, a second delete of the same
//! name, and a delete after the API itself is gone); and that the record is really taken
//! out of the store rather than hidden from a read — the name is free for a new create and
//! a `flatten`ed read of a model that referenced it stops resolving it.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::no_such_model;

use crate::models::{create_model, created_model, get_model};
use crate::rest_api_identifier_refusals::no_such_api;
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// `DeleteModel` for `name` on `api`.
async fn delete_model(gateway: &Gateway, api: &str, name: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/restapis/{api}/models/{name}"), None)
        .await
}

/// `DeleteModel`, asserting the modelled 202 and an empty body.
async fn deleted_model(gateway: &Gateway, api: &str, name: &str, case: &str) {
    let answer = delete_model(gateway, api, name).await;
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202 from DeleteModel, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case}: DeleteModel answers no body, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the modelled 404 `NotFoundException` stating `message`.
///
/// The message is asserted and not only the code, because the two identifiers this
/// operation resolves earn DIFFERENT sentences — the API is named back
/// (`crate::rest_api_identifier_refusals::no_such_api`, the one sentence this crate's
/// suites share) while a name that holds no model earns the sentence naming that NAME
/// back (`no_such_model`, taken from the crate rather than spelled again here).
fn assert_not_found(answer: &Answer, message: &str, case: &str) {
    assert_failure(answer, 404, "NotFoundException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// A JSON Schema document titled `title`, as a caller writes one.
fn schema(title: &str) -> String {
    json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": title,
        "type": "object",
        "properties": { "member": { "type": "string" } },
    })
    .to_string()
}

/// The table every "delete one of these" case runs over. Rows differ along what the rule
/// generalises over: the content-type family of the payload the model describes
/// (`application/json`, a `+json` structured syntax suffix, a media type with a parameter,
/// XML, plain text), which optional members the create set (`description`, `schema`,
/// neither), and the form the schema document takes — including one whose text is outside
/// ASCII, which the store keeps verbatim and the delete therefore has to address by name
/// whatever its body carries.
fn model_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a JSON model with every member set",
            json!({
                "name": "Pet",
                "contentType": "application/json",
                "description": "a pet",
                "schema": schema("Pet"),
            }),
        ),
        (
            "a JSON model with only its required members",
            json!({ "name": "Bare", "contentType": "application/json" }),
        ),
        (
            "a model whose content type carries the +json suffix",
            json!({
                "name": "Suffixed",
                "contentType": "application/vnd.pet+json",
                "schema": schema("Suffixed"),
            }),
        ),
        (
            "a model whose content type carries a parameter",
            json!({
                "name": "Parameterized",
                "contentType": "application/json; charset=utf-8",
                "schema": schema("Parameterized"),
            }),
        ),
        (
            "an XML model whose schema is not JSON",
            json!({
                "name": "PetXml",
                "contentType": "application/xml",
                "schema": "<xs:schema><xs:element name=\"pet\"/></xs:schema>",
            }),
        ),
        (
            "a plain-text model with no schema and a description",
            json!({
                "name": "Plain",
                "contentType": "text/plain",
                "description": "an opaque payload",
            }),
        ),
        (
            "a JSON model whose schema text is outside ASCII",
            json!({
                "name": "Unicode2",
                "contentType": "application/json",
                "description": "日本語 ✓ описание",
                "schema": schema("ペット — 🐈 schéma"),
            }),
        ),
    ]
}

#[tokio::test]
async fn a_delete_removes_the_named_model_and_leaves_every_sibling_standing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-delete").await;

    // Every row of the table is created on ONE API, so each delete runs against a store
    // that holds models of every other shape beside the one it names.
    let mut records: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in model_table() {
        let created = created_model(&gateway, &api, case, request).await;
        let name = string_member(&created, "name", case);
        records.push((case, name, created));
    }

    // Delete them one at a time. After each delete the named model is gone and every model
    // not yet deleted still reads back member for member.
    for index in 0..records.len() {
        let (case, name, _) = &records[index];
        deleted_model(&gateway, &api, name, case).await;
        assert_not_found(
            &get_model(&gateway, &api, name, false).await,
            &no_such_model(name),
            &format!("GetModel after deleting {case}"),
        );

        for (other_case, other_name, created) in &records[index + 1..] {
            let read = get_model(&gateway, &api, other_name, false).await;
            assert_eq!(
                read.status, 200,
                "case GetModel({other_case}) after deleting {case}: expected 200, got {} \
                 with {:?}",
                read.status, read.body
            );
            assert_eq!(
                &read.body, created,
                "case GetModel({other_case}) after deleting {case}: expected the stored \
                 model {created:?}, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_record_is_a_not_found_on_delete() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-delete-notfound").await;
    let (other, _other_root) = create_api(&gateway, "model-delete-notfound-other").await;
    let missing = "zzzzzzzzzz";

    let kept = created_model(
        &gateway,
        &api,
        "the model kept",
        json!({
            "name": "Kept",
            "contentType": "application/json",
            "schema": schema("Kept"),
        }),
    )
    .await;
    // A model is identified by its name UNDER an API, so the other API's model carries the
    // same name on purpose: a delete addressed through one API must not reach the other's.
    let elsewhere = created_model(
        &gateway,
        &other,
        "the model of the other API",
        json!({ "name": "Kept", "contentType": "application/xml" }),
    )
    .await;
    // A name ONLY the other API holds, so a delete of it through the first API resolves
    // nothing while the record itself is untouched.
    let only_elsewhere = created_model(
        &gateway,
        &other,
        "the name only the other API holds",
        json!({ "name": "Elsewhere", "contentType": "text/plain" }),
    )
    .await;

    // Rows span each identifier this operation resolves, one unknown at a time, each
    // asserted on the sentence its OWN identifier earns: the API is resolved first, so a
    // request whose API names nothing is refused for the API however its other identifier
    // resolves, and a request whose API does resolve is refused for the model.
    let unknown_api = no_such_api(missing);
    let unknown_nope = no_such_model("Nope");
    let unknown_elsewhere = no_such_model("Elsewhere");
    // A name no API could even hold (a model name is alphanumeric, so a create under this
    // one is refused): the refusal still names the value the request carried, because the
    // name is what the caller has to change.
    let unknown_non_ascii = no_such_model("Pét");
    for (case, target_api, target_model, message) in [
        (
            "DeleteModel on an unknown API",
            missing,
            "Kept",
            unknown_api.as_str(),
        ),
        (
            "DeleteModel for an unknown model",
            api.as_str(),
            "Nope",
            unknown_nope.as_str(),
        ),
        (
            "DeleteModel on an unknown API and model",
            missing,
            "Nope",
            unknown_api.as_str(),
        ),
        (
            "DeleteModel for a name only another API holds",
            api.as_str(),
            "Elsewhere",
            unknown_elsewhere.as_str(),
        ),
        (
            "DeleteModel for a name outside ASCII",
            api.as_str(),
            "Pét",
            unknown_non_ascii.as_str(),
        ),
    ] {
        assert_not_found(
            &delete_model(&gateway, target_api, target_model).await,
            message,
            case,
        );
    }

    // None of those refusals took anything away: both models still read back.
    for (case, target_api, created) in [
        ("the model kept", api.as_str(), &kept),
        ("the model of the other API", other.as_str(), &elsewhere),
    ] {
        let read = get_model(&gateway, target_api, "Kept", false).await;
        assert_eq!(
            &read.body, created,
            "case GetModel({case}) after the refusals: expected {created:?}, got {:?}",
            read.body
        );
    }

    let read = get_model(&gateway, &other, "Elsewhere", false).await;
    assert_eq!(
        read.body, only_elsewhere,
        "case GetModel(the name only the other API holds) after the refusals: expected \
         {only_elsewhere:?}, got {:?}",
        read.body
    );

    // Deleting through one API leaves the other API's model of that name standing.
    deleted_model(&gateway, &api, "Kept", "the model kept").await;
    let read = get_model(&gateway, &other, "Kept", false).await;
    assert_eq!(
        read.body, elsewhere,
        "case GetModel(the other API) after the delete: expected {elsewhere:?}, got {:?}",
        read.body
    );

    // A delete is not idempotent: the second one names no model, because the first took
    // the record away.
    for round in 0..2 {
        assert_not_found(
            &delete_model(&gateway, &api, "Kept").await,
            &no_such_model("Kept"),
            &format!("DeleteModel again (round {round})"),
        );
    }

    // The models of a deleted API go with it, so a delete addressed through that API
    // resolves nothing either — and the API itself is what no longer resolves, so this is
    // the API's own sentence.
    let dropped = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    assert_not_found(
        &delete_model(&gateway, &api, "Kept").await,
        &no_such_api(&api),
        "DeleteModel after DeleteRestApi",
    );
}

#[tokio::test]
async fn a_deleted_model_is_out_of_the_store_rather_than_hidden_from_a_read() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "model-delete-store").await;

    // The reference form API Gateway addresses another model of the same API by: the
    // model's own URL under the API that holds it.
    let reference = format!("https://apigateway.amazonaws.com/restapis/{api}/models/Leg");
    let leg_schema = json!({ "type": "object", "properties": { "leg": { "type": "string" } } });
    let parent_schema = json!({
        "type": "object",
        "properties": { "front": { "$ref": reference } },
    })
    .to_string();

    created_model(
        &gateway,
        &api,
        "the referenced model",
        json!({
            "name": "Leg",
            "contentType": "application/json",
            "schema": leg_schema.to_string(),
        }),
    )
    .await;
    created_model(
        &gateway,
        &api,
        "the referencing model",
        json!({
            "name": "Pet",
            "contentType": "application/json",
            "schema": parent_schema.clone(),
        }),
    )
    .await;

    // While the referenced model is stored, a flattened read inlines its schema.
    let flattened = get_model(&gateway, &api, "Pet", true).await;
    let reported = string_member(&flattened.body, "schema", "before the delete");
    let read: Value = serde_json::from_str(&reported).unwrap_or_else(|_| {
        panic!("case before the delete: expected a JSON schema, got {reported:?}")
    });
    assert_eq!(
        read,
        json!({ "type": "object", "properties": { "front": leg_schema.clone() } }),
        "case before the delete: expected the referenced schema inlined, got {read:?}"
    );

    deleted_model(&gateway, &api, "Leg", "the referenced model").await;

    // The record is GONE from the store, so there is no schema left to inline: the
    // referencing model is reported exactly as it was stored.
    let flattened = get_model(&gateway, &api, "Pet", true).await;
    assert_eq!(
        string_member(&flattened.body, "schema", "after the delete"),
        parent_schema,
        "case after the delete: expected the stored schema, got {:?}",
        flattened.body["schema"]
    );

    // And the name is free again: a create under it is accepted rather than refused as a
    // conflict, and stores its OWN record under a freshly minted id.
    let recreated = created_model(
        &gateway,
        &api,
        "the create under the freed name",
        json!({
            "name": "Leg",
            "contentType": "text/plain",
            "description": "a leg, in text",
        }),
    )
    .await;
    assert_eq!(
        string_member(&recreated, "contentType", "the create under the freed name"),
        "text/plain",
        "case the create under the freed name: expected its own content type, got \
         {recreated:?}"
    );
    let read = get_model(&gateway, &api, "Leg", false).await;
    assert_eq!(
        read.body, recreated,
        "case GetModel after the create under the freed name: expected {recreated:?}, got \
         {:?}",
        read.body
    );
    // The new record carries no schema, so the reference resolves to nothing to inline and
    // the referencing model still reports its stored text.
    let flattened = get_model(&gateway, &api, "Pet", true).await;
    assert_eq!(
        string_member(&flattened.body, "schema", "after the create under the freed name"),
        parent_schema,
        "case after the create under the freed name: expected the stored schema, got {:?}",
        flattened.body["schema"]
    );

    // The delete took away ONE record and not the collection: the model that was not
    // deleted still holds its name, so a create under it is still the modelled conflict.
    assert_failure(
        &create_model(
            &gateway,
            &api,
            json!({ "name": "Pet", "contentType": "application/json" }),
        )
        .await,
        409,
        "ConflictException",
        "CreateModel under a name still held",
    );
}
