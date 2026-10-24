// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's PATCH-style update operations: `UpdateApiKey`,
//! `UpdateResource`, `UpdateMethod` and `UpdateRestApi`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! create helpers are this crate's existing harness, reused from `rest_api_lifecycle`
//! and `api_key_lifecycle`.
//!
//! The RULE under test is one rule for all four operations: an `Update*` call applies
//! every entry of `patchOperations` to the stored resource, in request order, and a
//! later read of that resource reports exactly what the patch set — and a patch the
//! service cannot apply is refused with a modelled error and changes nothing.
//!
//! Each test asserts that rule over a SET of inputs rather than one recorded call: a
//! table of members and ops per resource, the documented value catalogs of the
//! enum-typed members, the published bounds of `minimumCompressionSize`, a table of
//! path segments and nesting depths for a renamed resource, and the ops and paths no
//! resource supports.

use serde_json::{json, Value};

use crate::api_key_lifecycle::{assert_failure, create_key, get_key};
use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Answer, Gateway};

/// One row of a patch table: the case name, the `patchOperations` the row sends, and
/// the members the patched resource must report afterwards.
type Row = (&'static str, Vec<Value>, Vec<(&'static str, Value)>);

/// One `patchOperations` entry, as a client spells it on the wire.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// A `replace` entry, the op a client issues most.
fn replace(path: &str, value: &str) -> Value {
    op("replace", path, Some(value))
}

/// Send `operations` as a PATCH to `uri`.
async fn patch(gateway: &Gateway, uri: &str, operations: Vec<Value>) -> Answer {
    gateway
        .call(
            "PATCH",
            uri,
            Some(json!({ "patchOperations": Value::Array(operations) })),
        )
        .await
}

/// Send `operations` to `uri`, asserting the modelled 200 and returning the body.
///
/// `pub(crate)` so a sibling suite reuses this one PATCH-and-assert wrapper rather than
/// keeping its own copy (the `update_stage_readers` suite does).
pub(crate) async fn patch_ok(
    gateway: &Gateway,
    case: &str,
    uri: &str,
    operations: Vec<Value>,
) -> Value {
    let answer = patch(gateway, uri, operations).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the modelled 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The baseline API key every `UpdateApiKey` row starts from: each patchable member has
/// a known value, except `customerId`, which is absent so an `add` has something to set.
fn baseline_key() -> Value {
    json!({
        "name": "baseline-key",
        "description": "initial description",
        "enabled": true,
    })
}

/// A created baseline key: its id and the created body.
async fn baseline(gateway: &Gateway, case: &str) -> (String, Value) {
    let created = create_key(gateway, case, baseline_key()).await;
    let id = string_member(&created, "id", case);
    (id, created)
}

/// Assert every `(member, expected)` pair of `expectations` holds in `body`.
fn assert_members(body: &Value, expectations: &[(&str, Value)], case: &str, source: &str) {
    for (member, expected) in expectations {
        assert_eq!(
            body[*member], *expected,
            "case {case}: {source} must report {member} as {expected:?}, got {:?}",
            body[*member]
        );
    }
}

/// A JSON number as `f64`, for comparing two timestamps.
fn number(body: &Value, member: &str, case: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {body:?}"))
}

#[tokio::test]
async fn a_patched_api_key_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which member the path names,
    // that member's type (string, boolean, a string absent by default), how many ops
    // one request carries, and whether two ops target one member. Every row uses
    // `replace`, because that is the op the ApiKey's single-valued members take — see
    // `an_unsupported_patch_path_is_refused_with_the_path_the_op_and_the_supported_set`
    // for the per-op path sets and
    // `an_api_key_collection_path_takes_an_add_and_a_remove` for the collections.
    let cases: Vec<Row> = vec![
        (
            "replace /name",
            vec![replace("/name", "baseline-key-renamed")],
            vec![("name", json!("baseline-key-renamed"))],
        ),
        (
            "replace /description",
            vec![replace("/description", "updated description")],
            vec![("description", json!("updated description"))],
        ),
        (
            "replace /enabled with false",
            vec![replace("/enabled", "false")],
            vec![("enabled", json!(false))],
        ),
        (
            "replace /enabled with true",
            vec![replace("/enabled", "true")],
            vec![("enabled", json!(true))],
        ),
        (
            "replace /customerId on a member that was absent",
            vec![replace("/customerId", "marketplace-customer-7")],
            vec![("customerId", json!("marketplace-customer-7"))],
        ),
        (
            "replace /description with an empty value",
            vec![replace("/description", "")],
            vec![("description", json!(""))],
        ),
        (
            "three members in one request",
            vec![
                replace("/name", "renamed-and-described"),
                replace("/description", "updated description"),
                replace("/enabled", "false"),
            ],
            vec![
                ("name", json!("renamed-and-described")),
                ("description", json!("updated description")),
                ("enabled", json!(false)),
            ],
        ),
        (
            "two ops on one member: the last applied wins",
            vec![replace("/name", "first"), replace("/name", "second")],
            vec![("name", json!("second"))],
        ),
        (
            "an empty list changes nothing",
            vec![],
            vec![
                ("name", json!("baseline-key")),
                ("description", json!("initial description")),
                ("enabled", json!(true)),
            ],
        ),
    ];

    let gateway = Gateway::new();
    for (case, operations, expectations) in &cases {
        let (id, created) = baseline(&gateway, case).await;
        let updated = patch_ok(
            &gateway,
            case,
            &format!("/apikeys/{id}"),
            operations.clone(),
        )
        .await;

        assert_members(&updated, expectations, case, "UpdateApiKey");
        assert_eq!(
            updated["id"], created["id"],
            "case {case}: a patch must not change the key's id, got {:?}",
            updated["id"]
        );
        assert_eq!(
            updated["createdDate"], created["createdDate"],
            "case {case}: a patch must not move createdDate, got {:?}",
            updated["createdDate"]
        );
        assert!(
            updated["value"].is_null(),
            "case {case}: UpdateApiKey must withhold the key value, got {:?}",
            updated["value"]
        );

        // The round trip: the patch changed the STORED record, not just the answer.
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApiKey, got {} with {:?}",
            read.status, read.body
        );
        assert_members(&read.body, expectations, case, "GetApiKey");
    }
}

#[tokio::test]
async fn a_successful_api_key_patch_advances_last_updated_date_and_a_refused_one_does_not() {
    let gateway = Gateway::new();
    let (id, created) = baseline(&gateway, "lastUpdatedDate").await;
    let created_date = number(&created, "createdDate", "lastUpdatedDate");
    let before = number(&created, "lastUpdatedDate", "lastUpdatedDate");

    let updated = patch_ok(
        &gateway,
        "a successful patch",
        &format!("/apikeys/{id}"),
        vec![replace("/description", "sim2real-probe replaced")],
    )
    .await;
    let after = number(&updated, "lastUpdatedDate", "a successful patch");
    assert!(
        after >= before,
        "case a successful patch: lastUpdatedDate must not go backwards, got {after} after {before}"
    );
    assert_eq!(
        number(&updated, "createdDate", "a successful patch"),
        created_date,
        "case a successful patch: createdDate must not move, got {:?}",
        updated["createdDate"]
    );

    // A refused patch changes nothing at all, lastUpdatedDate included.
    let refused = patch(
        &gateway,
        &format!("/apikeys/{id}"),
        vec![replace("/nosuchmember", "x")],
    )
    .await;
    assert_failure(&refused, 400, "BadRequestException", "a refused patch");
    let read = get_key(&gateway, &id, false).await;
    assert_eq!(
        number(&read.body, "lastUpdatedDate", "a refused patch"),
        after,
        "case a refused patch: lastUpdatedDate must stay where the last applied patch left it, got {:?}",
        read.body["lastUpdatedDate"]
    );
    assert_eq!(
        read.body["description"],
        json!("sim2real-probe replaced"),
        "case a refused patch: the record must keep the applied value, got {:?}",
        read.body["description"]
    );
}

#[tokio::test]
async fn a_patch_this_service_cannot_apply_is_refused_and_changes_nothing() {
    // Rows differ along the KIND of refusal: an op no single-valued member supports,
    // an op missing its value, a path naming no member, a path that is not a JSON
    // Pointer, a path reaching into a single-valued member, a value that is not of the
    // member's type, and a list whose LAST op is refused (so the earlier ops must not
    // be kept either).
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("move is not supported", vec![op("move", "/name", None)]),
        (
            "copy is not supported",
            vec![json!({ "op": "copy", "path": "/name", "from": "/description" })],
        ),
        ("test is not supported", vec![op("test", "/name", None)]),
        ("a replace with no value", vec![op("replace", "/name", None)]),
        ("an entry with no path", vec![json!({ "op": "replace" })]),
        ("an entry with no op", vec![json!({ "path": "/name" })]),
        (
            "a path naming no updateable member",
            vec![replace("/nosuchmember", "x")],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("name", "x")],
        ),
        ("an empty path", vec![replace("", "x")]),
        (
            "a path reaching into a single-valued member",
            vec![replace("/name/first", "x")],
        ),
        (
            "a non-boolean value for a boolean member",
            vec![replace("/enabled", "yes")],
        ),
        (
            "remove of a single-valued member",
            vec![op("remove", "/enabled", None)],
        ),
        (
            "add of a single-valued member",
            vec![op("add", "/description", Some("x"))],
        ),
        (
            "a list whose last op is refused",
            vec![
                replace("/description", "never stored"),
                replace("/nosuchmember", "x"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let baseline_members = vec![
        ("name", json!("baseline-key")),
        ("description", json!("initial description")),
        ("enabled", json!(true)),
        ("customerId", Value::Null),
    ];

    for (case, operations) in &cases {
        let (id, _) = baseline(&gateway, case).await;
        let answer = patch(&gateway, &format!("/apikeys/{id}"), operations.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);

        // Nothing was applied: the stored record is the one the key was created with.
        let read = get_key(&gateway, &id, false).await;
        assert_members(&read.body, &baseline_members, case, "GetApiKey");
    }

    // `move`, `copy` and `test` are declared by the model's `Op` enum but name nothing
    // on these resources, and API Gateway answers all three with one message, as a
    // recorded refusal of a `move` reports.
    for kind in ["move", "copy", "test"] {
        let case = format!("UpdateApiKey({kind})");
        let (id, _) = baseline(&gateway, &case).await;
        let answer = patch(
            &gateway,
            &format!("/apikeys/{id}"),
            vec![json!({ "op": kind, "path": "/description", "from": "/name" })],
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some("Invalid patch operation specified. Must be 'add'|'remove'|'replace'"),
            "case {case}: expected the one refusal for an op no resource supports, got {:?}",
            answer.body["message"]
        );
    }

    // An `op` the model's `Op` enum does not declare is refused by the modelled-value    // check the request walk runs BEFORE the handler, so it answers that refusal's
    // `ValidationException` rather than this operation's `BadRequestException` — and
    // the record is left alone just the same.
    let (id, _) = baseline(&gateway, "an off-model op").await;
    let answer = patch(
        &gateway,
        &format!("/apikeys/{id}"),
        vec![json!({ "op": "upsert", "path": "/name", "value": "x" })],
    )
    .await;
    assert_failure(&answer, 400, "ValidationException", "an off-model op");
    let read = get_key(&gateway, &id, false).await;
    assert_members(&read.body, &baseline_members, "an off-model op", "GetApiKey");

    // An identifier no key has is the modelled NotFoundException, whichever patch it
    // carries.
    for (case, operations) in [
        ("a well-formed patch", vec![replace("/name", "x")]),
        ("an empty list", vec![]),
    ] {
        let answer = patch(&gateway, "/apikeys/zzzzzzzzzz", operations).await;
        assert_failure(
            &answer,
            404,
            "NotFoundException",
            &format!("UpdateApiKey(unknown key, {case})"),
        );
    }
}

#[tokio::test]
async fn an_unsupported_patch_path_is_refused_with_the_path_the_op_and_the_supported_set() {
    // The refusal's TEXT is part of the rule here, because API Gateway builds it from
    // the request: it names the path, the op, and the paths the resource has FOR THAT
    // OP. Two recorded refusals are the authority for the two ApiKey sets asserted
    // below: a `replace` of `/id` reads `Invalid patch path  '/id' specified for op
    // 'replace'. Must be one of: [/description, /enabled, /name, /customerId]`, and a
    // `remove` of `/description` reads `Invalid patch path  '/description' specified
    // for op 'remove'. Must be one of: [/stages, /labels]` — so a single-valued member
    // takes a `replace` and a collection takes an `add` or a `remove`.
    const REPLACE_PATHS: &str = "[/description, /enabled, /name, /customerId]";
    const COLLECTION_PATHS: &str = "[/stages, /labels]";

    let gateway = Gateway::new();
    let (id, _) = baseline(&gateway, "unsupported api key paths").await;

    // Rows differ in the member the path names — a service-assigned one, a secret, a
    // collection, a map entry, a single-valued one — and in the op offered for it.
    for (path, kind) in [
        ("/id", "replace"),
        ("/value", "replace"),
        ("/createdDate", "replace"),
        ("/stageKeys", "replace"),
        ("/nosuchmember", "replace"),
        ("/lastUpdatedDate", "remove"),
        ("/description", "remove"),
        ("/name", "add"),
        ("/tags/team", "add"),
    ] {
        let case = format!("UpdateApiKey({kind} {path})");
        let answer = patch(
            &gateway,
            &format!("/apikeys/{id}"),
            vec![op(kind, path, Some("x"))],
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let expected_set = if kind == "replace" {
            REPLACE_PATHS
        } else {
            COLLECTION_PATHS
        };
        assert_eq!(
            answer.body["message"].as_str(),
            Some(
                format!(
                    "Invalid patch path  '{path}' specified for op '{kind}'. Must be one of: {expected_set}"
                )
                .as_str()
            ),
            "case {case}: the refusal must name the path, the op and the supported set, got {:?}",
            answer.body["message"]
        );
    }

    // The same rule on the other PATCH-style operations, each listing its own set.
    let (api, root) = create_api(&gateway, "unsupported-paths").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "pets");
    let put = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        put.status, 201,
        "case PutMethod(GET): expected 201, got {} with {:?}",
        put.status, put.body
    );

    for (case, uri, path, expected_member) in [
        (
            "UpdateRestApi",
            format!("/restapis/{api}"),
            "/rootResourceId",
            "/name",
        ),
        (
            "UpdateResource",
            format!("/restapis/{api}/resources/{resource_id}"),
            "/path",
            "/pathPart",
        ),
        (
            "UpdateMethod",
            format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            "/httpMethod",
            "/apiKeyRequired",
        ),
    ] {
        let answer = patch(&gateway, &uri, vec![replace(path, "x")]).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let message = answer.body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a message, got {:?}", answer.body))
            .to_owned();
        let prefix =
            format!("Invalid patch path  '{path}' specified for op 'replace'. Must be one of: [");
        assert!(
            message.starts_with(&prefix),
            "case {case}: expected the refusal to start {prefix:?}, got {message:?}"
        );
        assert!(
            message.contains(expected_member),
            "case {case}: the supported set must name {expected_member}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_api_key_collection_path_takes_an_add_and_a_remove() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "patch-stages-one").await;
    let (second, _) = create_api(&gateway, "patch-stages-two").await;
    let (id, _) = baseline(&gateway, "collection paths").await;
    let uri = format!("/apikeys/{id}");

    // Rows differ in how the entry is addressed — the op's `value`, and the entry as a
    // JSON Pointer token with its `/` escaped `~1` — in the op, and in how many
    // associations the key holds when the op is applied. `stageKeys` is the member the
    // `ApiKey` shape reports the stage associations through, which is what makes the
    // applied patch readable.
    let cases: Vec<(&str, Vec<Value>, Vec<String>)> = vec![
        (
            "add the first stage by value",
            vec![op("add", "/stages", Some(&format!("{first}/prod")))],
            vec![format!("{first}/prod")],
        ),
        (
            "add a second stage of the same api",
            vec![op("add", "/stages", Some(&format!("{first}/beta")))],
            vec![format!("{first}/prod"), format!("{first}/beta")],
        ),
        (
            "adding one the key already holds changes nothing",
            vec![op("add", "/stages", Some(&format!("{first}/prod")))],
            vec![format!("{first}/prod"), format!("{first}/beta")],
        ),
        (
            "add a stage of another api as an escaped pointer token",
            vec![op("add", &format!("/stages/{second}~1canary"), None)],
            vec![
                format!("{first}/prod"),
                format!("{first}/beta"),
                format!("{second}/canary"),
            ],
        ),
        (
            "remove one association",
            vec![op("remove", "/stages", Some(&format!("{first}/beta")))],
            vec![format!("{first}/prod"), format!("{second}/canary")],
        ),
        (
            "removing one the key does not hold changes nothing",
            vec![op("remove", "/stages", Some(&format!("{first}/beta")))],
            vec![format!("{first}/prod"), format!("{second}/canary")],
        ),
        (
            "a label is accepted and reported by no member of the ApiKey shape",
            vec![op("add", "/labels", Some("production"))],
            vec![format!("{first}/prod"), format!("{second}/canary")],
        ),
        (
            "and removed again",
            vec![op("remove", "/labels", Some("production"))],
            vec![format!("{first}/prod"), format!("{second}/canary")],
        ),
    ];

    for (case, operations, expected) in &cases {
        let updated = patch_ok(&gateway, case, &uri, operations.clone()).await;
        let expected: Value = Value::Array(expected.iter().cloned().map(Value::String).collect());
        assert_eq!(
            updated["stageKeys"], expected,
            "case {case}: expected {expected:?}, got {:?}",
            updated["stageKeys"]
        );
        let read = get_key(&gateway, id.as_str(), false).await;
        assert_eq!(
            read.body["stageKeys"], expected,
            "case {case}: GetApiKey must report the stored associations, got {:?}",
            read.body["stageKeys"]
        );
    }

    // A stage of an API that does not exist is the modelled NotFoundException; an entry
    // that does not name one stage of one API is a bad request.
    let refused: Vec<(&str, Vec<Value>, u16, &str)> = vec![
        (
            "add a stage of an unknown api",
            vec![op("add", "/stages", Some("zzzzzzzzzz/prod"))],
            404,
            "NotFoundException",
        ),
        (
            "an entry naming no stage",
            vec![op("add", "/stages", Some(&first))],
            400,
            "BadRequestException",
        ),
        (
            "an entry with an empty stage name",
            vec![op("add", "/stages", Some(&format!("{first}/")))],
            400,
            "BadRequestException",
        ),
        (
            "an add with no entry at all",
            vec![op("add", "/stages", None)],
            400,
            "BadRequestException",
        ),
    ];
    for (case, operations, status, code) in &refused {
        let answer = patch(&gateway, &uri, operations.clone()).await;
        assert_failure(&answer, *status, code, case);
    }
}

/// `GetResource` for `resource_id`, asserting the modelled 200 and returning the body.
async fn get_resource(gateway: &Gateway, api: &str, resource_id: &str, case: &str) -> Value {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/resources/{resource_id}"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetResource, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

#[tokio::test]
async fn renaming_a_resource_rematerialises_its_path_and_every_path_beneath_it() {
    // Rows differ in the shape of the new segment: a plain word, a version segment, a
    // path-parameter segment (the `{name}` form API Gateway routes on), one carrying
    // punctuation, and a single character.
    let segments = ["hello", "v2", "{petId}", "orders.v1", "a"];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patch-resources").await;
    let top = create_resource(&gateway, &api, &root, "pets").await;
    let top_id = string_member(&top, "id", "pets");
    let middle = create_resource(&gateway, &api, &top_id, "owners").await;
    let middle_id = string_member(&middle, "id", "owners");
    let leaf = create_resource(&gateway, &api, &middle_id, "photos").await;
    let leaf_id = string_member(&leaf, "id", "photos");

    for segment in segments {
        let case = format!("replace /pathPart with {segment:?}");
        let updated = patch_ok(
            &gateway,
            &case,
            &format!("/restapis/{api}/resources/{top_id}"),
            vec![replace("/pathPart", segment)],
        )
        .await;
        assert_eq!(
            updated["pathPart"].as_str(),
            Some(segment),
            "case {case}: expected the renamed segment, got {:?}",
            updated["pathPart"]
        );
        assert_eq!(
            updated["path"].as_str(),
            Some(format!("/{segment}").as_str()),
            "case {case}: path is derived from the segment, got {:?}",
            updated["path"]
        );

        // The derived path follows for the renamed resource AND for every resource
        // beneath it, as read back through GetResource.
        for (resource_id, expected) in [
            (&top_id, format!("/{segment}")),
            (&middle_id, format!("/{segment}/owners")),
            (&leaf_id, format!("/{segment}/owners/photos")),
        ] {
            let read = get_resource(&gateway, &api, resource_id, &case).await;
            assert_eq!(
                read["path"].as_str(),
                Some(expected.as_str()),
                "case {case}: expected {expected:?} for {resource_id}, got {:?}",
                read["path"]
            );
        }
    }
}

#[tokio::test]
async fn moving_a_resource_reparents_it_and_rematerialises_the_subtree() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patch-resource-moves").await;
    let top = create_resource(&gateway, &api, &root, "pets").await;
    let top_id = string_member(&top, "id", "pets");
    let middle = create_resource(&gateway, &api, &top_id, "owners").await;
    let middle_id = string_member(&middle, "id", "owners");
    let leaf = create_resource(&gateway, &api, &middle_id, "photos").await;
    let leaf_id = string_member(&leaf, "id", "photos");

    // Moving the middle resource to the root takes its own subtree with it and leaves
    // the resource it was under where it was.
    let case = "replace /parentId with the root resource";
    let updated = patch_ok(
        &gateway,
        case,
        &format!("/restapis/{api}/resources/{middle_id}"),
        vec![replace("/parentId", &root)],
    )
    .await;
    assert_eq!(
        updated["parentId"].as_str(),
        Some(root.as_str()),
        "case {case}: expected the new parent, got {:?}",
        updated["parentId"]
    );
    for (resource_id, expected) in [
        (&top_id, "/pets"),
        (&middle_id, "/owners"),
        (&leaf_id, "/owners/photos"),
    ] {
        let read = get_resource(&gateway, &api, resource_id, case).await;
        assert_eq!(
            read["path"].as_str(),
            Some(expected),
            "case {case}: expected {expected:?} for {resource_id}, got {:?}",
            read["path"]
        );
    }

    // Moving it back under a resource that is not one of its descendants is accepted
    // again, so the move is not one-way.
    let case = "move it back under the resource it came from";
    patch_ok(
        &gateway,
        case,
        &format!("/restapis/{api}/resources/{middle_id}"),
        vec![replace("/parentId", &top_id)],
    )
    .await;
    let read = get_resource(&gateway, &api, &leaf_id, case).await;
    assert_eq!(
        read["path"].as_str(),
        Some("/pets/owners/photos"),
        "case {case}: expected the restored path, got {:?}",
        read["path"]
    );

    // Rows differ along the KIND of refusal a resource patch has: a parent that does
    // not exist, a parent inside the resource's own subtree, a segment that is not one
    // segment, a path naming a derived or service-assigned member, and an op the
    // member does not support.
    let refused: Vec<(&str, String, Vec<Value>, u16, &str)> = vec![
        (
            "a parent no resource has",
            middle_id.clone(),
            vec![replace("/parentId", "zzzzzzzzzz")],
            404,
            "NotFoundException",
        ),
        (
            "the resource itself as its parent",
            middle_id.clone(),
            vec![replace("/parentId", &middle_id)],
            400,
            "BadRequestException",
        ),
        (
            "its own descendant as its parent",
            middle_id.clone(),
            vec![replace("/parentId", &leaf_id)],
            400,
            "BadRequestException",
        ),
        (
            "a segment carrying a separator",
            middle_id.clone(),
            vec![replace("/pathPart", "owners/photos")],
            400,
            "BadRequestException",
        ),
        (
            "an empty segment",
            middle_id.clone(),
            vec![replace("/pathPart", "")],
            400,
            "BadRequestException",
        ),
        (
            "remove of the segment",
            middle_id.clone(),
            vec![op("remove", "/pathPart", None)],
            400,
            "BadRequestException",
        ),
        (
            "the derived path member",
            middle_id.clone(),
            vec![replace("/path", "/elsewhere")],
            400,
            "BadRequestException",
        ),
        (
            "the service-assigned id",
            middle_id.clone(),
            vec![replace("/id", "zzzzzzzzzz")],
            400,
            "BadRequestException",
        ),
        (
            "renaming the root resource",
            root.clone(),
            vec![replace("/pathPart", "renamed-root")],
            400,
            "BadRequestException",
        ),
        (
            "moving the root resource",
            root.clone(),
            vec![replace("/parentId", &top_id)],
            400,
            "BadRequestException",
        ),
    ];

    for (case, resource_id, operations, status, code) in &refused {
        let answer = patch(
            &gateway,
            &format!("/restapis/{api}/resources/{resource_id}"),
            operations.clone(),
        )
        .await;
        assert_failure(&answer, *status, code, case);
        // The tree is the one the accepted moves left behind.
        let read = get_resource(&gateway, &api, &leaf_id, case).await;
        assert_eq!(
            read["path"].as_str(),
            Some("/pets/owners/photos"),
            "case {case}: a refused patch must change no path, got {:?}",
            read["path"]
        );
    }

    // Two siblings cannot share one segment, so renaming onto a taken one is the
    // modelled conflict — and the resource keeps the segment it had.
    let sibling = create_resource(&gateway, &api, &root, "clinics").await;
    let sibling_id = string_member(&sibling, "id", "clinics");
    let case = "renaming onto a sibling's segment";
    let answer = patch(
        &gateway,
        &format!("/restapis/{api}/resources/{sibling_id}"),
        vec![replace("/pathPart", "pets")],
    )
    .await;
    assert_failure(&answer, 409, "ConflictException", case);
    let read = get_resource(&gateway, &api, &sibling_id, case).await;
    assert_eq!(
        read["pathPart"].as_str(),
        Some("clinics"),
        "case {case}: the refused rename must leave the segment alone, got {:?}",
        read["pathPart"]
    );

    // Moving onto a parent that already holds that segment is the same conflict.
    let case = "moving under a parent that holds the segment";
    let clash = create_resource(&gateway, &api, &top_id, "clinics").await;
    let clash_id = string_member(&clash, "id", "pets/clinics");
    let answer = patch(
        &gateway,
        &format!("/restapis/{api}/resources/{clash_id}"),
        vec![replace("/parentId", &root)],
    )
    .await;
    assert_failure(&answer, 409, "ConflictException", case);

    // An unknown API or resource is the modelled NotFoundException.
    for (case, uri) in [
        (
            "an unknown api",
            format!("/restapis/zzzzzzzzzz/resources/{middle_id}"),
        ),
        (
            "an unknown resource",
            format!("/restapis/{api}/resources/zzzzzzzzzz"),
        ),
    ] {
        let answer = patch(&gateway, &uri, vec![replace("/pathPart", "x")]).await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }
}

/// `GetMethod` for `verb` on `resource_id`, asserting the modelled 200.
async fn get_method(
    gateway: &Gateway,
    api: &str,
    resource_id: &str,
    verb: &str,
    case: &str,
) -> Value {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetMethod, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

#[tokio::test]
async fn a_patched_method_member_is_stored_and_read_back() {
    // The `authorizationType` catalog is the member's own documented value list:
    // "Valid values are NONE for open access, AWS_IAM for using AWS IAM permissions,
    // CUSTOM for using a custom authorizer, or COGNITO_USER_POOLS for using a Cognito
    // user pool" (`Method$authorizationType`).
    let authorization_types = ["NONE", "AWS_IAM", "CUSTOM", "COGNITO_USER_POOLS"];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patch-methods").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "pets");
    let put = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE", "operationName": "listPets" })),
        )
        .await;
    assert_eq!(
        put.status, 201,
        "case PutMethod(GET): expected 201, got {} with {:?}",
        put.status, put.body
    );
    let uri = format!("/restapis/{api}/resources/{resource_id}/methods/GET");

    // Every documented authorization type is accepted and read back, not just the one
    // the method was created with.
    for authorization_type in authorization_types {
        let case = format!("replace /authorizationType with {authorization_type:?}");
        let updated =
            patch_ok(&gateway, &case, &uri, vec![replace("/authorizationType", authorization_type)])
                .await;
        assert_eq!(
            updated["authorizationType"].as_str(),
            Some(authorization_type),
            "case {case}: expected the patched type, got {:?}",
            updated["authorizationType"]
        );
        let read = get_method(&gateway, &api, &resource_id, "GET", &case).await;
        assert_eq!(
            read["authorizationType"].as_str(),
            Some(authorization_type),
            "case {case}: GetMethod must report the stored type, got {:?}",
            read["authorizationType"]
        );
    }

    // Rows differ along the member's type and the op: a boolean member, optional
    // strings, a map keyed by a parameter name, a map keyed by a content type whose `/`
    // is an escaped pointer token, and a string collection.
    let cases: Vec<Row> = vec![
        (
            "switch apiKeyRequired on",
            vec![replace("/apiKeyRequired", "true")],
            vec![("apiKeyRequired", json!(true))],
        ),
        (
            "switch apiKeyRequired off again",
            vec![replace("/apiKeyRequired", "false")],
            vec![("apiKeyRequired", json!(false))],
        ),
        (
            "replace the operation name",
            vec![replace("/operationName", "listAllPets")],
            vec![("operationName", json!("listAllPets"))],
        ),
        (
            "remove the operation name",
            vec![op("remove", "/operationName", None)],
            vec![("operationName", Value::Null)],
        ),
        (
            "set the authorizer and the validator in one request",
            vec![
                replace("/authorizerId", "authz1"),
                replace("/requestValidatorId", "val1"),
            ],
            vec![
                ("authorizerId", json!("authz1")),
                ("requestValidatorId", json!("val1")),
            ],
        ),
        (
            "add two request parameters",
            vec![
                op("add", "/requestParameters/method.request.querystring.q", Some("true")),
                op("add", "/requestParameters/method.request.header.X-Trace", Some("false")),
            ],
            vec![(
                "requestParameters",
                json!({
                    "method.request.querystring.q": true,
                    "method.request.header.X-Trace": false,
                }),
            )],
        ),
        (
            "replace one request parameter's flag",
            vec![replace("/requestParameters/method.request.querystring.q", "false")],
            vec![(
                "requestParameters",
                json!({
                    "method.request.querystring.q": false,
                    "method.request.header.X-Trace": false,
                }),
            )],
        ),
        (
            "remove one request parameter",
            vec![op("remove", "/requestParameters/method.request.header.X-Trace", None)],
            vec![(
                "requestParameters",
                json!({ "method.request.querystring.q": false }),
            )],
        ),
        (
            "add a request model under an escaped content type",
            vec![op("add", "/requestModels/application~1json", Some("Pet"))],
            vec![("requestModels", json!({ "application/json": "Pet" }))],
        ),
        (
            "replace that request model",
            vec![replace("/requestModels/application~1json", "Empty")],
            vec![("requestModels", json!({ "application/json": "Empty" }))],
        ),
        (
            "remove that request model",
            vec![op("remove", "/requestModels/application~1json", None)],
            vec![("requestModels", Value::Null)],
        ),
        (
            "add two authorization scopes",
            vec![
                op("add", "/authorizationScopes", Some("pets/read")),
                op("add", "/authorizationScopes", Some("pets/write")),
            ],
            vec![("authorizationScopes", json!(["pets/read", "pets/write"]))],
        ),
        (
            "remove one authorization scope",
            vec![op("remove", "/authorizationScopes", Some("pets/read"))],
            vec![("authorizationScopes", json!(["pets/write"]))],
        ),
    ];

    for (case, operations, expectations) in &cases {
        let updated = patch_ok(&gateway, case, &uri, operations.clone()).await;
        assert_members(&updated, expectations, case, "UpdateMethod");
        let read = get_method(&gateway, &api, &resource_id, "GET", case).await;
        assert_members(&read, expectations, case, "GetMethod");
        assert_eq!(
            read["httpMethod"].as_str(),
            Some("GET"),
            "case {case}: a patch must not change the method's verb, got {:?}",
            read["httpMethod"]
        );
    }

    // Rows differ along the kind of refusal; after each, the method is the one the
    // accepted rows left behind.
    let refused: Vec<(&str, Vec<Value>)> = vec![
        (
            "remove of a member that always has a value",
            vec![op("remove", "/authorizationType", None)],
        ),
        (
            "an empty authorization type",
            vec![replace("/authorizationType", "")],
        ),
        (
            "the method's own verb",
            vec![replace("/httpMethod", "POST")],
        ),
        (
            "a request parameter with no name in the path",
            vec![replace("/requestParameters", "true")],
        ),
        (
            "a non-boolean request parameter flag",
            vec![op("add", "/requestParameters/method.request.header.X", Some("maybe"))],
        ),
        (
            "a sub-resource with its own operation",
            vec![replace("/methodIntegration/type", "MOCK")],
        ),
    ];
    for (case, operations) in &refused {
        let answer = patch(&gateway, &uri, operations.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let read = get_method(&gateway, &api, &resource_id, "GET", case).await;
        assert_eq!(
            read["authorizationType"].as_str(),
            Some("COGNITO_USER_POOLS"),
            "case {case}: a refused patch must leave the stored method alone, got {:?}",
            read["authorizationType"]
        );
    }

    // An unknown API, resource or verb is the modelled NotFoundException.
    for (case, uri) in [
        (
            "an unknown api",
            format!("/restapis/zzzzzzzzzz/resources/{resource_id}/methods/GET"),
        ),
        (
            "an unknown resource",
            format!("/restapis/{api}/resources/zzzzzzzzzz/methods/GET"),
        ),
        (
            "a verb with no method on the resource",
            format!("/restapis/{api}/resources/{resource_id}/methods/DELETE"),
        ),
    ] {
        let answer = patch(&gateway, &uri, vec![replace("/apiKeyRequired", "true")]).await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }
}

#[tokio::test]
async fn a_patched_rest_api_member_is_stored_and_read_back() {
    let gateway = Gateway::new();
    let (api, _) = create_api(&gateway, "patch-rest-api").await;
    let uri = format!("/restapis/{api}");

    // Rows differ along the member's type: required and optional strings, a boolean, a
    // nullable integer, enum-typed members whose value sets the model declares, a
    // string collection addressed both ways, and the nested endpoint configuration.
    //
    // The `apiKeySource` catalog is the model's own `ApiKeySourceType` enum
    // (AUTHORIZER, HEADER) and the endpoint types are its `EndpointType` enum
    // (EDGE, PRIVATE, REGIONAL).
    let cases: Vec<Row> = vec![
        (
            "replace the name",
            vec![replace("/name", "patch-rest-api-renamed")],
            vec![("name", json!("patch-rest-api-renamed"))],
        ),
        (
            "set the description and the version at once",
            vec![
                replace("/description", "the patched api"),
                replace("/version", "2026-10-22"),
            ],
            vec![
                ("description", json!("the patched api")),
                ("version", json!("2026-10-22")),
            ],
        ),
        (
            "remove the description",
            vec![op("remove", "/description", None)],
            vec![("description", Value::Null)],
        ),
        (
            "switch the default endpoint off",
            vec![replace("/disableExecuteApiEndpoint", "true")],
            vec![("disableExecuteApiEndpoint", json!(true))],
        ),
        (
            "replace the api key source",
            vec![replace("/apiKeySource", "AUTHORIZER")],
            vec![("apiKeySource", json!("AUTHORIZER"))],
        ),
        (
            "replace the resource policy",
            vec![replace("/policy", "{\"Version\":\"2012-10-17\"}")],
            vec![("policy", json!("{\"Version\":\"2012-10-17\"}"))],
        ),
        (
            "set the minimum compression size to its lower bound",
            vec![replace("/minimumCompressionSize", "0")],
            vec![("minimumCompressionSize", json!(0))],
        ),
        (
            "set the minimum compression size to its upper bound",
            vec![replace("/minimumCompressionSize", "10485760")],
            vec![("minimumCompressionSize", json!(10_485_760))],
        ),
        (
            "remove the minimum compression size, which disables compression",
            vec![op("remove", "/minimumCompressionSize", None)],
            vec![("minimumCompressionSize", Value::Null)],
        ),
        (
            "add a binary media type by value",
            vec![op("add", "/binaryMediaTypes", Some("image/png"))],
            vec![("binaryMediaTypes", json!(["image/png"]))],
        ),
        (
            "add another as an escaped pointer token",
            vec![op("add", "/binaryMediaTypes/application~1pdf", None)],
            vec![("binaryMediaTypes", json!(["image/png", "application/pdf"]))],
        ),
        (
            "remove one binary media type",
            vec![op("remove", "/binaryMediaTypes", Some("image/png"))],
            vec![("binaryMediaTypes", json!(["application/pdf"]))],
        ),
        (
            "add an endpoint type",
            vec![op("add", "/endpointConfiguration/types/REGIONAL", None)],
            vec![(
                "endpointConfiguration",
                // The API was created with no endpointConfiguration, so it holds the
                // one a created API is given; the added type joins it.
                json!({ "types": ["EDGE", "REGIONAL"], "ipAddressType": "ipv4" }),
            )],
        ),
        (
            "set the endpoint ip address type",
            vec![replace("/endpointConfiguration/ipAddressType", "dualstack")],
            vec![(
                "endpointConfiguration",
                json!({ "types": ["EDGE", "REGIONAL"], "ipAddressType": "dualstack" }),
            )],
        ),
        (
            "remove an endpoint type",
            vec![op("remove", "/endpointConfiguration/types", Some("EDGE"))],
            vec![(
                "endpointConfiguration",
                json!({ "types": ["REGIONAL"], "ipAddressType": "dualstack" }),
            )],
        ),
    ];

    for (case, operations, expectations) in &cases {
        let updated = patch_ok(&gateway, case, &uri, operations.clone()).await;
        assert_members(&updated, expectations, case, "UpdateRestApi");
        let read = gateway.call("GET", &uri, None).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetRestApi, got {} with {:?}",
            read.status, read.body
        );
        assert_members(&read.body, expectations, case, "GetRestApi");
        assert_eq!(
            read.body["id"].as_str(),
            Some(api.as_str()),
            "case {case}: a patch must not change the API's id, got {:?}",
            read.body["id"]
        );
    }

    // Rows differ along the kind of refusal, including each published bound of
    // `minimumCompressionSize` overstepped by one — "non-negative between 0 and
    // 10485760 (10M) bytes, inclusive" (`RestApi$minimumCompressionSize`) — and a value
    // outside an enum-typed member's declared set.
    let refused: Vec<(&str, Vec<Value>)> = vec![
        (
            "a compression size below the lower bound",
            vec![replace("/minimumCompressionSize", "-1")],
        ),
        (
            "a compression size above the upper bound",
            vec![replace("/minimumCompressionSize", "10485761")],
        ),
        (
            "a compression size that is not an integer",
            vec![replace("/minimumCompressionSize", "1 MB")],
        ),
        (
            "an api key source outside the enum",
            vec![replace("/apiKeySource", "COOKIE")],
        ),
        (
            "an endpoint type outside the enum",
            vec![op("add", "/endpointConfiguration/types/SATELLITE", None)],
        ),
        (
            "an endpoint configuration member that does not exist",
            vec![replace("/endpointConfiguration/nosuchmember", "x")],
        ),
        ("removing the name", vec![op("remove", "/name", None)]),
        (
            "a service-assigned member",
            vec![replace("/rootResourceId", "zzzzzzzzzz")],
        ),
        (
            "tags, which have their own operations",
            vec![op("add", "/tags/team", Some("api"))],
        ),
    ];

    for (case, operations) in &refused {
        let answer = patch(&gateway, &uri, operations.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let read = gateway.call("GET", &uri, None).await;
        assert_eq!(
            read.body["name"].as_str(),
            Some("patch-rest-api-renamed"),
            "case {case}: a refused patch must leave the stored API alone, got {:?}",
            read.body["name"]
        );
        assert_eq!(
            read.body["minimumCompressionSize"],
            Value::Null,
            "case {case}: a refused patch must not set a member, got {:?}",
            read.body["minimumCompressionSize"]
        );
    }

    // An id no API has is the modelled NotFoundException.
    let answer = patch(&gateway, "/restapis/zzzzzzzzzz", vec![replace("/name", "x")]).await;
    assert_failure(
        &answer,
        404,
        "NotFoundException",
        "UpdateRestApi(unknown api)",
    );
}

#[tokio::test]
async fn a_patch_changes_only_the_record_it_names() {
    let gateway = Gateway::new();
    let (first_id, _) = baseline(&gateway, "first key").await;
    let (second_id, second) = baseline(&gateway, "second key").await;

    patch_ok(
        &gateway,
        "patch the first key",
        &format!("/apikeys/{first_id}"),
        vec![replace("/name", "only-the-first"), replace("/enabled", "false")],
    )
    .await;

    let untouched = get_key(&gateway, &second_id, false).await;
    for member in ["name", "description", "enabled", "createdDate", "lastUpdatedDate"] {
        assert_eq!(
            untouched.body[member], second[member],
            "case the other key: {member} must be untouched, got {:?} after {:?}",
            untouched.body[member], second[member]
        );
    }
}
