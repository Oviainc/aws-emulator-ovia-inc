// SPDX-License-Identifier: CC0-1.0

//! The pricing plan as a BILLING object: the tier a bundled plan is billed at,
//! and the plan change that is accepted now and in force later.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so the route, the status code and the
//! serialized body are all exercised, and every accepted change is read back
//! through `GetPricingPlan`.
//!
//! The rules asserted here, each over a table of inputs rather than one case:
//!
//! * a plan that names bundles reports a `bundleInformation.pricingTier`, and
//!   which tier that is follows the account's billable entity count: `TIER_1` is
//!   the smallest band and each further band a larger count
//!   (`harness::TIER_BY_ENTITY_COUNT`, whose rows sit on each band boundary and
//!   one entity past it);
//! * a band the account's count later crosses moves the plan's tier WITHOUT
//!   anybody asking, which is what the modelled `updateReason`
//!   `ENTITY_COUNT_UPDATE` records;
//! * a change the service bills from the start of the next cycle — leaving a
//!   bundled plan for the pay-as-you-go mode — is reported as
//!   `pendingPricingPlan` with a later `effectiveDateTime` while
//!   `currentPricingPlan` keeps reporting the plan still in force, and becomes
//!   the plan in force once that instant has passed, at that instant;
//! * a further change SUPERSEDES a pending change that has not taken effect,
//!   which the plan then carries as `updateReason` `OVERWRITTEN`, and the
//!   superseded change never becomes the plan in force;
//! * a pending change that HAS taken effect is the plan the next change is read
//!   against, so it is not superseded.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    PRICING_MODE_VALUES, PRICING_TIER_VALUES,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    pricing_mode_is_bundled, pricing_tier_for, PRICING_MODE_BASIC, PRICING_PLAN_PENDING_SPAN,
    UPDATE_REASON_ENTITY_COUNT_UPDATE, UPDATE_REASON_OVERWRITTEN,
    UPDATE_REASON_PRICING_MODE_UPDATE, UPDATE_REASON_PRICING_TIER_UPDATE,
};

use crate::harness::{entity_count_in_tier, seed_entities, Emulator, TIER_BY_ENTITY_COUNT};

/// The bundle a case buys when the bundle list itself is not what it is about.
const BUNDLE: &str = "twinmaker-basic";

/// How long to wait for a change the service accepted for a later instant: the
/// span it is due after, plus enough slack that a loaded machine has reached it.
fn past_the_pending_span() -> Duration {
    PRICING_PLAN_PENDING_SPAN + Duration::from_millis(250)
}

/// The request body for a plan change, omitting `bundleNames` when the mode
/// names none (the pay-as-you-go mode must not carry any).
fn request(mode: &str, bundles: &[&str]) -> Value {
    if bundles.is_empty() {
        json!({"pricingMode": mode})
    } else {
        json!({"pricingMode": mode, "bundleNames": bundles})
    }
}

/// The epoch seconds a reported `Timestamp` member carries.
fn epoch_secs(case: &str, member: &str, value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an epoch-seconds number, got {value:?}")
    })
}

/// The bundle names a reported plan carries, empty when it reports no
/// `bundleInformation`.
fn reported_bundles(plan: &Value) -> Vec<String> {
    plan["bundleInformation"]["bundleNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: the tier a bundled plan is billed at is the band the account's billable
/// entity count falls in — `TIER_1` the smallest, each further band a larger
/// count, the last one unbounded — so the band table is monotonic in the count
/// and every tier the model defines is a band some count reaches.
#[tokio::test]
async fn the_tier_bands_cover_every_modelled_tier_and_rise_with_the_count() {
    let mut highest = 0usize;
    for (count, tier) in TIER_BY_ENTITY_COUNT {
        let band = PRICING_TIER_VALUES
            .iter()
            .position(|modelled| modelled == tier)
            .unwrap_or_else(|| panic!("case {count} entities: {tier:?} is not a modelled tier"));
        assert!(
            band >= highest,
            "case {count} entities: the tier must not fall as the count rises, got {tier:?} \
             (band {band}) after band {highest}"
        );
        highest = band;
        let reported = pricing_tier_for(i64::try_from(*count).expect("a count fits a Long"));
        assert_eq!(
            reported, *tier,
            "case {count} entities: expected tier {tier:?}, got {reported:?}"
        );
    }
    assert_eq!(
        highest,
        PRICING_TIER_VALUES.len() - 1,
        "the band table must reach the last modelled tier {:?}, got up to band {highest}",
        PRICING_TIER_VALUES.last()
    );
    // One value past the largest count the service can hold: the top band is
    // unbounded, so it is still the last modelled tier.
    let beyond = pricing_tier_for(i64::MAX);
    assert_eq!(
        Some(beyond),
        PRICING_TIER_VALUES.last().copied(),
        "case the largest Long: expected the last modelled tier, got {beyond:?}"
    );
}

/// RULE: a plan that names bundles is billed by TIER, so it reports
/// `bundleInformation.pricingTier` — the band the account's billable entity
/// count places it in — and `UpdatePricingPlan` and `GetPricingPlan` report the
/// same one.
#[tokio::test]
async fn a_bundled_plan_reports_the_tier_its_entity_count_places_it_in() {
    // Every band boundary and one entity past it, for the mode whose name says
    // it is billed in tiers.
    let mut cases: Vec<(String, &str, usize, &str)> = TIER_BY_ENTITY_COUNT
        .iter()
        .map(|(count, tier)| {
            (
                format!("TIERED_BUNDLE at {count} billable entities"),
                "TIERED_BUNDLE",
                *count,
                *tier,
            )
        })
        .collect();
    // A tier is what any BUNDLED plan is billed at, not only that one: the mode
    // catalogue is the model's own `PricingMode` enum.
    for mode in PRICING_MODE_VALUES {
        if !pricing_mode_is_bundled(mode) {
            continue;
        }
        for (count, tier) in [(0usize, "TIER_1"), (entity_count_in_tier("TIER_2"), "TIER_2")] {
            cases.push((
                format!("{mode} at {count} billable entities"),
                mode,
                count,
                tier,
            ));
        }
    }

    for (case, mode, count, expected_tier) in cases {
        let emulator = Emulator::new();
        seed_entities(&emulator, count).await;

        let resp = emulator.update_pricing_plan(request(mode, &[BUNDLE])).await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from UpdatePricingPlan, got {resp:?}"
        );
        let current = resp.member("currentPricingPlan");
        assert_eq!(
            current["bundleInformation"]["pricingTier"].as_str(),
            Some(expected_tier),
            "case {case:?}: expected currentPricingPlan.bundleInformation.pricingTier \
             {expected_tier:?}, got {:?}",
            current["bundleInformation"]
        );
        assert_eq!(
            current["billableEntityCount"].as_i64(),
            Some(i64::try_from(count).expect("a count fits a Long")),
            "case {case:?}: expected billableEntityCount {count}, got {:?}",
            current["billableEntityCount"]
        );

        // The round trip: the tier is persisted on the plan, not computed for
        // the answer of the call that set it.
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
    }
}

/// RULE: the pay-as-you-go plan is not billed by tier — it buys no bundle, so it
/// reports no `bundleInformation` and no tier — whatever the account's billable
/// entity count is.
#[tokio::test]
async fn the_pay_as_you_go_plan_reports_no_tier() {
    for count in [0usize, entity_count_in_tier("TIER_2")] {
        let case = format!("the pay-as-you-go plan at {count} billable entities");
        let emulator = Emulator::new();
        seed_entities(&emulator, count).await;

        let resp = emulator
            .update_pricing_plan(request(PRICING_MODE_BASIC, &[]))
            .await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from UpdatePricingPlan, got {resp:?}"
        );
        assert!(
            resp.member("currentPricingPlan")["bundleInformation"].is_null(),
            "case {case:?}: a plan naming no bundle reports no bundleInformation, got {:?}",
            resp.member("currentPricingPlan")["bundleInformation"]
        );
    }
}

/// RULE: the tier a bundled plan is billed at follows the account's billable
/// entity count, so the SERVICE moves it when the count crosses a band — a
/// change no caller asked for, which the plan carries as `updateReason`
/// `ENTITY_COUNT_UPDATE` while keeping its mode and its bundles. Reporting the
/// moved plan twice reports it identically: the move is owed once.
#[tokio::test]
async fn the_service_moves_the_tier_when_the_entity_count_crosses_a_band() {
    // Each row: the band to start in, then the band to grow into.
    let rows = [
        ("TIER_1", "TIER_2"),
        ("TIER_1", "TIER_3"),
        ("TIER_2", "TIER_4"),
    ];

    for (from_tier, to_tier) in rows {
        let case = format!("from {from_tier} up to {to_tier}");
        let emulator = Emulator::new();
        let start = entity_count_in_tier(from_tier);
        seed_entities(&emulator, start).await;

        let set = emulator
            .update_pricing_plan(request("TIERED_BUNDLE", &[BUNDLE]))
            .await;
        assert_eq!(
            set.status, 200,
            "case {case:?}: expected 200 from UpdatePricingPlan, got {set:?}"
        );
        assert_eq!(
            set.member("currentPricingPlan")["bundleInformation"]["pricingTier"].as_str(),
            Some(from_tier),
            "case {case:?}: expected the plan to start in {from_tier:?}, got {set:?}"
        );
        assert_eq!(
            set.member("currentPricingPlan")["updateReason"].as_str(),
            Some(UPDATE_REASON_PRICING_MODE_UPDATE),
            "case {case:?}: a change a caller asked for says so, got {:?}",
            set.member("currentPricingPlan")["updateReason"]
        );

        // Entities enough to cross into the higher band. The plan itself is
        // untouched by the creates.
        seed_entities(&emulator, entity_count_in_tier(to_tier)).await;

        let read = emulator.get_pricing_plan().await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPricingPlan, got {read:?}"
        );
        let moved = read.member("currentPricingPlan");
        assert_eq!(
            moved["bundleInformation"]["pricingTier"].as_str(),
            Some(to_tier),
            "case {case:?}: expected the tier to move to {to_tier:?}, got {moved:?}"
        );
        assert_eq!(
            moved["updateReason"].as_str(),
            Some(UPDATE_REASON_ENTITY_COUNT_UPDATE),
            "case {case:?}: a tier the service moved itself says so, got {:?}",
            moved["updateReason"]
        );
        assert_eq!(
            moved["pricingMode"].as_str(),
            Some("TIERED_BUNDLE"),
            "case {case:?}: a tier move leaves the mode alone, got {moved:?}"
        );
        assert_eq!(
            reported_bundles(moved),
            vec![BUNDLE.to_string()],
            "case {case:?}: a tier move leaves the bundles alone, got {moved:?}"
        );

        let again = emulator.get_pricing_plan().await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: the move is owed once, so a second read reports the same plan, \
             got {again:?}"
        );
    }
}

/// RULE: a change the service bills from the start of the next cycle — leaving a
/// bundled plan for the pay-as-you-go mode — is accepted now and in force later:
/// it is reported as `pendingPricingPlan` with an `effectiveDateTime` after the
/// instant it was accepted, `currentPricingPlan` keeps reporting the plan still
/// in force, and once that instant has passed the change IS the plan in force,
/// effective at the instant it fell due.
#[tokio::test]
async fn leaving_a_bundled_plan_for_pay_as_you_go_is_pending_until_it_takes_effect() {
    for mode in PRICING_MODE_VALUES {
        if !pricing_mode_is_bundled(mode) {
            continue;
        }
        let case = format!("off {mode} onto the pay-as-you-go plan");
        let emulator = Emulator::new();

        let onto = emulator.update_pricing_plan(request(mode, &[BUNDLE])).await;
        assert_eq!(
            onto.status, 200,
            "case {case:?}: expected 200 buying the bundle, got {onto:?}"
        );
        let in_force = onto.member("currentPricingPlan").clone();
        assert!(
            onto.member("pendingPricingPlan").is_null(),
            "case {case:?}: buying a bundle takes effect at once, got {:?}",
            onto.member("pendingPricingPlan")
        );

        let off = emulator
            .update_pricing_plan(request(PRICING_MODE_BASIC, &[]))
            .await;
        assert_eq!(
            off.status, 200,
            "case {case:?}: expected 200 leaving the bundle, got {off:?}"
        );
        assert_eq!(
            off.member("currentPricingPlan"),
            &in_force,
            "case {case:?}: the plan in force must be untouched while the change is pending, \
             got {off:?}"
        );
        let pending = off.member("pendingPricingPlan").clone();
        assert_eq!(
            pending["pricingMode"].as_str(),
            Some(PRICING_MODE_BASIC),
            "case {case:?}: expected the pending plan to name the requested mode, got {pending:?}"
        );
        assert!(
            pending["bundleInformation"].is_null(),
            "case {case:?}: the pay-as-you-go plan buys no bundle, got {pending:?}"
        );
        assert_eq!(
            pending["updateReason"].as_str(),
            Some(UPDATE_REASON_PRICING_MODE_UPDATE),
            "case {case:?}: expected the pending plan's updateReason to name the mode change, \
             got {pending:?}"
        );
        let accepted_at = epoch_secs(&case, "pendingPricingPlan.updateDateTime", &pending["updateDateTime"]);
        let due_at = epoch_secs(
            &case,
            "pendingPricingPlan.effectiveDateTime",
            &pending["effectiveDateTime"],
        );
        assert!(
            due_at > accepted_at,
            "case {case:?}: a change that is not in force yet falls due after it was accepted, \
             got effectiveDateTime {due_at} against updateDateTime {accepted_at}"
        );

        // Until that instant passes, a reader reports the same pair.
        let read = emulator.get_pricing_plan().await;
        assert_eq!(
            read.body, off.body,
            "case {case:?}: GetPricingPlan must report the same pair while the change is \
             pending, got {read:?}"
        );

        tokio::time::sleep(past_the_pending_span()).await;

        let landed = emulator.get_pricing_plan().await;
        assert_eq!(
            landed.status, 200,
            "case {case:?}: expected 200 from GetPricingPlan, got {landed:?}"
        );
        let current = landed.member("currentPricingPlan");
        assert_eq!(
            current["pricingMode"].as_str(),
            Some(PRICING_MODE_BASIC),
            "case {case:?}: the change must be in force once its instant has passed, got {landed:?}"
        );
        assert!(
            current["bundleInformation"].is_null(),
            "case {case:?}: the landed plan buys no bundle, got {current:?}"
        );
        let landed_effective = epoch_secs(
            &case,
            "currentPricingPlan.effectiveDateTime",
            &current["effectiveDateTime"],
        );
        assert!(
            (landed_effective - due_at).abs() < 0.001,
            "case {case:?}: the landed plan is effective at the instant it fell due {due_at}, \
             got {landed_effective}"
        );
        assert!(
            landed.member("pendingPricingPlan").is_null(),
            "case {case:?}: nothing is pending once the change has landed, got {:?}",
            landed.member("pendingPricingPlan")
        );
        let again = emulator.get_pricing_plan().await;
        assert_eq!(
            again.body, landed.body,
            "case {case:?}: reading a landed change must not move it, got {again:?}"
        );
    }
}

/// RULE: a change SUPERSEDES one that was accepted but has not taken effect —
/// the superseded change never becomes the plan in force, and the change that
/// replaced it carries `updateReason` `OVERWRITTEN`, on whichever plan it is
/// itself (in force at once, or pending in its turn).
#[tokio::test]
async fn a_change_supersedes_a_pending_change_that_has_not_taken_effect() {
    /// One row: the change that supersedes the pending one, and whether it is
    /// itself a change the service defers.
    struct Row {
        case: &'static str,
        mode: &'static str,
        bundles: Vec<&'static str>,
        deferred: bool,
    }
    let rows = vec![
        Row {
            case: "superseded by buying a bundle again",
            mode: "TIERED_BUNDLE",
            bundles: vec!["twinmaker-scene"],
            deferred: false,
        },
        Row {
            case: "superseded by buying a bundle in another mode",
            mode: "STANDARD",
            bundles: vec![BUNDLE],
            deferred: false,
        },
        Row {
            case: "superseded by the same move off the bundle",
            mode: PRICING_MODE_BASIC,
            bundles: vec![],
            deferred: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        assert_eq!(
            emulator
                .update_pricing_plan(request("TIERED_BUNDLE", &[BUNDLE]))
                .await
                .status,
            200,
            "case {case:?}: the bundle the account starts on must be bought"
        );
        let deferral = emulator
            .update_pricing_plan(request(PRICING_MODE_BASIC, &[]))
            .await;
        assert!(
            !deferral.member("pendingPricingPlan").is_null(),
            "case {case:?}: leaving the bundle must leave a change pending, got {deferral:?}"
        );

        let superseding = emulator
            .update_pricing_plan(request(row.mode, &row.bundles))
            .await;
        assert_eq!(
            superseding.status, 200,
            "case {case:?}: expected 200 from the superseding change, got {superseding:?}"
        );
        let member = if row.deferred {
            "pendingPricingPlan"
        } else {
            "currentPricingPlan"
        };
        let plan = superseding.member(member);
        assert_eq!(
            plan["updateReason"].as_str(),
            Some(UPDATE_REASON_OVERWRITTEN),
            "case {case:?}: expected {member}.updateReason {UPDATE_REASON_OVERWRITTEN:?}, \
             got {plan:?}"
        );
        assert_eq!(
            plan["pricingMode"].as_str(),
            Some(row.mode),
            "case {case:?}: expected {member}.pricingMode {:?}, got {plan:?}",
            row.mode
        );
        if !row.deferred {
            assert!(
                superseding.member("pendingPricingPlan").is_null(),
                "case {case:?}: a superseded change is gone, not pending, got {superseding:?}"
            );
        }

        // Past the instant the superseded change would have fallen due: it
        // never becomes the plan in force.
        tokio::time::sleep(past_the_pending_span()).await;
        let read = emulator.get_pricing_plan().await;
        let current = read.member("currentPricingPlan");
        assert_eq!(
            current["pricingMode"].as_str(),
            Some(row.mode),
            "case {case:?}: the plan in force is the superseding change's, got {read:?}"
        );
        assert_eq!(
            current["updateReason"].as_str(),
            Some(UPDATE_REASON_OVERWRITTEN),
            "case {case:?}: the plan in force keeps the reason it was recorded with, got {read:?}"
        );
        assert!(
            read.member("pendingPricingPlan").is_null(),
            "case {case:?}: nothing is left pending, got {read:?}"
        );
    }
}

/// RULE: a pending change that HAS taken effect is the plan in force, so the next
/// change is read against it rather than superseding it — the reason the next
/// change records describes the move from the plan that landed.
#[tokio::test]
async fn a_change_that_has_taken_effect_is_what_the_next_change_is_read_against() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .update_pricing_plan(request("TIERED_BUNDLE", &[BUNDLE]))
            .await
            .status,
        200,
        "the bundle the account starts on must be bought"
    );
    let deferral = emulator
        .update_pricing_plan(request(PRICING_MODE_BASIC, &[]))
        .await;
    assert!(
        !deferral.member("pendingPricingPlan").is_null(),
        "leaving the bundle must leave a change pending, got {deferral:?}"
    );
    tokio::time::sleep(past_the_pending_span()).await;

    // The pay-as-you-go plan is in force by now, so asking for it again moves
    // no mode and supersedes nothing.
    let resp = emulator
        .update_pricing_plan(request(PRICING_MODE_BASIC, &[]))
        .await;
    assert_eq!(
        resp.status, 200,
        "expected 200 from the change after the pending one landed, got {resp:?}"
    );
    let current = resp.member("currentPricingPlan");
    assert_eq!(
        current["pricingMode"].as_str(),
        Some(PRICING_MODE_BASIC),
        "the change must name the pay-as-you-go plan, got {resp:?}"
    );
    assert_eq!(
        current["updateReason"].as_str(),
        Some(UPDATE_REASON_PRICING_TIER_UPDATE),
        "a change read against a landed plan supersedes nothing, so its reason is not \
         {UPDATE_REASON_OVERWRITTEN:?}, got {:?}",
        current["updateReason"]
    );
    assert!(
        resp.member("pendingPricingPlan").is_null(),
        "nothing is pending after a change that takes effect at once, got {resp:?}"
    );
    let effective = epoch_secs(
        "a landed change",
        "currentPricingPlan.effectiveDateTime",
        &current["effectiveDateTime"],
    );
    let updated = epoch_secs(
        "a landed change",
        "currentPricingPlan.updateDateTime",
        &current["updateDateTime"],
    );
    assert!(
        (effective - updated).abs() < 0.001,
        "a change onto the plan already in force takes effect at once, got effectiveDateTime \
         {effective} against updateDateTime {updated}"
    );
}
