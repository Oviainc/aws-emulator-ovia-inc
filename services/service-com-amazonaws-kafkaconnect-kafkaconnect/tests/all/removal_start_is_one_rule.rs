// SPDX-License-Identifier: CC0-1.0

//! The removal-start rule every delete of this service runs, driven end to end
//! through ALL THREE operations that start one: `DeleteConnector`,
//! `DeleteCustomPlugin` and `DeleteWorkerConfiguration`.
//!
//! RULE. A delete starts the removal of the record it names and leaves the
//! removal itself to the clock: the first call moves the record to its kind's
//! `DELETING` state and owes the removal one `TRANSITION_SPAN` from THAT call,
//! so the record stays readable in that state for the span and is then gone; a
//! repeat while the removal is still owed answers the same ARN and the same
//! `DELETING` state and leaves the due instant where it was, so retrying never
//! postpones the removal.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, with the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! delete running a transition of its own would fail here.
//!
//! The table spans what the rule generalises over: each kind of resource a
//! delete of this service names (one per operation that starts a removal),
//! crossed with how many times the delete is called and how far apart — once,
//! three times back to back, and twice with the repeat landing halfway through
//! the span — and with the state the record was in when the delete landed, its
//! kind's ready state or the transient state a just-created record reports.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::Instant;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    CONNECTOR_DELETING, CUSTOM_PLUGIN_DELETING, TRANSITION_SPAN, WORKER_CONFIGURATION_DELETING,
};
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

/// Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`:
/// a legal `propertiesFileContent`, since no row is about the properties file.
const PROPERTIES: &str =
    "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=";

/// Long enough that one change the clock owes after a call is due. The span is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// The instant a row reads its record back expecting it gone: a margin past one
/// span from the FIRST delete. The margin is shorter than the longest gap a row
/// leaves between its deletes ([`Gap::HalfOfTheSpan`]), so this instant is still
/// EARLIER than one span from any repeat — a delete that restarted the clock
/// would leave the record readable here.
fn observed_gone_at(first_delete: Instant) -> Instant {
    first_delete + TRANSITION_SPAN + TRANSITION_SPAN / 5
}

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

/// A member of a JSON object as a string, or `""`.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `CreateCustomPlugin` body for `name`.
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`.
fn connector_request(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{name}.kafka.{REGION}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
    })
}

/// A kind of resource a delete of this service names: one per operation that
/// starts a removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `DeleteConnector`.
    Connector,
    /// `DeleteCustomPlugin`.
    CustomPlugin,
    /// `DeleteWorkerConfiguration`.
    WorkerConfiguration,
}

impl Kind {
    /// The collection this kind is created in and read back under.
    fn collection(self) -> &'static str {
        match self {
            Kind::Connector => "/v1/connectors",
            Kind::CustomPlugin => "/v1/custom-plugins",
            Kind::WorkerConfiguration => "/v1/worker-configurations",
        }
    }

    /// The output member carrying this kind's ARN.
    fn arn_member(self) -> &'static str {
        match self {
            Kind::Connector => "connectorArn",
            Kind::CustomPlugin => "customPluginArn",
            Kind::WorkerConfiguration => "workerConfigurationArn",
        }
    }

    /// The output member carrying this kind's lifecycle state.
    fn state_member(self) -> &'static str {
        match self {
            Kind::Connector => "connectorState",
            Kind::CustomPlugin => "customPluginState",
            Kind::WorkerConfiguration => "workerConfigurationState",
        }
    }

    /// The state this kind reports while its removal is owed, taken from the
    /// service's own constants so the expectation cannot drift from them.
    fn deleting(self) -> &'static str {
        match self {
            Kind::Connector => CONNECTOR_DELETING,
            Kind::CustomPlugin => CUSTOM_PLUGIN_DELETING,
            Kind::WorkerConfiguration => WORKER_CONFIGURATION_DELETING,
        }
    }
}

/// How long a row leaves between its deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// One call straight after the other, as a retrying client sends them.
    BackToBack,
    /// Halfway through the span, so a repeat that restarted the clock would be
    /// visible at [`observed_gone_at`].
    HalfOfTheSpan,
}

impl Gap {
    fn duration(self) -> Duration {
        match self {
            Gap::BackToBack => Duration::ZERO,
            Gap::HalfOfTheSpan => TRANSITION_SPAN / 2,
        }
    }
}

/// One row: a kind, whether its record had reached its kind's ready state
/// before the delete landed, and the delete calls the row makes.
struct Case {
    kind: Kind,
    /// Whether the row waits for the clock to bring the record up before
    /// deleting it. `false` deletes a record still in the transient state its
    /// create left it in, which the rule treats no differently.
    settled: bool,
    deletes: usize,
    gap: Gap,
}

impl Case {
    /// This row's name, carried by every assertion message.
    fn label(&self) -> String {
        format!(
            "{:?}/{}-deletes/{:?}/{}",
            self.kind,
            self.deletes,
            self.gap,
            if self.settled { "ready" } else { "transient" }
        )
    }
}

/// Every kind of resource a delete names, crossed with the delete programs the
/// rule must answer identically.
const CASES: &[Case] = &[
    Case {
        kind: Kind::Connector,
        settled: true,
        deletes: 2,
        gap: Gap::HalfOfTheSpan,
    },
    Case {
        kind: Kind::Connector,
        settled: false,
        deletes: 3,
        gap: Gap::BackToBack,
    },
    Case {
        kind: Kind::CustomPlugin,
        settled: true,
        deletes: 2,
        gap: Gap::HalfOfTheSpan,
    },
    Case {
        kind: Kind::CustomPlugin,
        settled: false,
        deletes: 1,
        gap: Gap::BackToBack,
    },
    Case {
        kind: Kind::WorkerConfiguration,
        settled: true,
        deletes: 2,
        gap: Gap::HalfOfTheSpan,
    },
    Case {
        kind: Kind::WorkerConfiguration,
        settled: true,
        deletes: 3,
        gap: Gap::BackToBack,
    },
];

/// `GET {collection}/{arn}` — this kind's by-ARN read.
async fn describe(emulator: &MultiServiceRouter, kind: Kind, arn: &str) -> (u16, Value) {
    let path = format!("{}/{}", kind.collection(), label(arn));
    call(emulator, "GET", &path, Value::Null).await
}

/// `DELETE {collection}/{arn}` — this kind's delete.
async fn delete(emulator: &MultiServiceRouter, kind: Kind, arn: &str) -> (u16, Value) {
    let path = format!("{}/{}", kind.collection(), label(arn));
    call(emulator, "DELETE", &path, Value::Null).await
}

/// Create one resource of this row's kind and return its ARN. A connector names
/// `plugin_arn`, which must already be `ACTIVE`.
async fn create(
    emulator: &MultiServiceRouter,
    case: &Case,
    name: &str,
    plugin_arn: &str,
) -> String {
    let body = match case.kind {
        Kind::Connector => connector_request(name, plugin_arn),
        Kind::CustomPlugin => plugin_request(name),
        Kind::WorkerConfiguration => json!({"name": name, "propertiesFileContent": PROPERTIES}),
    };
    let (status, created) = call(emulator, "POST", case.kind.collection(), body).await;
    assert_eq!(
        status,
        200,
        "case {:?}: the create must succeed, got {status} {created}",
        case.label()
    );
    text(&created, case.kind.arn_member())
}

/// Create one `ACTIVE` custom plugin for the connector rows to run on.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    past_one_transition().await;
    text(&created, "customPluginArn")
}

#[tokio::test]
async fn one_rule_starts_every_removal_and_a_repeat_never_postpones_it() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "removal-start-host-plugin").await;

    for (index, case) in CASES.iter().enumerate() {
        let row = case.label();
        let name = format!("removal-start-{index}");
        let arn = create(&emulator, case, &name, &plugin_arn).await;
        if case.settled {
            past_one_transition().await;
        }

        // The row must really start a removal, so the record it names is in some
        // other state when the first delete lands.
        let (status, before) = describe(&emulator, case.kind, &arn).await;
        assert_eq!(
            status, 200,
            "case {row:?}: the record must be readable before the delete, got {status} {before}"
        );
        let was = text(&before, case.kind.state_member());
        assert_ne!(
            was,
            case.kind.deleting(),
            "case {row:?}: the row must start the removal, not observe one, got {was:?}"
        );

        // Every delete of the row answers the same ARN and the same DELETING
        // state: the first starts the removal, each repeat reports it.
        let first_delete = Instant::now();
        let mut answers = Vec::new();
        for attempt in 1..=case.deletes {
            if attempt > 1 {
                tokio::time::sleep(case.gap.duration()).await;
            }
            let (status, answered) = delete(&emulator, case.kind, &arn).await;
            assert_eq!(
                status, 200,
                "case {row:?} attempt {attempt}: the delete must succeed, got {status} {answered}"
            );
            let named = text(&answered, case.kind.arn_member());
            assert_eq!(
                named, arn,
                "case {row:?} attempt {attempt}: the answer names the record it deleted, \
                 expected {arn:?}, got {named:?}"
            );
            let state = text(&answered, case.kind.state_member());
            assert_eq!(
                state,
                case.kind.deleting(),
                "case {row:?} attempt {attempt}: expected {:?}, got {state:?}",
                case.kind.deleting()
            );
            answers.push((attempt, named, state));
        }
        let Some(first) = answers.first().cloned() else {
            panic!("case {row:?}: every row makes at least one delete");
        };
        for (attempt, named, state) in &answers {
            assert_eq!(
                (named, state),
                (&first.1, &first.2),
                "case {row:?} attempt {attempt}: a repeat must answer as the first delete did, \
                 expected {:?}, got {:?}",
                (&first.1, &first.2),
                (named, state)
            );
        }

        // The span is non-zero, so a caller reading straight after the delete
        // still sees the record in the state the delete moved it to.
        let (status, during) = describe(&emulator, case.kind, &arn).await;
        assert_eq!(
            status, 200,
            "case {row:?}: a record whose removal is owed is still readable, got {status} {during}"
        );
        let state = text(&during, case.kind.state_member());
        assert_eq!(
            state,
            case.kind.deleting(),
            "case {row:?}: the read shows the state the delete moved it to, expected {:?}, \
             got {state:?}",
            case.kind.deleting()
        );

        // The removal is owed from the FIRST delete, so it is done by an instant
        // that is less than one span after any repeat: a repeat that restarted
        // the clock would still be holding the record here.
        tokio::time::sleep_until(observed_gone_at(first_delete)).await;
        let (status, after) = describe(&emulator, case.kind, &arn).await;
        assert_eq!(
            status, 404,
            "case {row:?}: the removal must be done one span after the FIRST delete, \
             got {status} {after}"
        );
        let code = error_code(&after);
        assert!(
            code.contains("NotFoundException"),
            "case {row:?}: expected NotFoundException, got {code:?} {after}"
        );
    }
}
