// SPDX-License-Identifier: CC0-1.0

//! The ONE page walk every paginated listing of this service performs.
//!
//! The rule: a page is the slice of the already-selected sequence that starts
//! where the previous page stopped and holds at most `maxResults` items, and a
//! `nextToken` is carried exactly while items remain after it, resuming at the
//! item the page stopped before. So walking a listing at ANY page size yields
//! the same items, in the same order, with no item served twice and none
//! skipped.
//!
//! [`state::serve_page`] is that walk, and the cases below drive it twice over:
//! directly, over a table of sequence lengths, offsets and page sizes — including
//! the boundaries an operation cannot be asked for over the wire (an offset at
//! and past the end of the sequence) — and then through EVERY paginated
//! operation of this service, each the way a client drives it: an awsJson1_0
//! `POST /` through the same [`MultiServiceRouter`] the emulator binary mounts.
//! Both the plain listings and the ones that GROUP before paging are rows, so a
//! listing that walked its own pages differently from its siblings fails here.
//!
//! The inventory the listings report over — Compute Optimizer's own analysis
//! output, the enrollment of other accounts, and automation events — is not
//! minted by any operation of this API, so it is seeded through the writers the
//! sibling suites own; the automation rules are created over the wire.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RecommendedAction;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, PageCursor, StoredStep,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_association::seed_enrollment;
use crate::automation_events::{now_secs, seed as seed_events, stored_event};
use crate::automation_rule_lifecycle::{call_ok, emulator_with_engine, is_next_token, minimal_rule};
use crate::recommended_actions::{record, stored as stored_action};

/// The two `RecommendedActionType` values the model declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`). The seeded
/// inventory alternates between them, so the listings that GROUP by the type
/// report more than one group and their walks span more than one page.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// Three terminal `EventStatus` values
/// (`smithy_query(query="shape",shape="EventStatus")`), so the summary that
/// groups by status reports three groups. A seeded event is already in the
/// status it names, so the clock owes it no transition and the number of items
/// a listing reports cannot move between the pages of one walk.
const EVENT_STATUSES: [&str; 3] = ["Complete", "Failed", "RollbackComplete"];

/// How many member accounts the fixture enrolls.
const MEMBER_ACCOUNTS: usize = 5;

/// How many recommended actions the fixture records.
const ACTIONS: usize = 6;

/// How many automation events the fixture seeds.
const EVENTS: usize = 6;

/// How many automation rules the fixture creates over the wire.
const RULES: usize = 4;

/// How many steps the fixture's first event carries.
const STEPS: usize = 5;

/// A page size above every total the fixture leaves, and inside the smallest
/// `maxResults` range this service models (`range(min: 1, max: 100)`), so one
/// request serves a whole listing and is the reference the paged walks are
/// compared against.
const WHOLE_LISTING: i32 = 100;

/// The page sizes every listing is walked at.
const WALK_SIZES: [i32; 3] = [1, 2, 3];

/// A ceiling on the pages of one walk, so a non-advancing token fails the case
/// instead of hanging it.
const MAX_PAGES: usize = 1_000;

/// `n` distinct member account ids in the modelled `AccountId` form
/// (`^[0-9]{12}$`), none of them the calling account.
pub(crate) fn member_account_ids(n: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("{:012}", 700_000_000_000u64 + index as u64))
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
fn step(index: usize) -> StoredStep {
    StoredStep {
        step_id: format!("stp{index:013}"),
        step_type: "CreateEbsSnapshot".to_string(),
        step_status: state::STEP_STATUS_READY.to_string(),
        resource_id: Some(format!("vol-{index:08}")),
        start_timestamp: None,
        completed_timestamp: None,
        estimated_monthly_savings: None,
    }
}

/// One paginated listing: the operation, the request that selects everything it
/// reports, and the output member its page lands in.
struct Listing {
    operation: &'static str,
    request: Value,
    items: &'static str,
}

/// The request both preview operations report the whole seeded inventory for.
fn preview_request() -> Value {
    json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    })
}

/// Every paginated operation of this service: the four that page a filtered
/// sequence of stored items, the three that page the groups a summary computed,
/// the preview that pages a selected inventory, and the one that pages an
/// event's steps.
fn listings(event_id: &str) -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListAccounts",
            request: json!({}),
            items: "accounts",
        },
        Listing {
            operation: "ListAutomationRules",
            request: json!({}),
            items: "automationRules",
        },
        Listing {
            operation: "ListRecommendedActions",
            request: json!({}),
            items: "recommendedActions",
        },
        Listing {
            operation: "ListRecommendedActionSummaries",
            request: json!({}),
            items: "recommendedActionSummaries",
        },
        Listing {
            operation: "ListAutomationRulePreview",
            request: preview_request(),
            items: "previewResults",
        },
        Listing {
            operation: "ListAutomationRulePreviewSummaries",
            request: preview_request(),
            items: "previewResultSummaries",
        },
        Listing {
            operation: "ListAutomationEvents",
            request: json!({}),
            items: "automationEvents",
        },
        Listing {
            operation: "ListAutomationEventSummaries",
            request: json!({}),
            items: "automationEventSummaries",
        },
        Listing {
            operation: "ListAutomationEventSteps",
            request: json!({"eventId": event_id}),
            items: "automationEventSteps",
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

/// One reported item as the walk is compared on: everything it carries except
/// the window a summary says it was computed over, whose open end is the instant
/// of the CALL and so differs between two requests by design. Which items a walk
/// serves, and in which order, is what the cases below are about.
fn comparable(item: &Value) -> Value {
    let mut item = item.clone();
    if let Some(members) = item.as_object_mut() {
        members.remove("timePeriod");
    }
    item
}

/// The items one response reports, as the walk is compared on them.
fn comparable_items(body: &Value, member_name: &str) -> Vec<Value> {
    items_of(body, member_name).iter().map(comparable).collect()
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

/// A router with every paginated listing of this service given something to
/// report, and the id of the one event that carries steps.
pub(crate) async fn emulator_with_a_fixture() -> (MultiServiceRouter, String) {
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
    // retrieval window. The first one carries the steps the step listing pages.
    let created = now_secs();
    let mut events = Vec::with_capacity(EVENTS);
    for index in 0..EVENTS {
        let mut event = stored_event(
            index,
            engine::EMULATOR_ACCOUNT_ID,
            SNAPSHOT_AND_DELETE,
            EVENT_STATUSES[index % EVENT_STATUSES.len()],
            created - (index as i64) * 60,
        );
        if index == 0 {
            event.steps = (0..STEPS).map(step).collect();
        }
        events.push(event);
    }
    let stepped_event_id = events[0].event_id.clone();
    seed_events(&engine, &events);

    for index in 0..RULES {
        call_ok(
            &router,
            "CreateAutomationRule",
            &minimal_rule(&format!("shared-page-walk-{index}")),
            "seed-rules",
        )
        .await;
    }

    (router, stepped_event_id)
}

// ---------------------------------------------------------------------------
// The walk itself
// ---------------------------------------------------------------------------

/// The shared walk over the whole span of what can reach it: sequences shorter
/// than, exactly as long as and longer than one page, offsets at the start, in
/// the middle, at the very end and past the end of the sequence, and page sizes
/// from one to larger than the sequence itself. In every case the page is the
/// slice that starts at the offset, and a token is minted exactly while items
/// remain after it — carrying the offset of the item the page stopped before and
/// the fingerprint the selection was made under.
#[test]
fn the_shared_page_walk_serves_a_slice_and_mints_the_token_that_resumes_it() {
    const FINGERPRINT: &str = "a-selection-fingerprint";
    // (case, how many items the selected sequence holds, the offset resumed
    // from, the page size)
    let cases: [(&str, usize, usize, usize); 11] = [
        ("an-empty-sequence", 0, 0, 5),
        ("fewer-items-than-one-page", 3, 0, 5),
        ("exactly-one-page", 5, 0, 5),
        ("one-item-past-the-page", 6, 0, 5),
        ("the-smallest-page", 6, 0, 1),
        ("resumed-mid-sequence", 6, 2, 3),
        ("resumed-at-the-last-item", 6, 5, 5),
        ("resumed-exactly-at-the-end", 6, 6, 5),
        ("resumed-past-the-end", 6, 9, 5),
        ("a-page-larger-than-the-sequence", 4, 1, 1_000),
        ("many-items-at-an-odd-page-size", 1_000, 400, 7),
    ];

    for (case, length, offset, page_size) in cases {
        let (page, token): (Vec<String>, Option<String>) = state::serve_page(
            (0..length).map(|index| format!("item-{index:05}")),
            offset,
            page_size,
            FINGERPRINT.to_string(),
        );

        let served = length.saturating_sub(offset).min(page_size);
        let expected: Vec<String> = (offset..offset + served)
            .map(|index| format!("item-{index:05}"))
            .collect();
        assert_eq!(
            page, expected,
            "case {case:?}: a page of {page_size} resumed at {offset} of {length} items must be \
             the slice {expected:?}, got {page:?}"
        );

        let remaining = length.saturating_sub(offset + served);
        match token {
            None => assert_eq!(
                remaining, 0,
                "case {case:?}: a page with {remaining} items after it must carry a token, got \
                 none"
            ),
            Some(token) => {
                assert!(
                    remaining > 0,
                    "case {case:?}: nothing remains after this page, so it must carry no token, \
                     got {token:?}"
                );
                assert!(
                    is_next_token(&token),
                    "case {case:?}: a minted token must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                );
                let cursor = state::decode_page_token(&token);
                assert_eq!(
                    cursor,
                    Some(PageCursor {
                        offset: offset + served,
                        filter_fingerprint: FINGERPRINT.to_string(),
                    }),
                    "case {case:?}: the token must resume at {} under this selection, got \
                     {cursor:?}",
                    offset + served
                );
            }
        }
    }
}

/// The walk takes any sequence by value as well as any lazy one, so a caller
/// that already collected its selection pages it through the same helper.
#[test]
fn the_shared_page_walk_pages_an_owned_sequence() {
    let owned: Vec<u32> = (0..4).collect();
    let (page, token): (Vec<u32>, Option<String>) =
        state::serve_page(owned, 1, 2, "fingerprint".to_string());
    assert_eq!(
        page,
        vec![1, 2],
        "case \"owned-sequence\": expected the slice [1, 2], got {page:?}"
    );
    let cursor = state::decode_page_token(token.as_deref().unwrap_or_default());
    assert_eq!(
        cursor.map(|cursor| cursor.offset),
        Some(3),
        "case \"owned-sequence\": the token must resume at 3, got {token:?}"
    );
}

/// Every paginated operation of this service walks its pages by that one rule:
/// walked at any page size, a listing reports exactly the items one whole-listing
/// request reports, in the same order, with every page but the last filled to the
/// page size, a token on a page exactly while items remain after it, and each
/// token resuming at the item its page stopped before.
#[tokio::test]
async fn every_paginated_listing_walks_its_pages_by_the_shared_rule() {
    let (router, event_id) = emulator_with_a_fixture().await;

    for listing in listings(&event_id) {
        // The reference: the whole listing in one page, which is what the paged
        // walks below must reproduce item for item.
        let case = listing.operation;
        let mut whole = listing.request.clone();
        whole["maxResults"] = json!(WHOLE_LISTING);
        let whole_page = call_ok(&router, listing.operation, &whole, case).await;
        let expected = comparable_items(&whole_page, listing.items);
        assert_eq!(
            token_of(&whole_page, case),
            None,
            "case {case:?}: the fixture must leave this listing fewer than {WHOLE_LISTING} items, \
             so the reference request serves all of them, got {whole_page}"
        );
        assert!(
            !expected.is_empty(),
            "case {case:?}: the fixture must leave this listing something to page, got \
             {whole_page}"
        );

        for page_size in WALK_SIZES {
            let limit = usize::try_from(page_size).expect("a walk page size is a positive count");
            let case = format!("{} at maxResults={page_size}", listing.operation);
            let case = case.as_str();
            let mut served: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;

            loop {
                let mut request = listing.request.clone();
                request["maxResults"] = json!(page_size);
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let resumed_from = served.len();
                let body = call_ok(&router, listing.operation, &request, case).await;
                let items = comparable_items(&body, listing.items);
                assert!(
                    items.len() <= limit,
                    "case {case:?}: a page holds at most {limit} items, got {}",
                    items.len()
                );
                served.extend_from_slice(&items);
                pages += 1;
                assert!(
                    pages <= MAX_PAGES,
                    "case {case:?}: the walk must terminate, got {} items over {pages} pages",
                    served.len()
                );

                token = token_of(&body, case);
                assert_eq!(
                    token.is_some(),
                    served.len() < expected.len(),
                    "case {case:?}: page {pages} must carry a token exactly while items remain, \
                     got {body}"
                );
                match &token {
                    // Only the LAST page of a walk may be short: a page that
                    // carries a token was cut by the page size, not by the end
                    // of the sequence.
                    Some(token) => {
                        assert_eq!(
                            items.len(),
                            limit,
                            "case {case:?}: page {pages} carries a token, so it must hold {limit} \
                             items, got {}",
                            items.len()
                        );
                        let cursor = state::decode_page_token(token);
                        assert_eq!(
                            cursor.as_ref().map(|cursor| cursor.offset),
                            Some(resumed_from + items.len()),
                            "case {case:?}: page {pages} resumed at {resumed_from}, so its token \
                             must resume at {}, got {cursor:?}",
                            resumed_from + items.len()
                        );
                    }
                    None => break,
                }
            }

            assert_eq!(
                served.len(),
                expected.len(),
                "case {case:?}: the walk must serve every item exactly once — {} in all — got {} \
                 over {pages} pages",
                expected.len(),
                served.len()
            );
            assert_eq!(
                served, expected,
                "case {case:?}: the walk must report the items the whole listing reports, in the \
                 same order; expected {expected:?}, got {served:?}"
            );
        }
    }
}
