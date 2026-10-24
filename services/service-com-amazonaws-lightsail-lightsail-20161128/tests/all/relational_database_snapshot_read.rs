// SPDX-License-Identifier: CC0-1.0

//! The single read of a Lightsail managed relational database snapshot —
//! `GetRelationalDatabaseSnapshot` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetRelationalDatabaseSnapshot` answers the ONE stored
//! snapshot record the requested name addresses, every member read out of that
//! record — so it is member-for-member the entry `GetRelationalDatabaseSnapshots`
//! lists, whatever the source database's engine, bundle, location or tags, and
//! whatever region the read is signed for. It is a reporting call: it persists
//! nothing of its own, so repeated reads answer the same record and the
//! `creating` → `available` transition arrives from the CLOCK rather than from
//! being read. A well-formed name the account holds no snapshot under — including
//! a name that addresses some other resource — is refused with the service's
//! `DoesNotExist` envelope (HTTP 400, `NotFoundException`, body code
//! `DoesNotExist`, a message naming the resource type and the requested name),
//! never a zeroed 200.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The snapshot `name` as `GetRelationalDatabaseSnapshot` reports it, read in
/// `region` when one is given.
async fn read(emulator: &Emulator, region: Option<&str>, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            region,
            "GetRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": name}),
        )
        .await["relationalDatabaseSnapshot"]
        .clone()
}

/// The snapshot `name` as the LISTING reports it, which must report it exactly
/// once.
async fn listed(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok("GetRelationalDatabaseSnapshots", json!({}))
        .await;
    let entries = body["relationalDatabaseSnapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case listing snapshots: expected an array, got {body:?}"))
        .clone();
    let matching: Vec<&Value> = entries
        .iter()
        .filter(|entry| entry["name"].as_str() == Some(name))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case reading {name} back from the listing: expected exactly one entry, got {matching:?}"
    );
    matching[0].clone()
}

/// The stored database `name`, as `GetRelationalDatabase` reports it.
async fn database(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": name}),
        )
        .await["relationalDatabase"]
        .clone()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One snapshot the single read must answer, with the source database it was
/// taken of.
///
/// The rows differ along everything the rule generalises over: the source's
/// ENGINE and engine version (MySQL, and PostgreSQL at three published series),
/// its bundle — and so the disk size the snapshot reports, from the smallest
/// published plan to the largest, standard and high-availability — whether the
/// source request named an Availability Zone at all, the zone's region (and so
/// the region the snapshot's ARN and location carry), the region the READ is
/// signed for, which this operation must ignore, the number of tags, and the
/// form of the snapshot name (hyphens, underscores, digits, shortest allowed).
struct Row {
    case: &'static str,
    db_name: &'static str,
    /// The region the source create is signed for, or `None` for a request that
    /// carries no credential scope at all.
    db_region: Option<&'static str>,
    availability_zone: Option<&'static str>,
    /// A blueprint id this service publishes (`GetRelationalDatabaseBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service publishes (`GetRelationalDatabaseBundles`).
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The region the READ is signed for: a snapshot is located where its source
    /// is, so this must change nothing about the answer.
    read_region: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`GetRelationalDatabaseBlueprints`,
/// `GetRelationalDatabaseBundles`); the sources are created through the service,
/// so a wrong id fails the create rather than this read.
const ROWS: &[Row] = &[
    Row {
        case: "mysql on the smallest published bundle, no tags, unsigned read",
        db_name: "read-db-mysql-micro",
        db_region: None,
        availability_zone: Some("us-east-2a"),
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        snapshot_name: "read-db-mysql-micro-snapshot",
        read_region: None,
        tags: &[],
    },
    Row {
        case: "postgres 15 on the largest published bundle, read signed for another region",
        db_name: "read-db-pg15-large",
        db_region: Some("us-east-1"),
        availability_zone: Some("us-east-1c"),
        blueprint_id: "postgres_15",
        bundle_id: "large_1_0",
        snapshot_name: "read_pg15_large_snap_1",
        read_region: Some("eu-central-1"),
        tags: &[("env", "prod")],
    },
    Row {
        case: "postgres 13, the oldest published series, on a high-availability bundle, three tags",
        db_name: "read-db-pg13-ha",
        db_region: Some("ap-northeast-1"),
        availability_zone: Some("ap-northeast-1c"),
        blueprint_id: "postgres_13",
        bundle_id: "small_ha_1_0",
        snapshot_name: "read-db-pg13-ha-2026-10-24",
        read_region: Some("ap-northeast-1"),
        tags: &[("env", "staging"), ("owner", "data"), ("cost-centre", "42")],
    },
    Row {
        case: "a source the request placed by region alone, postgres 17 on a medium bundle",
        db_name: "read-db-default-zone",
        db_region: Some("us-west-2"),
        availability_zone: None,
        blueprint_id: "postgres_17",
        bundle_id: "medium_1_0",
        snapshot_name: "read-snap-4",
        read_region: Some("us-west-2"),
        tags: &[("tier", "bronze")],
    },
    Row {
        case: "mysql on a micro high-availability bundle, shortest snapshot name, two tags",
        db_name: "read_db_mysql_ha",
        db_region: Some("ca-central-1"),
        availability_zone: Some("ca-central-1b"),
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_ha_1_0",
        snapshot_name: "r5",
        read_region: Some("eu-west-3"),
        tags: &[("env", "dev"), ("owner", "qa")],
    },
];

/// The `tags` member a row's request carries, in the modelled `TagList` form.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Create `row`'s source database and then its snapshot.
async fn create_row(emulator: &Emulator, row: &Row) {
    let mut db_body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "masterDatabaseName": "appdata",
        "masterUsername": "dbmaster",
    });
    if let Some(zone) = row.availability_zone {
        db_body["availabilityZone"] = json!(zone);
    }
    emulator
        .ok_at(None, row.db_region, "CreateRelationalDatabase", db_body)
        .await;

    let mut snapshot_body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseSnapshotName": row.snapshot_name,
    });
    if !row.tags.is_empty() {
        snapshot_body["tags"] = tag_list(row.tags);
    }
    emulator
        .ok_at(
            None,
            row.db_region,
            "CreateRelationalDatabaseSnapshot",
            snapshot_body,
        )
        .await;
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: the single read answers the ONE persisted record — the source's own
/// stored values, the request's tags verbatim, and server-assigned members
/// computed from them plus the store's own allocators — whatever region the read
/// is signed for.
#[tokio::test]
async fn the_single_read_answers_the_stored_record() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        create_row(&emulator, row).await;

        let source = database(&emulator, row.db_name).await;
        let snapshot = read(&emulator, row.read_region, row.snapshot_name).await;

        assert_eq!(
            snapshot["name"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the snapshot name the request carried, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["resourceType"].as_str(),
            Some("RelationalDatabaseSnapshot"),
            "case {case}: expected the RelationalDatabaseSnapshot resource type, got {snapshot:?}"
        );

        // Everything the snapshot says about its source is the source's own
        // stored value.
        assert_eq!(
            snapshot["engine"], source["engine"],
            "case {case}: expected the source's own engine {:?}, got {:?}",
            source["engine"], snapshot["engine"]
        );
        assert_eq!(
            snapshot["engineVersion"], source["engineVersion"],
            "case {case}: expected the source's own engine version {:?}, got {:?}",
            source["engineVersion"], snapshot["engineVersion"]
        );
        assert_eq!(
            snapshot["sizeInGb"], source["hardware"]["diskSizeInGb"],
            "case {case}: expected the source's own disk size {:?}, got {:?}",
            source["hardware"]["diskSizeInGb"], snapshot["sizeInGb"]
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseName"].as_str(),
            Some(row.db_name),
            "case {case}: expected the source database's name, got {snapshot:?}"
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseArn"], source["arn"],
            "case {case}: expected the source database's own ARN {:?}, got {:?}",
            source["arn"], snapshot["fromRelationalDatabaseArn"]
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseBundleId"], source["relationalDatabaseBundleId"],
            "case {case}: expected the source's own bundle id {:?}, got {:?}",
            source["relationalDatabaseBundleId"], snapshot["fromRelationalDatabaseBundleId"]
        );
        assert_eq!(
            snapshot["fromRelationalDatabaseBlueprintId"], source["relationalDatabaseBlueprintId"],
            "case {case}: expected the source's own blueprint id {:?}, got {:?}",
            source["relationalDatabaseBlueprintId"], snapshot["fromRelationalDatabaseBlueprintId"]
        );

        // Located where the source is, whatever region the read was signed for.
        assert_eq!(
            snapshot["location"], source["location"],
            "case {case}: expected the source's own location {:?}, got {:?}",
            source["location"], snapshot["location"]
        );

        // The tags the create carried, verbatim and in order — and an empty
        // list, not an absent one, for a request that carried none.
        assert_eq!(
            snapshot["tags"],
            tag_list(row.tags),
            "case {case}: expected the tags the create carried, got {:?}",
            snapshot["tags"]
        );

        // The ARN carries the SOURCE's region and the canonical emulator
        // account, and ends in a minted uuid; the support code names the same id.
        let region = source["location"]["regionName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a source region, got {source:?}"));
        let arn = snapshot["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{region}:{ACCOUNT}:RelationalDatabaseSnapshot/");
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert_eq!(
            snapshot["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            snapshot["supportCode"]
        );
        assert!(
            snapshot["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            snapshot["createdAt"]
        );
        let state = snapshot["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "creating" || state == "available",
            "case {case}: expected a creating or available snapshot, got {state:?}"
        );
    }
}

/// RULE: the single read and the listing report the SAME record, member for
/// member — there is one stored snapshot, not one per reader.
#[tokio::test]
async fn the_single_read_and_the_listing_agree() {
    let emulator = Emulator::new();
    for row in ROWS {
        create_row(&emulator, row).await;
    }

    // Read every snapshot once the clock has settled them all, so a state
    // difference cannot be mistaken for a disagreement between the two readers.
    for row in ROWS {
        wait_for_state(&emulator, row.snapshot_name, "available").await;
    }

    for row in ROWS {
        let case = row.case;
        let single = read(&emulator, row.read_region, row.snapshot_name).await;
        let from_listing = listed(&emulator, row.snapshot_name).await;
        assert_eq!(
            single, from_listing,
            "case {case}: expected the single read to answer the listed record {from_listing:?}, \
             got {single:?}"
        );
    }
}

/// Poll the single read until the snapshot `name` reports `want`, answering it.
///
/// The transition is owed by the CLOCK, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = read(emulator, None, name).await;
        if snapshot["state"].as_str() == Some(want) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            snapshot["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: reading is not mutating — a snapshot read straight after its create is
/// still `creating`, however many times it is read, and the CLOCK alone settles
/// it into `available` without changing anything else about the record.
#[tokio::test]
async fn reading_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let row = &ROWS[0];
    create_row(&emulator, row).await;

    // Reading repeatedly cannot advance the state: the span the clock owes is
    // not consumed by a reader.
    let fresh = read(&emulator, None, row.snapshot_name).await;
    assert_eq!(
        fresh["state"].as_str(),
        Some("creating"),
        "case reading straight after the create: expected a creating snapshot, got {fresh:?}"
    );
    // Repeated reads answer the same record. Only the clock may change the
    // state, and only forwards — so a read is allowed to observe the settle the
    // clock already owed, and nothing else about the record may move with it.
    let mut seen_available = false;
    for attempt in 0..5 {
        let again = read(&emulator, None, row.snapshot_name).await;
        let state = again["state"].as_str().unwrap_or_default().to_string();
        assert!(
            state == "creating" || state == "available",
            "case read {attempt} after the create: expected a creating or available snapshot, got \
             {state:?}"
        );
        assert!(
            !(seen_available && state == "creating"),
            "case read {attempt} after the create: a settled snapshot may not go back to \
             creating, got {again:?}"
        );
        seen_available |= state == "available";
        let mut without_state = again.clone();
        without_state["state"] = fresh["state"].clone();
        assert_eq!(
            without_state, fresh,
            "case read {attempt} after the create: expected the same record {fresh:?} but for the \
             clock-owed state, got {again:?}"
        );
    }

    let settled = wait_for_state(&emulator, row.snapshot_name, "available").await;
    // Settling changes the state and NOTHING else about the record.
    for member in [
        "name",
        "arn",
        "supportCode",
        "createdAt",
        "location",
        "resourceType",
        "tags",
        "engine",
        "engineVersion",
        "sizeInGb",
        "fromRelationalDatabaseName",
        "fromRelationalDatabaseArn",
        "fromRelationalDatabaseBundleId",
        "fromRelationalDatabaseBlueprintId",
    ] {
        assert_eq!(
            settled[member], fresh[member],
            "case settling to available: expected {member} to be untouched, got {:?}",
            settled[member]
        );
    }

    // And an available snapshot stays available: a terminal state is not moved
    // by being read either.
    let after = read(&emulator, None, row.snapshot_name).await;
    assert_eq!(
        after, settled,
        "case re-reading a settled snapshot: expected the same record {settled:?}, got {after:?}"
    );
}

/// RULE: a well-formed name the account holds no DATABASE SNAPSHOT under is
/// refused with the service's `DoesNotExist` envelope — HTTP 400,
/// `NotFoundException`, body code `DoesNotExist` and a message naming the
/// resource type and the requested name — and never a zeroed 200.
#[tokio::test]
async fn a_snapshot_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // An empty account refuses too: there is nothing to confuse the lookup with.
    let (status, body) = emulator
        .call(
            "GetRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": "nothing-here"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an empty account: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case an empty account: expected the modelled NotFoundException, got {body:?}"
    );

    // Now a populated account: a database, an instance and a disk snapshot all
    // hold names of their own, and none of them is a database snapshot.
    create_row(&emulator, &ROWS[0]).await;
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "read-disk",
                "availabilityZone": "us-east-2a",
                "sizeInGb": 8,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": "read-disk", "diskSnapshotName": "read-disk-snapshot"}),
        )
        .await;

    for (case, name) in [
        ("a snapshot name never created", "read-absent-snapshot"),
        (
            "the SOURCE DATABASE's own name, which is not a snapshot",
            ROWS[0].db_name,
        ),
        ("a DISK snapshot's name", "read-disk-snapshot"),
        ("a disk's name", "read-disk"),
        (
            "a name that differs from a stored one only in case",
            "READ-DB-MYSQL-MICRO-SNAPSHOT",
        ),
    ] {
        let (status, body) = emulator
            .call(
                "GetRelationalDatabaseSnapshot",
                json!({"relationalDatabaseSnapshotName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The RelationalDatabaseSnapshot does not exist: {name}").as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
        assert!(
            body["relationalDatabaseSnapshot"].is_null(),
            "case {case}: a refusal carries no snapshot, got {body:?}"
        );
    }

    // A refused read leaves the one snapshot the account does hold readable.
    let still_there = read(&emulator, None, ROWS[0].snapshot_name).await;
    assert_eq!(
        still_there["name"].as_str(),
        Some(ROWS[0].snapshot_name),
        "case after the refusals: expected the stored snapshot to still read back, got \
         {still_there:?}"
    );
}
