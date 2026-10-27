// SPDX-License-Identifier: CC0-1.0

//! Deleting a billing group that automatically associates newly added accounts, end to
//! end over the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: a billing group that holds the consolidated billing family's
//! automatic-association election (`AccountGrouping.AutoAssociate` enabled — "will
//! automatically associate newly added Amazon Web Services accounts that join your
//! consolidated billing family", the member's own documentation) cannot be deleted while
//! it holds it: `DeleteBillingGroup` is refused 400 `ValidationException` with the reason
//! the model enumerates for exactly this case,
//! `CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP`
//! (`smithy_query(query="shape",shape="ValidationExceptionReason")`), and the refusal
//! writes nothing — the group, the custom line items that apply to it and its account
//! associations all survive. Turning the flag off (`UpdateBillingGroup`) is what makes
//! the group deletable.
//!
//! The rule is about the FLAG the addressed group carries, not about the rest of the
//! request, so the rows below span the shape of the group that holds the election (how
//! many linked accounts, which billing-owner class, which optional members, whether it
//! owns a custom line item) and the two forms the modelled `BillingGroupArn` accepts
//! (the full ARN and the bare id). The served rows span every way a group can lack the
//! election: the flag omitted, spelled `false`, or turned off after being on.
//!
//! Every assertion reads state back through the read-only `ListBillingGroups`,
//! `ListCustomLineItems` and `ListAccountAssociations`.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The operations' modelled HTTP bindings
/// (`smithy_query(query="digest")`).
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";
const CREATE_GROUP_PATH: &str = "/create-billing-group";
const UPDATE_GROUP_PATH: &str = "/update-billing-group";
const DELETE_GROUP_PATH: &str = "/delete-billing-group";
const LIST_GROUPS_PATH: &str = "/list-billing-groups";
const CREATE_ITEM_PATH: &str = "/create-custom-line-item";
const LIST_ITEMS_PATH: &str = "/list-custom-line-items";
const LIST_ASSOCIATIONS_PATH: &str = "/list-account-associations";

/// The `ValidationExceptionReason` the model enumerates for a delete of a group that
/// still holds the automatic-association election.
const CANNOT_DELETE: &str = "CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP";

/// The `ValidationExceptionReason` an `Arn` naming no stored group earns — the refusal
/// this operation already answered, which the rule under test must not displace.
const INVALID_BILLING_GROUP: &str = "INVALID_BILLING_GROUP";

/// A live emulator holding only this service, with the emulated account designated the
/// organization's PAYER account — Billing Conductor's control plane is authorized for
/// the payer account alone, so this is the world in which these calls are reachable.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine the service was built on, kept so a row can record the billing
    /// transfer a transfer-billing group names.
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

    /// Record a billing transfer the organization HAS, and hand its ARN back — the
    /// shared seam [`crate::billing_transfer_identity::designate_transfer`].
    fn designate_transfer(&self, arn: &str) -> String {
        crate::billing_transfer_identity::designate_transfer(&self.engine, arn)
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
            "setup CreatePricingPlan {name:?}: expected 200, got {status} with {body}"
        );
        body["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `CreateBillingGroup` spelled as `Name` plus the computation preference naming
    /// `plan`, with the members of `extra` merged in beside them. Must succeed.
    async fn create_group(&self, name: &str, plan: &str, extra: &Value) -> String {
        let mut body = json!({
            "Name": name,
            "ComputationPreference": { "PricingPlanArn": plan },
        });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let (status, parsed) = self.post(CREATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {body}: expected 200, got {status} with {parsed}"
        );
        parsed["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `CreateCustomLineItem` applying to `group_arn`: the created item's ARN.
    async fn create_item(&self, group_arn: &str, name: &str) -> String {
        let (status, body) = self
            .post(
                CREATE_ITEM_PATH,
                &json!({
                    "Name": name,
                    "Description": format!("the {name} custom line item"),
                    "BillingGroupArn": group_arn,
                    "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 2.5 } },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreateCustomLineItem {name:?}: expected 200, got {status} with {body}"
        );
        body["Arn"].as_str().unwrap_or_default().to_string()
    }

    /// `UpdateBillingGroup` against `arn`, carrying the members of `patch` beside it.
    async fn update(&self, case: &str, arn: &str, patch: &Value) -> Value {
        let mut body = json!({ "Arn": arn });
        if let Some(members) = patch.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let (status, parsed) = self.post(UPDATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateBillingGroup expected 200, got {status} with {parsed}"
        );
        parsed
    }

    /// `DeleteBillingGroup` on `arn`, exactly as spelled.
    async fn delete(&self, arn: &str) -> (u16, Value) {
        self.post(DELETE_GROUP_PATH, &json!({ "Arn": arn })).await
    }

    /// The whole modelled view `ListBillingGroups` reports one group with, or
    /// [`Value::Null`] when state holds no such group.
    async fn listed_group(&self, case: &str, arn: &str) -> Value {
        let (status, body) = self
            .post(LIST_GROUPS_PATH, &json!({ "Filters": { "Arns": [arn] } }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListBillingGroups expected 200, got {status} with {body}"
        );
        body["BillingGroups"]
            .as_array()
            .and_then(|groups| groups.first())
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Does `ListCustomLineItems` still report the item `arn`?
    async fn reports_item(&self, case: &str, arn: &str) -> bool {
        let (status, body) = self
            .post(LIST_ITEMS_PATH, &json!({ "Filters": { "Arns": [arn] } }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCustomLineItems expected 200, got {status} with {body}"
        );
        body["CustomLineItems"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    }

    /// The billing group `ListAccountAssociations` reports for one account.
    async fn owner_of(&self, case: &str, account_id: &str) -> Option<String> {
        let (status, body) = self
            .post(
                LIST_ASSOCIATIONS_PATH,
                &json!({ "Filters": { "AccountId": account_id } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAccountAssociations expected 200, got {status} with {body}"
        );
        body["LinkedAccounts"]
            .as_array()
            .and_then(|accounts| accounts.first())
            .and_then(|entry| entry.get("BillingGroupArn"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The bare `[a-zA-Z0-9]{10,12}` id the `BillingGroupArn` pattern accepts beside the
/// full ARN; both name the same stored group.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
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
/// (`smithy_query(query="shape",shape="AccountGrouping")`).
fn transfer_arn(suffix: &str) -> String {
    format!(
        "arn:aws:organizations::{}:transfer/o-abcd123456/billing/inbound/rt-{suffix}",
        ::engine::emulator_account_id()
    )
}

/// Assert one refusal carries the modelled 400 `ValidationException` with `reason`, the
/// member at fault, and a message that states the rule and names every fragment of
/// `must_name` — the variable parts the message takes from the request and from state.
fn assert_refused(case: &str, status: u16, body: &Value, reason: &str, must_name: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(reason),
        "case {case:?}: expected Reason {reason}, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some("Arn"),
        "case {case:?}: expected the refusal to name the Arn member, got {body}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    for fragment in must_name {
        assert!(
            message.contains(fragment),
            "case {case:?}: expected the message to name {fragment:?}, got {message:?}"
        );
    }
    assert_eq!(
        body["Fields"][0]["Message"].as_str(),
        Some(message),
        "case {case:?}: expected the faulting field to carry the same message, got {body}"
    );
}

/// The full `BillingGroupArn` the service normalises a bare id to — the form every
/// message and every read reports, whichever form the request spelled.
fn full_arn(id: &str) -> String {
    format!(
        "arn:aws:billingconductor::{}:billinggroup/{id}",
        ::engine::emulator_account_id()
    )
}

/// RULE: whatever the elected group looks like and whichever form of its ARN the
/// request spells, deleting it while it holds the election is refused
/// `CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP` and the group survives intact.
#[tokio::test]
async fn deleting_the_elected_group_is_refused_and_writes_nothing() {
    // Rows differ along what the rule generalises over: the shape of the group holding
    // the election (membership size, billing-owner class, optional members) and the
    // form of the ARN the delete names it by. One group at a time may hold the
    // election, so each row stands up its own emulator.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "one linked account, which is also the primary account",
            json!({
                "AccountGrouping": { "LinkedAccountIds": ["111111111111"], "AutoAssociate": true },
                "PrimaryAccountId": "111111111111",
            }),
            false,
        ),
        (
            "three linked accounts, named by the bare id",
            json!({
                "AccountGrouping": {
                    "LinkedAccountIds": ["333344445555", "444455556666", "555566667777"],
                    "AutoAssociate": true,
                },
                "PrimaryAccountId": "333344445555",
            }),
            true,
        ),
        (
            "no linked accounts and no billing owner",
            json!({ "AccountGrouping": { "AutoAssociate": true } }),
            false,
        ),
        (
            "a description and tags, named by the bare id",
            json!({
                "Description": "the group that holds the election",
                "AccountGrouping": { "LinkedAccountIds": ["888888888888"], "AutoAssociate": true },
                "PrimaryAccountId": "888888888888",
                "Tags": { "team": "billing" },
            }),
            true,
        ),
    ];

    for (case, extra, by_bare_id) in rows {
        let em = Emulator::new();
        let plan = em.create_plan("DeleteElectedPlan").await;
        let group = em.create_group("ElectedGroup", &plan, &extra).await;
        let before = em.listed_group(case, &group).await;
        assert!(
            !before.is_null(),
            "case {case:?}: expected the created group to read back, got {before}"
        );

        let named = if by_bare_id {
            bare_id(&group)
        } else {
            group.clone()
        };
        let (status, body) = em.delete(&named).await;
        assert_refused(
            case,
            status,
            &body,
            CANNOT_DELETE,
            &[&group, "AccountGrouping.AutoAssociate"],
        );

        let after = em.listed_group(case, &group).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused delete to leave the group untouched, \
             got {after} after {before}"
        );
    }
}

/// RULE: the refusal destroys nothing the group owned — the custom line items that
/// apply to it and the account associations it holds are all still reported after it.
#[tokio::test]
async fn the_refusal_keeps_the_items_and_associations_the_group_owned() {
    let em = Emulator::new();
    let plan = em.create_plan("DeleteElectedOwnerPlan").await;
    let case = "an elected group owning custom line items and associations";
    let group = em
        .create_group(
            "ElectedOwner",
            &plan,
            &json!({
                "AccountGrouping": {
                    "LinkedAccountIds": ["121212121212", "131313131313"],
                    "AutoAssociate": true,
                },
                "PrimaryAccountId": "121212121212",
            }),
        )
        .await;
    let flat_item = em.create_item(&group, "ElectedOwnerFlat").await;
    let second_item = em.create_item(&group, "ElectedOwnerSecond").await;

    let (status, body) = em.delete(&group).await;
    assert_refused(
        case,
        status,
        &body,
        CANNOT_DELETE,
        &[&group, "AccountGrouping.AutoAssociate"],
    );

    for item in [&flat_item, &second_item] {
        assert!(
            em.reports_item(case, item).await,
            "case {case:?}: expected custom line item {item} to survive the refused delete"
        );
    }
    for account in ["121212121212", "131313131313"] {
        assert_eq!(
            em.owner_of(case, account).await.as_deref(),
            Some(group.as_str()),
            "case {case:?}: expected account {account} to stay associated with {group}"
        );
    }
}

/// RULE: a group that does NOT hold the election is deletable — the flag omitted,
/// spelled `false`, or turned off after being on — and the delete answers the ARN and
/// takes the group out of state.
#[tokio::test]
async fn a_group_without_the_election_is_deleted() {
    // Rows differ along how the group comes to lack the election, and along the form of
    // the ARN the delete names it by.
    let rows: Vec<(&str, Value, Option<Value>, bool)> = vec![
        (
            "AccountGrouping naming no AutoAssociate at all",
            json!({
                "AccountGrouping": { "LinkedAccountIds": ["222222222222"] },
                "PrimaryAccountId": "222222222222",
            }),
            None,
            false,
        ),
        (
            "AutoAssociate spelled false",
            json!({
                "AccountGrouping": { "LinkedAccountIds": ["555555555555"], "AutoAssociate": false },
                "PrimaryAccountId": "555555555555",
            }),
            None,
            true,
        ),
        (
            "the election turned off by UpdateBillingGroup",
            json!({
                "AccountGrouping": { "LinkedAccountIds": ["666666666666"], "AutoAssociate": true },
                "PrimaryAccountId": "666666666666",
            }),
            Some(json!({ "AccountGrouping": { "AutoAssociate": false } })),
            false,
        ),
        (
            "the election turned off, the delete naming the bare id",
            json!({
                "AccountGrouping": { "AutoAssociate": true },
                "Description": "an elected group that gave the election up",
            }),
            Some(json!({ "AccountGrouping": { "AutoAssociate": false } })),
            true,
        ),
    ];

    for (case, extra, patch, by_bare_id) in rows {
        let em = Emulator::new();
        let plan = em.create_plan("DeleteUnelectedPlan").await;
        let group = em.create_group("UnelectedGroup", &plan, &extra).await;
        if let Some(patch) = &patch {
            em.update(case, &group, patch).await;
        }

        let named = if by_bare_id {
            bare_id(&group)
        } else {
            group.clone()
        };
        let (status, body) = em.delete(&named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"].as_str(),
            Some(group.as_str()),
            "case {case:?}: expected the deleted group's ARN, got {body}"
        );

        let after = em.listed_group(case, &group).await;
        assert!(
            after.is_null(),
            "case {case:?}: expected the group to be gone from state, got {after}"
        );
    }
}

/// RULE: the refusal is the group's condition, not a one-shot — it answers every delete
/// of the elected group, and once the caller turns the flag off the SAME group deletes.
#[tokio::test]
async fn the_refusal_stands_until_the_flag_is_turned_off() {
    let em = Emulator::new();
    let plan = em.create_plan("DeleteHandoverPlan").await;
    let case = "a transfer-billing group that holds the election";
    let transfer = em.designate_transfer(&transfer_arn("1234abcd"));
    let group = em
        .create_group(
            "HandoverGroup",
            &plan,
            &json!({
                "AccountGrouping": {
                    "LinkedAccountIds": ["999999999999"],
                    "AutoAssociate": true,
                    "ResponsibilityTransferArn": transfer,
                },
            }),
        )
        .await;

    for attempt in 1..=2 {
        let (status, body) = em.delete(&group).await;
        assert_refused(
            &format!("{case}, attempt {attempt}"),
            status,
            &body,
            CANNOT_DELETE,
            &[&group, "AccountGrouping.AutoAssociate"],
        );
    }

    em.update(
        case,
        &group,
        &json!({ "AccountGrouping": { "AutoAssociate": false } }),
    )
    .await;
    assert_eq!(
        em.listed_group(case, &group).await["AccountGrouping"]["AutoAssociate"].as_bool(),
        Some(false),
        "case {case:?}: expected the election to be given up before the delete"
    );

    let (status, body) = em.delete(&group).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to be served once the flag is off, got \
         {status} with {body}"
    );
    let after = em.listed_group(case, &group).await;
    assert!(
        after.is_null(),
        "case {case:?}: expected the group to be gone from state, got {after}"
    );
}

/// RULE: the election refusal does not displace the one an `Arn` naming no stored group
/// already earned — including a second delete of a group that was just deleted.
#[tokio::test]
async fn an_arn_naming_no_group_is_still_invalid_billing_group() {
    let em = Emulator::new();
    let plan = em.create_plan("DeleteMissingPlan").await;
    let group = em
        .create_group(
            "DeletedTwiceGroup",
            &plan,
            &json!({
                "AccountGrouping": { "LinkedAccountIds": ["777777777777"] },
                "PrimaryAccountId": "777777777777",
            }),
        )
        .await;
    let (status, body) = em.delete(&group).await;
    assert_eq!(
        status, 200,
        "setup: expected the first delete to be served, got {status} with {body}"
    );

    // Rows differ along the form the unknown ARN takes: the group just deleted, a bare
    // id state never held, and a full ARN state never held.
    let rows: Vec<(&str, String, String)> = vec![
        ("the group just deleted", group.clone(), group.clone()),
        (
            "a bare id state never held",
            "ABCDEF123456".to_string(),
            full_arn("ABCDEF123456"),
        ),
        (
            "a full ARN state never held",
            full_arn("ZZZZ999999"),
            full_arn("ZZZZ999999"),
        ),
    ];

    for (case, named, resolved) in rows {
        let (status, body) = em.delete(&named).await;
        assert_refused(case, status, &body, INVALID_BILLING_GROUP, &[&resolved]);
    }
}
