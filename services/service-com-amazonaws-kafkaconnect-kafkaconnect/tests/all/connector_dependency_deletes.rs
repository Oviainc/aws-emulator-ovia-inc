// SPDX-License-Identifier: CC0-1.0

//! What a delete answers for a resource a stored connector runs on, end to end
//! over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler that answered from the request rather than from state would fail
//! here.
//!
//! The rule under test, asserted over a SET of inputs rather than the one
//! request the issue cited: a resource a connector this account holds names —
//! one of the custom plugins of its `plugins`, or the worker configuration it
//! runs with — is not removed while that connector is stored, because a
//! connector is only ever stored against a reference that resolves, in its own
//! region, to an ACTIVE resource. So every ARN `DescribeConnector` echoes back
//! resolves through that resource's own read, and the delete of a named resource
//! is refused instead with the modelled `BadRequestException` (HTTP 400), on the
//! member that carried the ARN.
//!
//! The rows vary along what that rule generalises over: the KIND of dependency
//! (a custom plugin and a worker configuration, the two references a connector
//! stores), whether the connector names the other kind as well, the COUNT of
//! connectors naming one resource, and the STATE of the naming connector (one
//! still being created, one running, and one whose own delete is already in
//! flight). The complement is asserted too: a resource of the same kind that no
//! connector names is deleted while the named one is refused, and the refused
//! delete is accepted once the clock has dropped the last connector that named
//! it.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// carries. No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `message` a failure response carries, or `""`.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Wait past the span the clock owes a transition, so every change due has
/// landed by the next call.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// A well-formed IAM role ARN, as every accepted `CreateConnector` carries.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/connector-role")
}

/// A `CreateConnector` body naming `plugins` and, when given, a worker
/// configuration.
fn connector_body(name: &str, plugins: &[String], worker: Option<&str>) -> Value {
    let mut body = json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {"tasks.max": "1"},
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": "b-1.example:9098",
            "vpc": {"subnets": ["subnet-1", "subnet-2"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "IAM"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": plugins
            .iter()
            .map(|arn| json!({"customPlugin": {"customPluginArn": arn, "revision": 1}}))
            .collect::<Vec<Value>>(),
        "serviceExecutionRoleArn": role_arn(),
    });
    if let Some(worker) = worker {
        body["workerConfiguration"] = json!({"workerConfigurationArn": worker, "revision": 1});
    }
    body
}

/// A custom plugin created and settled into ACTIVE, with its ARN: a connector
/// can only be created against an ACTIVE plugin, so every row below needs one.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/custom-plugins",
        json!({
            "name": name,
            "contentType": "ZIP",
            "location": {"s3Location": {
                "bucketArn": "arn:aws:s3:::plugin-bucket",
                "fileKey": "plugins/p.zip",
            }},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the plugin {name} must succeed, got {status} {created}"
    );
    let arn = text(&created, "customPluginArn");
    past_the_transition().await;
    arn
}

/// A worker configuration created (ACTIVE at once), with its ARN.
async fn worker_configuration(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/worker-configurations",
        // `QUJD` is base64 of `ABC`: the body is stored verbatim, so any legal
        // base64 will do here.
        json!({"name": name, "propertiesFileContent": "QUJD"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the worker configuration {name} must succeed, got {status} {created}"
    );
    text(&created, "workerConfigurationArn")
}

/// Create a connector on `plugins` (and `worker`), with its ARN.
async fn connector(
    emulator: &MultiServiceRouter,
    name: &str,
    plugins: &[String],
    worker: Option<&str>,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/connectors",
        connector_body(name, plugins, worker),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the connector {name} must succeed, got {status} {created}"
    );
    text(&created, "connectorArn")
}

/// The two references a connector stores, and so the two resources whose delete
/// this rule governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dependency {
    CustomPlugin,
    WorkerConfiguration,
}

impl Dependency {
    /// The request member that carries this resource's ARN, which a refusal
    /// blames.
    fn member(self) -> &'static str {
        match self {
            Self::CustomPlugin => "customPluginArn",
            Self::WorkerConfiguration => "workerConfigurationArn",
        }
    }

    /// The collection route this resource's delete and read sit under.
    fn route(self) -> &'static str {
        match self {
            Self::CustomPlugin => "custom-plugins",
            Self::WorkerConfiguration => "worker-configurations",
        }
    }

    /// The message a delete of a resource of this kind that a connector names is
    /// refused with.
    fn in_use_message(self) -> String {
        let resource = match self {
            Self::CustomPlugin => "custom plugin",
            Self::WorkerConfiguration => "worker configuration",
        };
        format!(
            "Invalid parameter {}: The {resource} is in use by a connector.",
            self.member()
        )
    }

    /// The member of this resource's own read that reports its state.
    fn state_member(self) -> &'static str {
        match self {
            Self::CustomPlugin => "customPluginState",
            Self::WorkerConfiguration => "workerConfigurationState",
        }
    }
}

/// `DELETE` the resource `arn` names, through its own route.
async fn delete_dependency(
    emulator: &MultiServiceRouter,
    kind: Dependency,
    arn: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v1/{}/{}", kind.route(), label(arn)),
        Value::Null,
    )
    .await
}

/// `GET` the resource `arn` names, through its own route.
async fn read_dependency(
    emulator: &MultiServiceRouter,
    kind: Dependency,
    arn: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v1/{}/{}", kind.route(), label(arn)),
        Value::Null,
    )
    .await
}

/// Assert that every ARN `DescribeConnector` echoes for `connector_arn` resolves
/// through the resource's own read — the invariant the refusal protects.
async fn assert_every_reference_resolves(
    emulator: &MultiServiceRouter,
    case: &str,
    connector_arn: &str,
) {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the connector must still read back, got {status} {described}"
    );
    let plugins = described["plugins"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|plugin| text(&plugin["customPlugin"], "customPluginArn"))
        .collect::<Vec<String>>();
    assert!(
        !plugins.is_empty(),
        "case {case:?}: the connector must report the plugins it runs on, got {described}"
    );
    for arn in &plugins {
        let (status, answered) = read_dependency(emulator, Dependency::CustomPlugin, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the plugin {arn:?} the connector reports must resolve, got {status} \
             {answered}"
        );
    }
    let reported_worker = described["workerConfiguration"]["workerConfigurationArn"]
        .as_str()
        .map(str::to_string);
    if let Some(arn) = reported_worker {
        let (status, answered) =
            read_dependency(emulator, Dependency::WorkerConfiguration, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the worker configuration {arn:?} the connector reports must resolve, \
             got {status} {answered}"
        );
    }
}

/// One row of the table: which resource a connector names, whether it names the
/// other kind too, how many connectors name it, and what state they are in when
/// the delete is tried.
///
/// A connector's `plugins` is "a single-element list containing one customPlugin
/// object" (`CreateConnectorRequest$plugins`), so a connector names exactly one
/// plugin and at most one worker configuration: the counts a row varies are the
/// number of CONNECTORS naming one resource, not the number of references one
/// connector carries.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    kind: Dependency,
    /// Whether the connectors also name a worker configuration — so the rule
    /// holds for each reference a connector carries, not only for the kind the
    /// delete names.
    with_worker: bool,
    /// How many connectors name the resource.
    connectors: usize,
    /// Whether the naming connectors are left in CREATING (the delete is tried
    /// straight after the create), or settled into RUNNING first.
    settled: bool,
    /// Whether the naming connectors' own delete is started first, so they are
    /// DELETING but still stored.
    deleting: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "one running connector naming only the plugin",
        kind: Dependency::CustomPlugin,
        with_worker: false,
        connectors: 1,
        settled: true,
        deleting: false,
    },
    Row {
        case: "one connector still being created, naming only the plugin",
        kind: Dependency::CustomPlugin,
        with_worker: false,
        connectors: 1,
        settled: false,
        deleting: false,
    },
    Row {
        case: "one running connector naming the plugin and a worker configuration",
        kind: Dependency::CustomPlugin,
        with_worker: true,
        connectors: 1,
        settled: true,
        deleting: false,
    },
    Row {
        case: "three running connectors on one plugin",
        kind: Dependency::CustomPlugin,
        with_worker: false,
        connectors: 3,
        settled: true,
        deleting: false,
    },
    Row {
        case: "one connector whose own delete is already in flight, the plugin",
        kind: Dependency::CustomPlugin,
        with_worker: false,
        connectors: 1,
        settled: true,
        deleting: true,
    },
    Row {
        case: "one running connector naming the worker configuration",
        kind: Dependency::WorkerConfiguration,
        with_worker: true,
        connectors: 1,
        settled: true,
        deleting: false,
    },
    Row {
        case: "one connector still being created, naming the worker configuration",
        kind: Dependency::WorkerConfiguration,
        with_worker: true,
        connectors: 1,
        settled: false,
        deleting: false,
    },
    Row {
        case: "two running connectors on one worker configuration",
        kind: Dependency::WorkerConfiguration,
        with_worker: true,
        connectors: 2,
        settled: true,
        deleting: false,
    },
    Row {
        case: "one connector whose own delete is already in flight, the worker configuration",
        kind: Dependency::WorkerConfiguration,
        with_worker: true,
        connectors: 1,
        settled: true,
        deleting: true,
    },
];

#[tokio::test]
async fn a_resource_a_stored_connector_runs_on_is_not_deleted() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();

        // The one plugin the connectors run on, and the worker configuration
        // when the row names one.
        let plugin = active_plugin(&emulator, &format!("plugin-{index}")).await;
        let worker = if row.with_worker {
            Some(worker_configuration(&emulator, &format!("worker-{index}")).await)
        } else {
            None
        };
        let under_delete = match row.kind {
            Dependency::CustomPlugin => plugin.clone(),
            Dependency::WorkerConfiguration => worker
                .clone()
                .expect("a worker-configuration row names one"),
        };
        let plugins = [plugin];

        let mut connector_arns = Vec::new();
        for slot in 0..row.connectors {
            connector_arns.push(
                connector(
                    &emulator,
                    &format!("connector-{index}-{slot}"),
                    &plugins,
                    worker.as_deref(),
                )
                .await,
            );
        }
        if row.settled {
            past_the_transition().await;
        }
        if row.deleting {
            for arn in &connector_arns {
                let (status, deleted) = call(
                    &emulator,
                    "DELETE",
                    &format!("/v1/connectors/{}", label(arn)),
                    Value::Null,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the connector's own delete must be accepted, got {status} \
                     {deleted}"
                );
            }
        }

        // The delete of the resource those connectors run on is refused, on the
        // member that carried its ARN.
        let (status, answered) = delete_dependency(&emulator, row.kind, &under_delete).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting a resource a stored connector names must be refused, got \
             {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {answered}"
        );
        let expected = row.kind.in_use_message();
        let actual = message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );

        // The refusal wrote nothing: the resource is still readable, still
        // ACTIVE, and no clock-owed removal was started — so it is still there
        // once the span a delete would have taken has passed.
        let (status, described) = read_dependency(&emulator, row.kind, &under_delete).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the resource readable, got {status} \
             {described}"
        );
        assert_eq!(
            text(&described, row.kind.state_member()),
            "ACTIVE",
            "case {case:?}: a refused delete must not move the resource's state, got {described}"
        );
        past_the_transition().await;
        let (status, described) = read_dependency(&emulator, row.kind, &under_delete).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must not drop the resource on the clock, got \
             {status} {described}"
        );

        // And the invariant the refusal protects holds for every connector that
        // is still stored: every ARN the reads echo resolves. A row that put its
        // connectors into DELETING has had them dropped by the wait above, so
        // there is nothing left to read for those.
        if !row.deleting {
            for arn in &connector_arns {
                assert_every_reference_resolves(&emulator, case, arn).await;
            }
        }
    }
}

#[tokio::test]
async fn a_resource_no_connector_runs_on_is_deleted() {
    for kind in [Dependency::CustomPlugin, Dependency::WorkerConfiguration] {
        let case = format!("{kind:?}");
        let emulator = emulator();

        // Two resources of the same kind, in the same region, of which the
        // connector names only the first.
        let named_plugin = active_plugin(&emulator, "named-plugin").await;
        let spare_plugin = active_plugin(&emulator, "spare-plugin").await;
        let named_worker = worker_configuration(&emulator, "named-worker").await;
        let spare_worker = worker_configuration(&emulator, "spare-worker").await;
        let (named, spare) = match kind {
            Dependency::CustomPlugin => (named_plugin.clone(), spare_plugin.clone()),
            Dependency::WorkerConfiguration => (named_worker.clone(), spare_worker.clone()),
        };
        let connector_arn = connector(
            &emulator,
            "one-connector",
            std::slice::from_ref(&named_plugin),
            Some(named_worker.as_str()),
        )
        .await;
        past_the_transition().await;

        // The resource no connector names is deleted while the named one is
        // refused: the refusal keys on the reference, not on this account
        // holding connectors at all.
        let (status, deleted) = delete_dependency(&emulator, kind, &spare).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting a resource no connector names must be accepted, got \
             {status} {deleted}"
        );
        assert_eq!(
            text(&deleted, kind.state_member()),
            "DELETING",
            "case {case:?}: the accepted delete must report DELETING, got {deleted}"
        );
        let (status, answered) = delete_dependency(&emulator, kind, &named).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the named resource must still be refused, got {status} {answered}"
        );

        // Once the connector that named it is gone, the same delete is accepted
        // and the clock drops the resource: the store never holds a connector
        // naming a resource it does not hold.
        let (status, deleted) = call(
            &emulator,
            "DELETE",
            &format!("/v1/connectors/{}", label(&connector_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the connector's delete must be accepted, got {status} {deleted}"
        );

        // While that connector is still stored, DELETING, the refusal stands.
        let (status, answered) = delete_dependency(&emulator, kind, &named).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a DELETING connector still names it, got {status} {answered}"
        );

        past_the_transition().await;
        let (status, deleted) = delete_dependency(&emulator, kind, &named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with no connector left the delete must be accepted, got {status} \
             {deleted}"
        );
        assert_eq!(
            text(&deleted, kind.state_member()),
            "DELETING",
            "case {case:?}: the accepted delete must report DELETING, got {deleted}"
        );
        past_the_transition().await;
        let (status, answered) = read_dependency(&emulator, kind, &named).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the clock must drop the deleted resource, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
        let (status, listed) = call(&emulator, "GET", "/v1/connectors", Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListConnectors must succeed, got {status} {listed}"
        );
        assert_eq!(
            listed["connectors"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no connector may be left naming a dropped resource, got {listed}"
        );
    }
}
