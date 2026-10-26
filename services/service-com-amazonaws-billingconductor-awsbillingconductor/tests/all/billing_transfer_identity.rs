// SPDX-License-Identifier: CC0-1.0

//! Which billing transfer a `ResponsibilityTransferArn` names, over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE under test: a billing transfer is an AWS Organizations resource Billing Conductor
//! never creates, and the modelled `ResponsibilityTransferArn` spells its WHOLE identity
//! (`smithy_query(query="shape",shape="ResponsibilityTransferArn")`:
//! `^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$`)
//! — the management account, the organization, the direction and the transfer id. So an
//! operation that addresses a billing transfer resolves it in the set of transfers the
//! organization HAS: an ARN that conforms to the pattern but names no such transfer, in
//! any one of those segments, is answered with the modelled 404
//! `ResourceNotFoundException` ("The request references a resource that doesn't exist")
//! and writes nothing, while a transfer the organization has is served — with the
//! documented default (`Enabled` false, no `PricingPlanArn`, an EMPTY `LastModifiedTime`)
//! while "the preference has never been set for the billing transfer".
//!
//! Each table's rows differ along what the rule generalises over: WHICH segment of the
//! ARN makes the transfer a stranger (the partition, the management account, the
//! organization id, the direction, the transfer id), and — for the transfers that do
//! exist — the form of each of those segments at the bounds the pattern admits (both
//! directions, the shortest and the longest organization id and transfer id).
//!
//! The consequence the store must not invent is pinned too: a preference set against a
//! transfer that does not exist would hold its pricing plan back from `DeletePricingPlan`
//! with the modelled `PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT`, so this
//! suite checks the refused write leaves the plan deletable, and that a transfer that
//! DOES exist still holds it back.
//!
//! Every row's result is read back out of state through a second call —
//! `GetBillingTransferPreference` for a preference, `DeletePricingPlan` for whether a
//! plan is held back — rather than taken from the response of the call under test.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /get-billing-transfer-preference` — the reader's modelled HTTP binding
/// (`smithy_query(query="operation",operation="GetBillingTransferPreference")`).
const GET_PATH: &str = "/get-billing-transfer-preference";

/// `PUT /update-billing-transfer-preference` — the writer's modelled binding.
const UPDATE_PATH: &str = "/update-billing-transfer-preference";

/// `POST /create-pricing-plan`, which gives an enabled preference a plan to name.
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// `POST /delete-pricing-plan` — the probe for "you can't delete that pricing plan".
const DELETE_PLAN_PATH: &str = "/delete-pricing-plan";

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine the service was built on, kept so the suite can record the billing
    /// transfers the organization has — see [`Emulator::designate_transfer`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER (management) account.
    ///
    /// Billing Conductor is a payer-account-only API: its control plane is authorized for
    /// the payer account alone and the store records no payer account by default, so this
    /// harness designates one — the organization whose billing transfers these operations
    /// answer for. The denial every other caller gets is asserted in
    /// `tests/all/payer_account_gate.rs`.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Record a billing transfer the organization HAS, and hand its ARN back —
    /// [`designate_transfer`], the seam every suite that needs a transfer shares.
    fn designate_transfer(&self, arn: &str) -> String {
        designate_transfer(&self.engine, arn)
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

    /// `GetBillingTransferPreference` for one transfer, whatever it answers.
    async fn get(&self, transfer: &str) -> (u16, Value) {
        self.call(
            "POST",
            GET_PATH,
            &json!({ "ResponsibilityTransferArn": transfer }),
        )
        .await
    }

    /// `UpdateBillingTransferPreference` with one preference, whatever it answers.
    async fn update(&self, transfer: &str, preference: &Value) -> (u16, Value) {
        self.call(
            "PUT",
            UPDATE_PATH,
            &json!({
                "ResponsibilityTransferArn": transfer,
                "AutoBillingTransferBillingGroupCreation": preference,
            }),
        )
        .await
    }

    /// `CreatePricingPlan`, asserting the setup succeeded: the plan's full ARN.
    async fn create_plan(&self, case: &str, name: &str) -> String {
        let (status, parsed) = self
            .call("POST", CREATE_PLAN_PATH, &json!({ "Name": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreatePricingPlan {name} expected 200, got {status} with \
             {parsed:?}"
        );
        parsed["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// `DeletePricingPlan` for one plan, whatever it answers.
    async fn delete_plan(&self, plan: &str) -> (u16, Value) {
        self.call("POST", DELETE_PLAN_PATH, &json!({ "Arn": plan }))
            .await
    }
}

/// Record a billing transfer the organization HAS in `engine`'s store, and hand its ARN
/// back.
///
/// `pub(crate)` because every suite that drives an operation addressing a billing transfer
/// needs it, the way `tests/all/payer_account_gate.rs` shares its payer designation: a
/// billing transfer is an AWS Organizations resource and Billing Conductor creates none,
/// so the store holds no transfer until something records one, and an operation addressing
/// a transfer the organization does not have is refused. This stands in for the
/// Organizations surface that mints them, exactly as the suites stand in for the payer
/// designation.
pub(crate) fn designate_transfer(engine: &engine::InMemoryEngine, arn: &str) -> String {
    assert!(
        engine
            .state::<BillingConductorState>()
            .lock()
            .record_billing_transfer(arn),
        "setup: the billing transfer {arn} must be recorded"
    );
    arn.to_string()
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The emulated account — the management account of the organization this store records.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A `ResponsibilityTransferArn` built segment by segment, to the modelled pattern
/// (`smithy_query(query="shape",shape="ResponsibilityTransferArn")`).
fn transfer_arn(partition: &str, account_id: &str, org: &str, direction: &str, id: &str) -> String {
    format!("arn:{partition}:organizations::{account_id}:transfer/o-{org}/billing/{direction}/rt-{id}")
}

/// A billing transfer of THIS organization, in the usual partition.
fn ours(org: &str, direction: &str, id: &str) -> String {
    transfer_arn("aws", account(), org, direction, id)
}

/// Assert both operations answer the modelled 404 for `transfer`, naming the resource
/// they could not find.
async fn assert_not_found(em: &Emulator, case: &str, transfer: &str) {
    for (operation, (status, body)) in [
        ("GetBillingTransferPreference", em.get(transfer).await),
        (
            "UpdateBillingTransferPreference",
            em.update(transfer, &json!({ "Enabled": false })).await,
        ),
    ] {
        assert_eq!(
            status, 404,
            "case {case:?}: expected {operation} to answer 404 for a transfer the organization \
             does not have, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected {operation} to answer ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(transfer),
            "case {case:?}: expected {operation} to name the transfer it could not find, got \
             {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("transfer"),
            "case {case:?}: expected {operation} to report the transfer resource type, got \
             {body:?}"
        );
    }
}

/// Assert `transfer` reads back as a transfer that exists and whose preference was never
/// set: the documented default with an EMPTY `LastModifiedTime`.
async fn assert_never_set(em: &Emulator, case: &str, transfer: &str) {
    let (status, body) = em.get(transfer).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a transfer the organization has to be served, got {status} with \
         {body:?}"
    );
    assert_eq!(
        body["ResponsibilityTransferArn"].as_str(),
        Some(transfer),
        "case {case:?}: expected the answer to name the transfer, got {body:?}"
    );
    assert_eq!(
        body["AutoBillingTransferBillingGroupCreation"]["Enabled"].as_bool(),
        Some(false),
        "case {case:?}: the preference is disabled by default, got {body:?}"
    );
    assert!(
        body["AutoBillingTransferBillingGroupCreation"]["PricingPlanArn"].is_null(),
        "case {case:?}: a disabled preference names no pricing plan, got {body:?}"
    );
    assert!(
        body["LastModifiedTime"].as_i64().is_none(),
        "case {case:?}: LastModifiedTime is empty while the preference has never been set, got \
         {body:?}"
    );
}

/// RULE: an ARN that conforms to the modelled pattern but names no billing transfer of
/// the organization — in ANY segment of the identity it spells — is answered 404
/// `ResourceNotFoundException` by both billing-transfer operations, and nothing is
/// recorded for it; a transfer the organization HAS is served with the documented
/// default.
#[tokio::test]
async fn a_transfer_the_organization_does_not_have_is_not_found() {
    let em = Emulator::new();
    // The transfer the organization really has; every stranger below differs from it in
    // exactly ONE segment.
    let held = em.designate_transfer(&ours("abcd123456", "inbound", "1234abcd"));

    // Rows differ along which segment of the ARN makes it name no transfer of this
    // organization: the transfer id (never minted, and at each bound the pattern admits),
    // the organization id (another organization of this account, at each bound), the
    // direction, the management account, and the partition.
    let strangers = [
        (
            "a transfer id nobody ever created",
            ours("abcd123456", "inbound", "zzzzzzzz"),
        ),
        (
            "the shortest transfer id the pattern admits, never created",
            ours("abcd123456", "inbound", "00000000"),
        ),
        (
            "the longest transfer id the pattern admits, never created",
            ours("abcd123456", "inbound", "0123456789abcdef0123456789abcdef"),
        ),
        (
            "the held transfer id under another organization of this account",
            ours("abcd123457", "inbound", "1234abcd"),
        ),
        (
            "the shortest organization id the pattern admits, never created",
            ours("0123456789", "inbound", "1234abcd"),
        ),
        (
            "the longest organization id the pattern admits, never created",
            ours("0123456789abcdefghij0123456789ab", "inbound", "1234abcd"),
        ),
        (
            "the held transfer with its direction flipped",
            ours("abcd123456", "outbound", "1234abcd"),
        ),
        (
            "the held transfer under another organization's management account",
            transfer_arn("aws", "210987654321", "abcd123456", "inbound", "1234abcd"),
        ),
        (
            "the held transfer in another partition",
            transfer_arn("aws-cn", account(), "abcd123456", "inbound", "1234abcd"),
        ),
    ];

    for (case, stranger) in &strangers {
        // Both operations refuse it, and the refused write recorded nothing: the stranger
        // is still not found, and the transfer that DOES exist is still "never set".
        assert_not_found(&em, case, stranger).await;
        assert_not_found(&em, case, stranger).await;
        assert_never_set(&em, case, &held).await;
    }

    // The transfers the organization HAS are served, whatever form each segment takes:
    // both directions, and the shortest and the longest organization id and transfer id
    // the modelled pattern admits.
    let existing = [
        ("the transfer already held", held.clone()),
        (
            "an outbound transfer",
            em.designate_transfer(&ours("abcd123456", "outbound", "beef1234")),
        ),
        (
            "the shortest organization id and transfer id",
            em.designate_transfer(&ours("0123456789", "inbound", "0a1b2c3d")),
        ),
        (
            "the longest organization id and transfer id",
            em.designate_transfer(&ours(
                "0123456789abcdefghij0123456789ab",
                "outbound",
                "fedcba9876543210fedcba9876543210",
            )),
        ),
    ];

    for (case, transfer) in &existing {
        assert_never_set(&em, case, transfer).await;
    }

    // And a preference set on one of them reads back, while its neighbours stay "never
    // set": the preference belongs to the transfer its whole ARN names.
    let plan = em.create_plan("setup", "TransferIdentityPlan").await;
    let (status, body) = em
        .update(&held, &json!({ "Enabled": true, "PricingPlanArn": &plan }))
        .await;
    assert_eq!(
        status, 200,
        "expected the preference of a transfer the organization has to be recorded, got {status} \
         with {body:?}"
    );
    let (status, read) = em.get(&held).await;
    assert_eq!(
        status, 200,
        "expected the held transfer to be served after the write, got {status} with {read:?}"
    );
    assert_eq!(
        read["AutoBillingTransferBillingGroupCreation"]["PricingPlanArn"].as_str(),
        Some(plan.as_str()),
        "expected the recorded pricing plan to read back, got {read:?}"
    );
    for (case, transfer) in existing.iter().skip(1) {
        assert_never_set(&em, case, transfer).await;
    }
    for (case, stranger) in &strangers {
        assert_not_found(&em, case, stranger).await;
    }
}

/// RULE: a preference is never recorded against a billing transfer the organization does
/// not have, so such a transfer cannot hold a pricing plan back: `DeletePricingPlan`
/// answers the modelled 409 `ConflictException`
/// (`PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT`) only while a transfer that
/// EXISTS names that plan in an enabled preference.
#[tokio::test]
async fn a_transfer_that_does_not_exist_holds_no_pricing_plan_back() {
    // Rows differ along the segment that makes the stranger a stranger and along how the
    // preference spells the plan (full ARN, bare id).
    let rows = [
        (
            "a transfer id nobody ever created, named by full ARN",
            ours("deleteorg0", "inbound", "zzzzzzzz"),
            ours("deleteorg0", "inbound", "11112222"),
            true,
        ),
        (
            "another organization of this account, named by bare id",
            ours("deleteorg1", "outbound", "1234abcd"),
            ours("deleteorg2", "outbound", "1234abcd"),
            false,
        ),
        (
            "another organization's management account, named by full ARN",
            transfer_arn("aws", "210987654321", "deleteorg3", "inbound", "1234abcd"),
            ours("deleteorg3", "inbound", "1234abcd"),
            true,
        ),
    ];

    for (case, stranger, existing, by_full_arn) in &rows {
        let em = Emulator::new();
        let plan = em.create_plan(case, "StrangerConflictPlan").await;
        let named = if *by_full_arn {
            plan.clone()
        } else {
            plan.rsplit('/').next().unwrap_or_default().to_string()
        };

        let (status, body) = em
            .update(
                stranger,
                &json!({ "Enabled": true, "PricingPlanArn": &named }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the write against a transfer that does not exist to be \
             refused 404, got {status} with {body:?}"
        );

        // Nothing was recorded, so nothing holds the plan back.
        let (deleted, deleted_body) = em.delete_plan(&plan).await;
        assert_eq!(
            deleted, 200,
            "case {case:?}: a transfer that does not exist must not hold the plan back, got \
             {deleted} with {deleted_body:?}"
        );

        // The contrast, in a fresh store: a transfer the organization HAS, with the same
        // preference, does hold the plan back.
        let em = Emulator::new();
        let plan = em.create_plan(case, "StrangerConflictPlan").await;
        let named = if *by_full_arn {
            plan.clone()
        } else {
            plan.rsplit('/').next().unwrap_or_default().to_string()
        };
        em.designate_transfer(existing);
        let (status, body) = em
            .update(
                existing,
                &json!({ "Enabled": true, "PricingPlanArn": &named }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the preference of a transfer the organization has to be \
             recorded, got {status} with {body:?}"
        );
        let (held_back, held_body) = em.delete_plan(&plan).await;
        assert_eq!(
            held_back, 409,
            "case {case:?}: a plan an enabled preference of an existing transfer names cannot be \
             deleted, got {held_back} with {held_body:?}"
        );
        assert_eq!(
            held_body["Reason"].as_str(),
            Some("PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT"),
            "case {case:?}: expected the preference-reference reason, got {held_body:?}"
        );
    }
}
