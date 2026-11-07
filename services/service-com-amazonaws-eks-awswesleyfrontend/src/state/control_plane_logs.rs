// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs side of a cluster's `logging`: the group a cluster's control
//! plane logs are exported to, the stream each enabled log type exports under, and the
//! export itself.
//!
//! `LogSetup$enabled` is "If a log type is enabled, that log type exports its control
//! plane logs to CloudWatch Logs. If a log type isn't enabled, that log type doesn't
//! export its control plane logs. Each individual log type can be enabled or disabled
//! independently", so a configuration this service STORES is only half of the behaviour:
//! the export is what a client observes, and it is observed in CloudWatch Logs rather
//! than on the cluster. A cluster that reports `api` enabled and exports nothing is a
//! cluster whose setting had no effect.
//!
//! Three things the export is made of, each of them a value Amazon EKS publishes:
//!
//! * the GROUP is `/aws/eks/<cluster name>/cluster` ([`cluster_log_group_name`]), one per
//!   cluster, and the service creates it when the first enabled type needs it — a client
//!   that enables logging never creates it itself;
//! * the STREAM is the control plane COMPONENT whose logs that type carries
//!   ([`control_plane_log_stream_name`]), so each log type exports under its own stream
//!   and a type nobody enabled has none;
//! * nothing is created at all for a configuration that enables NO type
//!   ([`enabled_log_types`] is empty), which is what makes a disabled type observably
//!   different from an enabled one.
//!
//! The export goes out over the in-process callback, the same seam the rest of this
//! service's cross-service work uses (`super::callback_client!`), and is BEST EFFORT: an
//! emulator with no CloudWatch Logs mounted beside this service, or a write CloudWatch
//! Logs refuses, is reported in this service's own tracing rather than failing the
//! cluster call, which asked for a cluster write and has the configuration stored either
//! way. Nothing here deletes a group or a stream: the log records in it belong to the
//! account, and nothing documents Amazon EKS removing a customer's logs when a type is
//! disabled or a cluster is deleted.

use super::*;

/// The log group one cluster's control plane logs are exported to:
/// `/aws/eks/<cluster name>/cluster`, the name Amazon EKS publishes for it.
///
/// Built from the cluster's own name, so each cluster of the account has a group of its
/// own and a client that knows the cluster knows the group.
pub fn cluster_log_group_name(cluster_name: &str) -> String {
    format!("/aws/eks/{cluster_name}/cluster")
}

/// The log types `logging` ENABLES, in the order the model declares them
/// ([`crate::ops::types::LOG_TYPE_VALUES`]).
///
/// A type is enabled when ANY entry of `clusterLogging` that enables logging names it —
/// "each individual log type can be enabled or disabled independently" — so a type named
/// twice, or named in both an enabled and a disabled entry, is reported once and a
/// configuration carrying no entry at all enables nothing. This is the ONE reading of
/// which types are on: [`super::reported_logging`] projects the same answer onto the
/// `Logging` a cluster reports.
pub fn enabled_log_types(logging: Option<&Logging>) -> Vec<&'static str> {
    let setups = logging
        .and_then(|logging| logging.cluster_logging.as_deref())
        .unwrap_or_default();
    LOG_TYPE_VALUES
        .iter()
        .copied()
        .filter(|log_type| {
            setups.iter().any(|setup| {
                setup.enabled.unwrap_or(false)
                    && setup.types.as_deref().is_some_and(|types| {
                        types.iter().any(|named| named == log_type)
                    })
            })
        })
        .collect()
}

/// The control plane component whose logs one log type carries, which is the component
/// Amazon EKS names that type's log streams after: the Kubernetes API server for `api`,
/// its audit log for `audit`, the IAM authenticator for `authenticator`, and the
/// controller manager and scheduler for the two types named after them.
///
/// `None` for a value that is not one of the five types `LogType` declares, which no
/// stored configuration carries (the boundary validator refuses an unmodelled member of
/// the enum) and for which there is no component to name a stream after.
fn control_plane_component(log_type: &str) -> Option<&'static str> {
    match log_type {
        "api" => Some("kube-apiserver"),
        "audit" => Some("kube-apiserver-audit"),
        "authenticator" => Some("authenticator"),
        "controllerManager" => Some("kube-controller-manager"),
        "scheduler" => Some("kube-scheduler"),
        _ => None,
    }
}

/// How many hex characters of the cluster's resource id the stream suffix carries.
const LOG_STREAM_SUFFIX_CHARS: usize = 10;

/// The log stream one enabled log type exports under:
/// `<component>-<suffix>` ([`control_plane_component`]).
///
/// The suffix is derived from the cluster's own stable resource id
/// ([`super::derive_cluster_resource_id`]), so two clusters never name the same stream,
/// the name is the same on every export for one cluster — a later export appends to the
/// stream the earlier one opened — and nothing in the request decides it. `resource_id`
/// is expected to be that 32-hex-character id; a shorter value is used as it is, because
/// a stream name is not what a cluster call is refused over.
pub fn control_plane_log_stream_name(log_type: &str, resource_id: &str) -> Option<String> {
    let component = control_plane_component(log_type)?;
    let suffix = resource_id
        .get(..LOG_STREAM_SUFFIX_CHARS)
        .unwrap_or(resource_id)
        .to_ascii_lowercase();
    Some(format!("{component}-{suffix}"))
}

/// What ONE log record delivered to a type's stream says.
///
/// This emulator runs no Kubernetes control plane, so it synthesises none of that plane's
/// traffic: what it delivers is the one thing it knows about the export — that this type
/// of this cluster's control plane logs is being exported, and the versions of the control
/// plane doing the exporting — written as a JSON object whose members are spelled the way
/// this service's own model spells them (`logType`, `clusterName`, `version`,
/// `platformVersion`). A member the cluster does not carry is left out rather than written
/// as `null`.
fn export_record(
    cluster_name: &str,
    log_type: &str,
    component: &str,
    version: Option<&str>,
    platform_version: Option<&str>,
) -> String {
    let mut document = ::serde_json::Map::new();
    document.insert(
        "clusterName".to_string(),
        ::serde_json::Value::String(cluster_name.to_string()),
    );
    document.insert(
        "logType".to_string(),
        ::serde_json::Value::String(log_type.to_string()),
    );
    document.insert(
        "component".to_string(),
        ::serde_json::Value::String(component.to_string()),
    );
    for (member, value) in [("version", version), ("platformVersion", platform_version)] {
        if let Some(value) = value {
            document.insert(
                member.to_string(),
                ::serde_json::Value::String(value.to_string()),
            );
        }
    }
    document.insert(
        "message".to_string(),
        ::serde_json::Value::String(format!(
            "{component} control plane logs of cluster {cluster_name} are exported to this \
             log stream"
        )),
    );
    ::serde_json::Value::Object(document).to_string()
}

/// Everything one export needs off the cluster it is for, read while the store's lock is
/// held so the export itself runs with none.
#[derive(Debug, Clone)]
pub struct ControlPlaneLogExport {
    /// The cluster whose control plane is exporting, which names both the group and the
    /// Region the group is created in.
    pub cluster_name: String,
    /// The cluster's 32-hex-character resource id, which the stream names are built from.
    pub resource_id: String,
    /// The cluster's Kubernetes version, as the record holds it.
    pub version: Option<String>,
    /// The cluster's Amazon EKS platform version, as the record holds it.
    pub platform_version: Option<String>,
    /// The log types this export turns on, in the model's own order.
    pub log_types: Vec<&'static str>,
}

impl ControlPlaneLogExport {
    /// What `logging` asks this cluster to export, or `None` when it enables no type —
    /// the case in which nothing is created at all.
    pub fn of(record: &ClusterRecord, logging: Option<&Logging>) -> Option<Self> {
        let log_types = enabled_log_types(logging);
        if log_types.is_empty() {
            return None;
        }
        Some(Self {
            cluster_name: record.name.clone(),
            resource_id: derive_cluster_resource_id(&record.arn),
            version: record.version.clone(),
            platform_version: record.platform_version.clone(),
            log_types,
        })
    }
}

/// Export the control plane logs of the types this cluster has enabled: create the
/// cluster's log group if CloudWatch Logs does not hold it yet, then open the stream of
/// each enabled type and deliver one record to it ([`export_record`]).
///
/// Best effort and never fatal — see this module's header. Call it with NO store guard
/// held: it awaits.
pub async fn export_control_plane_logs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    export: &ControlPlaneLogExport,
) {
    // No Region label, so no endpoint to reach CloudWatch Logs at: the same silence an
    // unmounted CloudWatch Logs leaves, which a cluster call is not refused over.
    let Some(endpoint) = peer_endpoint("logs", region) else {
        return;
    };
    let logs = callback_client!(aws_sdk_cloudwatchlogs, ctx, endpoint, region);
    let log_group = cluster_log_group_name(&export.cluster_name);
    let created = logs
        .create_log_group()
        .log_group_name(&log_group)
        .send()
        .await;
    match created {
        Ok(_) => {}
        // The group is already there — this cluster has exported before, or the account
        // created it itself — which is exactly the state this step owes the caller.
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
        Err(error) => {
            ::tracing::warn!(
                cluster_name = %export.cluster_name,
                %log_group,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the cluster's control plane log group could not be created, so its \
                 control plane logs are not exported"
            );
            return;
        }
    }

    let exported_at = epoch_millis(SystemTime::now());
    for log_type in &export.log_types {
        let Some(log_stream) = control_plane_log_stream_name(log_type, &export.resource_id) else {
            continue;
        };
        let opened = logs
            .create_log_stream()
            .log_group_name(&log_group)
            .log_stream_name(&log_stream)
            .send()
            .await;
        match opened {
            Ok(_) => {}
            // The stream this component already exports under: a later export appends to
            // it rather than opening a second one.
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service| service.is_resource_already_exists_exception()) => {}
            Err(error) => {
                ::tracing::warn!(
                    cluster_name = %export.cluster_name,
                    %log_group,
                    %log_stream,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the log stream of this control plane log type could not be opened"
                );
                continue;
            }
        }
        let Some(component) = control_plane_component(log_type) else {
            continue;
        };
        let record = export_record(
            &export.cluster_name,
            log_type,
            component,
            export.version.as_deref(),
            export.platform_version.as_deref(),
        );
        let Ok(event) = ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
            .timestamp(exported_at)
            .message(record)
            .build()
        else {
            continue;
        };
        let delivered = logs
            .put_log_events()
            .log_group_name(&log_group)
            .log_stream_name(&log_stream)
            .log_events(event)
            .send()
            .await;
        if let Err(error) = delivered {
            ::tracing::warn!(
                cluster_name = %export.cluster_name,
                %log_group,
                %log_stream,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "this control plane log type's record could not be delivered"
            );
        }
    }
}

/// `at` in milliseconds since the Unix epoch, which is the stamp a log event carries.
///
/// An instant before the epoch, which no log event can carry, clamps to the epoch rather
/// than wrapping.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
