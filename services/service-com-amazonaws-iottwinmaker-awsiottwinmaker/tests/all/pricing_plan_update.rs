// SPDX-License-Identifier: CC0-1.0

//! `UpdatePricingPlan`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so the route, the status code and the
//! serialized body are all exercised, not just a handler body. Each accepted
//! change is also read BACK through `GetPricingPlan`, so what the call answers
//! and what the store holds are shown to be the same plan.
//!
//! The suite asserts RULES over tables of inputs, the rows differing along what
//! each rule generalises over — mode, bundle count, bundle order, and the plan
//! the change starts from:
//!
//! * the plan in force after the call is the plan the request names: its
//!   `pricingMode` is the requested mode and its `bundleInformation.bundleNames`
//!   are the requested bundles, in order, reported exactly when the mode is a
//!   bundled one. The mode catalog is the model's own `PricingMode` enum
//!   (`ops::types::PRICING_MODE_VALUES`), so no row is keyed to one request;
//! * the change takes effect at once: `effectiveDateTime` equals
//!   `updateDateTime`, both fall inside the call, and nothing is left pending;
//! * `updateReason` says what the change was, read off the plan it replaced —
//!   `PRICING_MODE_UPDATE` for a mode change, `PRICING_TIER_UPDATE` when only
//!   the bundles moved, `OVERWRITTEN` when it superseded a change that had not
//!   taken effect;
//! * `billableEntityCount` is the account's entity count, so it moves with a
//!   `CreateEntity`;
//! * a request whose `pricingMode` is not modelled, whose `bundleNames` fall
//!   outside `@length(1..10)`, or whose mode and bundles contradict each other
//!   is refused with HTTP 400 `ValidationException` and leaves the stored plan
//!   untouched.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::PRICING_MODE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    pricing_mode_is_bundled, PricingPlanRecord, PRICING_MODE_BASIC,
    UPDATE_REASON_OVERWRITTEN, UPDATE_REASON_PRICING_MODE_UPDATE,
    UPDATE_REASON_PRICING_TIER_UPDATE,
};

use crate::harness::{Emulator, REGION};

/// The largest number of names `PricingBundles` admits (`@length(1..10)`), from
/// the `PricingBundles` list's own constraint.
const MAX_PRICING_BUNDLES: usize = 10;

/// An instant as epoch seconds, the unit a restJson1 `Timestamp` carries.
fn epoch_secs_of(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// The epoch seconds a reported `Timestamp` member carries.
fn epoch_secs(case: &str, member: &str, value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an epoch-seconds number, got {value:?}")
    })
}

/// The bundle names a reported plan carries, empty when it reports no
/// `bundleInformation` at all.
fn reported_bundles(plan: &Value) -> Vec<String> {
    plan["bundleInformation"]["bundleNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert that `plan` reports the mode and the bundles a request named, with
/// `bundleInformation` present exactly when the mode is a bundled one.
fn assert_plan_names(case: &str, member: &str, plan: &Value, mode: &str, bundles: &[String]) {
    assert_eq!(
        plan["pricingMode"].as_str(),
        Some(mode),
        "case {case:?}: expected {member}.pricingMode {mode:?}, got {:?}",
        plan["pricingMode"]
    );
    if bundles.is_empty() {
        assert!(
            plan["bundleInformation"].is_null(),
            "case {case:?}: a plan naming no bundle must report no {member}.bundleInformation, \
             got {:?}",
            plan["bundleInformation"]
        );
    } else {
        assert_eq!(
            reported_bundles(plan),
            bundles,
            "case {case:?}: expected {member}.bundleInformation.bundleNames {bundles:?}, got {:?}",
            plan["bundleInformation"]
        );
    }
    // Every required member of `PricingPlan` has to be there whatever the mode.
    for required in ["pricingMode", "updateReason", "effectiveDateTime", "updateDateTime"] {
        assert!(
            !plan[required].is_null(),
            "case {case:?}: {member}.{required} is @required, got {plan:?}"
        );
    }
}

/// Assert that a plan took effect at once inside the window `[before, after]`:
/// its `effectiveDateTime` is its `updateDateTime`, and both fall in the window.
fn assert_effective_at_once(
    case: &str,
    member: &str,
    plan: &Value,
    before: SystemTime,
    after: SystemTime,
) -> f64 {
    let effective = epoch_secs(case, &format!("{member}.effectiveDateTime"), &plan["effectiveDateTime"]);
    let updated = epoch_secs(case, &format!("{member}.updateDateTime"), &plan["updateDateTime"]);
    assert!(
        (effective - updated).abs() < 0.001,
        "case {case:?}: a change that takes effect at once reports {member}.effectiveDateTime == \
         updateDateTime, got {effective} and {updated}"
    );
    let low = epoch_secs_of(before);
    let high = epoch_secs_of(after);
    assert!(
        updated >= low && updated <= high,
        "case {case:?}: expected {member}.updateDateTime inside the call's window \
         [{low}, {high}], got {updated}"
    );
    updated
}

/// The bundle names to send for `mode`: a bundled mode must name at least one
/// (its plan is reported through `bundleInformation`, whose `bundleNames` is
/// `@required`), and the pay-as-you-go mode must name none.
fn bundles_for(mode: &str, count: usize) -> Vec<String> {
    if pricing_mode_is_bundled(mode) {
        (0..count.max(1)).map(|i| format!("bundle-{i}")).collect()
    } else {
        Vec::new()
    }
}

/// The request body for a plan change, omitting `bundleNames` entirely when the
/// change names none.
fn request(mode: &str, bundles: &[String]) -> Value {
    let mut body = json!({"pricingMode": mode});
    if !bundles.is_empty() {
        body["bundleNames"] = json!(bundles);
    }
    body
}

/// RULE: the plan in force after an accepted change is the plan the request
/// named — the requested mode, and the requested bundles in the order given,
/// reported through `bundleInformation` exactly when the mode is a bundled one —
/// the change takes effect at once, nothing is left pending, and
/// `GetPricingPlan` reads the very same plan back.
#[tokio::test]
async fn an_accepted_change_is_the_plan_in_force_and_reads_back() {
    // The rows span every modelled `PricingMode` and, for the bundled ones, the
    // bundle counts at both ends of `PricingBundles`' `@length(1..10)` plus one
    // in between, and a bundle list whose order is not sorted.
    let mut cases: Vec<(String, &str, Vec<String>)> = Vec::new();
    for mode in PRICING_MODE_VALUES {
        for count in [1usize, 2, MAX_PRICING_BUNDLES] {
            let bundles = bundles_for(mode, count);
            cases.push((format!("{mode} with {} bundles", bundles.len()), mode, bundles));
            if !pricing_mode_is_bundled(mode) {
                // The pay-as-you-go mode has exactly one shape; one row is it.
                break;
            }
        }
    }
    // Order is the caller's, not a sort: `PricingBundles` is a list.
    cases.push((
        "a bundled mode with bundles out of alphabetical order".to_string(),
        "TIERED_BUNDLE",
        vec![
            "zeta-bundle".to_string(),
            "alpha-bundle".to_string(),
            "mu-bundle".to_string(),
        ],
    ));

    for (case, mode, bundles) in cases {
        let emulator = Emulator::new();
        let before = SystemTime::now();
        let resp = emulator.update_pricing_plan(request(mode, &bundles)).await;
        let after = SystemTime::now();
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from UpdatePricingPlan, got {resp:?}"
        );
        let current = resp.member("currentPricingPlan");
        assert_plan_names(&case, "currentPricingPlan", current, mode, &bundles);
        let updated = assert_effective_at_once(&case, "currentPricingPlan", current, before, after);
        assert!(
            resp.member("pendingPricingPlan").is_null(),
            "case {case:?}: a change that takes effect at once leaves nothing pending, got {:?}",
            resp.member("pendingPricingPlan")
        );

        // The round trip: an independent read reports the plan the update
        // persisted, instants included, and reports it the same way twice.
        let read = emulator.get_pricing_plan().await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPricingPlan, got {read:?}"
        );
        assert_eq!(
            read.member("currentPricingPlan"),
            current,
            "case {case:?}: GetPricingPlan must report the plan the update persisted, got {read:?}"
        );
        assert!(
            read.member("pendingPricingPlan").is_null(),
            "case {case:?}: nothing is pending after the update, got {:?}",
            read.member("pendingPricingPlan")
        );
        let again = emulator.get_pricing_plan().await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: reading the plan must not change what a later caller sees, got \
             {again:?}"
        );
        let read_updated = epoch_secs(
            &case,
            "currentPricingPlan.updateDateTime",
            &read.member("currentPricingPlan")["updateDateTime"],
        );
        assert!(
            (read_updated - updated).abs() < 0.001,
            "case {case:?}: expected the stored updateDateTime {updated}, got {read_updated}"
        );
    }
}

/// RULE: `updateReason` is read off the plan the change REPLACED, not off the
/// request alone: `PRICING_MODE_UPDATE` when the mode differs from the mode in
/// force, `PRICING_TIER_UPDATE` when the mode is unchanged so only the bundles
/// can have moved, and `OVERWRITTEN` when the change superseded one that had
/// been accepted but had not taken effect.
#[tokio::test]
async fn the_update_reason_describes_the_change() {
    /// One row: the plan to arrive at, and whether a change was pending.
    struct Row {
        case: &'static str,
        /// The change(s) to make before the one under test, in order.
        before: Vec<(&'static str, Vec<&'static str>)>,
        /// Seed a pending change the call under test must supersede.
        pending: bool,
        mode: &'static str,
        bundles: Vec<&'static str>,
        expected_reason: &'static str,
    }

    let rows = vec![
        Row {
            case: "away from the default pay-as-you-go plan",
            before: vec![],
            pending: false,
            mode: "TIERED_BUNDLE",
            bundles: vec!["twinmaker-basic"],
            expected_reason: UPDATE_REASON_PRICING_MODE_UPDATE,
        },
        Row {
            case: "back onto the pay-as-you-go plan",
            before: vec![("TIERED_BUNDLE", vec!["twinmaker-basic"])],
            pending: false,
            mode: PRICING_MODE_BASIC,
            bundles: vec![],
            expected_reason: UPDATE_REASON_PRICING_MODE_UPDATE,
        },
        Row {
            case: "between two bundled modes",
            before: vec![("STANDARD", vec!["twinmaker-basic"])],
            pending: false,
            mode: "TIERED_BUNDLE",
            bundles: vec!["twinmaker-basic"],
            expected_reason: UPDATE_REASON_PRICING_MODE_UPDATE,
        },
        Row {
            case: "the same mode with a longer bundle list",
            before: vec![("TIERED_BUNDLE", vec!["twinmaker-basic"])],
            pending: false,
            mode: "TIERED_BUNDLE",
            bundles: vec!["twinmaker-basic", "twinmaker-matterport"],
            expected_reason: UPDATE_REASON_PRICING_TIER_UPDATE,
        },
        Row {
            case: "the same mode with a different bundle",
            before: vec![("STANDARD", vec!["twinmaker-basic"])],
            pending: false,
            mode: "STANDARD",
            bundles: vec!["twinmaker-scene"],
            expected_reason: UPDATE_REASON_PRICING_TIER_UPDATE,
        },
        Row {
            case: "the same mode and the same bundles",
            before: vec![("TIERED_BUNDLE", vec!["twinmaker-basic"])],
            pending: false,
            mode: "TIERED_BUNDLE",
            bundles: vec!["twinmaker-basic"],
            expected_reason: UPDATE_REASON_PRICING_TIER_UPDATE,
        },
        Row {
            case: "the pay-as-you-go plan re-set on itself",
            before: vec![(PRICING_MODE_BASIC, vec![])],
            pending: false,
            mode: PRICING_MODE_BASIC,
            bundles: vec![],
            expected_reason: UPDATE_REASON_PRICING_TIER_UPDATE,
        },
        Row {
            case: "superseding a pending change, same mode",
            before: vec![("TIERED_BUNDLE", vec!["twinmaker-basic"])],
            pending: true,
            mode: "TIERED_BUNDLE",
            bundles: vec!["twinmaker-scene"],
            expected_reason: UPDATE_REASON_OVERWRITTEN,
        },
        Row {
            case: "superseding a pending change, new mode",
            before: vec![("TIERED_BUNDLE", vec!["twinmaker-basic"])],
            pending: true,
            mode: PRICING_MODE_BASIC,
            bundles: vec![],
            expected_reason: UPDATE_REASON_OVERWRITTEN,
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        for (mode, bundles) in &row.before {
            let bundles: Vec<String> = bundles.iter().map(|b| (*b).to_string()).collect();
            let resp = emulator.update_pricing_plan(request(mode, &bundles)).await;
            assert_eq!(
                resp.status, 200,
                "case {:?}: the setup change to {mode:?} must be accepted, got {resp:?}",
                row.case
            );
        }
        if row.pending {
            // A change accepted but not yet in force can only be put in the
            // store directly: nothing in this emulator defers one.
            let state = emulator.state();
            state.lock().region_mut(REGION).pending_pricing_plan =
                Some(PricingPlanRecord::default_plan(SystemTime::now()));
        }

        let bundles: Vec<String> = row.bundles.iter().map(|b| (*b).to_string()).collect();
        let resp = emulator.update_pricing_plan(request(row.mode, &bundles)).await;
        assert_eq!(
            resp.status, 200,
            "case {:?}: expected 200 from UpdatePricingPlan, got {resp:?}",
            row.case
        );
        assert_eq!(
            resp.member("currentPricingPlan")["updateReason"].as_str(),
            Some(row.expected_reason),
            "case {:?}: expected updateReason {:?}, got {:?}",
            row.case,
            row.expected_reason,
            resp.member("currentPricingPlan")["updateReason"]
        );
        assert!(
            resp.member("pendingPricingPlan").is_null(),
            "case {:?}: a superseded change is gone, not pending, got {:?}",
            row.case,
            resp.member("pendingPricingPlan")
        );
        // The reason is persisted, not just answered.
        let read = emulator.get_pricing_plan().await;
        assert_eq!(
            read.member("currentPricingPlan")["updateReason"].as_str(),
            Some(row.expected_reason),
            "case {:?}: GetPricingPlan must report the stored updateReason, got {read:?}",
            row.case
        );
        assert!(
            read.member("pendingPricingPlan").is_null(),
            "case {:?}: nothing is pending in the store, got {read:?}",
            row.case
        );
    }
}

/// RULE: successive changes each replace the plan WHOLE — the plan in force
/// carries only what the latest request named, never a member left over from the
/// plan before it — and each change's `updateDateTime` is at or after the one
/// before.
#[tokio::test]
async fn each_change_replaces_the_plan_whole() {
    let emulator = Emulator::new();

    // A walk whose steps differ in mode, in bundle count and in direction, so a
    // leftover bundle from an earlier step would show up.
    let walk: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "onto a three-bundle tiered plan",
            "TIERED_BUNDLE",
            vec!["b1", "b2", "b3"],
        ),
        ("down to a one-bundle tiered plan", "TIERED_BUNDLE", vec!["b9"]),
        ("across to a standard plan", "STANDARD", vec!["b9", "b8"]),
        ("off bundles entirely", PRICING_MODE_BASIC, vec![]),
        ("back onto a bundled plan", "STANDARD", vec!["b4"]),
    ];

    let mut previous_update = 0.0_f64;
    for (case, mode, bundles) in walk {
        let bundles: Vec<String> = bundles.iter().map(|b| (*b).to_string()).collect();
        let before = SystemTime::now();
        let resp = emulator.update_pricing_plan(request(mode, &bundles)).await;
        let after = SystemTime::now();
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from UpdatePricingPlan, got {resp:?}"
        );
        let current = resp.member("currentPricingPlan");
        assert_plan_names(case, "currentPricingPlan", current, mode, &bundles);
        let updated = assert_effective_at_once(case, "currentPricingPlan", current, before, after);
        assert!(
            updated >= previous_update,
            "case {case:?}: expected updateDateTime at or after the previous change's \
             {previous_update}, got {updated}"
        );
        previous_update = updated;

        let read = emulator.get_pricing_plan().await;
        assert_plan_names(
            case,
            "currentPricingPlan (read back)",
            read.member("currentPricingPlan"),
            mode,
            &bundles,
        );
    }
}

/// RULE: `billableEntityCount` on the plan this call reports is the account's
/// current entity count, derived from the stored entities across every
/// workspace — not a counter the pricing call maintains.
#[tokio::test]
async fn the_reported_plan_carries_the_accounts_entity_count() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("alpha").await.status,
        200,
        "the first workspace must be created"
    );
    assert_eq!(
        emulator.create_workspace("beta").await.status,
        200,
        "the second workspace must be created"
    );

    // Each row: the entities to add first (workspace, id), then the count the
    // plan must report.
    struct Row {
        case: &'static str,
        entities: Vec<(&'static str, &'static str)>,
        expected_count: i64,
    }
    let rows = vec![
        Row {
            case: "an account with no entities",
            entities: vec![],
            expected_count: 0,
        },
        Row {
            case: "one entity",
            entities: vec![("alpha", "e1")],
            expected_count: 1,
        },
        Row {
            case: "entities in two workspaces",
            entities: vec![("alpha", "e2"), ("beta", "e3")],
            expected_count: 3,
        },
    ];

    for Row {
        case,
        entities,
        expected_count,
    } in rows
    {
        for (workspace_id, entity_id) in &entities {
            let resp = emulator
                .create_entity(
                    workspace_id,
                    json!({"entityId": entity_id, "entityName": entity_id}),
                )
                .await;
            assert_eq!(
                resp.status, 200,
                "case {case:?}: expected 200 from CreateEntity, got {resp:?}"
            );
        }
        let resp = emulator
            .update_pricing_plan(request("TIERED_BUNDLE", &["twinmaker-basic".to_string()]))
            .await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from UpdatePricingPlan, got {resp:?}"
        );
        assert_eq!(
            resp.member("currentPricingPlan")["billableEntityCount"].as_i64(),
            Some(expected_count),
            "case {case:?}: expected billableEntityCount {expected_count}, got {:?}",
            resp.member("currentPricingPlan")["billableEntityCount"]
        );
        // The same count the reader derives, from the same entities.
        let read = emulator.get_pricing_plan().await;
        assert_eq!(
            read.member("currentPricingPlan")["billableEntityCount"].as_i64(),
            Some(expected_count),
            "case {case:?}: GetPricingPlan must derive the same count, got {read:?}"
        );
    }
}

/// RULE: a request the service cannot turn into a reportable plan is refused
/// with the modelled `ValidationException` (HTTP 400) and leaves the stored plan
/// exactly as it was — a `pricingMode` outside the modelled enum, a
/// `bundleNames` outside `PricingBundles`' `@length(1..10)`, or a mode and a
/// bundle list that contradict each other.
#[tokio::test]
async fn a_request_that_names_no_reportable_plan_is_refused() {
    // One name past the modelled maximum of ten.
    let eleven: Vec<String> = (0..=MAX_PRICING_BUNDLES).map(|i| format!("b{i}")).collect();

    let mut rows: Vec<(String, Value)> = vec![
        (
            "a pricingMode outside the modelled enum".to_string(),
            json!({"pricingMode": "PREMIUM"}),
        ),
        (
            "a pricingMode in the wrong case".to_string(),
            json!({"pricingMode": "basic"}),
        ),
        (
            "an empty pricingMode".to_string(),
            json!({"pricingMode": ""}),
        ),
        (
            "an empty bundleNames, one below the modelled minimum".to_string(),
            json!({"pricingMode": "TIERED_BUNDLE", "bundleNames": []}),
        ),
        (
            "eleven bundleNames, one above the modelled maximum".to_string(),
            json!({"pricingMode": "TIERED_BUNDLE", "bundleNames": eleven}),
        ),
    ];
    // The mode/bundle contradiction, over every modelled mode: a bundled mode
    // naming no bundle, and the pay-as-you-go mode naming one.
    for mode in PRICING_MODE_VALUES {
        if pricing_mode_is_bundled(mode) {
            rows.push((
                format!("{mode} with no bundleNames"),
                json!({"pricingMode": mode}),
            ));
        } else {
            rows.push((
                format!("{mode} with bundleNames"),
                json!({"pricingMode": mode, "bundleNames": ["twinmaker-basic"]}),
            ));
        }
    }

    for (case, body) in rows {
        let emulator = Emulator::new();
        // Start from a plan with known content, so a refusal that wrote
        // anything would show.
        let seeded = emulator
            .update_pricing_plan(request("STANDARD", &["seed-bundle".to_string()]))
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: the seeding change must be accepted, got {seeded:?}"
        );
        let before = emulator.get_pricing_plan().await;

        let refused = emulator.update_pricing_plan(body.clone()).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the modelled 400 for {body}, got {refused:?}"
        );
        assert!(
            refused.member("message").is_string(),
            "case {case:?}: a refusal must say why, got {refused:?}"
        );

        let after = emulator.get_pricing_plan().await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused change must leave the stored plan untouched, got {after:?}"
        );
    }
}
