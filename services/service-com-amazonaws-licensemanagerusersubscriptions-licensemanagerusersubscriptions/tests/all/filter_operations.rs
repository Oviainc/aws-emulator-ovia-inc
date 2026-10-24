// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the comparisons a `Filter.Operation` can name, driven over the
//! real wire path: every call goes through the router's `dispatch`, so the request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * `Filter.Operation` names a comparison, and the whole relational family the model
//!   documents by example (`eq`, `ne`, `gt`, `geq`, `lt`, `leq`) is honoured — under the
//!   short spelling or its PascalCase name, in any letter case — alongside the substring
//!   searches; on the string attributes these lists expose, the ordering comparisons order
//!   values lexicographically;
//! * a spelling outside that set is a `ValidationException` (HTTP 400) naming the
//!   spelling that was refused;
//! * the rule belongs to the ONE filter helper every list operation of this service calls,
//!   so it holds for `ListProductSubscriptions`, `ListIdentityProviders`, `ListInstances`
//!   and `ListLicenseServerEndpoints` alike;
//! * the ordering comparisons are complements: for any value, `lt` and `geq` partition a
//!   list exactly, and so do `leq` and `gt`.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ops::types::{
    ActiveDirectoryIdentityProvider, IdentityProvider,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState, InstanceRecord,
};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// registration helpers come from the sibling suites rather than a second copy.
use crate::identity_provider_subscriptions::{
    as_str, call, harness, provider_union, register, Harness, Reply, ACCOUNT, PRODUCTS,
};
use crate::license_server_endpoints::seed as seed_endpoints;

/// A comparison a `Filter.Operation` can name, and what it means for a string attribute.
///
/// The ordering members are the family the model's own examples are drawn from: "The type
/// of search (For example, eq, geq, leq)" (smithy_query doc, shape `Filter`, member
/// `Operation`).
#[derive(Debug, Clone, Copy)]
enum Comparison {
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanOrEquals,
    LessThan,
    LessThanOrEquals,
    Contains,
    BeginsWith,
}

impl Comparison {
    /// Whether an attribute holding `actual` satisfies this comparison against `wanted`.
    fn holds(self, actual: &str, wanted: &str) -> bool {
        match self {
            Self::Equals => actual == wanted,
            Self::NotEquals => actual != wanted,
            Self::GreaterThan => actual > wanted,
            Self::GreaterThanOrEquals => actual >= wanted,
            Self::LessThan => actual < wanted,
            Self::LessThanOrEquals => actual <= wanted,
            Self::Contains => actual.contains(wanted),
            Self::BeginsWith => actual.starts_with(wanted),
        }
    }
}

/// The spellings a client may write, with the comparison each one names. The rows differ
/// along the form of the spelling: the short relational one the model documents by
/// example, its PascalCase name, and a letter case neither is written in.
const SPELLINGS: &[(&str, Comparison)] = &[
    ("eq", Comparison::Equals),
    ("Equals", Comparison::Equals),
    ("EQ", Comparison::Equals),
    ("ne", Comparison::NotEquals),
    ("NotEquals", Comparison::NotEquals),
    ("gt", Comparison::GreaterThan),
    ("GreaterThan", Comparison::GreaterThan),
    ("geq", Comparison::GreaterThanOrEquals),
    ("GEQ", Comparison::GreaterThanOrEquals),
    ("GreaterThanOrEquals", Comparison::GreaterThanOrEquals),
    ("lt", Comparison::LessThan),
    ("LessThan", Comparison::LessThan),
    ("leq", Comparison::LessThanOrEquals),
    ("lessthanorequals", Comparison::LessThanOrEquals),
    ("LessThanOrEquals", Comparison::LessThanOrEquals),
    ("contains", Comparison::Contains),
    ("Contains", Comparison::Contains),
    ("beginswith", Comparison::BeginsWith),
    ("BeginsWith", Comparison::BeginsWith),
];

/// One list operation under test: how it is called, which member carries its page, which
/// member identifies a record, the filterable attribute being compared, and what the store
/// holds — `(identifier, the attribute's value)` for every record, in the order the list
/// reports them.
struct ListUnderTest {
    case: &'static str,
    path: &'static str,
    request: Value,
    items: &'static str,
    identifier: &'static str,
    attribute: &'static str,
    records: Vec<(String, String)>,
}

impl ListUnderTest {
    /// The identifiers this list reports under `filter`, which must be answered with HTTP
    /// 200: a spelling the service honours is not a refusal.
    async fn selected(&self, h: &Harness, filter: Value, case: &str) -> Vec<String> {
        let mut body = self.request.clone();
        body["Filters"] = json!([filter]);
        let reply = call(h, self.path, &body).await;
        let page = reply.ok(case);
        page.get(self.items)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: {} is missing from {page:?}", self.items))
            .iter()
            .map(|item| as_str(item, self.identifier, case).to_string())
            .collect()
    }

    /// What `comparison` selects from the store, in list order.
    fn expected(&self, comparison: Comparison, wanted: &str) -> Vec<String> {
        self.records
            .iter()
            .filter(|(_, value)| comparison.holds(value, wanted))
            .map(|(identifier, _)| identifier.clone())
            .collect()
    }

    /// The values the comparisons are probed with: every value in the store (so the
    /// boundary `geq`/`leq` turn on is exercised), one character short of a stored value
    /// and one character past it (the neighbouring boundaries `gt`/`lt` turn on), the empty
    /// value, and a value past everything stored.
    fn probes(&self) -> Vec<String> {
        let mut probes: Vec<String> = self
            .records
            .iter()
            .map(|(_, value)| value.clone())
            .collect();
        if let Some((_, first)) = self.records.first() {
            if first.len() > 1 {
                probes.push(first[..first.len() - 1].to_string());
            }
            probes.push(format!("{first}~"));
        }
        probes.push(String::new());
        probes.push("zzzzzzzz".to_string());
        probes.sort();
        probes.dedup();
        probes
    }
}

/// `ListProductSubscriptions` over the subscriptions of one registration, filtering on the
/// `Username` attribute its model documents.
async fn product_subscriptions_list() -> (Harness, ListUnderTest) {
    let h = harness();
    let directory_id = "d-0123456789";
    let product = PRODUCTS[0];
    register(
        &h,
        &json!({
            "IdentityProvider": provider_union(directory_id),
            "Product": product,
            "Settings": {
                "Subnets": ["subnet-0123456789abcdef0"],
                "SecurityGroupId": "sg-0123abcd",
            },
        }),
    )
    .await
    .ok("register the provider the subscriptions are started through");

    // The user names differ along the form the rule generalises over: a plain name, a UPN,
    // a name carrying punctuation, and a name that extends another.
    let usernames = ["alice", "alice.admin", "bob", "carol@example.com"];
    let mut records = Vec::new();
    for username in usernames {
        let reply = call(
            &h,
            "/user/StartProductSubscription",
            &json!({
                "Username": username,
                "IdentityProvider": provider_union(directory_id),
                "Product": product,
            }),
        )
        .await;
        let summary = reply.ok(username)["ProductUserSummary"].clone();
        records.push((
            as_str(&summary, "ProductUserArn", username).to_string(),
            username.to_string(),
        ));
    }

    (
        h,
        ListUnderTest {
            case: "ListProductSubscriptions",
            path: "/user/ListProductSubscriptions",
            request: json!({
                "IdentityProvider": provider_union(directory_id),
                "Product": product,
            }),
            items: "ProductUserSummaries",
            identifier: "ProductUserArn",
            attribute: "Username",
            records,
        },
    )
}

/// `ListIdentityProviders` over several registrations, filtering on the `DirectoryId`
/// attribute its model documents.
async fn identity_providers_list() -> (Harness, ListUnderTest) {
    let h = harness();
    // The directory ids the harness registers in the account's fleet, plus a shared one
    // (`sd-`), so the values compared differ in their prefix as well as their digits.
    let registrations = [
        ("d-0123456789", PRODUCTS[0]),
        ("d-00000000ff", PRODUCTS[1]),
        ("d-123456789a", PRODUCTS[2]),
        ("sd-abcdef0123", PRODUCTS[3]),
    ];
    let mut records = Vec::new();
    for (directory_id, product) in registrations {
        let reply = register(
            &h,
            &json!({
                "IdentityProvider": provider_union(directory_id),
                "Product": product,
                "Settings": {
                    "Subnets": ["subnet-0123456789abcdef0"],
                    "SecurityGroupId": "sg-0123abcd",
                },
            }),
        )
        .await;
        let summary = reply.ok(directory_id)["IdentityProviderSummary"].clone();
        records.push((
            as_str(&summary, "IdentityProviderArn", directory_id).to_string(),
            directory_id.to_string(),
        ));
    }

    (
        h,
        ListUnderTest {
            case: "ListIdentityProviders",
            path: "/identity-provider/ListIdentityProviders",
            request: json!({}),
            items: "IdentityProviderSummaries",
            identifier: "IdentityProviderArn",
            attribute: "DirectoryId",
            records,
        },
    )
}

/// `ListInstances` over seeded instances, filtering on the `Status` attribute its model
/// documents.
///
/// An instance enters this service's view through the user-association plane, so the store
/// is written through the same `upsert_instance` seam that writer uses — as the
/// `list_instances` suite does.
fn instances_list() -> (Harness, ListUnderTest) {
    let h = harness();
    // The statuses differ along the value the rule orders: two instances share one status,
    // so a comparison can select several, and the values span the alphabet.
    let seeded = [
        ("i-0123456789abcdef0", "ACTIVE"),
        ("i-abcdef01", "REGISTERED"),
        ("i-00000000000000001", "ACTIVE"),
        ("i-0000beef", "TERMINATED"),
    ];
    let store = h
        .engine
        .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    {
        let mut guard = store.lock();
        for (instance_id, status) in seeded {
            guard.upsert_instance(InstanceRecord {
                instance_id: instance_id.to_string(),
                status: status.to_string(),
                products: vec![PRODUCTS[0].to_string()],
                status_message: None,
                owner_account_id: ACCOUNT.to_string(),
                identity_provider: Some(IdentityProvider::ActiveDirectoryIdentityProvider(
                    ActiveDirectoryIdentityProvider {
                        directory_id: Some("d-0123456789".to_string()),
                        ..Default::default()
                    },
                )),
                last_status_check_at: None,
            });
        }
    }

    (
        h,
        ListUnderTest {
            case: "ListInstances",
            path: "/instance/ListInstances",
            request: json!({}),
            items: "InstanceSummaries",
            identifier: "InstanceId",
            attribute: "Status",
            records: seeded
                .iter()
                .map(|(instance_id, status)| ((*instance_id).to_string(), (*status).to_string()))
                .collect(),
        },
    )
}

/// `ListLicenseServerEndpoints` over created endpoints, filtering on the
/// `IdentityProviderArn` attribute its model documents.
async fn license_server_endpoints_list() -> (Harness, ListUnderTest) {
    let h = harness();
    let created = seed_endpoints(&h).await;
    let records = created
        .iter()
        .map(|(_, provider_arn, endpoint_arn)| (endpoint_arn.clone(), provider_arn.clone()))
        .collect();

    (
        h,
        ListUnderTest {
            case: "ListLicenseServerEndpoints",
            path: "/license-server/ListLicenseServerEndpoints",
            request: json!({}),
            items: "LicenseServerEndpoints",
            identifier: "LicenseServerEndpointArn",
            attribute: "IdentityProviderArn",
            records,
        },
    )
}

/// Every list operation of this service that takes `Filters`, each on its own emulator so
/// one list's records are exactly what it seeded.
async fn every_list() -> Vec<(Harness, ListUnderTest)> {
    vec![
        product_subscriptions_list().await,
        identity_providers_list().await,
        instances_list(),
        license_server_endpoints_list().await,
    ]
}

/// RULE: a `Filter.Operation` naming a comparison in the relational family the model
/// documents by example, or a substring search, selects exactly the records whose
/// filterable attribute satisfies that comparison — on every list operation, whichever
/// spelling and letter case the client wrote.
#[tokio::test]
async fn every_documented_comparison_selects_on_every_list() {
    for (h, list) in every_list().await {
        for wanted in list.probes() {
            for (spelling, comparison) in SPELLINGS {
                let case = format!(
                    "{}: {} {spelling:?} {wanted:?}",
                    list.case, list.attribute
                );
                let expected = list.expected(*comparison, &wanted);
                let got = list
                    .selected(
                        &h,
                        json!({
                            "Attribute": list.attribute,
                            "Operation": spelling,
                            "Value": wanted,
                        }),
                        &case,
                    )
                    .await;
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected {expected:?}, got {got:?}"
                );
            }
        }
    }
}

/// RULE (invariant): the ordering comparisons are complements — for any value, `lt` and
/// `geq` partition a list exactly, and so do `leq` and `gt`.
#[tokio::test]
async fn the_ordering_comparisons_partition_every_list() {
    for (h, list) in every_list().await {
        let all: Vec<String> = list
            .records
            .iter()
            .map(|(identifier, _)| identifier.clone())
            .collect();
        for wanted in list.probes() {
            for (below, above) in [("lt", "geq"), ("leq", "gt")] {
                let case = format!(
                    "{}: {} {below:?}/{above:?} {wanted:?}",
                    list.case, list.attribute
                );
                let mut partitioned = Vec::new();
                for operation in [below, above] {
                    partitioned.extend(
                        list.selected(
                            &h,
                            json!({
                                "Attribute": list.attribute,
                                "Operation": operation,
                                "Value": wanted,
                            }),
                            &case,
                        )
                        .await,
                    );
                }
                let mut sorted = partitioned.clone();
                sorted.sort();
                sorted.dedup();
                assert_eq!(
                    sorted.len(),
                    partitioned.len(),
                    "case {case:?}: the two comparisons must be disjoint, got {partitioned:?}"
                );
                let mut expected = all.clone();
                expected.sort();
                assert_eq!(
                    sorted, expected,
                    "case {case:?}: together they must cover the list, expected {expected:?}, got {sorted:?}"
                );
            }
        }
    }
}

/// RULE: a spelling outside the comparisons this service implements is a
/// `ValidationException` (HTTP 400) naming the spelling that was refused — on every list
/// operation, and whatever the store holds.
#[tokio::test]
async fn a_spelling_outside_the_documented_comparisons_is_refused() {
    // Each row is a spelling no comparison of the family answers to: a query language's
    // operator, a symbol, a near-miss of a supported name, a surrounding-whitespace
    // variant, and the empty spelling.
    let refused = [
        "like", "between", "in", "regex", ">=", "=<", "equal", "greater", "beginwith", " eq",
        "eq ", "",
    ];
    for (h, list) in every_list().await {
        for spelling in refused {
            let case = format!("{}: {} {spelling:?}", list.case, list.attribute);
            let mut body = list.request.clone();
            body["Filters"] = json!([{
                "Attribute": list.attribute,
                "Operation": spelling,
                "Value": "x",
            }]);
            let reply = call(&h, list.path, &body).await;
            assert_refused(&reply, spelling, &case);
        }
    }

    // Well-formedness does not depend on the store, so the same spellings are refused by a
    // list whose store holds nothing.
    let empty = harness();
    for spelling in refused {
        let case = format!("empty store: {spelling:?}");
        let reply = call(
            &empty,
            "/instance/ListInstances",
            &json!({ "Filters": [{ "Attribute": "Status", "Operation": spelling, "Value": "x" }] }),
        )
        .await;
        assert_refused(&reply, spelling, &case);
    }
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) and that its message
/// names the spelling the request carried.
fn assert_refused(reply: &Reply, spelling: &str, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {reply:?}"
    );
    let message = reply.message().unwrap_or_default();
    assert!(
        message.contains(spelling),
        "case {case:?}: the refusal must name the spelling it refused, got {message:?}"
    );
}
