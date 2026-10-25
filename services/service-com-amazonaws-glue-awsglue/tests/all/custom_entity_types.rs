// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue custom entity type slice: `CreateCustomEntityType` and
//! the `GetCustomEntityType` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip suite is a table: each row is one input along the axis the rule
//! generalises over (how many context words the pattern carries, the form of the regular
//! expression, the form and length of the name, whether tags were sent), and a bare
//! name-plus-expression pattern — the shape a lifecycle client sends — is one row among
//! them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCustomEntityTypeInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, CustomEntityTypeEntry, MAX_CUSTOM_ENTITY_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, ok};

/// The largest `ContextWords` list the model accepts: "len 1..20" on
/// `CreateCustomEntityTypeRequest$ContextWords` (`smithy_query operation=
/// CreateCustomEntityType`). A row at this bound proves the whole list round-trips, in
/// order, rather than only its head.
const MAX_CONTEXT_WORDS: usize = 20;

/// The longest `Name`/`RegexString` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=CreateCustomEntityType`).
const MAX_NAME_LENGTH: usize = 255;

/// Create a custom pattern and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateCustomEntityType", body).await
}

/// The `GetCustomEntityType` response for `name`.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetCustomEntityType", &json!({ "Name": name })).await
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateCustomEntityType` persists is what `GetCustomEntityType` reports: the
/// create answers with the name it stored, and every member of the pattern the request
/// carried comes back from the store unchanged.
///
/// The rows differ along the number of context words (none, one, the twenty the model
/// allows), the form of the regular expression, the form and length of the name, and
/// whether the create carried tags.
#[tokio::test]
async fn created_custom_entity_types_are_read_back_from_the_store() {
    let router = emulator();

    let long_name = "a".repeat(MAX_NAME_LENGTH);
    let context_words: Vec<String> = (0..MAX_CONTEXT_WORDS)
        .map(|index| format!("context-word-{index}"))
        .collect();

    let cases: Vec<(&str, Value)> = vec![
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
            "a regular expression carrying escapes, classes and a tab",
            json!({
                "Name": "escaped-expression",
                "RegexString": "^\\s*(?:[A-Z]{2}\\d{6})\t+$",
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({
                "Name": long_name,
                "RegexString": "x",
            }),
        ),
        (
            "a name carrying spaces and a tab",
            json!({
                "Name": "pattern one\ttwo",
                "RegexString": "[A-Za-z ]+",
                "ContextWords": ["two words", "three\twords"],
            }),
        ),
        (
            "tags sent with the create",
            json!({
                "Name": "tagged-pattern",
                "RegexString": "TAG-[0-9]{4}",
                "ContextWords": ["tagged"],
                "Tags": { "owner": "data-governance", "tier": "gold" },
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // "The name of the custom pattern you created": the create reports the name it
        // stored, which is what a later read addresses the pattern by.
        let created = create(&router, request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateCustomEntityType must report the stored name, got {created}"
        );

        let read = get(&router, name).await;
        // Every member of the pattern the request carried reappears unchanged.
        let expected = json!({
            "Name": name,
            "RegexString": request["RegexString"],
            "ContextWords": request.get("ContextWords").cloned().unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &read);

        // `GetCustomEntityTypeResponse` models no `Tags` member, so the tags a create
        // carried are not part of the pattern a read reports.
        assert!(
            read.get("Tags").is_none(),
            "case {case}: GetCustomEntityType must report no Tags, got {read}"
        );
    }

    // Each pattern is its own record: reading one back after every create has landed
    // still reports that pattern's own expression, so one create never overwrote another.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_eq!(
            read.get("RegexString"),
            request.get("RegexString"),
            "case {case}: each pattern keeps its own RegexString, got {read}"
        );
    }
}

/// A create that named no context words reports NO context words, not an empty list:
/// "If no context words are passed only a regular expression is checked", so the absence
/// is part of the pattern's meaning.
#[tokio::test]
async fn a_pattern_created_without_context_words_reports_none() {
    let router = emulator();

    for (case, request) in [
        (
            "no ContextWords member",
            json!({ "Name": "bare", "RegexString": "[0-9]+" }),
        ),
        (
            "no ContextWords member, a longer expression",
            json!({ "Name": "bare-too", "RegexString": "^[A-Z]{2}-[0-9]{6}$" }),
        ),
    ] {
        create(&router, &request).await;
        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert!(
            read.get("ContextWords").is_none_or(Value::is_null),
            "case {case}: expected no ContextWords, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A pattern name is unique per account: the second create under a name already taken is
/// refused with the modelled `AlreadyExistsException`, and the stored pattern is left as
/// the first create wrote it.
#[tokio::test]
async fn a_duplicate_pattern_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same name, different expression",
            json!({ "Name": "duplicate-one", "RegexString": "FIRST-[0-9]+" }),
            json!({ "Name": "duplicate-one", "RegexString": "SECOND-[0-9]+" }),
        ),
        (
            "same name, context words added",
            json!({ "Name": "duplicate-two", "RegexString": "KEEP-[0-9]+" }),
            json!({
                "Name": "duplicate-two",
                "RegexString": "KEEP-[0-9]+",
                "ContextWords": ["added"],
            }),
        ),
        (
            "same name and the identical definition",
            json!({
                "Name": "duplicate-three",
                "RegexString": "SAME-[0-9]+",
                "ContextWords": ["same"],
            }),
            json!({
                "Name": "duplicate-three",
                "RegexString": "SAME-[0-9]+",
                "ContextWords": ["same"],
            }),
        ),
    ];

    for (case, first, second) in &cases {
        create(&router, first).await;

        let (status, body) = call(&router, "CreateCustomEntityType", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );

        // The stored pattern is still the FIRST create's, member for member: the
        // context words the refused create carried are not there unless the first one
        // sent them.
        let name = first["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        let expected = json!({
            "Name": name,
            "RegexString": first["RegexString"],
            "ContextWords": first.get("ContextWords").cloned().unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &read);
    }
}

/// A read for a name this account holds no pattern under is refused with the modelled
/// `EntityNotFoundException`. The rows differ along how the requested name misses: never
/// created, a prefix of a stored name, a suffix of one, and a stored name in another
/// case — the model documents no case fold for a pattern name, so a differently-cased
/// name is a different pattern.
#[tokio::test]
async fn reading_an_unknown_pattern_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({ "Name": "StoredPattern", "RegexString": "[0-9]+" }),
    )
    .await;

    for (case, name) in [
        ("never created", "no-such-pattern"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Pattern"),
        ("a stored name lowercased", "storedpattern"),
        ("a stored name uppercased", "STOREDPATTERN"),
    ] {
        let (status, body) = call(&router, "GetCustomEntityType", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "StoredPattern").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("StoredPattern"),
        "case the stored name: expected the stored pattern, got {read}"
    );
}

/// The store holds a bounded number of custom patterns: the create at the ceiling
/// succeeds, the one past it is refused with the modelled limit error, and freeing a slot
/// lets a create succeed again — so the ceiling bounds what is stored rather than
/// latching on how many creates were ever made.
#[tokio::test]
async fn a_create_past_the_custom_entity_type_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_CUSTOM_ENTITY_TYPES - 1 {
            let name = format!("seeded-{index}");
            let input = CreateCustomEntityTypeInput {
                name: name.clone(),
                regex_string: "[0-9]+".to_string(),
                ..CreateCustomEntityTypeInput::default()
            };
            store
                .custom_entity_types
                .insert(name, CustomEntityTypeEntry::from_create_input(&input));
        }
    }

    let last = json!({ "Name": "last-pattern", "RegexString": "[0-9]+" });
    let past = json!({ "Name": "one-too-many", "RegexString": "[0-9]+" });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateCustomEntityType", &past).await;
    assert_eq!(
        status, 400,
        "case pattern {} of {MAX_CUSTOM_ENTITY_TYPES}: expected 400, got {status} with {body}",
        MAX_CUSTOM_ENTITY_TYPES + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one pattern too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().custom_entity_types.remove("seeded-0");
    }
    create(&router, &past).await;
}
