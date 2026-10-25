// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDocumentationVersion`.
//!
//! Every case drives the real wire path — a `restJson1` `PATCH
//! /restapis/{restApiId}/documentation/versions/{documentationVersion}` through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], [`Answer`], `create_api`,
//! `string_member`, `page_items` and `assert_failure` are this crate's one wire-test
//! harness, reused from `rest_api_lifecycle`, and the snapshots under test are created
//! and read through the `documentation_versions` suite's own helpers.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a patch of `/description` stores the op's value as the snapshot's description, over
//!   a TABLE of descriptions differing along what the text holds (length, emptiness,
//!   script, escapes, the store's character bound) and over both ops that carry a value
//!   (`add`, `replace`), each read back through `GetDocumentationVersion` and
//!   `GetDocumentationVersions`;
//! * a `remove` clears the description, which an optional member can be;
//! * the members a patch does not name are untouched: the version identifier stays the
//!   record's key and the creation date is not re-stamped by an update;
//! * a path naming no updateable member of the `DocumentationVersion` shape is refused
//!   with this service's one unaddressable-path sentence, carrying the path the request
//!   sent and nothing else;
//! * an op the resource does not support is refused, over `move`, `copy` and `test`, and
//!   over an entry carrying no op or no path at all;
//! * a refusal changes nothing — the stored snapshot is re-read after each one, and a
//!   list whose LATER op is refused leaves the whole list unapplied;
//! * an update of an API or a snapshot that does not exist is the modelled 404, including
//!   a version identifier addressed through another API.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    invalid_patch_path, MAX_STRING_CHARS,
};

use crate::documentation_versions::{create_version, get_version, snapshot_request};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// The version identifier the snapshots under test are created with.
const VERSION: &str = "1.0.0";

/// The description they are created with, so a refused patch is seen not to have changed
/// it.
const ORIGINAL_DESCRIPTION: &str = "the original snapshot description";

/// One API with one documentation version on it, as every case below starts.
async fn api_with_version(gateway: &Gateway, name: &str) -> String {
    let (api, _root) = create_api(gateway, name).await;
    let created = create_version(
        gateway,
        &api,
        snapshot_request(VERSION, Some(ORIGINAL_DESCRIPTION)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case setup {name}: expected 201 from CreateDocumentationVersion, got {} with {:?}",
        created.status, created.body
    );
    api
}

/// `UpdateDocumentationVersion` with `patches` sent verbatim as `patchOperations`.
async fn update_version(gateway: &Gateway, api: &str, version: &str, patches: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/documentation/versions/{version}"),
            Some(json!({ "patchOperations": patches })),
        )
        .await
}

/// One `patchOperations` entry, with `value` present only when the case carries one.
fn patch(op: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": op, "path": path, "value": value }),
        None => json!({ "op": op, "path": path }),
    }
}

/// The `DocumentationVersion` `GetDocumentationVersion` reports, asserting the modelled
/// 200.
async fn read_version(gateway: &Gateway, api: &str, version: &str, case: &str) -> Value {
    let answer = get_version(gateway, api, version).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetDocumentationVersion expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The same record read through the COLLECTION, so an update is not only visible to the
/// single reader.
async fn listed_version(gateway: &Gateway, api: &str, version: &str, case: &str) -> Value {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/versions?limit=500"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetDocumentationVersions expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let items = page_items(&answer.body, case);
    items
        .iter()
        .find(|item| item["version"].as_str() == Some(version))
        .unwrap_or_else(|| {
            panic!("case {case}: expected the snapshot {version} to be listed, got {items:?}")
        })
        .clone()
}

/// Assert `body` reports `description` for the snapshot's description — `None` for a
/// snapshot whose description was cleared or never set.
fn assert_description(body: &Value, description: Option<&str>, case: &str, source: &str) {
    match description {
        Some(expected) => assert_eq!(
            body["description"].as_str(),
            Some(expected),
            "case {case}: {source} expected description {expected:?}, got {:?}",
            body["description"]
        ),
        None => assert!(
            body["description"].is_null(),
            "case {case}: {source} expected no description, got {:?}",
            body["description"]
        ),
    }
}

/// The descriptions a patch must be able to set, differing along what the text holds: one
/// word, a sentence, no text at all, text outside ASCII, text carrying JSON escapes, and
/// text AT the store's character bound (this crate's own `MAX_STRING_CHARS`; API Gateway
/// publishes no length for `DocumentationVersion$description`). Each row is a value the
/// model admits: `description` is a plain `String` with no pattern and no length
/// constraint (`smithy_query(query="shape", shape="DocumentationVersion")`).
fn descriptions() -> Vec<(&'static str, String)> {
    vec![
        ("one word", "released".to_string()),
        (
            "a sentence",
            "the snapshot published to the pet store portal".to_string(),
        ),
        ("no text at all", String::new()),
        ("text outside ASCII", "ロボット — les animaux".to_string()),
        (
            "text carrying JSON escapes",
            "a \"quoted\" line\nand a tab\there".to_string(),
        ),
        ("text at the character bound", "d".repeat(MAX_STRING_CHARS)),
    ]
}

#[tokio::test]
async fn a_patch_of_description_stores_the_value_it_carries() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-description").await;
    let created_date = read_version(&gateway, &api, VERSION, "setup").await["createdDate"].clone();

    // Both ops that carry a value set the member: `value` is "applicable for the add or
    // replace operation" (`PatchOperation$value`).
    for op in ["replace", "add"] {
        for (row, description) in descriptions() {
            let case = format!("op={op}, {row}");
            let answer = update_version(
                &gateway,
                &api,
                VERSION,
                json!([patch(op, "/description", Some(&description))]),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_description(&answer.body, Some(&description), &case, "the update");
            // The members the patch did not name are untouched: the identifier stays the
            // record's key, and the creation date is stamped once by the create rather
            // than re-stamped by an update.
            assert_eq!(
                answer.body["version"].as_str(),
                Some(VERSION),
                "case {case}: expected the version identifier back, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["createdDate"], created_date,
                "case {case}: an update must not re-stamp the creation date, got {:?} after {:?}",
                answer.body["createdDate"], created_date
            );

            // Round trip: the patched snapshot is read back through both implemented
            // readers, not just reported by the writer.
            let read = read_version(&gateway, &api, VERSION, &case).await;
            assert_eq!(
                read, answer.body,
                "case {case}: the read must report what the update stored, got {read:?} after {:?}",
                answer.body
            );
            let listed = listed_version(&gateway, &api, VERSION, &case).await;
            assert_description(&listed, Some(&description), &case, "the collection");
        }
    }
}

#[tokio::test]
async fn a_remove_clears_the_description_and_an_empty_list_changes_nothing() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-remove").await;

    // `remove` clears a member a snapshot need not carry: `description` is optional on
    // `CreateDocumentationVersionRequest`, so a snapshot with none is a state the store
    // holds.
    let case = "remove /description";
    let answer = update_version(
        &gateway,
        &api,
        VERSION,
        json!([patch("remove", "/description", None)]),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_description(&answer.body, None, case, "the update");
    assert_description(
        &read_version(&gateway, &api, VERSION, case).await,
        None,
        case,
        "GetDocumentationVersion",
    );
    assert_description(
        &listed_version(&gateway, &api, VERSION, case).await,
        None,
        case,
        "GetDocumentationVersions",
    );

    // A list of no ops is a request that changes nothing and still reports the stored
    // snapshot.
    let case = "an empty list of ops";
    let answer = update_version(&gateway, &api, VERSION, json!([])).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_description(&answer.body, None, case, "the update");

    // And a later `replace` sets it again, so the clear above is not one-way.
    let case = "replace after a remove";
    let answer = update_version(
        &gateway,
        &api,
        VERSION,
        json!([patch("replace", "/description", Some("set again"))]),
    )
    .await;
    assert_description(&answer.body, Some("set again"), case, "the update");
    assert_description(
        &read_version(&gateway, &api, VERSION, case).await,
        Some("set again"),
        case,
        "GetDocumentationVersion",
    );
}

#[tokio::test]
async fn a_list_of_ops_is_applied_in_request_order() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-lists").await;

    // Each row is a list of descriptions to set in one request; the request's answer is
    // the LAST one, because the ops are applied in request order.
    let rows: &[&[&str]] = &[
        &["only op"],
        &["first", "second"],
        &["one", "", "three"],
    ];
    for values in rows {
        let case = format!("a list of {} ops", values.len());
        let patches: Vec<Value> = values
            .iter()
            .map(|value| patch("replace", "/description", Some(value)))
            .collect();
        let expected = values
            .last()
            .copied()
            .expect("every row carries at least one op");
        let answer = update_version(&gateway, &api, VERSION, json!(patches)).await;
        assert_description(&answer.body, Some(expected), &case, "the update");
        assert_description(
            &read_version(&gateway, &api, VERSION, &case).await,
            Some(expected),
            &case,
            "GetDocumentationVersion",
        );
    }
}

#[tokio::test]
async fn a_path_naming_no_updateable_member_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-paths").await;

    // Each row is a path naming a member of the `DocumentationVersion` shape that a patch
    // cannot change — the identifier the record is keyed by and the service-stamped
    // creation date — or a member the shape does not declare at all. The refusal's TEXT is
    // part of the rule: this control plane answers every unaddressable path with the one
    // sentence `invalid_patch_path` builds, carrying the path the request sent and nothing
    // else, whichever op named it.
    for path in ["/version", "/createdDate", "/unknown"] {
        for op in ["replace", "add", "remove"] {
            let case = format!("op={op} {path}");
            let answer =
                update_version(&gateway, &api, VERSION, json!([patch(op, path, Some("x"))])).await;
            assert_failure(&answer, 400, BAD_REQUEST, &case);
            assert_eq!(
                answer.body["message"].as_str(),
                Some(invalid_patch_path(path).as_str()),
                "case {case}: the refusal must be the unaddressable-path sentence carrying the path alone, got {:?}",
                answer.body["message"]
            );
            assert_description(
                &read_version(&gateway, &api, VERSION, &case).await,
                Some(ORIGINAL_DESCRIPTION),
                &case,
                "GetDocumentationVersion",
            );
        }
    }

    // A path that reaches INTO the single-valued description, and a `path` that is not a
    // JSON Pointer into the resource at all, are the same 400 carrying that same sentence.
    for path in ["/description/text", "/description/0", "/", "", "description"] {
        let case = format!("replace {path:?}");
        let answer = update_version(
            &gateway,
            &api,
            VERSION,
            json!([patch("replace", path, Some("x"))]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(invalid_patch_path(path).as_str()),
            "case {case}: expected the unaddressable-path sentence, got {:?}",
            answer.body["message"]
        );
        assert_description(
            &read_version(&gateway, &api, VERSION, &case).await,
            Some(ORIGINAL_DESCRIPTION),
            &case,
            "GetDocumentationVersion",
        );
    }

    // A value past the store's character bound is refused, and the bound itself is
    // accepted — asserted in the round-trip table above — so this is the bound and not a
    // blanket refusal of long text.
    let case = "one character past the bound";
    let past_bound = "d".repeat(MAX_STRING_CHARS + 1);
    let answer = update_version(
        &gateway,
        &api,
        VERSION,
        json!([patch("replace", "/description", Some(&past_bound))]),
    )
    .await;
    assert_failure(&answer, 400, BAD_REQUEST, case);
    assert_description(
        &read_version(&gateway, &api, VERSION, case).await,
        Some(ORIGINAL_DESCRIPTION),
        case,
        "GetDocumentationVersion",
    );
}

#[tokio::test]
async fn an_op_the_snapshot_does_not_support_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-ops").await;

    // `move`, `copy` and `test` name nothing on a single-valued member: "Not all valid
    // operations are supported for a given resource" (`PatchOperation$op`).
    for op in ["move", "copy", "test"] {
        let case = format!("op={op} on /description");
        let answer = update_version(
            &gateway,
            &api,
            VERSION,
            json!([patch(op, "/description", Some(ORIGINAL_DESCRIPTION))]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        assert_description(
            &read_version(&gateway, &api, VERSION, &case).await,
            Some(ORIGINAL_DESCRIPTION),
            &case,
            "GetDocumentationVersion",
        );
    }

    // An entry the handler cannot read at all is the same 400: the model declares `op`
    // and `path` optional, so an entry carrying neither reaches the handler.
    let incomplete: &[(&str, Value)] = &[
        ("no op", json!({ "path": "/description", "value": "x" })),
        ("no path", json!({ "op": "replace", "value": "x" })),
        ("neither op nor path", json!({})),
    ];
    for (case, entry) in incomplete {
        let answer = update_version(&gateway, &api, VERSION, json!([entry.clone()])).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        assert_description(
            &read_version(&gateway, &api, VERSION, case).await,
            Some(ORIGINAL_DESCRIPTION),
            case,
            "GetDocumentationVersion",
        );
    }

    // An `add` or `replace` carrying no value has nothing to set the member to.
    for op in ["replace", "add"] {
        let case = format!("op={op}, no value");
        let answer = update_version(
            &gateway,
            &api,
            VERSION,
            json!([patch(op, "/description", None)]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        assert_description(
            &read_version(&gateway, &api, VERSION, &case).await,
            Some(ORIGINAL_DESCRIPTION),
            &case,
            "GetDocumentationVersion",
        );
    }
}

#[tokio::test]
async fn a_list_whose_later_op_is_refused_leaves_the_snapshot_untouched() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-atomicity").await;

    // Each row is a second op that is refused for a different reason, after an op that
    // would have been applied: an unsupported path, an unsupported op, an op carrying no
    // value, and a value past the store's character bound.
    let past_bound = "d".repeat(MAX_STRING_CHARS + 1);
    let rows: Vec<(&str, Value)> = vec![
        (
            "an unsupported path",
            patch("replace", "/version", Some("2.0.0")),
        ),
        (
            "an unsupported op",
            patch("move", "/description", Some("moved")),
        ),
        (
            "an add carrying no value",
            patch("add", "/description", None),
        ),
        (
            "a value past the character bound",
            patch("replace", "/description", Some(&past_bound)),
        ),
    ];
    for (case, refused) in &rows {
        let answer = update_version(
            &gateway,
            &api,
            VERSION,
            json!([
                patch("replace", "/description", Some("would be applied")),
                refused.clone()
            ]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        assert_description(
            &read_version(&gateway, &api, VERSION, case).await,
            Some(ORIGINAL_DESCRIPTION),
            case,
            "GetDocumentationVersion",
        );
    }
}

#[tokio::test]
async fn an_update_of_a_record_that_does_not_exist_is_not_found() {
    let gateway = Gateway::new();
    let api = api_with_version(&gateway, "update-documentation-version-missing").await;
    // A second API with no snapshot of its own, so a version identifier addressed through
    // the wrong API is a miss rather than a cross-API read.
    let (other, _root) = create_api(&gateway, "update-documentation-version-other").await;

    // Each row is an identifier pair naming no stored snapshot: an API that was never
    // created, an identifier that was never created, one that differs only in case, an
    // identifier addressed through another API, and the API's own id used as one.
    let rows: &[(&str, &str, &str)] = &[
        ("an unknown API", "zzzzzzzzzz", VERSION),
        ("an unknown snapshot", api.as_str(), "2.0.0"),
        (
            "an identifier that extends a stored one",
            api.as_str(),
            "1.0.0-beta",
        ),
        ("a snapshot of another API", other.as_str(), VERSION),
        ("the API id used as an identifier", api.as_str(), api.as_str()),
    ];
    for (case, rest_api, version) in rows {
        let answer = update_version(
            &gateway,
            rest_api,
            version,
            json!([patch("replace", "/description", Some("x"))]),
        )
        .await;
        assert_failure(&answer, 404, NOT_FOUND, case);
    }

    // None of those refusals touched the snapshot that does exist.
    let read = read_version(&gateway, &api, VERSION, "after the not-found cases").await;
    assert_description(
        &read,
        Some(ORIGINAL_DESCRIPTION),
        "after the not-found cases",
        "GetDocumentationVersion",
    );
    assert_eq!(
        string_member(&read, "version", "after the not-found cases"),
        VERSION,
        "case after the not-found cases: expected the identifier back, got {read:?}"
    );
}
