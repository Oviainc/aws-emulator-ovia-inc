// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `SearchSchemas`.
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
    create_registry, schema_names, schema_uri, walk, Emulator, ACCOUNT, REGION,
};

/// The registry every case below searches.
const REGISTRY: &str = "search-registry";

/// One schema the table creates: its name and the content stored for version 1.
///
/// Rows differ along everything the keyword rule generalises over: where the keyword
/// occurs (name only, content only, both), the case of the stored text, and the
/// alphabet and form of the content.
const STORED: &[(&str, &str, &str)] = &[
    // (schema name, Type, Content)
    (
        "order.placed_v1",
        "OpenApi3",
        r#"{"title":"OrderPlaced","type":"object"}"#,
    ),
    (
        "order.cancelled_v1",
        "JSONSchemaDraft4",
        r#"{"title":"OrderCancelled","type":"object"}"#,
    ),
    // The keyword "order" occurs in this schema's CONTENT and not in its name.
    (
        "payment-received",
        "OpenApi3",
        r#"{"title":"PaymentReceived","detail":"an order id"}"#,
    ),
    ("inventory", "OpenApi3", r#"{"openapi":"3.0.0"}"#),
    ("Mixed-CASE-Name", "OpenApi3", "x"),
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
    for (name, schema_type, content) in STORED {
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, name),
                Some(json!({"Content": content, "Type": schema_type})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
    }
}

/// RULE: a search reports exactly the schemas of the named registry whose own text —
/// the schema name and the content of every stored version — includes EVERY keyword
/// the request names, matched as a case-insensitive substring; a request that names no
/// keyword limits nothing.
#[tokio::test]
async fn keywords_select_exactly_the_schemas_whose_text_includes_every_one() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Results come back in the registry's key order, which is byte order over the
    // schema names: upper case sorts before lower case.
    let everything = vec![
        "Mixed-CASE-Name",
        "inventory",
        "order.cancelled_v1",
        "order.placed_v1",
        "payment-received",
    ];

    for (keywords, expected) in [
        // One keyword in the name of two schemas and in the content of a third.
        (
            "order",
            vec!["order.cancelled_v1", "order.placed_v1", "payment-received"],
        ),
        // Matching is case-insensitive on both sides.
        (
            "ORDER",
            vec!["order.cancelled_v1", "order.placed_v1", "payment-received"],
        ),
        ("mixed-case", vec!["Mixed-CASE-Name"]),
        // A keyword that occurs only in the stored content.
        ("openapi", vec!["inventory"]),
        ("3.0.0", vec!["inventory"]),
        // Every keyword must occur, so a second keyword only narrows.
        ("order placed", vec!["order.placed_v1"]),
        ("order,cancelled", vec!["order.cancelled_v1"]),
        ("order zzz", vec![]),
        // A substring, not a prefix and not an exact name.
        ("placed_v1", vec!["order.placed_v1"]),
        ("laced", vec!["order.placed_v1"]),
        // No keyword at all limits nothing.
        ("", everything.clone()),
        // Nothing stored includes this.
        ("no-schema-says-this", vec![]),
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

    for (name, schema_type, _) in STORED {
        // Each row is selected on its own exact name, so the search returns just it.
        let answer = emulator.call("GET", &search_uri(REGISTRY, name), None).await;
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
            "case {name:?}: a schema's own name selects exactly it, got {matches:?}"
        );
        let summary = &matches[0];

        assert_eq!(
            summary["RegistryName"].as_str(),
            Some(REGISTRY),
            "case {name:?}: the summary names the registry searched, got {:?}",
            summary["RegistryName"]
        );
        assert_eq!(
            summary["SchemaName"].as_str(),
            Some(*name),
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
        // and `UpdateSchema` is still a scaffold (its own stub issue), so one.
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
            Some(*schema_type),
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
        let again = emulator.call("GET", &search_uri(REGISTRY, name), None).await;
        assert_eq!(
            again.body["Schemas"][0]["SchemaVersions"][0]["CreatedDate"],
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
    // Seven schemas the keyword matches and seven it excludes, interleaved, so a
    // page boundary can fall between two matches with a non-match on either side.
    let mut matching: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = format!("order-{index:02}");
        create(&emulator, &name, "x").await;
        matching.push(name);
        create(&emulator, &format!("other-{index:02}"), "y").await;
    }
    matching.sort();

    for limit in [None, Some(1), Some(2), Some(7), Some(10)] {
        let walked = walk(
            &emulator,
            &search_uri(REGISTRY, "order"),
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

async fn create(emulator: &Emulator, name: &str, content: &str) {
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
        .call("GET", &format!("{}&limit=1", search_uri(REGISTRY, "order")), None)
        .await;
    assert_eq!(first.status, 200, "expected 200, got {:?}", first.body);
    let token = first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("three schemas match, so page 1 of 1 carries a token: {:?}", first.body))
        .to_string();

    // Rows: the token's own search resumes; every other selection, and an unusable
    // limit, is a 400.
    let resumed = emulator
        .call(
            "GET",
            &format!("{}&limit=1&nextToken={token}", search_uri(REGISTRY, "order")),
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
        format!("{}&limit=1&nextToken=not-a-token", search_uri(REGISTRY, "order")),
        format!("{}&limit=0", search_uri(REGISTRY, "order")),
        format!("{}&limit=-1", search_uri(REGISTRY, "order")),
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
        // Every keyword occurs in `order.placed_v1`, so a request at the cap is a
        // search that still has an answer to give, not an empty one.
        let keywords: Vec<String> = (0..count)
            .map(|index| ["order", "placed", "title", "object", "v1"][index % 5].to_string())
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
                vec!["order.placed_v1"],
                "case {count} keyword(s): every keyword occurs in exactly one schema"
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
            .call("GET", &search_uri(registry, "order"), None)
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
    for keywords in ["order", "", "no-schema-says-this", "openapi"] {
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
