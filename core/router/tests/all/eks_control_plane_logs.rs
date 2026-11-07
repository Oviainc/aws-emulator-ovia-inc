// SPDX-License-Identifier: CC0-1.0

//! An Amazon EKS cluster's CONTROL PLANE LOGS, across the two services that have to agree
//! for a client to read them.
//!
//! `LogSetup$enabled` is "If a log type is enabled, that log type exports its control
//! plane logs to CloudWatch Logs. If a log type isn't enabled, that log type doesn't
//! export its control plane logs. Each individual log type can be enabled or disabled
//! independently", so the externally visible effect of the setting lives in CloudWatch
//! Logs, not on the cluster: the log group `/aws/eks/<cluster name>/cluster` — created by
//! the service, not by the client — and one log stream per exporting type, named after the
//! control plane component whose logs that type carries. Only this configuration, EKS and
//! CloudWatch Logs on ONE engine (which is what `core/router` ships), makes that
//! observable, which is why this suite lives here.
//!
//! The rules these rows pin:
//!
//! * a cluster CREATED with types enabled exports each of them, and only them;
//! * a cluster whose types are turned on AFTER the fact, through `UpdateClusterConfig`,
//!   exports from then on — the group appears for a client that never created it, and so
//!   does the stream of each type the request enabled;
//! * a configuration that enables NO type (every entry disabled, or no `logging` at all)
//!   creates no group and no stream at all, so a disabled type is observably different
//!   from an enabled one;
//! * each type is independent: enabling a second type later adds its stream and leaves the
//!   first one's where it is.
//!
//! The rows differ along how many types one call enables, which types they are (every
//! member of `LogType` is exercised), and whether they are enabled at create time or
//! afterwards.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-west-2";

/// The two signing names, which is how the router picks the service that serves each
/// request.
const EKS: &str = "eks";
const LOGS: &str = "logs";

/// The `X-Amz-Target` prefix CloudWatch Logs names an operation under: its service shape's
/// name. (EKS is `restJson1` and carries none.)
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The control plane component each log type's streams are named after, as the Amazon EKS
/// control plane logging documentation publishes them — one entry per member of `LogType`.
const COMPONENT_OF_LOG_TYPE: &[(&str, &str)] = &[
    ("api", "kube-apiserver"),
    ("audit", "kube-apiserver-audit"),
    ("authenticator", "authenticator"),
    ("controllerManager", "kube-controller-manager"),
    ("scheduler", "kube-scheduler"),
];

/// How long the suite waits for a cluster's owed `CREATING` -> `ACTIVE` move to fall due
/// before it polls for it again.
const POLL_WAIT: Duration = Duration::from_millis(100);

/// How many times it polls for that move, which bounds the wait.
const POLL_ATTEMPTS: usize = 60;

/// The two services over ONE shared engine, as `router/src/lib.rs` mounts them.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These rows exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn dispatch(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        target: Option<String>,
        body: Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://localhost:9297{uri}"))
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(target) = target {
            request = request.header("x-amz-target", target);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(Bytes::from(body.to_string()))
                    .expect("a well-formed request"),
            )
            .await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One EKS call (restJson1), with the status it answered.
    async fn eks(&self, method: &str, uri: &str, body: Value) -> (u16, Value) {
        self.dispatch(EKS, method, uri, None, body).await
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self
            .dispatch(
                LOGS,
                "POST",
                "/",
                Some(format!("{LOGS_TARGET_PREFIX}.{operation}")),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected logs {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create a cluster, with `logging` when the row carries one.
    async fn create_cluster(&self, cluster: &str, logging: Option<Value>, case: &str) {
        let mut body = json!({
            "name": cluster,
            "roleArn": format!("arn:aws:iam::{}:role/eksClusterRole", engine::emulator_account_id()),
            "resourcesVpcConfig": { "subnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"] },
        });
        if let Some(logging) = logging {
            body["logging"] = logging;
        }
        let (status, parsed) = self.eks("POST", "/clusters", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the cluster must be created, got {status} with {parsed}"
        );
    }

    /// Wait for the cluster's owed `CREATING` -> `ACTIVE` move to be materialised by a
    /// read, which is what `UpdateClusterConfig` needs before it accepts a change.
    async fn wait_until_active(&self, cluster: &str, case: &str) {
        for _ in 0..POLL_ATTEMPTS {
            let (status, parsed) = self
                .eks("GET", &format!("/clusters/{cluster}"), json!({}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the cluster must be readable, got {status} with {parsed}"
            );
            if parsed["cluster"]["status"] == json!("ACTIVE") {
                return;
            }
            tokio::time::sleep(POLL_WAIT).await;
        }
        panic!("case {case:?}: the cluster never reached ACTIVE");
    }

    /// Turn log types on (or off) through `UpdateClusterConfig`.
    async fn update_logging(&self, cluster: &str, logging: Value, case: &str) -> Value {
        let (status, parsed) = self
            .eks(
                "POST",
                &format!("/clusters/{cluster}/update-config"),
                json!({ "logging": logging }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateClusterConfig must answer 200, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["update"]["type"],
            json!("LoggingUpdate"),
            "case {case:?}: a logging change is a LoggingUpdate, got {parsed}"
        );
        parsed
    }

    /// Whether CloudWatch Logs holds the cluster's log group, as a client finds it:
    /// `DescribeLogGroups` narrowed to that name.
    async fn holds_log_group(&self, cluster: &str, case: &str) -> bool {
        let group = cluster_log_group(cluster);
        let answer = self
            .logs(
                "DescribeLogGroups",
                json!({ "logGroupNamePrefix": &group }),
                case,
            )
            .await;
        answer["logGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .any(|held| held["logGroupName"] == json!(group))
    }

    /// The log streams the cluster's group holds, by name.
    async fn log_streams(&self, cluster: &str, case: &str) -> Vec<String> {
        let answer = self
            .logs(
                "DescribeLogStreams",
                json!({ "logGroupName": cluster_log_group(cluster) }),
                case,
            )
            .await;
        answer["logStreams"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_owned))
            .collect()
    }

    /// The messages one stream of the cluster's group holds.
    async fn log_events(&self, cluster: &str, stream: &str, case: &str) -> Vec<String> {
        let answer = self
            .logs(
                "GetLogEvents",
                json!({
                    "logGroupName": cluster_log_group(cluster),
                    "logStreamName": stream,
                }),
                case,
            )
            .await;
        answer["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|event| event["message"].as_str().map(str::to_owned))
            .collect()
    }
}

/// The log group a cluster's control plane logs are exported to.
fn cluster_log_group(cluster: &str) -> String {
    format!("/aws/eks/{cluster}/cluster")
}

/// One `logging` payload enabling `types`, as a client writes it.
fn enabling(types: &[&str]) -> Value {
    json!({ "clusterLogging": [{ "types": types, "enabled": true }] })
}

/// One `logging` payload disabling `types`.
fn disabling(types: &[&str]) -> Value {
    json!({ "clusterLogging": [{ "types": types, "enabled": false }] })
}

/// The component a log type's stream is named after.
fn component_of(log_type: &str) -> &'static str {
    COMPONENT_OF_LOG_TYPE
        .iter()
        .find(|(declared, _)| *declared == log_type)
        .map(|(_, component)| *component)
        .unwrap_or_else(|| panic!("{log_type:?} is not a declared log type"))
}

/// The component one stream belongs to: the LONGEST published component name the stream is
/// that component's name followed by its suffix. The longest, because one component's name
/// is a prefix of another's (`kube-apiserver` of `kube-apiserver-audit`).
fn component_of_stream(stream: &str) -> Option<&'static str> {
    COMPONENT_OF_LOG_TYPE
        .iter()
        .map(|(_, component)| *component)
        .filter(|component| {
            stream
                .strip_prefix(component)
                .is_some_and(|suffix| suffix.starts_with('-'))
        })
        .max_by_key(|component| component.len())
}

/// Assert that the cluster's group holds exactly one stream per type in `exporting`, each
/// named after that type's component, and nothing else.
async fn assert_exports(emulator: &Emulator, cluster: &str, exporting: &[&str], case: &str) {
    assert!(
        emulator.holds_log_group(cluster, case).await,
        "case {case:?}: CloudWatch Logs must hold {}, got none",
        cluster_log_group(cluster)
    );
    let streams = emulator.log_streams(cluster, case).await;
    assert_eq!(
        streams.len(),
        exporting.len(),
        "case {case:?}: expected one stream per exporting type {exporting:?}, got {streams:?}"
    );
    for log_type in exporting {
        let component = component_of(log_type);
        let found: Vec<&String> = streams
            .iter()
            .filter(|stream| component_of_stream(stream) == Some(component))
            .collect();
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: {log_type:?} must export under exactly one {component:?} \
             stream, got {streams:?}"
        );
        // A stream that exists but holds nothing would be a group with no logs in it: the
        // export delivers the control plane's own record for that type.
        let events = emulator.log_events(cluster, found[0], case).await;
        assert!(
            !events.is_empty(),
            "case {case:?}: {log_type:?}'s stream {:?} must hold the exported record, got \
             {events:?}",
            found[0]
        );
        assert!(
            events
                .iter()
                .all(|message| message.contains(log_type) && message.contains(cluster)),
            "case {case:?}: every record of {log_type:?} names its type and its cluster, \
             got {events:?}"
        );
    }
}

/// Assert that nothing was created for a cluster that exports no type.
async fn assert_no_export(emulator: &Emulator, cluster: &str, case: &str) {
    assert!(
        !emulator.holds_log_group(cluster, case).await,
        "case {case:?}: a cluster that exports no log type must have no log group, got {}",
        cluster_log_group(cluster)
    );
}

/// One row of the create-time table.
struct CreateRow {
    case: &'static str,
    cluster: &'static str,
    /// The `logging` the create carries, if any.
    logging: Option<Value>,
    /// The types that must export, in the model's own order.
    exporting: Vec<&'static str>,
}

#[tokio::test]
async fn a_cluster_created_with_log_types_enabled_exports_each_of_them() {
    let emulator = Emulator::new();

    let rows = vec![
        CreateRow {
            case: "one type enabled at create time",
            cluster: "cp-logs-api",
            logging: Some(enabling(&["api"])),
            exporting: vec!["api"],
        },
        CreateRow {
            case: "two types enabled at create time",
            cluster: "cp-logs-api-audit",
            logging: Some(enabling(&["api", "audit"])),
            exporting: vec!["api", "audit"],
        },
        CreateRow {
            case: "every declared type enabled at create time",
            cluster: "cp-logs-all",
            logging: Some(enabling(&[
                "api",
                "audit",
                "authenticator",
                "controllerManager",
                "scheduler",
            ])),
            exporting: vec![
                "api",
                "audit",
                "authenticator",
                "controllerManager",
                "scheduler",
            ],
        },
        CreateRow {
            case: "a type the issue did not cite, enabled on its own",
            cluster: "cp-logs-scheduler",
            logging: Some(enabling(&["scheduler"])),
            exporting: vec!["scheduler"],
        },
        CreateRow {
            case: "every type explicitly disabled",
            cluster: "cp-logs-disabled",
            logging: Some(disabling(&[
                "api",
                "audit",
                "authenticator",
                "controllerManager",
                "scheduler",
            ])),
            exporting: vec![],
        },
        CreateRow {
            case: "no logging configuration at all",
            cluster: "cp-logs-absent",
            logging: None,
            exporting: vec![],
        },
    ];

    for row in rows {
        emulator
            .create_cluster(row.cluster, row.logging.clone(), row.case)
            .await;
        if row.exporting.is_empty() {
            assert_no_export(&emulator, row.cluster, row.case).await;
        } else {
            assert_exports(&emulator, row.cluster, &row.exporting, row.case).await;
        }
    }
}

#[tokio::test]
async fn enabling_a_log_type_after_the_fact_creates_the_group_and_its_stream() {
    let emulator = Emulator::new();

    // Each row turns types on through `UpdateClusterConfig` on a cluster created WITHOUT
    // logging, which is the workflow a client uses to turn control plane logging on after
    // the fact.
    for (case, cluster, first, second) in [
        (
            "one type enabled, then a second one",
            "cp-logs-update-api",
            vec!["api"],
            vec!["audit"],
        ),
        (
            "two types enabled at once, then a third",
            "cp-logs-update-pair",
            vec!["authenticator", "scheduler"],
            vec!["controllerManager"],
        ),
    ] {
        emulator.create_cluster(cluster, None, case).await;
        // Nothing exports yet: the create enabled no type.
        assert_no_export(&emulator, cluster, case).await;

        emulator.wait_until_active(cluster, case).await;
        emulator
            .update_logging(cluster, enabling(&first), case)
            .await;
        assert_exports(&emulator, cluster, &first, case).await;

        // Each type is independent: turning the next one on adds its stream and leaves the
        // first one's in place. The request carries BOTH, which is the configuration the
        // cluster is to end up with.
        emulator.wait_until_active(cluster, case).await;
        let mut both = first.clone();
        both.extend(second.iter().copied());
        emulator.update_logging(cluster, enabling(&both), case).await;
        assert_exports(&emulator, cluster, &both, case).await;
    }
}
