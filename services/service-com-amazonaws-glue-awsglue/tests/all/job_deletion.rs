// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteJob`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `DeleteJob` removes the job definition the request names from the
//! Region it was created in, with everything held on that record — its tags and the runs
//! started of it — and answers with the name it was given; the delete is idempotent, so a
//! name the Region holds no job under is answered the same way ("if the job definition is
//! not found, no exception is thrown") and leaves every other record standing.
//!
//! The recorded case — one delete of a stored job — is ONE row of the tables below; the
//! rows differ along the command kind, how many members the stored definition carries,
//! the form and length of the job's name, how many runs had been started of it, and
//! whether it carried tags.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::TaggedResourceKind;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, error_code, ok, REGION};

/// The role ARN every job in this suite is created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// Another Region's endpoint, to show a delete resolves the job in the endpoint's Region
/// alone.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the deletion table: a job definition to store, how many runs to start of it
/// before the delete, and the bystanders the delete must leave alone.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The job definition's name.
    name: String,
    /// The members the stored definition carries beside `Name`, `Role` and `Command`.
    extras: Value,
    /// The `JobCommand$Name` of the stored definition — one of the four kinds the service
    /// documents (`glueetl`, `pythonshell`, `gluestreaming`, `glueray`).
    command: &'static str,
    /// How many runs `StartJobRun` starts of the definition before it is deleted.
    runs: usize,
}

/// The `CreateJob` body one row stores: the three required members, the row's command
/// kind, its extra members, and a concurrency that admits the runs the row starts.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "Name": row.name,
        "Role": ROLE_ARN,
        "Command": { "Name": row.command, "ScriptLocation": "s3://etl-scripts/before.py" },
        "Tags": { "owner": "data-platform" },
    });
    if let Some(members) = row.extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    if row.runs > 1 {
        // "The maximum number of concurrent runs allowed for the job": the row holds
        // several runs at once, so the stored definition admits them.
        body["ExecutionProperty"] = json!({ "MaxConcurrentRuns": row.runs });
    }
    body
}

/// The rows: the class the rule generalises over, one row per axis.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a minimal Spark ETL definition",
            name: "etl-usage-events".to_string(),
            extras: json!({}),
            command: "glueetl",
            runs: 0,
        },
        Row {
            case: "a fully populated definition",
            name: "Full.Definition_job".to_string(),
            extras: json!({
                "JobMode": "VISUAL",
                "JobRunQueuingEnabled": true,
                "Description": "the definition a delete removes",
                "LogUri": "s3://glue-logs/before/",
                "DefaultArguments": { "--enable-metrics": "true" },
                "NonOverridableArguments": { "--locked": "before" },
                "Connections": { "Connections": ["warehouse", "lake"] },
                "MaxRetries": 4,
                "Timeout": 123,
                "SecurityConfiguration": "glue-security",
                "NotificationProperty": { "NotifyDelayAfter": 9 },
                "GlueVersion": "4.0",
                "WorkerType": "G.4X",
                "NumberOfWorkers": 5,
                "MaintenanceWindow": "Sun:3",
            }),
            command: "glueetl",
            runs: 1,
        },
        Row {
            case: "a Python shell definition with runs started of it",
            name: "python shell report".to_string(),
            extras: json!({ "MaxCapacity": 0.0625 }),
            command: "pythonshell",
            runs: 3,
        },
        Row {
            case: "a streaming definition",
            name: "stream-ingest".to_string(),
            extras: json!({ "GlueVersion": "3.0" }),
            command: "gluestreaming",
            runs: 0,
        },
        Row {
            case: "a Ray definition",
            name: "ray-feature-build".to_string(),
            extras: json!({ "WorkerType": "Z.2X", "NumberOfWorkers": 2 }),
            command: "glueray",
            runs: 2,
        },
        Row {
            case: "a name at the modelled maximum length",
            // `NameString`'s `length 1..255` (smithy_query operation=DeleteJob): the
            // longest name a job can be stored under.
            name: "j".repeat(255),
            extras: json!({}),
            command: "glueetl",
            runs: 0,
        },
        Row {
            case: "a one-character name",
            // The other published bound of that same `length 1..255`.
            name: "j".to_string(),
            extras: json!({}),
            command: "glueetl",
            runs: 1,
        },
    ]
}

/// The names `ListJobs` reports through this Region's endpoint.
async fn listed_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let body = ok(router, "ListJobs", &json!({})).await;
    body["JobNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListJobs must answer a JobNames list, got {body}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed name is a string, got {name}"))
                .to_string()
        })
        .collect()
}

/// The names `BatchGetJobs` could NOT resolve for `names`.
async fn jobs_not_found(router: &MultiServiceRouter, case: &str, names: &[&str]) -> Vec<String> {
    let body = ok(router, "BatchGetJobs", &json!({ "JobNames": names })).await;
    body["JobsNotFound"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchGetJobs must answer a JobsNotFound list, got {body}")
        })
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a missing name is a string, got {name}"))
                .to_string()
        })
        .collect()
}

/// Start `count` runs of `name` and answer their ids.
async fn start_runs(router: &MultiServiceRouter, case: &str, name: &str, count: usize) -> Vec<String> {
    let mut started = Vec::new();
    for _ in 0..count {
        let body = ok(router, "StartJobRun", &json!({ "JobName": name })).await;
        started.push(
            body["JobRunId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: StartJobRun answers a JobRunId, got {body}"))
                .to_string(),
        );
    }
    started
}

/// A delete removes the named definition and the whole record with it, answers with the
/// name, leaves every other job standing, and frees the name for the next create.
#[tokio::test]
async fn a_delete_removes_the_named_definition_and_answers_with_its_name() {
    for row in rows() {
        let router = emulator();
        let case = row.case;
        // A bystander beside each row's job, so a delete that removed more than the one
        // record it names is visible.
        let bystander = "bystander-etl";
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": bystander,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/bystander.py" },
                "Tags": { "kept": "yes" },
            }),
        )
        .await;

        ok(&router, "CreateJob", &create_body(&row)).await;
        let run_ids = start_runs(&router, case, &row.name, row.runs).await;

        let deleted = ok(&router, "DeleteJob", &json!({ "JobName": row.name })).await;
        assert_eq!(
            deleted["JobName"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: the delete must answer with the name it deleted, got {deleted}"
        );

        // The definition is gone: the read of it is the modelled not-found refusal.
        let (status, body) = call(&router, "GetJob", &json!({ "JobName": row.name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted job must not be readable, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException from the read, got {}",
            error_code(&body)
        );

        // And it is gone from every projection of the namespace.
        let listed = listed_names(&router, case).await;
        assert_eq!(
            listed,
            vec![bystander.to_string()],
            "case {case:?}: the delete must leave only the bystander listed, got {listed:?}"
        );
        let missing = jobs_not_found(&router, case, &[&row.name, bystander]).await;
        assert_eq!(
            missing,
            vec![row.name.clone()],
            "case {case:?}: the batch read must report only the deleted name as missing, got \
             {missing:?}"
        );

        // The runs started of the definition went with it: the history is not readable,
        // and neither is any run of it.
        let (status, body) = call(&router, "GetJobRuns", &json!({ "JobName": row.name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted job's run history must be gone, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException from the run listing, got {}",
            error_code(&body)
        );
        for run_id in &run_ids {
            let (status, body) = call(
                &router,
                "GetJobRun",
                &json!({ "JobName": row.name, "RunId": run_id }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: run {run_id} must be gone with its job, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case:?}: expected EntityNotFoundException for run {run_id}, got {}",
                error_code(&body)
            );
        }

        // The tag set went with the record too: the ARN names nothing to report tags for.
        let arn = TaggedResourceKind::Job.arn(REGION, &row.name);
        let (status, body) = call(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted job holds no tags to read, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException from the tag read, got {}",
            error_code(&body)
        );

        // The bystander is untouched, tags included.
        let kept = ok(&router, "GetJob", &json!({ "JobName": bystander })).await;
        assert_eq!(
            kept["Job"]["Name"].as_str(),
            Some(bystander),
            "case {case:?}: the delete must leave the bystander readable, got {kept}"
        );
        let kept_tags = ok(
            &router,
            "GetTags",
            &json!({ "ResourceArn": TaggedResourceKind::Job.arn(REGION, bystander) }),
        )
        .await;
        assert_eq!(
            kept_tags.get("Tags"),
            Some(&json!({ "kept": "yes" })),
            "case {case:?}: the delete must leave the bystander's tags alone, got {kept_tags}"
        );

        // The name is free again, and the record created under it is the new one: no tags
        // and no runs survive from the deleted definition.
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": row.name,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/after.py" },
            }),
        )
        .await;
        let again = ok(&router, "GetJob", &json!({ "JobName": row.name })).await;
        assert_eq!(
            again["Job"]["Command"]["ScriptLocation"].as_str(),
            Some("s3://etl-scripts/after.py"),
            "case {case:?}: the re-created job is the one this create sent, got {again}"
        );
        let history = ok(&router, "GetJobRuns", &json!({ "JobName": row.name })).await;
        assert_eq!(
            history["JobRuns"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the re-created job starts with no runs, got {history}"
        );
        let fresh_tags = ok(
            &router,
            "GetTags",
            &json!({ "ResourceArn": TaggedResourceKind::Job.arn(REGION, &row.name) }),
        )
        .await;
        assert_eq!(
            fresh_tags.get("Tags"),
            Some(&json!({})),
            "case {case:?}: the deleted job's tags must not survive into the next create, got \
             {fresh_tags}"
        );
    }
}

/// A delete of a name the endpoint's Region holds no job under is served, not refused:
/// it answers with the name it was given and changes nothing.
///
/// The rows are the ways a name can name no job here: one nothing ever created, one just
/// deleted, one differing only in case from a stored job, one at the modelled maximum
/// length, and the name of a job stored in ANOTHER Region.
#[tokio::test]
async fn a_delete_of_a_name_no_job_is_stored_under_changes_nothing() {
    let router = emulator();
    let stored = "etl-usage-events";
    let elsewhere = "eu-only-job";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": stored,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    // A job of another Region, created through that Region's endpoint.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "CreateJob",
        &json!({
            "Name": elsewhere,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/eu.py" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region's create must be served, got {status} with {body}"
    );

    // One name that WAS stored here, deleted first, so the second delete of it is the
    // idempotent one.
    let gone = "already-deleted";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": gone,
            "Role": ROLE_ARN,
            "Command": { "Name": "pythonshell", "ScriptLocation": "s3://etl-scripts/py.py" },
        }),
    )
    .await;
    ok(&router, "DeleteJob", &json!({ "JobName": gone })).await;

    let unknown: Vec<(&str, String)> = vec![
        ("never created", "no-such-job".to_string()),
        ("already deleted", gone.to_string()),
        ("differing only in case", stored.to_uppercase()),
        ("at the modelled maximum length", "n".repeat(255)),
        ("stored in another Region", elsewhere.to_string()),
    ];
    for (case, name) in &unknown {
        let answer = ok(&router, "DeleteJob", &json!({ "JobName": name })).await;
        assert_eq!(
            answer["JobName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: an idempotent delete answers with the name it was given, got {answer}"
        );

        // Nothing else moved: this Region still holds exactly the one job it stored.
        let listed = listed_names(&router, case).await;
        assert_eq!(
            listed,
            vec![stored.to_string()],
            "case {case:?}: the delete must leave the stored job listed, got {listed:?}"
        );
        let job = ok(&router, "GetJob", &json!({ "JobName": stored })).await;
        assert_eq!(
            job["Job"]["Command"]["ScriptLocation"].as_str(),
            Some("s3://etl-scripts/etl.py"),
            "case {case:?}: the stored definition must be untouched, got {job}"
        );
    }

    // The other Region's job is still served there: a delete addressed to this endpoint
    // never reached it.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetJob",
        &json!({ "JobName": elsewhere }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region must still serve its own job, got {status} with {body}"
    );
    assert_eq!(
        body["Job"]["Command"]["ScriptLocation"].as_str(),
        Some("s3://etl-scripts/eu.py"),
        "the other Region's definition must be untouched, got {body}"
    );

    // And a delete through THAT endpoint does remove it, so the misses above are about
    // the Region and not about the name.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "DeleteJob",
        &json!({ "JobName": elsewhere }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the owning Region's delete must be served, got {status} with {body}"
    );
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetJob",
        &json!({ "JobName": elsewhere }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the other Region's job must be gone after its own delete, got {status} with {body}"
    );
}
