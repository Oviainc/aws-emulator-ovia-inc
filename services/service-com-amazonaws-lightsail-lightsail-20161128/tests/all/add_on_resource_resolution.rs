// SPDX-License-Identifier: CC0-1.0

//! Which of an account's resources an add-on-addressing Lightsail operation
//! admits, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which
//! is what names the Region a request is made in.
//!
//! The rule under test: an operation that addresses a resource THROUGH its
//! add-ons resolves the name it was given against the resources whose modelled
//! shape carries an `addOns` member — the virtual private server and the block
//! storage disk (`Instance$addOns` and `Disk$addOns`, the only two shapes with
//! that member) — and every such operation resolves it the same way. So a name
//! that is some OTHER kind of resource of this account, a name nothing was
//! created under, or a name whose resource has since been deleted is refused by
//! all of them alike, with one `DoesNotExist` envelope carrying one sentence,
//! and nothing is written; while a name that IS one of those two resolves, and
//! what the call then answers is about the call itself rather than about the
//! name.
//!
//! The rows span what the rule generalises over: every kind of resource this
//! account can hold whose name the caller chooses (the two that carry add-ons
//! and each kind that does not, including the snapshots taken OF the two that
//! do), and each of the four operations that address a resource through its
//! add-ons.

use serde_json::{json, Map, Value};

// The harness and the wire error code this test target already exposes: this
// suite drives the service through the same front door rather than standing up
// a second one.
use crate::instance_lifecycle::{error_code, Emulator};

/// The Region every case is made in: the one the domain, distribution and
/// certificate APIs are served from, so a single account can hold a resource of
/// every kind at once.
const REGION: &str = "us-east-1";

/// The zone every resource that names one is created in; an add-on is not
/// regional, so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint and a small plan: neither selects anything about an
/// add-on.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "micro_3_0";

/// The automatic snapshot add-on — the member of the modelled `AddOnType` enum
/// both kinds of add-on-carrying resource accept, so one add-on type serves
/// every case.
const AUTO_SNAPSHOT: &str = "AutoSnapshot";

/// The daily hour the cases enable that add-on at: an hourly increment, which is
/// what the modelled `AutoSnapshotAddOnRequest$snapshotTimeOfDay`
/// documentation requires ("Must be in `HH:00` format, and in an hourly
/// increment").
const SNAPSHOT_HOUR: &str = "07:00";

/// The date every `DeleteAutoSnapshot` case names: a day long past, so no
/// schedule of this account has taken a copy on it.
const DATE: &str = "2000-01-01";

/// The name of the one virtual private server this suite creates, which the
/// snapshot, alarm and distribution rows refer to as their source.
const INSTANCE: &str = "add-on-resolve-instance";
/// The name of the one block storage disk, which the disk snapshot row refers to.
const DISK: &str = "add-on-resolve-disk";
/// The name of the one database, which the database snapshot row refers to.
const DATABASE: &str = "add-on-resolve-database";

// ---------------------------------------------------------------------------
// The kinds of resource an account can hold
// ---------------------------------------------------------------------------

/// What an add-on-carrying kind needs beside its name: the `ResourceType` it
/// reports and the read that reports the add-ons it holds.
struct Carrier {
    /// The modelled `ResourceType` value the resource reports.
    resource_type: &'static str,
    /// That resource's own reader, the member it takes the name in, and the
    /// member of the answer the resource is carried in.
    reader: &'static str,
    name_member: &'static str,
    envelope: &'static str,
}

/// One kind of Lightsail resource whose name the caller chooses.
struct Kind {
    case: &'static str,
    name: &'static str,
    /// The call that creates one, the request it takes, and the HTTP status that
    /// call answers.
    create: (&'static str, Value, u16),
    /// `Some` for a kind whose modelled shape carries an `addOns` member, which
    /// is the virtual private server and the block storage disk alone.
    carries: Option<Carrier>,
}

/// Every kind, dependencies first: the server a snapshot is taken of, an alarm
/// monitors and a distribution originates at, the disk a disk snapshot is taken
/// of and the database a database snapshot is taken of are each created by an
/// earlier row.
///
/// A DNS zone is deliberately absent: it is named by a dotted `DomainName`,
/// which the modelled `ResourceName` pattern on every one of these requests does
/// not admit, so such a name never reaches the resolution this suite is about.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "a virtual private server",
            name: INSTANCE,
            create: (
                "CreateInstances",
                json!({
                    "instanceNames": [INSTANCE],
                    "availabilityZone": ZONE,
                    "blueprintId": BLUEPRINT,
                    "bundleId": BUNDLE,
                }),
                200,
            ),
            carries: Some(Carrier {
                resource_type: "Instance",
                reader: "GetInstance",
                name_member: "instanceName",
                envelope: "instance",
            }),
        },
        Kind {
            case: "a block storage disk",
            name: DISK,
            create: (
                "CreateDisk",
                json!({"diskName": DISK, "availabilityZone": ZONE, "sizeInGb": 8}),
                200,
            ),
            carries: Some(Carrier {
                resource_type: "Disk",
                reader: "GetDisk",
                name_member: "diskName",
                envelope: "disk",
            }),
        },
        Kind {
            case: "a database",
            name: DATABASE,
            create: (
                "CreateRelationalDatabase",
                json!({
                    "relationalDatabaseName": DATABASE,
                    "availabilityZone": ZONE,
                    "relationalDatabaseBlueprintId": "mysql_8_4",
                    "relationalDatabaseBundleId": "micro_1_0",
                    "masterDatabaseName": "appdb",
                    "masterUsername": "dbadmin",
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a snapshot of a block storage disk",
            name: "add-on-resolve-disk-snapshot",
            create: (
                "CreateDiskSnapshot",
                json!({
                    "diskName": DISK,
                    "diskSnapshotName": "add-on-resolve-disk-snapshot",
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a snapshot of a virtual private server",
            name: "add-on-resolve-instance-snapshot",
            create: (
                "CreateInstanceSnapshot",
                json!({
                    "instanceName": INSTANCE,
                    "instanceSnapshotName": "add-on-resolve-instance-snapshot",
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a snapshot of a database",
            name: "add-on-resolve-db-snapshot",
            create: (
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": DATABASE,
                    "relationalDatabaseSnapshotName": "add-on-resolve-db-snapshot",
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a static IP address",
            name: "add-on-resolve-static-ip",
            create: (
                "AllocateStaticIp",
                json!({"staticIpName": "add-on-resolve-static-ip"}),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "an alarm",
            name: "add-on-resolve-alarm",
            create: (
                "PutAlarm",
                json!({
                    "alarmName": "add-on-resolve-alarm",
                    "monitoredResourceName": INSTANCE,
                    "metricName": "CPUUtilization",
                    "comparisonOperator": "GreaterThanThreshold",
                    "threshold": 80,
                    "evaluationPeriods": 2,
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a CDN distribution",
            name: "add-on-resolve-distribution",
            create: (
                "CreateDistribution",
                json!({
                    "distributionName": "add-on-resolve-distribution",
                    "origin": {"name": INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a certificate",
            name: "add-on-resolve-certificate",
            create: (
                "CreateCertificate",
                json!({
                    "certificateName": "add-on-resolve-certificate",
                    "domainName": "add-on-resolve-cert.example",
                }),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a bucket",
            name: "add-on-resolve-bucket",
            create: (
                "CreateBucket",
                json!({"bucketName": "add-on-resolve-bucket", "bundleId": "small_1_0"}),
                200,
            ),
            carries: None,
        },
        Kind {
            case: "a container service",
            name: "add-on-resolve-containers",
            create: (
                "CreateContainerService",
                json!({"serviceName": "add-on-resolve-containers", "power": "nano", "scale": 2}),
                // `CreateContainerService` answers the modelled 201 for the
                // resource it created.
                201,
            ),
            carries: None,
        },
        Kind {
            case: "a load balancer",
            name: "add-on-resolve-balancer",
            create: (
                "CreateLoadBalancer",
                json!({"loadBalancerName": "add-on-resolve-balancer", "instancePort": 80}),
                200,
            ),
            carries: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Driving the operations that address a resource through its add-ons
// ---------------------------------------------------------------------------

/// The operations that resolve a name against this account's add-on-carrying
/// resources, and the request each one sends for `name`: the two that change a
/// resource's add-ons, and the two that address the copies the automatic
/// snapshot add-on takes (which are held on the same resources, "an Amazon
/// Lightsail instance or disk").
fn add_on_calls(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "EnableAddOn",
            json!({
                "resourceName": name,
                "addOnRequest": {
                    "addOnType": AUTO_SNAPSHOT,
                    "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": SNAPSHOT_HOUR},
                },
            }),
        ),
        (
            "DisableAddOn",
            json!({"resourceName": name, "addOnType": AUTO_SNAPSHOT}),
        ),
        ("GetAutoSnapshots", json!({"resourceName": name})),
        (
            "DeleteAutoSnapshot",
            json!({"resourceName": name, "date": DATE}),
        ),
    ]
}

/// Create one resource of every kind, answering the table so a case can ask each
/// row what it is.
async fn create_every_kind(emulator: &Emulator) -> Vec<Kind> {
    let table = kinds();
    for kind in &table {
        let (operation, request, want_status) = &kind.create;
        let (status, body) = emulator
            .call_at(None, Some(REGION), operation, request.clone())
            .await;
        assert_eq!(
            status, *want_status,
            "case {:?}: creating it with {operation} must answer HTTP {want_status}, got \
             {status} {body:?}",
            kind.case
        );
    }
    table
}

/// The add-ons the resource `name` of an add-on-carrying `carrier` reports, read
/// back through that resource's own reader.
async fn reported_add_ons(
    emulator: &Emulator,
    case: &str,
    name: &str,
    carrier: &Carrier,
) -> Vec<Value> {
    let mut request = Map::new();
    request.insert(carrier.name_member.to_string(), json!(name));
    let body = emulator
        .ok_at(None, Some(REGION), carrier.reader, Value::Object(request))
        .await;
    let resource = &body[carrier.envelope];
    assert_eq!(
        resource["resourceType"].as_str(),
        Some(carrier.resource_type),
        "case {case:?}: expected {} to report its own resource type, got {resource:?}",
        carrier.reader
    );
    resource["addOns"].as_array().cloned().unwrap_or_default()
}

/// Assert that every add-on-addressing operation refuses `name` with the service's
/// `DoesNotExist` envelope, and that they all refuse it with the SAME sentence —
/// one resolution rule, one refusal.
async fn assert_refused_by_every_add_on_operation(emulator: &Emulator, case: &str, name: &str) {
    let mut refusals: Vec<(&'static str, String)> = Vec::new();
    for (operation, request) in add_on_calls(name) {
        let (status, body) = emulator.call_at(None, Some(REGION), operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), body["code"].as_str()),
            (400, "NotFoundException", Some("DoesNotExist")),
            "case {case:?} through {operation}: expected the DoesNotExist envelope, got \
             {status} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains("resourceName") && message.contains(name),
            "case {case:?} through {operation}: expected the message to name the member the \
             caller sent and the name asked for, got {body:?}"
        );
        refusals.push((operation, message));
    }
    let (first_operation, first_message) = refusals
        .first()
        .expect("every case sends at least one add-on operation")
        .clone();
    for (operation, message) in &refusals[1..] {
        assert_eq!(
            message, &first_message,
            "case {case:?}: {operation} must refuse with the same sentence as \
             {first_operation}, got {message:?}"
        );
    }
}

/// Assert that a kind whose modelled shape carries an `addOns` member is SERVED
/// by the add-on operations: the add-on it is given is stored and read back, the
/// copies of it are reported rather than refused, disabling it removes it, and
/// the one refusal left is about the copy a request asked for rather than about
/// the name.
async fn assert_served_for_a_carrier(emulator: &Emulator, kind: &Kind, carrier: &Carrier) {
    let case = kind.case;
    let name = kind.name;

    // Nothing has been copied of it yet, and that is an empty list rather than a
    // refusal: the resource exists.
    let reported = emulator
        .ok_at(
            None,
            Some(REGION),
            "GetAutoSnapshots",
            json!({"resourceName": name}),
        )
        .await;
    assert_eq!(
        (
            reported["resourceName"].as_str(),
            reported["resourceType"].as_str()
        ),
        (Some(name), Some(carrier.resource_type)),
        "case {case:?}: expected the answer to name the resource and its kind, got {reported:?}"
    );
    assert_eq!(
        reported["autoSnapshots"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: a resource no copy has been taken of reports an empty list, got \
         {reported:?}"
    );

    // The add-on is stored on the resource the request named, with the hour it
    // carried, and every later read of that resource reports it.
    emulator
        .ok_at(
            None,
            Some(REGION),
            "EnableAddOn",
            json!({
                "resourceName": name,
                "addOnRequest": {
                    "addOnType": AUTO_SNAPSHOT,
                    "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": SNAPSHOT_HOUR},
                },
            }),
        )
        .await;
    let enabled = reported_add_ons(emulator, case, name, carrier).await;
    assert_eq!(
        enabled
            .iter()
            .map(|add_on| (add_on["name"].as_str(), add_on["snapshotTimeOfDay"].as_str()))
            .collect::<Vec<_>>(),
        vec![(Some(AUTO_SNAPSHOT), Some(SNAPSHOT_HOUR))],
        "case {case:?}: expected exactly the add-on the request described, got {enabled:?}"
    );

    // The name RESOLVED, so the one thing left to refuse is the copy the request
    // asked for: the refusal names the resource, its kind and the date, which the
    // refusal of a name that resolves to nothing cannot.
    let (status, body) = emulator
        .call_at(
            None,
            Some(REGION),
            "DeleteAutoSnapshot",
            json!({"resourceName": name, "date": DATE}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["code"].as_str()),
        (400, "NotFoundException", Some("DoesNotExist")),
        "case {case:?}: a date no copy was taken on is the DoesNotExist envelope, got \
         {status} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(carrier.resource_type) && message.contains(name) && message.contains(DATE),
        "case {case:?}: expected the refusal to be about the copy asked for, naming the \
         resource, its kind and the date, got {body:?}"
    );

    // Disabling it is served too, and removes it.
    emulator
        .ok_at(
            None,
            Some(REGION),
            "DisableAddOn",
            json!({"resourceName": name, "addOnType": AUTO_SNAPSHOT}),
        )
        .await;
    let disabled = reported_add_ons(emulator, case, name, carrier).await;
    assert!(
        disabled.is_empty(),
        "case {case:?}: expected the add-on to be gone, got {disabled:?}"
    );
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: of every kind of resource an account can hold, the add-on operations
/// admit exactly the two whose modelled shape carries an `addOns` member, and
/// refuse every other kind — the snapshots taken OF those two included — with
/// one `DoesNotExist` envelope and one sentence.
#[tokio::test]
async fn an_add_on_operation_admits_exactly_the_add_on_carrying_kinds() {
    let emulator = Emulator::new();
    let table = create_every_kind(&emulator).await;

    // A guard on the TABLE, not on the emulator: the two kinds marked as
    // carrying add-ons are the two shapes the model gives an `addOns` member
    // (`smithy_query(query="shape",shape="Instance,Disk")`), so a kind added to
    // the table above is classified deliberately rather than by default. The
    // per-kind loop below is what carries the rule.
    let carrying = table.iter().filter(|kind| kind.carries.is_some()).count();
    assert_eq!(
        carrying, 2,
        "case the table itself: the model gives an addOns member to two shapes, got {carrying}"
    );

    for kind in &table {
        match &kind.carries {
            Some(carrier) => assert_served_for_a_carrier(&emulator, kind, carrier).await,
            None => assert_refused_by_every_add_on_operation(&emulator, kind.case, kind.name).await,
        }
    }
}

/// RULE: a name this account holds no add-on-carrying resource of is refused the
/// same way whether it never named one or no longer does — and a refused request
/// writes nothing, so the name still names nothing afterwards.
#[tokio::test]
async fn an_add_on_operation_refuses_a_name_the_account_no_longer_holds() {
    let emulator = Emulator::new();

    // A resource that existed and does not any more.
    let gone = "add-on-resolve-gone-instance";
    emulator
        .ok_at(
            None,
            Some(REGION),
            "CreateInstances",
            json!({
                "instanceNames": [gone],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT,
                "bundleId": BUNDLE,
            }),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(REGION),
            "DeleteInstance",
            json!({"instanceName": gone}),
        )
        .await;

    let never = "add-on-resolve-never-created";
    for (case, name) in [
        ("a name nothing was ever created under", never),
        ("a virtual private server that has been deleted", gone),
    ] {
        assert_refused_by_every_add_on_operation(&emulator, case, name).await;

        // The refused requests wrote nothing: the name still names no resource of
        // either add-on-carrying kind.
        for (reader, member) in [("GetInstance", "instanceName"), ("GetDisk", "diskName")] {
            let mut request = Map::new();
            request.insert(member.to_string(), json!(name));
            let (status, body) = emulator
                .call_at(None, Some(REGION), reader, Value::Object(request))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "NotFoundException"),
                "case {case:?} through {reader}: a refused add-on request creates nothing, got \
                 {status} {body:?}"
            );
        }
    }
}
