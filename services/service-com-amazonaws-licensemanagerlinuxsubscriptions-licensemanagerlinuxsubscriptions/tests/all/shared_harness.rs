// SPDX-License-Identifier: CC0-1.0

//! The ONE wire harness of this crate, driven through an operation of each suite that used
//! to state its own copy of it: the tagging slice (`TagResource` `PUT /tags/{resourceArn}`
//! and `ListTagsForResource` `GET /tags/{resourceArn}`, owned by `resource_tags.rs`) and
//! the two discovery lists (`POST /subscription/ListLinuxSubscriptions` and
//! `POST /subscription/ListLinuxSubscriptionInstances`, owned by `linux_subscriptions.rs`).
//!
//! The rule: ONE implementation states what an SDK client's request looks like on the wire
//! for every operation of this service — it builds the emulator, percent-encodes a value
//! bound to an `@httpLabel`, dispatches the request through the router, and reads the wire
//! error code out of the reply (the `x-amzn-errortype` header, falling back to the body's
//! `__type`) — so an operation answers the same way whichever suite drives it.
//!
//! The rows differ along what that rule generalises over: the HTTP method and path shape
//! the operation binds (`PUT`, `GET` and `POST`), the characters the labelled value
//! carries, the size of the payload, how many records the store holds, and the two refusal
//! taxonomies (an absent resource on the tag surface, a filter the list cannot apply) — so
//! both a 200 and a non-2xx answer travel the one path.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::harness::{
    call, enable_discovery, harness, list_tags, provider_arn_for, reported_tags,
    seed_discovered_instance, seed_provider, tag, Harness, Reply,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::DiscoveredInstanceRecord;

/// A Secrets Manager ARN satisfying the modelled `SecretArn` pattern.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:rhel-token";

/// `POST /subscription/ListLinuxSubscriptions` (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
/// `POST /subscription/ListLinuxSubscriptionInstances` (`OPERATIONS.md`).
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// Two of the three ways a subscription can be licensed, from the model's documentation
/// for `Subscription$Type`: "subscription-included with Amazon EC2, Bring Your Own
/// Subscription model (BYOS), or from the Amazon Web Services Marketplace".
const TYPE_INCLUDED: &str = "Subscription-included (Amazon EC2)";
const TYPE_BYOS: &str = "Bring Your Own Subscription model (BYOS)";

/// The refusal the tag surface states for a `resourceArn` addressing no resource of this
/// account.
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");
/// The refusal either list states for a `Filter` it cannot apply.
const INVALID_FILTER: (u16, &str) = (400, "ValidationException");

/// One row of the tag round trip: the resource id the labelled ARN addresses, and the
/// pairs the write names.
struct TagCase {
    label: &'static str,
    id: &'static str,
    pairs: &'static [(&'static str, &'static str)],
}

const TAG_CASES: &[TagCase] = &[
    TagCase {
        label: "one pair",
        id: "00000000-0000-4000-8000-000000000002",
        pairs: &[("env", "prod")],
    },
    TagCase {
        label: "three pairs",
        id: "00000000-0000-4000-8000-000000000003",
        pairs: &[("env", "prod"), ("owner", "platform"), ("cost-centre", "42")],
    },
    TagCase {
        label: "URI-significant values",
        id: "00000000-0000-4000-8000-000000000004",
        pairs: &[
            ("path", "a/b:c?d=e&f"),
            ("arn", "arn:aws:iam::000000000000:role/x"),
        ],
    },
    TagCase {
        label: "non-ASCII keys and values",
        id: "00000000-0000-4000-8000-000000000005",
        pairs: &[("propriétaire", "équipe-plate-forme"), ("团队", "平台")],
    },
    TagCase {
        label: "a one-character resource id",
        id: "a",
        pairs: &[("env", "dev")],
    },
];

/// One discovered instance for the fixture store: instance id, status, subscription name,
/// subscription type.
type InstanceRow = (&'static str, &'static str, &'static str, &'static str);

/// What the store holds for the two lists: one subscription used by a running and a
/// stopped instance, and one used by a single running instance, so the counts are not the
/// record count.
const DISCOVERED: &[InstanceRow] = &[
    ("i-shared-01", "running", "RHEL", TYPE_INCLUDED),
    ("i-shared-02", "stopped", "RHEL", TYPE_INCLUDED),
    (
        "i-shared-03",
        "running",
        "SUSE Linux Enterprise Server",
        TYPE_BYOS,
    ),
];

/// A JSON tag map from `(key, value)` pairs.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect::<Map<String, Value>>(),
    )
}

/// The pairs a row names, as a sorted map, so two sets compare independently of order.
fn expected_tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The Region `DISCOVERED` was found in, which is therefore the one the account's settings
/// discover.
const DISCOVERED_REGION: &str = "us-west-2";

/// Put `DISCOVERED` in the store through the shared seeding seam, under the settings that
/// make those records discovered data: discovery enabled for the Region they were found in,
/// because what the lists report is the inventory of the Regions the settings discover now.
async fn discover(h: &Harness) {
    enable_discovery(h, &[DISCOVERED_REGION]).await;
    for (instance_id, status, subscription_name, subscription_type) in DISCOVERED {
        seed_discovered_instance(
            h,
            DiscoveredInstanceRecord {
                instance_id: (*instance_id).to_string(),
                account_id: "000000000000".to_string(),
                ami_id: "ami-0001".to_string(),
                instance_type: "t3.micro".to_string(),
                status: (*status).to_string(),
                region: DISCOVERED_REGION.to_string(),
                usage_operation: "RunInstances:0010".to_string(),
                subscription_name: (*subscription_name).to_string(),
                subscription_type: (*subscription_type).to_string(),
                ..DiscoveredInstanceRecord::default()
            },
        );
    }
}

/// A string member of a list entry.
fn member(entry: &Value, name: &str, case: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
        .to_string()
}

/// The entries a list body reports under `key`.
fn entries<'a>(reply: &'a Reply, key: &str, case: &str) -> &'a Vec<Value> {
    reply
        .ok(case)
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} in {:?}", reply.body))
}

/// The one harness carries the tagging slice: a write travels as a `PUT` whose
/// `resourceArn` is one percent-encoded path segment, and the `GET` that reads it back
/// reports exactly what the write merged in — for every class of labelled id and payload.
#[tokio::test]
async fn the_shared_harness_round_trips_the_tag_operations() {
    let h = harness();

    let mut addressed = Vec::new();
    for row in TAG_CASES {
        let case = format!("TagResource with {}", row.label);
        let arn = seed_provider(&h, row.id, SECRET_ARN, &[]);
        assert_eq!(
            arn,
            provider_arn_for(row.id),
            "case {case:?}: the seeded resource must be addressed by its own ARN, got {arn:?}"
        );

        tag(&h, &arn, &tag_map(row.pairs)).await.ok(&case);

        let case = format!("ListTagsForResource with {}", row.label);
        let got = reported_tags(&list_tags(&h, &arn).await, &case);
        let want = expected_tags(row.pairs);
        assert_eq!(
            got, want,
            "case {case:?}: expected the written set {want:?}, got {got:?}"
        );
        addressed.push((case, arn, want));
    }

    // Every resource still reports its OWN set: the one path addressed the resource the
    // label named, row after row.
    for (case, arn, want) in &addressed {
        let got = reported_tags(&list_tags(&h, arn).await, case);
        assert_eq!(
            got, *want,
            "case {case:?}: after the other rows' writes, {arn} reports {got:?}"
        );
    }
}

/// The same harness carries the two discovery lists: a `POST` body travels through the one
/// request builder, and both lists project the records the one seeding seam placed.
#[tokio::test]
async fn the_shared_harness_drives_both_discovery_lists() {
    let h = harness();
    discover(&h).await;

    let case = "ListLinuxSubscriptions over the seeded store";
    let reply = call(&h, SUBSCRIPTIONS, &json!({})).await;
    let got: Vec<(String, String, i64)> = entries(&reply, "Subscriptions", case)
        .iter()
        .map(|entry| {
            (
                member(entry, "Name", case),
                member(entry, "Type", case),
                entry
                    .get("InstanceCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("case {case:?}: no InstanceCount in {entry:?}")),
            )
        })
        .collect();
    // One subscription per distinct (name, type) pair, counting the RUNNING instances.
    let want: Vec<(String, String, i64)> = vec![
        ("RHEL".to_string(), TYPE_INCLUDED.to_string(), 1),
        (
            "SUSE Linux Enterprise Server".to_string(),
            TYPE_BYOS.to_string(),
            1,
        ),
    ];
    assert_eq!(
        got, want,
        "case {case:?}: expected {want:?}, got {got:?}"
    );

    let case = "ListLinuxSubscriptionInstances over the seeded store";
    let reply = call(&h, INSTANCES, &json!({})).await;
    let got: Vec<String> = entries(&reply, "Instances", case)
        .iter()
        .map(|entry| member(entry, "InstanceID", case))
        .collect();
    let want: Vec<String> = DISCOVERED
        .iter()
        .map(|(instance_id, _, _, _)| (*instance_id).to_string())
        .collect();
    assert_eq!(
        got, want,
        "case {case:?}: expected {want:?}, got {got:?}"
    );
}

/// The one reply vocabulary reads the wire error code for BOTH refusal taxonomies the two
/// suites assert, so neither suite needs its own copy of the extraction.
#[tokio::test]
async fn the_shared_harness_reads_the_wire_error_code_of_every_refusal() {
    let h = harness();
    discover(&h).await;
    let live = seed_provider(&h, "00000000-0000-4000-8000-0000000000ff", SECRET_ARN, &[]);
    let absent = provider_arn_for("55555555-5555-4555-8555-555555555555");

    // An ARN addressing no resource, on the write and on the read of the tag surface.
    let (status, code) = NOT_FOUND;
    let case = "TagResource on an ARN addressing no resource";
    tag(&h, &absent, &tag_map(&[("env", "prod")]))
        .await
        .error(case, status, code);
    let case = "ListTagsForResource on an ARN addressing no resource";
    list_tags(&h, &absent).await.error(case, status, code);

    // A filter neither list documents, on each list.
    let (status, code) = INVALID_FILTER;
    for (path, filter, why) in [
        (
            SUBSCRIPTIONS,
            json!({ "Name": "Region", "Values": ["us-west-2"] }),
            "Region is documented for the instance list, not for this one",
        ),
        (
            INSTANCES,
            json!({ "Name": "Subscription", "Values": ["RHEL"] }),
            "Subscription is documented for the subscription list, not for this one",
        ),
    ] {
        let case = format!("{path} {filter}: {why}");
        call(&h, path, &json!({ "Filters": [filter] }))
            .await
            .error(&case, status, code);
    }

    // And the refusals changed nothing the next caller sees, over both surfaces.
    let case = "the live resource after the refusals";
    let got = reported_tags(&list_tags(&h, &live).await, case);
    assert!(
        got.is_empty(),
        "case {case:?}: expected the untouched empty set, got {got:?}"
    );
    let case = "the discovered instances after the refusals";
    let reply = call(&h, INSTANCES, &json!({})).await;
    assert_eq!(
        entries(&reply, "Instances", case).len(),
        DISCOVERED.len(),
        "case {case:?}: expected the store untouched, got {:?}",
        reply.body
    );
}
