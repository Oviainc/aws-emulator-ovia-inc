// SPDX-License-Identifier: CC0-1.0

//! The create lifecycle an environment follows, as a function of WHO manages its VPC
//! endpoints, end to end over HTTP through the router this service mounts in production.
//!
//! The rule every case asserts: a create whose VPC endpoints the caller manages is paused
//! at creation — the environment reports the paused status and stays there, because no
//! operation of this service reports that the caller's endpoints now exist, and the only
//! status change the clock owes it is the create failure once the documented grace period
//! has passed — while a create whose endpoints the service manages proceeds from "being
//! created" to "ready".
//!
//! The table below varies what the rule generalises over: the `EndpointManagement` value
//! (both the model lists, plus the request that names none), the environment class and
//! sizing the mode is combined with, the web-server access mode, and whether the create
//! carries tags and logging at all. The enum catalog the rows draw from is this service's
//! own model (`smithy_query(query="shape", shape="EndpointManagement")` → CUSTOMER |
//! SERVICE); no row is privileged.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Every `EndpointManagement` value, from
/// `smithy_query(query="shape", shape="EndpointManagement")`.
const ENDPOINT_MANAGEMENT_MODES: [&str; 2] = ["CUSTOMER", "SERVICE"];

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a status change the clock owes
/// has certainly come due.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

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

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
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
        let body = response.into_body().to_vec();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error}: {}",
                    String::from_utf8_lossy(&body)
                )
            })
        };
        (status, json)
    }

    async fn create(&self, case: &Case) -> (u16, Value) {
        self.send(
            "PUT",
            &format!("/environments/{}", case.name),
            Some(case.create_body()),
        )
        .await
    }

    async fn update(&self, name: &str, body: Value) -> (u16, Value) {
        self.send("PATCH", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn delete(&self, name: &str) -> (u16, Value) {
        self.send("DELETE", &format!("/environments/{name}"), None)
            .await
    }

    async fn list(&self) -> (u16, Value) {
        self.send("GET", "/environments", None).await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, name: &str) -> Value {
        let (status, json) = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {name}: expected GetEnvironment 200, got {status} {json}"
        );
        json.get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {name}: GetEnvironment answered without Environment"))
    }
}

/// One create: its endpoint-management mode (or none), with a different shape of request
/// around it so the lifecycle is asserted against several creates, not one.
struct Case {
    name: &'static str,
    /// `None` is a create that names no `EndpointManagement` at all.
    endpoint_management: Option<&'static str>,
    environment_class: Option<&'static str>,
    webserver_access_mode: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    logging: &'static [(&'static str, bool, &'static str)],
}

impl Case {
    fn create_body(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert(
            "ExecutionRoleArn".into(),
            json!("arn:aws:iam::000000000000:role/mwaa-exec"),
        );
        body.insert(
            "SourceBucketArn".into(),
            json!("arn:aws:s3:::my-airflow-bucket-unique-name"),
        );
        body.insert("DagS3Path".into(), json!("dags"));
        body.insert(
            "NetworkConfiguration".into(),
            json!({
                "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                "SecurityGroupIds": ["sg-00000000000000001"],
            }),
        );
        if let Some(value) = self.endpoint_management {
            body.insert("EndpointManagement".into(), json!(value));
        }
        if let Some(value) = self.environment_class {
            body.insert("EnvironmentClass".into(), json!(value));
        }
        if let Some(value) = self.webserver_access_mode {
            body.insert("WebserverAccessMode".into(), json!(value));
        }
        if !self.tags.is_empty() {
            let mut tags = serde_json::Map::new();
            for (key, value) in self.tags {
                tags.insert((*key).into(), json!(value));
            }
            body.insert("Tags".into(), Value::Object(tags));
        }
        if !self.logging.is_empty() {
            let mut logging = serde_json::Map::new();
            for (member, enabled, level) in self.logging {
                logging.insert(
                    (*member).into(),
                    json!({"Enabled": enabled, "LogLevel": level}),
                );
            }
            body.insert("LoggingConfiguration".into(), Value::Object(logging));
        }
        Value::Object(body)
    }

    /// The mode the environment ends up in: the one the create named, or the service's
    /// default for a create that named none.
    fn expected_mode(&self) -> &str {
        self.endpoint_management
            .unwrap_or(state::DEFAULT_ENDPOINT_MANAGEMENT)
    }

    /// Whether the caller owns this environment's VPC endpoints, so its create is paused.
    fn is_customer_managed(&self) -> bool {
        self.expected_mode() == state::ENDPOINT_MANAGEMENT_CUSTOMER
    }

    /// The status the environment reports straight after the create answers.
    fn expected_first_status(&self) -> &'static str {
        if self.is_customer_managed() {
            state::STATUS_PENDING
        } else {
            state::STATUS_CREATING
        }
    }

    /// The status the environment reports once every change the clock owes within
    /// [`PAST_TRANSITION`] has come due.
    fn expected_settled_status(&self) -> &'static str {
        if self.is_customer_managed() {
            // Still paused: the endpoints are the caller's to create, and no operation of
            // this service says they exist.
            state::STATUS_PENDING
        } else {
            state::STATUS_AVAILABLE
        }
    }
}

/// The table: every `EndpointManagement` the model lists plus the create that names none,
/// each combined with a different class, access mode, tag set and logging set.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            // The create that names no mode at all: it gets the service-managed lifecycle.
            name: "ModeOmitted",
            endpoint_management: None,
            environment_class: None,
            webserver_access_mode: None,
            tags: &[],
            logging: &[],
        },
        Case {
            // Caller-managed, on the one class whose unit defaults differ, with tags and
            // every log type configured.
            name: "CustomerMicroLoaded",
            endpoint_management: Some("CUSTOMER"),
            environment_class: Some("mw1.micro"),
            webserver_access_mode: Some("PRIVATE_ONLY"),
            tags: &[("Team", "Data"), ("CostCenter", "4711")],
            logging: &[
                ("DagProcessingLogs", true, "DEBUG"),
                ("SchedulerLogs", false, "WARNING"),
                ("TaskLogs", true, "INFO"),
                ("WebserverLogs", false, "ERROR"),
                ("WorkerLogs", true, "CRITICAL"),
            ],
        },
        Case {
            // Service-managed, on the largest class, with nothing else named.
            name: "ServiceLargeBare",
            endpoint_management: Some("SERVICE"),
            environment_class: Some("mw1.2xlarge"),
            webserver_access_mode: Some("PUBLIC_AND_PRIVATE"),
            tags: &[],
            logging: &[],
        },
    ];
    // One row per mode the model lists, with every other optional member omitted: the mode
    // alone decides the lifecycle.
    for (index, mode) in ENDPOINT_MANAGEMENT_MODES.iter().enumerate() {
        cases.push(Case {
            name: MODE_CASE_NAMES[index],
            endpoint_management: Some(mode),
            environment_class: None,
            webserver_access_mode: None,
            tags: &[],
            logging: &[],
        });
    }
    cases
}

/// `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`, so each row carries a literal name.
const MODE_CASE_NAMES: [&str; 2] = ["ModeOnlyCustomer", "ModeOnlyService"];

#[tokio::test]
async fn the_create_lifecycle_follows_who_manages_the_vpc_endpoints() {
    let harness = Harness::new();

    for case in cases() {
        let (status, json) = harness.create(&case).await;
        assert_eq!(
            status, 200,
            "case {}: expected CreateEnvironment 200, got {status} {json}",
            case.name
        );

        let environment = harness.environment(case.name).await;
        let reported = environment.get("Status").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported,
            json!(case.expected_first_status()),
            "case {}: a {} create reports {} straight after the call, got {reported:?}",
            case.name,
            case.expected_mode(),
            case.expected_first_status()
        );
        assert_eq!(
            environment.get("EndpointManagement").cloned(),
            Some(json!(case.expected_mode())),
            "case {}: EndpointManagement is reported beside the status, got {:?}",
            case.name,
            environment.get("EndpointManagement")
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    for case in cases() {
        // Read twice: a paused environment must still be paused on the second read, so
        // reporting it cannot be what resumes it.
        for read in 1..=2 {
            let environment = harness.environment(case.name).await;
            let reported = environment.get("Status").cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported,
                json!(case.expected_settled_status()),
                "case {}: read {read} of a {} create must report {}, got {reported:?}",
                case.name,
                case.expected_mode(),
                case.expected_settled_status()
            );
            assert_eq!(
                environment.get("EndpointManagement").cloned(),
                Some(json!(case.expected_mode())),
                "case {}: EndpointManagement on read {read}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_paused_create_is_reported_by_every_reader_and_refuses_an_update() {
    let harness = Harness::new();
    for case in cases() {
        let (status, json) = harness.create(&case).await;
        assert_eq!(
            status, 200,
            "case {}: create must succeed, got {status} {json}",
            case.name
        );
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    // Every environment is stored, paused or not: pausing a create is a status, not a
    // refusal to record the environment.
    let (status, json) = harness.list().await;
    assert_eq!(status, 200, "ListEnvironments must answer 200, got {json}");
    let listed: BTreeSet<String> = json
        .get("Environments")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListEnvironments answered without Environments: {json}"))
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    for case in cases() {
        assert!(
            listed.contains(case.name),
            "case {}: a {} environment must be listed, got {listed:?}",
            case.name,
            case.expected_mode()
        );
    }

    for case in cases() {
        let (status, json) = harness.update(case.name, json!({"MaxWorkers": 7})).await;
        if case.is_customer_managed() {
            // The create has not finished: there is nothing running to reconfigure.
            assert_eq!(
                status, 400,
                "case {}: updating a paused environment must be refused, got {status} {json}",
                case.name
            );
            let code = json
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                code, "ValidationException",
                "case {}: the refusal of an update on a paused environment, got {json}",
                case.name
            );
            let environment = harness.environment(case.name).await;
            assert_eq!(
                environment.get("Status").cloned(),
                Some(json!(state::STATUS_PENDING)),
                "case {}: a refused update leaves the paused status alone",
                case.name
            );
            assert_eq!(
                environment.get("MaxWorkers").cloned(),
                Some(json!(i64::from(state::DEFAULT_MAX_WORKERS))),
                "case {}: a refused update changes no member, got {:?}",
                case.name,
                environment.get("MaxWorkers")
            );
        } else {
            assert_eq!(
                status, 200,
                "case {}: an available environment takes the update, got {status} {json}",
                case.name
            );
        }
    }

    // A paused environment can still be deleted — the caller's way out of the pause.
    for case in cases().iter().filter(|case| case.is_customer_managed()) {
        let (status, json) = harness.delete(case.name).await;
        assert_eq!(
            status, 200,
            "case {}: deleting a paused environment must succeed, got {status} {json}",
            case.name
        );
        let (status, _) = harness
            .send("GET", &format!("/environments/{}", case.name), None)
            .await;
        assert_eq!(
            status, 404,
            "case {}: a deleted environment is gone from GetEnvironment",
            case.name
        );
    }
}

#[tokio::test]
async fn the_only_change_owed_a_paused_create_is_the_documented_failure() {
    // The grace period is 72 hours, so the failure it comes due at cannot be observed by
    // waiting: assert the lifecycle the create stores instead. One queued change per mode,
    // with the documented span and the status it lands in.
    let now = SystemTime::now();
    for mode in ENDPOINT_MANAGEMENT_MODES {
        let (status, pending) = state::create_lifecycle(mode, now);
        let expected: (&str, &str, Duration) = if mode == state::ENDPOINT_MANAGEMENT_CUSTOMER {
            (
                state::STATUS_PENDING,
                state::STATUS_CREATE_FAILED,
                state::CUSTOMER_ENDPOINT_GRACE_PERIOD,
            )
        } else {
            (
                state::STATUS_CREATING,
                state::STATUS_AVAILABLE,
                state::TRANSITION_SPAN,
            )
        };
        assert_eq!(
            status, expected.0,
            "case {mode}: the status a create stores, got {status}"
        );
        assert_eq!(
            pending.len(),
            1,
            "case {mode}: exactly one change is owed a new environment, got {pending:?}"
        );
        let owed = &pending[0];
        assert_eq!(
            owed.to, expected.1,
            "case {mode}: where the owed change lands, got {:?}",
            owed.to
        );
        assert_eq!(
            owed.due_at,
            now + expected.2,
            "case {mode}: the owed change comes due {:?} after the create, got {:?}",
            expected.2,
            owed.due_at.duration_since(now)
        );
    }
    // The documented grace period, from `EndpointManagement`'s own doc: "If you do not
    // take action to create the endpoints within 72 hours, the status will change to
    // CREATE_FAILED."
    assert_eq!(
        state::CUSTOMER_ENDPOINT_GRACE_PERIOD,
        Duration::from_secs(72 * 3600),
        "the grace period a paused create gets, got {:?}",
        state::CUSTOMER_ENDPOINT_GRACE_PERIOD
    );
    // A create that names no mode follows the service-managed lifecycle, which is the
    // default mode's.
    assert_eq!(
        state::create_lifecycle(state::DEFAULT_ENDPOINT_MANAGEMENT, now).0,
        state::STATUS_CREATING,
        "the default mode's create is in progress, not paused"
    );
}
