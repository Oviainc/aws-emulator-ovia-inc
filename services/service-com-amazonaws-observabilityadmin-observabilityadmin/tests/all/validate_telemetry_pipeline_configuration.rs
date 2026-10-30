// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ValidateTelemetryPipelineConfiguration`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handler — through the
//! ONE harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. the operation REPORTS the defects of the configuration it is handed: a 200
//!    carrying one `ValidationError` per defect, each with the category it falls in and
//!    a `FieldMap` naming the field of the document it is about — and a 200 carrying no
//!    `Errors` at all for a configuration this emulator would run;
//! 2. a body that is not a document the emulator parses is the
//!    `PIPELINE_CONFIGURATION_PARSE_ERROR` defect and the only one reported for such a
//!    body, since a body that was never read has no fields to report anything else
//!    against;
//! 3. a document that WAS read is checked against the SCHEMA a pipeline definition
//!    follows, one reported error per problem: a required property the document does not
//!    declare (`MISSING_FIELD`), a property the schema does not define (`UNKNOWN_PROPERTY`,
//!    carrying the path it is written at), a property carrying a number of items the
//!    schema does not allow, a POSITION of the processor sequence this emulator cannot
//!    run (its own category, since the document may be one the real service accepts),
//!    and every field naming an ARN carrying one — the IAM role syntax where the field
//!    names the role a plugin assumes;
//! 4. nothing is stored and nothing is created: the call leaves the store exactly as it
//!    found it, so the same call answers the same way twice.

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::ComAmazonawsObservabilityadminObservabilityadminState;

use crate::telemetry_rules::{call, emulator, Emulator};

/// The region every request below signs for.
const HOME_REGION: &str = "us-west-2";

/// `ValidationError$Reason` for a body that could not be parsed as a document at all,
/// for a document that was parsed but declares something a pipeline definition may
/// not, and for a field whose value does not have the form that field takes: the
/// category codes this service answers.
const PARSE_ERROR: &str = "PIPELINE_CONFIGURATION_PARSE_ERROR";
const VALIDATION_ERROR: &str = "PIPELINE_CONFIGURATION_VALIDATION_ERROR";
const INVALID_PATTERN: &str = "INVALID_PATTERN";
/// The category for a processor, or a processor option, this emulator does not run —
/// its own code, because such a document may be one the real service accepts.
const UNSUPPORTED_PROCESSOR: &str = "UNSUPPORTED_PROCESSOR";
/// The two categories a schema validator reports a property problem under: a required
/// property that is not there, and a property the schema does not define.
const MISSING_FIELD: &str = "MISSING_FIELD";
const UNKNOWN_PROPERTY: &str = "UNKNOWN_PROPERTY";

/// One ARN of each form this service's model declares
/// (`smithy_query(query="shape", shape="IamRoleArn,ResourceArn")`): the IAM role a
/// pipeline plugin assumes, and the resource a sink writes to.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/TelemetryPipelineRole";
const BUCKET_ARN: &str = "arn:aws:s3:::telemetry-archive";

/// One `ValidateTelemetryPipelineConfiguration` call.
async fn validate(emulator: &Emulator, configuration: &str) -> (u16, Value) {
    call(
        emulator,
        HOME_REGION,
        "/ValidateTelemetryPipelineConfiguration",
        &json!({ "Configuration": { "Body": configuration } }),
    )
    .await
}

/// The reported defects as `(Reason, the one FieldMap key)` pairs, sorted so a case
/// asserts the SET of defects rather than the order two independent checks ran in.
fn defects(case: &str, response: &Value) -> Vec<(String, String)> {
    let errors = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut reported: Vec<(String, String)> = errors
        .iter()
        .map(|error| {
            let message = error
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !message.is_empty(),
                "case {case:?}: every reported defect says what is wrong, got {error:?}"
            );
            let reason = error
                .get("Reason")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported defect carries a Reason, got {error:?}")
                });
            let field_map = error
                .get("FieldMap")
                .and_then(Value::as_object)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported defect carries a FieldMap, got {error:?}")
                });
            assert_eq!(
                field_map.len(),
                1,
                "case {case:?}: a defect names the ONE field it is about, got {field_map:?}"
            );
            let (field, issue) = field_map.iter().next().expect("one field");
            assert!(
                issue.as_str().is_some_and(|issue| !issue.is_empty()),
                "case {case:?}: the FieldMap entry for {field} carries the issue, got {issue:?}"
            );
            (reason.to_string(), field.clone())
        })
        .collect();
    reported.sort();
    reported
}

/// A pipeline definition written at the top level, with one source, the `processor`
/// text given, and one sink.
fn top_level(processor: &str) -> String {
    format!(
        "source:\n  \
           http: {{}}\n\
         {processor}\
         sink:\n  \
           - cloudwatch_logs:\n      \
               log_group: /aws/telemetry/example\n"
    )
}

/// The same definition under a section name, which is the other form this format
/// writes a pipeline in.
fn named_section(name: &str, processor: &str) -> String {
    format!(
        "{name}:\n  \
           source:\n    \
             http: {{}}\n\
         {processor}  \
           sink:\n    \
             - cloudwatch_logs:\n        \
                 log_group: /aws/telemetry/example\n"
    )
}

// =============================================================================
// A configuration this emulator would run reports no defect
// =============================================================================

/// A configuration that satisfies every rule a pipeline definition follows answers a
/// 200 with NO `Errors`: the output's list is "a list of validation errors found in
/// the pipeline configuration, if any", so its absence is the verdict "valid".
///
/// The rows differ along what the rule generalises over: where the pipeline is written
/// (at the top level or as a named section), how many processors it declares (none,
/// one, several), how its one source and one sink are written (a mapping, a sequence of
/// one), how the document is spelled (YAML or the JSON it is a superset of), and how
/// many sections the chain spans.
#[tokio::test]
async fn a_configuration_this_emulator_would_run_reports_no_defect() {
    let emulator = emulator();

    for (case, configuration) in [
        (
            "a top-level pipeline with one processor",
            top_level("processor:\n  - parse_json: {}\n"),
        ),
        (
            "a top-level pipeline with no processor, which is optional",
            top_level(""),
        ),
        (
            "a top-level pipeline with several processors",
            top_level(
                "processor:\n  \
                   - parse_json: {}\n  \
                   - trim_string:\n      \
                       with_keys: [\"message\"]\n  \
                   - uppercase_string:\n      \
                       with_keys: [\"message\"]\n",
            ),
        ),
        (
            "a named section",
            named_section("log-pipeline", "  processor:\n    - parse_json: {}\n"),
        ),
        (
            "a sink written as a mapping of one plugin rather than a sequence",
            "source:\n  http: {}\nsink:\n  cloudwatch_logs:\n    log_group: /aws/x\n".to_string(),
        ),
        (
            "a document carrying members beside its one section",
            "version: \"2\"\n".to_string()
                + &named_section("log-pipeline", "  processor:\n    - parse_json: {}\n"),
        ),
        (
            "the same pipeline written as JSON, which YAML 1.2 is a superset of",
            json!({
                "log-pipeline": {
                    "source": { "http": {} },
                    "processor": [{ "parse_json": {} }],
                    "sink": [{ "cloudwatch_logs": { "log_group": "/aws/x" } }],
                }
            })
            .to_string(),
        ),
        (
            "a sink naming the role it assumes by its ARN",
            format!(
                "source:\n  http: {{}}\nsink:\n  - cloudwatch_logs:\n      log_group: /aws/x\n      \
                 sts_role_arn: \"{ROLE_ARN}\"\n"
            ),
        ),
        (
            "a source naming its role under the aws block, nested deeper in the document",
            format!(
                "source:\n  s3:\n    aws:\n      sts_role_arn: \"{ROLE_ARN}\"\n      \
                 region: us-west-2\nsink:\n  - cloudwatch_logs:\n      log_group: /aws/x\n"
            ),
        ),
        (
            "a sink naming the resource it writes to by its ARN, which is not a role",
            format!(
                "source:\n  http: {{}}\nsink:\n  - s3:\n      bucket_arn: \"{BUCKET_ARN}\"\n      \
                 sts_role_arn: \"{ROLE_ARN}\"\n"
            ),
        ),
        (
            "the same role field spelled the way JSON writes it",
            format!(
                "{{\"source\": {{\"http\": {{}}}}, \"sink\": [{{\"s3\": \
                 {{\"stsRoleArn\": \"{ROLE_ARN}\"}}}}]}}"
            ),
        ),
        (
            "two sections chained by a pipeline sink and a pipeline source",
            "entry:\n  \
               source:\n    \
                 http: {}\n  \
               processor:\n    \
                 - trim_string:\n        \
                     with_keys: [\"message\"]\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: exit\n\
             exit:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: entry\n  \
               processor:\n    \
                 - uppercase_string:\n        \
                     with_keys: [\"message\"]\n  \
               sink:\n    \
                 - cloudwatch_logs:\n        \
                     log_group: /aws/x\n"
                .to_string(),
        ),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            defects(case, &response),
            Vec::<(String, String)>::new(),
            "case {case:?}: expected no defect for {configuration}, got {response:?}"
        );
    }
}

// =============================================================================
// A body that is not a document the emulator reads
// =============================================================================

/// A body the reader cannot parse is reported as the `PIPELINE_CONFIGURATION_PARSE_ERROR`
/// defect against `Configuration.Body`, and as that ONE defect: nothing was read, so
/// there is no field in the document to report anything else against.
///
/// The rows span the ways a body is unreadable: a syntax error, an unterminated
/// collection, an alias (which expands the document it appears in), and nesting past
/// what the reader reads.
#[tokio::test]
async fn a_body_the_reader_cannot_parse_is_one_parse_error_defect() {
    let emulator = emulator();
    // Balanced flow collections, nested deeper than the reader descends.
    let deeply_nested = format!("source: {}{}", "[".repeat(40), "]".repeat(40));

    for (case, configuration) in [
        (
            "a mapping entry followed by a sequence entry at the same level",
            "source:\n  http: {}\n- broken\n".to_string(),
        ),
        (
            "an unterminated flow sequence",
            "source:\n  http: {}\nsink: [cloudwatch_logs\n".to_string(),
        ),
        (
            "a key with no value and a stray colon",
            "source: : http\n".to_string(),
        ),
        (
            "an alias, which expands the document it appears in",
            "base: &anchor\n  http: {}\nsource: *anchor\nsink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
        ),
        ("nesting past what the reader reads", deeply_nested),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );
        assert_eq!(
            defects(case, &response),
            vec![(PARSE_ERROR.to_string(), "Configuration.Body".to_string())],
            "case {case:?}: expected one parse-error defect, got {response:?}"
        );
    }
}

// =============================================================================
// A document that was read, against the rules a pipeline definition follows
// =============================================================================

/// A document the reader read is checked against the rules a pipeline definition
/// follows, and every defect is reported against the field it is about.
///
/// The rows span the rules: the document declares a pipeline at all, its sections form
/// one chain, each section declares exactly one source and exactly one sink, and its
/// processors are ones this emulator runs with options they take. Each case lists the
/// `(Reason, field)` pairs the answer must carry — so a document with two defects
/// reports two.
///
/// A defect in how the document assembles its pipeline is reported against
/// `Configuration.Body`, the document itself; one in a field the document declares is
/// reported against that field's name, with the message saying where in the document
/// it is.
#[tokio::test]
async fn every_defect_of_a_document_is_reported_against_its_field() {
    let emulator = emulator();

    for (case, configuration, expected) in [
        (
            "a document that is not a mapping",
            "just some text\n".to_string(),
            vec![(MISSING_FIELD, "source"), (MISSING_FIELD, "sink")],
        ),
        (
            "an empty document declaring no pipeline",
            " \n".to_string(),
            vec![(MISSING_FIELD, "source"), (MISSING_FIELD, "sink")],
        ),
        (
            "a mapping declaring no pipeline section",
            "version: \"2\"\ntags:\n  owner: team\n".to_string(),
            vec![
                (UNKNOWN_PROPERTY, "path"),
                (MISSING_FIELD, "source"),
                (MISSING_FIELD, "sink"),
            ],
        ),
        (
            "a top-level pipeline with no source",
            "processor:\n  - parse_json: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            vec![(MISSING_FIELD, "source")],
        ),
        (
            "a top-level pipeline with no sink",
            "source:\n  http: {}\nprocessor:\n  - parse_json: {}\n".to_string(),
            vec![(MISSING_FIELD, "sink")],
        ),
        (
            "a named section with neither a source nor a sink",
            "log-pipeline:\n  processor:\n    - parse_json: {}\n".to_string(),
            vec![(MISSING_FIELD, "sink"), (MISSING_FIELD, "source")],
        ),
        (
            "a section naming two sources",
            "source:\n  http: {}\n  s3: {}\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            vec![(VALIDATION_ERROR, "source")],
        ),
        (
            "a section naming two sinks",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n  - s3: {}\n".to_string(),
            vec![(VALIDATION_ERROR, "sink")],
        ),
        (
            "a source that names no plugin at all",
            "source:\nsink:\n  - cloudwatch_logs: {}\n".to_string(),
            vec![(VALIDATION_ERROR, "source")],
        ),
        (
            "a processor this emulator does not run",
            top_level("processor:\n  - grok:\n      match: \"%{COMMONAPACHELOG}\"\n"),
            vec![(UNSUPPORTED_PROCESSOR, "path")],
        ),
        (
            "an option the processor does not take",
            top_level("processor:\n  - csv:\n      delete_source: true\n"),
            vec![(UNSUPPORTED_PROCESSOR, "path")],
        ),
        (
            "an entry naming two processors at once",
            top_level("processor:\n  - csv: {}\n    trim_string: {}\n"),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a processor section that is not a sequence",
            "source:\n  http: {}\nprocessor:\n  csv: {}\nsink:\n  - cloudwatch_logs: {}\n"
                .to_string(),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a pipeline sink naming a section the document does not declare",
            "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        name: missing\n"
                .to_string(),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "sections handing their records round a cycle",
            "a:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: b\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: b\n\
             b:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: a\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: a\n"
                .to_string(),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a section no sink of the entry section hands its records to",
            named_section("entry", "") + &named_section("orphan", ""),
            vec![(VALIDATION_ERROR, "Configuration.Body")],
        ),
        (
            "a section with a processor defect AND no sink",
            "source:\n  http: {}\nprocessor:\n  - grok: {}\n".to_string(),
            vec![(UNSUPPORTED_PROCESSOR, "path"), (MISSING_FIELD, "sink")],
        ),
        (
            "a role ARN that is not an ARN at all",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      \
               sts_role_arn: TelemetryPipelineRole\n"
                .to_string(),
            vec![(INVALID_PATTERN, "sts_role_arn")],
        ),
        (
            "a role ARN whose account id is not twelve digits",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      \
               sts_role_arn: \"arn:aws:iam::12345:role/Example\"\n"
                .to_string(),
            vec![(INVALID_PATTERN, "sts_role_arn")],
        ),
        (
            "a role ARN field carrying no value",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      sts_role_arn:\n".to_string(),
            vec![(INVALID_PATTERN, "sts_role_arn")],
        ),
        (
            "a role ARN field carrying a mapping rather than an ARN",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs:\n      sts_role_arn:\n        \
               name: Example\n"
                .to_string(),
            vec![(INVALID_PATTERN, "sts_role_arn")],
        ),
        (
            "an empty role ARN under the source's aws block",
            "source:\n  s3:\n    aws:\n      sts_role_arn: \"\"\nsink:\n  \
               - cloudwatch_logs: {}\n"
                .to_string(),
            vec![(INVALID_PATTERN, "sts_role_arn")],
        ),
        (
            "a bucket_arn, a field the same rule covers, that is not an ARN",
            format!(
                "source:\n  http: {{}}\nsink:\n  - s3:\n      bucket_arn: telemetry-archive\n      \
                 sts_role_arn: \"{ROLE_ARN}\"\n"
            ),
            vec![(INVALID_PATTERN, "bucket_arn")],
        ),
        (
            "a role ARN that is a well-formed ARN but not an IAM role's",
            format!(
                "source:\n  http: {{}}\nsink:\n  - s3:\n      sts_role_arn: \"{BUCKET_ARN}\"\n"
            ),
            vec![(INVALID_PATTERN, "sts_role_arn")],
        ),
        (
            "a role field spelled the way JSON writes it, carrying no ARN",
            "{\"source\": {\"http\": {}}, \"sink\": [{\"s3\": {\"stsRoleArn\": \"pipeline-role\"}}]}"
                .to_string(),
            vec![(INVALID_PATTERN, "stsRoleArn")],
        ),
        (
            "an ARN defect AND a structural one, each reported in its own right",
            "source:\n  http: {}\n  s3: {}\nsink:\n  - cloudwatch_logs:\n      \
               sts_role_arn: not-an-arn\n"
                .to_string(),
            vec![(INVALID_PATTERN, "sts_role_arn"), (VALIDATION_ERROR, "source")],
        ),
    ] {
        let (status, response) = validate(&emulator, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a defect is reported, not refused; got {status} {response:?}"
        );
        let mut expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(reason, field): (&str, &str)| (reason.to_string(), field.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            defects(case, &response),
            expected,
            "case {case:?}: expected those defects for {configuration}, got {response:?}"
        );
    }
}

// =============================================================================
// What a reported defect says
// =============================================================================

/// A defect says enough to fix the document: the pattern defect names the value that
/// broke the rule, where in the document that value is, and the constraint it did not
/// satisfy — and it takes all three from the request, so a second document with a
/// different field, path and value is described by its own three.
#[tokio::test]
async fn a_pattern_defect_names_the_value_its_path_and_the_constraint() {
    let emulator = emulator();

    for (case, configuration, path, value) in [
        (
            "a role ARN in the source's aws block",
            "log-pipeline:\n  source:\n    s3:\n      aws:\n        \
               sts_role_arn: pipeline-role\n  sink:\n    - cloudwatch_logs: {}\n",
            "log-pipeline.source.s3.aws.sts_role_arn",
            "pipeline-role",
        ),
        (
            "a bucket ARN in the second sink entry of a chained section",
            "entry:\n  source:\n    http: {}\n  sink:\n    - pipeline:\n        name: exit\n\
             exit:\n  source:\n    pipeline:\n      name: entry\n  sink:\n    - s3:\n        \
               bucket_arn: telemetry-archive\n",
            "exit.sink[0].s3.bucket_arn",
            "telemetry-archive",
        ),
    ] {
        let (status, response) = validate(&emulator, configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let errors = response
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let message = errors
            .iter()
            .filter_map(|error| error.get("Message").and_then(Value::as_str))
            .find(|message| message.contains(path))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a defect naming {path}, got {response:?}")
            });
        assert!(
            message.contains(value),
            "case {case:?}: expected the message to carry the value {value}, got {message}"
        );
        assert!(
            message.contains("pattern") && message.contains("^arn:"),
            "case {case:?}: expected the message to carry the constraint, got {message}"
        );
    }
}

// =============================================================================
// Nothing is created and nothing is stored
// =============================================================================

/// The call validates a configuration; it does not deploy one. Nothing it is handed
/// becomes a stored pipeline, so a `GetTelemetryPipeline` for the section the document
/// named is still a 404, the store still holds nothing, and a second identical call
/// answers exactly what the first one did.
#[tokio::test]
async fn validating_a_configuration_creates_nothing() {
    let emulator = emulator();
    let configuration = named_section("log-pipeline", "  processor:\n    - parse_json: {}\n");

    let (status, first) = validate(&emulator, &configuration).await;
    assert_eq!(status, 200, "expected 200, got {status} {first:?}");
    assert!(
        first.get("Errors").is_none(),
        "expected no defect, got {first:?}"
    );

    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": "log-pipeline" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a validated configuration is not a stored pipeline, got {status} {read:?}"
    );

    let stored = {
        let state = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
        let store = state.lock();
        store.pipeline_count(HOME_REGION)
    };
    assert_eq!(
        stored, 0,
        "the store holds no pipeline after a validate call, got {stored}"
    );

    let (status, second) = validate(&emulator, &configuration).await;
    assert_eq!(status, 200, "expected 200, got {status} {second:?}");
    assert_eq!(
        second, first,
        "a validate reads nothing it could have changed, got {second:?} after {first:?}"
    );
}

/// A configuration this operation reports no defect in is one `CreateTelemetryPipeline`
/// accepts, and the pipeline it then stores carries that very body back out of
/// `GetTelemetryPipeline`: the two operations read the same document, so a clean
/// verdict here is not a verdict about some other document.
#[tokio::test]
async fn a_configuration_reported_clean_is_one_a_create_accepts() {
    let emulator = emulator();
    let configuration = top_level("processor:\n  - parse_json: {}\n");

    let (status, verdict) = validate(&emulator, &configuration).await;
    assert_eq!(status, 200, "expected 200, got {status} {verdict:?}");
    assert!(
        verdict.get("Errors").is_none(),
        "expected no defect, got {verdict:?}"
    );

    let (status, created) = call(
        &emulator,
        HOME_REGION,
        "/CreateTelemetryPipeline",
        &json!({
            "Name": "validated-pipeline",
            "Configuration": { "Body": configuration },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a configuration reported clean is accepted, got {status} {created:?}"
    );

    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": "validated-pipeline" }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {read:?}");
    assert_eq!(
        read.pointer("/Pipeline/Configuration/Body")
            .and_then(Value::as_str),
        Some(configuration.as_str()),
        "the stored pipeline carries the validated document, got {read:?}"
    );
}
