// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisassociateAccounts`, the inverse of `AssociateAccounts`.
//!
//! RULE under test: `DisassociateAccounts` removes exactly the account ids the
//! request names from the GIVEN billing group, so the group's `Size` drops by the
//! number of members it took out, each removed account is released by the
//! account-association index (`ListAccountAssociations` reports it `UNMONITORED` and
//! another group may then take it), and the response echoes the group's ARN. A
//! request that names an account the group does not hold is refused as an INPUT
//! fault with the modelled 400 `ValidationException` and the reason
//! `ValidationExceptionReason` enumerates for it
//! (`smithy_query(query="shape",shape="ValidationExceptionReason")`):
//! `ACCOUNTS_NOT_ASSOCIATED` for a non-member, `PRIMARY_CANNOT_DISASSOCIATE` for the
//! group's own primary account and `DUPLICATE_ACCOUNT` for an id listed twice; a
//! group that is not stored is a 404 `ResourceNotFoundException`. Every refusal
//! changes nothing.
//!
//! The rows span what the rule generalises over rather than one call: how many
//! accounts one call removes (0 — the lower modelled bound of `AccountIdList` — 1, 2
//! and 3), which members they are and in which order they are listed, whether the
//! group is named by full ARN or by the bare id its pattern also accepts, and — for
//! the refusals — whether the account at fault is in no group, in another group, or
//! the primary account, and where in the list it sits.
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
    /// designated payer is what makes this surface reachable at all. The denial every
    /// other caller gets is asserted in `tests/all/payer_account_gate.rs`.
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

/// The bare resource id of a billing-group ARN — the other spelling the modelled
/// `BillingGroupArn` pattern accepts for the same group.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
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
    plan: &str,
    primary: Option<&str>,
    linked: &[&str],
) -> String {
    let mut input = json!({
        "Name": name,
        "AccountGrouping": {"LinkedAccountIds": linked},
        "ComputationPreference": {"PricingPlanArn": plan},
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

/// The stored `Size` of one billing group, read back through `ListBillingGroups`.
async fn group_size(em: &Emulator, group: &str) -> i64 {
    let (status, body) = em
        .post("/list-billing-groups", &json!({"Filters": {"Arns": [group]}}))
        .await;
    assert_eq!(
        status, 200,
        "ListBillingGroups {group}: expected 200, got {status} with {body:?}"
    );
    let listed = body["BillingGroups"].as_array().cloned().unwrap_or_default();
    let element = listed.first().cloned().unwrap_or(Value::Null);
    element["Size"]
        .as_i64()
        .unwrap_or_else(|| panic!("ListBillingGroups {group}: no Size in {element:?}"))
}

/// The billing group one account is associated with, read back through
/// `ListAccountAssociations`: `Some(arn)` when it is a member of a group, `None`
/// when the service knows the account but it belongs to none (`UNMONITORED`).
async fn association_of(em: &Emulator, account_id: &str) -> Option<String> {
    let (status, body) = em
        .post(
            "/list-account-associations",
            &json!({"Filters": {"AccountId": account_id}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ListAccountAssociations {account_id}: expected 200, got {status} with {body:?}"
    );
    let listed = body["LinkedAccounts"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "ListAccountAssociations {account_id}: expected exactly one element, got {listed:?}"
    );
    listed[0]["BillingGroupArn"].as_str().map(str::to_string)
}

/// Every account id one billing group holds, read back through the `Association`
/// filter that selects a single group's members.
async fn members_of(em: &Emulator, group: &str) -> Vec<String> {
    let (status, body) = em
        .post(
            "/list-account-associations",
            &json!({"Filters": {"Association": group}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ListAccountAssociations for {group}: expected 200, got {status} with {body:?}"
    );
    body["LinkedAccounts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|element| element["AccountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: `DisassociateAccounts` takes exactly the named accounts out of the given
/// group, so its `Size`, its membership listing and each account's association all
/// move together; the response echoes the group's ARN whichever spelling named it.
#[tokio::test]
async fn disassociate_accounts_removes_exactly_the_named_members() {
    let em = Emulator::new();
    let plan = create_plan(&em, "DisassociatePlan").await;
    let accounts: Vec<String> = (1..=7).map(|n| format!("{n:012}")).collect();
    let linked: Vec<&str> = accounts.iter().map(String::as_str).collect();
    let group = create_group(
        &em,
        "DisassociateGroup",
        &plan,
        Some("999988887777"),
        &linked,
    )
    .await;
    let bare = bare_id(&group);

    // Rows differ along how many accounts one call removes (0 is the lower modelled
    // bound of `AccountIdList`), which members they are, the order they are listed
    // in, and which spelling of `Arn` names the group. The expected size is
    // cumulative over the 7 members set up above.
    let cases: Vec<(&str, &str, Vec<&str>, i64)> = vec![
        (
            "no accounts at all (the lower modelled bound)",
            group.as_str(),
            vec![],
            7,
        ),
        ("one member", group.as_str(), vec!["000000000001"], 6),
        (
            "two members in one call",
            group.as_str(),
            vec!["000000000002", "000000000003"],
            4,
        ),
        (
            "three members, listed in reverse order",
            group.as_str(),
            vec!["000000000006", "000000000005", "000000000004"],
            1,
        ),
        ("the group named by its bare id", bare.as_str(), vec!["000000000007"], 0),
    ];

    let mut released: Vec<String> = Vec::new();
    for (case, arn, removed, expected_size) in cases {
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": arn, "AccountIds": removed}),
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

        let size = group_size(&em, &group).await;
        assert_eq!(
            size, expected_size,
            "case {case:?}: expected Size {expected_size}, got {size}"
        );

        for account_id in &removed {
            let association = association_of(&em, account_id).await;
            assert_eq!(
                association, None,
                "case {case:?}: expected account {account_id} to belong to no group, got {association:?}"
            );
            released.push((*account_id).to_string());
        }

        let mut members = members_of(&em, &group).await;
        members.sort();
        let mut expected: Vec<String> = accounts
            .iter()
            .filter(|account_id| !released.contains(account_id))
            .cloned()
            .collect();
        expected.sort();
        assert_eq!(
            members, expected,
            "case {case:?}: expected the group to hold {expected:?}, got {members:?}"
        );
    }
}

/// RULE: a disassociated account is genuinely released — the one-group-per-account
/// index no longer names the group it left, so any group (including the one it came
/// from) may associate it again.
#[tokio::test]
async fn a_disassociated_account_can_be_associated_again() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ReassociatePlan").await;
    let first = create_group(
        &em,
        "ReassociateFirst",
        &plan,
        Some("999988887777"),
        &["111122223333", "222233334444"],
    )
    .await;
    let second = create_group(&em, "ReassociateSecond", &plan, None, &[]).await;

    // Rows differ along which group takes the released account next: the one it came
    // from, and a different one.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("re-associated to the group it left", "111122223333", first.as_str()),
        ("associated to a different group", "222233334444", second.as_str()),
    ];

    for (case, account_id, next_group) in cases {
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": first, "AccountIds": [account_id]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the disassociation to succeed, got {status} with {body:?}"
        );

        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": next_group, "AccountIds": [account_id]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-association to succeed, got {status} with {body:?}"
        );
        let association = association_of(&em, account_id).await;
        assert_eq!(
            association.as_deref(),
            Some(next_group),
            "case {case:?}: expected account {account_id} in {next_group}, got {association:?}"
        );
    }
}

/// RULE: a request naming an account the given group does not hold is refused 400
/// `ValidationException` with `ACCOUNTS_NOT_ASSOCIATED`, and changes nothing.
#[tokio::test]
async fn an_account_the_group_does_not_hold_is_refused() {
    let em = Emulator::new();
    let plan = create_plan(&em, "NotAssociatedPlan").await;
    let group = create_group(
        &em,
        "NotAssociatedGroup",
        &plan,
        Some("999988887777"),
        &["111122223333", "222233334444", "777788889999"],
    )
    .await;
    let other = create_group(&em, "NotAssociatedOther", &plan, None, &["333344445555"]).await;

    // Rows differ along why the account is not a member (never seen, released into
    // no group, held by ANOTHER group) and where in the list it sits (only, first,
    // last, middle).
    let (status, body) = em
        .post(
            "/disassociate-accounts",
            &json!({"Arn": group, "AccountIds": ["222233334444"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected the release of 222233334444 to succeed, got {status} with {body:?}"
    );

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("an account the service has never seen", vec!["444455556666"]),
        (
            "an account released into no group",
            vec!["222233334444"],
        ),
        (
            "an account another group holds",
            vec!["333344445555"],
        ),
        (
            "a member first, a non-member last",
            vec!["111122223333", "444455556666"],
        ),
        (
            "a non-member first, a member last",
            vec!["444455556666", "111122223333"],
        ),
        (
            "a non-member in the middle of members",
            vec!["111122223333", "333344445555", "777788889999"],
        ),
    ];

    for (case, requested) in cases {
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": group, "AccountIds": requested}),
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
        assert_eq!(
            body["Reason"].as_str(),
            Some("ACCOUNTS_NOT_ASSOCIATED"),
            "case {case:?}: expected the ACCOUNTS_NOT_ASSOCIATED reason, got {body:?}"
        );
        assert_eq!(
            field_names(&body),
            vec!["AccountIds".to_string()],
            "case {case:?}: expected AccountIds named in Fields, got {body:?}"
        );
    }

    // Nothing moved: the group still holds its two remaining members, the released
    // account still belongs to no group, and the other group keeps its own.
    let mut members = members_of(&em, &group).await;
    members.sort();
    assert_eq!(
        members,
        vec!["111122223333".to_string(), "777788889999".to_string()],
        "expected the refused calls to leave the membership alone, got {members:?}"
    );
    let size = group_size(&em, &group).await;
    assert_eq!(
        size, 2,
        "expected the refused calls to leave Size at 2, got {size}"
    );
    let association = association_of(&em, "222233334444").await;
    assert_eq!(
        association, None,
        "expected the released account to stay in no group, got {association:?}"
    );
    let association = association_of(&em, "333344445555").await;
    assert_eq!(
        association.as_deref(),
        Some(other.as_str()),
        "expected the other group to keep its member, got {association:?}"
    );
}

/// RULE: the group's own primary account cannot be taken out of it — the
/// `PRIMARY_CANNOT_DISASSOCIATE` reason the model enumerates — and the refusal
/// changes nothing.
#[tokio::test]
async fn the_primary_account_of_the_group_cannot_be_disassociated() {
    let em = Emulator::new();
    let plan = create_plan(&em, "PrimaryPlan").await;
    // Rows differ along whether the primary account is also a linked member of the
    // group it is the primary account of, and on the group's other membership.
    let member_primary = create_group(
        &em,
        "PrimaryIsMember",
        &plan,
        Some("111122223333"),
        &["111122223333", "222233334444"],
    )
    .await;
    let loose_primary =
        create_group(&em, "PrimaryIsNotMember", &plan, Some("555566667777"), &[]).await;

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "the primary account, which is also a member",
            member_primary.as_str(),
            vec!["111122223333"],
        ),
        (
            "the primary account beside a plain member",
            member_primary.as_str(),
            vec!["222233334444", "111122223333"],
        ),
        (
            "a primary account that is not a member of its group",
            loose_primary.as_str(),
            vec!["555566667777"],
        ),
    ];

    for (case, group, requested) in cases {
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": group, "AccountIds": requested}),
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
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRIMARY_CANNOT_DISASSOCIATE"),
            "case {case:?}: expected the PRIMARY_CANNOT_DISASSOCIATE reason, got {body:?}"
        );
    }

    // The refusals changed nothing: both accounts of the first group are still its
    // members.
    let mut members = members_of(&em, &member_primary).await;
    members.sort();
    assert_eq!(
        members,
        vec!["111122223333".to_string(), "222233334444".to_string()],
        "expected the refused calls to leave the membership alone, got {members:?}"
    );
}

/// RULE: a request that names the same account twice is refused with the
/// `DUPLICATE_ACCOUNT` reason, and one that names a group the service does not hold
/// is a 404 `ResourceNotFoundException` — whichever spelling of `Arn` names it.
#[tokio::test]
async fn a_duplicate_account_and_an_unknown_group_are_refused() {
    let em = Emulator::new();
    let plan = create_plan(&em, "RefusalPlan").await;
    let group = create_group(
        &em,
        "RefusalGroup",
        &plan,
        Some("999988887777"),
        &["111122223333", "222233334444"],
    )
    .await;

    // Rows differ along how many ids the list holds and where the repeat sits.
    let duplicates: Vec<(&str, Vec<&str>)> = vec![
        ("one account listed twice", vec!["111122223333", "111122223333"]),
        (
            "one account repeated among others",
            vec!["111122223333", "222233334444", "111122223333"],
        ),
    ];
    for (case, requested) in duplicates {
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": group, "AccountIds": requested}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("DUPLICATE_ACCOUNT"),
            "case {case:?}: expected the DUPLICATE_ACCOUNT reason, got {body:?}"
        );
    }

    // Rows differ along which form of ARN names the absent group: the full ARN and
    // the bare id the pattern also accepts, at both ends of its 10..12 length.
    let unknown_groups: Vec<(&str, &str)> = vec![
        (
            "a full ARN naming no stored group",
            "arn:aws:billingconductor::000000000000:billinggroup/ZZZZZZZZZZZZ",
        ),
        ("a 10-character bare id naming no stored group", "ZZZZZZZZZZ"),
        ("a 12-character bare id naming no stored group", "ZZZZZZZZZZZZ"),
    ];
    for (case, arn) in unknown_groups {
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": arn, "AccountIds": ["111122223333"]}),
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

    // Every refusal left the group as it was.
    let mut members = members_of(&em, &group).await;
    members.sort();
    assert_eq!(
        members,
        vec!["111122223333".to_string(), "222233334444".to_string()],
        "expected the refused calls to leave the membership alone, got {members:?}"
    );
}
