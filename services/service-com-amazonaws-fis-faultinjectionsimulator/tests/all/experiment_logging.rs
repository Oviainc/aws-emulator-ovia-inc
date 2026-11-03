// SPDX-License-Identifier: CC0-1.0

//! EXPERIMENT LOGGING: what a template's `logConfiguration` makes an experiment's run
//! DELIVER, end to end over HTTP, across the three services it spans.
//!
//! RULE: an experiment started from a template that configures a logging destination
//! delivers the records of its own run to that destination while it runs, so a later read
//! of the destination service reports them — and a template that configures none delivers
//! nothing. The destination is whatever the configuration names: a CloudWatch Logs log
//! group (addressed by ARN), an Amazon S3 bucket under an optional key prefix, or both at
//! once; every record carries the experiment it belongs to, the event, the instant that
//! event fell due and the `logSchemaVersion` the configuration asked for.
//!
//! This suite lives beside the service rather than in `core/router` because the two
//! destination services are mounted on the SAME engine FIS runs on, exactly as the
//! assembled emulator mounts them: the records are read back through the real
//! `logs:GetLogEvents` and `s3:ListObjectsV2` / `s3:GetObject`, so what is asserted is what
//! those services persisted and not a copy.
//!
//! The rows differ along what the rule generalises over: WHICH destinations the
//! configuration names (CloudWatch Logs, S3, both, neither), the FORM of the log group ARN
//! (with and without the `:*` suffix CloudWatch Logs reports for a group), whether an S3
//! prefix is named at all and how deep it is, and the `logSchemaVersion` the records are
//! written under.
//!
//! Two further rules are pinned by their own cases below:
//!
//! * the run's records keep arriving AS the run proceeds — the start and its target
//!   resolution when the experiment is started, the terminal record once the run ends — so
//!   a client that reads the destination after the run finds the whole run and not just its
//!   beginning. `StopExperiment` is part of that: a stopped run's records say so, and so is
//!   a stop the SERVICE performs when the account's safety lever is engaged.
//! * delivery is BEST EFFORT. A destination that does not resolve (a log group nobody
//!   created, a bucket that is not there) is not the caller's problem: the FIS call that
//!   would have written to it is answered exactly as it would be with a destination that
//!   does resolve, and the experiment still runs to a terminal status.

use std::time::Duration;

use serde_json::{json, Value};

use crate::experiment_lifecycle::{
    create_template, get_experiment, poll_until_final, send, start_experiment, stop_experiment,
    token, Answer, FALLBACK_REGION,
};

/// The account id every ARN this emulator mints carries, which the log group ARNs below
/// name too.
const ACCOUNT_ID: &str = "000000000000";

/// A role ARN the emulator's IAM store holds no role for, which is what every row here
/// runs under: with no stored role there is nothing to show about its trust policy or its
/// permissions, so the run goes ahead. (See `tests/all/experiment_run.rs`.)
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-logging";

/// The bucket every S3 row delivers into.
pub(crate) const BUCKET: &str = "fis-experiment-logs";

/// How long a row waits for its run to reach a terminal status: the emulator's whole
/// compressed run window plus the transitions around it and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(25);

/// How many experiments may be active at once — the account's published quota, which is
/// what the rows are started in groups of.
const ACTIVE_EXPERIMENTS_QUOTA: usize = 5;

/// The `log_type` of the record a started experiment produces, as
/// `src/state/experiment_logs.rs` names it. Spelled out here rather than imported, so a
/// changed vocabulary fails this suite.
pub(crate) const LOG_TYPE_EXPERIMENT_START: &str = "experiment-start";
/// See [`LOG_TYPE_EXPERIMENT_START`].
const LOG_TYPE_TARGET_RESOLUTION: &str = "target-resolution";
/// See [`LOG_TYPE_EXPERIMENT_START`].
const LOG_TYPE_EXPERIMENT_STATE_CHANGE: &str = "experiment-state-change";
/// See [`LOG_TYPE_EXPERIMENT_START`].
pub(crate) const LOG_TYPE_EXPERIMENT_END: &str = "experiment-end";

/// The account's one safety lever, which FIS names by a fixed id.
const LEVER_ID: &str = "default";

/// One emulator carrying FIS, CloudWatch Logs and Amazon S3 over ONE shared engine,
/// exactly as the production router builds them.
pub(crate) fn emulator() -> smithy_server_runtime::MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        std::sync::Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine),
    );
    router
}

/// One CloudWatch Logs call (awsJson1_1), answered with the HTTP status and the JSON body.
async fn logs(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "logs.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{FALLBACK_REGION}/logs/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One path-style Amazon S3 call: S3 is a REST service, so the bucket and key live in the
/// URI and the body is the object's own bytes.
pub(crate) async fn s3(
    router: &smithy_server_runtime::MultiServiceRouter,
    method: &str,
    path: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "s3.us-east-1.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{FALLBACK_REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// The log group ARN for `name`, in the form `with_suffix` asks for: CloudWatch Logs
/// reports a group's ARN with a trailing `:*`, and the FIS documentation shows a logging
/// destination written either way.
pub(crate) fn log_group_arn(name: &str, with_suffix: bool) -> String {
    let base = format!("arn:aws:logs:{FALLBACK_REGION}:{ACCOUNT_ID}:log-group:{name}");
    if with_suffix {
        format!("{base}:*")
    } else {
        base
    }
}

/// Create the log group `name`, so the destination the configuration names resolves.
///
/// A group another case already created is accepted, and NOTHING else is: a refusal of any
/// other kind would leave a case asserting against a destination that is not there.
pub(crate) async fn given_log_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) {
    let (status, body) = logs(router, "CreateLogGroup", &json!({ "logGroupName": name })).await;
    if status == 200 {
        return;
    }
    let code = body["__type"].as_str().unwrap_or_default();
    assert!(
        code.ends_with("ResourceAlreadyExistsException"),
        "creating the log group {name} must succeed or report the group is already there, \
         got {status} with {body:?}"
    );
}

/// Create [`BUCKET`], so the S3 destination resolves.
pub(crate) async fn given_bucket(router: &smithy_server_runtime::MultiServiceRouter) {
    let (status, body) = s3(router, "PUT", &format!("/{BUCKET}")).await;
    assert!(
        status == 200 || status == 409,
        "creating s3://{BUCKET} must succeed or already exist, got {status} with {body}"
    );
}

/// `GetLogEvents` on the stream an experiment's run writes to: the HTTP status it answered
/// and the records it reported, oldest first, parsed.
///
/// The status is handed back rather than swallowed, so a case asserting that NOTHING was
/// delivered cannot pass on a call that merely failed.
async fn log_events(
    router: &smithy_server_runtime::MultiServiceRouter,
    log_group: &str,
    id: &str,
) -> (u16, Vec<Value>) {
    let (status, body) = logs(
        router,
        "GetLogEvents",
        &json!({
            "logGroupName": log_group,
            "logStreamName": id,
            "startFromHead": true
        }),
    )
    .await;
    let records = body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str())
                .filter_map(|message| serde_json::from_str::<Value>(message).ok())
                .collect()
        })
        .unwrap_or_default();
    (status, records)
}

/// Every record delivered to `log_group` for the experiment `id`, oldest first, parsed —
/// for a stream that must be readable.
pub(crate) async fn records_in_log_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    log_group: &str,
    id: &str,
) -> Vec<Value> {
    let (status, records) = log_events(router, log_group, id).await;
    assert_eq!(
        status, 200,
        "reading the stream {id} of {log_group} must succeed, got {status}"
    );
    records
}

/// The keys of every object under `prefix` in [`BUCKET`], from `ListObjectsV2`.
pub(crate) async fn keys_under(
    router: &smithy_server_runtime::MultiServiceRouter,
    prefix: &str,
) -> Vec<String> {
    let query = if prefix.is_empty() {
        format!("/{BUCKET}?list-type=2")
    } else {
        format!("/{BUCKET}?list-type=2&prefix={prefix}")
    };
    let (status, listing) = s3(router, "GET", &query).await;
    assert_eq!(
        status, 200,
        "listing s3://{BUCKET} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// The `log_type` of every record, in the order they were delivered.
pub(crate) fn log_types(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| record["log_type"].as_str())
        .collect()
}

/// What one row configures, and what that configuration must make the run deliver.
struct Row {
    case: &'static str,
    /// The template's `logConfiguration`, or `None` for a template that configures none.
    log_configuration: Option<Value>,
    /// The log group every record must reach, when the configuration names one.
    log_group: Option<&'static str>,
    /// The key prefix every object must be written under, when the configuration names an
    /// S3 destination: `Some("")` is a destination with no prefix at all.
    s3_prefix: Option<&'static str>,
    /// The `logSchemaVersion` every record must carry.
    schema_version: Option<i64>,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "cloudwatch-logs-only, the arn carrying the :* suffix",
            log_configuration: Some(json!({
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": log_group_arn("/aws/fis/suffixed", true)
                },
                "logSchemaVersion": 2
            })),
            log_group: Some("/aws/fis/suffixed"),
            s3_prefix: None,
            schema_version: Some(2),
        },
        Row {
            case: "cloudwatch-logs-only, the arn without the suffix",
            log_configuration: Some(json!({
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": log_group_arn("/aws/fis/plain", false)
                },
                "logSchemaVersion": 1
            })),
            log_group: Some("/aws/fis/plain"),
            s3_prefix: None,
            schema_version: Some(1),
        },
        Row {
            case: "s3-only under a single-segment prefix",
            log_configuration: Some(json!({
                "s3Configuration": { "bucketName": BUCKET, "prefix": "experiments" },
                "logSchemaVersion": 2
            })),
            log_group: None,
            s3_prefix: Some("experiments"),
            schema_version: Some(2),
        },
        Row {
            case: "s3-only under a nested prefix",
            log_configuration: Some(json!({
                "s3Configuration": { "bucketName": BUCKET, "prefix": "logs/fis/runs" },
                "logSchemaVersion": 1
            })),
            log_group: None,
            s3_prefix: Some("logs/fis/runs"),
            schema_version: Some(1),
        },
        Row {
            case: "s3-only with no prefix at all",
            log_configuration: Some(json!({
                "s3Configuration": { "bucketName": BUCKET },
                "logSchemaVersion": 2
            })),
            log_group: None,
            s3_prefix: Some(""),
            schema_version: Some(2),
        },
        Row {
            case: "both destinations at once",
            log_configuration: Some(json!({
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": log_group_arn("/aws/fis/both", true)
                },
                "s3Configuration": { "bucketName": BUCKET, "prefix": "both" },
                "logSchemaVersion": 2
            })),
            log_group: Some("/aws/fis/both"),
            s3_prefix: Some("both"),
            schema_version: Some(2),
        },
        Row {
            case: "no logConfiguration at all",
            log_configuration: None,
            log_group: None,
            s3_prefix: None,
            schema_version: None,
        },
    ]
}

/// A `clientToken`-safe rendering of a case label: `ClientToken` is modelled
/// `^[\S]+$`, so a label's spaces and punctuation become `-`.
pub(crate) fn slug(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The template `row` configures: one target resolved by ARN (so the run has a resolution
/// to report) and one action that names no duration (so the run is the compressed floor).
fn template_body(row: &Row) -> Value {
    let mut body = json!({
        "clientToken": token(&format!("logging-template-{}", slug(row.case))),
        "description": format!("experiment logging: {}", row.case),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {
            "instances": {
                "resourceType": "aws:ec2:instance",
                "resourceArns": [
                    "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"
                ],
                "selectionMode": "ALL"
            }
        },
        "actions": {
            "assert": {
                "actionId": "aws:cloudwatch:assert-alarm-state",
                "parameters": {
                    "alarmArns": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:cpu-high",
                    "alarmStates": "INSUFFICIENT_DATA"
                }
            }
        }
    });
    if let Some(configuration) = &row.log_configuration {
        body["logConfiguration"] = configuration.clone();
    }
    body
}

/// Create `row`'s template and start an experiment from it, answering the experiment id.
async fn start_row(router: &smithy_server_runtime::MultiServiceRouter, row: &Row) -> String {
    let created = create_template(router, template_body(row)).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the template to be created, got {} with {:?}",
        row.case, created.status, created.body
    );
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let started = start_experiment(
        router,
        json!({
            "clientToken": token(&format!("logging-start-{}", slug(row.case))),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {:?}: expected the start to be accepted, got {} with {:?}",
        row.case, started.status, started.body
    );
    started.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no experiment id in {:?}", row.case, started.body))
        .to_string()
}

/// Assert every record delivered for `id` belongs to it, carries `row`'s schema version
/// and a timestamp, and is one of the lifecycle records this service produces.
fn assert_envelopes(row: &Row, where_: &str, id: &str, records: &[Value]) {
    for record in records {
        assert_eq!(
            record["id"].as_str(),
            Some(id),
            "case {:?}: a record in {where_} must name the experiment it belongs to, \
             got {record:?}",
            row.case
        );
        assert_eq!(
            record["log_schema_version"].as_i64(),
            row.schema_version,
            "case {:?}: a record in {where_} must carry the configured logSchemaVersion, \
             got {record:?}",
            row.case
        );
        let stamp = record["event_timestamp"].as_str().unwrap_or_default();
        assert!(
            stamp.ends_with('Z') && stamp.len() >= 20,
            "case {:?}: a record in {where_} must carry an RFC 3339 UTC event_timestamp, \
             got {stamp:?} in {record:?}",
            row.case
        );
        let log_type = record["log_type"].as_str().unwrap_or_default();
        assert!(
            [
                LOG_TYPE_EXPERIMENT_START,
                LOG_TYPE_TARGET_RESOLUTION,
                LOG_TYPE_EXPERIMENT_STATE_CHANGE,
                LOG_TYPE_EXPERIMENT_END,
            ]
            .contains(&log_type),
            "case {:?}: a record in {where_} must report a lifecycle event, got {record:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_configured_experiment_delivers_its_run_to_the_destination_it_names() {
    let router = emulator();
    let rows = rows();

    // Every destination the rows name exists before any experiment is started: a log group
    // is a resource FIS writes INTO, not one it creates.
    given_bucket(&router).await;
    for row in &rows {
        if let Some(log_group) = row.log_group {
            given_log_group(&router, log_group).await;
        }
    }

    // The rows are started in groups no larger than the account's active-experiment quota,
    // so their runs overlap instead of being waited out one after another.
    for group in rows.chunks(ACTIVE_EXPERIMENTS_QUOTA.min(3)) {
        let mut started: Vec<(&Row, String)> = Vec::new();
        for row in group {
            let id = start_row(&router, row).await;
            started.push((row, id));
        }

        // The START's own records are there as soon as the start is answered: the run has
        // begun, so what the start made true is already at the destination.
        for (row, id) in &started {
            if let Some(log_group) = row.log_group {
                let records = records_in_log_group(&router, log_group, id).await;
                assert_envelopes(row, "CloudWatch Logs", id, &records);
                let types = log_types(&records);
                assert!(
                    types.contains(&LOG_TYPE_EXPERIMENT_START),
                    "case {:?}: the configured log group must hold the start record, \
                     got {types:?}",
                    row.case
                );
                assert!(
                    types.contains(&LOG_TYPE_TARGET_RESOLUTION),
                    "case {:?}: the run resolved a target, so the log group must hold the \
                     resolution record, got {types:?}",
                    row.case
                );
            }
            if let Some(prefix) = row.s3_prefix {
                let keys = keys_under(&router, prefix).await;
                let own: Vec<&String> = keys.iter().filter(|key| key.contains(id)).collect();
                assert!(
                    !own.is_empty(),
                    "case {:?}: the configured bucket must hold the start's objects under \
                     {prefix:?}, got {keys:?}",
                    row.case
                );
                for key in &own {
                    assert!(
                        key.starts_with(&if prefix.is_empty() {
                            format!("{id}/")
                        } else {
                            format!("{prefix}/{id}/")
                        }),
                        "case {:?}: an object must be written under the configured prefix, \
                         got {key:?}",
                        row.case
                    );
                }
            }
            if row.log_configuration.is_none() {
                // A template that configures nothing delivers nothing, anywhere: neither
                // the group another row configured nor the bucket holds anything of this
                // experiment's. The answer itself is asserted, not just its record count,
                // so this cannot pass on a read that merely failed: either the group holds
                // no stream for this experiment at all, or it holds one with no records.
                let (status, body) = logs(
                    &router,
                    "GetLogEvents",
                    &json!({
                        "logGroupName": "/aws/fis/suffixed",
                        "logStreamName": id,
                        "startFromHead": true
                    }),
                )
                .await;
                let delivered = body["events"]
                    .as_array()
                    .is_some_and(|events| !events.is_empty());
                assert!(
                    !delivered,
                    "case {:?}: a template with no logConfiguration must deliver no log \
                     events, got {status} with {body:?}",
                    row.case
                );
                if status != 200 {
                    let code = body["__type"].as_str().unwrap_or_default();
                    assert!(
                        code.ends_with("ResourceNotFoundException"),
                        "case {:?}: the only refusal expected is a stream that was never \
                         created, got {status} with {body:?}",
                        row.case
                    );
                }
                let keys = keys_under(&router, "").await;
                let own: Vec<&String> = keys.iter().filter(|key| key.contains(id)).collect();
                assert!(
                    own.is_empty(),
                    "case {:?}: a template with no logConfiguration must write no objects, \
                     got {own:?}",
                    row.case
                );
            }
        }

        // And the run keeps delivering: once it has ended, the destination holds the
        // terminal record too, reporting the status the run reached.
        for (row, id) in &started {
            let experiment = poll_until_final(&router, id, TERMINAL_BUDGET).await;
            let status = experiment["state"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                status, "completed",
                "case {:?}: expected the run to reach the status its actions earned, \
                 got {experiment:?}",
                row.case
            );

            if let Some(log_group) = row.log_group {
                let records = records_in_log_group(&router, log_group, id).await;
                assert_envelopes(row, "CloudWatch Logs", id, &records);
                let types = log_types(&records);
                assert_eq!(
                    types.first(),
                    Some(&LOG_TYPE_EXPERIMENT_START),
                    "case {:?}: the run's records arrive in the order it produced them, \
                     got {types:?}",
                    row.case
                );
                assert!(
                    types.contains(&LOG_TYPE_EXPERIMENT_STATE_CHANGE),
                    "case {:?}: the move to running is a record of the run, got {types:?}",
                    row.case
                );
                let end = records
                    .iter()
                    .find(|record| record["log_type"].as_str() == Some(LOG_TYPE_EXPERIMENT_END))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: the log group must hold the run's terminal record, \
                             got {types:?}",
                            row.case
                        )
                    });
                assert_eq!(
                    end["details"]["status"].as_str(),
                    Some(status.as_str()),
                    "case {:?}: the terminal record reports the status the run reached, \
                     got {end:?}",
                    row.case
                );
                assert!(
                    end["details"]["reason"]
                        .as_str()
                        .is_some_and(|reason| !reason.is_empty()),
                    "case {:?}: the terminal record says why the run ended, got {end:?}",
                    row.case
                );
            }

            if let Some(prefix) = row.s3_prefix {
                let keys = keys_under(&router, prefix).await;
                let mut own: Vec<&String> = keys.iter().filter(|key| key.contains(id)).collect();
                own.sort();
                assert!(
                    own.iter()
                        .any(|key| key.ends_with(&format!("-{LOG_TYPE_EXPERIMENT_END}.json"))),
                    "case {:?}: the bucket must hold the run's terminal record, got {own:?}",
                    row.case
                );
                // The objects are the records: each one is readable and is this
                // experiment's.
                let mut records = Vec::new();
                for key in &own {
                    let (status, body) = s3(&router, "GET", &format!("/{BUCKET}/{key}")).await;
                    assert_eq!(
                        status, 200,
                        "case {:?}: the object {key} must be readable, got {status} \
                         with {body}",
                        row.case
                    );
                    records.push(serde_json::from_str::<Value>(&body).unwrap_or_else(|error| {
                        panic!(
                            "case {:?}: the object {key} must hold one JSON record \
                             ({error}), got {body}",
                            row.case
                        )
                    }));
                }
                assert_envelopes(row, "Amazon S3", id, &records);
                let types = log_types(&records);
                assert!(
                    types.contains(&LOG_TYPE_EXPERIMENT_START)
                        && types.contains(&LOG_TYPE_TARGET_RESOLUTION),
                    "case {:?}: the bucket must hold the whole run, got {types:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE: a stop is part of the run, so the destination reports it — the request to stop and
/// the run's end under the status a stop earns.
#[tokio::test]
async fn a_stopped_experiment_delivers_the_stop_it_was_given() {
    const LOG_GROUP: &str = "/aws/fis/stopped";
    let router = emulator();
    given_log_group(&router, LOG_GROUP).await;

    let row = Row {
        case: "a stopped run",
        log_configuration: Some(json!({
            "cloudWatchLogsConfiguration": { "logGroupArn": log_group_arn(LOG_GROUP, true) },
            "logSchemaVersion": 2
        })),
        log_group: Some(LOG_GROUP),
        s3_prefix: None,
        schema_version: Some(2),
    };
    let id = start_row(&router, &row).await;

    let stopped = stop_experiment(&router, &id).await;
    assert_eq!(
        stopped.status, 200,
        "expected the stop to be accepted, got {} with {:?}",
        stopped.status, stopped.body
    );
    let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
    assert_eq!(
        experiment["state"]["status"].as_str(),
        Some("stopped"),
        "expected the stopped experiment to have ended, got {experiment:?}"
    );

    let records = records_in_log_group(&router, LOG_GROUP, &id).await;
    assert_envelopes(&row, "CloudWatch Logs", &id, &records);
    let types = log_types(&records);
    assert_eq!(
        types.first(),
        Some(&LOG_TYPE_EXPERIMENT_START),
        "the run's first record is its start, got {types:?}"
    );
    let statuses: Vec<&str> = records
        .iter()
        .filter_map(|record| record["details"]["status"].as_str())
        .collect();
    assert!(
        statuses.contains(&"stopping"),
        "the stop the experiment was given is a record of the run, got {statuses:?}"
    );
    let end = records
        .iter()
        .find(|record| record["log_type"].as_str() == Some(LOG_TYPE_EXPERIMENT_END))
        .unwrap_or_else(|| panic!("the run's terminal record is absent, got {types:?}"));
    assert_eq!(
        end["details"]["status"].as_str(),
        Some("stopped"),
        "the terminal record reports the status a stopped run reached, got {end:?}"
    );
}

/// RULE: a stop the SERVICE performs is part of the run just as a caller's `StopExperiment`
/// is, so it reaches the destination too — engaging the account's safety lever stops the
/// experiments that are running, and each stopped run's own records say so, carrying the
/// reason that stop was recorded under.
#[tokio::test]
async fn a_safety_lever_stop_reaches_the_destination_the_run_configured() {
    const LOG_GROUP: &str = "/aws/fis/lever";
    let router = emulator();
    given_log_group(&router, LOG_GROUP).await;

    let row = Row {
        case: "a run stopped by the safety lever",
        log_configuration: Some(json!({
            "cloudWatchLogsConfiguration": { "logGroupArn": log_group_arn(LOG_GROUP, true) },
            "logSchemaVersion": 2
        })),
        log_group: Some(LOG_GROUP),
        s3_prefix: None,
        schema_version: Some(2),
    };
    let id = start_row(&router, &row).await;

    let engaged = send(
        &router,
        "PATCH",
        &format!("/safetyLevers/{LEVER_ID}/state"),
        Some(json!({ "state": { "status": "engaged", "reason": "experiment logging" } })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        engaged.status, 200,
        "case {:?}: expected the lever to engage, got {} with {:?}",
        row.case, engaged.status, engaged.body
    );

    // The lever's stop is delivered by the call that performed it, so the records are at
    // the destination before anything reads the experiment back.
    let records = records_in_log_group(&router, LOG_GROUP, &id).await;
    assert_envelopes(&row, "CloudWatch Logs", &id, &records);
    let stopping = records
        .iter()
        .find(|record| record["details"]["status"].as_str() == Some("stopping"))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the stop the lever performed must be a record of the run, got {:?}",
                row.case,
                log_types(&records)
            )
        });
    assert!(
        stopping["details"]["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "case {:?}: the stopping record carries the reason the stop was recorded under, \
         got {stopping:?}",
        row.case
    );

    let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
    assert_eq!(
        experiment["state"]["status"].as_str(),
        Some("stopped"),
        "case {:?}: expected the lever to have stopped the run, got {experiment:?}",
        row.case
    );
    let records = records_in_log_group(&router, LOG_GROUP, &id).await;
    let end = records
        .iter()
        .find(|record| record["log_type"].as_str() == Some(LOG_TYPE_EXPERIMENT_END))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the run's terminal record is absent, got {:?}",
                row.case,
                log_types(&records)
            )
        });
    assert_eq!(
        end["details"]["status"].as_str(),
        Some("stopped"),
        "case {:?}: the terminal record reports the status a stopped run reached, got {end:?}",
        row.case
    );
}

/// RULE: delivery is best effort. A destination that does not resolve is not the caller's
/// problem — the FIS call is answered exactly as it is for a destination that does, and the
/// experiment runs to a terminal status either way.
///
/// The rows differ along which destination is missing: a log group nobody created, a bucket
/// that is not there, an ARN that names no log group at all, and both halves missing at
/// once.
#[tokio::test]
async fn a_destination_that_does_not_resolve_does_not_fail_the_run() {
    let router = emulator();

    let configurations: [(&str, Value); 4] = [
        (
            "a log group nobody created",
            json!({
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": log_group_arn("/aws/fis/never-created", true)
                },
                "logSchemaVersion": 2
            }),
        ),
        (
            "a bucket that is not there",
            json!({
                "s3Configuration": { "bucketName": "fis-no-such-bucket", "prefix": "x" },
                "logSchemaVersion": 2
            }),
        ),
        (
            "an arn that names no log group",
            json!({
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": "arn:aws:logs:us-east-1:000000000000:something-else"
                },
                "logSchemaVersion": 1
            }),
        ),
        (
            "both halves missing at once",
            json!({
                "cloudWatchLogsConfiguration": {
                    "logGroupArn": log_group_arn("/aws/fis/also-never-created", false)
                },
                "s3Configuration": { "bucketName": "fis-also-no-such-bucket" },
                "logSchemaVersion": 2
            }),
        ),
    ];

    for (case, configuration) in configurations {
        let created = create_template(
            &router,
            json!({
                "clientToken": token(&format!("unresolvable-template-{}", slug(case))),
                "description": format!("an unresolvable destination: {case}"),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": {
                    "assert": {
                        "actionId": "aws:cloudwatch:assert-alarm-state",
                        "parameters": {
                            "alarmArns": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:a",
                            "alarmStates": "OK"
                        }
                    }
                },
                "logConfiguration": configuration
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a destination FIS cannot write to is still a configuration it \
             stores, got {} with {:?}",
            created.status, created.body
        );
        let template_id = created.member("experimentTemplate")["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("unresolvable-start-{}", slug(case))),
                "experimentTemplateId": template_id
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the start to be answered, got {} with {:?}",
            started.status, started.body
        );
        let id = started.member("experiment")["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The record the start reports is the stored configuration, unchanged: a delivery
        // that could not happen changes nothing about the experiment.
        let read: Answer = get_experiment(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the experiment to be readable, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("experiment")["logConfiguration"]["logSchemaVersion"],
            configuration["logSchemaVersion"],
            "case {case:?}: the experiment reports the configuration it ran with, got {:?}",
            read.body
        );

        let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
        assert_eq!(
            experiment["state"]["status"].as_str(),
            Some("completed"),
            "case {case:?}: the run reaches its terminal status whatever its destination \
             did, got {experiment:?}"
        );
    }
}

/// RULE: the records of one experiment are its own. Two experiments started from one
/// template, logging to one destination, each deliver into a stream and a key space of
/// their own, so a reader can tell one run's records from the other's.
#[tokio::test]
async fn two_runs_sharing_one_destination_keep_their_records_apart() {
    const LOG_GROUP: &str = "/aws/fis/shared";
    const PREFIX: &str = "shared";
    let router = emulator();
    given_log_group(&router, LOG_GROUP).await;
    given_bucket(&router).await;

    let created = create_template(
        &router,
        json!({
            "clientToken": token("shared-destination-template"),
            "description": "two runs, one destination",
            "roleArn": ROLE_ARN,
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "actions": {
                "assert": {
                    "actionId": "aws:cloudwatch:assert-alarm-state",
                    "parameters": {
                        "alarmArns": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:cpu",
                        "alarmStates": "OK"
                    }
                }
            },
            "logConfiguration": {
                "cloudWatchLogsConfiguration": { "logGroupArn": log_group_arn(LOG_GROUP, true) },
                "s3Configuration": { "bucketName": BUCKET, "prefix": PREFIX },
                "logSchemaVersion": 2
            }
        }),
    )
    .await;
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut ids = Vec::new();
    for case in ["first", "second"] {
        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("shared-destination-start-{case}")),
                "experimentTemplateId": template_id
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the start to be accepted, got {} with {:?}",
            started.status, started.body
        );
        ids.push(
            started.member("experiment")["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert_ne!(ids[0], ids[1], "two starts are two experiments, got {ids:?}");

    for id in &ids {
        let experiment = poll_until_final(&router, id, TERMINAL_BUDGET).await;
        assert_eq!(
            experiment["state"]["status"].as_str(),
            Some("completed"),
            "expected the run {id} to have ended, got {experiment:?}"
        );
    }

    let row = Row {
        case: "two runs sharing one destination",
        log_configuration: None,
        log_group: Some(LOG_GROUP),
        s3_prefix: Some(PREFIX),
        schema_version: Some(2),
    };
    let keys = keys_under(&router, PREFIX).await;
    for id in &ids {
        let records = records_in_log_group(&router, LOG_GROUP, id).await;
        assert_envelopes(&row, "CloudWatch Logs", id, &records);
        assert!(
            log_types(&records).contains(&LOG_TYPE_EXPERIMENT_END),
            "expected the whole run of {id} in its own stream, got {records:?}"
        );
        let own: Vec<&String> = keys
            .iter()
            .filter(|key| key.starts_with(&format!("{PREFIX}/{id}/")))
            .collect();
        assert!(
            !own.is_empty(),
            "expected the records of {id} under their own key space, got {keys:?}"
        );
    }
    // And nothing of either run's landed outside the two key spaces.
    let stray: Vec<&String> = keys
        .iter()
        .filter(|key| ids.iter().all(|id| !key.contains(id.as_str())))
        .collect();
    assert!(
        stray.is_empty(),
        "every object under {PREFIX:?} belongs to one of the runs, got {stray:?}"
    );
}

/// RULE: the log destination and the report output are SEPARATE destinations the
/// configuration names separately, so each receives only what it was named for. A template
/// configuring both delivers the run's log records to its `logConfiguration` destinations
/// and its report to the `experimentReportConfiguration` output, and neither lands in the
/// other.
#[tokio::test]
async fn the_log_destination_and_the_report_output_stay_apart() {
    const LOG_GROUP: &str = "/aws/fis/with-a-report";
    const LOG_PREFIX: &str = "logs-half";
    const REPORT_BUCKET: &str = "fis-experiment-reports";
    let router = emulator();
    given_log_group(&router, LOG_GROUP).await;
    given_bucket(&router).await;
    let (status, body) = s3(&router, "PUT", &format!("/{REPORT_BUCKET}")).await;
    assert!(
        status == 200 || status == 409,
        "creating s3://{REPORT_BUCKET} must be answered, got {status} with {body}"
    );

    let row = Row {
        case: "a run configured for both logging and a report",
        log_configuration: Some(json!({
            "cloudWatchLogsConfiguration": { "logGroupArn": log_group_arn(LOG_GROUP, true) },
            "s3Configuration": { "bucketName": BUCKET, "prefix": LOG_PREFIX },
            "logSchemaVersion": 2
        })),
        log_group: Some(LOG_GROUP),
        s3_prefix: Some(LOG_PREFIX),
        schema_version: Some(2),
    };
    let mut body = template_body(&row);
    body["experimentReportConfiguration"] = json!({
        "outputs": { "s3Configuration": { "bucketName": REPORT_BUCKET, "prefix": "r" } },
        "dataSources": {
            "cloudWatchDashboards": [
                { "dashboardIdentifier": "arn:aws:cloudwatch::000000000000:dashboard/fis" }
            ]
        }
    });
    let created = create_template(&router, body).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the template to be created, got {} with {:?}",
        row.case, created.status, created.body
    );
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let started = start_experiment(
        &router,
        json!({
            "clientToken": token("logging-and-report-start"),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {:?}: expected the start to be accepted, got {} with {:?}",
        row.case, started.status, started.body
    );
    let id = started.member("experiment")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
    assert_eq!(
        experiment["experimentReportConfiguration"]["outputs"]["s3Configuration"]["bucketName"]
            .as_str(),
        Some(REPORT_BUCKET),
        "case {:?}: the experiment reports the report configuration it ran with, \
         got {experiment:?}",
        row.case
    );

    // The run's own records are at the destinations the logConfiguration named.
    let records = records_in_log_group(&router, LOG_GROUP, &id).await;
    assert_envelopes(&row, "CloudWatch Logs", &id, &records);
    let types = log_types(&records);
    assert!(
        types.contains(&LOG_TYPE_EXPERIMENT_START) && types.contains(&LOG_TYPE_EXPERIMENT_END),
        "case {:?}: the log group holds the whole run, got {types:?}",
        row.case
    );
    let log_keys = keys_under(&router, LOG_PREFIX).await;
    assert!(
        log_keys.iter().any(|key| key.contains(&id)),
        "case {:?}: the logConfiguration's bucket holds the run's records, got {log_keys:?}",
        row.case
    );

    // And nothing of the report is among them: the report output is a bucket of its own,
    // which holds only what the report configuration named it for.
    let (status, listing) = s3(&router, "GET", &format!("/{REPORT_BUCKET}?list-type=2")).await;
    assert_eq!(
        status, 200,
        "case {:?}: listing s3://{REPORT_BUCKET} must succeed, got {status} with {listing}",
        row.case
    );
    for log_type in [
        LOG_TYPE_EXPERIMENT_START,
        LOG_TYPE_TARGET_RESOLUTION,
        LOG_TYPE_EXPERIMENT_STATE_CHANGE,
        LOG_TYPE_EXPERIMENT_END,
    ] {
        assert!(
            !listing.contains(log_type),
            "case {:?}: no log record is written to the report output, got {listing}",
            row.case
        );
    }
    let report_arns: Vec<&str> = experiment["experimentReport"]["s3Reports"]
        .as_array()
        .map(|reports| {
            reports
                .iter()
                .filter_map(|report| report["arn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    for arn in &report_arns {
        assert!(
            arn.contains(REPORT_BUCKET),
            "case {:?}: the report is written to the output its own configuration named, \
             got {arn:?}",
            row.case
        );
    }
}
