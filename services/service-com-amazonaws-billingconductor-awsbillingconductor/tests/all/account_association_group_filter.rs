// SPDX-License-Identifier: CC0-1.0

//! The billing-group `Association` filter of `ListAccountAssociations`, end to end over
//! the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the
//! same path the production router serves.
//!
//! RULE: a listing's filter is a PREDICATE over the answer that listing reports, not a
//! lookup of a resource that has to exist — so an `Association` carrying a billing group
//! ARN ("Billing Group Arn: linked accounts that are associated to the provided Billing
//! Group Arn", `smithy_query(query="doc",shape="ListAccountAssociationsFilter",
//! member="Association")`) selects exactly the accounts the same read reports with that
//! `BillingGroupArn` for the billing period asked about, and selects NO account, rather
//! than refusing the read, when no membership of that period names it.
//!
//! The rows differ along what the rule generalises over: how the membership the filter
//! has to agree with ended (standing, disassociated, ended by a billing group that was
//! then DELETED, ended in an earlier period, never made at all), the billing period the
//! read asks about (before the membership, its first period, inside, the period it ended
//! in, after it, and none at all), the spelling of the ARN the filter carries (the full
//! ARN and the bare id, which the `Association` pattern equally admits), and — for the
//! ARNs nothing resolves — the partition, the account segment and the id length the same
//! pattern admits.
//!
//! The harness is `tests/all/account_association_billing_periods.rs`: the same emulator,
//! the same per-period reads and the same membership backdating the wire cannot ask for,
//! since a membership in a PAST period can only be made by the store.

use serde_json::json;
use std::collections::BTreeSet;

use crate::account_association_billing_periods::{current_period, Emulator};
use crate::pricing_rule_association_periods::shift_period;
/// The bare-id spelling of an ARN has ONE home in this suite
/// (`tests/all/tagging_surface.rs`), so the rule is not written twice: the
/// `Association` pattern makes the
/// `arn:aws(-cn)?:billingconductor::<account>:billinggroup/` prefix optional, exactly as
/// the tagging `Arn` pattern does for a custom line item
/// (`smithy_query(query="shape",shape="ListAccountAssociationsFilter")`).
use crate::tagging_surface::bare_id;

/// One account of the world, the billing group it was put in, and how its membership
/// ENDED — the dimension the rule must not depend on.
struct Row {
    /// The name of the case, carried into every assertion message.
    how: &'static str,
    /// Months before the current billing period the membership began in.
    began: i64,
    /// Months before the current billing period the membership ended in, when the store
    /// is what ended it; `None` when a call below ends it, or nothing does.
    ended: Option<i64>,
    /// The call that ends the membership at the period it is made in: a
    /// `DisassociateAccounts`, a `DeleteBillingGroup`, or neither.
    ending_call: Option<&'static str>,
}

/// The world's rows. Every row owns one account and one billing group, so the filter of
/// each group has an answer that differs from every other row's.
const ROWS: &[Row] = &[
    Row {
        how: "a standing membership",
        began: -12,
        ended: None,
        ending_call: None,
    },
    Row {
        how: "a membership ended by a disassociation",
        began: -12,
        ended: None,
        ending_call: Some("disassociate"),
    },
    Row {
        how: "a membership ended by deleting the billing group",
        began: -12,
        ended: None,
        ending_call: Some("delete"),
    },
    Row {
        how: "a membership that ended in an earlier period",
        began: -12,
        ended: Some(-6),
        ending_call: None,
    },
    Row {
        how: "a billing group no account was ever put in",
        began: 0,
        ended: None,
        ending_call: Some("never associated"),
    },
];

/// The account id of row `index`, in the 12-digit spelling the `AccountId` pattern
/// requires.
fn account_of(index: usize) -> String {
    format!("3000000000{:02}", index + 1)
}

/// Build the world the rows describe and answer each row's billing group ARN.
async fn build(em: &Emulator, plan: &str) -> Vec<String> {
    let now = current_period();
    let mut groups = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let account = account_of(index);
        let group = em
            .create_group(&format!("GroupFilter{index}"), plan, &[])
            .await;
        if row.ending_call != Some("never associated") {
            em.associate(&group, &[account.as_str()]).await;
            let start = shift_period(&now, row.began);
            let end = row.ended.map(|months| shift_period(&now, months));
            em.membership_ran(&account, &start, end.as_deref());
        }
        // A billing group is reported only from the period it was created in onward, so
        // the group has to exist in every period a row reads — and it has to be
        // backdated BEFORE the row's ending call, since a deleted group is no longer
        // there to backdate.
        em.group_created_in(&group, &shift_period(&now, row.began - 1));
        match row.ending_call {
            Some("disassociate") => em.disassociate(&group, &[account.as_str()]).await,
            Some("delete") => em.delete_group(&group).await,
            _ => {}
        }
        groups.push(group);
    }
    groups
}

/// The periods the rows are read at: a year before the memberships began, the period
/// they began in, the one in between, the period the earliest of them ended in, the
/// month before now, now, and a year on.
fn read_periods(now: &str) -> Vec<Option<String>> {
    let mut periods: Vec<Option<String>> = [-13, -12, -9, -6, -1, 0, 12]
        .iter()
        .map(|months| Some(shift_period(now, *months)))
        .collect();
    // An absent `BillingPeriod` means the current one, so the filter answers there too.
    periods.push(None);
    periods
}

/// RULE: the billing-group `Association` filter selects exactly the accounts the same
/// listing reports with that `BillingGroupArn` for the period asked about — in either
/// spelling of the ARN, whatever ended the membership and whether or not the group still
/// exists.
#[tokio::test]
async fn the_group_filter_selects_what_the_listing_reports_for_that_period() {
    let em = Emulator::new();
    let plan = em.create_plan("GroupFilterPlan").await;
    let groups = build(&em, &plan).await;
    let now = current_period();

    for period in read_periods(&now) {
        // What the listing itself reports for each account in this period — the answer
        // every filter below has to agree with. The row that associated no account has
        // none to read: an account this service has never seen is in no listing at all.
        let mut reported: Vec<(String, Option<String>)> = Vec::new();
        for (index, row) in ROWS.iter().enumerate() {
            if row.ending_call == Some("never associated") {
                continue;
            }
            let account = account_of(index);
            let case = format!("{} in the listing at {period:?}", row.how);
            let group = em.association_in(&case, &account, period.as_deref()).await;
            reported.push((account, group));
        }

        let mut monitored_by_group: BTreeSet<String> = BTreeSet::new();
        for (index, row) in ROWS.iter().enumerate() {
            let group = &groups[index];
            let expected: Vec<String> = reported
                .iter()
                .filter(|(_, reported_group)| reported_group.as_deref() == Some(group.as_str()))
                .map(|(account, _)| account.clone())
                .collect();
            monitored_by_group.extend(expected.iter().cloned());

            // Either spelling of one group's ARN names one group, so both select the
            // same accounts.
            for spelling in [group.clone(), bare_id(group).to_string()] {
                let case = format!("{} filtered by {spelling:?} at {period:?}", row.how);
                let selected = em
                    .filtered_accounts(&case, &spelling, period.as_deref())
                    .await;
                assert_eq!(
                    selected, expected,
                    "case {case}: the filter must select what the listing reports, expected \
                     {expected:?}, got {selected:?}"
                );
            }
        }

        // The group filters partition what `MONITORED` reports: an account is monitored
        // in a period exactly when some group's filter holds it then.
        let case = format!("MONITORED at {period:?}");
        let monitored: BTreeSet<String> = em
            .filtered_accounts(&case, "MONITORED", period.as_deref())
            .await
            .into_iter()
            .collect();
        assert_eq!(
            monitored, monitored_by_group,
            "case {case}: MONITORED must be exactly what the group filters select, expected \
             {monitored_by_group:?}, got {monitored:?}"
        );
    }
}

/// An `Association` value that is a well-formed billing group ARN no group of this world
/// answers to, with the dimension of the `Association` pattern it varies along. Source:
/// `smithy_query(query="shape",shape="ListAccountAssociationsFilter")` — the member's
/// pattern `^((arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}
/// |MONITORED|UNMONITORED)$`.
const UNRESOLVED_ASSOCIATIONS: &[(&str, &str)] = &[
    ("a bare id at the pattern's shortest", "ZZZZZZZZZZ"),
    ("a bare id at the pattern's longest", "ZZZZZZZZZZZZ"),
    ("a bare id of mixed case and digits", "aB3dE6gH9j"),
    (
        "a full ARN in this partition",
        "arn:aws:billingconductor::123456789012:billinggroup/QQQQQQQQQQ",
    ),
    (
        "a full ARN in another account",
        "arn:aws:billingconductor::210987654321:billinggroup/QQQQQQQQQQQQ",
    ),
    (
        "a full ARN in the other partition the pattern admits",
        "arn:aws-cn:billingconductor::123456789012:billinggroup/QQQQQQQQQQ",
    ),
];

/// RULE: an `Association` no membership of the period being read names selects no
/// account and the read still succeeds — the same answer the `Arns` filter of
/// `ListBillingGroups` and the `BillingGroupArns` filter of
/// `ListBillingGroupCostReports` give for an ARN nothing resolves.
#[tokio::test]
async fn a_group_filter_nothing_resolves_selects_no_account_rather_than_refusing() {
    let em = Emulator::new();
    let plan = em.create_plan("UnresolvedFilterPlan").await;
    let groups = build(&em, &plan).await;
    let now = current_period();

    // Rows the pattern admits that name no billing group this world ever minted.
    let mut cases: Vec<(String, String, Option<String>)> = UNRESOLVED_ASSOCIATIONS
        .iter()
        .map(|(case, association)| {
            (
                (*case).to_string(),
                (*association).to_string(),
                Some(now.clone()),
            )
        })
        .collect();
    // And the ARNs of groups that DO exist in the store, read at a period no membership
    // of theirs covers: the same answer, reached from the other side.
    for (index, row) in ROWS.iter().enumerate() {
        cases.push((
            format!("{}, read before it began", row.how),
            groups[index].clone(),
            Some(shift_period(&now, row.began - 1)),
        ));
    }
    // The group DELETED above, read at the current period: its memberships ended with
    // it, so nothing is in it now — and the group is no longer in the store at all,
    // which is the case that must not become a refusal.
    let deleted = ROWS
        .iter()
        .position(|row| row.ending_call == Some("delete"))
        .expect("one row deletes its billing group");
    cases.push((
        "the deleted billing group, read now".to_string(),
        groups[deleted].clone(),
        None,
    ));

    for (case, association, period) in cases {
        for spelling in [association.clone(), bare_id(&association).to_string()] {
            let case = format!("{case} filtered by {spelling:?}");
            let selected = em
                .filtered_accounts(&case, &spelling, period.as_deref())
                .await;
            assert!(
                selected.is_empty(),
                "case {case}: an Association no membership of the period names selects no \
                 account, got {selected:?}"
            );
        }
    }
}

/// RULE: it is the modelled `Association` pattern, enforced before the handler runs,
/// that refuses a value that is no billing group ARN at all — one character past either
/// published bound of the bare id's `{10,12}` length is a 400 `ValidationException`,
/// never the 404 of a resource lookup.
#[tokio::test]
async fn an_association_outside_the_modelled_pattern_is_a_validation_exception() {
    let em = Emulator::new();

    // The bound and one past it, in both directions: 10 and 12 characters are the
    // pattern's own bounds (asserted as selecting nothing above), 9 and 13 are past
    // them, and a lower-cased MONITORED is outside the enumerated spellings.
    let refused: &[(&str, &str)] = &[
        ("one character short of the pattern's shortest id", "ZZZZZZZZZ"),
        ("one character past the pattern's longest id", "ZZZZZZZZZZZZZ"),
        ("an id carrying a character the alphabet excludes", "ZZZZZZZZZ_"),
        ("a lower-cased MONITORED", "monitored"),
    ];
    for (case, association) in refused {
        let (status, body) = em
            .post(
                "/list-account-associations",
                &json!({"Filters": {"Association": association}}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {association:?}, got {status} with {body:?}"
        );
        // The wire error code of a failure response, without any namespace.
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.ends_with("ValidationException"),
            "case {case:?}: expected a ValidationException for {association:?}, got {body:?}"
        );
    }
}
