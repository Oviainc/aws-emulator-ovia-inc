// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE page-size rule every paginated Firewall Manager
//! operation resolves through.
//!
//! RULE under test: a paginated operation serves at most `MaxResults` items in a
//! page and at most the maximum the model's `com.amazonaws.fms#PaginationMaxResults`
//! declares for it (range 1..100) — including a request that names no
//! `MaxResults` at all — and the items a bound keeps off a page are reachable
//! through the `NextToken` that page carries, so every page size serves the same
//! collection, once, in the same order. The rule does not depend on which
//! operation is asked, nor on whether its own `MaxResults` member is optional or
//! required.
//!
//! The table in [`rows`] is the class the rule ranges over: one row per paginated
//! operation of this service, the rows differing in the store the page is drawn
//! from (policies, organization accounts, resource sets, one resource set's
//! membership, applications and
//! protocols lists, a vendor's firewall policies, compliance entries, the
//! organization's administrators, a Shield Advanced attack inventory, and the
//! cross-service inventory `ListDiscoveredResources` discovers), in whether the
//! page is a modelled list of strings, a modelled list of structures or the JSON
//! document `GetProtectionStatus` reports, and in whether the model makes
//! `MaxResults` optional or REQUIRED. Each row is walked at several page sizes,
//! including both published bounds of the modelled range and sizes that divide
//! the collection evenly and unevenly.
//!
//! Every call goes over the wire through the harness in
//! [`crate::policy_lifecycle`], on one router with Amazon EC2 mounted beside
//! Firewall Manager so the inventory `ListDiscoveredResources` pages is a real
//! cross-service read.
//!
//! [`the_page_size_rule_clamps_every_max_results_into_the_modelled_range`] pins
//! the edges of the rule the wire cannot reach: a `MaxResults` outside the
//! modelled range is refused before a handler sees it (the `protection_status`
//! and `third_party_firewall_firewall_policies` suites assert that refusal), so
//! the resolution's own behaviour on such a value is asserted directly.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::THIRD_PARTY_FIREWALL_VALUES;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness, the administrator gate, the store seam and the seeding
// helpers all live in one place for this test target, so no two suites can
// drift in how a request is signed or a resource is seeded.
use crate::admin_account_scopes::{put_admin_account, wait_out_the_onboarding};
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, ACCOUNT, REGION};
use crate::protection_status::{seed_attack, Attack};
use crate::tagging_and_managing_admins::minimal_policy;

/// The largest page any of these operations serves: the maximum the model's
/// `com.amazonaws.fms#PaginationMaxResults` declares for `MaxResults` (range
/// 1..100, from `smithy_query(query="shape", shape="PaginationMaxResults")`).
///
/// Deliberately an INDEPENDENT expectation, read off the model rather than
/// imported from `state::MAX_PAGE_SIZE`: if the two ever disagree, this suite
/// says so instead of agreeing with a wrong constant.
const MAX_PAGE_SIZE: usize = 100;

/// The smallest page the same range allows.
const MIN_PAGE_SIZE: usize = 1;

/// How many items the seeding below leaves in each collection, except the
/// organization accounts (see [`SEEDED_MEMBER_ACCOUNTS`]).
const SEEDED: usize = 5;

/// The organization accounts the seeding leaves: the [`SCOPE_ACCOUNTS`] a stored
/// policy names, the associated administrator's own account, and every account
/// onboarded as a delegated administrator.
const SEEDED_MEMBER_ACCOUNTS: usize = SCOPE_ACCOUNTS.len() + 1 + DELEGATED_ADMINS.len();

/// The accounts a stored policy names under the `ACCOUNT` key of its
/// `IncludeMap`, which is what puts them in the organization's accounts and in
/// that policy's compliance scope.
///
/// Every entry is an `AWSAccountId` as the model declares it
/// (`com.amazonaws.fms#AWSAccountId`: length 1..1024, `^[0-9]+$`), and the rows
/// differ in length and digits, including the one-digit and the long forms.
/// None of them is [`ACCOUNT`], so the administrator's own account is an extra
/// member account rather than one of these.
const SCOPE_ACCOUNTS: &[&str] = &[
    "111111111111",
    "222222222222",
    "3",
    "44444444444444444444",
    "555555555555",
];

/// The accounts onboarded as delegated administrators, each scoped to manage
/// [`ACCOUNT`], so they are reported both by `ListAdminAccountsForOrganization`
/// and by `ListAdminsManagingAccount` beside the default administrator.
const DELEGATED_ADMINS: &[&str] = &["222233334444", "7", "000000000001", "99999999999999999999"];

/// The image id the EC2 launches name. Any well-formed `ami-` id serves: what is
/// paged here is the instances that end up in EC2's store, not the image.
const AMI: &str = "ami-0123456789abcdef0";

/// Where a response carries its page, and how one item in it is identified.
enum Page {
    /// A modelled list member whose items are strings.
    Strings(&'static str),
    /// A modelled list member whose items are structures, identified by one of
    /// their members.
    Structs(&'static str, &'static str),
    /// The `Data` JSON document `GetProtectionStatus` reports, whose entries are
    /// identified by their `AttackId`.
    ProtectionData,
}

impl Page {
    /// The items one response page carries, in the order it served them.
    fn items(&self, case: &str, body: &Value) -> Vec<String> {
        match self {
            Self::Strings(member) => Self::list(case, body, member)
                .iter()
                .map(|item| Self::text(case, item))
                .collect(),
            Self::Structs(member, key) => Self::list(case, body, member)
                .iter()
                .map(|item| Self::text(case, &item[*key]))
                .collect(),
            Self::ProtectionData => {
                let document = body["Data"].as_str().unwrap_or_else(|| {
                    panic!("case {case}: the response must carry a Data document, got {body}")
                });
                let parsed: Value = serde_json::from_str(document).unwrap_or_else(|error| {
                    panic!("case {case}: Data must be a JSON document ({error}), got {document}")
                });
                parsed
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {case}: Data must be a JSON array, got {document}")
                    })
                    .iter()
                    .map(|entry| Self::text(case, &entry["AttackId"]))
                    .collect()
            }
        }
    }

    /// The modelled list member `member`, which a page always carries.
    fn list<'body>(case: &str, body: &'body Value, member: &str) -> &'body Vec<Value> {
        body[member].as_array().unwrap_or_else(|| {
            panic!("case {case}: the response must carry a {member} list, got {body}")
        })
    }

    /// One item's identity, which is always a string on the wire.
    fn text(case: &str, value: &Value) -> String {
        value
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a string item, got {value}"))
            .to_string()
    }
}

/// One row of the paginated-operation table.
struct Paginated {
    operation: &'static str,
    /// The request members beside `MaxResults` and `NextToken`.
    request: Value,
    page: Page,
    /// Whether the model makes `MaxResults` REQUIRED, so "no bound at all" is not
    /// a request this operation accepts.
    max_results_required: bool,
    /// How many items the seeding leaves this operation reporting.
    seeded: usize,
}

/// A fresh emulator: one shared engine with Firewall Manager and Amazon EC2 — the
/// service that owns the resources `ListDiscoveredResources` discovers — mounted
/// on one router, the way the emulator's own router assembles them.
fn emulator_with_ec2() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_fms_awsfms_20180101::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Store one policy and return the `PolicyId` the service minted for it.
async fn put_policy(router: &MultiServiceRouter, case: &str, policy: &Value) -> String {
    let (status, body) = call(router, "PutPolicy", &json!({ "Policy": policy })).await;
    assert_eq!(
        status, 200,
        "case {case}: expected PutPolicy to answer 200, got {status} with {body}"
    );
    body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: PutPolicy reported no PolicyId: {body}"))
        .to_string()
}

/// One call that must succeed, asserted as such.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected {operation} to answer 200, got {status} with {response}"
    );
    response
}

/// Launch `count` EC2 instances through EC2's own `ec2Query` API, so the
/// inventory `ListDiscoveredResources` pages was created by the service that
/// owns it.
async fn run_instances(router: &MultiServiceRouter, count: usize) {
    let body = format!(
        "Action=RunInstances&Version=2016-11-15&ImageId={AMI}&MinCount={count}&MaxCount={count}"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case run-instances: expected RunInstances to answer 200, got {status} with {xml}"
    );
    let minted = xml.matches("<instanceId>").count();
    assert_eq!(
        minted, count,
        "case run-instances: expected {count} instances, got {minted} in {xml}"
    );
}

/// The vendor whose tenant holds the seeded firewall policies: the first value of
/// the model's own `ThirdPartyFirewall` enum.
fn primary_vendor() -> &'static str {
    THIRD_PARTY_FIREWALL_VALUES
        .first()
        .expect("the model declares at least one ThirdPartyFirewall vendor")
}

/// A second vendor, whose tenant holds a DIFFERENT inventory, so a page of the
/// first vendor's policies cannot be a page of every vendor's.
fn other_vendor() -> &'static str {
    THIRD_PARTY_FIREWALL_VALUES
        .get(1)
        .expect("the model declares at least two ThirdPartyFirewall vendors")
}

/// An emulator with an administrator associated and every collection below
/// seeded, together with the table of operations that page them.
async fn seeded() -> (MultiServiceRouter, Vec<Paginated>) {
    let (router, engine) = emulator_with_ec2();
    associate_admin(&router).await;

    // A policy whose `IncludeMap` names the organization accounts: it is what
    // `ListMemberAccounts` and `ListComplianceStatus` draw their pages from.
    let mut scoped = minimal_policy("scoped-policy");
    scoped["IncludeMap"] = json!({ "ACCOUNT": SCOPE_ACCOUNTS });
    let scoped_policy_id = put_policy(&router, "scoped-policy", &scoped).await;
    // A Shield Advanced policy, the only type `GetProtectionStatus` reports for.
    // `AWS::ElasticLoadBalancingV2::LoadBalancer` is one of the resource types
    // `Policy$ResourceType` documents for it.
    let shield_policy_id = put_policy(
        &router,
        "shield-policy",
        &json!({
            "PolicyName": "shield-policy",
            "SecurityServicePolicyData": { "Type": "SHIELD_ADVANCED" },
            "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
            "ExcludeResourceTags": false,
            "RemediationEnabled": false
        }),
    )
    .await;
    for index in 0..(SEEDED - 2) {
        let name = format!("policy-{index}");
        put_policy(&router, &name, &minimal_policy(&name)).await;
    }

    for index in 0..SEEDED {
        let name = format!("set-{index}");
        ok(
            &router,
            &name,
            "PutResourceSet",
            &json!({
                "ResourceSet": {
                    "Name": name,
                    "ResourceTypeList": ["AWS::EC2::Instance"],
                },
            }),
        )
        .await;
        ok(
            &router,
            &name,
            "PutAppsList",
            &json!({
                "AppsList": {
                    "ListName": format!("apps-{index}"),
                    "AppsList": [{ "AppName": "https", "Protocol": "tcp", "Port": 443 }],
                },
            }),
        )
        .await;
        ok(
            &router,
            &name,
            "PutProtocolsList",
            &json!({
                "ProtocolsList": {
                    "ListName": format!("protocols-{index}"),
                    "ProtocolsList": ["TCP", "UDP"],
                },
            }),
        )
        .await;
    }

    // A resource set holding `SEEDED` resources, so the membership
    // `ListResourceSetResources` pages is a collection of its own. The resources
    // belong to an account the scoped policy above names, which is what puts
    // them in the organization and makes them associable.
    let created = ok(
        &router,
        "membership-set",
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": "membership-set",
                "ResourceTypeList": [state::RESOURCE_TYPE_EC2_INSTANCE],
            },
        }),
    )
    .await;
    let membership_set = created["ResourceSet"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case membership-set: PutResourceSet reported no Id: {created}"))
        .to_string();
    let members: Vec<String> = (0..SEEDED)
        .map(|index| {
            format!(
                "arn:aws:ec2:{REGION}:{}:instance/i-{index:017x}",
                SCOPE_ACCOUNTS[0]
            )
        })
        .collect();
    let associated = ok(
        &router,
        "membership-set",
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": membership_set, "Items": members }),
    )
    .await;
    assert_eq!(
        associated["FailedItems"],
        json!([]),
        "case membership-set: every seeded member must associate, got {associated}"
    );

    with_store(&engine, |store| {
        // A third-party firewall policy is created in the VENDOR's console and no
        // Firewall Manager operation creates one, so the only honest source of
        // one is something that put it in the store — as the
        // `third_party_firewall_firewall_policies` suite seeds it.
        for vendor in [primary_vendor(), other_vendor()] {
            store.third_party_firewalls.insert(
                vendor.to_string(),
                state::ThirdPartyFirewallRecord {
                    status: state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE.to_string(),
                    settles_to: None,
                    settles_at: None,
                    marketplace_onboarding_status: "COMPLETE".to_string(),
                },
            );
        }
        for index in 0..SEEDED {
            let record = state::ThirdPartyFirewallPolicyRecord {
                third_party_firewall: primary_vendor().to_string(),
                firewall_policy_id: format!("tenant-policy-{index}"),
                firewall_policy_name: format!("tenant policy {index}"),
            };
            store
                .third_party_firewall_policies
                .insert(record.key(), record);
        }
        // The other vendor's tenant holds a different, disjoint inventory.
        let decoy = state::ThirdPartyFirewallPolicyRecord {
            third_party_firewall: other_vendor().to_string(),
            firewall_policy_id: "other-vendor-policy".to_string(),
            firewall_policy_name: "the other vendor's policy".to_string(),
        };
        store
            .third_party_firewall_policies
            .insert(decoy.key(), decoy);

        // Shield Advanced detects attacks and this emulator stands in for no
        // attack detector, so the attacks are seeded through the store too. They
        // differ in start instant, duration, whether the attack is still ongoing
        // and attack-vector count.
        for (index, attack_id) in ATTACK_IDS.iter().enumerate() {
            let index_u64 = u64::try_from(index).expect("a small index fits a u64");
            seed_attack(
                store,
                &shield_policy_id,
                &Attack {
                    attack_id,
                    account: "111111111111",
                    start_secs: index_u64 * 600,
                    start_millis: index_u64 * 100,
                    duration_secs: if index % 2 == 0 {
                        Some(60 + index_u64)
                    } else {
                        None
                    },
                    vectors: if index % 2 == 0 {
                        &["REQUEST_FLOOD"]
                    } else {
                        &["SYN_FLOOD", "UDP_TRAFFIC"]
                    },
                },
            );
        }
    });

    // Delegated administrators, each scoped to manage this account, so both
    // administrator enumerations have a collection to page.
    let scope = json!({ "AccountScope": { "Accounts": [ACCOUNT] } });
    for account in DELEGATED_ADMINS {
        put_admin_account(&router, "delegated-admin", account, Some(&scope)).await;
    }
    // An administrator stays ONBOARDING for a span of the service's own choosing,
    // and only an onboarded one manages an account.
    wait_out_the_onboarding();

    run_instances(&router, SEEDED).await;

    let rows = vec![
        Paginated {
            operation: "ListPolicies",
            request: json!({}),
            page: Page::Structs("PolicyList", "PolicyId"),
            max_results_required: false,
            seeded: SEEDED,
        },
        Paginated {
            operation: "ListMemberAccounts",
            request: json!({}),
            page: Page::Strings("MemberAccounts"),
            max_results_required: false,
            seeded: SEEDED_MEMBER_ACCOUNTS,
        },
        Paginated {
            operation: "ListResourceSets",
            request: json!({}),
            page: Page::Structs("ResourceSets", "Id"),
            max_results_required: false,
            // The `SEEDED` sets of the loop above plus the one holding the
            // membership `ListResourceSetResources` pages.
            seeded: SEEDED + 1,
        },
        Paginated {
            operation: "ListResourceSetResources",
            request: json!({ "Identifier": membership_set }),
            page: Page::Structs("Items", "URI"),
            max_results_required: false,
            seeded: SEEDED,
        },
        Paginated {
            operation: "ListComplianceStatus",
            request: json!({ "PolicyId": scoped_policy_id }),
            page: Page::Structs("PolicyComplianceStatusList", "MemberAccount"),
            max_results_required: false,
            seeded: SCOPE_ACCOUNTS.len(),
        },
        Paginated {
            operation: "ListAdminAccountsForOrganization",
            request: json!({}),
            page: Page::Structs("AdminAccounts", "AdminAccount"),
            max_results_required: false,
            seeded: DELEGATED_ADMINS.len() + 1,
        },
        Paginated {
            operation: "ListAdminsManagingAccount",
            request: json!({}),
            page: Page::Strings("AdminAccounts"),
            max_results_required: false,
            seeded: DELEGATED_ADMINS.len() + 1,
        },
        Paginated {
            operation: "ListDiscoveredResources",
            request: json!({
                "MemberAccountIds": [ACCOUNT],
                "ResourceType": state::RESOURCE_TYPE_EC2_INSTANCE,
            }),
            page: Page::Structs("Items", "URI"),
            max_results_required: false,
            seeded: SEEDED,
        },
        Paginated {
            operation: "GetProtectionStatus",
            request: json!({ "PolicyId": shield_policy_id }),
            page: Page::ProtectionData,
            max_results_required: false,
            seeded: SEEDED,
        },
        // The three operations whose `MaxResults` the model makes REQUIRED.
        Paginated {
            operation: "ListAppsLists",
            request: json!({ "DefaultLists": false }),
            page: Page::Structs("AppsLists", "ListId"),
            max_results_required: true,
            seeded: SEEDED,
        },
        Paginated {
            operation: "ListProtocolsLists",
            request: json!({ "DefaultLists": false }),
            page: Page::Structs("ProtocolsLists", "ListId"),
            max_results_required: true,
            seeded: SEEDED,
        },
        Paginated {
            operation: "ListThirdPartyFirewallFirewallPolicies",
            request: json!({ "ThirdPartyFirewall": primary_vendor() }),
            page: Page::Structs("ThirdPartyFirewallFirewallPolicies", "FirewallPolicyId"),
            max_results_required: true,
            seeded: SEEDED,
        },
    ];
    (router, rows)
}

/// The attack ids seeded against the Shield Advanced policy, one per seeded
/// attack.
const ATTACK_IDS: [&str; SEEDED] = [
    "attack-one",
    "attack-two",
    "attack-three",
    "attack-four",
    "attack-five",
];

/// Read every page of one row at one page size, following `NextToken`, and return
/// the items in the order they were served.
///
/// Asserts the page invariants on EVERY page: a page never holds more than the
/// effective bound, a page that carries a token is exactly that bound long (so a
/// bound can never hide an item behind a short page), and the last page carries
/// no token.
async fn page_through(
    router: &MultiServiceRouter,
    row: &Paginated,
    max_results: Option<usize>,
) -> Vec<String> {
    let case = format!("{} at MaxResults={max_results:?}", row.operation);
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE);
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // Enough pages for the largest collection at the smallest page size, so a
    // token that never clears fails here instead of looping forever.
    for page in 0..(2 * MAX_PAGE_SIZE) {
        let mut request = row.request.clone();
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, row.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page} must answer 200, got {status} with {body}"
        );
        let items = row.page.items(&case, &body);
        assert!(
            items.len() <= bound,
            "case {case}: page {page} must hold at most {bound} items, got {}",
            items.len()
        );
        served.extend(items.iter().cloned());
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return served;
        }
        assert_eq!(
            items.len(),
            bound,
            "case {case}: page {page} carried a NextToken, so it must be a full page of {bound}, \
             got {} items",
            items.len()
        );
    }
    panic!(
        "case {case}: NextToken never cleared, having served {} items",
        served.len()
    );
}

/// Every paginated operation bounds its page by `MaxResults`, bounds it by the
/// modelled maximum with no `MaxResults` at all, and serves the whole collection
/// once through its `NextToken` whatever the page size.
#[tokio::test]
async fn every_paginated_operation_bounds_its_page_by_the_same_rule() {
    let (router, rows) = seeded().await;

    for row in &rows {
        // The whole collection in ONE page, which the modelled maximum allows
        // because every collection seeded above is smaller than it. It is the
        // expectation every smaller page size is held to.
        let whole = page_through(&router, row, Some(MAX_PAGE_SIZE)).await;
        assert_eq!(
            whole.len(),
            row.seeded,
            "case {}: expected the seeded {} items in one page of {MAX_PAGE_SIZE}, got {whole:?}",
            row.operation,
            row.seeded
        );

        // Page sizes spanning the modelled range: its published minimum, sizes
        // that divide the collection evenly and unevenly, one larger than the
        // collection, its published maximum — and, where the model allows it, no
        // bound at all.
        let mut sizes: Vec<Option<usize>> = vec![
            Some(MIN_PAGE_SIZE),
            Some(2),
            Some(3),
            Some(row.seeded),
            Some(row.seeded + 1),
            Some(MAX_PAGE_SIZE),
        ];
        if !row.max_results_required {
            sizes.push(None);
        }
        for size in sizes {
            let served = page_through(&router, row, size).await;
            assert_eq!(
                served, whole,
                "case {} at MaxResults={size:?}: every page size must serve the same items once, \
                 in the same order",
                row.operation
            );
        }
    }
}

/// The resolution itself, at the edges the wire cannot reach: a `MaxResults`
/// outside the modelled 1..100 range is refused before a handler sees it, so the
/// clamp that keeps a page from ever being empty-with-a-token is asserted here
/// directly.
///
/// The rows are the published bounds of
/// `com.amazonaws.fms#PaginationMaxResults`, one value past each of them, the
/// extremes of the member's `integer` type, and an interior value no issue
/// cited.
#[test]
fn the_page_size_rule_clamps_every_max_results_into_the_modelled_range() {
    let cases: &[(Option<i32>, usize)] = &[
        // No bound at all: the largest page the model allows.
        (None, MAX_PAGE_SIZE),
        // The published minimum, and one below it.
        (Some(1), MIN_PAGE_SIZE),
        (Some(0), MIN_PAGE_SIZE),
        (Some(-1), MIN_PAGE_SIZE),
        (Some(i32::MIN), MIN_PAGE_SIZE),
        // An interior value.
        (Some(37), 37),
        // The published maximum, and one above it.
        (Some(100), MAX_PAGE_SIZE),
        (Some(101), MAX_PAGE_SIZE),
        (Some(i32::MAX), MAX_PAGE_SIZE),
    ];
    for (requested, expected) in cases {
        let resolved = state::page_size(*requested);
        assert_eq!(
            resolved, *expected,
            "case MaxResults={requested:?}: expected a page size of {expected}, got {resolved}"
        );
    }
}
