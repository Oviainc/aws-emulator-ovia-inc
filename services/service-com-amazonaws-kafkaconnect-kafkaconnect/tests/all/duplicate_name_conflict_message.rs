// SPDX-License-Identifier: CC0-1.0

//! The `ConflictException` a create whose name is already taken answers with,
//! end to end over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON.
//!
//! The rule under test: a kafkaconnect create refused because the name it asks
//! for is already held answers HTTP 409 / `ConflictException` with a message in
//! the service's own member-naming form — `Invalid parameter <member>: ` followed
//! by the modelled conflict reason — where `<member>` is the REQUEST member that
//! carried the name, and the message carries no value taken from the request or
//! from the held resource (no name, and above all no generated ARN, which would
//! differ on every run).
//!
//! The table spans every create this service models a `ConflictException` for —
//! worker configurations (`name`), custom plugins (`name`) and connectors
//! (`connectorName`), the member names read from the model — crossed with names
//! differing along what the rule generalises over: length (the modelled
//! `__stringMin1Max128` minimum and maximum), character class (ASCII, digits,
//! punctuation, non-ASCII) and form. The issue's own case is ONE row.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::collections::BTreeSet;
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

/// The reason the modelled `ConflictException` documents for a name that is
/// already held: "HTTP Status Code 409: Conflict. A resource with this name
/// already exists. Retry your request with another name." — the status prefix is
/// the HTTP status itself and the retry advice is advice, so the reason the
/// message carries is the middle sentence.
const CONFLICT_REASON: &str = "A resource with this name already exists.";

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

/// A well-formed IAM role ARN, as every accepted `CreateConnector` carries.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/connector-role")
}

/// Which create a row drives, and the input member a name conflict is blamed on.
///
/// The member is the one the REQUEST names, read from the model:
/// `CreateWorkerConfiguration$name`, `CreateCustomPlugin$name` and
/// `CreateConnector$connectorName` — all three `__stringMin1Max128` and
/// `@required`.
#[derive(Clone, Copy, Debug)]
enum Create {
    WorkerConfiguration,
    CustomPlugin,
    Connector,
}

impl Create {
    /// The collection the create POSTs to.
    fn path(self) -> &'static str {
        match self {
            Self::WorkerConfiguration => "/v1/worker-configurations",
            Self::CustomPlugin => "/v1/custom-plugins",
            Self::Connector => "/v1/connectors",
        }
    }

    /// The request member that carries the name, which the refusal must name.
    fn member(self) -> &'static str {
        match self {
            Self::WorkerConfiguration | Self::CustomPlugin => "name",
            Self::Connector => "connectorName",
        }
    }

    /// The member of the success response that carries the created resource's
    /// ARN, used to check that no generated ARN leaks into the refusal.
    fn arn_member(self) -> &'static str {
        match self {
            Self::WorkerConfiguration => "workerConfigurationArn",
            Self::CustomPlugin => "customPluginArn",
            Self::Connector => "connectorArn",
        }
    }

    /// A valid create body asking for `name`. `plugin` is the ACTIVE custom
    /// plugin a connector runs, ignored by the other two.
    fn body(self, name: &str, plugin: &str) -> Value {
        match self {
            Self::WorkerConfiguration => json!({
                "name": name,
                // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
                "propertiesFileContent":
                    "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
            }),
            Self::CustomPlugin => json!({
                "name": name,
                "contentType": "ZIP",
                "location": {"s3Location": {
                    "bucketArn": "arn:aws:s3:::plugin-bucket",
                    "fileKey": "plugins/p.zip",
                }},
            }),
            Self::Connector => json!({
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
                "plugins": [{"customPlugin": {"customPluginArn": plugin, "revision": 1}}],
                "serviceExecutionRoleArn": role_arn(),
            }),
        }
    }
}

/// An ACTIVE custom plugin every connector row runs, with its ARN.
async fn active_plugin(emulator: &MultiServiceRouter) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        Create::CustomPlugin.path(),
        Create::CustomPlugin.body("conflict-probe-plugin", ""),
    )
    .await;
    assert_eq!(
        status, 200,
        "the plugin the connector rows run must be created, got {status} {created}"
    );
    // A created plugin is CREATING and settles into ACTIVE once the clock owes
    // the transition; only an ACTIVE plugin can back a connector.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    created["customPluginArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a customPluginArn in {created}"))
        .to_string()
}

/// The names the table is crossed with: `suffix` keeps each row's name distinct
/// per operation, and the names differ along length (the modelled minimum 1 and
/// maximum 128 of `__stringMin1Max128`), character class and form.
fn names(suffix: &str) -> Vec<(String, String)> {
    let max_length = {
        // Exactly 128 characters, the modelled maximum: the suffix plus padding.
        let mut name = format!("max-{suffix}-");
        name.push_str(&"a".repeat(128 - name.chars().count()));
        name
    };
    vec![
        ("a plain ASCII name".to_string(), format!("dup-plain-{suffix}")),
        (
            "a name at the modelled minimum length".to_string(),
            suffix.chars().next().unwrap_or('x').to_string(),
        ),
        ("a name at the modelled maximum length".to_string(), max_length),
        ("digits only".to_string(), format!("90210{suffix}")),
        (
            "punctuation the service allows".to_string(),
            format!("dup.name_{suffix}-2"),
        ),
        (
            "characters outside ASCII".to_string(),
            format!("dup-\u{e9}t\u{e9}-\u{30c4}-{suffix}"),
        ),
        (
            "a name that looks like an ARN".to_string(),
            format!("arn-like-{suffix}"),
        ),
    ]
}

#[tokio::test]
async fn a_duplicate_name_is_refused_naming_the_member_that_carried_it() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator).await;

    // Every message the table produced, so the rule's independence from the
    // request's values can be asserted per operation at the end.
    let mut per_operation: Vec<(Create, BTreeSet<String>)> = Vec::new();

    for (index, operation) in [
        Create::WorkerConfiguration,
        Create::CustomPlugin,
        Create::Connector,
    ]
    .into_iter()
    .enumerate()
    {
        let expected = format!("Invalid parameter {}: {CONFLICT_REASON}", operation.member());
        let mut messages = BTreeSet::new();

        for (case, name) in names(&format!("{index}")) {
            let case = format!("{operation:?} / {case}");

            // The name is free, so the first create takes it.
            let (status, created) = call(
                &emulator,
                "POST",
                operation.path(),
                operation.body(&name, &plugin),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first create of {name:?} must succeed, got {status} {created}"
            );
            let held_arn = created[operation.arn_member()]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a {} in {created}", operation.arn_member())
                })
                .to_string();

            // Repeating the same name twice: the refusal is the same answer both
            // times, so nothing about it is derived from the attempt.
            for attempt in 1..=2 {
                let (status, refused) = call(
                    &emulator,
                    "POST",
                    operation.path(),
                    operation.body(&name, &plugin),
                )
                .await;
                assert_eq!(
                    status, 409,
                    "case {case:?} attempt {attempt}: a repeated name is a 409, got {status} \
                     {refused}"
                );
                let code = error_code(&refused);
                assert!(
                    code.contains("ConflictException"),
                    "case {case:?} attempt {attempt}: expected ConflictException, got {code:?} in \
                     {refused}"
                );
                let actual = message(&refused);
                assert_eq!(
                    actual, expected,
                    "case {case:?} attempt {attempt}: expected the message {expected:?}, got \
                     {actual:?}"
                );
                // Nothing generated and nothing echoed: neither the held
                // resource's ARN nor any ARN at all is in the text.
                assert!(
                    !actual.contains(&held_arn) && !actual.contains("arn:"),
                    "case {case:?} attempt {attempt}: the message must carry no ARN (the held one \
                     is {held_arn:?}), got {actual:?}"
                );
                messages.insert(actual);
            }
        }

        per_operation.push((operation, messages));
    }

    // Per operation the message is ONE sentence whatever name was asked for: a
    // message that interpolated the request's name or the held resource's ARN
    // would give one per row here.
    for (operation, messages) in &per_operation {
        assert_eq!(
            messages.len(),
            1,
            "{operation:?}: the conflict message must not depend on the name asked for, got \
             {messages:?}"
        );
    }

    // Across operations only the member differs, so the three messages are
    // distinct exactly where the members are.
    let worker = &per_operation[0].1;
    let connector = &per_operation[2].1;
    assert_ne!(
        worker, connector,
        "CreateConnector blames connectorName and CreateWorkerConfiguration blames name, so the \
         two messages must differ, got {worker:?} and {connector:?}"
    );
}

#[tokio::test]
async fn a_name_freed_by_a_delete_is_no_longer_a_conflict() {
    let emulator = emulator();

    // The conflict is a fact about STATE, not about the name: once the holder is
    // gone the same name is taken again, and until then every attempt is refused
    // with the one message.
    let name = "reused-after-delete";
    let (status, created) = call(
        &emulator,
        "POST",
        Create::WorkerConfiguration.path(),
        Create::WorkerConfiguration.body(name, ""),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first create must succeed, got {status} {created}"
    );
    let arn = created["workerConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a workerConfigurationArn in {created}"))
        .to_string();

    let (status, refused) = call(
        &emulator,
        "POST",
        Create::WorkerConfiguration.path(),
        Create::WorkerConfiguration.body(name, ""),
    )
    .await;
    assert_eq!(
        status, 409,
        "the repeat must be a 409, got {status} {refused}"
    );
    let actual = message(&refused);
    assert_eq!(
        actual,
        format!("Invalid parameter name: {CONFLICT_REASON}"),
        "case \"held name\": got {actual:?}"
    );

    let encoded =
        percent_encoding::utf8_percent_encode(&arn, percent_encoding::NON_ALPHANUMERIC).to_string();
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v1/worker-configurations/{encoded}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete must be accepted, got {status} {deleted}"
    );
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;

    let (status, recreated) = call(
        &emulator,
        "POST",
        Create::WorkerConfiguration.path(),
        Create::WorkerConfiguration.body(name, ""),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"freed name\": the name must be free once the holder is gone, got {status} \
         {recreated}"
    );
    let fresh = recreated["workerConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a workerConfigurationArn in {recreated}"))
        .to_string();
    assert_ne!(
        fresh, arn,
        "case \"freed name\": the re-created configuration is a new resource, got {fresh:?}"
    );
}
