// SPDX-License-Identifier: CC0-1.0

//! What the period-scoped READS of a billing group answer for a group a delete has since
//! removed, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: a billing group bills a consolidated family FOR A BILLING PERIOD and a delete
//! takes effect in the period it is made in, so every read scoped to a billing period
//! reports the groups that were billing in THAT period — from the period a group was
//! created in through the last period before the delete, whether or not the group is still
//! stored — each as the generation of it that was in force then and with the membership
//! that was in force then, and reports no such group from the period of the delete onward.
//! One rule, so the three period-scoped reads of a group (`ListBillingGroups`,
//! `ListBillingGroupCostReports` and `GetBillingGroupCostReport`) and the period-scoped
//! read of its MEMBERSHIP (`ListAccountAssociations`) agree: the `BillingGroupArn` the
//! account listing reports for a period resolves, for that same period, to the group it
//! names.
//!
//! The rows differ along what the rule generalises over: how many periods the group's life
//! spanned (none at all — created and deleted in one period — one, two and thirteen,
//! crossing a year), where that life falls relative to the current billing period (ending
//! in it, in the previous one, a year back), the period the read asks about (before the
//! creation, the creation itself, inside, the last period before the delete, the period of
//! the delete, after it, the current one and a future one), and which of the four reads
//! asks.
//!
//! `tests/all/billing_group_billing_periods.rs` owns this rule for a LIVE group and
//! `tests/all/billing_group_period_occupancy.rs` what a delete keeps of the group's NAME;
//! this suite owns what the reads of the group itself answer once it is deleted.
//!
//! The wire can neither create nor delete a group in a PAST billing period — a write
//! happens now — so a row writes through the real wire and then backdates the stored
//! instants, which is the one thing the test cannot ask the service for.

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

const CREATE_PLAN_PATH: &str = "/create-pricing-plan";
const CREATE_GROUP_PATH: &str = "/create-billing-group";
const UPDATE_GROUP_PATH: &str = "/update-billing-group";
const DELETE_GROUP_PATH: &str = "/delete-billing-group";
const LIST_GROUPS_PATH: &str = "/list-billing-groups";
const LIST_REPORTS_PATH: &str = "/list-billing-group-cost-reports";
const GET_REPORT_PATH: &str = "/get-billing-group-cost-report";
const LIST_ASSOCIATIONS_PATH: &str = "/list-account-associations";

/// The billing period the service calls current, read from the same clock it answers from
/// so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// The account that is the billing owner, and the one member, of the `index`-th group
/// here.
///
/// One per group: a group's primary account is one of its own linked accounts and an
/// account belongs to at most one billing group at a time, so no two groups can name the
/// same one.
fn account_id(index: usize) -> String {
    format!("{:012}", 666600000000_u64 + index as u64)
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A live emulator holding only this service, with the engine kept so a row can backdate
/// the create, the write and the delete the wire could only make now.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this payer-account-only control
    /// plane is reachable at all (`tests/all/payer_account_gate.rs` owns the denial every
    /// other caller gets).
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

    /// Create a pricing plan and answer the ARN the service minted — a billing group's
    /// computation preference must name a plan that exists.
    async fn create_plan(&self, name: &str) -> String {
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

    /// Create a billing group computing with `plan` whose one linked account, `account`,
    /// is also its primary account, and answer the ARN the service minted.
    ///
    /// It has a billing owner, so it holds an operational `Status`, and it holds one
    /// account, so the `Size` a period reports for it is the membership of that period.
    async fn create_group(&self, case: &str, name: &str, plan: &str, account: &str) -> String {
        let (status, body) = self
            .post(
                CREATE_GROUP_PATH,
                &json!({
                    "Name": name,
                    "PrimaryAccountId": account,
                    "AccountGrouping": {"LinkedAccountIds": [account]},
                    "ComputationPreference": {"PricingPlanArn": plan},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: setup CreateBillingGroup {name:?} expected 200, got {status} with \
             {body:?}"
        );
        body["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no Arn in {body:?}"))
            .to_string()
    }

    /// `UpdateBillingGroup`: apply `changes` to the group, through the real wire.
    async fn update_group(&self, case: &str, group: &str, changes: &Value) {
        let mut body = changes.clone();
        body["Arn"] = json!(group);
        let (status, answer) = self.post(UPDATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: setup UpdateBillingGroup expected 200, got {status} with {answer:?}"
        );
    }

    /// `DeleteBillingGroup`: remove the group, which ends the memberships it held.
    async fn delete_group(&self, case: &str, group: &str) {
        let (status, body) = self.post(DELETE_GROUP_PATH, &json!({"Arn": group})).await;
        assert_eq!(
            status, 200,
            "case {case}: setup DeleteBillingGroup expected 200, got {status} with {body:?}"
        );
    }

    /// Backdate `group`'s creation — and its only generation — to `period`, as if the
    /// creating call had been made in that billing period: how a world where the group
    /// already existed in a closed period is set up, which no request can ask for.
    fn created_in(&self, group: &str, period: &str) {
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
    /// group as it stands had been made in that billing period — the other half of
    /// [`Self::created_in`] for a group whose generations started in different periods.
    fn generation_started_in(&self, group: &str, period: &str) {
        let at = period_start_millis(period);
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let record = state
            .billing_groups
            .get_mut(group)
            .expect("setup generation_started_in: the group was created above");
        record.last_modified_time_millis = at;
    }

    /// Backdate the DELETE of `group` to `period`, as if the deleting call had been made
    /// in that billing period: the wire can only delete now.
    fn deleted_in(&self, group: &str, period: &str) {
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

    /// Backdate one account's stored membership to the half-open period range
    /// `[start, end)`, as if the associating call had been made in `start` and the delete
    /// that ended it in `end` — the world no request can ask for, since a call happens
    /// now.
    fn membership_ran(&self, account: &str, start: &str, end: &str) {
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let memberships = state
            .account_associations
            .get_mut(account)
            .expect("setup membership_ran: the account was a member above");
        for membership in memberships.iter_mut() {
            membership.inclusive_start_billing_period = start.to_string();
            membership.exclusive_end_billing_period = Some(end.to_string());
        }
    }

    /// The `BillingGroups` `ListBillingGroups` reports for `period` under `filters`.
    async fn listed(&self, case: &str, period: &str, filters: &Value) -> Vec<Value> {
        let (status, body) = self
            .post(
                LIST_GROUPS_PATH,
                &json!({"BillingPeriod": period, "Filters": filters}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: ListBillingGroups expected 200, got {status} with {body:?}"
        );
        body["BillingGroups"].as_array().cloned().unwrap_or_default()
    }

    /// The `BillingGroupListElement` `ListBillingGroups` reports for ONE group in
    /// `period`, or `None` when that period reports no such group.
    async fn element_in(&self, case: &str, group: &str, period: &str) -> Option<Value> {
        self.listed(case, period, &json!({"Arns": [group]}))
            .await
            .into_iter()
            .find(|element| element["Arn"].as_str() == Some(group))
    }

    /// The `BillingGroupCostReportElement` `ListBillingGroupCostReports` reports for ONE
    /// group in `period`, or `None` when that period reports no such group.
    async fn cost_report_in(&self, case: &str, group: &str, period: &str) -> Option<Value> {
        let (status, body) = self
            .post(
                LIST_REPORTS_PATH,
                &json!({
                    "BillingPeriod": period,
                    "Filters": {"BillingGroupArns": [group]},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: ListBillingGroupCostReports expected 200, got {status} with {body:?}"
        );
        body["BillingGroupCostReports"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|element| element["Arn"].as_str() == Some(group))
    }

    /// `GetBillingGroupCostReport` for ONE group over the single billing period `period`,
    /// as the half-open range that covers exactly it.
    async fn report_in(&self, group: &str, period: &str) -> (u16, Value) {
        self.post(
            GET_REPORT_PATH,
            &json!({
                "Arn": group,
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period,
                    "ExclusiveEndBillingPeriod": shift_period(period, 1),
                },
            }),
        )
        .await
    }

    /// The `BillingGroupArn` `ListAccountAssociations` reports for one account in
    /// `period`: `Some(arn)` while it was in a group then, `None` when it was in none.
    async fn association_in(&self, case: &str, account: &str, period: &str) -> Option<String> {
        let (status, body) = self
            .post(
                LIST_ASSOCIATIONS_PATH,
                &json!({"BillingPeriod": period, "Filters": {"AccountId": account}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: ListAccountAssociations expected 200, got {status} with {body:?}"
        );
        let listed = body["LinkedAccounts"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {case}: expected exactly the account asked about, got {listed:?}"
        );
        listed[0]["BillingGroupArn"].as_str().map(str::to_string)
    }
}

/// One row of the table: a group whose life ran from `created` months from the current
/// billing period to the delete `deleted` months from it.
struct Row {
    case: &'static str,
    /// Names differ along the characters the modelled `BillingGroupName` pattern
    /// `^[a-zA-Z0-9_\+=\.\-@]+$` allows
    /// (`smithy_query(query="shape",shape="BillingGroupName")`).
    name: &'static str,
    created: i64,
    deleted: i64,
}

/// The rows differ along how many billing periods the group's life covered and where that
/// life falls relative to the CURRENT billing period: a life of no period at all (created
/// and deleted in one month, so no period ever reports it), of one period, of two, and of
/// thirteen crossing a year — ending in the current period, in the previous one, and a
/// year back.
const ROWS: &[Row] = &[
    Row {
        case: "created and deleted in the current billing period",
        name: "Deleted_Now",
        created: 0,
        deleted: 0,
    },
    Row {
        case: "created and deleted in the previous billing period",
        name: "Deleted@Prev",
        created: -1,
        deleted: -1,
    },
    Row {
        case: "one period of life, ending in the current billing period",
        name: "Deleted+One",
        created: -1,
        deleted: 0,
    },
    Row {
        case: "two periods of life, ending in the previous billing period",
        name: "Deleted.Two",
        created: -3,
        deleted: -1,
    },
    Row {
        case: "thirteen periods of life, ending in the current billing period",
        name: "Deleted-Year",
        created: -13,
        deleted: 0,
    },
    Row {
        case: "thirteen periods of life, ending a year back",
        name: "Deleted=Ancient",
        created: -25,
        deleted: -12,
    },
];

/// The periods one row reads, as offsets in months from the current billing period: a year
/// before the creation, the period before it, the creation itself, the one after it, the
/// last period before the delete, the period of the delete, the one after it, the current
/// one and a year ahead.
fn read_offsets(row: &Row) -> Vec<i64> {
    vec![
        row.created - 13,
        row.created - 1,
        row.created,
        row.created + 1,
        row.deleted - 1,
        row.deleted,
        row.deleted + 1,
        0,
        12,
    ]
}

#[tokio::test]
async fn a_deleted_group_is_reported_for_the_periods_it_billed_in_and_for_no_other() {
    let em = Emulator::new();
    let plan = em.create_plan("DeletedReadsPlan").await;
    let now = current_period();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let account = account_id(index);
        let created = shift_period(&now, row.created);
        let deleted = shift_period(&now, row.deleted);
        let group = em.create_group(case, row.name, &plan, &account).await;
        em.created_in(&group, &created);
        em.delete_group(case, &group).await;
        em.deleted_in(&group, &deleted);
        // The membership the group held ran exactly as long as the group did: a delete
        // ends it at the period the delete was made in.
        em.membership_ran(&account, &created, &deleted);

        for offset in read_offsets(row) {
            let period = shift_period(&now, offset);
            let case = format!("{case}, read at {period:?}");
            // The group was billing in the period exactly while it existed: from the one
            // it was created in up to, but not including, the one the delete was made in.
            let billing = row.created <= offset && offset < row.deleted;

            // Side 1 — the membership read: the account is reported in the group for the
            // periods the membership covered.
            let association = em.association_in(&case, &account, &period).await;
            let expected = billing.then(|| group.clone());
            assert_eq!(
                association, expected,
                "case {case}: expected the account's BillingGroupArn to be {expected:?}, got \
                 {association:?}"
            );

            // Side 2 — the group read, which has to agree with it: the same period
            // resolves that ARN to the group, with the membership it held then.
            let element = em.element_in(&case, &group, &period).await;
            assert_eq!(
                element.is_some(),
                billing,
                "case {case}: expected the listing to {} the group, got {element:?}",
                if billing { "report" } else { "report nothing for" }
            );
            if let Some(element) = &element {
                assert_eq!(
                    element["Name"].as_str(),
                    Some(row.name),
                    "case {case}: expected the Name the group carried then, got {element:?}"
                );
                assert_eq!(
                    element["Size"].as_i64(),
                    Some(1),
                    "case {case}: expected the one membership that covered the period, got \
                     {element:?}"
                );
                assert_eq!(
                    element["Status"].as_str(),
                    Some("ACTIVE"),
                    "case {case}: expected the status a group with a billing owner held, got \
                     {element:?}"
                );
                assert_eq!(
                    element["ComputationPreference"]["PricingPlanArn"].as_str(),
                    Some(plan.as_str()),
                    "case {case}: expected the plan the group computed with then, got {element:?}"
                );
                assert_eq!(
                    element["PrimaryAccountId"].as_str(),
                    Some(account.as_str()),
                    "case {case}: expected the group's billing owner, got {element:?}"
                );
            }

            // The cost report listing of the same period reports the same groups.
            let reported = em.cost_report_in(&case, &group, &period).await;
            assert_eq!(
                reported.is_some(),
                billing,
                "case {case}: expected ListBillingGroupCostReports to {} the group, got \
                 {reported:?}",
                if billing { "report" } else { "report nothing for" }
            );

            // And the report of that one group is served for a period it billed in and
            // names no group for any other.
            let (status, body) = em.report_in(&group, &period).await;
            if billing {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the report of a period the group billed in, got \
                     {status} with {body:?}"
                );
                let rows = body["BillingGroupCostReportResults"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    rows.len(),
                    1,
                    "case {case}: a report with no breakdown is one row, got {rows:?}"
                );
                assert_eq!(
                    rows[0]["Arn"].as_str(),
                    Some(group.as_str()),
                    "case {case}: expected the report to be of the group asked about, got {rows:?}"
                );
            } else {
                assert_eq!(
                    status, 404,
                    "case {case}: expected HTTP 404 for a period that held no such group, got \
                     {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case}: expected ResourceNotFoundException, got {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_generation_a_period_reports_for_a_deleted_group_is_the_one_in_force_then() {
    let em = Emulator::new();
    let first_plan = em.create_plan("GenerationFirstPlan").await;
    let second_plan = em.create_plan("GenerationSecondPlan").await;
    let now = current_period();
    let case = "created a year back, changed in between, deleted now";
    let account = account_id(100);

    // The group is created a year before the current period, its second generation takes
    // effect six periods later, and the delete is made now — so the periods it billed in
    // split between the two generations, six periods each.
    let created = shift_period(&now, -12);
    let changed = shift_period(&now, -6);
    let group = em
        .create_group(case, "GenerationFirst", &first_plan, &account)
        .await;
    em.created_in(&group, &created);
    em.update_group(
        case,
        &group,
        &json!({
            "Name": "GenerationSecond",
            "Status": "PENDING",
            "ComputationPreference": {"PricingPlanArn": second_plan},
        }),
    )
    .await;
    em.generation_started_in(&group, &changed);
    em.delete_group(case, &group).await;
    em.membership_ran(&account, &created, &now);

    // Rows differ along which generation's periods the read falls in: the first period of
    // the first generation, its last, the first of the second generation and its last.
    let reads: Vec<(i64, &str, &str, &str)> = vec![
        (-12, "GenerationFirst", "ACTIVE", &first_plan),
        (-7, "GenerationFirst", "ACTIVE", &first_plan),
        (-6, "GenerationSecond", "PENDING", &second_plan),
        (-1, "GenerationSecond", "PENDING", &second_plan),
    ];
    for (offset, name, status, plan) in reads {
        let period = shift_period(&now, offset);
        let case = format!("{case}, read at {period:?}");
        let element = em
            .element_in(&case, &group, &period)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected the deleted group to be reported"));
        assert_eq!(
            element["Name"].as_str(),
            Some(name),
            "case {case}: expected the Name the generation in force then carried, got {element:?}"
        );
        assert_eq!(
            element["Status"].as_str(),
            Some(status),
            "case {case}: expected the Status of the generation in force then, got {element:?}"
        );
        assert_eq!(
            element["ComputationPreference"]["PricingPlanArn"].as_str(),
            Some(plan),
            "case {case}: expected the plan the generation in force then computed with, got \
             {element:?}"
        );

        // The request's filters select on that same per-period answer, for a group the
        // store no longer holds just as for one it does.
        let by_name = em
            .listed(
                &case,
                &period,
                &json!({"Names": [{"SearchValue": name, "SearchOption": "STARTS_WITH"}]}),
            )
            .await;
        assert!(
            by_name
                .iter()
                .any(|element| element["Arn"].as_str() == Some(group.as_str())),
            "case {case}: the name filter selects on the name that period reports, got {by_name:?}"
        );
        let by_plan = em.listed(&case, &period, &json!({"PricingPlan": plan})).await;
        assert!(
            by_plan
                .iter()
                .any(|element| element["Arn"].as_str() == Some(group.as_str())),
            "case {case}: the pricing plan filter selects on the plan that period reports, got \
             {by_plan:?}"
        );
    }
}

#[tokio::test]
async fn a_period_lists_the_groups_it_held_live_and_deleted_in_one_sequence() {
    let em = Emulator::new();
    let plan = em.create_plan("SequencePlan").await;
    let now = current_period();
    let created = shift_period(&now, -6);
    let case = "three groups created together, the second and third deleted since";

    // Three groups created in the same earlier period, of which the SECOND is deleted in a
    // period after that one (so the read period still holds it) and the THIRD in the very
    // period the read asks about (so that period holds it no more).
    let mut groups: Vec<String> = Vec::new();
    for index in 0..3usize {
        let account = account_id(200 + index);
        let group = em
            .create_group(case, &format!("Sequenced{index}"), &plan, &account)
            .await;
        em.created_in(&group, &created);
        groups.push(group);
    }
    let read_period = shift_period(&now, -3);
    em.delete_group(case, &groups[1]).await;
    em.deleted_in(&groups[1], &shift_period(&now, -2));
    em.delete_group(case, &groups[2]).await;
    em.deleted_in(&groups[2], &read_period);

    let listed: Vec<String> = em
        .listed(case, &read_period, &json!({"Arns": groups.clone()}))
        .await
        .iter()
        .map(|element| element["Arn"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected = vec![groups[0].clone(), groups[1].clone()];
    assert_eq!(
        listed, expected,
        "case {case}: expected the live group and the one still billing in that period, in \
         creation order, got {listed:?}"
    );

    // One page per group, cut through the same sequence: the `NextToken` resumes it where
    // the page stopped, whether or not the group it stopped at is still stored.
    let mut paged: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "BillingPeriod": read_period,
            "MaxResults": 1,
            "Filters": {"Arns": groups.clone()},
        });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let (status, answer) = em.post(LIST_GROUPS_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: a page of one expected 200, got {status} with {answer:?}"
        );
        let page = answer["BillingGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page.len() <= 1,
            "case {case}: expected at most the one group asked for, got {page:?}"
        );
        paged.extend(
            page.iter()
                .map(|element| element["Arn"].as_str().unwrap_or_default().to_string()),
        );
        token = answer["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        paged, expected,
        "case {case}: expected the same sequence one group at a time, got {paged:?}"
    );
}
