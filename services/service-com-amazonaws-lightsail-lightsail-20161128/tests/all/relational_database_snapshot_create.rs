// SPDX-License-Identifier: CC0-1.0

//! Taking a point-in-time copy of ONE Lightsail managed relational database —
//! `CreateRelationalDatabaseSnapshot` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test, which the sibling read and listing suites do not pin:
//!
//! * the call is SERVED — never a 501 and never a hollow 200 — and answers
//!   exactly one operation-log entry for the one snapshot it created: the entry
//!   names this call's own operation type, the snapshot it created and the
//!   SOURCE's location, carries a minted id of its own, and is reported by
//!   `GetOperation` under that id;
//! * that entry is non-terminal when the call returns and the CLOCK, not a
//!   reader, settles it into `Succeeded` — by which time the snapshot it created
//!   is `available`, since one clock owes both;
//! * the caller's `tags` are kept verbatim — whatever their number up to the
//!   bound this service accepts, and whether or not a tag carries a value —
//!   while a list past that bound is refused with the modelled
//!   `InvalidInputException` (HTTP 400) and stores nothing; and
//! * taking a copy writes to the SNAPSHOT store only: the source database is
//!   exactly as it was afterwards, however many copies are taken of it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    MAX_TAGS, RELATIONAL_DATABASE_SNAPSHOT_PAGE_SIZE,
};

// The harness, the wire error code, the minted-id check, the tag and
// operation-log helpers, the generic page walk and the database reads sibling
// suites already expose: this suite drives the service through the same front
// door, and through the same operation-agnostic helpers, rather than keeping a
// second copy of any of them.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
use crate::instance_snapshot_create::{
    generated_tags, single_operation, tag_list, wait_for_terminal_operation,
};
use crate::paging::{names, walk_pages};
use crate::relational_database_start_stop::{database, wait_for_state};

/// The operation under test.
const OPERATION: &str = "CreateRelationalDatabaseSnapshot";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The stored snapshot `name`, as `GetRelationalDatabaseSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "GetRelationalDatabaseSnapshot",
            json!({"relationalDatabaseSnapshotName": name}),
        )
        .await["relationalDatabaseSnapshot"]
        .clone()
}

/// Every database-snapshot name the account holds, in name order, through the
/// crate's own page walk — which checks the paging contract and is bounded as it
/// goes, so a listing that does not advance fails the case rather than hanging.
async fn snapshot_names(emulator: &Emulator) -> Vec<String> {
    let (records, _) = walk_pages(
        emulator,
        "listing the account's database snapshots",
        None,
        "GetRelationalDatabaseSnapshots",
        "relationalDatabaseSnapshots",
        RELATIONAL_DATABASE_SNAPSHOT_PAGE_SIZE,
    )
    .await;
    let mut listed = names(&records);
    listed.sort();
    listed
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One `CreateRelationalDatabaseSnapshot` call the service must serve.
///
/// The rows differ along everything the rule generalises over: the source's
/// ENGINE and engine version (MySQL, and PostgreSQL at three published series),
/// its bundle — standard and high-availability, from the smallest published plan
/// to the largest — whether the source request placed it in an Availability Zone
/// at all, the zone's Region (and so the Region the logged entry is located in),
/// the Region the CREATE is signed for, which selects nothing because a snapshot
/// is located where its source is, the number of tags, and the form of the
/// snapshot name (hyphens, underscores, digits, the shortest the modelled
/// `ResourceName` pattern admits).
struct Row {
    case: &'static str,
    db_name: &'static str,
    /// The Region the source create is signed for, or `None` for a request that
    /// carries no credential scope at all.
    db_region: Option<&'static str>,
    availability_zone: Option<&'static str>,
    /// The Region the source is located in, which the entry must carry.
    source_region: &'static str,
    /// A blueprint id this service publishes (`GetRelationalDatabaseBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service publishes (`GetRelationalDatabaseBundles`).
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The Region the SNAPSHOT create is signed for.
    snapshot_region: Option<&'static str>,
    /// How many tags the request carries, built by [`generated_tags`].
    tag_count: usize,
}

/// The table. Blueprint and bundle ids are entries of this service's own
/// published catalogs (`GetRelationalDatabaseBlueprints`,
/// `GetRelationalDatabaseBundles`), so a wrong id fails the source create rather
/// than the call under test.
const ROWS: &[Row] = &[
    Row {
        case: "mysql on the smallest published bundle, no tags, neither call signed for a region",
        db_name: "create-db-mysql-micro",
        db_region: None,
        availability_zone: Some("us-east-2a"),
        source_region: "us-east-2",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_1_0",
        snapshot_name: "create-db-mysql-micro-snap",
        snapshot_region: None,
        tag_count: 0,
    },
    Row {
        case: "postgres 15 on the largest published bundle, one key-only tag, create signed for \
               another region",
        db_name: "create_db_pg15_large",
        db_region: Some("us-east-1"),
        availability_zone: Some("us-east-1c"),
        source_region: "us-east-1",
        blueprint_id: "postgres_15",
        bundle_id: "large_1_0",
        snapshot_name: "create_pg15_large_snap_1",
        snapshot_region: Some("eu-central-1"),
        tag_count: 1,
    },
    Row {
        case: "postgres 13, the oldest published series, on a high-availability bundle, three tags",
        db_name: "create-db-pg13-ha",
        db_region: Some("ap-northeast-1"),
        availability_zone: Some("ap-northeast-1c"),
        source_region: "ap-northeast-1",
        blueprint_id: "postgres_13",
        bundle_id: "small_ha_1_0",
        snapshot_name: "create-db-pg13-ha-2026-10-24",
        snapshot_region: Some("ap-northeast-1"),
        tag_count: 3,
    },
    Row {
        case: "a source the request placed by region alone, postgres 17 on a medium bundle, \
               shortest snapshot name",
        db_name: "create-db-default-zone",
        db_region: Some("us-west-2"),
        availability_zone: None,
        source_region: "us-west-2",
        blueprint_id: "postgres_17",
        bundle_id: "medium_1_0",
        snapshot_name: "c4",
        snapshot_region: Some("us-west-2"),
        tag_count: 5,
    },
    Row {
        case: "mysql on a micro high-availability bundle, four tags, create signed for a fifth \
               region",
        db_name: "create_db_mysql_ha",
        db_region: Some("ca-central-1"),
        availability_zone: Some("ca-central-1b"),
        source_region: "ca-central-1",
        blueprint_id: "mysql_8_4",
        bundle_id: "micro_ha_1_0",
        snapshot_name: "Create_Mysql_Ha_Snap_5",
        snapshot_region: Some("eu-west-3"),
        tag_count: 4,
    },
];

/// Stand `row`'s source database up.
async fn create_source(emulator: &Emulator, row: &Row) {
    let mut body = json!({
        "relationalDatabaseName": row.db_name,
        "relationalDatabaseBlueprintId": row.blueprint_id,
        "relationalDatabaseBundleId": row.bundle_id,
        "masterDatabaseName": "appdata",
        "masterUsername": "dbmaster",
    });
    if let Some(zone) = row.availability_zone {
        body["availabilityZone"] = json!(zone);
    }
    emulator
        .ok_at(None, row.db_region, "CreateRelationalDatabase", body)
        .await;
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a served create of one database snapshot answers exactly ONE
/// operation-log entry — this call's own operation type, against the snapshot it
/// created, in the SOURCE's location, under a minted id `GetOperation` reports it
/// by — and that entry is non-terminal when the call returns, settled into
/// `Succeeded` by the clock, by which time the copy it created is `available`.
#[tokio::test]
async fn a_create_answers_one_operation_the_clock_settles() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_source(&emulator, row).await;
        let source = database(&emulator, row.db_name).await;

        let tags = generated_tags(row.tag_count);
        let mut body = json!({
            "relationalDatabaseName": row.db_name,
            "relationalDatabaseSnapshotName": row.snapshot_name,
        });
        if row.tag_count > 0 {
            body["tags"] = tag_list(&tags);
        }
        // SERVED: a status of its own, never the 501 an unimplemented operation
        // answers.
        let (status, answered) = emulator
            .call_at(None, row.snapshot_region, OPERATION, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the create to be served, got {status} {answered:?}"
        );

        let operation = single_operation(case, &answered);
        assert_eq!(
            operation["operationType"].as_str(),
            Some(OPERATION),
            "case {case}: expected this call's own operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("RelationalDatabaseSnapshot"),
            "case {case}: expected the snapshot resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.snapshot_name),
            "case {case}: expected the entry to name the snapshot created, got {operation:?}"
        );
        assert_eq!(
            operation["location"], source["location"],
            "case {case}: a snapshot is located where its source is, so the entry carries {:?}, \
             got {:?}",
            source["location"], operation["location"]
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.source_region),
            "case {case}: expected the source's own region, not the region the create was signed \
             for, got {operation:?}"
        );
        assert!(
            operation["errorCode"].is_null() && operation["errorDetails"].is_null(),
            "case {case}: expected no error on a served create, got {operation:?}"
        );
        let created_at = operation["createdAt"].as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {case}: expected a creation timestamp on the entry, got {operation:?}"
        );

        let id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {case}: expected a uuid-shaped operation id, got {id:?}"
        );
        assert!(
            operation_ids.insert(id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {id:?}"
        );

        // Non-terminal when the call returns: the clock owes the copy its
        // `available` state, so it owes this entry its terminal status too.
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected the entry to be outstanding straight after the create, got \
             {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: expected an entry the clock still owes its terminal status, got \
             {operation:?}"
        );

        // The entry the create ANSWERED is the entry the log HOLDS: one record,
        // two readers.
        let logged = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        for member in [
            "id",
            "resourceName",
            "resourceType",
            "operationType",
            "operationDetails",
            "createdAt",
            "location",
        ] {
            assert_eq!(
                logged[member], operation[member],
                "case {case}: expected GetOperation to report the same {member}, got {:?}",
                logged[member]
            );
        }

        // The tags the request carried, verbatim and in order — and an empty
        // list, not an absent one, for a request that carried none:
        // `RelationalDatabaseSnapshot$tags` is a list the service reports for
        // every snapshot, as the sibling read and listing suites pin.
        let stored = snapshot(&emulator, row.snapshot_name).await;
        assert_eq!(
            stored["tags"],
            tag_list(&tags),
            "case {case}: expected the tags the request carried, got {:?}",
            stored["tags"]
        );
        assert_eq!(
            stored["fromRelationalDatabaseName"].as_str(),
            Some(row.db_name),
            "case {case}: expected the copy to name its source, got {stored:?}"
        );

        // One clock owes both: by the time the entry is terminal the copy it
        // created can be restored from.
        let settled = wait_for_terminal_operation(&emulator, &id).await;
        assert_eq!(
            settled["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected the settled entry to have succeeded, got {settled:?}"
        );
        let status_changed_at = settled["statusChangedAt"].as_f64().unwrap_or_default();
        assert!(
            status_changed_at > created_at,
            "case {case}: the settle is owed some time after the call, so expected a status \
             change after {created_at}, got {status_changed_at}"
        );
        let settled_snapshot = snapshot(&emulator, row.snapshot_name).await;
        assert_eq!(
            settled_snapshot["state"].as_str(),
            Some("available"),
            "case {case}: one clock owes the entry and the copy, so expected an available \
             snapshot once the entry settled, got {settled_snapshot:?}"
        );
    }
}

/// RULE: the caller's `tags` are kept verbatim for any list within the bound
/// this service accepts, and a list past that bound is refused with the modelled
/// `InvalidInputException` (HTTP 400) before anything is written.
///
/// The bound is tested below it, at it and past it: `crate::state::MAX_TAGS` is
/// the published Lightsail limit of tags per resource.
#[tokio::test]
async fn the_tag_list_bound_is_enforced_and_the_tags_are_kept() {
    let emulator = Emulator::new();
    create_source(&emulator, &ROWS[0]).await;
    let source_name = ROWS[0].db_name;

    // Within the bound: none, one, a middling list, one short of the bound, and
    // exactly the bound.
    for count in [0_usize, 1, 7, MAX_TAGS - 1, MAX_TAGS] {
        let case = format!("{count} tags");
        let name = format!("tagged-snap-{count}");
        let tags = generated_tags(count);
        let mut body = json!({
            "relationalDatabaseName": source_name,
            "relationalDatabaseSnapshotName": name,
        });
        if count > 0 {
            body["tags"] = tag_list(&tags);
        }
        let (status, answered) = emulator.call(OPERATION, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected a list within the bound to be served, got {status} \
             {answered:?}"
        );
        let stored = snapshot(&emulator, &name).await;
        assert_eq!(
            stored["tags"],
            tag_list(&tags),
            "case {case}: expected the tags the request carried, got {:?}",
            stored["tags"]
        );
    }

    let before = snapshot_names(&emulator).await;

    // Past the bound: refused, and nothing written.
    for count in [MAX_TAGS + 1, MAX_TAGS + 9, 10 * MAX_TAGS] {
        let case = format!("{count} tags");
        let name = format!("refused-snap-{count}");
        let (status, body) = emulator
            .call(
                OPERATION,
                json!({
                    "relationalDatabaseName": source_name,
                    "relationalDatabaseSnapshotName": name,
                    "tags": tag_list(&generated_tags(count)),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal past the bound, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&count.to_string()) && message.contains(&MAX_TAGS.to_string()),
            "case {case}: expected a message naming the list's size and the bound, got {body:?}"
        );

        let (read_status, read_body) = emulator
            .call(
                "GetRelationalDatabaseSnapshot",
                json!({"relationalDatabaseSnapshotName": name}),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create writes nothing, so the name names no snapshot, got \
             {read_status} {read_body:?}"
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist envelope for the unwritten name, got \
             {read_body:?}"
        );
        assert_eq!(
            snapshot_names(&emulator).await,
            before,
            "case {case}: a refused create leaves the account's snapshots untouched"
        );
    }
}

/// RULE: taking a copy writes to the SNAPSHOT store only — the source database is
/// exactly as it was afterwards, however many copies are taken of it, and each
/// copy is its own stored record.
#[tokio::test]
async fn a_create_leaves_its_source_untouched() {
    let emulator = Emulator::new();
    create_source(&emulator, &ROWS[1]).await;
    let source_name = ROWS[1].db_name;
    // Taken to its own terminal state first, so a later difference is the
    // create's doing rather than the clock's.
    let before = wait_for_state(&emulator, source_name, "available").await;

    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut names: Vec<String> = Vec::new();
    for index in 0..3 {
        let name = format!("untouched-snap-{index}");
        emulator
            .ok(
                OPERATION,
                json!({
                    "relationalDatabaseName": source_name,
                    "relationalDatabaseSnapshotName": name,
                }),
            )
            .await;
        let after = database(&emulator, source_name).await;
        assert_eq!(
            after, before,
            "case after taking copy {index}: expected the source database to be untouched, got \
             {after:?}"
        );
        let stored = snapshot(&emulator, &name).await;
        let arn = stored["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arns.insert(arn.clone()),
            "case after taking copy {index}: expected a copy with an identity of its own, got the \
             already-used {arn:?}"
        );
        names.push(name);
    }

    names.sort();
    assert_eq!(
        snapshot_names(&emulator).await,
        names,
        "case after three creates: expected three stored copies of the one source"
    );
}
