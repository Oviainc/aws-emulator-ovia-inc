// SPDX-License-Identifier: CC0-1.0

//! Standing a DynamoDB-stream-sourced Lambda event source mapping up, across four
//! services: DynamoDB mints the table and its stream, DynamoDB Streams serves that
//! stream to a consumer, SQS holds the queue the mapping names as its failure
//! destination, and Lambda accepts the mapping over the stream ARN DynamoDB minted.
//!
//! This suite needs more than one service by construction: the stream ARN is minted by
//! the DynamoDB crate, read by the DynamoDB Streams crate and consumed by the Lambda
//! crate, and the destination ARN by the SQS crate, so no single-service test can see
//! the hand-off.
//!
//! The rule under test: `StreamSpecification` is an ordinary optional `CreateTable`
//! member — "CreateTable is an asynchronous operation. Upon receiving a CreateTable
//! request, DynamoDB immediately returns a response with a TableStatus of CREATING.
//! After the table is created, DynamoDB sets the TableStatus to ACTIVE. You can use the
//! DescribeTable action to check the table status" — so asking for a stream changes
//! WHAT the table publishes and never WHETHER the table exists or when it becomes
//! describable: the table half and the stream half are registered together, the table
//! is addressable by `DescribeTable` from the instant the create returned, it reaches
//! `ACTIVE`, its data plane serves items, and the `LatestStreamArn` it reports is a
//! stream DynamoDB Streams resolves and Lambda accepts as a mapping's `EventSourceArn`
//! ("Amazon DynamoDB Streams – The ARN of the stream").
//!
//! The rows differ along what the rule generalises over — every `StreamViewType` the
//! model declares, crossed with both billing modes and with tables with and without a
//! sort key — because none of those may change when the table becomes describable or
//! whether its stream is mappable. Every assertion is made over the wire, so a resource
//! that was answered but never registered fails here.
//!
//! Both halves are asserted on ONE engine in ONE row on purpose: the defect this pins
//! against was observable only as the CONTRAST between them — the stream half resolvable
//! through `DescribeStream`/`GetShardIterator` while `DescribeTable`/`PutItem` on the
//! same table name were refused — which no single-service suite can see.
//!
//! What this suite deliberately does NOT assert is the DELIVERY of a batch to the
//! function or to the mapping's failure destination: running a function's deployment
//! package is not implemented in this emulator (`Invoke` says so and returns no result
//! it did not produce), so nothing here reports an invocation, a function error or a
//! discarded batch.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for; every ARN these services mint must
/// name it.
const REGION: &str = "us-west-2";

/// The account id every emulator ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// Every value the model's `StreamViewType` enum declares, read from the list the
/// DynamoDB Streams crate derives from its model, so these rows cannot drift from the
/// enum.
const VIEW_TYPES: &[&str] = STREAM_VIEW_TYPE_VALUES;

/// The two values the model's `BillingMode` enum declares
/// (`CreateTable$BillingMode`: "one of: PAY_PER_REQUEST | PROVISIONED").
const BILLING_MODES: [&str; 2] = ["PAY_PER_REQUEST", "PROVISIONED"];

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern
/// (`^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/stream-consumer";

/// An opaque deployment package, base64-encoded as `FunctionCode$ZipFile` is on the
/// wire. Its CONTENTS are never interpreted: no runtime in this emulator runs a
/// deployment package, and this suite never claims one ran.
const PACKAGE_ZIP_BASE64: &str = "cGFja2FnZQ==";

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateEventSourceMapping` is bound
/// (`POST /2015-03-31/event-source-mappings`).
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The `State` a freshly created mapping carries and the one it settles into, from
/// `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
const MAPPING_STATE_CREATING: &str = "Creating";
/// See [`MAPPING_STATE_CREATING`].
const MAPPING_STATE_ENABLED: &str = "Enabled";

/// How long a poll for a documented asynchronous transition may take before this suite
/// calls it a hang. Generous slack over the emulator's own spans, because a loaded
/// machine is not a defect: what the bound is for is that the wait TERMINATES, which is
/// exactly what a client's waiter needs.
const SETTLE_DEADLINE: Duration = Duration::from_secs(10);

/// How long one poll sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The four services this hand-off needs, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them. DynamoDB and DynamoDB Streams share the
/// `dynamodb` signing name, so both have to be mounted for either to be addressed the
/// way a client addresses it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// The SigV4 credential scope a request to `signing_name` in [`REGION`] carries. Nothing
/// in it is verified: the harness turns request authentication off, and it is what
/// routes the request and what Region the resources it makes are scoped to.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One `awsJson1_0` RPC call: `POST /` carrying `X-Amz-Target:
/// <target_prefix>.<operation>`, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `restJson1` call to Lambda, as its status and parsed body.
async fn rest(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer 200.
async fn ddb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "dynamodb", "DynamoDB_20120810", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A DynamoDB Streams call (`awsJson1_0`, `DynamoDBStreams_20120810.<Operation>`) that
/// must answer 200.
async fn streams(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "dynamodb",
        "DynamoDBStreams_20120810",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// An SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The `CreateTable` body of one row.
fn create_table_body(name: &str, view_type: &str, billing: &str, with_sort_key: bool) -> Value {
    let mut key_schema = vec![json!({"AttributeName": "pk", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "pk", "AttributeType": "S"})];
    if with_sort_key {
        key_schema.push(json!({"AttributeName": "sk", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "sk", "AttributeType": "N"}));
    }
    let mut body = json!({
        "TableName": name,
        "KeySchema": key_schema,
        "AttributeDefinitions": definitions,
        "BillingMode": billing,
        "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
    });
    if billing == "PROVISIONED" {
        body["ProvisionedThroughput"] = json!({"ReadCapacityUnits": 1, "WriteCapacityUnits": 1});
    }
    body
}

/// The item one of these tables holds, and the `Key` addressing it.
fn item(with_sort_key: bool) -> Value {
    let mut stored = json!({"pk": {"S": "trigger"}, "payload": {"S": "written"}});
    if with_sort_key {
        stored["sk"] = json!({"N": "1"});
    }
    stored
}

/// See [`item`].
fn key_of(with_sort_key: bool) -> Value {
    let mut key = json!({"pk": {"S": "trigger"}});
    if with_sort_key {
        key["sk"] = json!({"N": "1"});
    }
    key
}

/// Poll `DescribeTable` until the table reports `ACTIVE`, answering every status it
/// reported on the way. EVERY poll must answer 200: a table the create accepted is never
/// missing, which is what makes a client's `table_exists` waiter terminate.
async fn statuses_until_active(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Vec<String> {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    while started.elapsed() < SETTLE_DEADLINE {
        let described = ddb(router, case, "DescribeTable", &json!({"TableName": name})).await;
        let reported = described["Table"]["TableStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if seen.last() != Some(&reported) {
            seen.push(reported.clone());
        }
        if reported == "ACTIVE" {
            return seen;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the table never reached ACTIVE within {SETTLE_DEADLINE:?}, saw {seen:?}")
}

/// Poll `GetEventSourceMapping` until the mapping settles out of `Creating`, answering
/// the body it last reported.
async fn mapping_until_settled(router: &MultiServiceRouter, case: &str, uuid: &str) -> Value {
    let started = Instant::now();
    let uri = format!("{MAPPINGS_URI}/{uuid}");
    let mut last = Value::Null;
    while started.elapsed() < SETTLE_DEADLINE {
        let (status, body) = rest(router, "GET", &uri, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEventSourceMapping must resolve the mapping the create \
             accepted, got {status} with {body}"
        );
        if body["State"].as_str() != Some(MAPPING_STATE_CREATING) {
            return body;
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the mapping never settled out of {MAPPING_STATE_CREATING} within \
         {SETTLE_DEADLINE:?}, last saw {last}"
    )
}

/// Every row of the table below: a `StreamViewType`, the billing mode the table is
/// created in, and whether it has a sort key.
///
/// The two secondary dimensions vary on different strides, so the rows CROSS them rather
/// than correlating them: with the four view types the model declares, every (billing
/// mode, sort key) pair is covered.
fn rows() -> Vec<(String, &'static str, &'static str, bool)> {
    VIEW_TYPES
        .iter()
        .enumerate()
        .map(|(position, view_type)| {
            let billing = BILLING_MODES[position % BILLING_MODES.len()];
            let with_sort_key = (position / BILLING_MODES.len()) % 2 == 1;
            (
                format!("{view_type} in {billing} (sort key: {with_sort_key})"),
                *view_type,
                billing,
                with_sort_key,
            )
        })
        .collect()
}

#[tokio::test]
async fn a_stream_enabled_table_is_describable_and_its_stream_is_mappable_by_lambda() {
    for (case, view_type, billing, with_sort_key) in rows() {
        let router = router();
        let table = "orders";

        // 1. The table and its stream are registered by the SAME create.
        let created = ddb(
            &router,
            &case,
            "CreateTable",
            &create_table_body(table, view_type, billing, with_sort_key),
        )
        .await;
        assert_eq!(
            created["TableDescription"]["TableStatus"],
            json!("CREATING"),
            "case {case:?}: the create reports the table CREATING, got {created}"
        );
        let stream_arn = created["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            stream_arn.starts_with(&format!(
                "arn:aws:dynamodb:{REGION}:{ACCOUNT_ID}:table/{table}/stream/"
            )),
            "case {case:?}: the create must report the stream it enabled, got {created}"
        );
        assert!(
            created["TableDescription"]["LatestStreamLabel"]
                .as_str()
                .is_some_and(|label| !label.is_empty()),
            "case {case:?}: a reported stream carries its label, got {created}"
        );

        // 2. The FIRST DescribeTable, with no wait before it — the first poll a client's
        //    waiter makes — resolves the table rather than refusing it.
        let (status, first) = rpc(
            &router,
            "dynamodb",
            "DynamoDB_20120810",
            "DescribeTable",
            &json!({"TableName": table}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first DescribeTable after the create must answer 200, got \
             {status} with {first}"
        );
        let reported = first["Table"]["TableStatus"].as_str().unwrap_or_default();
        assert!(
            reported == "CREATING" || reported == "ACTIVE",
            "case {case:?}: expected TableStatus CREATING or ACTIVE, got {reported:?} in {first}"
        );
        assert_eq!(
            first["Table"]["LatestStreamArn"],
            json!(stream_arn),
            "case {case:?}: DescribeTable must report the same stream the create did, got {first}"
        );

        // 3. And the waiter TERMINATES: the status reaches ACTIVE.
        let seen = statuses_until_active(&router, &case, table).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some("ACTIVE"),
            "case {case:?}: the table must reach ACTIVE, saw {seen:?}"
        );

        // 4. The data plane serves the table, which is what the waiter is waited on for.
        ddb(
            &router,
            &case,
            "PutItem",
            &json!({"TableName": table, "Item": item(with_sort_key)}),
        )
        .await;
        let read = ddb(
            &router,
            &case,
            "GetItem",
            &json!({"TableName": table, "Key": key_of(with_sort_key)}),
        )
        .await;
        assert_eq!(
            read["Item"]["payload"],
            json!({"S": "written"}),
            "case {case:?}: the item written to the active table must read back, got {read}"
        );

        // 5. The stream half resolves through the STREAMS surface, on the same ARN, and
        //    reports the table it belongs to and the view type it captures.
        let described = streams(
            &router,
            &case,
            "DescribeStream",
            &json!({"StreamArn": stream_arn}),
        )
        .await;
        assert_eq!(
            described["StreamDescription"]["TableName"],
            json!(table),
            "case {case:?}: the stream must name the table it belongs to, got {described}"
        );
        assert_eq!(
            described["StreamDescription"]["StreamViewType"],
            json!(view_type),
            "case {case:?}: the stream must report the view type the create asked for, got \
             {described}"
        );
        let shard_id = described["StreamDescription"]["Shards"][0]["ShardId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the stream must have a shard, got {described}"))
            .to_string();
        let minted = streams(
            &router,
            &case,
            "GetShardIterator",
            &json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        assert!(
            minted["ShardIterator"]
                .as_str()
                .is_some_and(|iterator| !iterator.is_empty()),
            "case {case:?}: a consumer must get an iterator over the table's stream, got {minted}"
        );

        // 6. The queue a mapping names as its failure destination.
        let queue = sqs(
            &router,
            &case,
            "CreateQueue",
            &json!({"QueueName": "orders-dlq"}),
        )
        .await;
        let queue_url = queue["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {queue}"))
            .to_string();
        let attributes = sqs(
            &router,
            &case,
            "GetQueueAttributes",
            &json!({"QueueUrl": queue_url, "AttributeNames": ["QueueArn"]}),
        )
        .await;
        let queue_arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
            .to_string();

        // 7. The function the mapping invokes.
        let (status, function) = rest(
            &router,
            "POST",
            FUNCTIONS_URI,
            &json!({
                "FunctionName": "orders-consumer",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {"ZipFile": PACKAGE_ZIP_BASE64},
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateFunction must answer HTTP 201, got {status} with {function}"
        );

        // 8. And the mapping over the stream DynamoDB minted, with the queue as its
        //    failure destination: the mapping is accepted, reports what it was asked for,
        //    and settles into Enabled.
        let (status, mapping) = rest(
            &router,
            "POST",
            MAPPINGS_URI,
            &json!({
                "FunctionName": "orders-consumer",
                "EventSourceArn": stream_arn,
                "StartingPosition": "TRIM_HORIZON",
                "MaximumRetryAttempts": 1,
                "DestinationConfig": {"OnFailure": {"Destination": queue_arn}},
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateEventSourceMapping over the table's stream must be accepted \
             with HTTP 202, got {status} with {mapping}"
        );
        assert_eq!(
            mapping["EventSourceArn"],
            json!(stream_arn),
            "case {case:?}: the mapping must poll the stream it was given, got {mapping}"
        );
        assert_eq!(
            mapping["State"],
            json!(MAPPING_STATE_CREATING),
            "case {case:?}: an accepted mapping starts Creating, got {mapping}"
        );
        assert_eq!(
            mapping["DestinationConfig"]["OnFailure"]["Destination"],
            json!(queue_arn),
            "case {case:?}: the mapping must carry the failure destination it was given, got \
             {mapping}"
        );
        assert_eq!(
            mapping["MaximumRetryAttempts"],
            json!(1),
            "case {case:?}: the mapping must carry the retry bound it was given, got {mapping}"
        );
        let uuid = mapping["UUID"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a UUID, got {mapping}"))
            .to_string();

        let settled = mapping_until_settled(&router, &case, &uuid).await;
        assert_eq!(
            settled["State"],
            json!(MAPPING_STATE_ENABLED),
            "case {case:?}: a mapping created enabled settles into {MAPPING_STATE_ENABLED}, got \
             {settled}"
        );
        assert_eq!(
            settled["EventSourceArn"],
            json!(stream_arn),
            "case {case:?}: the settled mapping still polls the table's stream, got {settled}"
        );
        assert_eq!(
            settled["DestinationConfig"]["OnFailure"]["Destination"],
            json!(queue_arn),
            "case {case:?}: the settled mapping still carries its failure destination, got \
             {settled}"
        );
    }
}
