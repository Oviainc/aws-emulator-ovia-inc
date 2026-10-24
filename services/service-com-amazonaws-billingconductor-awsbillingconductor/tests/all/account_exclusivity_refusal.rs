// SPDX-License-Identifier: CC0-1.0

//! The refusal an account-id list earns when it names an account another billing
//! group already owns.
//!
//! RULE under test: an account belongs to at most one billing group, so a request
//! whose account-id list names an account already associated with a DIFFERENT
//! billing group is refused as an INPUT-validation failure — the modelled
//! `ValidationException` (HTTP 400, error code `ValidationException`) with
//! `Reason = ACCOUNTS_ALREADY_ASSOCIATED` (the value
//! `smithy_query(query="shape",shape="ValidationExceptionReason")` enumerates for
//! this case) and the input member that carried the id in `Fields` — and it changes
//! nothing. `ConflictException` stays reserved for the inconsistent-resource-state
//! cases `ConflictExceptionReason` enumerates, a name already in use among them.
//!
//! The rows span what the rule generalises over rather than one recorded call: the
//! operation and the member that carries the ids (`CreateBillingGroup`'s
//! `AccountGrouping.LinkedAccountIds`, `AssociateAccounts`'s `AccountIds`), how many
//! ids the list holds (1, 2, 3), where in the list the already-owned one sits (only,
//! first, middle, last), whether that account is its owner's primary account or a
//! plain member, which of two owning groups holds it, and — for the operation that
//! addresses a group — whether the group is named by full ARN or bare id.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.

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
    /// organization's PAYER account — Billing Conductor's control plane is authorized
    /// for the payer account alone (see `billing_group_core_loop`'s harness), so a
    /// designated payer is what makes these refusals reachable at all.
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

/// The `Fields[*].Name` values of a `ValidationException` body.
fn field_names(body: &Value) -> Vec<String> {
    body.get("Fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    field
                        .get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
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

/// The billing group an account is associated with, read back out of state.
async fn owner_of(em: &Emulator, account_id: &str) -> Option<String> {
    let (status, body) = em
        .post(
            "/list-account-associations",
            &json!({"Filters": {"AccountId": account_id}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ListAccountAssociations for {account_id}: expected 200, got {status} with {body:?}"
    );
    let listed = body["LinkedAccounts"].as_array().cloned().unwrap_or_default();
    assert!(
        listed.len() <= 1,
        "account {account_id}: expected at most one association, got {listed:?}"
    );
    listed
        .first()
        .and_then(|entry| entry.get("BillingGroupArn"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every stored billing group's name.
async fn group_names(em: &Emulator) -> Vec<String> {
    let (status, body) = em.post("/list-billing-groups", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListBillingGroups: expected 200, got {status} with {body:?}"
    );
    body["BillingGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The bare resource id of a full billing-group ARN — the other spelling the
/// modelled `BillingGroupArn` pattern accepts.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// RULE: every account-id list that names an account another billing group already
/// owns is refused 400 `ValidationException` with `ACCOUNTS_ALREADY_ASSOCIATED` and
/// the offending member in `Fields`, and the refusal changes nothing.
#[tokio::test]
async fn an_account_owned_by_another_group_is_a_validation_refusal() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ExclusivityRefusalPlan").await;
    // Two owning groups, so the rule is asserted over more than one owner: the
    // primary account of the first, a plain member of either.
    let first = create_group(
        &em,
        "FirstOwnerGroup",
        &plan,
        Some("111111111111"),
        &["111111111111", "222222222222"],
    )
    .await;
    let second = create_group(&em, "SecondOwnerGroup", &plan, None, &["333333333333"]).await;
    let target = create_group(&em, "TargetGroup", &plan, None, &[]).await;

    // Rows: (case, taken account, its owner, the request to refuse, the member that
    // carried the ids). They differ along the operation, the member, the list length,
    // the position of the already-owned id in the list, whether that account is its
    // owner's primary account, which owner holds it, and (for AssociateAccounts) how
    // the target group is addressed.
    let cases: Vec<(&str, &str, &str, &str, Value, &str)> = vec![
        (
            "AssociateAccounts, the only id, a primary account",
            "111111111111",
            &first,
            "/associate-accounts",
            json!({"Arn": target, "AccountIds": ["111111111111"]}),
            "AccountIds",
        ),
        (
            "AssociateAccounts, first of three, a plain member",
            "222222222222",
            &first,
            "/associate-accounts",
            json!({
                "Arn": target,
                "AccountIds": ["222222222222", "444444444444", "555555555555"],
            }),
            "AccountIds",
        ),
        (
            "AssociateAccounts, middle of three, the second owner's member",
            "333333333333",
            &second,
            "/associate-accounts",
            json!({
                "Arn": target,
                "AccountIds": ["444444444444", "333333333333", "555555555555"],
            }),
            "AccountIds",
        ),
        (
            "AssociateAccounts, last of two, group named by bare id",
            "222222222222",
            &first,
            "/associate-accounts",
            json!({
                "Arn": bare_id(&target),
                "AccountIds": ["444444444444", "222222222222"],
            }),
            "AccountIds",
        ),
        (
            "CreateBillingGroup, the only linked id, a primary account",
            "111111111111",
            &first,
            "/create-billing-group",
            json!({
                "Name": "StealOnlyId",
                "AccountGrouping": {"LinkedAccountIds": ["111111111111"]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
            "AccountGrouping.LinkedAccountIds",
        ),
        (
            "CreateBillingGroup, last of three linked ids",
            "333333333333",
            &second,
            "/create-billing-group",
            json!({
                "Name": "StealLastId",
                "AccountGrouping": {"LinkedAccountIds": [
                    "444444444444", "555555555555", "333333333333",
                ]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
            "AccountGrouping.LinkedAccountIds",
        ),
        (
            "CreateBillingGroup, a taken id beside a primary account of its own",
            "222222222222",
            &first,
            "/create-billing-group",
            json!({
                "Name": "StealWithPrimary",
                "AccountGrouping": {"LinkedAccountIds": ["666666666666", "222222222222"]},
                "ComputationPreference": {"PricingPlanArn": plan},
                "PrimaryAccountId": "666666666666",
            }),
            "AccountGrouping.LinkedAccountIds",
        ),
    ];

    for (case, taken, owner, path, input, field) in cases {
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
            Some("ACCOUNTS_ALREADY_ASSOCIATED"),
            "case {case:?}: expected the ACCOUNTS_ALREADY_ASSOCIATED reason, got {body:?}"
        );
        assert_eq!(
            field_names(&body),
            vec![field.to_string()],
            "case {case:?}: expected {field:?} named in Fields, got {body:?}"
        );
        assert!(
            body["Message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: expected a non-empty Message, got {body:?}"
        );

        // The refusal changed nothing: the account still belongs to its first group,
        // the accounts the request named beside it were not associated, and no group
        // the refused create named exists.
        assert_eq!(
            owner_of(&em, taken).await.as_deref(),
            Some(owner),
            "case {case:?}: expected account {taken} to stay with its first group"
        );
        let untouched = ["444444444444", "555555555555", "666666666666"];
        for account in untouched {
            assert_eq!(
                owner_of(&em, account).await,
                None,
                "case {case:?}: expected account {account} to stay in no group"
            );
        }
        if let Some(name) = input.get("Name").and_then(Value::as_str) {
            let names = group_names(&em).await;
            assert!(
                !names.contains(&name.to_string()),
                "case {case:?}: expected no group named {name:?} in state, got {names:?}"
            );
        }
    }
}

/// RULE (the other side of it): an account id no other group owns is ACCEPTED, and an
/// id already in the very group addressed is accepted too — the association it asks
/// for already holds.
#[tokio::test]
async fn an_account_no_other_group_owns_is_accepted() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ExclusivityAcceptPlan").await;
    let group = create_group(&em, "AcceptingGroup", &plan, None, &["111111111111"]).await;

    // Rows differ along how many free ids the call names and whether the list also
    // repeats an id THIS group already holds.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one free account",
            json!({"Arn": group, "AccountIds": ["222222222222"]}),
            vec!["111111111111", "222222222222"],
        ),
        (
            "two free accounts",
            json!({"Arn": group, "AccountIds": ["333333333333", "444444444444"]}),
            vec![
                "111111111111",
                "222222222222",
                "333333333333",
                "444444444444",
            ],
        ),
        (
            "an account already in THIS group, beside a free one",
            json!({"Arn": bare_id(&group), "AccountIds": ["111111111111", "555555555555"]}),
            vec![
                "111111111111",
                "222222222222",
                "333333333333",
                "444444444444",
                "555555555555",
            ],
        ),
    ];

    for (case, input, expected_members) in cases {
        let (status, body) = em.post("/associate-accounts", &input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        for account in &expected_members {
            assert_eq!(
                owner_of(&em, account).await.as_deref(),
                Some(group.as_str()),
                "case {case:?}: expected account {account} associated with the group"
            );
        }
    }

    // A create over accounts no group owns is served the same way.
    let fresh = create_group(
        &em,
        "SecondAcceptingGroup",
        &plan,
        Some("666666666666"),
        &["666666666666", "777777777777"],
    )
    .await;
    for account in ["666666666666", "777777777777"] {
        assert_eq!(
            owner_of(&em, account).await.as_deref(),
            Some(fresh.as_str()),
            "case \"CreateBillingGroup over free accounts\": expected {account} in the new group"
        );
    }
}

/// RULE: the two error classes are NOT interchangeable — a name already in use is the
/// inconsistent-state case `ConflictExceptionReason` enumerates
/// (`RESOURCE_NAME_CONFLICT`, HTTP 409), while a taken ACCOUNT is the input-validation
/// case above (HTTP 400). One request that breaks both is answered by whichever the
/// handler decides first, so each row breaks exactly one.
#[tokio::test]
async fn a_taken_name_is_a_conflict_and_a_taken_account_is_not() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ClassSplitPlan").await;
    create_group(&em, "TakenName", &plan, None, &["111111111111"]).await;

    // Rows: (case, request, expected status, expected error code, expected reason).
    let cases: Vec<(&str, Value, u16, &str, &str)> = vec![
        (
            "a billing-group name already in use",
            json!({
                "Name": "TakenName",
                "AccountGrouping": {"LinkedAccountIds": ["222222222222"]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
            409,
            "ConflictException",
            "RESOURCE_NAME_CONFLICT",
        ),
        (
            "a free name over an account another group owns",
            json!({
                "Name": "FreeName",
                "AccountGrouping": {"LinkedAccountIds": ["111111111111"]},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
            400,
            "ValidationException",
            "ACCOUNTS_ALREADY_ASSOCIATED",
        ),
    ];

    for (case, input, expected_status, expected_code, expected_reason) in cases {
        let (status, body) = em.post("/create-billing-group", &input).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some(expected_reason),
            "case {case:?}: expected the {expected_reason} reason, got {body:?}"
        );
    }
}
