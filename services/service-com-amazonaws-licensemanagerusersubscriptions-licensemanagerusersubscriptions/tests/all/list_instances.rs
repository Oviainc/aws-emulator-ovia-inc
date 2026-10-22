// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInstances`, driven over the real wire path: every call goes
//! through the router's `dispatch`, so the request is deserialized, constraint-checked,
//! handled and serialized exactly as a client's would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `ListInstances` REPORTS the instances held in this service's store — every member of
//!   every `InstanceSummary` is what state holds, an absent one stays absent, and the
//!   order is the order the instances entered the store;
//! * `Filters` select on the attributes the operation's model documents (`Status`,
//!   `InstanceId`) and refuse any other attribute, or a comparison this service does not
//!   implement, whatever the store holds;
//! * `MaxResults`/`NextToken` page the list exactly once over every element, and a token
//!   that is not a position in THIS list is refused;
//! * the call is reporting only: two identical calls answer identically and the store is
//!   unchanged by a read.
//!
//! An instance is not created by an API of this service (it enters the store through the
//! user-association plane, whose `AssociateUser` handler is still a scaffold stub — see
//! `issues/.../license-manager-user-subscriptions-7-operations-answer-http.md`, cause 1),
//! so these tests seed `state::instances` through the shared engine the handlers read,
//! exactly as that writer will, and then exercise `ListInstances` over the wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    self, ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState,
    InstanceRecord,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ops::types::{
    ActiveDirectoryIdentityProvider, IdentityProvider,
};

// ONE wire harness per service: these come from the sibling suite rather than a second
// copy of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    as_str, call, harness, list_providers, provider_union, register, Harness, Reply, ACCOUNT,
    PRODUCTS,
};

async fn list_instances(h: &Harness, body: &Value) -> Reply {
    // `ListInstances` is bound to POST /instance/ListInstances (smithy_query: operation
    // ListInstances, http.uri).
    call(h, "/instance/ListInstances", body).await
}

/// One instance to seed. The rows differ along what the rules generalise over: the
/// instance-id form, the status value, how many products the instance provides, whether
/// a status message, a status-check date or an identity provider is known.
#[derive(Debug, Clone, Copy)]
struct Seeded {
    case: &'static str,
    /// An EC2 instance id: `i-` plus 8 or 17 hex digits; both published forms appear below.
    instance_id: &'static str,
    /// `InstanceSummary.Status` is a free-form `String` in the model (no enum), so the
    /// status is opaque data here: the rules generalise over its value, not over a
    /// vocabulary.
    status: &'static str,
    products: &'static [&'static str],
    status_message: Option<&'static str>,
    directory_id: Option<&'static str>,
    /// Seconds since the epoch of the last status check, when one has happened.
    last_status_check_epoch: Option<u64>,
}

const SEEDED: &[Seeded] = &[
    Seeded {
        case: "17-hex id, one product, aws-managed directory, status checked",
        instance_id: "i-0123456789abcdef0",
        status: "ACTIVE",
        products: &[PRODUCTS[0]],
        status_message: None,
        directory_id: Some("d-0123456789"),
        last_status_check_epoch: Some(1_700_000_000),
    },
    Seeded {
        case: "8-hex id, two products, status message, shared directory",
        instance_id: "i-abcdef01",
        status: "ACTIVE",
        products: &[PRODUCTS[0], PRODUCTS[1]],
        status_message: Some("user-based subscriptions available"),
        directory_id: Some("sd-abcdef0123"),
        last_status_check_epoch: None,
    },
    Seeded {
        case: "three products, no identity provider known",
        instance_id: "i-00000000000000001",
        status: "REGISTERED",
        products: &[PRODUCTS[2], PRODUCTS[3], PRODUCTS[4]],
        status_message: None,
        directory_id: None,
        last_status_check_epoch: Some(1_700_000_500),
    },
    Seeded {
        case: "8-hex id whose id contains a searchable substring",
        instance_id: "i-0000beef",
        status: "TERMINATED",
        products: &[PRODUCTS[4]],
        status_message: Some("instance terminated"),
        directory_id: Some("d-00000000ff"),
        last_status_check_epoch: Some(1_650_000_000),
    },
    Seeded {
        case: "a status shared with another row, so a status filter selects several",
        instance_id: "i-0f0f0f0f0f0f0f0f0",
        status: "ACTIVE",
        products: &[PRODUCTS[3]],
        status_message: None,
        directory_id: None,
        last_status_check_epoch: None,
    },
    Seeded {
        case: "id outside the i-0 prefix, so a prefix search excludes it",
        instance_id: "i-feedface",
        status: "DEREGISTERED",
        products: &[PRODUCTS[1], PRODUCTS[2]],
        status_message: None,
        directory_id: Some("d-123456789a"),
        last_status_check_epoch: Some(1_600_000_000),
    },
];

impl Seeded {
    fn record(&self) -> InstanceRecord {
        InstanceRecord {
            instance_id: self.instance_id.to_string(),
            status: self.status.to_string(),
            products: self.products.iter().map(|p| (*p).to_string()).collect(),
            status_message: self.status_message.map(str::to_string),
            owner_account_id: ACCOUNT.to_string(),
            identity_provider: self.directory_id.map(|id| {
                IdentityProvider::ActiveDirectoryIdentityProvider(ActiveDirectoryIdentityProvider {
                    directory_id: Some(id.to_string()),
                    ..Default::default()
                })
            }),
            last_status_check_at: self
                .last_status_check_epoch
                .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }
}

/// Seed the store the handlers read, through the same shared engine the service was built
/// from — the writer `AssociateUser` will use is `upsert_instance`, so this is the state a
/// user association leaves behind.
fn seed(h: &Harness, rows: &[Seeded]) {
    let store = h
        .engine
        .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let mut guard = store.lock();
    for row in rows {
        guard.upsert_instance(row.record());
    }
}

fn summaries<'a>(reply: &'a Reply, case: &str) -> &'a Vec<Value> {
    reply.ok(case)["InstanceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries array in {:?}", reply.body))
}

fn listed_ids(reply: &Reply, case: &str) -> Vec<String> {
    summaries(reply, case)
        .iter()
        .map(|summary| as_str(summary, "InstanceId", case).to_string())
        .collect()
}

/// One filter, in the wire form `Filter` models: `Attribute`, an optional `Operation` and
/// `Value`.
fn filter(attribute: &str, operation: Option<&str>, value: &str) -> Value {
    let mut filter = json!({ "Attribute": attribute, "Value": value });
    if let Some(operation) = operation {
        filter["Operation"] = json!(operation);
    }
    filter
}

/// RULE: a reporting list answers from state — every member of every summary is the
/// stored value, a member state does not hold is absent, and the order is the order the
/// resources entered the store.
#[tokio::test]
async fn every_instance_in_state_is_reported_exactly_as_stored() {
    let h = harness();

    // An account with no instance providing user-based subscriptions is an empty list,
    // not a refusal and not a missing member.
    let empty = list_instances(&h, &json!({})).await;
    assert!(
        summaries(&empty, "empty store").is_empty(),
        "an empty store must answer an empty InstanceSummaries, got {:?}",
        empty.body
    );
    assert!(
        empty.ok("empty store").get("NextToken").is_none(),
        "a complete page must carry no NextToken, got {:?}",
        empty.body
    );

    seed(&h, SEEDED);
    let reply = list_instances(&h, &json!({})).await;
    let listed = summaries(&reply, "every seeded instance");
    assert_eq!(
        listed.len(),
        SEEDED.len(),
        "expected every seeded instance, got {listed:?}"
    );

    for (row, summary) in SEEDED.iter().zip(listed.iter()) {
        assert_eq!(
            as_str(summary, "InstanceId", row.case),
            row.instance_id,
            "case {:?}: the list must be in store order, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(summary, "Status", row.case),
            row.status,
            "case {:?}: expected the stored status, got {summary:?}",
            row.case
        );
        let products: Vec<&str> = summary["Products"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: Products is required, got {summary:?}", row.case))
            .iter()
            .map(|product| product.as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            products, row.products,
            "case {:?}: expected the stored products, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(summary, "OwnerAccountId", row.case),
            ACCOUNT,
            "case {:?}: expected the stored owner account, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("StatusMessage").and_then(Value::as_str),
            row.status_message,
            "case {:?}: StatusMessage must be the stored one, or absent, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("LastStatusCheckDate").and_then(Value::as_str),
            row.last_status_check_epoch
                .map(|secs| state::iso8601(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)))
                .as_deref(),
            "case {:?}: LastStatusCheckDate must be the stored instant, or absent, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("IdentityProvider").cloned(),
            row.directory_id.map(provider_union),
            "case {:?}: IdentityProvider must be the stored union, or absent, got {summary:?}",
            row.case
        );
    }
}

/// RULE: `Filters` select the records whose documented filterable attribute satisfies
/// every filter in the set; a set is a conjunction, and an absent `Operation` is equality.
#[tokio::test]
async fn filters_select_on_the_attributes_the_model_documents() {
    let h = harness();
    seed(&h, SEEDED);

    // The attributes `ListInstances` documents as filterable: "You can use the following
    // filters to streamline results: Status InstanceId" (smithy_query doc,
    // ListInstancesRequest$Filters).
    type Predicate = fn(&Seeded) -> bool;
    let cases: &[(&str, Value, Predicate)] = &[
        (
            "Status, operation omitted (equality)",
            json!([filter("Status", None, "ACTIVE")]),
            |row| row.status == "ACTIVE",
        ),
        (
            "Status eq",
            json!([filter("Status", Some("eq"), "ACTIVE")]),
            |row| row.status == "ACTIVE",
        ),
        (
            "Status Equals (PascalCase spelling)",
            json!([filter("Status", Some("Equals"), "REGISTERED")]),
            |row| row.status == "REGISTERED",
        ),
        (
            "Status ne",
            json!([filter("Status", Some("ne"), "ACTIVE")]),
            |row| row.status != "ACTIVE",
        ),
        (
            "InstanceId equality on a 17-hex id",
            json!([filter("InstanceId", Some("eq"), "i-0123456789abcdef0")]),
            |row| row.instance_id == "i-0123456789abcdef0",
        ),
        (
            "InstanceId equality on an 8-hex id",
            json!([filter("InstanceId", None, "i-abcdef01")]),
            |row| row.instance_id == "i-abcdef01",
        ),
        (
            "InstanceId Contains",
            json!([filter("InstanceId", Some("Contains"), "beef")]),
            |row| row.instance_id.contains("beef"),
        ),
        (
            "InstanceId BeginsWith",
            json!([filter("InstanceId", Some("BeginsWith"), "i-0")]),
            |row| row.instance_id.starts_with("i-0"),
        ),
        (
            "two filters are a conjunction",
            json!([
                filter("Status", None, "ACTIVE"),
                filter("InstanceId", Some("BeginsWith"), "i-0")
            ]),
            |row| row.status == "ACTIVE" && row.instance_id.starts_with("i-0"),
        ),
        (
            "a filter nothing satisfies",
            json!([filter("Status", None, "NO_SUCH_STATUS")]),
            |_row| false,
        ),
        (
            "a filter every record satisfies",
            json!([filter("InstanceId", Some("BeginsWith"), "i-")]),
            |_row| true,
        ),
    ];

    for (case, filters, matches) in cases {
        let reply = list_instances(&h, &json!({ "Filters": filters })).await;
        let expected: Vec<String> = SEEDED
            .iter()
            .filter(|row| matches(row))
            .map(|row| row.instance_id.to_string())
            .collect();
        assert_eq!(
            listed_ids(&reply, case),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.body
        );
    }
}

/// RULE: a filter naming an attribute the operation does not expose, or a comparison the
/// service does not implement, is a `ValidationException` (HTTP 400) — and because
/// well-formedness does not depend on the store, it is refused on an empty store too.
#[tokio::test]
async fn a_filter_the_operation_does_not_expose_is_refused() {
    let cases = [
        ("an attribute of another list operation", filter("Product", None, PRODUCTS[0])),
        ("a response member that is not filterable", filter("OwnerAccountId", None, ACCOUNT)),
        ("a plural of a documented attribute", filter("Products", None, PRODUCTS[0])),
        ("the wrong case of a documented attribute", filter("instanceId", None, "i-abcdef01")),
        ("another wrong case of a documented attribute", filter("INSTANCEID", None, "i-abcdef01")),
        ("an empty attribute name", filter("", None, "ACTIVE")),
        ("a comparison this service does not implement", filter("Status", Some("like"), "ACTIVE")),
        ("an empty operation", filter("Status", Some(""), "ACTIVE")),
    ];

    for (case, bogus) in cases {
        // Refused whatever the store holds: once with instances, once with none.
        for (store, rows) in [("a seeded store", SEEDED), ("an empty store", &[][..])] {
            let h = harness();
            seed(&h, rows);
            let reply = list_instances(&h, &json!({ "Filters": [bogus.clone()] })).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?} on {store}: ValidationException is modelled at HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?} on {store}: expected ValidationException, got {reply:?}"
            );
        }
    }
}

/// RULE: `MaxResults` + `NextToken` walk the list exactly once over every element,
/// whatever the page size, and the last page carries no token.
#[tokio::test]
async fn paging_walks_every_instance_exactly_once() {
    let h = harness();
    seed(&h, SEEDED);
    let all: Vec<String> = SEEDED
        .iter()
        .map(|row| row.instance_id.to_string())
        .collect();

    for page_size in 1..=SEEDED.len() + 1 {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = list_instances(&h, &request).await;
            let page = summaries(&reply, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            seen.extend(listed_ids(&reply, &case));
            token = reply.body.get("NextToken").and_then(Value::as_str).map(str::to_string);
            let remaining = all.len() - seen.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {case:?}: a token must be present exactly while elements remain ({remaining} left), got {reply:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging must walk every instance exactly once, in store order"
        );
    }

    // A filtered list pages the same way, through its own cursor.
    let filters = json!([filter("Status", None, "ACTIVE")]);
    let first = list_instances(&h, &json!({ "Filters": filters, "MaxResults": 1 })).await;
    let token = first.ok("filtered first page")["NextToken"]
        .as_str()
        .expect("a truncated filtered page carries a NextToken")
        .to_string();
    let second =
        list_instances(&h, &json!({ "Filters": filters, "MaxResults": 1, "NextToken": token }))
            .await;
    let active: Vec<String> = SEEDED
        .iter()
        .filter(|row| row.status == "ACTIVE")
        .map(|row| row.instance_id.to_string())
        .collect();
    assert_eq!(
        [
            listed_ids(&first, "filtered first page"),
            listed_ids(&second, "filtered second page")
        ]
        .concat(),
        active[..2].to_vec(),
        "a filtered list must page through its own cursor"
    );
}

/// RULE: a page size that cannot make progress, and a token that is not a position in
/// THIS list, are refused with `ValidationException` (HTTP 400) rather than silently
/// restarting the list.
#[tokio::test]
async fn a_page_request_that_names_no_position_in_this_list_is_refused() {
    let h = harness();
    seed(&h, SEEDED);

    // A token minted by a DIFFERENT list: the identity-provider list of the same service.
    register(
        &h,
        &json!({
            "IdentityProvider": provider_union("d-0123456789"),
            "Product": PRODUCTS[0],
            "Settings": { "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" },
        }),
    )
    .await
    .ok("a registration to page over");
    register(
        &h,
        &json!({
            "IdentityProvider": provider_union("d-0123456789"),
            "Product": PRODUCTS[1],
            "Settings": { "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" },
        }),
    )
    .await
    .ok("a second registration to page over");
    let providers = list_providers(&h, &json!({ "MaxResults": 1 })).await;
    let foreign = providers.ok("identity-provider first page")["NextToken"]
        .as_str()
        .expect("a truncated page carries a NextToken")
        .to_string();

    // A token minted by this list under DIFFERENT filters is a position in that list only.
    let filtered = list_instances(
        &h,
        &json!({ "Filters": [filter("Status", None, "ACTIVE")], "MaxResults": 1 }),
    )
    .await;
    let other_filters = filtered.ok("filtered first page")["NextToken"]
        .as_str()
        .expect("a truncated page carries a NextToken")
        .to_string();

    let cases = [
        ("a page size of zero", json!({ "MaxResults": 0 })),
        ("a negative page size", json!({ "MaxResults": -1 })),
        ("a token that is not hex", json!({ "NextToken": "not-a-token" })),
        ("a hex token from no list of ours", json!({ "NextToken": "6f746865723a30" })),
        ("a token minted by another list", json!({ "NextToken": foreign })),
        ("a token minted under other filters", json!({ "NextToken": other_filters })),
    ];
    for (case, request) in cases {
        let reply = list_instances(&h, &request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }
}

/// RULE: a reporting operation changes nothing a later caller sees — repeated reads, with
/// and without filters or paging, answer identically and leave the store as it was.
#[tokio::test]
async fn listing_instances_changes_nothing_a_later_caller_sees() {
    let h = harness();
    seed(&h, SEEDED);
    let requests = [
        ("unfiltered", json!({})),
        (
            "filtered",
            json!({ "Filters": [filter("Status", None, "ACTIVE")] }),
        ),
        ("first page", json!({ "MaxResults": 2 })),
    ];

    for (case, request) in requests {
        let first = list_instances(&h, &request).await;
        let second = list_instances(&h, &request).await;
        assert_eq!(
            first.ok(case),
            second.ok(case),
            "case {case:?}: a read must answer the same twice, got {:?} then {:?}",
            first.body,
            second.body
        );
    }

    let store = h
        .engine
        .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let guard = store.lock();
    assert_eq!(
        guard.instance_order.len(),
        SEEDED.len(),
        "a read must not add or drop an instance: {:?}",
        guard.instance_order
    );
    for row in SEEDED {
        let record = guard
            .instances
            .get(row.instance_id)
            .unwrap_or_else(|| panic!("case {:?}: the seeded instance is gone", row.case));
        assert_eq!(
            record.status, row.status,
            "case {:?}: a read must not move a status, got {record:?}",
            row.case
        );
        assert_eq!(
            record.last_status_check_at,
            row.last_status_check_epoch
                .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            "case {:?}: a read must not invent a status check, got {record:?}",
            row.case
        );
    }
}
