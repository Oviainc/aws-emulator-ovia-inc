// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an event source mapping: `CreateEventSourceMapping` "creates a
//! mapping between an event source and an Lambda Event Function",
//! `GetEventSourceMapping` "returns details about an event source mapping" and
//! `ListEventSourceMappings` "lists event source mappings. Specify an EventSourceArn to
//! show only event source mappings for a single event source."
//!
//! The rule under test: a created mapping is ONE record this emulator holds under a UUID
//! and an `event-source-mapping` ARN minted once — carrying the event source, the
//! function it resolves and every polling, filtering, destination and logging setting the
//! request stated, with each absent setting at the default its own member documents for
//! THAT event source — and `ListEventSourceMappings` reports those stored records, in one
//! stable order, narrowed to the event source or the function the request names, at most
//! 100 per page, continuing after the marker of the previous page so every mapping is
//! reported exactly once; `GetEventSourceMapping` reports the same record.
//!
//! Every row drives real HTTP requests through the router and reads the mapping back
//! through `GetEventSourceMapping` and `ListEventSourceMappings`, so a mapping that was
//! answered but never stored fails here. The rows differ along what the rule generalises
//! over: WHICH event source the mapping polls (a queue, each of the two shard sources,
//! an MSK cluster, a self-managed Kafka cluster, an Amazon MQ broker and a DocumentDB
//! change stream — the sources the `EventSourceArn` member enumerates), how the function
//! is addressed (bare name, ARN, partial ARN, qualified by a version), which optional
//! settings the request states (none, and each at a published bound) and whether the
//! mapping is enabled.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateEventSourceMapping` and `ListEventSourceMappings` are bound, from
/// `smithy_query(query="operation", operation="CreateEventSourceMapping")`
/// (`POST /2015-03-31/event-source-mappings`).
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// Where `ListTags` is bound (`GET /2017-03-31/tags/{Resource}`), which is how a
/// mapping's tags are read back.
const TAGS_URI: &str = "/2017-03-31/tags";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `State` a freshly created mapping carries, and the two it settles into, from
/// `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
const STATE_CREATING: &str = "Creating";
/// See [`STATE_CREATING`].
const STATE_ENABLED: &str = "Enabled";
/// See [`STATE_CREATING`].
const STATE_DISABLED: &str = "Disabled";

/// `StateTransitionReason` for a change a caller asked for.
const USER_INITIATED: &str = "USER_INITIATED";

/// The bounds the model puts on `EventSourceMappingArn` (`@length(min: 85, max: 120)`),
/// from `smithy_query(query="shape", shape="EventSourceMappingArn")`.
const MAPPING_ARN_MIN_LEN: usize = 85;
/// See [`MAPPING_ARN_MIN_LEN`].
const MAPPING_ARN_MAX_LEN: usize = 120;

/// The length the model puts on `UUIDString` (`@length(min: 36, max: 36)`).
const UUID_LEN: usize = 36;

/// The published bounds of `MaximumBatchingWindowInSeconds` (`@range(min: 0, max: 300)`),
/// `ParallelizationFactor` (`@range(min: 1, max: 10)`), `BatchSize`
/// (`@range(min: 1, max: 10000)`), `MaximumRecordAgeInSeconds`
/// (`@range(min: -1, max: 604800)`), `MaximumRetryAttempts` (`@range(min: -1, max:
/// 10000)`) and `TumblingWindowInSeconds` (`@range(min: 0, max: 900)`), from
/// `smithy_query(query="operation", operation="CreateEventSourceMapping")`.
const BATCHING_WINDOW_MAX: i64 = 300;
/// See [`BATCHING_WINDOW_MAX`].
const PARALLELIZATION_MAX: i64 = 10;
/// See [`BATCHING_WINDOW_MAX`].
const BATCH_SIZE_MAX: i64 = 10_000;
/// See [`BATCHING_WINDOW_MAX`].
const RECORD_AGE_MAX: i64 = 604_800;
/// See [`BATCHING_WINDOW_MAX`].
const TUMBLING_WINDOW_MAX: i64 = 900;

/// The most mappings one listing page reports: "Note that ListEventSourceMappings returns
/// a maximum of 100 items in each response, even if you set the number higher."
const PAGE_CAP: usize = 100;

/// The largest `MaxItems` the model admits (`@range(min: 1, max: 10000)`), which the cap
/// above still holds under.
const MAX_ITEMS_MAX: usize = 10_000;

/// An Amazon SQS queue, "Amazon Simple Queue Service – The ARN of the queue".
const QUEUE_SOURCE: &str = "arn:aws:sqs:us-east-1:000000000000:order-intake";
/// An Amazon Kinesis data stream, "Amazon Kinesis – The ARN of the data stream".
const KINESIS_SOURCE: &str = "arn:aws:kinesis:us-east-1:000000000000:stream/order-events";
/// An Amazon DynamoDB stream, "Amazon DynamoDB Streams – The ARN of the stream".
const DYNAMODB_SOURCE: &str =
    "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/2026-10-23T11:46:49.000";
/// An Amazon MSK cluster, "Amazon Managed Streaming for Apache Kafka – The ARN of the
/// cluster".
const MSK_SOURCE: &str =
    "arn:aws:kafka:us-east-1:000000000000:cluster/order-events/11111111-2222-3333-4444-555555555555-1";
/// An Amazon MQ broker, "Amazon MQ – The ARN of the broker".
const MQ_SOURCE: &str = "arn:aws:mq:us-east-1:000000000000:broker:order-broker:b-1111";
/// An Amazon DocumentDB change stream, "Amazon DocumentDB – The ARN of the DocumentDB
/// change stream", whose ARNs are in the `rds` namespace.
const DOCDB_SOURCE: &str = "arn:aws:rds:us-east-1:000000000000:cluster:order-docdb";

/// A well-formed KMS key ARN, satisfying the model's `KMSKeyArn` pattern
/// (`^(arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*)|()$`).
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/0b1c2d3e-4f56-7890-abcd-ef1234567890";

/// A well-formed `OnFailure` destination, satisfying the model's `DestinationArn`
/// pattern.
const FAILURE_DESTINATION: &str = "arn:aws:sqs:us-east-1:000000000000:order-dlq";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `EventSourceMappings` the body lists, as JSON objects.
    fn mappings(&self) -> Vec<Value> {
        self.json()
            .get("EventSourceMappings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The UUIDs the listed mappings carry, in the order they were reported.
    fn listed_uuids(&self) -> Vec<String> {
        self.mappings()
            .iter()
            .map(|mapping| {
                mapping
                    .get("UUID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `CreateEventSourceMapping`.
async fn create_mapping(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", MAPPINGS_URI, body).await
}

/// `GetEventSourceMapping` on the mapping `uuid` names.
async fn get_mapping(router: &MultiServiceRouter, uuid: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await
}

/// `ListEventSourceMappings` with the query parameters `query` names (already joined).
async fn list_mappings(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        MAPPINGS_URI.to_string()
    } else {
        format!("{MAPPINGS_URI}?{query}")
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// `ListTags` on the resource `arn` names.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{TAGS_URI}/{arn}"), &Value::Null).await
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN an event source mapping of `uuid` carries, as the model's
/// `EventSourceMappingArn` pattern spells it.
fn mapping_arn(uuid: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:event-source-mapping:{uuid}")
}

/// Create a function for a mapping to invoke, publishing version 1 when asked.
async fn create_function_publishing(router: &MultiServiceRouter, name: &str, publish: bool) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Create a function for a mapping to invoke.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    create_function_publishing(router, name, false).await;
}

/// The `CreateEventSourceMapping` body for `function` and `event_source`, plus whatever
/// `extras` states.
fn create_request(function: &str, event_source: Option<&str>, extras: &Value) -> Value {
    let mut body = Map::new();
    body.insert("FunctionName".to_string(), json!(function));
    if let Some(event_source) = event_source {
        body.insert("EventSourceArn".to_string(), json!(event_source));
    }
    if let Some(extras) = extras.as_object() {
        for (member, value) in extras {
            body.insert(member.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// Assert `answered` carries every member `expected` states, where a `Null` expectation
/// means the member must be ABSENT (the default its own documentation gives the member
/// for this event source is "no value at all").
fn assert_members(case: &str, label: &str, answered: &Value, expected: &Value) {
    for (member, want) in expected.as_object().expect("expectations are an object") {
        let got = answered.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            &got, want,
            "case {case:?}: {label} must report {member} as {want}, got {got} in {answered}"
        );
    }
}

/// Whether `uuid` is a hyphenated, 36-character UUID, as `UUIDString` and the
/// `EventSourceMappingArn` pattern require.
fn is_uuid(uuid: &str) -> bool {
    let groups: Vec<&str> = uuid.split('-').collect();
    uuid.len() == UUID_LEN
        && groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// One mapping row: which event source it polls, what else the request states, and what
/// the stored mapping must report.
struct MappingCase {
    name: &'static str,
    /// The event source ARN, or `None` for a self-managed Apache Kafka cluster, which is
    /// named by `SelfManagedEventSource` instead.
    event_source: Option<&'static str>,
    /// How the request addresses the function; `None` means the bare name.
    function_as: Option<String>,
    request_extras: Value,
    expected: Value,
}

/// The rows: one per event source the `EventSourceArn` member enumerates, plus rows that
/// state the optional settings at their published bounds, address the function by each
/// documented form, and disable the mapping.
fn mapping_cases(function: &str) -> Vec<MappingCase> {
    vec![
        MappingCase {
            name: "sqs-queue-defaults",
            event_source: Some(QUEUE_SOURCE),
            function_as: None,
            request_extras: json!({}),
            // "For Amazon SQS, the default is 10", "For Kinesis, DynamoDB, and Amazon SQS
            // event sources, the default batching window is 0 seconds", and the
            // stream-only error handling options apply to no queue.
            expected: json!({
                "BatchSize": 10,
                "MaximumBatchingWindowInSeconds": 0,
                "ParallelizationFactor": null,
                "MaximumRecordAgeInSeconds": null,
                "MaximumRetryAttempts": null,
                "BisectBatchOnFunctionError": null,
                "EventSourceArn": QUEUE_SOURCE,
            }),
        },
        MappingCase {
            name: "kinesis-stream-defaults",
            event_source: Some(KINESIS_SOURCE),
            function_as: None,
            // "Required for Amazon Kinesis and Amazon DynamoDB Stream event sources."
            request_extras: json!({ "StartingPosition": "LATEST" }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": 0,
                "ParallelizationFactor": 1,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "StartingPosition": "LATEST",
            }),
        },
        MappingCase {
            name: "dynamodb-stream-defaults",
            event_source: Some(DYNAMODB_SOURCE),
            function_as: None,
            request_extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": 0,
                "ParallelizationFactor": 1,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "StartingPosition": "TRIM_HORIZON",
            }),
        },
        MappingCase {
            name: "msk-cluster-defaults",
            event_source: Some(MSK_SOURCE),
            function_as: None,
            request_extras: json!({
                "Topics": ["order-events"],
                "AmazonManagedKafkaEventSourceConfig": { "ConsumerGroupId": "order-consumers" },
                "LoggingConfig": { "SystemLogLevel": "INFO" },
                "MetricsConfig": { "Metrics": ["EventCount", "ErrorCount"] },
            }),
            // "For Amazon MSK, Self-managed Apache Kafka, Amazon MQ, and DocumentDB event
            // sources, the default batching window is 500 ms", which this member's unit
            // cannot carry, so none is reported — and MSK IS one of the stream sources the
            // error handling options are available for.
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "ParallelizationFactor": null,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "Topics": ["order-events"],
                "LoggingConfig": { "SystemLogLevel": "INFO" },
                "MetricsConfig": { "Metrics": ["EventCount", "ErrorCount"] },
            }),
        },
        MappingCase {
            name: "self-managed-kafka-defaults",
            event_source: None,
            function_as: None,
            request_extras: json!({
                "SelfManagedEventSource": {
                    "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                },
                "SelfManagedKafkaEventSourceConfig": { "ConsumerGroupId": "self-managed-group" },
                "Topics": ["order-events"],
            }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "EventSourceArn": null,
                "MaximumRecordAgeInSeconds": -1,
                "MaximumRetryAttempts": -1,
                "BisectBatchOnFunctionError": false,
                "SelfManagedEventSource": {
                    "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                },
            }),
        },
        MappingCase {
            name: "mq-broker-defaults",
            event_source: Some(MQ_SOURCE),
            function_as: None,
            request_extras: json!({
                "Queues": ["order-jobs"],
                "SourceAccessConfigurations": [
                    { "Type": "BASIC_AUTH", "URI": "arn:aws:secretsmanager:us-east-1:000000000000:secret:mq-1" },
                ],
            }),
            // Amazon MQ is neither a shard source nor one of the stream sources the error
            // handling options are documented for.
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "ParallelizationFactor": null,
                "MaximumRecordAgeInSeconds": null,
                "MaximumRetryAttempts": null,
                "BisectBatchOnFunctionError": null,
                "Queues": ["order-jobs"],
            }),
        },
        MappingCase {
            name: "documentdb-change-stream-defaults",
            event_source: Some(DOCDB_SOURCE),
            function_as: None,
            request_extras: json!({
                "DocumentDBEventSourceConfig": {
                    "DatabaseName": "orders",
                    "CollectionName": "intake",
                    "FullDocument": "UpdateLookup",
                },
            }),
            expected: json!({
                "BatchSize": 100,
                "MaximumBatchingWindowInSeconds": null,
                "MaximumRecordAgeInSeconds": null,
                "DocumentDBEventSourceConfig": {
                    "DatabaseName": "orders",
                    "CollectionName": "intake",
                    "FullDocument": "UpdateLookup",
                },
            }),
        },
        MappingCase {
            name: "every-setting-at-its-upper-bound",
            event_source: Some("arn:aws:kinesis:us-east-1:000000000000:stream/bounded"),
            // "Function ARN – arn:aws:lambda:us-west-2:123456789012:function:MyFunction."
            function_as: Some(function_arn(function)),
            request_extras: json!({
                "StartingPosition": "TRIM_HORIZON",
                "BatchSize": BATCH_SIZE_MAX,
                "MaximumBatchingWindowInSeconds": BATCHING_WINDOW_MAX,
                "ParallelizationFactor": PARALLELIZATION_MAX,
                "MaximumRecordAgeInSeconds": RECORD_AGE_MAX,
                "MaximumRetryAttempts": 10_000,
                "TumblingWindowInSeconds": TUMBLING_WINDOW_MAX,
                "BisectBatchOnFunctionError": true,
                "FunctionResponseTypes": ["ReportBatchItemFailures"],
                "DestinationConfig": { "OnFailure": { "Destination": FAILURE_DESTINATION } },
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
            }),
            expected: json!({
                "BatchSize": BATCH_SIZE_MAX,
                "MaximumBatchingWindowInSeconds": BATCHING_WINDOW_MAX,
                "ParallelizationFactor": PARALLELIZATION_MAX,
                "MaximumRecordAgeInSeconds": RECORD_AGE_MAX,
                "MaximumRetryAttempts": 10_000,
                "TumblingWindowInSeconds": TUMBLING_WINDOW_MAX,
                "BisectBatchOnFunctionError": true,
                "FunctionResponseTypes": ["ReportBatchItemFailures"],
                "DestinationConfig": { "OnFailure": { "Destination": FAILURE_DESTINATION } },
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
            }),
        },
        MappingCase {
            name: "every-setting-at-its-lower-bound-disabled",
            event_source: Some("arn:aws:sqs:us-east-1:000000000000:bounded-queue"),
            // "Partial ARN – 123456789012:function:MyFunction."
            function_as: Some(format!("{ACCOUNT}:function:{function}")),
            request_extras: json!({
                "Enabled": false,
                "BatchSize": 1,
                "MaximumBatchingWindowInSeconds": 0,
                "ScalingConfig": { "MaximumConcurrency": 2 },
                "ProvisionedPollerConfig": { "MinimumPollers": 1, "MaximumPollers": 1 },
                "Tags": { "team": "orders" },
            }),
            expected: json!({
                "BatchSize": 1,
                "MaximumBatchingWindowInSeconds": 0,
                "ScalingConfig": { "MaximumConcurrency": 2 },
                "ProvisionedPollerConfig": { "MinimumPollers": 1, "MaximumPollers": 1 },
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_event_source_mapping_is_persisted_and_reported() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-reported-fn";
    create_function(&router, function).await;

    let mut uuids: BTreeSet<String> = BTreeSet::new();

    for case in mapping_cases(function) {
        let label = case.name;
        let request = create_request(
            case.function_as.as_deref().unwrap_or(function),
            case.event_source,
            &case.request_extras,
        );
        let created = create_mapping(&router, &request).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: CreateEventSourceMapping must answer 202 Accepted, got {} {}",
            created.status,
            created.text()
        );

        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {label:?}: expected a UUID, got {}", created.text()));
        assert!(
            is_uuid(&uuid),
            "case {label:?}: UUID must be a 36-character hyphenated UUID, got {uuid:?}"
        );
        assert!(
            uuids.insert(uuid.clone()),
            "case {label:?}: UUID {uuid:?} was already minted for another mapping"
        );

        let arn = created
            .string("EventSourceMappingArn")
            .unwrap_or_else(|| panic!("case {label:?}: expected an ARN, got {}", created.text()));
        assert_eq!(
            arn,
            mapping_arn(&uuid),
            "case {label:?}: the ARN must name this account's region and the mapping's UUID, \
             got {arn:?}"
        );
        assert!(
            arn.len() >= MAPPING_ARN_MIN_LEN && arn.len() <= MAPPING_ARN_MAX_LEN,
            "case {label:?}: the ARN must satisfy the modelled {MAPPING_ARN_MIN_LEN}..\
             {MAPPING_ARN_MAX_LEN} length, got {} in {arn:?}",
            arn.len()
        );
        assert_eq!(
            (
                created.string("FunctionArn"),
                created.string("State"),
                created.string("StateTransitionReason")
            ),
            (
                Some(function_arn(function)),
                Some(STATE_CREATING.to_string()),
                Some(USER_INITIATED.to_string())
            ),
            "case {label:?}: a created mapping names the function it resolved and starts \
             {STATE_CREATING}, got {}",
            created.text()
        );
        assert!(
            created.json().get("LastModified").is_some(),
            "case {label:?}: a created mapping reports when it was last modified, got {}",
            created.text()
        );
        assert!(
            created.json().get("LastProcessingResult").is_none(),
            "case {label:?}: no poller has processed anything, so no last processing result \
             is reported, got {}",
            created.text()
        );
        assert_members(label, "the create", &created.json(), &case.expected);

        // The round trip: the mapping is read back through GetEventSourceMapping, which
        // answers out of the store, so a mapping that was answered but never persisted
        // fails here.
        let fetched = get_mapping(&router, &uuid).await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: GetEventSourceMapping must report the stored mapping, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_members(label, "the get", &fetched.json(), &case.expected);
        assert_eq!(
            (
                fetched.string("UUID"),
                fetched.string("EventSourceMappingArn"),
                fetched.string("FunctionArn"),
                fetched.string("StateTransitionReason")
            ),
            (
                Some(uuid.clone()),
                Some(arn.clone()),
                Some(function_arn(function)),
                Some(USER_INITIATED.to_string())
            ),
            "case {label:?}: the get reports the same identity the create minted, got {}",
            fetched.text()
        );

        // And the listing reports the same record, narrowed to this mapping's own event
        // source where it has one.
        if let Some(event_source) = case.event_source {
            let listed = list_mappings(&router, &format!("EventSourceArn={event_source}")).await;
            assert_eq!(
                (listed.status, listed.listed_uuids()),
                (200, vec![uuid.clone()]),
                "case {label:?}: a listing of {event_source:?} reports exactly this mapping, \
                 got {} {}",
                listed.status,
                listed.text()
            );
            assert_members(
                label,
                "the listing",
                listed
                    .mappings()
                    .first()
                    .unwrap_or_else(|| panic!("case {label:?}: the listing reported no mapping")),
                &case.expected,
            );
        }
    }

    // Every mapping created above is reported by an unfiltered listing, exactly once.
    let listed = list_mappings(&router, "").await;
    assert_eq!(
        (listed.status, listed.listed_uuids().len()),
        (200, uuids.len()),
        "an unfiltered listing reports every stored mapping, got {} {}",
        listed.status,
        listed.text()
    );
    assert_eq!(
        listed.listed_uuids().into_iter().collect::<BTreeSet<_>>(),
        uuids,
        "an unfiltered listing reports exactly the mappings that were created"
    );
}

#[tokio::test]
async fn a_mapping_settles_into_the_state_its_enabled_setting_asks_for() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-settling-fn";
    create_function(&router, function).await;

    // "When true, the event source mapping is active. When false, Lambda pauses polling
    // and invocation. Default: True"
    let cases = [
        ("enabled-by-default", json!({}), STATE_ENABLED),
        ("enabled-explicitly", json!({ "Enabled": true }), STATE_ENABLED),
        ("disabled", json!({ "Enabled": false }), STATE_DISABLED),
    ];

    let mut uuids = Vec::new();
    for (index, (label, extras, expected)) in cases.iter().enumerate() {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:settling-{index}");
        let created = create_mapping(
            &router,
            &create_request(function, Some(&source), extras),
        )
        .await;
        assert_eq!(
            (created.status, created.string("State")),
            (202, Some(STATE_CREATING.to_string())),
            "case {label:?}: a created mapping starts {STATE_CREATING}, got {} {}",
            created.status,
            created.text()
        );
        uuids.push((
            label,
            created.string("UUID").expect("the create answers a UUID"),
            *expected,
            created
                .json()
                .get("LastModified")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: the create reports LastModified as a timestamp")
                }),
        ));
    }

    // The transition is owed by the clock, not by anybody calling: nothing here sleeps on
    // the emulator's behalf, it just waits past the instant the create made due.
    let delay = service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY;
    tokio::time::sleep(delay + std::time::Duration::from_millis(100)).await;

    for (label, uuid, expected, created_at) in uuids {
        let mut settled_at = None;
        for read in 1..=2 {
            let fetched = get_mapping(&router, &uuid).await;
            assert_eq!(
                (fetched.status, fetched.string("State")),
                (200, Some(expected.to_string())),
                "case {label:?} read {read}: expected {expected} once the transition was due, \
                 got {} {}",
                fetched.status,
                fetched.text()
            );
            // "The date that the event source mapping was last updated or that its state
            // changed": the transition IS a state change, so the instant moves with it —
            // to when the clock owed it, which is why two reads report the same instant.
            let reported = fetched.json().get("LastModified").and_then(Value::as_f64);
            assert!(
                reported.is_some_and(|modified| modified > created_at),
                "case {label:?} read {read}: LastModified must move with the state change \
                 from {created_at}, got {reported:?}"
            );
            if let Some(before) = settled_at {
                assert_eq!(
                    reported,
                    Some(before),
                    "case {label:?} read {read}: reading the mapping again must not move \
                     LastModified, got {reported:?} after {before}"
                );
            }
            settled_at = reported;
        }
        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let states: Vec<Value> = listed
            .mappings()
            .iter()
            .filter(|mapping| mapping.get("UUID").and_then(Value::as_str) == Some(uuid.as_str()))
            .map(|mapping| mapping.get("State").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            states,
            vec![json!(expected)],
            "case {label:?}: the listing reports the same settled state, got {}",
            listed.text()
        );
    }
}

#[tokio::test]
async fn a_listing_narrows_to_the_event_source_or_the_function_it_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_publishing(&router, "esm-filter-a", true).await;
    create_function(&router, "esm-filter-b").await;

    // Three mappings: two of function A (one of them against its published version 1) and
    // one of function B, two of which share an event source.
    let shared_source = "arn:aws:sqs:us-east-1:000000000000:shared-queue";
    let own_source = "arn:aws:sqs:us-east-1:000000000000:own-queue";

    let latest_of_a = create_mapping(
        &router,
        &create_request("esm-filter-a", Some(shared_source), &json!({})),
    )
    .await;
    let version_of_a = create_mapping(
        &router,
        &create_request("esm-filter-a:1", Some(own_source), &json!({})),
    )
    .await;
    let of_b = create_mapping(
        &router,
        &create_request("esm-filter-b", Some(shared_source), &json!({})),
    )
    .await;
    for (label, answer) in [
        ("latest-of-a", &latest_of_a),
        ("version-of-a", &version_of_a),
        ("of-b", &of_b),
    ] {
        assert_eq!(
            answer.status,
            202,
            "case {label:?}: the create must succeed, got {} {}",
            answer.status,
            answer.text()
        );
    }
    let latest_of_a = latest_of_a.string("UUID").expect("a UUID");
    let version_of_a = version_of_a.string("UUID").expect("a UUID");
    let of_b = of_b.string("UUID").expect("a UUID");

    assert_eq!(
        version_of_a_arn(&router, &version_of_a).await,
        Some(format!("{}:1", function_arn("esm-filter-a"))),
        "a mapping created against a qualified function name names that version"
    );

    // Each row: the query, and the mappings it must report.
    let cases: Vec<(&str, String, Vec<&String>)> = vec![
        (
            "by-event-source",
            format!("EventSourceArn={shared_source}"),
            vec![&latest_of_a, &of_b],
        ),
        (
            "by-other-event-source",
            format!("EventSourceArn={own_source}"),
            vec![&version_of_a],
        ),
        (
            "by-function-name",
            "FunctionName=esm-filter-a".to_string(),
            vec![&latest_of_a, &version_of_a],
        ),
        (
            "by-function-arn",
            format!("FunctionName={}", function_arn("esm-filter-a")),
            vec![&latest_of_a, &version_of_a],
        ),
        (
            "by-qualified-function",
            "FunctionName=esm-filter-a:1".to_string(),
            vec![&version_of_a],
        ),
        (
            "by-other-function",
            "FunctionName=esm-filter-b".to_string(),
            vec![&of_b],
        ),
        (
            "by-function-and-event-source",
            format!("FunctionName=esm-filter-a&EventSourceArn={shared_source}"),
            vec![&latest_of_a],
        ),
        (
            "by-event-source-nothing-maps",
            "EventSourceArn=arn:aws:sqs:us-east-1:000000000000:unmapped".to_string(),
            vec![],
        ),
    ];

    for (label, query, expected) in cases {
        let listed = list_mappings(&router, &query).await;
        let want: BTreeSet<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            (listed.status, listed.listed_uuids().into_iter().collect()),
            (200, want.clone()),
            "case {label:?}: {query:?} must report {want:?}, got {} {}",
            listed.status,
            listed.text()
        );
    }
}

/// The `FunctionArn` the mapping `uuid` reports, read back through the get.
async fn version_of_a_arn(router: &MultiServiceRouter, uuid: &str) -> Option<String> {
    get_mapping(router, uuid).await.string("FunctionArn")
}

#[tokio::test]
async fn a_listing_pages_through_every_mapping_exactly_once() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-paging-fn";
    create_function(&router, function).await;

    // One more mapping than a page may hold, so the documented cap is exercised past its
    // own bound.
    let total = PAGE_CAP + 1;
    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..total {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:paging-{index:04}");
        let answer = create_mapping(
            &router,
            &create_request(function, Some(&source), &json!({})),
        )
        .await;
        assert_eq!(
            answer.status,
            202,
            "mapping {index} must be created, got {} {}",
            answer.status,
            answer.text()
        );
        created.insert(answer.string("UUID").expect("the create answers a UUID"));
    }

    // "Note that ListEventSourceMappings returns a maximum of 100 items in each response,
    // even if you set the number higher."
    let capped = list_mappings(&router, &format!("MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        (capped.status, capped.listed_uuids().len()),
        (200, PAGE_CAP),
        "a page holds at most {PAGE_CAP} mappings however high MaxItems is, got {} {}",
        capped.status,
        capped.text()
    );
    assert!(
        capped.string("NextMarker").is_some(),
        "a truncated page carries the marker the next one continues after, got {}",
        capped.text()
    );

    // Every page size walks the whole set exactly once, in one stable order.
    for page_size in [1usize, 7, PAGE_CAP, MAX_ITEMS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= total + 1,
                "page size {page_size}: the walk must terminate, {pages} pages in"
            );
            let query = match marker.as_deref() {
                Some(marker) => format!("MaxItems={page_size}&Marker={marker}"),
                None => format!("MaxItems={page_size}"),
            };
            let page = list_mappings(&router, &query).await;
            assert_eq!(
                page.status,
                200,
                "page size {page_size}: the listing must succeed, got {} {}",
                page.status,
                page.text()
            );
            let uuids = page.listed_uuids();
            assert!(
                uuids.len() <= page_size.min(PAGE_CAP),
                "page size {page_size}: a page reports at most {} mappings, got {}",
                page_size.min(PAGE_CAP),
                uuids.len()
            );
            seen.extend(uuids);
            match page.string("NextMarker") {
                Some(next) => marker = Some(next),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: no mapping may be reported twice, got {} of {}",
            unique.len(),
            seen.len()
        );
        assert_eq!(
            unique, created,
            "page size {page_size}: the walk must report every created mapping"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "page size {page_size}: the order must be the stable one the marker continues"
        );
    }
}

#[tokio::test]
async fn encrypted_filter_criteria_are_reported_by_the_get_and_not_by_the_listing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-filter-criteria-fn";
    create_function(&router, function).await;

    let criteria = json!({ "Filters": [{ "Pattern": "{\"region\":[\"us-east-1\"]}" }] });
    // Two rows: the same criteria, encrypted with a customer managed key and not.
    let cases = [
        ("plaintext", json!({ "FilterCriteria": criteria }), true),
        (
            "encrypted",
            json!({ "FilterCriteria": criteria, "KMSKeyArn": KMS_KEY }),
            false,
        ),
    ];

    for (index, (label, extras, listed_in_plaintext)) in cases.iter().enumerate() {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:criteria-{index}");
        let created = create_mapping(&router, &create_request(function, Some(&source), extras)).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the create must succeed, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created.string("UUID").expect("the create answers a UUID");

        // "You can view this field in plaintext in the response of GetEventSourceMapping
        // and DeleteEventSourceMapping calls if you have kms:Decrypt permissions for the
        // correct KMS key."
        let fetched = get_mapping(&router, &uuid).await;
        assert_eq!(
            fetched.json().get("FilterCriteria"),
            Some(&criteria),
            "case {label:?}: the get reports the stored filter criteria, got {}",
            fetched.text()
        );

        // "If filter criteria is encrypted, this field shows up as null in the response of
        // ListEventSourceMapping API calls."
        let listed = list_mappings(&router, &format!("EventSourceArn={source}")).await;
        let reported = listed
            .mappings()
            .first()
            .and_then(|mapping| mapping.get("FilterCriteria").cloned());
        let expected = listed_in_plaintext.then(|| criteria.clone());
        assert_eq!(
            reported, expected,
            "case {label:?}: the listing must report {expected:?} for FilterCriteria, got {}",
            listed.text()
        );
    }
}

#[tokio::test]
async fn tags_a_mapping_was_created_with_are_reported_for_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-tagged-fn";
    create_function(&router, function).await;

    // Two rows: a mapping created with tags, and one created with none.
    let cases = [
        (
            "tagged",
            json!({ "Tags": { "team": "orders", "env": "test" } }),
            json!({ "env": "test", "team": "orders" }),
        ),
        ("untagged", json!({}), json!({})),
    ];

    for (index, (label, extras, expected)) in cases.iter().enumerate() {
        let source = format!("arn:aws:sqs:us-east-1:000000000000:tagged-{index}");
        let created =
            create_mapping(&router, &create_request(function, Some(&source), extras)).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the create must succeed, got {} {}",
            created.status,
            created.text()
        );
        let arn = created
            .string("EventSourceMappingArn")
            .expect("the create answers an ARN");
        let tagged = list_tags(&router, &arn).await;
        assert_eq!(
            (tagged.status, tagged.json().get("Tags")),
            (200, Some(expected)),
            "case {label:?}: ListTags on {arn:?} must report {expected}, got {} {}",
            tagged.status,
            tagged.text()
        );
    }
}

/// One refusal row: what to send, and the status (and modelled error code) it must be
/// answered with.
struct RefusalCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_request_no_event_source_mapping_can_satisfy_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-refusal-fn";
    create_function(&router, function).await;

    let taken = "arn:aws:sqs:us-east-1:000000000000:taken-queue";
    let created = create_mapping(&router, &create_request(function, Some(taken), &json!({}))).await;
    assert_eq!(
        created.status,
        202,
        "the mapping the conflict row re-creates must exist, got {} {}",
        created.status,
        created.text()
    );
    let known_uuid = created.string("UUID").expect("the create answers a UUID");

    let cases = vec![
        RefusalCase {
            name: "create-for-an-unknown-function",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request("esm-absent-fn", Some(taken), &json!({})),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "create-for-an-unknown-version",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                &format!("{function}:7"),
                Some("arn:aws:sqs:us-east-1:000000000000:versioned"),
                &json!({}),
            ),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "create-naming-no-event-source",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, None, &json!({})),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-naming-two-event-sources",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some("arn:aws:sqs:us-east-1:000000000000:both"),
                &json!({
                    "SelfManagedEventSource": {
                        "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                    },
                }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-on-a-kinesis-stream-without-a-starting-position",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, Some(KINESIS_SOURCE), &json!({})),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-on-a-dynamodb-stream-without-a-starting-position",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, Some(DYNAMODB_SOURCE), &json!({})),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-at-a-timestamp-on-a-source-that-does-not-support-it",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some(DYNAMODB_SOURCE),
                &json!({ "StartingPosition": "AT_TIMESTAMP", "StartingPositionTimestamp": 1_700_000_000 }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-at-a-timestamp-without-one",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some(KINESIS_SOURCE),
                &json!({ "StartingPosition": "AT_TIMESTAMP" }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-at-a-timestamp-in-the-future",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(
                function,
                Some(KINESIS_SOURCE),
                &json!({
                    "StartingPosition": "AT_TIMESTAMP",
                    "StartingPositionTimestamp": 4_000_000_000i64,
                }),
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "create-a-second-mapping-of-the-same-pair",
            method: "POST",
            uri: MAPPINGS_URI.to_string(),
            body: create_request(function, Some(taken), &json!({})),
            status: 409,
            code: "ResourceConflictException",
        },
        RefusalCase {
            name: "get-a-mapping-this-account-does-not-hold",
            method: "GET",
            uri: format!("{MAPPINGS_URI}/11111111-2222-3333-4444-555555555555"),
            body: Value::Null,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "list-the-mappings-of-an-unknown-function",
            method: "GET",
            uri: format!("{MAPPINGS_URI}?FunctionName=esm-absent-fn"),
            body: Value::Null,
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];

    for case in cases {
        let answer = call(&router, case.method, &case.uri, &case.body).await;
        assert_eq!(
            (answer.status, answer.error_code().contains(case.code)),
            (case.status, true),
            "case {:?}: expected {} {}, got {} {}",
            case.name,
            case.status,
            case.code,
            answer.status,
            answer.text()
        );
    }

    // A refusal changes nothing: the one mapping this account holds is still the one it
    // held before.
    let listed = list_mappings(&router, "").await;
    assert_eq!(
        (listed.status, listed.listed_uuids()),
        (200, vec![known_uuid]),
        "every refusal above must leave the store as it was, got {} {}",
        listed.status,
        listed.text()
    );
}
