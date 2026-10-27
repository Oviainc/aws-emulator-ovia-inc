// SPDX-License-Identifier: CC0-1.0

//! The teardown a `DeleteEnvironment` starts, end to end over HTTP through the router this
//! service mounts.
//!
//! The rule every case here asserts: `DeleteEnvironment` only STARTS the removal of the
//! environment — it answers an empty 200 and the environment stays stored, so every reader
//! still reports it, in the non-terminal status the model documents for a teardown in
//! progress (`smithy_query(query="doc", shape="Environment", member="Status")`:
//! `DELETING` — "The request to delete the environment is in progress."), with its name
//! still taken and no change accepted on it; only once the teardown has come due does the
//! record go, and every reader then answers as it does for a name the store never held.
//!
//! The rule is about the DELETE, not about any one environment, so each row below varies
//! the state the environment was in when the delete arrived — settled, still being
//! created, in the middle of an accepted update, paused on caller-managed VPC endpoints —
//! and whether it carries tags that have to survive the teardown window and then go with
//! the record. Every expected status is named through this service's own constants rather
//! than spelled out twice.

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

/// The region the `AUTHORIZATION` scope above signs for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Slack added to a span before asserting what the clock already owes, so a loaded machine
/// does not fail an assertion about a change that is due.
const MARGIN: Duration = Duration::from_millis(400);

/// How often a wait below re-reads the environment. Far shorter than
/// [`state::TRANSITION_SPAN`], so no phase of a lifecycle slips between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
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

    async fn create(&self, name: &str, row: Option<&Row>) -> Answer {
        let mut body = json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
            "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
            "DagS3Path": "dags",
            "NetworkConfiguration": {
                "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                "SecurityGroupIds": ["sg-111"],
            },
        });
        if let Some(row) = row {
            if let Some(tags) = row.tags() {
                body["Tags"] = tags;
            }
            if row.customer_managed_endpoints {
                body["EndpointManagement"] = json!(state::ENDPOINT_MANAGEMENT_CUSTOMER);
            }
        }
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn get(&self, name: &str) -> Answer {
        self.send("GET", &format!("/environments/{name}"), None).await
    }

    async fn update(&self, name: &str, patch: Value) -> Answer {
        self.send("PATCH", &format!("/environments/{name}"), Some(patch))
            .await
    }

    async fn delete(&self, name: &str) -> Answer {
        self.send("DELETE", &format!("/environments/{name}"), None)
            .await
    }

    async fn list_tags(&self, arn: &str) -> Answer {
        self.send("GET", &format!("/tags/{}", encode_label(arn)), None)
            .await
    }

    /// The names `ListEnvironments` reports.
    async fn listed(&self) -> Vec<String> {
        let answer = self.send("GET", "/environments", None).await;
        assert_eq!(
            answer.status, 200,
            "expected ListEnvironments 200, got {} {}",
            answer.status, answer.body
        );
        answer
            .body
            .get("Environments")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `Status` a successful `GetEnvironment` reports.
    async fn status_of(&self, case: &str, name: &str) -> String {
        let answer = self.get(name).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected GetEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        answer
            .body
            .pointer("/Environment/Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: GetEnvironment reported no Status"))
            .to_string()
    }

    /// Wait until the environment settles into `AVAILABLE`, so a row that wants a settled
    /// environment deletes one.
    async fn wait_until_available(&self, case: &str, name: &str) {
        let deadline = Instant::now() + state::TRANSITION_SPAN * 20;
        loop {
            let status = self.status_of(case, name).await;
            if status == state::STATUS_AVAILABLE {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the create never settled into {}, last saw {status}",
                state::STATUS_AVAILABLE
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// One row: the state the environment is in when the delete arrives, and what it carries.
struct Row {
    /// The environment name, which is also the case label.
    name: &'static str,
    /// Whether the create is left to settle into `AVAILABLE` before the delete.
    settle_first: bool,
    /// Whether an accepted update is started (and left unsettled) before the delete.
    update_first: bool,
    /// Whether the create asks the CALLER to own the environment's VPC endpoints, which
    /// pauses it in `PENDING`.
    customer_managed_endpoints: bool,
    /// The tags the create carries, which must outlive the delete and go with the record.
    tags: &'static [(&'static str, &'static str)],
    /// The statuses this row's lifecycle can be in at the instant the delete arrives. More
    /// than one where the row deletes an environment MID-lifecycle: the transition the
    /// clock owes it may already have landed on a loaded machine, and the rule the delete
    /// follows is the same whichever of them it arrives in.
    statuses_before_delete: &'static [&'static str],
}

impl Row {
    fn tags(&self) -> Option<Value> {
        if self.tags.is_empty() {
            return None;
        }
        let mut map = serde_json::Map::new();
        for (key, value) in self.tags {
            map.insert((*key).to_string(), json!(value));
        }
        Some(Value::Object(map))
    }

    fn arn(&self) -> String {
        format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{}", self.name)
    }
}

/// The states a delete is asserted over. Each row differs in the status the environment is
/// in when the delete arrives — the statuses this service's own lifecycles reach — and in
/// whether the record carries tags.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "SettledEnv",
            settle_first: true,
            update_first: false,
            customer_managed_endpoints: false,
            tags: &[],
            statuses_before_delete: &[state::STATUS_AVAILABLE],
        },
        Row {
            name: "SettledTaggedEnv",
            settle_first: true,
            update_first: false,
            customer_managed_endpoints: false,
            tags: &[("Team", "data-platform"), ("Env", "staging")],
            statuses_before_delete: &[state::STATUS_AVAILABLE],
        },
        Row {
            name: "StillCreatingEnv",
            settle_first: false,
            update_first: false,
            customer_managed_endpoints: false,
            tags: &[],
            statuses_before_delete: &[state::STATUS_CREATING, state::STATUS_AVAILABLE],
        },
        Row {
            name: "MidUpdateEnv",
            settle_first: true,
            update_first: true,
            customer_managed_endpoints: false,
            tags: &[("Team", "platform")],
            statuses_before_delete: &[
                state::STATUS_CREATING_SNAPSHOT,
                state::STATUS_UPDATING,
                state::STATUS_AVAILABLE,
            ],
        },
        Row {
            name: "PausedEnv",
            settle_first: false,
            update_first: false,
            customer_managed_endpoints: true,
            tags: &[],
            statuses_before_delete: &[state::STATUS_PENDING],
        },
    ]
}

/// Bring one row's environment to the state its delete has to arrive in.
async fn seed(harness: &Harness, row: &Row) {
    let created = harness.create(row.name, Some(row)).await;
    assert_eq!(
        created.status, 200,
        "case {}: expected CreateEnvironment 200, got {} {}",
        row.name, created.status, created.body
    );
    if row.settle_first {
        harness.wait_until_available(row.name, row.name).await;
    }
    if row.update_first {
        let updated = harness
            .update(row.name, json!({"MaxWorkers": 7}))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {}: expected UpdateEnvironment 200, got {} {}",
            row.name, updated.status, updated.body
        );
    }
    let status = harness.status_of(row.name, row.name).await;
    assert!(
        row.statuses_before_delete.contains(&status.as_str()),
        "case {}: the delete must arrive in one of {:?}, got {status}",
        row.name,
        row.statuses_before_delete
    );
}

#[tokio::test]
async fn a_delete_leaves_the_environment_readable_in_the_teardown_status() {
    for row in rows() {
        let harness = Harness::new();
        seed(&harness, &row).await;

        let deleted = harness.delete(row.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {}: expected DeleteEnvironment 200, got {} {}",
            row.name, deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {}: DeleteEnvironment answers an empty body, got {}",
            row.name,
            deleted.body
        );

        // The teardown is in progress, not finished: the record is still there and every
        // reader reports it in the status that says so.
        let status = harness.status_of(row.name, row.name).await;
        assert_eq!(
            status,
            state::STATUS_DELETING,
            "case {}: a read straight after the delete must report {}, got {status}",
            row.name,
            state::STATUS_DELETING
        );
        let listed = harness.listed().await;
        assert_eq!(
            listed,
            vec![row.name.to_string()],
            "case {}: ListEnvironments must still name an environment being torn down, \
             got {listed:?}",
            row.name
        );

        // The tags live on the record, so they are still readable for as long as it is.
        let tags = harness.list_tags(&row.arn()).await;
        assert_eq!(
            tags.status, 200,
            "case {}: the ARN of an environment being torn down still resolves, got {} {}",
            row.name, tags.status, tags.body
        );
        assert_eq!(
            tags.body.get("Tags").cloned(),
            row.tags(),
            "case {}: the tags of an environment being torn down, got {}",
            row.name,
            tags.body
        );
    }
}

#[tokio::test]
async fn an_environment_being_torn_down_accepts_no_change_and_keeps_its_name() {
    for row in rows() {
        let harness = Harness::new();
        seed(&harness, &row).await;
        assert_eq!(
            harness.delete(row.name).await.status,
            200,
            "case {}: delete",
            row.name
        );

        // Nothing can be applied to a record on its way out: the refusal names the status
        // it is actually in and the one status an update is accepted in.
        let refused = harness
            .update(row.name, json!({"MaxWorkers": 11}))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {}: an update of an environment being torn down must be refused, got {} {}",
            row.name, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {}: the refusal's error code, got {}",
            row.name,
            refused.body
        );
        assert_eq!(
            refused.message(),
            state::update_not_allowed_message(row.name, state::STATUS_DELETING),
            "case {}: the refusal's message, got {}",
            row.name,
            refused.message()
        );

        // And the name is still taken while the teardown runs, so it cannot be created
        // again — nor can the delete be replayed as an idempotent create.
        let taken = harness.create(row.name, Some(&row)).await;
        assert_eq!(
            taken.status, 400,
            "case {}: a create of a name still held by a teardown must be refused, got {} {}",
            row.name, taken.status, taken.body
        );
        assert_eq!(
            taken.message(),
            format!("An environment named {} already exists", row.name),
            "case {}: the refusal's message, got {}",
            row.name,
            taken.message()
        );
    }
}

#[tokio::test]
async fn every_reader_loses_the_environment_once_its_teardown_comes_due() {
    for row in rows() {
        let harness = Harness::new();
        seed(&harness, &row).await;
        assert_eq!(
            harness.delete(row.name).await.status,
            200,
            "case {}: delete",
            row.name
        );
        tokio::time::sleep(state::TRANSITION_SPAN + MARGIN).await;

        let gone = harness.get(row.name).await;
        assert_eq!(
            gone.status, 404,
            "case {}: GetEnvironment after the teardown came due, got {} {}",
            row.name, gone.status, gone.body
        );
        assert_eq!(
            gone.error_code(),
            "ResourceNotFoundException",
            "case {}: the error code of a read of a removed environment, got {}",
            row.name,
            gone.body
        );
        let listed = harness.listed().await;
        assert!(
            listed.is_empty(),
            "case {}: ListEnvironments after the teardown came due, got {listed:?}",
            row.name
        );
        let tags = harness.list_tags(&row.arn()).await;
        assert_eq!(
            tags.status, 404,
            "case {}: the ARN of a removed environment must resolve to nothing, got {} {}",
            row.name, tags.status, tags.body
        );
        let again = harness.delete(row.name).await;
        assert_eq!(
            again.status, 404,
            "case {}: DeleteEnvironment of a removed environment, got {} {}",
            row.name, again.status, again.body
        );

        // The name the teardown held is free again, and the environment created under it
        // is a NEW one: it starts its own lifecycle rather than reporting the teardown.
        let recreated = harness.create(row.name, Some(&row)).await;
        assert_eq!(
            recreated.status, 200,
            "case {}: the freed name must be creatable again, got {} {}",
            row.name, recreated.status, recreated.body
        );
        let status = harness.status_of(row.name, row.name).await;
        assert_ne!(
            status,
            state::STATUS_DELETING,
            "case {}: a re-created environment must not report the old teardown, got {status}",
            row.name
        );
    }
}

#[tokio::test]
async fn repeating_the_delete_neither_fails_nor_postpones_the_teardown() {
    let harness = Harness::new();
    let name = "PolledEnv";
    assert_eq!(
        harness.create(name, None).await.status,
        200,
        "case {name}: create"
    );
    harness.wait_until_available(name, name).await;

    let started = Instant::now();
    assert_eq!(
        harness.delete(name).await.status,
        200,
        "case {name}: the first delete"
    );

    // A caller polling the teardown with the delete itself gets the same empty 200 for as
    // long as the record is there, and the deadline the FIRST delete set stands: the
    // record is gone one transition span after that call, however many repeats landed in
    // between.
    while started.elapsed() < state::TRANSITION_SPAN {
        let repeated = harness.delete(name).await;
        assert_eq!(
            repeated.status, 200,
            "case {name}: a repeated delete during the teardown, got {} {}",
            repeated.status, repeated.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    tokio::time::sleep(MARGIN).await;
    let gone = harness.get(name).await;
    assert_eq!(
        gone.status, 404,
        "case {name}: a repeated delete must not postpone the teardown, got {} {}",
        gone.status, gone.body
    );
}
