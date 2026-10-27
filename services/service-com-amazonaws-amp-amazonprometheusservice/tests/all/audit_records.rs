// SPDX-License-Identifier: CC0-1.0

//! The audit record a handler leaves when it CHANGES this service's state.
//!
//! The rule these suites pin, over every mutating operation this crate implements: a
//! handler leaves exactly one audit record on each path where it writes to the store —
//! naming the operation, the ARN of the resource it changed and the access-key id the
//! REQUEST presented — and leaves none on a path that writes nothing: a reporting
//! operation, an idempotency replay answered from the stored record, or a refusal.
//!
//! A resource this service gives no ARN of its own (a rules-and-alerting or query
//! logging configuration, an alert manager definition, a resource-based policy, a
//! scraper logging configuration) is recorded under the ARN of the workspace or scraper
//! it hangs off, which is the resource the change is addressed to.
//!
//! Each suite drives the real request path (`crate::workspace_lifecycle::Amp`) and reads
//! the records back off ONE capturing `tracing` subscriber, installed as this test
//! binary's global default on first use and keeping a separate buffer per watching
//! thread — so a suite reads only the records its own calls left. It is global because a
//! thread-local default is rebuilt as the other suites in this target install and drop
//! theirs, which decides globally whether a callsite is enabled at all; a second global
//! default installed anywhere in this binary would therefore break these suites (the
//! install below says so). Nothing is written to disk.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;

use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use crate::anomaly_detector_update::{create_detector, put_detector, Spec};
use crate::logging_configuration::log_group_arn;
use crate::logging_configuration_deletion::{create_logging, delete_logging};
use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace};
use crate::rule_groups_namespace_update::{put_body, put_namespace};
use crate::scraper_creation::{eks_source_json, workspace_destination_json};
use crate::scraper_logging_configuration_deletion::{
    cloud_watch_logs, components, logging_configuration, seed_pair,
};
use crate::scraper_update::scrape_configuration_json;
use crate::workspace_lifecycle::{encode_label, text, wait_for_due_transition, Amp, Reply};

/// The access-key id every request this harness signs presents — the `Credential=` of
/// the `Authorization` header `crate::workspace_lifecycle::Amp::call` builds. A record
/// has to attribute the change to the key the REQUEST carried, never to the emulator's
/// own credentials.
const PRESENTED_AKID: &str = "AKIDEXAMPLE";

/// The fields the audit seam records, so a captured event is recognised by its SHAPE
/// rather than by a message this test would have to keep in step with the source.
const AUDIT_FIELDS: [&str; 3] = ["operation", "resource", "access_key_id"];

/// One captured `tracing` event: its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Whether this event is an audit record: one carrying every field the seam records.
    fn is_audit_record(&self) -> bool {
        AUDIT_FIELDS
            .iter()
            .all(|name| self.fields.contains_key(*name))
    }
}

/// Collects an event's fields into strings, whatever type each was recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps the events emitted on each WATCHED thread, so a
/// suite reads back the records its own calls left and nothing else.
///
/// Per thread rather than per subscriber because one subscriber is installed for the
/// whole test binary: a thread-local default is rebuilt as the other suites in this
/// target install and drop their own, which decides globally whether a callsite is
/// enabled at all. Each `#[tokio::test]` here drives its requests on its own thread
/// (the current-thread runtime), so the thread is what identifies a suite's records.
#[derive(Clone, Default)]
struct Capture {
    threads: Arc<Mutex<HashMap<ThreadId, Vec<CapturedEvent>>>>,
}

impl Capture {
    /// Start capturing the calling thread's events, discarding anything held for a
    /// thread this id belonged to before.
    fn watch(&self) {
        self.threads
            .lock()
            .expect("the capture buffer")
            .insert(std::thread::current().id(), Vec::new());
    }

    /// The audit records this thread has emitted since the last drain, oldest first,
    /// emptying its buffer — so each assertion below is about the calls made since the
    /// previous one.
    fn drain(&self) -> Vec<CapturedEvent> {
        let mut threads = self.threads.lock().expect("the capture buffer");
        let drained = threads
            .get_mut(&std::thread::current().id())
            .map(std::mem::take)
            .unwrap_or_default();
        drained
            .into_iter()
            .filter(CapturedEvent::is_audit_record)
            .collect()
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // These suites assert on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut threads = self.threads.lock().expect("the capture buffer");
        if let Some(buffer) = threads.get_mut(&std::thread::current().id()) {
            let mut fields = BTreeMap::new();
            event.record(&mut FieldCollector(&mut fields));
            buffer.push(CapturedEvent { fields });
        }
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Capture this thread's `tracing` events, through the one subscriber this test binary
/// installs as its global default (see the module header for why it is global). A thread
/// nobody watches is not recorded, so the other suites in this target cost one hashmap
/// lookup per event and nothing else.
fn capture_events() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    let capture = CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("this test binary installs no other global tracing subscriber");
        capture
    });
    capture.watch();
    capture
}

/// Exactly one audit record since the last drain, naming `operation`, `resource` and the
/// access-key id the request presented.
fn assert_audited(capture: &Capture, case: &str, operation: &str, resource: &str) {
    let records = capture.drain();
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected exactly one audit record for {operation}, got {records:?}"
    );
    let record = &records[0];
    assert_eq!(
        record.field("operation"),
        Some(operation),
        "case {case:?}: expected the record to name {operation}, got {record:?}"
    );
    assert_eq!(
        record.field("resource"),
        Some(resource),
        "case {case:?}: expected the record to name the resource {resource}, got {record:?}"
    );
    assert_eq!(
        record.field("access_key_id"),
        Some(PRESENTED_AKID),
        "case {case:?}: expected the record to attribute the change to {PRESENTED_AKID}, \
         got {record:?}"
    );
}

/// No audit record since the last drain: the calls in between changed nothing.
fn assert_not_audited(capture: &Capture, case: &str) {
    let records = capture.drain();
    assert!(
        records.is_empty(),
        "case {case:?}: a call that changed nothing must leave no audit record, got {records:?}"
    );
}

/// The modelled status every accepted call below answers with.
fn assert_status(case: &str, reply: &Reply, expected: u16) {
    assert_eq!(
        reply.status, expected,
        "case {case:?}: expected HTTP {expected}, got {reply:?}"
    );
}

/// One workspace, created through `CreateWorkspace` and settled to `ACTIVE` by the clock:
/// `(workspaceId, arn)`.
async fn active_workspace(amp: &Amp, region: &str, alias: &str) -> (String, String) {
    let created = amp
        .call("POST", "/workspaces", region, &json!({ "alias": alias }).to_string())
        .await;
    assert_status(alias, &created, 202);
    wait_for_due_transition();
    (
        text(&created.body["workspaceId"]),
        text(&created.body["arn"]),
    )
}

/// A valid alert manager definition document: `validate_alert_manager_definition`
/// requires the `alertmanager_config` key the service's own documentation describes.
fn alert_manager_definition(receiver: &str) -> String {
    format!("alertmanager_config: |\n  route:\n    receiver: {receiver}\n")
}

/// The body an alert manager create or update carries: `data` is a blob, so it rides the
/// wire base64-encoded, written with the codec the wire serializer itself uses.
fn definition_body(document: &str, token: Option<&str>) -> serde_json::Value {
    let mut body = json!({ "data": aws_smithy_types::base64::encode(document.as_bytes()) });
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    body
}

/// The `destinations` member of a query logging configuration: `LoggingDestinations` is
/// modelled `len 1..1`, so one `cloudWatchLogs` destination with its filters.
fn query_logging_body(region: &str, log_group: &str, token: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "destinations": [{
            "cloudWatchLogs": { "logGroupArn": log_group_arn("aws", region, log_group) },
            "filters": { "qspThreshold": 0 },
        }],
    });
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    body
}

/// Every operation that changes a WORKSPACE record leaves one record naming the
/// workspace's own ARN, and every operation that only reports leaves none — including
/// the replay of the delete, which writes nothing.
#[tokio::test]
async fn workspace_mutations_are_audited_and_reads_are_not() {
    let region = "us-west-2";
    let amp = Amp::new();
    let capture = capture_events();

    let created = amp
        .call(
            "POST",
            "/workspaces",
            region,
            &json!({ "alias": "audited", "clientToken": "ws-token" }).to_string(),
        )
        .await;
    assert_status("CreateWorkspace", &created, 202);
    let workspace_id = text(&created.body["workspaceId"]);
    let workspace_arn = text(&created.body["arn"]);
    assert_audited(capture, "a create", "CreateWorkspace", &workspace_arn);

    // The replayed `clientToken` answers from the workspace the first call wrote, so it
    // creates nothing and records nothing.
    let replayed = amp
        .call(
            "POST",
            "/workspaces",
            region,
            &json!({ "alias": "audited", "clientToken": "ws-token" }).to_string(),
        )
        .await;
    assert_status("CreateWorkspace replay", &replayed, 202);
    assert_not_audited(capture, "a replayed create");

    wait_for_due_transition();

    let renamed = amp
        .rename(region, &workspace_id, &json!({ "alias": "audited-again" }))
        .await;
    assert_status("UpdateWorkspaceAlias", &renamed, 204);
    assert_audited(capture, "a rename", "UpdateWorkspaceAlias", &workspace_arn);

    wait_for_due_transition();

    let configured = amp
        .call(
            "PATCH",
            &format!("/workspaces/{workspace_id}/configuration"),
            region,
            &json!({ "retentionPeriodInDays": 30 }).to_string(),
        )
        .await;
    assert_status("UpdateWorkspaceConfiguration", &configured, 202);
    assert_audited(
        capture,
        "a configuration update",
        "UpdateWorkspaceConfiguration",
        &workspace_arn,
    );

    let tagged = amp
        .tag(region, &workspace_arn, &json!({ "team": "metrics" }))
        .await;
    assert_status("TagResource", &tagged, 200);
    assert_audited(capture, "a tag merge", "TagResource", &workspace_arn);

    let untagged = amp.untag(region, &workspace_arn, &["team"]).await;
    assert_status("UntagResource", &untagged, 200);
    assert_audited(capture, "a tag removal", "UntagResource", &workspace_arn);

    // The reporting operations change nothing, so they account for nothing.
    for (case, method, uri) in [
        ("DescribeWorkspace", "GET", format!("/workspaces/{workspace_id}")),
        ("ListWorkspaces", "GET", "/workspaces".to_string()),
        (
            "DescribeWorkspaceConfiguration",
            "GET",
            format!("/workspaces/{workspace_id}/configuration"),
        ),
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{}", encode_label(&workspace_arn)),
        ),
        ("GetDefaultScraperConfiguration", "GET", "/scraperconfiguration".to_string()),
        ("ListScrapers", "GET", "/scrapers".to_string()),
    ] {
        let reply = amp.call(method, &uri, region, "").await;
        assert_status(case, &reply, 200);
        assert_not_audited(capture, case);
    }

    wait_for_due_transition();

    let deleted = amp.delete(region, &workspace_id, Some("ws-delete")).await;
    assert_status("DeleteWorkspace", &deleted, 202);
    assert_audited(capture, "a delete", "DeleteWorkspace", &workspace_arn);

    // The same delete again, while the removal the first one started is still owed: it
    // writes nothing, so it records nothing.
    let repeated = amp.delete(region, &workspace_id, Some("ws-delete")).await;
    assert_status("DeleteWorkspace replay", &repeated, 202);
    assert_not_audited(capture, "a replayed delete");
}

/// The anomaly detector mutators — the create, both of `PutAnomalyDetector`'s paths and
/// the delete — each leave one record naming the DETECTOR's ARN, and the detector reads
/// leave none.
#[tokio::test]
async fn anomaly_detector_mutations_are_audited() {
    let region = "eu-west-1";
    let amp = Amp::new();
    let (workspace_id, _) = active_workspace(&amp, region, "detectors").await;
    let capture = capture_events();

    let spec = Spec::new("sum(rate(requests_total[5m]))");
    let mut create = spec.create_body("audited-detector", &[("team", "metrics")]);
    create["clientToken"] = json!("detector-token");
    let created = create_detector(&amp, region, &workspace_id, &create).await;
    assert_status("CreateAnomalyDetector", &created, 202);
    let detector_id = text(&created.body["anomalyDetectorId"]);
    let detector_arn = text(&created.body["arn"]);
    assert_audited(
        capture,
        "a detector create",
        "CreateAnomalyDetector",
        &detector_arn,
    );

    let replayed = create_detector(&amp, region, &workspace_id, &create).await;
    assert_status("CreateAnomalyDetector replay", &replayed, 202);
    assert_not_audited(capture, "a replayed detector create");

    wait_for_due_transition();

    // `PutAnomalyDetector` on an id the workspace already holds: the UPDATE path.
    let mut update = Spec::new("avg(rate(requests_total[1m]))").put_body();
    update["clientToken"] = json!("detector-update");
    let updated = put_detector(&amp, region, &workspace_id, &detector_id, &update).await;
    assert_status("PutAnomalyDetector update", &updated, 202);
    assert_audited(
        capture,
        "a detector update",
        "PutAnomalyDetector",
        &detector_arn,
    );

    let replayed_update = put_detector(&amp, region, &workspace_id, &detector_id, &update).await;
    assert_status("PutAnomalyDetector replay", &replayed_update, 202);
    assert_not_audited(capture, "a replayed detector update");

    // `PutAnomalyDetector` on an id the workspace holds nothing for: the CREATE path,
    // which is the same work `CreateAnomalyDetector` does.
    let minted = "ad-audited-by-put";
    let put_created = put_detector(
        &amp,
        region,
        &workspace_id,
        minted,
        &Spec::new("max(node_memory_bytes)").put_body(),
    )
    .await;
    assert_status("PutAnomalyDetector create", &put_created, 202);
    let put_arn = text(&put_created.body["arn"]);
    assert_audited(
        capture,
        "a detector created by put",
        "PutAnomalyDetector",
        &put_arn,
    );

    for (case, uri) in [
        (
            "DescribeAnomalyDetector",
            format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
        ),
        (
            "ListAnomalyDetectors",
            format!("/workspaces/{workspace_id}/anomalydetectors"),
        ),
    ] {
        let reply = amp.call("GET", &uri, region, "").await;
        assert_status(case, &reply, 200);
        assert_not_audited(capture, case);
    }

    // A detector is deletable only once the retraining the update started has settled
    // ("Deletion is disallowed until status is ACTIVE"), so let the clock land it first.
    wait_for_due_transition();

    let deleted = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
            region,
            "",
        )
        .await;
    assert_status("DeleteAnomalyDetector", &deleted, 202);
    assert_audited(
        capture,
        "a detector delete",
        "DeleteAnomalyDetector",
        &detector_arn,
    );

    let repeated = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
            region,
            "",
        )
        .await;
    assert_status("DeleteAnomalyDetector repeat", &repeated, 202);
    assert_not_audited(capture, "a repeated detector delete");
}

/// The rule groups namespace mutators each leave one record naming the NAMESPACE's ARN,
/// and the namespace reads leave none.
#[tokio::test]
async fn rule_groups_namespace_mutations_are_audited() {
    let region = "us-east-1";
    let amp = Amp::new();
    let (workspace_id, _) = active_workspace(&amp, region, "namespaces").await;
    let capture = capture_events();

    let name = "audited.rules";
    let created = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body(name, b"groups: []\n", &[]),
    )
    .await;
    assert_status("CreateRuleGroupsNamespace", &created, 202);
    let namespace_arn = text(&created.body["arn"]);
    assert_audited(
        capture,
        "a namespace create",
        "CreateRuleGroupsNamespace",
        &namespace_arn,
    );

    wait_for_due_transition();

    let put = put_namespace(
        &amp,
        region,
        &workspace_id,
        name,
        &put_body(b"groups:\n- name: audited\n"),
    )
    .await;
    assert_status("PutRuleGroupsNamespace", &put, 202);
    assert_audited(
        capture,
        "a namespace update",
        "PutRuleGroupsNamespace",
        &namespace_arn,
    );

    wait_for_due_transition();

    for (case, uri) in [
        (
            "DescribeRuleGroupsNamespace",
            format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}"),
        ),
        (
            "ListRuleGroupsNamespaces",
            format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
        ),
    ] {
        let reply = amp.call("GET", &uri, region, "").await;
        assert_status(case, &reply, 200);
        assert_not_audited(capture, case);
    }

    let deleted = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}?clientToken=ns-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteRuleGroupsNamespace", &deleted, 202);
    assert_audited(
        capture,
        "a namespace delete",
        "DeleteRuleGroupsNamespace",
        &namespace_arn,
    );

    let repeated = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}?clientToken=ns-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteRuleGroupsNamespace replay", &repeated, 202);
    assert_not_audited(capture, "a replayed namespace delete");
}

/// A workspace's rules-and-alerting logging configuration has no ARN of its own, so each
/// of its three mutators records the WORKSPACE it hangs off; the describe records nothing.
#[tokio::test]
async fn logging_configuration_mutations_are_audited_under_the_workspace() {
    let region = "us-west-2";
    let amp = Amp::new();
    let (workspace_id, workspace_arn) = active_workspace(&amp, region, "logging").await;
    let capture = capture_events();

    let first = log_group_arn("aws", region, "aws/aps/audited");
    let created = create_logging(&amp, region, &workspace_id, &first, Some("log-create")).await;
    assert_status("CreateLoggingConfiguration", &created, 202);
    assert_audited(
        capture,
        "a logging create",
        "CreateLoggingConfiguration",
        &workspace_arn,
    );

    let replayed = create_logging(&amp, region, &workspace_id, &first, Some("log-create")).await;
    assert_status("CreateLoggingConfiguration replay", &replayed, 202);
    assert_not_audited(capture, "a replayed logging create");

    wait_for_due_transition();

    let second = log_group_arn("aws", region, "aws/aps/audited-2");
    let updated = amp
        .call(
            "PUT",
            &format!("/workspaces/{workspace_id}/logging"),
            region,
            &json!({ "logGroupArn": second, "clientToken": "log-update" }).to_string(),
        )
        .await;
    assert_status("UpdateLoggingConfiguration", &updated, 202);
    assert_audited(
        capture,
        "a logging update",
        "UpdateLoggingConfiguration",
        &workspace_arn,
    );

    let replayed_update = amp
        .call(
            "PUT",
            &format!("/workspaces/{workspace_id}/logging"),
            region,
            &json!({ "logGroupArn": second, "clientToken": "log-update" }).to_string(),
        )
        .await;
    assert_status("UpdateLoggingConfiguration replay", &replayed_update, 202);
    assert_not_audited(capture, "a replayed logging update");

    wait_for_due_transition();

    let described = amp
        .call("GET", &format!("/workspaces/{workspace_id}/logging"), region, "")
        .await;
    assert_status("DescribeLoggingConfiguration", &described, 200);
    assert_not_audited(capture, "DescribeLoggingConfiguration");

    let deleted = delete_logging(&amp, region, &workspace_id, Some("log-delete")).await;
    assert_status("DeleteLoggingConfiguration", &deleted, 202);
    assert_audited(
        capture,
        "a logging delete",
        "DeleteLoggingConfiguration",
        &workspace_arn,
    );

    let repeated = delete_logging(&amp, region, &workspace_id, Some("log-delete")).await;
    assert_status("DeleteLoggingConfiguration replay", &repeated, 202);
    assert_not_audited(capture, "a replayed logging delete");
}

/// The query logging configuration is the other per-workspace configuration, and its
/// three mutators follow the same rule: one record under the workspace's ARN each.
#[tokio::test]
async fn query_logging_configuration_mutations_are_audited_under_the_workspace() {
    let region = "ap-southeast-2";
    let amp = Amp::new();
    let (workspace_id, workspace_arn) = active_workspace(&amp, region, "query-logging").await;
    let capture = capture_events();

    let uri = format!("/workspaces/{workspace_id}/logging/query");
    let created = amp
        .call(
            "POST",
            &uri,
            region,
            &query_logging_body(region, "query-logs", Some("qlog-create")).to_string(),
        )
        .await;
    assert_status("CreateQueryLoggingConfiguration", &created, 202);
    assert_audited(
        capture,
        "a query logging create",
        "CreateQueryLoggingConfiguration",
        &workspace_arn,
    );

    let replayed = amp
        .call(
            "POST",
            &uri,
            region,
            &query_logging_body(region, "query-logs", Some("qlog-create")).to_string(),
        )
        .await;
    assert_status("CreateQueryLoggingConfiguration replay", &replayed, 202);
    assert_not_audited(capture, "a replayed query logging create");

    wait_for_due_transition();

    let updated = amp
        .call(
            "PUT",
            &uri,
            region,
            &query_logging_body(region, "query-logs-2", Some("qlog-update")).to_string(),
        )
        .await;
    assert_status("UpdateQueryLoggingConfiguration", &updated, 202);
    assert_audited(
        capture,
        "a query logging update",
        "UpdateQueryLoggingConfiguration",
        &workspace_arn,
    );

    let replayed_update = amp
        .call(
            "PUT",
            &uri,
            region,
            &query_logging_body(region, "query-logs-2", Some("qlog-update")).to_string(),
        )
        .await;
    assert_status("UpdateQueryLoggingConfiguration replay", &replayed_update, 202);
    assert_not_audited(capture, "a replayed query logging update");

    wait_for_due_transition();

    let described = amp.call("GET", &uri, region, "").await;
    assert_status("DescribeQueryLoggingConfiguration", &described, 200);
    assert_not_audited(capture, "DescribeQueryLoggingConfiguration");

    let deleted = amp
        .call("DELETE", &format!("{uri}?clientToken=qlog-delete"), region, "")
        .await;
    assert_status("DeleteQueryLoggingConfiguration", &deleted, 202);
    assert_audited(
        capture,
        "a query logging delete",
        "DeleteQueryLoggingConfiguration",
        &workspace_arn,
    );

    let repeated = amp
        .call("DELETE", &format!("{uri}?clientToken=qlog-delete"), region, "")
        .await;
    assert_status("DeleteQueryLoggingConfiguration repeat", &repeated, 202);
    assert_not_audited(capture, "a repeated query logging delete");
}

/// The alert manager definition and the resource-based policy are the remaining
/// per-workspace resources: every mutator of either records the workspace's ARN, and
/// their describes record nothing.
#[tokio::test]
async fn definition_and_policy_mutations_are_audited_under_the_workspace() {
    let region = "us-west-2";
    let amp = Amp::new();
    let (workspace_id, workspace_arn) = active_workspace(&amp, region, "definition").await;
    let capture = capture_events();

    let definition_uri = format!("/workspaces/{workspace_id}/alertmanager/definition");
    let created = amp
        .call(
            "POST",
            &definition_uri,
            region,
            &definition_body(&alert_manager_definition("default"), Some("def-create")).to_string(),
        )
        .await;
    assert_status("CreateAlertManagerDefinition", &created, 202);
    assert_audited(
        capture,
        "a definition create",
        "CreateAlertManagerDefinition",
        &workspace_arn,
    );

    let replayed = amp
        .call(
            "POST",
            &definition_uri,
            region,
            &definition_body(&alert_manager_definition("default"), Some("def-create")).to_string(),
        )
        .await;
    assert_status("CreateAlertManagerDefinition replay", &replayed, 202);
    assert_not_audited(capture, "a replayed definition create");

    wait_for_due_transition();

    let put = amp
        .call(
            "PUT",
            &definition_uri,
            region,
            &definition_body(&alert_manager_definition("pager"), Some("def-put")).to_string(),
        )
        .await;
    assert_status("PutAlertManagerDefinition", &put, 202);
    assert_audited(
        capture,
        "a definition update",
        "PutAlertManagerDefinition",
        &workspace_arn,
    );

    let replayed_put = amp
        .call(
            "PUT",
            &definition_uri,
            region,
            &definition_body(&alert_manager_definition("pager"), Some("def-put")).to_string(),
        )
        .await;
    assert_status("PutAlertManagerDefinition replay", &replayed_put, 202);
    assert_not_audited(capture, "a replayed definition update");

    wait_for_due_transition();

    let described = amp.call("GET", &definition_uri, region, "").await;
    assert_status("DescribeAlertManagerDefinition", &described, 200);
    assert_not_audited(capture, "DescribeAlertManagerDefinition");

    let deleted = amp
        .call(
            "DELETE",
            &format!("{definition_uri}?clientToken=def-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteAlertManagerDefinition", &deleted, 202);
    assert_audited(
        capture,
        "a definition delete",
        "DeleteAlertManagerDefinition",
        &workspace_arn,
    );

    let repeated = amp
        .call(
            "DELETE",
            &format!("{definition_uri}?clientToken=def-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteAlertManagerDefinition replay", &repeated, 202);
    assert_not_audited(capture, "a replayed definition delete");

    wait_for_due_transition();

    let policy_uri = format!("/workspaces/{workspace_id}/policy");
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "Audited",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": ["aps:QueryMetrics"],
            "Resource": "*",
        }],
    })
    .to_string();
    let attached = amp
        .call(
            "PUT",
            &policy_uri,
            region,
            &json!({ "policyDocument": document, "clientToken": "policy-put" }).to_string(),
        )
        .await;
    assert_status("PutResourcePolicy", &attached, 202);
    assert_audited(
        capture,
        "a policy put",
        "PutResourcePolicy",
        &workspace_arn,
    );

    let replayed_policy = amp
        .call(
            "PUT",
            &policy_uri,
            region,
            &json!({ "policyDocument": document, "clientToken": "policy-put" }).to_string(),
        )
        .await;
    assert_status("PutResourcePolicy replay", &replayed_policy, 202);
    assert_not_audited(capture, "a replayed policy put");

    wait_for_due_transition();

    let described_policy = amp.call("GET", &policy_uri, region, "").await;
    assert_status("DescribeResourcePolicy", &described_policy, 200);
    assert_not_audited(capture, "DescribeResourcePolicy");

    let detached = amp
        .call(
            "DELETE",
            &format!("{policy_uri}?clientToken=policy-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteResourcePolicy", &detached, 202);
    assert_audited(
        capture,
        "a policy delete",
        "DeleteResourcePolicy",
        &workspace_arn,
    );

    let repeated_detach = amp
        .call(
            "DELETE",
            &format!("{policy_uri}?clientToken=policy-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteResourcePolicy replay", &repeated_detach, 202);
    assert_not_audited(capture, "a replayed policy delete");
}

/// The scraper mutators leave one record naming the SCRAPER's ARN — the create, the
/// update and the delete — and so does the deletion of the logging configuration a
/// scraper holds, which has no ARN of its own. The scraper reads leave none.
#[tokio::test]
async fn scraper_mutations_are_audited() {
    let region = "us-west-2";
    let amp = Amp::new();
    let (_, workspace_arn) = active_workspace(&amp, region, "scrapers").await;
    let capture = capture_events();

    let created = amp
        .call(
            "POST",
            "/scrapers",
            region,
            &json!({
                "alias": "audited",
                "scrapeConfiguration": scrape_configuration_json("global:\n  scrape_interval: 30s\n"),
                "source": eks_source_json(region, "audited-cluster", &["subnet-0123456789abcdef0"], &[]),
                "destination": workspace_destination_json(&workspace_arn),
                "clientToken": "scraper-create",
            })
            .to_string(),
        )
        .await;
    assert_status("CreateScraper", &created, 202);
    let scraper_id = text(&created.body["scraperId"]);
    let scraper_arn = text(&created.body["arn"]);
    assert_audited(capture, "a scraper create", "CreateScraper", &scraper_arn);

    wait_for_due_transition();

    let updated = amp
        .call(
            "PUT",
            &format!("/scrapers/{scraper_id}"),
            region,
            &json!({ "alias": "audited-again", "clientToken": "scraper-update" }).to_string(),
        )
        .await;
    assert_status("UpdateScraper", &updated, 202);
    assert_audited(capture, "a scraper update", "UpdateScraper", &scraper_arn);

    let replayed_update = amp
        .call(
            "PUT",
            &format!("/scrapers/{scraper_id}"),
            region,
            &json!({ "alias": "audited-again", "clientToken": "scraper-update" }).to_string(),
        )
        .await;
    assert_status("UpdateScraper replay", &replayed_update, 202);
    assert_not_audited(capture, "a replayed scraper update");

    wait_for_due_transition();

    for (case, uri) in [
        ("DescribeScraper", format!("/scrapers/{scraper_id}")),
        ("ListScrapers", "/scrapers".to_string()),
    ] {
        let reply = amp.call("GET", &uri, region, "").await;
        assert_status(case, &reply, 200);
        assert_not_audited(capture, case);
    }

    let deleted = amp
        .call(
            "DELETE",
            &format!("/scrapers/{scraper_id}?clientToken=scraper-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteScraper", &deleted, 202);
    assert_audited(capture, "a scraper delete", "DeleteScraper", &scraper_arn);

    let repeated = amp
        .call(
            "DELETE",
            &format!("/scrapers/{scraper_id}?clientToken=scraper-delete"),
            region,
            "",
        )
        .await;
    assert_status("DeleteScraper replay", &repeated, 202);
    assert_not_audited(capture, "a replayed scraper delete");
}

/// `DeleteScraperLoggingConfiguration` records the SCRAPER it belongs to, and its repeat
/// — which writes nothing while the removal is already owed — records nothing.
///
/// The pair is seeded through the engine the mounted service reads, with the builders the
/// deletion suite uses: `UpdateScraperLoggingConfiguration` is still scaffolded, so
/// seeding is how a scraper with a logging configuration exists to delete at all.
#[tokio::test]
async fn scraper_logging_configuration_deletion_is_audited() {
    let region = "us-west-2";
    let amp = Amp::new();
    let scraper_id = "s-audited1-1111-1111-1111-111111111111";
    let host = seed_pair(
        &amp,
        region,
        &logging_configuration(
            scraper_id,
            service_com_amazonaws_amp_amazonprometheusservice::state::SCRAPER_LOGGING_STATUS_ACTIVE,
            cloud_watch_logs("aws/prometheus/audited"),
            components(&["COLLECTOR"]),
        ),
        "ws-audited1-1111-1111-1111-111111111111",
    );
    let capture = capture_events();

    let uri = format!("/scrapers/{scraper_id}/logging-configuration");
    let described = amp.call("GET", &uri, region, "").await;
    assert_status("DescribeScraperLoggingConfiguration", &described, 200);
    assert_not_audited(capture, "DescribeScraperLoggingConfiguration");

    let deleted = amp
        .call("DELETE", &format!("{uri}?clientToken=scraper-log"), region, "")
        .await;
    assert_status("DeleteScraperLoggingConfiguration", &deleted, 202);
    assert_audited(
        capture,
        "a scraper logging delete",
        "DeleteScraperLoggingConfiguration",
        &host.arn,
    );

    let repeated = amp
        .call("DELETE", &format!("{uri}?clientToken=scraper-log"), region, "")
        .await;
    assert_status("DeleteScraperLoggingConfiguration repeat", &repeated, 202);
    assert_not_audited(capture, "a repeated scraper logging delete");
}

/// A REFUSED mutation writes nothing, so it records nothing — one row per kind of
/// refusal a mutating operation answers: an unresolvable parent (404), an ARN that names
/// no live resource (404), a member outside its constraint (400) and a state the
/// operation forbids (409).
#[tokio::test]
async fn refused_mutations_leave_no_audit_record() {
    let region = "us-west-2";
    let amp = Amp::new();
    let (workspace_id, workspace_arn) = active_workspace(&amp, region, "refusals").await;
    let absent = "ws-00000000-0000-0000-0000-000000000000";
    let absent_arn = format!(
        "arn:aws:aps:{region}:{}:workspace/{absent}",
        engine::emulator_account_id()
    );
    let capture = capture_events();

    // (case, method, uri, body, expected status)
    let cases: Vec<(&str, &str, String, String, u16)> = vec![
        (
            "a namespace create inside a workspace nothing holds",
            "POST",
            format!("/workspaces/{absent}/rulegroupsnamespaces"),
            create_body("refused", b"groups: []\n", &[]).to_string(),
            404,
        ),
        (
            "a tag merge on an ARN that names no live resource",
            "POST",
            format!("/tags/{}", encode_label(&absent_arn)),
            json!({ "tags": { "team": "metrics" } }).to_string(),
            404,
        ),
        (
            "a tag removal on an ARN that names no live resource",
            "DELETE",
            format!("/tags/{}?tagKeys=team", encode_label(&absent_arn)),
            String::new(),
            404,
        ),
        (
            // `UpdateWorkspaceConfiguration` documents the rule the model cannot
            // express: at least one of the two members must be named.
            "a configuration update naming no member",
            "PATCH",
            format!("/workspaces/{workspace_id}/configuration"),
            json!({}).to_string(),
            400,
        ),
        (
            // The workspace holds no definition yet, so there is nothing to update.
            "an alert manager update on a workspace that holds no definition",
            "PUT",
            format!("/workspaces/{workspace_id}/alertmanager/definition"),
            definition_body(&alert_manager_definition("nobody"), None).to_string(),
            404,
        ),
        (
            // The workspace holds no logging configuration, so there is none to delete.
            "a logging delete on a workspace that holds none",
            "DELETE",
            format!("/workspaces/{workspace_id}/logging"),
            String::new(),
            404,
        ),
        (
            "a policy delete on a workspace that holds none",
            "DELETE",
            format!("/workspaces/{workspace_id}/policy"),
            String::new(),
            404,
        ),
        (
            "a scraper delete for an id nothing holds",
            "DELETE",
            "/scrapers/s-00000000-0000-0000-0000-000000000000".to_string(),
            String::new(),
            404,
        ),
    ];

    for (case, method, uri, body, expected) in cases {
        let reply = amp.call(method, &uri, region, &body).await;
        assert_status(case, &reply, expected);
        assert_not_audited(capture, case);
    }

    // A conflict is a refusal too: a namespace create is disallowed while the workspace
    // itself is being deleted.
    let deleted = amp.delete(region, &workspace_id, None).await;
    assert_status("DeleteWorkspace", &deleted, 202);
    assert_audited(
        capture,
        "the delete that makes the conflict",
        "DeleteWorkspace",
        &workspace_arn,
    );

    let conflicted = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body("refused", b"groups: []\n", &[]),
    )
    .await;
    assert_status("CreateRuleGroupsNamespace conflict", &conflicted, 409);
    assert_not_audited(capture, "a namespace create refused by the workspace status");
}
