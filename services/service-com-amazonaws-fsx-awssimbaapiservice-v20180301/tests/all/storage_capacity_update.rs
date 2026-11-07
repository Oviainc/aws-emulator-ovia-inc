// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an `UpdateFileSystem` storage-capacity target, driven over the wire.
//!
//! RULE: a storage-capacity target has to be one the file system can move to by the rule AWS
//! documents for its type on `UpdateFileSystemRequest$StorageCapacity` — at least 10 percent
//! greater than the current value for OpenZFS, Windows and an ONTAP increase, at least 9
//! percent smaller for an ONTAP decrease, and greater than the current value on the
//! deployment type's documented multiple for Lustre. A target outside its type's rule is
//! refused with `BadRequest` at HTTP 400 and mutates NOTHING; an accepted target is not
//! applied in line but recorded as one `FILE_SYSTEM_UPDATE` administrative action whose
//! `TargetFileSystemValues.StorageCapacity` names it, with the file system still reporting
//! the capacity it has and staying `AVAILABLE` until that action is `COMPLETED`.
//!
//! The tables below span what that rule generalises over: the file-system type, the storage
//! type and deployment type the floor depends on, and the FORM of the target (a decrease, an
//! equal value, an increase inside the band, the smallest increase that clears it, and a
//! value off the documented multiple). The harness is the lifecycle suite's, so every case
//! goes through the same front door a client uses and nothing reaches into the store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{call, emulator, ok, with_client_request_token, PAST_TRANSITION};

/// The modelled `AdministrativeActionType` an `UpdateFileSystem` starts
/// (`smithy_query(query="shape", shape="AdministrativeActionType")`).
const FILE_SYSTEM_UPDATE: &str = "FILE_SYSTEM_UPDATE";

/// The modelled `Status` values an administrative action reports
/// (`smithy_query(query="shape", shape="Status")`).
const PENDING: &str = "PENDING";
/// See [`PENDING`].
const COMPLETED: &str = "COMPLETED";

/// What the request asks of one file system, and what must come back.
#[derive(Debug)]
enum Target {
    /// The target clears its type's documented floor: HTTP 200, with the capacity still
    /// reported as it was and one administrative action carrying the target.
    Accepted(i64),
    /// The target does not: HTTP 400 `BadRequest` carrying exactly this `Message`, and
    /// nothing changed.
    Refused(i64, String),
}

/// One row of the table: a file system of one type and configuration, and the targets asked
/// of it in order.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The modelled `FileSystemType` value.
    file_system_type: &'static str,
    /// The modelled `StorageType` value, which the Lustre multiples depend on.
    storage_type: &'static str,
    /// The `CreateFileSystem` member the per-type configuration goes in, if any.
    configuration_member: Option<&'static str>,
    configuration: Value,
    /// The `StorageCapacity` the file system is created with, in GiB.
    capacity: i64,
    targets: Vec<Target>,
}

/// The `Message` of the 10-percent floor AWS documents for OpenZFS, for Windows and for an
/// ONTAP increase. The two sentences are fixed text; the requested target comes from the
/// request and the existing value from the file system's stored capacity.
fn ten_percent_message(requested: i64, current: i64) -> String {
    format!(
        "Invalid desired storage capacity provided: {requested} GiB. Desired storage capacity \
         must be at least 10% higher than the existing value of {current} GiB."
    )
}

/// The `Message` of the documented ONTAP decrease band ("the target value must be at least 9
/// percent smaller than the current SSD storage capacity").
fn nine_percent_message(requested: i64, current: i64) -> String {
    format!(
        "Invalid desired storage capacity provided: {requested} GiB. Desired storage capacity \
         must be at least 9% lower than the existing value of {current} GiB."
    )
}

/// The `Message` of the documented Lustre rule "the value must be greater than the current
/// storage capacity".
fn greater_than_message(requested: i64, current: i64) -> String {
    format!(
        "Invalid desired storage capacity provided: {requested} GiB. Desired storage capacity \
         must be greater than the existing value of {current} GiB."
    )
}

/// The `Message` of the documented Lustre multiples ("valid values are in multiples of 2400
/// GiB" for the SSD deployment types, "multiples of 6000 GiB for 12-MBps throughput per TiB
/// file systems and multiples of 1800 GiB for 40-MBps throughput per TiB file systems" for
/// `PERSISTENT HDD`).
fn multiple_message(requested: i64, multiple: i64) -> String {
    format!(
        "Invalid desired storage capacity provided: {requested} GiB. Desired storage capacity \
         must be a multiple of {multiple} GiB."
    )
}

/// The `Message` of the documented `SCRATCH_1` rule ("For `SCRATCH_1` file systems, you can't
/// increase the storage capacity").
fn increase_unsupported_message(requested: i64, deployment_type: &str) -> String {
    format!(
        "Invalid desired storage capacity provided: {requested} GiB. Storage capacity cannot be \
         increased for file systems with the {deployment_type} deployment type."
    )
}

/// The `Message` of the documented in-flight rule ("You can't make a storage capacity increase
/// request if there is an existing storage capacity increase request in progress").
fn in_progress_message(requested: i64) -> String {
    format!(
        "Invalid desired storage capacity provided: {requested} GiB. A storage capacity update \
         is already in progress for this file system."
    )
}

/// Create the file system one row describes and answer its `FileSystemId`.
async fn create(router: &crate::file_system_lifecycle::Emulator, row: &Row) -> String {
    let mut body = json!({
        "FileSystemType": row.file_system_type,
        "StorageType": row.storage_type,
        "StorageCapacity": row.capacity,
        "SubnetIds": [router.subnet_a()],
    });
    if let Some(member) = row.configuration_member {
        body[member] = row.configuration.clone();
    }
    let created = ok(router, "CreateFileSystem", &with_client_request_token(&body)).await;
    created["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Describe exactly one file system, requiring it to exist.
async fn describe_one(router: &MultiServiceRouter, id: &str) -> Value {
    let body = ok(
        router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [id] }),
    )
    .await;
    let systems = body["FileSystems"]
        .as_array()
        .expect("FileSystems must be a list")
        .clone();
    assert_eq!(
        systems.len(),
        1,
        "describing {id} must return exactly one file system, got {body}"
    );
    systems[0].clone()
}

/// The `AdministrativeActions` a `FileSystem` reports, as a list.
fn actions(file_system: &Value) -> Vec<Value> {
    file_system["AdministrativeActions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Wait out the clock-owed span and describe the file system, so an administrative action
/// that was due has certainly been settled.
async fn settled(router: &MultiServiceRouter, id: &str) -> Value {
    tokio::time::sleep(PAST_TRANSITION).await;
    describe_one(router, id).await
}

/// The table: one row per documented floor, with targets differing in FORM.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "OPENZFS SINGLE_AZ_1, 64 GiB",
            file_system_type: "OPENZFS",
            storage_type: "SSD",
            configuration_member: Some("OpenZFSConfiguration"),
            configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 64 }),
            capacity: 64,
            targets: vec![
                // A decrease, an equal target and an increase inside the 64 -> 70.4 band.
                Target::Refused(32, ten_percent_message(32, 64)),
                Target::Refused(64, ten_percent_message(64, 64)),
                Target::Refused(70, ten_percent_message(70, 64)),
                // The smallest target that clears the floor.
                Target::Accepted(71),
            ],
        },
        Row {
            case: "WINDOWS SINGLE_AZ_1, 100 GiB",
            file_system_type: "WINDOWS",
            storage_type: "SSD",
            configuration_member: Some("WindowsConfiguration"),
            configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 32 }),
            capacity: 100,
            targets: vec![
                Target::Refused(99, ten_percent_message(99, 100)),
                Target::Refused(100, ten_percent_message(100, 100)),
                Target::Refused(109, ten_percent_message(109, 100)),
                Target::Accepted(110),
            ],
        },
        Row {
            case: "ONTAP SINGLE_AZ_1, 1024 GiB, increase",
            file_system_type: "ONTAP",
            storage_type: "SSD",
            configuration_member: Some("OntapConfiguration"),
            configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 128 }),
            capacity: 1024,
            targets: vec![
                Target::Refused(1024, ten_percent_message(1024, 1024)),
                // 1126 is the largest target inside the 1024 -> 1126.4 band.
                Target::Refused(1126, ten_percent_message(1126, 1024)),
                Target::Accepted(1127),
            ],
        },
        Row {
            case: "ONTAP SINGLE_AZ_1, 1024 GiB, decrease",
            file_system_type: "ONTAP",
            storage_type: "SSD",
            configuration_member: Some("OntapConfiguration"),
            configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 128 }),
            capacity: 1024,
            targets: vec![
                // 932 is the smallest decrease that is NOT 9 percent smaller than 1024.
                Target::Refused(932, nine_percent_message(932, 1024)),
                Target::Accepted(931),
            ],
        },
        Row {
            case: "LUSTRE SCRATCH_2 SSD, 2400 GiB",
            file_system_type: "LUSTRE",
            storage_type: "SSD",
            configuration_member: Some("LustreConfiguration"),
            configuration: json!({ "DeploymentType": "SCRATCH_2" }),
            capacity: 2400,
            targets: vec![
                Target::Refused(1200, greater_than_message(1200, 2400)),
                Target::Refused(2400, greater_than_message(2400, 2400)),
                // Greater than the current value, but off the documented 2400 GiB multiple.
                Target::Refused(3600, multiple_message(3600, 2400)),
                Target::Accepted(4800),
            ],
        },
        Row {
            case: "LUSTRE SCRATCH_1 SSD, 1200 GiB",
            file_system_type: "LUSTRE",
            storage_type: "SSD",
            configuration_member: Some("LustreConfiguration"),
            configuration: json!({ "DeploymentType": "SCRATCH_1" }),
            capacity: 1200,
            targets: vec![
                // An increase, which the deployment type does not allow at all.
                Target::Refused(2400, increase_unsupported_message(2400, "SCRATCH_1")),
                // And a decrease, which Lustre does not allow either.
                Target::Refused(600, greater_than_message(600, 1200)),
            ],
        },
        Row {
            case: "LUSTRE PERSISTENT_1 HDD at 12 MBps/TiB, 6000 GiB",
            file_system_type: "LUSTRE",
            storage_type: "HDD",
            configuration_member: Some("LustreConfiguration"),
            configuration: json!({
                "DeploymentType": "PERSISTENT_1",
                "PerUnitStorageThroughput": 12,
            }),
            capacity: 6000,
            targets: vec![
                Target::Refused(7000, multiple_message(7000, 6000)),
                Target::Accepted(12000),
            ],
        },
        Row {
            case: "LUSTRE PERSISTENT_1 HDD at 40 MBps/TiB, 1800 GiB",
            file_system_type: "LUSTRE",
            storage_type: "HDD",
            configuration_member: Some("LustreConfiguration"),
            configuration: json!({
                "DeploymentType": "PERSISTENT_1",
                "PerUnitStorageThroughput": 40,
            }),
            capacity: 1800,
            targets: vec![
                Target::Refused(2000, multiple_message(2000, 1800)),
                Target::Accepted(3600),
            ],
        },
    ]
}

/// RULE, over the whole table: a target outside its type's documented floor is refused with
/// `BadRequest` at HTTP 400 carrying the documented `Message`, and mutates nothing — no
/// capacity change, no administrative action and no lifecycle move.
#[tokio::test]
async fn a_target_outside_its_types_documented_floor_is_refused_and_changes_nothing() {
    let router = emulator().await;
    for row in rows() {
        let id = create(&router, &row).await;
        // Created file systems come up `CREATING`; the floor is what is under test here, so
        // each case waits for the file system to be updatable at all.
        let created = settled(&router, &id).await;
        assert_eq!(
            created["Lifecycle"], json!("AVAILABLE"),
            "case {:?}: the file system must settle to AVAILABLE before it is updated, got {:?}",
            row.case, created["Lifecycle"]
        );
        assert_eq!(
            created["AdministrativeActions"], Value::Null,
            "case {:?}: a file system nothing has been asked of must report no \
             AdministrativeActions, got {:?}",
            row.case, created["AdministrativeActions"]
        );

        for target in &row.targets {
            let Target::Refused(requested, expected) = target else {
                continue;
            };
            let (status, answered) = call(
                &router,
                "UpdateFileSystem",
                &json!({ "FileSystemId": id, "StorageCapacity": requested }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {:?} target {requested}: expected HTTP 400, got {status} with body \
                 {answered}",
                row.case
            );
            assert_eq!(
                answered["__type"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('#')
                    .next()
                    .unwrap_or_default(),
                "BadRequest",
                "case {:?} target {requested}: expected Error.Code BadRequest, got body \
                 {answered}",
                row.case
            );
            assert_eq!(
                answered["Message"].as_str().unwrap_or_default(),
                expected.as_str(),
                "case {:?} target {requested}: expected the documented Message, got {:?}",
                row.case,
                answered["Message"]
            );

            let after = describe_one(&router, &id).await;
            assert_eq!(
                after["StorageCapacity"], json!(row.capacity),
                "case {:?} target {requested}: a refused update must leave the stored \
                 StorageCapacity at {}, got {:?}",
                row.case, row.capacity, after["StorageCapacity"]
            );
            assert_eq!(
                after["AdministrativeActions"], Value::Null,
                "case {:?} target {requested}: a refused update must record no administrative \
                 action, got {:?}",
                row.case, after["AdministrativeActions"]
            );
            assert_eq!(
                after["Lifecycle"], json!("AVAILABLE"),
                "case {:?} target {requested}: a refused update must not move the lifecycle, \
                 got {:?}",
                row.case, after["Lifecycle"]
            );
        }
    }
}

/// RULE, over the whole table: an accepted target is carried out by ONE `FILE_SYSTEM_UPDATE`
/// administrative action — the response still reports the capacity the file system HAS, the
/// action's `TargetFileSystemValues.StorageCapacity` names the target, the lifecycle stays
/// `AVAILABLE`, and the reported capacity changes only once that action is `COMPLETED`.
#[tokio::test]
async fn an_accepted_target_is_carried_out_by_one_file_system_update_action() {
    let router = emulator().await;
    for row in rows() {
        let Some(Target::Accepted(requested)) = row
            .targets
            .iter()
            .find(|target| matches!(target, Target::Accepted(_)))
        else {
            continue;
        };
        let requested = *requested;
        let id = create(&router, &row).await;
        settled(&router, &id).await;

        let updated = ok(
            &router,
            "UpdateFileSystem",
            &json!({ "FileSystemId": id, "StorageCapacity": requested }),
        )
        .await;
        let file_system = updated["FileSystem"].clone();
        assert_eq!(
            file_system["StorageCapacity"], json!(row.capacity),
            "case {:?}: the response must still report the capacity the file system has \
             ({}), got {:?}",
            row.case, row.capacity, file_system["StorageCapacity"]
        );
        assert_eq!(
            file_system["Lifecycle"], json!("AVAILABLE"),
            "case {:?}: a capacity change is performed in place, so the lifecycle must stay \
             AVAILABLE, got {:?}",
            row.case, file_system["Lifecycle"]
        );
        let recorded = actions(&file_system);
        assert_eq!(
            recorded.len(),
            1,
            "case {:?}: expected exactly one administrative action, got {recorded:?}",
            row.case
        );
        assert_eq!(
            recorded[0]["AdministrativeActionType"], json!(FILE_SYSTEM_UPDATE),
            "case {:?}: expected AdministrativeActionType {FILE_SYSTEM_UPDATE}, got {:?}",
            row.case, recorded[0]["AdministrativeActionType"]
        );
        assert_eq!(
            recorded[0]["Status"], json!(PENDING),
            "case {:?}: expected the action to be {PENDING} while the clock still owes it, \
             got {:?}",
            row.case, recorded[0]["Status"]
        );
        assert_eq!(
            recorded[0]["TargetFileSystemValues"]["StorageCapacity"], json!(requested),
            "case {:?}: expected TargetFileSystemValues.StorageCapacity {requested}, got {:?}",
            row.case, recorded[0]["TargetFileSystemValues"]
        );
        assert!(
            recorded[0]["RequestTime"].as_f64().unwrap_or_default() > 0.0,
            "case {:?}: expected a RequestTime on the action, got {:?}",
            row.case,
            recorded[0]["RequestTime"]
        );

        // "You can't make a storage capacity increase request if there is an existing storage
        // capacity increase request in progress."
        let second = requested * 2;
        let (status, answered) = call(
            &router,
            "UpdateFileSystem",
            &json!({ "FileSystemId": id, "StorageCapacity": second }),
        )
        .await;
        assert_eq!(
            (status, answered["Message"].as_str().unwrap_or_default()),
            (400, in_progress_message(second).as_str()),
            "case {:?}: a second capacity target while one is in flight must be refused, got \
             {status} with body {answered}",
            row.case
        );

        // And a read while the action is still owed reports the same thing the update did.
        let polled = describe_one(&router, &id).await;
        assert_eq!(
            polled["StorageCapacity"], json!(row.capacity),
            "case {:?}: a read before the action completes must report the unchanged \
             capacity, got {:?}",
            row.case, polled["StorageCapacity"]
        );

        // Once the clock has made the action due, it is COMPLETED and the capacity is the
        // target it carried.
        let done = settled(&router, &id).await;
        assert_eq!(
            done["StorageCapacity"], json!(requested),
            "case {:?}: expected StorageCapacity {requested} once the action completed, got \
             {:?}",
            row.case, done["StorageCapacity"]
        );
        assert_eq!(
            done["Lifecycle"], json!("AVAILABLE"),
            "case {:?}: the file system must still be AVAILABLE, got {:?}",
            row.case, done["Lifecycle"]
        );
        let settled_actions = actions(&done);
        assert_eq!(
            settled_actions.len(),
            1,
            "case {:?}: the completed action must still be reported, got {settled_actions:?}",
            row.case
        );
        assert_eq!(
            settled_actions[0]["Status"], json!(COMPLETED),
            "case {:?}: expected the action to reach {COMPLETED}, got {:?}",
            row.case, settled_actions[0]["Status"]
        );
        assert_eq!(
            settled_actions[0]["TargetFileSystemValues"]["StorageCapacity"], json!(requested),
            "case {:?}: the completed action must still carry the target it was given, got \
             {:?}",
            row.case, settled_actions[0]["TargetFileSystemValues"]
        );
    }
}

/// RULE: an update that carries no capacity target records no administrative action at all,
/// and one that carries another member moves the lifecycle to `UPDATING` the way a
/// customer-initiated update does — the two paths are independent.
#[tokio::test]
async fn an_update_that_carries_no_capacity_target_records_no_action() {
    let router = emulator().await;
    let row = Row {
        case: "OPENZFS SINGLE_AZ_1, 64 GiB",
        file_system_type: "OPENZFS",
        storage_type: "SSD",
        configuration_member: Some("OpenZFSConfiguration"),
        configuration: json!({ "DeploymentType": "SINGLE_AZ_1", "ThroughputCapacity": 64 }),
        capacity: 64,
        targets: Vec::new(),
    };
    let id = create(&router, &row).await;
    settled(&router, &id).await;

    // Rows: the member the update carries, which is not `StorageCapacity`.
    for member in [
        json!({ "NetworkType": "DUAL" }),
        json!({ "StorageType": "SSD" }),
        json!({ "OpenZFSConfiguration": { "ThroughputCapacity": 128 } }),
    ] {
        let mut body = json!({ "FileSystemId": id });
        for (key, value) in member.as_object().expect("the member is an object") {
            body[key.as_str()] = value.clone();
        }
        let updated = ok(&router, "UpdateFileSystem", &body).await;
        assert_eq!(
            updated["FileSystem"]["Lifecycle"], json!("UPDATING"),
            "case {member}: expected Lifecycle UPDATING for a customer-initiated update, got \
             {:?}",
            updated["FileSystem"]["Lifecycle"]
        );
        assert_eq!(
            updated["FileSystem"]["AdministrativeActions"], Value::Null,
            "case {member}: an update with no capacity target must record no administrative \
             action, got {:?}",
            updated["FileSystem"]["AdministrativeActions"]
        );
        assert_eq!(
            updated["FileSystem"]["StorageCapacity"], json!(row.capacity),
            "case {member}: the stored capacity must be untouched, got {:?}",
            updated["FileSystem"]["StorageCapacity"]
        );
        settled(&router, &id).await;
    }
}
