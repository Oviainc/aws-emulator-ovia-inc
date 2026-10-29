// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateBillingGroup` that modifies NOTHING does to a billing group's
//! GENERATION HISTORY, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: whether a call modified the group is decided by comparing the values a READ of
//! it reports before and after, not the internal form those values are kept in — so a
//! request naming only values the group already reports modifies nothing, which means it
//! leaves `LastModifiedTime` where it was and opens no generation of the group. A write
//! that lands in a billing period later than the one the current generation started in
//! closes that generation and spends a slot of the bounded version history every
//! period-scoped read is answered from, so a call that changed nothing must not reach
//! that write at all.
//!
//! `tests/all/update_billing_group.rs` owns the `LastModifiedTime` half of the rule over
//! every member class a request can name; this suite is about the second observable
//! consequence — the generation — which only shows up when the call lands in a LATER
//! billing period than the generation it would displace.
//!
//! The wire cannot write in a PAST billing period — a write happens now — so each row
//! writes through the real wire and then backdates the stored instants through the
//! harness `tests/all/billing_group_billing_periods.rs` already carries, which is the one
//! thing the test cannot ask the service for.

use serde_json::{json, Value};

use crate::billing_group_billing_periods::{
    assert_members, current_period, primary_account_id, Emulator,
};
use crate::pricing_rule_association_periods::{period_start_millis, shift_period};

/// `POST /update-billing-group` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="UpdateBillingGroup")`).
const UPDATE_GROUP_PATH: &str = "/update-billing-group";

/// One row: the `Status` values earlier served calls already put the group through, the
/// members the call under test names beside `Arn`, and whether that call modifies the
/// group.
struct Row {
    label: &'static str,
    /// The `Status` each prior `UpdateBillingGroup` asked for, in order — the history
    /// behind the status the group REPORTS when the call under test arrives.
    prior_statuses: &'static [&'static str],
    /// The members the call under test names beside `Arn`.
    patch: fn() -> Value,
    /// Does the call under test modify the group?
    modifies: bool,
}

/// The table: rows differ along whether the call names a value the group already reports
/// and along the history behind that reported value — a status no caller ever set, one
/// set to what the group already reported, and one set away from it — plus a control row
/// that really does move the group, so the row that changes nothing is measured against a
/// write that must still freeze its generation.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "the Status the group already reports, never having been set",
            prior_statuses: &[],
            patch: || json!({"Status": "ACTIVE"}),
            modifies: false,
        },
        Row {
            label: "the Status the group already reports, set once before",
            prior_statuses: &["ACTIVE"],
            patch: || json!({"Status": "ACTIVE"}),
            modifies: false,
        },
        Row {
            label: "the Status the group already reports, away and back before",
            prior_statuses: &["PENDING", "ACTIVE"],
            patch: || json!({"Status": "ACTIVE"}),
            modifies: false,
        },
        Row {
            label: "no member at all",
            prior_statuses: &[],
            patch: || json!({}),
            modifies: false,
        },
        Row {
            label: "a Status the group does not report",
            prior_statuses: &[],
            patch: || json!({"Status": "PENDING"}),
            modifies: true,
        },
        Row {
            label: "a Description the group does not hold",
            prior_statuses: &["ACTIVE"],
            patch: || json!({"Description": "moved on"}),
            modifies: true,
        },
    ]
}

#[tokio::test]
async fn a_call_that_modifies_nothing_opens_no_generation() {
    let em = Emulator::new();
    let plan = em.create_plan("NoOpGenerationPlan").await;
    let now = current_period();
    // The group and its only generation live a year back, so the call under test lands in
    // a LATER billing period than the generation it would displace — the condition under
    // which a write freezes a generation at all.
    let created_in = shift_period(&now, -12);
    let created_at = period_start_millis(&created_in);

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.label;
        let name = format!("NoOpGeneration{index}");
        let primary = primary_account_id(index);
        let group = em
            .create_group(
                &name,
                &plan,
                &primary,
                &json!({"Description": "as created", "AccountGrouping": {"AutoAssociate": false}}),
            )
            .await;
        // The status history this row starts from, through the real wire, and then the
        // whole group backdated: creation, the generation the call under test meets, and
        // the history those prior writes left behind.
        for prior in row.prior_statuses {
            em.update_group(case, &group, &json!({"Status": prior}))
                .await;
        }
        em.created_in(&group, &created_in);
        let generations_before = em.superseded_generations(&group);

        let (status, answer) = em
            .post(UPDATE_GROUP_PATH, &{
                let mut body = (row.patch)();
                body["Arn"] = json!(group);
                body
            })
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateBillingGroup to be served with 200, got {status} \
             with {answer:?}"
        );

        let generations_after = em.superseded_generations(&group);
        let reported = answer["LastModifiedTime"].as_i64().unwrap_or_default();
        let listed_now = em
            .element_in(case, &group, Some(&now))
            .await
            .unwrap_or_else(|| panic!("case {case}: the group exists now, so it is reported"));
        let listed_created = em
            .element_in(case, &group, Some(&created_in))
            .await
            .unwrap_or_else(|| panic!("case {case}: the group existed then, so it is reported"));

        if row.modifies {
            assert!(
                reported > created_at,
                "case {case}: expected a call that modified the group to move \
                 LastModifiedTime past {created_at}, got {reported}"
            );
            assert_eq!(
                generations_after,
                generations_before + 1,
                "case {case}: expected a call that modified the group in a later billing \
                 period to freeze the generation it displaced, got {generations_before} \
                 generations before and {generations_after} after"
            );
            // The frozen generation still answers for its own period, with the values the
            // group really carried then.
            assert_members(
                case,
                &listed_created,
                &json!({"Description": "as created", "Status": "ACTIVE",
                        "LastModifiedTime": created_at}),
            );
        } else {
            assert_eq!(
                reported, created_at,
                "case {case}: expected a call that modified nothing to leave \
                 LastModifiedTime at {created_at}, got {reported}"
            );
            assert_eq!(
                generations_after, generations_before,
                "case {case}: expected a call that modified nothing to freeze no \
                 generation, got {generations_before} generations before and \
                 {generations_after} after"
            );
            // Both periods are answered from the one generation the group still has, so
            // every member a read reports is the same in both.
            let expected = json!({"Description": "as created", "Status": "ACTIVE",
                                  "LastModifiedTime": created_at,
                                  "CreationTime": created_at});
            assert_members(case, &listed_created, &expected);
            assert_members(case, &listed_now, &expected);
            assert!(
                listed_now
                    .get("StatusReason")
                    .unwrap_or(&Value::Null)
                    .is_null(),
                "case {case}: an ACTIVE group has no status reason, got {listed_now:?}"
            );
        }
    }
}
