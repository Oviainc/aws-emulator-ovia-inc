// SPDX-License-Identifier: CC0-1.0

//! `ListExecutions`: the executions of one workflow, read back over the wire.
//!
//! RULE: `ListExecutions` reports exactly the execution records the store holds for the
//! workflow it names — one `ListedExecution` per record, projected from that record
//! (`ExecutionId`, `InitialFileLocation`, `ServiceMetadata` and the `ExecutionStatus`
//! the record's custom-step callbacks make due), in `ExecutionId` order, paged through
//! the opaque `NextToken` every `List*` of this service hands out; it echoes the
//! `WorkflowId` it was asked about, reports a workflow that holds no execution as an
//! empty list, changes nothing a later caller reads (the operation is `@readonly`), and
//! refuses a `WorkflowId` the store does not hold with the modelled
//! `ResourceNotFoundException` and a `NextToken` it did not hand out with
//! `InvalidNextTokenException`.
//!
//! The rows differ along what the rule generalises over: how many executions a workflow
//! holds (none, one, several, more than a page), which optional members a record carries
//! (an S3 or an EFS file location, with and without the optional S3 members, session
//! details with and without a `SessionId`, neither container at all), the page bound
//! asked for, how many custom steps are pending and which outcome each one reported.
//!
//! No emulated data plane uploads a file, so nothing but a seed starts an execution: the
//! records are SEEDED into the store the handlers read
//! ([`crate::harness::Emulator::store`]), exactly as
//! [`crate::resource_resolution_refusals`] seeds the ones its callbacks resolve, while
//! the workflow itself and every callback go over the wire.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::ops::types::{
    EfsFileLocation, FileLocation, S3FileLocation, ServiceMetadata, UserDetails,
};
use service_com_amazonaws_transfer_transferservice::state as st;

use crate::harness::{drain_pages, member_of, Answer, Emulator, ACCOUNT, REGION};

/// The `ExecutionStatus` enum, in full (`smithy_query(query="shape",
/// shape="ExecutionStatus")`: `[IN_PROGRESS, COMPLETED, EXCEPTION,
/// HANDLING_EXCEPTION]`). Every status a listing reports must be one of these.
const EXECUTION_STATUSES: [&str; 4] = [
    "IN_PROGRESS",
    "COMPLETED",
    "EXCEPTION",
    "HANDLING_EXCEPTION",
];

/// The `CustomStepStatus` enum, in full (`smithy_query(query="shape",
/// shape="CustomStepStatus")`: `[SUCCESS, FAILURE]`).
const SUCCESS: &str = "SUCCESS";
const FAILURE: &str = "FAILURE";

/// A `ServerId` satisfying `UserDetails.ServerId`'s modelled `len 19..19` pattern
/// `^s-([0-9a-f]{17})$` — the server an upload that started a run arrived on.
const SESSION_SERVER_ID: &str = "s-0123456789abcdef0";

/// `ExecutionId` number `index`, satisfying the modelled `len 36..36` UUID pattern
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
///
/// `index` leads, so ascending indices are also ascending ids: the order a listing
/// pages in is then the order the records were seeded in, and the expectation is written
/// without re-sorting.
fn execution_id(index: usize) -> String {
    format!("{index:08x}-1111-4222-8333-{index:012x}")
}

/// One seeded execution record and the `ListedExecution` the listing must project from
/// it, as the suite's table holds them.
struct Row {
    case: &'static str,
    /// The callback tokens the record's custom steps are pending on.
    tokens: Vec<&'static str>,
    initial_file_location: Option<FileLocation>,
    service_metadata: Option<ServiceMetadata>,
    /// The entry the listing must answer, WITHOUT `ExecutionId` (which the seeder
    /// assigns) — a record's optional container that is absent must be absent here too.
    expected: Value,
}

/// The session details of an upload, as a record holds them: the server and user the
/// file arrived from, and the session when one is recorded.
///
/// The values satisfy `UserDetails`' modelled patterns — `UserName`
/// `^[\w][\w@.-]{2,99}$`, `ServerId` `^s-([0-9a-f]{17})$`, `SessionId` `^[\w-]*$`
/// (`len 3..32`).
fn session(user_name: &str, session_id: Option<&str>) -> ServiceMetadata {
    ServiceMetadata {
        user_details: UserDetails {
            user_name: user_name.to_string(),
            server_id: SESSION_SERVER_ID.to_string(),
            session_id: session_id.map(str::to_string),
        },
    }
}

/// The record shapes a listing has to project, one row per combination of the optional
/// members a record can carry and the number of custom steps pending on it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an S3 upload with session details and one pending custom step",
            tokens: vec!["step_one"],
            initial_file_location: Some(FileLocation {
                s3_file_location: Some(S3FileLocation {
                    bucket: Some("uploads-bucket".to_string()),
                    key: Some("inbound/report.csv".to_string()),
                    version_id: None,
                    etag: None,
                }),
                efs_file_location: None,
            }),
            service_metadata: Some(session("alice", Some("session-0001"))),
            expected: json!({
                "Status": "IN_PROGRESS",
                "InitialFileLocation": {
                    "S3FileLocation": {
                        "Bucket": "uploads-bucket",
                        "Key": "inbound/report.csv",
                    },
                },
                "ServiceMetadata": {
                    "UserDetails": {
                        "UserName": "alice",
                        "ServerId": SESSION_SERVER_ID,
                        "SessionId": "session-0001",
                    },
                },
            }),
        },
        Row {
            case: "an S3 upload carrying every optional S3 member, two pending steps",
            tokens: vec!["step_one", "step_two"],
            initial_file_location: Some(FileLocation {
                s3_file_location: Some(S3FileLocation {
                    bucket: Some("uploads-bucket".to_string()),
                    key: Some("inbound/archive.zip".to_string()),
                    version_id: Some("3HL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY+MTRCxf3vjVBH40Nr8X8gdRQBpUMLUo"
                        .to_string()),
                    etag: Some("\"9b2cf535f27731c974343645a3985328\"".to_string()),
                }),
                efs_file_location: None,
            }),
            service_metadata: Some(session("transfer.user-2", Some("sess_2"))),
            expected: json!({
                "Status": "IN_PROGRESS",
                "InitialFileLocation": {
                    "S3FileLocation": {
                        "Bucket": "uploads-bucket",
                        "Key": "inbound/archive.zip",
                        "VersionId": "3HL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY+MTRCxf3vjVBH40Nr8X8gdRQBpUMLUo",
                        "Etag": "\"9b2cf535f27731c974343645a3985328\"",
                    },
                },
                "ServiceMetadata": {
                    "UserDetails": {
                        "UserName": "transfer.user-2",
                        "ServerId": SESSION_SERVER_ID,
                        "SessionId": "sess_2",
                    },
                },
            }),
        },
        Row {
            case: "an EFS upload whose session carries no SessionId, no pending step",
            tokens: vec![],
            initial_file_location: Some(FileLocation {
                s3_file_location: None,
                efs_file_location: Some(EfsFileLocation {
                    file_system_id: Some("fs-01234567".to_string()),
                    path: Some("/inbound/report.csv".to_string()),
                }),
            }),
            service_metadata: Some(session("bob@example.com", None)),
            expected: json!({
                // A run with no custom step waits on nothing, so it is COMPLETED.
                "Status": "COMPLETED",
                "InitialFileLocation": {
                    "EfsFileLocation": {
                        "FileSystemId": "fs-01234567",
                        "Path": "/inbound/report.csv",
                    },
                },
                "ServiceMetadata": {
                    "UserDetails": {
                        "UserName": "bob@example.com",
                        "ServerId": SESSION_SERVER_ID,
                    },
                },
            }),
        },
        Row {
            case: "a record with neither a file location nor session details",
            tokens: vec!["a", "b", "c"],
            initial_file_location: None,
            service_metadata: None,
            expected: json!({"Status": "IN_PROGRESS"}),
        },
    ]
}

/// Create the workflow the executions below belong to, over the wire, and return its
/// minted `WorkflowId`.
///
/// Its one step is a `CUSTOM` step, so `SendWorkflowStepState` is a callback this
/// workflow can take. `CustomStepDetails.Target` satisfies `CustomStepTarget`'s modelled
/// pattern `^arn:[a-z-]+:lambda:.*$`.
async fn create_workflow(emulator: &Emulator) -> String {
    let answer = emulator
        .call(
            "CreateWorkflow",
            json!({
                "Steps": [{
                    "Type": "CUSTOM",
                    "CustomStepDetails": {
                        "Name": "callback-step",
                        "Target": format!(
                            "arn:aws:lambda:{REGION}:{ACCOUNT}:function:workflow-callback"
                        ),
                        "TimeoutSeconds": 120,
                    },
                }],
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreateWorkflow: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WorkflowId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkflow returned no WorkflowId: {}", answer.body))
        .to_string()
}

/// Seed one execution record for `workflow_id`, with a pending custom step per token.
fn seed_execution(
    emulator: &Emulator,
    workflow_id: &str,
    execution_id: &str,
    tokens: &[&str],
    initial_file_location: Option<FileLocation>,
    service_metadata: Option<ServiceMetadata>,
) {
    let state = emulator.store();
    let mut guard = state.lock();
    guard.workflow_executions.insert(
        (workflow_id.to_string(), execution_id.to_string()),
        st::WorkflowExecution {
            workflow_id: workflow_id.to_string(),
            execution_id: execution_id.to_string(),
            initial_file_location,
            service_metadata,
            step_states: tokens
                .iter()
                .map(|token| ((*token).to_string(), None))
                .collect(),
        },
    );
}

/// Seed `count` plain executions for `workflow_id`, returning their ids in the order
/// the listing must report them.
fn seed_executions(emulator: &Emulator, workflow_id: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let id = execution_id(index);
            seed_execution(emulator, workflow_id, &id, &["step_one"], None, None);
            id
        })
        .collect()
}

/// One `ListExecutions` call.
async fn list_executions(emulator: &Emulator, request: Value) -> Answer {
    emulator.call("ListExecutions", request).await
}

/// One `ListExecutions` call that must succeed, returning its body.
async fn listed(emulator: &Emulator, case: &str, request: Value) -> Value {
    let answer = list_executions(emulator, request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListExecutions 200 for {request}, got {} with body {}",
        answer.status, answer.body
    );
    answer.body
}

/// The `Executions` of one answer, as a list. `Executions` is modelled required, so it
/// is present even when the workflow holds none.
fn executions_of(case: &str, body: &Value) -> Vec<Value> {
    member_of(body, "Executions")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Executions is modelled required, got {body}"))
        .clone()
}

/// The `ExecutionId` of each listed entry, in the order the answer reported them.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    executions_of(case, body)
        .iter()
        .map(|entry| {
            member_of(entry, "ExecutionId")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry carries no ExecutionId: {entry}"))
                .to_string()
        })
        .collect()
}

/// The `Status` each listed execution reports, by `ExecutionId`, asserting as it goes
/// that every one is a modelled `ExecutionStatus` value.
fn listed_statuses(case: &str, body: &Value) -> BTreeMap<String, String> {
    executions_of(case, body)
        .iter()
        .map(|entry| {
            let id = member_of(entry, "ExecutionId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let status = member_of(entry, "Status")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: {id} reports no Status: {entry}"))
                .to_string();
            assert!(
                EXECUTION_STATUSES.contains(&status.as_str()),
                "case {case:?}: {id} reports {status:?}, which is not an ExecutionStatus value"
            );
            (id, status)
        })
        .collect()
}

/// One `SendWorkflowStepState` callback, over the wire, which must be accepted.
async fn callback(
    emulator: &Emulator,
    case: &str,
    workflow_id: &str,
    execution_id: &str,
    token: &str,
    status: &str,
) {
    let answer = emulator
        .call(
            "SendWorkflowStepState",
            json!({
                "WorkflowId": workflow_id,
                "ExecutionId": execution_id,
                "Token": token,
                "Status": status,
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the {token} callback ({status}) to be accepted, got {} with \
         body {}",
        answer.status, answer.body
    );
}

/// One stored execution record as a snapshot holds it: its `(WorkflowId, ExecutionId)`
/// key and the outcome recorded for each of its custom steps.
type ExecutionSnapshot = ((String, String), BTreeMap<String, Option<String>>);

/// The whole store's execution records — the snapshot a read must leave untouched.
fn stored_executions(emulator: &Emulator) -> Vec<ExecutionSnapshot> {
    let state = emulator.store();
    let guard = state.lock();
    guard
        .workflow_executions
        .iter()
        .map(|(key, execution)| (key.clone(), execution.step_states.clone()))
        .collect()
}

/// Each record the store holds for the workflow is projected into one listed execution,
/// with exactly the members the record carries — and no execution of another workflow is
/// ever listed.
#[tokio::test]
async fn list_executions_projects_each_record_the_workflow_holds() {
    let emulator = Emulator::new();

    // Three workflows, so "the executions of this workflow" is not "every execution
    // stored": one holds the whole table, one holds a single record, one holds none.
    let populated = create_workflow(&emulator).await;
    let single = create_workflow(&emulator).await;
    let empty = create_workflow(&emulator).await;

    let table = rows();
    let mut expected_entries: Vec<(&str, Value)> = Vec::new();
    for (index, row) in table.iter().enumerate() {
        let id = execution_id(index);
        seed_execution(
            &emulator,
            &populated,
            &id,
            &row.tokens,
            row.initial_file_location.clone(),
            row.service_metadata.clone(),
        );
        let mut entry = row.expected.clone();
        entry["ExecutionId"] = json!(id);
        expected_entries.push((row.case, entry));
    }

    // The single-record workflow gets the LAST row, so it is not the first table entry
    // that a listing keyed to insertion order would happen to answer.
    let lone_id = execution_id(table.len());
    let lone_row = table.last().expect("the table has rows");
    seed_execution(
        &emulator,
        &single,
        &lone_id,
        &lone_row.tokens,
        lone_row.initial_file_location.clone(),
        lone_row.service_metadata.clone(),
    );

    let before = stored_executions(&emulator);

    let case = "the workflow holding the whole table";
    let body = listed(&emulator, case, json!({"WorkflowId": populated})).await;
    assert_eq!(
        member_of(&body, "WorkflowId").as_str(),
        Some(populated.as_str()),
        "case {case:?}: expected the WorkflowId echoed, got {body}"
    );
    assert!(
        member_of(&body, "NextToken").is_null(),
        "case {case:?}: every record fits one page, so no NextToken is due, got {body}"
    );
    let listed_entries = executions_of(case, &body);
    assert_eq!(
        listed_entries.len(),
        expected_entries.len(),
        "case {case:?}: expected {} entries, got {listed_entries:?}",
        expected_entries.len()
    );
    for (entry, (row_case, expected)) in listed_entries.iter().zip(expected_entries.iter()) {
        assert_eq!(
            entry, expected,
            "case {row_case:?}: expected the entry {expected}, got {entry}"
        );
    }

    let case = "the workflow holding one record";
    let body = listed(&emulator, case, json!({"WorkflowId": single})).await;
    let mut expected = lone_row.expected.clone();
    expected["ExecutionId"] = json!(lone_id);
    assert_eq!(
        executions_of(case, &body),
        vec![expected.clone()],
        "case {case:?}: expected exactly its own record, got {body}"
    );

    let case = "the workflow holding none";
    let body = listed(&emulator, case, json!({"WorkflowId": empty})).await;
    assert_eq!(
        executions_of(case, &body),
        Vec::<Value>::new(),
        "case {case:?}: expected an empty list, got {body}"
    );
    assert!(
        member_of(&body, "NextToken").is_null(),
        "case {case:?}: an empty listing hands out no NextToken, got {body}"
    );

    // The operation is `@readonly`: three listings later, every record is as it was, and
    // a repeated call answers the same thing.
    assert_eq!(
        stored_executions(&emulator),
        before,
        "listing must not change what a later caller reads"
    );
    let case = "the populated workflow, read a second time";
    let repeat = listed(&emulator, case, json!({"WorkflowId": populated})).await;
    let entries: Vec<Value> = expected_entries
        .iter()
        .map(|(_, entry)| entry.clone())
        .collect();
    assert_eq!(
        executions_of(case, &repeat),
        entries,
        "case {case:?}: a second read answers the same listing"
    );
}

/// The listing pages in `ExecutionId` order under every page bound: no page is longer
/// than the caller's `MaxResults`, the pages concatenate to the whole set exactly once,
/// and a `NextToken` comes back only while a further page remains.
#[tokio::test]
async fn list_executions_pages_in_execution_id_order() {
    let emulator = Emulator::new();
    let workflow_id = create_workflow(&emulator).await;

    // Seven records, so a page bound can fall short of, divide, equal and exceed the set.
    let ids = seed_executions(&emulator, &workflow_id, 7);
    let base = json!({"WorkflowId": workflow_id});

    // `MaxResults` is modelled `range 1..1000`: no bound, each published bound, and
    // values between them.
    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(1000)] {
        let case = format!("MaxResults {max_results:?}");
        let (page_sizes, keys) = drain_pages(
            &emulator,
            "ListExecutions",
            &base,
            "Executions",
            "ExecutionId",
            max_results,
        )
        .await;
        assert_eq!(
            keys, ids,
            "case {case:?}: expected every execution exactly once in ExecutionId order, \
             got {keys:?}"
        );
        if let Some(limit) = max_results {
            let limit = usize::try_from(limit).expect("the page bound is positive");
            assert!(
                page_sizes.iter().all(|size| *size <= limit),
                "case {case:?}: no page may exceed the bound, got {page_sizes:?}"
            );
            let expected_pages = ids.len().div_ceil(limit).max(1);
            assert_eq!(
                page_sizes.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} pages, got {page_sizes:?}"
            );
        } else {
            assert_eq!(
                page_sizes,
                vec![ids.len()],
                "case {case:?}: an unbounded call answers the whole set in one page"
            );
        }
    }

    // A token the service DID hand out resumes right after the page it ended: the first
    // page of two, then the rest.
    let case = "resuming from the token of the first page";
    let first = listed(
        &emulator,
        case,
        json!({"WorkflowId": workflow_id, "MaxResults": 2}),
    )
    .await;
    let token = member_of(&first, "NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken, got {first}"))
        .to_string();
    let rest = listed(
        &emulator,
        case,
        json!({"WorkflowId": workflow_id, "NextToken": token}),
    )
    .await;
    assert_eq!(
        listed_ids(case, &rest),
        ids[2..].to_vec(),
        "case {case:?}: expected the records after the first page, got {rest}"
    );
}

/// One row of the status table: the custom steps a seeded record is pending on, the
/// callbacks reported against it, and the `ExecutionStatus` the listing must then report.
struct StatusRow {
    case: &'static str,
    tokens: Vec<&'static str>,
    callbacks: Vec<(&'static str, &'static str)>,
    expected: &'static str,
}

/// Each listed execution reports the `ExecutionStatus` its OWN recorded custom-step
/// callbacks make due: a step that reported `FAILURE` is an exception, a record whose
/// every step has reported `SUCCESS` is completed (vacuously so for a run with no custom
/// step, which waits on nothing), and one still waiting on a step is in progress.
#[tokio::test]
async fn list_executions_reports_the_status_the_recorded_callbacks_make_due() {
    let emulator = Emulator::new();
    let workflow_id = create_workflow(&emulator).await;

    // The rows differ along how many steps are pending and which outcome each reported.
    let table: Vec<StatusRow> = vec![
        StatusRow {
            case: "no custom step is pending, so the run waits on nothing",
            tokens: vec![],
            callbacks: vec![],
            expected: "COMPLETED",
        },
        StatusRow {
            case: "one step pending, nothing reported yet",
            tokens: vec!["step_one"],
            callbacks: vec![],
            expected: "IN_PROGRESS",
        },
        StatusRow {
            case: "two steps, only the first reported SUCCESS",
            tokens: vec!["step_one", "step_two"],
            callbacks: vec![("step_one", SUCCESS)],
            expected: "IN_PROGRESS",
        },
        StatusRow {
            case: "every step reported SUCCESS",
            tokens: vec!["step_one", "step_two"],
            callbacks: vec![("step_one", SUCCESS), ("step_two", SUCCESS)],
            expected: "COMPLETED",
        },
        StatusRow {
            case: "the only step reported FAILURE",
            tokens: vec!["step_one"],
            callbacks: vec![("step_one", FAILURE)],
            expected: "EXCEPTION",
        },
        StatusRow {
            case: "one step succeeded and another failed",
            tokens: vec!["step_one", "step_two"],
            callbacks: vec![("step_one", SUCCESS), ("step_two", FAILURE)],
            expected: "EXCEPTION",
        },
        StatusRow {
            case: "a step failed while another is still pending",
            tokens: vec!["step_one", "step_two", "step_three"],
            callbacks: vec![("step_two", FAILURE)],
            expected: "EXCEPTION",
        },
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (index, row) in table.iter().enumerate() {
        let id = execution_id(index);
        seed_execution(&emulator, &workflow_id, &id, &row.tokens, None, None);
        for (token, status) in &row.callbacks {
            callback(&emulator, row.case, &workflow_id, &id, token, status).await;
        }
        expected.insert(id, row.expected.to_string());
    }

    let listing = "the statuses the callbacks made due";
    let body = listed(&emulator, listing, json!({"WorkflowId": workflow_id})).await;
    let reported = listed_statuses(listing, &body);
    for (index, row) in table.iter().enumerate() {
        let id = execution_id(index);
        let case = row.case;
        let want = row.expected;
        assert_eq!(
            reported.get(&id).map(String::as_str),
            Some(want),
            "case {case:?}: expected {id} to report {want}, got {reported:?}"
        );
    }
    assert_eq!(
        reported, expected,
        "every seeded record must be listed with the status its callbacks make due"
    );
}

/// A `WorkflowId` the store does not hold is refused as not found, whatever paging the
/// request carries — including a workflow that WAS created and has since been deleted.
#[tokio::test]
async fn list_executions_cannot_find_a_workflow_the_store_does_not_hold() {
    let emulator = Emulator::new();

    // One real workflow with a real execution, so the refusals below are not "nothing is
    // stored".
    let live = create_workflow(&emulator).await;
    seed_executions(&emulator, &live, 1);

    // A workflow this account DID have: created over the wire, then deleted.
    let deleted = create_workflow(&emulator).await;
    let answer = emulator
        .call("DeleteWorkflow", json!({"WorkflowId": deleted}))
        .await;
    assert_eq!(
        answer.status, 200,
        "DeleteWorkflow: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    // `WorkflowId`s satisfying the modelled `len 19..19` pattern `^w-([a-z0-9]{17})$`:
    // the low end of the id space, one of letters, the high end — none ever minted.
    let unminted = [
        "w-00000000000000000".to_string(),
        "w-abcdefghijklmnopq".to_string(),
        "w-zzzzzzzzzzzzzzzzz".to_string(),
        deleted,
    ];

    // The paging members must not change the answer: an unresolvable workflow is
    // refused before they are read.
    let pagings: [(&str, Value); 3] = [
        ("no paging", json!({})),
        ("a page bound", json!({"MaxResults": 5})),
        (
            // A well-formed token (the codec `st::encode_page_token` uses), not one a
            // listing of this workflow handed out — the workflow is refused before the
            // token is read, so the two are the same request here.
            "a well-formed token",
            json!({"NextToken": ::hex::encode(execution_id(0))}),
        ),
    ];

    for workflow_id in &unminted {
        for (paging_case, paging) in &pagings {
            let case = format!("ListExecutions on {workflow_id} with {paging_case}");
            let mut request = json!({"WorkflowId": workflow_id});
            for (member, value) in paging.as_object().expect("the paging row is an object") {
                request[member] = value.clone();
            }
            let answer = list_executions(&emulator, request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            assert_eq!(
                member_of(&answer.body, "ResourceType").as_str(),
                Some(st::RESOURCE_TYPE_WORKFLOW),
                "case {case:?}: expected the workflow ResourceType, got {}",
                answer.body
            );
            assert_eq!(
                member_of(&answer.body, "Resource").as_str(),
                Some(workflow_id.as_str()),
                "case {case:?}: expected the Resource to name the request's id, got {}",
                answer.body
            );
        }
    }

    // The live workflow still lists its own execution, so the refusals above are about
    // the id named and nothing else.
    let case = "the live workflow";
    let body = listed(&emulator, case, json!({"WorkflowId": live})).await;
    assert_eq!(
        listed_ids(case, &body),
        vec![execution_id(0)],
        "case {case:?}: expected its own execution, got {body}"
    );
}

/// A `NextToken` this service did not hand out resumes nothing, so it is refused with
/// the modelled `InvalidNextTokenException` rather than silently restarting the listing.
#[tokio::test]
async fn list_executions_refuses_a_next_token_it_did_not_hand_out() {
    let emulator = Emulator::new();
    let workflow_id = create_workflow(&emulator).await;
    seed_executions(&emulator, &workflow_id, 3);

    // Rows differ along HOW the token is malformed: not hex at all, odd-length hex, hex
    // of bytes that are not text, and a whole JSON cursor from some other service.
    let tokens = [
        "not-a-token",
        "zzzz",
        "0",
        "deadbeef",
        "{\"position\": 2}",
        " ",
    ];

    for token in tokens {
        let case = format!("NextToken {token:?}");
        let answer = list_executions(
            &emulator,
            json!({"WorkflowId": workflow_id, "NextToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {}",
            answer.body
        );
    }
}
