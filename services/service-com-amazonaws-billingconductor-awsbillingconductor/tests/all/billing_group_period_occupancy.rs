// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteBillingGroup` leaves behind for the billing period the deleted group
//! BILLED in, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: a billing group bills a consolidated family for a billing period, so a period
//! it billed in keeps the record of it and of the name it carried THEN; a request that
//! claims a billing group name the CURRENT billing period already holds — whether the
//! group holding it is live or was deleted during that period — is refused HTTP 400
//! `ValidationException` with reason `BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD`
//! and writes nothing, and only from the period after the delete is that name free
//! again. One rule, so every operation that claims a name answers it the same way:
//! `CreateBillingGroup` and the rename `UpdateBillingGroup` performs.
//!
//! `tests/all/billing_group_core_loop.rs` owns what a delete REMOVES (the group and its
//! account associations as the next caller sees them); this suite owns what it keeps.
//!
//! The wire cannot create or delete a group in a PAST billing period — a write happens
//! now — so a row writes through the real wire and then backdates the stored instants,
//! which is the one thing the test cannot ask the service for.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The month arithmetic over a `BillingPeriod` this crate's period-scoped suites share,
/// so one ordering rule serves every one of them.
use crate::pricing_rule_association_periods::{period_start_millis, shift_period};

const CREATE_PLAN_PATH: &str = "/create-pricing-plan";
const CREATE_GROUP_PATH: &str = "/create-billing-group";
const UPDATE_GROUP_PATH: &str = "/update-billing-group";
const DELETE_GROUP_PATH: &str = "/delete-billing-group";
const LIST_PATH: &str = "/list-billing-groups";

/// The reason the model enumerates for a billing group name the current billing period
/// already holds (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const OCCUPIED_REASON: &str = "BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD";

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// The account that serves as the billing owner of the `index`-th group here — one per
/// group, because a group's primary account is one of its own linked accounts and an
/// account belongs to at most one billing group at a time.
fn primary_account_id(index: usize) -> String {
    format!("{:012}", 777700000000_u64 + index as u64)
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

/// A live emulator holding only this service, with the engine kept so a row can
/// backdate a create or a delete the wire could only make now.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this payer-account-only
    /// control plane is reachable at all (`tests/all/payer_account_gate.rs` owns the
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

    /// `CreateBillingGroup` with `name`, computing with `plan`, whose one linked
    /// account is also its primary account. Answers the status and the body, so a row
    /// can assert either a mint or a refusal.
    async fn create_group(&self, name: &str, plan: &str, primary: &str) -> (u16, Value) {
        self.post(
            CREATE_GROUP_PATH,
            &json!({
                "Name": name,
                "PrimaryAccountId": primary,
                "AccountGrouping": {"LinkedAccountIds": [primary]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
        )
        .await
    }

    /// As [`Self::create_group`], for a create the row needs to SUCCEED: answers the
    /// ARN the service minted.
    async fn created_group(&self, case: &str, name: &str, plan: &str, primary: &str) -> String {
        let (status, body) = self.create_group(name, plan, primary).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBillingGroup {name:?} expected 200, got {status} with \
             {body:?}"
        );
        body["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Arn in {body:?}"))
            .to_string()
    }

    /// `DeleteBillingGroup` through the real wire.
    async fn delete_group(&self, case: &str, group: &str) {
        let (status, body) = self.post(DELETE_GROUP_PATH, &json!({"Arn": group})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup DeleteBillingGroup expected 200, got {status} with {body:?}"
        );
    }

    /// Backdate `group`'s creation — and its only generation — to `period`, as if the
    /// creating call had been made in that billing period: how a world where the group
    /// already existed in an earlier period is set up, which no request can ask for.
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

    /// Backdate the DELETE of `group` to `period`, as if the deleting call had been
    /// made in that billing period — the other half of the same setup: the wire can
    /// only delete now.
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

    /// The `BillingGroups` `ListBillingGroups` reports for `period`.
    async fn listed(&self, case: &str, period: &str) -> Vec<Value> {
        let (status, body) = self
            .post(LIST_PATH, &json!({"BillingPeriod": period}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListBillingGroups expected 200, got {status} with {body:?}"
        );
        body["BillingGroups"].as_array().cloned().unwrap_or_default()
    }

    /// The `Name` the listing of `period` reports for the group `arn`, or `None` when
    /// it reports no such group then.
    async fn name_in(&self, case: &str, period: &str, arn: &str) -> Option<String> {
        self.listed(case, period)
            .await
            .iter()
            .find(|element| element["Arn"].as_str() == Some(arn))
            .and_then(|element| element["Name"].as_str().map(str::to_string))
    }

    /// Every `Name` the listing of `period` reports.
    async fn names_in(&self, case: &str, period: &str) -> Vec<String> {
        self.listed(case, period)
            .await
            .iter()
            .map(|element| element["Name"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// Assert that `body` is the modelled refusal a claim on an occupied billing group
/// name earns: HTTP 400 `ValidationException`, the reason the model enumerates for it,
/// the input member at fault in `Fields`, and a message that names the billing period
/// at issue but NOT the name — `BillingGroupName` is `@sensitive`
/// (`smithy_query(query="shape",shape="BillingGroupName")`), so its value belongs in no
/// message.
fn assert_name_is_occupied(case: &str, status: u16, body: &Value, name: &str, member: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(OCCUPIED_REASON),
        "case {case:?}: expected Reason {OCCUPIED_REASON}, got {body:?}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some(member),
        "case {case:?}: expected {member} named in Fields, got {body:?}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    let period = current_period();
    assert!(
        message.contains(&period),
        "case {case:?}: expected the message to name the current billing period {period:?}, got \
         {message:?}"
    );
    assert!(
        !message.contains(name),
        "case {case:?}: a @sensitive name belongs in no message, got {message:?}"
    );
}

/// One row of the table: a group created `created` months from the current billing
/// period and deleted `deleted` months from it, and whether its name is still occupied
/// NOW.
struct Row {
    case: &'static str,
    /// Names differ along the characters the modelled `BillingGroupName` pattern
    /// `^[a-zA-Z0-9_\+=\.\-@]+$` allows
    /// (`smithy_query(query="shape",shape="BillingGroupName")`), so the occupancy rule
    /// is pinned on more than one spelling of a name.
    name: &'static str,
    created: i64,
    deleted: i64,
    occupied: bool,
}

/// The rows differ along where the group's life falls relative to the CURRENT billing
/// period: deleted in it (whether created in it, the month before, or a year before) —
/// where the name is still that period's — against deleted before it (the previous
/// period, and more than a year back), where the name is free again.
const ROWS: &[Row] = &[
    Row {
        case: "created and deleted in the current billing period",
        name: "Occupancy_Now",
        created: 0,
        deleted: 0,
        occupied: true,
    },
    Row {
        case: "created in the previous billing period, deleted in the current one",
        name: "Occupancy@Prev",
        created: -1,
        deleted: 0,
        occupied: true,
    },
    Row {
        case: "created a year before, deleted in the current billing period",
        name: "Occupancy+Year",
        created: -12,
        deleted: 0,
        occupied: true,
    },
    Row {
        case: "created and deleted in the previous billing period",
        name: "Freed.Prev",
        created: -1,
        deleted: -1,
        occupied: false,
    },
    Row {
        case: "created a year before and deleted in the previous billing period",
        name: "Freed-Year",
        created: -13,
        deleted: -1,
        occupied: false,
    },
    Row {
        case: "created and deleted more than a year before",
        name: "Freed=Ancient",
        created: -13,
        deleted: -13,
        occupied: false,
    },
];

#[tokio::test]
async fn a_deleted_group_keeps_its_name_for_the_billing_periods_it_billed_in() {
    let em = Emulator::new();
    let plan = em.create_plan("OccupancyPlan").await;
    let now = current_period();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let primary = primary_account_id(index);
        let arn = em.created_group(case, row.name, &plan, &primary).await;
        if row.created != 0 {
            em.created_in(&arn, &shift_period(&now, row.created));
        }
        em.delete_group(case, &arn).await;
        if row.deleted != 0 {
            em.deleted_in(&arn, &shift_period(&now, row.deleted));
        }

        // The whole point of the row: the SAME name, claimed again in the current
        // billing period.
        let (status, body) = em.create_group(row.name, &plan, &primary).await;
        if row.occupied {
            assert_name_is_occupied(case, status, &body, row.name, "Name");
            // The refusal minted nothing: no group of the current period carries the
            // name the request asked for.
            let names = em.names_in(case, &now).await;
            assert!(
                !names.contains(&row.name.to_string()),
                "case {case:?}: a refused create mints nothing, got {names:?}"
            );
            // And the refusal is keyed on the NAME the period holds, not on the delete:
            // another name, with the very accounts the deleted group held, is served.
            let next = format!("{}Next", row.name);
            let minted = em.created_group(case, &next, &plan, &primary).await;
            assert_eq!(
                em.name_in(case, &now, &minted).await,
                Some(next.clone()),
                "case {case:?}: expected the group named {next:?} to be readable back"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: the name is free from the period after the delete on, expected \
                 HTTP 200, got {status} with {body:?}"
            );
            let minted = body["Arn"].as_str().unwrap_or_default().to_string();
            assert_ne!(
                minted, arn,
                "case {case:?}: expected a new billing group, got the deleted group's ARN"
            );
            assert_eq!(
                em.name_in(case, &now, &minted).await,
                Some(row.name.to_string()),
                "case {case:?}: expected the re-created group to be readable back under {:?}",
                row.name
            );
        }
    }
}

#[tokio::test]
async fn a_rename_onto_a_name_the_current_billing_period_holds_is_refused() {
    let em = Emulator::new();
    let plan = em.create_plan("RenameOccupancyPlan").await;
    let now = current_period();

    // Rows differ along WHEN the group that held the target name was deleted: in the
    // current billing period (its name is still that period's) against the previous one
    // (free again).
    let rows: [(&str, &str, i64, bool); 2] = [
        ("deleted in the current billing period", "Taken_Now", 0, true),
        (
            "deleted in the previous billing period",
            "Taken_Earlier",
            -1,
            false,
        ),
    ];

    for (index, (case, target, deleted, occupied)) in rows.into_iter().enumerate() {
        // The group whose name the rename will claim, created and then deleted.
        let held = em
            .created_group(case, target, &plan, &primary_account_id(2 * index))
            .await;
        if deleted != 0 {
            em.created_in(&held, &shift_period(&now, deleted));
        }
        em.delete_group(case, &held).await;
        if deleted != 0 {
            em.deleted_in(&held, &shift_period(&now, deleted));
        }

        // The live group that asks to be renamed onto it.
        let own_name = format!("Renamer-{index}");
        let renamer = em
            .created_group(case, &own_name, &plan, &primary_account_id(2 * index + 1))
            .await;
        let (status, body) = em
            .post(UPDATE_GROUP_PATH, &json!({"Arn": renamer, "Name": target}))
            .await;

        if occupied {
            assert_name_is_occupied(case, status, &body, target, "Name");
            // A refused rename changes nothing: the group still answers under its own
            // name.
            assert_eq!(
                em.name_in(case, &now, &renamer).await,
                Some(own_name.clone()),
                "case {case:?}: a refused rename leaves the group's name alone"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: the name is free again, expected HTTP 200, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                em.name_in(case, &now, &renamer).await,
                Some(target.to_string()),
                "case {case:?}: expected the renamed group to read back under {target:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_name_a_billing_period_holds_is_the_one_the_group_carried_then() {
    let em = Emulator::new();
    let plan = em.create_plan("HistoryPlan").await;
    let now = current_period();
    let previous = shift_period(&now, -1);
    let case = "renamed in the current period, then deleted";

    // The group is created in the PREVIOUS billing period and renamed NOW, so one
    // period holds each name: the previous one `HistoricFirst`, the current one
    // `HistoricSecond`. Then it is deleted.
    let group = em
        .created_group(case, "HistoricFirst", &plan, &primary_account_id(0))
        .await;
    em.created_in(&group, &previous);
    let (status, body) = em
        .post(
            UPDATE_GROUP_PATH,
            &json!({"Arn": group, "Name": "HistoricSecond"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup UpdateBillingGroup expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        em.name_in(case, &previous, &group).await,
        Some("HistoricFirst".to_string()),
        "case {case:?}: the previous period reports the name the group carried then"
    );
    em.delete_group(case, &group).await;

    // The name the group carried in the CURRENT period is that period's.
    let (status, body) = em
        .create_group("HistoricSecond", &plan, &primary_account_id(1))
        .await;
    assert_name_is_occupied(case, status, &body, "HistoricSecond", "Name");

    // The name it carried only in an EARLIER period is not: that period holds it, the
    // current one never did.
    let reclaimed = em
        .created_group(case, "HistoricFirst", &plan, &primary_account_id(1))
        .await;
    assert_eq!(
        em.name_in(case, &now, &reclaimed).await,
        Some("HistoricFirst".to_string()),
        "case {case:?}: a name only an earlier period held is free in the current one"
    );
}
