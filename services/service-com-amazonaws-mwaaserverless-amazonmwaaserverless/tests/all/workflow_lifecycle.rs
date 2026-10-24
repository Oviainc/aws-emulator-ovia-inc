// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MWAA Serverless workflow lifecycle slice:
//! `CreateWorkflow`, `GetWorkflow`, `ListWorkflows`, `StartWorkflowRun`,
//! `GetWorkflowRun` and `DeleteWorkflow`.
//!
//! Every test drives the real wire path — `MultiServiceRouter::dispatch` over an
//! `awsJson1_0` request — against ONE shared engine, so what a test asserts is what a
//! client sees. The tables below span the class each rule generalises over rather than
//! pinning one recorded request.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::{
    ComAmazonawsMwaaserverlessAmazonmwaaserverlessState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The members `CreateWorkflowRequest` and `GetWorkflowResponse` share, from
/// `smithy_query(query="operation", operation="CreateWorkflow,GetWorkflow")`. A create
/// must be readable back through `GetWorkflow` on every one of them.
const ECHOED_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "DefinitionS3Location",
    "Code",
    "RoleArn",
    "EncryptionConfiguration",
    "LoggingConfiguration",
    "EngineVersion",
    "NetworkConfiguration",
    "TriggerMode",
];

/// One emulator, reached exactly the way the production router reaches it.
///
/// `pub(crate)` so the other suites of this test target drive the same wire path
/// rather than each building a harness of its own.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    region: &'static str,
}

impl Harness {
    pub(crate) fn new(region: &'static str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            engine,
            region,
        }
    }

    /// Call one operation over the wire and return its status and parsed body.
    pub(crate) async fn call(
        &self,
        operation: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let payload = serde_json::to_vec(&body).expect("serialize the request body");
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonMWAAServerless.{operation}"))
            // The SigV4 credential scope is both the routing signal the core reads
            // (`resolve_route_key`) and the region a minted ARN carries.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{}/airflow-serverless/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=00",
                    self.region
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    /// How many runs the STORE holds for a workflow — the only way to show that a
    /// repeated `ClientToken` created nothing while `ListWorkflowRuns` is still a stub.
    pub(crate) fn stored_run_count(&self, workflow_arn: &str) -> usize {
        self.engine
            .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
            .lock()
            .workflows
            .get(workflow_arn)
            .map_or(0, |workflow| workflow.runs.len())
    }

    /// The engine this emulator reads and writes, so a suite can inspect or seed a
    /// record no modelled operation of this service creates — a run's task
    /// instances, which come into being only when something executes a workflow
    /// definition and nothing in this emulator does.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// The `(WorkflowVersion, IsLatestVersion)` pairs the STORE holds for a workflow,
    /// in the order the versions were appended — the only way to show which single
    /// version is flagged latest while `ListWorkflowVersions` is still a stub.
    pub(crate) fn stored_versions(&self, workflow_arn: &str) -> Vec<(String, bool)> {
        self.engine
            .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
            .lock()
            .workflows
            .get(workflow_arn)
            .map(|workflow| {
                workflow
                    .versions
                    .iter()
                    .map(|record| (record.version.clone(), record.is_latest))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// `arn:aws:airflow-serverless:<region>:<account>:workflow/<name>-<10 alnum>`, written
/// out from the modelled `WorkflowArn` pattern
/// (`smithy_query(query="shape", shape="WorkflowArn")`):
/// `^arn:aws…:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$`
fn is_workflow_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:airflow-serverless:{region}:");
    let Some(rest) = arn.strip_prefix(&prefix) else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(name_and_suffix) = resource.strip_prefix("workflow/") else {
        return false;
    };
    if name_and_suffix.len() < 12 {
        return false;
    }
    let (name, dash_suffix) = name_and_suffix.split_at(name_and_suffix.len() - 11);
    let Some(suffix) = dash_suffix.strip_prefix('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        && (1..=255).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The modelled `WorkflowVersion`: `^[a-z0-9]{32}$`, length 32..32.
fn is_workflow_version(version: &str) -> bool {
    version.len() == 32
        && version
            .bytes()
            .all(|b| b.is_ascii_lowercase() && b.is_ascii_alphabetic() || b.is_ascii_digit())
}

/// The modelled `IdString` a `RunId` uses: `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`, length 1..255.
fn is_run_id(run_id: &str) -> bool {
    (1..=255).contains(&run_id.len())
        && run_id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// A valid IAM role ARN for the emulator's own account, as `RoleARN` models it.
pub(crate) fn role_arn() -> String {
    "arn:aws:iam::000000000000:role/mwaa-serverless-exec".to_string()
}

/// The create requests the round-trip table covers. Rows differ along what the rule
/// generalises over: which optional members are present, the form and length of the
/// name, map and list sizes, and whether the optional `Code` union is supplied.
fn create_cases() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "only the required members",
            serde_json::json!({
                "Name": "minimal",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/minimal.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
        (
            "every optional member, two tags, two subnets",
            serde_json::json!({
                "Name": "fully.specified-workflow_1",
                "DefinitionS3Location": {
                    "Bucket": "defs",
                    "ObjectKey": "dags/full.yaml",
                    "VersionId": "s3objectversion1",
                },
                "Code": {"S3Location": {"Bucket": "code", "ObjectKey": "src.zip"}},
                "RoleArn": role_arn(),
                "Description": "a workflow with every optional member supplied",
                "EncryptionConfiguration": {
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd",
                },
                "LoggingConfiguration": {"LogGroupName": "/aws/mwaa-serverless/full"},
                "EngineVersion": 1,
                "NetworkConfiguration": {
                    "SecurityGroupIds": ["sg-1"],
                    "SubnetIds": ["subnet-1", "subnet-2"],
                },
                "Tags": {"team": "data", "env": "test"},
                "TriggerMode": "SCHEDULED",
            }),
        ),
        (
            "empty collections, so a map and a list of size zero round-trip",
            serde_json::json!({
                "Name": "emptyCollections",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/empty.yaml"},
                "RoleArn": role_arn(),
                "NetworkConfiguration": {"SecurityGroupIds": [], "SubnetIds": []},
                "Tags": {},
            }),
        ),
        (
            "a single-character name, the shortest NameString allows",
            serde_json::json!({
                "Name": "a",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/a.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
        (
            "a 255-character name, the longest NameString allows",
            serde_json::json!({
                "Name": format!("x{}", "9".repeat(254)),
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/long.yaml"},
                "RoleArn": role_arn(),
                "Description": "the maximum-length name still mints a pattern-valid ARN",
            }),
        ),
        (
            "an AWS_MANAGED_KEY encryption configuration with no key id",
            serde_json::json!({
                "Name": "awsManagedKey",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/kms.yaml"},
                "RoleArn": role_arn(),
                "EncryptionConfiguration": {"Type": "AWS_MANAGED_KEY"},
                "TriggerMode": "ON_DEMAND",
            }),
        ),
    ]
}

#[tokio::test]
async fn create_mints_a_modelled_workflow_and_get_reads_every_member_back() {
    for (case, body) in create_cases() {
        // A fresh emulator per row, so one row's workflow cannot answer another's read.
        let harness = Harness::new("us-west-2");

        let (status, created) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
        );

        let arn = created["WorkflowArn"].as_str().unwrap_or_default();
        assert!(
            is_workflow_arn(arn, "us-west-2"),
            "case {case:?}: expected a WorkflowArn matching the modelled pattern, got {arn:?}"
        );
        let version = created["WorkflowVersion"].as_str().unwrap_or_default();
        assert!(
            is_workflow_version(version),
            "case {case:?}: expected a 32-character lowercase-hex WorkflowVersion, got {version:?}"
        );
        assert_eq!(
            created["WorkflowStatus"], "READY",
            "case {case:?}: expected WorkflowStatus READY, got {:?}",
            created["WorkflowStatus"]
        );
        assert_eq!(
            created["IsLatestVersion"],
            serde_json::Value::Bool(true),
            "case {case:?}: expected IsLatestVersion true, got {:?}",
            created["IsLatestVersion"]
        );
        assert!(
            created["CreatedAt"].is_number(),
            "case {case:?}: expected a CreatedAt timestamp, got {:?}",
            created["CreatedAt"]
        );
        assert!(
            created["RevisionId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected a RevisionId, got {:?}",
            created["RevisionId"]
        );

        // ROUND TRIP: read the resource back through the implemented reader.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowArn"], created["WorkflowArn"],
            "case {case:?}: GetWorkflow reported a different ARN: {:?}",
            fetched["WorkflowArn"]
        );
        assert_eq!(
            fetched["WorkflowVersion"], created["WorkflowVersion"],
            "case {case:?}: GetWorkflow reported a different version: {:?}",
            fetched["WorkflowVersion"]
        );
        assert_eq!(
            fetched["WorkflowStatus"], created["WorkflowStatus"],
            "case {case:?}: GetWorkflow reported a different status: {:?}",
            fetched["WorkflowStatus"]
        );
        assert_eq!(
            fetched["CreatedAt"], created["CreatedAt"],
            "case {case:?}: GetWorkflow reported a different CreatedAt: {:?}",
            fetched["CreatedAt"]
        );
        for member in ECHOED_MEMBERS {
            let supplied = &body[member];
            if supplied.is_null() {
                continue;
            }
            assert_eq!(
                &fetched[member], supplied,
                "case {case:?}: GetWorkflow must report the {member} supplied at create, \
                 got {:?}",
                fetched[member]
            );
        }
        assert!(
            fetched["ModifiedAt"].is_number(),
            "case {case:?}: expected a ModifiedAt timestamp, got {:?}",
            fetched["ModifiedAt"]
        );
        assert!(
            fetched["CodeSnapshottedAt"].is_number(),
            "case {case:?}: expected a CodeSnapshottedAt timestamp, got {:?}",
            fetched["CodeSnapshottedAt"]
        );

        // The version the create reported selects the same stored record.
        let (status, by_version) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a versioned GetWorkflow to succeed, got {status} \
             {by_version:?}"
        );
        assert_eq!(
            by_version["DefinitionS3Location"], fetched["DefinitionS3Location"],
            "case {case:?}: the stored version must carry the same definition, got {:?}",
            by_version["DefinitionS3Location"]
        );
    }
}

#[tokio::test]
async fn an_unknown_workflow_is_refused_and_a_duplicate_name_conflicts() {
    let harness = Harness::new("eu-west-1");

    // Pattern-valid ARNs the store has never held.
    let unknown = [
        "arn:aws:airflow-serverless:eu-west-1:000000000000:workflow/absent-0123456789",
        "arn:aws:airflow-serverless:eu-west-1:000000000000:workflow/other.one-abcdefghij",
    ];
    for arn in unknown {
        let (status, body) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {arn:?}: expected a 400 lookup miss, got {status} {body:?}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|t| t.contains("ResourceNotFoundException")),
            "case {arn:?}: expected a ResourceNotFoundException, got {:?}",
            body["__type"]
        );
    }

    // Names differing in form: a second create under a name already stored conflicts.
    for name in ["demo", "demo.two", "demo-three_4"] {
        let body = serde_json::json!({
            "Name": name,
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/d.yaml"},
            "RoleArn": role_arn(),
        });
        let (status, first) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}"
        );

        let (status, conflict) = harness.call("CreateWorkflow", body).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected ConflictException (409) for a duplicate name, got \
             {status} {conflict:?}"
        );
        assert!(
            conflict["__type"]
                .as_str()
                .is_some_and(|t| t.contains("ConflictException")),
            "case {name:?}: expected a ConflictException, got {:?}",
            conflict["__type"]
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_replays_the_first_create() {
    let harness = Harness::new("us-east-1");

    // Tokens differing in form, each with its own request.
    for (index, token) in ["t", "Token-1", "~!@#$%^&*()_+"].into_iter().enumerate() {
        let body = serde_json::json!({
            "Name": format!("tokened{index}"),
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/t.yaml"},
            "RoleArn": role_arn(),
            "ClientToken": token,
        });
        let (status, first) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}"
        );
        let (status, replay) = harness.call("CreateWorkflow", body).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the token replay to succeed, got {status} {replay:?}"
        );
        assert_eq!(
            replay, first,
            "case {token:?}: a repeated ClientToken must replay the first response, got {replay:?}"
        );

        // The same token with a DIFFERENT request is a conflict, not a second workflow.
        let (status, mismatch) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("tokened{index}-other"),
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/t.yaml"},
                    "RoleArn": role_arn(),
                    "ClientToken": token,
                }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {token:?}: expected ConflictException (409) for a reused token with a \
             different request, got {status} {mismatch:?}"
        );
    }

    // Exactly one workflow per token, never a duplicate.
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(status, 200, "expected ListWorkflows to succeed, got {status} {listed:?}");
    assert_eq!(
        listed["Workflows"].as_array().map(Vec::len),
        Some(3),
        "expected one workflow per ClientToken, got {:?}",
        listed["Workflows"]
    );
}

#[tokio::test]
async fn list_workflows_pages_over_every_persisted_workflow_exactly_once() {
    let harness = Harness::new("ap-southeast-2");

    // Seven workflows, so the page sizes below divide the set evenly and unevenly.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        let (status, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("paged-{index}"),
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/p.yaml"},
                    "RoleArn": role_arn(),
                    "Description": format!("workflow {index}"),
                }),
            )
            .await;
        assert_eq!(status, 200, "expected create {index} to succeed, got {created:?}");
        expected.push(
            created["WorkflowArn"]
                .as_str()
                .expect("a created workflow has an ARN")
                .to_string(),
        );
    }
    expected.sort();

    // `MaxResults` is modelled `@range(min: 1, max: 100)`: both published bounds plus
    // sizes smaller than, equal to and larger than the set.
    for page_size in [1_i64, 2, 3, 6, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = serde_json::json!({"MaxResults": page_size});
            if let Some(cursor) = token.as_deref() {
                request["NextToken"] = serde_json::Value::String(cursor.to_string());
            }
            let (status, page) = harness.call("ListWorkflows", request).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: expected ListWorkflows to succeed, got \
                 {status} {page:?}"
            );
            let items = page["Workflows"]
                .as_array()
                .expect("Workflows is a required list")
                .clone();
            assert!(
                items.len() as i64 <= page_size,
                "case MaxResults={page_size}: a page carried {} items",
                items.len()
            );
            for item in &items {
                let arn = item["WorkflowArn"]
                    .as_str()
                    .expect("WorkflowArn is required on a summary");
                assert!(
                    item["Name"].as_str().is_some_and(|n| n.starts_with("paged-")),
                    "case MaxResults={page_size}: expected the stored Name on the summary, \
                     got {:?}",
                    item["Name"]
                );
                assert!(
                    item["Description"]
                        .as_str()
                        .is_some_and(|d| d.starts_with("workflow ")),
                    "case MaxResults={page_size}: expected the stored Description on the \
                     summary, got {:?}",
                    item["Description"]
                );
                assert_eq!(
                    item["WorkflowStatus"], "READY",
                    "case MaxResults={page_size}: expected the stored status, got {:?}",
                    item["WorkflowStatus"]
                );
                seen.push(arn.to_string());
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case MaxResults={page_size}: pagination did not terminate"
            );
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case MaxResults={page_size}: paging must yield every workflow exactly once, \
             got {seen:?}"
        );
    }

    // The default `MaxResults` (20) covers the whole set in one page.
    let (status, defaulted) = harness.call("ListWorkflows", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListWorkflows with no MaxResults to succeed, got {status} {defaulted:?}"
    );
    assert_eq!(
        defaulted["Workflows"].as_array().map(Vec::len),
        Some(7),
        "the modelled default MaxResults of 20 must cover all seven, got {:?}",
        defaulted["Workflows"]
    );
    assert!(
        defaulted["NextToken"].is_null(),
        "a complete page must carry no NextToken, got {:?}",
        defaulted["NextToken"]
    );

    // A cursor this service did not issue is refused rather than silently ignored:
    // not hex at all, odd-length hex, hex of a NUL byte, and hex of "abc" — none of
    // which carries the marker the service's own cursors do.
    for bad in ["not-a-token", "zz", "00", "616263"] {
        let (status, body) = harness
            .call(
                "ListWorkflows",
                serde_json::json!({"MaxResults": 10, "NextToken": bad}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {bad:?}: expected ValidationException (400) for a foreign NextToken, got \
             {status} {body:?}"
        );
    }
}

/// The `OverrideParameters` documents the run table covers: a map<string, document>, so
/// the rows differ along the JSON type and nesting of the values.
fn override_parameter_cases() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("absent", serde_json::Value::Null),
        ("an empty map", serde_json::json!({})),
        ("a string value", serde_json::json!({"env": "test"})),
        (
            "scalars of every JSON type",
            serde_json::json!({
                "retries": 3,
                "ratio": 0.5,
                "dry_run": true,
                "note": "mixed",
            }),
        ),
        (
            "a nested object and a list",
            serde_json::json!({
                "config": {"region": "us-east-1", "tags": ["a", "b"]},
                "partitions": [1, 2, 3],
            }),
        ),
    ]
}

#[tokio::test]
async fn a_started_run_is_persisted_and_read_back_with_its_override_parameters() {
    for (case, parameters) in override_parameter_cases() {
        let harness = Harness::new("us-east-1");
        let (status, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": "runnable",
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/r.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: create failed: {created:?}");
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let workflow_version = created["WorkflowVersion"]
            .as_str()
            .expect("a created workflow has a version")
            .to_string();

        let mut start = serde_json::json!({"WorkflowArn": arn});
        if !parameters.is_null() {
            start["OverrideParameters"] = parameters.clone();
        }
        let (status, started) = harness.call("StartWorkflowRun", start).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
        );
        let run_id = started["RunId"].as_str().unwrap_or_default();
        assert!(
            is_run_id(run_id),
            "case {case:?}: expected a RunId matching the modelled pattern, got {run_id:?}"
        );
        assert_eq!(
            started["Status"], "QUEUED",
            "case {case:?}: a just-started run is still in its initial non-terminal status, \
             got {:?}",
            started["Status"]
        );
        assert!(
            started["StartedAt"].is_number(),
            "case {case:?}: expected a StartedAt timestamp, got {:?}",
            started["StartedAt"]
        );

        // ROUND TRIP: the run is read back out of the store.
        let (status, run) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {run:?}"
        );
        assert_eq!(
            run["RunId"], started["RunId"],
            "case {case:?}: GetWorkflowRun reported a different RunId: {:?}",
            run["RunId"]
        );
        assert_eq!(
            run["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the run's workflow ARN, got {:?}",
            run["WorkflowArn"]
        );
        assert_eq!(
            run["WorkflowVersion"].as_str(),
            Some(workflow_version.as_str()),
            "case {case:?}: a run defaults to the workflow's latest version, got {:?}",
            run["WorkflowVersion"]
        );
        assert_eq!(
            run["RunType"], "ON_DEMAND",
            "case {case:?}: a run started through StartWorkflowRun is ON_DEMAND, got {:?}",
            run["RunType"]
        );
        let expected_parameters = if parameters.is_null() {
            serde_json::json!({})
        } else {
            parameters.clone()
        };
        assert_eq!(
            run["OverrideParameters"], expected_parameters,
            "case {case:?}: OverrideParameters must come back exactly as supplied, got {:?}",
            run["OverrideParameters"]
        );
        let detail = &run["RunDetail"];
        assert_eq!(
            detail["RunId"], started["RunId"],
            "case {case:?}: RunDetail must carry the run's id, got {:?}",
            detail["RunId"]
        );
        assert_eq!(
            detail["RunType"], "ON_DEMAND",
            "case {case:?}: RunDetail must carry the run type, got {:?}",
            detail["RunType"]
        );
        assert_eq!(
            detail["RunState"], "QUEUED",
            "case {case:?}: RunDetail's RunState must match the run's status, got {:?}",
            detail["RunState"]
        );
        assert!(
            detail["CreatedAt"].is_number() && detail["StartedOn"].is_number(),
            "case {case:?}: RunDetail must carry CreatedAt and StartedOn, got {detail:?}"
        );
        for member in ["CompletedOn", "Duration", "ErrorMessage"] {
            assert!(
                detail[member].is_null(),
                "case {case:?}: {member} is null while the run is not complete, got {:?}",
                detail[member]
            );
        }

        // An id the workflow does not hold resolves no run, whatever its form.
        for unknown in ["doesnotexist", "run-0000000000000000", "a.b-c_d"] {
            let (status, body) = harness
                .call(
                    "GetWorkflowRun",
                    serde_json::json!({"WorkflowArn": arn, "RunId": unknown}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}/{unknown:?}: expected a 400 lookup miss, got \
                 {status} {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_run_leaves_its_non_terminal_status_when_the_clock_makes_the_change_due() {
    let harness = Harness::new("us-east-1");
    let (_, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "settling",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/s.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();
    let (_, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    let run_id = started["RunId"]
        .as_str()
        .expect("a started run has an id")
        .to_string();

    // Before the change falls due the caller still observes the initial status.
    let (_, queued) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        queued["RunDetail"]["RunState"], "QUEUED",
        "a run read back immediately is still QUEUED, got {:?}",
        queued["RunDetail"]["RunState"]
    );

    // The transition is owed by the clock, so it lands without anybody polling for it.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let (status, settled) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(status, 200, "expected GetWorkflowRun to succeed, got {settled:?}");
    let detail = &settled["RunDetail"];
    assert_eq!(
        detail["RunState"], "FAILED",
        "no Airflow engine runs the definition, so the run ends in the modelled failure \
         state, got {:?}",
        detail["RunState"]
    );
    assert!(
        detail["ErrorMessage"]
            .as_str()
            .is_some_and(|m| m.contains("Airflow")),
        "a failed run must say why it failed, got {:?}",
        detail["ErrorMessage"]
    );
    assert!(
        detail["CompletedOn"].is_number(),
        "a complete run carries CompletedOn, got {:?}",
        detail["CompletedOn"]
    );
    assert!(
        detail["Duration"].as_i64().is_some_and(|d| d >= 0),
        "a complete run carries a Duration, got {:?}",
        detail["Duration"]
    );

    // And the terminal status is stable across reads.
    let (_, again) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        again["RunDetail"], settled["RunDetail"],
        "a read must not change what a later caller sees, got {:?}",
        again["RunDetail"]
    );
}

#[tokio::test]
async fn starting_a_run_is_idempotent_and_an_unknown_workflow_is_refused() {
    let harness = Harness::new("us-east-1");
    let (_, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "idempotentRuns",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/i.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();

    let mut minted: Vec<String> = Vec::new();
    for token in ["run-token-a", "run-token-b"] {
        let request = serde_json::json!({
            "WorkflowArn": arn,
            "ClientToken": token,
            "OverrideParameters": {"token": token},
        });
        let (status, first) = harness.call("StartWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected StartWorkflowRun to succeed, got {status} {first:?}"
        );
        let (status, replay) = harness.call("StartWorkflowRun", request).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the token replay to succeed, got {status} {replay:?}"
        );
        assert_eq!(
            replay["RunId"], first["RunId"],
            "case {token:?}: a repeated ClientToken must not start a second run, got {:?}",
            replay["RunId"]
        );
        minted.push(
            first["RunId"]
                .as_str()
                .expect("a started run has an id")
                .to_string(),
        );
    }
    assert_ne!(
        minted[0], minted[1],
        "two different ClientTokens must start two different runs, got {minted:?}"
    );
    assert_eq!(
        harness.stored_run_count(&arn),
        2,
        "the store must hold exactly one run per ClientToken, got {}",
        harness.stored_run_count(&arn)
    );

    // Pattern-valid ARNs the store never held, and a version the workflow never had.
    let (status, body) = harness
        .call(
            "StartWorkflowRun",
            serde_json::json!({
                "WorkflowArn":
                    "arn:aws:airflow-serverless:us-east-1:000000000000:workflow/ghost-0123456789",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a 400 lookup miss for an unknown workflow, got {status} {body:?}"
    );

    // A `WorkflowVersion` of the modelled form (`^[a-z0-9]{32}$`) the workflow never had.
    let (status, body) = harness
        .call(
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": "0".repeat(32)}),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a 400 lookup miss for an unknown version, got {status} {body:?}"
    );
}

#[tokio::test]
async fn delete_removes_the_workflow_its_versions_and_its_runs() {
    let harness = Harness::new("us-west-2");

    // Rows differ along whether the delete names the version explicitly.
    for (case, name_the_version) in [("version omitted", false), ("version named", true)] {
        let (_, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": "deletable",
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/d.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let version = created["WorkflowVersion"]
            .as_str()
            .expect("a created workflow has a version")
            .to_string();
        let (_, started) = harness
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        let run_id = started["RunId"]
            .as_str()
            .expect("a started run has an id")
            .to_string();

        let mut request = serde_json::json!({"WorkflowArn": arn});
        if name_the_version {
            request["WorkflowVersion"] = serde_json::Value::String(version.clone());
        }
        let (status, deleted) = harness.call("DeleteWorkflow", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
        );
        assert_eq!(
            deleted["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: DeleteWorkflow must echo the ARN it deleted, got {:?}",
            deleted["WorkflowArn"]
        );
        assert_eq!(
            deleted["WorkflowVersion"].as_str(),
            Some(version.as_str()),
            "case {case:?}: DeleteWorkflow must echo the version it deleted, got {:?}",
            deleted["WorkflowVersion"]
        );

        // The workflow, its versions and its runs are all gone.
        let (status, _) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetWorkflow after a delete must be 400, got {status}"
        );
        let (status, _) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a versioned GetWorkflow after a delete must be 400, got {status}"
        );
        let (status, _) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetWorkflowRun after a delete must be 400, got {status}"
        );
        let (status, listed) = harness
            .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
            .await;
        assert_eq!(status, 200, "case {case:?}: ListWorkflows failed: {listed:?}");
        assert_eq!(
            listed["Workflows"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a deleted workflow must not be listed, got {:?}",
            listed["Workflows"]
        );
        let (status, _) = harness
            .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting an unknown ARN must be 400, got {status}"
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            0,
            "case {case:?}: the deleted workflow's runs must be gone too"
        );
    }
}

#[tokio::test]
async fn deleting_one_named_version_keeps_the_workflow_until_no_version_is_left() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ops::types::DefinitionS3Location;
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        WorkflowSnapshot, WorkflowVersionRecord,
    };

    // `DeleteWorkflowRequest.WorkflowVersion` is documented as "Optional. The specific
    // version of the workflow to delete. If not specified, all versions of the workflow
    // are deleted." The rule is defined over the SET of a workflow's versions, so each
    // row below has two of them. The second one is written straight into the store the
    // handlers read, rather than minted by `UpdateWorkflow`, so the row fixes both its
    // identifier and its `created_at` and can name the version it deletes exactly.
    let second = "b".repeat(32);
    for (case, delete_the_latest) in [("delete the latest", true), ("delete the older", false)] {
        let harness = Harness::new("us-west-2");
        let (_, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": "versioned",
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let first = created["WorkflowVersion"]
            .as_str()
            .expect("a created workflow has a version")
            .to_string();
        {
            let state = harness
                .engine
                .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
            let mut store = state.lock();
            let workflow = store.workflows.get_mut(&arn).expect("the created workflow");
            let created_at = workflow.created_at + Duration::from_secs(1);
            for record in &mut workflow.versions {
                record.is_latest = false;
            }
            workflow.versions.push(WorkflowVersionRecord {
                version: second.clone(),
                created_at,
                is_latest: true,
                snapshot: WorkflowSnapshot {
                    definition_s3_location: DefinitionS3Location {
                        bucket: "defs".to_string(),
                        object_key: "dags/v2.yaml".to_string(),
                        version_id: None,
                    },
                    code: None,
                    role_arn: role_arn(),
                    description: Some("the second version".to_string()),
                    encryption_configuration: None,
                    logging_configuration: None,
                    engine_version: None,
                    network_configuration: None,
                    trigger_mode: None,
                },
            });
            workflow.current_version = second.clone();
            workflow.code_snapshotted_at = created_at;
        }

        let (deleted, survivor) = if delete_the_latest {
            (second.clone(), first.clone())
        } else {
            (first.clone(), second.clone())
        };
        let (status, answered) = harness
            .call(
                "DeleteWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": deleted}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a version-scoped delete to succeed, got {status} \
             {answered:?}"
        );
        assert_eq!(
            answered["WorkflowVersion"].as_str(),
            Some(deleted.as_str()),
            "case {case:?}: the response must echo the version deleted, got {:?}",
            answered["WorkflowVersion"]
        );

        // One version is not all of them, so the workflow is still there, reporting the
        // newest version left.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a workflow with a version left must still be readable, got \
             {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowVersion"].as_str(),
            Some(survivor.as_str()),
            "case {case:?}: the workflow must report the newest version left, got {:?}",
            fetched["WorkflowVersion"]
        );
        let (status, _) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": deleted}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted version must be gone, got {status}"
        );

        // Deleting the last version is deleting all of them, so the record goes.
        let (status, answered) = harness
            .call(
                "DeleteWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": survivor}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the last version's delete to succeed, got {status} \
             {answered:?}"
        );
        let (status, _) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a workflow with no version left must be gone, got {status}"
        );
    }
}

#[tokio::test]
async fn the_caller_driven_stores_are_bounded() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        IdempotentResult, MAX_IDEMPOTENCY_TOKENS, MAX_RUNS_PER_WORKFLOW, MAX_WORKFLOWS,
    };

    let harness = Harness::new("us-east-1");

    // Fill the workflow store to the emulator's own ceiling, then show the next create
    // is refused rather than growing the process without bound.
    for index in 0..MAX_WORKFLOWS {
        let (status, body) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("bounded{index}"),
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/b.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case workflow {index}: expected a create below the ceiling to succeed, got \
             {status} {body:?}"
        );
    }
    let (status, refused) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "oneTooMany",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/b.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case one past MAX_WORKFLOWS: expected ServiceQuotaExceededException (402), got \
         {status} {refused:?}"
    );
    assert!(
        refused["__type"]
            .as_str()
            .is_some_and(|t| t.contains("ServiceQuotaExceededException")),
        "case one past MAX_WORKFLOWS: expected a ServiceQuotaExceededException, got {:?}",
        refused["__type"]
    );

    // The same bound on one workflow's runs, so workflows x runs is a finite product.
    let runs = Harness::new("us-east-1");
    let (_, created) = runs
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "boundedRuns",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/b.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();
    for index in 0..MAX_RUNS_PER_WORKFLOW {
        let (status, body) = runs
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case run {index}: expected a run below the ceiling to start, got {status} {body:?}"
        );
    }
    let (status, refused) = runs
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 402,
        "case one past MAX_RUNS_PER_WORKFLOW: expected ServiceQuotaExceededException (402), \
         got {status} {refused:?}"
    );
    assert_eq!(
        runs.stored_run_count(&arn),
        MAX_RUNS_PER_WORKFLOW,
        "the run store must stop at its ceiling, got {}",
        runs.stored_run_count(&arn)
    );

    // The idempotency ledger is keyed on a caller-chosen token, so it evicts rather
    // than growing: recording far more tokens than the cap leaves it at the cap.
    let ledger = Harness::new("us-east-1");
    let state = ledger
        .engine
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    for index in 0..(MAX_IDEMPOTENCY_TOKENS * 3) {
        state.lock().idempotency.record(
            format!("CreateWorkflow:token-{index}"),
            IdempotentResult::CreateWorkflow {
                request_fingerprint: format!("fingerprint-{index}"),
                workflow_arn: format!("arn-{index}"),
            },
        );
    }
    let remembered = state.lock().idempotency.len();
    assert_eq!(
        remembered, MAX_IDEMPOTENCY_TOKENS,
        "the idempotency ledger must stay at its cap, got {remembered}"
    );
    assert!(
        state.lock().idempotency.get("CreateWorkflow:token-0").is_none(),
        "the oldest token must have been evicted first"
    );
    assert!(
        state
            .lock()
            .idempotency
            .get(&format!(
                "CreateWorkflow:token-{}",
                MAX_IDEMPOTENCY_TOKENS * 3 - 1
            ))
            .is_some(),
        "the newest token must still be remembered"
    );
}

#[tokio::test]
async fn inputs_outside_the_modelled_constraints_are_refused() {
    let harness = Harness::new("us-east-1");
    let good_definition = serde_json::json!({"Bucket": "defs", "ObjectKey": "dags/v.yaml"});

    // Each row breaks ONE modelled constraint, named in the case label.
    let refused: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "Name with a leading '-' breaks ^[a-zA-Z0-9]+...",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "-leading-dash",
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "Name below the length minimum of 1",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "",
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "Name one past the length maximum of 255",
            "CreateWorkflow",
            serde_json::json!({
                "Name": format!("x{}", "9".repeat(255)),
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "RoleArn that is not an IAM role ARN",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "badRole",
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": "arn:aws:iam::000000000000:user/not-a-role",
            }),
        ),
        (
            "WorkflowArn that does not match the ARN pattern",
            "GetWorkflow",
            serde_json::json!({"WorkflowArn": "not-an-arn"}),
        ),
        (
            "WorkflowArn whose region segment is malformed",
            "GetWorkflow",
            serde_json::json!({
                "WorkflowArn":
                    "arn:aws:airflow-serverless:REGION:000000000000:workflow/demo-0123456789",
            }),
        ),
        (
            "MaxResults one below the range minimum of 1",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 0}),
        ),
        (
            "MaxResults one above the range maximum of 100",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 101}),
        ),
    ];
    for (case, operation, body) in refused {
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ValidationException (400), got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|t| t.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {:?}",
            answered["__type"]
        );
    }

    // Each published `MaxResults` bound itself is accepted.
    for page_size in [1, 100] {
        let (status, body) = harness
            .call("ListWorkflows", serde_json::json!({"MaxResults": page_size}))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={page_size}: a published bound must be accepted, got \
             {status} {body:?}"
        );
    }
}
