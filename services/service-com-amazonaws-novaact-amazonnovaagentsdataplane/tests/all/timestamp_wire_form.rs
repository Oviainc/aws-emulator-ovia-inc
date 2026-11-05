// SPDX-License-Identifier: CC0-1.0

//! The wire FORM of this service's timestamps.
//!
//! Every timestamp member of this model targets `com.amazonaws.novaact#DateTimestamp`,
//! which carries `@timestampFormat("date-time")`
//! (`smithy_query(query="shape", shape="DateTimestamp")`), so on the restJson1 wire each
//! one is an RFC 3339 / ISO-8601 STRING — never the protocol's default epoch-seconds
//! NUMBER. These cases assert that rule over a SET of inputs rather than one recorded
//! response: the rows vary name form and length, which optional members are present,
//! whether the optional instant is there at all, and the instant itself, because the rule
//! is about how a `date-time` member is ENCODED, not about any one value.
//!
//! The last case reaches the member schemas directly, which is the only reach a summary
//! shape whose list operation is still a scaffolded 501 has.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::state::{
    service_state, ComAmazonawsNovaactAmazonnovaagentsdataplaneState, WorkflowRunStatus,
};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::ops::types::{
    ActSummary, GetWorkflowDefinitionOutput, GetWorkflowRunOutput, WorkflowDefinitionSummary,
    WorkflowRunSummary,
};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator, reached the way the router reaches it, plus the engine handle so a case
/// can put the store into a state no implemented operation reaches yet.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these cases are about; the credential scope below is
        // still read for the routing key and the ARN region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20240101/us-east-1/nova-act/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef",
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// The store this service's handlers write to.
    fn store(&self) -> engine::ServiceState<ComAmazonawsNovaactAmazonnovaagentsdataplaneState> {
        service_state(&*self.engine).expect("the nova-act store is reachable from the engine")
    }
}

/// The member as the response carries it, refused when it is anything but a JSON string: a
/// `date-time` member that arrives as a number is exactly the defect these cases pin.
fn date_time_text<'a>(body: &'a Value, member: &str, case: &str) -> &'a str {
    let value = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry {member:?}: {body}"));
    value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member:?} must be an RFC 3339 date-time STRING, got {value} \
             (a JSON {})",
            json_kind(value)
        )
    })
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Parse the member the way a generated SDK deserializer for a `date-time` member does,
/// and check it reports the instant the store holds.
fn assert_date_time(text: &str, expected: SystemTime, case: &str, member: &str) {
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member:?} must parse as RFC 3339, got {text:?}: {error}")
    });
    // RFC 3339 in UTC: a date, the `T` separator and the `Z` zone. `Format::DateTime` is
    // what renders it, so this is a shape check on the rendering, not a re-implementation.
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case:?}: {member:?} must be an ISO-8601 instant in UTC, got {text:?}"
    );
    let stored = DateTime::from(expected);
    let drift = (parsed.as_secs_f64() - stored.as_secs_f64()).abs();
    assert!(
        drift < 1e-3,
        "case {case:?}: {member:?} must report the stored instant {stored:?}, got {parsed:?} \
         (drift {drift} s)"
    );
}

/// `GetWorkflowDefinition.createdAt` is required, so every definition answers one — as a
/// string, whatever the definition's name form or which optional members it carries.
#[tokio::test]
async fn get_workflow_definition_renders_created_at_as_a_date_time_string() {
    /// Rows vary what the rule generalises over: the name's length and character classes
    /// (the model constrains `WorkflowDefinitionName` to `^[a-zA-Z0-9_-]{1,40}$`, len
    /// 1..40 — `smithy_query(query="operation", operation="CreateWorkflowDefinition")`),
    /// and which optional members sit beside the timestamp in the same response.
    struct Case {
        label: &'static str,
        name: &'static str,
        description: Option<&'static str>,
        export_config: Option<Value>,
    }

    let cases = vec![
        Case {
            label: "shortest legal name, no optional members",
            name: "a",
            description: None,
            export_config: None,
        },
        Case {
            label: "longest legal name (40), description only",
            name: "w0123456789012345678901234567890123456789"
                .split_at(40)
                .0,
            description: Some("a definition described in one line"),
            export_config: None,
        },
        Case {
            label: "underscores and hyphens, export config only",
            name: "my_workflow-definition_2",
            description: None,
            export_config: Some(json!({ "s3BucketName": "nova-act-exports" })),
        },
        Case {
            label: "digits only, every optional member present",
            name: "20261023",
            description: Some("export plus description"),
            export_config: Some(json!({
                "s3BucketName": "nova-act-exports",
                "s3KeyPrefix": "runs/2026",
            })),
        },
    ];

    for case in cases {
        let label = case.label;
        let emulator = Emulator::new();

        let mut body = json!({ "name": case.name });
        if let Some(description) = case.description {
            body["description"] = json!(description);
        }
        if let Some(export_config) = case.export_config.clone() {
            body["exportConfig"] = export_config;
        }
        let (status, created) = emulator.call("PUT", "/workflow-definitions", Some(body)).await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateWorkflowDefinition must answer 201, got {status} {created}"
        );

        let stored_created_at = emulator
            .store()
            .lock()
            .workflow_definitions
            .get(case.name)
            .map(|definition| definition.created_at)
            .unwrap_or_else(|| panic!("case {label:?}: the definition must be stored"));

        let path = format!("/workflow-definitions/{}", case.name);
        let (status, read) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetWorkflowDefinition must answer 200, got {status} {read}"
        );

        let text = date_time_text(&read, "createdAt", label);
        assert_date_time(text, stored_created_at, label, "createdAt");

        // The whole response still carries the members beside the timestamp, so the
        // hand-written schema covers the shape and not just the one member.
        assert_eq!(
            read.get("name").and_then(Value::as_str),
            Some(case.name),
            "case {label:?}: the response must echo the stored name, got {read}"
        );
        assert_eq!(
            read.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {label:?}: a fresh definition reads back ACTIVE, got {read}"
        );
        assert_eq!(
            read.get("description").and_then(Value::as_str),
            case.description,
            "case {label:?}: the response must report the stored description, got {read}"
        );
        assert_eq!(
            read.get("exportConfig").cloned(),
            case.export_config,
            "case {label:?}: the response must report the stored export config, got {read}"
        );

        // A second read reports the same instant: the rendering is of stored state, not of
        // the moment the response was built.
        let (status, again) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a second GetWorkflowDefinition must answer 200, got {status} {again}"
        );
        assert_eq!(
            again.get("createdAt"),
            read.get("createdAt"),
            "case {label:?}: createdAt must be stable across reads, got {again}"
        );
    }
}

/// `GetWorkflowRun` carries a required `startedAt` and an optional `endedAt`, both
/// `date-time`: the required one on every read, the optional one only once the run has
/// finished — and when it is absent it is ABSENT, not null and not a zero epoch number.
#[tokio::test]
async fn get_workflow_run_renders_both_instants_as_date_time_strings() {
    /// Rows vary whether the optional instant exists, the terminal status that owes it,
    /// and how far after the start it falls (`WorkflowRunStatus` values from
    /// `smithy_query(query="shape", shape="WorkflowRunStatus")`).
    struct Case {
        label: &'static str,
        finished: Option<(WorkflowRunStatus, Duration)>,
        log_group_name: Option<&'static str>,
    }

    let cases = vec![
        Case {
            label: "a running run owes no endedAt",
            finished: None,
            log_group_name: None,
        },
        Case {
            label: "succeeded a second later, with a log group",
            finished: Some((WorkflowRunStatus::Succeeded, Duration::from_secs(1))),
            log_group_name: Some("/aws/nova-act/runs"),
        },
        Case {
            label: "failed after a sub-second run",
            finished: Some((WorkflowRunStatus::Failed, Duration::from_millis(250))),
            log_group_name: None,
        },
        Case {
            label: "timed out an hour later",
            finished: Some((WorkflowRunStatus::TimedOut, Duration::from_secs(3600))),
            log_group_name: Some("/aws/nova-act/timeouts"),
        },
    ];

    for case in cases {
        let label = case.label;
        let emulator = Emulator::new();
        let definition = "run-instants";

        let (status, created) = emulator
            .call(
                "PUT",
                "/workflow-definitions",
                Some(json!({ "name": definition })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateWorkflowDefinition must answer 201, got {status} {created}"
        );

        let mut run_body = json!({
            "modelId": "nova-act-v1",
            "clientInfo": { "compatibilityVersion": 1 },
        });
        if let Some(log_group_name) = case.log_group_name {
            run_body["logGroupName"] = json!(log_group_name);
        }
        let (status, run) = emulator
            .call(
                "PUT",
                &format!("/workflow-definitions/{definition}/workflow-runs"),
                Some(run_body),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateWorkflowRun must answer 201, got {status} {run}"
        );
        let run_id = run
            .get("workflowRunId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: the create must answer a run id: {run}"))
            .to_string();

        // `UpdateWorkflowRun` is still a scaffolded 501, so the only way to observe a
        // finished run is to put the store into that state the way that operation would.
        let (started_at, ended_at) = {
            let store = emulator.store();
            let mut store = store.lock();
            let stored = store
                .workflow_definitions
                .get_mut(definition)
                .and_then(|definition| definition.runs.get_mut(&run_id))
                .unwrap_or_else(|| panic!("case {label:?}: the run must be stored"));
            if let Some((status, elapsed)) = case.finished {
                stored.status = status;
                stored.ended_at = Some(stored.started_at + elapsed);
            }
            (stored.started_at, stored.ended_at)
        };

        let (status, read) = emulator
            .call(
                "GET",
                &format!("/workflow-definitions/{definition}/workflow-runs/{run_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetWorkflowRun must answer 200, got {status} {read}"
        );

        let text = date_time_text(&read, "startedAt", label);
        assert_date_time(text, started_at, label, "startedAt");

        match (case.finished, ended_at) {
            (Some(_), Some(ended_at)) => {
                let text = date_time_text(&read, "endedAt", label);
                assert_date_time(text, ended_at, label, "endedAt");
            }
            _ => assert!(
                read.get("endedAt").is_none(),
                "case {label:?}: a run that has not finished carries no endedAt, got {read}"
            ),
        }
    }
}

/// The member schema the JSON codec reads the wire form off: every member this model
/// targets at `DateTimestamp` reports `date-time`, and a member the model leaves alone
/// reports no format at all.
///
/// This is the only reach `WorkflowRunSummary` has while `ListWorkflowRuns` is still a
/// scaffolded 501, and it is what fails if any of these schemas goes back to the vendored
/// codegen's output, which drops the trait.
#[test]
fn every_modelled_date_time_member_schema_carries_the_date_time_format() {
    // Source: every member of this service targeting `com.amazonaws.novaact#DateTimestamp`,
    // from `smithy_query(query="operation", operation="GetWorkflowDefinition,GetWorkflowRun")`
    // and `smithy_query(query="shape",
    // shape="WorkflowDefinitionSummary,WorkflowRunSummary,ActSummary")`.
    let date_time_members: Vec<(&str, &Schema)> = vec![
        (
            "GetWorkflowDefinitionResponse$createdAt",
            &GetWorkflowDefinitionOutput::M_CREATED_AT,
        ),
        (
            "GetWorkflowRunResponse$startedAt",
            &GetWorkflowRunOutput::M_STARTED_AT,
        ),
        (
            "GetWorkflowRunResponse$endedAt",
            &GetWorkflowRunOutput::M_ENDED_AT,
        ),
        (
            "WorkflowDefinitionSummary$createdAt",
            &WorkflowDefinitionSummary::M_CREATED_AT,
        ),
        (
            "WorkflowRunSummary$startedAt",
            &WorkflowRunSummary::M_STARTED_AT,
        ),
        ("WorkflowRunSummary$endedAt", &WorkflowRunSummary::M_ENDED_AT),
        ("ActSummary$startedAt", &ActSummary::M_STARTED_AT),
        ("ActSummary$endedAt", &ActSummary::M_ENDED_AT),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model does not annotate keeps the protocol default, so the fix put no
    // format where the model names none.
    for (case, schema) in [
        (
            "GetWorkflowDefinitionResponse$name",
            &GetWorkflowDefinitionOutput::M_NAME,
        ),
        (
            "GetWorkflowDefinitionResponse$exportConfig",
            &GetWorkflowDefinitionOutput::M_EXPORT_CONFIG,
        ),
        (
            "GetWorkflowRunResponse$logGroupName",
            &GetWorkflowRunOutput::M_LOG_GROUP_NAME,
        ),
        ("ActSummary$status", &ActSummary::M_STATUS),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}
