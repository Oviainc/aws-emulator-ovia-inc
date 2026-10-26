// SPDX-License-Identifier: CC0-1.0

//! A write that changes only a resource's MEMBERSHIP still keeps the generation it
//! supersedes, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: for a resource whose current generation is in force from the billing period its
//! `LastModifiedTime` falls in, EVERY write that moves that instant closes the generation
//! it displaces and freezes it as it stood, so a read scoped to a billing period that
//! generation was in force in still reports the resource with the values it carried then
//! — whatever the write changed, and whether or not the changed member is one a
//! generation carries. A call that changes nothing moves nothing, so the generation in
//! force stays where it was.
//!
//! The rows span the membership writes the rule generalises over rather than one call:
//! `DisassociateAccounts` and `AssociateAccounts` on a billing group (one account, two
//! accounts, and the call that changes no membership at all), and
//! `AssociatePricingRules` on a pricing plan — the other resource whose generation starts
//! at its `LastModifiedTime`. `tests/all/billing_group_billing_periods.rs` and
//! `tests/all/pricing_plan_billing_periods.rs` own the same rule for the writes that
//! change a generation's own members (`UpdateBillingGroup`, `UpdatePricingPlan`); this
//! suite is about the writes that change only what the resource HOLDS.
//!
//! The wire cannot write in a PAST billing period — a write happens now — so a row writes
//! through the real wire and then backdates the stored instants, which is the one thing
//! the test cannot ask the service for. That is the harness
//! `tests/all/billing_group_billing_periods.rs` and
//! `tests/all/pricing_rule_association_periods.rs` already carry, driven from here.

use serde_json::{json, Value};

use crate::billing_group_billing_periods::{
    assert_members, current_period, primary_account_id, Emulator as GroupEmulator,
};
use crate::pricing_rule_association_periods::{
    period_start_millis, shift_period, Emulator as PlanEmulator,
};

/// `AssociateAccounts` and `DisassociateAccounts`, at the methods their modelled HTTP
/// bindings name (`smithy_query(query="operation",operation="DisassociateAccounts")`:
/// both are `POST`).
const ASSOCIATE_ACCOUNTS_PATH: &str = "/associate-accounts";
const DISASSOCIATE_ACCOUNTS_PATH: &str = "/disassociate-accounts";

/// The `index`-th plain member of the `row`-th group here: an account of the group that
/// is not its primary account, so a row may take it out.
///
/// One per group and per slot, because an account belongs to at most one billing group.
fn member_account_id(row: usize, index: usize) -> String {
    format!("{:012}", 888800000000_u64 + (row as u64) * 10 + index as u64)
}

/// The `index`-th account the `row`-th group does not hold yet, so a row may bring it in.
fn newcomer_account_id(row: usize, index: usize) -> String {
    format!("{:012}", 777700000000_u64 + (row as u64) * 10 + index as u64)
}

/// One row of the membership-write table: the call it makes, the accounts that call
/// names, and what the call leaves behind.
struct MembershipRow {
    label: String,
    /// The route the membership write goes to.
    path: &'static str,
    /// The `AccountIds` the write names.
    accounts: Vec<String>,
    /// `Size` the group reports in the current billing period once the write is served.
    size_after: i64,
    /// Does the call change the membership at all? A call that changes nothing leaves the
    /// group's current generation — and its `LastModifiedTime` — exactly where they were.
    changes: bool,
}

/// The table: rows differ along which membership write is made, how many accounts it
/// names (0 is the lower bound of the modelled `AccountIdList`), and whether it changes
/// the membership at all.
///
/// Each row names the accounts of its OWN group — an account belongs to at most one
/// billing group, so no two rows can share one.
fn membership_rows() -> Vec<MembershipRow> {
    vec![
        MembershipRow {
            label: "DisassociateAccounts takes one member out".to_string(),
            path: DISASSOCIATE_ACCOUNTS_PATH,
            accounts: vec![member_account_id(0, 0)],
            size_after: 2,
            changes: true,
        },
        MembershipRow {
            label: "DisassociateAccounts takes both plain members out".to_string(),
            path: DISASSOCIATE_ACCOUNTS_PATH,
            accounts: vec![member_account_id(1, 0), member_account_id(1, 1)],
            size_after: 1,
            changes: true,
        },
        MembershipRow {
            label: "DisassociateAccounts names no account at all".to_string(),
            path: DISASSOCIATE_ACCOUNTS_PATH,
            accounts: Vec::new(),
            size_after: 3,
            changes: false,
        },
        MembershipRow {
            label: "AssociateAccounts brings one new account in".to_string(),
            path: ASSOCIATE_ACCOUNTS_PATH,
            accounts: vec![newcomer_account_id(3, 0)],
            size_after: 4,
            changes: true,
        },
        MembershipRow {
            label: "AssociateAccounts brings two new accounts in".to_string(),
            path: ASSOCIATE_ACCOUNTS_PATH,
            accounts: vec![newcomer_account_id(4, 0), newcomer_account_id(4, 1)],
            size_after: 5,
            changes: true,
        },
        MembershipRow {
            label: "AssociateAccounts restates a membership the group already holds".to_string(),
            path: ASSOCIATE_ACCOUNTS_PATH,
            accounts: vec![member_account_id(5, 0)],
            size_after: 3,
            changes: false,
        },
    ]
}

/// `LastModifiedTime` as the listing reports the group for `period`.
async fn last_modified_in(em: &GroupEmulator, case: &str, group: &str, period: &str) -> i64 {
    let element = em
        .element_in(case, group, Some(period))
        .await
        .unwrap_or_else(|| panic!("case {case}: the group existed then, so it is reported"));
    element["LastModifiedTime"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: no LastModifiedTime in {element:?}"))
}

#[tokio::test]
async fn a_billing_group_membership_write_keeps_the_generation_it_supersedes() {
    let em = GroupEmulator::new();
    let first_plan = em.create_plan("MembershipFirstPlan").await;
    let second_plan = em.create_plan("MembershipSecondPlan").await;
    let now = current_period();
    // Three billing periods: the group is created two years back, a write a year back
    // left the generation the membership write must not lose, and the membership write
    // itself happens NOW — in a third, later period.
    let first = shift_period(&now, -24);
    let second = shift_period(&now, -12);

    for (index, row) in membership_rows().into_iter().enumerate() {
        let case = row.label.clone();
        let name = format!("Membership-{index}");
        let primary = primary_account_id(index);
        let members = json!([
            primary.clone(),
            member_account_id(index, 0),
            member_account_id(index, 1),
        ]);
        let group = em
            .create_group(
                &name,
                &first_plan,
                &primary,
                &json!({"Description": "first",
                        "AccountGrouping": {"LinkedAccountIds": members}}),
            )
            .await;
        em.created_in(&group, &first);

        // The second generation: a write that lands a year after the create, carrying
        // values of its own for every member a generation holds.
        em.update_group(
            &case,
            &group,
            &json!({"Name": format!("{name}-Renamed"), "Description": "second", "Status": "PENDING",
                    "ComputationPreference": {"PricingPlanArn": second_plan}}),
        )
        .await;
        em.generation_started_in(&group, &second);

        // The membership write under test, in a third billing period.
        let (status, body) = em
            .post(row.path, &json!({"Arn": group, "AccountIds": row.accounts}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the membership write to be served with 200, got {status} \
             with {body:?}"
        );

        // The generation the membership write displaced is still there, frozen as it
        // stood: every period it was in force in reports its values and its own
        // `LastModifiedTime`.
        for period in [second.clone(), shift_period(&second, 1)] {
            let case = format!("{case}, read at {period:?}");
            let element = em
                .element_in(&case, &group, Some(&period))
                .await
                .unwrap_or_else(|| {
                    panic!("case {case}: the group existed then, so it is reported")
                });
            assert_members(
                &case,
                &element,
                &json!({"Name": format!("{name}-Renamed"), "Description": "second",
                        "Status": "PENDING",
                        "ComputationPreference": {"PricingPlanArn": second_plan},
                        "LastModifiedTime": period_start_millis(&second)}),
            );
        }

        // The generation before it is untouched too, so the whole history survives the
        // write rather than only its newest entry.
        for period in [first.clone(), shift_period(&first, 1)] {
            let case = format!("{case}, read at {period:?}");
            let element = em
                .element_in(&case, &group, Some(&period))
                .await
                .unwrap_or_else(|| {
                    panic!("case {case}: the group existed then, so it is reported")
                });
            assert_members(
                &case,
                &element,
                &json!({"Name": name, "Description": "first", "Status": "ACTIVE",
                        "ComputationPreference": {"PricingPlanArn": first_plan},
                        "LastModifiedTime": period_start_millis(&first)}),
            );
        }

        // The current period reports the membership the write left, with the values the
        // generation it opened inherited — a membership write changes no member a
        // generation carries — whether the read names that period or none at all.
        for period in [None, Some(now.clone())] {
            let case = format!("{case}, read at {period:?}");
            let element = em
                .element_in(&case, &group, period.as_deref())
                .await
                .unwrap_or_else(|| panic!("case {case}: the group exists now, so it is reported"));
            assert_members(
                &case,
                &element,
                &json!({"Name": format!("{name}-Renamed"), "Description": "second",
                        "Status": "PENDING",
                        "ComputationPreference": {"PricingPlanArn": second_plan},
                        "Size": row.size_after}),
            );
        }

        // A write that changed the membership moved `LastModifiedTime` into the period it
        // was made in; one that changed nothing left it in the earlier generation's
        // period.
        let case = format!("{case}, LastModifiedTime now");
        let modified_now = last_modified_in(&em, &case, &group, &now).await;
        let generation_start = period_start_millis(&second);
        if row.changes {
            assert!(
                modified_now > generation_start,
                "case {case}: expected a write that changed the membership to move \
                 LastModifiedTime past {generation_start}, got {modified_now}"
            );
        } else {
            assert_eq!(
                modified_now, generation_start,
                "case {case}: expected a write that changed nothing to leave \
                 LastModifiedTime at {generation_start}, got {modified_now}"
            );
        }
    }
}

#[tokio::test]
async fn a_pricing_rule_association_keeps_the_pricing_plan_generation_it_supersedes() {
    let em = PlanEmulator::new();
    let now = current_period();
    let first = shift_period(&now, -24);
    let second = shift_period(&now, -12);

    let first_rule = em.create_rule("AssociationGenerationFirstRule").await;
    let second_rule = em.create_rule("AssociationGenerationSecondRule").await;
    let plan = em
        .create_plan(
            "AssociationGenerationPlan",
            std::slice::from_ref(&first_rule),
        )
        .await;
    em.create_plan_in(&plan, &first);

    // The second generation: a write a year after the create, then backdated so the
    // association below lands in a third, later billing period.
    crate::pricing_plan_billing_periods::update_plan(
        &em,
        "an association keeps the plan's generations",
        &plan,
        &json!({"Name": "AssociationGenerationPlan-Renamed", "Description": "second"}),
    )
    .await;
    em.restate_plan_generation_in(&plan, &second);

    // The write under test: `AssociatePricingRules` changes only what the plan
    // associates, and moves its `LastModifiedTime`.
    em.associate(&plan, std::slice::from_ref(&second_rule)).await;

    // Rows differ along where the period falls among the plan's generations: the first
    // generation's own period and the month after it, the second generation's period and
    // the month after it, and the current period the association landed in.
    let rows: Vec<(String, Value)> = vec![
        (first.clone(), json!({"Name": "AssociationGenerationPlan"})),
        (
            shift_period(&first, 1),
            json!({"Name": "AssociationGenerationPlan"}),
        ),
        (
            second.clone(),
            json!({"Name": "AssociationGenerationPlan-Renamed", "Description": "second",
                   "LastModifiedTime": period_start_millis(&second)}),
        ),
        (
            shift_period(&second, 1),
            json!({"Name": "AssociationGenerationPlan-Renamed", "Description": "second",
                   "LastModifiedTime": period_start_millis(&second)}),
        ),
        (
            now.clone(),
            json!({"Name": "AssociationGenerationPlan-Renamed", "Description": "second"}),
        ),
    ];

    for (period, expected) in rows {
        let case = format!("an association read at {period:?}");
        let element = crate::pricing_plan_billing_periods::element_in(
            &em,
            &case,
            &plan,
            Some(period.as_str()),
        )
        .await
        .unwrap_or_else(|| panic!("case {case}: the plan existed then, so it is reported"));
        assert_members(&case, &element, &expected);
    }

    // The association itself moved the plan's `LastModifiedTime` into the period it was
    // made in, which is what makes the generation above one the write had to freeze.
    let case = format!("an association read at {now:?}");
    let element =
        crate::pricing_plan_billing_periods::element_in(&em, &case, &plan, Some(now.as_str()))
            .await
            .unwrap_or_else(|| panic!("case {case}: the plan exists now, so it is reported"));
    let modified_now = element["LastModifiedTime"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: no LastModifiedTime in {element:?}"));
    let generation_start = period_start_millis(&second);
    assert!(
        modified_now > generation_start,
        "case {case}: expected the association to move LastModifiedTime past \
         {generation_start}, got {modified_now}"
    );
}
