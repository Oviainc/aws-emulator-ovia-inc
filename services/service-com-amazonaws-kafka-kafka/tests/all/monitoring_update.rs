// SPDX-License-Identifier: CC0-1.0

//! `UpdateMonitoring`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: an update of a provisioned cluster's monitoring settings is
//! accepted only for a cluster this endpoint's region holds that is `ACTIVE` and
//! whose stored version is the one the request names, and it is applied
//! asynchronously — the call hands back a cluster-operation ARN and the cluster
//! enters `UPDATING` with that operation as its active one, keeping its old
//! settings and version; once the clock makes the operation due, each setting the
//! request named is the cluster's own (a submitted `OpenMonitoring` or
//! `LoggingInfo` REPLACES the cluster's, so a switch or destination it does not
//! turn on is off, and a setting the request omitted is kept), every reader
//! derived from them changes with it, and the cluster gets a new version and
//! returns to `ACTIVE`. A refused call changes nothing.
//!
//! The rows differ along what the rule generalises over: which of the three
//! settings the request carries (each alone, two together, all three), which
//! `EnhancedMonitoring` level it names, which Prometheus exporters it turns on
//! and off, which broker-log destination it delivers to, and the cluster it
//! starts from — whose own monitoring settings decide what the replacement
//! changes.

use serde_json::{json, Value};

// The service's own operation type and refusal wording, so the suite and the
// handlers cannot drift apart on what the service does or says.
use service_com_amazonaws_kafka_kafka::state::{
    CLUSTER_VERSION_NOT_CURRENT_MESSAGE, CURRENT_VERSION_MEMBER, DEFAULT_ENHANCED_MONITORING,
    OPERATION_TYPE_UPDATE_MONITORING, PROVISIONED_ONLY_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `UpdateMonitoring`, which the model binds to
/// `PUT /v1/clusters/{ClusterArn}/monitoring`, as an endpoint for `region` serves
/// it.
async fn update_in(emulator: &Emulator, region: &str, arn: &str, body: Value) -> Reply {
    emulator
        .call_in(
            region,
            "PUT",
            &format!("/v1/clusters/{}/monitoring", pct(arn)),
            Some(body),
        )
        .await
}

/// The same call, signed for the region the clusters below live in.
async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    update_in(emulator, REGION, arn, body).await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}`.
async fn operation(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(arn)), None)
        .await
}

/// `DescribeCluster`'s `clusterInfo` for `arn` — the reader that proves what the
/// service persisted.
async fn described(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let reply = emulator.describe(arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.at("/clusterInfo").clone()
}

/// A member of a described cluster, or `Value::Null` when it is absent.
fn member(info: &Value, pointer: &str) -> Value {
    info.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The `openMonitoring` every reader reports for a cluster whose JMX exporter is
/// `jmx` and whose node exporter is `node`: both switches are reported resolved,
/// so a caller never has to guess which default applied.
fn open_monitoring(jmx: bool, node: bool) -> Value {
    json!({
        "prometheus": {
            "jmxExporter": { "enabledInBroker": jmx },
            "nodeExporter": { "enabledInBroker": node }
        }
    })
}

/// One row of the monitoring table.
#[derive(Debug)]
struct Case {
    /// The cluster the update starts from — its `enhancedMonitoring` is part of
    /// the row, so a replacement can be seen moving a non-default level.
    created: ClusterRequest,
    /// The `openMonitoring` the CREATE submits, if the starting cluster runs
    /// exporters already.
    created_open: Option<Value>,
    /// The `loggingInfo` the CREATE submits, if the starting cluster delivers
    /// broker logs already.
    created_logging: Option<Value>,
    /// The `enhancedMonitoring` the update submits, if it carries one.
    enhanced: Option<&'static str>,
    /// The `openMonitoring` the update submits, if it carries one.
    open: Option<Value>,
    /// The `loggingInfo` the update submits, if it carries one.
    logging: Option<Value>,
    /// The `enhancedMonitoring` every reader must report once the update has
    /// landed.
    expected_enhanced: &'static str,
    /// The exporter switches every reader must report once it has landed, as
    /// `(JMX, node)`.
    expected_exporters: (bool, bool),
}

impl Case {
    fn name(&self) -> &str {
        &self.created.name
    }

    /// The create body: the row's `CreateCluster` request plus the monitoring
    /// members the harness's builder does not carry.
    fn create_body(&self) -> Value {
        let mut body = self.created.to_json();
        if let Some(open) = &self.created_open {
            body["openMonitoring"] = open.clone();
        }
        if let Some(logging) = &self.created_logging {
            body["loggingInfo"] = logging.clone();
        }
        body
    }

    /// The update body: the version the cluster is at, plus whichever members
    /// this row submits.
    fn body(&self, version: &str) -> Value {
        let mut body = json!({ "currentVersion": version });
        if let Some(level) = self.enhanced {
            body["enhancedMonitoring"] = json!(level);
        }
        if let Some(open) = &self.open {
            body["openMonitoring"] = open.clone();
        }
        if let Some(logging) = &self.logging {
            body["loggingInfo"] = logging.clone();
        }
        body
    }

    /// The `loggingInfo` every reader must report once the update has landed: a
    /// submitted one REPLACES the cluster's, an omitted one keeps it, and a
    /// cluster that delivers no broker logs reports no member at all.
    fn expected_logging(&self) -> Value {
        self.logging
            .clone()
            .or_else(|| self.created_logging.clone())
            .unwrap_or(Value::Null)
    }
}

/// The table: one row per shape a monitoring update can take, each on a cluster
/// whose own settings decide what the update changes.
///
/// The `enhancedMonitoring` levels walk the model's `EnhancedMonitoring` enum
/// (`smithy_query(query="shape",shape="EnhancedMonitoring")`: DEFAULT,
/// PER_BROKER, PER_TOPIC_PER_BROKER, PER_TOPIC_PER_PARTITION), and the
/// broker-log destinations walk the `BrokerLogs` members (`cloudWatchLogs`,
/// `firehose`, `s3`).
fn cases() -> Vec<Case> {
    vec![
        // The metrics level ALONE, moved off the default.
        Case {
            created: ClusterRequest::named("per-broker"),
            created_open: None,
            created_logging: None,
            enhanced: Some("PER_BROKER"),
            open: None,
            logging: None,
            expected_enhanced: "PER_BROKER",
            expected_exporters: (false, false),
        },
        // Another level of the same enum, from a cluster already at one.
        Case {
            created: ClusterRequest {
                enhanced_monitoring: Some("PER_BROKER"),
                ..ClusterRequest::named("per-topic-per-partition")
            },
            created_open: None,
            created_logging: None,
            enhanced: Some("PER_TOPIC_PER_PARTITION"),
            open: None,
            logging: None,
            expected_enhanced: "PER_TOPIC_PER_PARTITION",
            expected_exporters: (false, false),
        },
        // …and back to the baseline the enum names, which is an update like any
        // other rather than a no-op.
        Case {
            created: ClusterRequest {
                enhanced_monitoring: Some("PER_TOPIC_PER_BROKER"),
                ..ClusterRequest::named("back-to-default")
            },
            created_open: None,
            created_logging: None,
            enhanced: Some(DEFAULT_ENHANCED_MONITORING),
            open: None,
            logging: None,
            expected_enhanced: DEFAULT_ENHANCED_MONITORING,
            expected_exporters: (false, false),
        },
        // Open monitoring ALONE: both Prometheus exporters turned on.
        Case {
            created: ClusterRequest::named("exporters-on"),
            created_open: None,
            created_logging: None,
            enhanced: None,
            open: Some(json!({
                "prometheus": {
                    "jmxExporter": { "enabledInBroker": true },
                    "nodeExporter": { "enabledInBroker": true }
                }
            })),
            logging: None,
            expected_enhanced: DEFAULT_ENHANCED_MONITORING,
            expected_exporters: (true, true),
        },
        // One exporter named, the other left out of the shape: an exporter
        // nobody asked for is off.
        Case {
            created: ClusterRequest::named("jmx-only"),
            created_open: None,
            created_logging: None,
            enhanced: None,
            open: Some(json!({ "prometheus": { "jmxExporter": { "enabledInBroker": true } } })),
            logging: None,
            expected_enhanced: DEFAULT_ENHANCED_MONITORING,
            expected_exporters: (true, false),
        },
        // The REPLACEMENT rule: a cluster running both exporters, updated with a
        // shape that names only the node one, ends up running only that one.
        Case {
            created: ClusterRequest::named("exporters-replaced"),
            created_open: Some(json!({
                "prometheus": {
                    "jmxExporter": { "enabledInBroker": true },
                    "nodeExporter": { "enabledInBroker": true }
                }
            })),
            created_logging: None,
            enhanced: None,
            open: Some(json!({ "prometheus": { "nodeExporter": { "enabledInBroker": true } } })),
            logging: None,
            expected_enhanced: DEFAULT_ENHANCED_MONITORING,
            expected_exporters: (false, true),
        },
        // Broker log delivery ALONE, to CloudWatch Logs.
        Case {
            created: ClusterRequest::named("logging-cloudwatch"),
            created_open: None,
            created_logging: None,
            enhanced: None,
            open: None,
            logging: Some(json!({
                "brokerLogs": {
                    "cloudWatchLogs": { "enabled": true, "logGroup": "/aws/msk/brokers" }
                }
            })),
            expected_enhanced: DEFAULT_ENHANCED_MONITORING,
            expected_exporters: (false, false),
        },
        // Another destination, replacing the one the cluster was created with.
        Case {
            created: ClusterRequest::named("logging-replaced"),
            created_open: None,
            created_logging: Some(json!({
                "brokerLogs": {
                    "cloudWatchLogs": { "enabled": true, "logGroup": "/aws/msk/first" }
                }
            })),
            enhanced: None,
            open: None,
            logging: Some(json!({
                "brokerLogs": {
                    "s3": { "enabled": true, "bucket": "broker-logs", "prefix": "msk/" }
                }
            })),
            expected_enhanced: DEFAULT_ENHANCED_MONITORING,
            expected_exporters: (false, false),
        },
        // Every setting at once, on a cluster that already runs one exporter and
        // delivers logs somewhere else.
        Case {
            created: ClusterRequest {
                enhanced_monitoring: Some("PER_BROKER"),
                ..ClusterRequest::named("all-three")
            },
            created_open: Some(json!({
                "prometheus": { "jmxExporter": { "enabledInBroker": true } }
            })),
            created_logging: Some(json!({
                "brokerLogs": { "s3": { "enabled": true, "bucket": "old-logs" } }
            })),
            enhanced: Some("PER_TOPIC_PER_BROKER"),
            open: Some(json!({
                "prometheus": { "nodeExporter": { "enabledInBroker": true } }
            })),
            logging: Some(json!({
                "brokerLogs": {
                    "firehose": { "enabled": true, "deliveryStream": "msk-broker-logs" }
                }
            })),
            expected_enhanced: "PER_TOPIC_PER_BROKER",
            expected_exporters: (false, true),
        },
        // One setting named leaves the OTHER two exactly as they were.
        Case {
            created: ClusterRequest {
                enhanced_monitoring: Some("PER_TOPIC_PER_PARTITION"),
                ..ClusterRequest::named("logging-only")
            },
            created_open: Some(json!({
                "prometheus": { "jmxExporter": { "enabledInBroker": true } }
            })),
            created_logging: None,
            enhanced: None,
            open: None,
            logging: Some(json!({
                "brokerLogs": { "cloudWatchLogs": { "enabled": false } }
            })),
            expected_enhanced: "PER_TOPIC_PER_PARTITION",
            expected_exporters: (true, false),
        },
    ]
}

/// RULE: an accepted monitoring update puts the cluster into `UPDATING` with the
/// returned operation as its active one and changes nothing else yet; once the
/// clock makes that operation due, each submitted setting is the cluster's own
/// and each omitted one is kept, both cluster readers follow, and the cluster is
/// `ACTIVE` at a new version.
#[tokio::test]
async fn a_monitoring_update_lands_once_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    let cases = cases();

    // PHASE 1 — every row's starting cluster, settled to ACTIVE.
    let mut arns = Vec::new();
    for case in &cases {
        let reply = emulator
            .call("POST", "/v1/clusters", Some(case.create_body()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected CreateCluster to return 200, got {} {}",
            case.name(),
            reply.status,
            reply.body
        );
        arns.push(
            reply.body["clusterArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: CreateCluster returned no clusterArn, got {}",
                        case.name(),
                        reply.body
                    )
                })
                .to_string(),
        );
    }
    sleep_past_activation();

    // PHASE 2 — start each update and read the cluster back IMMEDIATELY, while
    // its operation is still in flight.
    let mut started = Vec::new();
    for (case, arn) in cases.iter().zip(&arns) {
        let before = described(&emulator, case.name(), arn).await;
        assert_eq!(
            member(&before, "/state"),
            json!("ACTIVE"),
            "case {}: a cluster must be ACTIVE before it is updated, got {}",
            case.name(),
            member(&before, "/state")
        );
        let version = member(&before, "/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let monitoring_before = (
            member(&before, "/enhancedMonitoring"),
            member(&before, "/openMonitoring"),
            member(&before, "/loggingInfo"),
        );

        let reply = update(&emulator, arn, case.body(&version)).await;
        let mid = described(&emulator, case.name(), arn).await;

        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateMonitoring to return 200, got {} {}",
            case.name(),
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {}: expected the cluster's own ARN back, got {}",
            case.name(),
            reply.body["clusterArn"]
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: UpdateMonitoring returned no clusterOperationArn, got {}",
                    case.name(),
                    reply.body
                )
            })
            .to_string();
        let expected_prefix =
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{}/", case.name());
        assert!(
            operation_arn.starts_with(&expected_prefix),
            "case {}: expected a cluster-operation ARN starting {expected_prefix:?}, got {operation_arn:?}",
            case.name()
        );

        // In flight: the cluster is UPDATING, the operation is its active one,
        // and NOTHING it reports has changed yet.
        assert_eq!(
            member(&mid, "/state"),
            json!("UPDATING"),
            "case {}: expected the cluster to be UPDATING while its operation runs, got {}",
            case.name(),
            member(&mid, "/state")
        );
        assert_eq!(
            member(&mid, "/activeOperationArn"),
            json!(operation_arn.clone()),
            "case {}: expected activeOperationArn {operation_arn:?}, got {}",
            case.name(),
            member(&mid, "/activeOperationArn")
        );
        assert_eq!(
            member(&mid, "/currentVersion"),
            json!(version.clone()),
            "case {}: an in-flight update must not change the cluster version, got {}",
            case.name(),
            member(&mid, "/currentVersion")
        );
        for (pointer, expected) in [
            ("/enhancedMonitoring", monitoring_before.0.clone()),
            ("/openMonitoring", monitoring_before.1.clone()),
            ("/loggingInfo", monitoring_before.2.clone()),
        ] {
            assert_eq!(
                member(&mid, pointer),
                expected,
                "case {}: an in-flight update must leave {pointer} at {expected}, got {}",
                case.name(),
                member(&mid, pointer)
            );
        }

        // The operation record describes what it changes: its own type, the
        // monitoring settings as they are now, and the same settings as the
        // update leaves them.
        let in_flight = operation(&emulator, &operation_arn).await;
        assert_eq!(
            in_flight.status, 200,
            "case {}: expected DescribeClusterOperation to return 200, got {} {}",
            case.name(),
            in_flight.status,
            in_flight.body
        );
        for (pointer, expected) in [
            (
                "/clusterOperationInfo/operationType",
                json!(OPERATION_TYPE_UPDATE_MONITORING),
            ),
            (
                "/clusterOperationInfo/sourceClusterInfo/enhancedMonitoring",
                monitoring_before.0.clone(),
            ),
            (
                "/clusterOperationInfo/sourceClusterInfo/openMonitoring",
                monitoring_before.1.clone(),
            ),
            (
                "/clusterOperationInfo/targetClusterInfo/enhancedMonitoring",
                json!(case.expected_enhanced),
            ),
            (
                "/clusterOperationInfo/targetClusterInfo/openMonitoring",
                open_monitoring(case.expected_exporters.0, case.expected_exporters.1),
            ),
            (
                "/clusterOperationInfo/targetClusterInfo/loggingInfo",
                case.expected_logging(),
            ),
        ] {
            assert_eq!(
                member(&in_flight.body, pointer),
                expected,
                "case {}: expected the operation's {pointer} to be {expected}, got {}",
                case.name(),
                member(&in_flight.body, pointer)
            );
        }

        started.push((version, monitoring_before, operation_arn));
    }

    // Each update is its own operation, so no two share an ARN.
    let mut operations: Vec<&str> = started
        .iter()
        .map(|(_, _, operation_arn)| operation_arn.as_str())
        .collect();
    operations.sort_unstable();
    operations.dedup();
    assert_eq!(
        operations.len(),
        started.len(),
        "every update must mint its own cluster-operation ARN, got {operations:?}"
    );

    // PHASE 3 — once the clock owes the completion, the update is the cluster's.
    sleep_past_update();
    for ((case, arn), (version, _, operation_arn)) in cases.iter().zip(&arns).zip(&started) {
        let after = described(&emulator, case.name(), arn).await;
        assert_eq!(
            member(&after, "/state"),
            json!("ACTIVE"),
            "case {}: expected the cluster back to ACTIVE, got {}",
            case.name(),
            member(&after, "/state")
        );
        assert_eq!(
            member(&after, "/activeOperationArn"),
            Value::Null,
            "case {}: a finished operation must not stay active, got {}",
            case.name(),
            member(&after, "/activeOperationArn")
        );
        let new_version = member(&after, "/currentVersion");
        assert_ne!(
            new_version,
            json!(version.clone()),
            "case {}: a successful update must generate a new cluster version, got {new_version}",
            case.name()
        );

        // The submitted settings are now the cluster's own, and the ones the
        // request left out are exactly as they were.
        let expected_open = open_monitoring(case.expected_exporters.0, case.expected_exporters.1);
        for (pointer, expected) in [
            ("/enhancedMonitoring", json!(case.expected_enhanced)),
            ("/openMonitoring", expected_open.clone()),
            ("/loggingInfo", case.expected_logging()),
        ] {
            assert_eq!(
                member(&after, pointer),
                expected,
                "case {}: expected {pointer} to be {expected} after the update, got {}",
                case.name(),
                member(&after, pointer)
            );
        }

        // The V2 reader derives its answer from the same record, so it cannot
        // disagree about what the cluster now monitors.
        let v2 = emulator.describe_v2(arn).await;
        assert_eq!(
            v2.status, 200,
            "case {}: expected DescribeClusterV2 to return 200, got {} {}",
            case.name(),
            v2.status,
            v2.body
        );
        for (pointer, expected) in [
            (
                "/clusterInfo/provisioned/enhancedMonitoring",
                json!(case.expected_enhanced),
            ),
            ("/clusterInfo/provisioned/openMonitoring", expected_open),
            (
                "/clusterInfo/provisioned/loggingInfo",
                case.expected_logging(),
            ),
        ] {
            assert_eq!(
                member(&v2.body, pointer),
                expected,
                "case {}: expected the V2 reader's {pointer} to be {expected}, got {}",
                case.name(),
                member(&v2.body, pointer)
            );
        }

        // The operation the call minted is finished, and still describes the
        // change it applied.
        let finished = operation(&emulator, operation_arn).await;
        assert_eq!(
            member(&finished.body, "/clusterOperationInfo/operationState"),
            json!("UPDATE_COMPLETE"),
            "case {}: expected the operation to be complete, got {}",
            case.name(),
            member(&finished.body, "/clusterOperationInfo/operationState")
        );

        // The version rotated, so the version the update was issued against is
        // now stale and a repeat of the same call is refused.
        let repeat = update(&emulator, arn, case.body(version)).await;
        assert_eq!(
            (repeat.status, repeat.error_code()),
            (400, "BadRequestException"),
            "case {}: expected a stale currentVersion to be refused, got {} {}",
            case.name(),
            repeat.status,
            repeat.body
        );
    }
}

/// RULE: a monitoring update is refused — changing nothing — for a `ClusterArn`
/// this endpoint cannot parse (`BadRequestException`), for a well-formed one its
/// region holds no cluster for (`NotFoundException`), for a cluster that is not
/// `ACTIVE`, for a `CurrentVersion` that is not the one the cluster is at, and
/// for a request that names none of the three things the operation can change.
#[tokio::test]
async fn update_monitoring_refuses_what_it_cannot_apply() {
    let emulator = Emulator::new();
    let arn = emulator.create_ok(&ClusterRequest::named("refusals")).await;
    let monitoring = json!("PER_BROKER");

    // A cluster still being provisioned has no settled configuration to update,
    // whatever version the caller names.
    let creating = described(&emulator, "creating", &arn).await;
    assert_eq!(
        member(&creating, "/state"),
        json!("CREATING"),
        "case creating: expected a freshly created cluster to be CREATING, got {}",
        member(&creating, "/state")
    );
    let creating_version = member(&creating, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let reply = update(
        &emulator,
        &arn,
        json!({ "currentVersion": creating_version, "enhancedMonitoring": monitoring }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case creating: expected an update of a CREATING cluster to be refused, got {} {}",
        reply.status,
        reply.body
    );

    sleep_past_activation();
    let settled = described(&emulator, "settled", &arn).await;
    let version = member(&settled, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: every way the request can fail, as `(case, ARN, signing region,
    // body, status, error code)`. The ARN rows walk the grammar the service
    // parses — not an ARN, the wrong service, the wrong resource type, an extra
    // segment, an empty region — then a well-formed ARN no cluster holds, and
    // this cluster's own ARN seen from another region's endpoint.
    let rows: Vec<(&str, String, &str, Value, u16, &str)> = vec![
        (
            "not-an-arn",
            "kafka-cluster".to_string(),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "wrong-service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/refusals/abc-1"),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "wrong-resource-type",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/refusals/abc"),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "extra-segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/refusals/abc/extra"),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "empty-region",
            format!("arn:aws:kafka::{ACCOUNT}:cluster/refusals/abc-1"),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "absent-cluster",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
            ),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            404,
            "NotFoundException",
        ),
        (
            "another-region",
            arn.clone(),
            "us-east-1",
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            404,
            "NotFoundException",
        ),
        (
            "another-account",
            format!(
                "arn:aws:kafka:{REGION}:999988887777:cluster/other/11111111-2222-4333-8444-555555555555-1"
            ),
            REGION,
            json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
            403,
            "AccessDeniedException",
        ),
        (
            "stale-version",
            arn.clone(),
            REGION,
            json!({ "currentVersion": "K3AFVY5JQAABC", "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "empty-version",
            arn.clone(),
            REGION,
            json!({ "currentVersion": "", "enhancedMonitoring": monitoring }),
            400,
            "BadRequestException",
        ),
        (
            "nothing-to-change",
            arn.clone(),
            REGION,
            json!({ "currentVersion": version }),
            400,
            "BadRequestException",
        ),
    ];

    for (case, target, region, body, status, code) in &rows {
        let reply = update_in(&emulator, region, target, body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        // A refusal persists nothing: the cluster is still ACTIVE at the same
        // version, with no operation in flight and its original monitoring
        // settings.
        let unchanged = described(&emulator, case, &arn).await;
        for (pointer, expected) in [
            ("/state", json!("ACTIVE")),
            ("/currentVersion", json!(version.clone())),
            ("/activeOperationArn", Value::Null),
            (
                "/enhancedMonitoring",
                json!(DEFAULT_ENHANCED_MONITORING),
            ),
            ("/openMonitoring", open_monitoring(false, false)),
            ("/loggingInfo", Value::Null),
        ] {
            assert_eq!(
                member(&unchanged, pointer),
                expected,
                "case {case}: a refused update must leave {pointer} at {expected}, got {}",
                member(&unchanged, pointer)
            );
        }
    }

    // A level outside the model's `EnhancedMonitoring` enum is refused by the
    // boundary validator before the handler runs, so the code is whichever of the
    // two the framework answers — never a success, and never a 501.
    let unmodelled = update(
        &emulator,
        &arn,
        json!({ "currentVersion": version, "enhancedMonitoring": "PER_PARTITION" }),
    )
    .await;
    assert!(
        unmodelled.status == 400
            && matches!(
                unmodelled.error_code(),
                "BadRequestException" | "ValidationException"
            ),
        "case unmodelled-level: expected a 400 refusal, got {} {}",
        unmodelled.status,
        unmodelled.body
    );

    // A stale `CurrentVersion` is refused in the service's one sentence for that
    // condition, which names neither the quoted version nor the stored one.
    let stale = update(
        &emulator,
        &arn,
        json!({ "currentVersion": "K0000000000000", "enhancedMonitoring": monitoring }),
    )
    .await;
    assert_eq!(
        (
            stale.at("/invalidParameter").as_str(),
            stale.error_message()
        ),
        (
            Some(CURRENT_VERSION_MEMBER),
            CLUSTER_VERSION_NOT_CURRENT_MESSAGE
        ),
        "case stale-version: expected the service's stale-version refusal, got {}",
        stale.body
    );

    // The resource-level refusal carries the service's own wording for a
    // resource it does not hold.
    let absent = update_in(
        &emulator,
        REGION,
        &format!(
            "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
        ),
        json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
    )
    .await;
    assert_eq!(
        absent.error_message(),
        RESOURCE_NOT_FOUND_MESSAGE,
        "case absent-cluster: expected the service's not-found wording, got {}",
        absent.body
    );

    // A SERVERLESS cluster has no brokers to monitor, so the operation is
    // refused on its kind, in the service's own wording for that.
    let serverless = emulator
        .create_serverless_ok(
            "serverless-monitoring",
            &[vec!["subnet-a", "subnet-b"]],
            Some(true),
        )
        .await;
    sleep_past_activation();
    let refused = update(
        &emulator,
        &serverless,
        json!({ "currentVersion": "K3AFVY5JQAABC", "enhancedMonitoring": monitoring }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "BadRequestException"),
        "case serverless: expected a serverless cluster to be refused, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        PROVISIONED_ONLY_MESSAGE,
        "case serverless: expected the service's provisioned-only wording, got {}",
        refused.body
    );

    // A cluster with an update already in flight is not ACTIVE, so a second
    // update is refused — and the first one still lands.
    let accepted = update(
        &emulator,
        &arn,
        json!({ "currentVersion": version, "enhancedMonitoring": monitoring }),
    )
    .await;
    let second = update(
        &emulator,
        &arn,
        json!({ "currentVersion": version, "enhancedMonitoring": "PER_TOPIC_PER_BROKER" }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case in-flight: expected the first update to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        (second.status, second.error_code()),
        (400, "BadRequestException"),
        "case in-flight: expected a second update to be refused, got {} {}",
        second.status,
        second.body
    );

    sleep_past_update();
    let landed = described(&emulator, "in-flight", &arn).await;
    assert_eq!(
        member(&landed, "/enhancedMonitoring"),
        monitoring,
        "case in-flight: expected the accepted update to land, got {}",
        member(&landed, "/enhancedMonitoring")
    );
    assert_eq!(
        member(&landed, "/state"),
        json!("ACTIVE"),
        "case in-flight: expected the cluster back to ACTIVE, got {}",
        member(&landed, "/state")
    );
}

/// RULE: a request quoting a `CurrentVersion` that is not the version the
/// cluster is at is refused HTTP 400 `BadRequestException` naming
/// `currentVersion`, with the service's ONE sentence for that condition —
/// whichever operation quotes the version, and whatever the submitted and stored
/// versions are, since the sentence names neither and tells the caller to read
/// the current one back.
///
/// The rows are every operation of this service that quotes a cluster's version:
/// each cluster update, and the delete that takes it as a query parameter.
#[tokio::test]
async fn a_stale_cluster_version_gets_one_wording_from_every_operation() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_ok(&ClusterRequest::named("stale-wording"))
        .await;
    sleep_past_activation();
    let stored = member(&described(&emulator, "stale-wording", &arn).await, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Two stale quotes: a well-formed version token of another cluster's
    // numbering, and the stored one mangled — neither is this cluster's.
    let stale_quotes = [
        "K0000000000000".to_string(),
        format!("{}X", stored.to_lowercase()),
    ];

    for stale in &stale_quotes {
        // `(operation, method, URI, body)`. A body carries whatever the
        // operation needs to reach its version check at all, since an operation
        // that can change nothing is refused on that first.
        let calls: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "UpdateMonitoring",
                "PUT",
                format!("/v1/clusters/{}/monitoring", pct(&arn)),
                Some(json!({ "currentVersion": stale, "enhancedMonitoring": "PER_BROKER" })),
            ),
            (
                "UpdateSecurity",
                "PATCH",
                format!("/v1/clusters/{}/security", pct(&arn)),
                Some(json!({
                    "currentVersion": stale,
                    "clientAuthentication": { "unauthenticated": { "enabled": true } }
                })),
            ),
            (
                "UpdateConnectivity",
                "PUT",
                format!("/v1/clusters/{}/connectivity", pct(&arn)),
                Some(json!({
                    "currentVersion": stale,
                    "connectivityInfo": { "networkType": "DUAL" }
                })),
            ),
            (
                "UpdateStorage",
                "PUT",
                format!("/v1/clusters/{}/storage", pct(&arn)),
                Some(json!({ "currentVersion": stale, "volumeSizeGB": 2000 })),
            ),
            (
                "UpdateBrokerStorage",
                "PUT",
                format!("/v1/clusters/{}/nodes/storage", pct(&arn)),
                Some(json!({
                    "currentVersion": stale,
                    "targetBrokerEBSVolumeInfo": [
                        { "kafkaBrokerNodeId": "ALL", "volumeSizeGB": 2000 }
                    ]
                })),
            ),
            (
                "UpdateBrokerCount",
                "PUT",
                format!("/v1/clusters/{}/nodes/count", pct(&arn)),
                Some(json!({ "currentVersion": stale, "targetNumberOfBrokerNodes": 6 })),
            ),
            (
                "UpdateBrokerType",
                "PUT",
                format!("/v1/clusters/{}/nodes/type", pct(&arn)),
                Some(json!({ "currentVersion": stale, "targetInstanceType": "kafka.m5.xlarge" })),
            ),
            (
                "UpdateClusterConfiguration",
                "PUT",
                format!("/v1/clusters/{}/configuration", pct(&arn)),
                Some(json!({
                    "currentVersion": stale,
                    "configurationInfo": {
                        "arn": format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/cfg/abc-1"),
                        "revision": 1
                    }
                })),
            ),
            (
                "UpdateClusterKafkaVersion",
                "PUT",
                format!("/v1/clusters/{}/version", pct(&arn)),
                Some(json!({ "currentVersion": stale, "targetKafkaVersion": "3.7.x" })),
            ),
            (
                // The one operation that carries the version as a query
                // parameter rather than in a body.
                "DeleteCluster",
                "DELETE",
                format!("/v1/clusters/{}?currentVersion={}", pct(&arn), pct(stale)),
                None,
            ),
        ];

        for (operation, method, uri, body) in calls {
            let reply = emulator.call(method, &uri, body).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {operation}/{stale}: expected a 400 BadRequestException, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(CURRENT_VERSION_MEMBER),
                "case {operation}/{stale}: the refusal must name {CURRENT_VERSION_MEMBER}, got {}",
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                CLUSTER_VERSION_NOT_CURRENT_MESSAGE,
                "case {operation}/{stale}: expected the service's stale-version wording, got {:?}",
                reply.error_message()
            );
            // The sentence is the service's own: it carries neither the version
            // the caller quoted nor the one the cluster is at.
            assert!(
                !reply.error_message().contains(stale.as_str())
                    && !reply.error_message().contains(stored.as_str()),
                "case {operation}/{stale}: the refusal must leak no version, got {:?}",
                reply.error_message()
            );
        }
    }

    // Every one of those refusals changed nothing: the cluster is still there,
    // ACTIVE, at the version it was at.
    let after = described(&emulator, "stale-wording", &arn).await;
    for (pointer, expected) in [
        ("/state", json!("ACTIVE")),
        ("/currentVersion", json!(stored.clone())),
        ("/activeOperationArn", Value::Null),
    ] {
        assert_eq!(
            member(&after, pointer),
            expected,
            "case stale-wording: a refused call must leave {pointer} at {expected}, got {}",
            member(&after, pointer)
        );
    }
}
