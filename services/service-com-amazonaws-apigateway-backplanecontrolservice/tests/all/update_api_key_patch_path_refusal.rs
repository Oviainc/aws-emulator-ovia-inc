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
//! `op` of the same `patchOperations` entry and enumerates the paths the operation does
//! accept — so a caller reading it learns which entry of the batch was bad and what it
//! could have sent instead — and the stored key is left exactly as it was.
//!
//! The rows span what the rule generalises over rather than repeating one call: the form
//! of the unapplicable path (a member no shape declares, a member the service assigns,
//! the key's secret, a collection member, a map entry, a pointer reaching into a
//! single-valued member), the op that named it, and the entry's position in a batch. The
//! enumerated set is pinned from the other side too: every path it names is accepted for
//! a `replace` and reads back.

use serde_json::{json, Value};

use crate::api_key_lifecycle::{assert_failure, get_key};
use crate::patch_updates::{baseline, op, patch, patch_ok, replace};
use crate::rest_api_lifecycle::Gateway;

/// The paths `UpdateApiKey` accepts for a `replace`, in the order the service's own
/// refusal enumerates them: the `ApiKey`'s updateable single-valued members
/// (`smithy_query(query="operation", operation="UpdateApiKey")` for the shape, and the
/// enumeration AWS prints in this refusal for the order).
const SUPPORTED_PATHS: [&str; 4] = ["/description", "/enabled", "/name", "/customerId"];

/// The message this operation refuses `path`, named by `op`, with.
fn expected_message(path: &str, op: &str) -> String {
    format!(
        "Invalid patch path  '{path}' specified for op '{op}'. Must be one of: [{}]",
        SUPPORTED_PATHS.join(", ")
    )
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

#[tokio::test]
async fn an_unapplicable_api_key_patch_path_is_refused_with_the_path_the_op_and_the_supported_set() {
    let gateway = Gateway::new();

    // Rows differ in the FORM of the unapplicable path — a member no shape declares, a
    // member the service assigns, the key's secret, a collection member addressed as a
    // whole, a map entry, a pointer reaching into a single-valued member, a path that is
    // no JSON Pointer at all, an empty path — and in the op that named it.
    let rows: [(&str, &str); 14] = [
        ("/thisFieldDoesNotExist", "replace"),
        ("/value", "replace"),
        ("/id", "replace"),
        ("/createdDate", "replace"),
        ("/lastUpdatedDate", "remove"),
        ("/stageKeys", "replace"),
        ("/tags/team", "add"),
        ("/name/first", "replace"),
        ("/description", "remove"),
        ("/name", "add"),
        ("/enabled", "add"),
        ("/customerId", "add"),
        ("name", "replace"),
        ("", "replace"),
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
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected_message(path, kind).as_str()),
            "case {case}: expected {:?}, got {:?}",
            expected_message(path, kind),
            answer.body["message"]
        );

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
        assert_failure(&answer, 400, "BadRequestException", case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected_message(path, kind).as_str()),
            "case {case}: expected {:?}, got {:?}",
            expected_message(path, kind),
            answer.body["message"]
        );

        let read = get_key(&gateway, &id, false).await;
        assert_unchanged(&read.body, case);
    }
}

#[tokio::test]
async fn every_path_the_refusal_enumerates_is_accepted_for_a_replace() {
    let gateway = Gateway::new();

    // The other side of the rule: the set the refusal names is the set the operation
    // accepts, so each of its entries takes a `replace` and reads back. `/enabled` takes
    // the boolean it is modelled as; the rest take strings.
    for path in SUPPORTED_PATHS {
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
}
