// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the workflow DEFINITION a version snapshots, and the schedule
//! that definition declares — `GetWorkflow`'s `WorkflowDefinition` and
//! `ScheduleConfiguration`, and `ListWorkflowVersions`' `ScheduleConfiguration`.
//!
//! The rule under test: a read of a workflow version reports the definition CONTENT the
//! Amazon S3 object held when that version was created, plus the schedule that content
//! declares, read once at version creation and frozen in the version — so a read never
//! goes to Amazon S3, overwriting the object changes nothing a caller reads back, and a
//! changed definition reaches a caller only through a new version.
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because the
//! snapshot is taken by calling S3 in process exactly as the assembled emulator does.
//! The tables span what the rule generalises over — where the schedule is declared, how
//! it is spelled, the definition's own form and size, and which version is read — rather
//! than pinning one request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::workflow_definition::MAX_DEFINITION_BYTES;
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::Workflow;
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;
use smithy_server_runtime::MultiServiceRouter;

use crate::workflow_lifecycle::role_arn;

/// The regional endpoint every request below is signed for. The same region is the one
/// the definition object is staged in, because a workflow's definition is read where the
/// caller's own Amazon S3 requests go.
///
/// `pub(crate)` with the staging helpers below, so another suite that needs a workflow
/// whose definition really is an object in Amazon S3 drives the same path rather than
/// building a second harness.
pub(crate) const REGION: &str = "us-west-2";

/// The bucket every definition below is staged in.
pub(crate) const BUCKET: &str = "mwaa-serverless-definitions";

/// A router with this service and the Amazon S3 service on one shared engine — what
/// `build_router` assembles, narrowed to the services under test.
pub(crate) fn definition_router() -> MultiServiceRouter {
    definition_router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same router, built on an engine the CALLER keeps a handle to, so a suite whose
/// rule is about a STORED clock field — the instant a workflow's trigger next fires —
/// can seed and read that field while still driving every request through this wire
/// path.
pub(crate) fn definition_router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mwaaserverless_amazonmwaaserverless::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// Edit the workflow `arn` as the STORE holds it. The instant a trigger next fires is a
/// clock field of that record, so this is how a suite puts a firing in the past, or
/// reads the trigger, without racing the wall clock.
pub(crate) fn with_stored_workflow<R>(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    edit: impl FnOnce(&mut Workflow) -> R,
) -> R {
    let state = engine.state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let mut store = state.lock();
    let workflow = store
        .workflows
        .get_mut(arn)
        .expect("setup: the created workflow is stored");
    edit(workflow)
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes, so this is a raw request
/// rather than an `x-amz-target` call. The credential scope names `s3`, which is what the
/// router claims on.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        response.body().as_ref().to_vec(),
    )
}

/// Create [`BUCKET`], so the definitions below have somewhere to live. The create names
/// its `LocationConstraint`, because Amazon S3 serves a create that names none only at
/// the endpoint the unspecified constraint stands for.
pub(crate) async fn create_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{BUCKET}"),
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "the bucket must be created first, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Store `content` at `key` in [`BUCKET`], overwriting whatever was there.
pub(crate) async fn stage(router: &MultiServiceRouter, key: &str, content: &[u8]) {
    let (status, body) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), content).await;
    assert_eq!(
        status,
        200,
        "case {key:?}: the definition must be stored, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Call one operation of this service over the wire and return its status and parsed
/// body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let payload = serde_json::to_vec(&body).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonMWAAServerless.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{REGION}/\
                 airflow-serverless/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=00"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Create a workflow whose definition is the object at `key`, and answer its ARN.
pub(crate) async fn create_workflow(router: &MultiServiceRouter, name: &str, key: &str) -> String {
    create_workflow_at(router, name, BUCKET, key).await
}

/// Create a workflow whose definition is the object at `key`, carrying `trigger_mode`
/// when one is named and omitting the optional member entirely when it is `None`, and
/// answer its ARN.
pub(crate) async fn create_workflow_with_mode(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    trigger_mode: Option<&str>,
) -> String {
    let mut body = json!({
        "Name": name,
        "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
        "RoleArn": role_arn(),
    });
    if let Some(mode) = trigger_mode {
        body["TriggerMode"] = Value::String(mode.to_string());
    }
    let (status, created) = call(router, "CreateWorkflow", body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the workflow must be created, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string()
}

/// Create a workflow whose definition is the object `bucket`/`key` addresses, and answer
/// its ARN.
async fn create_workflow_at(
    router: &MultiServiceRouter,
    name: &str,
    bucket: &str,
    key: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreateWorkflow",
        json!({
            "Name": name,
            "DefinitionS3Location": {"Bucket": bucket, "ObjectKey": key},
            "RoleArn": role_arn(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the workflow must be created, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string()
}

/// `GetWorkflow` for `arn`, at `version` when one is named.
///
/// `pub(crate)` with [`reported_cron`] below, so a suite whose rule is about WHAT a read
/// reports for a declared schedule reads it back through this same wire path rather than
/// keeping a second copy of it.
pub(crate) async fn get_workflow(
    router: &MultiServiceRouter,
    arn: &str,
    version: Option<&str>,
) -> Value {
    let mut body = json!({"WorkflowArn": arn});
    if let Some(version) = version {
        body["WorkflowVersion"] = Value::String(version.to_string());
    }
    let (status, fetched) = call(router, "GetWorkflow", body).await;
    assert_eq!(
        status, 200,
        "GetWorkflow({arn}, {version:?}) must succeed, got {status} {fetched:?}"
    );
    fetched
}

/// The `CronExpression` a read reports, or `None` when it reports no schedule at all.
pub(crate) fn reported_cron(reported: &Value) -> Option<&str> {
    reported["ScheduleConfiguration"]["CronExpression"].as_str()
}

/// One row of the declaration table: the definition content, and the cron expression a
/// read must report for it.
struct Declaration {
    case: &'static str,
    key: &'static str,
    definition: &'static str,
    cron: Option<&'static str>,
}

/// The declaration table. Rows differ along what the rule generalises over: WHERE the
/// schedule is declared (the document root, the `dag` mapping, deeper, nowhere), under
/// WHICH key (Apache Airflow's `schedule` and its former `schedule_interval`), HOW the
/// value is spelled (a five-field cron expression, one with a year field, an Airflow
/// preset, a value that is no cron expression at all, a sequence), and in which FORM the
/// definition is written (block YAML, flow YAML, JSON — YAML 1.2 is a superset of JSON).
const DECLARATIONS: &[Declaration] = &[    Declaration {
        case: "a five-field cron expression at the document root",
        key: "dags/nightly.yaml",
        definition: "name: nightly\nschedule: \"0 12 * * *\"\ntasks:\n  - id: extract\n",
        cron: Some("0 12 * * *"),
    },
    Declaration {
        case: "a cron expression inside the dag mapping",
        key: "dags/nested.yaml",
        definition: "dag:\n  dag_id: nested\n  schedule: 15 3 * * 1\ntasks: []\n",
        cron: Some("15 3 * * 1"),
    },
    Declaration {
        case: "the former schedule_interval spelling",
        key: "dags/legacy.yaml",
        definition: "schedule_interval: \"*/15 * * * *\"\ntasks: []\n",
        cron: Some("*/15 * * * *"),
    },
    Declaration {
        case: "an Airflow preset",
        key: "dags/hourly.yaml",
        definition: "schedule: \"@hourly\"\n",
        cron: Some("0 * * * *"),
    },
    Declaration {
        case: "an Airflow preset with a weekday field",
        key: "dags/weekly.yaml",
        definition: "dag:\n  schedule: \"@weekly\"\n",
        cron: Some("0 0 * * 0"),
    },
    Declaration {
        case: "an Airflow preset with a step in its month field",
        key: "dags/quarterly.yaml",
        definition: "schedule: \"@quarterly\"\n",
        cron: Some("0 0 1 */3 *"),
    },
    Declaration {
        case: "a cron expression carrying a year field",
        key: "dags/yearful.yaml",
        definition: "schedule: 0 12 * * ? *\n",
        cron: Some("0 12 * * ? *"),
    },
    Declaration {
        case: "surrounding whitespace around the expression",
        key: "dags/padded.yaml",
        definition: "schedule: \"  30 4 1 * *  \"\n",
        cron: Some("30 4 1 * *"),
    },
    Declaration {
        case: "a definition written as JSON",
        key: "dags/flow.json",
        definition: "{\"dag\": {\"schedule\": \"0 3 * * *\"}, \"tasks\": []}",
        cron: Some("0 3 * * *"),
    },
    Declaration {
        case: "a cron expression naming its month and weekday",
        key: "dags/named.yaml",
        definition: "schedule: 0 6 1 JAN MON\ntasks: []\n",
        cron: Some("0 6 1 JAN MON"),
    },
    Declaration {
        case: "a day-of-month qualifier",
        key: "dags/last-day.yaml",
        definition: "schedule: 0 22 L * ?\n",
        cron: Some("0 22 L * ?"),
    },
    Declaration {
        case: "no schedule declared at all",
        key: "dags/manual.yaml",
        definition: "name: manual\ntasks:\n  - id: load\n",
        cron: None,
    },
    Declaration {
        case: "a preset that stands for no cron schedule",
        key: "dags/once.yaml",
        definition: "schedule: \"@once\"\ntasks: []\n",
        cron: None,
    },
    Declaration {
        case: "a value that is not a cron expression",
        key: "dags/prose.yaml",
        definition: "schedule: whenever you like\n",
        cron: None,
    },
    Declaration {
        case: "prose with as many words as a cron expression has fields",
        key: "dags/prose-five.yaml",
        definition: "schedule: run it every other day\n",
        cron: None,
    },
    Declaration {
        case: "a schedule key below the DAG level",
        key: "dags/task-level.yaml",
        definition: "tasks:\n  - id: t\n    schedule: 0 1 * * *\n",
        cron: None,
    },
    Declaration {
        case: "a schedule key carrying a sequence rather than an expression",
        key: "dags/sequence.yaml",
        definition: "schedule:\n  - 0 1 * * *\n  - 0 2 * * *\n",
        cron: None,
    },
];

#[tokio::test]
async fn get_workflow_reports_the_snapshotted_definition_and_its_declared_schedule() {
    let router = definition_router();
    create_bucket(&router).await;

    for (index, row) in DECLARATIONS.iter().enumerate() {
        let case = row.case;
        stage(&router, row.key, row.definition.as_bytes()).await;
        let name = format!("declared{index}");
        let arn = create_workflow(&router, &name, row.key).await;
        let fetched = get_workflow(&router, &arn, None).await;

        assert_eq!(
            fetched["WorkflowDefinition"].as_str(),
            Some(row.definition),
            "case {case:?}: expected the snapshotted definition content, got {:?}",
            fetched["WorkflowDefinition"]
        );
        assert_eq!(
            reported_cron(&fetched),
            row.cron,
            "case {case:?}: expected the cron expression {:?} the definition declares, \
             got {:?}",
            row.cron,
            fetched["ScheduleConfiguration"]
        );
        if row.cron.is_none() {
            assert!(
                fetched["ScheduleConfiguration"].is_null(),
                "case {case:?}: a definition declaring no cron schedule must report no \
                 ScheduleConfiguration, got {:?}",
                fetched["ScheduleConfiguration"]
            );
        }
        // The pointer is still reported beside the content it was snapshotted from.
        assert_eq!(
            fetched["DefinitionS3Location"]["ObjectKey"].as_str(),
            Some(row.key),
            "case {case:?}: expected the definition location to be reported too, got {:?}",
            fetched["DefinitionS3Location"]
        );
    }
}

#[tokio::test]
async fn the_snapshot_is_frozen_at_version_creation_and_survives_an_s3_overwrite() {
    // The rule is an invariant over the SET of a workflow's versions: each version
    // reports the content its own creation froze. Each row overwrites the one object both
    // versions point at, so the only thing telling the two snapshots apart is WHEN each
    // version was created.
    let rows = [
        (
            "a schedule replaced by another",
            "schedule: \"0 1 * * *\"\ntasks: []\n",
            Some("0 1 * * *"),
            "schedule: \"@hourly\"\ntasks: []\n",
            Some("0 * * * *"),
        ),
        (
            "a schedule removed",
            "schedule: \"0 6 * * 5\"\n",
            Some("0 6 * * 5"),
            "tasks:\n  - id: only\n",
            None,
        ),
        (
            "a schedule added where there was none",
            "tasks:\n  - id: only\n",
            None,
            "schedule: \"*/5 * * * *\"\ntasks:\n  - id: only\n",
            Some("*/5 * * * *"),
        ),
    ];

    for (index, (case, first, first_cron, second, second_cron)) in rows.into_iter().enumerate() {
        let router = definition_router();
        create_bucket(&router).await;
        let key = "dags/pipeline.yaml";
        stage(&router, key, first.as_bytes()).await;
        let arn = create_workflow(&router, &format!("frozen{index}"), key).await;
        let original_version = get_workflow(&router, &arn, None).await["WorkflowVersion"]
            .as_str()
            .expect("a created workflow reports its version")
            .to_string();

        // The object is overwritten AFTER the version was created, which must change
        // nothing a read reports.
        stage(&router, key, second.as_bytes()).await;
        let after_overwrite = get_workflow(&router, &arn, None).await;
        assert_eq!(
            after_overwrite["WorkflowDefinition"].as_str(),
            Some(first),
            "case {case:?}: an overwrite of the object must not change the snapshotted \
             definition, got {:?}",
            after_overwrite["WorkflowDefinition"]
        );
        assert_eq!(
            reported_cron(&after_overwrite),
            first_cron,
            "case {case:?}: an overwrite of the object must not change the reported \
             schedule, got {:?}",
            after_overwrite["ScheduleConfiguration"]
        );
        // Read twice: a read reports the frozen snapshot, so it cannot have refreshed it.
        let again = get_workflow(&router, &arn, None).await;
        assert_eq!(
            again["WorkflowDefinition"], after_overwrite["WorkflowDefinition"],
            "case {case:?}: two reads must agree, got {:?} then {:?}",
            after_overwrite["WorkflowDefinition"], again["WorkflowDefinition"]
        );

        // A new version is what picks the changed object up.
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must succeed, got {status} {updated:?}"
        );
        let new_version = updated["WorkflowVersion"]
            .as_str()
            .expect("an update reports the version it created")
            .to_string();

        let latest = get_workflow(&router, &arn, None).await;
        assert_eq!(
            latest["WorkflowDefinition"].as_str(),
            Some(second),
            "case {case:?}: the new version must report the object as it stands now, \
             got {:?}",
            latest["WorkflowDefinition"]
        );
        assert_eq!(
            reported_cron(&latest),
            second_cron,
            "case {case:?}: the new version must report the schedule its own definition \
             declares, got {:?}",
            latest["ScheduleConfiguration"]
        );

        // …and the earlier version still reports what IT froze.
        let earlier = get_workflow(&router, &arn, Some(&original_version)).await;
        assert_eq!(
            earlier["WorkflowDefinition"].as_str(),
            Some(first),
            "case {case:?}: version {original_version} must keep the definition it froze, \
             got {:?}",
            earlier["WorkflowDefinition"]
        );
        assert_eq!(
            reported_cron(&earlier),
            first_cron,
            "case {case:?}: version {original_version} must keep the schedule it froze, \
             got {:?}",
            earlier["ScheduleConfiguration"]
        );

        // Every version's own declared schedule is what the version listing reports.
        let (status, listed) = call(&router, "ListWorkflowVersions", json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the version listing must succeed, got {status} {listed:?}"
        );
        let summaries = listed["WorkflowVersions"]
            .as_array()
            .expect("a version listing reports a list")
            .clone();
        for (version, expected) in [
            (original_version.as_str(), first_cron),
            (new_version.as_str(), second_cron),
        ] {
            let summary = summaries
                .iter()
                .find(|summary| summary["WorkflowVersion"].as_str() == Some(version))
                .unwrap_or_else(|| panic!("case {case:?}: version {version} must be listed"));
            assert_eq!(
                reported_cron(summary),
                expected,
                "case {case:?}: the listing must report version {version}'s own schedule \
                 {expected:?}, got {:?}",
                summary["ScheduleConfiguration"]
            );
        }
    }
}

/// A definition padded to exactly `total` bytes: a declaration followed by a YAML comment
/// long enough to reach that size, so a row names the size it is testing.
fn padded_definition(total: usize) -> String {
    let mut text = String::with_capacity(total);
    text.push_str("schedule: \"0 1 * * *\"\n# ");
    while text.len() < total {
        text.push('a');
    }
    text
}

#[tokio::test]
async fn a_definition_at_the_size_ceiling_is_snapshotted_and_one_past_it_is_not() {
    let router = definition_router();
    create_bucket(&router).await;

    // The ceiling is a range, so it is tested on its published bound and one past it.
    for (index, (case, size, snapshotted)) in [
        ("one byte below the ceiling", MAX_DEFINITION_BYTES - 1, true),
        ("exactly at the ceiling", MAX_DEFINITION_BYTES, true),
        ("one byte past the ceiling", MAX_DEFINITION_BYTES + 1, false),
    ]
    .into_iter()
    .enumerate()
    {
        let definition = padded_definition(size);
        assert_eq!(
            definition.len(),
            size,
            "case {case:?}: the staged definition must be exactly {size} bytes, got {}",
            definition.len()
        );
        let key = format!("dags/sized-{index}.yaml");
        stage(&router, &key, definition.as_bytes()).await;
        let arn = create_workflow(&router, &format!("sized{index}"), &key).await;
        let fetched = get_workflow(&router, &arn, None).await;

        let expected = snapshotted.then_some(definition.as_str());
        assert_eq!(
            fetched["WorkflowDefinition"].as_str(),
            expected,
            "case {case:?}: expected the definition content to be {}, got {} bytes",
            if snapshotted { "reported" } else { "absent" },
            fetched["WorkflowDefinition"].as_str().unwrap_or("").len()
        );
        assert_eq!(
            reported_cron(&fetched),
            snapshotted.then_some("0 1 * * *"),
            "case {case:?}: the reported schedule must follow the snapshot, got {:?}",
            fetched["ScheduleConfiguration"]
        );
    }
}

#[tokio::test]
async fn a_hostile_definition_is_read_within_its_bounds_and_answers_the_same_way() {
    let router = definition_router();
    create_bucket(&router).await;

    // Rows differ along HOW the content resists being read: nesting far past anything a
    // DAG declaration uses, a document truncated mid-collection, one whose indentation is
    // not YAML at all, and one whose anchors would amplify if an alias were ever
    // expanded. Each row names what the read must still report: the definition content
    // round-trips either way, and the schedule is reported exactly when the walk reached
    // a DAG-level declaration before it stopped.
    let deep = format!(
        "schedule: \"0 1 * * *\"\ndeep: {}{}\n",
        "[".repeat(4_000),
        "]".repeat(4_000)
    );
    let deep_first = format!("deep: {}{}\n", "[".repeat(4_000), "]".repeat(4_000));
    let anchored = format!(
        "seed: &a [{}]\ngrown: &b [{}]\nschedule: \"@daily\"\n",
        ["x"; 40].join(","),
        ["*a"; 40].join(",")
    );
    let rows: [(&str, String, Option<&str>); 5] = [
        (
            "nesting far past the depth the walk follows, after the declaration",
            deep,
            Some("0 1 * * *"),
        ),
        (
            "nesting far past the depth the walk follows, and no declaration",
            deep_first,
            None,
        ),
        (
            "a document truncated inside a flow collection",
            "schedule: \"30 2 * * *\"\ntasks:\n  - [one, two".to_string(),
            Some("30 2 * * *"),
        ),
        (
            "indentation that is not YAML",
            "tasks:\n  - id: a\n\tid: b\n".to_string(),
            None,
        ),
        (
            "anchors an expansion would amplify",
            anchored,
            Some("0 0 * * *"),
        ),
    ];

    for (index, (case, definition, cron)) in rows.into_iter().enumerate() {
        assert!(
            definition.len() <= MAX_DEFINITION_BYTES,
            "case {case:?}: the row must stay within the snapshot ceiling, got {} bytes",
            definition.len()
        );
        let key = format!("dags/hostile-{index}.yaml");
        stage(&router, &key, definition.as_bytes()).await;
        let arn = create_workflow(&router, &format!("hostile{index}"), &key).await;
        let fetched = get_workflow(&router, &arn, None).await;

        assert_eq!(
            fetched["WorkflowDefinition"].as_str(),
            Some(definition.as_str()),
            "case {case:?}: the snapshotted content must round-trip whatever the walk \
             made of it, got {} bytes",
            fetched["WorkflowDefinition"].as_str().unwrap_or("").len()
        );
        assert_eq!(
            reported_cron(&fetched),
            cron,
            "case {case:?}: expected the cron expression {cron:?}, got {:?}",
            fetched["ScheduleConfiguration"]
        );
    }
}

#[tokio::test]
async fn a_definition_object_holding_nothing_readable_leaves_both_members_absent() {
    let router = definition_router();
    create_bucket(&router).await;
    stage(&router, "dags/empty.yaml", b"").await;
    stage(&router, "dags/binary.yaml", &[0xf0, 0x9f, 0x00, 0xff]).await;

    // Rows differ along WHY there is nothing to snapshot: no object, no bucket, an object
    // holding no bytes, and bytes that are not text. None of them is a reason to refuse
    // the create, and none of them reports content this service never read.
    for (index, (case, bucket, key)) in [
        ("no object at that key", BUCKET, "dags/absent.yaml"),
        ("no bucket at all", "mwaa-serverless-missing", "dags/any.yaml"),
        ("an object holding no bytes", BUCKET, "dags/empty.yaml"),
        ("bytes that are not text", BUCKET, "dags/binary.yaml"),
    ]
    .into_iter()
    .enumerate()
    {
        let arn = create_workflow_at(&router, &format!("unreadable{index}"), bucket, key).await;
        let fetched = get_workflow(&router, &arn, None).await;
        assert!(
            fetched["WorkflowDefinition"].is_null(),
            "case {case:?}: expected no definition content, got {:?}",
            fetched["WorkflowDefinition"]
        );
        assert!(
            fetched["ScheduleConfiguration"].is_null(),
            "case {case:?}: expected no schedule, got {:?}",
            fetched["ScheduleConfiguration"]
        );
        assert_eq!(
            fetched["DefinitionS3Location"]["Bucket"].as_str(),
            Some(bucket),
            "case {case:?}: the location the request named must still be reported, got {:?}",
            fetched["DefinitionS3Location"]
        );
    }
}
