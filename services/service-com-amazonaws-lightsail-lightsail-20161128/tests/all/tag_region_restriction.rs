// SPDX-License-Identifier: CC0-1.0

//! The Region restriction on a TAG change — `TagResource` and `UntagResource`
//! over the records Lightsail serves from one endpoint only — driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is `instance_lifecycle`'s and the tag helpers are
//! `resource_tags`'s, so this file stands up no second emulator and keeps no
//! second copy of the assertions; its `Authorization` header carries only the
//! credential SCOPE, which is what names the Region a request is made in.
//!
//! The rule under test: a record of a kind whose API Lightsail serves from the
//! N. Virginia (`us-east-1`) endpoint ALONE is reached only through that Region
//! WHICHEVER call reaches it, so a tag change that resolves to one — the
//! documentation covers creating, viewing and EDITING those records alike — is
//! refused in every other Region with the modelled `InvalidInputException` (HTTP
//! 400) carrying the documented restriction and writes nothing, while the same
//! call in that Region is served and round-trips through the record's own
//! reader. A record of any other kind addressed by its name alone is tagged from
//! any Region, because its own operations serve it from any Region too.
//!
//! `domain_region_restriction.rs` pins the same rule for the records' own
//! operations; this suite adds the two tagging doors onto them, and borrows that
//! suite's Region table and refusal text rather than restating either.

use serde_json::{json, Value};

use crate::domain_region_restriction::{other_regions, DOMAIN_REGION, REFUSAL};
use crate::instance_lifecycle::{error_code, Emulator};
use crate::resource_tags::{reported_tags, tag_list, want_tags, TagRow};

/// The tag set every record in this suite is created with.
const CREATED_TAGS: &[TagRow] = &[("env", Some("test")), ("retain", None)];

/// The tags a refused — or a served — tag change carries.
const ADDED_TAGS: &[TagRow] = &[("env", Some("prod")), ("owner", Some("platform"))];

/// The keys a removal names: one the record carries with a value, one it carries
/// as a key-only tag.
const REMOVED_KEYS: &[&str] = &["env", "retain"];

/// The instance the distribution originates at, and the resource the alarm
/// monitors. It carries no tag of its own, so every tag this suite observes
/// belongs to the record under test.
const SOURCE_INSTANCE: &str = "region-tag-origin";

/// A DNS zone of this account, whose dotted name no tagging request can carry
/// (see [`a_zone_name_never_reaches_the_tag_doors`]).
const ZONE: &str = "region-tag.example";

/// A kind of record a tag change can resolve.
///
/// The rows split along the one thing the rule turns on: whether the kind's API
/// is served from the domain Region alone (the CDN distribution and the SSL/TLS
/// certificate it is served with — the DNS zone is the third such kind, covered
/// by [`a_zone_name_never_reaches_the_tag_doors`]) or from every Region (a
/// virtual private server, a block storage disk, an alarm — each held under its
/// name alone, so a tag change in another Region resolves it).
#[derive(Clone, Copy, Debug)]
enum Kind {
    Certificate,
    Distribution,
    Instance,
    Disk,
    Alarm,
}

/// The kinds the restriction covers that a tagging request can name.
const RESTRICTED: &[Kind] = &[Kind::Certificate, Kind::Distribution];

/// The kinds it does not cover, each addressed by name alone.
const UNRESTRICTED: &[Kind] = &[Kind::Instance, Kind::Disk, Kind::Alarm];

impl Kind {
    /// The name this suite creates this kind's record under. Each satisfies both
    /// that resource's own modelled name constraint and the modelled
    /// `TagResource$resourceName` / `UntagResource$resourceName` pattern
    /// (`^\w[\w\-]*\w$`).
    fn name(self) -> &'static str {
        match self {
            Kind::Certificate => "region-tag-certificate",
            Kind::Distribution => "region-tag-distribution",
            Kind::Instance => "region-tag-instance",
            Kind::Disk => "region-tag-disk",
            Kind::Alarm => "region-tag-alarm",
        }
    }

    /// The create request this kind's record is made with, carrying `tags`.
    fn create_request(self, tags: &[TagRow]) -> (&'static str, Value) {
        let name = self.name();
        let (operation, mut body) = match self {
            Kind::Certificate => (
                "CreateCertificate",
                json!({"certificateName": name, "domainName": "region-tag.example"}),
            ),
            Kind::Distribution => (
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": SOURCE_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
            ),
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
            ),
        };
        if !tags.is_empty() {
            body["tags"] = tag_list(tags);
        }
        (operation, body)
    }

    /// Create this kind's record carrying `tags`, in the domain Region — the one
    /// Region every kind here can be created in.
    async fn create(self, emulator: &Emulator, tags: &[TagRow]) {
        let (operation, body) = self.create_request(tags);
        emulator
            .ok_at(None, Some(DOMAIN_REGION), operation, body)
            .await;
    }

    /// This kind's record as its own reader reports it, read in the domain
    /// Region, which serves every kind here.
    async fn read(self, emulator: &Emulator) -> Value {
        let name = self.name();
        let (operation, body, member) = match self {
            Kind::Certificate => (
                "GetCertificates",
                json!({"certificateName": name}),
                "certificates",
            ),
            Kind::Distribution => (
                "GetDistributions",
                json!({"distributionName": name}),
                "distributions",
            ),
            Kind::Instance => ("GetInstance", json!({"instanceName": name}), "instance"),
            Kind::Disk => ("GetDisk", json!({"diskName": name}), "disk"),
            Kind::Alarm => ("GetAlarms", json!({"alarmName": name}), "alarms"),
        };
        let answered = emulator
            .ok_at(None, Some(DOMAIN_REGION), operation, body)
            .await;
        match answered[member].as_array() {
            // A reader that answers a LIST was asked for the one record, so it
            // reports exactly that one.
            Some(entries) => {
                assert_eq!(
                    entries.len(),
                    1,
                    "case reading {name}: expected exactly the one named record in {member}, \
                     got {entries:?}"
                );
                entries[0].clone()
            }
            None => answered[member].clone(),
        }
    }

    /// The tags this kind's record reports now.
    async fn tags(self, emulator: &Emulator) -> Vec<(String, Option<String>)> {
        reported_tags(&self.read(emulator).await)
    }
}

/// One tag change, as the pair of operations that make one: the request body
/// built for `name`, and the name of the call.
#[derive(Clone, Copy, Debug)]
enum Change {
    Add,
    Remove,
}

/// Both halves of a tag change, so every case is driven through each.
const CHANGES: &[Change] = &[Change::Add, Change::Remove];

impl Change {
    /// The operation this half of the tag change calls.
    fn operation(self) -> &'static str {
        match self {
            Change::Add => "TagResource",
            Change::Remove => "UntagResource",
        }
    }

    /// The request this half of the tag change carries for `name`.
    fn body(self, name: &str) -> Value {
        match self {
            Change::Add => json!({"resourceName": name, "tags": tag_list(ADDED_TAGS)}),
            Change::Remove => json!({"resourceName": name, "tagKeys": REMOVED_KEYS}),
        }
    }
}

/// Stand up an emulator holding one record of every kind, each created in the
/// domain Region carrying [`CREATED_TAGS`], plus the instance the distribution
/// originates at and the alarm monitors and a DNS zone.
async fn emulator_with_every_kind() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
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
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "CreateDomain",
            json!({"domainName": ZONE, "tags": tag_list(CREATED_TAGS)}),
        )
        .await;
    for kind in RESTRICTED.iter().chain(UNRESTRICTED) {
        kind.create(&emulator, CREATED_TAGS).await;
    }
    emulator
}

// ---------------------------------------------------------------------------
// The restricted kinds
// ---------------------------------------------------------------------------

/// RULE: a tag change that resolves to a record of a kind served from the domain
/// Region alone is refused in every other Region with the modelled
/// `InvalidInputException` (HTTP 400) carrying the documented restriction, and
/// moves no tag.
#[tokio::test]
async fn a_tag_change_on_a_domain_region_record_is_refused_outside_that_region() {
    let emulator = emulator_with_every_kind().await;

    for region in other_regions() {
        for kind in RESTRICTED {
            for change in CHANGES {
                let case = format!("{} of {kind:?} in {region}", change.operation());
                let (status, body) = emulator
                    .call_in(
                        Some(region),
                        change.operation(),
                        change.body(kind.name()),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case}: expected a 400 refusal, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInputException",
                    "case {case}: expected the modelled InvalidInputException, got {body:?}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(REFUSAL),
                    "case {case}: expected the documented restriction, got {:?}",
                    body["message"]
                );

                let left = kind.tags(&emulator).await;
                assert_eq!(
                    left,
                    want_tags(CREATED_TAGS),
                    "case {case}: expected the refusal to move no tag, got {left:?}"
                );
            }
        }
    }
}

/// RULE: the same two calls ARE served for those records through the domain
/// Region — and through a request carrying no credential scope at all, which the
/// service serves in its default Region, the domain Region — so the tag they
/// write, and the tag they remove, round-trip through the record's own reader.
#[tokio::test]
async fn a_tag_change_on_a_domain_region_record_is_served_in_that_region() {
    // The two ways a request names the domain Region: the scope says so, or
    // there is no scope to say otherwise.
    for (scope, region) in [
        ("the us-east-1 credential scope", Some(DOMAIN_REGION)),
        ("no credential scope at all", None),
    ] {
        let emulator = emulator_with_every_kind().await;
        for kind in RESTRICTED {
            let case = format!("{kind:?} with {scope}");

            let answered = emulator
                .ok_at(None, region, "TagResource", Change::Add.body(kind.name()))
                .await;
            let operations = answered["operations"].as_array().unwrap_or_else(|| {
                panic!("case {case}: expected an operations array, got {answered:?}")
            });
            assert_eq!(
                operations.len(),
                1,
                "case {case}: expected one entry for the one record named, got {operations:?}"
            );

            // The merge: the key the record already carried takes the new value
            // in place, the new key is appended, and the untouched tag stays.
            let merged = kind.tags(&emulator).await;
            assert_eq!(
                merged,
                want_tags(&[
                    ("env", Some("prod")),
                    ("retain", None),
                    ("owner", Some("platform")),
                ]),
                "case {case}: expected the request's tags merged into the record, got {merged:?}"
            );

            emulator
                .ok_at(
                    None,
                    region,
                    "UntagResource",
                    Change::Remove.body(kind.name()),
                )
                .await;
            let left = kind.tags(&emulator).await;
            assert_eq!(
                left,
                want_tags(&[("owner", Some("platform"))]),
                "case {case}: expected only the unlisted tag to survive, got {left:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The kinds the restriction does not cover
// ---------------------------------------------------------------------------

/// RULE: a record of a kind whose own operations serve it from any Region is
/// tagged from any Region too — the restriction narrows exactly the kinds served
/// from the domain Region alone and nothing else.
#[tokio::test]
async fn a_tag_change_on_every_other_kind_is_served_in_every_region() {
    for region in other_regions() {
        let emulator = emulator_with_every_kind().await;
        for kind in UNRESTRICTED {
            let case = format!("{kind:?} in {region}");

            let (status, body) = emulator
                .call_in(Some(region), "TagResource", Change::Add.body(kind.name()))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected the tag write to be served, got {status} {body:?}"
            );
            let merged = kind.tags(&emulator).await;
            assert_eq!(
                merged,
                want_tags(&[
                    ("env", Some("prod")),
                    ("retain", None),
                    ("owner", Some("platform")),
                ]),
                "case {case}: expected the request's tags merged into the record, got {merged:?}"
            );

            let (status, body) = emulator
                .call_in(
                    Some(region),
                    "UntagResource",
                    Change::Remove.body(kind.name()),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected the tag removal to be served, got {status} {body:?}"
            );
            let left = kind.tags(&emulator).await;
            assert_eq!(
                left,
                want_tags(&[("owner", Some("platform"))]),
                "case {case}: expected only the unlisted tag to survive, got {left:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The third restricted kind, which no tagging request can name
// ---------------------------------------------------------------------------

/// RULE: the DNS zone is the third kind the restriction covers, and no tagging
/// request reaches one at all: a zone is named by a dotted `DomainName`, which
/// the modelled `TagResource$resourceName` / `UntagResource$resourceName`
/// pattern (`^\w[\w\-]*\w$`) does not admit, so the centralized constraint walk
/// refuses the request with its own `ValidationException` (HTTP 400) in every
/// Region — the domain Region included — and the zone's tags never move.
#[tokio::test]
async fn a_zone_name_never_reaches_the_tag_doors() {
    let emulator = emulator_with_every_kind().await;

    let mut regions: Vec<&str> = vec![DOMAIN_REGION];
    regions.extend(other_regions());
    for region in regions {
        for change in CHANGES {
            let case = format!("{} of the zone in {region}", change.operation());
            let (status, body) = emulator
                .call_in(Some(region), change.operation(), change.body(ZONE))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected the walk's refusal, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected the boundary walk's ValidationException, got {body:?}"
            );

            let zone = emulator
                .ok_at(
                    None,
                    Some(DOMAIN_REGION),
                    "GetDomain",
                    json!({"domainName": ZONE}),
                )
                .await["domain"]
                .clone();
            let left = reported_tags(&zone);
            assert_eq!(
                left,
                want_tags(CREATED_TAGS),
                "case {case}: expected the zone's tags untouched, got {left:?}"
            );
        }
    }
}
