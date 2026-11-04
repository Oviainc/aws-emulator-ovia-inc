// SPDX-License-Identifier: CC0-1.0

//! `ListAccountAssociations` over the consolidated billing family AWS Organizations
//! records, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE: the roster this operation reports is the consolidated billing family's own —
//! "list linked accounts that are linked to the payer account for the specified time
//! period ... The response will optionally include the billing group that's associated
//! with the linked account" (`smithy_query(query="doc",shape="ListAccountAssociations")`)
//! — so EVERY linked account of the payer account is reported whether or not a Billing
//! Conductor write has ever named it, each element carrying the `AccountId`, the
//! `AccountName` and `AccountEmail` the account's own record holds
//! (`smithy_query(query="shape",shape="AccountAssociationsListElement")`: both are
//! modelled output, unconstrained strings) and the `BillingGroupArn` of the membership
//! that was in force in the period asked about, absent when the account was in none.
//! Which accounts a family holds, and what each is called, is state the Organizations
//! control plane owns — an account joins a family THERE and no request of this API puts
//! one there — so both are read from that store through the one shared engine.
//!
//! The rows span what the rule generalises over: how many accounts the family holds (one,
//! two, three, five), whether a Billing Conductor write has ever named the account, the
//! form of the name and the email the organization records (one word, spaces,
//! punctuation, a long name, a non-ASCII name, several email domains), which accounts a
//! billing group holds, which `Association`/`AccountId` filter the read carries, and an
//! account the organization does NOT hold — including one a digit from a member.
//!
//! The member accounts are arranged through the Organizations service's own state module:
//! `CreateAccount` and `InviteAccountToOrganization` are still stubs in the Organizations
//! emulator, so there is no wire path that adds a member account, and that store is the
//! very state its handlers answer from (the same arrangement
//! `service-com-amazonaws-supportapp-supportapp`'s suites make). The management account
//! the organization DOES mint over the wire anchors it: the email this listing reports
//! for it is asserted against the `MasterAccountEmail` `DescribeOrganization` answers.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The region every request is signed for; neither service's resources are regional, so
/// no assertion depends on it.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix the Organizations emulator routes on (that crate's
/// `OPERATIONS.md`, "Wire protocol").
const ORGANIZATIONS_TARGET_PREFIX: &str = "AWSOrganizationsV20161128";

/// The two `Association` values that are not a billing group ARN, as
/// `ListAccountAssociationsFilter.Association` enumerates them.
const MONITORED: &str = "MONITORED";
const UNMONITORED: &str = "UNMONITORED";

/// The `ValidationExceptionReason` the model enumerates for an account list this service
/// may not take.
const ILLEGAL_ACCOUNTS: &str = "ILLEGAL_ACCOUNTS";

/// One member account of the organization: its id and the identity the organization
/// records for it. The forms differ along what the rule generalises over, since
/// `AccountName` and `AccountEmail` are unconstrained strings.
struct Member {
    id: &'static str,
    name: &'static str,
    email: &'static str,
}

/// The member accounts these suites draw on, in no particular order.
const MEMBERS: &[Member] = &[
    Member {
        id: "111122223333",
        name: "Dev",
        email: "dev@example.com",
    },
    Member {
        id: "222233334444",
        name: "Finance Team Account",
        email: "finance-team@example.com",
    },
    Member {
        id: "333344445555",
        name: "Acme, Inc. (Prod)",
        email: "prod+billing@example.co.uk",
    },
    Member {
        id: "444455556666",
        name: "A rather long friendly account name for one linked account",
        email: "long.name@subdomain.example.net",
    },
    Member {
        id: "555566667777",
        name: "Équipe Facturation — Montréal",
        email: "equipe@example.org",
    },
];

/// An account id the organization holds no record of.
const OUTSIDE: &str = "999999999999";

/// One digit away from the first member's id: membership is decided by the roster, not by
/// how closely an id resembles one of its accounts.
const OUTSIDE_NEAR_MEMBER: &str = "111122223334";

/// An emulator holding Billing Conductor and the Organizations control plane on ONE shared
/// engine, with the emulated account designated the organization's PAYER account — Billing
/// Conductor's control plane is the payer account's alone (see
/// `tests/all/payer_account_gate.rs` for the denial every other caller gets).
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
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
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // Mounting the Organizations control plane establishes the organization the
        // emulated account belongs to, so that store is cleared again right after: these
        // cases arrange the organization themselves — the one they create through that
        // service's own `CreateOrganization`, and the roster they record beside it — and
        // one of them is about the account being in none.
        *engine
            .state::<service_com_amazonaws_organizations_awsorganizationsv20161128::state::ComAmazonawsOrganizationsAwsorganizationsv20161128State>()
            .lock() = Default::default();
        Self { engine, router }
    }

    /// POST a JSON body to one of Billing Conductor's routes and answer the status plus
    /// the parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://billingconductor.{REGION}.amazonaws.com{path}"
            ))
            .header("host", format!("billingconductor.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/billingconductor/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
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

    /// Call one operation of the Organizations control plane mounted beside this service,
    /// over THAT service's wire protocol (`awsJson1_1`, routed by `X-Amz-Target`).
    async fn organizations(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("organizations.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("{ORGANIZATIONS_TARGET_PREFIX}.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/organizations/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=00"
                ),
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

    /// Put the emulated account in an organization through the Organizations emulator's
    /// own `CreateOrganization`, and answer the management account's id and email as
    /// `DescribeOrganization` reports them — the organization's own record of them, read
    /// back over the wire.
    async fn create_organization(&self) -> (String, String) {
        let (status, created) = self.organizations("CreateOrganization", &json!({})).await;
        assert_eq!(
            status, 200,
            "setup CreateOrganization: expected 200, got {status} with {created:?}"
        );
        let (status, described) = self
            .organizations("DescribeOrganization", &json!({}))
            .await;
        assert_eq!(
            status, 200,
            "setup DescribeOrganization: expected 200, got {status} with {described:?}"
        );
        let organization = &described["Organization"];
        (
            organization["MasterAccountId"]
                .as_str()
                .expect("DescribeOrganization reports the management account id")
                .to_string(),
            organization["MasterAccountEmail"]
                .as_str()
                .expect("DescribeOrganization reports the management account email")
                .to_string(),
        )
    }

    /// Record `member` as a member account of the organization.
    ///
    /// `CreateAccount` and `InviteAccountToOrganization` are stubs in the Organizations
    /// emulator, so no wire call adds a member account: the record is written into that
    /// service's own store, which is the state its handlers answer from, and it is DERIVED
    /// from the management account's record so nothing beside the id, the name and the
    /// email is invented here.
    fn record_member_account(&self, member: &Member) {
        let organizations =
            service_com_amazonaws_organizations_awsorganizationsv20161128::state::service_state(
                &*self.engine,
            )
            .expect("the Organizations store must be reachable on the shared engine");
        let mut store = organizations.lock();
        let management_id = store
            .organization
            .as_ref()
            .expect("the account must already be in an organization")
            .master_account_id
            .clone();
        let mut record = store
            .accounts
            .get(&management_id)
            .expect("the organization must hold its management account")
            .clone();
        record.arn = record.arn.replace(&management_id, member.id);
        record.id = member.id.to_string();
        record.name = member.name.to_string();
        record.email = member.email.to_string();
        store.accounts.insert(member.id.to_string(), record);
    }

    /// Put the emulated account in an organization holding the first `members` of
    /// [`MEMBERS`] beside its management account, and answer the identity the
    /// organization records for every one of its accounts, keyed by account id.
    async fn organization_of(&self, members: usize) -> BTreeMap<String, (String, String)> {
        let (management_id, management_email) = self.create_organization().await;
        let mut roster: BTreeMap<String, (String, String)> = BTreeMap::new();
        for member in &MEMBERS[..members] {
            self.record_member_account(member);
            roster.insert(
                member.id.to_string(),
                (member.name.to_string(), member.email.to_string()),
            );
        }
        // The management account's name is reported by no implemented Organizations read,
        // so it is taken from the record this listing has to agree with; its email is the
        // one `DescribeOrganization` answered above.
        let management_name = self.recorded_name(&management_id);
        roster.insert(management_id, (management_name, management_email));
        roster
    }

    /// The `name` the organization's record for `account_id` holds.
    fn recorded_name(&self, account_id: &str) -> String {
        let organizations =
            service_com_amazonaws_organizations_awsorganizationsv20161128::state::service_state(
                &*self.engine,
            )
            .expect("the Organizations store must be reachable on the shared engine");
        let store = organizations.lock();
        let name = store
            .accounts
            .get(account_id)
            .expect("the organization must hold the account")
            .name
            .clone();
        name
    }

    /// Create a pricing plan and answer the ARN the service minted.
    async fn create_plan(&self, name: &str) -> String {
        let (status, body) = self
            .post("/create-pricing-plan", &json!({"Name": name}))
            .await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name:?}: expected 200, got {status} with {body:?}"
        );
        body["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// Create a billing group holding `linked` and answer the ARN the service minted.
    async fn create_group(&self, name: &str, plan: &str, linked: &[&str]) -> String {
        let (status, body) = self
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

    /// The `LinkedAccounts` page of a `ListAccountAssociations` read carrying `filters`,
    /// as `AccountId` -> the element the listing reported for it.
    async fn list_associations(&self, case: &str, filters: Option<Value>) -> BTreeMap<String, Value> {
        let body = match filters {
            Some(filters) => json!({"Filters": filters}),
            None => json!({}),
        };
        let (status, answered) = self.post("/list-account-associations", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListAccountAssociations, got {status} with \
             {answered:?}"
        );
        answered["LinkedAccounts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|element| {
                (
                    element["AccountId"].as_str().unwrap_or_default().to_string(),
                    element,
                )
            })
            .collect()
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

/// Assert the element the listing reported for `account_id` carries the identity the
/// organization records for it, and the billing group `expected_group`.
fn assert_element(
    case: &str,
    account_id: &str,
    element: &Value,
    identity: &(String, String),
    expected_group: Option<&str>,
) {
    assert_eq!(
        element["AccountId"].as_str(),
        Some(account_id),
        "case {case:?}: expected the element to report AccountId {account_id:?}, got {element:?}"
    );
    assert_eq!(
        element["AccountName"].as_str(),
        Some(identity.0.as_str()),
        "case {case:?}: expected account {account_id} to report AccountName {:?}, got {element:?}",
        identity.0
    );
    assert_eq!(
        element["AccountEmail"].as_str(),
        Some(identity.1.as_str()),
        "case {case:?}: expected account {account_id} to report AccountEmail {:?}, got {element:?}",
        identity.1
    );
    assert_eq!(
        element["BillingGroupArn"].as_str(),
        expected_group,
        "case {case:?}: expected account {account_id} to report BillingGroupArn \
         {expected_group:?}, got {element:?}"
    );
}

/// RULE: every linked account of the consolidated billing family is reported, with the
/// name and email the organization records for it, before any billing group exists — so a
/// store no Billing Conductor write has touched still answers the family's roster, and
/// `UNMONITORED` reports all of it.
#[tokio::test]
async fn the_consolidated_billing_family_is_reported_before_any_billing_group_exists() {
    // Rows differ along how many accounts the family holds: the management account alone,
    // and families of two, three and five accounts beside it.
    for members in [0usize, 1, 2, 4] {
        let case = format!("a family of {} accounts", members + 1);
        let em = Emulator::new();
        let roster = em.organization_of(members).await;

        let reported = em.list_associations(&case, None).await;
        let reported_ids: Vec<&String> = reported.keys().collect();
        let expected_ids: Vec<&String> = roster.keys().collect();
        assert_eq!(
            reported_ids, expected_ids,
            "case {case:?}: expected the family's roster {expected_ids:?}, got {reported_ids:?}"
        );
        for (account_id, identity) in &roster {
            let element = reported
                .get(account_id)
                .unwrap_or_else(|| panic!("case {case:?}: account {account_id} was not reported"));
            assert_element(&case, account_id, element, identity, None);
        }

        // Every account is in no billing group, so UNMONITORED reports the whole family
        // and MONITORED reports none of it — the filters answer from the same roster.
        let unmonitored = em
            .list_associations(&case, Some(json!({"Association": UNMONITORED})))
            .await;
        assert_eq!(
            unmonitored.keys().collect::<Vec<&String>>(),
            expected_ids,
            "case {case:?}: expected UNMONITORED to report the whole family, got {:?}",
            unmonitored.keys().collect::<Vec<&String>>()
        );
        let monitored = em
            .list_associations(&case, Some(json!({"Association": MONITORED})))
            .await;
        assert!(
            monitored.is_empty(),
            "case {case:?}: expected MONITORED to report no account before any billing group \
             exists, got {:?}",
            monitored.keys().collect::<Vec<&String>>()
        );
    }
}

/// RULE: the `AccountName` and `AccountEmail` reported for an account are the ones the
/// account's own record in the organization holds, whatever form they take — they are
/// unconstrained strings, so no form of them is rewritten, truncated or invented.
#[tokio::test]
async fn the_name_and_email_reported_are_the_ones_the_organization_records() {
    let em = Emulator::new();
    let roster = em.organization_of(MEMBERS.len()).await;
    let reported = em.list_associations("every form of name and email", None).await;

    // Rows: one word, words with spaces, punctuation, a long name, a non-ASCII name, and
    // four different email domains — plus the management account the organization minted
    // itself, whose email `DescribeOrganization` answered.
    for (account_id, identity) in &roster {
        let case = format!("the identity recorded for {account_id}");
        let element = reported
            .get(account_id)
            .unwrap_or_else(|| panic!("case {case:?}: account {account_id} was not reported"));
        assert_element(&case, account_id, element, identity, None);
    }
    assert_eq!(
        reported.len(),
        MEMBERS.len() + 1,
        "expected every member account and the management account to be reported, got {:?}",
        reported.keys().collect::<Vec<&String>>()
    );
}

/// RULE: a billing group changes only the `BillingGroupArn` the listing reports for the
/// accounts it holds — the roster and every account's identity are the family's, so an
/// account the group does not hold stays reported with no group, and the
/// `Association`/`AccountId` filters select over that same answer.
#[tokio::test]
async fn a_billing_group_moves_only_the_group_the_listing_reports() {
    let em = Emulator::new();
    let roster = em.organization_of(4).await;
    let plan = em.create_plan("RosterPlan").await;
    let held = [MEMBERS[0].id, MEMBERS[2].id];
    let group = em.create_group("RosterGroup", &plan, &held).await;

    let case = "two of the family's accounts in a billing group";
    let reported = em.list_associations(case, None).await;
    assert_eq!(
        reported.keys().collect::<Vec<&String>>(),
        roster.keys().collect::<Vec<&String>>(),
        "case {case:?}: a billing group must not change the roster, got {:?}",
        reported.keys().collect::<Vec<&String>>()
    );
    for (account_id, identity) in &roster {
        let expected_group = held
            .contains(&account_id.as_str())
            .then_some(group.as_str());
        let element = reported
            .get(account_id)
            .unwrap_or_else(|| panic!("case {case:?}: account {account_id} was not reported"));
        assert_element(case, account_id, element, identity, expected_group);
    }

    // Each filter selects exactly the accounts the unfiltered read reports that way.
    let filters: [(&str, Value, Vec<String>); 5] = [
        (
            "MONITORED",
            json!({"Association": MONITORED}),
            held.iter().map(|id| (*id).to_string()).collect(),
        ),
        (
            "UNMONITORED",
            json!({"Association": UNMONITORED}),
            roster
                .keys()
                .filter(|id| !held.contains(&id.as_str()))
                .cloned()
                .collect(),
        ),
        (
            "the group's own ARN",
            json!({"Association": group}),
            held.iter().map(|id| (*id).to_string()).collect(),
        ),
        (
            "one account of the group by id",
            json!({"AccountId": MEMBERS[0].id}),
            vec![MEMBERS[0].id.to_string()],
        ),
        (
            "one account outside the group by id",
            json!({"AccountId": MEMBERS[1].id}),
            vec![MEMBERS[1].id.to_string()],
        ),
    ];
    for (case, filter, expected) in filters {
        let selected = em.list_associations(case, Some(filter)).await;
        let ids: Vec<String> = selected.keys().cloned().collect();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            ids, expected_sorted,
            "case {case:?}: expected the filter to select {expected_sorted:?}, got {ids:?}"
        );
        // Whatever the filter, every element it selects still carries the family's identity
        // for that account.
        for (account_id, element) in &selected {
            let identity = roster
                .get(account_id)
                .unwrap_or_else(|| panic!("case {case:?}: {account_id} is outside the family"));
            let expected_group = held
                .contains(&account_id.as_str())
                .then_some(group.as_str());
            assert_element(case, account_id, element, identity, expected_group);
        }
    }

    // A disassociated account is still a linked account of the family: it goes back to
    // being reported with no billing group, identity and all.
    let (status, body) = em
        .post(
            "/disassociate-accounts",
            &json!({"Arn": group, "AccountIds": [MEMBERS[0].id]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup DisassociateAccounts: expected 200, got {status} with {body:?}"
    );
    let case = "an account the group released";
    let reported = em.list_associations(case, None).await;
    let element = reported
        .get(MEMBERS[0].id)
        .unwrap_or_else(|| panic!("case {case:?}: the released account was not reported"));
    let identity = roster
        .get(MEMBERS[0].id)
        .expect("the released account is in the family");
    assert_element(case, MEMBERS[0].id, element, identity, None);
}

/// RULE: the family is the organization's roster, so an account the organization holds no
/// record of is outside the consolidated billing family: a request taking it into a
/// billing group's membership is refused `ILLEGAL_ACCOUNTS`, and the listing never reports
/// it.
#[tokio::test]
async fn an_account_the_organization_does_not_hold_is_outside_the_family() {
    // Rows differ along the id (unrelated, and one digit from a member's) and the
    // operation the id travels into.
    for outside in [OUTSIDE, OUTSIDE_NEAR_MEMBER] {
        let em = Emulator::new();
        let roster = em.organization_of(2).await;
        let plan = em.create_plan("OutsidePlan").await;
        let group = em.create_group("OutsideGroup", &plan, &[]).await;

        for (operation, path, body, member) in [
            (
                "AssociateAccounts",
                "/associate-accounts",
                json!({"Arn": group, "AccountIds": [outside]}),
                "AccountIds",
            ),
            (
                "CreateBillingGroup",
                "/create-billing-group",
                json!({
                    "Name": "OutsideCreatedGroup",
                    "AccountGrouping": {"LinkedAccountIds": [outside]},
                    "ComputationPreference": {"PricingPlanArn": plan},
                }),
                "AccountGrouping.LinkedAccountIds",
            ),
        ] {
            let case = format!("{operation} over {outside}");
            let (status, answered) = em.post(path, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {answered:?}"
            );
            assert_eq!(
                answered["Reason"].as_str(),
                Some(ILLEGAL_ACCOUNTS),
                "case {case:?}: expected the {ILLEGAL_ACCOUNTS} reason, got {answered:?}"
            );
            assert_eq!(
                answered["Fields"]
                    .as_array()
                    .and_then(|fields| fields.first())
                    .and_then(|field| field["Name"].as_str()),
                Some(member),
                "case {case:?}: expected the refusal to name the member {member:?}, got \
                 {answered:?}"
            );
        }

        let case = format!("the listing after {outside} was refused");
        let reported = em.list_associations(&case, None).await;
        assert_eq!(
            reported.keys().collect::<Vec<&String>>(),
            roster.keys().collect::<Vec<&String>>(),
            "case {case:?}: an account the organization does not hold is no linked account of \
             the family, got {:?}",
            reported.keys().collect::<Vec<&String>>()
        );
    }
}

/// RULE: the identity is reported where a record holds one and left ABSENT otherwise, and
/// membership needs evidence — on an engine whose Organizations store records no
/// organization there is no roster, so the accounts this service has been told about are
/// the ones it reports, each with no `AccountName`/`AccountEmail` to report.
#[tokio::test]
async fn without_an_organization_no_identity_is_invented() {
    let em = Emulator::new();
    let plan = em.create_plan("NoOrganizationPlan").await;
    // Rows differ along how the account came to this service's notice: named at create
    // time, added by a later association, and released again.
    let group = em
        .create_group("NoOrganizationGroup", &plan, &[MEMBERS[0].id])
        .await;
    let (status, body) = em
        .post(
            "/associate-accounts",
            &json!({"Arn": group, "AccountIds": [MEMBERS[1].id]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup AssociateAccounts: expected 200 where no roster is known, got {status} with \
         {body:?}"
    );
    let (status, body) = em
        .post(
            "/disassociate-accounts",
            &json!({"Arn": group, "AccountIds": [MEMBERS[1].id]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup DisassociateAccounts: expected 200, got {status} with {body:?}"
    );

    let case = "no organization is recorded";
    let reported = em.list_associations(case, None).await;
    for (account_id, expected_group) in [
        (MEMBERS[0].id, Some(group.as_str())),
        (MEMBERS[1].id, None),
    ] {
        let element = reported
            .get(account_id)
            .unwrap_or_else(|| panic!("case {case:?}: account {account_id} was not reported"));
        assert_eq!(
            element["BillingGroupArn"].as_str(),
            expected_group,
            "case {case:?}: expected account {account_id} to report BillingGroupArn \
             {expected_group:?}, got {element:?}"
        );
        assert!(
            element.get("AccountName").is_none_or(Value::is_null),
            "case {case:?}: expected no AccountName where no record holds one, got {element:?}"
        );
        assert!(
            element.get("AccountEmail").is_none_or(Value::is_null),
            "case {case:?}: expected no AccountEmail where no record holds one, got {element:?}"
        );
    }
}
