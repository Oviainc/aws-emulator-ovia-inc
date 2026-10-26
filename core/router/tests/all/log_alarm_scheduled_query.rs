// SPDX-License-Identifier: CC0-1.0

//! A CloudWatch log alarm's service-managed scheduled query is readable in CloudWatch Logs.
//!
//! The rule under test: **"When you create a log alarm, the operation creates a service-managed
//! CloudWatch Logs scheduled query that runs the query string you provide on the schedule you
//! configure" (`PutLogAlarm`) — so the account holds a scheduled query no `CreateScheduledQuery`
//! created, which `GetScheduledQuery` reports under either form of its identifier (the name of
//! the alarm it belongs to, or the `QueryARN` the alarm's configuration names it by), carrying
//! that alarm's query string, log groups, schedule, offsets and execution role, with the
//! `AWS_MANAGED` schedule type that distinguishes it from a query a caller created — and which
//! is gone once the alarm is.** Two crates have to agree for that to hold (CloudWatch owns the
//! alarm, CloudWatch Logs answers for the scheduled query), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: how many log groups the alarm's query
//! reads (none, one, several), whether the lookback window states an end offset, and which form
//! the schedule expression takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// CloudWatch Logs' `X-Amz-Target` prefix and the signing name its credential scope carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// CloudWatch's `X-Amz-Target` prefix and signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The role CloudWatch runs a log alarm's managed query under
/// (`ScheduledQueryConfiguration$ScheduledQueryRoleARN`).
const QUERY_ROLE: &str = "arn:aws:iam::000000000000:role/cwl-scheduled-query";

/// The schedule type of a query the service manages for an alarm, and the one of a query a
/// caller created — the two values the model's `ScheduleType` enumerates.
const AWS_MANAGED: &str = "AWS_MANAGED";
const CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// The SigV4 header a request carries, naming the signing name the router routes on.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
    )
}

/// Call one operation of either service over its awsJson wire.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    content_type: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Call one CloudWatch Logs operation.
async fn logs_call(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        router,
        LOGS_SIGNING_NAME,
        LOGS_TARGET_PREFIX,
        "application/x-amz-json-1.1",
        operation,
        request,
    )
    .await
}

/// Call one CloudWatch Logs operation, requiring it to succeed.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) -> Value {
    let (status, parsed) = logs_call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from logs {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Call one CloudWatch operation, requiring it to succeed.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, parsed) = call(
        router,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        "application/x-amz-json-1.0",
        operation,
        request,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with body \
         {parsed}"
    );
    parsed
}

/// The wire error code of an awsJson error body (`__type`, which may carry a namespace).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an error body must carry __type, got {body}"));
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// One log alarm to write: the parts of its managed query's configuration this row varies.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    alarm_name: &'static str,
    query_string: &'static str,
    schedule_expression: &'static str,
    start_time_offset: i64,
    end_time_offset: Option<i64>,
    log_group_identifiers: Option<&'static [&'static str]>,
}

/// The alarms this suite writes. Each row's managed query differs along what the rule
/// generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no log groups and no end offset",
            alarm_name: "errors-anywhere",
            query_string: "fields @message | filter @message like /ERROR/",
            schedule_expression: "rate(5 minutes)",
            start_time_offset: 360,
            end_time_offset: None,
            log_group_identifiers: None,
        },
        Row {
            case: "one log group",
            alarm_name: "errors-in-api",
            query_string: "fields @message | filter @message like /ERROR/ | stats count(*)",
            schedule_expression: "rate(1 minute)",
            start_time_offset: 120,
            end_time_offset: Some(0),
            log_group_identifiers: Some(&["/app/api"]),
        },
        Row {
            case: "several log groups and a cron schedule",
            alarm_name: "errors-across-the-fleet",
            query_string: "fields @message | stats count(*) by host",
            schedule_expression: "cron(0 * * * ? *)",
            start_time_offset: 3_600,
            end_time_offset: Some(60),
            log_group_identifiers: Some(&["/app/api", "/app/worker", "/app/batch"]),
        },
    ]
}

/// The `PutLogAlarm` request one row sends: every member the operation requires, with this
/// row's scheduled query configuration.
fn put_log_alarm_request(row: &Row) -> Value {
    let mut schedule = Map::new();
    schedule.insert(
        "ScheduleExpression".to_string(),
        json!(row.schedule_expression),
    );
    schedule.insert("StartTimeOffset".to_string(), json!(row.start_time_offset));
    if let Some(end) = row.end_time_offset {
        schedule.insert("EndTimeOffset".to_string(), json!(end));
    }

    let mut configuration = Map::new();
    configuration.insert("QueryString".to_string(), json!(row.query_string));
    configuration.insert("ScheduledQueryRoleARN".to_string(), json!(QUERY_ROLE));
    configuration.insert("AggregationExpression".to_string(), json!("count(*)"));
    configuration.insert(
        "ScheduleConfiguration".to_string(),
        Value::Object(schedule),
    );
    if let Some(identifiers) = row.log_group_identifiers {
        configuration.insert("LogGroupIdentifiers".to_string(), json!(identifiers));
    }

    json!({
        "AlarmName": row.alarm_name,
        "QueryResultsToEvaluate": 3,
        "QueryResultsToAlarm": 2,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "ScheduledQueryConfiguration": Value::Object(configuration),
    })
}

/// The one log alarm `DescribeAlarms` reports under `name`.
async fn log_alarm(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let body = cloudwatch(
        router,
        case,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["LogAlarm"]}),
    )
    .await;
    let alarms = body
        .get("LogAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        alarms.len(),
        1,
        "case {case:?}: expected exactly the log alarm {name}, got {alarms:?}"
    );
    alarms[0].clone()
}

/// One string member of a response, or `None` when the response omits it.
fn member(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

/// One integer member of a response, or `None` when the response omits it.
fn number(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(Value::as_i64)
}

/// Every log alarm's managed scheduled query is reported by CloudWatch Logs — under the alarm's
/// name and under the ARN its configuration names the query by alike — carrying that alarm's
/// configuration and the `AWS_MANAGED` schedule type.
#[tokio::test]
async fn a_log_alarms_managed_scheduled_query_is_reported_by_cloudwatch_logs() {
    let router = build_router();

    for row in rows() {
        let case = row.case;
        cloudwatch(&router, case, "PutLogAlarm", &put_log_alarm_request(&row)).await;

        let alarm = log_alarm(&router, case, row.alarm_name).await;
        let configuration = alarm
            .get("ScheduledQueryConfiguration")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the alarm reports its scheduled query configuration")
            });
        let query_arn = member(&configuration, "QueryARN").unwrap_or_else(|| {
            panic!("case {case:?}: the alarm names the managed query's ARN, got {configuration}")
        });
        // "The time stamp of the last update to the alarm configuration", which CloudWatch puts
        // on the wire as epoch seconds.
        let configured_at_secs = alarm
            .get("AlarmConfigurationUpdatedTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the alarm reports when its configuration was written")
            });

        // Both forms of `ScheduledQueryIdentifier` name the same managed query.
        for (under, identifier) in [("name", row.alarm_name.to_string()), ("ARN", query_arn.clone())]
        {
            let read = logs(
                &router,
                case,
                "GetScheduledQuery",
                &json!({"identifier": identifier}),
            )
            .await;

            assert_eq!(
                member(&read, "name").as_deref(),
                Some(row.alarm_name),
                "case {case:?} read by {under}: the managed query carries the alarm's name, got \
                 {read}"
            );
            assert_eq!(
                member(&read, "scheduledQueryArn").as_deref(),
                Some(query_arn.as_str()),
                "case {case:?} read by {under}: the managed query carries the ARN the alarm \
                 names it by, got {read}"
            );
            assert_eq!(
                member(&read, "queryString").as_deref(),
                Some(row.query_string),
                "case {case:?} read by {under}: the managed query runs the alarm's query \
                 string, got {read}"
            );
            assert_eq!(
                member(&read, "scheduleExpression").as_deref(),
                Some(row.schedule_expression),
                "case {case:?} read by {under}: the managed query runs on the alarm's \
                 schedule, got {read}"
            );
            assert_eq!(
                member(&read, "executionRoleArn").as_deref(),
                Some(QUERY_ROLE),
                "case {case:?} read by {under}: the managed query runs under the alarm's \
                 query role, got {read}"
            );
            assert_eq!(
                number(&read, "startTimeOffset"),
                Some(row.start_time_offset),
                "case {case:?} read by {under}: the lookback window is the alarm's, got {read}"
            );
            assert_eq!(
                number(&read, "endTimeOffset"),
                row.end_time_offset,
                "case {case:?} read by {under}: the end offset is reported only when the alarm \
                 states one, got {read}"
            );
            let expected_groups = row.log_group_identifiers.map(|groups| json!(groups));
            assert_eq!(
                read.get("logGroupIdentifiers").cloned(),
                expected_groups,
                "case {case:?} read by {under}: the managed query reads the alarm's log \
                 groups, expected {expected_groups:?}, got {read}"
            );
            assert_eq!(
                member(&read, "scheduleType").as_deref(),
                Some(AWS_MANAGED),
                "case {case:?} read by {under}: a query the service manages is {AWS_MANAGED}, \
                 got {read}"
            );
            assert_eq!(
                member(&read, "state").as_deref(),
                Some("ENABLED"),
                "case {case:?} read by {under}: the managed query of a configured alarm is \
                 enabled, got {read}"
            );

            let last_updated = number(&read, "lastUpdatedTime").unwrap_or_else(|| {
                panic!("case {case:?} read by {under}: the managed query reports when it was last updated, got {read}")
            });
            let expected_ms = (configured_at_secs * 1_000.0).round() as i64;
            assert_eq!(
                last_updated, expected_ms,
                "case {case:?} read by {under}: the managed query was last written when the \
                 alarm's configuration was ({expected_ms}), got {last_updated}"
            );
            // The alarm records no instant of its own creation, so its managed query reports
            // none rather than reporting an update instant as one.
            assert_eq!(
                read.get("creationTime"),
                None,
                "case {case:?} read by {under}: no creation instant is invented, got {read}"
            );
        }
    }
}

/// A query a caller created and a query the service manages are both reported, and the schedule
/// type is what tells them apart. An identifier neither of them answers to is refused.
#[tokio::test]
async fn managed_and_caller_created_queries_are_distinguished_and_unknown_ones_refused() {
    let router = build_router();
    const ALARM: &str = "managed-vs-created-alarm";
    const CREATED: &str = "managed-vs-created-query";

    cloudwatch(
        &router,
        "both kinds",
        "PutLogAlarm",
        &put_log_alarm_request(&Row {
            case: "both kinds",
            alarm_name: ALARM,
            query_string: "fields @message",
            schedule_expression: "rate(5 minutes)",
            start_time_offset: 300,
            end_time_offset: None,
            log_group_identifiers: None,
        }),
    )
    .await;
    logs(
        &router,
        "both kinds",
        "CreateScheduledQuery",
        &json!({
            "name": CREATED,
            "queryLanguage": "CWLI",
            "queryString": "fields @message",
            "scheduleExpression": "cron(0 * * * ? *)",
            "executionRoleArn": QUERY_ROLE,
        }),
    )
    .await;

    // (the identifier, the schedule type the query reported under it carries)
    for (identifier, expected) in [(ALARM, AWS_MANAGED), (CREATED, CUSTOMER_MANAGED)] {
        let read = logs(
            &router,
            identifier,
            "GetScheduledQuery",
            &json!({"identifier": identifier}),
        )
        .await;
        assert_eq!(
            member(&read, "scheduleType").as_deref(),
            Some(expected),
            "case {identifier:?}: expected schedule type {expected}, got {read}"
        );
    }

    let (status, body) = logs_call(
        &router,
        "GetScheduledQuery",
        &json!({"identifier": "neither-an-alarm-nor-a-query"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an identifier nothing holds\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"an identifier nothing holds\": expected the missing-resource refusal, got {body}"
    );
}

/// The managed query belongs to its alarm, so deleting the alarm takes the query with it while
/// a query a caller created outlives it.
#[tokio::test]
async fn deleting_the_alarm_takes_its_managed_query_with_it() {
    let router = build_router();
    const ALARM: &str = "transient-log-alarm";
    const CREATED: &str = "surviving-scheduled-query";

    cloudwatch(
        &router,
        "deletion",
        "PutLogAlarm",
        &put_log_alarm_request(&Row {
            case: "deletion",
            alarm_name: ALARM,
            query_string: "fields @message",
            schedule_expression: "rate(5 minutes)",
            start_time_offset: 300,
            end_time_offset: None,
            log_group_identifiers: None,
        }),
    )
    .await;
    logs(
        &router,
        "deletion",
        "CreateScheduledQuery",
        &json!({
            "name": CREATED,
            "queryLanguage": "SQL",
            "queryString": "SELECT count(*) FROM `/app/api`",
            "scheduleExpression": "cron(0 * * * ? *)",
            "executionRoleArn": QUERY_ROLE,
        }),
    )
    .await;
    logs(
        &router,
        "deletion",
        "GetScheduledQuery",
        &json!({"identifier": ALARM}),
    )
    .await;

    cloudwatch(
        &router,
        "deletion",
        "DeleteAlarms",
        &json!({"AlarmNames": [ALARM]}),
    )
    .await;

    let (status, body) = logs_call(&router, "GetScheduledQuery", &json!({"identifier": ALARM})).await;
    assert_eq!(
        status, 400,
        "case \"the alarm's query after the alarm is deleted\": expected HTTP 400, got {status} \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"the alarm's query after the alarm is deleted\": the managed query goes with its \
         alarm, got {body}"
    );

    let read = logs(
        &router,
        "deletion",
        "GetScheduledQuery",
        &json!({"identifier": CREATED}),
    )
    .await;
    assert_eq!(
        member(&read, "name").as_deref(),
        Some(CREATED),
        "case \"a created query after an alarm is deleted\": it is the alarm's query that goes, \
         got {read}"
    );
}
