// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` over the kinds of Lightsail resource that carry tags and are
//! addressed by their own create and Get — the instance, the block storage disk,
//! the managed database, the bucket, the load balancer and the contact method —
//! driven over the wire. The remaining tag-carrying kinds are walked by
//! `tagged_resource_coverage.rs`, which reuses this suite's helpers rather than
//! asserting the rule with a second harness.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `UntagResource` removes from the ONE resource
//! `resourceName` names exactly those of its tags whose KEY the request lists,
//! leaves every other tag of that resource — and every other resource's tags —
//! as they were, and so depends only on the SET of keys the request named: a key
//! the resource does not carry removes nothing, a key named twice removes the
//! same one tag, and the removal is observable only by reading the resource back
//! through its own Get. The resource is resolved by name whichever kind of
//! resource it is, in the Region the request is made in for the kinds whose
//! identity includes a Region; a name this account holds no resource of is
//! refused with this service's `DoesNotExist` envelope, and a `resourceArn` that
//! is not that resource's own is refused `InvalidInputException` (HTTP 400) and
//! removes nothing.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// One tag of a request, as `(key, value)` — a value of `None` is a key-only
/// tag, which the modelled `Tag$value` admits.
pub(crate) type TagRow = (&'static str, Option<&'static str>);

/// The tag set every resource in this suite is created with: two keys with
/// values, one key-only tag, and a key carrying a hyphen and a digits-only
/// value.
const TAGS: &[TagRow] = &[
    ("env", Some("test")),
    ("owner", Some("platform")),
    ("retain", None),
    ("cost-centre", Some("4711")),
];

/// The Region an unsigned request is served in
/// (`crate::state::DEFAULT_REGION`), which is the Region every resource this
/// suite creates without naming one lands in.
pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// The most tags one Lightsail resource can hold, from the tagging documentation
/// the modelled `tags` member links to ("You can add up to 50 tags to each
/// Lightsail resource"), which is also the most keys one removal can name.
const MAX_TAGS: usize = 50;

/// The kinds of Lightsail resource this suite tags, one group per shape of store
/// key: a resource addressed by its name alone (an instance, a block storage
/// disk, a managed relational database) and one whose identity includes the
/// Region it was created in (a bucket, a load balancer).
#[derive(Clone, Copy, Debug)]
enum Kind {
    Instance,
    Disk,
    Database,
    Bucket,
    LoadBalancer,
}

/// Every kind the table walks.
const KINDS: &[Kind] = &[
    Kind::Instance,
    Kind::Disk,
    Kind::Database,
    Kind::Bucket,
    Kind::LoadBalancer,
];

impl Kind {
    /// The `ResourceType` this kind reports, from the modelled `ResourceType`
    /// enum.
    fn resource_type(self) -> &'static str {
        match self {
            Kind::Instance => "Instance",
            Kind::Disk => "Disk",
            Kind::Database => "RelationalDatabase",
            Kind::Bucket => "Bucket",
            Kind::LoadBalancer => "LoadBalancer",
        }
    }

    /// The name this suite creates this kind's resource under. Each satisfies
    /// both that resource's own modelled name pattern and
    /// `UntagResource$resourceName`'s (`^\w[\w\-]*\w$`).
    fn name(self) -> &'static str {
        match self {
            Kind::Instance => "tagged-instance",
            Kind::Disk => "tagged-disk",
            Kind::Database => "tagged-database",
            Kind::Bucket => "tagged-bucket",
            Kind::LoadBalancer => "tagged-balancer",
        }
    }

    /// Create this kind's resource carrying `tags`, in `region`.
    async fn create(self, emulator: &Emulator, region: Option<&str>, tags: &[TagRow]) {
        let name = self.name();
        let (operation, mut body) = match self {
            Kind::Instance => (
                "CreateInstances",
                json!({
                    "instanceNames": [name],
                    "availabilityZone": "us-east-1a",
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": "micro_3_0",
                }),
            ),
            Kind::Disk => (
                "CreateDisk",
                json!({"diskName": name, "availabilityZone": "us-east-1a", "sizeInGb": 8}),
            ),
            Kind::Database => (
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseBlueprintId": "mysql_8_4",
                    "relationalDatabaseBundleId": "micro_1_0",
                    "masterDatabaseName": "appdb",
                    "masterUsername": "dbadmin",
                }),
            ),
            Kind::Bucket => (
                "CreateBucket",
                json!({"bucketName": name, "bundleId": "small_1_0"}),
            ),
            Kind::LoadBalancer => (
                "CreateLoadBalancer",
                json!({"loadBalancerName": name, "instancePort": 80}),
            ),
        };
        if !tags.is_empty() {
            body["tags"] = tag_list(tags);
        }
        emulator.ok_at(None, region, operation, body).await;
    }

    /// This kind's resource as its own Get reports it, read in `region`.
    async fn read(self, emulator: &Emulator, region: Option<&str>) -> Value {
        let name = self.name();
        match self {
            Kind::Instance => emulator
                .ok_at(None, region, "GetInstance", json!({"instanceName": name}))
                .await["instance"]
                .clone(),
            Kind::Disk => emulator
                .ok_at(None, region, "GetDisk", json!({"diskName": name}))
                .await["disk"]
                .clone(),
            Kind::Database => emulator
                .ok_at(
                    None,
                    region,
                    "GetRelationalDatabase",
                    json!({"relationalDatabaseName": name}),
                )
                .await["relationalDatabase"]
                .clone(),
            Kind::Bucket => {
                let body = emulator
                    .ok_at(None, region, "GetBuckets", json!({"bucketName": name}))
                    .await;
                let buckets = body["buckets"].as_array().unwrap_or_else(|| {
                    panic!("case reading {name}: expected a buckets array, got {body:?}")
                });
                assert_eq!(
                    buckets.len(),
                    1,
                    "case reading {name}: expected exactly the one named bucket, got {buckets:?}"
                );
                buckets[0].clone()
            }
            Kind::LoadBalancer => emulator
                .ok_at(
                    None,
                    region,
                    "GetLoadBalancer",
                    json!({"loadBalancerName": name}),
                )
                .await["loadBalancer"]
                .clone(),
        }
    }

    /// The tags this kind's resource reports now, read back in `region`.
    async fn tags(self, emulator: &Emulator, region: Option<&str>) -> Vec<(String, Option<String>)> {
        reported_tags(&self.read(emulator, region).await)
    }
}

/// `tags` as the modelled `TagList` a create request carries.
pub(crate) fn tag_list(tags: &[TagRow]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect(),
    )
}

/// The tags `resource` reports, as `(key, value)` pairs in report order.
pub(crate) fn reported_tags(resource: &Value) -> Vec<(String, Option<String>)> {
    resource["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a tagged resource always reports a tag list, got {resource:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// `tags` as the pairs a report must carry.
pub(crate) fn want_tags(tags: &[TagRow]) -> Vec<(String, Option<String>)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), value.map(str::to_string)))
        .collect()
}

/// Call `UntagResource` for `name` with `keys` in `region`, requiring success,
/// and answer the one operation-log entry it reports.
pub(crate) async fn untag(
    emulator: &Emulator,
    region: Option<&str>,
    name: &str,
    keys: &[&str],
) -> Value {
    let answered = emulator
        .ok_at(
            None,
            region,
            "UntagResource",
            json!({"resourceName": name, "tagKeys": keys}),
        )
        .await;
    let operations = answered["operations"].as_array().unwrap_or_else(|| {
        panic!("case untagging {name}: expected an operations array, got {answered:?}")
    });
    assert_eq!(
        operations.len(),
        1,
        "case untagging {name}: expected one entry for the one resource named, got {operations:?}"
    );
    operations[0].clone()
}

/// Assert the operation-log entry `UntagResource` answered is the terminal entry
/// for `name` of `resource_type` in `region`.
///
/// The entry reports NO `operationType`: the modelled `OperationType` enum
/// declares a member for each call that writes this log and none for a change to
/// a resource's tags, so there is no modelled value to report, and
/// `Operation$operationType` is an optional member.
pub(crate) fn assert_entry(
    case: &str,
    entry: &Value,
    name: &str,
    resource_type: &str,
    region: &str,
) {
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the entry to name the resource, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some(resource_type),
        "case {case}: expected resourceType {resource_type}, got {entry:?}"
    );
    assert_eq!(
        entry["status"].as_str(),
        Some("Succeeded"),
        "case {case}: expected a succeeded entry, got {entry:?}"
    );
    assert_eq!(
        entry["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal entry, got {entry:?}"
    );
    assert_eq!(
        entry["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: expected the entry in region {region}, got {entry:?}"
    );
    assert!(
        entry["operationType"].is_null(),
        "case {case}: expected no operationType, got {entry:?}"
    );
    let id = entry["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
}

// ---------------------------------------------------------------------------
// The kind table
// ---------------------------------------------------------------------------

/// RULE: whichever kind of resource the name addresses, `UntagResource` removes
/// the listed keys from it and leaves its other tags as they were.
#[tokio::test]
async fn every_kind_of_resource_loses_exactly_the_listed_keys() {
    for kind in KINDS {
        let case = format!("{kind:?}");
        let emulator = Emulator::new();
        kind.create(&emulator, None, TAGS).await;
        assert_eq!(
            kind.tags(&emulator, None).await,
            want_tags(TAGS),
            "case {case}: expected the created resource to carry the request's tags"
        );

        let entry = untag(&emulator, None, kind.name(), &["owner", "cost-centre"]).await;
        assert_entry(
            &case,
            &entry,
            kind.name(),
            kind.resource_type(),
            DEFAULT_REGION,
        );

        let left = kind.tags(&emulator, None).await;
        assert_eq!(
            left,
            want_tags(&[("env", Some("test")), ("retain", None)]),
            "case {case}: expected only the unlisted tags to survive, got {left:?}"
        );
    }
}

/// The contact method of `protocol`, as `GetContactMethods` reports it.
async fn reported_contact_method(emulator: &Emulator, protocol: &str) -> Value {
    let body = emulator
        .ok("GetContactMethods", json!({"protocols": [protocol]}))
        .await;
    let methods = body["contactMethods"]
        .as_array()
        .unwrap_or_else(|| panic!("case {protocol}: expected a contactMethods array, got {body:?}"));
    assert_eq!(
        methods.len(),
        1,
        "case {protocol}: expected the one contact method of that protocol, got {methods:?}"
    );
    methods[0].clone()
}

/// RULE: the resource is resolved by the NAME it reports, even where the store
/// holds it under something else — a contact method's record is keyed by its
/// Region and its protocol, and the name a tag change names it by is the one the
/// service minted and `GetContactMethods` publishes.
#[tokio::test]
async fn a_contact_method_is_untagged_by_the_name_it_reports() {
    let emulator = Emulator::new();
    // One contact method per protocol per Region, so the two protocols are two
    // resources — and only the one named loses a tag.
    for (protocol, endpoint) in [("Email", "ops@example.com"), ("SMS", "+12065550100")] {
        let mut body = json!({"protocol": protocol, "contactEndpoint": endpoint});
        body["tags"] = tag_list(TAGS);
        emulator.ok("CreateContactMethod", body).await;
    }

    let email = reported_contact_method(&emulator, "Email").await;
    let name = email["name"]
        .as_str()
        .expect("a contact method reports the name the service minted it under")
        .to_string();
    assert_eq!(
        reported_tags(&email),
        want_tags(TAGS),
        "case Email: expected the created contact method to carry the request's tags"
    );

    let entry = untag(&emulator, None, &name, &["owner", "cost-centre"]).await;
    assert_entry("Email", &entry, &name, "ContactMethod", DEFAULT_REGION);

    let left = reported_tags(&reported_contact_method(&emulator, "Email").await);
    assert_eq!(
        left,
        want_tags(&[("env", Some("test")), ("retain", None)]),
        "case Email: expected only the unlisted tags to survive, got {left:?}"
    );
    let untouched = reported_tags(&reported_contact_method(&emulator, "SMS").await);
    assert_eq!(
        untouched,
        want_tags(TAGS),
        "case SMS: expected the contact method that was not named to keep every tag, got {untouched:?}"
    );
}

// ---------------------------------------------------------------------------
// The key-set table
// ---------------------------------------------------------------------------

/// One removal case: the keys the request lists, and the tags the resource must
/// carry afterwards.
struct KeyRow {
    case: &'static str,
    keys: &'static [&'static str],
    left: &'static [TagRow],
}

/// The table. The rows differ along what the rule generalises over: how many
/// keys the request lists, whether a listed key is one the resource carries,
/// whether it is listed more than once, whether the tag it names has a value,
/// and the spelling of the key.
const KEY_ROWS: &[KeyRow] = &[
    KeyRow {
        case: "no key at all",
        keys: &[],
        left: TAGS,
    },
    KeyRow {
        case: "one key of several",
        keys: &["env"],
        left: &[
            ("owner", Some("platform")),
            ("retain", None),
            ("cost-centre", Some("4711")),
        ],
    },
    KeyRow {
        case: "a key the resource does not carry",
        keys: &["absent"],
        left: TAGS,
    },
    KeyRow {
        case: "the same key listed twice",
        keys: &["env", "env"],
        left: &[
            ("owner", Some("platform")),
            ("retain", None),
            ("cost-centre", Some("4711")),
        ],
    },
    KeyRow {
        case: "a key-only tag, removed by key",
        keys: &["retain"],
        left: &[
            ("env", Some("test")),
            ("owner", Some("platform")),
            ("cost-centre", Some("4711")),
        ],
    },
    KeyRow {
        case: "a carried key beside one the resource does not carry",
        keys: &["cost-centre", "absent"],
        left: &[
            ("env", Some("test")),
            ("owner", Some("platform")),
            ("retain", None),
        ],
    },
    KeyRow {
        // AWS tag keys are case sensitive, so a key that differs only in case
        // names no tag of this resource.
        case: "a key differing from a carried one only in case",
        keys: &["ENV"],
        left: TAGS,
    },
    KeyRow {
        case: "every key the resource carries",
        keys: &["env", "owner", "retain", "cost-centre"],
        left: &[],
    },
];

/// RULE: what a removal leaves behind is decided by the SET of keys the request
/// listed and nothing else.
#[tokio::test]
async fn the_listed_key_set_decides_what_is_left() {
    for row in KEY_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let kind = Kind::Disk;
        kind.create(&emulator, None, TAGS).await;

        let entry = untag(&emulator, None, kind.name(), row.keys).await;
        assert_entry(case, &entry, kind.name(), kind.resource_type(), DEFAULT_REGION);

        let left = kind.tags(&emulator, None).await;
        assert_eq!(
            left,
            want_tags(row.left),
            "case {case}: expected the unlisted tags to survive in order, got {left:?}"
        );
    }
}

/// RULE: removal depends only on the key set, so repeating a removal changes
/// nothing — the second call succeeds and the resource reports the same tags.
#[tokio::test]
async fn repeating_a_removal_changes_nothing() {
    for row in KEY_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let kind = Kind::Instance;
        kind.create(&emulator, None, TAGS).await;

        untag(&emulator, None, kind.name(), row.keys).await;
        let once = kind.tags(&emulator, None).await;
        let entry = untag(&emulator, None, kind.name(), row.keys).await;
        assert_entry(case, &entry, kind.name(), kind.resource_type(), DEFAULT_REGION);
        let twice = kind.tags(&emulator, None).await;

        assert_eq!(
            twice, once,
            "case {case}: expected the repeat removal to leave the same tags, got {twice:?}"
        );
        assert_eq!(
            twice,
            want_tags(row.left),
            "case {case}: expected the unlisted tags to survive the repeat, got {twice:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The size of the key list
// ---------------------------------------------------------------------------

/// RULE: how MANY keys the request lists does not decide anything — a list
/// longer than the tags a resource can hold is served, and removes exactly the
/// keys the resource carries.
#[tokio::test]
async fn the_length_of_the_key_list_decides_nothing() {
    let kind = Kind::Disk;
    // Under the number of tags a resource can hold, exactly that many, and far
    // past it: the published 50-tag limit is on the tags HELD, so none of the
    // three is refused.
    for (case, count) in [
        ("fewer keys than a resource can hold tags", MAX_TAGS - 1),
        ("as many keys as a resource can hold tags", MAX_TAGS),
        ("one key more", MAX_TAGS + 1),
        ("an order of magnitude more keys", 10 * MAX_TAGS),
    ] {
        let emulator = Emulator::new();
        kind.create(&emulator, None, TAGS).await;
        // The carried key comes first, so a served request is one that really
        // did remove a tag rather than one that named nothing; every other key
        // names no tag of this resource.
        let mut keys = vec!["env".to_string()];
        while keys.len() < count {
            keys.push(format!("filler-{}", keys.len()));
        }
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"resourceName": kind.name(), "tagKeys": keys}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 for {count} keys, got {status} {body:?}"
        );
        let left = kind.tags(&emulator, None).await;
        assert_eq!(
            left,
            want_tags(&[
                ("owner", Some("platform")),
                ("retain", None),
                ("cost-centre", Some("4711")),
            ]),
            "case {case}: expected the one carried key gone and nothing else, got {left:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// One resource, and only that one
// ---------------------------------------------------------------------------

/// RULE: the call names ONE resource, so no other resource's tags move — not
/// even one carrying the very same keys.
#[tokio::test]
async fn only_the_named_resource_loses_tags() {
    let emulator = Emulator::new();
    for kind in KINDS {
        kind.create(&emulator, None, TAGS).await;
    }

    for (named, target) in KINDS.iter().enumerate() {
        let case = format!("untagging {target:?}");
        let entry = untag(&emulator, None, target.name(), &["env"]).await;
        assert_entry(
            &case,
            &entry,
            target.name(),
            target.resource_type(),
            DEFAULT_REGION,
        );

        for (index, kind) in KINDS.iter().enumerate() {
            let tags = kind.tags(&emulator, None).await;
            // Every kind named so far has lost `env`; the rest still carry it.
            let want = if index <= named {
                want_tags(&[
                    ("owner", Some("platform")),
                    ("retain", None),
                    ("cost-centre", Some("4711")),
                ])
            } else {
                want_tags(TAGS)
            };
            assert_eq!(
                tags, want,
                "case {case}: expected {kind:?} to report {want:?}, got {tags:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a name this account holds no resource of is refused with Lightsail's
/// `DoesNotExist` envelope — including the name of a resource whose identity
/// includes a Region, asked for in another Region, which leaves that resource's
/// tags untouched.
#[tokio::test]
async fn a_name_this_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let kind = Kind::Bucket;
    kind.create(&emulator, Some(DEFAULT_REGION), TAGS).await;

    for (case, region, name) in [
        ("a name nothing was ever created under", None, "no-such-thing"),
        (
            "a name whose resource lives in another Region",
            Some("eu-west-1"),
            kind.name(),
        ),
        (
            "a deleted-looking name in the resource's own Region",
            Some(DEFAULT_REGION),
            "absent-bucket",
        ),
    ] {
        let (status, body) = emulator
            .call_at(
                None,
                region,
                "UntagResource",
                json!({"resourceName": name, "tagKeys": ["env"]}),
            )
            .await;
        // Lightsail answers a resource it cannot find with HTTP 400 and
        // `DoesNotExist` in the body's `code`, which is what an SDK client
        // branches on, with the shape name in `__type`.
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist code, got {body:?}"
        );
    }

    let tags = kind.tags(&emulator, Some(DEFAULT_REGION)).await;
    assert_eq!(
        tags,
        want_tags(TAGS),
        "case refused removals: expected the bucket's tags untouched, got {tags:?}"
    );

    // The same keys, asked for in the Region the bucket IS in, are removed: the
    // refusals above are about the resource not being there, nothing else.
    untag(&emulator, Some(DEFAULT_REGION), kind.name(), &["env"]).await;
    let tags = kind.tags(&emulator, Some(DEFAULT_REGION)).await;
    assert_eq!(
        tags,
        want_tags(&[
            ("owner", Some("platform")),
            ("retain", None),
            ("cost-centre", Some("4711")),
        ]),
        "case in-Region removal: expected the listed key gone, got {tags:?}"
    );
}

/// RULE: `resourceArn`, when the request carries one, is the ARN of the resource
/// `resourceName` names — one that is not is refused `InvalidInputException`
/// (HTTP 400) and removes nothing.
#[tokio::test]
async fn an_arn_that_is_not_the_named_resources_own_is_refused() {
    let emulator = Emulator::new();
    Kind::Instance.create(&emulator, None, TAGS).await;
    Kind::Disk.create(&emulator, None, TAGS).await;

    let instance_arn = Kind::Instance.read(&emulator, None).await["arn"]
        .as_str()
        .expect("an instance reports its ARN")
        .to_string();
    let disk_arn = Kind::Disk.read(&emulator, None).await["arn"]
        .as_str()
        .expect("a disk reports its ARN")
        .to_string();

    // Another resource's ARN, and an ARN of this service that names no resource
    // at all: both disagree with the name, so both are refused.
    let elsewhere = format!("arn:aws:lightsail:{DEFAULT_REGION}:000000000000:Instance/absent");
    for (case, arn) in [
        ("another resource's ARN", disk_arn.as_str()),
        ("an ARN naming no resource", elsewhere.as_str()),
    ] {
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({
                    "resourceName": Kind::Instance.name(),
                    "resourceArn": arn,
                    "tagKeys": ["env"],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
        let tags = Kind::Instance.tags(&emulator, None).await;
        assert_eq!(
            tags,
            want_tags(TAGS),
            "case {case}: expected the refusal to remove nothing, got {tags:?}"
        );
    }

    // The resource's OWN ARN beside its name is served.
    let answered = emulator
        .ok(
            "UntagResource",
            json!({
                "resourceName": Kind::Instance.name(),
                "resourceArn": instance_arn,
                "tagKeys": ["env"],
            }),
        )
        .await;
    let operations = answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case the resource's own ARN: expected operations, got {answered:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case the resource's own ARN: expected one entry, got {operations:?}"
    );
    assert_entry(
        "the resource's own ARN",
        &operations[0],
        Kind::Instance.name(),
        Kind::Instance.resource_type(),
        DEFAULT_REGION,
    );
    let tags = Kind::Instance.tags(&emulator, None).await;
    assert_eq!(
        tags,
        want_tags(&[
            ("owner", Some("platform")),
            ("retain", None),
            ("cost-centre", Some("4711")),
        ]),
        "case the resource's own ARN: expected the listed key gone, got {tags:?}"
    );
    // The disk the mismatching ARN belonged to kept every tag it had.
    let disk_tags = Kind::Disk.tags(&emulator, None).await;
    assert_eq!(
        disk_tags,
        want_tags(TAGS),
        "case the resource's own ARN: expected the other resource untouched, got {disk_tags:?}"
    );
}
