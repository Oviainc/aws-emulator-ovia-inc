// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs log groups an environment's ENABLED Apache Airflow log types
//! publish to: CloudWatch Logs is mounted beside this service on ONE engine and asked
//! through its own `DescribeLogGroups`, which is where a client looks for them.
//!
//! The RULE every row asserts: enabling an Apache Airflow log type asks Amazon MWAA to
//! publish that type's logs, and MWAA publishes them to a log group it creates for the
//! environment — `airflow-<EnvironmentName>-<LogType>` — whose ARN it then reports as
//! `LoggingConfiguration.<Module>.CloudWatchLogGroupArn` ("The Amazon Resource Name (ARN)
//! for the CloudWatch Logs group where the Apache Airflow log type … is published").
//! So for every log type a create or an update switches on, and for no other, CloudWatch
//! Logs holds a log group under that name, the ARN the environment reports for the type is
//! the ARN CloudWatch Logs reports for that group, and a group that was already there is
//! adopted rather than replaced or duplicated. A log type that publishes nothing has no
//! group and no reported ARN, and no read of an environment ever creates one.
//!
//! The rows differ along what the rule generalises over: WHICH log types are on (one, two,
//! all five, none, and one explicitly configured off), HOW they were switched on (by the
//! create or by a later update), the environment's NAME (one character, the punctuation
//! `EnvironmentName`'s pattern admits, the longest name it admits), the REGION the request
//! is signed for, and whether the group was already there before MWAA asked for it.
//!
//! The log-type names are written out here rather than read from this crate's own
//! constants, so a group created under a name no AWS client would look for fails these
//! rows.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a row signs for unless it is the row that varies the Region.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `X-Amz-Target` prefix of the CloudWatch Logs service (awsJson1.1).
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// Comfortably longer than this service's transition span, so an update the clock owes has
/// come due by the time a row reads the configuration it applied.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The longest `EnvironmentName` the model admits (`@length(1, 80)`). Source:
/// `smithy_query(query="shape", shape="EnvironmentName")`.
const MAX_ENVIRONMENT_NAME_LEN: usize = 80;

/// The five `LoggingConfiguration` module members, each beside the log type MWAA publishes
/// it under.
///
/// The member names are the modelled ones (`smithy_query(query="shape",
/// shape="LoggingConfiguration")`); the log types are the ones MWAA names its groups after
/// (`airflow-<EnvironmentName>-<LogType>`), the shape the
/// `ModuleLoggingConfiguration$CloudWatchLogGroupArn` doc's own example carries
/// (`arn:aws:logs:us-east-1:123456789012:log-group:airflow-MyMWAAEnvironment-MwaaEnvironment-DAGProcessing:*`).
const LOG_TYPES: [(&str, &str); 5] = [
    ("DagProcessingLogs", "DAGProcessing"),
    ("SchedulerLogs", "Scheduler"),
    ("TaskLogs", "Task"),
    ("WebserverLogs", "WebServer"),
    ("WorkerLogs", "Worker"),
];

/// This service and CloudWatch Logs on one engine: the groups MWAA creates are CloudWatch
/// Logs', so they are read in-process through its own surface — the shape
/// `core/router/src/lib.rs` mounts every service in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One Amazon MWAA call in `region`, over its restJson1 surface.
async fn mwaa(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("api.airflow.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/airflow/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch Logs call in `region`, over its awsJson1.1 surface.
async fn logs(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/logs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `LoggingConfiguration` a request carries for `enabled`: every module member named
/// there switched on, and every other one explicitly switched OFF — so a disabled log type
/// is a setting the request stated, not one it left out.
fn logging_configuration(enabled: &[&str]) -> Value {
    let mut modules = serde_json::Map::new();
    for (member, _) in LOG_TYPES {
        modules.insert(
            member.to_string(),
            json!({"Enabled": enabled.contains(&member), "LogLevel": "INFO"}),
        );
    }
    Value::Object(modules)
}

/// `CreateEnvironment` for `name` in `region`, with the log types `enabled` switched on.
async fn create_environment(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    enabled: &[&str],
) {
    let (status, body) = mwaa(
        router,
        region,
        "PUT",
        &format!("/environments/{name}"),
        Some(json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
            "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
            "DagS3Path": "dags",
            "NetworkConfiguration": {
                "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                "SecurityGroupIds": ["sg-111"],
            },
            "LoggingConfiguration": logging_configuration(enabled),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEnvironment for {name} must be served, got {status} {body}"
    );
}

/// `UpdateEnvironment` for `name`, carrying `patch` and nothing else.
async fn update_environment(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    patch: Value,
) {
    let (status, body) = mwaa(
        router,
        region,
        "PATCH",
        &format!("/environments/{name}"),
        Some(patch),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateEnvironment for {name} must be served, got {status} {body}"
    );
}

/// The `LoggingConfiguration` `GetEnvironment` reports for `name`.
async fn reported_logging(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> Value {
    let (status, body) = mwaa(router, region, "GET", &format!("/environments/{name}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnvironment for {name} must be served, got {status} {body}"
    );
    body.pointer("/Environment/LoggingConfiguration")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetEnvironment answered without a LoggingConfiguration: {body}")
        })
}

/// Every log group CloudWatch Logs holds whose name starts with `prefix`, keyed by name.
async fn described_log_groups(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    prefix: &str,
) -> BTreeMap<String, Value> {
    let (status, body) = logs(
        router,
        region,
        "DescribeLogGroups",
        &json!({"logGroupNamePrefix": prefix}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeLogGroups must be served, got {status} {body}"
    );
    body["logGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| {
                    group["logGroupName"]
                        .as_str()
                        .map(|name| (name.to_string(), group.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The name of the group a log type publishes to, as MWAA names it.
fn group_name(environment: &str, log_type: &str) -> String {
    format!("airflow-{environment}-{log_type}")
}

/// Assert that the groups CloudWatch Logs holds for `name` are EXACTLY the ones the log
/// types `enabled` publish to, and that each reported `CloudWatchLogGroupArn` is the ARN
/// CloudWatch Logs reports for the group it names (`LogGroup$arn`, the form carrying the
/// trailing `:*` that the `CloudWatchLogGroupArn` doc's example ends in) — while a log type
/// that publishes nothing reports no ARN and has no group.
async fn assert_groups_match_report(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    enabled: &[&str],
) {
    let logging = reported_logging(router, case, region, name).await;
    let held = described_log_groups(router, case, region, &format!("airflow-{name}-")).await;

    let mut expected: Vec<String> = Vec::new();
    for (member, log_type) in LOG_TYPES {
        let group = group_name(name, log_type);
        let reported = logging
            .pointer(&format!("/{member}/CloudWatchLogGroupArn"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if !enabled.contains(&member) {
            assert_eq!(
                reported, None,
                "case {case:?}: {member} publishes nothing, so it must report no \
                 CloudWatchLogGroupArn, got {reported:?}"
            );
            assert!(
                !held.contains_key(&group),
                "case {case:?}: no log group may be created for the disabled {member}, yet \
                 CloudWatch Logs holds {group:?}"
            );
            continue;
        }
        expected.push(group.clone());
        let reported = reported.unwrap_or_else(|| {
            panic!("case {case:?}: the enabled {member} must report a CloudWatchLogGroupArn")
        });
        assert!(
            reported.starts_with(&format!("arn:aws:logs:{region}:{ACCOUNT}:"))
                && reported.ends_with(&format!(":log-group:{group}:*")),
            "case {case:?}: {member} must report the ARN of the group {group:?} in {region}, \
             got {reported:?}"
        );
        let stored = held.get(&group).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the enabled {member} must have the log group {group:?} created \
                 for it, CloudWatch Logs holds {:?}",
                held.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            stored["arn"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: the ARN {member} reports must be the one CloudWatch Logs reports \
             for {group:?}, got {:?} against {reported:?}",
            stored["arn"]
        );
    }
    expected.sort();
    let names: Vec<String> = held.keys().cloned().collect();
    assert_eq!(
        names, expected,
        "case {case:?}: CloudWatch Logs must hold exactly the groups the enabled log types \
         publish to, got {names:?} against {expected:?}"
    );
}

#[tokio::test]
async fn every_log_type_a_create_enables_gets_the_log_group_its_reported_arn_names() {
    let longest = format!("E{}", "e".repeat(MAX_ENVIRONMENT_NAME_LEN - 1));
    // Each row: the case, the Region, the environment's name and the log types switched on.
    let rows: Vec<(&str, &str, String, Vec<&str>)> = vec![
        (
            "one log type",
            REGION,
            "LogsOne".to_string(),
            vec!["TaskLogs"],
        ),
        (
            "two log types",
            REGION,
            "Logs_Two-2".to_string(),
            vec!["SchedulerLogs", "WebserverLogs"],
        ),
        (
            "all five log types",
            REGION,
            "L".to_string(),
            LOG_TYPES.iter().map(|(member, _)| *member).collect(),
        ),
        ("no log type", REGION, "LogsNone".to_string(), vec![]),
        (
            "the longest name the model admits",
            REGION,
            longest,
            vec!["WorkerLogs"],
        ),
        (
            "another Region",
            "eu-west-2",
            "LogsLondon".to_string(),
            vec!["DagProcessingLogs", "TaskLogs"],
        ),
    ];

    for (case, region, name, enabled) in rows {
        let router = emulator();
        create_environment(&router, case, region, &name, &enabled).await;
        assert_groups_match_report(&router, case, region, &name, &enabled).await;

        // The create is `@idempotent`: replaying it adopts the groups the first one created
        // rather than opening a second set, so the assertion above holds unchanged.
        create_environment(&router, case, region, &name, &enabled).await;
        assert_groups_match_report(&router, case, region, &name, &enabled).await;
    }
}

#[tokio::test]
async fn a_log_type_an_update_switches_on_gets_its_group_and_switching_one_off_keeps_it() {
    let router = emulator();
    let case = "a log type switched on, then off, by an update";
    let name = "LogsUpdated";
    create_environment(&router, case, REGION, name, &["TaskLogs"]).await;
    // One wait for the create: an update is accepted only once the create has settled.
    tokio::time::sleep(PAST_TRANSITION).await;
    assert_groups_match_report(&router, case, REGION, name, &["TaskLogs"]).await;
    let task_group = group_name(name, "Task");
    let created_at = described_log_groups(&router, case, REGION, &task_group).await[&task_group]
        ["creationTime"]
        .clone();

    // Switching one ON: the group it publishes to is created by the call that asks for it,
    // so it is there before the update has even settled.
    update_environment(
        &router,
        case,
        REGION,
        name,
        json!({"LoggingConfiguration": {"SchedulerLogs": {"Enabled": true, "LogLevel": "WARNING"}}}),
    )
    .await;
    let scheduler_group = group_name(name, "Scheduler");
    let held = described_log_groups(&router, case, REGION, &format!("airflow-{name}-")).await;
    assert!(
        held.contains_key(&scheduler_group),
        "case {case:?}: the update that switched SchedulerLogs on must create \
         {scheduler_group:?}, CloudWatch Logs holds {:?}",
        held.keys().collect::<Vec<_>>()
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    assert_groups_match_report(&router, case, REGION, name, &["SchedulerLogs", "TaskLogs"]).await;

    // Switching one OFF: it publishes nothing, so no ARN is reported for it — and the group
    // is left where it is, because the Apache Airflow logs already in it belong to the
    // account.
    update_environment(
        &router,
        case,
        REGION,
        name,
        json!({"LoggingConfiguration": {"TaskLogs": {"Enabled": false, "LogLevel": "INFO"}}}),
    )
    .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let logging = reported_logging(&router, case, REGION, name).await;
    assert_eq!(
        logging.pointer("/TaskLogs/CloudWatchLogGroupArn"),
        None,
        "case {case:?}: TaskLogs publishes nothing once it is switched off, so no \
         CloudWatchLogGroupArn is reported for it, got {logging}"
    );
    let held = described_log_groups(&router, case, REGION, &format!("airflow-{name}-")).await;
    assert_eq!(
        held.keys().cloned().collect::<Vec<String>>(),
        vec![scheduler_group.clone(), task_group.clone()],
        "case {case:?}: switching a log type off leaves its group where it is, got {:?}",
        held.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        held[&task_group]["creationTime"], created_at,
        "case {case:?}: the group {task_group:?} must be the one the create made, not a \
         replacement, got {:?} against {created_at:?}",
        held[&task_group]["creationTime"]
    );
}

#[tokio::test]
async fn a_log_group_the_account_already_holds_is_adopted_rather_than_replaced() {
    let router = emulator();
    let case = "a log group the account created itself";
    let name = "LogsAdopted";
    let group = group_name(name, "Task");

    // The account creates the group first, under the name MWAA publishes TaskLogs to, and
    // sets a retention on it.
    let (status, body) = logs(
        &router,
        REGION,
        "CreateLogGroup",
        &json!({"logGroupName": group, "tags": {"owner": "the-account"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateLogGroup must be served, got {status} {body}"
    );
    let (status, body) = logs(
        &router,
        REGION,
        "PutRetentionPolicy",
        &json!({"logGroupName": group, "retentionInDays": 7}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutRetentionPolicy must be served, got {status} {body}"
    );
    let before = described_log_groups(&router, case, REGION, &group).await;
    let created_at = before[&group]["creationTime"].clone();

    // MWAA is then asked to publish that very log type: the group under that name is what
    // the environment publishes to, so it is adopted — not replaced, and not duplicated.
    create_environment(&router, case, REGION, name, &["TaskLogs"]).await;
    assert_groups_match_report(&router, case, REGION, name, &["TaskLogs"]).await;
    let after = described_log_groups(&router, case, REGION, &group).await;
    assert_eq!(
        after[&group]["creationTime"], created_at,
        "case {case:?}: the group {group:?} the account held must be kept, got a creationTime \
         of {:?} against {created_at:?}",
        after[&group]["creationTime"]
    );
    assert_eq!(
        after[&group]["retentionInDays"],
        json!(7),
        "case {case:?}: the retention the account set on {group:?} must survive, got {:?}",
        after[&group]["retentionInDays"]
    );
}

#[tokio::test]
async fn reading_an_environment_never_creates_a_log_group() {
    let router = emulator();
    let case = "reads of an environment that publishes nothing";
    let name = "LogsQuiet";
    create_environment(&router, case, REGION, name, &[]).await;

    for round in 0..3 {
        let logging = reported_logging(&router, case, REGION, name).await;
        for (member, _) in LOG_TYPES {
            assert_eq!(
                logging.pointer(&format!("/{member}/CloudWatchLogGroupArn")),
                None,
                "case {case:?} (read {round}): {member} publishes nothing, so it reports no \
                 CloudWatchLogGroupArn, got {logging}"
            );
        }
        let (status, body) = mwaa(&router, REGION, "GET", "/environments", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListEnvironments must be served, got {status} {body}"
        );
        let held = described_log_groups(&router, case, REGION, "airflow-").await;
        assert!(
            held.is_empty(),
            "case {case:?} (read {round}): a read must create no log group, CloudWatch Logs \
             holds {:?}",
            held.keys().collect::<Vec<_>>()
        );
    }
}
