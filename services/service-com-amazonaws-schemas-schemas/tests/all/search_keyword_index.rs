// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHAT `SearchSchemas` keyword-indexes a schema under.
//!
//! RULE under test: a keyword narrows a registry's schemas to those it OCCURS IN —
//! matched case-insensitively as a substring of the schema's name or of the text of any
//! definition the service holds for it (a published schema also carries the event
//! identity its document does not state) — and every keyword a request names must occur,
//! while a value carrying no keyword limits nothing and reports the whole registry.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`. The schemas are created through `CreateSchema` and
//! read back through `DescribeSchema`, so a handler answering from its input rather than
//! from the store would have to make those agree.
//!
//! The harness (`Emulator`, `Answer`, …) is the one `registry_schema_lifecycle` owns —
//! `tests/all/*.rs` are modules of ONE test crate.

use serde_json::{json, Value};

use super::registry_schema_lifecycle::{
    create_registry, schema_names, schema_uri, wait_for_keyword_index, Emulator,
};

/// The registry the table below creates its schemas in and searches.
const REGISTRY: &str = "keyword-index";

/// One schema the table stores.
///
/// Rows differ along everything the index rule generalises over: WHERE the keyword that
/// selects a row occurs (its name, its document's title, a component name, a property
/// name, an EventBridge extension value at the document root, on the conventional
/// `AWSEvent` component or several levels down), the schema `Type`, the letter case of
/// the stored text, and whether the document declares an event at all.
struct Row {
    name: &'static str,
    /// A member of `TYPE_VALUES`.
    schema_type: &'static str,
    content: &'static str,
}

const ROWS: &[Row] = &[
    // Declares both extensions on the conventional `AWSEvent` component, as a
    // discovered OpenAPI document does.
    Row {
        name: "catalog-orders-v3",
        schema_type: "OpenApi3",
        content: r##"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Basket"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.store","x-amazon-events-detail-type":"Basket Checked Out","properties":{"detail":{"$ref":"#/components/schemas/Basket"}}},"Basket":{"type":"object","properties":{"id":{"type":"string"}}}}}}"##,
    },
    // Declares both at a draft-04 document's own root.
    Row {
        name: "root-declared",
        schema_type: "JSONSchemaDraft4",
        content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Rooted","x-amazon-events-source":"my.root","x-amazon-events-detail-type":"Root Declared","type":"object"}"##,
    },
    // Declares a source only.
    Row {
        name: "source-only",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Sourced"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.sourceonly","properties":{}}}}}"#,
    },
    // Declares a detail type only.
    Row {
        name: "detail-only",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Detailed"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-detail-type":"Detail Only Here","properties":{}}}}}"#,
    },
    // Declares both several levels down, on the component that describes the event
    // rather than on an envelope.
    Row {
        name: "deeply-nested",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Nested"},"paths":{},"components":{"schemas":{"Outer":{"type":"object","properties":{"inner":{"type":"object","properties":{"event":{"type":"object","x-amazon-events-source":"my.nestedsource","x-amazon-events-detail-type":"Buried Event"}}}}}}}}"#,
    },
    // Declares NO event: a document a caller wrote by hand, under a generated name —
    // the recorded shape, whose unique property name is what the caller searches for.
    Row {
        name: "fruit-apple-7f3a1c",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Fruit apple"},"paths":{},"components":{"schemas":{"Fruit":{"type":"object","properties":{"apple":{"type":"string"}}}}}}"#,
    },
    // A draft-04 document that declares no event either, whose title and property name
    // are words no other row carries.
    Row {
        name: "plain-draft4",
        schema_type: "JSONSchemaDraft4",
        content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Widget Ledger","type":"object","properties":{"total":{"type":"number"}}}"##,
    },
];

/// Every stored name, in the order a listing of the registry collates them (by each
/// name's letters and digits, case-folded) — which is the order a search serves too.
const EVERY_NAME: &[&str] = &[
    "catalog-orders-v3",
    "deeply-nested",
    "detail-only",
    "fruit-apple-7f3a1c",
    "plain-draft4",
    "root-declared",
    "source-only",
];

/// `GET /v1/registries/name/{registry}/schemas/search?keywords=…`, with the one
/// character these cases use that a query string cannot carry raw.
fn search_uri(registry: &str, keywords: &str) -> String {
    format!(
        "/v1/registries/name/{registry}/schemas/search?keywords={}",
        keywords.replace(' ', "%20")
    )
}

/// Create one schema, asserting the create succeeded.
async fn create(emulator: &Emulator, registry: &str, name: &str, schema_type: &str, content: &str) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, name),
            Some(json!({"Content": content, "Type": schema_type})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// Create `REGISTRY` and every row of [`ROWS`] in it, and wait until the registry's
/// keyword index has caught up with them — what these cases assert is WHAT the index
/// holds; the catch-up itself is
/// `tests/all/schema_version_and_export_sequences.rs`'s case.
async fn seed(emulator: &Emulator) {
    create_registry(emulator, REGISTRY).await;
    for row in ROWS {
        create(emulator, REGISTRY, row.name, row.schema_type, row.content).await;
    }
    wait_for_keyword_index();
}

/// The schema names a search for `keywords` reports, asserting it answered 200.
async fn searched(emulator: &Emulator, keywords: &str) -> Vec<String> {
    let answer = emulator
        .call("GET", &search_uri(REGISTRY, keywords), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {keywords:?}: expected 200 from SearchSchemas, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.get("NextToken").is_none_or(Value::is_null),
        "case {keywords:?}: a single complete page carries no NextToken, got {:?}",
        answer.body["NextToken"]
    );
    schema_names(&answer)
}

/// RULE: a keyword selects exactly the schemas it occurs in — in the name, anywhere in
/// the stored document's text (an EventBridge extension value, a title, a component or
/// property name, the document's own vocabulary) — and a keyword occurring in none of
/// them selects nothing.
#[tokio::test]
async fn a_keyword_selects_the_schemas_it_occurs_in() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let everything: Vec<&str> = EVERY_NAME.to_vec();

    for (keywords, expected) in [
        // A whole declared source, and a substring of one.
        ("my.store", vec!["catalog-orders-v3"]),
        ("store", vec!["catalog-orders-v3"]),
        ("my.sourceonly", vec!["source-only"]),
        ("sourceonly", vec!["source-only"]),
        ("my.nestedsource", vec!["deeply-nested"]),
        ("my.root", vec!["root-declared"]),
        // A declared detail type: whole, in part, and as several keywords that must
        // all occur.
        ("basket", vec!["catalog-orders-v3"]),
        ("checked out", vec!["catalog-orders-v3"]),
        ("basket,checked,out", vec!["catalog-orders-v3"]),
        ("detail only here", vec!["detail-only"]),
        ("buried", vec!["deeply-nested"]),
        ("buried event", vec!["deeply-nested"]),
        ("root declared", vec!["root-declared"]),
        // Case is folded on both sides.
        ("MY.STORE", vec!["catalog-orders-v3"]),
        ("BaSkEt", vec!["catalog-orders-v3"]),
        // A keyword several rows carry reaches each of them; adding one only the first
        // carries narrows it to that row.
        (
            "my.",
            vec![
                "catalog-orders-v3",
                "deeply-nested",
                "root-declared",
                "source-only",
            ],
        ),
        ("my. store", vec!["catalog-orders-v3"]),
        // A schema's NAME, whole and in part: a caller knows the schema by it, so a
        // keyword drawn from it selects the schema.
        ("catalog-orders-v3", vec!["catalog-orders-v3"]),
        ("catalog", vec!["catalog-orders-v3"]),
        ("orders", vec!["catalog-orders-v3"]),
        ("v3", vec!["catalog-orders-v3"]),
        ("fruit-apple-7f3a1c", vec!["fruit-apple-7f3a1c"]),
        ("7f3a1c", vec!["fruit-apple-7f3a1c"]),
        ("plain-draft4", vec!["plain-draft4"]),
        ("source-only", vec!["source-only"]),
        ("deeply-nested", vec!["deeply-nested"]),
        ("root-declared", vec!["root-declared"]),
        // Anything else the document's text states: a property name (the recorded
        // case), a component name, a title, the document's own vocabulary.
        ("apple", vec!["fruit-apple-7f3a1c"]),
        ("fruit", vec!["fruit-apple-7f3a1c"]),
        ("widget", vec!["plain-draft4"]),
        ("ledger", vec!["plain-draft4"]),
        ("total", vec!["plain-draft4"]),
        ("detailed", vec!["detail-only"]),
        ("outer", vec!["deeply-nested"]),
        ("json-schema.org", vec!["plain-draft4", "root-declared"]),
        (
            "openapi",
            vec![
                "catalog-orders-v3",
                "deeply-nested",
                "detail-only",
                "fruit-apple-7f3a1c",
                "source-only",
            ],
        ),
        (
            "3.0.0",
            vec![
                "catalog-orders-v3",
                "deeply-nested",
                "detail-only",
                "fruit-apple-7f3a1c",
                "source-only",
            ],
        ),
        // And a keyword that appears nowhere at all.
        ("nothing-carries-this", vec![]),
        ("my.store widget", vec![]),
        // A value carrying no keyword limits nothing, so it reports the registry.
        ("", everything.clone()),
        ("  ", everything.clone()),
        (",, ,", everything.clone()),
    ] {
        let found = searched(&emulator, keywords).await;
        assert_eq!(
            found, expected,
            "case {keywords:?}: expected {expected:?}, got {found:?}"
        );
    }
}

/// RULE (the same rule as a property of every stored row): a keyword drawn from a
/// schema's NAME — the whole name, or any of the words punctuation splits it into —
/// selects that schema.
#[tokio::test]
async fn a_name_is_an_index_entry() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for row in ROWS {
        let mut candidates = vec![row.name.to_string()];
        candidates.extend(
            row.name
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_string),
        );
        for keyword in candidates {
            let found = searched(&emulator, &keyword).await;
            assert!(
                found.contains(&row.name.to_string()),
                "case {:?} keyword {keyword:?}: a word of a schema's name must select it, \
                 got {found:?}",
                row.name
            );
        }
    }
}

/// RULE (the recorded shape): a document a caller wrote by hand, created in a registry
/// of their own, is found by a word of its content and by its own name — and is read
/// back under that name with the bytes the create stored.
#[tokio::test]
async fn a_schema_a_caller_wrote_is_searchable_by_its_content_and_its_name() {
    let emulator = Emulator::new();
    let registry = "probe-registry-8c21";
    let schema = "probe-schemas-apple-8c21";
    let content = r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Probe"},"paths":{},"components":{"schemas":{"Probe":{"type":"object","properties":{"quokka":{"type":"string"}}}}}}"#;
    create_registry(&emulator, registry).await;
    create(&emulator, registry, schema, "OpenApi3", content).await;
    wait_for_keyword_index();

    // Rows: a property name only the document carries, its title, the schema's own
    // name, words of that name, and two of them at once.
    for keywords in [
        "quokka",
        "QUOKKA",
        "probe",
        "apple",
        "8c21",
        "probe-schemas-apple-8c21",
        "probe schemas apple",
        "quokka probe",
    ] {
        let answer = emulator
            .call("GET", &search_uri(registry, keywords), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {keywords:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let found = schema_names(&answer);
        assert_eq!(
            found,
            vec![schema.to_string()],
            "case {keywords:?}: expected the stored schema, got {found:?}"
        );
    }

    // A keyword nothing about it carries selects nothing, so the search still
    // discriminates.
    for keywords in ["wombat", "quokka wombat"] {
        let found = schema_names(
            &emulator
                .call("GET", &search_uri(registry, keywords), None)
                .await,
        );
        assert!(
            found.is_empty(),
            "case {keywords:?}: expected an empty result set, got {found:?}"
        );
    }

    // And the reader serves the document the create stored.
    let described = emulator
        .call("GET", &schema_uri(registry, schema), None)
        .await;
    assert_eq!(
        described.status, 200,
        "expected 200 from DescribeSchema, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.body["Content"].as_str(),
        Some(content),
        "DescribeSchema must serve the stored document, got {:?}",
        described.body["Content"]
    );
}

/// RULE: the index covers every version this service holds for a schema, so a version
/// whose document states something new adds those words without dropping the first's.
#[tokio::test]
async fn a_later_version_adds_its_text_to_the_index() {
    let emulator = Emulator::new();
    let registry = "versioned-index";
    let schema = "evolving";
    create_registry(&emulator, registry).await;
    create(
        &emulator,
        registry,
        schema,
        "OpenApi3",
        r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"First"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.firstsource","x-amazon-events-detail-type":"First Event","properties":{}}}}}"#,
    )
    .await;
    wait_for_keyword_index();

    assert_eq!(
        searched_in(&emulator, registry, "my.firstsource").await,
        vec![schema.to_string()],
        "the created version's declared source must select the schema"
    );
    assert!(
        searched_in(&emulator, registry, "my.secondsource")
            .await
            .is_empty(),
        "nothing states the second source yet"
    );

    let updated = emulator
        .call(
            "PUT",
            &schema_uri(registry, schema),
            Some(json!({
                "Content": r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Second"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.secondsource","x-amazon-events-detail-type":"Second Event","properties":{}}}}}"#,
                "Type": "OpenApi3",
            })),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateSchema, got {} {:?}",
        updated.status, updated.body
    );
    wait_for_keyword_index();

    // Both versions are held, so the schema is found under either one's text.
    for keywords in ["my.firstsource", "first event", "my.secondsource", "second"] {
        let found = searched_in(&emulator, registry, keywords).await;
        assert_eq!(
            found,
            vec![schema.to_string()],
            "case {keywords:?}: every stored version's text is indexed, got {found:?}"
        );
    }
}

/// The schema names a search of `registry` for `keywords` reports, asserting 200.
async fn searched_in(emulator: &Emulator, registry: &str, keywords: &str) -> Vec<String> {
    let answer = emulator
        .call("GET", &search_uri(registry, keywords), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {keywords:?}: expected 200 from SearchSchemas, got {} {:?}",
        answer.status, answer.body
    );
    schema_names(&answer)
}
