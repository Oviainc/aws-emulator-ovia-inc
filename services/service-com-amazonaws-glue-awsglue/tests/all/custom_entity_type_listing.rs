// SPDX-License-Identifier: CC0-1.0

//! `ListCustomEntityTypes`: the custom sensitive-data patterns this account holds.
//!
//! RULE: the operation "lists all the custom patterns that have been created", so it
//! reports every pattern the store holds — the same records `CreateCustomEntityType`
//! wrote and `GetCustomEntityType` reads back, member for member — one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes the walk, and
//! nothing but the patterns carrying every pair a `Tags` filter names.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many patterns are stored,
//! the page size asked for (including the published bounds 1 and 1000 and a size past the
//! stored count), which tags the filter names, and which members each stored pattern
//! carries.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, ok};

/// Register one custom pattern, with the tags the row asks for.
async fn given_pattern(router: &MultiServiceRouter, name: &str, tags: Value) {
    let mut body = json!({
        "Name": name,
        "RegexString": format!("{name}-[0-9]+"),
    });
    if !tags.is_null() {
        body["Tags"] = tags;
    }
    ok(router, "CreateCustomEntityType", &body).await;
}

/// The `Name` of every entry in one answer, in the order the service returned them.
fn listed_names(page: &Value) -> Vec<String> {
    page["CustomEntityTypes"]
        .as_array()
        .expect("CustomEntityTypes is a list")
        .iter()
        .map(|entry| {
            entry["Name"]
                .as_str()
                .expect("CustomEntityType.Name is a string")
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out for `request`, following its own `NextToken`,
/// and return the entries in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    base: &Value,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListCustomEntityTypes", &request).await;
        let names = listed_names(&page);
        if let Some(size) = page_size {
            assert!(
                names.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {names:?}"
            );
        }
        listed.extend(
            page["CustomEntityTypes"]
                .as_array()
                .expect("CustomEntityTypes is a list")
                .iter()
                .cloned(),
        );
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= limit,
            "case {case:?}: the walk must terminate, got {listed:?} for {limit} patterns"
        );
    }
    listed
}

#[tokio::test]
async fn every_stored_pattern_is_listed_and_paged() {
    // (case, how many patterns are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no patterns at all", 0, None),
        ("one pattern, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("pattern-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_pattern(&router, name, Value::Null).await;
        }

        let listed = walk(&router, case, &json!({}), *page_size, *stored).await;
        let names: Vec<String> = listed
            .iter()
            .map(|entry| {
                entry["Name"]
                    .as_str()
                    .expect("CustomEntityType.Name is a string")
                    .to_string()
            })
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored pattern in name order, got {names:?}"
        );

        // A pattern the listing reports is one `GetCustomEntityType` resolves, with the
        // same definition: one record, two reads.
        for entry in &listed {
            let name = entry["Name"].as_str().expect("the entry carries a Name");
            let read = ok(
                &router,
                "GetCustomEntityType",
                &json!({ "Name": name }),
            )
            .await;
            assert_eq!(
                read.get("RegexString"),
                entry.get("RegexString"),
                "case {case:?}: GetCustomEntityType({name}) must report the listed pattern, got \
                 {read} for {entry}"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_pattern_carries_every_member_the_create_stored() {
    let router = emulator();

    // The rows differ along which members the stored pattern carries: the two required
    // ones alone, one context word, several, and a create that also carried tags (which
    // `CustomEntityType` does not model, so they are not reported).
    let rows: &[(&str, Value)] = &[
        (
            "required members only",
            json!({ "Name": "aaa-bare", "RegexString": "[0-9]{3}-[0-9]{2}-[0-9]{4}" }),
        ),
        (
            "one context word",
            json!({
                "Name": "bbb-one-word",
                "RegexString": "PAT-[0-9]+",
                "ContextWords": ["patient"],
            }),
        ),
        (
            "several context words, order kept",
            json!({
                "Name": "ccc-many-words",
                "RegexString": "^[A-Z]{2}\\d{6}$",
                "ContextWords": ["alpha", "beta", "gamma"],
            }),
        ),
        (
            "tags sent with the create",
            json!({
                "Name": "ddd-tagged",
                "RegexString": "TAG-[0-9]{4}",
                "ContextWords": ["tagged"],
                "Tags": { "owner": "data-governance" },
            }),
        ),
    ];

    for (_, request) in rows {
        ok(&router, "CreateCustomEntityType", request).await;
    }

    let listed = walk(&router, "every member", &json!({}), None, rows.len()).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case every member: expected {} patterns, got {listed:?}",
        rows.len()
    );

    // The rows were created in name order, which is the order the listing reports.
    for ((case, request), entry) in rows.iter().zip(listed.iter()) {
        let expected = json!({
            "Name": request["Name"],
            "RegexString": request["RegexString"],
            "ContextWords": request.get("ContextWords").cloned().unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, entry);
        // `CustomEntityType` models no `Tags` member, so a listed pattern carries none.
        assert!(
            entry.get("Tags").is_none(),
            "case {case:?}: a listed pattern must report no Tags, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_patterns_carrying_every_pair() {
    let router = emulator();
    given_pattern(
        &router,
        "tagged-both",
        json!({ "team": "data", "env": "prod" }),
    )
    .await;
    given_pattern(&router, "tagged-team", json!({ "team": "data" })).await;
    given_pattern(&router, "tagged-other", json!({ "team": "ops" })).await;
    given_pattern(&router, "untagged", Value::Null).await;

    // (case, the Tags filter, the names it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "no filter: every pattern",
            Value::Null,
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "an empty filter narrows nothing",
            json!({}),
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "one pair",
            json!({ "team": "data" }),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({ "team": "data", "env": "prod" }),
            &["tagged-both"],
        ),
        ("a value nothing carries", json!({ "team": "finance" }), &[]),
        ("a key nothing carries", json!({ "owner": "data" }), &[]),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let page = ok(&router, "ListCustomEntityTypes", &request).await;
        assert_eq!(
            listed_names(&page),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the patterns carrying every pair of {filter}, got {page}"
        );
    }

    // A filter also narrows a PAGED walk, so the token resumes within the filtered set
    // rather than over the whole store.
    let listed = walk(
        &router,
        "one pair, one entry per page",
        &json!({ "Tags": { "team": "data" } }),
        Some(1),
        4,
    )
    .await;
    let names: Vec<String> = listed
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["tagged-both".to_string(), "tagged-team".to_string()],
        "case one pair, one entry per page: expected only the tagged patterns, got {names:?}"
    );
}

#[tokio::test]
async fn the_listing_reports_what_the_store_holds_now() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    for name in ["kept-one", "kept-two", "removed"] {
        given_pattern(&router, name, Value::Null).await;
    }

    let before = listed_names(&ok(&router, "ListCustomEntityTypes", &json!({})).await);
    assert_eq!(
        before,
        vec![
            "kept-one".to_string(),
            "kept-two".to_string(),
            "removed".to_string()
        ],
        "case before the record is dropped: expected every stored pattern, got {before:?}"
    );

    // `DeleteCustomEntityType` is still a scaffold, so the record is dropped from the
    // store the listing reads rather than through that operation.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().custom_entity_types.remove("removed");
    }

    let after = listed_names(&ok(&router, "ListCustomEntityTypes", &json!({})).await);
    assert_eq!(
        after,
        vec!["kept-one".to_string(), "kept-two".to_string()],
        "case after the record is dropped: a pattern the store no longer holds must not be \
         listed, got {after:?}"
    );

    // Listing is reporting only: two listings in a row agree, so a read changed nothing
    // a later caller sees.
    let again = listed_names(&ok(&router, "ListCustomEntityTypes", &json!({})).await);
    assert_eq!(
        again, after,
        "case listed twice: a listing must change nothing, got {again:?} then {after:?}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_pattern(&router, "bounded-pattern", Value::Null).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused
    // by the modelled constraint rather than served as some other page size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(
            &router,
            "ListCustomEntityTypes",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}
