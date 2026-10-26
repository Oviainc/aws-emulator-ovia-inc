// SPDX-License-Identifier: CC0-1.0

//! What `ListWorkflows` does with a pagination cursor that was issued at ANOTHER
//! regional endpoint.
//!
//! RULE under test: a `NextToken` is a position in the one walk it was issued for —
//! the list, and the partition that walk is over, which for this listing is the
//! regional endpoint serving it — so the listing accepts a cursor back only at the
//! endpoint that issued it and refuses every other with the modelled
//! `ValidationException` (HTTP 400, no page), instead of applying a position taken
//! from another partition, which silently skips or re-serves workflows.
//!
//! The table spans what the rule generalises over: which endpoint issued the cursor
//! and which endpoint is presented it, in both directions of ARN order (a
//! `WorkflowArn` embeds its region, so one region's ARNs sort wholly before
//! another's); how many workflows the endpoint presented the cursor holds next to the
//! one that issued it (more, as many, fewer, none at all); and the page size at each
//! published `MaxResults` bound. The control rows present each cursor back at the
//! endpoint that issued it, so the refusal is shown to be about the cursor's scope
//! rather than about carrying a cursor at all.
//!
//! Every test drives the real wire path against ONE shared engine, with the
//! credential-scope region as the only difference between two calls.

use crate::region_isolation::{create_at, listed_arns, REGION_POPULATIONS};
use crate::workflow_lifecycle::Harness;

/// The published bounds of `MaxResults` (`@range(min: 1, max: 100)`, `@default: 20`,
/// from `smithy_query(query="operation", operation="ListWorkflows")`), plus a size
/// between them: the page size must not change whether a foreign cursor is refused.
const PAGE_SIZES: &[(&str, u64)] = &[
    ("MaxResults at the minimum of 1", 1),
    ("MaxResults of 2", 2),
    ("MaxResults at the maximum of 100", 100),
];

/// The same names in every region, so a name is never what tells two endpoints'
/// workflows apart — only the endpoint they were created through does.
const NAMES: &[&str] = &["alpha", "beta.two", "gamma_3"];

/// Call `ListWorkflows` at `region`'s endpoint and report its status and body.
async fn list(
    harness: &Harness,
    region: &str,
    page_size: u64,
    token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"MaxResults": page_size});
    if let Some(cursor) = token {
        body["NextToken"] = serde_json::Value::String(cursor.to_string());
    }
    harness.call_in(region, "ListWorkflows", body).await
}

/// The `WorkflowArn`s one page reports, in the order it reported them.
fn arns(page: &serde_json::Value) -> Vec<String> {
    page["Workflows"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["WorkflowArn"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One endpoint in the table: its region, the ARNs it holds in listing (ARN) order,
/// and what this row varies.
struct Endpoint {
    region: &'static str,
    arns: Vec<String>,
}

/// Seed every endpoint of the table into ONE store: each region creates the first
/// `population` of the shared names through its own endpoint.
async fn endpoints(harness: &Harness) -> Vec<Endpoint> {
    let mut rows = Vec::new();
    for (region, population) in REGION_POPULATIONS {
        let mut arns = Vec::new();
        for name in NAMES.iter().take(*population) {
            arns.push(create_at(harness, region, name).await);
        }
        arns.sort();
        rows.push(Endpoint { region, arns });
    }
    rows
}

/// A cursor every endpoint holding more than one workflow issues at a page size of
/// 1, so the walk it belongs to still has at least one workflow to come.
async fn issued_cursors(harness: &Harness, rows: &[Endpoint]) -> Vec<(&'static str, String)> {
    let mut cursors = Vec::new();
    for row in rows {
        if row.arns.len() < 2 {
            continue;
        }
        let (status, first) = list(harness, row.region, 1, None).await;
        assert_eq!(
            status, 200,
            "setup {:?}: expected the first page to succeed, got {status} {first:?}",
            row.region
        );
        let token = first["NextToken"].as_str().unwrap_or_else(|| {
            panic!(
                "setup {:?}: {} workflows at MaxResults 1 must leave a cursor, got {first:?}",
                row.region,
                row.arns.len()
            )
        });
        cursors.push((row.region, token.to_string()));
    }
    assert!(
        cursors.len() >= 2,
        "setup: the table needs cursors from at least two endpoints to travel between, got {}",
        cursors.len()
    );
    cursors
}

#[tokio::test]
async fn a_cursor_issued_at_another_regional_endpoint_is_refused() {
    // ONE store, reached through every endpoint of the table.
    let harness = Harness::new(REGION_POPULATIONS[0].0);
    let rows = endpoints(&harness).await;
    let cursors = issued_cursors(&harness, &rows).await;

    for (issuer, token) in &cursors {
        for target in &rows {
            if target.region == *issuer {
                continue;
            }
            for (size_label, size) in PAGE_SIZES {
                let case = format!(
                    "cursor issued at {issuer:?} presented at {:?} holding {} workflows/\
                     {size_label}",
                    target.region,
                    target.arns.len()
                );
                let (status, answered) = list(&harness, target.region, *size, Some(token)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for a cursor issued at another endpoint, \
                     got {status} {answered:?}"
                );
                assert!(
                    answered["__type"]
                        .as_str()
                        .is_some_and(|kind| kind.contains("ValidationException")),
                    "case {case:?}: expected a ValidationException, got {:?}",
                    answered["__type"]
                );
                assert!(
                    answered["Workflows"].is_null(),
                    "case {case:?}: a refusal reports no page, got {:?}",
                    answered["Workflows"]
                );
            }
        }
    }

    // The refusals changed nothing a later caller sees: every endpoint still lists
    // exactly the workflows created through it.
    for row in &rows {
        let mut reported = listed_arns(&harness, row.region, 100).await;
        reported.sort();
        assert_eq!(
            reported, row.arns,
            "case {:?}: a refused cursor must not change what the endpoint serves, got \
             {reported:?}",
            row.region
        );
    }
}

#[tokio::test]
async fn a_cursor_presented_at_the_endpoint_that_issued_it_continues_that_walk() {
    let harness = Harness::new(REGION_POPULATIONS[0].0);
    let rows = endpoints(&harness).await;

    for row in &rows {
        for (size_label, size) in PAGE_SIZES {
            // The walk, taken through the cursors this endpoint itself issues: every
            // cursor accepted, and the walk covering that endpoint's workflows exactly
            // once rather than restarting or skipping.
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // One round trip more than the store can need, so a walk that never stops
            // fails the case instead of hanging it.
            for step in 0..=row.arns.len() + 1 {
                let case = format!("{:?}/{size_label}/step {step}", row.region);
                let (status, page) = list(&harness, row.region, *size, token.as_deref()).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected its own cursor to be accepted, got {status} {page:?}"
                );
                let page_arns = arns(&page);
                assert!(
                    page_arns.len() as u64 <= *size,
                    "case {case:?}: a page may carry at most MaxResults items, got {page_arns:?}"
                );
                assert!(
                    page_arns.iter().all(|arn| !seen.contains(arn)),
                    "case {case:?}: the walk must not repeat a workflow, got {page_arns:?} after \
                     {seen:?}"
                );
                seen.extend(page_arns);
                token = page["NextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert!(
                token.is_none(),
                "case {:?}/{size_label}: the walk must terminate, got a further cursor after \
                 {seen:?}",
                row.region
            );
            seen.sort();
            assert_eq!(
                seen, row.arns,
                "case {:?}/{size_label}: paging must cover exactly that endpoint's workflows, \
                 got {seen:?}",
                row.region
            );
        }
    }
}
