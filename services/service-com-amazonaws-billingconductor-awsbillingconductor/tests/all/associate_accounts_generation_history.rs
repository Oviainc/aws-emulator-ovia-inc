// SPDX-License-Identifier: CC0-1.0

//! `AssociateAccounts` and the billing group's GENERATION HISTORY, end to end over the
//! service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same
//! path the production router serves.
//!
//! RULE: a billing group's current generation is in force from the billing period its
//! `LastModifiedTime` falls in, so a write that moves that instant into a LATER period
//! closes the generation it displaces and freezes it as it stood, and opens a generation
//! of its own from the period it was made in; a write that lands inside the current
//! generation's own period replaces that generation, and a call that changes nothing
//! moves nothing. Every period therefore keeps reporting the values the group really
//! carried then (`Name`, `Description`, `ComputationPreference.PricingPlanArn`,
//! `Status`, `AccountGrouping.AutoAssociate`, `LastModifiedTime`), however many writes
//! came after it and whatever each of them changed.
//!
//! `tests/all/membership_writes_keep_superseded_generations.rs` owns the rule for the
//! generation a membership write FREEZES, over the membership writes of both resources.
//! This suite is about the generation `AssociateAccounts` OPENS: a group whose history
//! is three deep because an association stands between two value-changing writes, which
//! is what shows that the association neither lost the generation before it nor merged
//! into the one after it. `tests/all/billing_group_billing_periods.rs` owns the same rule
//! for `UpdateBillingGroup`.
//!
//! The wire cannot write in a PAST billing period — a write happens now — so a row writes
//! through the real wire and then backdates the stored instants, which is the one thing
//! the test cannot ask the service for. That is the harness
//! `tests/all/billing_group_billing_periods.rs` already carries, driven from here.

use serde_json::{json, Value};

use crate::billing_group_billing_periods::{
    assert_members, current_period, primary_account_id, Emulator,
};
use crate::pricing_rule_association_periods::{period_start_millis, shift_period};

/// `AssociateAccounts`, at the method its modelled HTTP binding names
/// (`smithy_query(query="operation",operation="AssociateAccounts")`: a `POST`).
const ASSOCIATE_ACCOUNTS_PATH: &str = "/associate-accounts";

/// The `index`-th account the `row`-th group here does not hold yet, so a row may bring
/// it in. One per row and per slot, because an account belongs to at most one billing
/// group.
fn newcomer_account_id(row: usize, index: usize) -> String {
    format!("{:012}", 555500000000_u64 + (row as u64) * 10 + index as u64)
}

/// One row of the association table: the accounts the `AssociateAccounts` call names,
/// what the group holds once it is served, and whether the call changes the membership
/// at all.
struct AssociationRow {
    label: &'static str,
    /// The `AccountIds` the call names. The row's position in the table is also the
    /// index of the group it addresses, so no two rows can name one account.
    accounts: Vec<String>,
    /// `Size` the group reports in the current billing period once the call is served.
    size_after: i64,
    /// Does the call change the membership? One that changes nothing leaves the group's
    /// current generation — and its `LastModifiedTime` — exactly where they were.
    changes: bool,
}

/// The table: rows differ along how many accounts the association brings in (0 is the
/// lower bound of the modelled `AccountIdList`, `len 0..30`), and along whether each
/// named account is one the group already holds — the two things that decide whether the
/// call changes the membership at all.
fn association_rows() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            label: "one account the group does not hold yet",
            accounts: vec![newcomer_account_id(0, 0)],
            size_after: 2,
            changes: true,
        },
        AssociationRow {
            label: "two new accounts at once",
            accounts: vec![newcomer_account_id(1, 0), newcomer_account_id(1, 1)],
            size_after: 3,
            changes: true,
        },
        AssociationRow {
            label: "three new accounts at once",
            accounts: vec![
                newcomer_account_id(2, 0),
                newcomer_account_id(2, 1),
                newcomer_account_id(2, 2),
            ],
            size_after: 4,
            changes: true,
        },
        AssociationRow {
            label: "no account at all",
            accounts: Vec::new(),
            size_after: 1,
            changes: false,
        },
        AssociationRow {
            label: "only an account the group already holds",
            accounts: vec![primary_account_id(4)],
            size_after: 1,
            changes: false,
        },
        AssociationRow {
            label: "a new account beside one it already holds",
            accounts: vec![primary_account_id(5), newcomer_account_id(5, 0)],
            size_after: 2,
            changes: true,
        },
    ]
}

/// The values the group's FIRST generation carried: the ones the create gave it.
fn created_values(name: &str, plan: &str) -> Value {
    json!({"Name": name, "Description": "first", "Status": "ACTIVE",
           "ComputationPreference": {"PricingPlanArn": plan},
           "AccountGrouping": {"AutoAssociate": false}})
}

/// The values the generation the association displaces carried: every member a write can
/// change, moved off the created ones.
fn renamed_values(name: &str, plan: &str) -> Value {
    json!({"Name": format!("{name}-Renamed"), "Description": "second", "Status": "PENDING",
           "ComputationPreference": {"PricingPlanArn": plan},
           "AccountGrouping": {"AutoAssociate": true}})
}

/// The values the write made AFTER the association left on the group.
fn latest_values(name: &str, plan: &str) -> Value {
    json!({"Name": format!("{name}-Latest"), "Description": "third", "Status": "ACTIVE",
           "ComputationPreference": {"PricingPlanArn": plan},
           "AccountGrouping": {"AutoAssociate": false}})
}

/// `expected` with `LastModifiedTime` laid on: the instant a generation's own billing
/// period begins, which is what a backdated generation reports.
fn with_last_modified(expected: &Value, at: i64) -> Value {
    let mut expected = expected.clone();
    expected["LastModifiedTime"] = json!(at);
    expected
}

/// `expected` with `Size` laid on.
fn with_size(expected: &Value, size: i64) -> Value {
    let mut expected = expected.clone();
    expected["Size"] = json!(size);
    expected
}

/// The `BillingGroupListElement` the listing reports for `group` in `period`, which it
/// must report at all for a period the group existed in.
async fn element(em: &Emulator, case: &str, group: &str, period: Option<&str>) -> Value {
    em.element_in(case, group, period)
        .await
        .unwrap_or_else(|| panic!("case {case}: the group existed then, so it is reported"))
}

/// `LastModifiedTime` as the listing reports the group for `period`.
async fn last_modified_in(em: &Emulator, case: &str, group: &str, period: &str) -> i64 {
    let reported = element(em, case, group, Some(period)).await;
    reported["LastModifiedTime"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: no LastModifiedTime in {reported:?}"))
}

#[tokio::test]
async fn an_association_opens_its_own_generation_and_keeps_the_ones_around_it() {
    let em = Emulator::new();
    let created_plan = em.create_plan("AssociationHistoryCreatedPlan").await;
    let renamed_plan = em.create_plan("AssociationHistoryRenamedPlan").await;
    let latest_plan = em.create_plan("AssociationHistoryLatestPlan").await;
    let now = current_period();
    // Four billing periods: the group is created two years back, a value-changing write a
    // year back left the generation the association must not lose, the association itself
    // lands half a year back, and a last value-changing write happens NOW.
    let created_in = shift_period(&now, -24);
    let renamed_in = shift_period(&now, -12);
    let associated_in = shift_period(&now, -6);

    for (index, row) in association_rows().into_iter().enumerate() {
        let case = row.label;
        let name = format!("AssociationHistory-{index}");
        let primary = primary_account_id(index);
        let created = created_values(&name, &created_plan);
        let renamed = renamed_values(&name, &renamed_plan);
        let latest = latest_values(&name, &latest_plan);

        let group = em
            .create_group(
                &name,
                &created_plan,
                &primary,
                &json!({"Description": "first", "AccountGrouping": {"AutoAssociate": false}}),
            )
            .await;
        em.created_in(&group, &created_in);

        // The generation the association will displace: a write a year after the create,
        // carrying values of its own for every member a generation holds.
        em.update_group(case, &group, &renamed).await;
        em.generation_started_in(&group, &renamed_in);

        // The call under test, through the real wire.
        let (status, body) = em
            .post(
                ASSOCIATE_ACCOUNTS_PATH,
                &json!({"Arn": group, "AccountIds": row.accounts}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected AssociateAccounts to be served with 200, got {status} \
             with {body:?}"
        );
        assert_eq!(
            body["Arn"],
            json!(group),
            "case {case}: expected the answer to name the billing group {group}, got {body:?}"
        );

        // Straight after the association: the generation it displaced is still reported,
        // frozen as it stood, for every period it was in force in, and so is the one
        // before it.
        for period in [renamed_in.clone(), shift_period(&renamed_in, 1)] {
            let case = format!("{case}, read at {period:?} after the association");
            let reported = element(&em, &case, &group, Some(&period)).await;
            assert_members(
                &case,
                &reported,
                &with_last_modified(&renamed, period_start_millis(&renamed_in)),
            );
        }
        for period in [created_in.clone(), shift_period(&created_in, 1)] {
            let case = format!("{case}, read at {period:?} after the association");
            let reported = element(&em, &case, &group, Some(&period)).await;
            assert_members(
                &case,
                &reported,
                &with_last_modified(&created, period_start_millis(&created_in)),
            );
        }

        // The current period reports the membership the association left, with the values
        // the generation it opened inherited — an association changes no member a
        // generation carries — whether the read names that period or none at all.
        for period in [None, Some(now.clone())] {
            let case = format!("{case}, read at {period:?} after the association");
            let reported = element(&em, &case, &group, period.as_deref()).await;
            assert_members(&case, &reported, &with_size(&renamed, row.size_after));
        }

        // An association that changed the membership moved `LastModifiedTime` into the
        // period it was made in — which is what makes the generation above one it had to
        // freeze; one that changed nothing left it in that generation's own period.
        let modified_case = format!("{case}, LastModifiedTime after the association");
        let modified_now = last_modified_in(&em, &modified_case, &group, &now).await;
        let renamed_start = period_start_millis(&renamed_in);
        if row.changes {
            assert!(
                modified_now > renamed_start,
                "case {modified_case}: expected an association that changed the membership \
                 to move LastModifiedTime past {renamed_start}, got {modified_now}"
            );
            // Backdate the generation the association opened, as if the call had been
            // made in a period of its own between the two value-changing writes.
            em.generation_started_in(&group, &associated_in);
        } else {
            assert_eq!(
                modified_now, renamed_start,
                "case {modified_case}: expected an association that changed nothing to \
                 leave LastModifiedTime at {renamed_start}, got {modified_now}"
            );
        }

        // A last value-changing write, NOW: it closes whatever generation the association
        // left, so the history the reads below walk is the whole one.
        em.update_group(case, &group, &latest).await;

        // The generation the association OPENED answers for its own period and the ones
        // after it, with the values it inherited and its own `LastModifiedTime` — the
        // association's period when the association moved the group, and the earlier
        // generation's period when it changed nothing and so opened no generation.
        let association_start = if row.changes {
            period_start_millis(&associated_in)
        } else {
            renamed_start
        };
        for period in [associated_in.clone(), shift_period(&associated_in, 1)] {
            let case = format!("{case}, read at {period:?} after the later write");
            let reported = element(&em, &case, &group, Some(&period)).await;
            assert_members(
                &case,
                &reported,
                &with_last_modified(&renamed, association_start),
            );
        }

        // And the generations around it are untouched: the whole history survives both
        // writes rather than only its newest entry.
        for period in [renamed_in.clone(), shift_period(&renamed_in, 1)] {
            let case = format!("{case}, read at {period:?} after the later write");
            let reported = element(&em, &case, &group, Some(&period)).await;
            assert_members(&case, &reported, &with_last_modified(&renamed, renamed_start));
        }
        for period in [created_in.clone(), shift_period(&created_in, 1)] {
            let case = format!("{case}, read at {period:?} after the later write");
            let reported = element(&em, &case, &group, Some(&period)).await;
            assert_members(
                &case,
                &reported,
                &with_last_modified(&created, period_start_millis(&created_in)),
            );
        }

        // The current period reports the last write's values over the membership the
        // association left, and nothing a write cannot change moved with any of it.
        let case = format!("{case}, read at the current period after the later write");
        let reported = element(&em, &case, &group, Some(&now)).await;
        assert_members(&case, &reported, &with_size(&latest, row.size_after));
        assert_members(
            &case,
            &reported,
            &json!({"Arn": group, "PrimaryAccountId": primary,
                    "BillingGroupType": "STANDARD",
                    "CreationTime": period_start_millis(&created_in)}),
        );
    }
}
