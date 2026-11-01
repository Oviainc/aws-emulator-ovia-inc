// SPDX-License-Identifier: CC0-1.0

//! What `InvokeRestApi` does with the `Body` document a call carries, end to end over the
//! router this service mounts in production.
//!
//! The rule under test: each document member of this operation is one the model documents
//! as a JSON object (`smithy_query(query="doc",shape="InvokeRestApiRequest",member="Body"`
//! — "provided as a JSON object" — and the same for `QueryParameters`), and an object is
//! what NAMES the values it carries, so a document of any other shape — an array, a
//! string, a number, a boolean — names none of them and the call carrying it is refused,
//! whatever method and path that call addresses, rather than served as if the caller had
//! supplied nothing. Both documents travel with the request rather than with the endpoint
//! it reaches, so neither is judged by the endpoint that happens to read one.
//!
//! Rows differ along the shape of the document, the method, the path, the class of call
//! (a paged collection, a read of one entity, a read that reads no document, a write, a
//! change, a delete, a method the path does not serve and a path this API does not serve)
//! and, for the served control rows, whether the document is absent, `null` or the object
//! the member is documented as.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The environment these suites invoke the Apache Airflow REST API of.
const ENVIRONMENT: &str = "BodyDocument";

/// The refusal a document that is no JSON object answers with: the Apache Airflow problem
/// document's `detail`, which names the member that could carry nothing.
const NOT_AN_OBJECT: &str = "the request body must be a JSON object";

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

    /// One member of the answered document.
    fn member(&self, name: &str) -> Value {
        self.document().get(name).cloned().unwrap_or(Value::Null)
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

    /// One Apache Airflow REST API call against the environment these suites use.
    async fn invoke(&self, body: Value) -> Answer {
        self.send("POST", &format!("/restapi/{ENVIRONMENT}"), Some(body))
            .await
    }

    /// The environment every suite here invokes, every optional member absent.
    async fn with_environment() -> Self {
        let harness = Self::new();
        let created = harness
            .send(
                "PUT",
                &format!("/environments/{ENVIRONMENT}"),
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
            .await;
        assert_eq!(
            created.status,
            200,
            "the fixture create of {ENVIRONMENT} must succeed, got {} {}",
            created.status,
            created.text()
        );
        harness
    }

    /// Seed one entity the refused calls below could otherwise have read, changed or
    /// removed.
    async fn seed(&self, path: &str, entity: Value) {
        let written = self.invoke(body_call("POST", path, entity)).await;
        assert_eq!(
            written.airflow_status(),
            200,
            "the fixture write to {path:?} must be served, got {}",
            written.text()
        );
    }
}

/// One Apache Airflow REST API call as a table row states it.
fn call(method: &str, path: &str) -> Value {
    json!({"Method": method, "Path": path})
}

/// The same, carrying a `Body` document of any shape.
fn body_call(method: &str, path: &str, body: Value) -> Value {
    json!({"Method": method, "Path": path, "Body": body})
}

/// Every shape a JSON document takes other than the object `Body` is documented as. A
/// document of each of them names no member, so it carries no entity at all.
fn non_object_documents() -> Vec<(&'static str, Value)> {
    vec![
        ("an array of entity objects", json!([{"key": "k", "value": "v"}])),
        ("an empty array", json!([])),
        ("a nested array", json!([[1, 2], [3]])),
        ("the text a form-encoded body carries", json!("limit=1&offset=0")),
        ("an empty string", json!("")),
        ("a bare number", json!(42)),
        ("a negative number", json!(-1)),
        ("a fractional number", json!(1.5)),
        ("a boolean", json!(true)),
        ("the other boolean", json!(false)),
    ]
}

/// Every class of call this API answers, each as the method and the path a row sends:
/// collections, reads, writes, deletes, a method a path does not serve and a path this
/// API does not serve.
fn call_classes() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("a paged collection", "GET", "/variables"),
        ("another paged collection", "GET", "/pools"),
        ("the empty DAG inventory", "GET", "/dags"),
        ("a read that reads no document", "GET", "/version"),
        ("a read of one entity", "GET", "/variables/k"),
        ("a read of one pool", "GET", "/pools/etl"),
        ("a create", "POST", "/variables"),
        ("a pool create", "POST", "/pools"),
        ("a change", "PATCH", "/variables/k"),
        ("a delete", "DELETE", "/variables/zz"),
        ("a pool delete", "DELETE", "/pools/etl"),
        ("a method the path does not serve", "PUT", "/variables"),
        ("a path this API does not serve", "GET", "/no/such/path"),
        ("a path addressed through a DAG", "GET", "/dags/example"),
    ]
}

/// A `Body` document that is not a JSON object carries no entity, so the call carrying it
/// is refused — with the Apache Airflow problem document its sibling `QueryParameters` is
/// refused by — whatever method and path the call addresses, rather than being served as
/// if the caller had carried no body.
#[tokio::test]
async fn a_body_document_that_is_not_an_object_is_refused_on_every_call() {
    let harness = Harness::with_environment().await;

    // Entities a refused read, change or delete could have reported or removed had the
    // malformed body been dropped instead of refused.
    harness
        .seed("/variables", json!({"key": "k", "value": "v"}))
        .await;
    harness
        .seed("/variables", json!({"key": "zz", "value": "v"}))
        .await;
    harness.seed("/pools", json!({"name": "etl", "slots": 3})).await;

    for (shape, body) in non_object_documents() {
        for (kind, method, path) in call_classes() {
            let case = format!("{shape} on {kind} ({method} {path})");
            let answer = harness.invoke(body_call(method, path, body.clone())).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a refused Apache Airflow call is the modelled exception's \
                 own HTTP 400, got {} {}",
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
                400,
                "case {case:?}: expected RestApiStatusCode 400, got {}",
                answer.text()
            );
            assert_eq!(
                answer.member("status"),
                json!(400),
                "case {case:?}: the problem document must name the same status, got {}",
                answer.document()
            );
            assert_eq!(
                answer.member("detail"),
                json!(NOT_AN_OBJECT),
                "case {case:?}: expected the detail of a document that is no object, got {}",
                answer.document()
            );
        }
    }

    // No refused call reached an endpoint: what a refused delete named is still there,
    // what a refused change named is unchanged, and what a refused create would have
    // stored was never stored.
    for (case, path, expected_status, expected_member, expected_value) in [
        ("the variable a refused delete named", "/variables/zz", 200, "value", json!("v")),
        ("the variable a refused change named", "/variables/k", 200, "value", json!("v")),
        ("the pool a refused delete named", "/pools/etl", 200, "slots", json!(3)),
        (
            "the collection a refused create would have grown",
            "/variables",
            200,
            "total_entries",
            json!(2),
        ),
        (
            "the pools a refused create would have grown",
            "/pools",
            200,
            "total_entries",
            json!(2),
        ),
    ] {
        let answer = harness.invoke(call("GET", path)).await;
        assert_eq!(
            answer.airflow_status(),
            expected_status,
            "case {case:?}: expected RestApiStatusCode {expected_status}, got {}",
            answer.text()
        );
        assert_eq!(
            answer.member(expected_member),
            expected_value,
            "case {case:?}: expected {expected_member} {expected_value}, got {}",
            answer.document()
        );
    }
}

/// What is refused is a document that could carry no entity, not carrying a body: a `Body`
/// that IS the documented object is acted on where the endpoint takes one and changes
/// nothing where it takes none, and the two documents that state nothing — an absent
/// member and the `null` that spells one — leave every endpoint to its own rule.
#[tokio::test]
async fn a_body_document_that_is_an_object_or_states_nothing_is_served() {
    let harness = Harness::with_environment().await;
    harness
        .seed("/variables", json!({"key": "k", "value": "v"}))
        .await;

    for (case, request, expected_status, expected_member, expected_value) in [
        (
            "no body at all on a read",
            call("GET", "/variables/k"),
            200,
            "value",
            json!("v"),
        ),
        (
            "the null that states no body on a read",
            body_call("GET", "/variables/k", Value::Null),
            200,
            "value",
            json!("v"),
        ),
        (
            "an object body a read reads nothing from",
            body_call("GET", "/variables/k", json!({"value": "ignored"})),
            200,
            "value",
            json!("v"),
        ),
        (
            "an empty object body a read reads nothing from",
            body_call("GET", "/version", json!({})),
            200,
            "git_version",
            Value::Null,
        ),
        (
            "the object body a change states its entity in",
            body_call("PATCH", "/variables/k", json!({"key": "k", "value": "w"})),
            200,
            "value",
            json!("w"),
        ),
        (
            "the object body a create states its entity in",
            body_call("POST", "/pools", json!({"name": "etl", "slots": 5})),
            200,
            "slots",
            json!(5),
        ),
        (
            "no body at all on a delete",
            call("DELETE", "/pools/etl"),
            204,
            "detail",
            Value::Null,
        ),
        (
            "the null that states no body on a delete",
            body_call("DELETE", "/variables/k", Value::Null),
            204,
            "detail",
            Value::Null,
        ),
    ] {
        let answer = harness.invoke(request).await;
        assert_eq!(
            answer.airflow_status(),
            expected_status,
            "case {case:?}: expected RestApiStatusCode {expected_status}, got {}",
            answer.text()
        );
        assert_eq!(
            answer.member(expected_member),
            expected_value,
            "case {case:?}: expected {expected_member} {expected_value}, got {}",
            answer.document()
        );
    }

    // A write, unlike a read, has to CARRY the entity it states, so a body that states
    // nothing at all is refused by that endpoint's own rule — with the same detail, since
    // a body naming nothing and a body of the wrong shape both state no entity.
    for (case, request) in [
        ("no body at all on a create", call("POST", "/variables")),
        (
            "the null that states no body on a create",
            body_call("POST", "/variables", Value::Null),
        ),
        ("no body at all on a change", call("PATCH", "/variables/k")),
        (
            "the null that states no body on a pool create",
            body_call("POST", "/pools", Value::Null),
        ),
    ] {
        let answer = harness.invoke(request).await;
        assert_eq!(
            answer.airflow_status(),
            400,
            "case {case:?}: expected RestApiStatusCode 400, got {}",
            answer.text()
        );
        assert_eq!(
            answer.member("detail"),
            json!(NOT_AN_OBJECT),
            "case {case:?}: expected the detail of a body stating no entity, got {}",
            answer.document()
        );
    }
}
