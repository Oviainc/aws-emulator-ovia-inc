// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue data quality ruleset slice: `CreateDataQualityRuleset`
//! and the `GetDataQualityRuleset` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip suite is a TABLE: each row is one input along the axis the rule
//! generalises over (whether the ruleset carries a description, a target table, a catalog
//! id on that table, a security configuration or tags; the form and size of the DQDL text;
//! the form and length of the name). The bare name-plus-ruleset create a lifecycle client
//! sends is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateDataQualityRulesetInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DataQualityRulesetEntry, MAX_DATA_QUALITY_RULESETS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=CreateDataQualityRuleset`).
const MAX_NAME_LENGTH: usize = 255;

/// The longest `Ruleset` the model accepts: "len 1..65536" on `DataQualityRulesetString`
/// (`smithy_query operation=CreateDataQualityRuleset`). A row at this bound proves the
/// whole DQDL text round-trips rather than only its head.
const MAX_RULESET_LENGTH: usize = 65_536;

/// The longest `Description` the model accepts: "len 0..2048" on `DescriptionString`
/// (`smithy_query operation=CreateDataQualityRuleset`).
const MAX_DESCRIPTION_LENGTH: usize = 2_048;

/// Create a ruleset and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateDataQualityRuleset", body).await
}

/// The `GetDataQualityRuleset` response for `name`.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetDataQualityRuleset", &json!({ "Name": name })).await
}

/// A DQDL ruleset whose text is padded out to `length` bytes, so a row can sit exactly on
/// the modelled bound. The padding is a comment, so the result is still one ruleset rather
/// than a string of the right size.
fn ruleset_of_length(length: usize) -> String {
    let head = "Rules = [ ColumnExists \"id\" ] # ";
    assert!(
        length > head.len(),
        "a padded ruleset must be longer than its head"
    );
    format!("{head}{}", "x".repeat(length - head.len()))
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateDataQualityRuleset` persists is what `GetDataQualityRuleset` reports: the
/// create answers with the name it stored, and every member of the ruleset the request
/// carried comes back from the store unchanged.
#[tokio::test]
async fn created_data_quality_rulesets_are_read_back_from_the_store() {
    let router = emulator();

    let long_name = "a".repeat(MAX_NAME_LENGTH);
    let long_ruleset = ruleset_of_length(MAX_RULESET_LENGTH);
    let long_description = "d".repeat(MAX_DESCRIPTION_LENGTH);

    let cases: Vec<(&str, Value)> = vec![
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
                "TargetTable": {
                    "DatabaseName": "sales",
                    "TableName": "customers",
                },
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
                "Ruleset": "Rules = [\n    ColumnExists \"a\",\n    Completeness \"a\" > 0.95,\n    Uniqueness \"a\" > 0.9\n]",
                "Description": "several rules over one column",
            }),
        ),
        (
            "a ruleset at the modelled length bound",
            json!({
                "Name": "longest-ruleset",
                "Ruleset": long_ruleset,
            }),
        ),
        (
            "a description at the modelled length bound",
            json!({
                "Name": "longest-description",
                "Ruleset": "Rules = [ ColumnExists \"x\" ]",
                "Description": long_description,
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({
                "Name": long_name,
                "Ruleset": "Rules = [ ColumnExists \"y\" ]",
            }),
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
                "Tags": { "owner": "data-governance", "tier": "gold" },
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
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // "A unique name for the data quality ruleset": the create reports the name it
        // stored, which is what a later read addresses the ruleset by.
        let created = create(&router, request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateDataQualityRuleset must report the stored name, got {created}"
        );

        let read = get(&router, name).await;
        let expected = json!({
            "Name": name,
            "Ruleset": request["Ruleset"],
            "Description": request.get("Description").cloned().unwrap_or(Value::Null),
            "TargetTable": request.get("TargetTable").cloned().unwrap_or(Value::Null),
            "DataQualitySecurityConfiguration": request
                .get("DataQualitySecurityConfiguration")
                .cloned()
                .unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &read);

        // A ruleset a client authored carries no recommendation run to link it to: the run
        // id is "generated" only "when a ruleset was created from a recommendation run".
        assert!(
            read.get("RecommendationRunId").is_none_or(Value::is_null),
            "case {case}: a directly created ruleset reports no RecommendationRunId, got {read}"
        );
        // `GetDataQualityRulesetResponse` models no `Tags` or `ClientToken` member, so
        // neither is part of the ruleset a read reports.
        for member in ["Tags", "ClientToken"] {
            assert!(
                read.get(member).is_none(),
                "case {case}: GetDataQualityRuleset must report no {member}, got {read}"
            );
        }
    }

    // Each ruleset is its own record: reading one back after every create has landed still
    // reports that ruleset's own DQDL, so one create never overwrote another.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_eq!(
            read.get("Ruleset"),
            request.get("Ruleset"),
            "case {case}: each ruleset keeps its own Ruleset text, got {read}"
        );
    }
}

/// A ruleset nothing has updated reports the instant it was created as both of its
/// timestamps, and both are stable across reads: a read is a report, not a write.
#[tokio::test]
async fn a_created_ruleset_reports_stable_create_instants() {
    let router = emulator();

    for (case, request) in [
        (
            "required members only",
            json!({ "Name": "instants-one", "Ruleset": "Rules = [ ColumnExists \"a\" ]" }),
        ),
        (
            "with a target table",
            json!({
                "Name": "instants-two",
                "Ruleset": "Rules = [ ColumnExists \"b\" ]",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
            }),
        ),
    ] {
        create(&router, &request).await;
        let name = request["Name"].as_str().expect("the case carries a Name");

        let first = get(&router, name).await;
        let created_on = first["CreatedOn"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: CreatedOn must be a timestamp, got {first}")
        });
        assert!(
            created_on > 0.0,
            "case {case}: expected a wall-clock CreatedOn, got {created_on}"
        );
        assert_eq!(
            first["LastModifiedOn"].as_f64(),
            Some(created_on),
            "case {case}: a ruleset nothing updated was last modified when it was created, \
             got {first}"
        );

        let second = get(&router, name).await;
        assert_eq!(
            second.get("CreatedOn"),
            first.get("CreatedOn"),
            "case {case}: two reads of one ruleset report the same CreatedOn, got {first} then \
             {second}"
        );
        assert_eq!(
            second.get("LastModifiedOn"),
            first.get("LastModifiedOn"),
            "case {case}: a read must not move LastModifiedOn, got {first} then {second}"
        );
    }
}

/// A create carrying the `ClientToken` the stored ruleset was created with AND the
/// definition it was created from is the SAME create arriving twice: it answers with the
/// stored name, stores nothing new and leaves the stored record — including its instants —
/// exactly as the first create wrote it. The rows differ along which members the replayed
/// definition carries.
#[tokio::test]
async fn a_retried_create_with_the_same_client_token_is_idempotent() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            "the two required members and a token",
            json!({
                "Name": "retry-required-only",
                "Ruleset": "Rules = [ ColumnExists \"a\" ]",
                "ClientToken": "token-required-only",
            }),
        ),
        (
            "a description and a target table replayed too",
            json!({
                "Name": "retry-full",
                "Ruleset": "Rules = [ IsComplete \"b\", RowCount > 0 ]",
                "Description": "replayed in full",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
                "ClientToken": "token-full",
            }),
        ),
        (
            "a security configuration and tags replayed too",
            json!({
                "Name": "retry-tagged",
                "Ruleset": "Rules = [ ColumnExists \"c\" ]",
                "DataQualitySecurityConfiguration": "dq-security-config",
                "Tags": { "owner": "data-governance" },
                "ClientToken": "token-tagged",
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        create(&router, request).await;
        let before = get(&router, name).await;

        let answered = create(&router, request).await;
        assert_eq!(
            answered.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: a retry must answer the stored name, got {answered}"
        );

        let after = get(&router, name).await;
        assert_eq!(
            after, before,
            "case {case}: a retry must store nothing; expected {before}, got {after}"
        );
    }
}

/// A create that reuses a stored ruleset's `ClientToken` while asking for a DIFFERENT
/// definition is not a retry of that create: it is refused with the modelled
/// `AlreadyExistsException` rather than having the definition it sent dropped behind a 200.
/// The rows differ along which member of the definition the second request changed.
#[tokio::test]
async fn reusing_a_client_token_with_a_different_definition_is_refused() {
    let router = emulator();

    let first = json!({
        "Name": "token-mismatch",
        "Ruleset": "Rules = [ ColumnExists \"first\" ]",
        "Description": "the stored description",
        "TargetTable": { "DatabaseName": "db", "TableName": "t" },
        "DataQualitySecurityConfiguration": "dq-security-config",
        "Tags": { "owner": "data-governance" },
        "ClientToken": "token-mismatch",
    });

    let cases: Vec<(&str, Value)> = vec![
        ("another Ruleset", json!("Rules = [ ColumnExists \"second\" ]")),
        ("another Description", json!("a different description")),
        ("another TargetTable", json!({ "DatabaseName": "db", "TableName": "other" })),
        ("another DataQualitySecurityConfiguration", json!("another-config")),
        ("another Tags map", json!({ "owner": "someone-else" })),
    ];
    // Which member each row changes, in the same order as the rows above.
    let members = [
        "Ruleset",
        "Description",
        "TargetTable",
        "DataQualitySecurityConfiguration",
        "Tags",
    ];

    create(&router, &first).await;
    let before = get(&router, "token-mismatch").await;

    for ((case, replacement), member) in cases.iter().zip(members) {
        let mut second = first.clone();
        second[member] = replacement.clone();

        let (status, body) = call(&router, "CreateDataQualityRuleset", &second).await;
        assert_eq!(
            status, 400,
            "case {case}: a reused token asking for another definition must be refused with \
             400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Another ruleset with the same name already exists: token-mismatch",
            "case {case}: expected the one duplicate wording this service gives, got {body}"
        );

        let after = get(&router, "token-mismatch").await;
        assert_eq!(
            after, before,
            "case {case}: a refused create must change nothing; expected {before}, got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A ruleset name is unique per account: a second create under a name already taken is
/// refused with the modelled `AlreadyExistsException`, and the stored ruleset is left as
/// the first create wrote it. The rows differ along how the second create differs from the
/// first (its DQDL, its description, its target table, its client token, or not at all).
#[tokio::test]
async fn a_duplicate_ruleset_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same name, different DQDL",
            json!({ "Name": "duplicate-one", "Ruleset": "Rules = [ ColumnExists \"first\" ]" }),
            json!({ "Name": "duplicate-one", "Ruleset": "Rules = [ ColumnExists \"second\" ]" }),
        ),
        (
            "same name and the identical definition",
            json!({ "Name": "duplicate-two", "Ruleset": "Rules = [ RowCount > 0 ]" }),
            json!({ "Name": "duplicate-two", "Ruleset": "Rules = [ RowCount > 0 ]" }),
        ),
        (
            "same name, a description added",
            json!({ "Name": "duplicate-three", "Ruleset": "Rules = [ RowCount > 1 ]" }),
            json!({
                "Name": "duplicate-three",
                "Ruleset": "Rules = [ RowCount > 1 ]",
                "Description": "added by the second create",
            }),
        ),
        (
            "same name, a target table added",
            json!({ "Name": "duplicate-four", "Ruleset": "Rules = [ RowCount > 2 ]" }),
            json!({
                "Name": "duplicate-four",
                "Ruleset": "Rules = [ RowCount > 2 ]",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
            }),
        ),
        (
            "same name, a client token the first create did not carry",
            json!({ "Name": "duplicate-five", "Ruleset": "Rules = [ RowCount > 3 ]" }),
            json!({
                "Name": "duplicate-five",
                "Ruleset": "Rules = [ RowCount > 4 ]",
                "ClientToken": "a-token-the-stored-ruleset-does-not-carry",
            }),
        ),
        (
            "same name, another client token",
            json!({
                "Name": "duplicate-six",
                "Ruleset": "Rules = [ RowCount > 5 ]",
                "ClientToken": "first-token",
            }),
            json!({
                "Name": "duplicate-six",
                "Ruleset": "Rules = [ RowCount > 6 ]",
                "ClientToken": "another-token",
            }),
        ),
    ];

    for (case, first, second) in &cases {
        create(&router, first).await;
        let name = first["Name"].as_str().expect("the case carries a Name");
        let before = get(&router, name).await;

        let (status, body) = call(&router, "CreateDataQualityRuleset", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
        // One wording for this collision, whichever member the second create differs by,
        // and it names the ruleset the create asked for.
        assert_eq!(
            error_message(&body),
            format!("Another ruleset with the same name already exists: {name}"),
            "case {case}: expected the one duplicate wording this service gives, got {body}"
        );

        // The stored ruleset is still the FIRST create's, member for member and instant
        // for instant.
        let after = get(&router, name).await;
        assert_eq!(
            after, before,
            "case {case}: a refused create must change nothing; expected {before}, got {after}"
        );
    }
}

/// A read for a name this account holds no ruleset under is refused with the modelled
/// `EntityNotFoundException`, in the one wording this service gives that miss. The rows
/// differ along how the requested name misses: never created, a prefix of a stored name, a
/// suffix of one, and a stored name in another case — the model documents no case fold for
/// a ruleset name, so a differently-cased name is a different ruleset.
#[tokio::test]
async fn reading_an_unknown_ruleset_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({ "Name": "StoredRuleset", "Ruleset": "Rules = [ ColumnExists \"a\" ]" }),
    )
    .await;

    for (case, name) in [
        ("never created", "no-such-ruleset"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Ruleset"),
        ("a stored name lowercased", "storedruleset"),
        ("a stored name uppercased", "STOREDRULESET"),
    ] {
        let (status, body) = call(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
        // The refusal names the ruleset the request addressed, which is its whole address.
        let message = error_message(&body);
        assert!(
            message.contains(name),
            "case {case}: the refusal must name {name:?}, got {message:?}"
        );
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "StoredRuleset").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("StoredRuleset"),
        "case the stored name: expected the stored ruleset, got {read}"
    );
}

/// The store holds a bounded number of rulesets: the create at the ceiling succeeds, the
/// one past it is refused with the modelled limit error, and freeing a slot lets a create
/// succeed again — so the ceiling bounds what is stored rather than latching on how many
/// creates were ever made.
#[tokio::test]
async fn a_create_past_the_data_quality_ruleset_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let now = std::time::SystemTime::now();
        for index in 0..MAX_DATA_QUALITY_RULESETS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateDataQualityRulesetInput {
                name: name.clone(),
                ruleset: "Rules = [ RowCount > 0 ]".to_string(),
                ..CreateDataQualityRulesetInput::default()
            };
            store.data_quality_rulesets.insert(
                name,
                DataQualityRulesetEntry::from_create_input(&input, now),
            );
        }
    }

    let last = json!({ "Name": "last-ruleset", "Ruleset": "Rules = [ RowCount > 0 ]" });
    let past = json!({ "Name": "one-too-many", "Ruleset": "Rules = [ RowCount > 0 ]" });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateDataQualityRuleset", &past).await;
    assert_eq!(
        status, 400,
        "case ruleset {} of {MAX_DATA_QUALITY_RULESETS}: expected 400, got {status} with {body}",
        MAX_DATA_QUALITY_RULESETS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one ruleset too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().data_quality_rulesets.remove("seeded-0");
    }
    create(&router, &past).await;
}
