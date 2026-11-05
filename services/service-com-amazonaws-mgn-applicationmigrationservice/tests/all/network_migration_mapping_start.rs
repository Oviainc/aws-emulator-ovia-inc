// SPDX-License-Identifier: CC0-1.0

//! `StartNetworkMigrationMapping`, end to end over HTTP.
//!
//! The rule under test: the mapping run belongs to ONE execution of ONE network
//! migration definition of the caller's account — a start records a mapping job under
//! that execution, moves the execution onto the mapping stage and answers the job's
//! minted `jobID`; a definition or an execution the account holds no record of is
//! refused with 404 `ResourceNotFoundException` and records nothing; and, the operation
//! being `@idempotent` with no idempotency token, the same request again while that job
//! is running answers the same job, while a start naming another
//! `securityGroupMappingStrategy` on an execution whose job is still running is refused
//! with 409 `ConflictException`.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and the started
//! mapping run is read back through `ListNetworkMigrationExecutions` — the implemented
//! reader of the execution the run belongs to. The store is reached directly only to
//! seed a precondition no sequence of calls can reach (another account's records, a job
//! whose work the clock has already finished), the way the other network-migration
//! suites do.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::ops::types::{
    TargetNetwork, TargetS3Configuration,
};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    NetworkMigrationDefinition, NetworkMigrationDefinitionConfiguration, NetworkMigrationExecution,
};

use crate::application_lifecycle::{error_code, Mgn, OTHER_ACCOUNT, REGION};

/// The modelled `http.uri` of `CreateNetworkMigrationDefinition`
/// (`smithy_query(query="operation",operation="CreateNetworkMigrationDefinition")`),
/// without the leading slash the harness adds.
const CREATE_DEFINITION: &str = "network-migration/CreateNetworkMigrationDefinition";

/// The modelled `http.uri` of `ListNetworkMigrationExecutions`, likewise.
const LIST_EXECUTIONS: &str = "network-migration/ListNetworkMigrationExecutions";

/// The modelled `http.uri` of `StartNetworkMigrationMapping`, likewise.
const START_MAPPING: &str = "network-migration/StartNetworkMigrationMapping";

/// The model's `SecurityGroupMappingStrategy` values, in the order the model lists them.
/// Source: `smithy_query(query="shape",shape="SecurityGroupMappingStrategy")` →
/// `values`.
const STRATEGIES: [&str; 3] = ["MAP", "SKIP", "MAP_DHCP"];

/// The `ExecutionStatus` an execution carries once a stage of it is running. Source:
/// `smithy_query(query="shape",shape="ExecutionStatus")` → `values`.
const EXECUTION_STATUS_STARTED: &str = "STARTED";

/// The `ExecutionStage` the mapping run belongs to. Source:
/// `smithy_query(query="shape",shape="ExecutionStage")` → `values`.
const EXECUTION_STAGE_MAPPING: &str = "MAPPING";

/// The `ExecutionStageActivity` of a mapping run. Source:
/// `smithy_query(query="shape",shape="ExecutionStageActivity")` → `values`.
const EXECUTION_ACTIVITY_MAPPING: &str = "MAPPING";

/// The `NetworkMigrationJobStatus` values a mapping job moves through. Source:
/// `smithy_query(query="shape",shape="NetworkMigrationJobStatus")` → `values`
/// (`PENDING`, `STARTED`, `SUCCEEDED`, `FAILED`).
const JOB_STATUS_STARTED: &str = "STARTED";
const JOB_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The group lengths of the modelled `NetworkMigrationJobID` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (length 36..36),
/// which the `NetworkMigrationExecutionID` pattern shares.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `id` is in the modelled `NetworkMigrationJobID` shape.
fn is_uuid_shaped(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    id.len() == 36
        && groups.len() == UUID_GROUPS.len()
        && groups
            .iter()
            .zip(UUID_GROUPS)
            .all(|(group, length)| {
                group.len() == length
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
}

/// An id in the modelled `NetworkMigrationJobID`/`NetworkMigrationExecutionID` shape
/// derived from `seed`, for naming a resource the store holds no record of.
fn uuid_shaped(seed: u32) -> String {
    format!("{seed:08x}-0000-4000-8000-{seed:012x}")
}

/// A minimal valid `CreateNetworkMigrationDefinition` body: the required members only
/// (`smithy_query(query="closure",operation="CreateNetworkMigrationDefinition")`).
fn definition_request(name: &str) -> Value {
    json!({
        "name": name,
        "targetS3Configuration": {
            "s3Bucket": "migration-artifacts",
            "s3BucketOwner": crate::application_lifecycle::EMULATOR_ACCOUNT,
        },
        "targetNetwork": {"topology": "ISOLATED_VPC"},
    })
}

/// `CreateNetworkMigrationDefinition`, answering the minted `networkMigrationDefinitionID`.
async fn create_definition(mgn: &Mgn, case: &str, name: &str) -> String {
    let (status, created) = mgn.call(CREATE_DEFINITION, &definition_request(name)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateNetworkMigrationDefinition expected 201, got {status} with {created:?}"
    );
    created["networkMigrationDefinitionID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no networkMigrationDefinitionID in {created:?}"))
        .to_string()
}

/// Every execution `ListNetworkMigrationExecutions` reports for `definition_id`.
async fn executions(mgn: &Mgn, case: &str, definition_id: &str) -> Vec<Value> {
    let (status, listed) = mgn
        .call(
            LIST_EXECUTIONS,
            &json!({"networkMigrationDefinitionID": definition_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListNetworkMigrationExecutions expected 200, got {status} with {listed:?}"
    );
    listed["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no items array in {listed:?}"))
        .clone()
}

/// The execution a created definition is staged with: the one execution the listing
/// reports for it.
async fn staged_execution(mgn: &Mgn, case: &str, definition_id: &str) -> Value {
    let items = executions(mgn, case, definition_id).await;
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: a created definition is staged exactly one execution, got {items:?}"
    );
    items[0].clone()
}

fn execution_id_of(case: &str, execution: &Value) -> String {
    execution["networkMigrationExecutionID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no networkMigrationExecutionID in {execution:?}"))
        .to_string()
}

/// `StartNetworkMigrationMapping` for `definition_id`/`execution_id`, under `strategy`
/// when one is named.
async fn start_mapping(
    mgn: &Mgn,
    definition_id: &str,
    execution_id: &str,
    strategy: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "networkMigrationDefinitionID": definition_id,
        "networkMigrationExecutionID": execution_id,
    });
    if let Some(strategy) = strategy {
        request["securityGroupMappingStrategy"] = json!(strategy);
    }
    mgn.call(START_MAPPING, &request).await
}

fn job_id_of(case: &str, body: &Value) -> String {
    body["jobID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no jobID in {body:?}"))
        .to_string()
}

/// The class a mapping start spans: no `securityGroupMappingStrategy` at all, and each
/// value the model declares for it — the recorded `MAP` call is one row.
fn strategy_cases() -> Vec<(String, Option<&'static str>)> {
    let mut cases = vec![("no strategy named".to_string(), None)];
    for strategy in STRATEGIES {
        cases.push((format!("strategy {strategy}"), Some(strategy)));
    }
    cases
}

/// Every start mints a job, persists it under the execution it runs, and moves that
/// execution onto the mapping stage — and the same request again, while that job is
/// running, answers the job it started rather than a second one.
#[tokio::test]
async fn a_mapping_start_records_a_job_and_moves_its_execution_onto_the_mapping_stage() {
    // Neither operation models an `accountID` member or an
    // `UninitializedAccountException`, so no `InitializeService` runs here.
    let mgn = Mgn::new();
    let mut minted = Vec::new();

    for (index, (case, strategy)) in strategy_cases().into_iter().enumerate() {
        let case = case.as_str();
        let definition_id = create_definition(&mgn, case, &format!("mapping-plan-{index}")).await;
        let staged = staged_execution(&mgn, case, &definition_id).await;
        let execution_id = execution_id_of(case, &staged);
        let staged_created_at = staged["createdAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: createdAt must be an epoch-seconds number, got {staged:?}")
        });

        let (status, started) = start_mapping(&mgn, &definition_id, &execution_id, strategy).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StartNetworkMigrationMapping expected 202, got {status} with {started:?}"
        );
        let job_id = job_id_of(case, &started);
        assert!(
            is_uuid_shaped(&job_id),
            "case {case:?}: jobID must match ^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$, got {job_id:?}"
        );

        // The job is state, not a value computed for the answer: it is stored under the
        // execution it runs, with the strategy the request named.
        {
            let store = mgn.store();
            let guard = store.lock();
            let jobs = guard.network_migration_mapping_jobs_of(
                crate::application_lifecycle::EMULATOR_ACCOUNT,
                &definition_id,
                &execution_id,
            );
            assert_eq!(
                jobs.len(),
                1,
                "case {case:?}: exactly one mapping job must be recorded for the execution, got {jobs:?}"
            );
            let job = jobs[0];
            assert_eq!(
                job.job_id, job_id,
                "case {case:?}: the stored job must be the one the answer named, got {:?}",
                job.job_id
            );
            assert_eq!(
                job.security_group_mapping_strategy.as_deref(),
                strategy,
                "case {case:?}: the stored strategy must be the request's, got {:?}",
                job.security_group_mapping_strategy
            );
            // The work the job runs is owed at a later instant, so a caller reading the
            // job straight after this call still sees it running.
            assert!(
                job.completes_at > job.created_at,
                "case {case:?}: the mapping work must take a non-zero span, got {:?} to {:?}",
                job.created_at,
                job.completes_at
            );
            assert!(
                job.is_running(job.created_at),
                "case {case:?}: the job must be running at the instant it started"
            );
            assert_eq!(
                job.status(job.created_at),
                JOB_STATUS_STARTED,
                "case {case:?}: a running job's status must be STARTED, got {:?}",
                job.status(job.created_at)
            );
            assert_eq!(
                job.ended_at(job.created_at),
                None,
                "case {case:?}: a running job has not ended, got {:?}",
                job.ended_at(job.created_at)
            );
            assert!(
                !job.is_running(job.completes_at),
                "case {case:?}: the job must have finished once the clock reaches {:?}",
                job.completes_at
            );
            assert_eq!(
                job.status(job.completes_at),
                JOB_STATUS_SUCCEEDED,
                "case {case:?}: a finished job's status must be SUCCEEDED, got {:?}",
                job.status(job.completes_at)
            );
            assert_eq!(
                job.ended_at(job.completes_at),
                Some(job.completes_at),
                "case {case:?}: a finished job ended when its work was due, got {:?}",
                job.ended_at(job.completes_at)
            );
        }

        // Round trip: the execution the run belongs to now reports the stage and the
        // activity this call started, through the implemented reader.
        let listed = staged_execution(&mgn, case, &definition_id).await;
        assert_eq!(
            listed["networkMigrationExecutionID"].as_str(),
            Some(execution_id.as_str()),
            "case {case:?}: the listing must report the same execution, got {listed:?}"
        );
        for (member, expected) in [
            ("status", EXECUTION_STATUS_STARTED),
            ("stage", EXECUTION_STAGE_MAPPING),
            ("activity", EXECUTION_ACTIVITY_MAPPING),
        ] {
            assert_eq!(
                listed[member].as_str(),
                Some(expected),
                "case {case:?}: {member} must be {expected:?} once the mapping run started, got {:?}",
                listed[member]
            );
        }
        let updated_at = listed["updatedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: updatedAt must be an epoch-seconds number, got {listed:?}")
        });
        assert!(
            updated_at >= staged_created_at,
            "case {case:?}: updatedAt must not precede the execution's createdAt, got {updated_at} against {staged_created_at}"
        );

        // A read changes nothing a later caller sees.
        let again = staged_execution(&mgn, case, &definition_id).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a second listing must answer the same execution, got {again:?}"
        );

        // `@idempotent` with no token: the same request again, while that job runs, is
        // the retry of the call that started it.
        let (status, retried) = start_mapping(&mgn, &definition_id, &execution_id, strategy).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the retry expected 202, got {status} with {retried:?}"
        );
        assert_eq!(
            job_id_of(case, &retried),
            job_id,
            "case {case:?}: a retry must answer the job the first call started, got {retried:?}"
        );
        assert_eq!(
            mgn.store().lock().network_migration_mapping_jobs.len(),
            index + 1,
            "case {case:?}: a retry must not record a second job"
        );

        minted.push(job_id);
    }

    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every start mints a distinct jobID, got {minted:?}"
    );
}

/// One row of the refusal table: which resource the request names, and what the service
/// must answer for it.
struct RefusalCase {
    case: &'static str,
    definition_id: String,
    execution_id: String,
    resource_type: &'static str,
    resource_id: String,
}

/// A start that names a definition or an execution the account holds no record of is
/// refused with 404 `ResourceNotFoundException`, naming the kind and the id that was
/// missing, and records nothing.
#[tokio::test]
async fn a_start_naming_no_recorded_definition_or_execution_is_refused() {
    let mgn = Mgn::new();

    // Two definitions of this account, each staged its own execution, so the table can
    // name an execution that exists but belongs to another definition.
    let first = create_definition(&mgn, "first definition", "mapping-plan-a").await;
    let second = create_definition(&mgn, "second definition", "mapping-plan-b").await;
    let first_execution = execution_id_of(
        "first definition",
        &staged_execution(&mgn, "first definition", &first).await,
    );
    let second_execution = execution_id_of(
        "second definition",
        &staged_execution(&mgn, "second definition", &second).await,
    );

    // A definition and an execution of ANOTHER account: a precondition no sequence of
    // this account's calls can reach.
    let foreign_definition = "nmd-aaaaaaaaaaaaaaaaa";
    let foreign_execution = uuid_shaped(0x5eed);
    {
        let store = mgn.store();
        let mut guard = store.lock();
        let stored = guard.insert_network_migration_definition(NetworkMigrationDefinition {
            arn: format!(
                "arn:aws:mgn:{REGION}:{OTHER_ACCOUNT}:network-migration-definition/{foreign_definition}"
            ),
            network_migration_definition_id: foreign_definition.to_string(),
            account_id: OTHER_ACCOUNT.to_string(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            configuration: NetworkMigrationDefinitionConfiguration {
                name: "other-account-plan".to_string(),
                target_s3_configuration: TargetS3Configuration {
                    s3_bucket: "migration-artifacts".to_string(),
                    s3_bucket_owner: OTHER_ACCOUNT.to_string(),
                },
                target_network: TargetNetwork {
                    topology: "ISOLATED_VPC".to_string(),
                    ..TargetNetwork::default()
                },
                ..NetworkMigrationDefinitionConfiguration::default()
            },
        });
        assert!(stored, "seeding another account's definition must store it");
        let stored = guard.insert_network_migration_execution(NetworkMigrationExecution {
            network_migration_execution_id: foreign_execution.clone(),
            network_migration_definition_id: foreign_definition.to_string(),
            account_id: OTHER_ACCOUNT.to_string(),
            created_at: Some(SystemTime::now()),
            updated_at: Some(SystemTime::now()),
            ..NetworkMigrationExecution::default()
        });
        assert!(stored, "seeding another account's execution must store it");
    }

    let unknown_definition = "nmd-zzzzzzzzzzzzzzzzz";
    let unknown_execution = uuid_shaped(0xfeed);
    let cases = vec![
        RefusalCase {
            case: "a definition no account holds",
            definition_id: unknown_definition.to_string(),
            execution_id: first_execution.clone(),
            resource_type: "NetworkMigrationDefinition",
            resource_id: unknown_definition.to_string(),
        },
        RefusalCase {
            case: "a definition of another account",
            definition_id: foreign_definition.to_string(),
            execution_id: foreign_execution.clone(),
            resource_type: "NetworkMigrationDefinition",
            resource_id: foreign_definition.to_string(),
        },
        RefusalCase {
            case: "an execution no definition holds",
            definition_id: first.clone(),
            execution_id: unknown_execution.clone(),
            resource_type: "NetworkMigrationExecution",
            resource_id: unknown_execution.clone(),
        },
        RefusalCase {
            case: "an execution of another definition of this account",
            definition_id: first.clone(),
            execution_id: second_execution.clone(),
            resource_type: "NetworkMigrationExecution",
            resource_id: second_execution.clone(),
        },
        RefusalCase {
            case: "an execution of another account under this account's definition",
            definition_id: second.clone(),
            execution_id: foreign_execution.clone(),
            resource_type: "NetworkMigrationExecution",
            resource_id: foreign_execution.clone(),
        },
    ];

    for refusal in cases {
        let case = refusal.case;
        for strategy in [None, Some(STRATEGIES[0])] {
            let (status, body) = start_mapping(
                &mgn,
                &refusal.definition_id,
                &refusal.execution_id,
                strategy,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?} (strategy {strategy:?}): expected 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} (strategy {strategy:?}): expected ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(refusal.resource_type),
                "case {case:?} (strategy {strategy:?}): resourceType must name the missing kind, got {:?}",
                body["resourceType"]
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(refusal.resource_id.as_str()),
                "case {case:?} (strategy {strategy:?}): resourceId must name the id the request asked for, got {:?}",
                body["resourceId"]
            );
            assert_eq!(
                body["message"].as_str(),
                Some(
                    format!(
                        "{} with Id: '{}' was not found",
                        refusal.resource_type, refusal.resource_id
                    )
                    .as_str()
                ),
                "case {case:?} (strategy {strategy:?}): message must name the kind and the id, got {:?}",
                body["message"]
            );
        }
    }

    // A refusal starts nothing: no job is recorded, and every execution still waits
    // where its definition's create staged it.
    assert!(
        mgn.store()
            .lock()
            .network_migration_mapping_jobs
            .is_empty(),
        "a refused start must record no mapping job"
    );
    for (case, definition_id) in [("first definition", &first), ("second definition", &second)] {
        let execution = staged_execution(&mgn, case, definition_id).await;
        assert_eq!(
            execution["status"].as_str(),
            Some("PENDING"),
            "case {case:?}: a refused start must leave the staged execution PENDING, got {execution:?}"
        );
    }
}

/// One execution runs one mapping job at a time: a start naming another strategy while
/// that job runs is refused with 409 `ConflictException`, and once the clock has
/// finished the job's work a start records a new job.
#[tokio::test]
async fn a_second_mapping_strategy_conflicts_while_the_job_runs_and_is_accepted_after_it() {
    let mgn = Mgn::new();
    let definition_id = create_definition(&mgn, "conflict", "mapping-plan-conflict").await;
    let execution_id = execution_id_of(
        "conflict",
        &staged_execution(&mgn, "conflict", &definition_id).await,
    );

    let (status, started) = start_mapping(
        &mgn,
        &definition_id,
        &execution_id,
        Some(STRATEGIES[0]),
    )
    .await;
    assert_eq!(
        status, 202,
        "the first start expected 202, got {status} with {started:?}"
    );
    let first_job = job_id_of("conflict", &started);

    // Every other strategy conflicts with the running job, whichever one it is.
    for strategy in [None, Some(STRATEGIES[1]), Some(STRATEGIES[2])] {
        let (status, body) =
            start_mapping(&mgn, &definition_id, &execution_id, strategy).await;
        assert_eq!(
            status, 409,
            "case {strategy:?}: expected 409 while the mapping job runs, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {strategy:?}: expected ConflictException, got {body:?}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(execution_id.as_str()),
            "case {strategy:?}: resourceId must name the execution the job runs for, got {:?}",
            body["resourceId"]
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!(
                    "Network migration execution {execution_id} is already running mapping job {first_job}."
                )
                .as_str()
            ),
            "case {strategy:?}: message must name the execution and the running job, got {:?}",
            body["message"]
        );
        assert_eq!(
            mgn.store().lock().network_migration_mapping_jobs.len(),
            1,
            "case {strategy:?}: a refused start must record no second job"
        );
    }

    // The clock finishes the job's work: rewind the instant its completion is owed at,
    // which no call can reach without waiting for it.
    {
        let store = mgn.store();
        let mut guard = store.lock();
        let job = guard
            .network_migration_mapping_jobs
            .get_mut(&first_job)
            .expect("the started job is in the store");
        job.completes_at = SystemTime::now() - Duration::from_secs(1);
    }

    let (status, body) = start_mapping(
        &mgn,
        &definition_id,
        &execution_id,
        Some(STRATEGIES[1]),
    )
    .await;
    assert_eq!(
        status, 202,
        "a start after the previous job finished expected 202, got {status} with {body:?}"
    );
    let second_job = job_id_of("after completion", &body);
    assert_ne!(
        second_job, first_job,
        "a start after the previous job finished must record a new job, got {second_job:?}"
    );
    let store = mgn.store();
    let guard = store.lock();
    let jobs = guard.network_migration_mapping_jobs_of(
        crate::application_lifecycle::EMULATOR_ACCOUNT,
        &definition_id,
        &execution_id,
    );
    assert_eq!(
        jobs.len(),
        2,
        "both mapping runs of the execution must be recorded, got {jobs:?}"
    );
    assert_eq!(
        jobs.iter().map(|job| job.job_id.clone()).collect::<Vec<_>>(),
        vec![first_job, second_job],
        "the jobs must be recorded in the order they were started, got {jobs:?}"
    );
}

/// The job belongs to one execution: two executions of one definition each run their
/// own mapping job, so a run on one does not conflict with a run on the other.
#[tokio::test]
async fn each_execution_of_a_definition_runs_its_own_mapping_job() {
    let mgn = Mgn::new();
    let definition_id = create_definition(&mgn, "two executions", "mapping-plan-two").await;
    let staged = execution_id_of(
        "two executions",
        &staged_execution(&mgn, "two executions", &definition_id).await,
    );

    // A second execution of the same definition: nothing modelled creates one (every
    // `Start*` operation names an execution that already exists), so it is seeded.
    let second = uuid_shaped(0xbeef);
    let stored = mgn
        .store()
        .lock()
        .insert_network_migration_execution(NetworkMigrationExecution {
            network_migration_execution_id: second.clone(),
            network_migration_definition_id: definition_id.clone(),
            account_id: crate::application_lifecycle::EMULATOR_ACCOUNT.to_string(),
            status: Some("PENDING".to_string()),
            stage: Some(EXECUTION_STAGE_MAPPING.to_string()),
            created_at: Some(SystemTime::now()),
            updated_at: Some(SystemTime::now()),
            ..NetworkMigrationExecution::default()
        });
    assert!(stored, "seeding a second execution must store it");

    let (status, first_body) =
        start_mapping(&mgn, &definition_id, &staged, Some(STRATEGIES[0])).await;
    assert_eq!(
        status, 202,
        "the staged execution's start expected 202, got {status} with {first_body:?}"
    );
    // A different strategy on the OTHER execution is not a conflict: the two runs are
    // separate work.
    let (status, second_body) =
        start_mapping(&mgn, &definition_id, &second, Some(STRATEGIES[2])).await;
    assert_eq!(
        status, 202,
        "the seeded execution's start expected 202, got {status} with {second_body:?}"
    );
    assert_ne!(
        job_id_of("seeded execution", &second_body),
        job_id_of("staged execution", &first_body),
        "each execution's run must mint its own jobID"
    );

    // Each execution reports its own run, and only its own.
    let items = executions(&mgn, "two executions", &definition_id).await;
    assert_eq!(
        items.len(),
        2,
        "both executions of the definition must be listed, got {items:?}"
    );
    for execution in items {
        let id = execution_id_of("two executions", &execution);
        assert_eq!(
            execution["activity"].as_str(),
            Some(EXECUTION_ACTIVITY_MAPPING),
            "execution {id:?}: activity must be the mapping run that started, got {execution:?}"
        );
        let store = mgn.store();
        let guard = store.lock();
        let jobs = guard.network_migration_mapping_jobs_of(
            crate::application_lifecycle::EMULATOR_ACCOUNT,
            &definition_id,
            &id,
        );
        assert_eq!(
            jobs.len(),
            1,
            "execution {id:?}: exactly one job must be recorded for it, got {jobs:?}"
        );
    }
}
