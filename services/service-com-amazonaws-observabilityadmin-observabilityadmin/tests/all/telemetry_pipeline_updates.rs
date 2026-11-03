// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateTelemetryPipeline` and for WHERE a configuration
//! defect is located in the report that carries it.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through the
//! ONE harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. an update replaces the stored configuration of the pipeline its identifier names,
//!    unless the new configuration declares a source type the stored one does not: this
//!    operation's own documentation says the source type "cannot be changed to a
//!    different source type" after creation, while "[p]rocessors can be added, removed,
//!    or modified". A refused update answers 400 `ValidationException` with the fixed
//!    sentence and leaves the stored configuration exactly as it was; an accepted one
//!    stores the new body verbatim;
//! 2. an identifier no stored pipeline answers is the modelled 404, whichever of the
//!    two forms `PipelineIdentifier` admits it took, and it stores nothing;
//! 3. the pipeline reports the transition an accepted update owes — a non-zero span —
//!    until it comes due, and `ACTIVE` from then on;
//! 4. a `ValidationError` PINNED TO A FIELD of a configuration document locates that
//!    field under the report's own `path` key, whose value is the field's full dotted
//!    document path; the field's own bare name is never a `FieldMap` key, because it
//!    cannot tell two fields of that name apart.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    now_epoch_millis, pipeline_arn, ComAmazonawsObservabilityadminObservabilityadminState,
    PIPELINE_ACTIVATION_DELAY_MS,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator};

/// The region every request below signs for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so an ARN that names a pipeline outside the caller's own endpoint
/// can be shown to resolve to nothing.
const OTHER_REGION: &str = "eu-west-1";

/// Two IAM role ARNs of the form this service's model declares for one
/// (`smithy_query(query="shape",shape="IamRoleArn")`), so a source's own options can be
/// changed from one well-formed value to another.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/TelemetryPipelineRole";
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/TelemetryPipelineRoleTwo";

/// Long enough after an accepted update for the transition it started to have come due,
/// whatever scheduling delay the test run adds on top.
fn past_activation_delay() -> Duration {
    Duration::from_millis(PIPELINE_ACTIVATION_DELAY_MS as u64 + 250)
}

async fn create(emulator: &Emulator, region: &str, name: &str, body: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/CreateTelemetryPipeline",
        &json!({ "Name": name, "Configuration": { "Body": body } }),
    )
    .await
}

async fn update(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    body: &str,
) -> (u16, Value) {
    call(
        emulator,
        region,
        "/UpdateTelemetryPipeline",
        &json!({
            "PipelineIdentifier": identifier,
            "Configuration": { "Body": body }
        }),
    )
    .await
}

async fn get(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": identifier }),
    )
    .await
}

/// A `TelemetryPipeline` member of a `GetTelemetryPipeline` response.
fn member<'a>(response: &'a Value, name: &str) -> Option<&'a Value> {
    response.get("Pipeline").and_then(|pipeline| {
        pipeline
            .get(name)
            .filter(|value| !matches!(value, Value::Null))
    })
}

/// The configuration body a read reports for the pipeline.
fn reported_body(response: &Value) -> String {
    member(response, "Configuration")
        .and_then(|configuration| configuration.get("Body"))
        .and_then(Value::as_str)
        .unwrap_or("<no Body>")
        .to_string()
}

/// Every reported `ValidationError` of a refusal as `(Reason, the one FieldMap key, its
/// value)` triples, sorted so a case asserts the SET of entries rather than the order
/// two independent checks ran in.
fn reported_errors(case: &str, response: &Value) -> Vec<(String, String, String)> {
    let mut reported: Vec<(String, String, String)> = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|error| {
            let field_map = error
                .get("FieldMap")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported error carries a FieldMap, got {error:?}")
                });
            assert_eq!(
                field_map.len(),
                1,
                "case {case:?}: an entry locates the ONE field it is about, got {field_map:?}"
            );
            let (key, value) = field_map.iter().next().expect("one entry");
            let message = error
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.is_empty(),
                "case {case:?}: every reported error says what is wrong, got {error:?}"
            );
            (
                error
                    .get("Reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// A pipeline definition under the property the schema requires of a document
/// (`pipeline`), with `source` the given source block and `sink` one CloudWatch Logs
/// sink, plus whatever `extra` the case writes into the section.
fn document(source: &str, extra: &str) -> String {
    format!(
        "pipeline:\n  \
           source:\n{source}  \
           {extra}\
           sink:\n    \
             - cloudwatch_logs:\n        \
                 log_group: /aws/telemetry/example\n"
    )
}

/// One row of the update table: the configuration the pipeline is created with, the one
/// the update submits, and the dotted document paths the refusal must locate — empty
/// when the update is one the source-type rule admits.
struct UpdateCase {
    case: &'static str,
    created: String,
    updated: String,
    refused_at: Vec<&'static str>,
}

/// `ValidationError$Reason` the emulator reports a document declaring something a
/// pipeline definition may not under.
const VALIDATION_ERROR: &str = "PIPELINE_CONFIGURATION_VALIDATION_ERROR";

/// `ValidationError$Reason` for a field whose value does not have the form that field
/// takes.
const INVALID_PATTERN: &str = "INVALID_PATTERN";

/// The `FieldMap` key a field-pinned report locates the field under.
const PATH_KEY: &str = "path";

/// An update keeps everything but the pipeline's SOURCE TYPE open: processors may be
/// added, removed or modified and the source's own options may change, while a
/// configuration declaring a source type the pipeline was not created with is refused
/// and changes nothing.
///
/// The rows differ along what the rule generalises over: WHAT the new configuration
/// changes (a processor added, removed, its options modified, the source's own options,
/// the sink's), how the document is spelled (YAML, and the JSON it is a superset of),
/// where the document writes its pipeline (under the schema's own property, at the top
/// level, under another name, as two chained sections), and which source type replaces
/// which.
#[tokio::test]
async fn an_update_may_change_everything_but_the_pipelines_source_type() {
    let emulator = emulator();

    let cloudwatch_source = "    cloudwatch_logs:\n      log_group: /aws/lambda/example\n";
    let s3_source = "    s3:\n      bucket: example-telemetry\n";

    let rows: Vec<UpdateCase> = vec![
        UpdateCase {
            case: "a processor added at the same source type",
            created: document(cloudwatch_source, ""),
            updated: document(cloudwatch_source, "processor:\n    - parse_json: {}\n  "),
            refused_at: vec![],
        },
        UpdateCase {
            case: "a processor removed at the same source type",
            created: document(
                cloudwatch_source,
                "processor:\n    - parse_json: {}\n    - trim_string:\n        \
                 with_keys: [\"message\"]\n  ",
            ),
            updated: document(cloudwatch_source, "processor:\n    - parse_json: {}\n  "),
            refused_at: vec![],
        },
        UpdateCase {
            case: "a processor's options modified at the same source type",
            created: document(
                cloudwatch_source,
                "processor:\n    - trim_string:\n        with_keys: [\"message\"]\n  ",
            ),
            updated: document(
                cloudwatch_source,
                "processor:\n    - trim_string:\n        with_keys: [\"body\"]\n  ",
            ),
            refused_at: vec![],
        },
        UpdateCase {
            case: "the source's own role ARN modified, which the source type is not",
            created: document(
                &format!("    cloudwatch_logs:\n      aws:\n        sts_role_arn: \"{ROLE_ARN}\"\n"),
                "",
            ),
            updated: document(
                &format!(
                    "    cloudwatch_logs:\n      aws:\n        \
                     sts_role_arn: \"{OTHER_ROLE_ARN}\"\n"
                ),
                "",
            ),
            refused_at: vec![],
        },
        UpdateCase {
            case: "the sink's log group modified at the same source type",
            created: document(cloudwatch_source, ""),
            updated: format!(
                "pipeline:\n  source:\n{cloudwatch_source}  sink:\n    - cloudwatch_logs:\n        \
                 log_group: /aws/telemetry/other\n"
            ),
            refused_at: vec![],
        },
        UpdateCase {
            case: "the source type changed from cloudwatch_logs to s3",
            created: document(cloudwatch_source, ""),
            updated: document(s3_source, ""),
            refused_at: vec!["pipeline.source.s3"],
        },
        UpdateCase {
            case: "the source type changed at the top level, where the document writes \
                   its pipeline directly",
            created: "source:\n  s3:\n    bucket: example-telemetry\nsink:\n  \
                      - cloudwatch_logs: {}\n"
                .to_string(),
            updated: "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            refused_at: vec!["source.http"],
        },
        UpdateCase {
            case: "the source type changed in a section declared under another name",
            created: "log-pipeline:\n  source:\n    http: {}\n  sink:\n    \
                      - cloudwatch_logs: {}\n"
                .to_string(),
            updated: "log-pipeline:\n  source:\n    s3:\n      bucket: example-telemetry\n  \
                      sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            refused_at: vec!["log-pipeline.source.s3"],
        },
        UpdateCase {
            case: "one of two chained sections changes its source type",
            created: "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        \
                      name: exit\nexit:\n  source:\n    pipeline:\n      name: entry\n  \
                      sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            updated: "entry:\n  source:\n    s3:\n      bucket: example-telemetry\n  sink:\n    \
                      - pipeline:\n        name: exit\nexit:\n  source:\n    pipeline:\n      \
                      name: entry\n  sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            refused_at: vec!["entry.source.s3"],
        },
        UpdateCase {
            case: "both of two chained sections change their source type",
            created: "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        \
                      name: exit\nexit:\n  source:\n    pipeline:\n      name: entry\n  \
                      sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            updated: "entry:\n  source:\n    s3:\n      bucket: example-telemetry\n  sink:\n    \
                      - pipeline:\n        name: exit\nexit:\n  source:\n    \
                      otel_logs: {}\n  sink:\n    - cloudwatch_logs: {}\n"
                .to_string(),
            refused_at: vec!["entry.source.s3", "exit.source.otel_logs"],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let name = format!("pip-update-{index}");
        let (status, created) = create(&emulator, HOME_REGION, &name, &row.created).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {created:?}"
        );

        let (status, before) = get(&emulator, HOME_REGION, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} {before:?}"
        );
        let created_at = member(&before, "CreatedTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();

        let (status, answered) = update(&emulator, HOME_REGION, &name, &row.updated).await;
        let (status_after, after) = get(&emulator, HOME_REGION, &name).await;
        assert_eq!(
            status_after, 200,
            "case {case:?}: expected the re-read to answer 200, got {status_after} {after:?}"
        );

        if row.refused_at.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to answer 200, got {status} {answered:?}"
            );
            assert_eq!(
                reported_body(&after),
                row.updated,
                "case {case:?}: the update's configuration is what a read reports: {after:?}"
            );
            let updated_at = member(&after, "LastUpdateTimeStamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            assert!(
                updated_at >= created_at,
                "case {case:?}: an update does not move LastUpdateTimeStamp backwards: \
                 created {created_at}, updated {updated_at}"
            );
            assert_eq!(
                member(&after, "CreatedTimeStamp").and_then(Value::as_i64),
                Some(created_at),
                "case {case:?}: an update leaves CreatedTimeStamp alone, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        assert_eq!(
            answered.get("Message").and_then(Value::as_str),
            Some("Validation errors found"),
            "case {case:?}: the refusal carries the service's fixed sentence: {answered:?}"
        );
        let mut expected: Vec<(String, String, String)> = row
            .refused_at
            .iter()
            .map(|path| {
                (
                    VALIDATION_ERROR.to_string(),
                    PATH_KEY.to_string(),
                    (*path).to_string(),
                )
            })
            .collect();
        expected.sort();
        assert_eq!(
            reported_errors(case, &answered),
            expected,
            "case {case:?}: expected the refusal to locate those declarations, got {answered:?}"
        );
        assert_eq!(
            reported_body(&after),
            row.created,
            "case {case:?}: a refused update leaves the stored configuration alone: {after:?}"
        );
        assert_eq!(
            member(&after, "LastUpdateTimeStamp").and_then(Value::as_i64),
            Some(created_at),
            "case {case:?}: a refused update changes nothing, so LastUpdateTimeStamp is \
             still the create instant: {after:?}"
        );
    }
}

/// An update of a pipeline nothing stored is the modelled 404, whichever of the two
/// forms `PipelineIdentifier` admits the request took, and it stores nothing.
#[tokio::test]
async fn an_update_of_a_pipeline_no_record_answers_is_a_modelled_404() {
    let emulator = emulator();

    // One pipeline lives in ANOTHER region, so its own ARN is an identifier that names
    // a real pipeline outside the endpoint the update below is signed for.
    let (status, created) = create(
        &emulator,
        OTHER_REGION,
        "elsewhere",
        "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n",
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let elsewhere_arn = created
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    for (case, identifier) in [
        ("a name nothing was ever created under", "never-created".to_string()),
        (
            "a well-formed ARN naming no pipeline",
            pipeline_arn(HOME_REGION, "0123456789abcdef0123456789abcdef"),
        ),
        ("the ARN of a pipeline in another region", elsewhere_arn),
    ] {
        let (status, response) = update(
            &emulator,
            HOME_REGION,
            &identifier,
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n",
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
        assert_eq!(
            response.get("Message").and_then(Value::as_str),
            Some("Pipeline not found"),
            "case {case:?}: the refusal carries the service's fixed sentence: {response:?}"
        );
        assert_eq!(
            response.get("ResourceId").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {case:?}: the refusal reports the identifier the request carried: \
             {response:?}"
        );
        assert_eq!(
            response.get("ResourceType").and_then(Value::as_str),
            Some("Pipeline"),
            "case {case:?}: the refusal names the resource type: {response:?}"
        );

        let stored = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>()
            .lock()
            .pipeline_count(HOME_REGION);
        assert_eq!(
            stored, 0,
            "case {case:?}: a refused update creates nothing, got {stored} records"
        );
    }
}

/// An accepted update starts a transition the clock owes: the pipeline reports the
/// transient status for a NON-ZERO span and the terminal one from then on, and the
/// configuration the update stored is what a read reports throughout.
#[tokio::test]
async fn an_accepted_update_reports_its_transition_until_it_comes_due() {
    let emulator = emulator();
    let created_body = "source:\n  cloudwatch_logs:\n    log_group: /aws/lambda/example\nsink:\n  \
                        - cloudwatch_logs: {}\n";
    let updated_body = "source:\n  cloudwatch_logs:\n    log_group: /aws/lambda/example\n\
                        processor:\n  - parse_json: {}\nsink:\n  - cloudwatch_logs: {}\n";

    let (status, created) = create(&emulator, HOME_REGION, "update-span", created_body).await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    tokio::time::sleep(past_activation_delay()).await;

    let (status, answered) = update(&emulator, HOME_REGION, "update-span", updated_body).await;
    assert_eq!(status, 200, "expected 200, got {status} {answered:?}");

    let (status, updating) = get(&emulator, HOME_REGION, "update-span").await;
    assert_eq!(status, 200, "expected 200, got {status} {updating:?}");
    assert_eq!(
        reported_body(&updating),
        updated_body,
        "the accepted body is stored whatever status the pipeline is in: {updating:?}"
    );

    // The span the update started is NON-ZERO and is what the status is derived from:
    // the record's own due instant is read here, so the assertion does not depend on
    // how long this test took to issue its read.
    let (updated_at_ms, active_at_ms) = {
        let state = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
        let store = state.lock();
        let record = store
            .pipeline(HOME_REGION, "update-span")
            .expect("the create stored a pipeline record");
        (record.last_update_timestamp_ms, record.active_at_ms)
    };
    assert!(
        active_at_ms > updated_at_ms,
        "the transition an update owes must be non-zero: updated {updated_at_ms}, \
         due {active_at_ms}"
    );
    let status_while_updating = member(&updating, "Status")
        .and_then(Value::as_str)
        .unwrap_or("<no Status>")
        .to_string();
    if now_epoch_millis() < active_at_ms {
        assert_eq!(
            status_while_updating, "UPDATING",
            "a read inside the span reports the transition in flight, got {updating:?}"
        );
        assert!(
            member(&updating, "StatusReason")
                .and_then(|reason| reason.get("Description"))
                .and_then(Value::as_str)
                .is_some_and(|reason| !reason.is_empty()),
            "an UPDATING pipeline reports why, got {updating:?}"
        );
    }

    tokio::time::sleep(past_activation_delay()).await;
    let (status, active) = get(&emulator, HOME_REGION, "update-span").await;
    assert_eq!(status, 200, "expected 200, got {status} {active:?}");
    assert_eq!(
        member(&active, "Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "once the transition comes due the pipeline is ACTIVE, got {active:?}"
    );
    assert_eq!(
        reported_body(&active),
        updated_body,
        "the clock moved the status; it did not change the configuration: {active:?}"
    );
}

/// One row of the located-defect table: the document, and the dotted document path of
/// each field-pinned defect it has.
struct LocatedDefect {
    case: &'static str,
    body: &'static str,
    /// The `(path, the bare field name that is NOT a key)` pair per defect.
    expected: Vec<(&'static str, &'static str)>,
}

/// A defect pinned to a FIELD of the document is located by the report's own `path`
/// key, carrying the field's full dotted document path — and the field's bare name is
/// never a key, so a client following the report's contract can always find the
/// location, and two fields of one name are told apart.
///
/// The rows differ along what the rule generalises over: WHICH field names an ARN
/// (`sts_role_arn`, `bucket_arn`, and the camelCase spelling a JSON document writes),
/// where in the document it sits (under the schema's own `pipeline` property, in a
/// section declared under another name, in a sink entry of a chained section, at the top
/// level), how deeply it is nested, and how many fields of ONE name a document carries.
#[tokio::test]
async fn a_field_pinned_defect_is_located_by_its_document_path() {
    let emulator = emulator();

    let rows: Vec<LocatedDefect> = vec![
        LocatedDefect {
            case: "a role ARN in the source's aws block of the document's pipeline",
            body: "pipeline:\n  source:\n    cloudwatch_logs:\n      aws:\n        \
                   sts_role_arn: not-a-role-arn\n  sink:\n    - cloudwatch_logs: {}\n",
            expected: vec![(
                "pipeline.source.cloudwatch_logs.aws.sts_role_arn",
                "sts_role_arn",
            )],
        },
        LocatedDefect {
            case: "a role ARN in a section declared under another name",
            body: "log-pipeline:\n  source:\n    s3:\n      aws:\n        \
                   sts_role_arn: pipeline-role\n  sink:\n    - cloudwatch_logs: {}\n",
            expected: vec![(
                "log-pipeline.source.s3.aws.sts_role_arn",
                "sts_role_arn",
            )],
        },
        LocatedDefect {
            case: "a bucket ARN in a sink entry of a chained section",
            body: "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        \
                   name: exit\nexit:\n  source:\n    pipeline:\n      name: entry\n  \
                   sink:\n    - s3:\n        bucket_arn: telemetry-archive\n",
            expected: vec![("exit.sink[0].s3.bucket_arn", "bucket_arn")],
        },
        LocatedDefect {
            case: "a role ARN written at the top level, where the document writes its \
                   pipeline directly",
            body: "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      \
                   sts_role_arn: TelemetryPipelineRole\n",
            expected: vec![("sink[0].cloudwatch_logs.sts_role_arn", "sts_role_arn")],
        },
        LocatedDefect {
            case: "the camelCase spelling a JSON document writes the role field in",
            body: "{\"source\": {\"http\": {}}, \"sink\": [{\"s3\": {\"stsRoleArn\": \
                   \"pipeline-role\"}}]}",
            expected: vec![("sink[0].s3.stsRoleArn", "stsRoleArn")],
        },
        LocatedDefect {
            case: "two fields of ONE name, in two sections, each located in its own right",
            body: "first:\n  source:\n    s3:\n      aws:\n        \
                   sts_role_arn: first-role\n  sink:\n    - pipeline:\n        \
                   name: second\nsecond:\n  source:\n    pipeline:\n      name: first\n  \
                   sink:\n    - cloudwatch_logs:\n        sts_role_arn: second-role\n",
            expected: vec![
                ("first.source.s3.aws.sts_role_arn", "sts_role_arn"),
                ("second.sink[0].cloudwatch_logs.sts_role_arn", "sts_role_arn"),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ValidateTelemetryPipelineConfiguration",
            &json!({ "Configuration": { "Body": row.body } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );

        let reported = reported_errors(case, &response);
        let mut expected: Vec<(String, String, String)> = row
            .expected
            .iter()
            .map(|(path, _)| {
                (
                    INVALID_PATTERN.to_string(),
                    PATH_KEY.to_string(),
                    (*path).to_string(),
                )
            })
            .collect();
        expected.sort();
        let pattern_defects: Vec<(String, String, String)> = reported
            .iter()
            .filter(|(reason, _, _)| reason == INVALID_PATTERN)
            .cloned()
            .collect();
        assert_eq!(
            pattern_defects, expected,
            "case {case:?}: expected those located defects for {:?}, got {response:?}",
            row.body
        );
        // The bare field name says nothing about WHICH field of that name the defect is
        // about, so it is never a key of the report.
        for (_, bare_name) in &row.expected {
            assert!(
                !reported.iter().any(|(_, key, _)| key == bare_name),
                "case {case:?}: {bare_name:?} is not a FieldMap key, got {response:?}"
            );
        }
    }
}
