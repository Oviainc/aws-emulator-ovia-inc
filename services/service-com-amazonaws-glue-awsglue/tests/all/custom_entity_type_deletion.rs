// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCustomEntityType`: the delete that removes one custom
//! sensitive-data pattern from the per-account namespace `CreateCustomEntityType` wrote
//! it into.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The suites are tables: each row is one input along an axis the rule generalises over
//! (how many context words the pattern carries, the form and length of its name, whether
//! it was created with tags; and, for the refusals, the way the requested name misses).
//! The bare name-plus-expression pattern a lifecycle client creates and then deletes is
//! one row among them.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The largest `ContextWords` list the model accepts: "len 1..20" on
/// `CreateCustomEntityTypeRequest$ContextWords` (`smithy_query
/// operation=CreateCustomEntityType`).
const MAX_CONTEXT_WORDS: usize = 20;

/// The longest `Name` the model accepts: "len 1..255" on `NameString` (`smithy_query
/// operation=DeleteCustomEntityType`).
const MAX_NAME_LENGTH: usize = 255;

/// Create a custom pattern.
async fn create(router: &smithy_server_runtime::MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateCustomEntityType", body).await
}

/// Delete the pattern called `name`, which must succeed, and return the body.
async fn delete(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Value {
    ok(router, "DeleteCustomEntityType", &json!({ "Name": name })).await
}

/// The names `ListCustomEntityTypes` reports, in the order the service returned them.
async fn listed_names(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListCustomEntityTypes", &json!({})).await;
    page["CustomEntityTypes"]
        .as_array()
        .expect("CustomEntityTypes is a list")
        .iter()
        .map(|pattern| {
            pattern["Name"]
                .as_str()
                .expect("CustomEntityType.Name is a string")
                .to_string()
        })
        .collect()
}

/// The rows every suite here deletes over: one pattern per axis the delete generalises
/// over, each a distinct record in the one per-account namespace.
fn deletion_cases() -> Vec<(&'static str, Value)> {
    let long_name = "a".repeat(MAX_NAME_LENGTH);
    let context_words: Vec<String> = (0..MAX_CONTEXT_WORDS)
        .map(|index| format!("context-word-{index}"))
        .collect();

    vec![
        (
            // The shape a lifecycle client sends: the two required members alone.
            "required members only",
            json!({
                "Name": "employee-id",
                "RegexString": "[0-9]{3}-[0-9]{2}-[0-9]{4}",
            }),
        ),
        (
            "one context word",
            json!({
                "Name": "patient-record",
                "RegexString": "PAT-[0-9]+",
                "ContextWords": ["patient"],
            }),
        ),
        (
            "context words at the modelled maximum",
            json!({
                "Name": "twenty-context-words",
                "RegexString": "SECRET-[A-Z]+",
                "ContextWords": context_words,
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": long_name, "RegexString": "x" }),
        ),
        (
            "a name carrying spaces and a tab",
            json!({
                "Name": "pattern one\ttwo",
                "RegexString": "[A-Za-z ]+",
                "ContextWords": ["two words"],
            }),
        ),
        (
            "tags sent with the create",
            json!({
                "Name": "tagged-pattern",
                "RegexString": "TAG-[0-9]{4}",
                "Tags": { "owner": "data-governance" },
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The delete removes the addressed pattern and reports its name
// ---------------------------------------------------------------------------

/// `DeleteCustomEntityType` removes the one record stored under the name it was given
/// and answers with that name, so the pattern a read resolved before the delete resolves
/// to nothing after it — whatever the deleted pattern's definition was.
#[tokio::test]
async fn deleting_a_pattern_removes_it_and_reports_its_name() {
    let router = emulator();

    let cases = deletion_cases();
    for (_, request) in &cases {
        create(&router, request).await;
    }

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // The pattern resolves before the delete, so the miss afterwards is the delete's
        // doing and not a lookup that never worked.
        let read = ok(&router, "GetCustomEntityType", &json!({ "Name": name })).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: the pattern must resolve before the delete, got {read:?}"
        );

        // "The name of the custom pattern you deleted."
        let deleted = delete(&router, name).await;
        assert_eq!(
            deleted.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: DeleteCustomEntityType must report the deleted name, got {deleted:?}"
        );

        let (status, body) = call(&router, "GetCustomEntityType", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a read after the delete must be refused with 400, got {status} with \
             {body:?}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException after the delete, got {body:?}"
        );
        // The read's miss is worded the way the delete's own miss is: the pattern the
        // request addressed, and the account whose namespace now holds none under it.
        let expected_message =
            format!("An existing Custom Entity Type: {name} cannot be found for Account Id {ACCOUNT_ID}");
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case}: expected the message {expected_message:?}, got {body:?}"
        );
    }

    // Every record is gone, and the namespace is the only place they were held.
    let listed = listed_names(&router).await;
    assert!(
        listed.is_empty(),
        "case every pattern deleted: expected an empty listing, got {listed:?}"
    );
}

/// A delete removes ONLY the pattern it names: every other stored pattern keeps its own
/// definition, and the listing still reports it.
#[tokio::test]
async fn a_delete_leaves_every_other_pattern_alone() {
    let router = emulator();

    let cases = deletion_cases();
    for (_, request) in &cases {
        create(&router, request).await;
    }

    let mut remaining: Vec<String> = cases
        .iter()
        .map(|(_, request)| request["Name"].as_str().unwrap().to_string())
        .collect();

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        delete(&router, name).await;
        remaining.retain(|stored| stored != name);

        // What is left is exactly the patterns not yet deleted, each still carrying the
        // expression its own create stored.
        let mut listed = listed_names(&router).await;
        listed.sort();
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: the listing must report only the patterns not yet deleted, got \
             {listed:?}"
        );

        for (other_case, other) in &cases {
            let other_name = other["Name"].as_str().expect("the case carries a Name");
            if !remaining.iter().any(|stored| stored == other_name) {
                continue;
            }
            let read = ok(
                &router,
                "GetCustomEntityType",
                &json!({ "Name": other_name }),
            )
            .await;
            assert_eq!(
                read.get("RegexString"),
                other.get("RegexString"),
                "case {case} then {other_case}: a surviving pattern keeps its own RegexString, \
                 got {read:?}"
            );
        }
    }
}

/// A deleted name is free again: a create under it succeeds and stores the NEW
/// definition, so the delete removed the record rather than reserving the name.
#[tokio::test]
async fn a_deleted_name_can_be_created_again() {
    let router = emulator();

    for (case, first, second) in [
        (
            "a different expression",
            json!({ "Name": "recreated-one", "RegexString": "FIRST-[0-9]+" }),
            json!({ "Name": "recreated-one", "RegexString": "SECOND-[0-9]+" }),
        ),
        (
            "context words added on the second create",
            json!({ "Name": "recreated-two", "RegexString": "KEEP-[0-9]+" }),
            json!({
                "Name": "recreated-two",
                "RegexString": "KEEP-[0-9]+",
                "ContextWords": ["added"],
            }),
        ),
    ] {
        let name = first["Name"].as_str().expect("the case carries a Name");
        create(&router, &first).await;
        delete(&router, name).await;
        create(&router, &second).await;

        let read = ok(&router, "GetCustomEntityType", &json!({ "Name": name })).await;
        assert_eq!(
            read.get("RegexString"),
            second.get("RegexString"),
            "case {case}: the re-created pattern must carry the second create's expression, got \
             {read:?}"
        );
        assert_eq!(
            read.get("ContextWords").cloned().unwrap_or(Value::Null),
            second.get("ContextWords").cloned().unwrap_or(Value::Null),
            "case {case}: the re-created pattern must carry the second create's context words, \
             got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A delete for a name this account holds no pattern under removes nothing and is refused
/// with the modelled `EntityNotFoundException` at HTTP 400. The rows differ along how the
/// requested name misses: never created, a prefix of a stored name, a suffix of one, a
/// stored name in another case (the model documents no case fold for a pattern name), and
/// a name whose pattern this very test already deleted.
#[tokio::test]
async fn deleting_an_unknown_pattern_is_refused_and_removes_nothing() {
    let router = emulator();

    create(
        &router,
        &json!({ "Name": "StoredPattern", "RegexString": "[0-9]+" }),
    )
    .await;
    create(
        &router,
        &json!({ "Name": "already-deleted", "RegexString": "[A-Z]+" }),
    )
    .await;
    delete(&router, "already-deleted").await;

    for (case, name) in [
        ("never created", "no-such-pattern"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Pattern"),
        ("a stored name lowercased", "storedpattern"),
        ("a stored name uppercased", "STOREDPATTERN"),
        ("a name whose pattern was already deleted", "already-deleted"),
    ] {
        let (status, body) =
            call(&router, "DeleteCustomEntityType", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body:?}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body:?}"
        );
        // The refusal names the pattern the request addressed and the account whose
        // namespace holds none under that name.
        let expected_message =
            format!("An existing Custom Entity Type: {name} cannot be found for Account Id {ACCOUNT_ID}");
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case}: expected the message {expected_message:?}, got {body:?}"
        );

        // The refused delete removed nothing: the one stored pattern is still there.
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec!["StoredPattern".to_string()],
            "case {case}: a refused delete must remove nothing, got {listed:?}"
        );
    }

    // The name as stored still deletes, so the misses above are misses and not a broken
    // lookup.
    let deleted = delete(&router, "StoredPattern").await;
    assert_eq!(
        deleted.get("Name").and_then(Value::as_str),
        Some("StoredPattern"),
        "case the stored name: expected the delete to report it, got {deleted:?}"
    );
}

/// A deleted pattern is gone from the batched read too, which reports it among the names
/// it could not find — so the delete removed the record from the one namespace every
/// reader resolves against, rather than hiding it from a single reader.
#[tokio::test]
async fn a_deleted_pattern_is_reported_as_not_found_by_the_batched_read() {
    let router = emulator();

    let cases = deletion_cases();
    for (_, request) in &cases {
        create(&router, request).await;
    }
    let names: Vec<String> = cases
        .iter()
        .map(|(_, request)| request["Name"].as_str().unwrap().to_string())
        .collect();

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        delete(&router, name).await;

        let batch = ok(
            &router,
            "BatchGetCustomEntityTypes",
            &json!({ "Names": names }),
        )
        .await;
        let not_found: Vec<String> = batch
            .get("CustomEntityTypesNotFound")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            not_found.iter().any(|missing| missing == name),
            "case {case}: the batched read must report {name:?} as not found, got {not_found:?}"
        );
    }
}
