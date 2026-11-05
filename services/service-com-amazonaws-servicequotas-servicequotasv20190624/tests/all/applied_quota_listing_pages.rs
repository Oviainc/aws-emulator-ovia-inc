// SPDX-License-Identifier: CC0-1.0

//! The page `ListServiceQuotas` serves for each narrowing a caller may ask the applied
//! listing for.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.ListServiceQuotas` dispatched through the same
//! `MultiServiceRouter` the emulator serves with, over ONE shared engine, so what is
//! exercised is the wire behaviour.
//!
//! The RULE, asserted over a TABLE rather than over one recorded request: every page of
//! this listing is BOUNDED — by the caller's `MaxResults` (`@range 1..100`) where it
//! names one, else by this listing's own unasked page size
//! (`crate::state::APPLIED_QUOTA_PAGE_SIZE`) — and a `NextToken` follows a page exactly
//! when the narrowed set has records past it, so chaining the token walks the whole
//! selected set, in the published order, reporting every record exactly once and none
//! twice.
//!
//! `unasked_page_size.rs` pins the sibling rule for the listings whose unasked answer is
//! their whole set; what this suite adds is the dimensions `ListServiceQuotas` narrows
//! its OWN set along, each of which changes how long the set is: the `ServiceCode` (a
//! handful of quotas, dozens, hundreds), the `QuotaAppliedAtLevel` plane (`ACCOUNT`,
//! `RESOURCE`, `ALL` — the last reporting one record per quota PLUS one per resource
//! context a value was applied to, so it is the longest), and the `QuotaCode` filter that
//! cuts the set to one quota.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    APPLIED_QUOTA_PAGE_SIZE, CONTEXT_ID_EVERY_RESOURCE, LEVEL_ACCOUNT, LEVEL_ALL, LEVEL_RESOURCE,
    MAX_PAGE_SIZE,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// The operation under test.
const OPERATION: &str = "ListServiceQuotas";

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has been decided afterwards and its value is in force.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// Service codes whose applied-quota sets span the lengths the rule generalises over:
/// `iam` publishes a handful of quotas, `acm` and `lambda` dozens, `ec2` hundreds —
/// more than the largest page a caller may ask for.
const QUOTA_SERVICES: &[&str] = &["iam", "acm", "lambda", "ec2"];

/// The service and quota the catalog publishes as adjustable PER RESOURCE, so a
/// resource plane can be applied to and listed (`src/state.rs`, the `es` rows).
const RESOURCE_SERVICE: &str = "es";
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";

/// How many distinct resource contexts a value is applied to: more than the handful a
/// single page once held, so a `RESOURCE` plane that is cut short at a fixed page size
/// fails here.
const SEEDED_CONTEXTS: usize = 12;

/// One row of the table: a narrowing of the applied listing, and the levels its records
/// may report.
struct Row {
    /// What names the case in every assertion.
    case: String,
    /// The request body, carrying no `MaxResults` and no `NextToken`.
    body: Value,
    /// The `QuotaAppliedAtLevel` values a record of this narrowing may carry.
    levels: Vec<&'static str>,
}

/// A domain ARN of this account, as a `ContextId` naming one resource.
fn domain(name: &str) -> String {
    format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/{name}")
}

/// The items a response carries under `Quotas`.
fn quotas(case: &str, response: &Value) -> Vec<Value> {
    response["Quotas"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Quotas list, got {response}"))
        .clone()
}

/// Apply a value to [`SEEDED_CONTEXTS`] distinct resources of one quota, plus the
/// every-resource context, and return that quota's code.
async fn seed_resource_planes(caller: &Client) -> String {
    let quota_code = quota_code_for(caller, RESOURCE_SERVICE, RESOURCE_QUOTA_NAME).await;
    let in_force = caller
        .ok(
            "GetServiceQuota",
            json!({"ServiceCode": RESOURCE_SERVICE, "QuotaCode": quota_code}),
        )
        .await["Quota"]["Value"]
        .as_f64()
        .unwrap_or_else(|| panic!("case seeding: {RESOURCE_QUOTA_NAME} must report a Value"));

    let mut contexts: Vec<String> = (0..SEEDED_CONTEXTS).map(|n| domain(&format!("d{n}"))).collect();
    contexts.push(CONTEXT_ID_EVERY_RESOURCE.to_string());
    for (step, context) in contexts.iter().enumerate() {
        caller
            .ok(
                "RequestServiceQuotaIncrease",
                json!({
                    "ServiceCode": RESOURCE_SERVICE,
                    "QuotaCode": quota_code,
                    "DesiredValue": in_force + 1.0 + step as f64,
                    "ContextId": context,
                }),
            )
            .await;
    }
    // Every decision the clock owes comes due, so each context has a value applied.
    tokio::time::sleep(PAST_DECISION).await;

    quota_code
}

/// The whole table: the narrowings of the applied listing, over the set lengths and
/// planes the rule generalises over.
async fn rows(caller: &Client) -> Vec<Row> {
    let resource_quota = seed_resource_planes(caller).await;
    let mut rows: Vec<Row> = Vec::new();

    // The default plane, over services whose quota sets run from a handful to hundreds.
    for service_code in QUOTA_SERVICES {
        rows.push(Row {
            case: format!("{service_code}, no narrowing"),
            body: json!({"ServiceCode": service_code}),
            levels: vec![LEVEL_ACCOUNT],
        });
    }

    // The three planes of the service a value was applied per resource for: ACCOUNT is
    // one record per quota, RESOURCE one per applied context, ALL both.
    for (level, levels) in [
        (LEVEL_ACCOUNT, vec![LEVEL_ACCOUNT]),
        (LEVEL_RESOURCE, vec![LEVEL_RESOURCE]),
        (LEVEL_ALL, vec![LEVEL_ACCOUNT, LEVEL_RESOURCE]),
    ] {
        rows.push(Row {
            case: format!("{RESOURCE_SERVICE}, QuotaAppliedAtLevel={level}"),
            body: json!({"ServiceCode": RESOURCE_SERVICE, "QuotaAppliedAtLevel": level}),
            levels,
        });
    }

    // The same planes narrowed to the ONE quota the contexts were applied to.
    for (level, levels) in [
        (LEVEL_RESOURCE, vec![LEVEL_RESOURCE]),
        (LEVEL_ALL, vec![LEVEL_ACCOUNT, LEVEL_RESOURCE]),
    ] {
        rows.push(Row {
            case: format!("{RESOURCE_SERVICE}/{resource_quota}, QuotaAppliedAtLevel={level}"),
            body: json!({
                "ServiceCode": RESOURCE_SERVICE,
                "QuotaCode": resource_quota,
                "QuotaAppliedAtLevel": level,
            }),
            levels,
        });
    }

    // A plane nothing was applied to: the empty set is a whole set too, and it is the
    // last page.
    rows.push(Row {
        case: format!("lambda, QuotaAppliedAtLevel={LEVEL_RESOURCE} with nothing applied"),
        body: json!({"ServiceCode": "lambda", "QuotaAppliedAtLevel": LEVEL_RESOURCE}),
        levels: vec![LEVEL_RESOURCE],
    });

    // The `QuotaCode` filter: one quota of a long listing, so the narrowing and not the
    // page size is what cuts the set down.
    let single = quota_code_for(caller, "lambda", "Concurrent executions").await;
    rows.push(Row {
        case: format!("lambda/{single}, one quota"),
        body: json!({"ServiceCode": "lambda", "QuotaCode": single}),
        levels: vec![LEVEL_ACCOUNT],
    });

    rows
}

/// RULE: an applied-quota listing called with no `MaxResults` serves this listing's own
/// unasked page — the first records of the narrowed set, bounded by
/// `APPLIED_QUOTA_PAGE_SIZE` — followed by a `NextToken` exactly while that set has
/// records past the page, and chaining that token yields the whole set once over.
#[tokio::test]
async fn an_unasked_applied_listing_serves_a_bounded_page_and_a_token_while_records_remain() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");
    let rows = rows(&caller).await;

    let mut empty = 0_usize;
    let mut past_a_handful = 0_usize;
    let mut past_the_largest_asked_page = 0_usize;

    for Row { case, body, levels } in &rows {
        // The set this narrowing publishes, as the token chain yields it at the largest
        // page size a caller may ask for: the reference every assertion is against.
        let whole = walk_pages(
            &caller,
            OPERATION,
            body,
            "Quotas",
            Some(MAX_PAGE_SIZE as i64),
        )
        .await;

        let unasked = caller.ok(OPERATION, body.clone()).await;
        let page = quotas(case, &unasked);
        let served = APPLIED_QUOTA_PAGE_SIZE.min(whole.len());
        assert_eq!(
            page.len(),
            served,
            "case {case}: an unasked page holds the first {served} of the {} records \
             selected, got {}",
            whole.len(),
            page.len()
        );
        assert_eq!(
            page,
            whole[..served],
            "case {case}: the unasked page must be the first records of the selected order"
        );
        assert_eq!(
            !unasked["NextToken"].as_str().unwrap_or_default().is_empty(),
            served < whole.len(),
            "case {case}: a NextToken follows this page exactly while records remain ({served} \
             of {} served), got {unasked}",
            whole.len()
        );

        // Chaining that token — every continuation naming no page size either — walks
        // the selected set exactly once over, in the published order.
        let walked = walk_pages(&caller, OPERATION, body, "Quotas", None).await;
        assert_eq!(
            walked, whole,
            "case {case}: the unasked token chain must yield the selected set exactly once"
        );

        // Every record belongs to the plane the narrowing asked for, so "the whole set"
        // is the whole set of THAT plane and not of another.
        for record in &page {
            let level = record["QuotaAppliedAtLevel"].as_str().unwrap_or_default();
            assert!(
                levels.contains(&level),
                "case {case}: every record reports a level in {levels:?}, got {record}"
            );
        }

        // Reading it again serves the same body: a listing reports, it does not consume.
        let again = caller.ok(OPERATION, body.clone()).await;
        assert_eq!(
            quotas(case, &again),
            page,
            "case {case}: a second unasked read must serve the same set"
        );

        if whole.is_empty() {
            empty += 1;
        }
        if whole.len() > 10 {
            past_a_handful += 1;
        }
        if whole.len() > MAX_PAGE_SIZE {
            past_the_largest_asked_page += 1;
        }
    }

    // The table must span the lengths the rule generalises over: an empty set, sets
    // past a handful of records (so past the unasked page, which is what makes the
    // token mandatory), and a set past the largest page a caller may ask for.
    assert!(
        empty == 1 && past_a_handful >= 4 && past_the_largest_asked_page >= 1,
        "case table: expected rows spanning an empty set, sets past a handful and one past \
         {MAX_PAGE_SIZE} records, got {empty}, {past_a_handful} and \
         {past_the_largest_asked_page}"
    );
}

/// RULE, the other half: a caller that DOES name `MaxResults` gets exactly that page
/// size out of the same narrowed set, with a `NextToken` while records remain that
/// resumes where the page stopped — and a continuation naming no page size is bounded by
/// this listing's own unasked page.
#[tokio::test]
async fn an_asked_page_size_cuts_the_same_narrowed_set() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");
    let rows = rows(&caller).await;

    for Row { case, body, .. } in &rows {
        let whole = walk_pages(
            &caller,
            OPERATION,
            body,
            "Quotas",
            Some(MAX_PAGE_SIZE as i64),
        )
        .await;
        if whole.is_empty() {
            // An empty set has no page to cut short; its last-page shape is asserted by
            // the unasked suite.
            continue;
        }

        // Both published bounds of the modelled `MaxResults` @range 1..100, and two
        // sizes between them that divide none of these sets evenly.
        for size in [1_usize, 5, 11, MAX_PAGE_SIZE] {
            let case = format!("{case} MaxResults={size}");
            let asked = caller
                .ok(OPERATION, {
                    let mut asked = body.clone();
                    asked["MaxResults"] = json!(size);
                    asked
                })
                .await;
            let page = quotas(&case, &asked);
            let taken = size.min(whole.len());
            assert_eq!(
                page.len(),
                taken,
                "case {case}: an asked page holds the asked size, expected {taken}, got {}",
                page.len()
            );
            assert_eq!(
                page,
                whole[..taken],
                "case {case}: the first page must be the first records of the selected order"
            );

            let token = asked["NextToken"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                !token.is_empty(),
                taken < whole.len(),
                "case {case}: a NextToken follows this page exactly while records remain ({} of \
                 {} served), got {asked}",
                taken,
                whole.len()
            );
            if token.is_empty() {
                continue;
            }

            // The continuation names no page size, so this listing's own unasked page
            // bounds it the same way the first page is bounded — and it resumes exactly
            // where the asked page stopped.
            let resumed = caller
                .ok(OPERATION, {
                    let mut resume = body.clone();
                    resume["NextToken"] = json!(token);
                    resume
                })
                .await;
            let rest = quotas(&format!("{case} resumed"), &resumed);
            let remaining = whole.len() - taken;
            let served = APPLIED_QUOTA_PAGE_SIZE.min(remaining);
            assert_eq!(
                rest,
                whole[taken..taken + served],
                "case {case} resumed: an unasked continuation carries the next {served} of the \
                 {remaining} records left, got {}",
                rest.len()
            );
            assert_eq!(
                !resumed["NextToken"].as_str().unwrap_or_default().is_empty(),
                served < remaining,
                "case {case} resumed: a NextToken follows the continuation exactly while records \
                 remain, got {resumed}"
            );
        }
    }
}
