// SPDX-License-Identifier: CC0-1.0

//! Create-time tags — the per-resource tag-key rule — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: "tag keys must be unique per resource" (the modelled
//! `TagResource` documentation) is a rule about the RESOURCE, so it holds however
//! the resource came to carry its tags. A create's `tags` list therefore becomes
//! the resource's tag set collapsed BY KEY — a key the list names more than once
//! is held ONCE, carrying the last value that list gave it, in the place of its
//! first mention — exactly as a tag change naming one key twice leaves the
//! resource. The list is still bounded at the 50 tags a Lightsail resource
//! carries, and the collapsed set is what a later tag change counts against that
//! bound and what a later tag change or removal reaches.
//!
//! So the cases are TABLES: one row per FORM the repetition takes (a key twice, a
//! key three times, two keys each twice, a repeat between unique keys, a repeat
//! that drops the value, a repeat that adds one, no repetition at all), and one
//! row per create OPERATION that accepts `tags`, because the rule is the
//! resource's and not one operation's.

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_TAGS;

use crate::instance_lifecycle::{error_code, Emulator};

/// The region every row but the domain one is driven in, and the zone inside it a
/// zonal resource is created in.
const REGION: &str = "us-west-2";
/// See [`REGION`].
const ZONE: &str = "us-west-2a";
/// A domain is created, read and edited through the N. Virginia endpoint alone
/// (the modelled `CreateDomain` `InvalidInputException`: "Domain and distribution
/// APIs are only available in the N. Virginia (us-east-1) Amazon Web Services
/// Region").
const DOMAIN_REGION: &str = "us-east-1";

/// A tag set as a request carries it and as a reader reports it: `(key, value)`
/// in order, where a value of `None` is a key-only tag (the modelled `Tag$value`
/// is optional).
type Pairs = Vec<(String, Option<String>)>;

/// A table row's tags, written as literals.
type Literal = &'static [(&'static str, Option<&'static str>)];

/// `tags` as the owned pairs every helper below takes.
fn pairs(tags: Literal) -> Pairs {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
        .collect()
}

/// The tag set `tags` describes, as a request body member.
fn tag_list(tags: &Pairs) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect(),
    )
}

/// The tags `resource` reports, in the order it reports them.
fn reported_tags(case: &str, resource: &Value) -> Pairs {
    resource["tags"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: a Lightsail resource reports its tags as a list, got {resource:?}")
        })
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// What kind of resource a row creates.
///
/// One variant per create operation of this service that accepts `tags` and whose
/// resource this emulator also reads back, so the table spans the operations
/// rather than one of them: the zonal creates, the snapshot creates, the
/// regionally identified creates, the alarm put and the domain create served in
/// one Region alone.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Disk,
    Instance,
    DiskSnapshot,
    InstanceSnapshot,
    Alarm,
    Bucket,
    ContainerService,
    LoadBalancer,
    RelationalDatabase,
    Domain,
}

impl Kind {
    /// The region a row of this kind is created and read in.
    fn region(self) -> &'static str {
        match self {
            Self::Domain => DOMAIN_REGION,
            _ => REGION,
        }
    }

    /// The operation this kind is created through, and the HTTP status that
    /// operation answers on success (`CreateContainerService` is modelled 201,
    /// the rest 200).
    fn create_call(self) -> (&'static str, u16) {
        match self {
            Self::Disk => ("CreateDisk", 200),
            Self::Instance => ("CreateInstances", 200),
            Self::DiskSnapshot => ("CreateDiskSnapshot", 200),
            Self::InstanceSnapshot => ("CreateInstanceSnapshot", 200),
            Self::Alarm => ("PutAlarm", 200),
            Self::Bucket => ("CreateBucket", 200),
            Self::ContainerService => ("CreateContainerService", 201),
            Self::LoadBalancer => ("CreateLoadBalancer", 200),
            Self::RelationalDatabase => ("CreateRelationalDatabase", 200),
            Self::Domain => ("CreateDomain", 200),
        }
    }

    /// The create body for a resource of this kind called `name`, without the
    /// `tags` member a case adds.
    fn create_body(self, name: &str) -> Value {
        match self {
            Self::Disk => json!({"diskName": name, "availabilityZone": ZONE, "sizeInGb": 8}),
            Self::Instance => json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
            Self::DiskSnapshot => {
                json!({"diskName": source_name(name), "diskSnapshotName": name})
            }
            Self::InstanceSnapshot => {
                json!({"instanceName": source_name(name), "instanceSnapshotName": name})
            }
            Self::Alarm => json!({
                "alarmName": name,
                "monitoredResourceName": source_name(name),
                "metricName": "CPUUtilization",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 80.0,
                "evaluationPeriods": 2,
            }),
            Self::Bucket => json!({"bucketName": name, "bundleId": "small_1_0"}),
            Self::ContainerService => json!({"serviceName": name, "power": "nano", "scale": 1}),
            Self::LoadBalancer => json!({"loadBalancerName": name, "instancePort": 80}),
            Self::RelationalDatabase => json!({
                "relationalDatabaseName": name,
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
            }),
            Self::Domain => json!({"domainName": name}),
        }
    }
}

/// The name of the source resource a snapshot is taken of, or the instance an
/// alarm watches.
fn source_name(name: &str) -> String {
    format!("{name}-source")
}

/// Call `operation` in `region` and require `want_status`, answering the body.
async fn call_ok(
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

/// Stand up whatever a resource of `kind` needs to exist before it can be
/// created: a snapshot needs its source, an alarm needs the resource it watches.
async fn prepare(emulator: &Emulator, kind: Kind, name: &str) {
    let source = source_name(name);
    match kind {
        Kind::DiskSnapshot => {
            call_ok(
                emulator,
                kind.region(),
                "CreateDisk",
                json!({"diskName": source, "availabilityZone": ZONE, "sizeInGb": 8}),
                200,
            )
            .await;
        }
        Kind::InstanceSnapshot | Kind::Alarm => {
            call_ok(
                emulator,
                kind.region(),
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
        }
        _ => {}
    }
}

/// Create a resource of `kind` called `name` carrying `tags` — or carrying no
/// `tags` member at all when `tags` is `None` — answering the HTTP status and the
/// parsed body, so a case about a refusal reads them.
async fn create(
    emulator: &Emulator,
    kind: Kind,
    name: &str,
    tags: Option<&Pairs>,
) -> (u16, Value) {
    prepare(emulator, kind, name).await;
    let mut body = kind.create_body(name);
    if let Some(tags) = tags {
        body["tags"] = tag_list(tags);
    }
    let (operation, _) = kind.create_call();
    emulator
        .call_in(Some(kind.region()), operation, body)
        .await
}

/// Create a resource of `kind` called `name` carrying `tags` and require the
/// success its create operation is modelled with.
async fn create_ok(emulator: &Emulator, case: &str, kind: Kind, name: &str, tags: Option<&Pairs>) {
    let (_, want_status) = kind.create_call();
    let (status, parsed) = create(emulator, kind, name, tags).await;
    assert_eq!(
        status, want_status,
        "case {case}: expected HTTP {want_status} from the create, got {status} {parsed:?}"
    );
}

/// The resource `name` of kind `kind`, read back through that kind's OWN reader
/// rather than through the create's answer.
async fn read_resource(emulator: &Emulator, kind: Kind, name: &str) -> Value {
    let region = kind.region();
    match kind {
        Kind::Disk => {
            call_ok(emulator, region, "GetDisk", json!({"diskName": name}), 200).await["disk"]
                .clone()
        }
        Kind::Instance => {
            call_ok(
                emulator,
                region,
                "GetInstance",
                json!({"instanceName": name}),
                200,
            )
            .await["instance"]
                .clone()
        }
        Kind::DiskSnapshot => {
            call_ok(
                emulator,
                region,
                "GetDiskSnapshot",
                json!({"diskSnapshotName": name}),
                200,
            )
            .await["diskSnapshot"]
                .clone()
        }
        Kind::InstanceSnapshot => {
            call_ok(
                emulator,
                region,
                "GetInstanceSnapshot",
                json!({"instanceSnapshotName": name}),
                200,
            )
            .await["instanceSnapshot"]
                .clone()
        }
        Kind::Alarm => first_of(
            call_ok(emulator, region, "GetAlarms", json!({"alarmName": name}), 200).await,
            "alarms",
            name,
        ),
        Kind::Bucket => first_of(
            call_ok(
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
            call_ok(
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
            call_ok(
                emulator,
                region,
                "GetLoadBalancer",
                json!({"loadBalancerName": name}),
                200,
            )
            .await["loadBalancer"]
                .clone()
        }
        Kind::RelationalDatabase => {
            call_ok(
                emulator,
                region,
                "GetRelationalDatabase",
                json!({"relationalDatabaseName": name}),
                200,
            )
            .await["relationalDatabase"]
                .clone()
        }
        Kind::Domain => {
            call_ok(
                emulator,
                region,
                "GetDomain",
                json!({"domainName": name}),
                200,
            )
            .await["domain"]
                .clone()
        }
    }
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

/// The tags the resource `name` carries now, as its own reader reports them.
async fn read_tags(emulator: &Emulator, case: &str, kind: Kind, name: &str) -> Pairs {
    reported_tags(case, &read_resource(emulator, kind, name).await)
}

/// The tags the disk `name` carries, as the PLURAL reader reports them — the
/// other door onto the same record.
async fn listed_disk_tags(emulator: &Emulator, case: &str, name: &str) -> Pairs {
    let disks = call_ok(emulator, REGION, "GetDisks", json!({}), 200).await;
    let listed = disks["disks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a disks array, got {disks:?}"))
        .iter()
        .find(|disk| disk["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case}: expected GetDisks to report '{name}'"))
        .clone();
    reported_tags(case, &listed)
}

/// One form the repetition takes: the list a create carries, and the set the
/// resource must end up holding.
struct Form {
    case: &'static str,
    requested: Literal,
    want: Literal,
}

/// The forms. Each row differs in HOW the request names its keys — how many times
/// one key appears, where the repeats sit among unique keys, and whether the
/// repeat carries a value — so the table spans the rule rather than the one list
/// a single request happened to carry.
const FORMS: &[Form] = &[
    Form {
        case: "an empty list leaves the resource with no tags",
        requested: &[],
        want: &[],
    },
    Form {
        case: "distinct keys are stored in the order the request named them",
        requested: &[("alpha", Some("1")), ("beta", None), ("gamma", Some("3"))],
        want: &[("alpha", Some("1")), ("beta", None), ("gamma", Some("3"))],
    },
    Form {
        case: "one key named twice: the last value wins",
        requested: &[("env", Some("a")), ("env", Some("b"))],
        want: &[("env", Some("b"))],
    },
    Form {
        case: "one key named three times: the last value wins",
        requested: &[("env", Some("a")), ("env", Some("b")), ("env", Some("c"))],
        want: &[("env", Some("c"))],
    },
    Form {
        case: "two keys each named twice, interleaved",
        requested: &[
            ("env", Some("a")),
            ("tier", Some("one")),
            ("env", Some("b")),
            ("tier", Some("two")),
        ],
        want: &[("env", Some("b")), ("tier", Some("two"))],
    },
    Form {
        case: "a repeated key keeps the place of its FIRST mention",
        requested: &[
            ("first", Some("1")),
            ("env", Some("a")),
            ("last", Some("9")),
            ("env", Some("b")),
        ],
        want: &[("first", Some("1")), ("env", Some("b")), ("last", Some("9"))],
    },
    Form {
        case: "the repeat drops the value, leaving a key-only tag",
        requested: &[("env", Some("a")), ("env", None)],
        want: &[("env", None)],
    },
    Form {
        case: "the repeat gives a key-only tag a value",
        requested: &[("env", None), ("env", Some("b"))],
        want: &[("env", Some("b"))],
    },
    Form {
        case: "a key-only tag named twice",
        requested: &[("env", None), ("env", None)],
        want: &[("env", None)],
    },
    Form {
        case: "the same key and the same value twice",
        requested: &[("env", Some("a")), ("env", Some("a"))],
        want: &[("env", Some("a"))],
    },
];

/// The create operations the rule is pinned on, one row per operation that
/// accepts `tags` and whose resource is read back here.
const KINDS: &[(Kind, &str)] = &[
    (Kind::Disk, "dup-disk"),
    (Kind::Instance, "dup-instance"),
    (Kind::DiskSnapshot, "dup-disk-snapshot"),
    (Kind::InstanceSnapshot, "dup-instance-snapshot"),
    (Kind::Alarm, "dup-alarm"),
    (Kind::Bucket, "dup-bucket"),
    (Kind::ContainerService, "dup-container-service"),
    (Kind::LoadBalancer, "dup-load-balancer"),
    (Kind::RelationalDatabase, "dup-database"),
    (Kind::Domain, "dup.example"),
];

/// RULE: a create's `tags` list becomes the resource's tag set collapsed by key —
/// a key the list names more than once is held once, carrying the last value that
/// list gave it, in the place of its first mention — and both readers of the
/// record report that one set.
#[tokio::test]
async fn a_create_collapses_its_tag_list_by_key() {
    let emulator = Emulator::new();
    for (index, form) in FORMS.iter().enumerate() {
        let name = format!("form-disk-{index}");
        let requested = pairs(form.requested);
        let want = pairs(form.want);
        create_ok(
            &emulator,
            form.case,
            Kind::Disk,
            &name,
            Some(&requested),
        )
        .await;

        let read = read_tags(&emulator, form.case, Kind::Disk, &name).await;
        assert_eq!(
            read, want,
            "case {}: expected {want:?} from GetDisk, got {read:?}",
            form.case
        );
        let listed = listed_disk_tags(&emulator, form.case, &name).await;
        assert_eq!(
            listed, want,
            "case {}: expected {want:?} from GetDisks, got {listed:?}",
            form.case
        );
    }

    // A create carrying no `tags` member at all is the same resource as one
    // carrying an empty list: no tags.
    create_ok(
        &emulator,
        "no tags member at all",
        Kind::Disk,
        "untagged-disk",
        None,
    )
    .await;
    let untagged = read_tags(&emulator, "no tags member at all", Kind::Disk, "untagged-disk").await;
    assert!(
        untagged.is_empty(),
        "case no tags member at all: expected no tags, got {untagged:?}"
    );
}

/// RULE: the rule belongs to the RESOURCE, so every create that accepts `tags`
/// applies it — the resource each one stores carries each key once.
#[tokio::test]
async fn every_create_that_takes_tags_collapses_by_key() {
    let emulator = Emulator::new();
    // One list for every kind, carrying a key named twice, a key named twice with
    // the value dropped, and a key named once — so each row exercises the
    // collapse, the value replacement and the untouched key at once.
    let requested = pairs(&[
        ("env", Some("a")),
        ("owner", Some("platform")),
        ("env", Some("b")),
        ("stage", Some("first")),
        ("stage", None),
    ]);
    let want = pairs(&[("env", Some("b")), ("owner", Some("platform")), ("stage", None)]);

    for (kind, name) in KINDS {
        let case = format!("{kind:?} created with a key named twice");
        create_ok(&emulator, &case, *kind, name, Some(&requested)).await;
        let read = read_tags(&emulator, &case, *kind, name).await;
        assert_eq!(
            read, want,
            "case {case}: expected {want:?} from its own reader, got {read:?}"
        );
    }
}

/// RULE: the bound is on the LIST the request carries — a list of more than the
/// tags a Lightsail resource holds is refused whatever its keys collapse to — and
/// the refusal names the count and the bound.
#[tokio::test]
async fn the_tag_list_is_still_bounded() {
    let emulator = Emulator::new();

    // A list at the bound whose keys collapse to half as many: served, and the
    // resource holds the collapsed set.
    let collapsing: Pairs = (0..MAX_TAGS)
        .map(|index| (format!("key-{}", index / 2), Some(format!("value-{index}"))))
        .collect();
    let case = "a list at the bound whose keys collapse to half as many";
    create_ok(
        &emulator,
        case,
        Kind::Disk,
        "collapsing-disk",
        Some(&collapsing),
    )
    .await;
    let read = read_tags(&emulator, case, Kind::Disk, "collapsing-disk").await;
    assert_eq!(
        read.len(),
        MAX_TAGS / 2,
        "case {case}: expected {} tags, got {read:?}",
        MAX_TAGS / 2
    );
    // The last value each key was given is the one it carries.
    for (position, (key, value)) in read.iter().enumerate() {
        assert_eq!(
            (key.as_str(), value.as_deref()),
            (
                format!("key-{position}").as_str(),
                Some(format!("value-{}", position * 2 + 1).as_str())
            ),
            "case {case}: expected key-{position} to carry the last value it was given, got \
             {key:?} = {value:?}"
        );
    }

    // Past the bound, in every form: all one key, all distinct keys, and a mix.
    let over: &[(&str, Pairs)] = &[
        (
            "one key past the bound, all distinct",
            (0..=MAX_TAGS)
                .map(|index| (format!("key-{index}"), Some(index.to_string())))
                .collect(),
        ),
        (
            "one key past the bound, every entry the same key",
            (0..=MAX_TAGS)
                .map(|index| ("env".to_string(), Some(index.to_string())))
                .collect(),
        ),
        (
            "an order of magnitude past the bound",
            (0..10 * MAX_TAGS)
                .map(|index| (format!("key-{}", index % 3), Some(index.to_string())))
                .collect(),
        ),
    ];
    for (case, requested) in over {
        let (status, body) = create(&emulator, Kind::Disk, "refused-disk", Some(requested)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&requested.len().to_string())
                && message.contains(&MAX_TAGS.to_string()),
            "case {case}: expected a message naming the {} entries and the bound of {MAX_TAGS}, \
             got {message:?}",
            requested.len()
        );
        // A refused create stores nothing.
        let (status, body) = emulator
            .call_in(Some(REGION), "GetDisk", json!({"diskName": "refused-disk"}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refused disk not to exist, got {status} {body:?}"
        );
    }
}

/// RULE: the collapsed set is the resource's real tag set, so a later tag change
/// reaches each key once, a removal of that key leaves nothing of it behind, and
/// only the collapsed count stands against the 50 tags a resource holds.
#[tokio::test]
async fn the_collapsed_set_is_what_later_tag_calls_act_on() {
    let emulator = Emulator::new();
    let name = "repairable-disk";
    let case = "a disk created with one key named twice";
    create_ok(
        &emulator,
        case,
        Kind::Disk,
        name,
        Some(&pairs(&[
            ("env", Some("a")),
            ("keep", Some("kept")),
            ("env", Some("b")),
        ])),
    )
    .await;

    // One tag change reaches the key, because the resource holds it once.
    call_ok(
        &emulator,
        REGION,
        "TagResource",
        json!({"resourceName": name, "tags": tag_list(&pairs(&[("env", Some("c"))]))}),
        200,
    )
    .await;
    let after_tag = read_tags(&emulator, case, Kind::Disk, name).await;
    assert_eq!(
        after_tag,
        pairs(&[("env", Some("c")), ("keep", Some("kept"))]),
        "case {case}: expected the tag change to reach the one env tag, got {after_tag:?}"
    );

    // Removing the key leaves nothing of it behind, and leaves the other tag.
    call_ok(
        &emulator,
        REGION,
        "UntagResource",
        json!({"resourceName": name, "tagKeys": ["env"]}),
        200,
    )
    .await;
    let after_untag = read_tags(&emulator, case, Kind::Disk, name).await;
    assert_eq!(
        after_untag,
        pairs(&[("keep", Some("kept"))]),
        "case {case}: expected only the untouched tag to remain, got {after_untag:?}"
    );

    // The collapsed count — not the length of the create's list — is what the
    // resulting-set bound of a later tag change counts against.
    let roomy = "roomy-disk";
    let case = "a disk created with two entries of one key";
    create_ok(
        &emulator,
        case,
        Kind::Disk,
        roomy,
        Some(&pairs(&[("env", Some("a")), ("env", Some("b"))])),
    )
    .await;
    let fresh: Pairs = (1..MAX_TAGS)
        .map(|index| (format!("key-{index}"), Some(index.to_string())))
        .collect();
    call_ok(
        &emulator,
        REGION,
        "TagResource",
        json!({"resourceName": roomy, "tags": tag_list(&fresh)}),
        200,
    )
    .await;
    let filled = read_tags(&emulator, case, Kind::Disk, roomy).await;
    assert_eq!(
        filled.len(),
        MAX_TAGS,
        "case {case}: expected the resource to fill to {MAX_TAGS} tags, got {}",
        filled.len()
    );

    // And one more key is past the bound, as it is for any other full resource.
    let (status, body) = emulator
        .call_in(
            Some(REGION),
            "TagResource",
            json!({
                "resourceName": roomy,
                "tags": tag_list(&pairs(&[("one-too-many", Some("x"))])),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 past the bound, got {status} {body:?}"
    );
    let unchanged = read_tags(&emulator, case, Kind::Disk, roomy).await;
    assert_eq!(
        unchanged.len(),
        MAX_TAGS,
        "case {case}: expected the refused call to change nothing, got {}",
        unchanged.len()
    );
}
