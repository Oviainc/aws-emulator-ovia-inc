// SPDX-License-Identifier: CC0-1.0

//! The ONE page walk every Lightsail listing of stored records shares
//! (`crate::state::resource_page`), driven over the wire through every listing
//! that used to walk its own records.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson envelope are all exercised rather than bypassed. The
//! `Authorization` header carries only a credential SCOPE, which is what names the
//! Region a request is made in.
//!
//! The rule under test, the one every row implements: a page of a listing is the
//! records that listing reports from the one `pageToken` NAMES onward in the
//! store's own total order by name, at most the page size of them, and
//! `nextPageToken` names the FIRST record after the page — absent once none
//! remain. A token is only ever answered by the record it names, so one naming no
//! record the listing reports is refused with the modelled
//! `InvalidInputException`. The walk over a listing's pages is the shared checked
//! one every listing suite in this target uses (`paging.rs::walk_pages`), wherever
//! it can read the records a listing reports.
//!
//! The rows span the listings that rule holds for: a listing of records keyed by
//! name alone, one of records whose identity includes the Region they were created
//! in, one keyed by name alone whose own documentation scopes it to the Region the
//! request is made in, one whose request NARROWS which records it reports, one that
//! reports names rather than records, and one whose record names the service mints
//! itself. Each
//! row's store holds one record past a page — the smallest store that makes the
//! walk issue a token — and each is walked under a TABLE of tokens a caller can
//! send: none, empty, below every name, on a name, just past a name, on the last
//! name, and past every name. What each is answered with is computed from the
//! records the listing holds rather than written out, so one table serves every
//! listing whatever its page size and however its records are named.
//!
//! The static catalogs share a walk of their own (`crate::state::catalog_page`),
//! whose token must NAME an entry; the last case here drives that one through the
//! database blueprint catalog, which used to walk its entries itself.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::Tag;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, StoredKeyPair, StoredLocation,
};

use crate::instance_lifecycle::{error_code, Emulator};
use crate::paging::{expected_page_sizes, names, walk_pages};
use crate::snapshot_copy::{wait_for_snapshot_state, Kind};

/// A walk that collects the pages `crate::paging::walk_pages` cannot read cannot
/// need more pages than this: anything past it means the walk is not advancing.
const MAX_PAGES: usize = 20;

/// The Availability Zone every resource this suite creates without naming a
/// Region is created in — one of the default Region's.
const ZONE: &str = "us-east-1a";

/// The Region the rows whose records are REGIONAL are created and read in, chosen
/// away from the default so a listing that ignored the credential scope would
/// report nothing.
const REGIONAL_ROW_REGION: &str = "us-west-2";

/// The Region a regional row's listing is read in a second time: one holding none
/// of its records.
const OTHER_REGION: &str = "eu-central-1";

/// The instance the alarms, the instance snapshots and the distributions of a row
/// are taken of or pulled from: not what any case here is about.
const ORIGIN_INSTANCE: &str = "pw-origin-instance";

/// The disk a row's disk snapshots — and the snapshot a row's exports are made
/// from — are taken of.
const SOURCE_DISK: &str = "pw-source-disk";

/// The snapshot a row's export snapshot records are made from.
const SOURCE_SNAPSHOT: &str = "pw-source-snapshot";

/// The instance the snapshot a row's CloudFormation stack records are made from
/// is taken of: a stack is built from the export of an INSTANCE snapshot.
const STACK_SOURCE_INSTANCE: &str = "pw-stack-source-instance";

/// That instance snapshot.
const STACK_SOURCE_SNAPSHOT: &str = "pw-stack-source-snapshot";

/// The managed database a row's database snapshots are taken of.
const SOURCE_DATABASE: &str = "pw-source-database";

/// How long a clock-owed transition this suite waits for may take.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// The listings
// ---------------------------------------------------------------------------

/// One listing whose records this suite stocks, and so one of the copies the
/// shared page walk replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    Instances,
    Disks,
    DiskSnapshots,
    InstanceSnapshots,
    Domains,
    StaticIps,
    KeyPairs,
    Alarms,
    Certificates,
    Distributions,
    Buckets,
    LoadBalancers,
    RelationalDatabases,
    RelationalDatabaseSnapshots,
    ActiveNames,
    ExportSnapshotRecords,
    CloudFormationStackRecords,
}

/// One row: the listing, the operation that answers it, the output member that
/// carries the page, the member of a reported record that carries the name a page
/// token names, the page size the handler uses, and the Region the listing is
/// scoped to when it is scoped to one.
#[derive(Debug, Clone, Copy)]
struct Row {
    listing: Listing,
    operation: &'static str,
    member: &'static str,
    /// `None` where a reported record IS the name — `GetActiveNames` reports the
    /// names the account holds, not records.
    name_member: Option<&'static str>,
    page_size: usize,
    region: Option<&'static str>,
}

/// Every listing that used to carry its own copy of the page walk. The page sizes
/// are the handlers' OWN constants, so a row keeps straddling the page boundary
/// whatever they are set to.
const ROWS: &[Row] = &[
    Row {
        listing: Listing::Instances,
        operation: "GetInstances",
        member: "instances",
        name_member: Some("name"),
        page_size: state::INSTANCE_PAGE_SIZE,
        region: None,
    },
    // `GetDisks` reports "all block storage disks in your AWS account and
    // region", so this row's records are created in — and the listing read in —
    // one Region away from the default.
    Row {
        listing: Listing::Disks,
        operation: "GetDisks",
        member: "disks",
        name_member: Some("name"),
        page_size: state::DISK_PAGE_SIZE,
        region: Some(REGIONAL_ROW_REGION),
    },
    Row {
        listing: Listing::DiskSnapshots,
        operation: "GetDiskSnapshots",
        member: "diskSnapshots",
        name_member: Some("name"),
        page_size: state::DISK_SNAPSHOT_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::InstanceSnapshots,
        operation: "GetInstanceSnapshots",
        member: "instanceSnapshots",
        name_member: Some("name"),
        page_size: state::INSTANCE_SNAPSHOT_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::Domains,
        operation: "GetDomains",
        member: "domains",
        name_member: Some("name"),
        page_size: state::DOMAIN_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::StaticIps,
        operation: "GetStaticIps",
        member: "staticIps",
        name_member: Some("name"),
        page_size: state::STATIC_IP_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::KeyPairs,
        operation: "GetKeyPairs",
        member: "keyPairs",
        name_member: Some("name"),
        page_size: state::KEY_PAIR_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::Alarms,
        operation: "GetAlarms",
        member: "alarms",
        name_member: Some("name"),
        page_size: state::ALARM_PAGE_SIZE,
        region: None,
    },
    // A certificate names itself through `certificateName`, not `name`: the page
    // walk is the same one, and what a token NAMES is this listing's own.
    Row {
        listing: Listing::Certificates,
        operation: "GetCertificates",
        member: "certificates",
        name_member: Some("certificateName"),
        page_size: state::CERTIFICATE_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::Distributions,
        operation: "GetDistributions",
        member: "distributions",
        name_member: Some("name"),
        page_size: state::DISTRIBUTION_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::Buckets,
        operation: "GetBuckets",
        member: "buckets",
        name_member: Some("name"),
        page_size: state::BUCKET_PAGE_SIZE,
        region: Some(REGIONAL_ROW_REGION),
    },
    Row {
        listing: Listing::LoadBalancers,
        operation: "GetLoadBalancers",
        member: "loadBalancers",
        name_member: Some("name"),
        page_size: state::LOAD_BALANCER_PAGE_SIZE,
        region: Some(REGIONAL_ROW_REGION),
    },
    Row {
        listing: Listing::RelationalDatabases,
        operation: "GetRelationalDatabases",
        member: "relationalDatabases",
        name_member: Some("name"),
        page_size: state::RELATIONAL_DATABASE_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::RelationalDatabaseSnapshots,
        operation: "GetRelationalDatabaseSnapshots",
        member: "relationalDatabaseSnapshots",
        name_member: Some("name"),
        page_size: state::RELATIONAL_DATABASE_SNAPSHOT_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::ActiveNames,
        operation: "GetActiveNames",
        member: "activeNames",
        name_member: None,
        page_size: state::ACTIVE_NAME_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::ExportSnapshotRecords,
        operation: "GetExportSnapshotRecords",
        member: "exportSnapshotRecords",
        name_member: Some("name"),
        page_size: state::EXPORT_SNAPSHOT_RECORD_PAGE_SIZE,
        region: None,
    },
    Row {
        listing: Listing::CloudFormationStackRecords,
        operation: "GetCloudFormationStackRecords",
        member: "cloudFormationStackRecords",
        name_member: Some("name"),
        page_size: state::CLOUD_FORMATION_STACK_RECORD_PAGE_SIZE,
        region: None,
    },
];

/// The name this suite gives `listing`'s record `index`. The index is
/// zero-padded, so name order is index order — and the records are created in the
/// reverse of it.
fn name_of(listing: Listing, index: usize) -> String {
    match listing {
        // A DNS zone is the one kind named by a dotted `DomainName`.
        Listing::Domains => format!("pw-zone-{index:02}.example"),
        Listing::Instances => format!("pw-instance-{index:02}"),
        Listing::Disks => format!("pw-disk-{index:02}"),
        Listing::DiskSnapshots => format!("pw-disk-snapshot-{index:02}"),
        Listing::InstanceSnapshots => format!("pw-instance-snapshot-{index:02}"),
        Listing::StaticIps => format!("pw-static-ip-{index:02}"),
        Listing::KeyPairs => format!("pw-key-pair-{index:02}"),
        Listing::Alarms => format!("pw-alarm-{index:02}"),
        Listing::Certificates => format!("pw-cert-{index:02}"),
        Listing::Distributions => format!("pw-distribution-{index:02}"),
        Listing::Buckets => format!("pw-bucket-{index:02}"),
        Listing::LoadBalancers => format!("pw-balancer-{index:02}"),
        Listing::RelationalDatabases => format!("pw-database-{index:02}"),
        Listing::RelationalDatabaseSnapshots => format!("pw-database-snapshot-{index:02}"),
        // The names this listing reports are the static IPs' own.
        Listing::ActiveNames => format!("pw-active-ip-{index:02}"),
        // The service mints the name of an export snapshot record and of a
        // CloudFormation stack record, so this suite chooses none.
        Listing::ExportSnapshotRecords | Listing::CloudFormationStackRecords => {
            format!("pw-minted-{index:02}")
        }
    }
}

// ---------------------------------------------------------------------------
// Stocking a row's store
// ---------------------------------------------------------------------------

/// Create the instance the alarms, instance snapshots and distributions of a row
/// are taken of or pulled from.
async fn create_instance(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;
}

/// Create the block storage disk a row's snapshots are taken of, in `zone`.
///
/// A disk's Region is the one the zone it was created in belongs to, never the
/// one the request was signed for, so a row whose listing is read in a Region
/// names a zone OF that Region.
async fn create_disk_in(emulator: &Emulator, name: &str, zone: &str) {
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": zone, "sizeInGb": 8}),
        )
        .await;
}

/// Create the block storage disk a row's snapshots are taken of, in the default
/// Region's zone every resource of this suite that names no Region uses.
async fn create_disk(emulator: &Emulator, name: &str) {
    create_disk_in(emulator, name, ZONE).await;
}

/// The first Availability Zone the service's own Region catalog publishes for
/// `region` — so no zone of a Region other than the default one is hand-written
/// here.
fn first_zone_of(region: &str) -> String {
    state::REGION_SPECS
        .iter()
        .find(|spec| spec.name == region)
        .unwrap_or_else(|| panic!("case stocking a row in {region}: expected the Region catalog to publish it"))
        .zone_names()
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case stocking a row in {region}: expected {region} to publish a zone"))
}

/// Create the disk snapshot a row's exports are made from, and wait
/// until the service has finished taking it — which is when it can be exported
/// (the refusal before that is pinned in `export_snapshot_completeness.rs`).
async fn create_source_snapshot(emulator: &Emulator) {
    create_disk(emulator, SOURCE_DISK).await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": SOURCE_DISK, "diskSnapshotName": SOURCE_SNAPSHOT}),
        )
        .await;
    wait_for_snapshot_state(
        emulator,
        Kind::DiskSnapshot,
        SOURCE_SNAPSHOT,
        Kind::DiskSnapshot.settled(),
    )
    .await;
}

/// Create the instance snapshot a row's stacks are made from, and wait until the
/// service has finished taking it — which is when it can be exported.
async fn create_stack_source_snapshot(emulator: &Emulator) {
    create_instance(emulator, STACK_SOURCE_INSTANCE).await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": STACK_SOURCE_INSTANCE,
                "instanceSnapshotName": STACK_SOURCE_SNAPSHOT,
            }),
        )
        .await;
    wait_for_snapshot_state(
        emulator,
        Kind::InstanceSnapshot,
        STACK_SOURCE_SNAPSHOT,
        Kind::InstanceSnapshot.settled(),
    )
    .await;
}

/// Every record `operation` reports, walking its pages without asserting
/// anything: how a row reads back the names the service minted for it.
async fn every_record(emulator: &Emulator, operation: &str, member: &str) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let body = emulator.ok(operation, request).await;
        let page = body[member]
            .as_array()
            .unwrap_or_else(|| panic!("case reading {operation}: expected an array, got {body:?}"))
            .clone();
        all.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return all,
        }
    }
    panic!("case reading {operation}: the walk did not terminate within {MAX_PAGES} pages");
}

/// Wait until every record `operation` reports is in `want`: the state the clock
/// owes them, which arrives without anyone asking more than once.
async fn wait_until_all_in_state(emulator: &Emulator, operation: &str, member: &str, want: &str) {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let records = every_record(emulator, operation, member).await;
        let pending: Vec<&Value> = records
            .iter()
            .filter(|record| record["state"].as_str() != Some(want))
            .collect();
        if pending.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for every {member} record to reach {want}: expected it within \
             {SETTLE_TIMEOUT:?}, got {pending:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Put `count` key pairs in the store with the ids and instants this walk needs,
/// which a create does not let a caller pick — exactly as the `key_pair_listing`
/// suite stocks its own listing.
fn seed_key_pairs(emulator: &Emulator, names: &[String]) {
    let seeded = state::with_state(emulator.engine(), |store| {
        // Seeded in REVERSE name order, like every other row's records.
        for (index, name) in names.iter().enumerate().rev() {
            let id = format!("05859e3d-331d-48ba-9034-{:012x}", index + 1);
            store.insert_key_pair(StoredKeyPair {
                name: name.clone(),
                arn: state::key_pair_arn(state::DEFAULT_REGION, &id),
                support_code: state::support_code(&id),
                created_at_ms: 1_700_000_000_000 + (index as i64) * 97_531,
                location: StoredLocation::regional(state::DEFAULT_REGION),
                tags: vec![Tag {
                    key: Some("suite".to_string()),
                    value: Some("page-walk".to_string()),
                }],
                fingerprint: format!("SHA256:hXy0pZ9Qw3kLmNoPqRsTuVwXyZaBcDeFgH{index:03}"),
            });
        }
        store.key_pairs.len()
    });
    assert_eq!(
        seeded,
        Some(names.len()),
        "case seeding {} key pairs: expected the store to hold them all",
        names.len()
    );
}

/// Stock `row`'s listing with `count` records, answering the names they carry — or
/// `None` where the service mints them and the listing itself is what reports
/// them.
///
/// Every row's records are created in the REVERSE of their name order, so a
/// listing that echoed the order they were created in rather than the store's own
/// order would fail.
async fn stock(emulator: &Emulator, row: &Row, count: usize) -> Option<Vec<String>> {
    let names: Vec<String> = (0..count).map(|index| name_of(row.listing, index)).collect();
    match row.listing {
        Listing::Instances => {
            for name in names.iter().rev() {
                create_instance(emulator, name).await;
            }
        }
        Listing::Disks => {
            let zone = match row.region {
                Some(region) => first_zone_of(region),
                None => ZONE.to_string(),
            };
            for name in names.iter().rev() {
                create_disk_in(emulator, name, &zone).await;
            }
        }
        Listing::DiskSnapshots => {
            create_disk(emulator, SOURCE_DISK).await;
            for name in names.iter().rev() {
                emulator
                    .ok(
                        "CreateDiskSnapshot",
                        json!({"diskName": SOURCE_DISK, "diskSnapshotName": name}),
                    )
                    .await;
            }
        }
        Listing::InstanceSnapshots => {
            create_instance(emulator, ORIGIN_INSTANCE).await;
            for name in names.iter().rev() {
                emulator
                    .ok(
                        "CreateInstanceSnapshot",
                        json!({
                            "instanceName": ORIGIN_INSTANCE,
                            "instanceSnapshotName": name,
                        }),
                    )
                    .await;
            }
        }
        Listing::Domains => {
            for name in names.iter().rev() {
                emulator.ok("CreateDomain", json!({"domainName": name})).await;
            }
        }
        Listing::StaticIps | Listing::ActiveNames => {
            for name in names.iter().rev() {
                emulator
                    .ok("AllocateStaticIp", json!({"staticIpName": name}))
                    .await;
            }
        }
        Listing::KeyPairs => seed_key_pairs(emulator, &names),
        Listing::Alarms => {
            create_instance(emulator, ORIGIN_INSTANCE).await;
            // One metric per alarm: an instance accepts only a couple of alarms
            // on any one of its metrics, so neighbouring records differ in the
            // metric they watch as well as in their name.
            let metrics = instance_metric_names();
            assert!(
                metrics.len() >= count,
                "case stocking the alarms: expected the metric catalog to hold one metric per \
                 alarm, got {metrics:?}"
            );
            for (index, name) in names.iter().enumerate().rev() {
                emulator
                    .ok(
                        "PutAlarm",
                        json!({
                            "alarmName": name,
                            "monitoredResourceName": ORIGIN_INSTANCE,
                            "metricName": metrics[index],
                            "comparisonOperator": "GreaterThanThreshold",
                            "threshold": 80,
                            "evaluationPeriods": 2,
                        }),
                    )
                    .await;
            }
        }
        Listing::Certificates => {
            for name in names.iter().rev() {
                emulator
                    .ok(
                        "CreateCertificate",
                        json!({"certificateName": name, "domainName": format!("{name}.example")}),
                    )
                    .await;
            }
        }
        Listing::Distributions => {
            create_instance(emulator, ORIGIN_INSTANCE).await;
            for name in names.iter().rev() {
                emulator
                    .ok(
                        "CreateDistribution",
                        json!({
                            "distributionName": name,
                            "origin": {"name": ORIGIN_INSTANCE},
                            "defaultCacheBehavior": {"behavior": "cache"},
                            "bundleId": "small_1_0",
                        }),
                    )
                    .await;
            }
        }
        Listing::Buckets => {
            for name in names.iter().rev() {
                emulator
                    .ok_at(
                        None,
                        row.region,
                        "CreateBucket",
                        json!({"bucketName": name, "bundleId": "small_1_0"}),
                    )
                    .await;
            }
        }
        Listing::LoadBalancers => {
            for name in names.iter().rev() {
                emulator
                    .ok_at(
                        None,
                        row.region,
                        "CreateLoadBalancer",
                        json!({"loadBalancerName": name, "instancePort": 80}),
                    )
                    .await;
            }
        }
        Listing::RelationalDatabases => {
            for name in names.iter().rev() {
                create_database(emulator, name).await;
            }
        }
        Listing::RelationalDatabaseSnapshots => {
            create_database(emulator, SOURCE_DATABASE).await;
            for name in names.iter().rev() {
                emulator
                    .ok(
                        "CreateRelationalDatabaseSnapshot",
                        json!({
                            "relationalDatabaseName": SOURCE_DATABASE,
                            "relationalDatabaseSnapshotName": name,
                        }),
                    )
                    .await;
            }
        }
        Listing::ExportSnapshotRecords => {
            create_source_snapshot(emulator).await;
            for _ in 0..count {
                emulator
                    .ok(
                        "ExportSnapshot",
                        json!({"sourceSnapshotName": SOURCE_SNAPSHOT}),
                    )
                    .await;
            }
            return None;
        }
        Listing::CloudFormationStackRecords => {
            create_stack_source_snapshot(emulator).await;
            // A stack is built from an export the clock has settled into
            // `Succeeded`, and a second stack from one export waits for the first
            // — so each stack here is built from an export of its own.
            for _ in 0..count {
                emulator
                    .ok(
                        "ExportSnapshot",
                        json!({"sourceSnapshotName": STACK_SOURCE_SNAPSHOT}),
                    )
                    .await;
            }
            wait_until_all_in_state(
                emulator,
                "GetExportSnapshotRecords",
                "exportSnapshotRecords",
                "Succeeded",
            )
            .await;
            let exports = every_record(
                emulator,
                "GetExportSnapshotRecords",
                "exportSnapshotRecords",
            )
            .await;
            assert_eq!(
                exports.len(),
                count,
                "case stocking the stack records: expected one export per stack, got {exports:?}"
            );
            for export in &exports {
                let source = export["name"].as_str().unwrap_or_default();
                emulator
                    .ok(
                        "CreateCloudFormationStack",
                        json!({"instances": [{
                            "sourceName": source,
                            "instanceType": "t2.micro",
                            "portInfoSource": "DEFAULT",
                            "availabilityZone": ZONE,
                        }]}),
                    )
                    .await;
            }
            return None;
        }
    }
    Some(names)
}

/// The metrics an alarm can watch on an instance, from the service's OWN catalog
/// (`state::ALARM_METRICS`), so the alarm row hand-writes no metric list.
fn instance_metric_names() -> Vec<&'static str> {
    state::ALARM_METRICS
        .iter()
        .filter(|spec| spec.resource_type == state::RESOURCE_TYPE_INSTANCE)
        .map(|spec| spec.name)
        .collect()
}

/// Create the managed relational database `name`.
async fn create_database(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "availabilityZone": ZONE,
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
            }),
        )
        .await;
}

// ---------------------------------------------------------------------------
// Reading one listing's pages
// ---------------------------------------------------------------------------

/// The name a reported record carries — the one a page token names.
fn name_in(row: &Row, record: &Value, case: &str) -> String {
    let name = match row.name_member {
        Some(member) => record[member].as_str(),
        None => record.as_str(),
    };
    name.unwrap_or_else(|| {
        panic!("case {case}: expected every reported record to carry its name, got {record:?}")
    })
    .to_string()
}

/// One page of `row`'s listing, in the Region its records belong to, resuming from
/// `token` when one is given: the HTTP status, the names it reported in order, and
/// the token it named for the page after it.
async fn page_result(
    emulator: &Emulator,
    row: &Row,
    case: &str,
    token: Option<&str>,
) -> (u16, Vec<String>, Option<String>, Value) {
    let request = match token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    let (status, body) = emulator
        .call_in(row.region, row.operation, request)
        .await;
    if status != 200 {
        return (status, Vec::new(), None, body);
    }
    let records = body[row.member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a {} array, got {body:?}",
                row.member
            )
        })
        .clone();
    let names = records
        .iter()
        .map(|record| name_in(row, record, case))
        .collect();
    let next = body["nextPageToken"].as_str().map(str::to_string);
    (status, names, next, body)
}

/// One page of `row`'s listing, requiring success: the names it reported in order,
/// and the token it named for the page after it.
async fn page(
    emulator: &Emulator,
    row: &Row,
    case: &str,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (status, names, next, body) = page_result(emulator, row, case, token).await;
    assert_eq!(
        status, 200,
        "case {case}: expected the listing to answer a page, got {status} {body:?}"
    );
    (names, next)
}

/// Every page of `row`'s listing: the names it reported in page order, and the
/// size of each page.
///
/// A listing whose records name themselves through `name` goes through the shared
/// checked walk every listing suite in this target uses
/// (`crate::paging::walk_pages`), which is where the paging contract is asserted
/// as the walk goes. The two listings that walk cannot read — a certificate names
/// itself through `certificateName`, and `GetActiveNames` reports the names
/// themselves rather than records — collect their pages here, and get the same
/// contract from the token table below, which computes the page the rule requires
/// for EVERY token, the issued ones included.
async fn walk(emulator: &Emulator, row: &Row, case: &str) -> (Vec<String>, Vec<usize>) {
    if row.name_member == Some("name") {
        let (records, page_sizes) = walk_pages(
            emulator,
            case,
            row.region,
            row.operation,
            row.member,
            row.page_size,
        )
        .await;
        return (names(&records), page_sizes);
    }

    let mut reported: Vec<String> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let (page_names, next) = page(emulator, row, case, token.as_deref()).await;
        page_sizes.push(page_names.len());
        reported.extend(page_names.iter().cloned());
        match next {
            Some(next) => token = Some(next),
            None => return (reported, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {MAX_PAGES} pages");
}

/// The page the rule requires for `token` over the records `held`, in the store's
/// own order: the records from the one it names onward, at most `page_size` of
/// them, and the name of the first record after that page. `None` where the token
/// names no record of `held`, which the listing refuses.
fn expected_page(
    held: &[String],
    token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<String>, Option<String>)> {
    let from: Vec<String> = match token.filter(|token| !token.is_empty()) {
        None => held.to_vec(),
        Some(token) => {
            let start = held.iter().position(|name| name.as_str() == token)?;
            held[start..].to_vec()
        }
    };
    let page: Vec<String> = from.iter().take(page_size).cloned().collect();
    Some((page, from.get(page_size).cloned()))
}

/// The tokens a caller can send against the records `held`: none at all, the empty
/// token, one below every name, one that IS a name, one just past a name, the last
/// name, and one past every name. The middle rows are derived from `held`, so the
/// table spans the same positions whatever the records are called.
fn token_cases(held: &[String]) -> Vec<(String, Option<String>)> {
    let inside = held[held.len() / 2].clone();
    let last = held[held.len() - 1].clone();
    vec![
        ("no token at all".to_string(), None),
        ("an empty token".to_string(), Some(String::new())),
        // `!` is the lowest printable character, so it sorts below every name a
        // Lightsail resource can carry.
        (
            "a token below every name".to_string(),
            Some("!".to_string()),
        ),
        ("a token that is the first name".to_string(), Some(held[0].clone())),
        ("a token that is a name inside the listing".to_string(), Some(inside.clone())),
        (
            "a token just past a name inside the listing".to_string(),
            Some(format!("{inside}~")),
        ),
        ("a token that is the last name".to_string(), Some(last.clone())),
        (
            "a token past every name".to_string(),
            Some(format!("{last}~")),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: every listing of stored records answers a page of them from the token
/// onward in the store's own total order, at most a page size at a time, naming
/// the first record after the page — one walk, driven through each of the listings
/// that used to carry a copy of it.
#[tokio::test(flavor = "current_thread")]
async fn every_resource_listing_pages_through_the_one_shared_walk() {
    for row in ROWS {
        // One record past a page: the smallest store that makes the walk issue a
        // token, so every row crosses a page boundary.
        let count = row.page_size + 1;
        let case = row.operation;
        let emulator = Emulator::new();
        let stocked = stock(&emulator, row, count).await;

        let (held, page_sizes) = walk(&emulator, row, case).await;
        assert_eq!(
            held.len(),
            count,
            "case {case}: expected the {count} records this suite stocked, got {held:?}"
        );
        assert_eq!(
            page_sizes,
            expected_page_sizes(count, row.page_size),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );
        let mut ordered = held.clone();
        ordered.sort();
        assert_eq!(
            held, ordered,
            "case {case}: expected the store's own total order by name, got {held:?}"
        );
        let distinct: BTreeSet<&String> = held.iter().collect();
        assert_eq!(
            distinct.len(),
            held.len(),
            "case {case}: expected every record once, got {held:?}"
        );
        if let Some(created) = &stocked {
            let mut expected = created.clone();
            expected.sort();
            assert_eq!(
                held, expected,
                "case {case}: expected exactly the records this suite created, got {held:?}"
            );
        }

        // Every token a caller can send lands on the same rule: the page is the
        // records from the one the token NAMES onward, bounded by the page size,
        // the next page starts at the first record after it, and a token that
        // names no record of the listing is refused.
        for (token_case, token) in token_cases(&held) {
            let case = format!("{} with {token_case}", row.operation);
            match expected_page(&held, token.as_deref(), row.page_size) {
                Some((expected, expected_next)) => {
                    let (names, next) = page(&emulator, row, &case, token.as_deref()).await;
                    assert_eq!(
                        names, expected,
                        "case {case}: expected the records from the token onward, got {names:?}"
                    );
                    assert_eq!(
                        next, expected_next,
                        "case {case}: expected the next page to start at {expected_next:?}, got \
                         {next:?}"
                    );
                }
                None => {
                    let (status, _, _, body) =
                        page_result(&emulator, row, &case, token.as_deref()).await;
                    assert_eq!(
                        status, 400,
                        "case {case}: expected the invalid-input status, got {status} {body:?}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "InvalidInputException",
                        "case {case}: expected the modelled InvalidInputException, got {body:?}"
                    );
                    assert!(
                        body["message"].as_str().is_some_and(|message| {
                            message.contains(token.as_deref().unwrap_or_default())
                        }),
                        "case {case}: expected the refusal to name the token sent, got {body:?}"
                    );
                }
            }
        }

        // A listing scoped to a Region reports that Region's records alone:
        // whether the Region is part of the key or selected on the walk, a
        // caller in another Region is told it holds none of them.
        if row.region.is_some() {
            let elsewhere = Row {
                region: Some(OTHER_REGION),
                ..*row
            };
            let case = format!("{} in a Region holding none of them", row.operation);
            let (names, next) = page(&emulator, &elsewhere, &case, None).await;
            assert!(
                names.is_empty(),
                "case {case}: expected an empty list, got {names:?}"
            );
            assert_eq!(
                next, None,
                "case {case}: expected no next page token, got {next:?}"
            );
        }
    }
}

/// RULE: the static catalogs share a page walk of their own, over entries a token
/// must NAME — so a page is the catalog from the named entry onward, at most a
/// page size at a time, and a token naming no entry is refused rather than
/// resumed. Driven through the database blueprint catalog, which used to walk its
/// entries itself.
#[tokio::test(flavor = "current_thread")]
async fn the_blueprint_catalog_pages_through_the_shared_catalog_walk() {
    let emulator = Emulator::new();
    let page_size = state::RELATIONAL_DATABASE_BLUEPRINT_PAGE_SIZE;

    /// One page of the catalog: the blueprint ids it reported and the token it
    /// named for the page after it.
    async fn catalog_page(
        emulator: &Emulator,
        case: &str,
        token: Option<&str>,
    ) -> (Vec<String>, Option<String>) {
        let request = match token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let body = emulator
            .ok("GetRelationalDatabaseBlueprints", request)
            .await;
        let ids = body["blueprints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a blueprints array, got {body:?}"))
            .iter()
            .map(|blueprint| {
                blueprint["blueprintId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected every entry to carry its id, got {blueprint:?}")
                    })
                    .to_string()
            })
            .collect();
        (ids, body["nextPageToken"].as_str().map(str::to_string))
    }

    // The whole catalog, walked page by page from the tokens it hands out.
    let mut all: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for index in 0..MAX_PAGES {
        let case = format!("the catalog page {index}");
        let (ids, next) = catalog_page(&emulator, &case, token.as_deref()).await;
        assert!(
            ids.len() <= page_size,
            "case {case}: a page carries at most {page_size} entries, got {ids:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                ids.first().map(String::as_str),
                Some(token.as_str()),
                "case {case}: a page token names the first entry of the page after it, got {ids:?}"
            );
        }
        all.extend(ids.iter().cloned());
        match next {
            Some(next) => {
                assert_eq!(
                    ids.len(),
                    page_size,
                    "case {case}: a page that is not the last one is full, got {ids:?}"
                );
                token = Some(next);
            }
            None => break,
        }
    }
    let distinct: BTreeSet<&String> = all.iter().collect();
    assert_eq!(
        distinct.len(),
        all.len(),
        "case the catalog walk: expected every blueprint once, got {all:?}"
    );
    assert!(
        all.len() > 1,
        "case the catalog walk: expected the catalog to hold several blueprints, got {all:?}"
    );

    // A token that NAMES an entry resumes at it; an entry inside the catalog is
    // not only its first.
    for (index, id) in all.iter().enumerate() {
        let case = format!("a token naming the blueprint {id}");
        let (ids, next) = catalog_page(&emulator, &case, Some(id)).await;
        let expected: Vec<String> = all.iter().skip(index).take(page_size).cloned().collect();
        assert_eq!(
            ids, expected,
            "case {case}: expected the catalog from that entry onward, got {ids:?}"
        );
        assert_eq!(
            next,
            all.get(index + page_size).cloned(),
            "case {case}: expected the token of the page after it, got {next:?}"
        );
    }

    // A token that names no entry of the catalog leaves no page to report.
    for token in ["mysql_0_0", "not-a-blueprint", "postgres"] {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseBlueprints",
                json!({"pageToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case the token {token:?}, which names no blueprint: expected the invalid-input \
             status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case the token {token:?}, which names no blueprint: expected the \
             InvalidInputException wire error, got {body:?}"
        );
    }
}
