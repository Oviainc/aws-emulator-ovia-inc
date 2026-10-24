// SPDX-License-Identifier: CC0-1.0

//! `GetBillingTransferPreference` and `UpdateBillingTransferPreference` end to end over
//! the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the
//! same path the production router serves.
//!
//! RULE under test: the auto billing group creation preference is per BILLING TRANSFER
//! state. `UpdateBillingTransferPreference` records the whole preference the request
//! carries for the transfer its `ResponsibilityTransferArn` names and stamps
//! `LastModifiedTime` with the instant it did, and `GetBillingTransferPreference`
//! reports exactly what the store holds for that transfer — the documented default
//! (`Enabled` false, no pricing plan) with an EMPTY `LastModifiedTime` while "the
//! preference has never been set for the billing transfer", and otherwise the last
//! recorded preference with the instant it was recorded. Around that core loop:
//!
//! * a billing transfer is an AWS Organizations resource whose ARN carries the
//!   management account of the organization that owns it, and Billing Conductor answers
//!   for the one organization whose management account is the payer — so an ARN owned by
//!   any other account names a transfer this organization does not have, and both
//!   operations answer the modelled 404 `ResourceNotFoundException` for it, writing
//!   nothing;
//! * `AutoTransferBillingGroupCreationPreference.PricingPlanArn` "is required when
//!   `Enabled` is `true`, and must be omitted when `Enabled` is `false`", and an enabled
//!   preference's plan has to exist — each refused with the modelled 400
//!   `ValidationException` (`MISSING_PRICING_PLAN_ARN`, `FIELD_VALIDATION_FAILED`,
//!   `MISSING_PRICINGPLAN`), writing nothing;
//! * "while a pricing plan is specified in an enabled preference, you can't delete that
//!   pricing plan": `DeletePricingPlan` answers the modelled 409 `ConflictException`
//!   with the `PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT` reason
//!   `ConflictExceptionReason` enumerates, and the plan is still there afterwards;
//! * `ClientToken` is `@idempotencyToken`, so a retry "completes successfully without
//!   performing any further actions": it answers with the preference the first call
//!   recorded and moves nothing. That replay is scoped to the billing transfer the
//!   recorded call addressed, because the preference is per transfer: the same token
//!   carried against ANOTHER transfer is a different request, applied there, and leaves
//!   the first transfer's preference and instant alone.
//!
//! Every table's rows differ along what the rule generalises over: the form of the
//! transfer ARN (direction `inbound`/`outbound`, the shortest and the longest
//! organization id and transfer id the modelled pattern admits), the form of the
//! preference (disabled, enabled naming a plan by full ARN, by bare id, and naming each
//! of the two plans Amazon Web Services owns), how many times one transfer's preference
//! is rewritten, which account owns a foreign transfer, why a preference is refused, how
//! many transfers hold one plan back, and the form of the `ClientToken`.
//!
//! Each row's result is read back out of state through a SECOND call rather than taken
//! from the response of the call under test: `GetBillingTransferPreference` is the
//! reader, and `DeletePricingPlan` / `CreatePricingPlan` report whether the plan a
//! preference named is still stored.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /get-billing-transfer-preference` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="GetBillingTransferPreference")`).
const GET_PATH: &str = "/get-billing-transfer-preference";

/// `PUT /update-billing-transfer-preference` — the modelled binding of the one writer
/// (`smithy_query(query="operation",operation="UpdateBillingTransferPreference")`).
const UPDATE_PATH: &str = "/update-billing-transfer-preference";

/// `POST /create-pricing-plan`, which gives an enabled preference a plan to name.
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// `POST /delete-pricing-plan` — the probe for "you can't delete that pricing plan".
const DELETE_PLAN_PATH: &str = "/delete-pricing-plan";

/// The header `ClientToken` travels in: its modelled
/// `@httpHeader("X-Amzn-Client-Token")` binding.
const CLIENT_TOKEN_HEADER: &str = "x-amzn-client-token";

/// The pricing plan ids Amazon Web Services owns rather than the caller, enumerated by
/// the modelled `PricingPlanArn` pattern beside the customer-minted `[a-zA-Z0-9]{10}`
/// form (`smithy_query(query="shape",shape="PricingPlanArn")`). Their ARN carries the
/// literal `aws` account segment the same pattern allows.
const AWS_MANAGED_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER (management) account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): its control plane is authorized for the payer account alone, and the store
    /// records no payer account by default — so this harness designates one, which is
    /// both the world in which these operations are reachable at all and the
    /// organization whose billing transfers they answer for. The denial every other
    /// caller gets is asserted in `tests/all/payer_account_gate.rs`.
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
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: &Value,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
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
        let request = builder
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
            &[],
        )
        .await
    }

    /// `GetBillingTransferPreference`, asserting it was served: the parsed body.
    async fn read_back(&self, case: &str, transfer: &str) -> Value {
        let (status, parsed) = self.get(transfer).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetBillingTransferPreference {transfer} expected 200, got {status} \
             with {parsed:?}"
        );
        parsed
    }

    /// `UpdateBillingTransferPreference` with the preference `preference`, whatever it
    /// answers. `token` is the `ClientToken`, carried in its modelled header.
    async fn update(
        &self,
        transfer: &str,
        preference: &Value,
        token: Option<&str>,
    ) -> (u16, Value) {
        let body = json!({
            "ResponsibilityTransferArn": transfer,
            "AutoBillingTransferBillingGroupCreation": preference,
        });
        let headers: Vec<(&str, &str)> = token
            .map(|value| vec![(CLIENT_TOKEN_HEADER, value)])
            .unwrap_or_default();
        self.call("PUT", UPDATE_PATH, &body, &headers).await
    }

    /// `UpdateBillingTransferPreference`, asserting it was served: the parsed body.
    async fn set(
        &self,
        case: &str,
        transfer: &str,
        preference: &Value,
        token: Option<&str>,
    ) -> Value {
        let (status, parsed) = self.update(transfer, preference, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateBillingTransferPreference {preference} expected 200, got \
             {status} with {parsed:?}"
        );
        parsed
    }

    /// `CreatePricingPlan`, asserting the setup succeeded: the plan's full ARN.
    async fn create_plan(&self, case: &str, name: &str) -> String {
        let (status, parsed) = self
            .call("POST", CREATE_PLAN_PATH, &json!({ "Name": name }), &[])
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
        self.call("POST", DELETE_PLAN_PATH, &json!({ "Arn": plan }), &[])
            .await
    }
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

/// The emulated account — the management account of the organization this store records,
/// and so the account every billing transfer it owns carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A `ResponsibilityTransferArn` of an organization whose management account is
/// `account_id`, built to the modelled pattern
/// (`^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$`,
/// `smithy_query(query="shape",shape="ResponsibilityTransferArn")`).
fn transfer_arn_of(account_id: &str, org: &str, direction: &str, id: &str) -> String {
    format!("arn:aws:organizations::{account_id}:transfer/o-{org}/billing/{direction}/rt-{id}")
}

/// A billing transfer of THIS organization.
fn transfer_arn(org: &str, direction: &str, id: &str) -> String {
    transfer_arn_of(account(), org, direction, id)
}

/// The full `PricingPlanArn` of a plan id held by this account.
fn plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:pricingplan/{id}", account())
}

/// The ARN of a plan Amazon Web Services owns: the `PricingPlanArn` pattern spells the
/// account segment of those as the literal `aws`.
fn managed_plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::aws:pricingplan/{id}")
}

/// The FULL plan ARN a preference request's `PricingPlanArn` resolves to: the member
/// admits the full ARN, the bare 10-character id of a customer plan, or the id of a plan
/// Amazon Web Services owns (whose ARN carries the literal `aws` account segment).
fn expected_plan(preference: &Value) -> Option<String> {
    let named = preference["PricingPlanArn"].as_str()?;
    if named.starts_with("arn:") {
        Some(named.to_string())
    } else if AWS_MANAGED_PLAN_IDS.contains(&named) {
        Some(managed_plan_arn(named))
    } else {
        Some(plan_arn(named))
    }
}

/// The bare-id spelling of a full ARN — the other form the modelled `PricingPlanArn`
/// pattern admits for the same plan.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// One preference a row sets, and what the store should hold afterwards.
struct Setting {
    /// The `AutoBillingTransferBillingGroupCreation` the request carries.
    request: Value,
    /// `Enabled` as a later read must report it.
    enabled: bool,
    /// The FULL pricing plan ARN a later read must report, if any.
    plan: Option<String>,
}

/// Assert one `GetBillingTransferPreference` body reports `enabled` / `plan`, and hand
/// back its `LastModifiedTime`.
fn assert_reports(
    case: &str,
    body: &Value,
    transfer: &str,
    enabled: bool,
    plan: Option<&str>,
) -> Option<i64> {
    assert_eq!(
        body["ResponsibilityTransferArn"].as_str(),
        Some(transfer),
        "case {case:?}: expected the answer to name the transfer {transfer}, got {body:?}"
    );
    let preference = &body["AutoBillingTransferBillingGroupCreation"];
    assert_eq!(
        preference["Enabled"].as_bool(),
        Some(enabled),
        "case {case:?}: expected Enabled {enabled}, got {body:?}"
    );
    assert_eq!(
        preference["PricingPlanArn"].as_str(),
        plan,
        "case {case:?}: expected PricingPlanArn {plan:?}, got {body:?}"
    );
    body["LastModifiedTime"].as_i64()
}

/// RULE: `GetBillingTransferPreference` reports what the store holds for the billing
/// transfer the request names — the documented default with an EMPTY `LastModifiedTime`
/// while nothing has set one, and otherwise the preference the last
/// `UpdateBillingTransferPreference` recorded, with the instant it recorded it. Each
/// transfer's preference is its own.
#[tokio::test]
async fn get_reports_the_preference_the_last_update_persisted() {
    let em = Emulator::new();
    let first_plan = em.create_plan("setup", "TransferPreferencePlanOne").await;
    let second_plan = em.create_plan("setup", "TransferPreferencePlanTwo").await;

    // The rows differ in the form of the transfer ARN (both directions, the shortest and
    // the longest organization id and transfer id the modelled pattern admits) and in
    // the form and number of the preferences set on it: disabled only, enabled by full
    // ARN, enabled by the bare id of the SAME plan, enabled then repointed at another
    // plan, enabled then disabled, and each of the two plans Amazon Web Services owns.
    let rows: Vec<(&str, String, Vec<Setting>)> = vec![
        (
            "a single disabled preference",
            transfer_arn("abcdefghij", "outbound", "abcdefgh"),
            vec![Setting {
                request: json!({ "Enabled": false }),
                enabled: false,
                plan: None,
            }],
        ),
        (
            "enabled, naming a plan by its full ARN",
            transfer_arn("abcdefghij", "inbound", "12345678"),
            vec![Setting {
                request: json!({ "Enabled": true, "PricingPlanArn": &first_plan }),
                enabled: true,
                plan: Some(first_plan.clone()),
            }],
        ),
        (
            "enabled, naming the same plan by its bare id",
            transfer_arn("0123456789abcdefghij0123456789ab", "inbound", "0a1b2c3d"),
            vec![Setting {
                request: json!({ "Enabled": true, "PricingPlanArn": bare_id(&first_plan) }),
                enabled: true,
                plan: Some(first_plan.clone()),
            }],
        ),
        (
            "enabled, then repointed at another plan, then disabled",
            transfer_arn(
                "zyxwvutsrq",
                "outbound",
                "0123456789abcdef0123456789abcdef",
            ),
            vec![
                Setting {
                    request: json!({ "Enabled": true, "PricingPlanArn": &first_plan }),
                    enabled: true,
                    plan: Some(first_plan.clone()),
                },
                Setting {
                    request: json!({ "Enabled": true, "PricingPlanArn": &second_plan }),
                    enabled: true,
                    plan: Some(second_plan.clone()),
                },
                Setting {
                    request: json!({ "Enabled": false }),
                    enabled: false,
                    plan: None,
                },
            ],
        ),
        (
            "enabled, naming the AWS-managed BasicPricingPlan",
            transfer_arn("managedplan0", "inbound", "11112222"),
            vec![Setting {
                request: json!({ "Enabled": true, "PricingPlanArn": AWS_MANAGED_PLAN_IDS[0] }),
                enabled: true,
                plan: Some(managed_plan_arn(AWS_MANAGED_PLAN_IDS[0])),
            }],
        ),
        (
            "enabled, naming the AWS-managed Passthrough plan by its full ARN",
            transfer_arn("managedplan1", "outbound", "33334444"),
            vec![Setting {
                request: json!({
                    "Enabled": true,
                    "PricingPlanArn": managed_plan_arn(AWS_MANAGED_PLAN_IDS[1]),
                }),
                enabled: true,
                plan: Some(managed_plan_arn(AWS_MANAGED_PLAN_IDS[1])),
            }],
        ),
    ];

    for (case, transfer, settings) in &rows {
        // Before anything sets it: the documented default, and NO `LastModifiedTime` —
        // "this value is empty if the preference has never been set for the billing
        // transfer".
        let unset = em.read_back(case, transfer).await;
        let never_set = assert_reports(case, &unset, transfer, false, None);
        assert!(
            never_set.is_none(),
            "case {case:?}: an unset preference must report no LastModifiedTime, got \
             {never_set:?} in {unset:?}"
        );

        let mut previous: Option<i64> = None;
        for setting in settings {
            let written = em.set(case, transfer, &setting.request, None).await;
            let stamped = assert_reports(
                case,
                &written,
                transfer,
                setting.enabled,
                setting.plan.as_deref(),
            );
            let stamped = stamped.unwrap_or_else(|| {
                panic!("case {case:?}: a recorded preference must carry LastModifiedTime: {written:?}")
            });
            assert!(
                stamped > 0,
                "case {case:?}: expected a millisecond instant, got {stamped} in {written:?}"
            );
            if let Some(earlier) = previous {
                assert!(
                    stamped >= earlier,
                    "case {case:?}: expected LastModifiedTime to move forward from {earlier}, \
                     got {stamped}"
                );
            }
            previous = Some(stamped);

            // The read is what the response claimed, out of state rather than echoed —
            // and a read does not change it, so the second one answers the same.
            let read = em.read_back(case, transfer).await;
            let read_stamp = assert_reports(
                case,
                &read,
                transfer,
                setting.enabled,
                setting.plan.as_deref(),
            );
            assert_eq!(
                read_stamp,
                Some(stamped),
                "case {case:?}: expected the read to report the recorded instant, got {read:?}"
            );
            let again = em.read_back(case, transfer).await;
            assert_eq!(
                again, read,
                "case {case:?}: a second read must answer the same, got {again:?} after {read:?}"
            );
        }
    }

    // Each transfer's preference is its own: every row's final state still reads back
    // after every other row ran.
    for (case, transfer, settings) in &rows {
        let last = settings
            .last()
            .expect("every row sets at least one preference");
        let read = em.read_back(case, transfer).await;
        assert_reports(case, &read, transfer, last.enabled, last.plan.as_deref());
    }
}

/// RULE: a billing transfer whose ARN names another organization's management account is
/// not a transfer this organization has, so both operations answer the modelled 404
/// `ResourceNotFoundException` and the write records nothing.
#[tokio::test]
async fn a_billing_transfer_of_another_organization_is_not_found() {
    let em = Emulator::new();
    let plan = em.create_plan("setup", "ForeignTransferPlan").await;
    let ours = transfer_arn("ourorgabcd", "inbound", "abcd1234");

    // The rows differ in which account owns the transfer and in the form of the ARN; the
    // last two are this organization's own id with a digit changed, so the only
    // difference from a transfer that IS ours is the owning account.
    let rows = [
        (
            "another organization's management account",
            transfer_arn_of("111122223333", "otherorgab", "inbound", "deadbeef"),
        ),
        (
            "an account that is nobody's in this emulator",
            transfer_arn_of("999999999999", "zzzzzzzzzz", "outbound", "99998888"),
        ),
        (
            "the payer account id with its last digit changed",
            transfer_arn_of("000000000001", "ourorgabcd", "inbound", "abcd1234"),
        ),
        (
            "the payer account id with its first digit changed",
            transfer_arn_of("100000000000", "ourorgabcd", "outbound", "abcd1234"),
        ),
    ];

    for (case, foreign) in &rows {
        for (operation, (status, body)) in [
            ("GetBillingTransferPreference", em.get(foreign).await),
            (
                "UpdateBillingTransferPreference",
                em.update(
                    foreign,
                    &json!({ "Enabled": true, "PricingPlanArn": &plan }),
                    None,
                )
                .await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?}: expected {operation} to answer 404 for a transfer of another \
                 organization, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected {operation} to answer ResourceNotFoundException, got \
                 {body:?}"
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(foreign.as_str()),
                "case {case:?}: expected {operation} to name the transfer it could not find, \
                 got {body:?}"
            );
            assert_eq!(
                body["ResourceType"].as_str(),
                Some("transfer"),
                "case {case:?}: expected {operation} to report the transfer resource type, got \
                 {body:?}"
            );
        }

        // The refused write recorded nothing: THIS organization's transfer is still
        // unset, and so is the foreign one if it ever became readable.
        let read = em.read_back(case, &ours).await;
        let stamp = assert_reports(case, &read, &ours, false, None);
        assert!(
            stamp.is_none(),
            "case {case:?}: a refused write must record nothing, got {read:?}"
        );
    }
}

/// RULE: `AutoTransferBillingGroupCreationPreference.PricingPlanArn` "is required when
/// `Enabled` is `true`, and must be omitted when `Enabled` is `false`", and the plan an
/// enabled preference names has to exist. Each refusal is the modelled 400
/// `ValidationException` with the reason the model enumerates, and records nothing.
#[tokio::test]
async fn an_enabled_preference_requires_a_pricing_plan_that_exists() {
    let em = Emulator::new();
    let plan = em.create_plan("setup", "PreferenceValidationPlan").await;
    let transfer = transfer_arn("validateorg", "inbound", "11223344");

    // The rows differ in which half of the conditional rule they break, in how the plan
    // is spelled, and in why a named plan resolves nowhere (never minted, another
    // account, another partition).
    let refused = [
        (
            "enabled with no pricing plan",
            json!({ "Enabled": true }),
            "MISSING_PRICING_PLAN_ARN",
        ),
        (
            "disabled with a pricing plan named by full ARN",
            json!({ "Enabled": false, "PricingPlanArn": &plan }),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            "disabled with a pricing plan named by bare id",
            json!({ "Enabled": false, "PricingPlanArn": bare_id(&plan) }),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            "disabled with an AWS-managed pricing plan",
            json!({ "Enabled": false, "PricingPlanArn": AWS_MANAGED_PLAN_IDS[0] }),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            "enabled with a bare plan id nothing ever created",
            json!({ "Enabled": true, "PricingPlanArn": "BOGUS00001" }),
            "MISSING_PRICINGPLAN",
        ),
        (
            "enabled with a plan ARN of another account",
            json!({
                "Enabled": true,
                "PricingPlanArn": "arn:aws:billingconductor::111122223333:pricingplan/ABCDEFGHIJ",
            }),
            "MISSING_PRICINGPLAN",
        ),
        (
            "enabled with an AWS-managed plan id in this account's ARN",
            json!({ "Enabled": true, "PricingPlanArn": plan_arn(AWS_MANAGED_PLAN_IDS[0]) }),
            "MISSING_PRICINGPLAN",
        ),
    ];

    for (case, preference, reason) in &refused {
        let (status, body) = em.update(&transfer, preference, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400 for {preference}, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some(*reason),
            "case {case:?}: expected the {reason} reason, got {body:?}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("AutoBillingTransferBillingGroupCreation.PricingPlanArn"),
            "case {case:?}: expected the refusal to name the member at fault, got {body:?}"
        );
        assert!(
            body["Message"].as_str().is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected a non-empty modelled Message, got {body:?}"
        );

        // A refusal writes nothing: the transfer's preference is still "never set".
        let read = em.read_back(case, &transfer).await;
        let stamp = assert_reports(case, &read, &transfer, false, None);
        assert!(
            stamp.is_none(),
            "case {case:?}: a refused write must record nothing, got {read:?}"
        );
    }

    // The accepted forms either side of the rule: enabled WITH a plan that exists, and
    // disabled with none.
    for (case, preference, enabled, expected) in [
        (
            "enabled with a plan that exists",
            json!({ "Enabled": true, "PricingPlanArn": &plan }),
            true,
            Some(plan.clone()),
        ),
        (
            "disabled with no plan",
            json!({ "Enabled": false }),
            false,
            None,
        ),
    ] {
        let written = em.set(case, &transfer, &preference, None).await;
        assert_reports(case, &written, &transfer, enabled, expected.as_deref());
    }
}

/// RULE: "while a pricing plan is specified in an enabled preference, you can't delete
/// that pricing plan" — `DeletePricingPlan` answers the modelled 409 `ConflictException`
/// with the `PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT` reason and leaves
/// the plan stored; a plan no ENABLED preference names deletes normally.
#[tokio::test]
async fn a_pricing_plan_an_enabled_preference_names_cannot_be_deleted() {
    // The rows differ in how the preference spelled the plan and in how many transfers
    // name it; the last row's preference is disabled, so it holds nothing back.
    let rows: Vec<(&str, bool, usize, bool)> = vec![
        ("one enabled preference naming the plan by full ARN", true, 1, true),
        ("one enabled preference naming the plan by bare id", false, 1, true),
        ("three enabled preferences naming the plan", true, 3, true),
        ("a disabled preference, which names no plan at all", true, 1, false),
    ];

    for (case, by_full_arn, transfers, enabled) in rows {
        let em = Emulator::new();
        let plan = em.create_plan(case, "ConflictPlan").await;
        let named = if by_full_arn {
            plan.clone()
        } else {
            bare_id(&plan)
        };
        let arns: Vec<String> = (0..transfers)
            .map(|index| transfer_arn(&format!("conflictor{index}"), "inbound", "5566778899"))
            .collect();
        for transfer in &arns {
            let preference = if enabled {
                json!({ "Enabled": true, "PricingPlanArn": &named })
            } else {
                json!({ "Enabled": false })
            };
            em.set(case, transfer, &preference, None).await;
        }

        let (status, body) = em.delete_plan(&plan).await;
        if enabled {
            assert_eq!(
                status, 409,
                "case {case:?}: expected the delete of a plan an enabled preference names to be \
                 refused 409, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?}: expected a ConflictException, got {body:?}"
            );
            assert_eq!(
                body["Reason"].as_str(),
                Some("PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT"),
                "case {case:?}: expected the preference-reference reason, got {body:?}"
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(plan.as_str()),
                "case {case:?}: expected the refusal to name the plan, got {body:?}"
            );

            // The refused delete left the plan stored: disabling every preference that
            // named it frees it, and the delete then succeeds.
            for transfer in &arns {
                em.set(case, transfer, &json!({ "Enabled": false }), None).await;
            }
            let (freed, freed_body) = em.delete_plan(&plan).await;
            assert_eq!(
                freed, 200,
                "case {case:?}: expected the plan to still exist and delete once no enabled \
                 preference names it, got {freed} with {freed_body:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to be served, got {status} with {body:?}"
            );
        }

        // Whatever the delete did, the preferences still read back: a disabled one keeps
        // the instant it was set at, and names no plan.
        for transfer in &arns {
            let read = em.read_back(case, transfer).await;
            let stamp = assert_reports(case, &read, transfer, false, None);
            assert!(
                stamp.is_some(),
                "case {case:?}: a preference that WAS set keeps its LastModifiedTime, got \
                 {read:?}"
            );
        }
    }
}

/// RULE: `ClientToken` is `@idempotencyToken` — "if the original request completes
/// successfully, any subsequent retries complete successfully without performing any
/// further actions", so a retry answers with the preference the first call recorded and
/// changes nothing. A different token is a different request.
#[tokio::test]
async fn a_retried_client_token_performs_no_further_action() {
    let em = Emulator::new();
    let first_plan = em.create_plan("setup", "IdempotentPlanOne").await;
    let second_plan = em.create_plan("setup", "IdempotentPlanTwo").await;

    // The rows differ in the form of the `ClientToken` (the modelled `^[a-zA-Z0-9-]+$`
    // at its length minimum of 1, a hyphenated token, and one at the length maximum of
    // 64) and in what the retry asks for.
    let rows: Vec<(&str, String, Value, Value)> = vec![
        (
            "a one-character token, retried with the same preference",
            "a".to_string(),
            json!({ "Enabled": true, "PricingPlanArn": &first_plan }),
            json!({ "Enabled": true, "PricingPlanArn": &first_plan }),
        ),
        (
            "a hyphenated token, retried asking for another plan",
            "retry-token-0001".to_string(),
            json!({ "Enabled": true, "PricingPlanArn": &first_plan }),
            json!({ "Enabled": true, "PricingPlanArn": &second_plan }),
        ),
        (
            "a 64-character token, retried asking to disable",
            "T".repeat(64),
            json!({ "Enabled": true, "PricingPlanArn": &second_plan }),
            json!({ "Enabled": false }),
        ),
    ];

    for (index, (case, token, first, retry)) in rows.iter().enumerate() {
        let transfer = transfer_arn(&format!("idempotent{index}"), "outbound", "a1b2c3d4");
        let written = em.set(case, &transfer, first, Some(token.as_str())).await;
        let stamped = assert_reports(
            case,
            &written,
            &transfer,
            first["Enabled"].as_bool().unwrap_or_default(),
            expected_plan(first).as_deref(),
        );

        let replayed = em.set(case, &transfer, retry, Some(token.as_str())).await;
        assert_eq!(
            replayed, written,
            "case {case:?}: a retry of the same ClientToken must answer what the first call \
             recorded, got {replayed:?} after {written:?}"
        );
        let read = em.read_back(case, &transfer).await;
        assert_eq!(
            read["AutoBillingTransferBillingGroupCreation"],
            written["AutoBillingTransferBillingGroupCreation"],
            "case {case:?}: a retry must leave the recorded preference alone, got {read:?}"
        );
        assert_eq!(
            read["LastModifiedTime"].as_i64(),
            stamped,
            "case {case:?}: a retry must not move LastModifiedTime, got {read:?}"
        );

        // A DIFFERENT token is a different request, so the same body now applies. The
        // fresh token is built per row rather than from `token`, which is already at the
        // modelled 64-character maximum in one of them.
        let fresh = format!("fresh-token-{index}");
        let applied = em.set(case, &transfer, retry, Some(&fresh)).await;
        let applied_plan = expected_plan(retry);
        assert_reports(
            case,
            &applied,
            &transfer,
            retry["Enabled"].as_bool().unwrap_or_default(),
            applied_plan.as_deref(),
        );
        let read = em.read_back(case, &transfer).await;
        assert_reports(
            case,
            &read,
            &transfer,
            retry["Enabled"].as_bool().unwrap_or_default(),
            applied_plan.as_deref(),
        );
    }
}

/// RULE: the preference is per BILLING TRANSFER, so a `ClientToken`'s replay is scoped to
/// the transfer the call it recorded addressed — the same token carried against another
/// transfer is a different request, recorded for THAT transfer, and the first transfer's
/// preference and `LastModifiedTime` are untouched by it.
#[tokio::test]
async fn a_client_token_replays_only_for_the_transfer_it_recorded() {
    let em = Emulator::new();
    let first_plan = em.create_plan("setup", "SharedTokenPlanOne").await;
    let second_plan = em.create_plan("setup", "SharedTokenPlanTwo").await;

    // The rows differ in the form of the `ClientToken` (the modelled `^[a-zA-Z0-9-]+$` at
    // its length minimum of 1, a hyphenated token, and one at the length maximum of 64)
    // and in the form of the two preferences the shared token carries: disabled, enabled
    // naming a plan by full ARN, enabled naming one by its bare id, and enabled naming a
    // plan Amazon Web Services owns.
    let rows: Vec<(&str, String, Value, Value)> = vec![
        (
            "a one-character token: disabled here, enabled by full ARN there",
            "b".to_string(),
            json!({ "Enabled": false }),
            json!({ "Enabled": true, "PricingPlanArn": &first_plan }),
        ),
        (
            "a hyphenated token: enabled by bare id here, by another plan's ARN there",
            "shared-token-0002".to_string(),
            json!({ "Enabled": true, "PricingPlanArn": bare_id(&first_plan) }),
            json!({ "Enabled": true, "PricingPlanArn": &second_plan }),
        ),
        (
            "a 64-character token: an AWS-managed plan here, disabled there",
            "S".repeat(64),
            json!({ "Enabled": true, "PricingPlanArn": AWS_MANAGED_PLAN_IDS[1] }),
            json!({ "Enabled": false }),
        ),
    ];

    for (index, (case, token, here, there)) in rows.iter().enumerate() {
        // Two transfers of this organization that differ in direction and in the length of
        // the transfer id, both inside the modelled pattern.
        let first = transfer_arn(&format!("tokenscope{index}"), "inbound", "aabbccdd");
        let second = transfer_arn(
            &format!("tokenscope{index}"),
            "outbound",
            "0123456789abcdef0123456789abcdef",
        );

        let here_plan = expected_plan(here);
        let there_plan = expected_plan(there);
        let here_enabled = here["Enabled"].as_bool().unwrap_or_default();
        let there_enabled = there["Enabled"].as_bool().unwrap_or_default();

        let recorded_here = em.set(case, &first, here, Some(token.as_str())).await;
        let stamped_here =
            assert_reports(case, &recorded_here, &first, here_enabled, here_plan.as_deref());

        // The SAME token against the other transfer: a different request, so it is
        // applied rather than answered with the first transfer's preference.
        let recorded_there = em.set(case, &second, there, Some(token.as_str())).await;
        assert_reports(
            case,
            &recorded_there,
            &second,
            there_enabled,
            there_plan.as_deref(),
        );
        let read_there = em.read_back(case, &second).await;
        assert_reports(
            case,
            &read_there,
            &second,
            there_enabled,
            there_plan.as_deref(),
        );

        // And the first transfer still holds exactly what its own call recorded.
        let read_here = em.read_back(case, &first).await;
        let read_stamp =
            assert_reports(case, &read_here, &first, here_enabled, here_plan.as_deref());
        assert_eq!(
            read_stamp, stamped_here,
            "case {case:?}: the other transfer's write must not move this one's \
             LastModifiedTime, got {read_here:?}"
        );

        // A retry on the transfer the token now records IS a retry: it answers what that
        // call recorded, even asked for the other preference, and moves nothing.
        let replayed = em.set(case, &second, here, Some(token.as_str())).await;
        assert_eq!(
            replayed, recorded_there,
            "case {case:?}: a retry on the recorded transfer must answer what it recorded, \
             got {replayed:?} after {recorded_there:?}"
        );
        let after = em.read_back(case, &second).await;
        assert_eq!(
            after, read_there,
            "case {case:?}: a retry must leave the recorded preference alone, got {after:?} \
             after {read_there:?}"
        );
    }
}
