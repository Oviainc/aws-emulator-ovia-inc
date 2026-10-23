// SPDX-License-Identifier: CC0-1.0

//! The payer-account authorization gate, and the tagging surface that is not on it.
//!
//! RULE under test: Billing Conductor is a PAYER-ACCOUNT-ONLY API. Every operation of
//! its control plane is authorized for the payer (management) account of the
//! consolidated billing family alone; a MODEL-CONFORMING call from any other caller is
//! refused HTTP 403 `AccessDeniedException` (the shape the model lists as a common error
//! of EVERY operation of this service, `errorKind` client, `@httpError 403`, `Message`
//! `@required`) whose `Message` is built FROM THE REQUEST — `Account <caller account id>
//! is not authorized to call <OperationName>. Only payer account is authorized`. The
//! decision is reached from the CALLER alone, so it precedes everything the request
//! says: the operation's own semantic validation (never its modelled 400), its resource
//! lookups (never a 404), and its dispatch — an operation nobody has implemented yet is
//! denied rather than answered with the scaffold's 501. ONE refusal still comes before
//! it: a member that breaks a constraint the MODEL declares is refused 400
//! `ValidationException` even for this caller, which is the order the recorded corpus
//! has (the same operations are recorded both ways). The two ungated operations are the
//! TAG operations, which resolve their `ResourceArn` instead and answer HTTP 404
//! `NotFoundException` "Resource was not found" for a resource this account does not
//! hold — including for a value the member's modelled `@pattern` refuses, which AWS does
//! not enforce as a request-rejecting constraint.
//!
//! The tables span what the rule generalises over rather than one recorded call: the
//! operation (EVERY one the model declares — the row set is checked against the model),
//! the HTTP method its route is bound to, whether the operation is implemented or still
//! scaffolded, which later stage the denied request would otherwise have reached
//! (semantic validation, a referenced-resource lookup, a resource existence check, the
//! scaffold), the constraint class a pre-gate refusal breaks, how many callers race,
//! which account the store records as the payer, and — on the tagging surface — the
//! resource type an ARN names and the form it takes (full ARN, bare id, another
//! account's). What the tag operations do with a resource that EXISTS is
//! `tests/all/tagging_surface.rs`'s subject; the rows here are about who may call them.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The account id every generated emulator presents, and so the account every request to
/// this emulator is made by — read from the one source of truth rather than re-spelled.
pub(crate) fn account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The operations AWS does NOT gate on the payer account: the tag operations, which the
/// recorded `TagResource` call from a non-payer account shows being answered by resource
/// resolution (404) rather than by the gate. Read from the service itself, so this suite
/// cannot drift from the gate it checks, and
/// [`every_payer_gated_operation_denies_a_caller_that_is_not_the_payer_account`] asserts
/// every OTHER operation the model declares is gated.
const UNGATED_OPERATIONS: &[&str] =
    service_com_amazonaws_billingconductor_awsbillingconductor::service::PAYER_UNGATED_OPERATIONS;

/// One of the pricing plans Amazon Web Services owns, in the full-ARN form
/// `ComputationPreference.PricingPlanArn`'s `PricingPlanFullArn` pattern requires
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`) — a plan a billing group
/// may name without anything having created it.
const MANAGED_PLAN_ARN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// This service's Smithy model, read at compile time so a model-derived test cannot
/// drift from the model this crate is generated from.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-billingconductor-awsbillingconductor.json"
));

/// A live emulator holding only this service.
///
/// `pub(crate)`, with the helpers below, because the sibling suite
/// `tests/all/payer_gate_precedes_idempotency_existence_and_assembly.rs` drives the same
/// gate from the same harness rather than standing up a second one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The engine the router was built over, so a test can change WHICH account the
    /// store records as the payer — the one input the gate's decision is made from.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// A router over this service's DEFAULT store: the emulated account as it stands up,
    /// a standalone account that is the payer account of no organization — the caller
    /// every recorded Billing Conductor response was taken from.
    pub(crate) fn standalone() -> Self {
        Self::with_payer(None)
    }

    /// The same router with `payer` designated the organization's payer account, so the
    /// admitted arm of the gate is reachable.
    pub(crate) fn with_payer(payer: Option<&str>) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = payer.map(str::to_string);
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// A router whose store records the EMULATED account as the payer account.
    pub(crate) fn payer() -> Self {
        Self::with_payer(Some(account_id()))
    }

    /// Withdraw the organization's payer account from the store, so the same caller is
    /// a non-payer from the next request on.
    fn revoke_payer(&self) {
        self.designate_payer(None);
    }

    /// Record `payer` as the organization's payer account in the store this router reads,
    /// so one router can serve both arms of the gate in turn.
    pub(crate) fn designate_payer(&self, payer: Option<&str>) {
        self.engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = payer.map(str::to_string);
    }

    /// Send one request over the wire and return the status, the parsed body and the
    /// `x-amzn-errortype` header restJson1 resolves an error shape from.
    pub(crate) async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value, Option<String>) {
        self.call_with_headers(method, path, body, &[]).await
    }

    /// The same call with extra request headers — the seam an operation whose member is
    /// modelled `@httpHeader` (`CreatePricingRule`'s `@idempotencyToken ClientToken`,
    /// bound to `X-Amzn-Client-Token`) is driven through.
    pub(crate) async fn call_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> (u16, Value, Option<String>) {
        let payload = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the body")),
            None => Bytes::new(),
        };
        let mut builder = http::Request::builder()
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
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(payload).expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed, error_type)
    }

    /// POST a JSON body, the shape most of this service's operations take.
    pub(crate) async fn post(&self, path: &str, body: &Value) -> (u16, Value, Option<String>) {
        self.call("POST", path, Some(body)).await
    }
}

/// The wire error code of a failure response (`__type`), without any namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` the gate owes for `operation`, built the way the service builds it: from
/// the calling account id and the operation name.
fn denial_message(operation: &str) -> String {
    format!(
        "Account {} is not authorized to call {operation}. Only payer account is authorized",
        account_id()
    )
}

/// A `{ResourceArn}` label as an SDK puts it on the wire: the sibling suite's encoder
/// (`tests/all/tagging_surface.rs`) is the one copy of that rule, because the modelled
/// `@httpLabel` binds ONE path segment and an ARN carries `/` and `:`.
fn label(value: &str) -> String {
    crate::tagging_surface::encode_label(value)
}

/// The `/tags/{ResourceArn}` route of one resource.
fn tags_path(resource_arn: &str) -> String {
    format!("/tags/{}", label(resource_arn))
}

/// The `UntagResource` route of one resource: its `TagKeys` are modelled
/// `@httpQuery("tagKeys")`, so they travel in the query string, not in a body.
fn untag_path(resource_arn: &str, tag_keys: &[&str]) -> String {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect();
    format!("{}?{}", tags_path(resource_arn), query.join("&"))
}

/// Assert that `answered` IS the payer-account denial of `operation`: the status, the
/// wire error code in both places restJson1 carries it, and the request-derived message.
pub(crate) fn assert_denied(case: &str, operation: &str, answered: &(u16, Value, Option<String>)) {
    let (status, body, error_type) = answered;
    assert_eq!(
        *status, 403,
        "case {case:?}: expected HTTP 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "AccessDeniedException",
        "case {case:?}: expected the modelled AccessDeniedException, got {body}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some("AccessDeniedException"),
        "case {case:?}: expected x-amzn-errortype: AccessDeniedException, got {error_type:?}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(denial_message(operation).as_str()),
        "case {case:?}: expected the request-derived Message {:?}, got {:?}",
        denial_message(operation),
        body["Message"]
    );
}

/// Every operation the model declares, as `(name, method, uri)`.
fn modelled_operations() -> BTreeSet<(String, String, String)> {
    let model: Value = serde_json::from_str(MODEL).expect("parse the Smithy model");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model's shapes")
        .clone();
    let mut operations = BTreeSet::new();
    for (id, shape) in &shapes {
        if shape.get("type").and_then(Value::as_str) != Some("operation") {
            continue;
        }
        let http = shape
            .get("traits")
            .and_then(|traits| traits.get("smithy.api#http"))
            .unwrap_or_else(|| panic!("operation {id} declares no @http binding"));
        let method = http["method"].as_str().expect("an @http method").to_string();
        let uri = http["uri"].as_str().expect("an @http uri").to_string();
        let name = id.rsplit('#').next().unwrap_or(id).to_string();
        operations.insert((name, method, uri));
    }
    operations
}

/// One MODEL-CONFORMING request per payer-gated operation: nothing in it breaks a
/// constraint the model declares, so the request gets past the boundary and the gate is
/// what answers it.
///
/// The ARNs and ids are pattern-conforming values that name nothing — the gate is
/// decided from the caller, so what a request points at cannot matter.
fn conforming_requests() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    // A pattern-conforming billing group / custom line item / pricing plan / pricing rule
    // id, and the transfer-relationship ARN the billing-transfer operations take
    // (`smithy_query(query="shape",shape="ResponsibilityTransferArn")`).
    let group = "ABCDEFGHIJKL";
    let item = "ABCDEFGHIJ";
    let plan = "ABCDEFGHIJ";
    let rule = "ABCDEFGHIJ";
    let transfer =
        "arn:aws:organizations::000000000000:transfer/o-abcdefghij/billing/outbound/rt-abcdefgh";
    vec![
        (
            "AssociateAccounts",
            "POST",
            "/associate-accounts",
            json!({ "Arn": group, "AccountIds": ["111122223333"] }),
        ),
        (
            "AssociatePricingRules",
            "PUT",
            "/associate-pricing-rules",
            json!({ "Arn": plan, "PricingRuleArns": [rule] }),
        ),
        (
            "BatchAssociateResourcesToCustomLineItem",
            "PUT",
            "/batch-associate-resources-to-custom-line-item",
            json!({ "TargetArn": item, "ResourceArns": [group] }),
        ),
        (
            "BatchDisassociateResourcesFromCustomLineItem",
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            json!({ "TargetArn": item, "ResourceArns": [group] }),
        ),
        (
            "CreateBillingGroup",
            "POST",
            "/create-billing-group",
            json!({
                "Name": "GateProbeGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": { "PricingPlanArn": MANAGED_PLAN_ARN },
            }),
        ),
        (
            "CreateCustomLineItem",
            "POST",
            "/create-custom-line-item",
            json!({
                "Name": "GateProbeItem",
                "Description": "a probe",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
            }),
        ),
        (
            "CreatePricingPlan",
            "POST",
            "/create-pricing-plan",
            json!({ "Name": "GateProbePlan" }),
        ),
        (
            "CreatePricingRule",
            "POST",
            "/create-pricing-rule",
            json!({
                "Name": "GateProbeRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 1.5,
            }),
        ),
        (
            "DeleteBillingGroup",
            "POST",
            "/delete-billing-group",
            json!({ "Arn": group }),
        ),
        (
            "DeleteCustomLineItem",
            "POST",
            "/delete-custom-line-item",
            json!({ "Arn": item }),
        ),
        (
            "DeletePricingPlan",
            "POST",
            "/delete-pricing-plan",
            json!({ "Arn": plan }),
        ),
        (
            "DeletePricingRule",
            "POST",
            "/delete-pricing-rule",
            json!({ "Arn": rule }),
        ),
        (
            "DisassociateAccounts",
            "POST",
            "/disassociate-accounts",
            json!({ "Arn": group, "AccountIds": ["111122223333"] }),
        ),
        (
            "DisassociatePricingRules",
            "PUT",
            "/disassociate-pricing-rules",
            json!({ "Arn": plan, "PricingRuleArns": [rule] }),
        ),
        (
            "GetBillingGroupCostReport",
            "POST",
            "/get-billing-group-cost-report",
            json!({ "Arn": group }),
        ),
        (
            "GetBillingTransferPreference",
            "POST",
            "/get-billing-transfer-preference",
            json!({ "ResponsibilityTransferArn": transfer }),
        ),
        (
            "ListAccountAssociations",
            "POST",
            "/list-account-associations",
            json!({}),
        ),
        (
            "ListBillingGroupCostReports",
            "POST",
            "/list-billing-group-cost-reports",
            json!({}),
        ),
        (
            "ListBillingGroups",
            "POST",
            "/list-billing-groups",
            json!({ "MaxResults": 5 }),
        ),
        (
            "ListCustomLineItemVersions",
            "POST",
            "/list-custom-line-item-versions",
            json!({ "Arn": item }),
        ),
        (
            "ListCustomLineItems",
            "POST",
            "/list-custom-line-items",
            json!({ "MaxResults": 5 }),
        ),
        (
            "ListPricingPlans",
            "POST",
            "/list-pricing-plans",
            json!({}),
        ),
        (
            "ListPricingPlansAssociatedWithPricingRule",
            "POST",
            "/list-pricing-plans-associated-with-pricing-rule",
            json!({ "PricingRuleArn": rule }),
        ),
        (
            "ListPricingRules",
            "POST",
            "/list-pricing-rules",
            json!({}),
        ),
        (
            "ListPricingRulesAssociatedToPricingPlan",
            "POST",
            "/list-pricing-rules-associated-to-pricing-plan",
            json!({ "PricingPlanArn": plan }),
        ),
        (
            "ListResourcesAssociatedToCustomLineItem",
            "POST",
            "/list-resources-associated-to-custom-line-item",
            json!({ "Arn": item }),
        ),
        (
            "UpdateBillingGroup",
            "POST",
            "/update-billing-group",
            json!({ "Arn": group, "Description": "gated" }),
        ),
        (
            "UpdateBillingTransferPreference",
            "PUT",
            "/update-billing-transfer-preference",
            json!({
                "ResponsibilityTransferArn": transfer,
                "AutoBillingTransferBillingGroupCreation": { "Enabled": false },
            }),
        ),
        (
            "UpdateCustomLineItem",
            "POST",
            "/update-custom-line-item",
            json!({ "Arn": item, "Description": "gated" }),
        ),
        (
            "UpdatePricingPlan",
            "PUT",
            "/update-pricing-plan",
            json!({ "Arn": plan, "Description": "gated" }),
        ),
        (
            "UpdatePricingRule",
            "PUT",
            "/update-pricing-rule",
            json!({ "Arn": rule, "Description": "gated" }),
        ),
    ]
}

/// THE RULE, over EVERY operation of the control plane: a caller that is not the payer
/// account is denied 403 `AccessDeniedException` with the request-derived message.
///
/// The rows are hand-written because each one has to be MODEL-CONFORMING (a request that
/// breaks a declared constraint is refused 400 before the gate — see
/// [`a_modelled_constraint_refusal_precedes_the_gate`]), but the SET of rows is checked
/// against the model: the gate is wired one operation at a time, so an operation left
/// out of it would silently serve a caller AWS refuses, and this test fails if any
/// modelled operation but the ungated tag operations is missing a row.
#[tokio::test]
async fn every_payer_gated_operation_denies_a_caller_that_is_not_the_payer_account() {
    let modelled = modelled_operations();
    assert!(
        modelled.len() >= 34,
        "expected the model to declare the whole surface, got {}",
        modelled.len()
    );
    let rows = conforming_requests();

    // Every modelled operation the gate covers has exactly one row, at the route and
    // with the method the model binds it to.
    let covered: BTreeSet<(String, String, String)> = rows
        .iter()
        .map(|(name, method, uri, _)| {
            ((*name).to_string(), (*method).to_string(), (*uri).to_string())
        })
        .collect();
    let expected: BTreeSet<(String, String, String)> = modelled
        .iter()
        .filter(|(name, _, _)| !UNGATED_OPERATIONS.contains(&name.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        covered, expected,
        "expected one conforming request per payer-gated operation, at the route the model \
         binds it to"
    );
    assert_eq!(
        rows.len(),
        covered.len(),
        "expected no operation to carry two rows"
    );

    let em = Emulator::standalone();
    for (operation, method, uri, body) in &rows {
        let answered = em.call(method, uri, Some(body)).await;
        assert_denied(&format!("{operation} ({method} {uri})"), operation, &answered);
    }
}

/// THE ORDER at the boundary: a request that breaks a constraint the MODEL declares is
/// refused 400 `ValidationException` even for a caller the payer gate would deny, which
/// is how the recorded corpus has it — the very operations recorded answering 403 to a
/// conforming request from these callers (`UpdateBillingGroup`, `DeleteBillingGroup`,
/// `AssociateAccounts`, `DisassociateAccounts`, `GetBillingGroupCostReport` and the batch
/// association operations) are recorded answering 400 with the constraint `Message` when
/// a member is non-conforming. The front door's constraint check runs ahead of the
/// service code that reads who is calling.
///
/// The rows differ along the operation, the member at fault, the constraint class and how
/// many violations one request carries.
#[tokio::test]
async fn a_modelled_constraint_refusal_precedes_the_gate() {
    // (case, operation, method, uri, body) — every one of these is sent by the caller the
    // gate denies, i.e. against a store that records NO payer account.
    let rows = vec![
        (
            "UpdateBillingGroup with a non-conforming Arn",
            "POST",
            "/update-billing-group",
            json!({ "Arn": "not-an-arn" }),
        ),
        (
            "DeleteBillingGroup with a non-conforming Arn",
            "POST",
            "/delete-billing-group",
            json!({ "Arn": "not-an-arn" }),
        ),
        (
            "GetBillingGroupCostReport with a non-conforming Arn",
            "POST",
            "/get-billing-group-cost-report",
            json!({ "Arn": "not-an-arn" }),
        ),
        (
            "AssociateAccounts with a bad account id and a bad Arn",
            "POST",
            "/associate-accounts",
            json!({ "Arn": "not-an-arn", "AccountIds": ["not-an-id"] }),
        ),
        (
            "DisassociateAccounts with a bad account id and a bad Arn",
            "POST",
            "/disassociate-accounts",
            json!({ "Arn": "not-an-arn", "AccountIds": ["not-an-id"] }),
        ),
        (
            "BatchAssociateResourcesToCustomLineItem with a bad resource ARN",
            "PUT",
            "/batch-associate-resources-to-custom-line-item",
            json!({ "TargetArn": "ABCDEFGHIJ", "ResourceArns": ["not-an-arn"] }),
        ),
        (
            "CreateBillingGroup with no required member at all",
            "POST",
            "/create-billing-group",
            json!({}),
        ),
        (
            "ListBillingGroups with a MaxResults below the modelled minimum",
            "POST",
            "/list-billing-groups",
            json!({ "MaxResults": 0 }),
        ),
    ];

    let em = Emulator::standalone();
    for (case, method, uri, body) in &rows {
        let (status, answered, error_type) = em.call(method, uri, Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the boundary's HTTP 400 ahead of the gate, got {status} \
             with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {answered}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected x-amzn-errortype: ValidationException, got {error_type:?}"
        );
        assert!(
            answered["Message"]
                .as_str()
                .is_some_and(|message| message.contains("validation error")),
            "case {case:?}: expected the modelled constraint Message, got {answered}"
        );
    }
}

/// THE RULE's ORDER inside the service: the gate is reached from the caller alone, so it
/// precedes everything the request SAYS — the operation's own semantic validation, its
/// referenced-resource lookups, its resource existence checks, and dispatch to an
/// operation nobody has implemented yet.
///
/// Each row is a MODEL-CONFORMING request (the boundary's own refusal is
/// [`a_modelled_constraint_refusal_precedes_the_gate`]) that the SAME service answers
/// differently for the payer account — the `payer_status` column is the stage the denied
/// request would otherwise have reached — and with the identical 403 denial for every
/// other caller.
#[tokio::test]
async fn the_gate_precedes_semantic_validation_resource_lookup_and_the_scaffold() {
    struct Row {
        case: &'static str,
        operation: &'static str,
        method: &'static str,
        uri: &'static str,
        body: Value,
        /// What the payer account gets for the same request: the stage the denied
        /// request would otherwise have reached.
        payer_status: u16,
    }

    let rows = vec![
        Row {
            case: "a well-formed CreateBillingGroup naming a pricing plan that exists",
            operation: "CreateBillingGroup",
            method: "POST",
            uri: "/create-billing-group",
            body: json!({
                "Name": "GatedGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": { "PricingPlanArn": MANAGED_PLAN_ARN },
            }),
            // Served: the create succeeds for the payer account.
            payer_status: 200,
        },
        Row {
            case: "a CreateBillingGroup with an empty account grouping and no primary account",
            operation: "CreateBillingGroup",
            method: "POST",
            uri: "/create-billing-group",
            body: json!({
                "Name": "GatedEmptyGroup",
                "AccountGrouping": { "LinkedAccountIds": [] },
                "ComputationPreference": { "PricingPlanArn": MANAGED_PLAN_ARN },
            }),
            // Whatever this operation makes of an empty grouping is its own concern
            // (here: a group in `PRIMARY_ACCOUNT_MISSING`); the rule under test is that
            // a caller that is not the payer account never reaches it.
            payer_status: 200,
        },
        Row {
            case: "a CreateBillingGroup naming a pricing plan that exists nowhere",
            operation: "CreateBillingGroup",
            method: "POST",
            uri: "/create-billing-group",
            body: json!({
                "Name": "GatedUnknownPlanGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": {
                    "PricingPlanArn":
                        "arn:aws:billingconductor::000000000000:pricingplan/BOGUS00007",
                },
            }),
            // The payer account reaches this operation's own semantic check: a pricing
            // plan that exists nowhere is its modelled 400 `ValidationException` with
            // the `MISSING_PRICINGPLAN` reason — a different 400 from the boundary's, and
            // still a 403 for everyone else.
            payer_status: 400,
        },
        Row {
            case: "a CreatePricingPlan naming a pricing rule that does not exist",
            operation: "CreatePricingPlan",
            method: "POST",
            uri: "/create-pricing-plan",
            body: json!({ "Name": "GatedPlan", "PricingRuleArns": ["BOGUS00001"] }),
            // Reaches the operation for the payer account; the reference check is that
            // operation's own concern, not the gate's.
            payer_status: 200,
        },
        Row {
            case: "a filter-less ListBillingGroups",
            operation: "ListBillingGroups",
            method: "POST",
            uri: "/list-billing-groups",
            body: json!({ "MaxResults": 5 }),
            payer_status: 200,
        },
        Row {
            case: "a filter-less ListPricingRules",
            operation: "ListPricingRules",
            method: "POST",
            uri: "/list-pricing-rules",
            body: json!({ "MaxResults": 5 }),
            // Served for the payer account: an authorized caller gets the page, which is
            // what makes "authorized, zero rules" and "not authorized" two different
            // answers again.
            payer_status: 200,
        },
        Row {
            case: "a filter-less ListAccountAssociations",
            operation: "ListAccountAssociations",
            method: "POST",
            uri: "/list-account-associations",
            body: json!({}),
            payer_status: 200,
        },
        Row {
            case: "a ListCustomLineItems with a page size",
            operation: "ListCustomLineItems",
            method: "POST",
            uri: "/list-custom-line-items",
            body: json!({ "MaxResults": 5 }),
            payer_status: 501,
        },
        Row {
            case: "a ListCustomLineItemVersions naming a bare custom line item",
            operation: "ListCustomLineItemVersions",
            method: "POST",
            uri: "/list-custom-line-item-versions",
            body: json!({ "Arn": "ABCDEFGHIJ", "MaxResults": 5 }),
            payer_status: 501,
        },
        Row {
            case: "a ListResourcesAssociatedToCustomLineItem naming a bare custom line item",
            operation: "ListResourcesAssociatedToCustomLineItem",
            method: "POST",
            uri: "/list-resources-associated-to-custom-line-item",
            body: json!({ "Arn": "ABCDEFGHIJ" }),
            // The payer account reaches the resource lookup: the modelled 404 for a
            // custom line item that does not exist. A caller that is not the payer
            // never learns whether it exists.
            payer_status: 404,
        },
        Row {
            case: "an AssociatePricingRules naming a plan and a rule that exist nowhere",
            operation: "AssociatePricingRules",
            method: "PUT",
            uri: "/associate-pricing-rules",
            body: json!({
                "Arn": "arn:aws:billingconductor::000000000000:pricingplan/BOGUS00002",
                "PricingRuleArns": ["BOGUS00003"],
            }),
            payer_status: 501,
        },
        Row {
            case: "a DisassociatePricingRules naming a plan and a rule that exist nowhere",
            operation: "DisassociatePricingRules",
            method: "PUT",
            uri: "/disassociate-pricing-rules",
            body: json!({
                "Arn": "arn:aws:billingconductor::000000000000:pricingplan/BOGUS00004",
                "PricingRuleArns": ["BOGUS00005"],
            }),
            payer_status: 501,
        },
        Row {
            case: "a DisassociateAccounts naming a billing group that exists nowhere",
            operation: "DisassociateAccounts",
            method: "POST",
            uri: "/disassociate-accounts",
            body: json!({
                "Arn": "arn:aws:billingconductor::000000000000:billinggroup/BOGUSGROUP1",
                "AccountIds": ["111111111111"],
            }),
            payer_status: 501,
        },
        Row {
            case: "an AssociateAccounts naming a billing group that exists nowhere",
            operation: "AssociateAccounts",
            method: "POST",
            uri: "/associate-accounts",
            body: json!({
                "Arn": "arn:aws:billingconductor::000000000000:billinggroup/BOGUSGROUP2",
                "AccountIds": ["111111111111"],
            }),
            // The payer account reaches the resource lookup: the modelled 404 for a
            // group that does not exist. A caller that is not the payer never learns
            // whether it exists.
            payer_status: 404,
        },
        Row {
            case: "a DeleteBillingGroup naming a billing group that exists nowhere",
            operation: "DeleteBillingGroup",
            method: "POST",
            uri: "/delete-billing-group",
            body: json!({
                "Arn": "arn:aws:billingconductor::000000000000:billinggroup/BOGUSGROUP3",
            }),
            // This operation models no 404 at all, so its own answer for a group that
            // does not exist is the modelled `ValidationException`
            // (`INVALID_BILLING_GROUP`). Which 4xx the operation chooses is its concern;
            // the gate's is that a caller that is not the payer never gets there.
            payer_status: 400,
        },
    ];

    let standalone = Emulator::standalone();
    let payer = Emulator::payer();
    for row in &rows {
        let denied = standalone.call(row.method, row.uri, Some(&row.body)).await;
        assert_denied(row.case, row.operation, &denied);

        let (status, body, _) = payer.call(row.method, row.uri, Some(&row.body)).await;
        assert_eq!(
            status, row.payer_status,
            "case {:?}: expected the payer account to reach HTTP {}, got {status} with {body}",
            row.case, row.payer_status
        );
        assert_ne!(
            error_code(&body),
            "AccessDeniedException",
            "case {:?}: the payer account must not be denied, got {body}",
            row.case
        );
    }
}

/// THE RULE is DETERMINISTIC: many concurrent identical calls are all denied identically
/// and none slips through to a later stage, because the decision depends on the caller
/// and the store alone — not on the order requests arrive in.
#[tokio::test]
async fn concurrent_identical_calls_are_all_denied_identically() {
    let em = Emulator::standalone();
    // (operation, method, uri, body) for the two association operations the recorded
    // race drives, each raced by the same number of callers.
    let races = [
        (
            "AssociatePricingRules",
            "PUT",
            "/associate-pricing-rules",
            json!({
                "Arn": "arn:aws:billingconductor::000000000000:pricingplan/BOGUS00002",
                "PricingRuleArns": ["BOGUS00003"],
            }),
        ),
        (
            "DisassociatePricingRules",
            "PUT",
            "/disassociate-pricing-rules",
            json!({
                "Arn": "arn:aws:billingconductor::000000000000:pricingplan/BOGUS00004",
                "PricingRuleArns": ["BOGUS00005"],
            }),
        ),
    ];

    for (operation, method, uri, body) in &races {
        // Six racers, as in the recorded run: every answer is awaited together, so the
        // calls are in flight at the same time.
        let racers = (0..6).map(|_| em.call(method, uri, Some(body)));
        let answers = futures_lite_join(racers).await;
        assert_eq!(
            answers.len(),
            6,
            "case {operation:?}: expected six answers, got {}",
            answers.len()
        );
        for (racer, answered) in answers.iter().enumerate() {
            assert_denied(
                &format!("{operation} racer {racer} of 6"),
                operation,
                answered,
            );
        }
    }
}

/// Await every future in `futures` concurrently and collect their answers in order.
///
/// A local join rather than a new dependency: the only thing this suite needs from a
/// futures library is "drive these N calls at once", which `FuturesUnordered` would do
/// no better for a fixed, small N.
pub(crate) async fn futures_lite_join<F: std::future::Future>(
    futures: impl Iterator<Item = F>,
) -> Vec<F::Output> {
    let mut pending: Vec<std::pin::Pin<Box<F>>> = futures.map(Box::pin).collect();
    let mut answers: Vec<Option<F::Output>> = (0..pending.len()).map(|_| None).collect();
    std::future::poll_fn(move |cx| {
        let mut all_done = true;
        for (index, future) in pending.iter_mut().enumerate() {
            if answers[index].is_some() {
                continue;
            }
            match future.as_mut().poll(cx) {
                std::task::Poll::Ready(answer) => answers[index] = Some(answer),
                std::task::Poll::Pending => all_done = false,
            }
        }
        if all_done {
            std::task::Poll::Ready(
                answers
                    .iter_mut()
                    .map(|answer| answer.take().expect("every future answered"))
                    .collect(),
            )
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

/// THE RULE is computed from PERSISTED STATE, not from a constant: the gate admits the
/// account the store records as the payer account and nobody else.
#[tokio::test]
async fn the_gate_admits_the_payer_account_the_store_records() {
    // (case, the payer account the store records, whether the emulated caller is admitted)
    let rows = [
        (
            "no organization: the store records no payer account",
            None,
            false,
        ),
        (
            "another account is the payer account",
            Some("111122223333"),
            false,
        ),
        (
            "the emulated account is the payer account",
            Some(account_id()),
            true,
        ),
    ];

    for (case, payer, admitted) in rows {
        let em = Emulator::with_payer(payer);
        let answered = em.post("/list-billing-groups", &json!({})).await;
        if admitted {
            let (status, body, _) = &answered;
            assert_eq!(
                *status, 200,
                "case {case:?}: expected the payer account to be served, got {status} with {body}"
            );
            assert!(
                body["BillingGroups"].is_array(),
                "case {case:?}: expected a billing-group page, got {body}"
            );
        } else {
            assert_denied(case, "ListBillingGroups", &answered);
        }
    }
}

/// The TAG operations are NOT on the gate: a call from a caller that is not the payer
/// account is answered by resolving its `ResourceArn` — HTTP 404 `NotFoundException`
/// "Resource was not found" for a resource this account does not hold, whether or not
/// the value satisfies the member's modelled `@pattern`, which AWS does not enforce as a
/// request-rejecting constraint.
#[tokio::test]
async fn the_tag_operations_resolve_the_arn_instead_of_denying_the_caller() {
    // (case, the ResourceArn, whether the modelled @pattern admits it) — the recorded
    // case is the `billinggroup/BOGUS00006` row, whose non-numeric suffix the pattern
    // refuses.
    let arns = [
        (
            "a billinggroup ARN with a non-numeric suffix (the recorded case)",
            "arn:aws:billingconductor::000000000000:billinggroup/BOGUS00006",
            false,
        ),
        (
            "a pattern-conforming billinggroup ARN naming no group",
            "arn:aws:billingconductor::000000000000:billinggroup/000000000001",
            true,
        ),
        (
            "a pattern-conforming pricingplan ARN naming no plan",
            "arn:aws:billingconductor::000000000000:pricingplan/ABCDEFGHIJ",
            true,
        ),
        (
            "a pattern-conforming pricingrule ARN naming no rule",
            "arn:aws:billingconductor::000000000000:pricingrule/ABCDEFGHIJ",
            true,
        ),
        (
            "a bare custom-line-item id naming no custom line item",
            "KLMNOPQRST",
            true,
        ),
        (
            "another account's billinggroup ARN",
            "arn:aws:billingconductor::111122223333:billinggroup/000000000001",
            true,
        ),
        (
            "a value no alternative of the pattern admits",
            "short",
            false,
        ),
    ];

    // The gate's own store state: NO payer account recorded, so every gated operation
    // would be denied — these are not.
    let em = Emulator::standalone();
    for (case, arn, conforming) in arns {
        let path = tags_path(arn);
        let calls = [
            (
                "TagResource",
                "POST",
                path.clone(),
                Some(json!({ "Tags": { "env": "x" } })),
            ),
            ("ListTagsForResource", "GET", path.clone(), None),
            (
                "UntagResource",
                "DELETE",
                untag_path(arn, &["env"]),
                None,
            ),
        ];
        for (operation, method, uri, body) in &calls {
            let (status, answered, error_type) = em.call(method, uri, body.as_ref()).await;
            let label = format!("{operation} with {case} (pattern-conforming: {conforming})");
            assert_eq!(
                status, 404,
                "case {label:?}: expected HTTP 404, got {status} with {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {label:?}: expected the recorded NotFoundException, got {answered}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("NotFoundException"),
                "case {label:?}: expected x-amzn-errortype: NotFoundException, got {error_type:?}"
            );
            assert_eq!(
                answered["Message"]
                    .as_str()
                    .or_else(|| answered["message"].as_str()),
                Some("Resource was not found"),
                "case {label:?}: expected the recorded message, got {answered}"
            );
        }
    }
}

/// The tag operations are off the gate for a resource that EXISTS, too: a caller that is
/// not the payer account tags, reads and untags a stored resource while the control plane
/// of the same service denies it. That is what the recorded `TagResource` reaching
/// resource resolution means — the control plane that CREATED the resource is the payer
/// account's, the tagging surface is not.
#[tokio::test]
async fn the_tag_operations_serve_a_caller_that_is_not_the_payer_account() {
    let em = Emulator::payer();
    let (status, created, _) = em
        .post("/create-pricing-plan", &json!({ "Name": "PlanToTag" }))
        .await;
    assert_eq!(
        status, 200,
        "setup CreatePricingPlan: expected 200, got {status} with {created}"
    );
    let path = tags_path(created["Arn"].as_str().expect("setup: no Arn"));

    // The organization's payer account is withdrawn from the store: the same caller is
    // now a NON-payer, and the control plane says so.
    em.revoke_payer();
    let denied = em.post("/list-billing-groups", &json!({})).await;
    assert_denied(
        "the control plane after the payer account is withdrawn",
        "ListBillingGroups",
        &denied,
    );

    // (case, method, uri, body, the tags the resource carries afterwards)
    let calls = vec![
        (
            "TagResource writes a pair",
            "POST",
            path.clone(),
            Some(json!({ "Tags": { "env": "prod" } })),
            json!({ "env": "prod" }),
        ),
        (
            "TagResource merges a second pair",
            "POST",
            path.clone(),
            Some(json!({ "Tags": { "owner": "finops" } })),
            json!({ "env": "prod", "owner": "finops" }),
        ),
        (
            "UntagResource removes exactly the key it names",
            "DELETE",
            untag_path(
                created["Arn"].as_str().expect("setup: no Arn"),
                &["env"],
            ),
            None,
            json!({ "owner": "finops" }),
        ),
    ];

    for (case, method, uri, body, expected) in &calls {
        let (status, answered, _) = em.call(method, uri, body.as_ref()).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the tag operation to be served for a non-payer caller, \
             got {status} with {answered}"
        );
        let (status, listed, _) = em.call("GET", &path, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected ListTagsForResource to be served for a non-payer caller, \
             got {status} with {listed}"
        );
        assert_eq!(
            listed["Tags"], *expected,
            "case {case:?}: expected the stored tags {expected}, got {listed}"
        );
    }
}
