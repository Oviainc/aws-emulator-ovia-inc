// SPDX-License-Identifier: CC0-1.0

//! The status sequence an accepted `UpdateEnvironment` walks, end to end over HTTP through
//! the router this service mounts.
//!
//! The rule every case here asserts: an accepted update of a settled environment answers
//! 200 and then walks `CREATING_SNAPSHOT` (Amazon MWAA snapshots the environment's
//! database cluster before the change is applied), then `UPDATING`, then `AVAILABLE` —
//! with `LastUpdate.Status` `PENDING` in every non-terminal status of that walk and
//! `SUCCESS` once it settles. The sequence belongs to the update itself, not to any
//! particular member, so the rows below vary what the patch carries: an Apache Airflow
//! version upgrade, a downgrade, the same version re-specified, a sizing change, an
//! environment-class change, a logging change, a networking change, and a patch carrying
//! several of those at once.
//!
//! The expected statuses are the `Environment.Status` values the member's own doc
//! enumerates (`smithy_query(query="doc", shape="Environment", member="Status")`), named
//! here through the service's own constants rather than spelled out twice.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// How often the status poll below reads the environment. Far shorter than
/// [`state::UPDATE_PHASE_SPAN`], so no phase of the update can slip between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long a poll waits for a lifecycle to settle before it calls the walk broken. Many
/// times the whole update span, so a slow machine does not fail the assertion.
const SETTLE_DEADLINE: Duration = Duration::from_secs(10);

/// The Apache Airflow version every environment here is created on: not the newest and not
/// the oldest supported one, so both an upgrade and a downgrade are reachable from it.
const SEED_AIRFLOW_VERSION: &str = "2.10.1";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

/// The emulator, assembled as the router assembles it: one shared engine, one service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body().to_vec();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// A create carrying the required members, on [`SEED_AIRFLOW_VERSION`].
    async fn create(&self, name: &str) -> Answer {
        let body = json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
            "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
            "DagS3Path": "dags",
            "AirflowVersion": SEED_AIRFLOW_VERSION,
            "NetworkConfiguration": {
                "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                "SecurityGroupIds": ["sg-111"],
            },
        });
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn update(&self, name: &str, patch: &Value) -> Answer {
        self.send("PATCH", &format!("/environments/{name}"), Some(patch.clone()))
            .await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, case: &str, name: &str) -> Value {
        let answer = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected GetEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        answer
            .body
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: GetEnvironment answered without Environment"))
    }
}

/// The status of an environment, read through `GetEnvironment`.
async fn status_of(harness: &Harness, case: &str, name: &str) -> String {
    let environment = harness.environment(case, name).await;
    environment
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: GetEnvironment reported no Status"))
        .to_string()
}

/// Poll until the environment reaches `AVAILABLE`, answering the DISTINCT statuses
/// observed in the order they were observed.
///
/// `check_last_update` is on for an update (where `LastUpdate` must be `PENDING` until the
/// update lands and `SUCCESS` after) and off for the create, which records no `LastUpdate`
/// at all.
async fn observe_until_available(
    harness: &Harness,
    case: &str,
    name: &str,
    check_last_update: bool,
) -> Vec<String> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut observed: Vec<String> = Vec::new();
    loop {
        let environment = harness.environment(case, name).await;
        let status = environment
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: GetEnvironment reported no Status"))
            .to_string();
        if check_last_update {
            let expected = if status == state::STATUS_AVAILABLE {
                state::UPDATE_STATUS_SUCCESS
            } else {
                state::UPDATE_STATUS_PENDING
            };
            let reported = environment.pointer("/LastUpdate/Status").cloned();
            assert_eq!(
                reported,
                Some(json!(expected)),
                "case {case}: in status {status} LastUpdate.Status must be {expected}, got {reported:?}"
            );
        }
        if observed.last().map(String::as_str) != Some(status.as_str()) {
            observed.push(status.clone());
        }
        if status == state::STATUS_AVAILABLE {
            return observed;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the lifecycle never reached {} within {SETTLE_DEADLINE:?}, \
             observed {observed:?}",
            state::STATUS_AVAILABLE
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row: a patch an accepted update carries, and the member a read must report back.
struct Row {
    label: &'static str,
    patch: Value,
    /// A member of the `Environment` the patch changes, with the value it must read back
    /// as — so each row proves the change landed as well as that the statuses walked.
    reads_back: (&'static str, Value),
}

/// The accepted patches the rule is asserted over. Each row differs in what the update
/// changes: the Apache Airflow version (up, down, and unchanged), the sizing, the
/// environment class, the logging configuration, the networking, and several at once.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an AirflowVersion upgrade",
            patch: json!({"AirflowVersion": "2.11.0"}),
            reads_back: ("AirflowVersion", json!("2.11.0")),
        },
        Row {
            label: "an AirflowVersion downgrade",
            patch: json!({"AirflowVersion": "2.8.1"}),
            reads_back: ("AirflowVersion", json!("2.8.1")),
        },
        Row {
            label: "the stored AirflowVersion re-specified",
            patch: json!({"AirflowVersion": SEED_AIRFLOW_VERSION}),
            reads_back: ("AirflowVersion", json!(SEED_AIRFLOW_VERSION)),
        },
        Row {
            label: "a sizing change, no version",
            patch: json!({"MinWorkers": 2, "MaxWorkers": 12}),
            reads_back: ("MaxWorkers", json!(12)),
        },
        Row {
            label: "an EnvironmentClass change",
            patch: json!({"EnvironmentClass": "mw1.medium"}),
            reads_back: ("EnvironmentClass", json!("mw1.medium")),
        },
        Row {
            label: "a LoggingConfiguration change",
            patch: json!({"LoggingConfiguration": {"TaskLogs": {"Enabled": true, "LogLevel": "WARNING"}}}),
            reads_back: ("AirflowVersion", json!(SEED_AIRFLOW_VERSION)),
        },
        Row {
            label: "a NetworkConfiguration change",
            patch: json!({"NetworkConfiguration": {"SecurityGroupIds": ["sg-222", "sg-333"]}}),
            reads_back: ("AirflowVersion", json!(SEED_AIRFLOW_VERSION)),
        },
        Row {
            label: "a version change together with several details",
            patch: json!({
                "AirflowVersion": "3.0.6",
                "Schedulers": 3,
                "WeeklyMaintenanceWindowStart": "TUE:03:30",
                "WorkerReplacementStrategy": "GRACEFUL"
            }),
            reads_back: ("WeeklyMaintenanceWindowStart", json!("TUE:03:30")),
        },
    ]
}

#[tokio::test]
async fn every_accepted_update_walks_creating_snapshot_then_updating_then_available() {
    let harness = Harness::new();
    let expected = vec![
        state::STATUS_CREATING_SNAPSHOT.to_string(),
        state::STATUS_UPDATING.to_string(),
        state::STATUS_AVAILABLE.to_string(),
    ];

    for (index, row) in rows().iter().enumerate() {
        let name = format!("UpdateWalkRow{index}");
        let case = format!("{} {}", row.label, row.patch);

        let created = harness.create(&name).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the fixture create must succeed, got {} {}",
            created.status, created.body
        );
        // A create walks CREATING -> AVAILABLE, so the update below starts from a settled
        // environment rather than from a lifecycle still in flight.
        let creating = observe_until_available(&harness, &case, &name, false).await;
        assert_eq!(
            creating,
            vec![
                state::STATUS_CREATING.to_string(),
                state::STATUS_AVAILABLE.to_string()
            ],
            "case {case}: a create walks CREATING then AVAILABLE, observed {creating:?}"
        );

        let answer = harness.update(&name, &row.patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateEnvironment 200, got {} {}",
            answer.status, answer.body
        );

        let observed = observe_until_available(&harness, &case, &name, true).await;
        assert_eq!(
            observed, expected,
            "case {case}: an accepted update must walk {expected:?}, observed {observed:?}"
        );

        // The change the update carried landed: the walk is not a status animation over an
        // environment nothing was written to.
        let (member, value) = &row.reads_back;
        let settled = harness.environment(&case, &name).await;
        assert_eq!(
            settled.get(*member),
            Some(value),
            "case {case}: {member} must read back as the update stored it, got {:?}",
            settled.get(*member)
        );
        assert_eq!(
            settled.pointer("/LastUpdate/Source").cloned(),
            Some(json!(state::UPDATE_SOURCE_API)),
            "case {case}: LastUpdate.Source names what drove the change, got {:?}",
            settled.pointer("/LastUpdate/Source")
        );
    }
}

#[tokio::test]
async fn the_snapshot_phase_is_observable_before_any_other_call_is_made() {
    // The first status an accepted update reports is CREATING_SNAPSHOT, read straight
    // after the update answers — no poll loop, no sleep: the phase is not instantaneous,
    // so a client that treats it as a phase of its own wait loop reaches that branch.
    let harness = Harness::new();
    let name = "SnapshotFirst";
    assert_eq!(
        harness.create(name).await.status,
        200,
        "the fixture create must succeed"
    );
    tokio::time::sleep(state::TRANSITION_SPAN + POLL_INTERVAL).await;
    assert_eq!(
        status_of(&harness, "seed", name).await,
        state::STATUS_AVAILABLE,
        "the fixture environment must settle into {} before the update",
        state::STATUS_AVAILABLE
    );

    for patch in [
        json!({"AirflowVersion": "2.11.0"}),
        json!({"MaxWorkers": 7}),
    ] {
        let case = format!("{patch}");
        let answer = harness.update(name, &patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            status_of(&harness, &case, name).await,
            state::STATUS_CREATING_SNAPSHOT,
            "case {case}: the status read straight after an accepted update must be {}",
            state::STATUS_CREATING_SNAPSHOT
        );

        // And the whole walk is done one TRANSITION_SPAN after the call, like a create.
        tokio::time::sleep(state::TRANSITION_SPAN + POLL_INTERVAL).await;
        assert_eq!(
            status_of(&harness, &case, name).await,
            state::STATUS_AVAILABLE,
            "case {case}: the update must have settled {:?} after the call",
            state::TRANSITION_SPAN
        );
    }
}
