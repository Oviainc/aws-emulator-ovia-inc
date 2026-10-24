// SPDX-License-Identifier: CC0-1.0

//! What `InvokeRestApi` answers for an environment that EXISTS, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule under test: the operation routes `Method`, `Path`, `QueryParameters` and
//! `Body` to the Apache Airflow REST API of the environment the request names, serves that
//! call from the environment's OWN stored Apache Airflow state, and reports what the call
//! answered — the HTTP status in `RestApiStatusCode` and the JSON document in
//! `RestApiResponse` — with a `4xx` answer carried by the modelled `RestApiClientException`
//! (HTTP 400) rather than by the operation's own 200.
//!
//! Every suite below asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over — the method, the path, the entity, the body, the page asked for and
//! the status class — never over one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// `RestApiStatusCode`: the status the Apache Airflow REST API call answered with,
    /// which a success and a `RestApiClientException` both carry.
    fn airflow_status(&self) -> i64 {
        self.json()
            .get("RestApiStatusCode")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    }

    /// `RestApiResponse`: the document that call answered with, or `Null` where it carried
    /// none.
    fn document(&self) -> Value {
        self.json()
            .get("RestApiResponse")
            .cloned()
            .unwrap_or(Value::Null)
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
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
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    /// Create one environment, every optional member absent.
    async fn create(&self, name: &str) -> Answer {
        self.send(
            "PUT",
            &format!("/environments/{name}"),
            Some(json!({
                "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
                "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
                "DagS3Path": "dags",
                "NetworkConfiguration": {
                    "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                    "SecurityGroupIds": ["sg-00000000000000001"],
                },
            })),
        )
        .await
    }

    /// One Apache Airflow REST API call against `name`.
    async fn invoke(&self, name: &str, body: Value) -> Answer {
        self.send("POST", &format!("/restapi/{name}"), Some(body))
            .await
    }

    /// The environment every suite here invokes.
    async fn with_environment(name: &str) -> Self {
        let harness = Self::new();
        let created = harness.create(name).await;
        assert_eq!(
            created.status,
            200,
            "the fixture create of {name} must succeed, got {} {}",
            created.status,
            created.text()
        );
        harness
    }
}

/// One Apache Airflow REST API call as a table row states it.
fn call(method: &str, path: &str) -> Value {
    json!({"Method": method, "Path": path})
}

/// The same, carrying a body.
fn call_with_body(method: &str, path: &str, body: Value) -> Value {
    json!({"Method": method, "Path": path, "Body": body})
}

/// The same, carrying query parameters.
fn call_with_query(method: &str, path: &str, query: Value) -> Value {
    json!({"Method": method, "Path": path, "QueryParameters": query})
}

/// One member of a document.
fn member<'d>(document: &'d Value, name: &str) -> Option<&'d Value> {
    document.get(name)
}

/// Every call this emulator serves answers the operation's own 200, carrying the Apache
/// Airflow status and the document that call computed from the environment's state.
///
/// Rows differ along the method, the path and the resource kind; the documents asserted on
/// are the ones each row's own state determines.
#[tokio::test]
async fn a_served_call_reports_the_airflow_status_and_its_document() {
    let harness = Harness::with_environment("RestApiEnv").await;

    // The version the environment runs is the one its own record holds, which a create
    // that named none defaults to — read it back through the control plane first, so the
    // assertion compares the data plane against stored state rather than against a
    // literal.
    let described = harness.send("GET", "/environments/RestApiEnv", None).await;
    let airflow_version = described
        .json()
        .get("Environment")
        .and_then(|environment| environment.get("AirflowVersion"))
        .and_then(Value::as_str)
        .expect("the stored environment reports an AirflowVersion")
        .to_string();

    for (case, body, status, expected) in [
        (
            "GET /version reports the version the environment runs",
            call("GET", "/version"),
            200,
            json!({"version": airflow_version, "git_version": Value::Null}),
        ),
        (
            "GET /dags reports an empty DAG inventory",
            call("GET", "/dags"),
            200,
            json!({"dags": [], "total_entries": 0}),
        ),
        (
            "GET /variables reports an empty collection",
            call("GET", "/variables"),
            200,
            json!({"variables": [], "total_entries": 0}),
        ),
        (
            "a trailing slash addresses the same collection",
            call("GET", "/variables/"),
            200,
            json!({"variables": [], "total_entries": 0}),
        ),
        (
            "POST /variables sets the variable the body names",
            call_with_body(
                "POST",
                "/variables",
                json!({"key": "schedule", "value": "@daily"}),
            ),
            200,
            json!({"key": "schedule", "value": "@daily", "description": Value::Null}),
        ),
        (
            "GET /variables/{key} reads it back",
            call("GET", "/variables/schedule"),
            200,
            json!({"key": "schedule", "value": "@daily", "description": Value::Null}),
        ),
        (
            "GET /variables now reports the one that was set",
            call("GET", "/variables"),
            200,
            json!({
                "variables": [{"key": "schedule", "value": "@daily", "description": Value::Null}],
                "total_entries": 1,
            }),
        ),
        (
            "PATCH /variables/{key} changes it",
            call_with_body(
                "PATCH",
                "/variables/schedule",
                json!({"key": "schedule", "value": "@hourly", "description": "how often"}),
            ),
            200,
            json!({"key": "schedule", "value": "@hourly", "description": "how often"}),
        ),
        (
            "GET /pools reports the pool Apache Airflow seeds a metadata database with",
            call("GET", "/pools"),
            200,
            json!({
                "pools": [{
                    "name": "default_pool",
                    "slots": 128,
                    "description": "Default pool",
                    "include_deferred": false,
                    "occupied_slots": 0,
                    "running_slots": 0,
                    "queued_slots": 0,
                    "scheduled_slots": 0,
                    "deferred_slots": 0,
                    "open_slots": 128,
                }],
                "total_entries": 1,
            }),
        ),
        (
            "POST /pools creates one, and its open slots are the slots it was given",
            call_with_body(
                "POST",
                "/pools",
                json!({"name": "etl", "slots": 7, "description": "the ETL pool"}),
            ),
            200,
            json!({
                "name": "etl",
                "slots": 7,
                "description": "the ETL pool",
                "include_deferred": false,
                "occupied_slots": 0,
                "running_slots": 0,
                "queued_slots": 0,
                "scheduled_slots": 0,
                "deferred_slots": 0,
                "open_slots": 7,
            }),
        ),
        (
            "PATCH /pools/{name} changes its slots",
            call_with_body(
                "PATCH",
                "/pools/etl",
                json!({"name": "etl", "slots": 9, "include_deferred": true}),
            ),
            200,
            json!({
                "name": "etl",
                "slots": 9,
                "description": Value::Null,
                "include_deferred": true,
                "occupied_slots": 0,
                "running_slots": 0,
                "queued_slots": 0,
                "scheduled_slots": 0,
                "deferred_slots": 0,
                "open_slots": 9,
            }),
        ),
    ] {
        let answer = harness.invoke("RestApiEnv", body).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a served call must answer the operation's own 200, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.airflow_status(),
            status,
            "case {case:?}: expected RestApiStatusCode {status}, got {}",
            answer.text()
        );
        assert_eq!(
            answer.document(),
            expected,
            "case {case:?}: expected RestApiResponse {expected}, got {}",
            answer.document()
        );
    }

    // A delete answers the status that carries no document at all, so there is no
    // `RestApiResponse` to report.
    for (case, path) in [
        ("a variable", "/variables/schedule"),
        ("a pool", "/pools/etl"),
    ] {
        let answer = harness.invoke("RestApiEnv", call("DELETE", path)).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a served delete must answer 200, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.airflow_status(),
            204,
            "case {case:?}: expected RestApiStatusCode 204, got {}",
            answer.text()
        );
        assert_eq!(
            answer.document(),
            Value::Null,
            "case {case:?}: a 204 carries no document, got {}",
            answer.document()
        );
        // And what was deleted is gone from the collection the next read reports.
        let gone = harness.invoke("RestApiEnv", call("GET", path)).await;
        assert_eq!(
            gone.airflow_status(),
            404,
            "case {case:?}: the deleted entity must be gone, got {}",
            gone.text()
        );
    }
}

/// A write through this API is visible to the next read through it, whatever the entity
/// and whatever its values.
///
/// Rows differ along the key, the value (its length, its characters, whether a description
/// travels with it) and the entity kind — a renamed row would not be a different input.
#[tokio::test]
async fn a_write_is_visible_to_the_next_read() {
    let harness = Harness::with_environment("RoundTrip").await;

    for (case, key, value, description) in [
        ("a short key and value", "a", "1", None),
        (
            "a key with the separators Apache Airflow variable keys carry",
            "data_lake.bucket-name",
            "s3://bucket/prefix",
            Some("where the lake lives"),
        ),
        (
            "a JSON document as the value",
            "connection_defaults",
            r#"{"retries": 3, "owner": "airflow"}"#,
            None,
        ),
        (
            // The bound the store itself declares, and the longest value it keeps.
            "a value at the longest this store keeps",
            "wide",
            &"x".repeat(state::MAX_AIRFLOW_VALUE_LENGTH),
            None,
        ),
        (
            // The longest key the modelled `RestApiPath` (@length(1, 64)) can still
            // address an entity by: 64 characters less the `/variables/` prefix.
            "a key at the longest a path can address",
            &"k".repeat(64 - "/variables/".len()),
            "1",
            None,
        ),
        (
            "a value carrying characters outside ASCII",
            "ünïcode",
            "ünïcode value ✅",
            Some("dëscription"),
        ),
    ] {
        let mut written = json!({"key": key, "value": value});
        if let Some(description) = description {
            written["description"] = json!(description);
        }
        let created = harness
            .invoke("RoundTrip", call_with_body("POST", "/variables", written))
            .await;
        assert_eq!(
            created.airflow_status(),
            200,
            "case {case:?}: the write must be served, got {}",
            created.text()
        );

        let read = harness
            .invoke("RoundTrip", call("GET", &format!("/variables/{key}")))
            .await;
        assert_eq!(
            read.airflow_status(),
            200,
            "case {case:?}: the variable must read back, got {}",
            read.text()
        );
        assert_eq!(
            member(&read.document(), "value"),
            Some(&json!(value)),
            "case {case:?}: expected the stored value {value:?}, got {}",
            read.document()
        );
        assert_eq!(
            member(&read.document(), "description"),
            Some(&description.map_or(Value::Null, |text| json!(text))),
            "case {case:?}: expected the stored description {description:?}, got {}",
            read.document()
        );
    }

    // Every one of them is in the collection, which reports how many it holds.
    let listed = harness.invoke("RoundTrip", call("GET", "/variables")).await;
    let entries = listed
        .document()
        .get("variables")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        6,
        "the collection must hold every variable written, got {}",
        listed.document()
    );
    assert_eq!(
        member(&listed.document(), "total_entries"),
        Some(&json!(6)),
        "total_entries must count the whole collection, got {}",
        listed.document()
    );

    // And a second environment's Apache Airflow state is its own: one web server's
    // metadata database is not another's.
    let created = harness.create("OtherEnv").await;
    assert_eq!(
        created.status,
        200,
        "the second fixture create must succeed, got {}",
        created.text()
    );
    let other = harness.invoke("OtherEnv", call("GET", "/variables")).await;
    assert_eq!(
        other.document(),
        json!({"variables": [], "total_entries": 0}),
        "a variable written to one environment must not appear in another, got {}",
        other.document()
    );
}

/// A call the Apache Airflow REST API refuses is the modelled `RestApiClientException`
/// (HTTP 400), carrying that call's own status and its problem document.
///
/// Rows differ along the refusal's class (not found, bad request, conflict, method not
/// allowed), the entity and the method.
#[tokio::test]
async fn an_airflow_refusal_is_the_modelled_client_exception() {
    let harness = Harness::with_environment("Refusals").await;

    // Two entities to refuse a second create of, and to address under a wrong method.
    for body in [
        call_with_body("POST", "/variables", json!({"key": "k", "value": "v"})),
        call_with_body("POST", "/pools", json!({"name": "etl", "slots": 3})),
    ] {
        let created = harness.invoke("Refusals", body).await;
        assert_eq!(
            created.status,
            200,
            "the fixture write must be served, got {}",
            created.text()
        );
    }

    for (case, body, status) in [
        (
            "a variable that was never set",
            call("GET", "/variables/absent"),
            404,
        ),
        (
            "a delete of a variable that was never set",
            call("DELETE", "/variables/absent"),
            404,
        ),
        (
            "a change of a variable that was never set",
            call_with_body(
                "PATCH",
                "/variables/absent",
                json!({"key": "absent", "value": "v"}),
            ),
            404,
        ),
        (
            "a body keyed to another variable than the path names",
            call_with_body(
                "PATCH",
                "/variables/k",
                json!({"key": "other", "value": "v"}),
            ),
            400,
        ),
        ("a write with no body at all", call("POST", "/variables"), 400),
        (
            "a body that is not a JSON object",
            call_with_body("POST", "/variables", json!("key=value")),
            400,
        ),
        (
            "a body missing the value member",
            call_with_body("POST", "/variables", json!({"key": "k"})),
            400,
        ),
        (
            "a body whose value is not a string",
            call_with_body("POST", "/variables", json!({"key": "k", "value": 7})),
            400,
        ),
        (
            // One character past the bound the store declares.
            "a value past the longest this store keeps",
            call_with_body(
                "POST",
                "/variables",
                json!({"key": "wide", "value": "x".repeat(state::MAX_AIRFLOW_VALUE_LENGTH + 1)}),
            ),
            400,
        ),
        (
            "a key past the longest this store keeps",
            call_with_body(
                "POST",
                "/variables",
                json!({"key": "k".repeat(state::MAX_AIRFLOW_KEY_LENGTH + 1), "value": "1"}),
            ),
            400,
        ),
        (
            "a pool whose name is already taken",
            call_with_body("POST", "/pools", json!({"name": "etl", "slots": 5})),
            409,
        ),
        (
            "a pool with no slots member",
            call_with_body("POST", "/pools", json!({"name": "unsized"})),
            400,
        ),
        (
            "a pool holding fewer slots than none",
            call_with_body("POST", "/pools", json!({"name": "negative", "slots": -1})),
            400,
        ),
        (
            "a delete of the pool every task with no pool of its own runs in",
            call("DELETE", "/pools/default_pool"),
            400,
        ),
        (
            "a pool that was never created",
            call("GET", "/pools/absent"),
            404,
        ),
        (
            "a DAG the environment holds none of",
            call("GET", "/dags/example_dag"),
            404,
        ),
        (
            "the DAG runs of a DAG the environment holds none of",
            call("GET", "/dags/example_dag/dagRuns"),
            404,
        ),
        (
            "a DAG run triggered on a DAG the environment holds none of",
            call_with_body(
                "POST",
                "/dags/example_dag/dagRuns",
                json!({"conf": {}, "dag_run_id": "manual__1"}),
            ),
            404,
        ),
        (
            "the task instances of a DAG the environment holds none of",
            call("GET", "/dags/x/dagRuns/manual__1/taskInstances"),
            404,
        ),
        (
            "a path this API does not serve",
            call("GET", "/eventLogs"),
            404,
        ),
        (
            "a method a served path does not take",
            call_with_body("PUT", "/variables", json!({"key": "k", "value": "v"})),
            405,
        ),
        (
            "a method the version endpoint does not take",
            call("DELETE", "/version"),
            405,
        ),
    ] {
        let answer = harness.invoke("Refusals", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a refused Apache Airflow call is the modelled exception's own \
             HTTP 400, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "RestApiClientException",
            "case {case:?}: expected RestApiClientException, got {}",
            answer.text()
        );
        assert_eq!(
            answer.airflow_status(),
            status,
            "case {case:?}: expected RestApiStatusCode {status}, got {}",
            answer.text()
        );
        let document = answer.document();
        assert_eq!(
            member(&document, "status"),
            Some(&json!(status)),
            "case {case:?}: the problem document must name the same status, got {document}"
        );
        assert!(
            member(&document, "detail")
                .and_then(Value::as_str)
                .is_some_and(|detail| !detail.is_empty()),
            "case {case:?}: the problem document must carry a detail, got {document}"
        );
        assert!(
            member(&document, "title")
                .and_then(Value::as_str)
                .is_some_and(|title| !title.is_empty()),
            "case {case:?}: the problem document must carry a title, got {document}"
        );
    }

    // None of those refusals wrote anything: the collection still holds exactly what the
    // two fixture writes put in it.
    for (case, path, member_name, expected) in [
        ("variables", "/variables", "variables", 1),
        ("pools", "/pools", "pools", 2),
    ] {
        let listed = harness.invoke("Refusals", call("GET", path)).await;
        assert_eq!(
            member(&listed.document(), "total_entries"),
            Some(&json!(expected)),
            "case {case:?}: a refused call must not change the collection, got {}",
            listed.document()
        );
        assert_eq!(
            listed
                .document()
                .get(member_name)
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(expected),
            "case {case:?}: expected {expected} entries, got {}",
            listed.document()
        );
    }
}

/// A collection call is paged by the `limit` and `offset` query parameters it carries, and
/// `total_entries` counts the whole collection rather than the page.
///
/// Rows differ along the window asked for — the default, a page inside the collection, one
/// at its end, one past it, the cap, and the two values no window can be read from — and a
/// parameter is accepted both as a number and as the text an HTTP query string carries it
/// in.
#[tokio::test]
async fn a_collection_is_paged_by_the_query_parameters() {
    let harness = Harness::with_environment("Paged").await;

    // Ten variables, keyed so their stored order is known.
    for index in 0..10 {
        let written = harness
            .invoke(
                "Paged",
                call_with_body(
                    "POST",
                    "/variables",
                    json!({"key": format!("key{index}"), "value": index.to_string()}),
                ),
            )
            .await;
        assert_eq!(
            written.airflow_status(),
            200,
            "the fixture write {index} must be served, got {}",
            written.text()
        );
    }

    for (case, query, expected_keys) in [
        (
            "no query parameters at all: the whole collection, under the default page size",
            None,
            vec![
                "key0", "key1", "key2", "key3", "key4", "key5", "key6", "key7", "key8", "key9",
            ],
        ),
        (
            "a limit inside the collection",
            Some(json!({"limit": 3})),
            vec!["key0", "key1", "key2"],
        ),
        (
            "a limit and an offset",
            Some(json!({"limit": 2, "offset": 4})),
            vec!["key4", "key5"],
        ),
        (
            "an offset at the last entry",
            Some(json!({"limit": 5, "offset": 9})),
            vec!["key9"],
        ),
        (
            "an offset past the end of the collection",
            Some(json!({"offset": 25})),
            vec![],
        ),
        (
            "a limit past the page cap, which is capped to it",
            Some(json!({"limit": 1_000})),
            vec![
                "key0", "key1", "key2", "key3", "key4", "key5", "key6", "key7", "key8", "key9",
            ],
        ),
        (
            "a limit asking for no entry falls back to the default page size",
            Some(json!({"limit": 0})),
            vec![
                "key0", "key1", "key2", "key3", "key4", "key5", "key6", "key7", "key8", "key9",
            ],
        ),
        (
            "the parameters as the text an HTTP query string carries them in",
            Some(json!({"limit": "2", "offset": "1"})),
            vec!["key1", "key2"],
        ),
    ] {
        let body = match query {
            Some(query) => call_with_query("GET", "/variables", query),
            None => call("GET", "/variables"),
        };
        let answer = harness.invoke("Paged", body).await;
        assert_eq!(
            answer.airflow_status(),
            200,
            "case {case:?}: the paged read must be served, got {}",
            answer.text()
        );
        let document = answer.document();
        let keys: Vec<String> = document
            .get("variables")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            keys, expected_keys,
            "case {case:?}: expected the page {expected_keys:?}, got {keys:?}"
        );
        assert_eq!(
            member(&document, "total_entries"),
            Some(&json!(10)),
            "case {case:?}: total_entries counts the whole collection, got {document}"
        );
    }

    // A window no page can be read from is refused, as a client error of the Apache
    // Airflow call.
    for (case, query) in [
        ("a negative limit", json!({"limit": -1})),
        ("a negative offset", json!({"offset": -5})),
        ("a limit that is not a number", json!({"limit": "two"})),
        ("an offset that is not a number", json!({"offset": {}})),
    ] {
        let answer = harness
            .invoke("Paged", call_with_query("GET", "/variables", query))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the modelled exception's HTTP 400, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.airflow_status(),
            400,
            "case {case:?}: expected RestApiStatusCode 400, got {}",
            answer.text()
        );
    }
}

/// A read through this API changes nothing a later caller sees: the same read answers the
/// same document, and nothing a read touches moves.
#[tokio::test]
async fn a_read_changes_nothing() {
    let harness = Harness::with_environment("Reads").await;
    let set = harness
        .invoke(
            "Reads",
            call_with_body("POST", "/variables", json!({"key": "k", "value": "v"})),
        )
        .await;
    assert_eq!(
        set.airflow_status(),
        200,
        "the fixture write must be served, got {}",
        set.text()
    );

    for path in ["/variables", "/variables/k", "/pools", "/pools/default_pool", "/dags", "/version"] {
        let first = harness.invoke("Reads", call("GET", path)).await;
        let second = harness.invoke("Reads", call("GET", path)).await;
        assert_eq!(
            first.airflow_status(),
            second.airflow_status(),
            "case {path:?}: two reads must answer the same status, got {} and {}",
            first.text(),
            second.text()
        );
        assert_eq!(
            first.document(),
            second.document(),
            "case {path:?}: two reads must answer the same document, got {} and {}",
            first.document(),
            second.document()
        );
    }
}
