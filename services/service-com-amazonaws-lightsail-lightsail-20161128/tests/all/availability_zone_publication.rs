// SPDX-License-Identifier: CC0-1.0

//! The Availability Zone a create request names, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are exercised rather than bypassed.
//!
//! The rule under test: an Availability Zone a create request names must be one
//! a Region this service SERVES publishes — the member documents `GetRegions` as
//! where a caller gets it ("Use the `get regions` operation to list the
//! Availability Zones where Lightsail is currently available") — so
//!
//! * every zone that catalog publishes is a place a resource is created in, and
//!   the stored `ResourceLocation` is that zone and its Region; and
//! * a zone the catalog does not publish — a letter the Region skips, a letter
//!   past the ones it has, a Region `GetRegions` does not list, or a string that
//!   is no zone name at all — is refused with the modelled
//!   `InvalidInputException` and creates nothing.
//!
//! Both halves are asserted for EVERY operation that creates a resource in a
//! caller-named zone, because they all resolve the zone through the one catalog;
//! the zone catalog itself is read out of `GetRegions` rather than copied into
//! this suite, so these cases pin the rule and not a second copy of the table.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// A blueprint id this service publishes (`GetBlueprints`).
const BLUEPRINT_ID: &str = "amazon_linux_2023";

/// A bundle id this service publishes (`GetBundles`).
const BUNDLE_ID: &str = "nano_3_0";

/// A managed-database blueprint and bundle this service publishes
/// (`GetRelationalDatabaseBlueprints`, `GetRelationalDatabaseBundles`).
const DATABASE_BLUEPRINT_ID: &str = "mysql_8_4";
/// See [`DATABASE_BLUEPRINT_ID`].
const DATABASE_BUNDLE_ID: &str = "micro_1_0";

/// A password drawn only from characters the modelled `masterUserPassword`
/// documentation admits (printable ASCII, no space and none of `/`, `"`, `@`).
const MASTER_USER_PASSWORD: &str = "Pw9#x!ZqPw9#x!Zq";

/// The zone every source a restore in this suite reads lives in, and so the
/// Region each restore has to stay inside: a snapshot is restorable only in its
/// own Region, which is a separate rule from the one under test here.
const SOURCE_ZONE: &str = "us-east-1a";

/// The size of the source disk, in GB; a restore of its snapshot may be no
/// smaller.
const SOURCE_DISK_SIZE_IN_GB: i64 = 32;

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, in `region` when one is given, answering
    /// its HTTP status and parsed body.
    async fn call_in(&self, region: Option<&str>, operation: &str, body: Value) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261026/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_in(None, operation, body).await
    }

    /// Call `operation` in `region` and require success, answering the body.
    async fn ok_in(&self, region: Option<&str>, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed, got {status} {parsed:?}"
        );
        parsed
    }

    /// Call `operation` and require success, answering the body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        self.ok_in(None, operation, body).await
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The zones `GetRegions` publishes, keyed by the Region that publishes them.
///
/// A Lightsail endpoint is regional and reports the Availability Zones of its
/// OWN Region, so the catalog is assembled by asking each Region it lists for
/// its own zones.
async fn published_catalog(emulator: &Emulator) -> BTreeMap<String, Vec<String>> {
    let listed = emulator.ok("GetRegions", json!({})).await;
    let names: Vec<String> = listed["regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the catalog: expected an array, got {listed:?}"))
        .iter()
        .map(|region| {
            region["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case reading the catalog: a region has no name"))
                .to_string()
        })
        .collect();
    assert!(
        names.len() > 1,
        "case reading the catalog: expected several served regions, got {names:?}"
    );

    let mut catalog = BTreeMap::new();
    for name in names {
        let body = emulator
            .ok_in(
                Some(&name),
                "GetRegions",
                json!({"includeAvailabilityZones": true}),
            )
            .await;
        let entry = body["regions"]
            .as_array()
            .unwrap_or_else(|| panic!("case the zones of {name}: expected an array, got {body:?}"))
            .iter()
            .find(|region| region["name"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("case the zones of {name}: it reports no entry of its own"))
            .clone();
        let zones: Vec<String> = entry["availabilityZones"]
            .as_array()
            .unwrap_or_else(|| panic!("case the zones of {name}: expected an array, got {entry:?}"))
            .iter()
            .filter_map(|zone| zone["zoneName"].as_str().map(str::to_string))
            .collect();
        assert!(
            !zones.is_empty(),
            "case the zones of {name}: expected the request's own region to publish its zones, \
             got {entry:?}"
        );
        catalog.insert(name, zones);
    }
    catalog
}

/// Every zone name the catalog publishes, across every Region.
async fn every_published_zone(emulator: &Emulator) -> BTreeSet<String> {
    published_catalog(emulator)
        .await
        .into_values()
        .flatten()
        .collect()
}

// ---------------------------------------------------------------------------
// The operations that create a resource in a zone the caller names
// ---------------------------------------------------------------------------

/// One operation that creates a resource in an Availability Zone the request
/// names.
///
/// The rows are every such operation this service has: a resource created from
/// scratch and one restored from a snapshot, for an instance, for a block
/// storage disk and for a managed database, plus the Amazon EC2 stack built from
/// an export snapshot record — the one whose zone arrives in a list member.
struct Target {
    case: &'static str,
    operation: &'static str,
    /// The member path the refusal must name, as the request carries it.
    member: &'static str,
    /// The name the created resource takes.
    name: &'static str,
    /// The operation that reports the stored resource, the member its identifier
    /// arrives in, and the member of the response carrying the record. `None` for
    /// the stack, whose record name the service mints, so its records are read
    /// as a list.
    reader: Option<(&'static str, &'static str, &'static str)>,
    /// A zone of the sources' own Region that this operation must accept. Each
    /// row takes a different letter, so acceptance is not pinned to one zone.
    accepted_zone: &'static str,
}

const TARGETS: &[Target] = &[
    Target {
        case: "an instance",
        operation: "CreateInstances",
        member: "availabilityZone",
        name: "zoned-instance",
        reader: Some(("GetInstance", "instanceName", "instance")),
        accepted_zone: "us-east-1b",
    },
    Target {
        case: "an instance restored from a snapshot",
        operation: "CreateInstancesFromSnapshot",
        member: "availabilityZone",
        name: "zoned-restored-instance",
        reader: Some(("GetInstance", "instanceName", "instance")),
        accepted_zone: "us-east-1c",
    },
    Target {
        case: "a block storage disk",
        operation: "CreateDisk",
        member: "availabilityZone",
        name: "zoned-disk",
        reader: Some(("GetDisk", "diskName", "disk")),
        accepted_zone: "us-east-1d",
    },
    Target {
        case: "a disk restored from a snapshot",
        operation: "CreateDiskFromSnapshot",
        member: "availabilityZone",
        name: "zoned-restored-disk",
        reader: Some(("GetDisk", "diskName", "disk")),
        accepted_zone: "us-east-1e",
    },
    Target {
        case: "an Amazon EC2 stack",
        operation: "CreateCloudFormationStack",
        member: "instances[].availabilityZone",
        name: "zoned-stack",
        reader: None,
        accepted_zone: "us-east-1f",
    },
    Target {
        case: "a managed database",
        operation: "CreateRelationalDatabase",
        member: "availabilityZone",
        name: "zoned-database",
        reader: Some((
            "GetRelationalDatabase",
            "relationalDatabaseName",
            "relationalDatabase",
        )),
        accepted_zone: "us-east-1c",
    },
    Target {
        case: "a managed database restored from a snapshot",
        operation: "CreateRelationalDatabaseFromSnapshot",
        member: "availabilityZone",
        name: "zoned-restored-database",
        reader: Some((
            "GetRelationalDatabase",
            "relationalDatabaseName",
            "relationalDatabase",
        )),
        accepted_zone: "us-east-1d",
    },
];

/// The settled sources the restores and the stack read: one instance snapshot,
/// one disk snapshot, one managed-database snapshot and one export snapshot
/// record, every one of them in the Region of [`SOURCE_ZONE`], so the only thing
/// that can refuse a create below is the zone it names.
struct Sources {
    instance_snapshot: String,
    disk_snapshot: String,
    database_snapshot: String,
    export_record: String,
}

/// The request `target` makes to create its resource under `name` in `zone`.
fn create_body(target: &Target, sources: &Sources, zone: &str, name: &str) -> Value {
    match target.operation {
        "CreateInstances" => json!({
            "instanceNames": [name],
            "availabilityZone": zone,
            "blueprintId": BLUEPRINT_ID,
            "bundleId": BUNDLE_ID,
        }),
        "CreateInstancesFromSnapshot" => json!({
            "instanceNames": [name],
            "instanceSnapshotName": sources.instance_snapshot,
            "availabilityZone": zone,
            "bundleId": BUNDLE_ID,
        }),
        "CreateDisk" => json!({
            "diskName": name,
            "availabilityZone": zone,
            "sizeInGb": SOURCE_DISK_SIZE_IN_GB,
        }),
        "CreateDiskFromSnapshot" => json!({
            "diskName": name,
            "diskSnapshotName": sources.disk_snapshot,
            "availabilityZone": zone,
            "sizeInGb": SOURCE_DISK_SIZE_IN_GB,
        }),
        "CreateCloudFormationStack" => json!({
            "instances": [{
                "sourceName": sources.export_record,
                "instanceType": "t2.micro",
                "portInfoSource": "DEFAULT",
                "availabilityZone": zone,
            }],
        }),
        "CreateRelationalDatabase" => json!({
            "relationalDatabaseName": name,
            "relationalDatabaseBlueprintId": DATABASE_BLUEPRINT_ID,
            "relationalDatabaseBundleId": DATABASE_BUNDLE_ID,
            "masterDatabaseName": "appdb",
            "masterUsername": "dbadmin",
            "masterUserPassword": MASTER_USER_PASSWORD,
            "availabilityZone": zone,
        }),
        "CreateRelationalDatabaseFromSnapshot" => json!({
            "relationalDatabaseName": name,
            "relationalDatabaseSnapshotName": sources.database_snapshot,
            "availabilityZone": zone,
        }),
        other => panic!("case building a request for {other}: this suite knows no such create"),
    }
}

/// The stack records this account holds, as `GetCloudFormationStackRecords`
/// reports them.
async fn stack_records(emulator: &Emulator) -> Vec<Value> {
    let body = emulator
        .ok("GetCloudFormationStackRecords", json!({}))
        .await;
    body["cloudFormationStackRecords"]
        .as_array()
        .unwrap_or_else(|| panic!("case listing stack records: expected an array, got {body:?}"))
        .clone()
}

/// The record the store holds for what a `target` call created under `name`, and
/// `None` when the store holds nothing for it.
///
/// A name that was never created is refused with this service's `DoesNotExist`
/// envelope, which is what "nothing was stored" looks like to a caller.
async fn stored(emulator: &Emulator, target: &Target, name: &str) -> Option<Value> {
    match target.reader {
        Some((operation, member, record)) => {
            let (status, body) = emulator.call(operation, json!({member: name})).await;
            if status == 200 {
                return Some(body[record].clone());
            }
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case reading {name} back: expected either the record or the DoesNotExist \
                 envelope, got {status} {body:?}"
            );
            None
        }
        None => stack_records(emulator).await.into_iter().next_back(),
    }
}

/// Stand up every source the restores and the stack read, settled into the state
/// each is usable from.
async fn settled_sources(emulator: &Emulator) -> Sources {
    // The source instance, and the snapshot taken of it once it is serving.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["zone-source-instance"],
                "availabilityZone": SOURCE_ZONE,
                "blueprintId": BLUEPRINT_ID,
                "bundleId": BUNDLE_ID,
            }),
        )
        .await;
    wait_for(
        emulator,
        "GetInstance",
        "instanceName",
        "zone-source-instance",
        "instance",
        "running",
    )
    .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "zone-source-instance",
                "instanceSnapshotName": "zone-source-instance-snapshot",
            }),
        )
        .await;
    wait_for(
        emulator,
        "GetInstanceSnapshot",
        "instanceSnapshotName",
        "zone-source-instance-snapshot",
        "instanceSnapshot",
        "available",
    )
    .await;

    // The source disk and its snapshot, which the export reads too.
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "zone-source-disk",
                "availabilityZone": SOURCE_ZONE,
                "sizeInGb": SOURCE_DISK_SIZE_IN_GB,
            }),
        )
        .await;
    wait_for(
        emulator,
        "GetDisk",
        "diskName",
        "zone-source-disk",
        "disk",
        "available",
    )
    .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({
                "diskName": "zone-source-disk",
                "diskSnapshotName": "zone-source-disk-snapshot",
            }),
        )
        .await;
    wait_for(
        emulator,
        "GetDiskSnapshot",
        "diskSnapshotName",
        "zone-source-disk-snapshot",
        "diskSnapshot",
        "completed",
    )
    .await;

    // The source database and its snapshot.
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "zone-source-database",
                "relationalDatabaseBlueprintId": DATABASE_BLUEPRINT_ID,
                "relationalDatabaseBundleId": DATABASE_BUNDLE_ID,
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
                "masterUserPassword": MASTER_USER_PASSWORD,
                "availabilityZone": SOURCE_ZONE,
            }),
        )
        .await;
    wait_for(
        emulator,
        "GetRelationalDatabase",
        "relationalDatabaseName",
        "zone-source-database",
        "relationalDatabase",
        "available",
    )
    .await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "zone-source-database",
                "relationalDatabaseSnapshotName": "zone-source-database-snapshot",
            }),
        )
        .await;
    wait_for(
        emulator,
        "GetRelationalDatabaseSnapshot",
        "relationalDatabaseSnapshotName",
        "zone-source-database-snapshot",
        "relationalDatabaseSnapshot",
        "available",
    )
    .await;

    // The export snapshot record a stack is built from, once the export has
    // succeeded: a stack is built from the export of an INSTANCE snapshot.
    emulator
        .ok(
            "ExportSnapshot",
            json!({"sourceSnapshotName": "zone-source-instance-snapshot"}),
        )
        .await;
    let export_record = succeeded_export_record(emulator).await;

    Sources {
        instance_snapshot: "zone-source-instance-snapshot".to_string(),
        disk_snapshot: "zone-source-disk-snapshot".to_string(),
        database_snapshot: "zone-source-database-snapshot".to_string(),
        export_record,
    }
}

/// Poll the resource `name` through `reader` until it reports `want`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for(
    emulator: &Emulator,
    reader: &str,
    member: &str,
    name: &str,
    record: &str,
    want: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let body = emulator.ok(reader, json!({member: name})).await;
        let state = body[record]["state"].clone();
        // An instance reports its state as a `name`/`code` pair; every other
        // record here reports a bare string.
        let reported = state
            .as_str()
            .or_else(|| state["name"].as_str())
            .unwrap_or_default()
            .to_string();
        if reported == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {name} to be {want}: expected it within 15s, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The name of the one export snapshot record this account holds, once the clock
/// has settled it into `Succeeded` — the state a stack can be created from.
async fn succeeded_export_record(emulator: &Emulator) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let body = emulator.ok("GetExportSnapshotRecords", json!({})).await;
        let records = body["exportSnapshotRecords"]
            .as_array()
            .unwrap_or_else(|| panic!("case reading export records: expected an array, got {body:?}"))
            .clone();
        assert_eq!(
            records.len(),
            1,
            "case reading export records: expected exactly the one export this suite made, got \
             {records:?}"
        );
        if records[0]["state"].as_str() == Some("Succeeded") {
            return records[0]["name"]
                .as_str()
                .expect("an export snapshot record carries a name")
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the export to succeed: expected it within 15s, got {:?}",
            records[0]["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The zones the catalog does not publish
// ---------------------------------------------------------------------------

/// One `availabilityZone` no served Region publishes, and what makes it one.
///
/// The rows differ along every form the rule generalises over: a letter past the
/// ones a Region has (in a six-zone, a four-zone and a two-zone Region), a
/// letter a Region SKIPS, a Region the modelled enum admits but `GetRegions`
/// does not list, and strings that are no `<region><letter>` zone name at all.
/// Each is checked against the published catalog before it is sent, so a row can
/// never silently become a zone the service serves.
struct Refused {
    case: &'static str,
    zone: &'static str,
}

const REFUSED_ZONES: &[Refused] = &[
    Refused {
        case: "a letter past the six zones of a six-zone region",
        zone: "us-east-1z",
    },
    Refused {
        case: "a letter a region skips",
        zone: "ap-northeast-1b",
    },
    Refused {
        case: "a letter past the four zones of a four-zone region",
        zone: "us-west-2e",
    },
    Refused {
        case: "a third letter in a two-zone region",
        zone: "ca-central-1c",
    },
    Refused {
        case: "a zone of a region the modelled enum admits and GetRegions does not list",
        zone: "us-west-1a",
    },
    Refused {
        case: "a region name with no zone letter",
        zone: "us-east-1",
    },
    Refused {
        case: "an uppercase zone letter",
        zone: "us-east-1A",
    },
    Refused {
        case: "two zone letters",
        zone: "us-east-1ab",
    },
    Refused {
        case: "a region name nothing matches",
        zone: "xx-east-9a",
    },
    Refused {
        case: "a bare word",
        zone: "anywhere",
    },
];

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every Availability Zone `GetRegions` publishes is a place a resource is
/// created in, and the created resource is stored in exactly that zone and that
/// zone's Region.
#[tokio::test]
async fn every_zone_the_catalog_publishes_is_a_place_a_resource_is_created_in() {
    let emulator = Emulator::new();
    let catalog = published_catalog(&emulator).await;

    for (region, zones) in &catalog {
        for zone in zones {
            let case = format!("the published zone {zone}");

            let instance_name = format!("host-{zone}");
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [&instance_name],
                        "availabilityZone": zone,
                        "blueprintId": BLUEPRINT_ID,
                        "bundleId": BUNDLE_ID,
                    }),
                )
                .await;
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": &instance_name}))
                .await["instance"]
                .clone();
            assert_eq!(
                (
                    instance["location"]["availabilityZone"].as_str(),
                    instance["location"]["regionName"].as_str(),
                ),
                (Some(zone.as_str()), Some(region.as_str())),
                "case {case}: expected the instance stored in its own zone and that zone's \
                 region, got {instance:?}"
            );
            assert!(
                instance["arn"]
                    .as_str()
                    .is_some_and(|arn| arn.contains(&format!(":{region}:"))),
                "case {case}: expected the instance ARN to carry the zone's region, got \
                 {instance:?}"
            );

            let disk_name = format!("disk-{zone}");
            emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": &disk_name,
                        "availabilityZone": zone,
                        "sizeInGb": SOURCE_DISK_SIZE_IN_GB,
                    }),
                )
                .await;
            let disk = emulator
                .ok("GetDisk", json!({"diskName": &disk_name}))
                .await["disk"]
                .clone();
            assert_eq!(
                (
                    disk["location"]["availabilityZone"].as_str(),
                    disk["location"]["regionName"].as_str(),
                ),
                (Some(zone.as_str()), Some(region.as_str())),
                "case {case}: expected the disk stored in its own zone and that zone's region, \
                 got {disk:?}"
            );
        }
    }
}

/// RULE: a zone no served Region publishes is refused by EVERY operation that
/// creates a resource in a caller-named zone — with the modelled
/// `InvalidInputException` (400) naming the member the zone arrived in, and
/// storing nothing — while a zone the catalog publishes is accepted by each of
/// them and stored as the created resource's location.
#[tokio::test]
async fn no_create_takes_a_zone_the_catalog_does_not_publish() {
    let emulator = Emulator::new();
    let published = every_published_zone(&emulator).await;
    let sources = settled_sources(&emulator).await;

    for row in REFUSED_ZONES {
        assert!(
            !published.contains(row.zone),
            "case {}: {} must not be a zone GetRegions publishes for this suite to be about \
             anything",
            row.case,
            row.zone
        );
    }

    for target in TARGETS {
        // Every refused zone, on every create, leaves the store as it was.
        for (index, row) in REFUSED_ZONES.iter().enumerate() {
            let case = format!("{} in {} ({})", target.case, row.zone, row.case);
            let name = format!("{}-{index}", target.name);
            let (status, body) = emulator
                .call(
                    target.operation,
                    create_body(target, &sources, row.zone, &name),
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
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(target.member) && message.contains(row.zone),
                "case {case}: expected the refusal to name {} and the zone it refused, got \
                 {body:?}",
                target.member
            );
            assert!(
                stored(&emulator, target, &name).await.is_none(),
                "case {case}: a refused create must store nothing"
            );
        }

        // The same request, in a zone of the sources' own region that the
        // catalog DOES publish, creates the resource there.
        let zone = target.accepted_zone;
        let case = format!("{} in the published zone {zone}", target.case);
        assert!(
            published.contains(zone),
            "case {case}: {zone} must be a zone GetRegions publishes for this case to be about \
             anything"
        );
        let name = format!("{}-accepted", target.name);
        emulator
            .ok_in(
                None,
                target.operation,
                create_body(target, &sources, zone, &name),
            )
            .await;
        let record = stored(&emulator, target, &name)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected the resource to be stored"));
        assert_eq!(
            record["location"]["availabilityZone"].as_str(),
            Some(zone),
            "case {case}: expected the resource stored in the zone the request named, got \
             {record:?}"
        );
    }
}
