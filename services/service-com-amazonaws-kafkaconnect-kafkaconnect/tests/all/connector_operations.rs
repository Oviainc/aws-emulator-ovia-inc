// SPDX-License-Identifier: CC0-1.0

//! Connector operations, end to end over HTTP: the record a restart starts, and the
//! two operations that report it.
//!
//! RULE. A call that starts asynchronous work against a connector STORES one connector
//! operation against that connector, and the reporting operations answer from that
//! stored record: `ListConnectorOperations` reports one summary per operation of the
//! connector it names (oldest first, paged by `maxResults` with an opaque `nextToken`,
//! each operation returned exactly once), `DescribeConnectorOperation` reports the one
//! operation an ARN names, and both carry the state the CLOCK has walked the operation
//! to — with no `endTime` until it has reached its terminal state. An ARN naming no
//! stored resource is the operations' modelled 404, and a `nextToken` this service did
//! not emit a 400.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response body read
//! back as JSON. Nothing reaches into the state struct, so a handler answering from the
//! request instead of from state would fail here.
//!
//! The tables span what the rule generalises over — each `onlyFailedTasks` form, every
//! page size across the published `maxResults` range, several connectors side by side,
//! and each non-terminal connector state a restart must be refused in — rather than a
//! single recorded call.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    RESTART_PROGRESSION, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN must
/// carry. No authenticator is registered, so the signature is never checked; the scope
/// is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Long enough that one change the clock owes after a call is due. The span itself is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// Long enough that a restart's whole progression is due: one span per state it passes
/// through, both of them the service's own constants.
async fn past_the_whole_restart() {
    let steps = u32::try_from(RESTART_PROGRESSION.len()).expect("a small progression");
    tokio::time::sleep(TRANSITION_SPAN * steps + Duration::from_millis(400)).await;
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

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS
/// SDK encodes a label: every character a path segment reserves (including the ARN's own
/// `:` and `/`) is escaped. Encoded with the same `percent-encoding` crate the server
/// side uses, rather than a second encoder.
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

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `value` is the UUID AWS gives the random part of a kafkaconnect resource
/// ARN: the 8-4-4-4-12 lowercase-hex form, with RFC 4122's version (4) and variant
/// (8..b) nibbles.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(want, group)| group.len() == *want)
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        && groups[2].starts_with('4')
        && matches!(groups[3].as_bytes().first(), Some(b'8' | b'9' | b'a' | b'b'))
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

/// Create one ACTIVE custom plugin and return its ARN.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) =
        call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    past_one_transition().await;
    text(&created, "customPluginArn")
}

/// Create the named connectors against `plugin_arn` and return their ARNs, in the same
/// order, once the clock has moved every one of them to `RUNNING`.
async fn running_connectors(
    emulator: &MultiServiceRouter,
    plugin_arn: &str,
    names: &[&str],
) -> Vec<String> {
    let mut arns = Vec::new();
    for name in names {
        let (status, created) = call(
            emulator,
            "POST",
            "/v1/connectors",
            connector_request(name, plugin_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnector must succeed, got {status} {created}"
        );
        arns.push(text(&created, "connectorArn"));
    }
    past_one_transition().await;
    arns
}

/// `DescribeConnector`'s `connectorState` for `connector_arn`.
async fn connector_state(emulator: &MultiServiceRouter, connector_arn: &str) -> String {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeConnector must succeed, got {status} {described}"
    );
    text(&described, "connectorState")
}

/// `RestartConnector` against `connector_arn`, with `only_failed_tasks` as the query
/// parameter when the case names one.
async fn restart(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    only_failed_tasks: Option<bool>,
) -> (u16, Value) {
    let mut path = format!("/v1/connectors/{}/restart", label(connector_arn));
    if let Some(only_failed_tasks) = only_failed_tasks {
        path.push_str(&format!("?onlyFailedTasks={only_failed_tasks}"));
    }
    call(emulator, "POST", &path, Value::Null).await
}

/// `ListConnectorOperations` for `connector_arn`, with the query string `query`
/// appended as-is (empty for none).
async fn list_operations(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    query: &str,
) -> (u16, Value) {
    let path = format!(
        "/v1/connectors/{}/operations{query}",
        label(connector_arn)
    );
    call(emulator, "GET", &path, Value::Null).await
}

/// `DescribeConnectorOperation` for `operation_arn`.
async fn describe_operation(
    emulator: &MultiServiceRouter,
    operation_arn: &str,
) -> (u16, Value) {
    let path = format!("/v1/connectorOperations/{}", label(operation_arn));
    call(emulator, "GET", &path, Value::Null).await
}

/// The summaries `ListConnectorOperations` answered with, as a list.
fn summaries(listed: &Value) -> Vec<Value> {
    listed["connectorOperations"]
        .as_array()
        .unwrap_or_else(|| panic!("a connectorOperations array in {listed}"))
        .clone()
}

/// One row of the restart table: the three forms `onlyFailedTasks` takes.
///
/// `RestartConnector$onlyFailedTasks` is modelled `__boolean` with `default false`, so
/// the class a restart generalises over is: the member absent, explicitly false, and
/// true. An operation is recorded the same way for each.
struct RestartCase {
    connector: &'static str,
    only_failed_tasks: Option<bool>,
}

const RESTART_CASES: &[RestartCase] = &[
    RestartCase {
        connector: "default-restart",
        only_failed_tasks: None,
    },
    RestartCase {
        connector: "all-tasks-restart",
        only_failed_tasks: Some(false),
    },
    RestartCase {
        connector: "failed-tasks-restart",
        only_failed_tasks: Some(true),
    },
];

#[tokio::test]
async fn a_restart_records_an_operation_that_reads_back_from_state_and_ends_on_the_clock() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "operation-plugin").await;
    let names: Vec<&str> = RESTART_CASES.iter().map(|case| case.connector).collect();
    let connector_arns = running_connectors(&emulator, &plugin_arn, &names).await;

    let mut started: Vec<(&RestartCase, String, String, String)> = Vec::new();
    for (case, connector_arn) in RESTART_CASES.iter().zip(&connector_arns) {
        let (status, restarted) = restart(&emulator, connector_arn, case.only_failed_tasks).await;
        assert_eq!(
            status, 200,
            "case {:?}: RestartConnector must succeed, got {status} {restarted}",
            case.connector
        );
        assert_eq!(
            text(&restarted, "connectorArn"),
            *connector_arn,
            "case {:?}: the answer must name the connector that was restarted, got {restarted}",
            case.connector
        );

        // The operation's ARN belongs to the connector's own scope, with its own minted
        // id appended: `…:connector-operation/<name>/<connector id>/<operation id>`.
        let operation_arn = text(&restarted, "connectorOperationArn");
        let prefix = format!(
            "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/{}/",
            case.connector
        );
        let ids: Vec<&str> = operation_arn
            .strip_prefix(&prefix)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a connectorOperationArn starting {prefix}, got \
                     {operation_arn:?}",
                    case.connector
                )
            })
            .split('/')
            .collect();
        assert!(
            ids.len() == 2 && ids.iter().all(|id| is_uuid(id)),
            "case {:?}: expected the connector's id and the operation's id, both UUIDs, got \
             {ids:?}",
            case.connector
        );

        // The connector itself is restarting until the clock brings it back.
        let state = connector_state(&emulator, connector_arn).await;
        assert_eq!(
            state, "RESTARTING",
            "case {:?}: a restarted connector must be RESTARTING, got {state:?}",
            case.connector
        );

        // Read straight back: the operation is stored, non-terminal, and carries no
        // `endTime` because it has not ended.
        let (status, described) = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeConnectorOperation must succeed, got {status} {described}",
            case.connector
        );
        assert_eq!(
            text(&described, "connectorOperationType"),
            "RESTART_CONNECTOR",
            "case {:?}: a restart records a RESTART_CONNECTOR operation, got {described}",
            case.connector
        );
        assert_eq!(
            text(&described, "connectorArn"),
            *connector_arn,
            "case {:?}: the operation must report the connector it ran against, got {described}",
            case.connector
        );
        let state = text(&described, "connectorOperationState");
        assert!(
            state == "PENDING" || state == "RESTART_IN_PROGRESS",
            "case {:?}: a just-started restart must still be running, got {state:?}",
            case.connector
        );
        assert!(
            described.get("endTime").is_none(),
            "case {:?}: a running operation carries no endTime, got {described}",
            case.connector
        );
        let creation_time = text(&described, "creationTime");
        assert!(
            !creation_time.is_empty(),
            "case {:?}: the operation must report its creationTime, got {described}",
            case.connector
        );

        // The list path reports the same stored record as the describe path.
        let (status, listed) = list_operations(&emulator, connector_arn, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: ListConnectorOperations must succeed, got {status} {listed}",
            case.connector
        );
        let items = summaries(&listed);
        assert_eq!(
            items.len(),
            1,
            "case {:?}: exactly the one restart must be listed, got {listed}",
            case.connector
        );
        assert_eq!(
            text(&items[0], "connectorOperationArn"),
            operation_arn,
            "case {:?}: the summary must name the stored operation, got {listed}",
            case.connector
        );
        assert_eq!(
            text(&items[0], "creationTime"),
            creation_time,
            "case {:?}: both read paths must report the stored creationTime, got {listed}",
            case.connector
        );
        assert!(
            items[0].get("endTime").is_none(),
            "case {:?}: a running operation's summary carries no endTime, got {listed}",
            case.connector
        );

        started.push((
            case,
            connector_arn.clone(),
            operation_arn,
            creation_time,
        ));
    }

    past_the_whole_restart().await;

    for (case, connector_arn, operation_arn, creation_time) in &started {
        let (status, described) = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeConnectorOperation must succeed, got {status} {described}",
            case.connector
        );
        assert_eq!(
            text(&described, "connectorOperationState"),
            "RESTART_COMPLETE",
            "case {:?}: the clock must finish the restart, got {described}",
            case.connector
        );
        let end_time = text(&described, "endTime");
        assert!(
            !end_time.is_empty() && end_time >= *creation_time,
            "case {:?}: a finished operation reports an endTime at or after its \
             creationTime {creation_time:?}, got {end_time:?}",
            case.connector
        );
        assert_eq!(
            text(&described, "creationTime"),
            *creation_time,
            "case {:?}: reading an operation must not change its creationTime, got {described}",
            case.connector
        );

        let (status, listed) = list_operations(&emulator, connector_arn, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: ListConnectorOperations must succeed, got {status} {listed}",
            case.connector
        );
        let items = summaries(&listed);
        assert_eq!(
            items.len(),
            1,
            "case {:?}: reading an operation must not add or drop one, got {listed}",
            case.connector
        );
        assert_eq!(
            text(&items[0], "connectorOperationState"),
            "RESTART_COMPLETE",
            "case {:?}: both read paths report the same stored state, got {listed}",
            case.connector
        );
        assert_eq!(
            text(&items[0], "endTime"),
            end_time,
            "case {:?}: both read paths report the same stored endTime, got {listed}",
            case.connector
        );

        // The connector is back to RUNNING, on the clock alone.
        let state = connector_state(&emulator, connector_arn).await;
        assert_eq!(
            state, "RUNNING",
            "case {:?}: the clock must return a restarted connector to RUNNING, got {state:?}",
            case.connector
        );
    }
}

#[tokio::test]
async fn a_connectors_operations_page_oldest_first_each_exactly_once() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "paging-plugin").await;
    let arns = running_connectors(&emulator, &plugin_arn, &["paged", "other"]).await;
    let (paged, other) = (&arns[0], &arns[1]);

    // Four operations against one connector, in a known order, plus one against the
    // connector beside it, which must never appear in the first one's pages.
    let mut expected: Vec<String> = Vec::new();
    for round in 0..4 {
        let (status, restarted) = restart(&emulator, paged, Some(round % 2 == 0)).await;
        assert_eq!(
            status, 200,
            "round {round}: RestartConnector must succeed, got {status} {restarted}"
        );
        expected.push(text(&restarted, "connectorOperationArn"));
        // The connector is RESTARTING until the clock returns it to RUNNING, and only a
        // RUNNING connector can be restarted again.
        past_one_transition().await;
    }
    let (status, restarted) = restart(&emulator, other, None).await;
    assert_eq!(
        status, 200,
        "the other connector's restart must succeed, got {status} {restarted}"
    );
    let other_operation = text(&restarted, "connectorOperationArn");

    // Every page size across the published `MaxResults` range (1..100) pages the same
    // four operations, each exactly once and oldest first.
    for page_size in [1_i32, 2, 4, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("?maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, listed) = list_operations(&emulator, paged, &query).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListConnectorOperations must succeed, got {status} \
                 {listed}"
            );
            let items = summaries(&listed);
            assert!(
                items.len() <= usize::try_from(page_size).expect("a positive page size"),
                "page size {page_size}: a page must not exceed maxResults, got {} items",
                items.len()
            );
            for summary in &items {
                let arn = text(summary, "connectorOperationArn");
                assert_ne!(
                    arn, other_operation,
                    "page size {page_size}: another connector's operation must not be \
                     listed, got {summary}"
                );
                assert_eq!(
                    text(summary, "connectorOperationType"),
                    "RESTART_CONNECTOR",
                    "page size {page_size}: every summary carries its stored type, got {summary}"
                );
                seen.push(arn);
            }
            pages += 1;
            assert!(pages <= 10, "page size {page_size}: paging must terminate");
            match listed.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: paging must not repeat an operation, saw {seen:?}"
        );
        assert_eq!(
            seen, expected,
            "page size {page_size}: paging must return every operation of the connector, \
             oldest first"
        );
    }

    // The connector beside it keeps its own single operation.
    let (status, listed) = list_operations(&emulator, other, "").await;
    assert_eq!(
        status, 200,
        "the other connector's operations must list, got {status} {listed}"
    );
    let items = summaries(&listed);
    assert_eq!(
        items.len(),
        1,
        "an operation is listed only for the connector it ran against, got {listed}"
    );
    assert_eq!(
        text(&items[0], "connectorOperationArn"),
        other_operation,
        "the other connector must list its own operation, got {listed}"
    );

    // The published `maxResults` range is 1..100: each bound is accepted and a value
    // past either end is refused by the model's own constraint.
    for max_results in [1, 100] {
        let (status, listed) =
            list_operations(&emulator, paged, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 200,
            "maxResults={max_results} is inside the modelled range, got {status} {listed}"
        );
    }
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 101)] {
        let (status, answered) =
            list_operations(&emulator, paged, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: maxResults={max_results} is outside the modelled range, got \
             {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {answered}"
        );
    }

    // A token this service never issued is a bad request, not an empty page and not a
    // silently honoured resume point: one that is not even the token's encoding, and one
    // that is — hex that decodes to a plausible key — but carries none of its marker.
    for forged in [
        "not-a-token".to_string(),
        "6161".to_string(),
        hex::encode(format!("{paged}/operation")),
    ] {
        let (status, answered) =
            list_operations(&emulator, paged, &format!("?nextToken={}", label(&forged))).await;
        assert_eq!(
            status, 400,
            "token {forged:?}: an unissued nextToken is a 400, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "token {forged:?}: expected BadRequestException, got {answered}"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_the_modelled_not_found() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "missing-plugin").await;
    let arns = running_connectors(&emulator, &plugin_arn, &["doomed"]).await;
    let doomed = &arns[0];

    let (status, restarted) = restart(&emulator, doomed, None).await;
    assert_eq!(
        status, 200,
        "the restart must succeed, got {status} {restarted}"
    );
    let doomed_operation = text(&restarted, "connectorOperationArn");

    // Every form of a `connectorArn` this emulator holds no connector of: one of the
    // right shape that was never created, one of another service, and a value that is no
    // ARN at all.
    let absent_connectors = [
        format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/never-made/{}", "0".repeat(8)),
        format!("arn:aws:iam::{ACCOUNT}:role/not-a-connector"),
        "not-an-arn".to_string(),
    ];
    for connector_arn in &absent_connectors {
        let (status, answered) = list_operations(&emulator, connector_arn, "").await;
        assert_eq!(
            status, 404,
            "case {connector_arn:?}: listing the operations of a connector this emulator \
             does not hold is a 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {connector_arn:?}: expected NotFoundException, got {answered}"
        );
        assert!(
            text(&answered, "message").contains("connectorArn"),
            "case {connector_arn:?}: the message must name the parameter it refused, got \
             {answered}"
        );
    }

    let absent_operations = [
        format!(
            "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/doomed/{}/{}",
            "0".repeat(8),
            "1".repeat(8)
        ),
        "not-an-arn".to_string(),
    ];
    for operation_arn in &absent_operations {
        let (status, answered) = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            status, 404,
            "case {operation_arn:?}: describing an operation this emulator does not hold is \
             a 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {operation_arn:?}: expected NotFoundException, got {answered}"
        );
        assert!(
            text(&answered, "message").contains("connectorOperationArn"),
            "case {operation_arn:?}: the message must name the parameter it refused, got \
             {answered}"
        );
    }

    // An operation belongs to its connector: once the connector's delete has finished,
    // neither the connector nor its operations are there to report.
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v1/connectors/{}", label(doomed)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConnector must succeed, got {status} {deleted}"
    );
    past_one_transition().await;

    let (status, answered) = list_operations(&emulator, doomed, "").await;
    assert_eq!(
        status, 404,
        "a deleted connector's operations must not list, got {status} {answered}"
    );
    let (status, answered) = describe_operation(&emulator, &doomed_operation).await;
    assert_eq!(
        status, 404,
        "a deleted connector's operation must not describe, got {status} {answered}"
    );
}

#[tokio::test]
async fn a_connector_that_is_not_running_is_refused_a_restart_and_records_nothing() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "refusal-plugin").await;

    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_request("refused", &plugin_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnector must succeed, got {status} {created}"
    );
    let connector_arn = text(&created, "connectorArn");

    /// Assert the restart was refused as a bad request, naming the member it blamed.
    async fn refused(emulator: &MultiServiceRouter, connector_arn: &str, case: &str) {
        let (status, answered) = restart(emulator, connector_arn, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a connector that is not RUNNING cannot be restarted, got \
             {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {answered}"
        );
        assert!(
            text(&answered, "message").contains("connectorArn"),
            "case {case:?}: the message must name the parameter it refused, got {answered}"
        );
    }

    // CREATING: the connector has no tasks to restart yet.
    refused(&emulator, &connector_arn, "CREATING").await;
    let (status, listed) = list_operations(&emulator, &connector_arn, "").await;
    assert_eq!(
        status, 200,
        "the connector's operations must list, got {status} {listed}"
    );
    assert!(
        summaries(&listed).is_empty(),
        "a refused restart must record no operation, got {listed}"
    );

    past_one_transition().await;
    let (status, restarted) = restart(&emulator, &connector_arn, None).await;
    assert_eq!(
        status, 200,
        "a RUNNING connector restarts, got {status} {restarted}"
    );

    // RESTARTING: the restart already under way is the one in progress.
    refused(&emulator, &connector_arn, "RESTARTING").await;
    let (status, listed) = list_operations(&emulator, &connector_arn, "").await;
    assert_eq!(
        status, 200,
        "the connector's operations must list, got {status} {listed}"
    );
    assert_eq!(
        summaries(&listed).len(),
        1,
        "a refused restart must record no second operation, got {listed}"
    );

    // DELETING: a connector on its way out is not restarted either.
    past_the_whole_restart().await;
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v1/connectors/{}", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConnector must succeed, got {status} {deleted}"
    );
    refused(&emulator, &connector_arn, "DELETING").await;
}
