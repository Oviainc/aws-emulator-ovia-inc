// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the pricing-plan + billing-group core loop, driven over the
//! service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the
//! same path the production router serves.
//!
//! Every test asserts a RULE over a set of inputs rather than one recorded call:
//! the rows of each table differ along what the rule generalises over (how many
//! accounts, which optional members, which filter, which page size), and the values
//! the assertions check come back out of state through a read operation.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

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
    /// records no payer account by default — so this harness designates one, which is the
    /// world in which the core loop these tests are about is reachable at all. The
    /// denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`,
    /// over the default store.
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

    /// POST a JSON body to one of this service's routes and return the status plus
    /// the parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.post_with_token(path, body, None).await
    }

    /// As [`Self::post`], with the `ClientToken` in the header the model binds it to
    /// (`X-Amzn-Client-Token`).
    async fn post_with_token(
        &self,
        path: &str,
        body: &Value,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
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
            );
        if let Some(token) = client_token {
            builder = builder.header("x-amzn-client-token", token);
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

/// Does `value` match `arn:aws:billingconductor::<12 digits>:<resource_type>/<id>`
/// with an `[a-zA-Z0-9]` id of `min..=max` characters — the full form of the
/// modelled `PricingPlanArn` / `BillingGroupArn` patterns?
fn is_service_arn(value: &str, resource_type: &str, min: usize, max: usize) -> bool {
    let Some(rest) = value.strip_prefix("arn:aws:billingconductor::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let Some(id) = resource.strip_prefix(&format!("{resource_type}/")) else {
        return false;
    };
    account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && (min..=max).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `PricingPlanArn`: `.../pricingplan/[a-zA-Z0-9]{10}`.
fn is_pricing_plan_arn(value: &str) -> bool {
    is_service_arn(value, "pricingplan", 10, 10)
}

/// `BillingGroupArn`: `.../billinggroup/[a-zA-Z0-9]{10,12}`.
fn is_billing_group_arn(value: &str) -> bool {
    is_service_arn(value, "billinggroup", 10, 12)
}

async fn create_plan(em: &Emulator, name: &str) -> String {
    let (status, body) = em.post("/create-pricing-plan", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "setup CreatePricingPlan {name:?}: expected 200, got {status} with {body:?}"
    );
    body["Arn"]
        .as_str()
        .expect("setup CreatePricingPlan: no Arn in the response")
        .to_string()
}

async fn create_group(
    em: &Emulator,
    name: &str,
    plan_arn: &str,
    primary: Option<&str>,
    linked: &[&str],
) -> String {
    let mut input = json!({
        "Name": name,
        "AccountGrouping": {"LinkedAccountIds": linked},
        "ComputationPreference": {"PricingPlanArn": plan_arn},
    });
    if let Some(primary) = primary {
        input["PrimaryAccountId"] = json!(primary);
    }
    let (status, body) = em.post("/create-billing-group", &input).await;
    assert_eq!(
        status, 200,
        "setup CreateBillingGroup {name:?}: expected 200, got {status} with {body:?}"
    );
    body["Arn"]
        .as_str()
        .expect("setup CreateBillingGroup: no Arn in the response")
        .to_string()
}

async fn list_groups(em: &Emulator, input: &Value) -> Vec<Value> {
    let (status, body) = em.post("/list-billing-groups", input).await;
    assert_eq!(
        status, 200,
        "ListBillingGroups {input}: expected 200, got {status} with {body:?}"
    );
    body["BillingGroups"].as_array().cloned().unwrap_or_default()
}

async fn list_associations(em: &Emulator, input: &Value) -> Vec<Value> {
    let (status, body) = em.post("/list-account-associations", input).await;
    assert_eq!(
        status, 200,
        "ListAccountAssociations {input}: expected 200, got {status} with {body:?}"
    );
    body["LinkedAccounts"].as_array().cloned().unwrap_or_default()
}

/// Every account id a filter selects, following `NextToken` to the last page.
async fn list_all_association_ids(em: &Emulator, input: &Value) -> Vec<String> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = input.clone();
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = em.post("/list-account-associations", &request).await;
        assert_eq!(
            status, 200,
            "ListAccountAssociations {request}: expected 200, got {status} with {body:?}"
        );
        for element in body["LinkedAccounts"].as_array().cloned().unwrap_or_default() {
            collected.push(element["AccountId"].as_str().unwrap_or_default().to_string());
        }
        next_token = body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
}

/// RULE: `CreatePricingPlan` mints a fresh ARN of the modelled form and PERSISTS the
/// plan, whatever optional members the request carries.
#[tokio::test]
async fn create_pricing_plan_mints_a_fresh_stored_arn_for_every_shape_of_input() {
    let long_name = "P".repeat(128);
    // Rows differ along the name's form and length and along which optional members
    // are present. Bounds are the modelled ones: Name len 1..128 with pattern
    // ^[a-zA-Z0-9_\+=\.\-@]+$, Description len 0..1024.
    let cases: Vec<(&str, Value)> = vec![
        ("name only", json!({"Name": "PlanMinimal"})),
        (
            "with a description",
            json!({"Name": "PlanDescribed", "Description": "demo"}),
        ),
        (
            "empty description (the lower modelled bound)",
            json!({"Name": "PlanEmptyDescription", "Description": ""}),
        ),
        (
            "with tags",
            json!({"Name": "PlanTagged", "Tags": {"team": "finance"}}),
        ),
        (
            "with pricing rule arns",
            json!({
                "Name": "PlanWithRules",
                "PricingRuleArns": ["arn:aws:billingconductor::000000000000:pricingrule/0123456789"],
            }),
        ),
        (
            "every punctuation the name pattern allows",
            json!({"Name": "a_b+c=d.e-f@g"}),
        ),
        (
            "128-character name (the upper modelled bound)",
            json!({"Name": long_name}),
        ),
        ("single-character name", json!({"Name": "x"})),
    ];

    let em = Emulator::new();
    let mut minted: Vec<String> = Vec::new();
    for (case, input) in cases {
        let (status, body) = em.post("/create-pricing-plan", &input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let arn = body["Arn"].as_str().unwrap_or_default().to_string();
        assert!(
            is_pricing_plan_arn(&arn),
            "case {case:?}: expected a pricing plan ARN of the modelled form, got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: expected a fresh ARN, got the already-minted {arn:?}"
        );
        // The plan is STORED, not just minted: CreateBillingGroup accepts only a
        // pricing plan that exists.
        let group = json!({
            "Name": format!("Group{}", minted.len()),
            "AccountGrouping": {"LinkedAccountIds": []},
            "ComputationPreference": {"PricingPlanArn": arn},
        });
        let (group_status, group_body) = em.post("/create-billing-group", &group).await;
        assert_eq!(
            group_status, 200,
            "case {case:?}: the minted plan must be stored, got {group_status} with {group_body:?}"
        );
        minted.push(arn);
    }
}

/// RULE: a created billing group reads back out of state with the membership and
/// preferences it was created with; `Size`, `Status` and `BillingGroupType` are
/// derived from them.
#[tokio::test]
async fn a_created_billing_group_reads_back_through_list_billing_groups() {
    let em = Emulator::new();
    let plan = create_plan(&em, "RoundTripPlan").await;

    // Rows differ along membership count and along whether the group has a billing
    // owner (a primary account) — which is what Status is computed from.
    let cases: Vec<(&str, Value, i64, &str)> = vec![
        (
            "no accounts and no primary account",
            json!({
                "Name": "GroupEmpty",
                "AccountGrouping": {"LinkedAccountIds": []},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
            0,
            "PRIMARY_ACCOUNT_MISSING",
        ),
        (
            "one linked account, no primary account",
            json!({
                "Name": "GroupOneNoPrimary",
                "AccountGrouping": {"LinkedAccountIds": ["111122223333"]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
            1,
            "PRIMARY_ACCOUNT_MISSING",
        ),
        (
            "one linked account that is the primary",
            json!({
                "Name": "GroupOne",
                "AccountGrouping": {"LinkedAccountIds": ["222233334444"]},
                "ComputationPreference": {"PricingPlanArn": plan},
                "PrimaryAccountId": "222233334444",
                "Description": "the single-account group",
            }),
            1,
            "ACTIVE",
        ),
        (
            "three linked accounts and auto-associate on",
            json!({
                "Name": "GroupThree",
                "AccountGrouping": {
                    "LinkedAccountIds": ["333344445555", "444455556666", "555566667777"],
                    "AutoAssociate": true,
                },
                "ComputationPreference": {"PricingPlanArn": plan},
                "PrimaryAccountId": "333344445555",
            }),
            3,
            "ACTIVE",
        ),
    ];

    for (case, input, expected_size, expected_status) in cases {
        let (status, body) = em.post("/create-billing-group", &input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let arn = body["Arn"].as_str().unwrap_or_default().to_string();
        assert!(
            is_billing_group_arn(&arn),
            "case {case:?}: expected a billing group ARN of the modelled form, got {arn:?}"
        );

        let listed = list_groups(&em, &json!({"Filters": {"Arns": [arn]}})).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly the created group back, got {listed:?}"
        );
        let element = &listed[0];
        assert_eq!(
            element["Name"], input["Name"],
            "case {case:?}: expected the created name, got {element:?}"
        );
        assert_eq!(
            element["Size"].as_i64(),
            Some(expected_size),
            "case {case:?}: expected Size {expected_size}, got {element:?}"
        );
        assert_eq!(
            element["Status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected Status {expected_status}, got {element:?}"
        );
        assert_eq!(
            element["ComputationPreference"]["PricingPlanArn"].as_str(),
            Some(plan.as_str()),
            "case {case:?}: expected the created pricing plan ARN, got {element:?}"
        );
        assert_eq!(
            element["PrimaryAccountId"], input["PrimaryAccountId"],
            "case {case:?}: expected the created primary account, got {element:?}"
        );
        assert_eq!(
            element["Description"], input["Description"],
            "case {case:?}: expected the created description, got {element:?}"
        );
        assert_eq!(
            element["AccountGrouping"]["AutoAssociate"], input["AccountGrouping"]["AutoAssociate"],
            "case {case:?}: expected the created AutoAssociate, got {element:?}"
        );
        assert_eq!(
            element["BillingGroupType"].as_str(),
            Some("STANDARD"),
            "case {case:?}: a group with no responsibility transfer ARN is STANDARD, got {element:?}"
        );
        let created = element["CreationTime"].as_i64().unwrap_or_default();
        let modified = element["LastModifiedTime"].as_i64().unwrap_or_default();
        assert!(
            created > 0 && modified >= created,
            "case {case:?}: expected epoch-milli timestamps with LastModifiedTime >= CreationTime, got {element:?}"
        );
        // Every linked account is now associated with this group.
        let members = list_associations(&em, &json!({"Filters": {"Association": arn}})).await;
        assert_eq!(
            i64::try_from(members.len()).unwrap_or(-1),
            expected_size,
            "case {case:?}: expected {expected_size} associated accounts, got {members:?}"
        );
    }
}

/// RULE: replaying a create with the same `ClientToken` completes without creating a
/// second resource — it answers with the ARN the first call minted.
#[tokio::test]
async fn replaying_a_client_token_creates_no_second_resource() {
    let em = Emulator::new();

    let plan_input = json!({"Name": "IdempotentPlan"});
    let (_, first_plan) = em
        .post_with_token("/create-pricing-plan", &plan_input, Some("token-plan"))
        .await;
    let (status, second_plan) = em
        .post_with_token("/create-pricing-plan", &plan_input, Some("token-plan"))
        .await;
    assert_eq!(
        status, 200,
        "case \"CreatePricingPlan replay\": expected 200, got {status} with {second_plan:?}"
    );
    assert_eq!(
        first_plan["Arn"], second_plan["Arn"],
        "case \"CreatePricingPlan replay\": expected the first ARN back, got {second_plan:?}"
    );

    let plan = first_plan["Arn"].as_str().unwrap_or_default().to_string();
    let group_input = json!({
        "Name": "IdempotentGroup",
        "AccountGrouping": {"LinkedAccountIds": ["111122223333"]},
        "ComputationPreference": {"PricingPlanArn": plan},
        "PrimaryAccountId": "111122223333",
    });
    let (_, first_group) = em
        .post_with_token("/create-billing-group", &group_input, Some("token-group"))
        .await;
    let (status, second_group) = em
        .post_with_token("/create-billing-group", &group_input, Some("token-group"))
        .await;
    assert_eq!(
        status, 200,
        "case \"CreateBillingGroup replay\": expected 200, got {status} with {second_group:?}"
    );
    assert_eq!(
        first_group["Arn"], second_group["Arn"],
        "case \"CreateBillingGroup replay\": expected the first ARN back, got {second_group:?}"
    );
    let listed = list_groups(&em, &json!({})).await;
    assert_eq!(
        listed.len(),
        1,
        "case \"CreateBillingGroup replay\": expected one group in state, got {listed:?}"
    );
}

/// RULE: a resource name is unique within its kind — creating a second one with a
/// name already in use is a 409 `ConflictException`.
#[tokio::test]
async fn a_duplicate_resource_name_is_a_conflict() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ConflictPlan").await;
    let group_input = json!({
        "Name": "ConflictGroup",
        "AccountGrouping": {"LinkedAccountIds": []},
        "ComputationPreference": {"PricingPlanArn": plan},
    });
    let (status, body) = em.post("/create-billing-group", &group_input).await;
    assert_eq!(
        status, 200,
        "setup CreateBillingGroup: expected 200, got {status} with {body:?}"
    );

    // One row per kind of named resource this slice creates.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "pricing plan name already in use",
            "/create-pricing-plan",
            json!({"Name": "ConflictPlan"}),
        ),
        (
            "billing group name already in use",
            "/create-billing-group",
            group_input.clone(),
        ),
    ];
    for (case, path, input) in cases {
        let (status, body) = em.post(path, &input).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body:?}"
        );
    }
}

/// RULE: a billing group's `ComputationPreference.PricingPlanArn` must name a plan
/// that exists — one the caller created, or one of the AWS-managed plans the
/// `PricingPlanFullArn` pattern enumerates
/// (`(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`).
#[tokio::test]
async fn a_billing_group_needs_a_pricing_plan_that_exists() {
    let em = Emulator::new();

    // Accepted: the AWS-managed plans, which no caller creates. Neither is the
    // customer-minted id form the other rows use.
    let accepted = [
        (
            "the AWS-managed BasicPricingPlan",
            "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan",
        ),
        (
            "the AWS-managed Passthrough plan",
            "arn:aws:billingconductor::aws:pricingplan/Passthrough",
        ),
    ];
    for (index, (case, plan)) in accepted.iter().enumerate() {
        let input = json!({
            "Name": format!("ManagedPlanGroup{index}"),
            "AccountGrouping": {"LinkedAccountIds": []},
            "ComputationPreference": {"PricingPlanArn": plan},
        });
        let (status, body) = em.post("/create-billing-group", &input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
    }

    // Refused: a well-formed plan ARN this emulator never minted. The operation
    // models no ResourceNotFoundException, so the modelled refusal is a 400
    // ValidationException with the reason the model enumerates.
    let refused = [
        (
            "a customer-form plan id that was never created",
            "arn:aws:billingconductor::000000000000:pricingplan/ZZZZZZZZZZ",
        ),
        (
            "a plan in another account",
            "arn:aws:billingconductor::123456789012:pricingplan/AAAAAAAAAA",
        ),
        (
            "a China-partition plan that was never created",
            "arn:aws-cn:billingconductor::000000000000:pricingplan/BBBBBBBBBB",
        ),
        // The managed ids are managed only as the ARN Amazon Web Services owns: the
        // same id under another partition or another account is an ordinary plan
        // that has to exist.
        (
            "a managed plan id in the China partition",
            "arn:aws-cn:billingconductor::aws:pricingplan/Passthrough",
        ),
        (
            "a managed plan id under a 12-digit account",
            "arn:aws:billingconductor::123456789012:pricingplan/BasicPricingPlan",
        ),
    ];
    for (index, (case, plan)) in refused.iter().enumerate() {
        let input = json!({
            "Name": format!("MissingPlanGroup{index}"),
            "AccountGrouping": {"LinkedAccountIds": []},
            "ComputationPreference": {"PricingPlanArn": plan},
        });
        let (status, body) = em.post("/create-billing-group", &input).await;
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
    }

    // And no group was created by a refused call.
    let listed = list_groups(&em, &json!({})).await;
    assert_eq!(
        listed.len(),
        accepted.len(),
        "expected only the accepted groups in state, got {listed:?}"
    );
}

/// RULE: `AssociateAccounts` appends the named accounts to the stored group, so its
/// `Size` and the account-association index both move with the membership; an
/// account already in the group is left as it is.
#[tokio::test]
async fn associate_accounts_appends_to_the_stored_group() {
    let em = Emulator::new();
    let plan = create_plan(&em, "AssociatePlan").await;
    let group = create_group(
        &em,
        "AssociateGroup",
        &plan,
        Some("111122223333"),
        &["111122223333"],
    )
    .await;

    // Rows differ along how many accounts one call associates, and whether they are
    // already members. The expected size is cumulative.
    let cases: Vec<(&str, Vec<&str>, i64)> = vec![
        ("no accounts at all (the lower modelled bound)", vec![], 1),
        ("one new account", vec!["444455556666"], 2),
        (
            "two new accounts in one call",
            vec!["555566667777", "666677778888"],
            4,
        ),
        ("an account that is already a member", vec!["444455556666"], 4),
        (
            "a mix of a member and a new account",
            vec!["444455556666", "777788889999"],
            5,
        ),
    ];

    for (case, accounts, expected_size) in cases {
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": group, "AccountIds": accounts}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["Arn"].as_str(),
            Some(group.as_str()),
            "case {case:?}: expected the group ARN echoed, got {body:?}"
        );

        let listed = list_groups(&em, &json!({"Filters": {"Arns": [group]}})).await;
        let element = listed.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            element["Size"].as_i64(),
            Some(expected_size),
            "case {case:?}: expected Size {expected_size}, got {element:?}"
        );
        let members = list_associations(&em, &json!({"Filters": {"Association": group}})).await;
        assert_eq!(
            i64::try_from(members.len()).unwrap_or(-1),
            expected_size,
            "case {case:?}: expected {expected_size} associated accounts, got {members:?}"
        );
        for member in &members {
            assert_eq!(
                member["BillingGroupArn"].as_str(),
                Some(group.as_str()),
                "case {case:?}: every associated account must carry the group ARN, got {member:?}"
            );
        }
    }
}

/// RULE: an account belongs to at most one billing group, and an operation that
/// names a billing group that does not exist refuses with the modelled error for a
/// missing resource.
#[tokio::test]
async fn associate_accounts_refuses_an_unknown_group_and_a_taken_account() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ExclusivityPlan").await;
    let owner = create_group(
        &em,
        "OwnerGroup",
        &plan,
        Some("111122223333"),
        &["111122223333", "222233334444"],
    )
    .await;
    let other = create_group(&em, "OtherGroup", &plan, Some("999988887777"), &[]).await;

    // Rows differ along which form of ARN names the absent group: the full ARN and
    // the bare id the pattern also accepts.
    let unknown_groups = [
        (
            "a full ARN naming no stored group",
            "arn:aws:billingconductor::000000000000:billinggroup/ZZZZZZZZZZZZ".to_string(),
        ),
        ("a bare id naming no stored group", "ZZZZZZZZZZ".to_string()),
    ];
    for (case, arn) in &unknown_groups {
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": arn, "AccountIds": ["333344445555"]}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // Rows differ along which already-owned account the second group asks for.
    let taken = ["111122223333", "222233334444"];
    for account in taken {
        let case = format!("account {account} is already in another group");
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": other, "AccountIds": [account]}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        // The refusal changed nothing: the account still belongs to the first group.
        let members = list_associations(&em, &json!({"Filters": {"AccountId": account}})).await;
        assert_eq!(
            members.len(),
            1,
            "case {case:?}: expected one association for the account, got {members:?}"
        );
        assert_eq!(
            members[0]["BillingGroupArn"].as_str(),
            Some(owner.as_str()),
            "case {case:?}: expected the account to stay with its first group, got {members:?}"
        );
    }

    // The same rule holds at create time: a group cannot be created over an account
    // another group already owns.
    let (status, body) = em
        .post(
            "/create-billing-group",
            &json!({
                "Name": "StealingGroup",
                "AccountGrouping": {"LinkedAccountIds": ["111122223333"]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"CreateBillingGroup over a taken account\": expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"CreateBillingGroup over a taken account\": expected ValidationException, got {body:?}"
    );
}

/// RULE: a request that names the same account twice is refused — with the
/// `DUPLICATE_ACCOUNT` reason the model enumerates.
#[tokio::test]
async fn an_account_listed_twice_in_one_request_is_refused() {
    let em = Emulator::new();
    let plan = create_plan(&em, "DuplicatePlan").await;
    let group = create_group(&em, "DuplicateGroup", &plan, Some("111122223333"), &[]).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "AssociateAccounts names one account twice",
            "/associate-accounts",
            json!({"Arn": group, "AccountIds": ["222233334444", "222233334444"]}),
        ),
        (
            "AssociateAccounts names one account twice among others",
            "/associate-accounts",
            json!({
                "Arn": group,
                "AccountIds": ["333344445555", "444455556666", "333344445555"],
            }),
        ),
        (
            "CreateBillingGroup names one linked account twice",
            "/create-billing-group",
            json!({
                "Name": "DuplicateLinked",
                "AccountGrouping": {"LinkedAccountIds": ["555566667777", "555566667777"]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
        ),
    ];
    for (case, path, input) in cases {
        let (status, body) = em.post(path, &input).await;
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
            Some("DUPLICATE_ACCOUNT"),
            "case {case:?}: expected the DUPLICATE_ACCOUNT reason, got {body:?}"
        );
    }
}

/// RULE: `ListBillingGroups` returns at most `MaxResults` elements and, when it
/// truncates, a `NextToken` that resumes exactly where it stopped — whatever the
/// page size.
#[tokio::test]
async fn list_billing_groups_pages_by_max_results() {
    let em = Emulator::new();
    let plan = create_plan(&em, "PagingPlan").await;
    let mut all = Vec::new();
    for index in 0..5 {
        all.push(create_group(&em, &format!("PagingGroup{index}"), &plan, None, &[]).await);
    }
    all.sort();

    // Rows differ along the page size: below, at, and above the number of groups,
    // plus the modelled bounds of MaxResults (1..100).
    for max_results in [1_i64, 2, 3, 5, 100] {
        let case = format!("MaxResults={max_results}");
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({"MaxResults": max_results});
            if let Some(token) = &next_token {
                input["NextToken"] = json!(token);
            }
            let (status, body) = em.post("/list-billing-groups", &input).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body:?}"
            );
            let page = body["BillingGroups"].as_array().cloned().unwrap_or_default();
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case:?}: expected at most {max_results} elements, got {page:?}"
            );
            for element in page {
                collected.push(element["Arn"].as_str().unwrap_or_default().to_string());
            }
            next_token = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, all,
            "case {case:?}: expected every group exactly once in order, got {collected:?}"
        );
    }
}

/// RULE: `ListBillingGroups`'s filters each narrow the listing by one property of
/// the stored group, and an absent filter narrows nothing.
#[tokio::test]
async fn list_billing_groups_honours_each_filter() {
    let em = Emulator::new();
    let plan_a = create_plan(&em, "FilterPlanA").await;
    let plan_b = create_plan(&em, "FilterPlanB").await;
    let active = create_group(
        &em,
        "AlphaActive",
        &plan_a,
        Some("111122223333"),
        &["111122223333"],
    )
    .await;
    let missing_primary = create_group(&em, "BetaPending", &plan_b, None, &[]).await;
    // A transfer-billing group: no primary account, but a responsibility transfer
    // relationship — the model documents `ResponsibilityTransferArn` as "When
    // specified, the PrimaryAccountId is no longer required". Its shape is the
    // member's own pattern,
    // ^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$
    let transfer_arn =
        "arn:aws:organizations::000000000000:transfer/o-abcd123456/billing/inbound/rt-1234abcd";
    let (status, body) = em
        .post(
            "/create-billing-group",
            &json!({
                "Name": "GammaTransfer",
                "AccountGrouping": {
                    "LinkedAccountIds": ["777788889999"],
                    "AutoAssociate": true,
                    "ResponsibilityTransferArn": transfer_arn,
                },
                "ComputationPreference": {"PricingPlanArn": plan_a},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup CreateBillingGroup GammaTransfer: expected 200, got {status} with {body:?}"
    );
    let transfer = body["Arn"].as_str().unwrap_or_default().to_string();

    // A group that bills through a transfer relationship is ACTIVE without a primary
    // account, and its type is TRANSFER_BILLING.
    let listed = list_groups(&em, &json!({"Filters": {"Arns": [transfer]}})).await;
    let element = listed.first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        element["Status"].as_str(),
        Some("ACTIVE"),
        "case \"transfer-billing group\": a responsibility transfer ARN stands in for the primary account, got {element:?}"
    );
    assert_eq!(
        element["BillingGroupType"].as_str(),
        Some("TRANSFER_BILLING"),
        "case \"transfer-billing group\": expected TRANSFER_BILLING, got {element:?}"
    );
    assert_eq!(
        element["AccountGrouping"]["ResponsibilityTransferArn"].as_str(),
        Some(transfer_arn),
        "case \"transfer-billing group\": expected the created transfer ARN, got {element:?}"
    );

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "no filters",
            json!({}),
            vec![&active, &missing_primary, &transfer],
        ),
        (
            "Arns names one group",
            json!({"Filters": {"Arns": [active]}}),
            vec![&active],
        ),
        (
            "Arns names both groups",
            json!({"Filters": {"Arns": [active, missing_primary]}}),
            vec![&active, &missing_primary],
        ),
        (
            "PricingPlan selects the groups computing with it",
            json!({"Filters": {"PricingPlan": plan_b}}),
            vec![&missing_primary],
        ),
        (
            "PricingPlan selects both groups computing with it",
            json!({"Filters": {"PricingPlan": plan_a}}),
            vec![&active, &transfer],
        ),
        (
            "Statuses selects ACTIVE",
            json!({"Filters": {"Statuses": ["ACTIVE"]}}),
            vec![&active, &transfer],
        ),
        (
            "Statuses selects PRIMARY_ACCOUNT_MISSING",
            json!({"Filters": {"Statuses": ["PRIMARY_ACCOUNT_MISSING"]}}),
            vec![&missing_primary],
        ),
        (
            "Names matches a prefix",
            json!({"Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Alpha"}]}}),
            vec![&active],
        ),
        (
            "Names matches a prefix shared by no group",
            json!({"Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Delta"}]}}),
            vec![],
        ),
        (
            "Names matches the transfer group's prefix",
            json!({"Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Gamma"}]}}),
            vec![&transfer],
        ),
        (
            "PrimaryAccountIds selects by primary account",
            json!({"Filters": {"PrimaryAccountIds": ["111122223333"]}}),
            vec![&active],
        ),
        (
            "BillingGroupTypes selects STANDARD",
            json!({"Filters": {"BillingGroupTypes": ["STANDARD"]}}),
            vec![&active, &missing_primary],
        ),
        (
            "BillingGroupTypes selects TRANSFER_BILLING",
            json!({"Filters": {"BillingGroupTypes": ["TRANSFER_BILLING"]}}),
            vec![&transfer],
        ),
        (
            "AutoAssociate selects the groups that have it on",
            json!({"Filters": {"AutoAssociate": true}}),
            vec![&transfer],
        ),
        (
            "AutoAssociate selects the groups that have it off",
            json!({"Filters": {"AutoAssociate": false}}),
            vec![&active, &missing_primary],
        ),
        (
            "ResponsibilityTransferArns selects by transfer relationship",
            json!({"Filters": {"ResponsibilityTransferArns": [transfer_arn]}}),
            vec![&transfer],
        ),
        (
            "ResponsibilityTransferArns matching no group",
            json!({"Filters": {"ResponsibilityTransferArns": ["arn:aws:organizations::000000000000:transfer/o-zzzz999999/billing/outbound/rt-9999zzzz"]}}),
            vec![],
        ),
    ];

    for (case, input, expected) in cases {
        let listed = list_groups(&em, &input).await;
        let mut got: Vec<String> = listed
            .iter()
            .map(|element| element["Arn"].as_str().unwrap_or_default().to_string())
            .collect();
        got.sort();
        let mut want: Vec<String> = expected.into_iter().cloned().collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?} from {listed:?}"
        );
    }
}

/// RULE: `ListAccountAssociations` reports every linked account it knows with the
/// billing group it is associated with, and its `Association` filter selects
/// `MONITORED`, `UNMONITORED` or one group's members.
#[tokio::test]
async fn list_account_associations_honours_each_filter() {
    let em = Emulator::new();
    let plan = create_plan(&em, "AssociationPlan").await;
    let group = create_group(
        &em,
        "AssociationGroup",
        &plan,
        Some("111122223333"),
        &["111122223333", "222233334444"],
    )
    .await;
    // A primary account that is not a member: known to the payer account, but not
    // associated with any billing group.
    let loose = create_group(&em, "LooseGroup", &plan, Some("999988887777"), &[]).await;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filters lists every known account",
            json!({}),
            vec!["111122223333", "222233334444", "999988887777"],
        ),
        (
            "MONITORED lists the associated accounts",
            json!({"Filters": {"Association": "MONITORED"}}),
            vec!["111122223333", "222233334444"],
        ),
        (
            "UNMONITORED lists the accounts in no group",
            json!({"Filters": {"Association": "UNMONITORED"}}),
            vec!["999988887777"],
        ),
        (
            "a billing group ARN lists that group's members",
            json!({"Filters": {"Association": group}}),
            vec!["111122223333", "222233334444"],
        ),
        (
            "an empty billing group lists nothing",
            json!({"Filters": {"Association": loose}}),
            vec![],
        ),
        (
            "AccountId selects one account",
            json!({"Filters": {"AccountId": "222233334444"}}),
            vec!["222233334444"],
        ),
        (
            "AccountIds selects several accounts",
            json!({"Filters": {"AccountIds": ["111122223333", "999988887777"]}}),
            vec!["111122223333", "999988887777"],
        ),
        (
            "AccountId and MONITORED together",
            json!({"Filters": {"Association": "MONITORED", "AccountId": "999988887777"}}),
            vec![],
        ),
    ];

    for (case, input, expected) in cases {
        let listed = list_associations(&em, &input).await;
        let got: Vec<String> = listed
            .iter()
            .map(|element| element["AccountId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?} from {listed:?}"
        );
    }

    // An `Association` naming a group that does not exist is a reference to a
    // resource that doesn't exist.
    let (status, body) = em
        .post(
            "/list-account-associations",
            &json!({"Filters": {"Association": "ZZZZZZZZZZ"}}),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"Association names no stored group\": expected HTTP 404, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"Association names no stored group\": expected ResourceNotFoundException, got {body:?}"
    );
}

/// RULE: a listing is bounded even when the request names no page size — the
/// server's default page size caps it and the modelled `NextToken` continues from
/// there. `ListAccountAssociations` models no `MaxResults` at all, so it is the
/// operation that pins the default.
#[tokio::test]
async fn a_listing_with_no_page_size_is_capped_and_continues_by_token() {
    const DEFAULT_PAGE_SIZE: usize = 100;

    let em = Emulator::new();
    let plan = create_plan(&em, "CapPlan").await;
    // 101 linked accounts: one more than the default page, built 30 at a time
    // because the model caps LinkedAccountIds / AccountIds at 30 per call.
    let accounts: Vec<String> = (1..=101).map(|n| format!("{n:012}")).collect();
    let first: Vec<&str> = accounts[..30].iter().map(String::as_str).collect();
    let group = create_group(&em, "CapGroup", &plan, Some(first[0]), &first).await;
    for chunk in accounts[30..].chunks(30) {
        let ids: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": group, "AccountIds": ids}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup AssociateAccounts {ids:?}: expected 200, got {status} with {body:?}"
        );
    }

    let (status, body) = em.post("/list-account-associations", &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"first page\": expected HTTP 200, got {status} with {body:?}"
    );
    let page = body["LinkedAccounts"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        page.len(),
        DEFAULT_PAGE_SIZE,
        "case \"first page\": expected the default page size, got {} elements",
        page.len()
    );
    let token = body["NextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "case \"first page\": expected a NextToken for the truncated listing, got {body:?}"
    );

    let (status, body) = em
        .post("/list-account-associations", &json!({"NextToken": token}))
        .await;
    assert_eq!(
        status, 200,
        "case \"second page\": expected HTTP 200, got {status} with {body:?}"
    );
    let rest = body["LinkedAccounts"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        rest.len(),
        accounts.len() - DEFAULT_PAGE_SIZE,
        "case \"second page\": expected the remainder, got {rest:?}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case \"second page\": expected no further NextToken, got {body:?}"
    );
    let collected: Vec<String> = page
        .iter()
        .chain(rest.iter())
        .map(|element| element["AccountId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        collected, accounts,
        "case \"both pages\": expected every account exactly once in order, got {collected:?}"
    );
}

/// RULE: deleting a billing group removes it and releases its accounts — the next
/// reader sees neither the group nor its associations; a delete of a group that does
/// not exist is refused and changes nothing.
#[tokio::test]
async fn deleting_a_billing_group_is_observable_by_the_next_caller() {
    let em = Emulator::new();
    let plan = create_plan(&em, "DeletePlan").await;

    // Rows differ along how the delete names the group (full ARN / bare id) and how
    // many accounts it releases.
    let cases: Vec<(&str, &str, Vec<&str>, bool)> = vec![
        ("an empty group, by full ARN", "DeleteEmpty", vec![], false),
        (
            "a one-account group, by full ARN",
            "DeleteOne",
            vec!["111122223333"],
            false,
        ),
        (
            "a three-account group, by bare id",
            "DeleteThree",
            vec!["222233334444", "333344445555", "444455556666"],
            true,
        ),
    ];

    for (case, name, accounts, by_bare_id) in cases {
        let arn = create_group(&em, name, &plan, accounts.first().copied(), &accounts).await;
        let named = if by_bare_id {
            arn.rsplit('/').next().unwrap_or(&arn).to_string()
        } else {
            arn.clone()
        };
        let (status, body) = em.post("/delete-billing-group", &json!({"Arn": named})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["Arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the deleted group's ARN, got {body:?}"
        );

        let listed = list_groups(&em, &json!({"Filters": {"Arns": [arn]}})).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected the group to be gone, got {listed:?}"
        );
        // Its accounts are released, not forgotten: still linked accounts of the
        // payer account, now in no billing group.
        let unmonitored = list_associations(&em, &json!({"Filters": {"Association": "UNMONITORED"}}))
            .await
            .iter()
            .map(|element| element["AccountId"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>();
        for account in &accounts {
            assert!(
                unmonitored.contains(&(*account).to_string()),
                "case {case:?}: expected {account} to be UNMONITORED after the delete, got {unmonitored:?}"
            );
        }

        // A second delete of the same group names a group that no longer exists.
        let (status, body) = em.post("/delete-billing-group", &json!({"Arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 on the second delete, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException on the second delete, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("INVALID_BILLING_GROUP"),
            "case {case:?}: expected the INVALID_BILLING_GROUP reason, got {body:?}"
        );
    }

    // The released accounts can join a new group afterwards: the exclusivity index
    // really was cleared.
    let regrouped = create_group(
        &em,
        "Regrouped",
        &plan,
        Some("111122223333"),
        &["111122223333", "222233334444"],
    )
    .await;
    let members = list_associations(&em, &json!({"Filters": {"Association": regrouped}})).await;
    assert_eq!(
        members.len(),
        2,
        "case \"reuse released accounts\": expected both accounts associated, got {members:?}"
    );
    // And they are reported once, as MONITORED — a re-associated account must not
    // still be listed as released.
    let unmonitored =
        list_all_association_ids(&em, &json!({"Filters": {"Association": "UNMONITORED"}})).await;
    for account in ["111122223333", "222233334444"] {
        assert!(
            !unmonitored.contains(&account.to_string()),
            "case \"reuse released accounts\": expected {account} to stop being UNMONITORED, got {unmonitored:?}"
        );
    }
    let monitored =
        list_all_association_ids(&em, &json!({"Filters": {"Association": "MONITORED"}})).await;
    assert_eq!(
        monitored
            .iter()
            .filter(|id| *id == "111122223333")
            .count(),
        1,
        "case \"reuse released accounts\": expected the account reported exactly once, got {monitored:?}"
    );
}

/// RULE: a `ClientToken` stops naming a resource that no longer exists — deleting a
/// billing group forgets the token that created it, so a replay creates a new group
/// instead of answering with a deleted ARN.
#[tokio::test]
async fn a_deleted_group_releases_its_client_token() {
    let em = Emulator::new();
    let plan = create_plan(&em, "TokenLifetimePlan").await;
    let input = json!({
        "Name": "TokenLifetimeGroup",
        "AccountGrouping": {"LinkedAccountIds": ["111122223333"]},
        "ComputationPreference": {"PricingPlanArn": plan},
        "PrimaryAccountId": "111122223333",
    });

    let (_, first) = em
        .post_with_token("/create-billing-group", &input, Some("token-lifetime"))
        .await;
    let created = first["Arn"].as_str().unwrap_or_default().to_string();
    let (status, body) = em
        .post("/delete-billing-group", &json!({"Arn": created}))
        .await;
    assert_eq!(
        status, 200,
        "setup DeleteBillingGroup: expected 200, got {status} with {body:?}"
    );

    let (status, replay) = em
        .post_with_token("/create-billing-group", &input, Some("token-lifetime"))
        .await;
    assert_eq!(
        status, 200,
        "case \"replay after delete\": expected HTTP 200, got {status} with {replay:?}"
    );
    let recreated = replay["Arn"].as_str().unwrap_or_default().to_string();
    assert_ne!(
        recreated, created,
        "case \"replay after delete\": expected a new ARN, got the deleted {created:?}"
    );
    let listed = list_groups(&em, &json!({"Filters": {"Arns": [recreated]}})).await;
    assert_eq!(
        listed.len(),
        1,
        "case \"replay after delete\": expected the new group to be readable, got {listed:?}"
    );
}

/// RULE: a store the caller can grow is bounded — a create past the documented
/// maximum is REFUSED with the modelled `ServiceLimitExceededException`, and the
/// resources already stored stay readable (the cap never evicts one).
#[tokio::test]
async fn creating_past_the_stored_maximum_is_refused() {
    // The bound the service documents and enforces in one place.
    use service_com_amazonaws_billingconductor_awsbillingconductor::state::MAX_PRICING_PLANS;

    let em = Emulator::new();
    let first = create_plan(&em, "LimitPlan0").await;
    for index in 1..MAX_PRICING_PLANS {
        let (status, body) = em
            .post(
                "/create-pricing-plan",
                &json!({"Name": format!("LimitPlan{index}")}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {index}: expected 200, got {status} with {body:?}"
        );
    }

    let (status, body) = em
        .post("/create-pricing-plan", &json!({"Name": "OneTooMany"}))
        .await;
    assert_eq!(
        status, 402,
        "case \"one plan past the maximum\": expected HTTP 402, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceLimitExceededException",
        "case \"one plan past the maximum\": expected ServiceLimitExceededException, got {body:?}"
    );
    assert_eq!(
        body["LimitCode"].as_str(),
        Some("PricingPlansPerAccount"),
        "case \"one plan past the maximum\": expected the limit code, got {body:?}"
    );

    // Nothing was evicted to make room: the first plan is still stored, so a billing
    // group may still name it.
    let (status, body) = em
        .post(
            "/create-billing-group",
            &json!({
                "Name": "LimitGroup",
                "AccountGrouping": {"LinkedAccountIds": []},
                "ComputationPreference": {"PricingPlanArn": first},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the first plan survives the cap\": expected HTTP 200, got {status} with {body:?}"
    );
}

/// RULE: the tail of released (`UNMONITORED`) accounts the service remembers is
/// bounded — once more accounts have been released than it keeps, the oldest are
/// forgotten and the newest are still reported.
#[tokio::test]
async fn the_released_account_tail_is_bounded() {
    // The bound the service documents and enforces in one place.
    use service_com_amazonaws_billingconductor_awsbillingconductor::state::MAX_UNASSOCIATED_ACCOUNTS;

    let em = Emulator::new();
    let plan = create_plan(&em, "TailPlan").await;
    // More released accounts than the cap, 30 at a time (the modelled per-call
    // maximum for LinkedAccountIds): create a group, then delete it to release them.
    let total = MAX_UNASSOCIATED_ACCOUNTS + 60;
    let accounts: Vec<String> = (1..=total).map(|n| format!("{n:012}")).collect();
    for (index, chunk) in accounts.chunks(30).enumerate() {
        let ids: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let arn = create_group(&em, &format!("TailGroup{index}"), &plan, None, &ids).await;
        let (status, body) = em.post("/delete-billing-group", &json!({"Arn": arn})).await;
        assert_eq!(
            status, 200,
            "setup DeleteBillingGroup {arn}: expected 200, got {status} with {body:?}"
        );
    }

    let unmonitored =
        list_all_association_ids(&em, &json!({"Filters": {"Association": "UNMONITORED"}})).await;
    assert_eq!(
        unmonitored.len(),
        MAX_UNASSOCIATED_ACCOUNTS,
        "case \"released tail\": expected the documented maximum, got {} ids",
        unmonitored.len()
    );
    let newest = accounts.last().expect("the account list is not empty");
    assert!(
        unmonitored.contains(newest),
        "case \"released tail\": expected the newest released account {newest} to be kept"
    );
    let oldest = &accounts[0];
    assert!(
        !unmonitored.contains(oldest),
        "case \"released tail\": expected the oldest released account {oldest} to be evicted"
    );
}
