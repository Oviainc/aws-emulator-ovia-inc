// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `SearchSchemas`.
//!
//! RULE under test: `Keywords` LIMITS the named registry's schemas to those every
//! requested keyword OCCURS IN, matched case-insensitively as a substring of the schema's
//! name or of the text of any definition the service holds for it; a value carrying no
//! keyword limits nothing and so reports the whole registry. The keyword index itself is
//! pinned over a wider table in `tests/all/search_keyword_index.rs`.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! schemas a search reports are created through `CreateSchema` and read back through
//! `DescribeSchema`, so a handler that answered from its input instead of from the
//! store would have to make those agree.
//!
//! The harness (`Emulator`, `Answer`, `walk`, …) is the one `registry_schema_lifecycle`
//! already owns — `tests/all/*.rs` are modules of ONE test crate.

use serde_json::{json, Value};

use super::registry_schema_lifecycle::{
    create_registry, schema_names, schema_uri, wait_for_keyword_index, walk, Emulator, ACCOUNT,
    REGION,
};

/// The registry every case below searches.
const REGISTRY: &str = "search-registry";

/// One schema the table creates.
///
/// Rows differ along everything the keyword rule generalises over: whether the keyword
/// that selects a row sits in an EventBridge extension, in the document's title, in a
/// component or property name or in the schema's own name; where in the document it sits
/// (an `OpenApi3` component, a draft-04 document's own root); and the case of the stored
/// text.
struct Stored {
    /// The schema's name — which a caller knows it by, so a keyword of it selects it.
    name: &'static str,
    /// A member of `TYPE_VALUES`.
    schema_type: &'static str,
    content: &'static str,
    /// One keyword that selects exactly this row, for the summary assertions.
    selector: &'static str,
}

/// The schemas every case below searches over.
const STORED: &[Stored] = &[
    // Declares its event in the `AWSEvent` component, as a discovered OpenAPI schema
    // does.
    Stored {
        name: "orders.placed",
        schema_type: "OpenApi3",
        content: r##"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"OrderPlaced"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-detail-type":"Order Placed v1","x-amazon-events-source":"my.orders","properties":{"detail":{"$ref":"#/components/schemas/OrderPlaced"}}},"OrderPlaced":{"type":"object","properties":{"id":{"type":"string"}}}}}}"##,
        selector: "placed",
    },
    // Declares the same source at the root of a draft-04 document: the extensions are
    // a property of the component that describes the event, wherever it sits.
    Stored {
        name: "orders.cancelled",
        schema_type: "JSONSchemaDraft4",
        content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","title":"OrderCancelled","x-amazon-events-source":"my.orders","x-amazon-events-detail-type":"Order Cancelled","type":"object"}"##,
        selector: "cancelled",
    },
    Stored {
        name: "payments",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"PaymentReceived"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.payments","x-amazon-events-detail-type":"Payment Received","properties":{}}}}}"#,
        selector: "received",
    },
    // Declares its event in upper case, and is selected by a lower-case keyword.
    Stored {
        name: "MiXeD-case",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Mixed"},"paths":{},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.MIXED","x-amazon-events-detail-type":"Case SHIFTED","properties":{}}}}}"#,
        selector: "shifted",
    },
    // Declares NO event: a document a caller wrote by hand. It is found by what its own
    // name and its document's text carry.
    Stored {
        name: "orders-report",
        schema_type: "OpenApi3",
        content: r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"Orders report"},"paths":{},"components":{"schemas":{"Report":{"type":"object","properties":{"orders":{"type":"array","items":{"type":"string"}}}}}}}"#,
        selector: "report",
    },
    // A draft-04 document with no EventBridge extension either, whose name shares no
    // substring with any other row: the ordinary `CreateSchema` case.
    Stored {
        name: "invoice-v2",
        schema_type: "JSONSchemaDraft4",
        content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Invoice","type":"object","properties":{"total":{"type":"number"}}}"##,
        selector: "invoice",
    },
];

/// `GET /v1/registries/name/{registry}/schemas/search?keywords=…`, with the few
/// characters these cases use that a query string cannot carry raw.
fn search_uri(registry: &str, keywords: &str) -> String {
    format!(
        "/v1/registries/name/{registry}/schemas/search?keywords={}",
        keywords.replace(' ', "%20")
    )
}

/// Create `REGISTRY` and every row of [`STORED`] in it.
async fn seed(emulator: &Emulator) {
    create_registry(emulator, REGISTRY).await;
    for stored in STORED {
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, stored.name),
                Some(json!({"Content": stored.content, "Type": stored.schema_type})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateSchema, got {} {:?}",
            stored.name, created.status, created.body
        );
    }
    // What these cases assert is WHAT the keyword index holds, so they start once it has
    // caught up with the writes above; the catch-up itself is
    // `tests/all/schema_version_and_export_sequences.rs`'s case.
    wait_for_keyword_index();
}

/// RULE: a search reports exactly the schemas of the named registry that EVERY keyword
/// the request names occurs in — as a case-insensitive substring of the schema's name or
/// of a stored definition's text — and a request that names no keyword limits nothing.
#[tokio::test]
async fn keywords_select_exactly_the_schemas_they_occur_in() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Results come back in the registry's key order, which is the codepoint order of
    // the schema names.
    let everything = vec![
        "MiXeD-case",
        "invoice-v2",
        "orders-report",
        "orders.cancelled",
        "orders.placed",
        "payments",
    ];

    for (keywords, expected) in [
        // The whole declared source, and a substring of it.
        ("my.orders", vec!["orders.cancelled", "orders.placed"]),
        // One keyword reaching every row that carries it: two declare it as their
        // source, and the third carries it in its name, its title and a property name.
        (
            "orders",
            vec!["orders-report", "orders.cancelled", "orders.placed"],
        ),
        // Matching is case-insensitive on both sides.
        (
            "ORDERS",
            vec!["orders-report", "orders.cancelled", "orders.placed"],
        ),
        ("mixed", vec!["MiXeD-case"]),
        ("SHIFTED", vec!["MiXeD-case"]),
        // A keyword of the declared detail type rather than of the source.
        ("placed", vec!["orders.placed"]),
        ("payment received", vec!["payments"]),
        // Every keyword must occur, so a second keyword only narrows: one from the
        // source and one from the detail type.
        ("orders placed", vec!["orders.placed"]),
        ("my,cancelled", vec!["orders.cancelled"]),
        ("orders zzz", vec![]),
        // A schema's NAME selects it, whole or in part — and so does the rest of what
        // its document states.
        ("report", vec!["orders-report"]),
        ("orders-report", vec!["orders-report"]),
        ("INVOICE", vec!["invoice-v2"]),
        ("invoice-v2", vec!["invoice-v2"]),
        ("v2", vec!["invoice-v2"]),
        ("orders.placed", vec!["orders.placed"]),
        ("total", vec!["invoice-v2"]),
        ("invoice total", vec!["invoice-v2"]),
        // A keyword of a DECLARED detail type still selects, even where it also looks
        // like a name fragment.
        ("V1", vec!["orders.placed"]),
        // A word of the document's own vocabulary is text the document states, so it
        // reaches every row stored in that language.
        (
            "openapi",
            vec![
                "MiXeD-case",
                "orders-report",
                "orders.placed",
                "payments",
            ],
        ),
        (
            "3.0.0",
            vec![
                "MiXeD-case",
                "orders-report",
                "orders.placed",
                "payments",
            ],
        ),
        // A request that names no keyword limits nothing, which is every schema the
        // registry holds — whether the value is empty or carries separators only.
        ("", everything.clone()),
        (" ", everything.clone()),
        (",,  ,", everything.clone()),
        ("nothing-carries-this", vec![]),
    ] {
        let answer = emulator.call("GET", &search_uri(REGISTRY, keywords), None).await;
        assert_eq!(
            answer.status, 200,
            "case {keywords:?}: expected 200 from SearchSchemas, got {} {:?}",
            answer.status, answer.body
        );
        let found = schema_names(&answer);
        assert_eq!(
            found, expected,
            "case {keywords:?}: expected {expected:?}, got {found:?}"
        );
        assert!(
            answer.body.get("NextToken").is_none_or(Value::is_null),
            "case {keywords:?}: a single complete page carries no NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }
}

/// RULE: every member of a search summary comes from what the create persisted — the
/// registry it lives in, the ARN minted at create time, the stored name, and one
/// version summary per stored version, agreeing with `DescribeSchema`.
#[tokio::test]
async fn a_match_reports_the_stored_schema_and_its_versions() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for stored in STORED {
        let name = stored.name;
        // Each row is read back through the keyword that selects exactly it.
        let selector = stored.selector;
        let answer = emulator
            .call("GET", &search_uri(REGISTRY, selector), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let matches = answer.body["Schemas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected a Schemas array in {:?}", answer.body));
        assert_eq!(
            matches.len(),
            1,
            "case {name:?}: only this schema carries {selector:?}, got {matches:?}"
        );
        let summary = matches
            .iter()
            .find(|item| item["SchemaName"].as_str() == Some(name))
            .unwrap_or_else(|| {
                panic!("case {name:?}: expected it among the matches of {selector:?}, got {matches:?}")
            });

        assert_eq!(
            summary["RegistryName"].as_str(),
            Some(REGISTRY),
            "case {name:?}: the summary names the registry searched, got {:?}",
            summary["RegistryName"]
        );
        assert_eq!(
            summary["SchemaName"].as_str(),
            Some(name),
            "case {name:?}: the summary names the stored schema, got {:?}",
            summary["SchemaName"]
        );
        assert_eq!(
            summary["SchemaArn"].as_str(),
            Some(format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/{REGISTRY}/{name}").as_str()),
            "case {name:?}: the summary reports the ARN the create minted, got {:?}",
            summary["SchemaArn"]
        );

        let versions = summary["SchemaVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected a SchemaVersions array in {summary:?}"));
        // One entry per version this service holds. `CreateSchema` stores version 1
        // and nothing here updates the schema, so one.
        assert_eq!(
            versions.len(),
            1,
            "case {name:?}: one summary per stored version, got {versions:?}"
        );
        assert_eq!(
            versions[0]["SchemaVersion"].as_str(),
            Some("1"),
            "case {name:?}: the stored version is 1, got {:?}",
            versions[0]["SchemaVersion"]
        );
        assert_eq!(
            versions[0]["Type"].as_str(),
            Some(stored.schema_type),
            "case {name:?}: the version reports the stored Type, got {:?}",
            versions[0]["Type"]
        );

        // The round trip: the version's creation instant is the one the create
        // stored, as the implemented reader reports it.
        let described = emulator
            .call("GET", &schema_uri(REGISTRY, name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        let searched_secs = timestamp_secs(&versions[0]["CreatedDate"], name);
        let described_secs = timestamp_secs(described.value_at("VersionCreatedDate"), name);
        assert_eq!(
            searched_secs, described_secs,
            "case {name:?}: CreatedDate must be the stored write time DescribeSchema reports, \
             got {searched_secs} vs {described_secs}"
        );

        // And it does not move between reads.
        let again = emulator
            .call("GET", &search_uri(REGISTRY, selector), None)
            .await;
        let again_summary = again.body["Schemas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected a Schemas array in {:?}", again.body))
            .iter()
            .find(|item| item["SchemaName"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {name:?}: the second search must report it too"))
            .clone();
        assert_eq!(
            again_summary["SchemaVersions"][0]["CreatedDate"],
            versions[0]["CreatedDate"],
            "case {name:?}: CreatedDate must not change between searches"
        );
    }
}

/// The epoch seconds a timestamp member carries.
///
/// What these cases own is the VALUE — fixed at write time and stable across reads —
/// not its rendering, so both the `date-time` string the model requires (pinned in
/// `tests/all/iso8601_timestamps.rs`) and a bare number of seconds are reduced to
/// seconds before comparing.
fn timestamp_secs(value: &Value, case: &str) -> i64 {
    match value {
        Value::String(rendered) => aws_smithy_types::DateTime::from_str(
            rendered,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| panic!("case {case:?}: {rendered:?} is not an ISO-8601 instant ({e})"))
        .secs(),
        Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {seconds:?} is not a number of seconds"))
            as i64,
        other => panic!("case {case:?}: {other:?} is not a timestamp"),
    }
}

/// RULE (an invariant): following `NextToken` walks a search exactly once — the
/// concatenated pages are the whole selection in key order, with no repeat and no gap —
/// for every page size.
#[tokio::test]
async fn following_next_token_walks_a_search_exactly_once() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    // Seven schemas declaring the keyword's event and seven declaring another,
    // interleaved, so a page boundary can fall between two matches with a non-match on
    // either side.
    let mut matching: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = format!("order-{index:02}");
        create(&emulator, &name, "my.orders").await;
        matching.push(name);
        create(&emulator, &format!("other-{index:02}"), "my.widgets").await;
    }
    matching.sort();
    wait_for_keyword_index();

    for limit in [None, Some(1), Some(2), Some(7), Some(10)] {
        let walked = walk(
            &emulator,
            &search_uri(REGISTRY, "orders"),
            limit,
            schema_names,
        )
        .await;
        assert_eq!(
            walked, matching,
            "case limit={limit:?}: a search must walk every match once, in key order"
        );
    }
}

/// Create a schema declaring `source` as the event source it describes.
async fn create(emulator: &Emulator, name: &str, source: &str) {
    let content = json!({
        "openapi": "3.0.0",
        "info": {"version": "1.0.0", "title": name},
        "paths": {},
        "components": {"schemas": {"AWSEvent": {
            "type": "object",
            "x-amazon-events-source": source,
            "x-amazon-events-detail-type": "Paged Event",
            "properties": {},
        }}},
    })
    .to_string();
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, name),
            Some(json!({"Content": content, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// RULE: a `NextToken` names a position within ONE search, so presenting it to a
/// differently-selected search is refused rather than skipping or repeating matches;
/// and a page of fewer than one item is not a page.
#[tokio::test]
async fn a_token_or_limit_a_search_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let first = emulator
        .call("GET", &format!("{}&limit=1", search_uri(REGISTRY, "orders")), None)
        .await;
    assert_eq!(first.status, 200, "expected 200, got {:?}", first.body);
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("two schemas are indexed under this keyword, so page 1 carries a token: {:?}", first.body))
        .to_string();

    // Rows: the token's own search resumes; every other selection, and an unusable
    // limit, is a 400.
    let resumed = emulator
        .call(
            "GET",
            &format!("{}&limit=1&nextToken={token}", search_uri(REGISTRY, "orders")),
            None,
        )
        .await;
    assert_eq!(
        resumed.status, 200,
        "the token's own search must resume, got {} {:?}",
        resumed.status, resumed.body
    );

    for case in [
        format!("{}&limit=1&nextToken={token}", search_uri(REGISTRY, "placed")),
        format!("{}&limit=1&nextToken={token}", search_uri(REGISTRY, "")),
        format!("{}&limit=1&nextToken=not-a-token", search_uri(REGISTRY, "orders")),
        format!("{}&limit=0", search_uri(REGISTRY, "orders")),
        format!("{}&limit=-1", search_uri(REGISTRY, "orders")),
    ] {
        let answer = emulator.call("GET", &case, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.body
        );
    }
}

/// RULE: a search costs one pass over the matched text per keyword, so the number of
/// keywords one request may name is capped — at the cap the search is served, past it
/// it is refused with 400 before anything is scanned.
#[tokio::test]
async fn a_search_naming_more_keywords_than_the_cap_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The bound is the service's own, read from where it is declared.
    let cap = service_com_amazonaws_schemas_schemas::state::MAX_KEYWORDS_PER_SEARCH;
    // Rows: two keywords, each side of the published bound, and one past it.
    for (count, expected) in [(2, 200), (cap - 1, 200), (cap, 200), (cap + 1, 400)] {
        // Every keyword occurs in what `orders.placed` declares (source `my.orders`,
        // detail type `Order Placed v1`), so a request at the cap is a search that
        // still has an answer to give, not an empty one.
        let keywords: Vec<String> = (0..count)
            .map(|index| ["orders", "placed", "v1", "my", "order"][index % 5].to_string())
            .collect();
        let answer = emulator
            .call("GET", &search_uri(REGISTRY, &keywords.join(" ")), None)
            .await;
        assert_eq!(
            answer.status, expected,
            "case {count} keyword(s): expected {expected}, got {} {:?}",
            answer.status, answer.body
        );
        if expected == 200 {
            assert_eq!(
                schema_names(&answer),
                vec!["orders.placed"],
                "case {count} keyword(s): every keyword is declared by exactly one schema"
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {count} keyword(s): expected BadRequestException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: the registry a search names is an `@httpLabel`, so searching one no caller
/// created is the same 404 every other registry-scoped operation answers.
#[tokio::test]
async fn searching_a_registry_that_does_not_exist_is_a_404() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;

    for (case, registry) in [
        ("never-created", "no-such-registry"),
        ("a-deleted-registry", REGISTRY),
    ] {
        if case == "a-deleted-registry" {
            let deleted = emulator
                .call("DELETE", &format!("/v1/registries/name/{registry}"), None)
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected 204 from DeleteRegistry, got {} {:?}",
                deleted.status, deleted.body
            );
        }
        let answer = emulator
            .call("GET", &search_uri(registry, "orders"), None)
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            answer.body
        );
    }
}

/// A search REPORTS: it must not change what a later caller sees.
#[tokio::test]
async fn a_search_changes_nothing() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let before = emulator
        .call("GET", &format!("/v1/registries/name/{REGISTRY}/schemas"), None)
        .await;
    for keywords in ["orders", "", "nothing-carries-this", "placed"] {
        let answer = emulator.call("GET", &search_uri(REGISTRY, keywords), None).await;
        assert_eq!(
            answer.status, 200,
            "case {keywords:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    let after = emulator
        .call("GET", &format!("/v1/registries/name/{REGISTRY}/schemas"), None)
        .await;
    assert_eq!(
        after.body, before.body,
        "searching must leave the store exactly as it was"
    );
}
