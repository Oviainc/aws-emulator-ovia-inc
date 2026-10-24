// SPDX-License-Identifier: CC0-1.0

//! The cap on automatic account association, end to end over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: `AccountGrouping.AutoAssociate` elects the ONE billing group of a
//! consolidated billing family that "will automatically associate newly added Amazon Web
//! Services accounts that join your consolidated billing family" (the member's own
//! documentation), so at most one stored group may have it enabled at a time: any
//! request that would leave a second group with it enabled is refused 400
//! `ValidationException` with the reason the model enumerates for exactly this case,
//! `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`
//! (`smithy_query(query="shape",shape="ValidationExceptionReason")`), and writes nothing.
//!
//! The cap is on STATE, not on one operation, so the rows below span every way the flag
//! can be turned on (`CreateBillingGroup`, `UpdateBillingGroup`), the shape of the group
//! that asks for it (no accounts, several accounts, a primary account, a responsibility
//! transfer relationship, a description), and the handover: once the holder turns the
//! flag off, the next group may take the election on an update or at create time. A
//! request that leaves the enabled count where it is (the holder restating the flag, a
//! group asking for `false` or naming no flag at all) is always served.
//!
//! Every assertion reads the result back out of state through the read-only
//! `ListBillingGroups` — including its own `AutoAssociate` filter, which may therefore
//! never report more groups than the cap allows.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /create-billing-group` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="CreateBillingGroup")`).
const CREATE_GROUP_PATH: &str = "/create-billing-group";

/// `POST /update-billing-group`: the other operation that can turn the flag on.
const UPDATE_GROUP_PATH: &str = "/update-billing-group";

/// `POST /create-pricing-plan` — a billing group's computation preference must name a
/// pricing plan that exists.
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// `POST /list-billing-groups` — the read-only reader every row verifies through.
const LIST_GROUPS_PATH: &str = "/list-billing-groups";

/// The `ValidationExceptionReason` the model enumerates for a request that would take
/// the family past the cap.
const TOO_MANY: &str = "TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS";

/// A live emulator holding only this service, with the emulated account designated the
/// organization's PAYER account — Billing Conductor's control plane is authorized for
/// the payer account alone, so this is the world in which these calls are reachable.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
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

    /// `CreatePricingPlan`: the created plan's full ARN.
    async fn create_plan(&self, name: &str) -> String {
        let (status, body) = self.post(CREATE_PLAN_PATH, &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name}: expected 200, got {status} with {body}"
        );
        body["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `CreateBillingGroup` exactly as a row spells it: status and body, unasserted.
    async fn create(&self, body: &Value) -> (u16, Value) {
        self.post(CREATE_GROUP_PATH, body).await
    }

    /// `CreateBillingGroup` that must succeed: the created group's full ARN.
    async fn created(&self, body: &Value) -> String {
        let (status, parsed) = self.create(body).await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {body}: expected 200, got {status} with {parsed}"
        );
        parsed["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `UpdateBillingGroup` against `arn`, carrying the members of `patch` beside it.
    async fn update(&self, arn: &str, patch: &Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        if let Some(members) = patch.as_object() {
            for (name, value) in members {
                body[name] = value.clone();
            }
        }
        self.post(UPDATE_GROUP_PATH, &body).await
    }

    /// Ask one group to enable automatic account association, naming no other member.
    async fn enable(&self, arn: &str) -> (u16, Value) {
        self.update(arn, &json!({ "AccountGrouping": { "AutoAssociate": true } }))
            .await
    }

    /// The ARNs state holds with automatic account association enabled, read through
    /// `ListBillingGroups`'s own `AutoAssociate` filter.
    async fn enabled_groups(&self) -> Vec<String> {
        let (status, body) = self
            .post(LIST_GROUPS_PATH, &json!({"Filters": {"AutoAssociate": true}}))
            .await;
        assert_eq!(
            status, 200,
            "ListBillingGroups(AutoAssociate=true): expected 200, got {status} with {body}"
        );
        body["BillingGroups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| group["Arn"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The names state holds a billing group under, whatever their flag.
    async fn all_names(&self) -> Vec<String> {
        let (status, body) = self.post(LIST_GROUPS_PATH, &json!({})).await;
        assert_eq!(
            status, 200,
            "ListBillingGroups(no filter): expected 200, got {status} with {body}"
        );
        body["BillingGroups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| group["Name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `AutoAssociate` one group reads back with, through the read-only lister.
    async fn listed_flag(&self, arn: &str) -> Option<bool> {
        let (status, body) = self
            .post(LIST_GROUPS_PATH, &json!({"Filters": {"Arns": [arn]}}))
            .await;
        assert_eq!(
            status, 200,
            "ListBillingGroups {arn}: expected 200, got {status} with {body}"
        );
        body["BillingGroups"]
            .as_array()
            .and_then(|groups| groups.first())
            .and_then(|group| group["AccountGrouping"]["AutoAssociate"].as_bool())
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

/// A `ResponsibilityTransferArn` of this account, under the member's own pattern
/// (`smithy_query(query="shape",shape="AccountGrouping")`:
/// `^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$`).
fn transfer_arn(suffix: &str) -> String {
    format!(
        "arn:aws:organizations::{}:transfer/o-abcd123456/billing/inbound/rt-{suffix}",
        ::engine::emulator_account_id()
    )
}

/// The group that holds the election in each test below: a plain group created with the
/// flag on.
async fn elected_holder(em: &Emulator, plan: &str) -> String {
    em.created(&json!({
        "Name": "ElectedGroup",
        "AccountGrouping": { "LinkedAccountIds": [], "AutoAssociate": true },
        "ComputationPreference": { "PricingPlanArn": plan },
        "PrimaryAccountId": "111111111111",
    }))
    .await
}

/// RULE: while one group holds the election, a `CreateBillingGroup` asking for
/// `AutoAssociate` is refused 400 `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS` and creates
/// nothing — whatever the rest of the request looks like.
#[tokio::test]
async fn a_second_create_asking_for_auto_associate_is_refused() {
    let em = Emulator::new();
    let plan = em.create_plan("AutoAssociateCapPlan").await;
    let holder = elected_holder(&em, &plan).await;

    // Rows differ along the shape of the group that asks for the election: its
    // membership size, its billing-owner class (a primary account, a responsibility
    // transfer relationship, neither), and the optional members it carries.
    let refused: Vec<(&str, Value)> = vec![
        (
            "no accounts and a primary account",
            json!({
                "Name": "Auto2",
                "AccountGrouping": { "LinkedAccountIds": [], "AutoAssociate": true },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "222222222222",
            }),
        ),
        (
            "three linked accounts",
            json!({
                "Name": "Auto3",
                "AccountGrouping": {
                    "LinkedAccountIds": ["333344445555", "444455556666", "555566667777"],
                    "AutoAssociate": true,
                },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "333344445555",
            }),
        ),
        (
            "no billing owner at all",
            json!({
                "Name": "Auto4",
                "AccountGrouping": { "AutoAssociate": true },
                "ComputationPreference": { "PricingPlanArn": plan },
            }),
        ),
        (
            "a transfer-billing group with a description and tags",
            json!({
                "Name": "Auto5",
                "Description": "the group that wanted the election",
                "AccountGrouping": {
                    "LinkedAccountIds": ["666677778888"],
                    "AutoAssociate": true,
                    "ResponsibilityTransferArn": transfer_arn("1234abcd"),
                },
                "ComputationPreference": { "PricingPlanArn": plan },
                "Tags": { "team": "billing" },
            }),
        ),
        (
            "a request carrying a client token",
            json!({
                "Name": "Auto6",
                "ClientToken": "auto-associate-cap-token",
                "AccountGrouping": { "LinkedAccountIds": [], "AutoAssociate": true },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "777777777777",
            }),
        ),
    ];

    for (case, body) in refused {
        let (status, response) = em.create(&body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert_eq!(
            response["Reason"].as_str(),
            Some(TOO_MANY),
            "case {case:?}: expected Reason {TOO_MANY}, got {response}"
        );
        assert_eq!(
            response["Fields"][0]["Name"].as_str(),
            Some("AccountGrouping.AutoAssociate"),
            "case {case:?}: the refusal must name the member at fault, got {response}"
        );

        // The refusal wrote nothing: no group under that name, and the election is
        // still exactly the one group that held it.
        let name = body["Name"].as_str().unwrap_or_default().to_string();
        let names = em.all_names().await;
        assert!(
            !names.contains(&name),
            "case {case:?}: a refused create must persist no group, got {names:?}"
        );
        let enabled = em.enabled_groups().await;
        assert_eq!(
            enabled,
            vec![holder.clone()],
            "case {case:?}: the election must still be held by exactly the first group, got {enabled:?}"
        );
    }
}

/// RULE: a create that does not ask for the election is served while the election is
/// held — the cap bounds only the groups that have the flag on.
#[tokio::test]
async fn a_create_that_does_not_ask_for_auto_associate_is_served() {
    let em = Emulator::new();
    let plan = em.create_plan("AutoAssociateFreePlan").await;
    let holder = elected_holder(&em, &plan).await;

    // Rows differ along how the request declines the election: explicitly false, the
    // member omitted from an `AccountGrouping` that carries other members, and an
    // `AccountGrouping` that carries nothing at all.
    let served: Vec<(&str, Value, Option<bool>)> = vec![
        (
            "AutoAssociate false",
            json!({
                "Name": "Plain1",
                "AccountGrouping": { "LinkedAccountIds": ["888899990000"], "AutoAssociate": false },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "888899990000",
            }),
            Some(false),
        ),
        (
            "AutoAssociate omitted beside a membership",
            json!({
                "Name": "Plain2",
                "AccountGrouping": { "LinkedAccountIds": ["999900001111"] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "999900001111",
            }),
            None,
        ),
        (
            "an empty AccountGrouping",
            json!({
                "Name": "Plain3",
                "AccountGrouping": {},
                "ComputationPreference": { "PricingPlanArn": plan },
            }),
            None,
        ),
    ];

    for (case, body, expected) in served {
        let (status, response) = em.create(&body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
        let arn = response["Arn"].as_str().unwrap_or_default().to_string();
        let flag = em.listed_flag(&arn).await;
        assert_eq!(
            flag, expected,
            "case {case:?}: expected the persisted AutoAssociate {expected:?}, got {flag:?}"
        );
        let enabled = em.enabled_groups().await;
        assert_eq!(
            enabled,
            vec![holder.clone()],
            "case {case:?}: a group that declined the election must not join it, got {enabled:?}"
        );
    }
}

/// RULE: the cap is on STATE, so `UpdateBillingGroup` cannot elect a second group
/// either; the group that already holds the election may restate it.
#[tokio::test]
async fn an_update_cannot_elect_a_second_group() {
    let em = Emulator::new();
    let plan = em.create_plan("AutoAssociateUpdatePlan").await;
    let holder = elected_holder(&em, &plan).await;
    let other = em
        .created(&json!({
            "Name": "Challenger",
            "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
            "ComputationPreference": { "PricingPlanArn": plan },
            "PrimaryAccountId": "111122223333",
        }))
        .await;

    let (status, response) = em.enable(&other).await;
    assert_eq!(
        status, 400,
        "case \"update elects a second group\": expected HTTP 400, got {status} with {response}"
    );
    assert_eq!(
        response["Reason"].as_str(),
        Some(TOO_MANY),
        "case \"update elects a second group\": expected Reason {TOO_MANY}, got {response}"
    );
    let flag = em.listed_flag(&other).await;
    assert_ne!(
        flag,
        Some(true),
        "case \"update elects a second group\": a refusal must write nothing, got {flag:?}"
    );
    let enabled = em.enabled_groups().await;
    assert_eq!(
        enabled,
        vec![holder.clone()],
        "case \"update elects a second group\": the enabled set must be unchanged, got {enabled:?}"
    );

    // The holder restating what it already has changes the enabled count by nothing, so
    // it is served rather than refused by its own election.
    let (status, response) = em.enable(&holder).await;
    assert_eq!(
        status, 200,
        "case \"the holder restates the election\": expected HTTP 200, got {status} with {response}"
    );
    let enabled = em.enabled_groups().await;
    assert_eq!(
        enabled,
        vec![holder.clone()],
        "case \"the holder restates the election\": the enabled set must be unchanged, got {enabled:?}"
    );
}

/// RULE: the cap bounds the enabled groups, not the sequence of calls — once the
/// holder hands the election back by turning the flag off, the next group may take it,
/// whether it asks on an update or at create time.
#[tokio::test]
async fn handing_the_election_back_frees_it_for_the_next_group() {
    // Case 1: the holder turns the flag off and a stored group takes the election.
    let em = Emulator::new();
    let plan = em.create_plan("AutoAssociateHandoverPlan").await;
    let holder = elected_holder(&em, &plan).await;
    let other = em
        .created(&json!({
            "Name": "Successor",
            "AccountGrouping": { "LinkedAccountIds": [] },
            "ComputationPreference": { "PricingPlanArn": plan },
            "PrimaryAccountId": "222233334444",
        }))
        .await;

    let (status, response) = em
        .update(
            &holder,
            &json!({ "AccountGrouping": { "AutoAssociate": false } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the holder turns the flag off\": expected HTTP 200, got {status} with {response}"
    );
    let enabled = em.enabled_groups().await;
    assert!(
        enabled.is_empty(),
        "case \"the holder turns the flag off\": the election must be free, got {enabled:?}"
    );

    let (status, response) = em.enable(&other).await;
    assert_eq!(
        status, 200,
        "case \"the successor takes the freed election\": expected HTTP 200, got {status} with {response}"
    );
    let enabled = em.enabled_groups().await;
    assert_eq!(
        enabled,
        vec![other.clone()],
        "case \"the successor takes the freed election\": expected exactly the successor, got {enabled:?}"
    );

    // And the group that handed it back cannot take it straight back while the
    // successor holds it.
    let (status, response) = em.enable(&holder).await;
    assert_eq!(
        status, 400,
        "case \"the former holder asks again\": expected HTTP 400, got {status} with {response}"
    );
    assert_eq!(
        response["Reason"].as_str(),
        Some(TOO_MANY),
        "case \"the former holder asks again\": expected Reason {TOO_MANY}, got {response}"
    );

    // Case 2: the election is freed the same way, and the next group takes it at CREATE
    // time rather than on an update.
    let em = Emulator::new();
    let plan = em.create_plan("AutoAssociateCreateAfterPlan").await;
    let holder = elected_holder(&em, &plan).await;
    let (status, response) = em
        .update(
            &holder,
            &json!({ "AccountGrouping": { "AutoAssociate": false } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the holder frees the election again\": expected HTTP 200, got {status} with {response}"
    );
    let enabled = em.enabled_groups().await;
    assert!(
        enabled.is_empty(),
        "case \"the holder frees the election again\": the election must be free, got {enabled:?}"
    );

    let fresh = em
        .created(&json!({
            "Name": "CreatedAfterHandover",
            "AccountGrouping": { "LinkedAccountIds": [], "AutoAssociate": true },
            "ComputationPreference": { "PricingPlanArn": plan },
            "PrimaryAccountId": "555566667777",
        }))
        .await;
    let enabled = em.enabled_groups().await;
    assert_eq!(
        enabled,
        vec![fresh],
        "case \"a create after the handover\": expected exactly the new group, got {enabled:?}"
    );
}
