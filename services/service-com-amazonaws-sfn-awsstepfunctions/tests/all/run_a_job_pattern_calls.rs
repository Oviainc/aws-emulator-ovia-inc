// SPDX-License-Identifier: CC0-1.0

//! The calls a run-a-job (`.sync`) service integration is MADE of: the request that
//! reports the job a `Task` state started, and the request that stops it.
//!
//! The rule under test: **a `.sync` `Resource` names a job this emulator starts,
//! reports and stops through the invoked service's own operations, and each of those
//! two calls is made with every member that operation needs to name the job — the
//! identifier the starting call answered, carried in the shape that operation types
//! the member as, beside every member of the STARTING request that is part of the
//! job's identity.** An integration whose job cannot be reported and stopped that way
//! starts nothing and says so.
//!
//! The rows span the whole catalogue
//! ([`service_com_amazonaws_sfn_awsstepfunctions::state::integration::RUN_A_JOB`]),
//! which is the data the pattern is performed from, and vary along what the rule
//! generalises over: whether the job's identifier names it on its own or beside
//! another member of the starting request, whether the stopping operation takes one
//! identifier or a batch of them, which spelling the definition wrote those members in
//! (the integration's own PascalCase and the invoked model's lowerCamel), and whether
//! the starting request carried them at all.
//!
//! The last table drives the rule's boundary end to end: a state machine whose single
//! `Task` state carries each `.sync` `Resource` is started, and what the execution
//! then reports says whether the pattern was performed — parked at the job the call
//! started — or refused because the catalogue names no way to report and stop it.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds and the history reader is `execution_history`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::integration::{
    describing, read, stopping, task_result, Api, JobNaming, RunAJob, RUN_A_JOB,
};
use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_SUBMITTED;

use crate::execution_history::history;
use crate::state_machine_lifecycle::{create_machine, string, Emulator};

/// A job identifier standing for the one a starting call answered. Its text is never
/// parsed by anything under test: what the tables assert is WHERE it is carried.
const JOB_ID: &str = "the-job-the-call-started";

/// The two spellings a definition may write an integration's parameters in: the
/// integration's own convention, PascalCase, and the lowerCamel an invoked model may
/// use for the same member. Both reach the operation, so both are a row here.
const SPELLINGS: [(&str, bool); 2] = [
    ("the integration's own PascalCase", true),
    ("the invoked model's lowerCamel", false),
];

/// `member` with its first character upper-cased (`upper`) or lower-cased.
fn spelled(member: &str, upper: bool) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) if upper => first.to_uppercase().chain(characters).collect(),
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The service `entry`'s `.sync` `Resource` resolves to a call of.
fn api_of(entry: &RunAJob) -> &'static Api {
    let resource = format!("arn:aws:states:::{}.sync", entry.starts);
    read(&resource)
        .unwrap_or_else(|why| panic!("case {:?}: expected a call, got {why:?}", entry.starts))
        .api
}

/// A starting request naming the job through every member `entry` says is part of its
/// identity, spelled with an upper or lower first character.
fn starting_request(entry: &RunAJob, upper: bool) -> Value {
    let mut request = serde_json::Map::new();
    for member in entry.names_job_with {
        request.insert(
            spelled(member, upper),
            Value::String(format!("the-{member}-the-state-named")),
        );
    }
    Value::Object(request)
}

/// What `entry`'s naming must hold for the request [`starting_request`] builds.
fn expected_naming(entry: &RunAJob) -> JobNaming {
    entry
        .names_job_with
        .iter()
        .map(|member| {
            (
                (*member).to_string(),
                format!("the-{member}-the-state-named"),
            )
        })
        .collect()
}

/// The reporting and stopping calls of every catalogued integration are made with the
/// members those operations name the job by, and with nothing else.
#[test]
fn each_catalogued_job_is_reported_and_stopped_by_the_members_that_name_it() {
    assert!(
        !RUN_A_JOB.is_empty(),
        "the catalogue of run-a-job integrations must not be empty"
    );
    for entry in RUN_A_JOB {
        let api = api_of(entry);
        // Either spelling of the state's own document reaches the operation, so the
        // naming is read out of both and carries the same members.
        for (spelling, upper) in SPELLINGS {
            let label = format!("{} named in {spelling}", entry.starts);
            let request = starting_request(entry, upper);
            let naming = entry.naming(&request);
            assert_eq!(
                naming,
                expected_naming(entry),
                "case {label:?}: expected the naming members of the starting request, got \
                 {naming:?}"
            );

            // The reporting call: the operation the entry names, carrying the
            // identifier under its own member beside the naming, and nothing more.
            let (reporting, reported) = describing(api, entry, JOB_ID, &naming);
            assert_eq!(
                reporting.operation, entry.describe,
                "case {label:?}: expected the reporting operation {}, got {}",
                entry.describe, reporting.operation
            );
            assert!(
                reporting.job.is_none() && !reporting.sdk_envelope,
                "case {label:?}: the job's report is read, so it parks at no job and carries no \
                 envelope, got {reporting:?}"
            );
            assert_eq!(
                reported.get(entry.describe_member).and_then(Value::as_str),
                Some(JOB_ID),
                "case {label:?}: expected the identifier under {:?}, got {reported:?}",
                entry.describe_member
            );
            assert_members(&label, &reported, entry.describe_member, &naming);

            // The stopping call: the same, with the identifier in the shape that
            // operation types the member as.
            let (stopping_call, to_stop) = stopping(api, entry, JOB_ID, &naming);
            assert_eq!(
                stopping_call.operation, entry.stop,
                "case {label:?}: expected the stopping operation {}, got {}",
                entry.stop, stopping_call.operation
            );
            let identifier = to_stop
                .get(entry.stop_member)
                .unwrap_or_else(|| panic!("case {label:?}: expected {:?} in {to_stop:?}", entry.stop_member));
            let expected = match entry.stops_in_batch {
                true => json!([JOB_ID]),
                false => json!(JOB_ID),
            };
            assert_eq!(
                identifier, &expected,
                "case {label:?}: expected the identifier as {expected:?}, got {identifier:?}"
            );
            assert_members(&label, &to_stop, entry.stop_member, &naming);
        }

        // A starting request that named none of them invents none: the call carries
        // the identifier alone, and the invoked operation answers for the rest.
        let label = format!("{} with a request naming nothing", entry.starts);
        let naming = entry.naming(&json!({ "Unrelated": "value" }));
        assert!(
            naming.is_empty(),
            "case {label:?}: expected no naming members, got {naming:?}"
        );
        let (_, reported) = describing(api, entry, JOB_ID, &naming);
        assert_members(&label, &reported, entry.describe_member, &naming);
        let (_, to_stop) = stopping(api, entry, JOB_ID, &naming);
        assert_members(&label, &to_stop, entry.stop_member, &naming);
    }
}

/// `request` carries the identifier member and every naming member, and no other.
fn assert_members(label: &str, request: &Value, identifier: &str, naming: &JobNaming) {
    let members = request
        .as_object()
        .unwrap_or_else(|| panic!("case {label:?}: expected an object, got {request:?}"));
    let mut expected: Vec<String> = naming.keys().cloned().collect();
    expected.push(identifier.to_string());
    expected.sort();
    let mut carried: Vec<String> = members.keys().cloned().collect();
    carried.sort();
    assert_eq!(
        carried, expected,
        "case {label:?}: expected exactly {expected:?}, got {carried:?}"
    );
    for (member, value) in naming {
        assert_eq!(
            members.get(member).and_then(Value::as_str),
            Some(value.as_str()),
            "case {label:?}: expected {member:?} to name the job, got {request:?}"
        );
    }
}

/// A member beside the job's status in the reporting call's answer, standing for the
/// job's own members (its identity, its instants, its reason): what the tables assert
/// is WHERE it ends up, never its text.
const BESIDE_THE_STATUS: &str = "a member of the job's own document";

/// The reporting answer of `entry` for a job in `status`: the status where that entry
/// says it sits, with a member of the job's own document beside the job's document —
/// that is, at the top level of whatever the entry says the task reports.
fn describe_answer(entry: &RunAJob, status: &str) -> Value {
    let (last, leading) = entry
        .status_at
        .split_last()
        .unwrap_or_else(|| panic!("case {:?}: an entry must name where its status sits", entry.starts));
    let mut document = json!({ *last: status });
    for member in leading.iter().rev() {
        document = json!({ *member: document });
    }
    // The marker goes where the job's own document is, so what a row asserts is that the
    // task reports THAT document and not the wrapper around it.
    let mut node = &mut document;
    for member in entry.reports_at {
        node = node
            .get_mut(*member)
            .unwrap_or_else(|| panic!("case {:?}: the answer must carry {member:?}", entry.starts));
    }
    if let Some(members) = node.as_object_mut() {
        members.insert("Marker".to_string(), json!(BESIDE_THE_STATUS));
    }
    document
}

/// The document `path` names in `document`.
fn value_at(document: &Value, path: &[&str]) -> Value {
    let mut node = document;
    for member in path {
        node = node
            .get(*member)
            .unwrap_or_else(|| panic!("the answer must carry {member:?}, got {document:?}"));
    }
    node.clone()
}

/// What a `.sync` task reports is what the invoked service said about the JOB: the
/// document that service's answer carries the job in, whether the job did its work (the
/// task's result) or ended without doing it (the failure's cause).
///
/// The rows span the whole catalogue and, within each entry, every status that entry
/// calls finished — so an integration whose answer wraps the job's document (Glue's
/// `GetJobRun`, which carries the run under `JobRun`) reports the RUN's own members at
/// the top level of the task's result, and one whose answer is the job's document
/// reports it whole.
#[test]
fn a_sync_task_reports_the_document_the_service_carries_the_job_in() {
    for entry in RUN_A_JOB {
        // The pattern's second revision parses the job's own JSON text in place, so
        // both revisions are a row: the unwrapping is the same either way.
        for parsed_output in [false, true] {
            for (status, finished_work) in entry
                .succeeded
                .iter()
                .map(|status| (status, true))
                .chain(entry.failed.iter().map(|status| (status, false)))
            {
                let label = format!(
                    "{} reporting {status} with parsed_output={parsed_output}",
                    entry.starts
                );
                let answer = describe_answer(entry, status);
                let expected = value_at(&answer, entry.reports_at);
                let outcome = entry
                    .outcome_of(&answer, parsed_output)
                    .unwrap_or_else(|| panic!("case {label:?}: {status} must end the job"));
                let reported = match (outcome, finished_work) {
                    (Ok(result), true) => (result, expected.clone()),
                    // A failure's cause IS that same document, written in the
                    // convention a service integration's result is read in
                    // ([`task_result`]), because `cause` is a string a `Catch` reads.
                    (Err(cause), false) => (
                        serde_json::from_str::<Value>(&cause).unwrap_or_else(|_| {
                            panic!(
                                "case {label:?}: the cause must be the job's document, got \
                                 {cause:?}"
                            )
                        }),
                        task_result(&expected),
                    ),
                    (outcome, _) => panic!(
                        "case {label:?}: {status} must end the task the way the catalogue says, \
                         got {outcome:?}"
                    ),
                };
                let (reported, expected) = reported;
                assert_eq!(
                    reported, expected,
                    "case {label:?}: expected the document the answer carries the job in, got \
                     {reported:?}"
                );
                // And the job's own members are at the TOP level of what the task
                // reports, which is what a `ResultPath` or a `Catch` of such a task
                // addresses.
                assert_eq!(
                    reported.get("Marker").and_then(Value::as_str),
                    Some(BESIDE_THE_STATUS),
                    "case {label:?}: the job's own members must be at the top level of the \
                     reported document, got {reported:?}"
                );
            }
        }
    }
}

/// The fixed words a `Resource` asking for the pattern over an integration the
/// catalogue does not name is refused with, which is the only answer that says the
/// pattern itself was not performed.
const NOTHING_STARTED: &str = "no job was started";

/// How long a row waits for an execution to leave `RUNNING`.
const PATIENCE: Duration = Duration::from_secs(5);

/// A `.sync` `Resource` whose integration IS catalogued has its starting call MADE,
/// and only one whose integration is not says no job was started for it.
///
/// Driven end to end through `StartExecution`: a state machine whose single `Task`
/// state carries the `Resource`, started and then read back through
/// `DescribeExecution` and `GetExecutionHistory`. The catalogued rows are read off the
/// catalogue itself, so a later entry is covered by this table without being named in
/// it, and an execution whose job this emulator serves itself is observed PARKED at
/// it. The rows differ along what the rule generalises over: whether the integration
/// is catalogued, and — among those that are — whether the service its call goes to is
/// mounted beside this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_sync_resource_outside_the_catalogue_starts_nothing() {
    // Integrations of services this emulator makes a task's call to, written in the
    // `.sync` form, that the catalogue does NOT name: each is a request-response
    // integration, so there is no job for this emulator to report or stop.
    let uncatalogued = [
        "arn:aws:states:::dynamodb:putItem.sync",
        "arn:aws:states:::sqs:sendMessage.sync",
        "arn:aws:states:::ecs:runTask.sync",
    ];

    let emulator = Emulator::new();
    // The job the one integration this service serves ITSELF starts, so that row is
    // observed parked at a job that really exists.
    let child = create_machine(
        &emulator,
        "held-child",
        r#"{"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":3600,"End":true}}}"#,
    )
    .await;

    for (index, entry) in RUN_A_JOB.iter().enumerate() {
        let case = format!("the catalogued {}", entry.starts);
        let outcome = run_sync_task(
            &emulator,
            index,
            &format!("arn:aws:states:::{}.sync", entry.starts),
            &child,
        )
        .await;
        match outcome.get("status").and_then(Value::as_str) {
            // The call was made and answered with the job it started: the execution
            // is parked at it, with nothing claimed for the task yet.
            Some("RUNNING") => assert!(
                history_types(&emulator, &string(&outcome, "executionArn"))
                    .await
                    .iter()
                    .any(|event| event == EVENT_TASK_SUBMITTED),
                "case {case:?}: a parked execution must carry its submission, got {outcome:?}"
            ),
            // The call was MADE and could not be served here — the service it goes to
            // is not mounted beside this one — so the task failed on what THAT service
            // answered, which the error names it by, and never on a refusal of the
            // pattern.
            status => {
                let cause = outcome
                    .get("cause")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let error = outcome
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let named = format!("{}.", api_of(entry).sdk_id);
                assert_eq!(
                    status,
                    Some("FAILED"),
                    "case {case:?}: an execution whose job could not be started fails, got \
                     {outcome:?}"
                );
                assert!(
                    error.starts_with(&named) && !cause.contains(NOTHING_STARTED),
                    "case {case:?}: expected the failure to come from {named} itself, got \
                     {outcome:?}"
                );
            }
        }
    }

    for (index, resource) in uncatalogued.into_iter().enumerate() {
        let case = format!("the uncatalogued {resource}");
        let outcome = run_sync_task(&emulator, RUN_A_JOB.len() + index, resource, &child).await;
        assert_eq!(
            outcome.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected a terminal failure, got {outcome:?}"
        );
        assert!(
            string(&outcome, "cause").contains(NOTHING_STARTED),
            "case {case:?}: expected the refusal to say no job was started, got {outcome:?}"
        );
    }
}

/// Start an execution of a single `Task` state carrying `resource` and report what
/// `DescribeExecution` says once it has either parked or finished.
///
/// The `Parameters` carry the request members of every catalogued integration, so the
/// document the state builds is the one its own call needs; `child` is the state
/// machine the nested-execution integration starts.
async fn run_sync_task(
    emulator: &Emulator,
    index: usize,
    resource: &str,
    child: &str,
) -> Value {
    let definition = json!({
        "StartAt": "Job",
        "States": {
            "Job": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": {
                    "StateMachineArn": child,
                    "JobName": "a-job",
                    "QueryString": "SELECT 1",
                    "TableName": "a-table",
                    "QueueUrl": "a-queue",
                    "Cluster": "a-cluster"
                },
                "End": true
            }
        }
    })
    .to_string();
    let machine = create_machine(emulator, &format!("sync-boundary-{index}"), &definition).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": format!("boundary-{index}") }),
        )
        .await;
    let execution_arn = string(&started, "executionArn");
    let began = Instant::now();
    loop {
        let described = emulator
            .ok(
                "DescribeExecution",
                &json!({ "executionArn": execution_arn }),
            )
            .await;
        // A parked execution stays RUNNING for as long as its job does, so the wait is
        // for an answer either way: parked at a job, or finished.
        if described.get("status").and_then(Value::as_str) != Some("RUNNING")
            || began.elapsed() > PATIENCE
        {
            return described;
        }
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The types of the events `GetExecutionHistory` reports for `execution_arn`.
async fn history_types(emulator: &Emulator, execution_arn: &str) -> Vec<String> {
    history(emulator, execution_arn, json!({}))
        .await
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}
