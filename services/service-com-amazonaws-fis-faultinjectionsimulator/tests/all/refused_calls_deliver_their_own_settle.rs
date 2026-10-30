// SPDX-License-Identifier: CC0-1.0

//! RULE: an operation whose settle materialises a transition the clock already owed is the
//! operation that DELIVERS the records that settle produced — to the destination each
//! experiment's template configured, with the state guard released — whatever it then
//! answers the caller. A refusal is no exception: the call that materialised the end of a
//! run cannot leave that run's terminal record parked in the store for some later call to
//! happen upon.
//!
//! The rows differ along what the rule generalises over: WHICH operation settles and then
//! refuses (`StopExperiment`, `StartExperiment`), WHICH refusal it answers with (a
//! `ValidationException` for an experiment already in a final state, a `ConflictException`
//! for a `clientToken` carried by another request, a `ResourceNotFoundException` for a
//! template that is not there) and WHERE the run's records go (a CloudWatch Logs log group,
//! an Amazon S3 prefix).
//!
//! Each row runs on its OWN emulator, because draining the outbox is a whole-store
//! operation: a call another row made would deliver this row's records and the case would
//! prove nothing about the call under test. Between the start and the refusal a row makes
//! NO call to FIS at all — the destination is read through CloudWatch Logs and Amazon S3,
//! which settle nothing — so the only call that can have delivered the terminal record is
//! the refused one.

use std::time::Duration;

use serde_json::{json, Value};

use crate::experiment_lifecycle::{
    create_template, get_experiment, start_experiment, stop_experiment, token, Answer,
};
use crate::experiment_logging::{
    emulator, given_bucket, given_log_group, keys_under, log_group_arn, log_types,
    records_in_log_group, s3, slug, BUCKET, LOG_TYPE_EXPERIMENT_END, LOG_TYPE_EXPERIMENT_START,
    ROLE_ARN,
};

/// How long every row waits for the run it started to have ELAPSED on the emulator's
/// compressed run clock: the whole span of a run whose action names no duration
/// (`MIN_RUN_SPAN`) plus the transition into `running` (`TRANSITION_DELAY`), and slack for
/// a loaded machine. Taken ONCE for every row, since each row has its own emulator.
const PAST_THE_RUN: Duration = Duration::from_secs(9);

/// The terminal status a run whose actions ran to the end reaches, which is what the
/// delivered record must report. Spelled out rather than imported, so a changed vocabulary
/// fails this suite.
const COMPLETED: &str = "completed";

/// An `ExperimentTemplateId` this service never minted — well-typed (`^[\S]+$`, within
/// `@length(0..64)`) and naming nothing.
const UNMINTED_TEMPLATE_ID: &str = "EXTTHISTEMPLATEWASNEVERMINTED";

/// Where a row's template sends the records of its run.
#[derive(Debug, Clone, Copy)]
enum Destination {
    /// A CloudWatch Logs log group, named here and addressed by its ARN.
    CloudWatchLogs(&'static str),
    /// [`BUCKET`], under this key prefix.
    S3(&'static str),
}

/// The FIS call a row makes once the run has elapsed — one this service settles the store
/// for and then REFUSES.
#[derive(Debug, Clone, Copy)]
enum Refused {
    /// `StopExperiment` on the experiment whose run has ended: the settle inside it is
    /// what materialises that end.
    StopTheEndedRun,
    /// `StartExperiment` carrying the `clientToken` of the start above on a request that
    /// start never made — refused for the token, after the settle that ends every run
    /// whose span has elapsed.
    StartWithTheTokenOfAnotherRequest,
    /// `StartExperiment` naming a template that is not there — refused for the template,
    /// after the same settle.
    StartFromAnUnmintedTemplate,
}

/// What one row configures, what it then asks for, and how that ask is refused.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    destination: Destination,
    /// The `logSchemaVersion` the configuration asks for, which every delivered record
    /// carries.
    schema_version: i64,
    refused: Refused,
    /// The HTTP status the refusal answers with.
    status: u16,
    /// The wire error code (`__type`) it carries.
    code: &'static str,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a stop of a run that has just ended, logging to CloudWatch Logs",
            destination: Destination::CloudWatchLogs("/aws/fis/refused-stop"),
            schema_version: 2,
            refused: Refused::StopTheEndedRun,
            status: 400,
            code: "ValidationException",
        },
        Row {
            case: "a stop of a run that has just ended, logging to Amazon S3",
            destination: Destination::S3("refused/stop"),
            schema_version: 1,
            refused: Refused::StopTheEndedRun,
            status: 400,
            code: "ValidationException",
        },
        Row {
            case: "a start refused for the token of another request",
            destination: Destination::CloudWatchLogs("/aws/fis/refused-token"),
            schema_version: 2,
            refused: Refused::StartWithTheTokenOfAnotherRequest,
            status: 409,
            code: "ConflictException",
        },
        Row {
            case: "a start refused for a template that is not there",
            destination: Destination::S3("refused/template"),
            schema_version: 1,
            refused: Refused::StartFromAnUnmintedTemplate,
            status: 404,
            code: "ResourceNotFoundException",
        },
    ]
}

/// The `message` an error body carries.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The destination `row` names, created before anything is started: a log group and a
/// bucket are resources FIS writes INTO, not ones it creates.
async fn given_destination(router: &smithy_server_runtime::MultiServiceRouter, row: &Row) {
    match row.destination {
        Destination::CloudWatchLogs(group) => given_log_group(router, group).await,
        Destination::S3(_) => given_bucket(router).await,
    }
}

/// The `logConfiguration` `row`'s template carries.
fn log_configuration(row: &Row) -> Value {
    match row.destination {
        Destination::CloudWatchLogs(group) => json!({
            "cloudWatchLogsConfiguration": { "logGroupArn": log_group_arn(group, true) },
            "logSchemaVersion": row.schema_version
        }),
        Destination::S3(prefix) => json!({
            "s3Configuration": { "bucketName": BUCKET, "prefix": prefix },
            "logSchemaVersion": row.schema_version
        }),
    }
}

/// `row`'s template: one target resolved by ARN, so the run has a resolution to report, and
/// one action naming no duration, so the run is the compressed floor and elapses inside
/// [`PAST_THE_RUN`].
fn template_body(row: &Row) -> Value {
    json!({
        "clientToken": token(&format!("refusal-template-{}", slug(row.case))),
        "description": format!("a refused call delivers what it settled: {}", row.case),
        "roleArn": ROLE_ARN,
        "logConfiguration": log_configuration(row),
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
    })
}

/// The run a row started: what the refusal below is built from.
struct Started {
    id: String,
    template_id: String,
    client_token: String,
}

/// Create `row`'s template and start one experiment from it.
async fn start_run(router: &smithy_server_runtime::MultiServiceRouter, row: &Row) -> Started {
    let created = create_template(router, template_body(row)).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the template to be created, got {} with {:?}",
        row.case, created.status, created.body
    );
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no template id in {:?}", row.case, created.body))
        .to_string();

    let client_token = token(&format!("refusal-start-{}", slug(row.case)));
    let started = start_experiment(
        router,
        json!({
            "clientToken": client_token,
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
        .unwrap_or_else(|| panic!("case {:?}: no experiment id in {:?}", row.case, started.body))
        .to_string();

    Started {
        id,
        template_id,
        client_token,
    }
}

/// Make the call `row` expects to be refused.
async fn refused_call(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &Row,
    started: &Started,
) -> Answer {
    match row.refused {
        Refused::StopTheEndedRun => stop_experiment(router, &started.id).await,
        Refused::StartWithTheTokenOfAnotherRequest => {
            start_experiment(
                router,
                json!({
                    "clientToken": started.client_token,
                    "experimentTemplateId": started.template_id,
                    // The token of the start above on a request that would tag the
                    // experiment differently: not the request that token started.
                    "tags": { "refusal": "by-token" }
                }),
            )
            .await
        }
        Refused::StartFromAnUnmintedTemplate => {
            start_experiment(
                router,
                json!({
                    "clientToken": token(&format!("refusal-unminted-{}", slug(row.case))),
                    "experimentTemplateId": UNMINTED_TEMPLATE_ID
                }),
            )
            .await
        }
    }
}

/// The message the refusal carries. Spelled out here rather than imported from the crate,
/// so changed wording fails this suite.
fn expected_message(row: &Row, started: &Started) -> String {
    match row.refused {
        Refused::StopTheEndedRun => format!(
            "Cannot modify the state of an experiment in final state: {}.",
            started.id
        ),
        Refused::StartWithTheTokenOfAnotherRequest => format!(
            "clientToken {} is not assigned to this resource.",
            started.client_token
        ),
        Refused::StartFromAnUnmintedTemplate => "ExperimentTemplate not found.".to_string(),
    }
}

/// Every record delivered for `id` to the destination `row` configured, parsed.
///
/// Read through the DESTINATION service — CloudWatch Logs or Amazon S3 — which settles no
/// experiment and drains no outbox, so reading it cannot be what delivers a record.
async fn records_at(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &Row,
    id: &str,
) -> Vec<Value> {
    match row.destination {
        Destination::CloudWatchLogs(group) => records_in_log_group(router, group, id).await,
        Destination::S3(prefix) => {
            let mut keys: Vec<String> = keys_under(router, prefix)
                .await
                .into_iter()
                .filter(|key| key.contains(id))
                .collect();
            keys.sort();
            let mut records = Vec::new();
            for key in &keys {
                let (status, body) = s3(router, "GET", &format!("/{BUCKET}/{key}")).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the object {key} must be readable, got {status} with {body}",
                    row.case
                );
                records.push(serde_json::from_str::<Value>(&body).unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: the object {key} must hold one JSON record ({error}), \
                         got {body}",
                        row.case
                    )
                }));
            }
            records
        }
    }
}

#[tokio::test]
async fn a_refused_call_delivers_the_records_its_own_settle_produced() {
    let rows = rows();

    // Every row's run is started first, each on its own emulator, so the wait for the runs
    // to elapse is taken ONCE rather than once per row.
    let mut started = Vec::new();
    for row in &rows {
        let router = emulator();
        given_destination(&router, row).await;
        let run = start_run(&router, row).await;

        // The start's own records are at the destination already, so what the assertions
        // below read is the place this run really writes to — a case cannot pass on a
        // destination nothing ever reaches.
        let delivered = records_at(&router, row, &run.id).await;
        let types = log_types(&delivered);
        assert!(
            types.contains(&LOG_TYPE_EXPERIMENT_START),
            "case {:?}: the configured destination must hold the start record, got {types:?}",
            row.case
        );

        started.push((row, router, run));
    }

    // The runs elapse with NO call to FIS made: nothing has settled the store, so the end
    // each run now owes the clock has been materialised by nobody.
    tokio::time::sleep(PAST_THE_RUN).await;

    for (row, router, run) in &started {
        let parked = records_at(router, row, &run.id).await;
        let types = log_types(&parked);
        assert!(
            !types.contains(&LOG_TYPE_EXPERIMENT_END),
            "case {:?}: no call has settled this run yet, so its terminal record cannot be \
             at the destination, got {types:?}",
            row.case
        );

        let refused = refused_call(router, row, run).await;
        assert_eq!(
            refused.status, row.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            row.case, row.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            row.code,
            "case {:?}: expected {}, got {:?}",
            row.case,
            row.code,
            refused.body
        );
        assert_eq!(
            message(&refused),
            expected_message(row, run),
            "case {:?}: expected the refusal's own wording, got {:?}",
            row.case,
            refused.body
        );

        // The refused call settled the store, and the settle is what materialised the end
        // of this run — so that call is the one that delivers its terminal record.
        let delivered = records_at(router, row, &run.id).await;
        let types = log_types(&delivered);
        let end = delivered
            .iter()
            .find(|record| record["log_type"].as_str() == Some(LOG_TYPE_EXPERIMENT_END))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the call that materialised the run's end must deliver its \
                     terminal record, got {types:?}",
                    row.case
                )
            });
        assert_eq!(
            end["id"].as_str(),
            Some(run.id.as_str()),
            "case {:?}: the terminal record names the experiment it belongs to, got {end:?}",
            row.case
        );
        assert_eq!(
            end["log_schema_version"].as_i64(),
            Some(row.schema_version),
            "case {:?}: the terminal record carries the configured logSchemaVersion, \
             got {end:?}",
            row.case
        );
        assert_eq!(
            end["details"]["status"].as_str(),
            Some(COMPLETED),
            "case {:?}: the terminal record reports the status the run reached, got {end:?}",
            row.case
        );

        // And it reports the very status the experiment itself now carries: the refused
        // call delivered the record of a state it really left behind.
        let read_back = get_experiment(router, &run.id).await;
        assert_eq!(
            read_back.member("experiment")["state"]["status"].as_str(),
            Some(COMPLETED),
            "case {:?}: expected the run to have ended, got {:?}",
            row.case,
            read_back.body
        );
    }
}
