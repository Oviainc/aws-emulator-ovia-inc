// SPDX-License-Identifier: CC0-1.0

//! What deleting the DESTINATION of a bulk import job does to the job, driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with, so
//! every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a bulk import job's destination is the session
//! dataset it ingests into together with the workspace that dataset lives in,
//! both resolved against the store when the job was created, and this service
//! models no call that deletes a job on its own — so a delete of either resource
//! takes the jobs addressed to it, and no later read reports a `datasetId` or a
//! `workspaceName` the store no longer holds. A job ingesting into time series
//! OUTSIDE a workspace names neither, so no container's delete touches it, and
//! neither does a delete of another workspace's dataset.
//!
//! The rows differ along what the rule generalises over: which resource of the
//! destination the row deletes, how many jobs are addressed to it, whether the
//! dataset's delete names the workspace scope it may carry, and whether the
//! lifecycle the create owes has settled when the delete lands — so a job taken
//! while it is still `PENDING` and one taken after it reached its terminal
//! status are both covered.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`,
//! `router_with_engine`) is the one the asset suite defines, the dataset create
//! is the data-segment suites' and the storage put is the
//! storage-configuration suite's; all three are reused rather than copied.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, BULK_IMPORT_JOB_STEP, RESOURCE_BULK_IMPORT_JOB,
    STORAGE_STATE_ENABLED, STORAGE_TYPE_SITEWISE_DEFAULT,
};

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine};
use crate::dataset_data_segments::create_dataset;
use crate::storage_configuration::put as put_storage;

/// The workspace whose destination resources the rows delete.
const WORKSPACE: &str = "plant-floor";

/// A second workspace, so every row measures what a delete removed against what
/// it had to leave alone.
const NEIGHBOUR: &str = "cold-store";

/// `POST /jobs` and `GET /jobs`, as `OPERATIONS.md` reports the routes.
const JOBS_URI: &str = "/jobs";

/// The `jobRoleArn` every create names: an ARN of the form the modelled `ARN`
/// pattern admits (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SiteWiseBulkImport";

/// A page size covering every job a row creates, so one listing reports them all.
/// `maxResults` is range 1..250 (smithy_query query="operation"
/// operation="ListBulkImportJobs").
const PAGE_SIZE: &str = "maxResults=250";

/// Long enough that both steps the clock owes a job have elapsed, so a row that
/// waits observes the terminal status whatever the scheduler does.
fn settled() -> Duration {
    2 * BULK_IMPORT_JOB_STEP + Duration::from_millis(250)
}

/// `CreateBulkImportJob`, ingesting into `destination` — the `(workspaceName,
/// datasetId)` pair naming a session dataset — or into time series outside a
/// workspace when the row names none. Answers the minted `jobId`.
async fn create_job(
    router: &MultiServiceRouter,
    destination: Option<(&str, &str)>,
    job_name: &str,
) -> String {
    let mut body = json!({
        "jobName": job_name,
        "jobRoleArn": JOB_ROLE_ARN,
        "files": [{ "bucket": "sitewise-import", "key": "turbines/2026/01.csv" }],
        "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some((workspace, dataset_id)) = destination {
        object.insert("workspaceName".to_string(), json!(workspace));
        object.insert("datasetId".to_string(), json!(dataset_id));
    }
    let (status, created) = call(router, "POST", JOBS_URI, Some(body)).await;
    assert_eq!(
        status, 202,
        "CreateBulkImportJob {job_name:?}: got {status} {created}"
    );
    let job_id = created["jobId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&job_id),
        "CreateBulkImportJob {job_name:?}: expected a modelled jobId, got {created}"
    );
    job_id
}

/// `DescribeBulkImportJob`, scoped to `workspace` when the read names one.
async fn describe_job(
    router: &MultiServiceRouter,
    job_id: &str,
    workspace: Option<&str>,
) -> (u16, Value) {
    let uri = match workspace {
        None => format!("{JOBS_URI}/{job_id}"),
        Some(workspace) => format!("{JOBS_URI}/{job_id}?workspaceName={workspace}"),
    };
    call(router, "GET", &uri, None).await
}

/// The `jobId`s the account-wide `ListBulkImportJobs` reports.
async fn listed_ids(router: &MultiServiceRouter) -> BTreeSet<String> {
    let (status, body) = call(router, "GET", &format!("{JOBS_URI}?{PAGE_SIZE}"), None).await;
    assert_eq!(
        status, 200,
        "ListBulkImportJobs expected 200, got {status} {body}"
    );
    body["jobSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries an id, got {summary}"))
                .to_string()
        })
        .collect()
}

/// `DeleteDataset`, naming the workspace scope only when the row carries it (the
/// member is optional; smithy_query query="operation" operation="DeleteDataset").
async fn delete_dataset(
    router: &MultiServiceRouter,
    dataset_id: &str,
    workspace: Option<&str>,
) -> (u16, Value) {
    let uri = match workspace {
        None => format!("/datasets/{dataset_id}"),
        Some(workspace) => format!("/datasets/{dataset_id}?workspaceName={workspace}"),
    };
    call(router, "DELETE", &uri, None).await
}

/// `DeleteWorkspace`.
async fn delete_workspace(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/workspaces/{name}"), None).await
}

/// A `DescribeBulkImportJob` body without the two members the CLOCK moves on its
/// own, so two reads taken at different instants are comparable.
fn without_clock(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("jobStatus");
        object.remove("jobLastUpdateDate");
    }
    copy
}

/// Every reference a stored bulk import job holds to a resource the store no
/// longer holds, as `"<jobId> -> <kind> <identifier>"`.
///
/// Read straight off the store, because the invariant is about what the store
/// KEEPS: a job the API can no longer reach through any scope would still be
/// reported by an account-wide read.
fn dangling_references(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    let mut dangling = Vec::new();
    for job in store.bulk_import_jobs.values() {
        if let Some(workspace) = job.workspace_name.as_deref() {
            if !store.workspaces.contains_key(workspace) {
                dangling.push(format!("{} -> workspace {workspace}", job.job_id));
            }
        }
        if let Some(dataset_id) = job.dataset_id.as_deref() {
            if !store.datasets.contains_key(dataset_id) {
                dangling.push(format!("{} -> dataset {dataset_id}", job.job_id));
            }
        }
    }
    dangling.sort();
    dangling
}

/// Which resource of a job's destination the row deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The session dataset the jobs ingest into.
    Dataset,
    /// The workspace that dataset lives in — reached after its datasets are
    /// deleted, which is what the container's delete requires.
    Workspace,
}

/// One row of the table.
struct Case {
    label: &'static str,
    target: Target,
    /// How many jobs the row addresses to the dataset it deletes.
    jobs: usize,
    /// Whether the dataset's delete names the workspace scope.
    scoped: bool,
    /// Whether the row waits for the lifecycle the create owes to settle before
    /// deleting.
    settled: bool,
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the-dataset-of-one-pending-job-deleted-unscoped",
            target: Target::Dataset,
            jobs: 1,
            scoped: false,
            settled: false,
        },
        Case {
            label: "the-dataset-of-three-settled-jobs-deleted-in-its-workspace",
            target: Target::Dataset,
            jobs: 3,
            scoped: true,
            settled: true,
        },
        Case {
            label: "the-dataset-of-no-job-at-all-deleted",
            target: Target::Dataset,
            jobs: 0,
            scoped: true,
            settled: false,
        },
        Case {
            label: "the-workspace-of-two-pending-jobs-deleted",
            target: Target::Workspace,
            jobs: 2,
            scoped: true,
            settled: false,
        },
        Case {
            label: "the-workspace-of-one-settled-job-deleted",
            target: Target::Workspace,
            jobs: 1,
            scoped: true,
            settled: true,
        },
    ]
}

#[tokio::test]
async fn a_delete_of_a_jobs_destination_takes_the_jobs_addressed_to_it() {
    for case in cases() {
        let (router, engine) = router_with_engine();

        // "Before you create a bulk import job that ingests data into time series
        // outside of a workspace, you must enable IoT SiteWise warm tier or IoT
        // SiteWise cold tier" — the precondition the account-wide job has.
        let (status, configured) = put_storage(
            &router,
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "warmTier": STORAGE_STATE_ENABLED,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: PutStorageConfiguration got {status} {configured}",
            case.label
        );

        let target_dataset =
            create_dataset(&router, "Target session", Some("SESSION"), Some(WORKSPACE)).await;
        let bystander_dataset =
            create_dataset(&router, "Bystander session", Some("SESSION"), Some(WORKSPACE)).await;
        let neighbour_dataset =
            create_dataset(&router, "Neighbour session", Some("SESSION"), Some(NEIGHBOUR)).await;

        let mut addressed = Vec::new();
        for at in 0..case.jobs {
            addressed.push(
                create_job(
                    &router,
                    Some((WORKSPACE, &target_dataset)),
                    &format!("target import {at}"),
                )
                .await,
            );
        }
        let bystander_job = create_job(
            &router,
            Some((WORKSPACE, &bystander_dataset)),
            "bystander import",
        )
        .await;
        let neighbour_job = create_job(
            &router,
            Some((NEIGHBOUR, &neighbour_dataset)),
            "neighbour import",
        )
        .await;
        let account_job = create_job(&router, None, "account import").await;

        // What the row's delete has to take, and what it has to leave: a job of
        // the dataset's own workspace but another dataset survives the DATASET's
        // delete and goes with the WORKSPACE's, while a job of another workspace
        // and one ingesting outside every workspace survive both.
        let mut swept: BTreeSet<String> = addressed.iter().cloned().collect();
        let mut survivors: BTreeSet<String> =
            BTreeSet::from([neighbour_job.clone(), account_job.clone()]);
        match case.target {
            Target::Dataset => {
                survivors.insert(bystander_job.clone());
            }
            Target::Workspace => {
                swept.insert(bystander_job.clone());
            }
        }

        // Every job reads back before the delete, so what the delete removed is
        // measured against what was really stored.
        let mut before_delete = Vec::new();
        for job_id in swept.iter().chain(survivors.iter()) {
            let (status, body) = describe_job(&router, job_id, None).await;
            assert_eq!(
                status, 200,
                "case {:?}: the job {job_id} must read back before the delete, \
                 got {status} {body}",
                case.label
            );
            if survivors.contains(job_id) {
                before_delete.push((job_id.clone(), without_clock(&body)));
            }
        }
        assert!(
            dangling_references(&engine).is_empty(),
            "case {:?}: the fixture must hold no dangling reference, got {:?}",
            case.label,
            dangling_references(&engine)
        );

        if case.settled {
            tokio::time::sleep(settled()).await;
        }

        match case.target {
            Target::Dataset => {
                let scope = case.scoped.then_some(WORKSPACE);
                let (status, answer) = delete_dataset(&router, &target_dataset, scope).await;
                assert_eq!(
                    status, 202,
                    "case {:?}: DeleteDataset expected 202, got {status} {answer}",
                    case.label
                );
            }
            Target::Workspace => {
                // "Before you delete a workspace, you must delete all resources
                // contained in or associated with the workspace": its datasets
                // are contents, so they go first.
                for dataset_id in [&target_dataset, &bystander_dataset] {
                    let (status, answer) =
                        delete_dataset(&router, dataset_id, Some(WORKSPACE)).await;
                    assert_eq!(
                        status, 202,
                        "case {:?}: DeleteDataset {dataset_id} expected 202, \
                         got {status} {answer}",
                        case.label
                    );
                }
                let (status, answer) = delete_workspace(&router, WORKSPACE).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: DeleteWorkspace expected 200, got {status} {answer}",
                    case.label
                );
            }
        }

        // Each job addressed to what the row deleted is gone from its reader.
        for job_id in &swept {
            let (status, body) = describe_job(&router, job_id, None).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {:?}: a job whose destination was deleted must answer the \
                 modelled absence, got {status} {body}",
                case.label
            );
            assert_eq!(
                body["message"],
                json!(not_found_message(RESOURCE_BULK_IMPORT_JOB, job_id)),
                "case {:?}: the refusal names the job the request addressed, got {body}",
                case.label
            );
        }

        // And from the account-wide listing, which no scope narrows.
        let listed = listed_ids(&router).await;
        assert_eq!(
            listed, survivors,
            "case {:?}: the listing must report exactly the surviving jobs, got {listed:?}",
            case.label
        );

        // Every other job is untouched, destination included.
        for (job_id, before) in &before_delete {
            let (status, body) = describe_job(&router, job_id, None).await;
            assert_eq!(
                status, 200,
                "case {:?}: the surviving job {job_id} must still read back, \
                 got {status} {body}",
                case.label
            );
            assert_eq!(
                &without_clock(&body),
                before,
                "case {:?}: the surviving job {job_id} must read back unchanged, got {body}",
                case.label
            );
        }

        assert!(
            dangling_references(&engine).is_empty(),
            "case {:?}: no stored job may name a destination the store no longer \
             holds, got {:?}",
            case.label,
            dangling_references(&engine)
        );
    }
}

#[tokio::test]
async fn a_workspace_delete_takes_a_job_naming_it_whatever_became_of_its_dataset() {
    let (router, engine) = router_with_engine();

    let (status, configured) = put_storage(
        &router,
        json!({
            "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
            "warmTier": STORAGE_STATE_ENABLED,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutStorageConfiguration got {status} {configured}"
    );

    let dataset = create_dataset(&router, "Orphan session", Some("SESSION"), Some(WORKSPACE)).await;
    let neighbour_dataset =
        create_dataset(&router, "Neighbour session", Some("SESSION"), Some(NEIGHBOUR)).await;
    let doomed = create_job(&router, Some((WORKSPACE, &dataset)), "orphan import").await;
    let neighbour_job = create_job(
        &router,
        Some((NEIGHBOUR, &neighbour_dataset)),
        "neighbour import",
    )
    .await;
    let account_job = create_job(&router, None, "account import").await;

    // The dataset leaves the store WITHOUT the delete that owns it, so what the
    // WORKSPACE's delete takes is measured on its own: the job still names the
    // workspace, and that is what has to take it.
    let removed = {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let mut store = state.lock();
        store.datasets.remove(&dataset)
    };
    assert!(
        removed.is_some(),
        "the fixture's dataset {dataset} must have been stored"
    );

    let (status, answer) = delete_workspace(&router, WORKSPACE).await;
    assert_eq!(
        status, 200,
        "DeleteWorkspace expected 200, got {status} {answer}"
    );

    // The job named the workspace, so it went with it: the account-wide read,
    // which no scope narrows, answers the modelled absence rather than reporting
    // a workspace the store no longer holds.
    let (status, gone) = describe_job(&router, &doomed, None).await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "a job naming the deleted workspace must answer the modelled absence, got {status} {gone}"
    );
    assert_eq!(
        gone["message"],
        json!(not_found_message(RESOURCE_BULK_IMPORT_JOB, &doomed)),
        "the refusal names the job the request addressed, got {gone}"
    );

    // The read scoped to that workspace agrees: the two forms can no longer
    // disagree about whether the job is there.
    let (status, scoped) = describe_job(&router, &doomed, Some(WORKSPACE)).await;
    assert_eq!(
        (status, error_code(&scoped)),
        (404, "ResourceNotFoundException"),
        "the workspace-scoped read of the same job must agree, got {status} {scoped}"
    );

    let listed = listed_ids(&router).await;
    assert_eq!(
        listed,
        BTreeSet::from([neighbour_job.clone(), account_job.clone()]),
        "the listing must report exactly the jobs of the destinations that stand, got {listed:?}"
    );
    for job_id in [&neighbour_job, &account_job] {
        let (status, body) = describe_job(&router, job_id, None).await;
        assert_eq!(
            status, 200,
            "the job {job_id} must survive a delete of another workspace, got {status} {body}"
        );
    }

    assert!(
        dangling_references(&engine).is_empty(),
        "no stored job may name a destination the store no longer holds, got {:?}",
        dangling_references(&engine)
    );
}
