// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTIFIERS this control plane mints: the width and the
//! alphabet of the id each kind of record is addressed by.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator, the request builder and the create helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `models` and
//! `execute_api_data_plane`.
//!
//! The RULE under test: an id API Gateway mints is lowercase alphanumeric, unique, and as
//! wide as the record it names — six characters for a record created INSIDE one REST API (a
//! `Resource`, a `Model`, a `Deployment`), ten for a record of the account (the `RestApi`
//! itself, and the root resource minted with it) — and every character position of it
//! varies, so no part of an id is a fixed literal a client could read as meaning anything.
//!
//! The cases assert that over the whole input space the rule generalises over: the record
//! kind, the creation sequence (the only input an id is derived from, walked for many
//! records of each kind rather than sampled once), and the operation the id is read back
//! through.

use std::collections::BTreeSet;

use serde_json::json;

use crate::models::{created_model, json_schema};
use crate::patch_updates::{patch_ok, replace};
use crate::rest_api_lifecycle::{
    create_api, make_deployable, resource_under, string_member, Gateway,
};

/// How many records of each kind the tables walk: the id is derived from the creation
/// sequence, so several in a row exercise consecutive seeds rather than one.
const RECORDS_PER_KIND: usize = 12;

/// The width of an id that names a record created inside one REST API.
const SUB_RESOURCE_ID_CHARS: usize = 6;

/// The width of an id that names a record of the account, and of the root resource a REST
/// API is created with.
const ACCOUNT_RECORD_ID_CHARS: usize = 10;

/// Assert `id` is an API Gateway identifier of `width` lowercase alphanumeric characters,
/// and that no record has been given it before.
fn assert_minted(case: &str, id: &str, width: usize, minted: &mut BTreeSet<String>) {
    assert_eq!(
        id.chars().count(),
        width,
        "case {case}: expected an id of {width} characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case}: expected a lowercase alphanumeric id, got {id:?}"
    );
    assert!(
        minted.insert(id.to_string()),
        "case {case}: the id {id:?} was minted twice"
    );
}

/// Assert that every character POSITION of the ids in `minted` carries more than one
/// character across them: an id whose tail never changes is a literal, not an identifier.
fn assert_every_position_varies(case: &str, minted: &[String], width: usize) {
    for position in 0..width {
        let seen: BTreeSet<Option<char>> = minted
            .iter()
            .map(|id| id.chars().nth(position))
            .collect();
        assert!(
            seen.len() > 1,
            "case {case}: character {position} of a minted id never varies across {minted:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_of_an_api_is_identified_by_six_lowercase_alphanumerics() {
    let gateway = Gateway::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let (api, root) = create_api(&gateway, "resource-ids").await;
    assert_minted(
        "the API's own id",
        &api,
        ACCOUNT_RECORD_ID_CHARS,
        &mut minted,
    );
    assert_minted(
        "the root resource created with the API",
        &root,
        ACCOUNT_RECORD_ID_CHARS,
        &mut minted,
    );

    let mut ids: Vec<String> = Vec::new();
    for index in 0..RECORDS_PER_KIND {
        let case = format!("CreateResource(pets-{index})");
        let id = resource_under(&gateway, &api, &root, &format!("pets-{index}")).await;
        assert_minted(&case, &id, SUB_RESOURCE_ID_CHARS, &mut minted);

        // The id is the one the record is addressed by: the read through it reports the
        // same resource.
        let read = gateway
            .call("GET", &format!("/restapis/{api}/resources/{id}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetResource, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "id", &case),
            id,
            "case {case}: GetResource must report the id it was addressed by, got {:?}",
            read.body
        );
        ids.push(id);
    }
    assert_every_position_varies("a minted resource id", &ids, SUB_RESOURCE_ID_CHARS);
}

#[tokio::test]
async fn a_model_of_an_api_is_identified_by_six_lowercase_alphanumerics() {
    let gateway = Gateway::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let (api, _root) = create_api(&gateway, "model-ids").await;

    let mut ids: Vec<String> = Vec::new();
    for index in 0..RECORDS_PER_KIND {
        let name = format!("Pet{index}");
        let case = format!("CreateModel({name})");
        let created = created_model(
            &gateway,
            &api,
            &case,
            json!({
                "name": name,
                "contentType": "application/json",
                "schema": json_schema(&name, 1),
            }),
        )
        .await;
        let id = string_member(&created, "id", &case);
        assert_minted(&case, &id, SUB_RESOURCE_ID_CHARS, &mut minted);

        // A model is addressed by its NAME, so the id a later read reports is the one the
        // create minted — the patch that renames nothing included.
        let patched = patch_ok(
            &gateway,
            &case,
            &format!("/restapis/{api}/models/{name}"),
            vec![replace("/description", "the pet payload")],
        )
        .await;
        assert_eq!(
            string_member(&patched, "id", &case),
            id,
            "case {case}: UpdateModel must report the stored id, got {patched:?}"
        );
        ids.push(id);
    }
    assert_every_position_varies("a minted model id", &ids, SUB_RESOURCE_ID_CHARS);
}

#[tokio::test]
async fn a_deployment_of_an_api_is_identified_by_six_lowercase_alphanumerics() {
    let gateway = Gateway::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let (api, _root) = create_api(&gateway, "deployment-ids").await;
    make_deployable(&gateway, &api).await;

    let mut ids: Vec<String> = Vec::new();
    for index in 0..RECORDS_PER_KIND {
        let case = format!("CreateDeployment({index})");
        // No `stageName`: a deployment is a snapshot of the API's methods and needs no
        // stage pointed at it, which is what lets one API carry more deployments than the
        // per-API stage quota admits stages.
        let created = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/deployments"),
                Some(json!({ "description": format!("snapshot {index}") })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", &case);
        assert_minted(&case, &id, SUB_RESOURCE_ID_CHARS, &mut minted);

        let read = gateway
            .call("GET", &format!("/restapis/{api}/deployments/{id}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDeployment, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "id", &case),
            id,
            "case {case}: GetDeployment must report the id it was addressed by, got {:?}",
            read.body
        );
        ids.push(id);
    }
    assert_every_position_varies("a minted deployment id", &ids, SUB_RESOURCE_ID_CHARS);
}

#[tokio::test]
async fn an_imported_api_identifies_its_resources_the_way_a_created_one_does() {
    // The width is the RECORD's, not the operation's: an API imported from a definition
    // reports resource ids of the same shape `CreateResource` mints.
    let gateway = Gateway::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let definition = json!({
        "openapi": "3.0.1",
        "info": { "title": "imported-ids" },
        "paths": {
            "/pets": { "get": { "responses": { "200": { "description": "ok" } } } },
            "/pets/{petId}": { "get": { "responses": { "200": { "description": "ok" } } } },
            "/store/orders": { "post": { "responses": { "200": { "description": "ok" } } } },
        },
    })
    .to_string();
    let answer = gateway
        .call_raw("POST", "/restapis?mode=import", &definition)
        .await;
    assert_eq!(
        answer.status, 201,
        "case ImportRestApi: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let api = string_member(&answer.body, "id", "ImportRestApi");
    assert_minted(
        "the imported API's own id",
        &api,
        ACCOUNT_RECORD_ID_CHARS,
        &mut minted,
    );

    // The root resource is minted with the API, so it carries the API's own width; every
    // resource the definition declares is created inside the API and carries the narrower
    // one.
    let root = string_member(&answer.body, "rootResourceId", "ImportRestApi");
    assert_minted(
        "the imported API's root resource",
        &root,
        ACCOUNT_RECORD_ID_CHARS,
        &mut minted,
    );

    let listed = gateway
        .call("GET", &format!("/restapis/{api}/resources"), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "case GetResources: expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    let items = listed.body["item"]
        .as_array()
        .unwrap_or_else(|| panic!("case GetResources: expected an item array, got {:?}", listed.body))
        .clone();
    // The root resource and the four the paths imply (`/pets`, `/pets/{petId}`, `/store`,
    // `/store/orders`).
    assert_eq!(
        items.len(),
        5,
        "case GetResources: expected the imported tree, got {items:?}"
    );
    let mut ids: Vec<String> = Vec::new();
    for item in &items {
        let path = string_member(item, "path", "GetResources");
        let id = string_member(item, "id", &path);
        if id == root {
            continue;
        }
        assert_minted(&path, &id, SUB_RESOURCE_ID_CHARS, &mut minted);
        ids.push(id);
    }
    assert_every_position_varies("an imported resource id", &ids, SUB_RESOURCE_ID_CHARS);
}
