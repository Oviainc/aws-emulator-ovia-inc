// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE `UpdateApiKey` refuses an unapplicable patch path
//! with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the key helpers and the PATCH helpers
//! are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `api_key_lifecycle` and `patch_updates`.
//!
//! The RULE under test: a patch path this operation cannot apply is refused with a
//! `BadRequestException` (HTTP 400) whose message quotes the rejected path, names the
//! `op` of the same `patchOperations` entry and enumerates the paths THAT OP accepts — so
//! the enumeration varies with the op and never offers the caller a path the op it names
//! would refuse — and the stored key is left exactly as it was.
//!
//! The rows span what the rule generalises over rather than repeating one call: the form
//! of the unapplicable path (a member no shape declares, a member the service assigns,
//! the key's secret, a multi-valued member named by a scalar op, a scalar member named by
//! a collection op, a map entry, a pointer reaching into a single-valued member, a path
//! that is no JSON Pointer), the op that named it, and the entry's position in a batch.
//! Each op's enumerated set is pinned from the other side too: every path it names is
//! accepted for that op and reads back.

use serde_json::{json, Value};

use crate::api_key_lifecycle::{assert_failure, get_key};
use crate::patch_updates::{baseline, op, patch, patch_ok, replace};
use crate::rest_api_lifecycle::{create_api, Gateway};

/// The paths `UpdateApiKey` accepts for a `replace`, in the order the service's own
/// refusal enumerates them: the `ApiKey`'s updateable single-valued members
/// (`smithy_query(query="operation", operation="UpdateApiKey")` for the shape, and the
/// enumeration AWS prints in this refusal for the order).
const REPLACE_PATHS: [&str; 4] = ["/description", "/enabled", "/name", "/customerId"];

/// The paths it accepts for an `add` or a `remove`: the key's multi-valued members, the
/// ones an element is added to or removed from. The order is the one AWS prints — a
/// recorded refusal of a `remove` of a scalar path enumerates `[/stages, /labels]`.
const COLLECTION_PATHS: [&str; 2] = ["/stages", "/labels"];

/// The one sentence API Gateway refuses an `op` it supports no path for with: `move`,
/// `copy` and `test` name nothing on these resources, so there is no permitted-path set
/// to enumerate for them.
const INVALID_OP: &str = "Invalid patch operation specified. Must be 'add'|'remove'|'replace'";

/// The paths `op` accepts on an API key, or `None` when the key supports no path for it.
fn supported_paths(op: &str) -> Option<&'static [&'static str]> {
    match op {
        "replace" => Some(&REPLACE_PATHS),
        "add" | "remove" => Some(&COLLECTION_PATHS),
        _ => None,
    }
}

/// The message this operation refuses `path`, named by `op`, with.
fn expected_message(path: &str, op: &str) -> String {
    match supported_paths(op) {
        Some(paths) => format!(
            "Invalid patch path  '{path}' specified for op '{op}'. Must be one of: [{}]",
            paths.join(", ")
        ),
        None => INVALID_OP.to_string(),
    }
}

/// The members the baseline key reports before any patch is applied.
fn baseline_members() -> Vec<(&'static str, Value)> {
    vec![
        ("name", json!("baseline-key")),
        ("description", json!("initial description")),
        ("enabled", json!(true)),
        ("customerId", Value::Null),
    ]
}

/// Assert `body` still reports the baseline members, i.e. the refused patch changed
/// nothing.
fn assert_unchanged(body: &Value, case: &str) {
    for (member, expected) in baseline_members() {
        assert_eq!(
            body[member], expected,
            "case {case}: a refused patch must leave {member} at {expected:?}, got {:?}",
            body[member]
        );
    }
}

/// Assert `answer` is the modelled refusal of `path` under `op`, and that the enumerated
/// set — the information the message exists to convey — does not offer the caller the
/// very path the op just rejected.
fn assert_refused(answer: &crate::rest_api_lifecycle::Answer, path: &str, kind: &str, case: &str) {
    assert_failure(answer, 400, "BadRequestException", case);
    let expected = expected_message(path, kind);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(expected.as_str()),
        "case {case}: expected {expected:?}, got {:?}",
        answer.body["message"]
    );
    if let Some(paths) = supported_paths(kind) {
        assert!(
            !paths.contains(&path),
            "case {case}: the enumerated set {paths:?} must not hold the rejected path {path:?}"
        );
    }
}

#[tokio::test]
async fn an_unapplicable_api_key_patch_path_is_refused_with_the_path_the_op_and_that_ops_set() {
    let gateway = Gateway::new();

    // Rows differ in the FORM of the unapplicable path — a member no shape declares, a
    // member the service assigns, the key's secret, a collection member addressed as a
    // whole, a map entry, a pointer reaching into a single-valued member, a path that is
    // no JSON Pointer at all, an empty path — and in the op that named it, which is what
    // selects the enumerated set.
    let rows: [(&str, &str); 20] = [
        ("/thisFieldDoesNotExist", "replace"),
        ("/value", "replace"),
        ("/id", "replace"),
        ("/createdDate", "replace"),
        ("/lastUpdatedDate", "remove"),
        ("/stageKeys", "replace"),
        ("/tags/team", "add"),
        ("/name/first", "replace"),
        ("/description", "remove"),
        ("/description", "add"),
        ("/enabled", "remove"),
        ("/name", "add"),
        ("/enabled", "add"),
        ("/customerId", "add"),
        ("/stages", "replace"),
        ("/labels", "replace"),
        ("name", "replace"),
        ("name", "remove"),
        ("", "replace"),
        ("", "add"),
    ];

    for (path, kind) in rows {
        let case = format!("UpdateApiKey({kind} {path})");
        let (id, _) = baseline(&gateway, &case).await;
        let answer = patch(
            &gateway,
            &format!("/apikeys/{id}"),
            vec![op(kind, path, Some("x"))],
        )
        .await;
        assert_refused(&answer, path, kind, &case);

        // The refusal mutated nothing: the stored record is the one the key was created
        // with.
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApiKey, got {} with {:?}",
            read.status, read.body
        );
        assert_unchanged(&read.body, &case);
    }
}

#[tokio::test]
async fn the_enumerated_set_follows_the_op_the_request_sent() {
    let gateway = Gateway::new();

    // ONE path, every op the model's `Op` enum declares: the enumeration must be the set
    // belonging to the op that was submitted, so the same rejected path yields different
    // sets under different ops, and an op with no set at all is refused as an op.
    let paths = ["/description", "/enabled", "/thisFieldDoesNotExist", "/id"];
    let ops = ["add", "remove", "move", "copy", "test"];

    for path in paths {
        for kind in ops {
            let case = format!("UpdateApiKey({kind} {path})");
            let (id, _) = baseline(&gateway, &case).await;
            let answer = patch(
                &gateway,
                &format!("/apikeys/{id}"),
                vec![op(kind, path, Some("x"))],
            )
            .await;
            assert_refused(&answer, path, kind, &case);
        }
    }

    // A path no op accepts is refused under a `replace` too, with the replace set.
    for path in ["/thisFieldDoesNotExist", "/id"] {
        let case = format!("UpdateApiKey(replace {path})");
        let (id, _) = baseline(&gateway, &case).await;
        let answer = patch(&gateway, &format!("/apikeys/{id}"), vec![replace(path, "x")]).await;
        assert_refused(&answer, path, "replace", &case);
    }
}

#[tokio::test]
async fn the_refusal_names_the_offending_entry_of_a_batch() {
    let gateway = Gateway::new();

    // Rows differ in where the unapplicable entry sits in the batch and in how many
    // applicable entries surround it: the message must name THAT entry's path and op,
    // whichever position it took, and the applicable entries must not be kept.
    let rows: Vec<(&str, Vec<Value>, &str, &str)> = vec![
        (
            "the only entry",
            vec![op("replace", "/value", Some("x"))],
            "/value",
            "replace",
        ),
        (
            "the last of two entries",
            vec![
                replace("/description", "never stored"),
                op("replace", "/thisFieldDoesNotExist", Some("x")),
            ],
            "/thisFieldDoesNotExist",
            "replace",
        ),
        (
            "the first of two entries",
            vec![
                op("add", "/value", Some("x")),
                replace("/description", "never stored"),
            ],
            "/value",
            "add",
        ),
        (
            "the middle of three entries",
            vec![
                replace("/name", "never stored"),
                op("remove", "/id", None),
                replace("/enabled", "false"),
            ],
            "/id",
            "remove",
        ),
    ];

    for (case, operations, path, kind) in rows {
        let (id, _) = baseline(&gateway, case).await;
        let answer = patch(&gateway, &format!("/apikeys/{id}"), operations).await;
        assert_refused(&answer, path, kind, case);

        let read = get_key(&gateway, &id, false).await;
        assert_unchanged(&read.body, case);
    }
}

#[tokio::test]
async fn every_path_the_refusal_enumerates_is_accepted_for_the_op_it_names() {
    let gateway = Gateway::new();

    // The other side of the rule: the set a refusal names for an op is the set that op
    // accepts, so each of its entries takes that op and reads back. `/enabled` takes the
    // boolean it is modelled as; the rest take strings.
    for path in REPLACE_PATHS {
        let case = format!("UpdateApiKey(replace {path})");
        let (id, _) = baseline(&gateway, &case).await;
        let (sent, expected) = if path == "/enabled" {
            ("false", json!(false))
        } else {
            ("patched", json!("patched"))
        };
        let member = path.trim_start_matches('/');
        let updated = patch_ok(
            &gateway,
            &case,
            &format!("/apikeys/{id}"),
            vec![replace(path, sent)],
        )
        .await;
        assert_eq!(
            updated[member], expected,
            "case {case}: UpdateApiKey must report {member} as {expected:?}, got {:?}",
            updated[member]
        );

        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.body[member], expected,
            "case {case}: GetApiKey must read back {member} as {expected:?}, got {:?}",
            read.body[member]
        );
    }

    // The collection set the same way: every path it names takes an `add` and a `remove`.
    // `/stages` is the member `stageKeys` reports, so the applied entry reads back there;
    // a label is reported by no member of the modelled `ApiKey`, so for it the accepted
    // 200 is what there is to assert.
    let (api, _root) = create_api(&gateway, "enumerated-collection-paths").await;
    for path in COLLECTION_PATHS {
        let case = format!("UpdateApiKey(add {path})");
        let (id, _) = baseline(&gateway, &case).await;
        let uri = format!("/apikeys/{id}");
        let entry = if path == "/stages" {
            format!("{api}/prod")
        } else {
            "production".to_string()
        };
        let expected = if path == "/stages" {
            json!([entry.clone()])
        } else {
            json!([])
        };

        let added = patch_ok(&gateway, &case, &uri, vec![op("add", path, Some(&entry))]).await;
        assert_eq!(
            added["stageKeys"], expected,
            "case {case}: expected stageKeys {expected:?}, got {:?}",
            added["stageKeys"]
        );
        let read = get_key(&gateway, &id, false).await;
        assert_eq!(
            read.body["stageKeys"], expected,
            "case {case}: GetApiKey must report the applied add, got {:?}",
            read.body["stageKeys"]
        );

        let case = format!("UpdateApiKey(remove {path})");
        let removed = patch_ok(&gateway, &case, &uri, vec![op("remove", path, Some(&entry))]).await;
        assert_eq!(
            removed["stageKeys"],
            json!([]),
            "case {case}: a remove must drop the entry again, got {:?}",
            removed["stageKeys"]
        );
    }
}
