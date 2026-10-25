// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ordering invariant `ListRules`' cursor paging rests on.
//!
//! `ListRules` walks the rule map in key order and hands back the last identifier it
//! reported as an exclusive cursor, so the identifier a rule is minted with has to sort
//! — under the BYTE comparison the map uses — in the same order the rules were created
//! in. Two observable consequences are asserted here, over a table of collection sizes
//! that spans every point at which a rendered identifier changes digit class (the first
//! letter, the first letter of the second letter range, and the rollover into a second
//! significant digit):
//!
//! 1. a full walk reports the rules of a resource type in creation order, whatever the
//!    page size, and the reported identifiers ascend;
//! 2. a rule created AFTER a page was handed back sorts after that page's cursor, so the
//!    resumed walk still reports it.
//!
//! Every case drives the service through the real router, exactly as a signed SDK
//! request would, and each row states its own case so a failure names the input.

use serde_json::{json, Value};

use crate::retention_rule_lifecycle::{create_rule, list_of, list_rules, retention, router};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{decode_page_token, is_rule_identifier};
use smithy_server_runtime::MultiServiceRouter;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// Collection sizes the ordering table spans. The identifier of the 10th rule is the
/// first to carry a letter, the 36th the first to carry one from the second letter range,
/// and the 62nd the first to need a second significant digit — the three points an
/// ordering defect in the rendering shows at — so each size crosses one of them.
const COLLECTION_SIZES: [usize; 5] = [1, 2, 11, 37, 63];

/// More pages than any row below can need, so a walk that fails to terminate fails the
/// case instead of hanging.
const MAX_PAGES: usize = 128;

/// Create `total` rules, cycling through `types` so a walk of one type has to skip the
/// rules of another, and return the identifiers it minted, in creation order, with the
/// type each one carries.
async fn create_rules(
    router: &MultiServiceRouter,
    total: usize,
    types: &[&'static str],
    case: &str,
) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for nth in 0..total {
        let resource_type = types[nth % types.len()];
        let (status, body) = create_rule(
            router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(1 + (nth as i64 % 3650)),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule #{nth}, got {status} / {body:?}"
        );
        let identifier = body["Identifier"].as_str().unwrap_or_default().to_string();
        assert!(
            is_rule_identifier(&identifier),
            "case {case:?}: CreateRule #{nth} minted {identifier:?}, which is not a \
             ^[0-9a-zA-Z]{{11}}$ rule identifier"
        );
        created.push((resource_type, identifier));
    }
    created
}

/// The identifiers of `resource_type`, in creation order.
fn of_type(created: &[(&'static str, String)], resource_type: &str) -> Vec<String> {
    created
        .iter()
        .filter(|(held, _)| *held == resource_type)
        .map(|(_, identifier)| identifier.clone())
        .collect()
}

/// One `ListRules` page: the identifiers it reported and the cursor it handed back.
async fn page(
    router: &MultiServiceRouter,
    resource_type: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let mut request = json!({ "ResourceType": resource_type });
    if let Some(size) = max_results {
        request["MaxResults"] = Value::from(size);
    }
    if let Some(token) = next_token {
        request["NextToken"] = Value::String(token.to_string());
    }
    let (status, body) = list_rules(router, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListRules, got {status} / {body:?}"
    );
    let reported = list_of(&body, "Rules")
        .iter()
        .map(|summary| summary["Identifier"].as_str().unwrap_or_default().to_string())
        .collect();
    let token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (reported, token)
}

/// Walk every page of `resource_type` and return the identifiers in reported order.
async fn walk(
    router: &MultiServiceRouter,
    resource_type: &str,
    max_results: Option<usize>,
    case: &str,
) -> Vec<String> {
    let mut reported = Vec::new();
    let mut token: Option<String> = None;
    for pages in 0.. {
        assert!(
            pages <= MAX_PAGES,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        let (ids, next) = page(router, resource_type, max_results, token.as_deref(), case).await;
        reported.extend(ids);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    reported
}

/// Assert `identifiers` ascend strictly as strings, which is how the rule map orders its
/// keys: creation order and key order have to be the same order.
fn assert_strictly_ascending(identifiers: &[String], case: &str) {
    for pair in identifiers.windows(2) {
        assert!(
            pair[0] < pair[1],
            "case {case:?}: identifiers must ascend in creation order, got {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[tokio::test]
async fn list_rules_reports_rules_in_creation_order_for_every_page_size() {
    for total in COLLECTION_SIZES {
        let router = router();
        let case = format!("{total} rules");
        let created = create_rules(&router, total, &RESOURCE_TYPES, &case).await;

        for resource_type in RESOURCE_TYPES {
            let expected = of_type(&created, resource_type);
            // Unbounded, one rule per page, and every page size around the collection.
            let mut page_sizes: Vec<Option<usize>> = vec![None];
            for size in [1usize, 2, 5, total, total + 1] {
                if !page_sizes.contains(&Some(size)) {
                    page_sizes.push(Some(size));
                }
            }
            for max_results in page_sizes {
                let case = format!(
                    "{total} rules, {resource_type}, MaxResults={}",
                    max_results.map_or("unset".to_string(), |size| size.to_string())
                );
                let reported = walk(&router, resource_type, max_results, &case).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: ListRules must report the rules of a type in creation \
                     order, got {reported:?}"
                );
                assert_strictly_ascending(&reported, &case);
            }
        }
    }
}

#[tokio::test]
async fn a_rule_created_after_a_page_is_reported_by_the_resumed_walk() {
    // Each row pages a collection that crosses one of the digit-class boundaries above,
    // with a page size small enough that the first page hands back a cursor.
    let rows = [(3usize, 1usize), (12, 5), (37, 10), (63, 30)];
    for (total, page_size) in rows {
        let case = format!("{total} rules, MaxResults={page_size}, one created mid-walk");
        let router = router();
        let resource_type = RESOURCE_TYPES[0];
        let created = create_rules(&router, total, &[resource_type], &case).await;
        let mut expected = of_type(&created, resource_type);

        let (first, token) = page(
            &router,
            resource_type,
            Some(page_size),
            None,
            &case,
        )
        .await;
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: expected a NextToken after the first page of {page_size}")
        });
        let cursor = decode_page_token(&token).unwrap_or_else(|| {
            panic!("case {case:?}: ListRules handed back a token it cannot decode: {token:?}")
        });
        assert_eq!(
            Some(&cursor),
            first.last(),
            "case {case:?}: the cursor must name the last rule the page reported, got {first:?}"
        );

        // A rule created between two pages: minted after the cursor, so it has to sort
        // after it and be reported by the rest of the walk.
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(99),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from the mid-walk CreateRule, got {status} / {body:?}"
        );
        let late = body["Identifier"].as_str().unwrap_or_default().to_string();
        assert!(
            late > cursor,
            "case {case:?}: a rule created after a page must sort after that page's \
             cursor {cursor:?}, got {late:?}"
        );
        expected.push(late.clone());

        let mut reported = first;
        let mut token = Some(token);
        for pages in 0.. {
            assert!(
                pages <= total + 2,
                "case {case:?}: paging did not terminate after {pages} resumed pages"
            );
            let Some(cursor) = token else { break };
            let (ids, next) = page(
                &router,
                resource_type,
                Some(page_size),
                Some(&cursor),
                &case,
            )
            .await;
            reported.extend(ids);
            token = next;
        }

        assert!(
            reported.contains(&late),
            "case {case:?}: the rule created mid-walk ({late:?}) must still be reported, \
             got {reported:?}"
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: the whole walk must report every rule once, in creation order, \
             got {reported:?}"
        );
        assert_strictly_ascending(&reported, &case);
    }
}
