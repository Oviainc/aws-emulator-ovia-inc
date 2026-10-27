// SPDX-License-Identifier: CC0-1.0

//! The ONE page-size rule every paginated listing of this service applies.
//!
//! The rule: a request that names no `maxResults` is served a page of
//! [`DEFAULT_MAX_RESULTS`] items, a request that names one is served at most
//! that many, and the size is never zero — so every page advances and every walk
//! terminates. Nothing is bounded ABOVE where the page is sized: each
//! operation's own `maxResults` range is what the protocol boundary enforces, so
//! `ListAccounts` (modelled `range(min: 1, max: 1000)`) serves a page larger
//! than the default, while an operation modelled `range(min: 1, max: 100)` can
//! never be asked for one.
//!
//! Every paginated operation of this service is a row below, and each row is
//! driven the way a client drives it — an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts — so a listing that sized
//! its own page differently from its siblings fails here. The rows differ in the
//! number of items behind them (none, two, a handful, more than the default) and
//! in the page size asked for (none, one, the default, and — where the model
//! admits it — more than the default).
//!
//! The inventory the listings report over is Compute Optimizer's own analysis
//! output and the enrollment of other accounts, neither of which an operation of
//! this API mints, so both are seeded through the writers the sibling suites
//! own; the automation rules are created over the wire.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RecommendedAction;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, StoredStep, DEFAULT_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_association::seed_enrollment;
use crate::automation_events::{seed as seed_events, stored_event};
use crate::automation_rule_lifecycle::{
    call_ok, emulator_with_engine, is_next_token, minimal_rule,
};
use crate::recommended_actions::{record, stored as stored_action};

/// The two `RecommendedActionType` values the model declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`). The seeded
/// inventory alternates between them, so the listings that GROUP by the type
/// report more than one group.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// Two terminal `EventStatus` values (`smithy_query(query="shape",shape="EventStatus")`).
/// Seeded events carry a terminal status so the clock owes them no transition
/// and the number of groups a summary reports cannot move under the cases below.
const EVENT_STATUS_COMPLETE: &str = "Complete";
const EVENT_STATUS_FAILED: &str = "Failed";

/// How many member accounts the fixture enrolls: more than the default page and
/// more than the largest page any case below asks for, so `ListAccounts` — the
/// one operation whose `maxResults` reaches past the default — has something to
/// serve on a bigger page.
const MEMBER_ACCOUNTS: usize = 200;

/// How many recommended actions the fixture records: more than the default page.
const ACTIONS: usize = 150;

/// How many automation events the fixture seeds: more than the default page.
const EVENTS: usize = 120;

/// How many automation rules the fixture creates: fewer than the default page,
/// so a listing whose items do not fill a page is a row as well.
const RULES: usize = 3;

/// The page size the totals are walked at: neither the default nor one, so a
/// walk that produced the totals cannot be the very computation under test.
const WALK_PAGE_SIZE: i32 = 7;

/// A ceiling on the pages of one walk, so a non-advancing token fails the case
/// instead of hanging it.
const MAX_PAGES: usize = 1_000;

/// The default page size as a count.
fn default_page_size() -> usize {
    usize::try_from(DEFAULT_MAX_RESULTS).expect("the default page size is a positive count")
}

/// `n` distinct member account ids in the modelled `AccountId` form
/// (`^[0-9]{12}$`), none of them the calling account.
fn member_account_ids(n: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("{:012}", 800_000_000_000u64 + index as u64))
        .collect()
}

/// `n` recommended actions in the calling account, alternating between the two
/// modelled action types.
fn inventory(n: usize) -> Vec<RecommendedAction> {
    (0..n)
        .map(|index| {
            stored_action(
                index,
                &format!("vol-{index:06}"),
                engine::EMULATOR_ACCOUNT_ID,
                "us-east-1",
                14,
                if index % 2 == 0 {
                    UPGRADE
                } else {
                    SNAPSHOT_AND_DELETE
                },
                Some("gp2"),
                false,
                &[("env", "prod")],
            )
        })
        .collect()
}

/// One step of a seeded event, already terminal.
fn step(index: usize, step_type: &str) -> StoredStep {
    StoredStep {
        step_id: format!("stp{index:013}"),
        step_type: step_type.to_string(),
        step_status: state::STEP_STATUS_READY.to_string(),
        resource_id: Some(format!("vol-{index:08}")),
        start_timestamp: None,
        completed_timestamp: None,
        estimated_monthly_savings: None,
    }
}

/// One paginated listing: the operation, the request that selects everything it
/// reports, the output member its page lands in, and whether the fixture leaves
/// it more items than one default page holds.
struct Listing {
    operation: &'static str,
    request: Value,
    items: &'static str,
    exceeds_the_default: bool,
}

/// The request both preview operations report the whole seeded inventory for:
/// every account of the organization and both action types, constrained by no
/// criteria.
fn preview_request() -> Value {
    json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    })
}

/// Every paginated operation of this service, with what the fixture leaves it to
/// report.
fn listings(event_id: &str) -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListAccounts",
            request: json!({}),
            items: "accounts",
            exceeds_the_default: true,
        },
        Listing {
            operation: "ListAutomationRules",
            request: json!({}),
            items: "automationRules",
            exceeds_the_default: false,
        },
        Listing {
            operation: "ListRecommendedActions",
            request: json!({}),
            items: "recommendedActions",
            exceeds_the_default: true,
        },
        Listing {
            operation: "ListRecommendedActionSummaries",
            request: json!({}),
            items: "recommendedActionSummaries",
            exceeds_the_default: false,
        },
        Listing {
            operation: "ListAutomationRulePreview",
            request: preview_request(),
            items: "previewResults",
            exceeds_the_default: true,
        },
        Listing {
            operation: "ListAutomationRulePreviewSummaries",
            request: preview_request(),
            items: "previewResultSummaries",
            exceeds_the_default: false,
        },
        Listing {
            operation: "ListAutomationEvents",
            request: json!({}),
            items: "automationEvents",
            exceeds_the_default: true,
        },
        Listing {
            operation: "ListAutomationEventSummaries",
            request: json!({}),
            items: "automationEventSummaries",
            exceeds_the_default: false,
        },
        Listing {
            operation: "ListAutomationEventSteps",
            request: json!({"eventId": event_id}),
            items: "automationEventSteps",
            exceeds_the_default: false,
        },
    ]
}

/// The items one response reports under `member_name`.
fn items_of(body: &Value, member_name: &str) -> Vec<Value> {
    body.get(member_name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `nextToken` one response carries, asserting its modelled form.
fn token_of(body: &Value, case: &str) -> Option<String> {
    let token = body.get("nextToken").and_then(Value::as_str)?;
    assert!(
        is_next_token(token),
        "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
    );
    Some(token.to_string())
}

/// How many items `listing` reports in total, walked at `page_size` — a size
/// that is neither the default nor one, so the total is known independently of
/// the pages the cases below assert about.
async fn walk_total(router: &MultiServiceRouter, listing: &Listing, page_size: i32) -> usize {
    let case = listing.operation;
    let limit = usize::try_from(page_size).expect("the walk page size is a positive count");
    let mut total = 0usize;
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = listing.request.clone();
        request["maxResults"] = json!(page_size);
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, listing.operation, &request, case).await;
        let items = items_of(&page, listing.items);
        assert!(
            items.len() <= limit,
            "case {case:?}: a page holds at most maxResults={page_size} items, got {}",
            items.len()
        );
        total += items.len();
        pages += 1;
        assert!(
            pages <= MAX_PAGES,
            "case {case:?}: the walk must terminate, got {total} items over {pages} pages"
        );
        token = token_of(&page, case);
        if token.is_none() {
            break;
        }
    }
    total
}

/// A router with every paginated listing of this service given something to
/// report: more accounts, recommended actions and automation events than one
/// default page holds, a handful of automation rules, and one event carrying
/// steps (whose id is handed back).
async fn emulator_with_a_fixture_spanning_the_default() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator_with_engine();

    for account_id in member_account_ids(MEMBER_ACCOUNTS) {
        seed_enrollment(
            &engine,
            &account_id,
            state::ENROLLED_STATUS,
            None,
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        );
    }

    record(&engine, inventory(ACTIONS));

    // Seeded events, each already terminal, created one minute apart inside the
    // retrieval window. The first one carries the steps the step listing reports.
    let created = crate::automation_events::now_secs();
    let mut events = Vec::with_capacity(EVENTS);
    for index in 0..EVENTS {
        let mut event = stored_event(
            index,
            engine::EMULATOR_ACCOUNT_ID,
            SNAPSHOT_AND_DELETE,
            if index % 2 == 0 {
                EVENT_STATUS_COMPLETE
            } else {
                EVENT_STATUS_FAILED
            },
            created - (index as i64) * 60,
        );
        if index == 0 {
            event.steps = vec![
                step(0, "CreateEbsSnapshot"),
                step(1, "DeleteEbsVolume"),
            ];
        }
        events.push(event);
    }
    let stepped_event_id = events[0].event_id.clone();
    seed_events(&engine, &events);

    for index in 0..RULES {
        call_ok(
            &router,
            "CreateAutomationRule",
            &minimal_rule(&format!("page-size-rule-{index}")),
            "seed-rules",
        )
        .await;
    }

    (router, stepped_event_id)
}

// ---------------------------------------------------------------------------
// The rule itself
// ---------------------------------------------------------------------------

/// The shared computation, over the whole span of what can reach it: no
/// `maxResults` takes the default, a size inside the modelled ranges is taken as
/// asked, a size past the default is NOT cut back (the operation whose model
/// admits one is served it), and a size that is zero or negative is floored at
/// one so a page always advances.
#[test]
fn the_shared_page_size_defaults_and_floors() {
    let default = default_page_size();
    // (case, the maxResults a request carried, the page size it gets)
    let cases: [(&str, Option<i32>, usize); 9] = [
        ("no-maxResults-takes-the-default", None, default),
        ("the-smallest-page-any-model-admits", Some(1), 1),
        ("inside-every-modelled-range", Some(7), 7),
        ("the-default-named-explicitly", Some(DEFAULT_MAX_RESULTS), default),
        ("past-the-default", Some(DEFAULT_MAX_RESULTS + 1), default + 1),
        // `ListAccounts` is modelled `range(min: 1, max: 1000)`, so its largest
        // page is ten times the default and must not be cut back to it.
        ("the-largest-page-this-service-models", Some(1_000), 1_000),
        ("zero-is-floored", Some(0), 1),
        ("a-negative-size-is-floored", Some(-1), 1),
        ("the-smallest-i32-is-floored", Some(i32::MIN), 1),
    ];

    for (case, max_results, expected) in cases {
        let actual = state::page_size(max_results);
        assert_eq!(
            actual, expected,
            "case {case:?}: maxResults={max_results:?} must size a page at {expected}, got \
             {actual}"
        );
    }
}

/// Every paginated operation of this service sizes its page by that one rule: a
/// request naming no `maxResults` is served `min(default, remaining)` items —
/// the same page as one naming the default explicitly — carries a token exactly
/// while items remain, and is served one item when it asks for one.
#[tokio::test]
async fn every_paginated_listing_sizes_its_page_by_the_shared_rule() {
    let (router, event_id) = emulator_with_a_fixture_spanning_the_default().await;
    let default = default_page_size();

    for listing in listings(&event_id) {
        let case = listing.operation;
        let total = walk_total(&router, &listing, WALK_PAGE_SIZE).await;
        assert_eq!(
            total > default,
            listing.exceeds_the_default,
            "case {case:?}: the fixture must leave this listing {} one default page, got {total} \
             items",
            if listing.exceeds_the_default {
                "more than"
            } else {
                "at most"
            }
        );

        // No `maxResults` at all: the default page.
        let defaulted = call_ok(&router, listing.operation, &listing.request, case).await;
        let served = items_of(&defaulted, listing.items).len();
        assert_eq!(
            served,
            total.min(default),
            "case {case:?}: a request naming no maxResults must be served min({default}, {total}) \
             items, got {served}"
        );
        assert_eq!(
            token_of(&defaulted, case).is_some(),
            total > served,
            "case {case:?}: a token belongs on the page exactly while items remain, got \
             {defaulted}"
        );

        // The default named explicitly: the same page, which is what makes the
        // constant the default rather than some other number.
        let mut explicit = listing.request.clone();
        explicit["maxResults"] = json!(DEFAULT_MAX_RESULTS);
        let explicit_page = call_ok(&router, listing.operation, &explicit, case).await;
        let explicit_served = items_of(&explicit_page, listing.items).len();
        assert_eq!(
            explicit_served, served,
            "case {case:?}: naming the default must serve the page a request naming nothing gets, \
             got {explicit_served} against {served}"
        );

        // The smallest page every model admits.
        let mut smallest = listing.request.clone();
        smallest["maxResults"] = json!(1);
        let smallest_page = call_ok(&router, listing.operation, &smallest, case).await;
        let smallest_served = items_of(&smallest_page, listing.items).len();
        assert_eq!(
            smallest_served,
            total.min(1),
            "case {case:?}: maxResults=1 must be served one item while any remain, got \
             {smallest_served}"
        );
    }
}

/// Where the model admits a page LARGER than the default, the listing serves it:
/// `ListAccountsRequest$maxResults` carries `range(min: 1, max: 1000)`
/// (`smithy_query(query="operation",operation="ListAccounts")`), so the page a
/// request asks for is never cut back to the default.
#[tokio::test]
async fn a_page_larger_than_the_default_is_served_where_the_model_admits_one() {
    let (router, event_id) = emulator_with_a_fixture_spanning_the_default().await;
    let listing = listings(&event_id)
        .into_iter()
        .find(|listing| listing.operation == "ListAccounts")
        .expect("ListAccounts is one of the paginated listings");
    let total = walk_total(&router, &listing, WALK_PAGE_SIZE).await;
    let default = default_page_size();
    assert!(
        total > default + 1,
        "case \"fixture\": this case needs more accounts than one default page, got {total}"
    );

    // (case, maxResults) — past the default, every stored account exactly, and
    // the published maximum.
    let sizes: [(&str, usize); 3] = [
        ("past-the-default", default + default / 2),
        ("every-stored-account", total),
        ("the-published-maximum", 1_000),
    ];

    for (case, max_results) in sizes {
        let page = call_ok(
            &router,
            "ListAccounts",
            &json!({"maxResults": max_results}),
            case,
        )
        .await;
        let served = items_of(&page, "accounts").len();
        assert_eq!(
            served,
            total.min(max_results),
            "case {case:?}: maxResults={max_results} must be served min({max_results}, {total}) \
             accounts rather than a page cut back to {default}, got {served}"
        );
        assert_eq!(
            token_of(&page, case).is_some(),
            total > served,
            "case {case:?}: a token belongs on the page exactly while accounts remain, got {page}"
        );
    }
}
