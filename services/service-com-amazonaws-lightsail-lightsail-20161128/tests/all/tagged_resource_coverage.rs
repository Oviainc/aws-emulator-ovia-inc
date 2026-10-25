// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` over the kinds of Lightsail resource whose modelled shape
//! carries a `tags` member that `resource_tags.rs` does not reach — the SSL/TLS
//! certificate, the CDN distribution, the three kinds of snapshot, the alarm and
//! the container service — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a tag change reaches EVERY resource of the account whose
//! modelled shape carries a `tags` member, each resolved by the name its own
//! reader serves it under — so a tag written at create is removable, and the
//! removal is observable through that reader. `resource_tags.rs` pins the same
//! rule for the instance, the block storage disk, the managed database, the
//! bucket, the load balancer and the contact method; the kinds below are the
//! rest of the class. The DNS zone is the one tag-carrying kind no case here can
//! drive: a zone is named by a dotted `DomainName`, which the modelled
//! `UntagResource$resourceName` pattern (`^\w[\w\-]*\w$`) does not admit.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, Emulator};
// The rule, the request shapes and the assertions are the sibling suite's: this
// one adds rows to the same class rather than a second harness.
use crate::resource_tags::{
    assert_entry, reported_tags, tag_list, untag, want_tags, TagRow, DEFAULT_REGION,
};

/// The tag set every resource in this suite is created with: two keys with
/// values, one key-only tag, and a key carrying a hyphen and a digits-only
/// value.
const TAGS: &[TagRow] = &[
    ("env", Some("staging")),
    ("team", Some("cdn")),
    ("keep", None),
    ("cost-centre", Some("8080")),
];

/// The keys every removal case in the kind table lists.
const REMOVED_KEYS: &[&str] = &["team", "cost-centre"];

/// What is left of [`TAGS`] once [`REMOVED_KEYS`] is removed.
const KEPT_TAGS: &[TagRow] = &[("env", Some("staging")), ("keep", None)];

/// The instance every kind that needs a source resource is built from: the
/// origin of the distribution, the source of the instance snapshot, and the
/// resource the alarm monitors.
const SOURCE_INSTANCE: &str = "coverage-instance";
/// The disk the disk snapshot is taken of.
const SOURCE_DISK: &str = "coverage-disk";
/// The managed database the database snapshot is taken of.
const SOURCE_DATABASE: &str = "coverage-database";

/// The kinds of tag-carrying resource this suite walks. The rows differ along
/// what the rule generalises over: the kind of resource, the operation that
/// creates it, the reader that serves it, and the shape of the key its record is
/// stored under — a name alone (the certificate, the distribution, each
/// snapshot, the alarm) or the Region together with the name (the container
/// service).
#[derive(Clone, Copy, Debug)]
enum Kind {
    Certificate,
    Distribution,
    DiskSnapshot,
    InstanceSnapshot,
    DatabaseSnapshot,
    Alarm,
    ContainerService,
}

/// Every kind the table walks.
const KINDS: &[Kind] = &[
    Kind::Certificate,
    Kind::Distribution,
    Kind::DiskSnapshot,
    Kind::InstanceSnapshot,
    Kind::DatabaseSnapshot,
    Kind::Alarm,
    Kind::ContainerService,
];

impl Kind {
    /// The `ResourceType` this kind reports, from the modelled `ResourceType`
    /// enum (`smithy_query(query="shape", shape="com.amazonaws.lightsail#ResourceType")`).
    fn resource_type(self) -> &'static str {
        match self {
            Kind::Certificate => "Certificate",
            Kind::Distribution => "Distribution",
            Kind::DiskSnapshot => "DiskSnapshot",
            Kind::InstanceSnapshot => "InstanceSnapshot",
            Kind::DatabaseSnapshot => "RelationalDatabaseSnapshot",
            Kind::Alarm => "Alarm",
            Kind::ContainerService => "ContainerService",
        }
    }

    /// The name this suite creates this kind's resource under. Each satisfies
    /// both that resource's own modelled name constraint and
    /// `UntagResource$resourceName`'s pattern (`^\w[\w\-]*\w$`).
    fn name(self) -> &'static str {
        match self {
            Kind::Certificate => "coverage-certificate",
            Kind::Distribution => "coverage-distribution",
            Kind::DiskSnapshot => "coverage-disk-snapshot",
            Kind::InstanceSnapshot => "coverage-instance-snapshot",
            Kind::DatabaseSnapshot => "coverage-database-snapshot",
            Kind::Alarm => "coverage-alarm",
            Kind::ContainerService => "coverage-containers",
        }
    }

    /// The create request this kind's resource is made with, and the HTTP status
    /// that create answers.
    fn create_request(self, tags: &[TagRow]) -> (&'static str, Value, u16) {
        let name = self.name();
        let (operation, mut body, status) = match self {
            Kind::Certificate => (
                "CreateCertificate",
                json!({"certificateName": name, "domainName": "coverage.example"}),
                200,
            ),
            Kind::Distribution => (
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": SOURCE_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
                200,
            ),
            Kind::DiskSnapshot => (
                "CreateDiskSnapshot",
                json!({"diskName": SOURCE_DISK, "diskSnapshotName": name}),
                200,
            ),
            Kind::InstanceSnapshot => (
                "CreateInstanceSnapshot",
                json!({"instanceName": SOURCE_INSTANCE, "instanceSnapshotName": name}),
                200,
            ),
            Kind::DatabaseSnapshot => (
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": SOURCE_DATABASE,
                    "relationalDatabaseSnapshotName": name,
                }),
                200,
            ),
            Kind::Alarm => (
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": SOURCE_INSTANCE,
                    "metricName": "CPUUtilization",
                    "comparisonOperator": "GreaterThanThreshold",
                    "threshold": 80,
                    "evaluationPeriods": 2,
                }),
                200,
            ),
            // `CreateContainerService` answers the modelled 201 for the resource
            // it created.
            Kind::ContainerService => (
                "CreateContainerService",
                json!({"serviceName": name, "power": "nano", "scale": 2}),
                201,
            ),
        };
        if !tags.is_empty() {
            body["tags"] = tag_list(tags);
        }
        (operation, body, status)
    }

    /// Create this kind's resource carrying `tags`.
    async fn create(self, emulator: &Emulator, tags: &[TagRow]) {
        let (operation, body, want_status) = self.create_request(tags);
        let (status, answered) = emulator.call(operation, body).await;
        assert_eq!(
            status, want_status,
            "case creating {self:?}: expected HTTP {want_status} from {operation}, \
             got {status} {answered:?}"
        );
    }

    /// This kind's resource as its own reader reports it.
    async fn read(self, emulator: &Emulator) -> Value {
        let name = self.name();
        match self {
            Kind::Certificate => first(
                emulator
                    .ok("GetCertificates", json!({"certificateName": name}))
                    .await,
                "certificates",
                name,
            ),
            Kind::Distribution => first(
                emulator
                    .ok("GetDistributions", json!({"distributionName": name}))
                    .await,
                "distributions",
                name,
            ),
            Kind::DiskSnapshot => emulator
                .ok("GetDiskSnapshot", json!({"diskSnapshotName": name}))
                .await["diskSnapshot"]
                .clone(),
            Kind::InstanceSnapshot => emulator
                .ok("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
                .await["instanceSnapshot"]
                .clone(),
            Kind::DatabaseSnapshot => emulator
                .ok(
                    "GetRelationalDatabaseSnapshot",
                    json!({"relationalDatabaseSnapshotName": name}),
                )
                .await["relationalDatabaseSnapshot"]
                .clone(),
            Kind::Alarm => first(
                emulator.ok("GetAlarms", json!({"alarmName": name})).await,
                "alarms",
                name,
            ),
            Kind::ContainerService => first(
                emulator
                    .ok("GetContainerServices", json!({"serviceName": name}))
                    .await,
                "containerServices",
                name,
            ),
        }
    }

    /// The tags this kind's resource reports now, read back through its own
    /// reader.
    async fn tags(self, emulator: &Emulator) -> Vec<(String, Option<String>)> {
        reported_tags(&self.read(emulator).await)
    }

    /// The ARN this kind's resource reports. A certificate reports it as
    /// `CertificateSummary$certificateArn`; every other kind as `arn`.
    async fn arn(self, emulator: &Emulator) -> String {
        let reported = self.read(emulator).await;
        let member = match self {
            Kind::Certificate => "certificateArn",
            _ => "arn",
        };
        reported[member]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {self:?}: expected a reported {member}, got {reported:?}")
            })
            .to_string()
    }
}

/// The one entry of the `member` list `body` reports, which must hold exactly
/// the resource `name` was asked for.
fn first(body: Value, member: &str, name: &str) -> Value {
    let entries = body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case reading {name}: expected a {member} array, got {body:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case reading {name}: expected exactly the one named resource in {member}, got {entries:?}"
    );
    entries[0].clone()
}

/// Create the resources the kinds above are built from — the instance a
/// distribution originates at, an instance snapshot is taken of and an alarm
/// monitors, the disk a disk snapshot is taken of, and the database a database
/// snapshot is taken of. None of them carries a tag, so every tag this suite
/// observes is one of the kind under test.
async fn create_sources(emulator: &Emulator) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [SOURCE_INSTANCE],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": SOURCE_DISK, "availabilityZone": "us-east-1a", "sizeInGb": 8}),
        )
        .await;
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": SOURCE_DATABASE,
                "availabilityZone": "us-east-1a",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
            }),
        )
        .await;
}

// ---------------------------------------------------------------------------
// The kind table
// ---------------------------------------------------------------------------

/// RULE: a tag a create wrote on any tag-carrying kind is removable, and the
/// removal is what that kind's own reader then reports.
#[tokio::test]
async fn every_tag_carrying_kind_loses_exactly_the_listed_keys() {
    for kind in KINDS {
        let case = format!("{kind:?}");
        let emulator = Emulator::new();
        create_sources(&emulator).await;
        kind.create(&emulator, TAGS).await;

        let created = kind.tags(&emulator).await;
        assert_eq!(
            created,
            want_tags(TAGS),
            "case {case}: expected the created resource to carry the request's tags, \
             got {created:?}"
        );

        let entry = untag(&emulator, None, kind.name(), REMOVED_KEYS).await;
        assert_entry(
            &case,
            &entry,
            kind.name(),
            kind.resource_type(),
            DEFAULT_REGION,
        );

        let left = kind.tags(&emulator).await;
        assert_eq!(
            left,
            want_tags(KEPT_TAGS),
            "case {case}: expected only the unlisted tags to survive, got {left:?}"
        );
    }
}

/// RULE: the removal depends only on the key set, so every key the resource
/// carries can be removed and repeating the call changes nothing.
#[tokio::test]
async fn every_key_can_be_removed_from_every_kind() {
    let all_keys: Vec<&str> = TAGS.iter().map(|(key, _)| *key).collect();
    for kind in KINDS {
        let case = format!("{kind:?}");
        let emulator = Emulator::new();
        create_sources(&emulator).await;
        kind.create(&emulator, TAGS).await;

        untag(&emulator, None, kind.name(), &all_keys).await;
        let once = kind.tags(&emulator).await;
        assert_eq!(
            once,
            want_tags(&[]),
            "case {case}: expected every tag gone, got {once:?}"
        );

        // A resource carrying no tag at all is still resolved, and a repeat
        // removal leaves it as it is.
        let entry = untag(&emulator, None, kind.name(), &all_keys).await;
        assert_entry(
            &case,
            &entry,
            kind.name(),
            kind.resource_type(),
            DEFAULT_REGION,
        );
        let twice = kind.tags(&emulator).await;
        assert_eq!(
            twice, once,
            "case {case}: expected the repeat removal to change nothing, got {twice:?}"
        );
    }
}

/// RULE: the call names ONE resource, so no other resource's tags move — not
/// even another kind carrying the very same keys.
#[tokio::test]
async fn only_the_named_resource_loses_tags() {
    let emulator = Emulator::new();
    create_sources(&emulator).await;
    for kind in KINDS {
        kind.create(&emulator, TAGS).await;
    }

    for (named, target) in KINDS.iter().enumerate() {
        let case = format!("untagging {target:?}");
        let entry = untag(&emulator, None, target.name(), REMOVED_KEYS).await;
        assert_entry(
            &case,
            &entry,
            target.name(),
            target.resource_type(),
            DEFAULT_REGION,
        );

        for (index, kind) in KINDS.iter().enumerate() {
            let tags = kind.tags(&emulator).await;
            // Every kind named so far has lost the listed keys; the rest still
            // carry every tag their create wrote.
            let want = if index <= named {
                want_tags(KEPT_TAGS)
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
// The ARN the request may carry as well
// ---------------------------------------------------------------------------

/// RULE: `resourceArn`, when the request carries one, is the ARN of the resource
/// `resourceName` names — its own ARN is served, another kind's is refused
/// `InvalidInputException` (HTTP 400) and removes nothing.
#[tokio::test]
async fn an_arn_is_checked_against_the_named_resource_of_every_kind() {
    let emulator = Emulator::new();
    create_sources(&emulator).await;
    for kind in KINDS {
        kind.create(&emulator, TAGS).await;
    }

    for (index, kind) in KINDS.iter().enumerate() {
        let case = format!("{kind:?}");
        // Another kind's ARN: the next kind in the table, so each row is
        // checked against a different one.
        let other = KINDS[(index + 1) % KINDS.len()];
        let other_arn = other.arn(&emulator).await;
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({
                    "resourceName": kind.name(),
                    "resourceArn": other_arn,
                    "tagKeys": REMOVED_KEYS,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for {other:?}'s ARN, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
        let untouched = kind.tags(&emulator).await;
        assert_eq!(
            untouched,
            want_tags(TAGS),
            "case {case}: expected the refusal to remove nothing, got {untouched:?}"
        );

        // The resource's OWN ARN beside its name is served.
        let own_arn = kind.arn(&emulator).await;
        let (status, answered) = emulator
            .call(
                "UntagResource",
                json!({
                    "resourceName": kind.name(),
                    "resourceArn": own_arn,
                    "tagKeys": REMOVED_KEYS,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected its own ARN to be served, got {status} {answered:?}"
        );
        let left = kind.tags(&emulator).await;
        assert_eq!(
            left,
            want_tags(KEPT_TAGS),
            "case {case}: expected the listed keys gone, got {left:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A name no resource of the account holds
// ---------------------------------------------------------------------------

/// RULE: a name this account holds no resource of is refused with Lightsail's
/// `DoesNotExist` envelope — including a name that merely LOOKS like one of
/// these kinds.
#[tokio::test]
async fn a_name_no_resource_of_these_kinds_holds_is_refused() {
    let emulator = Emulator::new();
    create_sources(&emulator).await;
    Kind::Certificate.create(&emulator, TAGS).await;

    for (case, name) in [
        ("a certificate name nothing was created under", "absent-certificate"),
        ("a snapshot name nothing was created under", "absent-snapshot"),
    ] {
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({"resourceName": name, "tagKeys": ["env"]}),
            )
            .await;
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

    let kept = Kind::Certificate.tags(&emulator).await;
    assert_eq!(
        kept,
        want_tags(TAGS),
        "case refused removals: expected the certificate's tags untouched, got {kept:?}"
    );
}
