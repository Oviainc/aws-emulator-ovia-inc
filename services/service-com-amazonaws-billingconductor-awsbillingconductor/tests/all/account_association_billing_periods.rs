// SPDX-License-Identifier: CC0-1.0

//! The BILLING PERIOD dimension of an account / billing group association, end to end
//! over the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] —
//! the same path the production router serves.
//!
//! RULE: a read scoped to a `BillingPeriod` answers from the account-to-billing-group
//! memberships that were in force in that month — each membership covering the
//! half-open range from the period it was made in to the period it ended in — and from
//! the current billing period when the request names none
//! (`ListAccountAssociations`: "list linked accounts that are linked to the payer
//! account for the specified time period. If no information is provided, the current
//! billing period is used"). So an account is reported with a `BillingGroupArn` for
//! the periods its membership covered and as `UNMONITORED` for every other period,
//! whether the membership ended because it was disassociated or because the group was
//! deleted; the `MONITORED`/`UNMONITORED`/billing-group `Association` filters select on
//! that same per-period answer, and `BillingGroupListElement.Size` counts the
//! membership of the period the read names.
//!
//! The rows differ along what the rule generalises over: the period a membership began
//! in (the epoch month, a December, a January, both spellings of one month the modelled
//! `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows, and a far-future month),
//! how long it lasted (one month, two, thirteen, and still standing), how it ended
//! (nothing, a disassociation, a deleted group), the period the read asks about
//! (before the start, the start itself, inside, the end, after the end, and none at
//! all) and which `Association` filter the read carries.
//!
//! The wire cannot make a membership in a PAST period — an associating call happens now
//! — so a row makes its association through the real wire and then backdates the stored
//! period range, the one thing the test cannot ask the service for (the same setup
//! `tests/all/pricing_rule_association_periods.rs` uses for a pricing rule
//! association).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The month arithmetic over a `BillingPeriod` this crate's period-scoped suites
/// share (`tests/all/pricing_rule_association_periods.rs`), so one ordering rule
/// serves every one of them.
use crate::pricing_rule_association_periods::{period_start_millis, shift_period};

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// A live emulator holding only this service, with the engine kept so a row can
/// backdate a membership the wire could only make now.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this payer-account-only
    /// control plane is reachable at all (see `tests/all/payer_account_gate.rs` for the
    /// denial every other caller gets).
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// POST a JSON body to one of this service's routes and return the status plus the
    /// parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a pricing plan and answer the ARN the service minted.
    async fn create_plan(&self, name: &str) -> String {
        let (status, body) = self.post("/create-pricing-plan", &json!({"Name": name})).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name:?}: expected 200, got {status} with {body:?}"
        );
        body["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// Create a billing group holding `linked`, and answer the ARN the service minted.
    async fn create_group(&self, name: &str, plan: &str, linked: &[&str]) -> String {
        let (status, body) = self
            .post(
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "PrimaryAccountId": "999988887777",
                    "AccountGrouping": {"LinkedAccountIds": linked},
                    "ComputationPreference": {"PricingPlanArn": plan},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {name:?}: expected 200, got {status} with {body:?}"
        );
        body["Arn"]
            .as_str()
            .expect("setup CreateBillingGroup: no Arn in the response")
            .to_string()
    }

    /// `AssociateAccounts`: add `accounts` to the group's membership.
    async fn associate(&self, group: &str, accounts: &[&str]) {
        let (status, body) = self
            .post(
                "/associate-accounts",
                &json!({"Arn": group, "AccountIds": accounts}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup AssociateAccounts on {group:?}: expected 200, got {status} with {body:?}"
        );
    }

    /// `DisassociateAccounts`: take `accounts` out of the group.
    async fn disassociate(&self, group: &str, accounts: &[&str]) {
        let (status, body) = self
            .post(
                "/disassociate-accounts",
                &json!({"Arn": group, "AccountIds": accounts}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup DisassociateAccounts on {group:?}: expected 200, got {status} with {body:?}"
        );
    }

    /// `DeleteBillingGroup`: remove the group, which ends the memberships it held.
    async fn delete_group(&self, group: &str) {
        let (status, body) = self
            .post("/delete-billing-group", &json!({"Arn": group}))
            .await;
        assert_eq!(
            status, 200,
            "setup DeleteBillingGroup {group:?}: expected 200, got {status} with {body:?}"
        );
    }

    /// Backdate `group`'s creation to `period`, as if the creating call had been made in
    /// that billing period — the setup a row needs because a billing group is reported
    /// only for the periods from the one it was created in onward
    /// (`tests/all/billing_group_billing_periods.rs` owns that rule), so the group a row
    /// reads the per-period membership of has to exist in every period it reads.
    fn group_created_in(&self, group: &str, period: &str) {
        let at = period_start_millis(period);
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let record = state
            .billing_groups
            .get_mut(group)
            .expect("setup group_created_in: the group was created above");
        record.creation_time_millis = at;
        record.last_modified_time_millis = at;
    }

    /// Backdate one account's stored membership to the half-open period range
    /// `[start, end)`, as if the associating call had been made in `start` and the
    /// membership had ended in `end` — the world no request can ask for, since a call
    /// happens now.
    fn membership_ran(&self, account_id: &str, start: &str, end: Option<&str>) {
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let memberships = state
            .account_associations
            .get_mut(account_id)
            .expect("setup membership_ran: the account was associated above");
        for membership in memberships.iter_mut() {
            membership.inclusive_start_billing_period = start.to_string();
            membership.exclusive_end_billing_period = end.map(str::to_string);
        }
    }

    /// The `BillingGroupArn` `ListAccountAssociations` reports for one account in
    /// `period` (`None` names no period at all): `Some(arn)` while it is in a group,
    /// `None` when it is `UNMONITORED` then.
    async fn association_in(
        &self,
        case: &str,
        account_id: &str,
        period: Option<&str>,
    ) -> Option<String> {
        let mut body = json!({"Filters": {"AccountId": account_id}});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self.post("/list-account-associations", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListAccountAssociations expected 200, got {status} with {answer:?}"
        );
        let listed = answer["LinkedAccounts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {case}: expected exactly one linked account, got {listed:?}"
        );
        assert_eq!(
            listed[0]["AccountId"].as_str(),
            Some(account_id),
            "case {case}: expected the account asked about, got {listed:?}"
        );
        listed[0]["BillingGroupArn"].as_str().map(str::to_string)
    }

    /// The account ids `ListAccountAssociations` reports in `period` under an
    /// `Association` filter (`MONITORED`, `UNMONITORED` or a billing group ARN).
    async fn filtered_accounts(
        &self,
        case: &str,
        association: &str,
        period: Option<&str>,
    ) -> Vec<String> {
        let mut body = json!({"Filters": {"Association": association}});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self.post("/list-account-associations", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListAccountAssociations with Association {association:?} expected 200, \
             got {status} with {answer:?}"
        );
        let mut accounts: Vec<String> = answer["LinkedAccounts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|element| element["AccountId"].as_str().unwrap_or_default().to_string())
            .collect();
        accounts.sort();
        accounts
    }

    /// The `Size` `ListBillingGroups` reports for one group in `period`.
    async fn group_size_in(&self, case: &str, group: &str, period: Option<&str>) -> i64 {
        let mut body = json!({"Filters": {"Arns": [group]}});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, answer) = self.post("/list-billing-groups", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListBillingGroups expected 200, got {status} with {answer:?}"
        );
        answer["BillingGroups"][0]["Size"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: ListBillingGroups reported no Size: {answer:?}"))
    }
}

/// The periods a membership began in. Rows differ along the month form the modelled
/// `BillingPeriod` pattern allows (`0?[1-9]`, so `2026-7` and `2026-07` are ONE
/// period), the end-of-year and start-of-year months whose neighbours cross a year, the
/// epoch month, and a far-future one.
const START_PERIODS: &[&str] = &["1970-01", "1999-12", "2026-1", "2026-07", "2030-3", "9980-12"];

/// RULE: a membership that still stands is reported from the period it began in onward
/// and for no earlier period — in the `BillingGroupArn` of the listing, in the
/// `MONITORED`/`UNMONITORED` filters and in the group's `Size`.
#[tokio::test]
async fn a_standing_membership_is_reported_from_the_period_it_began_in_onward() {
    let em = Emulator::new();
    let plan = em.create_plan("StandingPlan").await;
    for (index, start) in START_PERIODS.iter().enumerate() {
        let account = format!("{:012}", index + 1);
        let group = em
            .create_group(&format!("StandingGroup{index}"), &plan, &[])
            .await;
        em.associate(&group, &[account.as_str()]).await;
        em.membership_ran(&account, start, None);
        // The group has to exist in every period this row reads, since a read reports a
        // group only for the periods from the one it was created in onward.
        em.group_created_in(&group, &shift_period(start, -13));

        // Rows differ along the period the read asks about: a year before, the month
        // before, the start itself, the month after and a year after.
        let reads: Vec<(String, bool)> = vec![
            (shift_period(start, -13), false),
            (shift_period(start, -1), false),
            ((*start).to_string(), true),
            (shift_period(start, 1), true),
            (shift_period(start, 13), true),
        ];
        for (period, in_force) in reads {
            let case = format!("membership from {start:?} read at {period:?}");
            let expected = in_force.then(|| group.clone());
            let reported = em.association_in(&case, &account, Some(&period)).await;
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?}, got {reported:?}"
            );

            // The filters select on the same per-period answer.
            let monitored = em.filtered_accounts(&case, "MONITORED", Some(&period)).await;
            assert_eq!(
                monitored.contains(&account),
                in_force,
                "case {case}: MONITORED must hold {account} only in force, got {monitored:?}"
            );
            let unmonitored = em
                .filtered_accounts(&case, "UNMONITORED", Some(&period))
                .await;
            assert_eq!(
                unmonitored.contains(&account),
                !in_force,
                "case {case}: UNMONITORED must hold {account} only out of force, got \
                 {unmonitored:?}"
            );
            let in_group = em.filtered_accounts(&case, &group, Some(&period)).await;
            assert_eq!(
                in_group.contains(&account),
                in_force,
                "case {case}: the group's own filter must hold {account} only in force, got \
                 {in_group:?}"
            );

            // `Size` counts the membership of the period the read names.
            let size = em.group_size_in(&case, &group, Some(&period)).await;
            let expected_size = i64::from(in_force);
            assert_eq!(
                size, expected_size,
                "case {case}: expected Size {expected_size}, got {size}"
            );
        }

        // Both spellings of ONE month name one period, so they answer the same.
        let (year, month) = start
            .split_once('-')
            .expect("a start period is spelled YYYY-MM");
        let month: u32 = month.parse().expect("a start period has a numeric month");
        for spelling in [format!("{year}-{month}"), format!("{year}-{month:02}")] {
            let case = format!("membership from {start:?} read at {spelling:?}");
            let reported = em.association_in(&case, &account, Some(&spelling)).await;
            assert_eq!(
                reported.as_deref(),
                Some(group.as_str()),
                "case {case}: either spelling of the start period reports the membership, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: a membership that ENDED covers the half-open range of periods it ran for — the
/// period it began in up to, but not including, the period it ended in — so the account
/// is reported in that group for those periods and `UNMONITORED` for every other.
#[tokio::test]
async fn a_membership_that_ended_covers_the_periods_it_ran_for_and_no_other() {
    let em = Emulator::new();
    let plan = em.create_plan("EndedPlan").await;

    // Rows differ along the period the membership began in and how many periods it ran
    // for: one month, two, and thirteen (crossing a year).
    let spans: Vec<(&str, i64)> = vec![
        ("1970-01", 1),
        ("1999-12", 2),
        ("2026-1", 13),
        ("2026-07", 1),
        ("2030-3", 2),
        ("9980-12", 13),
    ];
    for (index, (start, months)) in spans.into_iter().enumerate() {
        let account = format!("{:012}", 100 + index);
        let group = em
            .create_group(&format!("EndedGroup{index}"), &plan, &[])
            .await;
        em.associate(&group, &[account.as_str()]).await;
        let end = shift_period(start, months);
        em.membership_ran(&account, start, Some(&end));
        // The group has to exist in every period this row reads — see
        // [`Emulator::group_created_in`].
        em.group_created_in(&group, &shift_period(start, -1));

        // Rows differ along where in or out of the range the read falls: before the
        // start, the start, the last period the membership covers, the period it ended
        // in (excluded) and a year after that.
        let reads: Vec<(String, bool)> = vec![
            (shift_period(start, -1), false),
            ((*start).to_string(), true),
            (shift_period(start, months - 1), true),
            (end.clone(), false),
            (shift_period(&end, 12), false),
        ];
        for (period, in_force) in reads {
            let case = format!("membership {start:?}..{end:?} read at {period:?}");
            let expected = in_force.then(|| group.clone());
            let reported = em.association_in(&case, &account, Some(&period)).await;
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?}, got {reported:?}"
            );
            let size = em.group_size_in(&case, &group, Some(&period)).await;
            let expected_size = i64::from(in_force);
            assert_eq!(
                size, expected_size,
                "case {case}: expected Size {expected_size}, got {size}"
            );
        }
    }
}

/// RULE: how a membership ended does not change what the periods it ran for report — a
/// disassociation and a deleted billing group both end it at the period the call was
/// made in, so an earlier period still reports the group and the current one reports
/// `UNMONITORED`.
#[tokio::test]
async fn a_membership_ended_by_a_release_or_a_delete_still_covers_its_own_periods() {
    let em = Emulator::new();
    let plan = em.create_plan("ReleasedPlan").await;
    let now = current_period();

    // Rows differ along how the membership ended: nothing ended it, the account was
    // disassociated, and the whole billing group was deleted.
    let cases: &[&str] = &["standing", "disassociated", "group deleted"];
    for (index, how) in cases.iter().enumerate() {
        let account = format!("{:012}", 200 + index);
        let group = em
            .create_group(&format!("ReleasedGroup{index}"), &plan, &[])
            .await;
        em.associate(&group, &[account.as_str()]).await;
        // The membership began a year ago, which only the store can say.
        let start = shift_period(&now, -12);
        em.membership_ran(&account, &start, None);
        // The group has to exist in every period this row reads — see
        // [`Emulator::group_created_in`].
        em.group_created_in(&group, &shift_period(&start, -1));
        match *how {
            "disassociated" => em.disassociate(&group, &[account.as_str()]).await,
            "group deleted" => em.delete_group(&group).await,
            _ => {}
        }
        let still_standing = *how == "standing";

        // Every period the membership ran for reports the group, whether or not the
        // group still exists; the period the call ended it in reports no group.
        let reads: Vec<(String, bool)> = vec![
            (shift_period(&start, -1), false),
            (start.clone(), true),
            (shift_period(&now, -1), true),
            (now.clone(), still_standing),
            (shift_period(&now, 12), still_standing),
        ];
        for (period, in_group) in reads {
            let case = format!("{how} membership read at {period:?}");
            let expected = in_group.then(|| group.clone());
            let reported = em.association_in(&case, &account, Some(&period)).await;
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?}, got {reported:?}"
            );
        }

        // A read that names no period asks about the current one, so it agrees with the
        // explicit current period.
        let case = format!("{how} membership read with no period");
        let defaulted = em.association_in(&case, &account, None).await;
        let explicit = em.association_in(&case, &account, Some(&now)).await;
        assert_eq!(
            defaulted, explicit,
            "case {case}: an absent BillingPeriod means the current one, got {defaulted:?} \
             against {explicit:?}"
        );
    }
}

/// RULE: a membership made NOW is in force in the current period and in no earlier one,
/// a release in the same period leaves the account in no group for every period, and the
/// group an account ends up in after being moved is the one the covering periods report.
/// Reads change nothing.
#[tokio::test]
async fn a_membership_made_now_starts_in_the_current_period_and_a_move_is_reported_per_period() {
    let em = Emulator::new();
    let plan = em.create_plan("NowPlan").await;
    let first = em.create_group("NowFirst", &plan, &["111122223333"]).await;
    let second = em.create_group("NowSecond", &plan, &[]).await;
    let now = current_period();

    // Rows differ along the period asked about: none at all, the current period named
    // explicitly, a future period, the month before and the epoch month.
    let reads: Vec<(&str, Option<String>, bool)> = vec![
        ("no period at all", None, true),
        ("the current period", Some(now.clone()), true),
        ("a far-future period", Some("9999-12".to_string()), true),
        ("the period before", Some(shift_period(&now, -1)), false),
        ("the epoch month", Some("1970-01".to_string()), false),
    ];
    for (case, period, in_force) in &reads {
        let expected = in_force.then(|| first.clone());
        let reported = em
            .association_in(case, "111122223333", period.as_deref())
            .await;
        assert_eq!(
            reported, expected,
            "case {case:?}: a membership made now holds from this period on, expected \
             {expected:?}, got {reported:?}"
        );
    }

    // Released in the period it was made in, the membership covers no period at all:
    // from the start of that month the account was, in the end, in no group.
    em.disassociate(&first, &["111122223333"]).await;
    for (case, period, _) in &reads {
        let reported = em
            .association_in(case, "111122223333", period.as_deref())
            .await;
        assert_eq!(
            reported, None,
            "case {case:?}: a membership begun and ended in one period covers no period, got \
             {reported:?}"
        );
    }

    // Moved to another group in the same period, the account is reported in the group it
    // ended up in for the periods that membership covers.
    em.associate(&second, &["111122223333"]).await;
    for (case, period, in_force) in &reads {
        let expected = in_force.then(|| second.clone());
        let reported = em
            .association_in(case, "111122223333", period.as_deref())
            .await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // The reads changed nothing: the sizes still report the membership that stands.
    let first_size = em.group_size_in("after the reads", &first, None).await;
    assert_eq!(
        first_size, 0,
        "case \"after the reads\": the group the account left holds nobody, got {first_size}"
    );
    let second_size = em.group_size_in("after the reads", &second, None).await;
    assert_eq!(
        second_size, 1,
        "case \"after the reads\": the group it joined holds it, got {second_size}"
    );
}
