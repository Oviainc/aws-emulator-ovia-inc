// SPDX-License-Identifier: CC0-1.0

//! The Lightsail block storage disk — create and read — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateDisk` persists ONE block storage disk per
//! `diskName` the request names — the name, size, zone, tags and add-ons the
//! request carried verbatim, every server-assigned member (ARN, support code,
//! creation time, location, resource type, IOPS, state) computed from the
//! request plus the store's own allocators — so a later `GetDisk` reports
//! exactly that record; the disk is created `pending` and the clock, not a
//! reader, settles it into `available`; a request outside what Lightsail creates
//! (a size outside the published range, a zone no Lightsail region has, a name
//! the account already holds, an add-on a disk cannot carry) is refused and
//! stores nothing; and a name that was never created is refused with the
//! service's `DoesNotExist` envelope.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The published size range of a Lightsail block storage disk, in GB: 8 GB to
/// 16 TB. The modelled `CreateDisk$sizeInGb` carries no `@range`, so these are
/// the bounds the service itself enforces.
const MIN_SIZE_IN_GB: i64 = 8;
/// See [`MIN_SIZE_IN_GB`].
const MAX_SIZE_IN_GB: i64 = 16 * 1024;

/// The IOPS baseline a Lightsail disk inherits from the EBS general purpose SSD
/// (`gp2`) volume behind it: 3 IOPS per GB, never below 100 and never above
/// 16,000.
const IOPS_PER_GB: i64 = 3;
/// See [`IOPS_PER_GB`].
const IOPS_FLOOR: i64 = 100;
/// See [`IOPS_PER_GB`].
const IOPS_CEILING: i64 = 16_000;

/// The IOPS a disk of `size_in_gb` must report, from that same baseline.
fn want_iops(size_in_gb: i64) -> i64 {
    (size_in_gb * IOPS_PER_GB).clamp(IOPS_FLOOR, IOPS_CEILING)
}

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
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

    /// Call `operation` and require success, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// The stored disk `name`, as `GetDisk` reports it.
    async fn disk(&self, name: &str) -> Value {
        self.ok("GetDisk", json!({"diskName": name})).await["disk"].clone()
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

/// Whether `value` has the shape of the UUID Lightsail's resource and operation
/// ids take: five hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let lengths: Vec<usize> = value.split('-').map(str::len).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// Whether `value` satisfies the modelled `TimeOfDay` pattern
/// `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$`.
fn is_modelled_time_of_day(value: &str) -> bool {
    let (hour, minute) = match value.split_once(':') {
        Some(split) => split,
        None => return false,
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u32>().is_ok_and(|hour| hour <= 23)
        && minute.parse::<u32>().is_ok_and(|minute| minute <= 59)
}

/// Poll `GetDisk` until it reports `want`, answering the disk.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let disk = emulator.disk(name).await;
        if disk["state"].as_str() == Some(want) {
            return disk;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            disk["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateDisk` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the Availability
/// Zone (and so the region in the ARN and the location), the size — including
/// each published bound, the size that drives the IOPS floor and the one that
/// drives its ceiling — the number of tags, and whether the request carries an
/// add-on (with and without its own `snapshotTimeOfDay`).
struct Row {
    case: &'static str,
    disk_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    size_in_gb: i64,
    tags: &'static [(&'static str, &'static str)],
    /// `Some(time)` asks for the `AutoSnapshot` add-on with that
    /// `snapshotTimeOfDay`; `Some("")` asks for it without one, so the service
    /// picks the hour; `None` asks for no add-on at all.
    auto_snapshot_at: Option<&'static str>,
}

/// The table. The issue's own case — a 32 GB disk in one zone, no tags, no
/// add-ons — is the first row.
const ROWS: &[Row] = &[
    Row {
        case: "the documented 32 GB example, no tags, no add-ons",
        disk_name: "my-disk",
        availability_zone: "us-east-2a",
        region: "us-east-2",
        size_in_gb: 32,
        tags: &[],
        auto_snapshot_at: None,
    },
    Row {
        case: "the smallest published size, in another region's zone, one tag",
        disk_name: "small-disk-8",
        availability_zone: "eu-west-1c",
        region: "eu-west-1",
        size_in_gb: MIN_SIZE_IN_GB,
        tags: &[("tier", "edge")],
        auto_snapshot_at: None,
    },
    Row {
        case: "a size above the IOPS floor, two tags, an add-on with its own hour",
        disk_name: "data-disk-512",
        availability_zone: "ap-southeast-2b",
        region: "ap-southeast-2",
        size_in_gb: 512,
        tags: &[("stack", "prod"), ("owner", "storage-team")],
        auto_snapshot_at: Some("06:00"),
    },
    Row {
        case: "a size the service picks the snapshot hour for",
        disk_name: "mid_disk_2048",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        size_in_gb: 2048,
        tags: &[],
        auto_snapshot_at: Some(""),
    },
    Row {
        case: "the largest published size, where the IOPS ceiling binds",
        disk_name: "big-disk-16tb",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        size_in_gb: MAX_SIZE_IN_GB,
        tags: &[("keep", "yes")],
        auto_snapshot_at: None,
    },
];

/// The `CreateDisk` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "diskName": row.disk_name,
        "availabilityZone": row.availability_zone,
        "sizeInGb": row.size_in_gb,
    });
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    match row.auto_snapshot_at {
        None => {}
        Some("") => body["addOns"] = json!([{"addOnType": "AutoSnapshot"}]),
        Some(at) => {
            body["addOns"] = json!([{
                "addOnType": "AutoSnapshot",
                "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": at},
            }]);
        }
    }
    body
}

/// RULE: every member `GetDisk` reports comes from the one record `CreateDisk`
/// stored — the caller's name, zone, size, tags and add-ons verbatim, and every
/// server-assigned member computed from them plus the store's allocators.
#[tokio::test]
async fn a_disk_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let name = row.disk_name;

        // The create answers ONE operation-log entry — `CreateDisk` creates one
        // disk — and it is not terminal yet, because the disk is still being
        // provisioned when the call returns.
        let created = emulator.ok("CreateDisk", create_body(row)).await;
        let operations = created["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {created:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateDisk"),
            "case {case}: expected the CreateDisk operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Disk"),
            "case {case}: expected the Disk resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the disk, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: expected a Started operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected a non-terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the operation to be located in the requested zone, got \
             {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used \
             {operation_id:?}"
        );

        // The stored record, read back through the implemented reader.
        let disk = emulator.disk(name).await;
        assert_eq!(
            disk["name"].as_str(),
            Some(name),
            "case {case}: expected the disk name the request carried, got {disk:?}"
        );
        assert_eq!(
            disk["resourceType"].as_str(),
            Some("Disk"),
            "case {case}: expected the Disk resource type, got {disk:?}"
        );
        assert_eq!(
            disk["sizeInGb"].as_i64(),
            Some(row.size_in_gb),
            "case {case}: expected the size the request carried, got {disk:?}"
        );
        assert_eq!(
            disk["iops"].as_i64(),
            Some(want_iops(row.size_in_gb)),
            "case {case}: expected the gp2 baseline IOPS for {} GB, got {:?}",
            row.size_in_gb,
            disk["iops"]
        );
        assert_eq!(
            disk["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the requested zone, got {:?}",
            disk["location"]
        );
        assert_eq!(
            disk["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the zone's own region, got {:?}",
            disk["location"]
        );
        assert_eq!(
            disk["isSystemDisk"].as_bool(),
            Some(false),
            "case {case}: a created disk is additional block storage, not a system disk, \
             got {disk:?}"
        );
        assert_eq!(
            disk["isAttached"].as_bool(),
            Some(false),
            "case {case}: a new disk is attached to nothing, got {disk:?}"
        );
        assert!(
            disk["attachedTo"].is_null(),
            "case {case}: a new disk is attached to nothing, got {:?}",
            disk["attachedTo"]
        );
        assert_eq!(
            disk["attachmentState"].as_str(),
            Some("detached"),
            "case {case}: expected the discontinued attachmentState to mirror isAttached, \
             got {disk:?}"
        );
        assert!(
            disk["path"].is_null(),
            "case {case}: nothing has attached the disk, so no path is mounted, got {:?}",
            disk["path"]
        );
        assert!(
            disk["gbInUse"].is_null(),
            "case {case}: gbInUse is discontinued, so it must not be reported, got {:?}",
            disk["gbInUse"]
        );
        assert!(
            disk["autoMountStatus"].is_null(),
            "case {case}: autoMountStatus applies only to Lightsail for Research, got {:?}",
            disk["autoMountStatus"]
        );

        // The tags the request carried, verbatim and in order.
        let reported_tags: Vec<(String, String)> = disk["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {disk:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, want_tags,
            "case {case}: expected the tags the request carried, got {:?}",
            disk["tags"]
        );

        // The add-ons the request asked for: enabled, with the hour the request
        // named or — where it named none — one in the modelled `TimeOfDay` form.
        let add_ons = disk["addOns"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an addOns array, got {disk:?}"))
            .clone();
        match row.auto_snapshot_at {
            None => assert!(
                add_ons.is_empty(),
                "case {case}: expected no add-ons, got {add_ons:?}"
            ),
            Some(at) => {
                assert_eq!(
                    add_ons.len(),
                    1,
                    "case {case}: expected the one requested add-on, got {add_ons:?}"
                );
                assert_eq!(
                    add_ons[0]["name"].as_str(),
                    Some("AutoSnapshot"),
                    "case {case}: expected the AutoSnapshot add-on, got {add_ons:?}"
                );
                assert_eq!(
                    add_ons[0]["status"].as_str(),
                    Some("Enabled"),
                    "case {case}: expected the add-on enabled, got {add_ons:?}"
                );
                let reported_at = add_ons[0]["snapshotTimeOfDay"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if at.is_empty() {
                    assert!(
                        is_modelled_time_of_day(&reported_at),
                        "case {case}: expected a modelled TimeOfDay the service picked, got \
                         {reported_at:?}"
                    );
                } else {
                    assert_eq!(
                        reported_at, at,
                        "case {case}: expected the hour the request named, got \
                         {reported_at:?}"
                    );
                }
            }
        }

        // The ARN carries the zone's region and the canonical emulator account,
        // and ends in a uuid no other disk is using; the support code names the
        // same id.
        let arn = disk["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Disk/", row.region);
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            disk["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            disk["supportCode"]
        );
        assert!(
            disk["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            disk["createdAt"]
        );

        // The disk is one of the modelled `DiskState` values throughout, and it
        // is never attached by a read.
        let state = disk["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "pending" || state == "available",
            "case {case}: expected a pending or available disk, got {state:?}"
        );
    }
}

/// RULE: each created disk is its own record, so creating one never disturbs
/// another and every one of them is still readable afterwards with its own size.
#[tokio::test]
async fn each_disk_is_stored_independently() {
    let emulator = Emulator::new();
    for row in ROWS {
        emulator.ok("CreateDisk", create_body(row)).await;
    }

    for row in ROWS {
        let disk = emulator.disk(row.disk_name).await;
        assert_eq!(
            disk["name"].as_str(),
            Some(row.disk_name),
            "case {}: expected '{}' to still read back, got {disk:?}",
            row.case,
            row.disk_name
        );
        assert_eq!(
            disk["sizeInGb"].as_i64(),
            Some(row.size_in_gb),
            "case {}: expected its own size to be untouched, got {disk:?}",
            row.case
        );
        assert_eq!(
            disk["tags"].as_array().map(Vec::len),
            Some(row.tags.len()),
            "case {}: expected its own tags to be untouched, got {:?}",
            row.case,
            disk["tags"]
        );
    }
}

/// RULE: a created disk is `pending` when the call returns and the CLOCK settles
/// it into `available`; a read reports the state, never moves it, and the
/// operation the create logged settles with it.
#[tokio::test]
async fn the_clock_settles_a_new_disk_into_available() {
    let emulator = Emulator::new();
    let name = "awaited-disk";
    let created = emulator
        .ok(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": "us-east-2a", "sizeInGb": 32}),
        )
        .await;
    assert_eq!(
        created["operations"][0]["status"].as_str(),
        Some("Started"),
        "case straight after the create: expected a Started operation, got {created:?}"
    );

    let disk = emulator.disk(name).await;
    assert_eq!(
        disk["state"].as_str(),
        Some("pending"),
        "case straight after create: expected pending, got {:?}",
        disk["state"]
    );

    let settled = wait_for_state(&emulator, name, "available").await;
    assert_eq!(
        settled["isAttached"].as_bool(),
        Some(false),
        "case after the clock settled it: a settled disk is still attached to nothing, got \
         {settled:?}"
    );

    // Reading changes nothing a later caller sees: the same record, twice.
    let again = emulator.disk(name).await;
    assert_eq!(
        again, settled,
        "case reading twice after it settled: expected the same record, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `sizeInGb` outside the size range Lightsail creates a disk in is
/// refused with the modelled `InvalidInputException` (400) and stores nothing.
///
/// The cases sit one GB past each published bound, at zero and negative, and far
/// past the ceiling.
#[tokio::test]
async fn a_size_outside_the_published_range_is_refused_and_stores_nothing() {
    let cases: &[(&str, i64)] = &[
        ("one GB below the smallest published size", MIN_SIZE_IN_GB - 1),
        ("zero GB", 0),
        ("a negative size", -32),
        ("one GB above the largest published size", MAX_SIZE_IN_GB + 1),
        ("far above the largest published size", 1_000_000),
    ];

    for (case, size_in_gb) in cases {
        let emulator = Emulator::new();
        let name = "sized-disk";
        let (status, body) = emulator
            .call(
                "CreateDisk",
                json!({
                    "diskName": name,
                    "availabilityZone": "us-east-2a",
                    "sizeInGb": size_in_gb,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal for {size_in_gb} GB, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("sizeInGb"),
            "case {case}: expected the refusal to name the member, got {body:?}"
        );

        let (read_status, read_body) = emulator.call("GetDisk", json!({"diskName": name})).await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create stores nothing, so the read must be refused too, \
             got {read_status} {read_body:?}"
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: a refused create stores nothing, so the read must answer code \
             DoesNotExist, got {read_body:?}"
        );
    }

    // Each published bound itself is accepted, in the same harness the refusals
    // ran in.
    for (case, size_in_gb) in [
        ("the smallest published size", MIN_SIZE_IN_GB),
        ("the largest published size", MAX_SIZE_IN_GB),
    ] {
        let emulator = Emulator::new();
        let name = "bound-disk";
        emulator
            .ok(
                "CreateDisk",
                json!({
                    "diskName": name,
                    "availabilityZone": "us-east-2a",
                    "sizeInGb": size_in_gb,
                }),
            )
            .await;
        let disk = emulator.disk(name).await;
        assert_eq!(
            disk["sizeInGb"].as_i64(),
            Some(size_in_gb),
            "case {case}: expected {size_in_gb} GB to be accepted and stored, got {disk:?}"
        );
    }
}

/// RULE: an `availabilityZone` that is not a zone of a Lightsail region is
/// refused with the modelled `InvalidInputException` (400) and stores nothing.
#[tokio::test]
async fn a_zone_that_is_no_lightsail_zone_is_refused() {
    let cases: &[(&str, &str)] = &[
        ("a region with no zone letter", "us-east-2"),
        ("a region no Lightsail region name matches", "xx-east-9a"),
        ("an uppercase zone letter", "us-east-2A"),
        ("a bare word", "anywhere"),
    ];

    for (case, availability_zone) in cases {
        let emulator = Emulator::new();
        let name = "zoned-disk";
        let (status, body) = emulator
            .call(
                "CreateDisk",
                json!({
                    "diskName": name,
                    "availabilityZone": availability_zone,
                    "sizeInGb": 32,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal for {availability_zone:?}, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("availabilityZone"),
            "case {case}: expected the refusal to name the member, got {body:?}"
        );

        let (read_status, read_body) = emulator.call("GetDisk", json!({"diskName": name})).await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create stores nothing, so the read must be refused too, \
             got {read_status} {read_body:?}"
        );
    }
}

/// RULE: of the modelled `AddOnType` members only `AutoSnapshot` describes a
/// disk, so asking for the idle-stop add-on on a disk is refused and stores
/// nothing.
#[tokio::test]
async fn an_add_on_a_disk_cannot_carry_is_refused() {
    let emulator = Emulator::new();
    let name = "add-on-disk";
    let (status, body) = emulator
        .call(
            "CreateDisk",
            json!({
                "diskName": name,
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
                "addOns": [{
                    "addOnType": "StopInstanceOnIdle",
                    "stopInstanceOnIdleRequest": {"threshold": "5", "duration": "20"},
                }],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an instance add-on on a disk: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case an instance add-on on a disk: expected the modelled InvalidInputException, got \
         {body:?}"
    );
    assert!(
        body["message"].as_str().unwrap_or_default().contains("addOns"),
        "case an instance add-on on a disk: expected the refusal to name the member, got \
         {body:?}"
    );

    let (read_status, read_body) = emulator.call("GetDisk", json!({"diskName": name})).await;
    assert_eq!(
        read_status, 400,
        "case an instance add-on on a disk: a refused create stores nothing, got \
         {read_status} {read_body:?}"
    );
}

/// RULE: a disk name addresses ONE disk, so creating one the account already
/// holds is refused and leaves the stored disk exactly as it was.
#[tokio::test]
async fn an_existing_disk_cannot_be_created_again() {
    let emulator = Emulator::new();
    let name = "only-once";
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": name,
                "availabilityZone": "us-east-2a",
                "sizeInGb": 32,
                "tags": [{"key": "keep", "value": "me"}],
            }),
        )
        .await;
    let before = wait_for_state(&emulator, name, "available").await;

    let (status, body) = emulator
        .call(
            "CreateDisk",
            json!({"diskName": name, "availabilityZone": "eu-west-1a", "sizeInGb": 1024}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case creating an existing disk: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case creating an existing disk: expected the modelled InvalidInputException, got \
         {body:?}"
    );

    let after = emulator.disk(name).await;
    assert_eq!(
        after, before,
        "case creating an existing disk: expected the stored disk to be untouched, got \
         {after:?}"
    );
}

/// RULE: a well-formed name that names no stored disk is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name —
/// never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_disk_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "present-disk", "availabilityZone": "us-east-2a", "sizeInGb": 8}),
        )
        .await;

    for name in ["absent-disk", "present-disk-2", "other_disk"] {
        let (status, body) = emulator.call("GetDisk", json!({"diskName": name})).await;
        assert_eq!(
            status, 400,
            "case reading {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Disk does not exist: {name}").as_str()),
            "case reading {name}: expected the resource-type/name message, got {body:?}"
        );
    }
}
