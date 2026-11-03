// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue classifier slice: `CreateClassifier`, the
//! `GetClassifier` and `GetClassifiers` that read back what it stored, the
//! `UpdateClassifier` that modifies it and the `DeleteClassifier` that removes it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The suites are tables: a row is one input along the axis the rule generalises over —
//! which of the four classifier kinds the request names, which optional members that kind
//! carries, the form and length of the name, how many classifiers a page spans, and how a
//! requested name misses.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ClassifierEntry, ComAmazonawsGlueAwsglueState, FIRST_CLASSIFIER_VERSION, MAX_CLASSIFIERS,
};
use service_com_amazonaws_glue_awsglue::ops::types::CreateClassifierInput;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    assert_echoed, call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID,
};

/// The longest `Name` the model accepts for a classifier: "len 1..255" on `NameString`
/// (`smithy_query operation=GetClassifier`).
const MAX_NAME_LENGTH: usize = 255;

/// The four members of `CreateClassifierRequest`, one per classifier kind
/// (`smithy_query operation=CreateClassifier`). A request names its kind by carrying one
/// of them, and a read reports the classifier under the matching member of `Classifier`
/// (`smithy_query shape=Classifier`).
const CLASSIFIER_KINDS: [&str; 4] = [
    "GrokClassifier",
    "XMLClassifier",
    "JsonClassifier",
    "CsvClassifier",
];

/// Create a classifier and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateClassifier", body).await
}

/// The `GetClassifier` response for `name`.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetClassifier", &json!({ "Name": name })).await
}

/// The classifier a read reported, under the member for `kind`.
fn reported(read: &Value, kind: &str) -> Value {
    read["Classifier"][kind].clone()
}

/// The `Name` the one kind a request body names carries.
fn requested_name(request: &Value) -> String {
    let (_, definition) = request
        .as_object()
        .expect("a request body is an object")
        .iter()
        .next()
        .expect("a request body names one kind");
    definition["Name"]
        .as_str()
        .expect("the named kind carries a Name")
        .to_string()
}

/// Every classifier kind, with the optional members it carries, as a create body. The
/// rows span the four kinds and, within each, whether the optional members are present:
/// absent and present are different classifiers, and a read has to report each as stored.
fn create_cases() -> Vec<(&'static str, &'static str, Value)> {
    let long_name = "a".repeat(MAX_NAME_LENGTH);
    vec![
        (
            "grok, required members only",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "grok-bare",
                    "Classification": "syslog",
                    "GrokPattern": "%{TIMESTAMP_ISO8601:timestamp} %{GREEDYDATA:message}",
                },
            }),
        ),
        (
            "grok with custom patterns",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "grok-custom",
                    "Classification": "omniture",
                    "GrokPattern": "%{MY_ID:id}",
                    "CustomPatterns": "MY_ID [0-9A-F]{8}\nMY_WORD \\w+",
                },
            }),
        ),
        (
            "xml, required members only",
            "XMLClassifier",
            json!({
                "XMLClassifier": {
                    "Name": "xml-bare",
                    "Classification": "xml",
                },
            }),
        ),
        (
            "xml with a row tag",
            "XMLClassifier",
            json!({
                "XMLClassifier": {
                    "Name": "xml-rows",
                    "Classification": "catalog-export",
                    "RowTag": "record",
                },
            }),
        ),
        (
            "json",
            "JsonClassifier",
            json!({
                "JsonClassifier": {
                    "Name": "json-records",
                    "JsonPath": "$.records[*]",
                },
            }),
        ),
        (
            "csv, name only",
            "CsvClassifier",
            json!({ "CsvClassifier": { "Name": "csv-bare" } }),
        ),
        (
            // `ContainsHeader` is a `CsvHeaderOption` (`UNKNOWN`, `PRESENT`, `ABSENT`) and
            // `Serde` a `CsvSerdeOption` (`OpenCSVSerDe`, `LazySimpleSerDe`, `None`) —
            // the published value lists of `smithy_query shape=CsvHeaderOption,
            // CsvSerdeOption`.
            "csv with every member",
            "CsvClassifier",
            json!({
                "CsvClassifier": {
                    "Name": "csv-full",
                    "Delimiter": "|",
                    "QuoteSymbol": "'",
                    "ContainsHeader": "PRESENT",
                    "Header": ["id", "region", "amount"],
                    "DisableValueTrimming": true,
                    "AllowSingleColumn": false,
                    "CustomDatatypeConfigured": true,
                    "CustomDatatypes": ["BINARY", "DECIMAL", "TIMESTAMP"],
                    "Serde": "LazySimpleSerDe",
                },
            }),
        ),
        (
            "csv naming the other header option and serde",
            "CsvClassifier",
            json!({
                "CsvClassifier": {
                    "Name": "csv-other-values",
                    "Delimiter": "\t",
                    "ContainsHeader": "ABSENT",
                    "Serde": "None",
                },
            }),
        ),
        (
            "a name at the modelled length bound",
            "JsonClassifier",
            json!({
                "JsonClassifier": { "Name": long_name, "JsonPath": "$" },
            }),
        ),
        (
            "a name carrying a space and a tab",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "classifier one\ttwo",
                    "Classification": "logs",
                    "GrokPattern": "%{GREEDYDATA:line}",
                },
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateClassifier` persists is what `GetClassifier` reports: every member of the
/// kind the request named comes back from the store unchanged, under that kind's member of
/// `Classifier`, and the three server-set members are the service's own — the first
/// version, and one creation instant that is also the last-updated instant until a write
/// lands.
#[tokio::test]
async fn created_classifiers_are_read_back_from_the_store() {
    let router = emulator();
    let cases = create_cases();

    for (case, kind, request) in &cases {
        let name = requested_name(request);

        // `CreateClassifierResponse` declares no member, so the create's own answer is
        // the 200 alone.
        let created = create(&router, request).await;
        assert!(
            created.as_object().is_none_or(|members| members.is_empty()),
            "case {case}: CreateClassifier must answer with no member, got {created}"
        );

        let read = get(&router, &name).await;
        let reported = reported(&read, kind);
        assert_echoed(case, &request[kind], &reported);

        // The kind is which field the create named, so no other kind's member is filled:
        // that is how a client tells a grok classifier from a CSV one.
        for other in CLASSIFIER_KINDS.iter().filter(|other| *other != kind) {
            assert!(
                read["Classifier"].get(other).is_none_or(Value::is_null),
                "case {case}: only {kind} must be reported, got {read}"
            );
        }

        assert_eq!(
            reported.get("Version").and_then(Value::as_i64),
            Some(FIRST_CLASSIFIER_VERSION),
            "case {case}: a fresh classifier is version {FIRST_CLASSIFIER_VERSION}, got \
             {reported}"
        );
        let creation_time = reported
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime, got {reported}"));
        let last_updated = reported
            .get("LastUpdated")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a LastUpdated, got {reported}"));
        assert_eq!(
            creation_time, last_updated,
            "case {case}: no write has landed, so LastUpdated is the creation instant, got \
             {reported}"
        );
    }

    // Each classifier is its own record: reading one back after every create has landed
    // still reports that classifier's own definition, so no create overwrote another.
    for (case, kind, request) in &cases {
        let name = requested_name(request);
        let read = get(&router, &name).await;
        assert_echoed(case, &request[kind], &reported(&read, kind));
    }
}

/// Two reads of one classifier agree, and the stored timestamps do not move because
/// something read them: a read reports what the create wrote.
#[tokio::test]
async fn reading_a_classifier_twice_reports_the_same_record() {
    let router = emulator();
    for (case, kind, request) in create_cases() {
        create(&router, &request).await;
        let name = requested_name(&request);
        let first = reported(&get(&router, &name).await, kind);
        let second = reported(&get(&router, &name).await, kind);
        assert_eq!(
            first, second,
            "case {case}: two reads must report one record, got {first} then {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// `UpdateClassifier` replaces the members it carries and preserves the ones it omits, and
/// the write moves the server-set members: the version advances by one and the
/// last-updated instant moves off the creation instant.
///
/// The rows differ along the kind, which members the update carries (one, several, the
/// name alone) and whether the member being written was set at create.
#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_preserves_the_rest() {
    let router = emulator();

    let cases: Vec<(&str, &str, Value, Value, Value)> = vec![
        (
            "grok, one member rewritten",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "grok-update-one",
                    "Classification": "syslog",
                    "GrokPattern": "%{GREEDYDATA:line}",
                    "CustomPatterns": "KEEP \\w+",
                },
            }),
            json!({ "GrokClassifier": { "Name": "grok-update-one", "GrokPattern": "%{WORD:w}" } }),
            json!({
                "Name": "grok-update-one",
                "Classification": "syslog",
                "GrokPattern": "%{WORD:w}",
                "CustomPatterns": "KEEP \\w+",
            }),
        ),
        (
            "grok, a member the create left absent is added",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "grok-update-add",
                    "Classification": "logs",
                    "GrokPattern": "%{GREEDYDATA:line}",
                },
            }),
            json!({
                "GrokClassifier": {
                    "Name": "grok-update-add",
                    "CustomPatterns": "ADDED [0-9]+",
                },
            }),
            json!({
                "Name": "grok-update-add",
                "Classification": "logs",
                "GrokPattern": "%{GREEDYDATA:line}",
                "CustomPatterns": "ADDED [0-9]+",
            }),
        ),
        (
            "xml, both members rewritten",
            "XMLClassifier",
            json!({
                "XMLClassifier": {
                    "Name": "xml-update-both",
                    "Classification": "xml",
                    "RowTag": "record",
                },
            }),
            json!({
                "XMLClassifier": {
                    "Name": "xml-update-both",
                    "Classification": "catalog-export",
                    "RowTag": "row",
                },
            }),
            json!({
                "Name": "xml-update-both",
                "Classification": "catalog-export",
                "RowTag": "row",
            }),
        ),
        (
            "json, the path rewritten",
            "JsonClassifier",
            json!({ "JsonClassifier": { "Name": "json-update", "JsonPath": "$.records[*]" } }),
            json!({ "JsonClassifier": { "Name": "json-update", "JsonPath": "$.items[*]" } }),
            json!({ "Name": "json-update", "JsonPath": "$.items[*]" }),
        ),
        (
            "csv, some members rewritten and the rest preserved",
            "CsvClassifier",
            json!({
                "CsvClassifier": {
                    "Name": "csv-update",
                    "Delimiter": ",",
                    "QuoteSymbol": "\"",
                    "ContainsHeader": "PRESENT",
                    "Header": ["id", "amount"],
                    "DisableValueTrimming": true,
                    "Serde": "OpenCSVSerDe",
                },
            }),
            json!({
                "CsvClassifier": {
                    "Name": "csv-update",
                    "Delimiter": ";",
                    "Header": ["id", "amount", "region"],
                    "AllowSingleColumn": true,
                },
            }),
            json!({
                "Name": "csv-update",
                "Delimiter": ";",
                "QuoteSymbol": "\"",
                "ContainsHeader": "PRESENT",
                "Header": ["id", "amount", "region"],
                "DisableValueTrimming": true,
                "AllowSingleColumn": true,
                "Serde": "OpenCSVSerDe",
            }),
        ),
        (
            "the name alone, which writes no definition member",
            "JsonClassifier",
            json!({ "JsonClassifier": { "Name": "json-untouched", "JsonPath": "$" } }),
            json!({ "JsonClassifier": { "Name": "json-untouched" } }),
            json!({ "Name": "json-untouched", "JsonPath": "$" }),
        ),
    ];

    for (case, kind, created, update, expected) in &cases {
        create(&router, created).await;
        let name = requested_name(created);
        let before = reported(&get(&router, &name).await, kind);

        // `UpdateClassifierResponse` declares no member either.
        let answered = ok(&router, "UpdateClassifier", update).await;
        assert!(
            answered.as_object().is_none_or(|members| members.is_empty()),
            "case {case}: UpdateClassifier must answer with no member, got {answered}"
        );

        let after = reported(&get(&router, &name).await, kind);
        assert_echoed(case, expected, &after);
        assert_eq!(
            after.get("Version").and_then(Value::as_i64),
            Some(FIRST_CLASSIFIER_VERSION + 1),
            "case {case}: a write advances the version by one, got {after}"
        );
        assert_eq!(
            after.get("CreationTime"),
            before.get("CreationTime"),
            "case {case}: the creation instant is not a write's to change, got {after}"
        );
        let created_at = after
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime, got {after}"));
        let last_updated = after
            .get("LastUpdated")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a LastUpdated, got {after}"));
        assert!(
            last_updated >= created_at,
            "case {case}: the write's instant is not before the create's, got {after}"
        );
    }
}

/// A classifier's kind is fixed at create, so an update naming another kind's field is
/// refused and the stored classifier is left as the create wrote it.
#[tokio::test]
async fn an_update_naming_another_kind_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a csv update of a grok classifier",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "kind-grok",
                    "Classification": "syslog",
                    "GrokPattern": "%{GREEDYDATA:line}",
                },
            }),
            json!({ "CsvClassifier": { "Name": "kind-grok", "Delimiter": ";" } }),
        ),
        (
            "a grok update of a json classifier",
            "JsonClassifier",
            json!({ "JsonClassifier": { "Name": "kind-json", "JsonPath": "$" } }),
            json!({ "GrokClassifier": { "Name": "kind-json", "GrokPattern": "%{WORD:w}" } }),
        ),
        (
            "an xml update of a csv classifier",
            "CsvClassifier",
            json!({ "CsvClassifier": { "Name": "kind-csv", "Delimiter": "," } }),
            json!({ "XMLClassifier": { "Name": "kind-csv", "RowTag": "row" } }),
        ),
    ];

    for (case, kind, created, update) in &cases {
        create(&router, created).await;
        let name = requested_name(created);
        let before = reported(&get(&router, &name).await, kind);

        let (status, body) = call(&router, "UpdateClassifier", update).await;
        assert_eq!(
            status, 400,
            "case {case}: a kind that is not the stored one must be refused with 400, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );

        let after = reported(&get(&router, &name).await, kind);
        assert_eq!(
            before, after,
            "case {case}: a refused update writes nothing, got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// `GetClassifiers` reports every stored classifier, in name order, and pages over them
/// with a continuation token: the pages concatenate to the whole set, with no duplicate
/// and no omission, and the last page carries no token.
///
/// The rows differ along the page size, from one classifier per page to a page larger than
/// the set.
#[tokio::test]
async fn the_classifier_listing_pages_over_every_stored_classifier_in_name_order() {
    let router = emulator();
    let mut expected: Vec<String> = Vec::new();
    for (_, _, request) in create_cases() {
        create(&router, &request).await;
        expected.push(requested_name(&request));
    }
    expected.sort();

    for page_size in [1_usize, 2, 3, expected.len(), expected.len() + 1] {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One request per page, plus a guard so a token that never clears cannot spin.
        for _ in 0..expected.len() + 2 {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(cursor) = token.as_ref() {
                request["NextToken"] = json!(cursor);
            }
            let page = ok(&router, "GetClassifiers", &request).await;
            let listed = page["Classifiers"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a Classifiers list, got {page}"))
                .clone();
            assert!(
                listed.len() <= page_size,
                "case {case}: a page must not exceed the requested size, got {page}"
            );
            for classifier in &listed {
                // Each entry is one kind's member, the same projection a single read
                // serves, so the name is read off whichever member is filled.
                let name = CLASSIFIER_KINDS
                    .iter()
                    .find_map(|kind| classifier[*kind].get("Name"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case}: every entry names one kind, got {classifier}")
                    });
                seen.push(name.to_string());
            }
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case}: the last page must carry no NextToken, got {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case}: the pages must concatenate to every stored name in name order"
        );
    }
}

/// What the listing reports is what a single read reports: the listing is a projection of
/// the same records, not a second copy of them.
#[tokio::test]
async fn the_listing_reports_what_a_single_read_reports() {
    let router = emulator();
    let cases = create_cases();
    for (_, _, request) in &cases {
        create(&router, request).await;
    }

    let page = ok(&router, "GetClassifiers", &json!({})).await;
    let listed = page["Classifiers"]
        .as_array()
        .expect("GetClassifiers reports a list")
        .clone();
    assert_eq!(
        listed.len(),
        cases.len(),
        "case every stored classifier: expected {} entries, got {page}",
        cases.len()
    );

    for (case, kind, request) in &cases {
        let name = requested_name(request);
        let read = reported(&get(&router, &name).await, kind);
        let from_listing = listed
            .iter()
            .find(|classifier| classifier[*kind]["Name"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("case {case}: {name} is missing from {page}"));
        assert_eq!(
            from_listing[*kind], read,
            "case {case}: the listing and the read must report one record"
        );
    }
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// `DeleteClassifier` removes the record: a later read of that name finds nothing, the
/// listing no longer carries it, and the name is free for a new create of any kind.
#[tokio::test]
async fn a_deleted_classifier_is_gone_and_its_name_is_free() {
    let router = emulator();

    for (case, kind, request) in create_cases() {
        create(&router, &request).await;
        let name = requested_name(&request);

        ok(&router, "DeleteClassifier", &json!({ "Name": name })).await;

        let (status, body) = call(&router, "GetClassifier", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a deleted classifier must not be readable, got {status} with {body}"
        );
        let page = ok(&router, "GetClassifiers", &json!({})).await;
        assert_eq!(
            page["Classifiers"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a deleted classifier must leave the listing, got {page}"
        );

        // The name is free again, and for another kind than the one deleted.
        let fresh = json!({ "JsonClassifier": { "Name": name, "JsonPath": "$.fresh" } });
        create(&router, &fresh).await;
        let read = get(&router, &name).await;
        assert_echoed(case, &fresh["JsonClassifier"], &reported(&read, "JsonClassifier"));
        assert!(
            kind == "JsonClassifier" || read["Classifier"].get(kind).is_none_or(Value::is_null),
            "case {case}: the new create decides the kind, got {read}"
        );
        ok(&router, "DeleteClassifier", &json!({ "Name": name })).await;
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A name this account holds no classifier under is refused by every operation that
/// addresses one, with the modelled `EntityNotFoundException` and the sentence the service
/// words THAT operation's refusal with: the read and the update state the service's own
/// doubled sentence (`state::messages::classifier_not_found`), and the delete states the
/// miss once over the account and the name
/// (`state::messages::classifier_not_found_for_delete`).
///
/// The rows differ along how the requested name misses — never created, a prefix of a
/// stored name, a suffix of one, a stored name in another case (the model documents no
/// case fold for a classifier name) — and along the operation that addresses it.
#[tokio::test]
async fn addressing_an_unknown_classifier_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({ "JsonClassifier": { "Name": "StoredClassifier", "JsonPath": "$" } }),
    )
    .await;

    for (case, name) in [
        ("never created", "no-such-classifier"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Classifier"),
        ("a stored name lowercased", "storedclassifier"),
        ("a stored name uppercased", "STOREDCLASSIFIER"),
    ] {
        let requests: Vec<(&str, Value, String)> = vec![
            (
                "GetClassifier",
                json!({ "Name": name }),
                format!("Classifier {name} not found. not found."),
            ),
            (
                "DeleteClassifier",
                json!({ "Name": name }),
                format!("Classifier {ACCOUNT_ID}:{name} not found."),
            ),
            (
                "UpdateClassifier",
                json!({ "JsonClassifier": { "Name": name, "JsonPath": "$.other" } }),
                format!("Classifier {name} not found. not found."),
            ),
        ];
        for (operation, request, expected) in &requests {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case} through {operation}: expected 400 for {name:?}, got {status} with \
                 {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case} through {operation}: expected EntityNotFoundException, got {body}"
            );
            assert_eq!(
                &error_message(&body),
                expected,
                "case {case} through {operation}: expected Message {expected:?}, got {body}"
            );
        }
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "StoredClassifier").await;
    assert_eq!(
        read["Classifier"]["JsonClassifier"]["Name"].as_str(),
        Some("StoredClassifier"),
        "case the stored name: expected the stored classifier, got {read}"
    );
}

/// A classifier name is unique per account, whatever kind holds it: the second create
/// under a name already taken is refused with the modelled `AlreadyExistsException`, and
/// the stored classifier is left as the first create wrote it.
#[tokio::test]
async fn a_duplicate_classifier_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "the same kind, a different definition",
            "JsonClassifier",
            json!({ "JsonClassifier": { "Name": "duplicate-one", "JsonPath": "$.first" } }),
            json!({ "JsonClassifier": { "Name": "duplicate-one", "JsonPath": "$.second" } }),
        ),
        (
            "the same kind and the identical definition",
            "CsvClassifier",
            json!({ "CsvClassifier": { "Name": "duplicate-two", "Delimiter": ";" } }),
            json!({ "CsvClassifier": { "Name": "duplicate-two", "Delimiter": ";" } }),
        ),
        (
            "another kind under the same name",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "duplicate-three",
                    "Classification": "syslog",
                    "GrokPattern": "%{GREEDYDATA:line}",
                },
            }),
            json!({ "XMLClassifier": { "Name": "duplicate-three", "Classification": "xml" } }),
        ),
    ];

    for (case, kind, first, second) in &cases {
        create(&router, first).await;
        let name = requested_name(first);

        let (status, body) = call(&router, "CreateClassifier", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );

        // The stored classifier is still the FIRST create's, kind and members.
        let read = get(&router, &name).await;
        assert_echoed(case, &first[kind], &reported(&read, kind));
    }
}

/// The kind to create is which field of the request is present, so a request naming none
/// names no classifier and one naming several names no single classifier: both are refused
/// with the modelled `InvalidInputException` and nothing is stored.
#[tokio::test]
async fn a_create_naming_no_kind_or_several_is_refused() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        ("no kind at all", json!({})),
        (
            "two kinds",
            json!({
                "JsonClassifier": { "Name": "ambiguous-two", "JsonPath": "$" },
                "CsvClassifier": { "Name": "ambiguous-two", "Delimiter": ";" },
            }),
        ),
        (
            "two kinds under different names",
            json!({
                "JsonClassifier": { "Name": "ambiguous-json", "JsonPath": "$" },
                "XMLClassifier": { "Name": "ambiguous-xml", "Classification": "xml" },
            }),
        ),
        (
            "all four kinds",
            json!({
                "GrokClassifier": {
                    "Name": "ambiguous-all",
                    "Classification": "syslog",
                    "GrokPattern": "%{GREEDYDATA:line}",
                },
                "XMLClassifier": { "Name": "ambiguous-all", "Classification": "xml" },
                "JsonClassifier": { "Name": "ambiguous-all", "JsonPath": "$" },
                "CsvClassifier": { "Name": "ambiguous-all", "Delimiter": ";" },
            }),
        ),
    ];

    for (case, request) in &cases {
        let (status, body) = call(&router, "CreateClassifier", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
    }

    // Nothing was stored by any of them.
    let page = ok(&router, "GetClassifiers", &json!({})).await;
    assert_eq!(
        page["Classifiers"].as_array().map(Vec::len),
        Some(0),
        "case a refused create: the store must stay empty, got {page}"
    );

    // An update is about which field is present in the same way.
    for (case, request) in [
        ("an update naming no kind", json!({})),
        (
            "an update naming two kinds",
            json!({
                "JsonClassifier": { "Name": "whatever", "JsonPath": "$" },
                "CsvClassifier": { "Name": "whatever", "Delimiter": ";" },
            }),
        ),
    ] {
        let (status, body) = call(&router, "UpdateClassifier", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
    }
}

/// A CSV classifier's quote symbol "must be different from the column delimiter"
/// (`CsvClassifier$QuoteSymbol`), so a write that would leave the two the same character
/// is refused — on a create, and on an update whichever of the two members it carries.
#[tokio::test]
async fn a_csv_classifier_cannot_quote_with_its_delimiter() {
    let router = emulator();

    // The rows differ along the character the two members share.
    for (case, separator) in [
        ("a comma", ","),
        ("a semicolon", ";"),
        ("a quote", "\""),
        ("a tab", "\t"),
        ("a pipe", "|"),
    ] {
        let request = json!({
            "CsvClassifier": {
                "Name": format!("csv-collision-{}", separator.escape_default()),
                "Delimiter": separator,
                "QuoteSymbol": separator,
            },
        });
        let (status, body) = call(&router, "CreateClassifier", &request).await;
        assert_eq!(
            status, 400,
            "case create with {case} as both separators: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case create with {case} as both separators: expected InvalidInputException, got \
             {body}"
        );
    }

    // On an update the rule is about the classifier the write would leave behind, so the
    // member the request carries is checked against the one the record already holds.
    let stored = json!({
        "CsvClassifier": { "Name": "csv-separators", "Delimiter": ",", "QuoteSymbol": "'" },
    });
    create(&router, &stored).await;
    let before = reported(&get(&router, "csv-separators").await, "CsvClassifier");

    for (case, update) in [
        (
            "the quote symbol moved onto the stored delimiter",
            json!({ "CsvClassifier": { "Name": "csv-separators", "QuoteSymbol": "," } }),
        ),
        (
            "the delimiter moved onto the stored quote symbol",
            json!({ "CsvClassifier": { "Name": "csv-separators", "Delimiter": "'" } }),
        ),
        (
            "both moved onto one character",
            json!({
                "CsvClassifier": {
                    "Name": "csv-separators",
                    "Delimiter": "#",
                    "QuoteSymbol": "#",
                },
            }),
        ),
    ] {
        let (status, body) = call(&router, "UpdateClassifier", &update).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        let after = reported(&get(&router, "csv-separators").await, "CsvClassifier");
        assert_eq!(
            before, after,
            "case {case}: a refused update writes nothing, got {after}"
        );
    }

    // Moving both to a pair that differs is accepted, so the refusals above are about the
    // collision and not about writing those members at all.
    ok(
        &router,
        "UpdateClassifier",
        &json!({
            "CsvClassifier": { "Name": "csv-separators", "Delimiter": "#", "QuoteSymbol": "~" },
        }),
    )
    .await;
    let after = reported(&get(&router, "csv-separators").await, "CsvClassifier");
    assert_echoed(
        "a pair that differs",
        &json!({ "Delimiter": "#", "QuoteSymbol": "~" }),
        &after,
    );
}

/// The store holds a bounded number of classifiers: the create at the ceiling succeeds,
/// the one past it is refused with the closest client error `CreateClassifier` models, and
/// freeing a slot lets a create succeed again — so the ceiling bounds what is stored
/// rather than latching on how many creates were ever made.
#[tokio::test]
async fn a_create_past_the_classifier_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_CLASSIFIERS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateClassifierInput {
                json_classifier: Some(
                    service_com_amazonaws_glue_awsglue::ops::types::CreateJsonClassifierRequest {
                        name: name.clone(),
                        json_path: "$".to_string(),
                    },
                ),
                ..CreateClassifierInput::default()
            };
            let entry = ClassifierEntry::from_create_input(&input, std::time::SystemTime::now())
                .expect("the seeded request names one kind");
            store.classifiers.insert(name, entry);
        }
    }

    let last = json!({ "JsonClassifier": { "Name": "last-classifier", "JsonPath": "$" } });
    let past = json!({ "JsonClassifier": { "Name": "one-too-many", "JsonPath": "$" } });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateClassifier", &past).await;
    assert_eq!(
        status, 400,
        "case classifier {} of {MAX_CLASSIFIERS}: expected 400, got {status} with {body}",
        MAX_CLASSIFIERS + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one classifier too many: expected InvalidInputException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().classifiers.remove("seeded-0");
    }
    create(&router, &past).await;
}
