// SPDX-License-Identifier: CC0-1.0

//! `UpdateStorage` and the cluster operation it records, driven end to end over the
//! same `MultiServiceRouter` the emulator serves from.
//!
//! RULE under test: an `UpdateStorage` call against an ACTIVE cluster is ACCEPTED —
//! it records a cluster operation, reports the cluster as `UPDATING` with that
//! operation active, and the clock applies the change (and generates the cluster's
//! next `CurrentVersion`) once the update span has passed; a storage member the
//! request does not name keeps its stored value. A call the service refuses changes
//! nothing, and the resource named in the URI is resolved BEFORE anything in the
//! body is judged, so an ARN no cluster holds is a `NotFoundException` whatever
//! storage values the body carries.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what it
//! generalises over: which members the request names, each published bound of the
//! volume-size range and one past it, both `StorageMode` values, provisioned
//! throughput on and off, and how many updates a cluster has had.
//!
//! The harness (`Emulator`, `ClusterRequest`, `pct`, `sleep_past_activation`,
//! `sleep_past_update`) is the crate's shared `kafka_harness`; only the two routes
//! this suite needs are added here.

use serde_json::{json, Map, Value};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The wire values this suite expects, written out rather than imported from the
/// implementation, so an assertion cannot agree with a constant that is wrong.
///
/// The volume-size bounds are the `range` the model puts on
/// `EBSStorageInfo$VolumeSize` (`__integerMin1Max16384`), the storage mode is the
/// documented default of `StorageMode`, and the operation type and states are the
/// ones MSK reports for a storage update.
const MIN_VOLUME_SIZE_GB: i64 = 1;
const MAX_VOLUME_SIZE_GB: i64 = 16_384;
const DEFAULT_STORAGE_MODE: &str = "LOCAL";
const OPERATION_TYPE_UPDATE_STORAGE: &str = "UPDATE_STORAGE";
const OPERATION_STATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
const OPERATION_STATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// `UpdateStorage`: `PUT /v1/clusters/{ClusterArn}/storage`, per the operation's
/// `http` binding in the model.
async fn update_storage(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/storage", pct(arn)),
            Some(body),
        )
        .await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}`.
async fn describe_operation(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(arn)), None)
        .await
}

/// `DescribeCluster`, which must answer 200 for a cluster this suite created.
async fn describe_ok(emulator: &Emulator, case: &str, arn: &str) -> Reply {
    let reply = emulator.describe(arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// One row of the accepted-update table: how the cluster is created, and which
/// storage members the update names.
#[derive(Debug, Clone)]
struct UpdateCase {
    name: &'static str,
    /// The EBS volume size the cluster is created with; `None` omits
    /// `storageInfo` entirely, which is how a row covers an update of a cluster
    /// that carries no stored volume size.
    created_volume_size: Option<i64>,
    /// The `StorageMode` the cluster is created with; `None` leaves the request
    /// without one, so the documented default applies.
    created_storage_mode: Option<&'static str>,
    /// `VolumeSizeGB` the update asks for, if any.
    volume_size_gb: Option<i64>,
    /// `ProvisionedThroughput` the update asks for, as `(enabled, MiB/s)`, if any.
    throughput: Option<(bool, i64)>,
    /// `StorageMode` the update asks for, if any.
    storage_mode: Option<&'static str>,
}

impl UpdateCase {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            created_volume_size: None,
            created_storage_mode: None,
            volume_size_gb: None,
            throughput: None,
            storage_mode: None,
        }
    }

    /// The `CreateCluster` body for this row.
    fn create_body(&self) -> Value {
        let mut body = ClusterRequest::named(self.name).to_json();
        if let Some(volume_size) = self.created_volume_size {
            body["brokerNodeGroupInfo"]["storageInfo"] =
                json!({ "ebsStorageInfo": { "volumeSize": volume_size } });
        }
        if let Some(storage_mode) = self.created_storage_mode {
            body["storageMode"] = json!(storage_mode);
        }
        body
    }

    /// The `UpdateStorage` body for this row, updating from `current_version`.
    fn update_body(&self, current_version: &str) -> Value {
        let mut body = Map::new();
        body.insert("currentVersion".to_string(), json!(current_version));
        if let Some(volume_size) = self.volume_size_gb {
            body.insert("volumeSizeGB".to_string(), json!(volume_size));
        }
        if let Some((enabled, throughput)) = self.throughput {
            body.insert(
                "provisionedThroughput".to_string(),
                json!({ "enabled": enabled, "volumeThroughput": throughput }),
            );
        }
        if let Some(storage_mode) = self.storage_mode {
            body.insert("storageMode".to_string(), json!(storage_mode));
        }
        Value::Object(body)
    }

    /// The volume size the cluster must report once the update has landed: the
    /// requested one, or the stored one when the request named none.
    fn expected_volume_size(&self) -> Option<i64> {
        self.volume_size_gb.or(self.created_volume_size)
    }

    /// The storage mode it must report, by the same rule.
    fn expected_storage_mode(&self) -> &'static str {
        self.storage_mode
            .or(self.created_storage_mode)
            .unwrap_or(DEFAULT_STORAGE_MODE)
    }
}

/// Every row of the accepted-update table.
///
/// The rows differ along which members the request names (size alone, mode alone,
/// throughput alone, every combination), the volume size (both published bounds of
/// `EBSStorageInfo$VolumeSize`'s 1..16384 range and a value between them), the
/// cluster's starting storage (absent, small, large, `TIERED`), both `StorageMode`
/// values, and throughput on and off.
fn accepted_rows() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            // The low published bound of the volume-size range, on a cluster
            // that carries no stored size at all.
            volume_size_gb: Some(MIN_VOLUME_SIZE_GB),
            ..UpdateCase::new("size-at-min")
        },
        UpdateCase {
            // The high published bound, growing from a small volume.
            created_volume_size: Some(100),
            volume_size_gb: Some(MAX_VOLUME_SIZE_GB),
            ..UpdateCase::new("size-at-max")
        },
        UpdateCase {
            created_volume_size: Some(1000),
            volume_size_gb: Some(2000),
            ..UpdateCase::new("size-grown")
        },
        UpdateCase {
            // Mode only: the stored volume size must survive untouched.
            created_volume_size: Some(512),
            storage_mode: Some("TIERED"),
            ..UpdateCase::new("mode-only-tiered")
        },
        UpdateCase {
            // Size only on a TIERED cluster: the mode must survive untouched.
            created_volume_size: Some(300),
            created_storage_mode: Some("TIERED"),
            volume_size_gb: Some(600),
            ..UpdateCase::new("size-only-keeps-mode")
        },
        UpdateCase {
            // Throughput only, named without a size or a mode.
            created_volume_size: Some(1000),
            throughput: Some((true, 250)),
            ..UpdateCase::new("throughput-only-on")
        },
        UpdateCase {
            created_volume_size: Some(1000),
            throughput: Some((false, 0)),
            ..UpdateCase::new("throughput-only-off")
        },
        UpdateCase {
            // Every member at once.
            created_volume_size: Some(1000),
            volume_size_gb: Some(4000),
            throughput: Some((true, 500)),
            storage_mode: Some("TIERED"),
            ..UpdateCase::new("every-member")
        },
    ]
}

/// RULE: an accepted `UpdateStorage` records an operation and reports the cluster as
/// `UPDATING` with that operation active and its PREVIOUS storage settings; once the
/// clock's update span has passed, the cluster reports exactly the submitted
/// settings — a member the request did not name keeping its stored value — at a new
/// `CurrentVersion`, and the operation reads back as complete.
#[tokio::test]
async fn an_accepted_storage_update_lands_on_the_cluster() {
    let rows = accepted_rows();
    let emulator = Emulator::new();

    // Create every row's cluster first, so the suite waits out ONE activation
    // span rather than one per row.
    let mut arns = Vec::new();
    for row in &rows {
        let reply = emulator
            .call("POST", "/v1/clusters", Some(row.create_body()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected CreateCluster to return 200, got {} {}",
            row.name, reply.status, reply.body
        );
        arns.push(
            reply.body["clusterArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {}: CreateCluster returned no clusterArn", row.name))
                .to_string(),
        );
    }
    sleep_past_activation();

    let mut started = Vec::new();
    for (row, arn) in rows.iter().zip(&arns) {
        let before = describe_ok(&emulator, row.name, arn).await;
        let version_before = before.at("/clusterInfo/currentVersion").clone();
        let size_before = before
            .at("/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/volumeSize")
            .clone();
        let mode_before = before.at("/clusterInfo/storageMode").clone();
        assert_eq!(
            before.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {}: the cluster must be ACTIVE before the update, got {}",
            row.name,
            before.body
        );

        let version = version_before
            .as_str()
            .unwrap_or_else(|| panic!("case {}: DescribeCluster reported no currentVersion", row.name))
            .to_string();
        let updated = update_storage(&emulator, arn, row.update_body(&version)).await;
        assert_eq!(
            updated.status, 200,
            "case {}: expected UpdateStorage to return 200, got {} {}",
            row.name, updated.status, updated.body
        );
        assert_eq!(
            updated.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {}: UpdateStorage must answer the cluster's own ARN, got {}",
            row.name,
            updated.body
        );
        let operation_arn = updated.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: UpdateStorage returned no clusterOperationArn: {}",
                    row.name, updated.body
                )
            })
            .to_string();
        let expected_prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/");
        assert!(
            operation_arn.starts_with(&expected_prefix),
            "case {}: expected a cluster-operation ARN under {expected_prefix:?}, got {operation_arn:?}",
            row.name
        );

        // Mid-update: the cluster is UPDATING, the operation is active, and
        // nothing about its storage or version has moved yet.
        let during = describe_ok(&emulator, row.name, arn).await;
        assert_eq!(
            during.at("/clusterInfo/state").as_str(),
            Some("UPDATING"),
            "case {}: expected state UPDATING while the update runs, got {}",
            row.name,
            during.body
        );
        assert_eq!(
            during.at("/clusterInfo/activeOperationArn").as_str(),
            Some(operation_arn.as_str()),
            "case {}: expected the update to be the cluster's active operation, got {}",
            row.name,
            during.body
        );
        assert_eq!(
            during.at("/clusterInfo/currentVersion"),
            &version_before,
            "case {}: the version must not move before the change lands, got {}",
            row.name,
            during.body
        );
        assert_eq!(
            during.at("/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/volumeSize"),
            &size_before,
            "case {}: the volume size must not move before the change lands, got {}",
            row.name,
            during.body
        );
        assert_eq!(
            during.at("/clusterInfo/storageMode"),
            &mode_before,
            "case {}: the storage mode must not move before the change lands, got {}",
            row.name,
            during.body
        );

        let running = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            running.status, 200,
            "case {}: expected DescribeClusterOperation to return 200, got {} {}",
            row.name, running.status, running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/operationState").as_str(),
            Some(OPERATION_STATE_IN_PROGRESS),
            "case {}: expected a running operation, got {}",
            row.name,
            running.body
        );
        assert!(
            running.at("/clusterOperationInfo/endTime").is_null(),
            "case {}: a running operation has not ended, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/operationType").as_str(),
            Some(OPERATION_TYPE_UPDATE_STORAGE),
            "case {}: wrong operationType, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/clusterArn").as_str(),
            Some(arn.as_str()),
            "case {}: the operation must name its own cluster, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/operationArn").as_str(),
            Some(operation_arn.as_str()),
            "case {}: the operation must name its own ARN, got {}",
            row.name,
            running.body
        );
        // The source is where the cluster was, the target where the update takes
        // it, both reported per broker under the keyword ALL.
        assert_eq!(
            running.at("/clusterOperationInfo/sourceClusterInfo/storageMode"),
            &mode_before,
            "case {}: wrong sourceClusterInfo storageMode, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/sourceClusterInfo/brokerEBSVolumeInfo/0/volumeSizeGB"),
            &size_before,
            "case {}: wrong sourceClusterInfo volumeSizeGB, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running
                .at("/clusterOperationInfo/targetClusterInfo/brokerEBSVolumeInfo/0/kafkaBrokerNodeId")
                .as_str(),
            Some("ALL"),
            "case {}: a cluster-wide storage change is reported for broker ALL, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running
                .at("/clusterOperationInfo/targetClusterInfo/brokerEBSVolumeInfo/0/volumeSizeGB")
                .as_i64(),
            row.expected_volume_size(),
            "case {}: wrong targetClusterInfo volumeSizeGB, got {}",
            row.name,
            running.body
        );
        assert_eq!(
            running
                .at("/clusterOperationInfo/targetClusterInfo/storageMode")
                .as_str(),
            Some(row.expected_storage_mode()),
            "case {}: wrong targetClusterInfo storageMode, got {}",
            row.name,
            running.body
        );

        started.push((operation_arn, version));
    }

    sleep_past_update();

    for ((row, arn), (operation_arn, version_before)) in rows.iter().zip(&arns).zip(&started) {
        let after = describe_ok(&emulator, row.name, arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {}: expected the cluster to be ACTIVE again, got {}",
            row.name,
            after.body
        );
        assert!(
            after.at("/clusterInfo/activeOperationArn").is_null(),
            "case {}: a finished update is no longer active, got {}",
            row.name,
            after.body
        );
        assert_eq!(
            after
                .at("/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/volumeSize")
                .as_i64(),
            row.expected_volume_size(),
            "case {}: wrong persisted volumeSize, got {}",
            row.name,
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/storageMode").as_str(),
            Some(row.expected_storage_mode()),
            "case {}: wrong persisted storageMode, got {}",
            row.name,
            after.body
        );
        if let Some((enabled, throughput)) = row.throughput {
            let stored = after.at(
                "/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/provisionedThroughput",
            );
            assert_eq!(
                stored.get("enabled").and_then(Value::as_bool),
                Some(enabled),
                "case {}: wrong persisted provisionedThroughput, got {stored}",
                row.name
            );
            assert_eq!(
                stored.get("volumeThroughput").and_then(Value::as_i64),
                Some(throughput),
                "case {}: wrong persisted volumeThroughput, got {stored}",
                row.name
            );
        }
        let version_after = after.at("/clusterInfo/currentVersion").as_str().unwrap_or("");
        assert_ne!(
            version_after, version_before,
            "case {}: an applied change must generate a new version, got {version_after:?}",
            row.name
        );
        assert!(
            version_after.starts_with('K') && version_after.len() == 14,
            "case {}: expected an MSK version token, got {version_after:?}",
            row.name
        );

        let finished = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            finished.at("/clusterOperationInfo/operationState").as_str(),
            Some(OPERATION_STATE_COMPLETE),
            "case {}: expected a finished operation, got {}",
            row.name,
            finished.body
        );
        assert!(
            !finished.at("/clusterOperationInfo/endTime").is_null(),
            "case {}: a finished operation reports an endTime, got {}",
            row.name,
            finished.body
        );
        assert!(
            !finished.at("/clusterOperationInfo/creationTime").is_null(),
            "case {}: an operation reports its creationTime, got {}",
            row.name,
            finished.body
        );
    }
}

/// RULE: the resource named in the URI is resolved BEFORE the body, so an
/// `UpdateStorage` against a well-formed cluster ARN no live cluster holds is a 404
/// `NotFoundException` whatever storage values the body carries — while a value that
/// is not a cluster ARN at all is a 400 `BadRequestException`.
#[tokio::test]
async fn an_unknown_cluster_is_not_found_whatever_the_body_asks_for() {
    let emulator = Emulator::new();
    let present = emulator.create_ok(&ClusterRequest::named("present")).await;
    sleep_past_activation();
    let version = describe_ok(&emulator, "present", &present)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("DescribeCluster reports a currentVersion")
        .to_string();

    let absent =
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1");
    let other_region =
        format!("arn:aws:kafka:us-east-1:{ACCOUNT}:cluster/present/11111111-2222-4333-8444-555555555555-1");

    // Rows: every form `VolumeSizeGB` can take — absent, each published bound,
    // one past each bound, zero, negative — plus a body that names only the
    // storage mode. None of them changes the answer for an ARN no cluster holds.
    let volume_forms: Vec<Option<i64>> = vec![
        None,
        Some(MIN_VOLUME_SIZE_GB),
        Some(MAX_VOLUME_SIZE_GB),
        Some(MIN_VOLUME_SIZE_GB - 1),
        Some(MAX_VOLUME_SIZE_GB + 1),
        Some(-4096),
        Some(0),
    ];
    for arn in [&absent, &other_region] {
        for form in &volume_forms {
            let mut body = Map::new();
            body.insert("currentVersion".to_string(), json!(version));
            if let Some(volume_size) = form {
                body.insert("volumeSizeGB".to_string(), json!(volume_size));
            } else {
                body.insert("storageMode".to_string(), json!("TIERED"));
            }
            let reply = update_storage(&emulator, arn, Value::Object(body)).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException"),
                "case arn={arn:?} volumeSizeGB={form:?}: expected 404 NotFoundException, got {} {}",
                reply.status,
                reply.body
            );
        }
    }

    // Rows: one per way the ARN grammar can fail, as the readers of this service
    // already separate a request it cannot parse from a resource it cannot find.
    let malformed = vec![
        "not-an-arn".to_string(),
        format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/present/abc"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc/extra"),
        format!("arn:aws:kafka::{ACCOUNT}:cluster/present/abc-1"),
    ];
    for arn in &malformed {
        let reply = update_storage(
            &emulator,
            arn,
            json!({ "currentVersion": version, "volumeSizeGB": 1000 }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case arn={arn:?}: expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The cluster that does exist is untouched by any of the refusals above.
    let described = describe_ok(&emulator, "present", &present).await;
    assert_eq!(
        described.at("/clusterInfo/currentVersion").as_str(),
        Some(version.as_str()),
        "a refused update must not move the cluster's version, got {}",
        described.body
    );
    assert_eq!(
        described.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "a refused update must not move the cluster's state, got {}",
        described.body
    );
}

/// RULE: a storage update the service refuses changes nothing — the cluster keeps its
/// state, its version and its stored storage settings, and no operation becomes
/// active on it.
#[tokio::test]
async fn a_refused_storage_update_changes_nothing() {
    let emulator = Emulator::new();
    let body = {
        let mut body = ClusterRequest::named("refusals").to_json();
        body["brokerNodeGroupInfo"]["storageInfo"] =
            json!({ "ebsStorageInfo": { "volumeSize": 1000 } });
        body
    };
    let reply = emulator.call("POST", "/v1/clusters", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "expected CreateCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    let arn = reply.body["clusterArn"]
        .as_str()
        .expect("CreateCluster returns a clusterArn")
        .to_string();

    // A cluster still being provisioned has no settled configuration to update
    // from, so the update is refused while it is CREATING.
    let creating = update_storage(
        &emulator,
        &arn,
        json!({ "currentVersion": "K0000000000000", "volumeSizeGB": 2000 }),
    )
    .await;
    assert_eq!(
        (creating.status, creating.error_code()),
        (400, "BadRequestException"),
        "case state=CREATING: expected 400 BadRequestException, got {} {}",
        creating.status,
        creating.body
    );

    sleep_past_activation();
    let version = describe_ok(&emulator, "refusals", &arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("DescribeCluster reports a currentVersion")
        .to_string();

    // Rows: each way a body can be wrong for a cluster that DOES exist — a
    // volume size past each published bound of the 1..16384 range, a size below
    // the stored 1000 GiB (storage can only grow), a stale version, and a body
    // that names no storage member at all.
    let rows: Vec<(&str, Value)> = vec![
        (
            "size-below-min",
            json!({ "currentVersion": version, "volumeSizeGB": MIN_VOLUME_SIZE_GB - 1 }),
        ),
        (
            "size-above-max",
            json!({ "currentVersion": version, "volumeSizeGB": MAX_VOLUME_SIZE_GB + 1 }),
        ),
        (
            "size-zero",
            json!({ "currentVersion": version, "volumeSizeGB": 0 }),
        ),
        (
            "size-negative",
            json!({ "currentVersion": version, "volumeSizeGB": -1000 }),
        ),
        (
            "size-decreased",
            json!({ "currentVersion": version, "volumeSizeGB": 999 }),
        ),
        (
            "stale-version",
            json!({ "currentVersion": "K0000000000000", "volumeSizeGB": 2000 }),
        ),
        (
            // Off-model `StorageMode`: only a value the model's enum names may
            // reach the stored record, so the request is refused whether the
            // boundary validator or the handler catches it.
            "storage-mode-off-model",
            json!({ "currentVersion": version, "storageMode": "BOGUS" }),
        ),
        ("no-change-named", json!({ "currentVersion": version })),
    ];

    for (case, body) in &rows {
        let reply = update_storage(&emulator, &arn, body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected the request to be refused with 400, got {} {}",
            reply.status, reply.body
        );
        let described = describe_ok(&emulator, case, &arn).await;
        assert_eq!(
            described.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: a refused update must not move the version, got {}",
            described.body
        );
        assert_eq!(
            described.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: a refused update must not move the state, got {}",
            described.body
        );
        assert!(
            described.at("/clusterInfo/activeOperationArn").is_null(),
            "case {case}: a refused update must not start an operation, got {}",
            described.body
        );
        assert_eq!(
            described
                .at("/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/volumeSize")
                .as_i64(),
            Some(1000),
            "case {case}: a refused update must not change the stored volume size, got {}",
            described.body
        );
    }

    // The same size the cluster already has is not a decrease, so it IS accepted:
    // the refusal above is about shrinking, not about repeating.
    let repeated = update_storage(
        &emulator,
        &arn,
        json!({ "currentVersion": version, "volumeSizeGB": 1000 }),
    )
    .await;
    assert_eq!(
        repeated.status, 200,
        "case size-unchanged: expected UpdateStorage to return 200, got {} {}",
        repeated.status, repeated.body
    );
}

/// RULE: every update gets its OWN operation record, each readable on its own ARN,
/// and the version the next update must quote is the one the previous update
/// generated. An operation is a resource of its cluster: it is unreachable once the
/// cluster is deleted, and from any other region.
#[tokio::test]
async fn each_update_records_its_own_operation() {
    let emulator = Emulator::new();
    let arn = emulator.create_ok(&ClusterRequest::named("history")).await;
    sleep_past_activation();

    // Rows: successive updates on one cluster, each quoting the version the
    // previous one generated and growing the volume.
    let sizes = [200, 400, 400, 16_384];
    let mut operations = Vec::new();
    for size in sizes {
        let version = describe_ok(&emulator, "history", &arn)
            .await
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case size={size}: DescribeCluster reported no version"))
            .to_string();
        let reply = update_storage(
            &emulator,
            &arn,
            json!({ "currentVersion": version, "volumeSizeGB": size }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case size={size}: expected UpdateStorage to return 200, got {} {}",
            reply.status, reply.body
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case size={size}: no clusterOperationArn: {}", reply.body))
            .to_string();
        operations.push((size, operation_arn));
        // The next row must quote the version THIS update generates, so wait for
        // the change the clock owes before asking for it.
        sleep_past_update();
    }

    let mut distinct: Vec<&String> = operations.iter().map(|(_, arn)| arn).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        operations.len(),
        "every update must record its own operation, got {operations:?}"
    );
    for (size, operation_arn) in &operations {
        let reply = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            reply.status, 200,
            "case size={size}: expected DescribeClusterOperation to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply
                .at("/clusterOperationInfo/targetClusterInfo/brokerEBSVolumeInfo/0/volumeSizeGB")
                .as_i64(),
            Some(*size),
            "case size={size}: the operation must report what it changed, got {}",
            reply.body
        );
        // Another region's endpoint serves none of this region's operations.
        let elsewhere = emulator
            .call_in(
                "eu-west-1",
                "GET",
                &format!("/v1/operations/{}", pct(operation_arn)),
                None,
            )
            .await;
        assert_eq!(
            (elsewhere.status, elsewhere.error_code()),
            (404, "NotFoundException"),
            "case size={size}: an operation of another region must be unreachable, got {} {}",
            elsewhere.status,
            elsewhere.body
        );
    }

    // Rows: a well-formed operation ARN nothing holds, and values that are not
    // operation ARNs at all.
    let unknown = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/history/11111111-2222-4333-8444-555555555555-1/99999999-8888-4777-8666-555555555555"
    );
    let reply = describe_operation(&emulator, &unknown).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (404, "NotFoundException"),
        "case arn={unknown:?}: expected 404 NotFoundException, got {} {}",
        reply.status,
        reply.body
    );
    for malformed in [
        "not-an-arn".to_string(),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/history/abc-1"),
        format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster-operation/history/abc-1/def"),
        format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/history/abc-1"),
    ] {
        let reply = describe_operation(&emulator, &malformed).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case arn={malformed:?}: expected 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
    }

    // An operation belongs to its cluster, so deleting the cluster takes its
    // history with it.
    let deleted = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&arn)), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCluster to return 200, got {} {}",
        deleted.status, deleted.body
    );
    for (size, operation_arn) in &operations {
        let reply = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "NotFoundException"),
            "case size={size}: a deleted cluster's operations are gone, got {} {}",
            reply.status,
            reply.body
        );
    }
}
