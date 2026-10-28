// SPDX-License-Identifier: CC0-1.0

//! WHEN the configuration an accepted `UpdateEnvironment` carries becomes visible, end to
//! end over HTTP through the router this service mounts.
//!
//! The rule every case here asserts: an accepted update is applied by the PHASE that
//! applies it, not by the call that accepts it — while the environment is still in a phase
//! that precedes the apply (Amazon MWAA snapshots the environment's database cluster first,
//! so it can restore the environment if the update fails), every member reads back with the
//! value the environment is running on and the update reports itself as not yet landed;
//! from the apply phase on, every member the patch carried reads back with its new value.
//! So the new configuration is visible EXACTLY when the environment has left the snapshot
//! phase, which is what each row below polls for.
//!
//! The rule belongs to the update itself rather than to any one member, so the rows vary
//! what the patch carries and how: a scalar string, a scalar number, a pair of numbers, an
//! enumerated value, a nested structure, a map, a member the create left absent, and
//! several members at once. The statuses and update statuses are named through the
//! service's own constants, whose values are the ones `Environment.Status` and
//! `LastUpdate.Status` enumerate (`smithy_query(query="doc", shape="Environment",
//! member="Status")` and `smithy_query(query="doc", shape="LastUpdate", member="Status")`).

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

/// How often a row reads the environment while its update walks the phases. Far shorter
/// than [`state::UPDATE_PHASE_SPAN`], so no phase can slip between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a transition the clock owes has
/// certainly come due.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// How long a row waits for its update to settle before it calls the walk broken. Many
/// times the whole update span, so a slow machine does not fail the assertion.
const SETTLE_DEADLINE: Duration = Duration::from_secs(10);

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

    /// A create carrying the required members plus the row's own seed members.
    async fn create(&self, name: &str, seed: &Value) -> Answer {
        let mut body = json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
            "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
            "DagS3Path": "dags",
            "NetworkConfiguration": {
                "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                "SecurityGroupIds": ["sg-111"],
            },
        });
        let target = body.as_object_mut().expect("an object body");
        for (member, value) in seed.as_object().expect("an object of seed members") {
            target.insert(member.clone(), value.clone());
        }
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

/// The reported `Environment` without the two members that describe the LIFECYCLE rather
/// than the configuration, so two reads can be compared on configuration alone.
fn configuration_of(environment: &Value) -> Value {
    let mut reduced = environment.clone();
    let object = reduced.as_object_mut().expect("an object Environment");
    object.remove("Status");
    object.remove("LastUpdate");
    reduced
}

fn status_of(case: &str, environment: &Value) -> String {
    environment
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: GetEnvironment reported no Status"))
        .to_string()
}

/// One row: what the environment was created with, what the patch changes, and the values
/// the patch must make readable — by JSON pointer, so a nested member is named as precisely
/// as a top-level one.
struct Row {
    label: &'static str,
    /// Members the create carries beyond the required ones, so each member the patch names
    /// already has a stored value to be observed INSTEAD of the patched one.
    seed: Value,
    patch: Value,
    /// Each `(JSON pointer, value)` the patch must make readable.
    applied: Vec<(&'static str, Value)>,
}

/// The accepted patches the rule is asserted over, differing in the shape of what they
/// carry: a scalar string, a pair of numbers, a single number, an enumerated value, a
/// nested structure, a map value, a member the create left absent, and several at once.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a scalar string: an AirflowVersion upgrade",
            seed: json!({"AirflowVersion": "2.10.1"}),
            patch: json!({"AirflowVersion": "2.11.0"}),
            applied: vec![("/AirflowVersion", json!("2.11.0"))],
        },
        Row {
            label: "a scalar string: an AirflowVersion downgrade",
            seed: json!({"AirflowVersion": "2.10.1"}),
            patch: json!({"AirflowVersion": "2.8.1"}),
            applied: vec![("/AirflowVersion", json!("2.8.1"))],
        },
        Row {
            label: "a pair of numbers: the worker counts",
            seed: json!({"MinWorkers": 1, "MaxWorkers": 10}),
            patch: json!({"MinWorkers": 3, "MaxWorkers": 12}),
            applied: vec![("/MinWorkers", json!(3)), ("/MaxWorkers", json!(12))],
        },
        Row {
            label: "a single number: the scheduler count",
            seed: json!({"EnvironmentClass": "mw1.large", "Schedulers": 2}),
            patch: json!({"Schedulers": 4}),
            applied: vec![("/Schedulers", json!(4))],
        },
        Row {
            label: "an enumerated value: the web-server access mode",
            seed: json!({"WebserverAccessMode": "PUBLIC_ONLY"}),
            patch: json!({"WebserverAccessMode": "PRIVATE_ONLY"}),
            applied: vec![("/WebserverAccessMode", json!("PRIVATE_ONLY"))],
        },
        Row {
            label: "a nested structure: one log type's level",
            seed: json!({"LoggingConfiguration": {"TaskLogs": {"Enabled": true, "LogLevel": "INFO"}}}),
            patch: json!({"LoggingConfiguration": {"TaskLogs": {"Enabled": true, "LogLevel": "WARNING"}}}),
            applied: vec![(
                "/LoggingConfiguration/TaskLogs/LogLevel",
                json!("WARNING"),
            )],
        },
        Row {
            label: "a nested list: the security groups",
            seed: json!({}),
            patch: json!({"NetworkConfiguration": {"SecurityGroupIds": ["sg-222", "sg-333"]}}),
            applied: vec![(
                "/NetworkConfiguration/SecurityGroupIds",
                json!(["sg-222", "sg-333"]),
            )],
        },
        Row {
            label: "a map value: an Apache Airflow configuration option",
            seed: json!({"AirflowConfigurationOptions": {"core.parallelism": "32"}}),
            patch: json!({"AirflowConfigurationOptions": {"core.parallelism": "128"}}),
            applied: vec![(
                "/AirflowConfigurationOptions/core.parallelism",
                json!("128"),
            )],
        },
        Row {
            label: "members the create left absent: an S3 object and its version",
            seed: json!({}),
            patch: json!({
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "zZ9yY8xX7wW",
            }),
            applied: vec![
                ("/RequirementsS3Path", json!("requirements.txt")),
                ("/RequirementsS3ObjectVersion", json!("zZ9yY8xX7wW")),
            ],
        },
        Row {
            label: "several members at once, one of them the update's own strategy",
            seed: json!({"AirflowVersion": "2.10.1", "EnvironmentClass": "mw1.small"}),
            patch: json!({
                "AirflowVersion": "3.0.6",
                "EnvironmentClass": "mw1.medium",
                "WeeklyMaintenanceWindowStart": "TUE:03:30",
                "WorkerReplacementStrategy": "GRACEFUL",
            }),
            applied: vec![
                ("/AirflowVersion", json!("3.0.6")),
                ("/EnvironmentClass", json!("mw1.medium")),
                ("/WeeklyMaintenanceWindowStart", json!("TUE:03:30")),
            ],
        },
    ]
}

fn env_name(index: usize) -> String {
    format!("ApplyPhaseRow{index}")
}

#[tokio::test]
async fn an_accepted_update_is_visible_exactly_once_the_phase_that_applies_it_comes_due() {
    let harness = Harness::new();
    let rows = rows();

    for (index, row) in rows.iter().enumerate() {
        let answer = harness.create(&env_name(index), &row.seed).await;
        assert_eq!(
            answer.status, 200,
            "case {}: the fixture create must succeed, got {} {}",
            row.label, answer.status, answer.body
        );
    }
    // One wait for every row: an update is accepted only once the create has settled.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (index, row) in rows.iter().enumerate() {
        let name = env_name(index);
        let case = format!("{} {}", row.label, row.patch);
        let before = harness.environment(&case, &name).await;
        assert_eq!(
            status_of(&case, &before).as_str(),
            state::STATUS_AVAILABLE,
            "case {case}: the fixture environment must be settled before the update"
        );

        let answer = harness.update(&name, &row.patch).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateEnvironment 200, got {} {}",
            answer.status, answer.body
        );

        // Poll the whole walk. Every read is judged by the phase it lands in, so the rule
        // is asserted as a biconditional: the patched values are readable if and only if
        // the environment has left the phase that precedes the apply.
        let deadline = Instant::now() + SETTLE_DEADLINE;
        let mut saw_snapshot_phase = false;
        loop {
            let during = harness.environment(&case, &name).await;
            let status = status_of(&case, &during);
            let last_update = during.pointer("/LastUpdate/Status").cloned();
            if status == state::STATUS_CREATING_SNAPSHOT {
                saw_snapshot_phase = true;
                assert_eq!(
                    configuration_of(&during),
                    configuration_of(&before),
                    "case {case}: in status {status} the environment must still report the \
                     configuration it is running on, got {}",
                    configuration_of(&during)
                );
                assert_eq!(
                    last_update,
                    Some(json!(state::UPDATE_STATUS_PENDING)),
                    "case {case}: in status {status} LastUpdate.Status must be {}, got \
                     {last_update:?}",
                    state::UPDATE_STATUS_PENDING
                );
            } else {
                assert!(
                    status == state::STATUS_UPDATING || status == state::STATUS_AVAILABLE,
                    "case {case}: an accepted update walks only the statuses of an update, \
                     got {status}"
                );
                for (pointer, expected) in &row.applied {
                    let reported = during.pointer(pointer).cloned();
                    assert_eq!(
                        reported.as_ref(),
                        Some(expected),
                        "case {case}: in status {status} {pointer} must read back as the \
                         patch left it, got {reported:?}"
                    );
                }
                let expected_update_status = if status == state::STATUS_AVAILABLE {
                    state::UPDATE_STATUS_SUCCESS
                } else {
                    state::UPDATE_STATUS_PENDING
                };
                assert_eq!(
                    last_update,
                    Some(json!(expected_update_status)),
                    "case {case}: in status {status} LastUpdate.Status must be \
                     {expected_update_status}, got {last_update:?}"
                );
                if status == state::STATUS_AVAILABLE {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the update never settled within {SETTLE_DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        assert!(
            saw_snapshot_phase,
            "case {case}: the phase that precedes the apply must be observable — nothing \
             was read in status {}",
            state::STATUS_CREATING_SNAPSHOT
        );

        // The update's own metadata is recorded when the call is accepted, because it
        // describes the UPDATE rather than the configuration the environment is running.
        let settled = harness.environment(&case, &name).await;
        if let Some(strategy) = row.patch.get("WorkerReplacementStrategy") {
            assert_eq!(
                settled.pointer("/LastUpdate/WorkerReplacementStrategy"),
                Some(strategy),
                "case {case}: LastUpdate.WorkerReplacementStrategy must be what the update \
                 asked for, got {:?}",
                settled.pointer("/LastUpdate/WorkerReplacementStrategy")
            );
        }
        // Nothing the patch did not carry moved: the apply phase applies the merge, not a
        // fresh configuration.
        for (member, value) in before.as_object().expect("an object Environment") {
            let patched = row.patch.get(member).is_some()
                || row
                    .applied
                    .iter()
                    .any(|(pointer, _)| pointer.starts_with(&format!("/{member}")));
            if patched || member == "Status" || member == "LastUpdate" {
                continue;
            }
            assert_eq!(
                settled.get(member),
                Some(value),
                "case {case}: {member} was not in the patch and must be unchanged, got {:?}",
                settled.get(member)
            );
        }
    }
}

#[tokio::test]
async fn a_second_update_is_judged_against_the_configuration_the_first_one_landed() {
    // The deferral is a deferral, not a loss: once the apply phase has come due the stored
    // configuration IS the merged one, so the next update merges onto it and the business
    // rules are judged against it. Here the first update raises the worker ceiling and the
    // second one raises the floor to it — a patch that would be refused against the
    // configuration the environment started on.
    let harness = Harness::new();
    let name = "ApplyPhaseChain";
    let case = "a second update merged onto the first";
    assert_eq!(
        harness
            .create(name, &json!({"MinWorkers": 1, "MaxWorkers": 2}))
            .await
            .status,
        200,
        "case {case}: the fixture create must succeed"
    );
    tokio::time::sleep(PAST_TRANSITION).await;

    assert_eq!(
        harness.update(name, &json!({"MaxWorkers": 20})).await.status,
        200,
        "case {case}: the first update must be accepted"
    );
    tokio::time::sleep(PAST_TRANSITION).await;

    let answer = harness.update(name, &json!({"MinWorkers": 20})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: a floor the LANDED ceiling admits must be accepted, got {} {}",
        answer.status, answer.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;

    let settled = harness.environment(case, name).await;
    for (member, expected) in [("MinWorkers", json!(20)), ("MaxWorkers", json!(20))] {
        assert_eq!(
            settled.get(member),
            Some(&expected),
            "case {case}: {member} must read back as the two updates left it, got {:?}",
            settled.get(member)
        );
    }
}
