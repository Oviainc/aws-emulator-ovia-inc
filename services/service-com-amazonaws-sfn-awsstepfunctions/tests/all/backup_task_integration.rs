// SPDX-License-Identifier: CC0-1.0

//! A `Task` state's AWS SDK integration with a service whose protocol is a REST one
//! and whose operations carry no `httpPayload` member: AWS Backup.
//!
//! The rule under test: a `Task` state whose `Resource` names an AWS SDK integration
//! (`arn:aws:states:::aws-sdk:<sdk name>:<action>`) performs that service's own API
//! operation — the document the state built IS the operation's request, wherever that
//! service's protocol puts each member (a path label, a query parameter, or the
//! document body a REST-JSON operation with no payload member carries), and the
//! operation's own response document IS the state's result. So a task's call is really
//! served by the other service's handler, its effect is readable afterwards through
//! that service's own API, and a call that service REFUSED is the task's own failure
//! named `<the service's SDK id>.<the exception its SDK resolves the refusal to>`.
//!
//! The recorded case this suite's issue was filed about — an EventBridge rule target
//! whose state machine has one `arn:aws:states:::aws-sdk:backup:startBackupJob` task —
//! is ONE row of the first table.
//!
//! Both services are mounted on ONE engine, the shape `core/router/src/lib.rs` mounts
//! them in, so every assertion is over what AWS Backup's own handlers really stored.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// The vault every row below backs up into. Inside the modelled `BackupVaultName`
/// pattern, `^[a-zA-Z0-9\-\_]{2,50}$`.
const VAULT: &str = "task-vault";

/// The protected resource a job is started for: a DynamoDB table's ARN, the form the
/// recorded orchestration passes as `ResourceArn`.
const RESOURCE: &str = "arn:aws:dynamodb:us-west-2:000000000000:table/orders";

/// A second protected resource, so a filtered listing is shown to filter rather than
/// to answer everything.
const OTHER_RESOURCE: &str = "arn:aws:dynamodb:us-west-2:000000000000:table/invoices";

/// The role a backup job is created with (`StartBackupJob$IamRoleArn`).
const BACKUP_ROLE: &str = "arn:aws:iam::000000000000:role/AWSBackupDefaultServiceRole";

/// Long enough that a started backup job has reached its terminal state: the job
/// settles one `state::JOB_SETTLE_AFTER` (500 ms) after it is started, which the
/// Backup crate's own suites wait out the same way.
const PAST_THE_JOB_SETTLE: Duration = Duration::from_millis(900);

/// Step Functions and AWS Backup over ONE engine, so the call a task makes is served
/// by the same store this suite reads back through AWS Backup's own API.
struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// One `awsJson1_0` Step Functions call, requiring a 200. The credential scope of
    /// the `Authorization` header is what routes the request and names the Region its
    /// ARNs carry.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/states/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("states.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let (status, parsed) = self.dispatch(request).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One `restJson1` AWS Backup call, as a client of that service addresses it: the
    /// method and path its operation binds, and the document body it carries.
    async fn backup(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/backup/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef"
        );
        let mut builder = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("backup.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let payload = body.map_or_else(Bytes::new, |document| {
            Bytes::from(serde_json::to_vec(document).expect("the request body serializes"))
        });
        let request = builder.body(payload).expect("a well-formed request");
        self.dispatch(request).await
    }

    /// The same, requiring a 200.
    async fn backup_ok(&self, method: &str, path: &str, body: Option<&Value>) -> Value {
        let (status, parsed) = self.backup(method, path, body).await;
        assert_eq!(
            status, 200,
            "case {method} {path}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Create a state machine with one `Task` state calling `resource` with
    /// `parameters`, start one execution of it over `input`, and describe it once the
    /// clock has made the outcome due.
    async fn run(&self, name: &str, resource: &str, parameters: &Value, input: &Value) -> Value {
        let definition = json!({
            "StartAt": "Work",
            "States": {
                "Work": {
                    "Type": "Task",
                    "Resource": resource,
                    "Parameters": parameters,
                    "End": true
                }
            }
        })
        .to_string();
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "once",
                    "input": input.to_string()
                }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        self.sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await
    }

    /// A vault of `name`, created through AWS Backup's own `CreateBackupVault`.
    async fn given_vault(&self, name: &str) {
        self.backup_ok("PUT", &format!("/backup-vaults/{name}"), Some(&json!({})))
            .await;
    }

    /// A backup job of `resource`, started through AWS Backup's own `StartBackupJob`,
    /// and what that call answered.
    async fn given_job(&self, resource: &str) -> Value {
        self.backup_ok(
            "PUT",
            "/backup-jobs",
            Some(&json!({
                "BackupVaultName": VAULT,
                "ResourceArn": resource,
                "IamRoleArn": BACKUP_ROLE
            })),
        )
        .await
    }
}

/// The result document a SUCCEEDED execution reports.
fn result(described: &Value, case: &str) -> Value {
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described}"
    );
    described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described}"))
}

/// One row of the table: a backup operation a `Task` state names, and the members of
/// that operation's own output its result must carry.
struct Row {
    case: &'static str,
    /// The state machine's name, which has to be unique in the account.
    machine: &'static str,
    action: &'static str,
    parameters: Value,
    reports: &'static [&'static str],
}

// ---------------------------------------------------------------------------
// The call is made, over every binding the service's protocol uses
// ---------------------------------------------------------------------------

/// A `Task` state's AWS SDK integration performs the AWS Backup operation its action
/// names, and the state's result is that operation's own response document.
///
/// The rows differ along what the rule generalises over: the ACTION — none of which
/// this emulator names in code — the METHOD it is made with (a `PUT` that writes, a
/// `GET` that reads), where each member of the request goes on the wire (the document
/// body, one path label, two of them with an ARN as the second, a query parameter),
/// and where the request's values come from (a path into the execution input, or a
/// literal in the definition).
#[tokio::test]
async fn a_task_state_calls_the_backup_operation_its_resource_names() {
    let chain = Chain::new();
    chain.given_vault(VAULT).await;
    let seeded = chain.given_job(RESOURCE).await;
    let job_id = string(&seeded, "BackupJobId");
    let point_arn = string(&seeded, "RecoveryPointArn");

    let rows = vec![
        // The recorded case: the vault, the resource and the role are taken from the
        // execution input, which is what a rule's delivery carries.
        Row {
            case: "startBackupJob, the recorded orchestration's own call",
            machine: "starts-a-backup",
            action: "startBackupJob",
            parameters: json!({
                "BackupVaultName.$": "$.vaultName",
                "ResourceArn.$": "$.resourceArn",
                "IamRoleArn.$": "$.iamRoleArn"
            }),
            reports: &["BackupJobId", "RecoveryPointArn", "CreationDate", "IsParent"],
        },
        // A `PUT` whose name is a path label and whose tags are the body.
        Row {
            case: "createBackupVault, a write whose name is in the path",
            machine: "creates-a-vault",
            action: "createBackupVault",
            parameters: json!({
                "BackupVaultName": "task-made-vault",
                "BackupVaultTags": { "made-by": "a-task" }
            }),
            reports: &["BackupVaultName", "BackupVaultArn", "CreationDate"],
        },
        // A read addressed by its path alone.
        Row {
            case: "describeBackupVault, a read addressed by its path",
            machine: "reads-a-vault",
            action: "describeBackupVault",
            parameters: json!({ "BackupVaultName": VAULT }),
            reports: &["BackupVaultName", "BackupVaultArn", "NumberOfRecoveryPoints"],
        },
        // The same, with the value the path carries taken from the input.
        Row {
            case: "describeBackupJob, a read whose id comes from the input",
            machine: "reads-a-job",
            action: "describeBackupJob",
            parameters: json!({ "BackupJobId.$": "$.jobId" }),
            reports: &["BackupJobId", "ResourceArn", "BackupVaultName", "State"],
        },
        // Two path labels, the second of them an ARN.
        Row {
            case: "describeRecoveryPoint, a read with an ARN in its path",
            machine: "reads-a-recovery-point",
            action: "describeRecoveryPoint",
            parameters: json!({
                "BackupVaultName": VAULT,
                "RecoveryPointArn.$": "$.recoveryPointArn"
            }),
            reports: &["RecoveryPointArn", "BackupVaultName", "ResourceArn", "Status"],
        },
        // A filter the operation binds to the QUERY rather than to a body.
        Row {
            case: "listBackupJobs, a read whose filter is a query parameter",
            machine: "lists-jobs",
            action: "listBackupJobs",
            parameters: json!({ "ByResourceArn.$": "$.resourceArn" }),
            reports: &["BackupJobs"],
        },
        Row {
            case: "listRecoveryPointsByBackupVault, a path and a query together",
            machine: "lists-recovery-points",
            action: "listRecoveryPointsByBackupVault",
            parameters: json!({ "BackupVaultName": VAULT, "MaxResults": 10 }),
            reports: &["RecoveryPoints"],
        },
    ];

    let input = json!({
        "vaultName": VAULT,
        "resourceArn": RESOURCE,
        "iamRoleArn": BACKUP_ROLE,
        "jobId": job_id,
        "recoveryPointArn": point_arn
    });

    for row in rows {
        let Row {
            case,
            machine,
            action,
            parameters,
            reports,
        } = row;
        let resource = format!("arn:aws:states:::aws-sdk:backup:{action}");
        let described = chain.run(machine, &resource, &parameters, &input).await;
        let reported = result(&described, case);
        for member in reports {
            assert!(
                reported.get(*member).is_some(),
                "case {case:?}: expected the result to carry {member:?}, got {reported}"
            );
        }
        // The AWS SDK integration family reports the operation's own members alone:
        // the SDK response envelope belongs to the optimized family.
        assert!(
            reported.get("SdkHttpMetadata").is_none(),
            "case {case:?}: the aws-sdk family reports no SDK envelope, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// The effect is really there, and the filters really filter
// ---------------------------------------------------------------------------

/// The job a `startBackupJob` task started is AWS Backup's own stored job: it is
/// readable afterwards through that service's `DescribeBackupJob`, it reaches a
/// non-failed state on the clock, and a listing filtered by the resource reports it
/// while one filtered by another resource does not.
#[tokio::test]
async fn the_job_a_backup_task_started_is_stored_by_backup_itself() {
    let chain = Chain::new();
    chain.given_vault(VAULT).await;

    let described = chain
        .run(
            "orchestrated-backup",
            "arn:aws:states:::aws-sdk:backup:startBackupJob",
            &json!({
                "BackupVaultName.$": "$.vaultName",
                "ResourceArn.$": "$.resourceArn",
                "IamRoleArn": BACKUP_ROLE
            }),
            &json!({ "vaultName": VAULT, "resourceArn": RESOURCE }),
        )
        .await;
    let reported = result(&described, "the task's own result");
    let job_id = string(&reported, "BackupJobId");

    // The far end: AWS Backup's own reader answers for the job the task started.
    let read = chain
        .backup_ok("GET", &format!("/backup-jobs/{job_id}"), None)
        .await;
    assert_eq!(
        read.get("ResourceArn").and_then(Value::as_str),
        Some(RESOURCE),
        "case \"the job the task started\": expected the resource the task named, got {read}"
    );
    assert_eq!(
        read.get("BackupVaultName").and_then(Value::as_str),
        Some(VAULT),
        "case \"the job the task started\": expected the vault the task named, got {read}"
    );
    assert_eq!(
        read.get("RecoveryPointArn").and_then(Value::as_str),
        reported.get("RecoveryPointArn").and_then(Value::as_str),
        "case \"the job the task started\": the recovery point the task reported is the \
         stored one, got {read}"
    );

    // The state the job reaches is the service's own, and it is not a failed one.
    std::thread::sleep(PAST_THE_JOB_SETTLE);
    let settled = chain
        .backup_ok("GET", &format!("/backup-jobs/{job_id}"), None)
        .await;
    let state = settled
        .get("State")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string();
    assert!(
        !["FAILED", "ABORTED", "EXPIRED"].contains(&state.as_str()),
        "case \"the job the task started\": expected a state that is not a failed one, got \
         {state:?} in {settled}"
    );

    // A second resource, backed up through the same task form, so the filtered
    // listings below are shown to filter.
    let other = chain
        .run(
            "orchestrated-backup-other",
            "arn:aws:states:::aws-sdk:backup:startBackupJob",
            &json!({
                "BackupVaultName": VAULT,
                "ResourceArn": OTHER_RESOURCE,
                "IamRoleArn": BACKUP_ROLE
            }),
            &json!({}),
        )
        .await;
    let other_id = string(&result(&other, "a second resource"), "BackupJobId");

    for (resource, expected) in [(RESOURCE, &job_id), (OTHER_RESOURCE, &other_id)] {
        let listed = chain
            .backup_ok(
                "GET",
                &format!("/backup-jobs?resourceArn={resource}"),
                None,
            )
            .await;
        let ids: Vec<String> = listed
            .get("BackupJobs")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {resource:?}: expected BackupJobs in {listed}"))
            .iter()
            .map(|job| string(job, "BackupJobId"))
            .collect();
        assert_eq!(
            ids,
            vec![expected.clone()],
            "case {resource:?}: expected exactly the job the task started for that resource, \
             got {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the service refused, and what no call is made for
// ---------------------------------------------------------------------------

/// One row: a call AWS Backup refuses, and the error name the task reports it under.
struct Refusal {
    case: &'static str,
    machine: &'static str,
    action: &'static str,
    parameters: Value,
    error: &'static str,
    /// A fragment the cause must carry, for a failure that is not a refusal of the
    /// call but work no call was made for.
    cause: Option<&'static str>,
}

/// A refused call is the task's own failure, named after the exception AWS Backup's
/// own SDK resolves the refusal to — and an operation of that service whose request
/// this emulator binds nowhere is still reported as work nothing was run for, never as
/// a result.
///
/// The rows differ in which modelled error the service answered, which of its
/// operations answered it, and whether a call was made at all.
#[tokio::test]
async fn a_refused_backup_call_fails_the_task_under_backups_own_exception() {
    let chain = Chain::new();
    chain.given_vault(VAULT).await;

    let rows = vec![
        Refusal {
            case: "a vault the account does not hold",
            machine: "backs-up-nowhere",
            action: "startBackupJob",
            parameters: json!({
                "BackupVaultName": "no-such-vault",
                "ResourceArn": RESOURCE,
                "IamRoleArn": BACKUP_ROLE
            }),
            // AWS Backup's model spells this error's shape with the `Exception` suffix
            // already, so the class is the shape name itself.
            error: "Backup.ResourceNotFoundException",
            cause: None,
        },
        Refusal {
            case: "a job id nobody holds",
            machine: "reads-no-job",
            action: "describeBackupJob",
            parameters: json!({ "BackupJobId": "00000000-0000-4000-8000-000000000000" }),
            error: "Backup.ResourceNotFoundException",
            cause: None,
        },
        Refusal {
            case: "a start window below the documented minimum",
            machine: "backs-up-too-soon",
            action: "startBackupJob",
            parameters: json!({
                "BackupVaultName": VAULT,
                "ResourceArn": RESOURCE,
                "IamRoleArn": BACKUP_ROLE,
                "StartWindowMinutes": 1
            }),
            // A value the service itself refuses, under another error of its model.
            error: "Backup.InvalidParameterValueException",
            cause: None,
        },
        Refusal {
            case: "an operation of the service whose request is bound nowhere",
            machine: "lists-tags",
            action: "listTags",
            parameters: json!({ "ResourceArn": RESOURCE }),
            // Nothing was run for it, which is the ASL runtime failure rather than a
            // refusal of a call that was made.
            error: "States.Runtime",
            cause: Some("does not bind to that service's REST protocol"),
        },
    ];

    for row in rows {
        let Refusal {
            case,
            machine,
            action,
            parameters,
            error,
            cause,
        } = row;
        let resource = format!("arn:aws:states:::aws-sdk:backup:{action}");
        let described = chain.run(machine, &resource, &parameters, &json!({})).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the refused call to fail the execution, got {described}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(error),
            "case {case:?}: expected error {error:?}, got {described}"
        );
        if let Some(fragment) = cause {
            let reported = described
                .get("cause")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                reported.contains(fragment),
                "case {case:?}: expected a cause carrying {fragment:?}, got {reported:?}"
            );
        }
    }
}
