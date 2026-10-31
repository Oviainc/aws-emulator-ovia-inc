// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDataQualityRuleset`: the delete that removes one data
//! quality ruleset from the per-account namespace `CreateDataQualityRuleset` wrote it
//! into.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The suites are tables: each row is one input along an axis the rule generalises over
//! (whether the ruleset carries a description, a target table, a security configuration,
//! tags or a client token; the form and size of its DQDL text; the form and length of its
//! name; and, for the refusals, the way the requested name misses). The bare
//! name-plus-ruleset create a lifecycle client deletes afterwards is one row among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=DeleteDataQualityRuleset`).
const MAX_NAME_LENGTH: usize = 255;

/// The longest `Ruleset` the model accepts: "len 1..65536" on `DataQualityRulesetString`
/// (`smithy_query operation=CreateDataQualityRuleset`).
const MAX_RULESET_LENGTH: usize = 65_536;

/// Create a ruleset.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateDataQualityRuleset", body).await
}

/// Delete the ruleset called `name`, which must succeed, and return the body.
async fn delete(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "DeleteDataQualityRuleset", &json!({ "Name": name })).await
}

/// The names `ListDataQualityRulesets` reports, in the order the service returned them.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListDataQualityRulesets", &json!({})).await;
    page.get("Rulesets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report a Rulesets list, got {page}"))
        .iter()
        .map(|ruleset| {
            ruleset["Name"]
                .as_str()
                .expect("DataQualityRulesetListDetails.Name is a string")
                .to_string()
        })
        .collect()
}

/// A DQDL ruleset padded out to `length` characters, so a row can sit exactly on the
/// modelled bound. The padding is a comment, so the result is still one ruleset.
fn ruleset_of_length(length: usize) -> String {
    let head = "Rules = [ ColumnExists \"id\" ] # ";
    assert!(
        length > head.len(),
        "a padded ruleset must be longer than its head"
    );
    format!("{head}{}", "x".repeat(length - head.len()))
}

/// The rows every suite here deletes over: one ruleset per axis the delete generalises
/// over, each a distinct record in the one per-account namespace.
fn deletion_cases() -> Vec<(&'static str, Value)> {
    let long_name = "a".repeat(MAX_NAME_LENGTH);
    let long_ruleset = ruleset_of_length(MAX_RULESET_LENGTH);

    vec![
        (
            // The shape a lifecycle client sends: the two required members alone.
            "required members only",
            json!({
                "Name": "orders-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"order_id\" ]",
            }),
        ),
        (
            "a description and a target table",
            json!({
                "Name": "customers-ruleset",
                "Ruleset": "Rules = [ IsComplete \"customer_id\", RowCount > 0 ]",
                "Description": "checks the customer table",
                "TargetTable": { "DatabaseName": "sales", "TableName": "customers" },
            }),
        ),
        (
            "a target table naming its catalog id",
            json!({
                "Name": "catalog-scoped-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"sku\" ]",
                "TargetTable": {
                    "DatabaseName": "inventory",
                    "TableName": "items",
                    "CatalogId": crate::wire::ACCOUNT_ID,
                },
            }),
        ),
        (
            "a multi-line ruleset carrying several rules",
            json!({
                "Name": "multi-line-ruleset",
                "Ruleset": "Rules = [\n    ColumnExists \"a\",\n    Completeness \"a\" > 0.95\n]",
            }),
        ),
        (
            "a ruleset at the modelled length bound",
            json!({ "Name": "longest-ruleset", "Ruleset": long_ruleset }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": long_name, "Ruleset": "Rules = [ ColumnExists \"y\" ]" }),
        ),
        (
            "a name carrying spaces and a tab",
            json!({
                "Name": "ruleset one\ttwo",
                "Ruleset": "Rules = [ ColumnExists \"z\" ]",
            }),
        ),
        (
            "a security configuration and tags",
            json!({
                "Name": "encrypted-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"secret\" ]",
                "DataQualitySecurityConfiguration": "dq-security-config",
                "Tags": { "owner": "data-governance" },
            }),
        ),
        (
            "a client token on the create",
            json!({
                "Name": "tokenised-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"token\" ]",
                "ClientToken": "9f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8",
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The delete removes the addressed ruleset
// ---------------------------------------------------------------------------

/// `DeleteDataQualityRuleset` removes the one record stored under the name it was given,
/// so the ruleset a read resolved before the delete resolves to nothing after it —
/// whatever the deleted ruleset's definition was. The response models no member, so a
/// successful delete answers with an empty body.
#[tokio::test]
async fn deleting_a_ruleset_removes_it_from_the_store() {
    let router = emulator();

    let cases = deletion_cases();
    for (_, request) in &cases {
        create(&router, request).await;
    }

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // The ruleset resolves before the delete, so the miss afterwards is the delete's
        // doing and not a lookup that never worked.
        let read = ok(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: the ruleset must resolve before the delete, got {read}"
        );

        // `DeleteDataQualityRulesetResponse` models no member
        // (`smithy_query operation=DeleteDataQualityRuleset`), so the delete reports
        // nothing but its success.
        let deleted = delete(&router, name).await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case}: the delete models no response member, got {deleted}"
        );

        let (status, body) = call(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a read after the delete must be refused with 400, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException after the delete, got {body}"
        );
        // The read's miss is worded the way every ruleset miss of this service is: the
        // ruleset the request addressed, which is its whole address.
        let expected_message =
            format!("Cannot find Data Quality Ruleset in account {ACCOUNT_ID} with name {name}");
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case}: expected the message {expected_message:?}, got {body}"
        );
    }

    // Every record is gone, and the namespace is the only place they were held.
    let listed = listed_names(&router).await;
    assert!(
        listed.is_empty(),
        "case every ruleset deleted: expected an empty listing, got {listed:?}"
    );
}

/// A delete removes ONLY the ruleset it names: every other stored ruleset keeps its own
/// DQDL text, and the listing still reports it.
#[tokio::test]
async fn a_delete_leaves_every_other_ruleset_alone() {
    let router = emulator();

    let cases = deletion_cases();
    for (_, request) in &cases {
        create(&router, request).await;
    }

    let mut remaining: Vec<String> = cases
        .iter()
        .map(|(_, request)| {
            request["Name"]
                .as_str()
                .expect("the case carries a Name")
                .to_string()
        })
        .collect();

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        delete(&router, name).await;
        remaining.retain(|stored| stored != name);

        // What is left is exactly the rulesets not yet deleted, each still carrying the
        // DQDL its own create stored.
        let mut listed = listed_names(&router).await;
        listed.sort();
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: the listing must report only the rulesets not yet deleted, got \
             {listed:?}"
        );

        for (other_case, other) in &cases {
            let other_name = other["Name"].as_str().expect("the case carries a Name");
            if !remaining.iter().any(|stored| stored == other_name) {
                continue;
            }
            let read = ok(
                &router,
                "GetDataQualityRuleset",
                &json!({ "Name": other_name }),
            )
            .await;
            assert_eq!(
                read.get("Ruleset"),
                other.get("Ruleset"),
                "case {case} then {other_case}: a surviving ruleset keeps its own Ruleset text, \
                 got {read}"
            );
        }
    }
}

/// A deleted name is free again: a create under it succeeds and stores the NEW
/// definition, so the delete removed the record rather than reserving the name — and the
/// re-created ruleset's instants are its own, not the deleted record's.
#[tokio::test]
async fn a_deleted_ruleset_name_can_be_created_again() {
    let router = emulator();

    for (case, first, second) in [
        (
            "a different DQDL text",
            json!({ "Name": "recreated-one", "Ruleset": "Rules = [ ColumnExists \"first\" ]" }),
            json!({ "Name": "recreated-one", "Ruleset": "Rules = [ ColumnExists \"second\" ]" }),
        ),
        (
            "a description added on the second create",
            json!({ "Name": "recreated-two", "Ruleset": "Rules = [ RowCount > 0 ]" }),
            json!({
                "Name": "recreated-two",
                "Ruleset": "Rules = [ RowCount > 0 ]",
                "Description": "added by the second create",
            }),
        ),
        (
            "a target table added on the second create",
            json!({ "Name": "recreated-three", "Ruleset": "Rules = [ RowCount > 1 ]" }),
            json!({
                "Name": "recreated-three",
                "Ruleset": "Rules = [ RowCount > 1 ]",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
            }),
        ),
        (
            "the same client token as the deleted record",
            json!({
                "Name": "recreated-four",
                "Ruleset": "Rules = [ RowCount > 2 ]",
                "ClientToken": "a-reused-token",
            }),
            json!({
                "Name": "recreated-four",
                "Ruleset": "Rules = [ RowCount > 3 ]",
                "ClientToken": "a-reused-token",
            }),
        ),
    ] {
        let name = first["Name"].as_str().expect("the case carries a Name");
        create(&router, &first).await;
        delete(&router, name).await;
        create(&router, &second).await;

        let read = ok(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            read.get("Ruleset"),
            second.get("Ruleset"),
            "case {case}: the re-created ruleset must carry the second create's DQDL, got {read}"
        );
        assert_eq!(
            read.get("Description").cloned().unwrap_or(Value::Null),
            second.get("Description").cloned().unwrap_or(Value::Null),
            "case {case}: the re-created ruleset must carry the second create's Description, got \
             {read}"
        );
        assert_eq!(
            read.get("TargetTable").cloned().unwrap_or(Value::Null),
            second.get("TargetTable").cloned().unwrap_or(Value::Null),
            "case {case}: the re-created ruleset must carry the second create's TargetTable, got \
             {read}"
        );
    }
}

/// The delete removes the record from the one namespace every operation that addresses a
/// ruleset by name resolves against, rather than hiding it from a single reader: after a
/// delete the update of that name misses too, in the same wording.
#[tokio::test]
async fn an_update_after_the_delete_misses() {
    let router = emulator();

    for (case, request) in deletion_cases() {
        let name = request["Name"].as_str().expect("the case carries a Name");
        create(&router, &request).await;
        delete(&router, name).await;

        let (status, body) = call(
            &router,
            "UpdateDataQualityRuleset",
            &json!({ "Name": name, "Description": "an update after the delete" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an update after the delete must be refused with 400, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for the update, got {body}"
        );
        let expected_message =
            format!("Cannot find Data Quality Ruleset in account {ACCOUNT_ID} with name {name}");
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case}: expected the message {expected_message:?}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A delete for a name this account holds no ruleset under removes nothing and is refused
/// with the modelled `EntityNotFoundException` at HTTP 400. The rows differ along how the
/// requested name misses: never created, a prefix of a stored name, a suffix of one, a
/// stored name in another case (the model documents no case fold for a ruleset name), and
/// a name whose ruleset this very test already deleted.
#[tokio::test]
async fn deleting_an_unknown_ruleset_is_refused_and_removes_nothing() {
    let router = emulator();

    create(
        &router,
        &json!({ "Name": "StoredRuleset", "Ruleset": "Rules = [ ColumnExists \"a\" ]" }),
    )
    .await;
    create(
        &router,
        &json!({ "Name": "already-deleted", "Ruleset": "Rules = [ RowCount > 0 ]" }),
    )
    .await;
    delete(&router, "already-deleted").await;

    for (case, name) in [
        ("never created", "no-such-ruleset"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Ruleset"),
        ("a stored name lowercased", "storedruleset"),
        ("a stored name uppercased", "STOREDRULESET"),
        ("a name whose ruleset was already deleted", "already-deleted"),
    ] {
        let (status, body) =
            call(&router, "DeleteDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
        // The refusal names the ruleset the request addressed, which is its whole address.
        let expected_message =
            format!("Cannot find Data Quality Ruleset in account {ACCOUNT_ID} with name {name}");
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case}: expected the message {expected_message:?}, got {body}"
        );

        // The refused delete removed nothing: the one stored ruleset is still there.
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec!["StoredRuleset".to_string()],
            "case {case}: a refused delete must remove nothing, got {listed:?}"
        );
    }

    // The name as stored still deletes, so the misses above are misses and not a broken
    // lookup.
    delete(&router, "StoredRuleset").await;
    let listed = listed_names(&router).await;
    assert!(
        listed.is_empty(),
        "case the stored name: expected the delete to remove it, got {listed:?}"
    );
}
