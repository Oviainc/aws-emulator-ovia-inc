// SPDX-License-Identifier: CC0-1.0

//! Taking a point-in-time copy of ONE Lightsail instance —
//! `CreateInstanceSnapshot` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test, which the sibling read and listing suites do not pin:
//!
//! * the call is SERVED — never a 501 and never a hollow 200 — and answers
//!   exactly one operation-log entry for the one snapshot it created: the entry
//!   names this call's own operation type, the snapshot it created and the
//!   source's location, carries a minted id of its own, and is reported by
//!   `GetOperation` under that id;
//! * the entry is non-terminal when the call returns and the CLOCK, not a
//!   reader, settles it into `Succeeded` — at the same instant the snapshot it
//!   created becomes `available`, since one clock owes both;
//! * the caller's `tags` are kept verbatim — whatever their number up to the
//!   bound this service accepts, and whether or not a tag carries a value —
//!   while a list past that bound is refused with the modelled
//!   `InvalidInputException` (HTTP 400) before anything is written; and
//! * creating a snapshot is a mutation of the SNAPSHOT store only: the source
//!   instance is exactly as it was afterwards, however many snapshots are taken
//!   of it.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_TAGS;

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::disk_attachment::wait_for_instance_state;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The operation under test.
const OPERATION: &str = "CreateInstanceSnapshot";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One tag a request carries: a key and the value the modelled `Tag` shape
/// leaves optional.
type TagPair = (String, Option<String>);

/// The `tags` member a request carries, in the modelled `TagList` form — a tag
/// with no value carries its key alone, since `Tag$value` is optional.
///
/// The sibling `resource_tags::tag_list` builds the same JSON but over
/// `&'static str` rows, so it cannot carry the tags a case GENERATES at run time
/// to drive the list bound; this takes owned pairs instead.
fn tag_list(tags: &[TagPair]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect(),
    )
}

/// `count` distinct tags, the first of them carrying its key alone: the form of
/// a tag is not what the bound is about, so one generator serves every count.
fn generated_tags(count: usize) -> Vec<TagPair> {
    (0..count)
        .map(|index| {
            let key = format!("key-{index:02}");
            if index == 0 {
                (key, None)
            } else {
                (key, Some(format!("value-{index:02}")))
            }
        })
        .collect()
}

/// The stored instance snapshot `name`, as `GetInstanceSnapshot` reports it.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
        .await["instanceSnapshot"]
        .clone()
}

/// The stored instance `name`, as `GetInstance` reports it.
async fn instance(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
}

/// Every instance snapshot name the account holds, walking every page of
/// `GetInstanceSnapshots`.
async fn snapshot_names(emulator: &Emulator) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok("GetInstanceSnapshots", json!({"pageToken": token}))
                    .await
            }
            None => emulator.ok("GetInstanceSnapshots", json!({})).await,
        };
        let page = body["instanceSnapshots"]
            .as_array()
            .unwrap_or_else(|| panic!("case listing snapshots: expected an array, got {body:?}"))
            .clone();
        for entry in page {
            names.push(entry["name"].as_str().unwrap_or_default().to_string());
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            names.len() < 10_000,
            "case listing snapshots: the walk did not terminate"
        );
    }
    names.sort();
    names
}

/// The ONE operation-log entry a create answered, which is the only entry a
/// create of one snapshot may answer.
fn single_operation(case: &str, answered: &Value) -> Value {
    let operations = answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
        .clone();
    assert_eq!(
        operations.len(),
        1,
        "case {case}: one snapshot is created, so one operation is logged, got {operations:?}"
    );
    operations[0].clone()
}

/// Poll `GetOperation` until the entry `id` is terminal, answering it.
///
/// The terminal status is owed by the CLOCK, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_terminal_operation(emulator: &Emulator, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let operation = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        if operation["isTerminal"].as_bool() == Some(true) {
            return operation;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for operation {id} to settle: expected a terminal entry within 5s, got \
             {operation:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The create table
// ---------------------------------------------------------------------------

/// One `CreateInstanceSnapshot` call the service must serve.
///
/// The rows differ along everything the rule generalises over: the source's
/// blueprint (a Linux distribution, an application blueprint, Windows), its plan
/// — from the smallest published bundle through a 320 GB one, including a bundle
/// id carrying the platform token — the Availability Zone and so the region the
/// logged operation is located in, the region the CREATE is signed for (which
/// selects nothing, since a snapshot is located where its source is), the
/// lifecycle state the source is in when the snapshot is taken (running,
/// stopped), the number and form of the tags, and the form of the snapshot name.
struct Row {
    case: &'static str,
    instance_name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    /// A blueprint id this service publishes (`GetBlueprints`).
    blueprint_id: &'static str,
    /// A bundle id this service serves (`GetBundles`); its leading token names
    /// the plan's size.
    bundle_id: &'static str,
    snapshot_name: &'static str,
    /// The region the CREATE is signed for, when the row signs it for one.
    create_region: Option<&'static str>,
    /// Whether the source is stopped before the snapshot is taken: AWS
    /// snapshots a running and a stopped instance alike.
    stop_source_first: bool,
    /// How many tags the request carries, built by [`generated_tags`].
    tag_count: usize,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a running linux source on the smallest published bundle, no tags, create signed \
               for no region",
        instance_name: "create-linux-nano",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "nano_3_0",
        snapshot_name: "create-linux-nano-snap",
        create_region: None,
        stop_source_first: false,
        tag_count: 0,
    },
    Row {
        case: "an application blueprint on a small bundle, one key-only tag, create signed for \
               another region",
        instance_name: "create_wordpress_1",
        availability_zone: "eu-west-1b",
        region: "eu-west-1",
        blueprint_id: "wordpress",
        bundle_id: "small_3_0",
        snapshot_name: "create_wordpress_snap_1",
        create_region: Some("us-west-2"),
        stop_source_first: false,
        tag_count: 1,
    },
    Row {
        case: "windows on a bundle whose id carries the platform token, four tags",
        instance_name: "CreateWin2016",
        availability_zone: "ap-southeast-2a",
        region: "ap-southeast-2",
        blueprint_id: "windows_server_2016",
        bundle_id: "medium_win_3_0",
        snapshot_name: "CreateWin2016-2026-10-24",
        create_region: Some("ap-southeast-2"),
        stop_source_first: false,
        tag_count: 4,
    },
    Row {
        case: "a STOPPED debian source, two tags, shortest snapshot name the ResourceName \
               pattern admits",
        instance_name: "create-stopped-source",
        availability_zone: "us-west-2a",
        region: "us-west-2",
        blueprint_id: "debian_12",
        bundle_id: "micro_3_0",
        snapshot_name: "s1",
        create_region: None,
        stop_source_first: true,
        tag_count: 2,
    },
    Row {
        case: "alma linux on the largest bundle in the table, in a fifth region, three tags",
        instance_name: "create_alma_xlarge",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        blueprint_id: "almalinux_9",
        bundle_id: "xlarge_3_0",
        snapshot_name: "Create_Alma_Snapshot_5",
        create_region: Some("eu-central-1"),
        stop_source_first: false,
        tag_count: 3,
    },
];

/// Stand `row`'s source instance up, taking it to the state the row snapshots it
/// in.
async fn create_source(emulator: &Emulator, row: &Row) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [row.instance_name],
                "availabilityZone": row.availability_zone,
                "blueprintId": row.blueprint_id,
                "bundleId": row.bundle_id,
            }),
        )
        .await;
    wait_for_instance_state(emulator, row.instance_name, "running").await;
    if row.stop_source_first {
        emulator
            .ok("StopInstance", json!({"instanceName": row.instance_name}))
            .await;
        wait_for_instance_state(emulator, row.instance_name, "stopped").await;
    }
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a served `CreateInstanceSnapshot` answers exactly ONE operation-log
/// entry — this call's own operation type, against the snapshot it created, in
/// the source's location, under a minted id `GetOperation` reports it by — and
/// that entry is non-terminal when the call returns, settled into `Succeeded` by
/// the clock at the same instant the snapshot it created becomes `available`.
#[tokio::test]
async fn a_create_answers_one_operation_the_clock_settles() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_source(&emulator, row).await;
        let source = instance(&emulator, row.instance_name).await;

        let tags = generated_tags(row.tag_count);
        let mut body = json!({
            "instanceName": row.instance_name,
            "instanceSnapshotName": row.snapshot_name,
        });
        if row.tag_count > 0 {
            body["tags"] = tag_list(&tags);
        }
        // SERVED: a status of its own, never the 501 an unimplemented operation
        // answers.
        let (status, answered) = emulator
            .call_at(None, row.create_region, OPERATION, body)
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
            Some("InstanceSnapshot"),
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
            Some(row.region),
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

        // Non-terminal when the call returns: the clock owes the snapshot its
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
        // list, not an absent one, for a request that carried none.
        let stored = snapshot(&emulator, row.snapshot_name).await;
        assert_eq!(
            stored["tags"],
            tag_list(&tags),
            "case {case}: expected the tags the request carried, got {:?}",
            stored["tags"]
        );
        assert_eq!(
            stored["fromInstanceName"].as_str(),
            Some(row.instance_name),
            "case {case}: expected the snapshot to name its source, got {stored:?}"
        );

        // One clock settles both: once the entry is terminal the snapshot it
        // created is available.
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
            "case {case}: one clock owes the entry and the snapshot, so expected an available \
             snapshot once the entry settled, got {settled_snapshot:?}"
        );
    }
}

/// RULE: the caller's `tags` are kept verbatim for any list within the bound
/// this service accepts, and a list past that bound is refused with the modelled
/// `InvalidInputException` (HTTP 400) before anything is written.
///
/// The bound is tested at it, below it and past it: `crate::state::MAX_TAGS` is
/// the published Lightsail limit of tags per resource.
#[tokio::test]
async fn the_tag_list_bound_is_enforced_and_the_tags_are_kept() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["tagged-source"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "ubuntu_22_04",
                "bundleId": "nano_3_0",
            }),
        )
        .await;

    // Within the bound: none, one, a middling list, and exactly the bound.
    for count in [0_usize, 1, 7, MAX_TAGS] {
        let case = format!("{count} tags");
        let name = format!("tagged-snap-{count}");
        let tags = generated_tags(count);
        let mut body = json!({"instanceName": "tagged-source", "instanceSnapshotName": name});
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
    for count in [MAX_TAGS + 1, MAX_TAGS + 9] {
        let case = format!("{count} tags");
        let name = format!("refused-snap-{count}");
        let (status, body) = emulator
            .call(
                OPERATION,
                json!({
                    "instanceName": "tagged-source",
                    "instanceSnapshotName": name,
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
            .call("GetInstanceSnapshot", json!({"instanceSnapshotName": name}))
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

/// RULE: creating a snapshot writes to the SNAPSHOT store only — the source
/// instance is exactly as it was afterwards, however many snapshots are taken of
/// it, so nothing about taking a copy moves the source's state or its hardware.
#[tokio::test]
async fn a_create_leaves_its_source_untouched() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["untouched-source"],
                "availabilityZone": "eu-west-3a",
                "blueprintId": "nodejs",
                "bundleId": "large_3_0",
            }),
        )
        .await;
    // Taken to its own terminal state first, so a later difference is the
    // create's doing rather than the clock's.
    wait_for_instance_state(&emulator, "untouched-source", "running").await;
    let before = instance(&emulator, "untouched-source").await;
    let state_before = emulator
        .ok("GetInstanceState", json!({"instanceName": "untouched-source"}))
        .await;

    for index in 0..3 {
        let name = format!("untouched-snap-{index}");
        emulator
            .ok(
                OPERATION,
                json!({"instanceName": "untouched-source", "instanceSnapshotName": name}),
            )
            .await;
        let after = instance(&emulator, "untouched-source").await;
        assert_eq!(
            after, before,
            "case after taking snapshot {index}: expected the source instance to be untouched, \
             got {after:?}"
        );
        let state_after = emulator
            .ok("GetInstanceState", json!({"instanceName": "untouched-source"}))
            .await;
        assert_eq!(
            state_after, state_before,
            "case after taking snapshot {index}: expected the source's state to be untouched, got \
             {state_after:?}"
        );
    }

    // And each copy is its own record, so the three creates stored three
    // snapshots of the one source.
    let names = snapshot_names(&emulator).await;
    assert_eq!(
        names,
        vec![
            "untouched-snap-0".to_string(),
            "untouched-snap-1".to_string(),
            "untouched-snap-2".to_string(),
        ],
        "case after three creates: expected three stored snapshots, got {names:?}"
    );
}
