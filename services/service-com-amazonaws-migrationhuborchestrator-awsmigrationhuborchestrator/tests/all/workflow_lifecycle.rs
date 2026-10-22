// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the migration-workflow lifecycle.
//!
//! Every case drives real restJson1 requests through a `MultiServiceRouter` with this
//! service mounted, so the request walk (route -> deserialize -> validate -> handle ->
//! serialize) is the same one the emulator serves in production.
//!
//! The rule under test: every workflow response member is computed from the workflow
//! record this service persisted, so a create is observable through a later
//! get/list and a start/stop/delete is observable through a later get/list. The
//! tables below span that class rather than one recorded request: the `StepInput`
//! union variants, parameter/target/tag counts, optional members present and absent,
//! page sizes, filters, and the published `maxResults` bounds.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_migrationhuborchestrator_awsmigrationhuborchestrator as mho;
use smithy_server_runtime::MultiServiceRouter;

/// Long enough that every clock-owed status transition
/// ([`mho::state::STATUS_SETTLE_AFTER`]) has come due.
fn wait_for_status_to_settle() {
    std::thread::sleep(mho::state::STATUS_SETTLE_AFTER + Duration::from_millis(250));
}

/// What one request answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the parsed body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn id(&self) -> &str {
        self.body["id"].as_str().unwrap_or_default()
    }
}

/// One emulator with this service mounted, answering over its real HTTP surface.
struct Emulator {
    router: MultiServiceRouter,
    region: &'static str,
}

impl Emulator {
    fn in_region(region: &'static str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pinned off so the suite does not depend on the process-wide auth switch;
        // the Authorization header below is still what routes the request and names
        // the region every ARN carries.
        router.set_request_authentication(false);
        router.register_service(mho::EmulatedService::new(engine));
        Self { router, region }
    }

    fn new() -> Self {
        Self::in_region("us-west-2")
    }

    /// A SigV4 credential scope naming this service and region. `resolve_route_key`
    /// reads it to pick the claiming service, and the handlers read its region.
    fn authorization(&self) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTACCESSKEY/20260101/{}/migrationhub-orchestrator/aws4_request, \
             SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000",
            self.region
        )
    }

    async fn call(&self, method: &str, target: &str, body: Option<Value>) -> Reply {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(target)
            .header("authorization", self.authorization());
        let payload = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize request body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = builder.body(payload).expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create(&self, request: Value) -> Reply {
        self.call("POST", "/migrationworkflow/", Some(request)).await
    }

    async fn get(&self, id: &str) -> Reply {
        self.call("GET", &format!("/migrationworkflow/{id}"), None)
            .await
    }

    async fn start(&self, id: &str) -> Reply {
        self.call("POST", &format!("/migrationworkflow/{id}/start"), None)
            .await
    }

    async fn stop(&self, id: &str) -> Reply {
        self.call("POST", &format!("/migrationworkflow/{id}/stop"), None)
            .await
    }

    async fn delete(&self, id: &str) -> Reply {
        self.call("DELETE", &format!("/migrationworkflow/{id}"), None)
            .await
    }

    async fn list(&self, query: &str) -> Reply {
        self.call("GET", &format!("/migrationworkflows?{query}"), None)
            .await
    }

    /// ListWorkflows with no query string at all, so every optional member and
    /// `maxResults`' modelled default are what the handler sees.
    async fn list_without_query(&self) -> Reply {
        self.call("GET", "/migrationworkflows", None).await
    }
}

/// One CreateWorkflow request, and what the stored record must therefore report.
struct CreateCase {
    case: &'static str,
    request: Value,
}

/// The create table. Rows differ along what the rule generalises over: which
/// `StepInput` union variant each input parameter carries (all four the union
/// declares in `src/ops/types/shared_types.rs`, generated from the Smithy model),
/// how many parameters/targets/tags there are, and which optional members are
/// present at all.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "integerValue parameter, every optional member present",
            request: json!({
                "name": "demo-wf",
                "templateId": "tmpl-1",
                "inputParameters": { "Source": { "integerValue": 1 } },
                "description": "the first workflow",
                "applicationConfigurationId": "ads-config-1",
                "stepTargets": ["server-a"],
                "tags": { "env": "test" },
            }),
        },
        CreateCase {
            case: "stringValue parameter, no optional member at all",
            request: json!({
                "name": "minimal",
                "templateId": "tmpl-2",
                "inputParameters": { "Region": { "stringValue": "eu-west-1" } },
            }),
        },
        CreateCase {
            case: "listOfStringsValue parameter, three step targets",
            request: json!({
                "name": "fan-out",
                "templateId": "tmpl-2",
                "inputParameters": { "Hosts": { "listOfStringsValue": ["a", "b", "c"] } },
                "stepTargets": ["server-a", "server-b", "server-c"],
            }),
        },
        CreateCase {
            case: "mapOfStringValue parameter, two tags",
            request: json!({
                "name": "mapped",
                "templateId": "tmpl-3",
                "inputParameters": { "Credentials": { "mapOfStringValue": { "user": "root" } } },
                "tags": { "env": "prod", "team": "migrations" },
            }),
        },
        CreateCase {
            case: "four parameters mixing every union variant",
            request: json!({
                "name": "mixed inputs",
                "templateId": "tmpl-3",
                "inputParameters": {
                    "Count": { "integerValue": 42 },
                    "Name": { "stringValue": "mixed" },
                    "Hosts": { "listOfStringsValue": ["h1", "h2"] },
                    "Extra": { "mapOfStringValue": { "k": "v" } },
                },
                "description": "",
                "stepTargets": [],
                "tags": {},
            }),
        },
    ]
}

/// The one member the request does not supply: `id`. Checked against
/// `MigrationWorkflowId`'s modelled constraints (len 1..100, `^[a-zA-Z0-9-]+$`;
/// see `OPERATIONS.md`).
fn assert_id_is_wire_legal(case: &str, id: &str) {
    assert!(
        (1..=100).contains(&id.len()),
        "case {case:?}: generated id must satisfy MigrationWorkflowId len 1..100, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: generated id must satisfy ^[a-zA-Z0-9-]+$, got {id:?}"
    );
}

#[tokio::test]
async fn a_created_workflow_is_read_back_out_of_state() {
    let emulator = Emulator::new();
    for CreateCase { case, request } in create_cases() {
        let created = emulator.create(request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateWorkflow must answer 200, got {created:?}"
        );

        let id = created.id().to_string();
        assert_id_is_wire_legal(case, &id);
        assert_eq!(
            created.body["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:migrationhub-orchestrator:us-west-2:{}:workflow/{id}",
                    engine::EMULATOR_ACCOUNT_ID
                )
                .as_str()
            ),
            "case {case:?}: arn must name the signed region, the emulator account and the new id, got {:?}",
            created.body["arn"]
        );
        assert_eq!(
            created.body["status"].as_str(),
            Some("CREATING"),
            "case {case:?}: a freshly created workflow is CREATING, got {:?}",
            created.body["status"]
        );
        assert!(
            created.body["creationTime"].as_f64().is_some(),
            "case {case:?}: creationTime must come off the server clock, got {:?}",
            created.body["creationTime"]
        );

        // Echo-only members must equal what was persisted, under their OUTPUT names.
        assert_eq!(
            created.body["name"], request["name"],
            "case {case:?}: CreateWorkflow name, got {:?}",
            created.body["name"]
        );
        assert_eq!(
            created.body["templateId"], request["templateId"],
            "case {case:?}: CreateWorkflow templateId, got {:?}",
            created.body["templateId"]
        );
        assert_eq!(
            created.body["workflowInputs"], request["inputParameters"],
            "case {case:?}: workflowInputs must be the stored inputParameters, got {:?}",
            created.body["workflowInputs"]
        );
        assert_eq!(
            created.body["stepTargets"],
            request
                .get("stepTargets")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: stepTargets, got {:?}",
            created.body["stepTargets"]
        );
        assert_eq!(
            created.body["tags"],
            request.get("tags").cloned().unwrap_or_else(|| json!({})),
            "case {case:?}: tags, got {:?}",
            created.body["tags"]
        );

        // THE ROUND TRIP: GetWorkflow reports the same record, not the request.
        let fetched = emulator.get(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetWorkflow must answer 200, got {fetched:?}"
        );
        for member in ["id", "arn", "name", "templateId", "workflowInputs", "tags"] {
            assert_eq!(
                fetched.body[member], created.body[member],
                "case {case:?}: GetWorkflow {member} must equal what CreateWorkflow persisted, got {:?}",
                fetched.body[member]
            );
        }
        assert_eq!(
            fetched.body["creationTime"], created.body["creationTime"],
            "case {case:?}: creationTime must be stable across reads, got {:?}",
            fetched.body["creationTime"]
        );
        assert_eq!(
            fetched.body["description"],
            request.get("description").cloned().unwrap_or(Value::Null),
            "case {case:?}: GetWorkflow description, got {:?}",
            fetched.body["description"]
        );
        assert_eq!(
            fetched.body["adsApplicationConfigurationId"],
            request
                .get("applicationConfigurationId")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: GetWorkflow adsApplicationConfigurationId, got {:?}",
            fetched.body["adsApplicationConfigurationId"]
        );
        // No step exists until the workflow-step operations are implemented, and
        // nothing has started the workflow, so these carry no value yet.
        for absent in [
            "lastStartTime",
            "lastStopTime",
            "lastModifiedTime",
            "endTime",
            "totalSteps",
            "completedSteps",
        ] {
            assert_eq!(
                fetched.body[absent],
                Value::Null,
                "case {case:?}: GetWorkflow {absent} must be absent on a freshly created workflow, got {:?}",
                fetched.body[absent]
            );
        }
    }
}

#[tokio::test]
async fn every_created_workflow_gets_its_own_id_and_record() {
    let emulator = Emulator::new();
    let mut ids = Vec::new();
    for CreateCase { case, request } in create_cases() {
        let created = emulator.create(request).await;
        let id = created.id().to_string();
        assert!(
            !ids.contains(&id),
            "case {case:?}: CreateWorkflow reused an id already in state: {id:?}"
        );
        ids.push(id);
    }
    for id in &ids {
        let fetched = emulator.get(id).await;
        assert_eq!(
            fetched.status, 200,
            "every created workflow must still be readable: {id:?} got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn the_arn_region_comes_from_the_requests_credential_scope() {
    // Two rows of one table: the same create against two signing regions.
    for region in ["us-east-1", "eu-central-1"] {
        let emulator = Emulator::in_region(region);
        let created = emulator
            .create(json!({
                "name": "regional",
                "templateId": "tmpl-1",
                "inputParameters": { "Source": { "stringValue": "s" } },
            }))
            .await;
        let expected = format!(
            "arn:aws:migrationhub-orchestrator:{region}:{}:workflow/{}",
            engine::EMULATOR_ACCOUNT_ID,
            created.id()
        );
        assert_eq!(
            created.body["arn"].as_str(),
            Some(expected.as_str()),
            "case {region:?}: arn must carry the signed region, got {:?}",
            created.body["arn"]
        );
        let fetched = emulator.get(created.id()).await;
        assert_eq!(
            fetched.body["arn"], created.body["arn"],
            "case {region:?}: GetWorkflow must report the stored arn, got {:?}",
            fetched.body["arn"]
        );
    }
}

#[tokio::test]
async fn start_and_stop_are_observable_through_get() {
    let emulator = Emulator::new();

    // One row per create case: the lifecycle holds for every shape of workflow.
    for CreateCase { case, request } in create_cases() {
        let id = emulator.create(request).await.id().to_string();

        let started = emulator.start(&id).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartWorkflow must answer 200, got {started:?}"
        );
        assert_eq!(
            started.body["status"].as_str(),
            Some("STARTING"),
            "case {case:?}: StartWorkflow reports the transient status, got {:?}",
            started.body["status"]
        );
        let last_start = started.body["lastStartTime"].as_f64();
        assert!(
            last_start.is_some(),
            "case {case:?}: StartWorkflow must stamp lastStartTime, got {:?}",
            started.body["lastStartTime"]
        );
        assert_eq!(
            started.body["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: StartWorkflow answers out of the stored record, got {:?}",
            started.body["id"]
        );

        let after_start = emulator.get(&id).await;
        assert!(
            ["STARTING", "IN_PROGRESS"]
                .contains(&after_start.body["status"].as_str().unwrap_or_default()),
            "case {case:?}: after StartWorkflow the status is STARTING or the settled IN_PROGRESS, got {:?}",
            after_start.body["status"]
        );
        assert_eq!(
            after_start.body["lastStartTime"].as_f64(),
            last_start,
            "case {case:?}: GetWorkflow must report the stored lastStartTime, got {:?}",
            after_start.body["lastStartTime"]
        );

        wait_for_status_to_settle();
        let settled = emulator.get(&id).await;
        assert_eq!(
            settled.body["status"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: STARTING settles to IN_PROGRESS once the clock makes it due, got {:?}",
            settled.body["status"]
        );

        let stopped = emulator.stop(&id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: StopWorkflow must answer 200, got {stopped:?}"
        );
        assert_eq!(
            stopped.body["status"].as_str(),
            Some("PAUSING"),
            "case {case:?}: StopWorkflow reports the transient status, got {:?}",
            stopped.body["status"]
        );
        let last_stop = stopped.body["lastStopTime"].as_f64();
        assert!(
            last_stop.is_some(),
            "case {case:?}: StopWorkflow must stamp lastStopTime, got {:?}",
            stopped.body["lastStopTime"]
        );

        wait_for_status_to_settle();
        let after_stop = emulator.get(&id).await;
        assert_eq!(
            after_stop.body["status"].as_str(),
            Some("PAUSED"),
            "case {case:?}: PAUSING settles to PAUSED, got {:?}",
            after_stop.body["status"]
        );
        assert_eq!(
            after_stop.body["lastStopTime"].as_f64(),
            last_stop,
            "case {case:?}: GetWorkflow must report the stored lastStopTime, got {:?}",
            after_stop.body["lastStopTime"]
        );
        assert_eq!(
            after_stop.body["lastStartTime"].as_f64(),
            last_start,
            "case {case:?}: stopping must not disturb the stored lastStartTime, got {:?}",
            after_stop.body["lastStartTime"]
        );
    }
}

#[tokio::test]
async fn reading_a_workflow_repeatedly_does_not_move_it() {
    let emulator = Emulator::new();
    let id = emulator
        .create(json!({
            "name": "stable",
            "templateId": "tmpl-1",
            "inputParameters": { "Source": { "integerValue": 7 } },
        }))
        .await
        .id()
        .to_string();
    wait_for_status_to_settle();

    // Once settled, nothing is owed, so any number of reads report the same record.
    let first = emulator.get(&id).await;
    for round in 0..4 {
        let again = emulator.get(&id).await;
        assert_eq!(
            again.body, first.body,
            "case read #{round}: a read must not change what a later caller sees, got {:?}",
            again.body
        );
    }
    let listed = emulator.list("maxResults=0").await;
    assert_eq!(
        listed.body["migrationWorkflowSummary"][0]["status"].as_str(),
        Some("NOT_STARTED"),
        "case list-after-reads: CREATING settles to NOT_STARTED, got {:?}",
        listed.body["migrationWorkflowSummary"][0]["status"]
    );
}

/// Create one workflow per row and return the ids in creation order.
async fn seed(emulator: &Emulator, rows: &[(&str, &str)]) -> Vec<String> {
    let mut ids = Vec::new();
    for (name, template_id) in rows {
        let created = emulator
            .create(json!({
                "name": name,
                "templateId": template_id,
                "inputParameters": { "Source": { "stringValue": name } },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "seeding {name:?} must succeed, got {created:?}"
        );
        ids.push(created.id().to_string());
    }
    ids
}

const SEED_ROWS: &[(&str, &str)] = &[
    ("alpha", "tmpl-1"),
    ("beta", "tmpl-2"),
    ("gamma", "tmpl-1"),
    ("delta", "tmpl-3"),
    ("epsilon", "tmpl-1"),
];

/// Walk every page of `query`, following `nextToken`, and return the ids seen.
async fn page_through(emulator: &Emulator, query: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..32 {
        let target = match &token {
            Some(value) => format!("{query}&nextToken={value}"),
            None => query.to_string(),
        };
        let reply = emulator.list(&target).await;
        assert_eq!(
            reply.status, 200,
            "case {target:?}: ListWorkflows must answer 200, got {reply:?}"
        );
        let summaries = reply.body["migrationWorkflowSummary"]
            .as_array()
            .unwrap_or_else(|| panic!("case {target:?}: migrationWorkflowSummary is required"))
            .clone();
        for summary in &summaries {
            seen.push(summary["id"].as_str().unwrap_or_default().to_string());
        }
        match reply.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return seen,
        }
    }
    panic!("case {query:?}: pagination did not terminate after 32 pages, saw {seen:?}");
}

#[tokio::test]
async fn list_workflows_pages_every_stored_workflow_exactly_once() {
    let emulator = Emulator::new();
    let created = seed(&emulator, SEED_ROWS).await;

    // Page sizes spanning the published `maxResults` range 0..100: both bounds, a
    // size that divides the set, one that does not, and one larger than the set.
    for max_results in [0_i32, 1, 2, 3, 5, 6, 100] {
        let seen = page_through(&emulator, &format!("maxResults={max_results}")).await;
        assert_eq!(
            seen, created,
            "case maxResults={max_results}: paging must yield every stored workflow once, in creation order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_workflows_truncates_at_max_results() {
    let emulator = Emulator::new();
    seed(&emulator, SEED_ROWS).await;

    for (max_results, expected_len, expects_token) in
        [(1_i32, 1_usize, true), (2, 2, true), (5, 5, false), (100, 5, false)]
    {
        let reply = emulator.list(&format!("maxResults={max_results}")).await;
        let page = reply.body["migrationWorkflowSummary"]
            .as_array()
            .expect("migrationWorkflowSummary is required")
            .len();
        assert_eq!(
            page, expected_len,
            "case maxResults={max_results}: page length, got {page}"
        );
        assert_eq!(
            reply.body["nextToken"].is_string(),
            expects_token,
            "case maxResults={max_results}: nextToken is present only when a page remains, got {:?}",
            reply.body["nextToken"]
        );
    }

    // `maxResults=0` is the modelled default: it asks for the service's default page
    // size (`DEFAULT_PAGE_SIZE`), which is larger than this seed, so one page holds
    // the lot. It is NOT "no truncation" — see the resource-guard test.
    let all = emulator.list("maxResults=0").await;
    assert!(
        SEED_ROWS.len() < mho::state::DEFAULT_PAGE_SIZE,
        "this case only reads as one page while the seed is smaller than the default page size"
    );
    assert_eq!(
        all.body["migrationWorkflowSummary"]
            .as_array()
            .expect("migrationWorkflowSummary is required")
            .len(),
        SEED_ROWS.len(),
        "case maxResults=0: the default page size holds this whole seed, got {:?}",
        all.body["migrationWorkflowSummary"]
    );
    assert!(
        all.body["nextToken"].is_null(),
        "case maxResults=0: the last page carries no nextToken, got {:?}",
        all.body["nextToken"]
    );
}

#[tokio::test]
async fn max_results_outside_its_published_range_is_refused() {
    let emulator = Emulator::new();
    // The bounds `MaxResults` publishes (range 0..100, per OPERATIONS.md), and one
    // step past each of them.
    for (max_results, expected) in [("0", 200_u16), ("100", 200), ("101", 400), ("-1", 400)] {
        let reply = emulator.list(&format!("maxResults={max_results}")).await;
        assert_eq!(
            reply.status, expected,
            "case maxResults={max_results}: expected HTTP {expected}, got {reply:?}"
        );
    }
}

#[tokio::test]
async fn list_workflows_honours_every_modelled_filter() {
    let emulator = Emulator::new();
    let created = seed(&emulator, SEED_ROWS).await;
    // Settle first, so the status filter is asserted against a known status.
    wait_for_status_to_settle();

    let by_index = |indexes: &[usize]| -> Vec<String> {
        indexes.iter().map(|i| created[*i].clone()).collect()
    };

    // (query, expected ids) — one row per modelled filter, including a value that
    // matches nothing and a value the issue did not cite.
    let cases: Vec<(String, Vec<String>)> = vec![
        ("templateId=tmpl-1".to_string(), by_index(&[0, 2, 4])),
        ("templateId=tmpl-2".to_string(), by_index(&[1])),
        ("templateId=tmpl-9".to_string(), Vec::new()),
        ("name=delta".to_string(), by_index(&[3])),
        ("name=epsilon".to_string(), by_index(&[4])),
        ("name=nobody".to_string(), Vec::new()),
        ("status=NOT_STARTED".to_string(), created.clone()),
        ("status=COMPLETED".to_string(), Vec::new()),
        (
            "templateId=tmpl-1&name=gamma".to_string(),
            by_index(&[2]),
        ),
        ("templateId=tmpl-1&name=beta".to_string(), Vec::new()),
        // No Application Discovery Service application name is resolved in this
        // slice, so no stored workflow carries one to match.
        (
            "adsApplicationConfigurationName=some-app".to_string(),
            Vec::new(),
        ),
    ];

    for (filter, expected) in cases {
        let seen = page_through(&emulator, &format!("maxResults=2&{filter}")).await;
        assert_eq!(
            seen, expected,
            "case {filter:?}: filtered paging must yield exactly the matching stored workflows, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_workflow_summaries_are_built_from_the_stored_record() {
    let emulator = Emulator::new();
    let created = emulator
        .create(json!({
            "name": "summarised",
            "templateId": "tmpl-7",
            "inputParameters": { "Source": { "integerValue": 3 } },
            "tags": { "env": "test" },
        }))
        .await;
    let fetched = emulator.get(created.id()).await;
    let listed = emulator.list("maxResults=1").await;
    let summary = &listed.body["migrationWorkflowSummary"][0];

    for (summary_member, get_member) in [
        ("id", "id"),
        ("name", "name"),
        ("templateId", "templateId"),
        ("creationTime", "creationTime"),
        ("statusMessage", "statusMessage"),
        ("endTime", "endTime"),
        ("totalSteps", "totalSteps"),
        ("completedSteps", "completedSteps"),
        ("adsApplicationConfigurationName", "adsApplicationName"),
    ] {
        assert_eq!(
            summary[summary_member], fetched.body[get_member],
            "case summary {summary_member:?}: ListWorkflows and GetWorkflow read one record, got {:?} vs {:?}",
            summary[summary_member], fetched.body[get_member]
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator, SEED_ROWS).await;
    for token in ["not-a-token", "zz", "%7Bjson%7D"] {
        let reply = emulator
            .list(&format!("maxResults=2&nextToken={token}"))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: a foreign nextToken is a client error, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {token:?}: a foreign nextToken is refused as ValidationException, got {:?}",
            reply.error_type
        );
    }
}

#[tokio::test]
async fn delete_removes_the_workflow_from_every_reader() {
    let emulator = Emulator::new();
    let created = seed(&emulator, SEED_ROWS).await;

    // Delete every seeded workflow in turn; each one must vanish from get and list
    // while the others stay exactly as they were.
    let mut remaining = created.clone();
    for id in &created {
        let deleted = emulator.delete(id).await;
        assert_eq!(
            deleted.status, 202,
            "case {id:?}: DeleteWorkflow's modelled success status is 202, got {deleted:?}"
        );
        assert_eq!(
            deleted.body["status"].as_str(),
            Some("DELETING"),
            "case {id:?}: DeleteWorkflow reports the deleting status, got {:?}",
            deleted.body["status"]
        );
        assert_eq!(
            deleted.body["id"].as_str(),
            Some(id.as_str()),
            "case {id:?}: DeleteWorkflow answers out of the record it removed, got {:?}",
            deleted.body["id"]
        );
        assert!(
            deleted.body["arn"].as_str().is_some_and(|arn| arn.ends_with(id)),
            "case {id:?}: DeleteWorkflow reports the stored arn, got {:?}",
            deleted.body["arn"]
        );

        let gone = emulator.get(id).await;
        assert_eq!(
            gone.status, 404,
            "case {id:?}: a deleted workflow is not found, got {gone:?}"
        );

        let again = emulator.delete(id).await;
        assert_eq!(
            again.status, 404,
            "case {id:?}: deleting a workflow that is already gone is not found, got {again:?}"
        );

        remaining.retain(|known| known != id);
        let seen = page_through(&emulator, "maxResults=2").await;
        assert_eq!(
            seen, remaining,
            "case {id:?}: ListWorkflows must show exactly the surviving workflows, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_workflow_id_is_resource_not_found() {
    let emulator = Emulator::new();
    // Live workflows exist, so a 404 is about the id and not about an empty store.
    seed(&emulator, SEED_ROWS).await;

    // Every id-addressed workflow operation, over several wire-legal ids that name
    // no stored workflow.
    for id in ["does-not-exist", "wf-0000000000000000000", "A", &"z".repeat(100)] {
        for (operation, reply) in [
            ("GetWorkflow", emulator.get(id).await),
            ("StartWorkflow", emulator.start(id).await),
            ("StopWorkflow", emulator.stop(id).await),
            ("DeleteWorkflow", emulator.delete(id).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {operation} {id:?}: an unknown id is HTTP 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {operation} {id:?}: an unknown id is ResourceNotFoundException, got {:?}",
                reply.error_type
            );
        }
    }
}

#[tokio::test]
async fn the_statuses_this_service_stores_are_wire_legal() {
    // Source of truth: MIGRATION_WORKFLOW_STATUS_ENUM_VALUES in
    // `src/ops/types/shared_types.rs`, generated from the Smithy model.
    for status in [
        mho::state::STATUS_CREATING,
        mho::state::STATUS_NOT_STARTED,
        mho::state::STATUS_STARTING,
        mho::state::STATUS_IN_PROGRESS,
        mho::state::STATUS_PAUSING,
        mho::state::STATUS_PAUSED,
        mho::state::STATUS_DELETING,
    ] {
        assert!(
            mho::ops::types::MIGRATION_WORKFLOW_STATUS_ENUM_VALUES.contains(&status),
            "case {status:?}: every stored status must be a MigrationWorkflowStatusEnum value, which {:?} does not list",
            mho::ops::types::MIGRATION_WORKFLOW_STATUS_ENUM_VALUES
        );
    }
}

#[tokio::test]
async fn a_pagination_cursor_round_trips_through_its_encoding() {
    // The cursor is opaque but must survive a round trip for every creation
    // position it can name, and must satisfy `nextToken`'s len 0..2048 / `.*\S.*`.
    for position in [0_u64, 1, 7, 1_000, u64::from(u32::MAX), u64::MAX] {
        let token = mho::state::encode_page_token(position);
        assert!(
            !token.is_empty() && token.len() <= 2048 && !token.contains(char::is_whitespace),
            "case {position}: a cursor must satisfy nextToken len 0..2048 and .*\\S.*, got {token:?}"
        );
        assert_eq!(
            mho::state::decode_page_token(&token),
            Some(position),
            "case {position}: a cursor must decode back to the position it names, got {:?}",
            mho::state::decode_page_token(&token)
        );
    }
    // Not hex, odd-length hex, hex of text without this service's prefix, and hex of
    // the prefix with no position: none of them names a page.
    for foreign in [
        "",
        "not-hex",
        "0",
        &hex::encode("7"),
        &hex::encode("mhow1:"),
        &hex::encode("mhow1:nope"),
        &hex::encode("mhow1:-1"),
    ] {
        assert!(
            mho::state::decode_page_token(foreign).is_none(),
            "case {foreign:?}: a token this service did not mint must decode to nothing, got {:?}",
            mho::state::decode_page_token(foreign)
        );
    }
}

#[tokio::test]
async fn a_running_workflow_must_be_paused_before_it_can_be_deleted() {
    let emulator = Emulator::new();
    // The modelled documentation for DeleteWorkflow: "You must pause a running
    // workflow in Migration Hub Orchestrator console to delete it." One row per
    // running status the workflow passes through.
    for settle_first in [false, true] {
        let id = emulator
            .create(json!({
                "name": "running",
                "templateId": "tmpl-1",
                "inputParameters": { "Source": { "stringValue": "s" } },
            }))
            .await
            .id()
            .to_string();
        emulator.start(&id).await;
        if settle_first {
            // STARTING has settled to IN_PROGRESS; still running either way.
            wait_for_status_to_settle();
        }

        let status = emulator.get(&id).await.body["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let refused = emulator.delete(&id).await;
        assert_eq!(
            refused.status, 400,
            "case settled={settle_first} ({status}): deleting a running workflow is refused, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case settled={settle_first} ({status}): the refusal is ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            emulator.get(&id).await.status,
            200,
            "case settled={settle_first} ({status}): a refused delete must leave the workflow in state"
        );

        // Paused, the same delete is allowed.
        emulator.stop(&id).await;
        let deleted = emulator.delete(&id).await;
        assert_eq!(
            deleted.status, 202,
            "case settled={settle_first} ({status}): a paused workflow deletes, got {deleted:?}"
        );
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_resource_guard() {
    let emulator = Emulator::new();
    // Fill the store to its bound, then show that one more create is refused and
    // that freeing a record lets exactly one more through.
    for index in 0..mho::state::MAX_LIVE_WORKFLOWS {
        let created = emulator
            .create(json!({
                "name": "filler",
                "templateId": "tmpl-1",
                "inputParameters": { "Source": { "integerValue": 1 } },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case fill #{index}: a create below the bound must succeed, got {created:?}"
        );
    }

    let refused = emulator
        .create(json!({
            "name": "one-too-many",
            "templateId": "tmpl-1",
            "inputParameters": { "Source": { "integerValue": 1 } },
        }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case at-bound: a create past MAX_LIVE_WORKFLOWS is refused, got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ValidationException"),
        "case at-bound: the refusal is ValidationException, got {:?}",
        refused.error_type
    );

    // One small page, whatever the store holds: maxResults absent means the default
    // page size, not the whole store.
    let unbounded = emulator.list_without_query().await;
    assert_eq!(
        unbounded
            .body["migrationWorkflowSummary"]
            .as_array()
            .expect("migrationWorkflowSummary is required")
            .len(),
        mho::state::DEFAULT_PAGE_SIZE,
        "case no maxResults: a page is capped at the default page size, got {:?}",
        unbounded.body["nextToken"]
    );
    assert!(
        unbounded.body["nextToken"].is_string(),
        "case no maxResults: a capped page must carry a cursor, got {:?}",
        unbounded.body["nextToken"]
    );

    let first_id = unbounded.body["migrationWorkflowSummary"][0]["id"]
        .as_str()
        .expect("a page entry carries its id")
        .to_string();
    assert_eq!(
        emulator.delete(&first_id).await.status,
        202,
        "case free-one: deleting a workflow must succeed"
    );
    let after_free = emulator
        .create(json!({
            "name": "after-free",
            "templateId": "tmpl-1",
            "inputParameters": { "Source": { "integerValue": 1 } },
        }))
        .await;
    assert_eq!(
        after_free.status, 200,
        "case free-one: freeing a record lets another create through, got {after_free:?}"
    );
}

#[tokio::test]
async fn a_cursor_survives_the_deletion_of_the_workflow_it_names() {
    let emulator = Emulator::new();
    let created = seed(&emulator, SEED_ROWS).await;

    // Page once, delete everything the page returned (so the cursor's own workflow
    // is gone), then follow the cursor: the remaining workflows must still come
    // back, never a silently empty page.
    let first = emulator.list("maxResults=2").await;
    let token = first.body["nextToken"]
        .as_str()
        .expect("a truncated page carries a cursor")
        .to_string();
    for summary in first.body["migrationWorkflowSummary"]
        .as_array()
        .expect("migrationWorkflowSummary is required")
    {
        let id = summary["id"].as_str().expect("a page entry carries its id");
        assert_eq!(
            emulator.delete(id).await.status,
            202,
            "case delete {id:?}: deleting a page entry must succeed"
        );
    }

    let resumed = emulator
        .list(&format!("maxResults=2&nextToken={token}"))
        .await;
    let seen: Vec<String> = resumed.body["migrationWorkflowSummary"]
        .as_array()
        .expect("migrationWorkflowSummary is required")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        seen,
        created[2..4].to_vec(),
        "case stale cursor: a page must resume after the cursor's creation position even when that workflow is gone, got {seen:?}"
    );
}
