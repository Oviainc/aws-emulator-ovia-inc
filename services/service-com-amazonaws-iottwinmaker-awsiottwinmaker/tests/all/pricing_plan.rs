// SPDX-License-Identifier: CC0-1.0

//! `GetPricingPlan`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so the route, the status code and the
//! serialized body are all exercised, not just a handler body.
//!
//! The tests assert RULES over tables of inputs rather than one recorded case:
//!
//! * the reported `currentPricingPlan`/`pendingPricingPlan` is the plan the store
//!   holds — its mode, its update reason, its bundles and tier, and its instants —
//!   for EVERY value the model's `PricingMode`, `UpdateReason` and `PricingTier`
//!   enums define, not only the ones a recording happened to carry;
//! * `bundleInformation` is reported exactly when the stored plan names bundles,
//!   since `BundleInformation::bundleNames` is `@required` `@length(1..10)`;
//! * `billableEntityCount` equals the number of entities the service currently
//!   holds, so it moves with a create and a delete in any workspace;
//! * an account whose plan nobody has changed reads as the service default
//!   (`BASIC`, `updateReason` `DEFAULT`, no bundles, nothing pending), and reading
//!   it does not change what a later caller sees.
//!
//! A stored plan is seeded through the service's own store (the same store the
//! handlers read) because `UpdatePricingPlan`, the only operation that writes one,
//! is still a scaffold stub with an issue of its own; the entity counts, by
//! contrast, are built with real `CreateEntity`/`DeleteEntity` calls.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    PRICING_MODE_VALUES, PRICING_TIER_VALUES, UPDATE_REASON_VALUES,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    PricingPlanRecord, PRICING_MODE_BASIC, UPDATE_REASON_DEFAULT,
};

use crate::harness::Emulator;

/// An instant a whole number of seconds after the epoch, so a seeded plan's
/// timestamps are exact on the wire rather than compared with a tolerance.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// A stored plan, spelled out member by member.
fn plan(
    pricing_mode: &str,
    update_reason: &str,
    bundle_names: &[&str],
    pricing_tier: Option<&str>,
    effective_secs: u64,
    update_secs: u64,
) -> PricingPlanRecord {
    PricingPlanRecord {
        pricing_mode: pricing_mode.to_string(),
        update_reason: update_reason.to_string(),
        bundle_names: bundle_names.iter().map(|name| (*name).to_string()).collect(),
        pricing_tier: pricing_tier.map(str::to_string),
        effective_date_time: at(effective_secs),
        update_date_time: at(update_secs),
    }
}

/// The epoch seconds a restJson1 `Timestamp` member carries, which the protocol
/// serializes as an epoch-seconds number.
fn epoch_secs(case: &str, member: &str, value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an epoch-seconds number, got {value:?}")
    })
}

/// Assert that the reported plan `reported` is the stored plan `expected`,
/// carrying the billable entity count `expected_count`.
fn assert_reports_plan(
    case: &str,
    member: &str,
    reported: &Value,
    expected: &PricingPlanRecord,
    expected_count: i64,
) {
    assert_eq!(
        reported["pricingMode"].as_str(),
        Some(expected.pricing_mode.as_str()),
        "case {case:?}: wrong {member}.pricingMode, got {:?}",
        reported["pricingMode"]
    );
    assert_eq!(
        reported["updateReason"].as_str(),
        Some(expected.update_reason.as_str()),
        "case {case:?}: wrong {member}.updateReason, got {:?}",
        reported["updateReason"]
    );
    assert_eq!(
        reported["billableEntityCount"].as_i64(),
        Some(expected_count),
        "case {case:?}: expected {member}.billableEntityCount {expected_count}, got {:?}",
        reported["billableEntityCount"]
    );

    let effective = epoch_secs(
        case,
        &format!("{member}.effectiveDateTime"),
        &reported["effectiveDateTime"],
    );
    let expected_effective = at_secs(expected.effective_date_time);
    assert!(
        (effective - expected_effective).abs() < 0.001,
        "case {case:?}: expected {member}.effectiveDateTime {expected_effective}, got {effective}"
    );
    let updated = epoch_secs(
        case,
        &format!("{member}.updateDateTime"),
        &reported["updateDateTime"],
    );
    let expected_updated = at_secs(expected.update_date_time);
    assert!(
        (updated - expected_updated).abs() < 0.001,
        "case {case:?}: expected {member}.updateDateTime {expected_updated}, got {updated}"
    );

    let bundle_information = &reported["bundleInformation"];
    if expected.bundle_names.is_empty() {
        assert!(
            bundle_information.is_null(),
            "case {case:?}: a plan with no bundles must report no {member}.bundleInformation, \
             got {bundle_information:?}"
        );
        return;
    }
    let reported_names: Vec<String> = bundle_information["bundleNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported_names, expected.bundle_names,
        "case {case:?}: wrong {member}.bundleInformation.bundleNames, got {reported_names:?}"
    );
    assert_eq!(
        bundle_information["pricingTier"].as_str(),
        expected.pricing_tier.as_deref(),
        "case {case:?}: wrong {member}.bundleInformation.pricingTier, got {:?}",
        bundle_information["pricingTier"]
    );
}

/// A stored instant as epoch seconds, the unit the wire carries.
fn at_secs(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a stored instant is after the epoch")
        .as_secs_f64()
}

/// Seed the account's stored plan (and pending plan) the way
/// `UpdatePricingPlan` will once it is implemented.
fn seed(emulator: &Emulator, current: &PricingPlanRecord, pending: Option<&PricingPlanRecord>) {
    let state = emulator.state();
    let mut state = state.lock();
    state.pricing_plan = current.clone();
    state.pending_pricing_plan = pending.cloned();
}

/// An account nobody has changed the plan of is on the service default, and
/// reading it neither invents instants that move nor changes what the next
/// caller sees.
#[tokio::test]
async fn an_untouched_account_reads_as_the_default_plan() {
    let emulator = Emulator::new();
    let resp = emulator.get_pricing_plan().await;
    assert_eq!(
        resp.status, 200,
        "an untouched account: expected 200 from GetPricingPlan, got {resp:?}"
    );
    let current = resp.member("currentPricingPlan").clone();
    assert_eq!(
        current["pricingMode"].as_str(),
        Some(PRICING_MODE_BASIC),
        "an untouched account: expected the default pricingMode, got {:?}",
        current["pricingMode"]
    );
    assert_eq!(
        current["updateReason"].as_str(),
        Some(UPDATE_REASON_DEFAULT),
        "an untouched account: expected the default updateReason, got {:?}",
        current["updateReason"]
    );
    assert!(
        current["bundleInformation"].is_null(),
        "an untouched account: the default plan names no bundles, got {:?}",
        current["bundleInformation"]
    );
    assert_eq!(
        current["billableEntityCount"].as_i64(),
        Some(0),
        "an untouched account: expected billableEntityCount 0, got {:?}",
        current["billableEntityCount"]
    );
    assert!(
        resp.member("pendingPricingPlan").is_null(),
        "an untouched account: nothing is pending, got {:?}",
        resp.member("pendingPricingPlan")
    );

    let effective = epoch_secs(
        "an untouched account",
        "currentPricingPlan.effectiveDateTime",
        &current["effectiveDateTime"],
    );
    let now = at_secs(SystemTime::now());
    assert!(
        effective > 0.0 && effective <= now,
        "an untouched account: the default plan must already be in effect, got \
         effectiveDateTime {effective} against now {now}"
    );

    // A report must not move what a later caller sees: a second read answers the
    // same plan, instants included.
    let again = emulator.get_pricing_plan().await;
    assert_eq!(
        again.body, resp.body,
        "an untouched account: a second GetPricingPlan must report the same plan, got {again:?}"
    );
}

/// The reported plan is the STORED plan, over a table of plans differing in
/// mode, update reason, bundle count, tier and instants — and in whether a plan
/// change is pending.
#[tokio::test]
async fn the_reported_plan_is_the_stored_plan() {
    // The largest bundle list `PricingBundles` admits (@length(1..10)).
    let ten_bundles: Vec<String> = (0..10).map(|i| format!("bundle-{i}")).collect();
    let ten_bundles: Vec<&str> = ten_bundles.iter().map(String::as_str).collect();

    let cases: Vec<(&str, PricingPlanRecord, Option<PricingPlanRecord>)> = vec![
        (
            "the default plan, explicitly stored",
            PricingPlanRecord::default_plan(at(1_700_000_000)),
            None,
        ),
        (
            "standard, one bundle, no tier",
            plan(
                "STANDARD",
                "PRICING_MODE_UPDATE",
                &["twinmaker-basic"],
                None,
                1_700_000_100,
                1_700_000_050,
            ),
            None,
        ),
        (
            "tiered bundle, three bundles, TIER_3",
            plan(
                "TIERED_BUNDLE",
                "PRICING_TIER_UPDATE",
                &["twinmaker-basic", "twinmaker-matterport", "twinmaker-scene"],
                Some("TIER_3"),
                1_700_100_000,
                1_700_099_999,
            ),
            None,
        ),
        (
            "tiered bundle at the modelled maximum of ten bundles",
            plan(
                "TIERED_BUNDLE",
                "OVERWRITTEN",
                &ten_bundles,
                Some("TIER_4"),
                1_800_000_000,
                1_799_999_000,
            ),
            None,
        ),
        (
            "a pending change away from a tiered bundle",
            plan(
                "TIERED_BUNDLE",
                "ENTITY_COUNT_UPDATE",
                &["twinmaker-basic"],
                Some("TIER_1"),
                1_700_200_000,
                1_700_200_000,
            ),
            Some(plan(
                "STANDARD",
                "PRICING_MODE_UPDATE",
                &[],
                None,
                1_700_300_000,
                1_700_250_000,
            )),
        ),
        (
            "a pending change into a bundle, from a bundle-less plan",
            plan("BASIC", "DEFAULT", &[], None, 1_600_000_000, 1_600_000_000),
            Some(plan(
                "TIERED_BUNDLE",
                "PRICING_TIER_UPDATE",
                &["twinmaker-basic", "twinmaker-matterport"],
                Some("TIER_2"),
                1_900_000_000,
                1_850_000_000,
            )),
        ),
    ];

    for (case, current, pending) in cases {
        let emulator = Emulator::new();
        seed(&emulator, &current, pending.as_ref());

        let resp = emulator.get_pricing_plan().await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from GetPricingPlan, got {resp:?}"
        );
        assert_reports_plan(
            case,
            "currentPricingPlan",
            resp.member("currentPricingPlan"),
            &current,
            0,
        );
        match &pending {
            None => assert!(
                resp.member("pendingPricingPlan").is_null(),
                "case {case:?}: nothing is pending, got {:?}",
                resp.member("pendingPricingPlan")
            ),
            Some(pending) => assert_reports_plan(
                case,
                "pendingPricingPlan",
                resp.member("pendingPricingPlan"),
                pending,
                0,
            ),
        }
    }
}

/// Every value the model defines for `PricingMode`, `UpdateReason` and
/// `PricingTier` survives the round trip verbatim — the catalogs come from the
/// model's own enums (`ops::types::PRICING_MODE_VALUES`,
/// `UPDATE_REASON_VALUES`, `PRICING_TIER_VALUES`), so no reported value is a
/// case keyed to one recorded request.
#[tokio::test]
async fn every_modelled_enum_value_round_trips() {
    for mode in PRICING_MODE_VALUES {
        for reason in UPDATE_REASON_VALUES {
            let case = format!("{mode} / {reason}");
            let emulator = Emulator::new();
            let stored = plan(mode, reason, &[], None, 1_700_000_000, 1_700_000_000);
            seed(&emulator, &stored, None);
            let resp = emulator.get_pricing_plan().await;
            assert_eq!(
                resp.status, 200,
                "case {case:?}: expected 200 from GetPricingPlan, got {resp:?}"
            );
            assert_reports_plan(
                &case,
                "currentPricingPlan",
                resp.member("currentPricingPlan"),
                &stored,
                0,
            );
        }
    }

    for tier in PRICING_TIER_VALUES {
        let case = format!("tier {tier}");
        let emulator = Emulator::new();
        let stored = plan(
            "TIERED_BUNDLE",
            "PRICING_TIER_UPDATE",
            &["twinmaker-basic"],
            Some(tier),
            1_700_000_000,
            1_700_000_000,
        );
        seed(&emulator, &stored, None);
        let resp = emulator.get_pricing_plan().await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from GetPricingPlan, got {resp:?}"
        );
        assert_reports_plan(
            &case,
            "currentPricingPlan",
            resp.member("currentPricingPlan"),
            &stored,
            0,
        );
    }
}

/// `billableEntityCount` is the number of entities the service currently holds,
/// across every workspace: it moves with a create and a delete, and a workspace
/// on its own is not a billable entity.
#[tokio::test]
async fn the_billable_entity_count_follows_the_stored_entities() {
    let emulator = Emulator::new();
    // A stored plan with known instants, so the walk below can also show that
    // counting entities leaves the PLAN itself untouched.
    let stored = plan(
        "STANDARD",
        "ENTITY_COUNT_UPDATE",
        &["twinmaker-basic"],
        None,
        1_700_000_000,
        1_700_000_000,
    );
    seed(&emulator, &stored, None);

    /// One step of the walk: what to do, then the count the plan must report.
    enum Step {
        CreateWorkspace(&'static str),
        CreateEntity(&'static str, &'static str),
        DeleteEntity(&'static str, &'static str),
    }

    let steps: Vec<(&str, Option<Step>, i64)> = vec![
        ("an empty account", None, 0),
        (
            "one workspace, no entities",
            Some(Step::CreateWorkspace("alpha")),
            0,
        ),
        (
            "the first entity",
            Some(Step::CreateEntity("alpha", "e1")),
            1,
        ),
        (
            "a second entity in the same workspace",
            Some(Step::CreateEntity("alpha", "e2")),
            2,
        ),
        (
            "a second workspace, still empty",
            Some(Step::CreateWorkspace("beta")),
            2,
        ),
        (
            "an entity in the second workspace counts too",
            Some(Step::CreateEntity("beta", "e3")),
            3,
        ),
        (
            "deleting an entity drops the count",
            Some(Step::DeleteEntity("alpha", "e1")),
            2,
        ),
        (
            "deleting the last entity of a workspace drops it too",
            Some(Step::DeleteEntity("beta", "e3")),
            1,
        ),
    ];

    for (case, step, expected_count) in steps {
        match step {
            None => {}
            Some(Step::CreateWorkspace(workspace_id)) => {
                let resp = emulator.create_workspace(workspace_id).await;
                assert_eq!(
                    resp.status, 200,
                    "case {case:?}: expected 200 from CreateWorkspace, got {resp:?}"
                );
            }
            Some(Step::CreateEntity(workspace_id, entity_id)) => {
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
            Some(Step::DeleteEntity(workspace_id, entity_id)) => {
                let resp = emulator.delete_entity(workspace_id, entity_id, false).await;
                assert_eq!(
                    resp.status, 200,
                    "case {case:?}: expected 200 from DeleteEntity, got {resp:?}"
                );
            }
        }

        let resp = emulator.get_pricing_plan().await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: expected 200 from GetPricingPlan, got {resp:?}"
        );
        assert_reports_plan(
            case,
            "currentPricingPlan",
            resp.member("currentPricingPlan"),
            &stored,
            expected_count,
        );
    }
}

/// The billable entity count describes the ACCOUNT, so a pending plan is
/// reported with the same count as the current one.
#[tokio::test]
async fn a_pending_plan_carries_the_same_billable_count() {
    let emulator = Emulator::new();
    let current = plan(
        "BASIC",
        "DEFAULT",
        &[],
        None,
        1_700_000_000,
        1_700_000_000,
    );
    let pending = plan(
        "TIERED_BUNDLE",
        "PRICING_TIER_UPDATE",
        &["twinmaker-basic"],
        Some("TIER_1"),
        1_900_000_000,
        1_700_000_500,
    );
    seed(&emulator, &current, Some(&pending));

    assert_eq!(
        emulator.create_workspace("counted").await.status,
        200,
        "the seeded workspace must be created"
    );
    for entity_id in ["e1", "e2", "e3"] {
        let resp = emulator
            .create_entity(
                "counted",
                json!({"entityId": entity_id, "entityName": entity_id}),
            )
            .await;
        assert_eq!(
            resp.status, 200,
            "entity {entity_id:?}: expected 200 from CreateEntity, got {resp:?}"
        );
    }

    let resp = emulator.get_pricing_plan().await;
    assert_eq!(
        resp.status, 200,
        "a pending plan: expected 200 from GetPricingPlan, got {resp:?}"
    );
    assert_reports_plan(
        "a pending plan: current",
        "currentPricingPlan",
        resp.member("currentPricingPlan"),
        &current,
        3,
    );
    assert_reports_plan(
        "a pending plan: pending",
        "pendingPricingPlan",
        resp.member("pendingPricingPlan"),
        &pending,
        3,
    );
}
