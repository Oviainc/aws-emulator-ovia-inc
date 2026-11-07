// SPDX-License-Identifier: CC0-1.0

//! What a cluster's `logging` configuration EXPORTS, as this service derives it.
//!
//! RULE: `LogSetup$enabled` is "If a log type is enabled, that log type exports its
//! control plane logs to CloudWatch Logs. If a log type isn't enabled, that log type
//! doesn't export its control plane logs. Each individual log type can be enabled or
//! disabled independently" — so a configuration decides, per log type and independently,
//! whether that type's control plane logs are exported, and the export of a cluster lands
//! in the cluster's own log group `/aws/eks/<cluster name>/cluster`, one log stream per
//! exporting type, named after the control plane component whose logs that type carries.
//! A configuration that enables NO type exports nothing, so no group and no stream exist
//! for it.
//!
//! This suite pins the three halves of that rule this service decides on its own — which
//! types a configuration enables, the group name a cluster's export goes to, and the
//! stream name each type exports under — and that a cluster served by an emulator with no
//! CloudWatch Logs mounted beside it is still created and updated (the export is a call
//! to another service, and nothing about a cluster write depends on it). The export
//! ITSELF, which only exists when CloudWatch Logs is mounted beside this service, is
//! pinned end to end in `core/router/tests/all/eks_control_plane_logs.rs`.
//!
//! The rows span what the rule generalises over: how many types one entry enables, how
//! many entries a configuration carries, a type named twice, a type named in both an
//! enabled and a disabled entry, every one of the five types `LogType` declares, and the
//! two configurations that enable nothing (an all-disabled one and one with no entry at
//! all).

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::ops::types::{
    LogSetup, Logging, LOG_TYPE_VALUES,
};
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_log_group_name, control_plane_log_stream_name, derive_cluster_resource_id,
    enabled_log_types, service_state,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, describe, REGION};
use crate::wire::{call, emulator_with_engine, Answer};

/// A role of the single account this emulator serves, which is what `iam:PassRole`
/// allows.
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// One `LogSetup` as a request writes it.
fn setup(types: &[&str], enabled: bool) -> LogSetup {
    LogSetup {
        types: Some(types.iter().map(|value| (*value).to_string()).collect()),
        enabled: Some(enabled),
    }
}

/// One `logging` member built out of `setups`.
fn logging(setups: Vec<LogSetup>) -> Logging {
    Logging {
        cluster_logging: Some(setups),
    }
}

/// One row of the enablement table: a `logging` configuration and the log types it
/// enables, in the order the model declares them.
struct EnablementRow {
    case: &'static str,
    logging: Logging,
    enabled: Vec<&'static str>,
}

/// The table. Every type named is a member of `LogType` (`api`, `audit`,
/// `authenticator`, `controllerManager`, `scheduler`), and the rows include types the
/// issue did not cite.
fn enablement_rows() -> Vec<EnablementRow> {
    vec![
        EnablementRow {
            case: "one entry enabling one type",
            logging: logging(vec![setup(&["api"], true)]),
            enabled: vec!["api"],
        },
        EnablementRow {
            case: "one entry enabling two types",
            logging: logging(vec![setup(&["api", "audit"], true)]),
            enabled: vec!["api", "audit"],
        },
        EnablementRow {
            case: "one entry enabling a type the issue did not cite",
            logging: logging(vec![setup(&["scheduler"], true)]),
            enabled: vec!["scheduler"],
        },
        EnablementRow {
            case: "two entries, each enabling its own type",
            logging: logging(vec![
                setup(&["authenticator"], true),
                setup(&["controllerManager"], true),
            ]),
            enabled: vec!["authenticator", "controllerManager"],
        },
        EnablementRow {
            case: "every type the model declares, enabled at once",
            logging: logging(vec![setup(LOG_TYPE_VALUES, true)]),
            enabled: LOG_TYPE_VALUES.to_vec(),
        },
        EnablementRow {
            case: "one type named twice in the same entry",
            logging: logging(vec![setup(&["audit", "audit"], true)]),
            enabled: vec!["audit"],
        },
        EnablementRow {
            case: "a type enabled by one entry and disabled by another",
            logging: logging(vec![setup(&["api"], true), setup(&["api"], false)]),
            enabled: vec!["api"],
        },
        EnablementRow {
            case: "one entry enabling, one entry disabling another type",
            logging: logging(vec![
                setup(&["api"], true),
                setup(&["audit", "scheduler"], false),
            ]),
            enabled: vec!["api"],
        },
        EnablementRow {
            case: "every type disabled",
            logging: logging(vec![setup(LOG_TYPE_VALUES, false)]),
            enabled: vec![],
        },
        EnablementRow {
            case: "an enabling entry that names no type",
            logging: logging(vec![setup(&[], true)]),
            enabled: vec![],
        },
        EnablementRow {
            case: "a configuration with no entry at all",
            logging: logging(vec![]),
            enabled: vec![],
        },
    ]
}

/// The `logging` of a row, as the JSON a request carries.
fn logging_json(configuration: &Logging) -> serde_json::Value {
    let setups: Vec<serde_json::Value> = configuration
        .cluster_logging
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|setup| {
            serde_json::json!({
                "types": setup.types.clone().unwrap_or_default(),
                "enabled": setup.enabled.unwrap_or(false),
            })
        })
        .collect();
    serde_json::json!({ "clusterLogging": setups })
}

/// A `CreateCluster` body, carrying `logging` when a row has one.
fn cluster_body(name: &str, configuration: Option<&Logging>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "roleArn": PASSABLE_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    if let Some(configuration) = configuration {
        body["logging"] = logging_json(configuration);
    }
    body
}

/// The types the cluster `described` reports as EXPORTING, read off the two `LogSetup`
/// groups a cluster reports (the enabled set first, then the disabled set).
fn reported_enabled_types(described: &Answer) -> Vec<String> {
    described.cluster()["logging"]["clusterLogging"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|setup| setup["enabled"] == serde_json::json!(true))
        .flat_map(|setup| {
            setup["types"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

/// Move the cluster's owed `CREATING` -> `ACTIVE` instant into the past, so the next read
/// materialises it: an update can only be started on an `ACTIVE` cluster, and nothing in
/// the emulator sleeps for a due instant.
fn make_active(engine: &std::sync::Arc<engine::InMemoryEngine>, cluster: &str) {
    let store = service_state(&**engine).expect("the eks store");
    let mut guard = store.lock();
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .expect("the cluster just created");
    record.active_at = Some(SystemTime::now() - Duration::from_secs(1));
}

async fn update_logging(
    router: &MultiServiceRouter,
    cluster: &str,
    configuration: &Logging,
) -> Answer {
    let body = serde_json::json!({ "logging": logging_json(configuration) });
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{cluster}/update-config"),
        &body.to_string(),
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE (which types export): a type exports when any entry that enables logging
// names it, independently of every other type — and that is what the cluster
// reports back as its enabled group.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_log_type_is_enabled_independently_of_every_other() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in enablement_rows().into_iter().enumerate() {
        let derived = enabled_log_types(Some(&row.logging));
        assert_eq!(
            derived, row.enabled,
            "case {:?}: expected the exporting types {:?}, got {derived:?}",
            row.case, row.enabled
        );

        // The same reading through the operations: a cluster created with the row's
        // configuration reports exactly those types as enabled.
        let cluster = format!("log-export-create-{index}");
        let created = create(&router, &cluster_body(&cluster, Some(&row.logging))).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the cluster must be created, got {created:?}",
            row.case
        );
        let described = describe(&router, &cluster).await;
        assert_eq!(
            reported_enabled_types(&described),
            row.enabled,
            "case {:?}: the created cluster must report {:?} as exporting, got {}",
            row.case,
            row.enabled,
            described.cluster()["logging"]
        );

        // And the same configuration asked for AFTER the fact, through
        // `UpdateClusterConfig`, is accepted and reports a `LoggingUpdate` — with no
        // CloudWatch Logs mounted beside this service, the export itself is silent and a
        // cluster write never depends on it.
        make_active(&engine, &cluster);
        let updated = update_logging(&router, &cluster, &row.logging).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: enabling the same types through UpdateClusterConfig must be \
             accepted, got {updated:?} ({})",
            row.case,
            updated.message()
        );
        assert_eq!(
            updated.body["update"]["type"],
            serde_json::json!("LoggingUpdate"),
            "case {:?}: a logging change is a LoggingUpdate, got {}",
            row.case,
            updated.body["update"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (where the export lands): the group is the cluster's own
// `/aws/eks/<cluster name>/cluster`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_export_group_is_named_after_the_cluster() {
    // The rows span the characters a cluster name may carry (letters, digits, hyphens and
    // underscores, per `CreateCluster$name`), including names the issue did not cite.
    for cluster in [
        "a",
        "sim2real-eks-d9efb6d49b09",
        "Cluster_9-name",
        "0123456789",
    ] {
        let group = cluster_log_group_name(cluster);
        assert_eq!(
            group,
            format!("/aws/eks/{cluster}/cluster"),
            "case {cluster:?}: expected the cluster's own log group, got {group:?}"
        );
        // A name CloudWatch Logs can hold, which is what makes the group creatable at
        // all (`service_common::cloudwatch_logs`, the one reading of that rule).
        assert!(
            service_common::cloudwatch_logs::is_log_group_name(&group),
            "case {cluster:?}: the group name must be one CloudWatch Logs holds, got \
             {group:?}"
        );
    }

    // Two clusters never share a group: the name carries the cluster's own.
    let groups: std::collections::BTreeSet<String> = ["one", "two", "three"]
        .into_iter()
        .map(cluster_log_group_name)
        .collect();
    assert_eq!(
        groups.len(),
        3,
        "each cluster must have a log group of its own, got {groups:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE (one stream per exporting type): each type exports under the stream of the
// control plane component whose logs it carries, suffixed from the cluster's own
// stable resource id.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_log_type_exports_under_its_own_component_stream() {
    let resource_id =
        derive_cluster_resource_id("arn:aws:eks:us-west-2:000000000000:cluster/stream-rows");

    // The component each type's streams are named after, as the Amazon EKS control plane
    // logging documentation publishes them.
    for (log_type, component) in [
        ("api", "kube-apiserver"),
        ("audit", "kube-apiserver-audit"),
        ("authenticator", "authenticator"),
        ("controllerManager", "kube-controller-manager"),
        ("scheduler", "kube-scheduler"),
    ] {
        let stream = control_plane_log_stream_name(log_type, &resource_id)
            .unwrap_or_else(|| panic!("case {log_type:?}: every declared log type has a stream"));
        let expected_prefix = format!("{component}-");
        let suffix = stream.strip_prefix(&expected_prefix).unwrap_or_else(|| {
            panic!(
                "case {log_type:?}: expected a stream named {expected_prefix:?} plus a \
                 suffix, got {stream:?}"
            )
        });
        assert!(
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {log_type:?}: the stream suffix must be hex of the cluster's resource \
             id, got {suffix:?} in {stream:?}"
        );
        // The same cluster and type name the same stream on every export, so a later
        // export appends to the stream the earlier one opened.
        assert_eq!(
            control_plane_log_stream_name(log_type, &resource_id).as_deref(),
            Some(stream.as_str()),
            "case {log_type:?}: the stream name must be stable, got {stream:?}"
        );
    }

    // Every declared type has a stream, no two types share one, and two clusters do not
    // share a type's stream either.
    let streams: std::collections::BTreeSet<String> = LOG_TYPE_VALUES
        .iter()
        .filter_map(|log_type| control_plane_log_stream_name(log_type, &resource_id))
        .collect();
    assert_eq!(
        streams.len(),
        LOG_TYPE_VALUES.len(),
        "each log type must export under a stream of its own, got {streams:?}"
    );
    let other_id =
        derive_cluster_resource_id("arn:aws:eks:us-west-2:000000000000:cluster/other-rows");
    for log_type in LOG_TYPE_VALUES {
        assert_ne!(
            control_plane_log_stream_name(log_type, &resource_id),
            control_plane_log_stream_name(log_type, &other_id),
            "case {log_type:?}: two clusters must not share a stream, got {:?}",
            control_plane_log_stream_name(log_type, &resource_id)
        );
    }
}
