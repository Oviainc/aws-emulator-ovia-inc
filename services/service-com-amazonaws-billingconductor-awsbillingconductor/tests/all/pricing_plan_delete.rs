// SPDX-License-Identifier: CC0-1.0

//! `DeletePricingPlan` end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE under test: `DeletePricingPlan` removes from state the ONE pricing plan its
//! `Arn` identifies (the modelled `PricingPlanArn` pattern spells a plan either as the
//! full ARN or as the bare id, and both name the same record), answers with that plan's
//! full ARN, frees its name for a later create and leaves every other plan where it
//! was. It refuses instead, writing nothing, when
//!
//! * a stored billing group's `ComputationPreference` still names the plan — "the
//!   pricing plan must not be associated with any billing groups to delete
//!   successfully" — with the modelled 409 `ConflictException` carrying the
//!   `PRICING_PLAN_ATTACHED_TO_BILLING_GROUP_DELETE_CONFLICT` reason
//!   `ConflictExceptionReason` enumerates;
//! * no plan of this account answers to the `Arn`, with the modelled 400
//!   `ValidationException` (`MISSING_PRICINGPLAN`) — this operation models no
//!   `ResourceNotFoundException`;
//! * the `Arn` names one of the plans Amazon Web Services owns, with the modelled 400
//!   `ValidationException` (`MISMATCHED_PRICINGPLAN_ARN`): those belong to no account,
//!   so they are not this account's to delete and they stay usable afterwards.
//!
//! Every table's rows differ along what the rule generalises over: how the `Arn` is
//! spelled (full ARN / bare id), which optional members the deleted plan carries and
//! how many pricing rules it associates, how many billing groups reference it (0, 1, 2)
//! and in which spelling, why no plan answers to an ARN (never minted, another account,
//! another partition, already deleted), and which AWS-managed plan is addressed.
//!
//! Each row's result is read back out of state through a SECOND call rather than taken
//! from the response of the call under test: the read-only
//! `ListPricingRulesAssociatedToPricingPlan` (404 `ResourceNotFoundException` for a plan
//! that is not stored) is the existence probe, an `Arn`-only `UpdatePricingPlan` reports
//! what state holds, and `CreatePricingPlan` consults the name-uniqueness index.
//! `ListPricingPlans` — the one reader that would report all of it at once — is still a
//! scaffold (`src/ops/list_pricing_plans.rs` carries the `SCAFFOLDED, NOT IMPLEMENTED`
//! marker), so it is not used here.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /delete-pricing-plan` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="DeletePricingPlan")`).
const DELETE_PATH: &str = "/delete-pricing-plan";

/// `POST /create-pricing-plan`, which gives this suite something to delete.
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// `POST /create-billing-group`, which gives a plan a holder.
const CREATE_GROUP_PATH: &str = "/create-billing-group";

/// `POST /delete-billing-group`, which releases a plan's holder again.
const DELETE_GROUP_PATH: &str = "/delete-billing-group";

/// `POST /list-pricing-rules-associated-to-pricing-plan` — the read-only existence
/// probe: 200 for a stored plan, 404 `ResourceNotFoundException` for one that is not.
const LIST_RULES_PATH: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// `PUT /update-pricing-plan` — an `Arn`-only update names no member, so it reports the
/// plan's stored state (and 404s when there is none).
const UPDATE_PLAN_PATH: &str = "/update-pricing-plan";

/// The pricing plan ids Amazon Web Services owns rather than the caller, enumerated by
/// the modelled `PricingPlanArn` pattern beside the customer-minted `[a-zA-Z0-9]{10}`
/// form (`smithy_query(query="shape",shape="PricingPlanArn")`).
const AWS_MANAGED_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): its control plane is authorized for the payer account alone, and the store
    /// records no payer account by default — so this harness designates one, which is
    /// the world in which the plan lifecycle this suite is about is reachable at all.
    /// The denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one request over the wire and return the status plus the parsed body.
    async fn call(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
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

    /// `CreatePricingPlan`, asserting the setup succeeded: the created plan's full ARN.
    async fn create_plan(&self, case: &str, body: &Value) -> String {
        let (status, parsed) = self.call("POST", CREATE_PLAN_PATH, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreatePricingPlan {body}: expected 200, got {status} with \
             {parsed:?}"
        );
        parsed["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// `CreatePricingPlan` without asserting it succeeded — the probe that reads the
    /// name-uniqueness index.
    async fn try_create_plan(&self, body: &Value) -> (u16, Value) {
        self.call("POST", CREATE_PLAN_PATH, body).await
    }

    /// `CreateBillingGroup` naming `plan`, asserting the setup succeeded: the group's ARN.
    async fn create_group(&self, case: &str, name: &str, plan: &str) -> String {
        let body = json!({
            "Name": name,
            "AccountGrouping": { "LinkedAccountIds": [] },
            "ComputationPreference": { "PricingPlanArn": plan },
        });
        let (status, parsed) = self.call("POST", CREATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBillingGroup {body}: expected 200, got {status} with \
             {parsed:?}"
        );
        parsed["Arn"]
            .as_str()
            .expect("setup CreateBillingGroup: no Arn in the response")
            .to_string()
    }

    /// `DeleteBillingGroup`, asserting the setup succeeded.
    async fn delete_group(&self, case: &str, group: &str) {
        let (status, parsed) = self
            .call("POST", DELETE_GROUP_PATH, &json!({ "Arn": group }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup DeleteBillingGroup {group}: expected 200, got {status} with \
             {parsed:?}"
        );
    }

    /// The call under test.
    async fn delete_plan(&self, arn: &str) -> (u16, Value) {
        self.call("POST", DELETE_PATH, &json!({ "Arn": arn })).await
    }

    /// Whether state still holds the plan, decided by the read-only
    /// `ListPricingRulesAssociatedToPricingPlan`: 200 when the plan exists, 404 when it
    /// does not.
    async fn plan_exists(&self, case: &str, arn: &str) -> bool {
        let (status, parsed) = self
            .call("POST", LIST_RULES_PATH, &json!({ "PricingPlanArn": arn }))
            .await;
        match status {
            200 => true,
            404 => false,
            other => panic!(
                "case {case:?}: ListPricingRulesAssociatedToPricingPlan {arn}: expected 200 or \
                 404, got {other} with {parsed:?}"
            ),
        }
    }

    /// The rule ARNs the plan associates, read through the same read-only reader.
    async fn associated_rules(&self, case: &str, arn: &str) -> Vec<String> {
        let (status, parsed) = self
            .call("POST", LIST_RULES_PATH, &json!({ "PricingPlanArn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPricingRulesAssociatedToPricingPlan {arn}: expected 200, got \
             {status} with {parsed:?}"
        );
        parsed["PricingRuleArns"]
            .as_array()
            .map(|arns| {
                arns.iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An `Arn`-only `UpdatePricingPlan`: it names no member, so it replaces nothing and
    /// reports the plan's stored state — or 404s when no plan is stored.
    async fn read_plan(&self, arn: &str) -> (u16, Value) {
        self.call("PUT", UPDATE_PLAN_PATH, &json!({ "Arn": arn })).await
    }
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<none>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The emulator's account id — the account every ARN this service mints carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The full `PricingRuleArn` of a rule id held by this account.
fn rule_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:pricingrule/{id}", account())
}

/// The full ARN of an AWS-managed pricing plan: the `PricingPlanArn` pattern spells the
/// account segment of those as the literal `aws`.
fn managed_plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::aws:pricingplan/{id}")
}

/// The bare-id form of a full ARN — the other spelling the modelled pattern admits for
/// the same resource.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// How one row spells the plan ARN its request addresses: both spellings the modelled
/// `PricingPlanArn` pattern admits name the same record.
type ArnSpelling = fn(&str) -> String;

/// The full ARN, unchanged.
fn as_full_arn(arn: &str) -> String {
    arn.to_string()
}

/// A well-formed 10-character rule id (`[a-zA-Z0-9]{10}`) derived from `index`.
fn rule_id(index: usize) -> String {
    format!("RULE{index:06}")
}

/// One row of the delete table.
struct DeleteCase {
    label: &'static str,
    /// The `CreatePricingPlan` body, minus the `Name` this suite assigns.
    plan: Value,
    /// How the `DeletePricingPlan` request spells the plan's ARN.
    spelling: ArnSpelling,
}

/// RULE: `DeletePricingPlan` removes the plan its `Arn` identifies — in either spelling
/// the modelled pattern admits — answers with that plan's FULL ARN, frees its name and
/// leaves every other plan in state.
#[tokio::test]
async fn delete_removes_the_addressed_plan_and_frees_its_name() {
    let cases = [
        DeleteCase {
            label: "a plan with nothing but a name, by full ARN",
            plan: json!({}),
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "a plan with nothing but a name, by bare id",
            plan: json!({}),
            spelling: bare_id,
        },
        DeleteCase {
            label: "a plan with a description and tags, by bare id",
            plan: json!({ "Description": "doomed", "Tags": { "team": "billing" } }),
            spelling: bare_id,
        },
        DeleteCase {
            label: "a plan with an empty description, by full ARN",
            plan: json!({ "Description": "" }),
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "a plan associating one pricing rule, by full ARN",
            plan: json!({ "PricingRuleArns": [rule_arn(&rule_id(1))] }),
            spelling: as_full_arn,
        },
        DeleteCase {
            label: "a plan associating three pricing rules, by bare id",
            plan: json!({
                "PricingRuleArns": [
                    rule_arn(&rule_id(2)),
                    rule_arn(&rule_id(3)),
                    rule_id(4),
                ],
            }),
            spelling: bare_id,
        },
    ];

    let em = Emulator::new();
    // A plan no row addresses: the delete must leave it alone.
    let bystander = em
        .create_plan("bystander", &json!({ "Name": "BystanderPlan" }))
        .await;

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("DoomedPlan{index}");
        let mut body = case.plan.clone();
        body["Name"] = json!(name);
        let arn = em.create_plan(label, &body).await;
        assert!(
            em.plan_exists(label, &arn).await,
            "case {label:?}: the created plan {arn} is not in state"
        );

        let addressed = (case.spelling)(&arn);
        let (status, answered) = em.delete_plan(&addressed).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 deleting {addressed}, got {status} with \
             {answered:?}"
        );
        assert_eq!(
            answered["Arn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the deleted plan's full ARN, got {answered:?}"
        );

        // The plan is GONE from state, in either spelling of its identity.
        assert!(
            !em.plan_exists(label, &arn).await,
            "case {label:?}: the deleted plan {arn} is still in state"
        );
        assert!(
            !em.plan_exists(label, &bare_id(&arn)).await,
            "case {label:?}: the deleted plan is still in state under its bare id"
        );
        let (read_status, read_body) = em.read_plan(&arn).await;
        assert_eq!(
            read_status, 404,
            "case {label:?}: expected an Arn-only UpdatePricingPlan on the deleted plan to 404, \
             got {read_status} with {read_body:?}"
        );
        assert_eq!(
            error_code(&read_body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException reading the deleted plan, got \
             {read_body:?}"
        );

        // Its name is free again: the uniqueness index released it with the plan.
        let (recreate_status, recreated) = em.try_create_plan(&json!({ "Name": name })).await;
        assert_eq!(
            recreate_status, 200,
            "case {label:?}: expected the deleted plan's name {name:?} to be reusable, got \
             {recreate_status} with {recreated:?}"
        );
        let reborn = recreated["Arn"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            reborn, arn,
            "case {label:?}: expected a fresh ARN for the re-created plan, got the deleted \
             plan's {arn}"
        );
        assert!(
            em.associated_rules(label, &reborn).await.is_empty(),
            "case {label:?}: the re-created plan must not inherit the deleted plan's \
             associations"
        );

        // Only the addressed plan went.
        assert!(
            em.plan_exists(label, &bystander).await,
            "case {label:?}: deleting {arn} also removed the unrelated plan {bystander}"
        );
    }
}

/// RULE: a pricing plan a stored billing group's `ComputationPreference` names cannot be
/// deleted ("the pricing plan must not be associated with any billing groups to delete
/// successfully"): the call is refused 409 `ConflictException` with the
/// `PRICING_PLAN_ATTACHED_TO_BILLING_GROUP_DELETE_CONFLICT` reason and writes nothing,
/// and the same delete succeeds once no group names the plan.
#[tokio::test]
async fn a_plan_a_billing_group_computes_with_cannot_be_deleted() {
    // Rows differ along how many billing groups name the plan (0, 1, 2) and along the
    // spelling the `DeletePricingPlan` request addresses it in. A group always names a
    // plan by its FULL ARN: `ComputationPreference.PricingPlanArn` targets
    // `PricingPlanFullArn`, whose pattern has no optional prefix
    // (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
    let cases: Vec<(&str, usize, ArnSpelling)> = vec![
        ("no billing group names the plan", 0, as_full_arn),
        ("one group, deleting by full ARN", 1, as_full_arn),
        ("one group, deleting by bare id", 1, bare_id),
        ("two groups, deleting by full ARN", 2, as_full_arn),
        ("two groups, deleting by bare id", 2, bare_id),
    ];

    let em = Emulator::new();
    for (index, (case, groups, spelling)) in cases.iter().enumerate() {
        let plan = em
            .create_plan(case, &json!({ "Name": format!("HeldPlan{index}") }))
            .await;
        let addressed = spelling(&plan);
        let mut group_arns = Vec::new();
        for ordinal in 0..*groups {
            group_arns.push(
                em.create_group(case, &format!("HeldGroup{index}_{ordinal}"), &plan)
                    .await,
            );
        }

        if *groups == 0 {
            let (status, body) = em.delete_plan(&addressed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to succeed, got {status} with {body:?}"
            );
            assert!(
                !em.plan_exists(case, &plan).await,
                "case {case:?}: the deleted plan is still in state"
            );
            continue;
        }

        let (status, body) = em.delete_plan(&addressed).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRICING_PLAN_ATTACHED_TO_BILLING_GROUP_DELETE_CONFLICT"),
            "case {case:?}: expected the reason the model enumerates for this conflict, got \
             {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(plan.as_str()),
            "case {case:?}: expected the conflict to name the plan in use, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingplan"),
            "case {case:?}: expected the conflict to name the resource type, got {body:?}"
        );
        // A refusal writes nothing: the plan, its name and its associations are all
        // still there.
        assert!(
            em.plan_exists(case, &plan).await,
            "case {case:?}: a refused delete must leave the plan in state"
        );
        let (taken_status, taken) = em
            .try_create_plan(&json!({ "Name": format!("HeldPlan{index}") }))
            .await;
        assert_eq!(
            taken_status, 409,
            "case {case:?}: a refused delete must leave the plan's name taken, got \
             {taken_status} with {taken:?}"
        );

        // Once no group names it, the very same delete goes through.
        for group in &group_arns {
            em.delete_group(case, group).await;
        }
        let (status, body) = em.delete_plan(&addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to succeed once no group names the plan, got \
             {status} with {body:?}"
        );
        assert!(
            !em.plan_exists(case, &plan).await,
            "case {case:?}: the deleted plan is still in state"
        );
    }
}

/// RULE: a `DeletePricingPlan` whose `Arn` names no plan of this account — never
/// created, in another account or another partition, or already deleted — is refused
/// with the modelled 400 `ValidationException` and the `MISSING_PRICINGPLAN` reason the
/// model enumerates, naming `Arn` as the member at fault.
#[tokio::test]
async fn deleting_a_plan_that_is_not_stored_is_refused() {
    let em = Emulator::new();
    let doomed = em
        .create_plan("a plan to delete twice", &json!({ "Name": "DeletedTwice" }))
        .await;
    let (status, body) = em.delete_plan(&doomed).await;
    assert_eq!(
        status, 200,
        "setup DeletePricingPlan: expected 200, got {status} with {body:?}"
    );

    // The rows differ along WHY no plan is stored under the ARN: never minted (in both
    // spellings the pattern allows), minted in another account or another partition, or
    // deleted a moment ago (again in both spellings).
    let cases: Vec<(&str, String)> = vec![
        ("a bare id that was never created", "ZZZZZZZZZZ".to_string()),
        (
            "a full ARN that was never created",
            format!("arn:aws:billingconductor::{}:pricingplan/AAAAAAAAAA", account()),
        ),
        (
            "a plan in another account",
            "arn:aws:billingconductor::123456789012:pricingplan/BBBBBBBBBB".to_string(),
        ),
        (
            "a plan in the China partition",
            "arn:aws-cn:billingconductor::000000000000:pricingplan/CCCCCCCCCC".to_string(),
        ),
        ("the full ARN of a deleted plan", doomed.clone()),
        ("the bare id of a deleted plan", bare_id(&doomed)),
    ];
    for (case, arn) in &cases {
        let (status, body) = em.delete_plan(arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("MISSING_PRICINGPLAN"),
            "case {case:?}: expected the MISSING_PRICINGPLAN reason, got {body:?}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("Arn"),
            "case {case:?}: expected the refusal to name the member at fault, got {body:?}"
        );
    }
}

/// RULE: the pricing plans Amazon Web Services owns — the ids the modelled
/// `PricingPlanArn` pattern enumerates beside a customer-minted one — belong to no
/// account, so deleting one is refused with the modelled 400 `ValidationException`
/// (`MISMATCHED_PRICINGPLAN_ARN`) and the plan stays usable afterwards.
#[tokio::test]
async fn an_aws_managed_plan_cannot_be_deleted() {
    let em = Emulator::new();
    // Rows: every AWS-managed id the pattern names, in both spellings it admits. The
    // list's source is the pattern itself (see `AWS_MANAGED_PLAN_IDS`), so a plan this
    // issue never cited (`Passthrough`) is covered too.
    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for id in AWS_MANAGED_PLAN_IDS {
        cases.push((format!("{id} by bare id"), (*id).to_string(), id));
        cases.push((format!("{id} by full ARN"), managed_plan_arn(id), id));
    }

    for (index, (case, arn, managed_id)) in cases.iter().enumerate() {
        let (status, body) = em.delete_plan(arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("MISMATCHED_PRICINGPLAN_ARN"),
            "case {case:?}: expected the MISMATCHED_PRICINGPLAN_ARN reason, got {body:?}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("Arn"),
            "case {case:?}: expected the refusal to name the member at fault, got {body:?}"
        );

        // The refusal wrote nothing: the managed plan is still there to compute with.
        assert!(
            em.plan_exists(case, arn).await,
            "case {case:?}: the AWS-managed plan must still exist after a refused delete"
        );
        // A billing group names a plan by its FULL ARN (`PricingPlanFullArn`), whatever
        // spelling this row addressed the delete with.
        em.create_group(case, &format!("ManagedPlanGroup{index}"), &managed_plan_arn(managed_id))
            .await;
    }
}
