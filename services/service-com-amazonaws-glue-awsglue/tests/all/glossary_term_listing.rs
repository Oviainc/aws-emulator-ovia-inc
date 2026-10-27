// SPDX-License-Identifier: CC0-1.0

//! `ListGlossaryTerms`: the glossary terms one business glossary holds.
//!
//! RULE: the operation "lists glossary terms within a business glossary", so it reports
//! every stored term recorded against the glossary the request's `GlossaryIdentifier`
//! names — the same records `CreateGlossaryTerm` wrote and `GetGlossaryTerm` reads back —
//! projected to the three members `GlossaryTermItem` declares, one `MaxResults`-sized page
//! at a time in term-name order with a `NextToken` that resumes the walk, and nothing
//! belonging to any other glossary.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many terms are stored, the
//! page size asked for (including the published bounds 1 and 1000, a size smaller than the
//! stored count and one past it), which glossary the terms were created in, and which
//! members each stored term carries.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, ok};

/// Create a glossary and return the identifier it minted.
async fn given_glossary(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateGlossary", &json!({ "Name": name })).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossary must mint an Id, got {created}"))
        .to_string()
}

/// Register one term in `glossary`, with the members the row asks for, and return the
/// `GlossaryTermId` the create minted.
async fn given_term(router: &MultiServiceRouter, glossary: &str, members: &Value) -> String {
    let mut body = members.clone();
    body["GlossaryIdentifier"] = json!(glossary);
    let created = ok(router, "CreateGlossaryTerm", &body).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossaryTerm must mint an Id, got {created}"))
        .to_string()
}

/// The entries of one answer, as a list.
fn items(page: &Value) -> Vec<Value> {
    page["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("Items must be a list, got {page}"))
        .clone()
}

/// The `Name` of every entry in one answer, in the order the service returned them.
fn listed_names(page: &Value) -> Vec<String> {
    items(page)
        .iter()
        .map(|item| {
            item["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("GlossaryTermItem.Name must be a string, got {item}"))
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out for one glossary, following its own `NextToken`,
/// and return the entries in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    glossary: &str,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "GlossaryIdentifier": glossary });
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListGlossaryTerms", &request).await;
        let entries = items(&page);
        if let Some(size) = page_size {
            assert!(
                entries.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {entries:?}"
            );
        }
        listed.extend(entries);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            listed.len() <= limit,
            "case {case:?}: the walk must terminate, got {listed:?} for {limit} terms"
        );
    }
    listed
}

#[tokio::test]
async fn every_term_of_the_glossary_is_listed_and_paged() {
    // (case, how many terms are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no terms at all", 0, None),
        ("one term, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page exactly the stored count", 4, Some(4)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 6, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        let glossary = given_glossary(&router, "finance").await;
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored).map(|index| format!("term-{index:02}")).collect();
        for name in expected.iter().rev() {
            given_term(&router, &glossary, &json!({ "Name": name })).await;
        }

        let listed = walk(&router, case, &glossary, *page_size, *stored).await;
        let names: Vec<String> = listed
            .iter()
            .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored term in name order, got {names:?}"
        );

        // A term the listing reports is one `GetGlossaryTerm` resolves, with the same
        // record: one record, two reads.
        for item in &listed {
            let id = item["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the entry must carry an Id, got {item}"));
            let read = ok(&router, "GetGlossaryTerm", &json!({ "Identifier": id })).await;
            assert_eq!(
                read.get("Name"),
                item.get("Name"),
                "case {case:?}: GetGlossaryTerm({id}) must report the listed term, got {read} for \
                 {item}"
            );
            assert_eq!(
                read.get("GlossaryId").and_then(Value::as_str),
                Some(glossary.as_str()),
                "case {case:?}: a listed term must belong to the glossary listed, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_term_carries_the_summary_members_the_create_stored() {
    let router = emulator();
    let glossary = given_glossary(&router, "metrics").await;

    // The rows differ along which members the stored term carries: the required name
    // alone, each description on its own, both together, and the shortest and longest
    // values the model accepts for a name (`GlossaryTermName`, len 1..256) and for a short
    // description (`GlossaryShortDescription`, len 1..1024).
    let rows: &[(&str, Value)] = &[
        ("the required member only", json!({ "Name": "aaa-bare" })),
        (
            "a short description only",
            json!({ "Name": "bbb-short", "ShortDescription": "Share of customers that left" }),
        ),
        (
            "a long description only",
            json!({ "Name": "ccc-long", "LongDescription": "Revenue kept from the cohort" }),
        ),
        (
            "both descriptions",
            json!({
                "Name": "ddd-both",
                "ShortDescription": "Revenue less cost of revenue",
                "LongDescription": "Revenue less the cost of delivering it",
            }),
        ),
        (
            "a one-character name and short description",
            json!({ "Name": "e", "ShortDescription": "s" }),
        ),
        (
            "values at the modelled length bounds",
            json!({
                "Name": format!("fff{}", "n".repeat(253)),
                "ShortDescription": "s".repeat(1024),
                "LongDescription": "l".repeat(4096),
            }),
        ),
    ];

    for (_, members) in rows {
        given_term(&router, &glossary, members).await;
    }

    let listed = walk(&router, "summary members", &glossary, None, rows.len()).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case summary members: expected {} terms, got {listed:?}",
        rows.len()
    );

    // The rows were created in name order, which is the order the listing reports.
    for ((case, members), item) in rows.iter().zip(listed.iter()) {
        assert_eq!(
            item.get("Name"),
            members.get("Name"),
            "case {case:?}: expected the stored name, got {item}"
        );
        assert_eq!(
            item.get("ShortDescription"),
            members.get("ShortDescription"),
            "case {case:?}: expected the stored short description, got {item}"
        );
        // `GlossaryTermItem` is a summary of three members (`smithy_query
        // shape=GlossaryTermItem`): the long description is not one of them, so it is
        // reported by `GetGlossaryTerm` and not by the listing.
        assert!(
            item.get("LongDescription").is_none(),
            "case {case:?}: a listed term must report no LongDescription, got {item}"
        );
        let id = item["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the entry must carry an Id, got {item}"));
        let read = ok(&router, "GetGlossaryTerm", &json!({ "Identifier": id })).await;
        assert_eq!(
            read.get("LongDescription"),
            members.get("LongDescription"),
            "case {case:?}: GetGlossaryTerm must report the stored long description, got {read}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_only_the_glossary_the_request_names() {
    let router = emulator();
    let finance = given_glossary(&router, "finance").await;
    let marketing = given_glossary(&router, "marketing").await;
    let empty = given_glossary(&router, "empty").await;

    // A term's name is unique WITHIN its glossary and free in another, so the same name is
    // stored in both: what separates the two listings is the glossary, not the name.
    for name in ["churn", "margin"] {
        given_term(&router, &finance, &json!({ "Name": name })).await;
    }
    for name in ["churn", "reach"] {
        given_term(&router, &marketing, &json!({ "Name": name })).await;
    }

    // (case, the identifier listed, the names it must report)
    let rows: &[(&str, &str, &[&str])] = &[
        ("the first glossary", finance.as_str(), &["churn", "margin"]),
        (
            "the second glossary",
            marketing.as_str(),
            &["churn", "reach"],
        ),
        ("a glossary holding no term", empty.as_str(), &[]),
        // `ListGlossaryTerms` models no not-found error (`smithy_query
        // operation=ListGlossaryTerms`), so an identifier no term is recorded against is a
        // listing of nothing rather than a refusal.
        ("an identifier nothing is stored under", "no-such-id", &[]),
    ];

    for (case, identifier, expected) in rows {
        let page = ok(
            &router,
            "ListGlossaryTerms",
            &json!({ "GlossaryIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected only the terms of {identifier}, got {page}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: a last segment must carry no NextToken, got {page}"
        );
    }

    // The scoping also holds for a PAGED walk, so the token resumes within one glossary
    // rather than over every stored term.
    let listed = walk(&router, "one term per page", &finance, Some(1), 4).await;
    let names: Vec<String> = listed
        .iter()
        .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["churn".to_string(), "margin".to_string()],
        "case one term per page: expected only the first glossary's terms, got {names:?}"
    );
}

#[tokio::test]
async fn the_listing_reports_what_the_store_holds_now() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let glossary = given_glossary(&router, "finance").await;
    let mut ids = Vec::new();
    for name in ["kept-one", "kept-two", "removed"] {
        ids.push(given_term(&router, &glossary, &json!({ "Name": name })).await);
    }

    let before = listed_names(&ok(
        &router,
        "ListGlossaryTerms",
        &json!({ "GlossaryIdentifier": glossary }),
    )
    .await);
    assert_eq!(
        before,
        vec![
            "kept-one".to_string(),
            "kept-two".to_string(),
            "removed".to_string()
        ],
        "case before the record is dropped: expected every stored term, got {before:?}"
    );

    // Listing is reporting only: two listings in a row agree, so a read changed nothing a
    // later caller sees.
    let again = listed_names(&ok(
        &router,
        "ListGlossaryTerms",
        &json!({ "GlossaryIdentifier": glossary }),
    )
    .await);
    assert_eq!(
        again, before,
        "case listed twice: a listing must change nothing, got {again:?} then {before:?}"
    );

    // `DeleteGlossaryTerm` is still a scaffold, so the record is dropped from the store the
    // listing reads rather than through that operation.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let removed = ids.last().expect("three terms were created").clone();
        state.lock().glossary_terms.remove(&removed);
    }

    let after = listed_names(&ok(
        &router,
        "ListGlossaryTerms",
        &json!({ "GlossaryIdentifier": glossary }),
    )
    .await);
    assert_eq!(
        after,
        vec!["kept-one".to_string(), "kept-two".to_string()],
        "case after the record is dropped: a term the store no longer holds must not be listed, \
         got {after:?}"
    );

    // A term keeps the glossary it was created in even once that glossary is gone — which
    // is what `GetGlossaryTerm` reports too — so the orphaned records are still the listing
    // of that identifier. `DeleteGlossary` is a scaffold, so the glossary is dropped from
    // the store directly.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().glossaries.remove(&glossary);
    }
    let orphaned = listed_names(&ok(
        &router,
        "ListGlossaryTerms",
        &json!({ "GlossaryIdentifier": glossary }),
    )
    .await);
    assert_eq!(
        orphaned, after,
        "case the glossary is gone: its terms must still be listed, got {orphaned:?} then \
         {after:?}"
    );
}

#[tokio::test]
async fn a_page_resumes_at_the_entry_after_the_token() {
    let router = emulator();
    let glossary = given_glossary(&router, "finance").await;
    let names = ["alpha", "bravo", "charlie", "delta"];
    for name in names {
        given_term(&router, &glossary, &json!({ "Name": name })).await;
    }

    // (case, the page size, the names of the first page, the token it must carry)
    let rows: &[(&str, i32, &[&str], Option<&str>)] = &[
        ("one at a time", 1, &["alpha"], Some("alpha")),
        ("two at a time", 2, &["alpha", "bravo"], Some("bravo")),
        (
            "three at a time",
            3,
            &["alpha", "bravo", "charlie"],
            Some("charlie"),
        ),
        (
            "every term in one page",
            4,
            &["alpha", "bravo", "charlie", "delta"],
            None,
        ),
    ];

    for (case, page_size, first, token) in rows {
        let page = ok(
            &router,
            "ListGlossaryTerms",
            &json!({ "GlossaryIdentifier": glossary, "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            first
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the first {page_size} terms in name order, got {page}"
        );
        assert_eq!(
            page.get("NextToken").and_then(Value::as_str),
            *token,
            "case {case:?}: expected the token {token:?}, got {page}"
        );

        // Continuing from the token reports the REST of the terms, each exactly once.
        if let Some(cursor) = token {
            let next = ok(
                &router,
                "ListGlossaryTerms",
                &json!({
                    "GlossaryIdentifier": glossary,
                    "NextToken": cursor,
                }),
            )
            .await;
            let rest: Vec<String> = names
                .iter()
                .filter(|name| !first.contains(name))
                .map(|name| (*name).to_string())
                .collect();
            assert_eq!(
                listed_names(&next),
                rest,
                "case {case:?}: expected the terms after {cursor}, got {next}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    let glossary = given_glossary(&router, "finance").await;
    given_term(&router, &glossary, &json!({ "Name": "bounded" })).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused
    // by the modelled constraint rather than served as some other page size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(
            &router,
            "ListGlossaryTerms",
            &json!({ "GlossaryIdentifier": glossary, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }

    // Each published bound itself is served.
    for (case, max_results) in [("the minimum", 1), ("the maximum", 1_000)] {
        let page = ok(
            &router,
            "ListGlossaryTerms",
            &json!({ "GlossaryIdentifier": glossary, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed_names(&page),
            vec!["bounded".to_string()],
            "case {case:?}: MaxResults={max_results} must be served, got {page}"
        );
    }
}
