// SPDX-License-Identifier: CC0-1.0

//! The BILLING PERIOD dimension of a billing group itself, end to end over the service's
//! real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE: a read scoped to a billing period reports each resource AS IT STOOD in that
//! period — `ListBillingGroups` "retrieve[s] a list of billing groups for the given
//! billing period" — so a billing group is reported only for the periods from the one it
//! was created in onward, and the members a write can change (`Name`, `Description`,
//! `ComputationPreference`, `Status`, `AccountGrouping.AutoAssociate`,
//! `LastModifiedTime`) carry the values that held in the period asked about rather than
//! the current ones. A write takes effect in the period it is made in, so one period has
//! one generation of the group and a later write never shows up in an earlier period. A
//! read that names no period asks about the current one, and the request's `Filters`
//! select on that same per-period answer.
//!
//! The members no write can change (`Arn`, `PrimaryAccountId`,
//! `AccountGrouping.ResponsibilityTransferArn`, `BillingGroupType`, `CreationTime` —
//! none of them changeable through `UpdateBillingGroupInput`,
//! `smithy_query(query="shape",shape="UpdateBillingGroupInput")`) are the same in every
//! period, which each table asserts beside the ones that change.
//!
//! `tests/all/account_association_billing_periods.rs` owns the same rule for the
//! MEMBERSHIP a period reports (`BillingGroupListElement.Size`), and
//! `tests/all/pricing_rule_billing_periods.rs` owns it for a pricing rule; this suite is
//! about a billing group's own per-period identity and values.
//!
//! The wire cannot create or update a group in a PAST period — a write happens now — so a
//! row writes through the real wire and then backdates the stored instants, which is the
//! one thing the test cannot ask the service for.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The month arithmetic over a `BillingPeriod` this crate's period-scoped suites share
/// (`tests/all/pricing_rule_association_periods.rs`), so one ordering rule serves every
/// one of them.
use crate::pricing_rule_association_periods::{period_start_millis, shift_period};

/// `POST /list-billing-groups` — the period-scoped read under test.
const LIST_PATH: &str = "/list-billing-groups";

/// The writes that give a row something to read, at the methods their modelled HTTP
/// bindings name (`smithy_query(query="operation",operation="UpdateBillingGroup")`: a
/// `POST`).
const CREATE_GROUP_PATH: &str = "/create-billing-group";
const UPDATE_GROUP_PATH: &str = "/update-billing-group";
const DELETE_GROUP_PATH: &str = "/delete-billing-group";
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// The account that serves as the billing owner of the `index`-th group here: a group
/// needs one to hold an operational `Status` at all (the model's
/// `ResponsibilityTransferArn` is "When specified, the PrimaryAccountId is no longer
/// required").
///
/// One per group, because a group's primary account is one of its OWN linked accounts and
/// an account belongs to at most one billing group — so no two groups of one family can
/// name the same primary account.
pub(crate) fn primary_account_id(index: usize) -> String {
    format!("{:012}", 999900000000_u64 + index as u64)
}

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
pub(crate) fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// A live emulator holding only this service, with the engine kept so a row can backdate
/// a group the wire could only create now.
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this payer-account-only control
    /// plane is reachable at all (see `tests/all/payer_account_gate.rs` for the denial
    /// every other caller gets).
    pub(crate) fn new() -> Self {
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
    pub(crate) async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
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

    /// Create a pricing plan and answer the ARN the service minted — a billing group's
    /// computation preference must name a plan that exists.
    pub(crate) async fn create_plan(&self, name: &str) -> String {
        let (status, body) = self.post(CREATE_PLAN_PATH, &json!({"Name": name})).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name:?}: expected 200, got {status} with {body:?}"
        );
        body["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup CreatePricingPlan {name:?}: no Arn in {body:?}"))
            .to_string()
    }

    /// Create a billing group computing with `plan` whose primary account is `primary`,
    /// with the members `extra` names laid over the defaults, and answer the ARN the
    /// service minted.
    ///
    /// The primary account is also the group's one linked account, since the account that
    /// serves as a group's main account is an account of that group. The members of
    /// `extra`'s own `AccountGrouping` are laid over the default grouping one by one, so a
    /// row naming `AutoAssociate` keeps that membership.
    pub(crate) async fn create_group(
        &self,
        name: &str,
        plan: &str,
        primary: &str,
        extra: &Value,
    ) -> String {
        let mut body = json!({
            "Name": name,
            "PrimaryAccountId": primary,
            "AccountGrouping": {"LinkedAccountIds": [primary]},
            "ComputationPreference": {"PricingPlanArn": plan},
        });
        for (member, value) in extra.as_object().into_iter().flatten() {
            if member == "AccountGrouping" {
                for (inner, inner_value) in value.as_object().into_iter().flatten() {
                    body["AccountGrouping"][inner] = inner_value.clone();
                }
            } else {
                body[member] = value.clone();
            }
        }
        let (status, answer) = self.post(CREATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {name:?}: expected 200, got {status} with {answer:?}"
        );
        answer["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup CreateBillingGroup {name:?}: no Arn in {answer:?}"))
            .to_string()
    }

    /// `UpdateBillingGroup`: apply `changes` to the group, through the real wire.
    pub(crate) async fn update_group(&self, case: &str, group: &str, changes: &Value) {
        let mut body = changes.clone();
        body["Arn"] = json!(group);
        let (status, answer) = self.post(UPDATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: setup UpdateBillingGroup expected 200, got {status} with {answer:?}"
        );
    }

    /// Backdate `group`'s creation to `period`, as if the creating call had been made in
    /// that billing period: the way a world where a group already existed in a closed
    /// period is set up, which no request can ask for (a create happens now). The group's
    /// only generation starts there too, since nothing has superseded it yet.
    pub(crate) fn created_in(&self, group: &str, period: &str) {
        let at = period_start_millis(period);
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let record = state
            .billing_groups
            .get_mut(group)
            .expect("setup created_in: the group was created above");
        record.creation_time_millis = at;
        record.last_modified_time_millis = at;
    }

    /// Backdate the group's CURRENT generation to `period`, as if the write that left the
    /// group as it stands had been made in that billing period: how a world whose
    /// generations started in DIFFERENT periods is set up, which no request can ask for
    /// (a write happens now).
    pub(crate) fn generation_started_in(&self, group: &str, period: &str) {
        let at = period_start_millis(period);
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let record = state
            .billing_groups
            .get_mut(group)
            .expect("setup generation_started_in: the group was created above");
        record.last_modified_time_millis = at;
    }

    /// `DeleteBillingGroup`: remove the group through the real wire, which keeps the
    /// record of the periods it billed in.
    pub(crate) async fn delete_group(&self, case: &str, group: &str) {
        let (status, answer) = self
            .post(DELETE_GROUP_PATH, &json!({"Arn": group}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: setup DeleteBillingGroup expected 200, got {status} with {answer:?}"
        );
    }

    /// Backdate the DELETE of `group` to `period`, as if the deleting call had been made
    /// in that billing period: the wire can only delete now.
    pub(crate) fn deleted_in(&self, group: &str, period: &str) {
        let at = period_start_millis(period);
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let deleted = state
            .deleted_billing_groups
            .iter_mut()
            .find(|deleted| deleted.record.arn == group)
            .expect("setup deleted_in: the group was deleted above");
        deleted.deletion_time_millis = at;
    }

    /// How many SUPERSEDED generations of `group` the store holds: the slots this group
    /// has spent of the bounded version history a period-scoped read of a closed period
    /// is answered from. Counted from the store because no read reports the size of that
    /// history.
    pub(crate) fn superseded_generations(&self, group: &str) -> usize {
        let state = self.engine.state::<BillingConductorState>();
        let state = state.lock();
        state
            .billing_groups
            .get(group)
            .expect("superseded_generations: the group was created above")
            .superseded_versions
            .len()
    }

    /// The `BillingGroups` `ListBillingGroups` reports for `body`, in response order.
    async fn listed(&self, case: &str, body: &Value) -> Vec<Value> {
        let (status, answer) = self.post(LIST_PATH, body).await;
        assert_eq!(
            status, 200,
            "case {case}: ListBillingGroups expected 200, got {status} with {answer:?}"
        );
        answer["BillingGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The `BillingGroupListElement` `ListBillingGroups` reports for ONE group in
    /// `period` (`None` names no period at all), or `None` when the listing does not
    /// report the group for that period at all.
    pub(crate) async fn element_in(
        &self,
        case: &str,
        group: &str,
        period: Option<&str>,
    ) -> Option<Value> {
        let mut body = json!({"Filters": {"Arns": [group]}});
        if let Some(period) = period {
            body["BillingPeriod"] = json!(period);
        }
        self.listed(case, &body).await.first().cloned()
    }

    /// Does the listing report `group` for `period` under `filters`?
    async fn matches_filter(
        &self,
        case: &str,
        group: &str,
        period: &str,
        filters: &Value,
    ) -> bool {
        let body = json!({"BillingPeriod": period, "Filters": filters});
        self.listed(case, &body)
            .await
            .iter()
            .any(|element| element["Arn"].as_str() == Some(group))
    }
}

/// Assert that `element` reports every member `expected` names with the value it names.
pub(crate) fn assert_members(case: &str, element: &Value, expected: &Value) {
    let members = expected
        .as_object()
        .expect("an expectation is a map of member to value");
    for (member, value) in members {
        assert_eq!(
            element.get(member).unwrap_or(&Value::Null),
            value,
            "case {case}: expected {member} to be {value}, got {:?} in {element:?}",
            element.get(member)
        );
    }
}

/// `StatusReason` is "the reason why the billing group is in its current status", so it
/// is reported exactly when the status it explains is not the plain `ACTIVE` one — in
/// every period, for the status that period reports.
fn assert_status_reason_explains_status(case: &str, element: &Value) {
    let status = element["Status"].as_str().unwrap_or_default();
    let reason = element.get("StatusReason").and_then(Value::as_str);
    if status == "ACTIVE" {
        assert_eq!(
            reason, None,
            "case {case}: an ACTIVE group has no status reason, got {reason:?}"
        );
    } else {
        assert!(
            reason.is_some_and(|reason| !reason.is_empty()),
            "case {case}: the status {status:?} must come with the reason for it, got {reason:?}"
        );
    }
}

/// The periods a group created in `creation` must and must not be reported for: the
/// creation period itself and every later one, against the earlier ones. The rows cross a
/// year boundary in both directions.
fn read_periods(creation: &str) -> Vec<(String, bool)> {
    vec![
        (shift_period(creation, -13), false),
        (shift_period(creation, -1), false),
        (creation.to_string(), true),
        (shift_period(creation, 1), true),
        (shift_period(creation, 13), true),
    ]
}

/// The periods a group of the table is created in. Rows differ along the month form the
/// modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows (`0?[1-9]`, so
/// `2024-1` and `2024-01` are ONE period), the end-of-year and start-of-year months whose
/// neighbours cross a year, the epoch month, and a far-future one.
const CREATION_PERIODS: &[&str] = &["1970-01", "1999-12", "2024-1", "2024-07", "2031-3", "9980-12"];

#[tokio::test]
async fn a_group_is_reported_only_for_the_periods_from_the_one_it_was_created_in_onward() {
    let em = Emulator::new();
    let plan = em.create_plan("CreatedPlan").await;
    for (index, creation) in CREATION_PERIODS.iter().enumerate() {
        let case = format!("created in {creation:?}");
        let group = em
            .create_group(
                &format!("Created-{index}"),
                &plan,
                &primary_account_id(index),
                &json!({}),
            )
            .await;
        em.created_in(&group, creation);

        for (period, exists) in read_periods(creation) {
            let case = format!("created in {creation:?} read at {period:?}");
            let reported = em.element_in(&case, &group, Some(&period)).await;
            assert_eq!(
                reported.is_some(),
                exists,
                "case {case}: a group is reported exactly for the periods from its creation \
                 onward, expected {}, got {reported:?}",
                if exists { "reported" } else { "not reported" }
            );
        }

        // Both spellings of ONE month name one period, so they answer the same.
        let (year, month) = creation
            .split_once('-')
            .expect("a creation period is spelled YYYY-MM");
        let month: u32 = month
            .parse()
            .expect("a creation period has a numeric month");
        for spelling in [format!("{year}-{month}"), format!("{year}-{month:02}")] {
            let case = format!("created in {creation:?} read at {spelling:?}");
            let reported = em.element_in(&case, &group, Some(&spelling)).await;
            assert!(
                reported.is_some(),
                "case {case}: either spelling of the creation period reports the group, got \
                 {reported:?}"
            );
        }

        // A read that names no period asks about the CURRENT one, so a group created in a
        // period still to come is not reported then either.
        let case = format!("{case} read with no period");
        let now = current_period();
        let expected = crate::pricing_rule_association_periods::period_months(creation)
            <= crate::pricing_rule_association_periods::period_months(&now);
        let reported = em.element_in(&case, &group, None).await;
        assert_eq!(
            reported.is_some(),
            expected,
            "case {case}: a read naming no period asks about {now:?}, expected {}, got \
             {reported:?}",
            if expected { "reported" } else { "not reported" }
        );
    }
}

/// One row of the per-period value table: what the group was created as, what a later
/// write changed, and the members a read of each period must report.
struct ValueRow {
    label: &'static str,
    /// `CreateBillingGroup` members laid over this suite's defaults.
    create: Value,
    /// `UpdateBillingGroup` members beside `Arn`.
    update: Value,
    /// The members the earlier period must report — the group as it was created.
    before: Value,
    /// The members the current period must report — the group as the write left it.
    after: Value,
}

/// The table: one row per member of `BillingGroupListElement` a write can change
/// (`smithy_query(query="shape",shape="UpdateBillingGroupInput")` carries `Name`,
/// `Description`, `ComputationPreference`, `Status` and `AccountGrouping`), each changed
/// by a write that lands in a LATER billing period than the one the group was created in.
fn value_rows(first_plan: &str, second_plan: &str) -> Vec<ValueRow> {
    vec![
        ValueRow {
            label: "Name",
            create: json!({}),
            update: json!({"Name": "Renamed-By-The-Write"}),
            before: json!({"Name": "Valued-0"}),
            after: json!({"Name": "Renamed-By-The-Write"}),
        },
        ValueRow {
            label: "Description",
            create: json!({"Description": "as created"}),
            update: json!({"Description": "as updated"}),
            before: json!({"Description": "as created"}),
            after: json!({"Description": "as updated"}),
        },
        ValueRow {
            label: "ComputationPreference",
            create: json!({}),
            update: json!({"ComputationPreference": {"PricingPlanArn": second_plan}}),
            before: json!({"ComputationPreference": {"PricingPlanArn": first_plan}}),
            after: json!({"ComputationPreference": {"PricingPlanArn": second_plan}}),
        },
        ValueRow {
            label: "Status",
            create: json!({}),
            update: json!({"Status": "PENDING"}),
            before: json!({"Status": "ACTIVE"}),
            after: json!({"Status": "PENDING"}),
        },
        ValueRow {
            label: "AccountGrouping.AutoAssociate",
            create: json!({"AccountGrouping": {"AutoAssociate": false}}),
            update: json!({"AccountGrouping": {"AutoAssociate": true}}),
            before: json!({"AccountGrouping": {"AutoAssociate": false}}),
            after: json!({"AccountGrouping": {"AutoAssociate": true}}),
        },
    ]
}

#[tokio::test]
async fn a_write_shows_up_from_the_period_it_was_made_in_and_in_no_earlier_one() {
    let em = Emulator::new();
    let first_plan = em.create_plan("ValuedFirstPlan").await;
    let second_plan = em.create_plan("ValuedSecondPlan").await;
    let now = current_period();
    // The group is created two years back, so the update the wire makes NOW lands in a
    // later billing period than the creation and therefore opens a new generation.
    let creation = shift_period(&now, -24);

    for (index, row) in value_rows(&first_plan, &second_plan).iter().enumerate() {
        let case = format!("{} rewritten", row.label);
        let name = format!("Valued-{index}");
        let primary = primary_account_id(index);
        let group = em
            .create_group(&name, &first_plan, &primary, &row.create)
            .await;
        em.created_in(&group, &creation);
        em.update_group(&case, &group, &row.update).await;

        // The period the group was created in reports the group AS CREATED, and so does
        // every period up to the one the write landed in.
        for period in [creation.clone(), shift_period(&now, -1)] {
            let case = format!("{case} read at {period:?}");
            let element = em
                .element_in(&case, &group, Some(&period))
                .await
                .unwrap_or_else(|| panic!("case {case}: the group existed then, so it is reported"));
            // The `Name` row is the only one whose created name is the row's own; every
            // other row keeps the name the create gave, which is what `before` names.
            let mut before = row.before.clone();
            if row.label != "Name" {
                before["Name"] = json!(name);
            }
            assert_members(&case, &element, &before);
            assert_status_reason_explains_status(&case, &element);
            // Nothing a write cannot change moves with the period.
            assert_members(
                &case,
                &element,
                &json!({"Arn": group, "PrimaryAccountId": primary,
                        "BillingGroupType": "STANDARD"}),
            );
        }

        // The current period reports the group as the write left it, whether the read
        // names that period or none at all.
        for period in [None, Some(now.clone())] {
            let case = format!("{case} read at {period:?}");
            let element = em
                .element_in(&case, &group, period.as_deref())
                .await
                .unwrap_or_else(|| panic!("case {case}: the group exists now, so it is reported"));
            assert_members(&case, &element, &row.after);
            assert_status_reason_explains_status(&case, &element);
        }

        // The reads changed nothing: the earlier period still reports the group as it was
        // created.
        let case = format!("{case} re-read at {creation:?}");
        let element = em
            .element_in(&case, &group, Some(&creation))
            .await
            .unwrap_or_else(|| {
                panic!("case {case}: a read must not change what a later caller sees")
            });
        let mut before = row.before.clone();
        if row.label != "Name" {
            before["Name"] = json!(name);
        }
        assert_members(&case, &element, &before);
    }
}

#[tokio::test]
async fn each_period_reports_the_generation_that_was_in_force_in_it() {
    let em = Emulator::new();
    let first_plan = em.create_plan("GenerationsFirstPlan").await;
    let second_plan = em.create_plan("GenerationsSecondPlan").await;
    let now = current_period();
    // Three generations: the create, a write a year later, and a write made NOW. The
    // first two are backdated, because the wire can only write in the current period.
    let first = shift_period(&now, -24);
    let second = shift_period(&now, -12);

    let group = em
        .create_group(
            "Generations",
            &first_plan,
            &primary_account_id(0),
            &json!({"Description": "first"}),
        )
        .await;
    em.created_in(&group, &first);
    em.update_group(
        "three generations",
        &group,
        &json!({"Description": "second",
                "ComputationPreference": {"PricingPlanArn": second_plan}}),
    )
    .await;
    em.generation_started_in(&group, &second);
    em.update_group(
        "three generations",
        &group,
        &json!({"Description": "third", "Status": "PENDING"}),
    )
    .await;

    // Rows differ along where the period falls among the generations: before the group
    // existed, the first generation's own period and the month after it, the second
    // generation's period and the month after it, the current period and a year on.
    let rows: Vec<(String, Option<Value>)> = vec![
        (shift_period(&first, -1), None),
        (
            first.clone(),
            Some(json!({"Description": "first", "Status": "ACTIVE",
                        "ComputationPreference": {"PricingPlanArn": first_plan}})),
        ),
        (
            shift_period(&first, 1),
            Some(json!({"Description": "first", "Status": "ACTIVE",
                        "ComputationPreference": {"PricingPlanArn": first_plan}})),
        ),
        (
            second.clone(),
            Some(json!({"Description": "second", "Status": "ACTIVE",
                        "ComputationPreference": {"PricingPlanArn": second_plan}})),
        ),
        (
            shift_period(&second, 1),
            Some(json!({"Description": "second", "Status": "ACTIVE",
                        "ComputationPreference": {"PricingPlanArn": second_plan}})),
        ),
        (
            now.clone(),
            Some(json!({"Description": "third", "Status": "PENDING",
                        "ComputationPreference": {"PricingPlanArn": second_plan}})),
        ),
        (
            shift_period(&now, 13),
            Some(json!({"Description": "third", "Status": "PENDING",
                        "ComputationPreference": {"PricingPlanArn": second_plan}})),
        ),
    ];

    for (period, expected) in rows {
        let case = format!("generations read at {period:?}");
        let reported = em.element_in(&case, &group, Some(&period)).await;
        match (&expected, &reported) {
            (Some(expected), Some(element)) => {
                assert_members(&case, element, expected);
                assert_status_reason_explains_status(&case, element);
            }
            (None, None) => {}
            _ => panic!("case {case}: expected {expected:?}, got {reported:?}"),
        }
    }

    // `LastModifiedTime` orders with the generations: each generation reports the write
    // that left it, so the earlier period reports the earlier instant, while
    // `CreationTime` is the group's own and belongs to every generation of it.
    let mut last_modified: Vec<i64> = Vec::new();
    for period in [&first, &second, &now] {
        let case = format!("LastModifiedTime at {period:?}");
        let element = em
            .element_in(&case, &group, Some(period))
            .await
            .unwrap_or_else(|| panic!("case {case}: the group existed then, so it is reported"));
        last_modified.push(
            element["LastModifiedTime"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case}: no LastModifiedTime in {element:?}")),
        );
        assert_eq!(
            element["CreationTime"].as_i64(),
            Some(last_modified[0]),
            "case {case}: expected the group's creation instant in every period, got {element:?}"
        );
    }
    assert!(
        last_modified.windows(2).all(|pair| pair[0] < pair[1]),
        "expected each generation's LastModifiedTime to come after the one it replaced, got \
         {last_modified:?}"
    );
}

#[tokio::test]
async fn a_filter_selects_on_the_values_the_period_reports() {
    let em = Emulator::new();
    let first_plan = em.create_plan("FilteredFirstPlan").await;
    let second_plan = em.create_plan("FilteredSecondPlan").await;
    let now = current_period();
    let creation = shift_period(&now, -24);

    let group = em
        .create_group(
            "Filtered-Before",
            &first_plan,
            &primary_account_id(0),
            &json!({"AccountGrouping": {"AutoAssociate": false}}),
        )
        .await;
    em.created_in(&group, &creation);
    em.update_group(
        "filters",
        &group,
        &json!({"Name": "Filtered-After", "Status": "PENDING",
                "ComputationPreference": {"PricingPlanArn": second_plan},
                "AccountGrouping": {"AutoAssociate": true}}),
    )
    .await;

    // Rows differ along the filter member — one per `ListBillingGroupsFilter` predicate a
    // write can move (`PricingPlan`, `Statuses`, `AutoAssociate`, `Names`) — and along
    // whether the value is the one the earlier generation or the later one carried.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("the created plan", json!({"PricingPlan": first_plan}), true),
        (
            "the written plan",
            json!({"PricingPlan": second_plan}),
            false,
        ),
        ("the created status", json!({"Statuses": ["ACTIVE"]}), true),
        ("the written status", json!({"Statuses": ["PENDING"]}), false),
        ("the created flag", json!({"AutoAssociate": false}), true),
        ("the written flag", json!({"AutoAssociate": true}), false),
        (
            "the created name",
            json!({"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Filtered-Before"}]}),
            true,
        ),
        (
            "the written name",
            json!({"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Filtered-After"}]}),
            false,
        ),
    ];

    for (label, filters, matches_creation_period) in rows {
        // The creation period reports the group as created, so a filter naming a created
        // value selects it there and one naming a written value does not.
        let case = format!("{label} at {creation:?}");
        let selected = em
            .matches_filter(&case, &group, &creation, &filters)
            .await;
        assert_eq!(
            selected, matches_creation_period,
            "case {case}: a filter selects on the values the period reports, expected {}, got \
             {selected}",
            if matches_creation_period {
                "selected"
            } else {
                "not selected"
            }
        );

        // The current period reports the group as the write left it, so the same filters
        // select the other way round.
        let case = format!("{label} at {now:?}");
        let selected = em.matches_filter(&case, &group, &now, &filters).await;
        assert_eq!(
            selected, !matches_creation_period,
            "case {case}: a filter selects on the values the period reports, expected {}, got \
             {selected}",
            if matches_creation_period {
                "not selected"
            } else {
                "selected"
            }
        );

        // A period before the group existed holds no group at all, so no filter selects
        // it there.
        let before = shift_period(&creation, -1);
        let case = format!("{label} at {before:?}");
        let selected = em.matches_filter(&case, &group, &before, &filters).await;
        assert!(
            !selected,
            "case {case}: a period before the group was created reports no group, got {selected}"
        );
    }
}

#[tokio::test]
async fn a_page_walks_only_the_groups_the_period_holds() {
    let em = Emulator::new();
    let plan = em.create_plan("PagedPlan").await;
    let now = current_period();
    // Three groups, each created one year apart, so each period holds a different number
    // of them.
    let periods = [
        shift_period(&now, -36),
        shift_period(&now, -24),
        shift_period(&now, -12),
    ];
    let mut groups: Vec<String> = Vec::new();
    for (index, period) in periods.iter().enumerate() {
        let group = em
            .create_group(
                &format!("Paged-{index}"),
                &plan,
                &primary_account_id(index),
                &json!({}),
            )
            .await;
        em.created_in(&group, period);
        groups.push(group);
    }

    // Rows differ along how many groups the period holds (0, 1, 2, 3) and along the page
    // size the walk uses, including each bound of the modelled 1..100 `MaxResults` range.
    for (held, period) in [
        (0, shift_period(&periods[0], -1)),
        (1, periods[0].clone()),
        (2, periods[1].clone()),
        (3, periods[2].clone()),
        (3, now.clone()),
    ] {
        let expected: Vec<String> = groups.iter().take(held).cloned().collect();
        for max_results in [1_i64, 2, 3, 100] {
            let case = format!("read at {period:?} with MaxResults={max_results}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut body = json!({"BillingPeriod": period, "MaxResults": max_results});
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let (status, answer) = em.post(LIST_PATH, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case}: ListBillingGroups expected 200, got {status} with {answer:?}"
                );
                let page = answer["BillingGroups"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                    "case {case}: a page is at most MaxResults, got {page:?}"
                );
                collected.extend(
                    page.iter()
                        .map(|group| group["Arn"].as_str().unwrap_or_default().to_string()),
                );
                token = answer["NextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected, expected,
                "case {case}: a period reports exactly the groups that existed in it, expected \
                 {expected:?}, got {collected:?}"
            );
        }
    }
}
