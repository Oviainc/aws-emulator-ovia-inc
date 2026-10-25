// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHAT `SearchSchemas` keyword-indexes a schema under.
//!
//! RULE under test: a keyword selects a schema only through the event the schema's
//! document DECLARES — the event source and detail type an EventBridge document records
//! in its `x-amazon-events-source` / `x-amazon-events-detail-type` extensions, wherever
//! in the document they sit and in any stored version — matched case-insensitively as a
//! substring. A schema's NAME is how every other operation addresses it, not an index
//! entry, and the rest of a document's text is not indexed either: a schema whose
//! document declares no event is indexed under nothing, so no keyword selects it, even
//! one that occurs in its name. It is still reported by a search that names no keyword
//! (which limits nothing), still listed, and still readable under its name.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`. The schemas are created through `CreateSchema` and
//! read back through `DescribeSchema`, so a handler answering from its input rather than
//! from the store would have to make those agree.
//!
//! The harness (`Emulator`, `Answer`, …) is the one `registry_schema_lifecycle` owns —
//! `tests/all/*.rs` are modules of ONE test crate.

use serde_json::{json, Value};

use super::registry_schema_lifecycle::{create_registry, schema_names, schema_uri, Emulator};

/// The registry the table below creates its schemas in and searches.
const REGISTRY: &str = "keyword-index";

/// One schema the table stores.
///
/// Rows differ along everything the index rule generalises over: whether the document
/// declares both event extensions, one of them or neither; the schema `Type`; WHERE the
/// extensions sit (a document's own root, the conventional `AWSEvent` component, a
/// component nested several levels down); and whether a token of the schema's name also
/// occurs in what the document declares.
struct Row {
    name: &'static str,
    /// A member of `TYPE_VALUES`.
    schema_type: &'static str,
    content: &'static str,
    /// The event values this row's document declares, as it carries them — the whole of
    /// what the row is indexed under. Empty for a document that declares no event.
    declared: &'static [&'static str],
}

const ROWS: &[Row] = &[
    // Declares both extensions on the conventional `AWSEvent` component, as a
    // discovered OpenAPI document does.
    Row {
        name: "catalog-orders-v3",
        schema_type: "OpenApi3",
        content: r##"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Basket"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.store","x-amazon-events-detail-type":"Basket Checked Out","properties":{"detail":{"$ref":"#/components/schemas/Basket"}}},"Basket":{"type":"object","properties":{"id":{"type":"string"}}}}}}"##,
        declared: &["my.store", "Basket Checked Out"],
    },
    // Declares both at a draft-04 document's own root, and one of its name's tokens
    // (`root`) occurs in what it declares while the whole hyphenated name does not.
    Row {
        name: "root-declared",
        schema_type: "JSONSchemaDraft4",
        content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Rooted","x-amazon-events-source":"my.root","x-amazon-events-detail-type":"Root Declared","type":"object"}"##,
        declared: &["my.root", "Root Declared"],
    },
    // Declares a source only.
    Row {
        name: "source-only",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Sourced"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.sourceonly","properties":{}}}}}"#,
        declared: &["my.sourceonly"],
    },
    // Declares a detail type only.
    Row {
        name: "detail-only",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Detailed"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-detail-type":"Detail Only Here","properties":{}}}}}"#,
        declared: &["Detail Only Here"],
    },
    // Declares both several levels down, on the component that describes the event
    // rather than on an envelope: the extensions are a property of a component, wherever
    // it sits.
    Row {
        name: "deeply-nested",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Nested"},"paths":{},"components":{"schemas":{"Outer":{"type":"object","properties":{"inner":{"type":"object","properties":{"event":{"type":"object","x-amazon-events-source":"my.nestedsource","x-amazon-events-detail-type":"Buried Event"}}}}}}}}"#,
        declared: &["my.nestedsource", "Buried Event"],
    },
    // Declares NO event: a document a caller wrote by hand, under a generated name —
    // the shape the recorded search was answered empty for.
    Row {
        name: "fruit-apple-7f3a1c",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Fruit apple"},"paths":{},"components":{"schemas":{"Fruit":{"type":"object","properties":{"apple":{"type":"string"}}}}}}"#,
        declared: &[],
    },
    // A draft-04 document that declares no event either, whose text carries words no
    // keyword may reach.
    Row {
        name: "plain-draft4",
        schema_type: "JSONSchemaDraft4",
        content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Widget Ledger","type":"object","properties":{"total":{"type":"number"}}}"##,
        declared: &[],
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

/// Create `REGISTRY` and every row of [`ROWS`] in it.
async fn seed(emulator: &Emulator) {
    create_registry(emulator, REGISTRY).await;
    for row in ROWS {
        create(emulator, REGISTRY, row.name, row.schema_type, row.content).await;
    }
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

/// RULE: a keyword selects a schema only through the event its document declares; a
/// keyword that occurs only in a schema's name, or only elsewhere in its document's
/// text, selects nothing.
#[tokio::test]
async fn only_a_declared_event_is_keyword_indexed() {
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
        // A keyword two rows declare reaches both; adding one only the first declares
        // narrows it to that row.
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
        // A keyword nothing declares selects nothing, even where it is a whole stored
        // NAME or one of its tokens.
        ("catalog-orders-v3", vec![]),
        ("catalog", vec![]),
        ("orders", vec![]),
        ("v3", vec![]),
        ("fruit-apple-7f3a1c", vec![]),
        ("fruit", vec![]),
        ("apple", vec![]),
        ("7f3a1c", vec![]),
        ("plain-draft4", vec![]),
        ("source-only", vec![]),
        ("detail-only", vec![]),
        ("deeply-nested", vec![]),
        ("root-declared", vec![]),
        // Nor when it occurs only elsewhere in a document's text: a title, a component
        // name, a property name, the document's own vocabulary.
        ("widget", vec![]),
        ("ledger", vec![]),
        ("detailed", vec![]),
        ("openapi", vec![]),
        ("3.0.0", vec![]),
        ("properties", vec![]),
        ("outer", vec![]),
        ("total", vec![]),
        ("json-schema.org", vec![]),
        // And a keyword that appears nowhere at all.
        ("nothing-declares-this", vec![]),
        // A value carrying no keyword limits nothing, so it reports the registry — the
        // rows no keyword selects included.
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
/// selects that schema only when the word also occurs in what its document declares.
#[tokio::test]
async fn a_name_is_not_an_index_entry() {
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
            let folded = keyword.to_lowercase();
            // The word may legitimately select the row by occurring in a value the
            // document DECLARES (`root-declared` declares `my.root`, so `root` does).
            let declared = row
                .declared
                .iter()
                .any(|value| value.to_lowercase().contains(&folded));
            let found = searched(&emulator, &keyword).await;
            assert_eq!(
                found.contains(&row.name.to_string()),
                declared,
                "case {:?} keyword {keyword:?}: a name is not an index entry, so it may \
                 select the schema only through its declared {:?}, got {found:?}",
                row.name,
                row.declared
            );
        }
    }
}

/// RULE: a schema no keyword selects is still THERE — a search naming no keyword
/// reports it and `DescribeSchema` serves it under its name. This is the recorded
/// shape: a hand-written document created in a caller's own registry, searched for by
/// words of its own name, is answered with an EMPTY result set.
#[tokio::test]
async fn a_schema_declaring_no_event_is_searchable_by_nothing_but_still_readable() {
    let emulator = Emulator::new();
    let registry = "probe-registry-8c21";
    let schema = "probe-schemas-apple-8c21";
    let content = r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Probe"},"paths":{},"components":{"schemas":{"Probe":{"type":"object","properties":{"apple":{"type":"string"}}}}}}"#;
    create_registry(&emulator, registry).await;
    create(&emulator, registry, schema, "OpenApi3", content).await;

    // Every word of the names the request names, and the names themselves: none of
    // them is something the registry indexes this schema under.
    for keywords in [
        "apple",
        "probe",
        "schemas",
        "8c21",
        "probe-schemas-apple-8c21",
        "probe schemas apple",
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
        assert!(
            found.is_empty(),
            "case {keywords:?}: expected an empty result set, got {found:?}"
        );
    }

    // It is in the registry all the same: the search that limits nothing reports it,
    // and the reader serves the document the create stored.
    let unlimited = emulator
        .call("GET", &search_uri(registry, ""), None)
        .await;
    assert_eq!(
        schema_names(&unlimited),
        vec![schema.to_string()],
        "a search naming no keyword must report the stored schema, got {:?}",
        unlimited.body
    );
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
/// that declares another event adds that event's keywords without dropping the first's.
#[tokio::test]
async fn a_later_version_adds_its_declared_event_to_the_index() {
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

    assert_eq!(
        searched_in(&emulator, registry, "my.firstsource").await,
        vec![schema.to_string()],
        "the created version's declared source must select the schema"
    );
    assert!(
        searched_in(&emulator, registry, "my.secondsource")
            .await
            .is_empty(),
        "nothing declares the second source yet"
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

    // Both versions are held, so the schema is found under either event.
    for keywords in ["my.firstsource", "first event", "my.secondsource", "second"] {
        let found = searched_in(&emulator, registry, keywords).await;
        assert_eq!(
            found,
            vec![schema.to_string()],
            "case {keywords:?}: every stored version's declared event is indexed, got {found:?}"
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
