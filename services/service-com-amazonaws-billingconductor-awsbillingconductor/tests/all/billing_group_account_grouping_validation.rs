// SPDX-License-Identifier: CC0-1.0

//! `CreateBillingGroup`'s `AccountGrouping` validation, end to end over the service's
//! real `restJson1` wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: a billing group may only be created over relationships that HOLD —
//! the account designated its main account is one of the group's own accounts
//! (`PrimaryAccountId` is "The account ID that serves as the main account in a billing
//! group", and `ValidationExceptionReason` enumerates `PRIMARY_NOT_ASSOCIATED`), and a
//! transfer relationship it bills through is one this organization has (the same
//! predicate `GetBillingTransferPreference` resolves a transfer with). A grouping that
//! fails either is refused 400 `ValidationException`, naming the member at fault, and
//! PERSISTS NOTHING; a grouping that holds is served and reads back through the
//! read-only `ListBillingGroups` / `ListAccountAssociations`.
//!
//! Rows span the class rather than one request: the size of the account list (none, one,
//! several, the modelled maximum of 30), where in it the primary account sits (first,
//! middle, last, absent, held by another group), and whether a transfer relationship
//! rides along — and, for the transfer, the owner account, the organization id, the
//! direction and the transfer id the member's own pattern admits.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The routes these rows drive, at the methods their modelled HTTP bindings name
/// (`smithy_query(query="operation",operation="CreateBillingGroup")`).
const CREATE_GROUP_PATH: &str = "/create-billing-group";
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";
const LIST_GROUPS_PATH: &str = "/list-billing-groups";
const LIST_ASSOCIATIONS_PATH: &str = "/list-account-associations";
const GET_TRANSFER_PREFERENCE_PATH: &str = "/get-billing-transfer-preference";

/// The `ValidationExceptionReason` the model enumerates for an account designated a
/// group's main account while it is not one of that group's accounts.
const PRIMARY_NOT_ASSOCIATED: &str = "PRIMARY_NOT_ASSOCIATED";

/// The reason the model enumerates for an ARN that names no such resource.
const INVALID_ARN: &str = "INVALID_ARN";

/// The largest account list the model admits
/// (`smithy_query(query="shape",shape="AccountGrouping")`: `LinkedAccountIds` length
/// 0..30).
const MAX_LINKED_ACCOUNT_IDS: usize = 30;

/// A live emulator holding only this service, with the emulated account designated the
/// organization's PAYER account — Billing Conductor's control plane is authorized for the
/// payer account alone, so this is the world in which these calls are reachable.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine the service was built on, kept so the suite can record the billing
    /// transfers the organization has — see [`Emulator::designate_transfer`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
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

    /// Record a billing transfer the organization HAS, and hand its ARN back — the shared
    /// seam [`crate::billing_transfer_identity::designate_transfer`].
    fn designate_transfer(&self, arn: &str) -> String {
        crate::billing_transfer_identity::designate_transfer(&self.engine, arn)
    }

    /// POST a JSON body to one of this service's routes and answer the status plus the
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

    /// `CreatePricingPlan`: the created plan's full ARN — a billing group's computation
    /// preference must name a plan that exists.
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

    /// The `BillingGroupListElement` the read-only lister reports for one group.
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

    /// The names state holds a billing group under, read through the read-only lister.
    async fn all_names(&self) -> Vec<String> {
        let (status, body) = self.post(LIST_GROUPS_PATH, &json!({})).await;
        assert_eq!(
            status, 200,
            "ListBillingGroups (no filter): expected 200, got {status} with {body}"
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

    /// The billing group one account is associated with, read through
    /// `ListAccountAssociations`: `Some(Some(arn))` when it is a member of a group,
    /// `Some(None)` when the service knows the account and it belongs to none
    /// (`UNMONITORED`), `None` when the service does not know the account at all.
    async fn association_of(&self, account_id: &str) -> Option<Option<String>> {
        let (status, body) = self
            .post(
                LIST_ASSOCIATIONS_PATH,
                &json!({"Filters": {"AccountId": account_id}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "ListAccountAssociations {account_id}: expected 200, got {status} with {body}"
        );
        let listed = body["LinkedAccounts"].as_array().cloned().unwrap_or_default();
        listed
            .first()
            .map(|element| element["BillingGroupArn"].as_str().map(str::to_string))
    }

    /// `GetBillingTransferPreference`: the status the reader answers for one transfer ARN,
    /// which is how this suite checks the create refuses exactly the transfers no reader
    /// of this store can resolve.
    async fn transfer_preference_status(&self, transfer_arn: &str) -> u16 {
        let (status, _) = self
            .post(
                GET_TRANSFER_PREFERENCE_PATH,
                &json!({ "ResponsibilityTransferArn": transfer_arn }),
            )
            .await;
        status
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

/// A `ResponsibilityTransferArn` of the organization whose management account is
/// `account_id`, built to the member's own pattern
/// (`^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$`,
/// `smithy_query(query="shape",shape="AccountGrouping")`).
fn transfer_arn_of(account_id: &str, org: &str, direction: &str, id: &str) -> String {
    format!("arn:aws:organizations::{account_id}:transfer/o-{org}/billing/{direction}/rt-{id}")
}

/// The `index`-th account id of this suite, as the modelled `AccountId` pattern spells one
/// (`^[0-9]{12}$`).
fn account_id(index: usize) -> String {
    format!("{:012}", 100_000_000_000_u64 + index as u64)
}

/// `index` account ids, each distinct.
fn account_ids(count: usize) -> Vec<String> {
    (0..count).map(account_id).collect()
}

/// One row of the refusal table: the request body under test and what makes it different
/// from the others.
struct Refusal {
    case: String,
    body: Value,
    /// The `ValidationExceptionReason` the refusal must carry.
    reason: &'static str,
    /// The input member the refusal must name in `Fields`.
    member: &'static str,
}

/// RULE: a `CreateBillingGroup` whose `PrimaryAccountId` is not one of
/// `AccountGrouping.LinkedAccountIds` is refused 400 `ValidationException` with the
/// `PRIMARY_NOT_ASSOCIATED` reason, naming `PrimaryAccountId`, and persists nothing — no
/// group, and no account association for either the primary or the accounts the refused
/// request listed.
#[tokio::test]
async fn a_primary_account_outside_the_grouping_is_refused_and_persists_nothing() {
    let em = Emulator::new();
    let plan = em.create_plan("GroupingRefusalPlan").await;

    // A group that already owns an account, so one row can designate an account another
    // group holds — which is not an account of the group being created either.
    let held = account_id(900);
    let (status, owner) = em
        .create(&json!({
            "Name": "OwningGroup",
            "AccountGrouping": { "LinkedAccountIds": [held] },
            "ComputationPreference": { "PricingPlanArn": plan },
            "PrimaryAccountId": held,
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup CreateBillingGroup OwningGroup: expected 200, got {status} with {owner}"
    );
    let owner_arn = owner["Arn"].as_str().unwrap_or_default().to_string();

    // Rows differ along the size of the account list the group is created with (none, one,
    // several, the modelled maximum), where the designated primary account stands with
    // respect to it (outside it, outside it but owned by another group), and the optional
    // members riding along (a client token, tags, a description, a transfer relationship).
    let many = account_ids(5);
    let maximum = account_ids(MAX_LINKED_ACCOUNT_IDS);
    let rows = vec![
        Refusal {
            case: "no accounts at all".to_string(),
            body: json!({
                "Name": "NoAccounts",
                "AccountGrouping": { "LinkedAccountIds": [] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(1),
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: "the AccountGrouping names no account list at all".to_string(),
            body: json!({
                "Name": "OmittedAccountList",
                "AccountGrouping": {},
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(2),
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: "one account, and the primary is another".to_string(),
            body: json!({
                "Name": "OneOtherAccount",
                "AccountGrouping": { "LinkedAccountIds": [account_id(3)] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(4),
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: format!("{} accounts, and the primary is none of them", many.len()),
            body: json!({
                "Name": "SeveralAccounts",
                "Description": "the group that designated an outsider",
                "AccountGrouping": { "LinkedAccountIds": many },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(99),
                "Tags": { "team": "billing" },
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: format!(
                "the modelled maximum of {MAX_LINKED_ACCOUNT_IDS} accounts, and the primary is \
                 none of them"
            ),
            body: json!({
                "Name": "MaximumAccounts",
                "AccountGrouping": { "LinkedAccountIds": maximum },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(98),
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: "the primary account is held by ANOTHER billing group".to_string(),
            body: json!({
                "Name": "BorrowedPrimary",
                "AccountGrouping": { "LinkedAccountIds": [account_id(5)] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": held,
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: "a transfer-billing group whose primary is outside its accounts".to_string(),
            body: json!({
                "Name": "TransferOutsidePrimary",
                "AccountGrouping": {
                    "LinkedAccountIds": [account_id(6)],
                    "ResponsibilityTransferArn": em.designate_transfer(&transfer_arn_of(
                        account(),
                        "abcd123456",
                        "inbound",
                        "1234abcd",
                    )),
                },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(7),
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
        Refusal {
            case: "a request carrying a client token".to_string(),
            body: json!({
                "Name": "TokenedOutsidePrimary",
                "ClientToken": "grouping-refusal-token",
                "AccountGrouping": { "LinkedAccountIds": [account_id(8)] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": account_id(9),
            }),
            reason: PRIMARY_NOT_ASSOCIATED,
            member: "PrimaryAccountId",
        },
    ];

    for row in &rows {
        let case = &row.case;
        let (status, response) = em.create(&row.body).await;
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
            Some(row.reason),
            "case {case:?}: expected Reason {:?}, got {response}",
            row.reason
        );
        assert_eq!(
            response["Fields"][0]["Name"].as_str(),
            Some(row.member),
            "case {case:?}: the refusal must name the member at fault, got {response}"
        );

        // The refusal wrote nothing: no group under that name, the designated primary is
        // unknown to the service, and so is every account the refused request listed.
        let name = row.body["Name"].as_str().unwrap_or_default().to_string();
        let names = em.all_names().await;
        assert!(
            !names.contains(&name),
            "case {case:?}: a refused create must persist no group, got {names:?}"
        );
        let primary = row.body["PrimaryAccountId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if primary != held {
            let association = em.association_of(&primary).await;
            assert_eq!(
                association, None,
                "case {case:?}: a refused create must leave its primary account unknown, got \
                 {association:?}"
            );
        }
        for listed in row.body["AccountGrouping"]["LinkedAccountIds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let member = listed.as_str().unwrap_or_default().to_string();
            let association = em.association_of(&member).await;
            assert_eq!(
                association, None,
                "case {case:?}: a refused create must associate none of the accounts it listed, \
                 got {association:?} for {member}"
            );
        }
    }

    // And the group that already owned an account still owns exactly it: no refusal moved
    // another group's membership.
    let association = em.association_of(&held).await;
    assert_eq!(
        association,
        Some(Some(owner_arn)),
        "expected the owning group to keep its account, got {association:?}"
    );
}

/// One row of the served table: the accounts the group is created with and which of them
/// it designates as its primary account.
struct Served {
    case: String,
    /// The accounts the grouping lists.
    linked: Vec<String>,
    /// The index into `linked` of the account designated the primary, if any.
    primary: Option<usize>,
}

/// RULE: a `CreateBillingGroup` whose `PrimaryAccountId` IS one of its accounts is served,
/// and the group reads back through the read-only `ListBillingGroups` with that primary
/// account, the `Size` of its membership and an operational `Status`; a create that
/// designates no primary account at all is served too, and reports the absence of a
/// billing owner.
#[tokio::test]
async fn a_primary_account_inside_the_grouping_is_served_and_reads_back() {
    let em = Emulator::new();
    let plan = em.create_plan("GroupingServedPlan").await;

    // Rows differ along the size of the account list (one, two, several, the modelled
    // maximum) and where in it the primary account stands (the only one, the first, the
    // middle, the last) — and one row designates none at all.
    let rows = [
        Served {
            case: "one account, which is the primary".to_string(),
            linked: account_ids(1),
            primary: Some(0),
        },
        Served {
            case: "two accounts, the first is the primary".to_string(),
            linked: account_ids(2).into_iter().map(shift(10)).collect(),
            primary: Some(0),
        },
        Served {
            case: "five accounts, a middle one is the primary".to_string(),
            linked: account_ids(5).into_iter().map(shift(20)).collect(),
            primary: Some(2),
        },
        Served {
            case: "five accounts, the last is the primary".to_string(),
            linked: account_ids(5).into_iter().map(shift(30)).collect(),
            primary: Some(4),
        },
        Served {
            case: format!("the modelled maximum of {MAX_LINKED_ACCOUNT_IDS} accounts"),
            linked: account_ids(MAX_LINKED_ACCOUNT_IDS)
                .into_iter()
                .map(shift(40))
                .collect(),
            primary: Some(MAX_LINKED_ACCOUNT_IDS - 1),
        },
        Served {
            case: "accounts but no primary account designated".to_string(),
            linked: account_ids(2).into_iter().map(shift(80)).collect(),
            primary: None,
        },
        Served {
            case: "no accounts and no primary account designated".to_string(),
            linked: Vec::new(),
            primary: None,
        },
    ];


    for (index, row) in rows.iter().enumerate() {
        let case = &row.case;
        let name = format!("ServedGroup{index}");
        let mut body = json!({
            "Name": name,
            "AccountGrouping": { "LinkedAccountIds": row.linked },
            "ComputationPreference": { "PricingPlanArn": plan },
        });
        let primary = row.primary.map(|at| row.linked[at].clone());
        if let Some(primary) = &primary {
            body["PrimaryAccountId"] = json!(primary);
        }
        let (status, response) = em.create(&body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
        let arn = response["Arn"].as_str().unwrap_or_default().to_string();

        // The round trip: the group reads back out of state with the primary account it
        // designated, the membership it listed and the status its billing owner implies.
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["PrimaryAccountId"].as_str(),
            primary.as_deref(),
            "case {case:?}: expected the designated primary account {primary:?}, got {listed}"
        );
        let expected_size = i64::try_from(row.linked.len()).unwrap_or(i64::MAX);
        assert_eq!(
            listed["Size"].as_i64(),
            Some(expected_size),
            "case {case:?}: expected Size {expected_size}, got {listed}"
        );
        let expected_status = if primary.is_some() {
            "ACTIVE"
        } else {
            "PRIMARY_ACCOUNT_MISSING"
        };
        assert_eq!(
            listed["Status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected Status {expected_status:?}, got {listed}"
        );

        // Every account it listed — the primary among them — is associated with exactly
        // this group, so the group's main account is never reported outside it.
        for member in &row.linked {
            let association = em.association_of(member).await;
            assert_eq!(
                association,
                Some(Some(arn.clone())),
                "case {case:?}: expected account {member} in {arn}, got {association:?}"
            );
        }
    }
}

/// Add `offset` to the numeric part of an account id, so each row of a table owns a
/// distinct set of accounts (an account belongs to at most one billing group).
fn shift(offset: usize) -> impl Fn(String) -> String {
    move |id: String| {
        let value: u64 = id.parse().unwrap_or_default();
        format!("{:012}", value + offset as u64)
    }
}

/// RULE: a `CreateBillingGroup` naming an `AccountGrouping.ResponsibilityTransferArn`
/// that is NOT a billing transfer of this organization is refused 400
/// `ValidationException` naming that member and persists nothing — the same transfers the
/// read-only `GetBillingTransferPreference` answers 404 for; one that IS this
/// organization's is served and reads back as a `TRANSFER_BILLING` group.
#[tokio::test]
async fn a_transfer_relationship_outside_this_organization_is_refused() {
    let em = Emulator::new();
    let plan = em.create_plan("TransferGroupingPlan").await;
    // One transfer the organization really has, so every refused row below differs from a
    // transfer that exists in exactly one segment of the ARN.
    let held = em.designate_transfer(&transfer_arn_of(
        account(),
        "abcd123456",
        "inbound",
        "1234abcd",
    ));

    // Rows differ along the segment of the transfer ARN that makes it name no transfer of
    // this organization: the owner account (another account of the same length, an account
    // nobody holds), the organization id, the direction and the transfer id — every
    // segment the member's pattern spells is part of the transfer's identity, so a change
    // in any ONE of them names a transfer the organization does not have.
    let foreign = [
        (
            "another organization's management account",
            transfer_arn_of("210987654321", "abcd123456", "inbound", "1234abcd"),
        ),
        (
            "an outbound transfer of another account",
            transfer_arn_of("999988887777", "zzzz999999", "outbound", "9999zzzz"),
        ),
        (
            "a long organization id and a long transfer id",
            transfer_arn_of(
                "123456789012",
                "abcdefghij0123456789abcdefghij01",
                "inbound",
                "0123456789abcdefghij0123456789ab",
            ),
        ),
        (
            "this organization, but a transfer id nobody ever created",
            transfer_arn_of(account(), "abcd123456", "inbound", "zzzzzzzz"),
        ),
        (
            "the held transfer's id under another organization of this account",
            transfer_arn_of(account(), "abcd123457", "inbound", "1234abcd"),
        ),
        (
            "the held transfer with its direction flipped",
            transfer_arn_of(account(), "abcd123456", "outbound", "1234abcd"),
        ),
    ];


    for (index, (case, transfer)) in foreign.iter().enumerate() {
        let name = format!("ForeignTransferGroup{index}");
        let (status, response) = em
            .create(&json!({
                "Name": name,
                "AccountGrouping": { "ResponsibilityTransferArn": transfer },
                "ComputationPreference": { "PricingPlanArn": plan },
            }))
            .await;
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
            Some(INVALID_ARN),
            "case {case:?}: expected Reason {INVALID_ARN:?}, got {response}"
        );
        assert_eq!(
            response["Fields"][0]["Name"].as_str(),
            Some("AccountGrouping.ResponsibilityTransferArn"),
            "case {case:?}: the refusal must name the member at fault, got {response}"
        );
        let names = em.all_names().await;
        assert!(
            !names.contains(&name),
            "case {case:?}: a refused create must persist no group, got {names:?}"
        );

        // The create refuses exactly the transfers no reader of this store can resolve.
        let reader = em.transfer_preference_status(transfer).await;
        assert_eq!(
            reader, 404,
            "case {case:?}: the transfer the create refused must be one the reader does not \
             hold, got {reader}"
        );
    }

    // A transfer of THIS organization is served, whatever direction, organization id and
    // transfer id it carries — and the group reads back as a transfer-billing group whose
    // billing owner is the relationship rather than an account.
    let owned = [
        ("the transfer this organization already held", held),
        (
            "another inbound transfer of this organization",
            em.designate_transfer(&transfer_arn_of(
                account(),
                "abcd123456",
                "inbound",
                "5678abcd",
            )),
        ),
        (
            "an outbound transfer of this organization",
            em.designate_transfer(&transfer_arn_of(
                account(),
                "zzzz999999",
                "outbound",
                "beef1234",
            )),
        ),
    ];

    for (index, (case, transfer)) in owned.iter().enumerate() {
        let name = format!("OwnedTransferGroup{index}");
        let (status, response) = em
            .create(&json!({
                "Name": name,
                "AccountGrouping": { "ResponsibilityTransferArn": transfer },
                "ComputationPreference": { "PricingPlanArn": plan },
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
        let arn = response["Arn"].as_str().unwrap_or_default().to_string();
        let listed = em.listed(&arn).await;
        assert_eq!(
            listed["AccountGrouping"]["ResponsibilityTransferArn"].as_str(),
            Some(transfer.as_str()),
            "case {case:?}: expected the transfer relationship to read back, got {listed}"
        );
        assert_eq!(
            listed["BillingGroupType"].as_str(),
            Some("TRANSFER_BILLING"),
            "case {case:?}: a group billing through a transfer is TRANSFER_BILLING, got {listed}"
        );
        let reader = em.transfer_preference_status(transfer).await;
        assert_eq!(
            reader, 200,
            "case {case:?}: the transfer the create accepted must be one the reader holds, got \
             {reader}"
        );
    }
}
