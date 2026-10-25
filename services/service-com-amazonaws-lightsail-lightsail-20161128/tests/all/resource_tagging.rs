// SPDX-License-Identifier: CC0-1.0

//! `TagResource` — tagging a stored Lightsail resource, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has, called
//! with a credential scope where a case needs a region.
//!
//! The rule under test: `TagResource` merges the tags a request carries into the
//! tag set of the ONE stored record its `resourceName` addresses — whatever kind
//! of resource that is — so a key the resource already holds takes the new value
//! in place, a key it does not hold is appended in the order the request named
//! it, every other tag it carries is left alone, and the next read of that
//! resource through its own reader reports exactly that set; the resulting set is
//! bounded by the 50 tags a Lightsail resource may carry; a `resourceArn` the
//! request carries as well must be that same resource's own, or the call is
//! refused; and a name this account holds no resource of is refused with the
//! service's `DoesNotExist` envelope and changes nothing.
//!
//! So the cases are TABLES: one row per taggable resource KIND (the name-keyed
//! ones and the regionally identified ones), and one row per form the merge takes
//! (a first tag, several new keys at once, a value replaced, a key-only tag given
//! a value, a value dropped back to a key-only tag, one key named twice in one
//! request, an empty request).

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_TAGS;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The region every row but the regional cases is driven in, and the zone inside
/// it a zonal resource is created in.
const REGION: &str = "us-west-2";
/// See [`REGION`].
const ZONE: &str = "us-west-2a";

/// A tag set as a request carries it: `(key, value)`, where a value of `None` is
/// a key-only tag (the modelled `Tag$value` is optional).
type Tags = &'static [(&'static str, Option<&'static str>)];

/// What kind of resource a row tags.
///
/// A Lightsail resource is taggable exactly when the shape it is read back as
/// carries a `tags` member, so these are kinds of that sort this emulator both
/// creates and reads back: the ones a resource NAME alone addresses (an instance,
/// a disk, a disk snapshot, an alarm) and the ones a Region and a name address
/// together (a bucket, a container service, a load balancer).
#[derive(Clone, Copy, Debug)]
enum Kind {
    Instance,
    Disk,
    DiskSnapshot,
    Alarm,
    Bucket,
    ContainerService,
    LoadBalancer,
}

impl Kind {
    /// The value of the modelled `ResourceType` enum this kind reports, which is
    /// what the log entry of a tagging call against it must carry.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Instance => "Instance",
            Self::Disk => "Disk",
            Self::DiskSnapshot => "DiskSnapshot",
            Self::Alarm => "Alarm",
            Self::Bucket => "Bucket",
            Self::ContainerService => "ContainerService",
            Self::LoadBalancer => "LoadBalancer",
        }
    }
}

/// One kind row: the resource to stand up, and the tags it is created with.
struct Row {
    case: &'static str,
    kind: Kind,
    name: &'static str,
    /// The tags the CREATE carries, which is the set the tagging call merges
    /// into.
    created: Tags,
}

/// The kind rows. Every taggable kind this suite drives appears once, created
/// with a different starting set — none, one, two, a key-only tag — so the merge
/// is exercised against each starting shape.
const ROWS: &[Row] = &[
    Row {
        case: "an instance created with no tags at all",
        kind: Kind::Instance,
        name: "tagged-instance",
        created: &[],
    },
    Row {
        case: "a block storage disk created with one tag",
        kind: Kind::Disk,
        name: "tagged-disk",
        created: &[("stage", Some("dev"))],
    },
    Row {
        case: "a disk snapshot created with a key-only tag",
        kind: Kind::DiskSnapshot,
        name: "tagged-disk-snapshot",
        created: &[("key-only", None)],
    },
    Row {
        case: "an alarm created with two tags",
        kind: Kind::Alarm,
        name: "tagged-alarm",
        created: &[("stage", Some("dev")), ("owner", Some("platform"))],
    },
    Row {
        case: "a bucket, a regionally identified resource, created with one tag",
        kind: Kind::Bucket,
        name: "tagged-bucket",
        created: &[("stage", Some("dev"))],
    },
    Row {
        case: "a container service, created with no tags at all",
        kind: Kind::ContainerService,
        name: "tagged-container-service",
        created: &[],
    },
    Row {
        case: "a load balancer, created with a key-only tag",
        kind: Kind::LoadBalancer,
        name: "tagged-load-balancer",
        created: &[("key-only", None)],
    },
];

/// The tag set `tags` describes, as a request body member.
fn tag_list(tags: Tags) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect(),
    )
}

/// The tags `resource` reports, as `(key, value)` pairs in the order it reports
/// them.
fn reported_tags(resource: &Value) -> Vec<(String, Option<String>)> {
    resource["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a Lightsail resource reports its tags as a list, got {resource:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// `tags` as the same `(key, value)` pairs [`reported_tags`] answers, so an
/// expectation written as a table compares against a read.
fn want_tags(tags: Tags) -> Vec<(String, Option<String>)> {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
        .collect()
}

/// Call `operation` in `region` and require `want_status`, answering the body.
async fn ok_in(
    emulator: &Emulator,
    region: &str,
    operation: &str,
    body: Value,
    want_status: u16,
) -> Value {
    let (status, parsed) = emulator.call_in(Some(region), operation, body).await;
    assert_eq!(
        status, want_status,
        "case {operation} in {region}: expected HTTP {want_status}, got {status} {parsed:?}"
    );
    parsed
}

/// The name of the source resource a snapshot row is taken of.
fn source_name(name: &str) -> String {
    format!("{name}-source")
}

/// Stand up the resource `row` names in `region`, with the tags the row's create
/// carries.
async fn create(emulator: &Emulator, region: &str, row: &Row) {
    let tags = tag_list(row.created);
    match row.kind {
        Kind::Instance => {
            let mut body = json!({
                "instanceNames": [row.name],
                "availabilityZone": ZONE,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            });
            body["tags"] = tags;
            ok_in(emulator, region, "CreateInstances", body, 200).await;
        }
        Kind::Disk => {
            let mut body =
                json!({"diskName": row.name, "availabilityZone": ZONE, "sizeInGb": 8});
            body["tags"] = tags;
            ok_in(emulator, region, "CreateDisk", body, 200).await;
        }
        Kind::DiskSnapshot => {
            let source = source_name(row.name);
            ok_in(
                emulator,
                region,
                "CreateDisk",
                json!({"diskName": source, "availabilityZone": ZONE, "sizeInGb": 8}),
                200,
            )
            .await;
            let mut body = json!({"diskName": source, "diskSnapshotName": row.name});
            body["tags"] = tags;
            ok_in(emulator, region, "CreateDiskSnapshot", body, 200).await;
        }
        Kind::Alarm => {
            let source = source_name(row.name);
            ok_in(
                emulator,
                region,
                "CreateInstances",
                json!({
                    "instanceNames": [source],
                    "availabilityZone": ZONE,
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": "nano_3_0",
                }),
                200,
            )
            .await;
            let mut body = json!({
                "alarmName": row.name,
                "monitoredResourceName": source,
                "metricName": "CPUUtilization",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 80.0,
                "evaluationPeriods": 2,
            });
            body["tags"] = tags;
            ok_in(emulator, region, "PutAlarm", body, 200).await;
        }
        Kind::Bucket => {
            let mut body = json!({"bucketName": row.name, "bundleId": "small_1_0"});
            body["tags"] = tags;
            ok_in(emulator, region, "CreateBucket", body, 200).await;
        }
        Kind::ContainerService => {
            let mut body = json!({"serviceName": row.name, "power": "nano", "scale": 1});
            body["tags"] = tags;
            // `CreateContainerService` answers HTTP 201, its modelled success
            // status.
            ok_in(emulator, region, "CreateContainerService", body, 201).await;
        }
        Kind::LoadBalancer => {
            let mut body = json!({"loadBalancerName": row.name, "instancePort": 80});
            body["tags"] = tags;
            ok_in(emulator, region, "CreateLoadBalancer", body, 200).await;
        }
    }
}

/// The resource `name` of kind `kind`, read back through that kind's OWN reader
/// rather than through the tagging call's answer.
async fn read_resource(emulator: &Emulator, region: &str, kind: Kind, name: &str) -> Value {
    match kind {
        Kind::Instance => {
            ok_in(
                emulator,
                region,
                "GetInstance",
                json!({"instanceName": name}),
                200,
            )
            .await["instance"]
                .clone()
        }
        Kind::Disk => {
            ok_in(emulator, region, "GetDisk", json!({"diskName": name}), 200).await["disk"].clone()
        }
        Kind::DiskSnapshot => {
            ok_in(
                emulator,
                region,
                "GetDiskSnapshot",
                json!({"diskSnapshotName": name}),
                200,
            )
            .await["diskSnapshot"]
                .clone()
        }
        Kind::Alarm => first_of(
            ok_in(
                emulator,
                region,
                "GetAlarms",
                json!({"alarmName": name}),
                200,
            )
            .await,
            "alarms",
            name,
        ),
        Kind::Bucket => first_of(
            ok_in(
                emulator,
                region,
                "GetBuckets",
                json!({"bucketName": name}),
                200,
            )
            .await,
            "buckets",
            name,
        ),
        Kind::ContainerService => first_of(
            ok_in(
                emulator,
                region,
                "GetContainerServices",
                json!({"serviceName": name}),
                200,
            )
            .await,
            "containerServices",
            name,
        ),
        Kind::LoadBalancer => {
            ok_in(
                emulator,
                region,
                "GetLoadBalancer",
                json!({"loadBalancerName": name}),
                200,
            )
            .await["loadBalancer"]
                .clone()
        }
    }
}

/// The tags the resource `name` carries now, as its own reader reports them.
async fn read_tags(
    emulator: &Emulator,
    region: &str,
    kind: Kind,
    name: &str,
) -> Vec<(String, Option<String>)> {
    reported_tags(&read_resource(emulator, region, kind, name).await)
}

/// The ARN the resource `name` reports — the one value a tagging call may name it
/// by besides its name.
async fn read_arn(emulator: &Emulator, region: &str, kind: Kind, name: &str) -> String {
    let resource = read_resource(emulator, region, kind, name).await;
    resource["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case reading {name}: expected an arn, got {resource:?}"))
        .to_string()
}

/// The one resource a listing reader answers for `name`, from the `member` array
/// of its response.
fn first_of(body: Value, member: &str, name: &str) -> Value {
    let listed = body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case reading {name}: expected a {member} array, got {body:?}"))
        .clone();
    assert_eq!(
        listed.len(),
        1,
        "case reading {name}: expected exactly one {member} entry, got {listed:?}"
    );
    listed[0].clone()
}

/// Tag `name` in `region` with `tags`, answering the `operations` the call
/// reports.
async fn tag(emulator: &Emulator, region: &str, name: &str, tags: Tags) -> Vec<Value> {
    let answered = ok_in(
        emulator,
        region,
        "TagResource",
        json!({"resourceName": name, "tags": tag_list(tags)}),
        200,
    )
    .await;
    answered["operations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case tagging {name}: expected an operations array, got {answered:?}")
        })
        .clone()
}

/// Assert the one log entry a tagging call answers with describes that call:
/// against the resource it named, of the kind that resource is, terminal, and
/// located where the resource is.
fn assert_logged(case: &str, operations: &[Value], kind: Kind, name: &str, region: &str) {
    assert_eq!(
        operations.len(),
        1,
        "case {case}: one tagging call changes one resource, so it logs one operation, got \
         {operations:?}"
    );
    let operation = &operations[0];
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the operation against the named resource, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(kind.resource_type()),
        "case {case}: expected the resource's own type, got {operation:?}"
    );
    // NO `operationType`: the modelled `OperationType` enum declares a member for
    // each call that writes this log and none for a change to a resource's tags,
    // and `Operation$operationType` is optional — the same entry `UntagResource`
    // writes (pinned in `resource_tags.rs`).
    assert!(
        operation["operationType"].is_null(),
        "case {case}: expected no operationType, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: the tags are stored by the time the call answers, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: a tag change has nothing left for the clock to settle, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: expected the operation where the resource is, got {operation:?}"
    );
    assert!(
        operation["createdAt"].is_number(),
        "case {case}: expected the operation to carry its creation time, got {operation:?}"
    );
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a tagging call merges its tags into the stored record its name
/// addresses, whatever kind of resource that is, and the kind's own reader
/// reports the merged set afterwards.
#[tokio::test]
async fn every_taggable_kind_merges_the_request_into_its_stored_tags() {
    let emulator = Emulator::new();

    // The same request against every kind: one key each row already holds (for
    // the rows that hold one), one key none of them holds, and a key-only tag.
    const ADDED: Tags = &[
        ("stage", Some("prod")),
        ("cost-centre", Some("4711")),
        ("audited", None),
    ];

    for row in ROWS {
        let case = row.case;
        create(&emulator, REGION, row).await;
        let before = read_tags(&emulator, REGION, row.kind, row.name).await;
        assert_eq!(
            before,
            want_tags(row.created),
            "case {case}: expected the create's own tags before tagging, got {before:?}"
        );

        let operations = tag(&emulator, REGION, row.name, ADDED).await;
        assert_logged(case, &operations, row.kind, row.name, REGION);

        // A key the resource already held keeps its PLACE and takes the new
        // value; a key it did not hold is appended in request order.
        let mut want = want_tags(row.created);
        for (key, value) in ADDED {
            let value = value.map(str::to_string);
            match want.iter_mut().find(|(held, _)| held == key) {
                Some(held) => held.1 = value,
                None => want.push((key.to_string(), value)),
            }
        }
        let after = read_tags(&emulator, REGION, row.kind, row.name).await;
        assert_eq!(
            after, want,
            "case {case}: expected {want:?} after tagging, got {after:?}"
        );

        // The read is a read: it changed nothing, so the next one answers the
        // same set.
        let again = read_tags(&emulator, REGION, row.kind, row.name).await;
        assert_eq!(
            again, want,
            "case {case}: a read must not change the tags, got {again:?}"
        );
    }
}

/// One step of the merge table: what the request carries, and the whole set the
/// resource must report afterwards.
struct Step {
    case: &'static str,
    request: Tags,
    want: Tags,
}

/// The forms the merge takes, applied in order to one resource that starts with
/// no tags at all.
const STEPS: &[Step] = &[
    Step {
        case: "the first tag on an untagged resource",
        request: &[("alpha", Some("1"))],
        want: &[("alpha", Some("1"))],
    },
    Step {
        case: "two new keys at once, appended in request order",
        request: &[("beta", Some("2")), ("gamma", None)],
        want: &[("alpha", Some("1")), ("beta", Some("2")), ("gamma", None)],
    },
    Step {
        case: "a key already held takes the new value in its own place",
        request: &[("beta", Some("22"))],
        want: &[("alpha", Some("1")), ("beta", Some("22")), ("gamma", None)],
    },
    Step {
        case: "a key-only tag given a value",
        request: &[("gamma", Some("3"))],
        want: &[("alpha", Some("1")), ("beta", Some("22")), ("gamma", Some("3"))],
    },
    Step {
        case: "a value dropped back to a key-only tag",
        request: &[("alpha", None)],
        want: &[("alpha", None), ("beta", Some("22")), ("gamma", Some("3"))],
    },
    Step {
        case: "one key named twice in one request: the last value wins",
        request: &[("delta", Some("first")), ("delta", Some("last"))],
        want: &[
            ("alpha", None),
            ("beta", Some("22")),
            ("gamma", Some("3")),
            ("delta", Some("last")),
        ],
    },
    Step {
        case: "an empty tag list changes nothing",
        request: &[],
        want: &[
            ("alpha", None),
            ("beta", Some("22")),
            ("gamma", Some("3")),
            ("delta", Some("last")),
        ],
    },
];

/// RULE: the merge is by tag key — "tag keys must be unique per resource" — so a
/// key is held once however many calls name it, and the set the reader reports is
/// the one the sequence of calls built.
#[tokio::test]
async fn the_merge_is_by_key_over_a_sequence_of_tagging_calls() {
    let emulator = Emulator::new();
    // Two kinds, so the rule is pinned on a name-keyed resource AND on a
    // regionally identified one.
    let rows = [
        Row {
            case: "an instance",
            kind: Kind::Instance,
            name: "merge-instance",
            created: &[],
        },
        Row {
            case: "a bucket",
            kind: Kind::Bucket,
            name: "merge-bucket",
            created: &[],
        },
    ];

    for row in &rows {
        create(&emulator, REGION, row).await;
        for step in STEPS {
            let case = format!("{} / {}", row.case, step.case);
            let operations = tag(&emulator, REGION, row.name, step.request).await;
            assert_logged(&case, &operations, row.kind, row.name, REGION);
            let after = read_tags(&emulator, REGION, row.kind, row.name).await;
            assert_eq!(
                after,
                want_tags(step.want),
                "case {case}: expected {:?}, got {after:?}",
                step.want
            );
        }
    }
}

/// RULE: the tag set a resource ends up with is bounded — a Lightsail resource
/// carries at most [`MAX_TAGS`] tags — and a request that would take it past the
/// bound is refused and leaves the set it already holds alone.
#[tokio::test]
async fn the_resulting_tag_set_is_bounded() {
    let emulator = Emulator::new();
    let row = Row {
        case: "a bounded instance",
        kind: Kind::Instance,
        name: "bounded-instance",
        created: &[],
    };
    create(&emulator, REGION, &row).await;

    // Exactly the bound, in one call: served.
    let full: Vec<(String, Option<String>)> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index}"), Some(format!("value-{index}"))))
        .collect();
    let request = Value::Array(
        full.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    );
    ok_in(
        &emulator,
        REGION,
        "TagResource",
        json!({"resourceName": row.name, "tags": request}),
        200,
    )
    .await;
    let at_bound = read_tags(&emulator, REGION, row.kind, row.name).await;
    assert_eq!(
        at_bound, full,
        "case the bound in one call: expected {MAX_TAGS} tags, got {at_bound:?}"
    );

    // A key it already holds does not grow the set, so it is served AT the
    // bound.
    ok_in(
        &emulator,
        REGION,
        "TagResource",
        json!({
            "resourceName": row.name,
            "tags": [{"key": "key-0", "value": "replaced"}],
        }),
        200,
    )
    .await;
    let mut want = full.clone();
    want[0].1 = Some("replaced".to_string());
    let replaced = read_tags(&emulator, REGION, row.kind, row.name).await;
    assert_eq!(
        replaced, want,
        "case replacing a value at the bound: expected {want:?}, got {replaced:?}"
    );

    // One key past the bound is refused, and changes nothing.
    let (status, body) = emulator
        .call_in(
            Some(REGION),
            "TagResource",
            json!({
                "resourceName": row.name,
                "tags": [{"key": "one-too-many", "value": "x"}],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one key past the bound: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case one key past the bound: expected the modelled refusal, got {body:?}"
    );
    let unchanged = read_tags(&emulator, REGION, row.kind, row.name).await;
    assert_eq!(
        unchanged, want,
        "case one key past the bound: a refused request changes nothing, got {unchanged:?}"
    );

    // The same bound on a FRESH resource asked for one tag too many in a single
    // call.
    let other = Row {
        case: "a second bounded instance",
        kind: Kind::Instance,
        name: "bounded-instance-two",
        created: &[],
    };
    create(&emulator, REGION, &other).await;
    let too_many = Value::Array(
        (0..=MAX_TAGS)
            .map(|index| json!({"key": format!("key-{index}"), "value": "x"}))
            .collect(),
    );
    let (status, body) = emulator
        .call_in(
            Some(REGION),
            "TagResource",
            json!({"resourceName": other.name, "tags": too_many}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {} past the bound in one call: expected HTTP 400, got {status} {body:?}",
        MAX_TAGS + 1
    );
    let none = read_tags(&emulator, REGION, other.kind, other.name).await;
    assert!(
        none.is_empty(),
        "case {} past the bound in one call: nothing is stored, got {none:?}",
        MAX_TAGS + 1
    );

    // A request carrying far more than the bound is refused on the request's own
    // list, before any record is reached: the same refusal, and still nothing
    // stored.
    let far_too_many = 10 * MAX_TAGS;
    let (status, body) = emulator
        .call_in(
            Some(REGION),
            "TagResource",
            json!({
                "resourceName": other.name,
                "tags": Value::Array(
                    (0..far_too_many)
                        .map(|index| json!({"key": format!("key-{index}"), "value": "x"}))
                        .collect(),
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {far_too_many} tags in one request: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {far_too_many} tags in one request: expected the modelled refusal, got {body:?}"
    );
    let still_none = read_tags(&emulator, REGION, other.kind, other.name).await;
    assert!(
        still_none.is_empty(),
        "case {far_too_many} tags in one request: nothing is stored, got {still_none:?}"
    );
}

/// RULE: a name this account holds no resource of is refused with Lightsail's
/// `DoesNotExist` envelope — HTTP 400, the modelled `NotFoundException` on the
/// wire — and stores nothing.
#[tokio::test]
async fn a_name_no_resource_holds_is_refused() {
    let emulator = Emulator::new();
    // One resource exists, so the refusals below are about the NAME rather than
    // about an empty store.
    create(
        &emulator,
        REGION,
        &Row {
            case: "the one resource",
            kind: Kind::Instance,
            name: "the-only-instance",
            created: &[],
        },
    )
    .await;

    for (case, name) in [
        ("a name nothing was ever created under", "never-created"),
        ("the source name of a resource that was never made", "the-only-instance-x"),
        ("a name differing only in case", "The-Only-Instance"),
    ] {
        let (status, body) = emulator
            .call_in(
                Some(REGION),
                "TagResource",
                json!({"resourceName": name, "tags": [{"key": "stage", "value": "prod"}]}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the not-found envelope, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist code a client branches on, got {body:?}"
        );
    }

    // The resource that DOES exist was not touched by any of those calls.
    let tags = read_tags(&emulator, REGION, Kind::Instance, "the-only-instance").await;
    assert!(
        tags.is_empty(),
        "case the one resource: a refused request stores nothing, got {tags:?}"
    );
}

/// RULE: a resource a Region and a name identify together is addressed in its own
/// Region, so the same name in two Regions names two resources with two tag sets.
#[tokio::test]
async fn a_regional_resource_is_tagged_in_its_own_region() {
    let emulator = Emulator::new();
    // A load balancer name "identifies one load balancer in ONE Region" (pinned
    // in `load_balancer_provisioning.rs`), which is what lets one name hold two
    // records here.
    const NAME: &str = "shared-balancer-name";
    const ELSEWHERE: &str = "eu-west-1";

    for region in [REGION, ELSEWHERE] {
        ok_in(
            &emulator,
            region,
            "CreateLoadBalancer",
            json!({"loadBalancerName": NAME, "instancePort": 80}),
            200,
        )
        .await;
    }

    // Each Region's load balancer takes its own tags.
    for (region, tags) in [
        (REGION, &[("where", Some("west"))] as Tags),
        (
            ELSEWHERE,
            &[("where", Some("ireland")), ("extra", None)] as Tags,
        ),
    ] {
        let operations = tag(&emulator, region, NAME, tags).await;
        assert_logged(region, &operations, Kind::LoadBalancer, NAME, region);
        let read = read_tags(&emulator, region, Kind::LoadBalancer, NAME).await;
        assert_eq!(
            read,
            want_tags(tags),
            "case tagging in {region}: expected {tags:?}, got {read:?}"
        );
    }

    // And neither Region's call reached the other's record.
    for (region, want) in [
        (REGION, &[("where", Some("west"))] as Tags),
        (
            ELSEWHERE,
            &[("where", Some("ireland")), ("extra", None)] as Tags,
        ),
    ] {
        let read = read_tags(&emulator, region, Kind::LoadBalancer, NAME).await;
        assert_eq!(
            read,
            want_tags(want),
            "case the load balancer in {region}: expected {want:?}, got {read:?}"
        );
    }

    // A Region that holds no load balancer of that name refuses the call.
    let (status, body) = emulator
        .call_in(
            Some("ap-southeast-2"),
            "TagResource",
            json!({"resourceName": NAME, "tags": [{"key": "where", "value": "sydney"}]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a region holding no load balancer of that name: expected HTTP 400, got {status} \
         {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case a region holding no load balancer of that name: expected the not-found envelope, \
         got {body:?}"
    );
}

/// RULE: `resourceArn`, when a request carries one, is the ARN of the SAME
/// resource `resourceName` names, so a request whose two members disagree is
/// refused and changes nothing (the rule `UntagResource` already answers by).
#[tokio::test]
async fn a_resource_arn_the_request_carries_must_be_the_named_resource_s_own() {
    let emulator = Emulator::new();
    for name in ["arn-instance", "arn-other-instance"] {
        ok_in(
            &emulator,
            REGION,
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
            200,
        )
        .await;
    }
    let own = read_arn(&emulator, REGION, Kind::Instance, "arn-instance").await;
    let other = read_arn(&emulator, REGION, Kind::Instance, "arn-other-instance").await;

    // The resource's own ARN beside its own name: served, and the tags land.
    ok_in(
        &emulator,
        REGION,
        "TagResource",
        json!({
            "resourceName": "arn-instance",
            "resourceArn": own,
            "tags": [{"key": "via", "value": "arn"}],
        }),
        200,
    )
    .await;
    let want = want_tags(&[("via", Some("arn"))]);
    let tagged = read_tags(&emulator, REGION, Kind::Instance, "arn-instance").await;
    assert_eq!(
        tagged, want,
        "case the resource's own arn: expected {want:?}, got {tagged:?}"
    );

    // An ARN that is not that resource's own is refused, whether it belongs to
    // another resource of this account or to no record at all. Each is a value
    // the modelled `ResourceArn` pattern admits, so the refusal is this handler's
    // rather than the boundary walk's.
    for (case, arn) in [
        ("another resource's own arn", other.as_str()),
        (
            "an arn of another account",
            "arn:aws:lightsail:us-west-2:999999999999:Instance/00000000-0000-4000-8000-000000000000",
        ),
    ] {
        let (status, body) = emulator
            .call_in(
                Some(REGION),
                "TagResource",
                json!({
                    "resourceName": "arn-instance",
                    "resourceArn": arn,
                    "tags": [{"key": "sneaked", "value": "in"}],
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
            "case {case}: expected the modelled refusal, got {body:?}"
        );
        let unchanged = read_tags(&emulator, REGION, Kind::Instance, "arn-instance").await;
        assert_eq!(
            unchanged, want,
            "case {case}: a refused request changes nothing, got {unchanged:?}"
        );
    }

    // And the other resource was never touched by any of it.
    let untouched = read_tags(&emulator, REGION, Kind::Instance, "arn-other-instance").await;
    assert!(
        untouched.is_empty(),
        "case the other resource: expected no tags, got {untouched:?}"
    );
}
