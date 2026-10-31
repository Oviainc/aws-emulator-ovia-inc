// SPDX-License-Identifier: CC0-1.0

//! `UpdateBillingTransferPreference` for a call that restates the preference the billing
//! transfer already carries, end to end over the service's real `restJson1` wire through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] — the same path the production
//! router serves.
//!
//! RULE: `LastModifiedTime` is "the most recent time when the preference was modified"
//! (`smithy_query(query="shape",shape="UpdateBillingTransferPreferenceOutput")`), so it
//! moves only when the call actually modified the preference the store holds: a request
//! carrying the `Enabled` and the pricing plan the transfer already holds modified
//! nothing, and both the answer and a following `GetBillingTransferPreference` report the
//! instant the earlier call stamped. A request asking for an `Enabled` or a plan the
//! transfer does not hold moves the instant into the window of its own call, and the new
//! preference reads back.
//!
//! Both payload members are `@required`
//! (`smithy_query(query="shape",shape="UpdateBillingTransferPreferenceInput")`), so
//! restating what is stored is this operation's only no-op form. The rows differ along
//! which part of the preference the second call restates or moves (`Enabled` alone, the
//! plan alone, both at once), and along how the plan is spelled — by its full ARN, by its
//! bare 10-character id, or as one of the plans Amazon Web Services owns, the spellings
//! the modelled `PricingPlanArn` pattern admits and the service normalises to one stored
//! value — beside the form of the transfer ARN (both directions, the shortest and the
//! longest organization id and transfer id the modelled `ResponsibilityTransferArn`
//! pattern admits). Each row then restates what its second call LEFT, so the no-op rule
//! is asserted on every state the table reaches.
//!
//! `tests/all/billing_transfer_preference.rs` owns this pair's core loop and refusals;
//! `tests/all/update_billing_group.rs` and `tests/all/pricing_rule_unchanged_update.rs`
//! own the same instant rule for the sibling resources.

use serde_json::{json, Value};

use crate::billing_transfer_preference::{
    assert_reports, bare_id, managed_plan_arn, transfer_arn, Emulator, AWS_MANAGED_PLAN_IDS,
};

/// Wall-clock millis since the epoch, read from the same clock the service stamps
/// `LastModifiedTime` from, so a row can bracket the instant a call must report.
fn now_millis() -> i64 {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::now_millis()
}

/// How long a row waits before its second call, so that a moved instant is observable:
/// the clock the service stamps from has millisecond resolution.
const CLOCK_TICK: std::time::Duration = std::time::Duration::from_millis(5);

/// One row of the table: the preference the transfer starts holding, the preference the
/// second call asks for, whether that asks for anything the transfer does not already
/// hold, and the preference a read must report afterwards.
struct Row {
    label: &'static str,
    /// The billing transfer this row's preference belongs to.
    transfer: String,
    /// The `AutoBillingTransferBillingGroupCreation` the first call records.
    first: Value,
    /// The `AutoBillingTransferBillingGroupCreation` the second call carries.
    second: Value,
    /// Does the second call ask for anything the stored preference does not hold?
    modifies: bool,
    /// `Enabled` as a read must report it once the second call is served.
    enabled: bool,
    /// The FULL pricing plan ARN a read must report afterwards, if any.
    plan: Option<String>,
}

fn rows(plan_one: &str, plan_two: &str) -> Vec<Row> {
    let managed_one = managed_plan_arn(AWS_MANAGED_PLAN_IDS[0]);
    let managed_two = managed_plan_arn(AWS_MANAGED_PLAN_IDS[1]);
    vec![
        Row {
            label: "the identical enabled preference, the plan by the same full ARN",
            transfer: transfer_arn("restatedabc", "inbound", "11112222"),
            first: json!({ "Enabled": true, "PricingPlanArn": plan_one }),
            second: json!({ "Enabled": true, "PricingPlanArn": plan_one }),
            modifies: false,
            enabled: true,
            plan: Some(plan_one.to_string()),
        },
        Row {
            label: "the same plan named by its bare id instead of its ARN",
            transfer: transfer_arn("restatedabc", "outbound", "33334444"),
            first: json!({ "Enabled": true, "PricingPlanArn": plan_one }),
            second: json!({ "Enabled": true, "PricingPlanArn": bare_id(plan_one) }),
            modifies: false,
            enabled: true,
            plan: Some(plan_one.to_string()),
        },
        Row {
            label: "the same plan named by its full ARN after it was set by its bare id",
            transfer: transfer_arn("0123456789abcdefghij0123456789ab", "inbound", "0a1b2c3d"),
            first: json!({ "Enabled": true, "PricingPlanArn": bare_id(plan_two) }),
            second: json!({ "Enabled": true, "PricingPlanArn": plan_two }),
            modifies: false,
            enabled: true,
            plan: Some(plan_two.to_string()),
        },
        Row {
            label: "the identical disabled preference, which names no plan at all",
            transfer: transfer_arn("restateddis", "outbound", "0123456789abcdef0123456789abcdef"),
            first: json!({ "Enabled": false }),
            second: json!({ "Enabled": false }),
            modifies: false,
            enabled: false,
            plan: None,
        },
        Row {
            label: "an AWS-managed plan restated by its full ARN after its bare id",
            transfer: transfer_arn("restatedmgd", "inbound", "55556666"),
            first: json!({ "Enabled": true, "PricingPlanArn": AWS_MANAGED_PLAN_IDS[0] }),
            second: json!({ "Enabled": true, "PricingPlanArn": &managed_one }),
            modifies: false,
            enabled: true,
            plan: Some(managed_one.clone()),
        },
        Row {
            label: "the preference repointed at another plan of this account",
            transfer: transfer_arn("movedplanab", "outbound", "77778888"),
            first: json!({ "Enabled": true, "PricingPlanArn": plan_one }),
            second: json!({ "Enabled": true, "PricingPlanArn": plan_two }),
            modifies: true,
            enabled: true,
            plan: Some(plan_two.to_string()),
        },
        Row {
            label: "the preference repointed from one AWS-managed plan at the other",
            transfer: transfer_arn("movedmanaged", "inbound", "9999aaaa"),
            first: json!({ "Enabled": true, "PricingPlanArn": &managed_one }),
            second: json!({ "Enabled": true, "PricingPlanArn": AWS_MANAGED_PLAN_IDS[1] }),
            modifies: true,
            enabled: true,
            plan: Some(managed_two.clone()),
        },
        Row {
            label: "the preference switched off, which drops the plan it named",
            transfer: transfer_arn("switchedoff", "outbound", "bbbbcccc"),
            first: json!({ "Enabled": true, "PricingPlanArn": plan_one }),
            second: json!({ "Enabled": false }),
            modifies: true,
            enabled: false,
            plan: None,
        },
        Row {
            label: "the preference switched on, which takes a plan it did not name",
            transfer: transfer_arn("switchedon0", "inbound", "ddddeeee"),
            first: json!({ "Enabled": false }),
            second: json!({ "Enabled": true, "PricingPlanArn": plan_one }),
            modifies: true,
            enabled: true,
            plan: Some(plan_one.to_string()),
        },
    ]
}

/// The `LastModifiedTime` of one 200 answer, asserting the body reports `enabled` /
/// `plan` for `transfer` and carries the instant a recorded preference always has.
fn stamped_instant(case: &str, body: &Value, row: &Row) -> i64 {
    let reported = assert_reports(
        case,
        body,
        &row.transfer,
        row.enabled,
        row.plan.as_deref(),
    );
    reported.unwrap_or_else(|| {
        panic!("case {case:?}: a recorded preference must carry LastModifiedTime: {body:?}")
    })
}

/// RULE: the instant a reader sees moves only when the call modified the preference.
#[tokio::test]
async fn last_modified_time_moves_only_when_the_update_changed_the_preference() {
    let em = Emulator::new();
    let plan_one = em.create_plan("setup", "RestatedPreferencePlanOne").await;
    let plan_two = em.create_plan("setup", "RestatedPreferencePlanTwo").await;
    let rows = rows(&plan_one, &plan_two);

    // The transfers this organization has, as the Organizations surface that minted them
    // would have recorded: an operation addressing a transfer it does not have answers
    // 404 instead.
    for row in &rows {
        em.designate_transfer(&row.transfer);
    }

    for row in &rows {
        let case = row.label;

        // The preference the row starts from, and the instant that first call stamped.
        let first = em.set(case, &row.transfer, &row.first, None).await;
        let opening = first["LastModifiedTime"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: the first call must stamp LastModifiedTime: {first:?}")
        });
        tokio::time::sleep(CLOCK_TICK).await;

        let before_call = now_millis();
        let answer = em.set(case, &row.transfer, &row.second, None).await;
        let after_call = now_millis();
        let reported = stamped_instant(case, &answer, row);
        if row.modifies {
            assert!(
                (before_call..=after_call).contains(&reported),
                "case {case:?}: expected a call that modified the preference to stamp \
                 LastModifiedTime inside its own window [{before_call}, {after_call}], got \
                 {reported}"
            );
            assert!(
                reported > opening,
                "case {case:?}: expected LastModifiedTime to move forward from {opening}, got \
                 {reported}"
            );
        } else {
            assert_eq!(
                reported, opening,
                "case {case:?}: expected a call that modified nothing to leave LastModifiedTime \
                 at {opening}, got {reported} in {answer:?}"
            );
        }

        // The answer is what a later reader sees, out of state rather than echoed.
        let read = em.read_back(case, &row.transfer).await;
        let stored = stamped_instant(case, &read, row);
        assert_eq!(
            stored, reported,
            "case {case:?}: expected the stored preference to report the answered instant \
             {reported}, got {stored} in {read:?}"
        );

        // Whatever the second call did, restating what it LEFT modifies nothing: the
        // instant stays put, for every state this table reaches.
        tokio::time::sleep(CLOCK_TICK).await;
        let restated = em.set(case, &row.transfer, &row.second, None).await;
        let restated_instant = stamped_instant(case, &restated, row);
        assert_eq!(
            restated_instant, reported,
            "case {case:?}: expected restating the stored preference to leave LastModifiedTime \
             at {reported}, got {restated_instant} in {restated:?}"
        );
        let read_again = em.read_back(case, &row.transfer).await;
        assert_eq!(
            read_again, read,
            "case {case:?}: a restatement must leave the stored preference exactly as it was, \
             got {read_again:?} after {read:?}"
        );
    }
}

/// RULE, on the other side of the same comparison: a restatement of one transfer's
/// preference is not a write anywhere, so the preference of every OTHER transfer — and
/// the instant it carries — is left alone, however many restatements are served.
#[tokio::test]
async fn a_restated_preference_leaves_every_other_transfer_alone() {
    let em = Emulator::new();
    let plan = em.create_plan("setup", "RestatementNeighbourPlan").await;

    // The neighbours differ in direction, in whether their preference is enabled and in
    // how the plan they name is spelled, so the one restated transfer is surrounded by
    // every form of stored record.
    let neighbours = [
        (
            "a disabled neighbour",
            transfer_arn("neighbourab", "inbound", "10102020"),
            json!({ "Enabled": false }),
            false,
            None,
        ),
        (
            "a neighbour enabled by the plan's full ARN",
            transfer_arn("neighbourab", "outbound", "30304040"),
            json!({ "Enabled": true, "PricingPlanArn": &plan }),
            true,
            Some(plan.clone()),
        ),
        (
            "a neighbour enabled by the plan's bare id",
            transfer_arn("neighbourcd", "inbound", "50506060"),
            json!({ "Enabled": true, "PricingPlanArn": bare_id(&plan) }),
            true,
            Some(plan.clone()),
        ),
    ];
    let subject = transfer_arn("restatedsub", "outbound", "70708080");
    em.designate_transfer(&subject);
    for (_, transfer, _, _, _) in &neighbours {
        em.designate_transfer(transfer);
    }

    let mut instants = Vec::new();
    for (case, transfer, preference, _, _) in &neighbours {
        let written = em.set(case, transfer, preference, None).await;
        instants.push(written["LastModifiedTime"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: the recorded preference must carry LastModifiedTime")
        }));
    }
    let preference = json!({ "Enabled": true, "PricingPlanArn": &plan });
    let subject_instant = em
        .set("the restated transfer", &subject, &preference, None)
        .await["LastModifiedTime"]
        .as_i64()
        .expect("the recorded preference must carry LastModifiedTime");
    tokio::time::sleep(CLOCK_TICK).await;

    // A run of restatements, the operation's only no-op form.
    for _ in 0..3 {
        em.set("the restated transfer", &subject, &preference, None)
            .await;
    }

    let read = em.read_back("the restated transfer", &subject).await;
    assert_eq!(
        assert_reports("the restated transfer", &read, &subject, true, Some(&plan)),
        Some(subject_instant),
        "expected the restated transfer to keep LastModifiedTime at {subject_instant}, got \
         {read:?}"
    );
    for ((case, transfer, _, enabled, plan), instant) in neighbours.iter().zip(instants) {
        let read = em.read_back(case, transfer).await;
        assert_eq!(
            assert_reports(case, &read, transfer, *enabled, plan.as_deref()),
            Some(instant),
            "case {case:?}: expected the neighbour's LastModifiedTime to stay at {instant}, got \
             {read:?}"
        );
    }
}
