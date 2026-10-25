// SPDX-License-Identifier: CC0-1.0

//! What a custom line item may be attached to, end to end over the wire:
//! `CreateBillingGroup` / `AssociateAccounts` / `DisassociateAccounts` ->
//! `CreateCustomLineItem` -> `ListCustomLineItems` /
//! `BatchAssociateResourcesToCustomLineItem` /
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: a custom line item is attached only to what the billing group it applies to
//! carries — the resources a percentage item's charge is computed over are that group
//! itself or custom line items of that same group, and the account the item is applied
//! to is an account that group bills. Anything else is refused with the modelled
//! `ValidationException` (400) carrying the `ValidationExceptionReason` the model
//! enumerates for that fault, nothing is persisted, and the per-resource channel of
//! `BatchAssociateResourcesToCustomLineItem` reports the same refusal as the
//! `AssociateResourceErrorReason` that classifies it.
//!
//! The rows differ along the class of the resource (billing group / custom line item),
//! the form it is named in (full ARN / bare id), its position in a list of several, the
//! side of the scope it is on, and — for the account — how the account came to be
//! billed by the group or to leave it.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// An AWS-managed pricing plan every billing group here computes with: the
/// `PricingPlanFullArn` pattern enumerates it, so no plan has to be created first
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
const MANAGED_PRICING_PLAN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// An account the HOME billing group bills (`AccountId`, `^[0-9]{12}$`).
const HOME_ACCOUNT: &str = "111111111111";

/// A second account the home billing group bills, so acceptance does not depend on
/// which member of the group is named.
const OTHER_HOME_ACCOUNT: &str = "222222222222";

/// An account another billing group bills: an account belongs to at most one billing
/// group, so this one is never billed by the home group.
const FOREIGN_ACCOUNT: &str = "333333333333";

/// An account in no billing group at all.
const UNASSOCIATED_ACCOUNT: &str = "444444444444";

/// An account associated to the home group AFTER it was created, so membership gained
/// through `AssociateAccounts` is tested too.
const LATE_ACCOUNT: &str = "555555555555";

/// An account released from the home group again, so membership LOST through
/// `DisassociateAccounts` is tested too.
const RELEASED_ACCOUNT: &str = "666666666666";

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — Billing Conductor's control plane is authorized
    /// for the payer account alone, so a designated payer is what makes this surface
    /// reachable at all.
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

    /// Send a JSON body to one of this service's routes and return the status and the
    /// parsed body. The method matters: the batch association routes are `PUT`
    /// (`smithy_query(query="operation",operation=
    /// "BatchAssociateResourcesToCustomLineItem")`).
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
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

    /// Create a billing group billing the accounts `linked`, and return its ARN.
    async fn billing_group(&self, name: &str, linked: &[&str]) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": linked },
                    "ComputationPreference": { "PricingPlanArn": MANAGED_PRICING_PLAN },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case billing group {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("billing group {name:?}"))
    }

    /// `CreateCustomLineItem` with the members `extra` carries on top of the ones every
    /// create needs: the raw status and body, because half of these rows are refusals.
    async fn create_item(&self, group_arn: &str, name: &str, extra: &Value) -> (u16, Value) {
        let mut body = json!({
            "Name": name,
            "Description": "a custom line item",
            "BillingGroupArn": group_arn,
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
        });
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        self.send("POST", "/create-custom-line-item", &body).await
    }

    /// A flat custom line item, asserted to be created: the resource other rows name.
    async fn flat_item(&self, group_arn: &str, name: &str) -> String {
        let (status, body) = self.create_item(group_arn, name, &json!({})).await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("custom line item {name:?}"))
    }

    /// A percentage custom line item, asserted to be created: the only kind resources
    /// are associated to.
    async fn percentage_item(&self, group_arn: &str, name: &str) -> String {
        let (status, body) = self
            .create_item(
                group_arn,
                name,
                &json!({
                    "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 10.0 } }
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case percentage item {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("percentage item {name:?}"))
    }

    /// `BatchAssociateResourcesToCustomLineItem` with the resources named exactly as
    /// the caller spells them.
    async fn associate(&self, target: &str, resources: &[String]) -> (u16, Value) {
        self.send(
            "PUT",
            "/batch-associate-resources-to-custom-line-item",
            &json!({ "TargetArn": target, "ResourceArns": resources }),
        )
        .await
    }

    /// The `(Arn, Relationship)` pairs `ListResourcesAssociatedToCustomLineItem`
    /// reports for an item, sorted.
    async fn associations(&self, case: &str, target: &str) -> Vec<(String, String)> {
        let (status, body) = self
            .send(
                "POST",
                "/list-resources-associated-to-custom-line-item",
                &json!({ "Arn": target }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading the associations, got {status} with {body}"
        );
        let mut pairs: Vec<(String, String)> = body
            .get("AssociatedResources")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|element| {
                (
                    string_at(element, "Arn"),
                    string_at(element, "Relationship"),
                )
            })
            .collect();
        pairs.sort();
        pairs
    }

    /// The `Name`s `ListCustomLineItems` reports for the request's `Filters`.
    async fn listed_names(&self, case: &str, filters: Value) -> Vec<String> {
        let (status, body) = self
            .send("POST", "/list-custom-line-items", &json!({ "Filters": filters }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 listing the items, got {status} with {body}"
        );
        body.get("CustomLineItems")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|element| string_at(element, "Name"))
            .collect()
    }
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, label: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected an Arn member, got {body}"))
        .to_string()
}

/// One string member of a response element, or the empty string when it carries none.
fn string_at(element: &Value, member: &str) -> String {
    element
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
        .unwrap_or(&Vec::new())
        .iter()
        .map(|field| string_at(field, "Name"))
        .collect()
}

/// The bare resource id of a full ARN — the OTHER form every association member admits
/// (`^(arn:...:(customlineitem|billinggroup)/)?[a-zA-Z0-9]{10,12}$`).
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// The resources of the two billing groups a row can name, so a row names a resource
/// without knowing the ARNs the store minted.
struct World {
    emulator: Emulator,
    /// The billing group the custom line item under test applies to.
    home: String,
    /// A custom line item of the home group.
    sibling: String,
    /// Another billing group, which the home group's items have nothing to do with.
    foreign: String,
    /// A custom line item of that other billing group.
    foreigners_item: String,
}

impl World {
    /// Two billing groups, each with one custom line item of its own, and the accounts
    /// each group bills.
    async fn new() -> Self {
        let emulator = Emulator::new();
        let home = emulator
            .billing_group("home", &[HOME_ACCOUNT, OTHER_HOME_ACCOUNT, RELEASED_ACCOUNT])
            .await;
        let foreign = emulator.billing_group("foreign", &[FOREIGN_ACCOUNT]).await;
        let sibling = emulator.flat_item(&home, "sibling").await;
        let foreigners_item = emulator.flat_item(&foreign, "foreignitem").await;
        Self {
            emulator,
            home,
            sibling,
            foreign,
            foreigners_item,
        }
    }

    /// The resource a row names, in the form the row asks for.
    fn resource(&self, resource: Resource) -> String {
        match resource {
            Resource::OwnGroup => self.home.clone(),
            Resource::OwnGroupById => bare_id(&self.home),
            Resource::SiblingItem => self.sibling.clone(),
            Resource::SiblingItemById => bare_id(&self.sibling),
            Resource::OtherGroup => self.foreign.clone(),
            Resource::OtherGroupById => bare_id(&self.foreign),
            Resource::OtherGroupsItem => self.foreigners_item.clone(),
            Resource::OtherGroupsItemById => bare_id(&self.foreigners_item),
        }
    }

    /// The FULL ARN of the resource a row names, which is what a read reports whichever
    /// form the request used.
    fn resolved(&self, resource: Resource) -> String {
        match resource {
            Resource::OwnGroup | Resource::OwnGroupById => self.home.clone(),
            Resource::SiblingItem | Resource::SiblingItemById => self.sibling.clone(),
            Resource::OtherGroup | Resource::OtherGroupById => self.foreign.clone(),
            Resource::OtherGroupsItem | Resource::OtherGroupsItemById => {
                self.foreigners_item.clone()
            }
        }
    }
}

/// Which resource a row names, and in which of the two forms the member admits.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// The billing group the item applies to, by full ARN.
    OwnGroup,
    /// The billing group the item applies to, by bare id.
    OwnGroupById,
    /// A custom line item of that same billing group, by full ARN.
    SiblingItem,
    /// A custom line item of that same billing group, by bare id.
    SiblingItemById,
    /// Another billing group, by full ARN.
    OtherGroup,
    /// Another billing group, by bare id.
    OtherGroupById,
    /// A custom line item of another billing group, by full ARN.
    OtherGroupsItem,
    /// A custom line item of another billing group, by bare id.
    OtherGroupsItemById,
}

/// Every class of resource, on both sides of the scope and in both forms the member
/// admits: a percentage custom line item is created with the associated resources its
/// own billing group carries, and refused — with the modelled reason, naming the member
/// at fault, having persisted nothing — for any other resource.
#[tokio::test]
async fn an_associated_value_is_in_scope_of_the_billing_group_the_item_applies_to() {
    // (case, the resource the create's `AssociatedValues` names, the relationship it
    // holds when accepted, the `ValidationExceptionReason` when refused)
    // `ValidationExceptionReason` values from
    // `smithy_query(query="shape",shape="ValidationExceptionReason")`.
    let cases: [(&str, Resource, Option<&str>, Option<&str>); 8] = [
        ("the own billing group by ARN", Resource::OwnGroup, Some("PARENT"), None),
        (
            "the own billing group by bare id",
            Resource::OwnGroupById,
            Some("PARENT"),
            None,
        ),
        (
            "a custom line item of the same group by ARN",
            Resource::SiblingItem,
            Some("CHILD"),
            None,
        ),
        (
            "a custom line item of the same group by bare id",
            Resource::SiblingItemById,
            Some("CHILD"),
            None,
        ),
        (
            "another billing group by ARN",
            Resource::OtherGroup,
            None,
            Some("MISMATCHED_BILLINGGROUP_ARN"),
        ),
        (
            "another billing group by bare id",
            Resource::OtherGroupById,
            None,
            Some("MISMATCHED_BILLINGGROUP_ARN"),
        ),
        (
            "a custom line item of another group by ARN",
            Resource::OtherGroupsItem,
            None,
            Some("ILLEGAL_CHILD_ASSOCIATE_RESOURCE"),
        ),
        (
            "a custom line item of another group by bare id",
            Resource::OtherGroupsItemById,
            None,
            Some("ILLEGAL_CHILD_ASSOCIATE_RESOURCE"),
        ),
    ];

    for (case, resource, relationship, refused) in cases {
        let world = World::new().await;
        let named = world.resource(resource);
        let (status, body) = world
            .emulator
            .create_item(
                &world.home,
                "subject",
                &json!({
                    "ChargeDetails": {
                        "Type": "FEE",
                        "Percentage": { "PercentageValue": 5.0, "AssociatedValues": [named] },
                    }
                }),
            )
            .await;

        match (relationship, refused) {
            (Some(relationship), _) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {body}"
                );
                let arn = arn_of(&body, case);
                assert_eq!(
                    world.emulator.associations(case, &arn).await,
                    vec![(world.resolved(resource), relationship.to_string())],
                    "case {case:?}: expected the resource persisted as a {relationship} of the \
                     created item"
                );
            }
            (None, expected) => {
                let expected = expected.unwrap_or_default();
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected the modelled validation error, got {body}"
                );
                assert_eq!(
                    body.get("Reason").and_then(Value::as_str),
                    Some(expected),
                    "case {case:?}: expected the reason {expected}, got {body}"
                );
                assert_eq!(
                    field_names(&body),
                    vec!["ChargeDetails.Percentage.AssociatedValues".to_string()],
                    "case {case:?}: expected the member at fault named in Fields, got {body}"
                );
                assert!(
                    world
                        .emulator
                        .listed_names(case, json!({ "Names": ["subject"] }))
                        .await
                        .is_empty(),
                    "case {case:?}: expected a refused create to mint no custom line item"
                );
            }
        }
    }
}

/// A list of several associated values is refused for the one value out of scope,
/// wherever it sits in the list, and nothing of the request is persisted — not the item
/// and not the associations its in-scope values would have held.
#[tokio::test]
async fn one_out_of_scope_value_refuses_the_whole_create() {
    // (case, the resources the create's `AssociatedValues` names, the reason)
    let rows: [(&str, Vec<Resource>, &str); 4] = [
        (
            "out of scope first",
            vec![Resource::OtherGroup, Resource::OwnGroup],
            "MISMATCHED_BILLINGGROUP_ARN",
        ),
        (
            "out of scope last",
            vec![Resource::OwnGroup, Resource::OtherGroupsItem],
            "ILLEGAL_CHILD_ASSOCIATE_RESOURCE",
        ),
        (
            "out of scope between two in scope",
            vec![
                Resource::OwnGroup,
                Resource::OtherGroupsItemById,
                Resource::SiblingItem,
            ],
            "ILLEGAL_CHILD_ASSOCIATE_RESOURCE",
        ),
        (
            "two resources of another group",
            vec![Resource::OtherGroup, Resource::OtherGroupsItem],
            "MISMATCHED_BILLINGGROUP_ARN",
        ),
    ];

    for (case, resources, expected) in rows {
        let world = World::new().await;
        let named: Vec<String> = resources
            .iter()
            .map(|resource| world.resource(*resource))
            .collect();
        let (status, body) = world
            .emulator
            .create_item(
                &world.home,
                "subject",
                &json!({
                    "ChargeDetails": {
                        "Type": "FEE",
                        "Percentage": { "PercentageValue": 5.0, "AssociatedValues": named },
                    }
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the reason {expected}, got {body}"
        );
        assert!(
            world
                .emulator
                .listed_names(case, json!({ "Names": ["subject"] }))
                .await
                .is_empty(),
            "case {case:?}: expected a refused create to mint no custom line item"
        );
        // The sibling item of the home group holds nothing either: the refused create
        // wrote no association anywhere.
        assert!(
            world
                .emulator
                .associations(case, &world.sibling)
                .await
                .is_empty(),
            "case {case:?}: expected a refused create to persist no association"
        );
    }
}

/// `BatchAssociateResourcesToCustomLineItem` decides the same scope per resource: an
/// out-of-scope resource is its own failed element, classified by the modelled
/// `AssociateResourceErrorReason`, while the in-scope resources of the same request are
/// associated.
#[tokio::test]
async fn a_batch_association_reports_an_out_of_scope_resource_as_its_own_failure() {
    // (case, the resource the request names beside the item's own billing group,
    // whether it can be associated)
    let cases: [(&str, Resource, bool); 4] = [
        ("a custom line item of the same group", Resource::SiblingItem, true),
        (
            "a custom line item of the same group by bare id",
            Resource::SiblingItemById,
            true,
        ),
        ("another billing group", Resource::OtherGroup, false),
        (
            "a custom line item of another group",
            Resource::OtherGroupsItem,
            false,
        ),
    ];

    for (case, resource, associable) in cases {
        let world = World::new().await;
        let target = world.emulator.percentage_item(&world.home, "target").await;
        let named = world.resource(resource);
        let (status, body) = world
            .emulator
            .associate(&target, &[world.home.clone(), named.clone()])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a per-resource outcome is still a 200 batch answer, got {status} \
             with {body}"
        );

        let succeeded: Vec<String> = body
            .get("SuccessfullyAssociatedResources")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|element| string_at(element, "Arn"))
            .collect();
        let failed: Vec<(String, String)> = body
            .get("FailedAssociatedResources")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .map(|element| {
                (
                    string_at(element, "Arn"),
                    element
                        .get("Error")
                        .map(|error| string_at(error, "Reason"))
                        .unwrap_or_default(),
                )
            })
            .collect();

        let mut expected_associations = vec![(world.home.clone(), "PARENT".to_string())];
        if associable {
            assert_eq!(
                succeeded,
                vec![world.home.clone(), named.clone()],
                "case {case:?}: expected both resources associated as the caller named them, \
                 got {body}"
            );
            assert!(
                failed.is_empty(),
                "case {case:?}: expected no failed resource, got {failed:?}"
            );
            expected_associations.push((world.resolved(resource), "CHILD".to_string()));
        } else {
            assert_eq!(
                succeeded,
                vec![world.home.clone()],
                "case {case:?}: expected the in-scope resource still associated, got {body}"
            );
            assert_eq!(
                failed,
                vec![(named.clone(), "ILLEGAL_CUSTOMLINEITEM".to_string())],
                "case {case:?}: expected the out-of-scope resource reported failed with the \
                 modelled reason, got {body}"
            );
        }
        expected_associations.sort();
        assert_eq!(
            world.emulator.associations(case, &target).await,
            expected_associations,
            "case {case:?}: expected only the in-scope resources to be persisted"
        );
    }
}

/// How a row names the account the custom line item is applied to.
#[derive(Debug, Clone, Copy)]
enum Account {
    /// An account the home billing group has billed since it was created.
    Member,
    /// Another such account, so acceptance does not depend on which member it is.
    SecondMember,
    /// An account `AssociateAccounts` added to the home group after the fact.
    AssociatedLater,
    /// An account `DisassociateAccounts` released from the home group again.
    Released,
    /// An account another billing group bills.
    OtherGroups,
    /// An account no billing group bills.
    Unassociated,
    /// No `AccountId` at all: the item applies to the whole billing group.
    None,
}

/// The account the item is applied to is an account the billing group bills: the
/// members it was created with and the ones associated later are accepted and reported
/// back by `Filters.AccountIds`, while an account the group does not bill — another
/// group's, one in no group, one released from this group — is refused with the modelled
/// reason and nothing is persisted.
#[tokio::test]
async fn the_account_an_item_is_applied_to_is_one_the_billing_group_bills() {
    // (case, how the row names the account, the account id, accepted)
    let cases: [(&str, Account, Option<&str>, bool); 7] = [
        ("a member of the group", Account::Member, Some(HOME_ACCOUNT), true),
        (
            "another member of the group",
            Account::SecondMember,
            Some(OTHER_HOME_ACCOUNT),
            true,
        ),
        (
            "an account associated after the group was created",
            Account::AssociatedLater,
            Some(LATE_ACCOUNT),
            true,
        ),
        ("no AccountId at all", Account::None, None, true),
        (
            "an account of another billing group",
            Account::OtherGroups,
            Some(FOREIGN_ACCOUNT),
            false,
        ),
        (
            "an account of no billing group",
            Account::Unassociated,
            Some(UNASSOCIATED_ACCOUNT),
            false,
        ),
        (
            "an account released from the group",
            Account::Released,
            Some(RELEASED_ACCOUNT),
            false,
        ),
    ];

    for (case, account, account_id, accepted) in cases {
        let world = World::new().await;
        match account {
            Account::AssociatedLater => {
                let (status, body) = world
                    .emulator
                    .send(
                        "POST",
                        "/associate-accounts",
                        &json!({ "Arn": world.home, "AccountIds": [LATE_ACCOUNT] }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the account to be associated, got {status} with \
                     {body}"
                );
            }
            Account::Released => {
                let (status, body) = world
                    .emulator
                    .send(
                        "POST",
                        "/disassociate-accounts",
                        &json!({ "Arn": world.home, "AccountIds": [RELEASED_ACCOUNT] }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the account to be released, got {status} with {body}"
                );
            }
            _ => {}
        }

        let extra = match account_id {
            Some(account_id) => json!({ "AccountId": account_id }),
            None => json!({}),
        };
        let (status, body) = world
            .emulator
            .create_item(&world.home, "subject", &extra)
            .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            // Read the item back the way a caller selects on the account it applies to.
            let filters = match account_id {
                Some(account_id) => json!({ "AccountIds": [account_id] }),
                None => json!({ "Names": ["subject"] }),
            };
            assert_eq!(
                world.emulator.listed_names(case, filters).await,
                vec!["subject".to_string()],
                "case {case:?}: expected the created item reported for the account it applies to"
            );
        } else {
            let account_id = account_id.unwrap_or_default();
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected the modelled validation error, got {body}"
            );
            assert_eq!(
                body.get("Reason").and_then(Value::as_str),
                Some("ILLEGAL_ACCOUNT_ID"),
                "case {case:?}: expected the modelled reason, got {body}"
            );
            assert_eq!(
                field_names(&body),
                vec!["AccountId".to_string()],
                "case {case:?}: expected the member at fault named in Fields, got {body}"
            );
            assert!(
                world
                    .emulator
                    .listed_names(case, json!({ "AccountIds": [account_id] }))
                    .await
                    .is_empty(),
                "case {case:?}: expected no item reported under an account the group does not \
                 bill"
            );
        }
    }
}
