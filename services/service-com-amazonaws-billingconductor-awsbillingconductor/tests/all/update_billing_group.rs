// SPDX-License-Identifier: CC0-1.0

//! `UpdateBillingGroup` end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE under test: `UpdateBillingGroup` resolves the billing group its `Arn` names
//! (the modelled `BillingGroupArn` pattern spells that either as the full ARN or as the
//! bare id, and both name ONE record), replaces exactly the members the request carries
//! (`Name`, `Description`, `ComputationPreference.PricingPlanArn`, `Status`,
//! `AccountGrouping.AutoAssociate`), keeps every member it omits — and everything the
//! operation does not model at all (the membership, the primary account, the tags, the
//! creation time) — at the value state already holds, and answers with the group's whole
//! state afterwards, including the `Size`, `Status` and `StatusReason` the service
//! derives from the record rather than storing.
//!
//! The refusals: an `Arn` no group of this account answers to is 404
//! `ResourceNotFoundException`; a `Name` another group holds is 409 `ConflictException`
//! (`RESOURCE_NAME_CONFLICT`); a `ComputationPreference` naming a pricing plan that does
//! not exist is 400 `ValidationException` (`MISSING_PRICINGPLAN`); a `Status` the group's
//! own condition does not admit is 400 (`INVALID_BILLING_GROUP_STATUS`); and a
//! `ResponsibilityTransferArn` other than the one the group already carries is 400,
//! because the model documents that member as "Modifications to the
//! ResponsibilityTransferArn are not permitted for existing billing groups". A refusal
//! writes nothing.
//!
//! Every table's rows differ along what the rule generalises over: which optional
//! members the request names (none, one, several, all), the member class they fall in (a
//! name under a pattern, free text including the empty string and the modelled maximum
//! length, an enum value, a boolean, an ARN), how the `Arn` is spelled, which class of
//! pricing plan the computation preference names (this account's, Amazon Web Services'
//! own, none at all), and which billing-owner class the addressed group falls in (a
//! primary account, a responsibility transfer, neither).
//!
//! Each row's result is read back out of state through the read-only `ListBillingGroups`
//! rather than taken from the response of the call under test, so an assertion passes
//! only if the change was persisted.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /update-billing-group` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="UpdateBillingGroup")`).
const UPDATE_PATH: &str = "/update-billing-group";

/// `POST /create-billing-group`, which gives this suite something to update.
const CREATE_GROUP_PATH: &str = "/create-billing-group";

/// `POST /create-pricing-plan` — a billing group's computation preference must name a
/// pricing plan that exists.
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// `POST /list-billing-groups` — the read-only reader every row verifies through.
const LIST_GROUPS_PATH: &str = "/list-billing-groups";

/// `POST /associate-accounts` — the only way a group's membership (and so its `Size`)
/// grows, which `UpdateBillingGroup` must leave alone.
const ASSOCIATE_PATH: &str = "/associate-accounts";

/// The pricing plan ids Amazon Web Services owns rather than the caller, enumerated by
/// the modelled `PricingPlanArn` pattern beside the customer-minted `[a-zA-Z0-9]{10}`
/// form (`smithy_query(query="shape",shape="PricingPlanArn")`). A billing group may
/// compute with one without the caller ever having created it.
const AWS_MANAGED_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// The modelled maximum length of `BillingGroupDescription` (len 0..1024).
const MAX_DESCRIPTION_LEN: usize = 1024;

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
    /// the world in which the update these tests are about is reachable at all. The
    /// denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`.
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

    /// `CreateBillingGroup`: the created group's full ARN.
    async fn create_group(&self, body: &Value) -> String {
        let (status, parsed) = self.post(CREATE_GROUP_PATH, body).await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {body}: expected 200, got {status} with {parsed}"
        );
        parsed["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `UpdateBillingGroup` against `arn`, carrying the members of `patch` beside it.
    async fn update_group(&self, arn: &str, patch: &Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        if let Some(members) = patch.as_object() {
            for (name, value) in members {
                body[name] = value.clone();
            }
        }
        self.post(UPDATE_PATH, &body).await
    }

    /// The `BillingGroupListElement` the read-only `ListBillingGroups` reports for one
    /// group — the independent view every row's assertions are made against.
    async fn listed(&self, arn: &str) -> Value {
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
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// `AssociateAccounts`, so a group under test holds a membership no update may
    /// touch.
    async fn associate(&self, arn: &str, account_ids: &[&str]) {
        let (status, body) = self
            .post(
                ASSOCIATE_PATH,
                &json!({ "Arn": arn, "AccountIds": account_ids }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup AssociateAccounts {arn}: expected 200, got {status} with {body}"
        );
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

/// The emulator's account id — the account every ARN this service mints carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The full ARN of an AWS-managed pricing plan: the `PricingPlanArn` pattern spells the
/// account segment of those as the literal `aws`.
fn managed_plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::aws:pricingplan/{id}")
}

/// The full `BillingGroupArn` of a group id held by this account.
fn group_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:billinggroup/{id}", account())
}

/// The bare-id form of a full ARN — the other spelling the modelled
/// `BillingGroupArn` pattern admits for the same resource.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// A `ResponsibilityTransferArn` of this account, under the member's own pattern
/// (`^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$`).
fn transfer_arn(suffix: &str) -> String {
    format!(
        "arn:aws:organizations::{}:transfer/o-abcd123456/billing/inbound/rt-{suffix}",
        account()
    )
}

/// Wall-clock millis since the epoch — the unit the modelled `Instant` long carries, so
/// a test can bracket the `LastModifiedTime` a call must report.
fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("the host clock is after the epoch")
            .as_millis(),
    )
    .expect("the epoch millis fit in an i64")
}

/// How one row spells the billing group ARN its request addresses: both spellings the
/// modelled `BillingGroupArn` pattern admits name the same record.
type ArnSpelling = fn(&str) -> String;

/// How one row builds the members it names beside `Arn`, from the ARN of the pricing
/// plan the group under test computes with.
type PatchBuilder = fn(&str) -> Value;

/// The request that creates the group the member table starts from: every member
/// `UpdateBillingGroup` can change carries a known starting value, and the group also
/// holds state no update may touch (a primary account, a membership and a tag).
fn base_group(name: &str, plan_arn: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the group as created",
        "PrimaryAccountId": "111122223333",
        "AccountGrouping": { "LinkedAccountIds": ["111122223333"], "AutoAssociate": false },
        "ComputationPreference": { "PricingPlanArn": plan_arn },
        "Tags": { "team": "billing" },
    })
}

/// One row of the member-replacement table: the members the request names beside `Arn`,
/// and the whole state the group must hold afterwards.
struct UpdateCase {
    label: &'static str,
    /// Built per row because some rows name an ARN only known at run time.
    patch: fn(&str, &str) -> Value,
    name: &'static str,
    description: Option<&'static str>,
    /// `None` means "the plan the group was created with".
    pricing_plan: Option<fn(&str, &str) -> String>,
    status: &'static str,
    auto_associate: bool,
}

#[tokio::test]
async fn update_replaces_the_members_it_names_and_keeps_the_rest() {
    let long_description: &'static str = Box::leak("d".repeat(MAX_DESCRIPTION_LEN).into_boxed_str());

    let cases = [
        UpdateCase {
            label: "no member named: the group is untouched",
            patch: |_, _| json!({}),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "Name only",
            patch: |_, _| json!({ "Name": "RenamedGroup" }),
            name: "RenamedGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "Name set to the name the group already holds",
            patch: |_, _| json!({ "Name": "BaseGroup" }),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "Description only",
            patch: |_, _| json!({ "Description": "a second description" }),
            name: "BaseGroup",
            description: Some("a second description"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "an empty Description is a value, not an omission",
            patch: |_, _| json!({ "Description": "" }),
            name: "BaseGroup",
            description: Some(""),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "a Description at the modelled maximum length",
            patch: |_, _| json!({ "Description": "d".repeat(MAX_DESCRIPTION_LEN) }),
            name: "BaseGroup",
            description: None, // asserted against `long_description` below
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "ComputationPreference moves the group to a second plan of this account",
            patch: |_, other_plan| json!({ "ComputationPreference": { "PricingPlanArn": other_plan } }),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: Some(|_, other_plan| other_plan.to_string()),
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "ComputationPreference moves the group to an Amazon Web Services managed plan",
            patch: |_, _| {
                json!({ "ComputationPreference": { "PricingPlanArn": managed_plan_arn(AWS_MANAGED_PLAN_IDS[0]) } })
            },
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: Some(|_, _| managed_plan_arn(AWS_MANAGED_PLAN_IDS[0])),
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "ComputationPreference moves the group to the other managed plan",
            patch: |_, _| {
                json!({ "ComputationPreference": { "PricingPlanArn": managed_plan_arn(AWS_MANAGED_PLAN_IDS[1]) } })
            },
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: Some(|_, _| managed_plan_arn(AWS_MANAGED_PLAN_IDS[1])),
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "Status PENDING",
            patch: |_, _| json!({ "Status": "PENDING" }),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "PENDING",
            auto_associate: false,
        },
        UpdateCase {
            label: "Status ACTIVE on a group that is already ACTIVE",
            patch: |_, _| json!({ "Status": "ACTIVE" }),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "AccountGrouping turns AutoAssociate on",
            patch: |_, _| json!({ "AccountGrouping": { "AutoAssociate": true } }),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: true,
        },
        UpdateCase {
            label: "AccountGrouping leaves AutoAssociate off",
            patch: |_, _| json!({ "AccountGrouping": { "AutoAssociate": false } }),
            name: "BaseGroup",
            description: Some("the group as created"),
            pricing_plan: None,
            status: "ACTIVE",
            auto_associate: false,
        },
        UpdateCase {
            label: "every member the operation models at once",
            patch: |_, other_plan| {
                json!({
                    "Name": "WholesaleRename",
                    "Description": "replaced wholesale",
                    "ComputationPreference": { "PricingPlanArn": other_plan },
                    "Status": "PENDING",
                    "AccountGrouping": { "AutoAssociate": true },
                })
            },
            name: "WholesaleRename",
            description: Some("replaced wholesale"),
            pricing_plan: Some(|_, other_plan| other_plan.to_string()),
            status: "PENDING",
            auto_associate: true,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // One emulator per row, so each row starts from the same base group and the
        // uniqueness index of one row's rename cannot reach the next.
        let em = Emulator::new();
        let plan = em.create_plan(&format!("UpdPlanA{index:03}")).await;
        let other_plan = em.create_plan(&format!("UpdPlanB{index:03}")).await;
        let arn = em.create_group(&base_group("BaseGroup", &plan)).await;
        // A second account joins, so `Size` is 2 and is this operation's to report but
        // never to change.
        em.associate(&arn, &["444455556666"]).await;

        let patch = (case.patch)(&plan, &other_plan);
        let (status, response) = em.update_group(&arn, &patch).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {response}",
            case.label
        );

        let expected_description = case.description.unwrap_or(long_description);
        let expected_plan = case
            .pricing_plan
            .map_or_else(|| plan.clone(), |pick| pick(&plan, &other_plan));

        // The response reports the group's whole state after the call ...
        assert_eq!(
            response["Arn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: expected the addressed ARN, got {response}",
            case.label
        );
        assert_eq!(
            response["Name"].as_str(),
            Some(case.name),
            "case {:?}: expected Name {:?}, got {response}",
            case.label,
            case.name
        );
        assert_eq!(
            response["Description"].as_str(),
            Some(expected_description),
            "case {:?}: expected a {}-byte Description, got {:?}",
            case.label,
            expected_description.len(),
            response["Description"].as_str().map(str::len)
        );
        assert_eq!(
            response["PricingPlanArn"].as_str(),
            Some(expected_plan.as_str()),
            "case {:?}: expected PricingPlanArn {expected_plan:?}, got {response}",
            case.label
        );
        assert_eq!(
            response["Status"].as_str(),
            Some(case.status),
            "case {:?}: expected Status {:?}, got {response}",
            case.label,
            case.status
        );
        assert_eq!(
            response["Size"].as_i64(),
            Some(2),
            "case {:?}: expected the membership the update never touches, got {response}",
            case.label
        );
        assert_eq!(
            response["PrimaryAccountId"].as_str(),
            Some("111122223333"),
            "case {:?}: expected the primary account the update never touches, got {response}",
            case.label
        );
        assert_eq!(
            response["AccountGrouping"]["AutoAssociate"].as_bool(),
            Some(case.auto_associate),
            "case {:?}: expected AutoAssociate {}, got {response}",
            case.label,
            case.auto_associate
        );

        // ... and so does state, read back through the read-only lister.
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["Name"].as_str(),
            Some(case.name),
            "case {:?}: ListBillingGroups must report the persisted Name, got {listed}",
            case.label
        );
        assert_eq!(
            listed["Description"].as_str(),
            Some(expected_description),
            "case {:?}: ListBillingGroups must report the persisted Description, got {:?}",
            case.label,
            listed["Description"].as_str().map(str::len)
        );
        assert_eq!(
            listed["ComputationPreference"]["PricingPlanArn"].as_str(),
            Some(expected_plan.as_str()),
            "case {:?}: ListBillingGroups must report the persisted plan, got {listed}",
            case.label
        );
        assert_eq!(
            listed["Status"].as_str(),
            Some(case.status),
            "case {:?}: ListBillingGroups must report the persisted Status, got {listed}",
            case.label
        );
        assert_eq!(
            listed["AccountGrouping"]["AutoAssociate"].as_bool(),
            Some(case.auto_associate),
            "case {:?}: ListBillingGroups must report the persisted AutoAssociate, got {listed}",
            case.label
        );
        assert_eq!(
            listed["Size"].as_i64(),
            Some(2),
            "case {:?}: the membership is not this operation's to change, got {listed}",
            case.label
        );
        assert_eq!(
            listed["BillingGroupType"].as_str(),
            Some("STANDARD"),
            "case {:?}: a group with no transfer relationship stays STANDARD, got {listed}",
            case.label
        );
        // A `PENDING` group says why it is not billing; an `ACTIVE` one has nothing to
        // explain.
        assert_eq!(
            listed["StatusReason"].is_string(),
            case.status != "ACTIVE",
            "case {:?}: StatusReason must accompany every status but ACTIVE, got {listed}",
            case.label
        );
    }
}

#[tokio::test]
async fn both_arn_spellings_address_the_one_record() {
    // The two spellings the modelled `BillingGroupArn` pattern admits for one resource.
    let spellings: [(&str, ArnSpelling); 2] = [
        ("the full ARN", |arn| arn.to_string()),
        ("the bare resource id", bare_id),
    ];

    for (form, spell) in spellings {
        let em = Emulator::new();
        let plan = em.create_plan("SpellingPlan").await;
        let arn = em.create_group(&base_group("SpellingGroup", &plan)).await;

        let (status, response) = em
            .update_group(
                &spell(&arn),
                &json!({ "Description": "addressed by one spelling" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {form:?}: expected 200, got {status} with {response}"
        );
        assert_eq!(
            response["Arn"].as_str(),
            Some(arn.as_str()),
            "case {form:?}: both spellings name the full ARN record, got {response}"
        );
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["Description"].as_str(),
            Some("addressed by one spelling"),
            "case {form:?}: the update must reach the one stored record, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_arn_no_group_answers_to_is_not_found() {
    let em = Emulator::new();
    let plan = em.create_plan("NotFoundPlan").await;
    let live = em.create_group(&base_group("LiveGroup", &plan)).await;

    // Rows differ along how the absent group is spelled, and the id's length within the
    // `[a-zA-Z0-9]{10,12}` the pattern admits.
    let cases = [
        ("a bare id of 10 characters", "BOGUS00001".to_string()),
        ("a bare id of 12 characters", "BOGUS0000123".to_string()),
        ("this account's full ARN", group_arn("BOGUS00002")),
        (
            "another account's full ARN",
            "arn:aws:billingconductor::111122223333:billinggroup/BOGUS00003".to_string(),
        ),
    ];

    for (case, identifier) in cases {
        let (status, response) = em
            .update_group(&identifier, &json!({ "Description": "never applied" }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
    }

    // The refusals wrote nothing: the live group is as it was created.
    let listed = em.listed(&live).await;
    assert_eq!(
        listed["Description"].as_str(),
        Some("the group as created"),
        "a not-found refusal must write nothing, got {listed}"
    );
}

#[tokio::test]
async fn a_name_another_group_holds_is_a_conflict_and_writes_nothing() {
    let em = Emulator::new();
    let plan = em.create_plan("ConflictPlan").await;
    let first = em.create_group(&base_group("FirstGroup", &plan)).await;
    let second = em.create_group(&base_group2("SecondGroup", &plan)).await;

    // Rows differ along which group is addressed and which group's name is asked for.
    let cases = [
        ("the first group asks for the second's name", &first, "SecondGroup"),
        ("the second group asks for the first's name", &second, "FirstGroup"),
    ];

    for (case, arn, name) in cases {
        let (status, response) = em
            .update_group(arn, &json!({ "Name": name, "Description": "never applied" }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response}"
        );
        assert_eq!(
            response["Reason"].as_str(),
            Some("RESOURCE_NAME_CONFLICT"),
            "case {case:?}: expected the name-conflict reason, got {response}"
        );
        // The whole request was refused, including the member that was free to change.
        let listed = em.listed(arn).await;
        assert_eq!(
            listed["Description"].as_str(),
            Some("the group as created"),
            "case {case:?}: a conflict must write nothing, got {listed}"
        );
    }

    // And the names are still each group's own, so the index was not disturbed.
    for (arn, name) in [(&first, "FirstGroup"), (&second, "SecondGroup")] {
        let listed = em.listed(arn).await;
        assert_eq!(
            listed["Name"].as_str(),
            Some(name),
            "case {name:?}: the uniqueness index must still hold the original name, got {listed}"
        );
    }

    // A rename frees the old name for another group: the name index follows the rename
    // rather than keeping both spellings.
    let (status, response) = em
        .update_group(&first, &json!({ "Name": "ThirdGroup" }))
        .await;
    assert_eq!(
        status, 200,
        "renaming to a free name: expected 200, got {status} with {response}"
    );
    let (status, response) = em
        .update_group(&second, &json!({ "Name": "FirstGroup" }))
        .await;
    assert_eq!(
        status, 200,
        "taking the freed name: expected 200, got {status} with {response}"
    );
}

/// A second base group: same shape, a primary account of its own (an account belongs to
/// at most one billing group).
fn base_group2(name: &str, plan_arn: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the group as created",
        "PrimaryAccountId": "777788889999",
        "AccountGrouping": { "LinkedAccountIds": ["777788889999"] },
        "ComputationPreference": { "PricingPlanArn": plan_arn },
    })
}

#[tokio::test]
async fn a_computation_preference_naming_no_plan_is_refused_and_writes_nothing() {
    let em = Emulator::new();
    let plan = em.create_plan("PreferencePlan").await;
    let arn = em.create_group(&base_group("PreferenceGroup", &plan)).await;

    // Rows differ along which class of nonexistent plan the preference names: an id
    // nobody created, and a conforming plan ARN of another account.
    let cases = [
        (
            "a plan id nobody created",
            format!("arn:aws:billingconductor::{}:pricingplan/MISSING001", account()),
        ),
        (
            "another account's plan",
            "arn:aws:billingconductor::111122223333:pricingplan/OTHERACCT1".to_string(),
        ),
    ];

    for (case, plan_arn) in cases {
        let (status, response) = em
            .update_group(
                &arn,
                &json!({
                    "Description": "never applied",
                    "ComputationPreference": { "PricingPlanArn": plan_arn },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert_eq!(
            response["Reason"].as_str(),
            Some("MISSING_PRICINGPLAN"),
            "case {case:?}: expected the missing-plan reason, got {response}"
        );
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["Description"].as_str(),
            Some("the group as created"),
            "case {case:?}: a refusal must write nothing, got {listed}"
        );
        assert_eq!(
            listed["ComputationPreference"]["PricingPlanArn"].as_str(),
            Some(plan.as_str()),
            "case {case:?}: the group must still compute with its created plan, got {listed}"
        );
    }
}

/// One row of the status table: the billing-owner class of the group, the `Status` the
/// request names, and whether the group may hold it.
struct StatusCase {
    label: &'static str,
    /// The `CreateBillingGroup` members that give the group its billing owner.
    owner: fn() -> Value,
    requested: &'static str,
    /// `Some(status)` when the call is served and the group then holds `status`;
    /// `None` when the request is refused.
    settled: Option<&'static str>,
}

#[tokio::test]
async fn status_is_settable_only_among_the_statuses_the_group_can_hold() {
    // The billing-owner classes: a primary account, a responsibility transfer
    // relationship ("When specified, the PrimaryAccountId is no longer required"), and
    // neither.
    fn primary_account() -> Value {
        json!({ "PrimaryAccountId": "111122223333" })
    }
    fn transfer_relationship() -> Value {
        json!({ "AccountGrouping": { "ResponsibilityTransferArn": transfer_arn("1234abcd") } })
    }
    fn no_owner() -> Value {
        json!({})
    }

    // Every value of the modelled `BillingGroupStatus` enum
    // (`smithy_query(query="shape",shape="BillingGroupStatus")`) against every owner
    // class.
    let cases = [
        StatusCase {
            label: "a group with a primary account may be made PENDING",
            owner: primary_account,
            requested: "PENDING",
            settled: Some("PENDING"),
        },
        StatusCase {
            label: "a group with a primary account may be made ACTIVE",
            owner: primary_account,
            requested: "ACTIVE",
            settled: Some("ACTIVE"),
        },
        StatusCase {
            label: "PRIMARY_ACCOUNT_MISSING is not a caller's to set",
            owner: primary_account,
            requested: "PRIMARY_ACCOUNT_MISSING",
            settled: None,
        },
        StatusCase {
            label: "a transfer-billing group may be made PENDING",
            owner: transfer_relationship,
            requested: "PENDING",
            settled: Some("PENDING"),
        },
        StatusCase {
            label: "a transfer-billing group may be made ACTIVE",
            owner: transfer_relationship,
            requested: "ACTIVE",
            settled: Some("ACTIVE"),
        },
        StatusCase {
            label: "a group with no billing owner cannot be made ACTIVE",
            owner: no_owner,
            requested: "ACTIVE",
            settled: None,
        },
        StatusCase {
            label: "a group with no billing owner cannot be made PENDING",
            owner: no_owner,
            requested: "PENDING",
            settled: None,
        },
        StatusCase {
            label: "a group with no billing owner cannot be told it is missing one",
            owner: no_owner,
            requested: "PRIMARY_ACCOUNT_MISSING",
            settled: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let em = Emulator::new();
        let plan = em.create_plan(&format!("StatusPlan{index:03}")).await;
        let mut body = json!({
            "Name": format!("StatusGroup{index:03}"),
            "AccountGrouping": {},
            "ComputationPreference": { "PricingPlanArn": plan },
        });
        for (member, value) in (case.owner)().as_object().into_iter().flatten() {
            body[member] = value.clone();
        }
        let arn = em.create_group(&body).await;
        let before = em.listed(&arn).await;

        let (status, response) = em
            .update_group(&arn, &json!({ "Status": case.requested }))
            .await;

        match case.settled {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected 200, got {status} with {response}",
                    case.label
                );
                assert_eq!(
                    response["Status"].as_str(),
                    Some(expected),
                    "case {:?}: expected Status {expected:?}, got {response}",
                    case.label
                );
                let listed = em.listed(&arn).await;
                assert_eq!(
                    listed["Status"].as_str(),
                    Some(expected),
                    "case {:?}: ListBillingGroups must report the persisted Status, got {listed}",
                    case.label
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected 400, got {status} with {response}",
                    case.label
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {:?}: expected ValidationException, got {response}",
                    case.label
                );
                assert_eq!(
                    response["Reason"].as_str(),
                    Some("INVALID_BILLING_GROUP_STATUS"),
                    "case {:?}: expected the invalid-status reason, got {response}",
                    case.label
                );
                let listed = em.listed(&arn).await;
                assert_eq!(
                    listed["Status"].as_str(),
                    before["Status"].as_str(),
                    "case {:?}: a refusal must leave the status where it was, got {listed}",
                    case.label
                );
            }
        }
    }
}

#[tokio::test]
async fn a_status_round_trips_and_a_group_losing_its_owner_reports_the_absence() {
    let em = Emulator::new();
    let plan = em.create_plan("RoundTripPlan").await;
    let arn = em.create_group(&base_group("RoundTripGroup", &plan)).await;

    // The invariant: a status set is a status reported, and setting it back restores the
    // earlier reading — over the whole settable set, twice around.
    for expected in ["PENDING", "ACTIVE", "PENDING", "ACTIVE"] {
        let (status, response) = em.update_group(&arn, &json!({ "Status": expected })).await;
        assert_eq!(
            status, 200,
            "case {expected:?}: expected 200, got {status} with {response}"
        );
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["Status"].as_str(),
            Some(expected),
            "case {expected:?}: the status must round-trip through state, got {listed}"
        );
    }

    // A group with no billing owner reports PRIMARY_ACCOUNT_MISSING whatever was asked
    // for, because that status reports the absence rather than a request.
    let ownerless = em
        .create_group(&json!({
            "Name": "OwnerlessGroup",
            "AccountGrouping": {},
            "ComputationPreference": { "PricingPlanArn": plan },
        }))
        .await;
    let (status, response) = em
        .update_group(&ownerless, &json!({ "Description": "no owner here" }))
        .await;
    assert_eq!(
        status, 200,
        "an ownerless group still takes other members: expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["Status"].as_str(),
        Some("PRIMARY_ACCOUNT_MISSING"),
        "an ownerless group reports the absence of a billing owner, got {response}"
    );
    assert!(
        response["StatusReason"].is_string(),
        "an ownerless group says why it is in that status, got {response}"
    );
}

#[tokio::test]
async fn the_responsibility_transfer_arn_of_an_existing_group_cannot_be_modified() {
    let em = Emulator::new();
    let plan = em.create_plan("TransferPlan").await;
    let standard = em.create_group(&base_group("StandardGroup", &plan)).await;
    let transfer = em
        .create_group(&json!({
            "Name": "TransferGroup",
            "Description": "the group as created",
            "AccountGrouping": { "ResponsibilityTransferArn": transfer_arn("1234abcd") },
            "ComputationPreference": { "PricingPlanArn": plan },
        }))
        .await;

    // Rows differ along the group's own transfer relationship (none, one) and the value
    // the request names (a new one, a different one, the one already held).
    let refused = [
        (
            "a standard group is given a transfer relationship",
            &standard,
            transfer_arn("1234abcd"),
        ),
        (
            "a transfer group is moved to another relationship",
            &transfer,
            transfer_arn("9999zzzz"),
        ),
    ];

    for (case, arn, requested) in refused {
        let (status, response) = em
            .update_group(
                arn,
                &json!({
                    "Description": "never applied",
                    "AccountGrouping": { "ResponsibilityTransferArn": requested },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        let listed = em.listed(arn).await;
        assert_eq!(
            listed["Description"].as_str(),
            Some("the group as created"),
            "case {case:?}: a refusal must write nothing, got {listed}"
        );
    }

    // Naming the value the group already holds modifies nothing, so it is served — and
    // so is an `AccountGrouping` that names only the member this operation may change.
    let served: [(&str, &String, Value); 2] = [
        (
            "a transfer group restates the relationship it holds",
            &transfer,
            json!({ "AccountGrouping": {
                "ResponsibilityTransferArn": transfer_arn("1234abcd"),
                "AutoAssociate": true,
            }}),
        ),
        (
            "a standard group names only AutoAssociate",
            &standard,
            json!({ "AccountGrouping": { "AutoAssociate": true } }),
        ),
    ];

    for (case, arn, patch) in served {
        let (status, response) = em.update_group(arn, &patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response}"
        );
        let listed = em.listed(arn).await;
        assert_eq!(
            listed["AccountGrouping"]["AutoAssociate"].as_bool(),
            Some(true),
            "case {case:?}: the member the operation may change must be persisted, got {listed}"
        );
    }

    // The relationships themselves are untouched, and so are the types derived from
    // them.
    let listed = em.listed(&transfer).await;
    assert_eq!(
        listed["AccountGrouping"]["ResponsibilityTransferArn"].as_str(),
        Some(transfer_arn("1234abcd").as_str()),
        "the transfer group keeps the relationship it was created with, got {listed}"
    );
    assert_eq!(
        listed["BillingGroupType"].as_str(),
        Some("TRANSFER_BILLING"),
        "the transfer group keeps its derived type, got {listed}"
    );
    let listed = em.listed(&standard).await;
    assert!(
        listed["AccountGrouping"]["ResponsibilityTransferArn"].is_null(),
        "the standard group gained no relationship, got {listed}"
    );
    assert_eq!(
        listed["BillingGroupType"].as_str(),
        Some("STANDARD"),
        "the standard group keeps its derived type, got {listed}"
    );
}

#[tokio::test]
async fn last_modified_time_moves_only_when_the_group_changed() {
    // Rows differ along whether the request asks for anything the group does not
    // already hold: no member at all, the values it holds, and each kind of real change.
    let cases: [(&str, PatchBuilder, bool); 7] = [
        ("no member named", |_| json!({}), false),
        (
            "the Name it already holds",
            |_| json!({ "Name": "ClockGroup" }),
            false,
        ),
        (
            "the Description it already holds",
            |_| json!({ "Description": "the group as created" }),
            false,
        ),
        (
            "the plan it already computes with",
            |plan| json!({ "ComputationPreference": { "PricingPlanArn": plan } }),
            false,
        ),
        (
            "a new Description",
            |_| json!({ "Description": "moved on" }),
            true,
        ),
        ("a new Name", |_| json!({ "Name": "ClockRenamed" }), true),
        ("a new Status", |_| json!({ "Status": "PENDING" }), true),
    ];

    for (index, (case, patch, moves)) in cases.into_iter().enumerate() {
        let em = Emulator::new();
        let plan = em.create_plan(&format!("ClockPlan{index:03}")).await;
        let arn = em.create_group(&base_group("ClockGroup", &plan)).await;
        let created = em.listed(&arn).await;
        let creation_time = created["CreationTime"].as_i64().unwrap_or_default();
        let modified_before = created["LastModifiedTime"].as_i64().unwrap_or_default();
        // The clock this emulator reads has millisecond resolution, so a change is
        // observable as "not earlier than the instant before the call".
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let before_call = now_millis();

        let (status, response) = em.update_group(&arn, &patch(&plan)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response}"
        );
        let after_call = now_millis();
        let reported = response["LastModifiedTime"].as_i64().unwrap_or_default();
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["LastModifiedTime"].as_i64(),
            Some(reported),
            "case {case:?}: the response and state must agree, got {listed}"
        );

        if moves {
            assert!(
                reported >= before_call && reported <= after_call,
                "case {case:?}: expected a LastModifiedTime within [{before_call}, {after_call}], \
                 got {reported}"
            );
        } else {
            assert_eq!(
                reported, modified_before,
                "case {case:?}: a call that modified nothing must leave LastModifiedTime where it \
                 was ({modified_before}), got {reported}"
            );
        }
        assert_eq!(
            listed["CreationTime"].as_i64(),
            Some(creation_time),
            "case {case:?}: the creation time is never an update's to change, got {listed}"
        );
    }
}
