// SPDX-License-Identifier: CC0-1.0

//! Which accounts a billing group may hold, end to end over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: a billing group groups the accounts of ONE consolidated billing
//! family, so an operation that takes account ids into a group's membership takes only
//! accounts of the family the store records — `AssociateAccounts` "connects an array of
//! account IDs in a consolidated billing family to a predefined billing group. The
//! account IDs must be a part of the consolidated billing family during the current
//! month, and not already associated with another billing group", and
//! `AccountGrouping.LinkedAccountIds` repeats the first of those preconditions
//! ("Account IDs must be a part of the consolidated billing family, and not associated
//! with another billing group"). A request naming an account the family does not hold is
//! refused HTTP 400 `ValidationException` with the `ILLEGAL_ACCOUNTS` reason the model
//! enumerates for an account list this service may not take
//! (`smithy_query(query="shape",shape="ValidationExceptionReason")`), naming the member
//! that carried it, and PERSISTS NOTHING: the group's `Size` does not move and
//! `ListAccountAssociations` reports no association for the refused account. Every
//! account the family DOES hold is taken, reads back through `ListBillingGroups` /
//! `ListAccountAssociations`, and is a linked account this service reports whether or not
//! a billing group holds it.
//!
//! Which accounts a family holds is an AWS Organizations fact that no Billing Conductor
//! request writes, so the roster is recorded in the store the way the organization's payer
//! account is ([`record_billing_family`], beside `payer_account_id`); a store that records
//! no roster knows of no account that it is outside the family, and the last row set pins
//! that it refuses none on that ground.
//!
//! Rows span the class rather than one request: the size of the recorded family (none,
//! one, several), the operation and the member the ids travel in (`AssociateAccounts`'s
//! `AccountIds`, `CreateBillingGroup`'s `AccountGrouping.LinkedAccountIds`), how many
//! accounts one request names (one, two, three, four), how many of them are outside the
//! family, and where in the list they sit (first, between, last) — including an id one
//! digit from a family account, so the decision is membership and not resemblance.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The routes these rows drive, at the methods their modelled HTTP bindings name
/// (`smithy_query(query="operation",operation="AssociateAccounts,CreateBillingGroup")`).
const ASSOCIATE_PATH: &str = "/associate-accounts";
const CREATE_GROUP_PATH: &str = "/create-billing-group";
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";
const LIST_GROUPS_PATH: &str = "/list-billing-groups";
const LIST_ASSOCIATIONS_PATH: &str = "/list-account-associations";

/// The `ValidationExceptionReason` the model enumerates for an account list this service
/// may not take.
const ILLEGAL_ACCOUNTS: &str = "ILLEGAL_ACCOUNTS";

/// Account ids of the recorded family, and ids outside it: twelve digits, as `AccountId`'s
/// modelled `^[0-9]{12}$` requires.
const FAMILY_A: &str = "111122223333";
const FAMILY_B: &str = "222233334444";
const FAMILY_C: &str = "333344445555";
const FAMILY_D: &str = "444455556666";
const FAMILY_E: &str = "555566667777";
const FAMILY_F: &str = "666677778888";
const OUTSIDE_A: &str = "999999999999";
const OUTSIDE_B: &str = "888888888888";
/// One digit away from [`FAMILY_A`]: membership is decided by the roster, not by how
/// closely an id resembles one of its accounts.
const OUTSIDE_NEAR_FAMILY_A: &str = "111122223334";

/// Record the linked accounts of the consolidated billing family a store's organization
/// has — the seam this suite shares with any other that needs a family, as
/// `crate::billing_transfer_identity::designate_transfer` shares the billing transfers.
///
/// An account joins a consolidated billing family in AWS Organizations, which this
/// emulator does not serve, so no request of this service can record one.
pub(crate) fn record_billing_family(engine: &Arc<engine::InMemoryEngine>, accounts: &[&str]) {
    let roster: BTreeSet<String> = accounts.iter().map(|id| (*id).to_string()).collect();
    engine
        .state::<BillingConductorState>()
        .lock()
        .billing_family_account_ids = Some(roster);
}

/// A live emulator holding only this service, with the emulated account designated the
/// organization's PAYER account — Billing Conductor's control plane is authorized for the
/// payer account alone, so this is the world in which these calls are reachable.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine the service was built on, kept so a row can record the consolidated
    /// billing family's roster.
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

    /// Record the family's roster for this store.
    fn record_family(&self, accounts: &[&str]) {
        record_billing_family(&self.engine, accounts);
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
        let (status, body) = self.post(CREATE_PLAN_PATH, &json!({"Name": name})).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name}: expected 200, got {status} with {body}"
        );
        body["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `CreateBillingGroup` over `accounts`, asserted to succeed: the created group's ARN.
    async fn create_group(
        &self,
        name: &str,
        plan_arn: &str,
        accounts: &[&str],
        primary: Option<&str>,
    ) -> String {
        let mut body = json!({
            "Name": name,
            "AccountGrouping": {"LinkedAccountIds": accounts},
            "ComputationPreference": {"PricingPlanArn": plan_arn},
        });
        if let Some(primary) = primary {
            body["PrimaryAccountId"] = json!(primary);
        }
        let (status, created) = self.post(CREATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {name}: expected 200, got {status} with {created}"
        );
        created["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// The `BillingGroups` page of a `ListBillingGroups` read.
    async fn list_groups(&self) -> Vec<Value> {
        let (status, body) = self.post(LIST_GROUPS_PATH, &json!({})).await;
        assert_eq!(
            status, 200,
            "ListBillingGroups: expected 200, got {status} with {body}"
        );
        body["BillingGroups"].as_array().cloned().unwrap_or_default()
    }

    /// `Size` — the number of accounts in the group `group_arn` names.
    async fn group_size(&self, group_arn: &str) -> i64 {
        self.list_groups()
            .await
            .into_iter()
            .find(|group| group["Arn"].as_str() == Some(group_arn))
            .and_then(|group| group["Size"].as_i64())
            .unwrap_or(-1)
    }

    /// The `LinkedAccounts` page of a `ListAccountAssociations` read, as `AccountId` ->
    /// `BillingGroupArn` — the group being `None` for an account in no billing group
    /// (`UNMONITORED`), and an absent key an account this service reports as no linked
    /// account at all.
    async fn list_associations(&self) -> BTreeMap<String, Option<String>> {
        let (status, body) = self.post(LIST_ASSOCIATIONS_PATH, &json!({})).await;
        assert_eq!(
            status, 200,
            "ListAccountAssociations: expected 200, got {status} with {body}"
        );
        body["LinkedAccounts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|element| {
                (
                    element["AccountId"].as_str().unwrap_or_default().to_string(),
                    element["BillingGroupArn"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    /// The billing group `ListAccountAssociations` reports `account_id` against, `None`
    /// when it reports the account with no group or does not report it at all.
    async fn association_of(&self, account_id: &str) -> Option<String> {
        self.list_associations().await.get(account_id).cloned().flatten()
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

/// One request that names at least one account the recorded family does not hold.
struct Row {
    case: &'static str,
    /// The linked accounts the store records for the consolidated billing family.
    family: &'static [&'static str],
    /// The account ids the request names, in request order.
    requested: &'static [&'static str],
    /// The account the refusal must name: the FIRST requested id the family does not
    /// hold, since that is the one the request is refused over.
    offending: &'static str,
}

/// The rows: the size of the family, how many ids one request names, how many of them are
/// outside it and where they sit in the list.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one outside account, a family of one",
            family: &[FAMILY_A],
            requested: &[OUTSIDE_A],
            offending: OUTSIDE_A,
        },
        Row {
            case: "one outside account, a family of three",
            family: &[FAMILY_A, FAMILY_B, FAMILY_C],
            requested: &[OUTSIDE_A],
            offending: OUTSIDE_A,
        },
        Row {
            case: "an outside account before two family accounts",
            family: &[FAMILY_A, FAMILY_B, FAMILY_C],
            requested: &[OUTSIDE_A, FAMILY_A, FAMILY_B],
            offending: OUTSIDE_A,
        },
        Row {
            case: "an outside account between two family accounts",
            family: &[FAMILY_A, FAMILY_B, FAMILY_C],
            requested: &[FAMILY_A, OUTSIDE_A, FAMILY_B],
            offending: OUTSIDE_A,
        },
        Row {
            case: "an outside account after three family accounts",
            family: &[FAMILY_A, FAMILY_B, FAMILY_C],
            requested: &[FAMILY_A, FAMILY_B, FAMILY_C, OUTSIDE_A],
            offending: OUTSIDE_A,
        },
        Row {
            case: "two outside accounts, the first of them named",
            family: &[FAMILY_A],
            requested: &[OUTSIDE_B, OUTSIDE_A],
            offending: OUTSIDE_B,
        },
        Row {
            case: "an account one digit from a family account",
            family: &[FAMILY_A],
            requested: &[OUTSIDE_NEAR_FAMILY_A],
            offending: OUTSIDE_NEAR_FAMILY_A,
        },
        Row {
            case: "every requested account, over a family that holds none",
            family: &[],
            requested: &[FAMILY_A, FAMILY_B],
            offending: FAMILY_A,
        },
    ]
}

/// Assert `answered` IS the refusal the rule owes for `offending`: the status, the modelled
/// error, the reason the model enumerates, the member at fault and the message built from
/// the request.
fn assert_refused(case: &str, member: &str, offending: &str, answered: &(u16, Value)) {
    let (status, body) = answered;
    assert_eq!(
        *status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(ILLEGAL_ACCOUNTS),
        "case {case:?}: expected the {ILLEGAL_ACCOUNTS} reason, got {body}"
    );
    let expected_message =
        format!("account {offending} is not a part of the consolidated billing family");
    assert_eq!(
        body["Message"].as_str(),
        Some(expected_message.as_str()),
        "case {case:?}: expected the message {expected_message:?}, got {body}"
    );
    let fields = body["Fields"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        fields.first().and_then(|field| field["Name"].as_str()),
        Some(member),
        "case {case:?}: expected the refusal to name the member {member:?}, got {body}"
    );
}

/// RULE: `AssociateAccounts` takes only accounts of the recorded consolidated billing
/// family; a request naming one outside it is refused `ILLEGAL_ACCOUNTS`, the group's
/// `Size` does not move and no association is recorded for any account the request named.
#[tokio::test]
async fn associate_accounts_refuses_an_account_outside_the_consolidated_billing_family() {
    for row in rows() {
        let em = Emulator::new();
        em.record_family(row.family);
        let plan = em.create_plan("FamilyPlan").await;
        // The group starts EMPTY, so every row — including the family that holds no
        // accounts at all — addresses a group that exists, and the `Size` a refusal must
        // leave alone is the same for all of them.
        let group = em.create_group("FamilyGroup", &plan, &[], None).await;

        let answered = em
            .post(
                ASSOCIATE_PATH,
                &json!({"Arn": group, "AccountIds": row.requested}),
            )
            .await;
        assert_refused(row.case, "AccountIds", row.offending, &answered);

        assert_eq!(
            em.group_size(&group).await,
            0,
            "case {:?}: the refusal must leave the group's Size where it was",
            row.case
        );
        let associations = em.list_associations().await;
        for account in row.requested {
            let held = associations.get(*account);
            assert!(
                held.is_none_or(|group_arn| group_arn.is_none()),
                "case {:?}: the refusal must associate no account it named, got {held:?}",
                row.case
            );
        }
        assert!(
            !associations.contains_key(row.offending),
            "case {:?}: an account outside the family is no linked account of it, got \
             {associations:?}",
            row.case
        );
    }
}

/// RULE: the same precondition holds where a group is CREATED over an account list
/// (`AccountGrouping.LinkedAccountIds`), so a group the rule would refuse to associate
/// accounts into cannot be created over them either, and nothing is persisted.
#[tokio::test]
async fn create_billing_group_refuses_an_account_outside_the_consolidated_billing_family() {
    for row in rows() {
        let em = Emulator::new();
        em.record_family(row.family);
        let plan = em.create_plan("FamilyPlan").await;

        let answered = em
            .post(
                CREATE_GROUP_PATH,
                &json!({
                    "Name": "OutsideFamilyGroup",
                    "AccountGrouping": {"LinkedAccountIds": row.requested},
                    "ComputationPreference": {"PricingPlanArn": plan},
                }),
            )
            .await;
        assert_refused(
            row.case,
            "AccountGrouping.LinkedAccountIds",
            row.offending,
            &answered,
        );

        assert!(
            em.list_groups().await.is_empty(),
            "case {:?}: a refused create must persist no billing group",
            row.case
        );
        let associations = em.list_associations().await;
        assert!(
            associations.values().all(Option::is_none),
            "case {:?}: a refused create must associate nothing, got {associations:?}",
            row.case
        );
    }
}

/// RULE: every account the recorded family DOES hold is taken — at create time and by a
/// later association — and reads back through the reporting operations; a family account no
/// group holds is still a linked account the family reports, with no billing group.
#[tokio::test]
async fn every_account_of_the_recorded_family_is_taken_and_reads_back() {
    let family = [
        FAMILY_A, FAMILY_B, FAMILY_C, FAMILY_D, FAMILY_E, FAMILY_F,
    ];
    let em = Emulator::new();
    em.record_family(&family);
    let plan = em.create_plan("FamilyPlan").await;

    // Two of the family's accounts at create time, one of them the group's main account.
    let group = em
        .create_group(
            "FamilyGroup",
            &plan,
            &[FAMILY_A, FAMILY_B],
            Some(FAMILY_A),
        )
        .await;
    assert_eq!(
        em.group_size(&group).await,
        2,
        "expected the two accounts the create named"
    );

    // Rows differ along how many of the family's remaining accounts one call associates.
    let additions: [(&str, &[&str], i64); 2] = [
        ("one more family account", &[FAMILY_C], 3),
        ("two more family accounts in one call", &[FAMILY_D, FAMILY_E], 5),
    ];
    for (case, accounts, expected_size) in additions {
        let (status, body) = em
            .post(
                ASSOCIATE_PATH,
                &json!({"Arn": group, "AccountIds": accounts}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for accounts of the family, got {status} with \
             {body}"
        );
        assert_eq!(
            em.group_size(&group).await,
            expected_size,
            "case {case:?}: expected Size {expected_size} after the association"
        );
    }

    let associations = em.list_associations().await;
    for account in [FAMILY_A, FAMILY_B, FAMILY_C, FAMILY_D, FAMILY_E] {
        assert_eq!(
            associations.get(account).cloned().flatten(),
            Some(group.clone()),
            "expected account {account} to read back against the group, got {associations:?}"
        );
    }
    // The family's remaining account is in no billing group, and is reported as a linked
    // account of the family all the same.
    assert!(
        associations.contains_key(FAMILY_F),
        "expected the family account no group holds to be reported, got {associations:?}"
    );
    assert_eq!(
        associations.get(FAMILY_F).cloned().flatten(),
        None,
        "expected the unassociated family account to be reported with no billing group, got \
         {associations:?}"
    );
}

/// RULE: the refusal needs evidence. A store that records no family roster knows of no
/// account that it is outside the family — this emulator serves no AWS Organizations
/// surface for one to be read from — so it refuses none on that ground, and the accounts it
/// is asked about are the linked accounts it reports.
#[tokio::test]
async fn without_a_recorded_family_no_account_is_refused_for_membership() {
    // Rows differ along the operation the ids travel into, and along the id itself.
    let em = Emulator::new();
    let plan = em.create_plan("UnknownFamilyPlan").await;
    let group = em
        .create_group("UnknownFamilyGroup", &plan, &[FAMILY_A], Some(FAMILY_A))
        .await;

    for (case, account) in [
        ("an account no earlier call named", OUTSIDE_A),
        ("another account no earlier call named", OUTSIDE_B),
    ] {
        let (status, body) = em
            .post(
                ASSOCIATE_PATH,
                &json!({"Arn": group, "AccountIds": [account]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 where the family roster is unrecorded, got \
             {status} with {body}"
        );
        assert_eq!(
            em.association_of(account).await,
            Some(group.clone()),
            "case {case:?}: expected the account to read back against the group"
        );
    }
    assert_eq!(
        em.group_size(&group).await,
        3,
        "expected the group to hold the accounts the calls named"
    );
}
