// SPDX-License-Identifier: CC0-1.0

//! The one-account-one-billing-group refusal, asserted across the whole modelled
//! span of the account-id list that carries it.
//!
//! RULE under test: an account belongs to at most one billing group, so a request
//! whose account-id list names an account CURRENTLY associated with a different
//! billing group is an input-validation failure — HTTP 400, error code
//! `ValidationException`, `Reason = ACCOUNTS_ALREADY_ASSOCIATED` (the value
//! `smithy_query(query="shape",shape="ValidationExceptionReason")` enumerates for
//! this case; no value of `ConflictExceptionReason` describes it) and the input
//! member that carried the id in `Fields` — and it writes nothing. The refusal is
//! about who owns the account at the moment of the call, not about the id itself: an
//! account released by `DisassociateAccounts` is accepted again, and the group that
//! held it is then refused in its turn.
//!
//! `tests/all/account_exclusivity_refusal.rs` spans the OPERATIONS and members the
//! rule reaches; the rows here span the SIZE and shape of the list instead, over the
//! whole modelled range of `AccountIdList` (`@length(0..30)`, per
//! `smithy_query(query="shape",shape="AccountIdList")`): the empty list, 1, 2, 15 and
//! the maximum 30 ids, with the already-owned id first, in the middle and last, and
//! two different owning groups. The accepting side is asserted at the same bounds, so
//! the maximum the model can express is served rather than refused.
//!
//! Each suite in `tests/all/` is a private module of the single `all` test target, so
//! a sibling's harness is not reachable from here; the few lines of wire plumbing
//! below are local for that reason, as they are in every other suite.
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

/// The modelled maximum of `AccountIdList`
/// (`smithy_query(query="shape",shape="AccountIdList")`: len 0..30) — the largest
/// list one `AssociateAccounts` call can carry.
const MAX_ACCOUNT_IDS_PER_CALL: usize = 30;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's payer account — Billing Conductor's control plane is authorized
    /// for the payer account alone, so a designated payer is what makes this surface
    /// reachable at all (the denial every other caller gets is asserted in
    /// `tests/all/payer_account_gate.rs`).
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

/// A 12-digit account id (`AccountId`: `^[0-9]{12}$`) from a small ordinal.
fn account(n: usize) -> String {
    format!("{n:012}")
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

async fn create_group(em: &Emulator, name: &str, plan: &str, linked: &[String]) -> String {
    let (status, body) = em
        .post(
            "/create-billing-group",
            &json!({
                "Name": name,
                "AccountGrouping": {"LinkedAccountIds": linked},
                "ComputationPreference": {"PricingPlanArn": plan},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup CreateBillingGroup {name:?}: expected 200, got {status} with {body:?}"
    );
    body["Arn"]
        .as_str()
        .expect("setup CreateBillingGroup: no Arn in the response")
        .to_string()
}

/// The billing group an account is associated with, read back through
/// `ListAccountAssociations`: `None` when it belongs to no group (or the service has
/// never seen it).
async fn owner_of(em: &Emulator, account_id: &str) -> Option<String> {
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

/// A list of `size` account ids, free except for `taken` placed at index `position`.
fn list_with_taken_id(size: usize, position: usize, taken: &str) -> Vec<String> {
    // The filler ids start at 100 so they never collide with the owned ones set up
    // below; they are free, so a served call would associate them — which is what the
    // "nothing was written" assertions check did not happen.
    let mut ids: Vec<String> = (0..size - 1).map(|n| account(100 + n)).collect();
    ids.insert(position, taken.to_string());
    ids
}

/// RULE: whatever the size of the list and wherever in it the already-owned id sits,
/// the answer is 400 `ValidationException` / `ACCOUNTS_ALREADY_ASSOCIATED` naming
/// `AccountIds`, and nothing is written.
#[tokio::test]
async fn an_account_another_group_owns_is_refused_at_every_modelled_list_size() {
    let em = Emulator::new();
    let plan = create_plan(&em, "ListBoundsPlan").await;
    let first_owner = create_group(&em, "FirstOwner", &plan, &[account(1), account(2)]).await;
    let second_owner = create_group(&em, "SecondOwner", &plan, &[account(3)]).await;
    let target = create_group(&em, "BoundsTarget", &plan, &[]).await;

    // Rows: (case, list size, where the owned id sits, the owned id, its owner). The
    // sizes walk the modelled range of `AccountIdList` from the smallest non-empty
    // list to its published maximum, and the position walks first / middle / last.
    let cases: Vec<(&str, usize, usize, String, &str)> = vec![
        ("the only id in the list", 1, 0, account(1), &first_owner),
        ("first of two", 2, 0, account(2), &first_owner),
        ("last of two", 2, 1, account(3), &second_owner),
        ("middle of fifteen", 15, 7, account(1), &first_owner),
        (
            "first of the maximum the model allows",
            MAX_ACCOUNT_IDS_PER_CALL,
            0,
            account(3),
            &second_owner,
        ),
        (
            "middle of the maximum the model allows",
            MAX_ACCOUNT_IDS_PER_CALL,
            MAX_ACCOUNT_IDS_PER_CALL / 2,
            account(2),
            &first_owner,
        ),
        (
            "last of the maximum the model allows",
            MAX_ACCOUNT_IDS_PER_CALL,
            MAX_ACCOUNT_IDS_PER_CALL - 1,
            account(1),
            &first_owner,
        ),
    ];

    for (case, size, position, taken, owner) in cases {
        let ids = list_with_taken_id(size, position, &taken);
        assert_eq!(
            ids.len(),
            size,
            "case {case:?}: expected a list of {size} ids, got {ids:?}"
        );
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": target, "AccountIds": ids}),
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
            Some("ACCOUNTS_ALREADY_ASSOCIATED"),
            "case {case:?}: expected the ACCOUNTS_ALREADY_ASSOCIATED reason, got {body:?}"
        );
        assert_eq!(
            field_names(&body),
            vec!["AccountIds".to_string()],
            "case {case:?}: expected AccountIds named in Fields, got {body:?}"
        );
        // The message's variable parts come from the request and the stored state: the
        // account the list named and the group that holds it.
        let message = body["Message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(&taken) && message.contains(owner),
            "case {case:?}: expected the message to name {taken} and {owner}, got {message:?}"
        );

        // Nothing was written: the taken account stayed with its owner, no free id in
        // the refused list was associated, and the addressed group is still empty.
        let actual = owner_of(&em, &taken).await;
        assert_eq!(
            actual.as_deref(),
            Some(owner),
            "case {case:?}: expected account {taken} to stay with {owner}, got {actual:?}"
        );
        for free in ids.iter().filter(|id| id.as_str() != taken.as_str()) {
            let actual = owner_of(&em, free).await;
            assert_eq!(
                actual, None,
                "case {case:?}: expected account {free} to stay in no group, got {actual:?}"
            );
        }
        let size = group_size(&em, &target).await;
        assert_eq!(
            size, 0,
            "case {case:?}: expected the addressed group to stay empty, got Size {size}"
        );
    }
}

/// RULE (the accepting side, at the same bounds): a list of ids no other group owns is
/// served — including the empty list the model's lower bound allows, the maximum it
/// allows, and a list repeating ids the addressed group already holds.
#[tokio::test]
async fn a_list_of_free_ids_is_served_at_both_modelled_bounds() {
    let em = Emulator::new();
    let plan = create_plan(&em, "BoundsAcceptPlan").await;
    let group = create_group(&em, "BoundsAccepting", &plan, &[]).await;

    let full: Vec<String> = (0..MAX_ACCOUNT_IDS_PER_CALL).map(account).collect();
    // Rows: (case, the ids the call names, the group's expected Size afterwards). They
    // walk the modelled bounds of `AccountIdList`: 0, the maximum, and the maximum
    // again — the second call asks for associations that already hold, so it adds
    // nothing.
    let cases: Vec<(&str, Vec<String>, i64)> = vec![
        ("the empty list (the modelled lower bound)", vec![], 0),
        (
            "the maximum number of ids the model allows",
            full.clone(),
            MAX_ACCOUNT_IDS_PER_CALL as i64,
        ),
        (
            "the same maximum list again, all of it already in THIS group",
            full.clone(),
            MAX_ACCOUNT_IDS_PER_CALL as i64,
        ),
        (
            "one id already in THIS group beside one free id",
            vec![account(0), account(500)],
            MAX_ACCOUNT_IDS_PER_CALL as i64 + 1,
        ),
    ];

    for (case, ids, expected_size) in cases {
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": group, "AccountIds": ids}),
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
        for id in &ids {
            let actual = owner_of(&em, id).await;
            assert_eq!(
                actual.as_deref(),
                Some(group.as_str()),
                "case {case:?}: expected account {id} in the addressed group, got {actual:?}"
            );
        }
    }
}

/// RULE: the refusal is keyed to CURRENT ownership rather than to the account id — a
/// released account is accepted again, and the group that held it is refused in its
/// turn once another group has taken it.
#[tokio::test]
async fn the_refusal_tracks_current_ownership_rather_than_the_account_id() {
    let em = Emulator::new();
    let plan = create_plan(&em, "OwnershipFlipPlan").await;

    // Rows differ along how many ids travel together (one id alone, two at once) and
    // on which group holds them first.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("one account moved between groups", vec![account(11)]),
        (
            "two accounts moved together",
            vec![account(12), account(13)],
        ),
    ];

    for (row, (case, ids)) in cases.into_iter().enumerate() {
        let holder = create_group(&em, &format!("FlipHolder{row}"), &plan, &ids).await;
        let taker = create_group(&em, &format!("FlipTaker{row}"), &plan, &[]).await;

        // While the first group holds them, the second group is refused.
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": taker, "AccountIds": ids}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 while the first group holds the accounts, \
             got {status} with {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("ACCOUNTS_ALREADY_ASSOCIATED"),
            "case {case:?}: expected the ACCOUNTS_ALREADY_ASSOCIATED reason, got {body:?}"
        );

        // Released, the very same ids are accepted.
        let (status, body) = em
            .post(
                "/disassociate-accounts",
                &json!({"Arn": holder, "AccountIds": ids}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the release to succeed, got {status} with {body:?}"
        );
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": taker, "AccountIds": ids}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the released ids to be accepted, got {status} with {body:?}"
        );
        for id in &ids {
            let actual = owner_of(&em, id).await;
            assert_eq!(
                actual.as_deref(),
                Some(taker.as_str()),
                "case {case:?}: expected account {id} in the second group, got {actual:?}"
            );
        }

        // And the group that used to hold them is now the one refused.
        let (status, body) = em
            .post(
                "/associate-accounts",
                &json!({"Arn": holder, "AccountIds": ids}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the former owner to be refused in its turn, \
             got {status} with {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("ACCOUNTS_ALREADY_ASSOCIATED"),
            "case {case:?}: expected the ACCOUNTS_ALREADY_ASSOCIATED reason, got {body:?}"
        );
        let message = body["Message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(&taker),
            "case {case:?}: expected the message to name the current owner {taker}, \
             got {message:?}"
        );
    }
}
