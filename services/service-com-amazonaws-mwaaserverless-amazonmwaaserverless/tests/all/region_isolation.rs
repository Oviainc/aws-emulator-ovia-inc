// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule that a REGIONAL endpoint serves only the resources
//! created through it.
//!
//! Every workflow of this service is created at the endpoint named by the caller's
//! SigV4 credential scope, so that region is the partition of the store: a listing
//! reports only its own region's workflows, a reference to a workflow of another
//! region resolves to nothing, and a workflow name is taken only in the region that
//! holds it.
//!
//! Every test drives the real wire path — `MultiServiceRouter::dispatch` over an
//! `awsJson1_0` request — against ONE shared engine, with the credential-scope region
//! as the only difference between two calls. The tables span the class the rule
//! generalises over (which region, how many workflows it holds, which operation
//! resolves the reference, what form the name takes) rather than pinning one request.

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    LOOKUP_MISS_STATUS, TASK_INSTANCE_STATUS_NONE, WORKFLOW_DELETED_AFTER,
};

use crate::task_instance_reads::{instance, seed};
use crate::workflow_lifecycle::{is_workflow_arn, role_arn, Harness};

/// Credential-scope regions the modelled `WorkflowArn` pattern accepts
/// (`([a-z]{2}-[a-z]+-[0-9]{1})`, from `smithy_query(query="shape",
/// shape="WorkflowArn")`), each paired with how many workflows its endpoint creates.
/// The counts differ so the rule is asserted for an endpoint holding none, one and
/// several.
const REGION_POPULATIONS: &[(&str, usize)] = &[
    ("us-east-1", 3),
    ("eu-west-1", 1),
    ("ap-southeast-2", 0),
    ("sa-east-1", 2),
];

/// Region pairs a cross-region reference is tried across: the home endpoint that
/// created the workflow, and a foreign endpoint that must not resolve it.
const REGION_PAIRS: &[(&str, &str)] = &[
    ("us-east-1", "eu-west-1"),
    ("ap-southeast-2", "me-central-1"),
    ("sa-east-1", "ca-central-1"),
];

/// Create one workflow at `region`'s endpoint and report its ARN.
async fn create_at(harness: &Harness, region: &str, name: &str) -> String {
    let (status, created) = harness
        .call_in(
            region,
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/region.py"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {region:?}/{name:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"].as_str().unwrap_or_default();
    assert!(
        is_workflow_arn(arn, region),
        "case {region:?}/{name:?}: expected an ARN minted for that region, got {arn:?}"
    );
    arn.to_string()
}

/// Every `WorkflowArn` `ListWorkflows` reports at `region`'s endpoint, paged
/// `page_size` at a time through the tokens the operation itself issues.
async fn listed_arns(harness: &Harness, region: &str, page_size: u64) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more round trip than there are workflows in the whole table, so a listing
    // that never stops fails the test instead of hanging it.
    for _ in 0..32 {
        let mut request = serde_json::json!({"MaxResults": page_size});
        if let Some(cursor) = token.as_deref() {
            request["NextToken"] = serde_json::Value::String(cursor.to_string());
        }
        let (status, page) = harness.call_in(region, "ListWorkflows", request).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected ListWorkflows 200, got {status} {page:?}"
        );
        let entries = page["Workflows"].as_array().cloned().unwrap_or_default();
        for entry in &entries {
            let arn = entry["WorkflowArn"].as_str().unwrap_or_default();
            assert!(
                is_workflow_arn(arn, region),
                "case {region:?}: a listing may only report ARNs of its own region, got {arn:?}"
            );
            arns.push(arn.to_string());
        }
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return arns;
        }
    }
    panic!("case {region:?}: ListWorkflows never stopped paging, collected {arns:?}");
}

#[tokio::test]
async fn list_workflows_reports_only_the_workflows_created_through_that_region_endpoint() {
    // ONE store, reached through four different regional endpoints.
    let harness = Harness::new(REGION_POPULATIONS[0].0);

    // The same three names in every region, so a name is never what tells the
    // regions' workflows apart.
    let names = ["alpha", "beta.two", "gamma_3"];
    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (region, population) in REGION_POPULATIONS {
        let mut arns = Vec::new();
        for name in names.iter().take(*population) {
            arns.push(create_at(&harness, region, name).await);
        }
        arns.sort();
        expected.push((region, arns));
    }

    // Every page size: one workflow per page (so the cursor is exercised), and one
    // page large enough for the whole region.
    for page_size in [1, 100] {
        for (region, arns) in &expected {
            let mut reported = listed_arns(&harness, region, page_size).await;
            reported.sort();
            assert_eq!(
                &reported, arns,
                "case {region:?} with MaxResults {page_size}: expected exactly that region's \
                 workflows {arns:?}, got {reported:?}"
            );
        }
    }
}

/// One row of the cross-region reference table: an implemented operation that resolves
/// a workflow by ARN, and the request body it takes for a stored workflow, run and task
/// instance.
struct ReferenceCase {
    case: &'static str,
    operation: &'static str,
    /// `(WorkflowArn, RunId, TaskInstanceId)` -> request body.
    body: fn(&str, &str, &str) -> serde_json::Value,
    /// What the FOREIGN endpoint must answer: the reference resolves to nothing there,
    /// and each operation reports that in the vocabulary its own modelled error set
    /// allows.
    foreign: ForeignAnswer,
}

/// How an operation reports a reference its endpoint does not hold.
#[derive(Debug, Clone, Copy)]
enum ForeignAnswer {
    /// The operation declares `ResourceNotFoundException`, so it refuses
    /// (`smithy_query(query="operation", operation="GetWorkflow")` and its siblings).
    LookupMiss,
    /// The operation declares no not-found error — both child-resource listings
    /// (`smithy_query(query="operation",
    /// operation="ListWorkflowRuns,ListWorkflowVersions")`) — so it answers 200 with
    /// this output member empty.
    EmptyPage(&'static str),
}

/// Every implemented operation of this service that resolves a workflow by ARN
/// (`grep -L 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/*.rs`): reporting ones and
/// mutating ones alike, because the rule is about what the endpoint HOLDS, not about
/// what the call does with it.
fn reference_cases() -> Vec<ReferenceCase> {
    vec![
        ReferenceCase {
            case: "GetWorkflow reads the workflow",
            operation: "GetWorkflow",
            body: |arn, _run, _task| serde_json::json!({"WorkflowArn": arn}),
            foreign: ForeignAnswer::LookupMiss,
        },
        ReferenceCase {
            case: "ListWorkflowVersions pages over its versions",
            operation: "ListWorkflowVersions",
            body: |arn, _run, _task| serde_json::json!({"WorkflowArn": arn}),
            foreign: ForeignAnswer::EmptyPage("WorkflowVersions"),
        },
        ReferenceCase {
            case: "ListTagsForResource reads its tags",
            operation: "ListTagsForResource",
            body: |arn, _run, _task| serde_json::json!({"ResourceArn": arn}),
            foreign: ForeignAnswer::LookupMiss,
        },
        ReferenceCase {
            case: "ListWorkflowRuns pages over its runs",
            operation: "ListWorkflowRuns",
            body: |arn, _run, _task| serde_json::json!({"WorkflowArn": arn}),
            foreign: ForeignAnswer::EmptyPage("WorkflowRuns"),
        },
        ReferenceCase {
            case: "GetWorkflowRun reads one of its runs",
            operation: "GetWorkflowRun",
            body: |arn, run, _task| serde_json::json!({"WorkflowArn": arn, "RunId": run}),
            foreign: ForeignAnswer::LookupMiss,
        },
        ReferenceCase {
            case: "GetTaskInstance reads a task instance of one of its runs",
            operation: "GetTaskInstance",
            body: |arn, run, task| serde_json::json!({
                "WorkflowArn": arn,
                "RunId": run,
                "TaskInstanceId": task,
            }),
            foreign: ForeignAnswer::LookupMiss,
        },
        ReferenceCase {
            case: "StartWorkflowRun starts a run under it",
            operation: "StartWorkflowRun",
            body: |arn, _run, _task| serde_json::json!({"WorkflowArn": arn}),
            foreign: ForeignAnswer::LookupMiss,
        },
        ReferenceCase {
            case: "UpdateWorkflow appends a version to it",
            operation: "UpdateWorkflow",
            body: |arn, _run, _task| serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/updated.py"},
                "RoleArn": role_arn(),
            }),
            foreign: ForeignAnswer::LookupMiss,
        },
        ReferenceCase {
            case: "DeleteWorkflow tears it down",
            operation: "DeleteWorkflow",
            body: |arn, _run, _task| serde_json::json!({"WorkflowArn": arn}),
            foreign: ForeignAnswer::LookupMiss,
        },
    ]
}

#[tokio::test]
async fn a_workflow_reference_resolves_to_nothing_outside_the_region_that_created_it() {
    const TASK_INSTANCE_ID: &str = "probe.task.0";

    for (index, row) in reference_cases().into_iter().enumerate() {
        let (home, foreign) = REGION_PAIRS[index % REGION_PAIRS.len()];
        // A fresh store per row, so one row's workflow cannot answer another's call.
        let harness = Harness::new(home);
        let arn = create_at(&harness, home, "shared").await;

        let (status, started) = harness
            .call_in(
                home,
                "StartWorkflowRun",
                serde_json::json!({"WorkflowArn": arn}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the setup StartWorkflowRun to succeed, got {status} {started:?}",
            row.case
        );
        let run_id = started["RunId"]
            .as_str()
            .expect("setup: a started run has a RunId")
            .to_string();
        // Nothing in this API creates a task instance (no Apache Airflow engine runs a
        // definition here), so the one `GetTaskInstance` reads is seeded into the store.
        seed(
            &harness,
            &arn,
            &run_id,
            instance(TASK_INSTANCE_ID, TASK_INSTANCE_STATUS_NONE),
        );

        let request = (row.body)(&arn, &run_id, TASK_INSTANCE_ID);

        let (status, refused) = harness
            .call_in(foreign, row.operation, request.clone())
            .await;
        match row.foreign {
            ForeignAnswer::LookupMiss => {
                assert_eq!(
                    status,
                    LOOKUP_MISS_STATUS,
                    "case {:?} signed for {foreign:?}: expected a lookup miss for a {home:?} \
                     workflow, got {status} {refused:?}",
                    row.case
                );
                assert!(
                    refused["__type"]
                        .as_str()
                        .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
                    "case {:?} signed for {foreign:?}: expected a ResourceNotFoundException, \
                     got {:?}",
                    row.case,
                    refused["__type"]
                );
            }
            ForeignAnswer::EmptyPage(items) => {
                assert_eq!(
                    status, 200,
                    "case {:?} signed for {foreign:?}: a listing that declares no not-found \
                     error answers an empty page, got {status} {refused:?}",
                    row.case
                );
                assert_eq!(
                    refused[items].as_array().map(Vec::len),
                    Some(0),
                    "case {:?} signed for {foreign:?}: expected no {items} of a {home:?} \
                     workflow, got {:?}",
                    row.case,
                    refused[items]
                );
            }
        }

        // The foreign call changed nothing: the home endpoint still holds the workflow,
        // with the one run the setup started.
        let (status, still_there) = harness
            .call_in(home, "GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: the {home:?} workflow must survive a {foreign:?} call, got {status} \
             {still_there:?}",
            row.case
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            1,
            "case {:?}: a {foreign:?} call must start no run on the {home:?} workflow, store \
             holds {} runs",
            row.case,
            harness.stored_run_count(&arn)
        );

        // The very same request at the home endpoint is served.
        let (status, served) = harness.call_in(home, row.operation, request).await;
        assert_eq!(
            status, 200,
            "case {:?} signed for {home:?}: expected the operation to be served, got {status} \
             {served:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_workflow_name_is_taken_only_in_the_region_that_holds_it() {
    // Names differing in form: the shortest the modelled `NameString` allows, one with
    // a dot, one with a dash and an underscore, and the longest it allows.
    let names: Vec<String> = vec![
        "a".to_string(),
        "etl".to_string(),
        "etl.daily-load_2".to_string(),
        format!("x{}", "9".repeat(254)),
    ];

    for name in names {
        // ONE store; each region's endpoint creates its own workflow under this name.
        let harness = Harness::new(REGION_PAIRS[0].0);
        let mut minted: Vec<(&str, String)> = Vec::new();
        for (region, _) in REGION_PAIRS {
            let arn = create_at(&harness, region, &name).await;
            assert!(
                minted.iter().all(|(_, held)| held != &arn),
                "case {name:?}: {region:?} must mint its own ARN, got the already-minted {arn:?}"
            );
            minted.push((region, arn));
        }

        // Each region reads back ITS workflow under that name, and only that one.
        for (region, arn) in &minted {
            let (status, fetched) = harness
                .call_in(region, "GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?} in {region:?}: expected GetWorkflow 200, got {status} {fetched:?}"
            );
            assert_eq!(
                fetched["Name"].as_str(),
                Some(name.as_str()),
                "case {name:?} in {region:?}: expected the created name, got {:?}",
                fetched["Name"]
            );
            let reported = listed_arns(&harness, region, 100).await;
            assert_eq!(
                reported,
                vec![arn.clone()],
                "case {name:?} in {region:?}: expected only that region's workflow, got \
                 {reported:?}"
            );
        }

        // A SECOND create under that name at an endpoint that already holds it is still
        // the conflict the uniqueness rule requires.
        for (region, arn) in &minted {
            let (status, conflict) = harness
                .call_in(
                    region,
                    "CreateWorkflow",
                    serde_json::json!({
                        "Name": name,
                        "DefinitionS3Location": {
                            "Bucket": "defs",
                            "ObjectKey": "dags/region.py",
                        },
                        "RoleArn": role_arn(),
                    }),
                )
                .await;
            assert_eq!(
                status, 409,
                "case {name:?} in {region:?}: expected a ConflictException (409) for the name \
                 that region already holds, got {status} {conflict:?}"
            );
            assert_eq!(
                conflict["ResourceId"].as_str(),
                Some(arn.as_str()),
                "case {name:?} in {region:?}: the conflict must name that region's holder, got \
                 {:?}",
                conflict["ResourceId"]
            );
        }
    }
}

#[tokio::test]
async fn one_region_teardown_leaves_every_other_region_untouched() {
    let harness = Harness::new(REGION_PAIRS[0].0);
    let mut minted: Vec<(&str, String)> = Vec::new();
    for (region, _) in REGION_PAIRS {
        minted.push((region, create_at(&harness, region, "etl").await));
    }

    // Tear the first region's workflow down and let the clock finish its removal.
    let (torn_down_region, torn_down_arn) = minted.remove(0);
    let (status, deleted) = harness
        .call_in(
            torn_down_region,
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": torn_down_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {torn_down_region:?}: expected DeleteWorkflow 200, got {status} {deleted:?}"
    );
    tokio::time::sleep(WORKFLOW_DELETED_AFTER + std::time::Duration::from_millis(50)).await;

    let reported = listed_arns(&harness, torn_down_region, 100).await;
    assert!(
        reported.is_empty(),
        "case {torn_down_region:?}: expected an empty listing after the teardown, got {reported:?}"
    );

    // Every other region still holds its own workflow under the same name.
    for (region, arn) in &minted {
        let reported = listed_arns(&harness, region, 100).await;
        assert_eq!(
            reported,
            vec![arn.clone()],
            "case {region:?}: a {torn_down_region:?} teardown must leave it alone, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_client_token_replays_only_at_the_endpoint_that_first_used_it() {
    // Tokens differing in form, since the rule is about the endpoint and not the token.
    for token in ["00000000-0000-4000-8000-000000000000", "token.1", "T"] {
        let harness = Harness::new(REGION_PAIRS[0].0);
        let request = serde_json::json!({
            "Name": "etl",
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/region.py"},
            "RoleArn": role_arn(),
            "ClientToken": token,
        });

        let mut minted: Vec<(&str, String)> = Vec::new();
        for (region, _) in REGION_PAIRS {
            let (status, created) = harness
                .call_in(region, "CreateWorkflow", request.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {token:?} in {region:?}: expected CreateWorkflow 200, got {status} \
                 {created:?}"
            );
            let arn = created["WorkflowArn"].as_str().unwrap_or_default();
            assert!(
                is_workflow_arn(arn, region),
                "case {token:?} in {region:?}: expected an ARN of that region, got {arn:?}"
            );
            assert!(
                minted.iter().all(|(_, held)| held != arn),
                "case {token:?} in {region:?}: a token of another endpoint must not replay, got \
                 the already-minted {arn:?}"
            );
            minted.push((region, arn.to_string()));
        }

        // At the endpoint that first used it, the token still replays its own answer.
        for (region, arn) in &minted {
            let (status, replayed) = harness
                .call_in(region, "CreateWorkflow", request.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {token:?} in {region:?}: expected the replay to succeed, got {status} \
                 {replayed:?}"
            );
            assert_eq!(
                replayed["WorkflowArn"].as_str(),
                Some(arn.as_str()),
                "case {token:?} in {region:?}: expected the replay to report that region's \
                 workflow, got {:?}",
                replayed["WorkflowArn"]
            );
        }
    }
}
