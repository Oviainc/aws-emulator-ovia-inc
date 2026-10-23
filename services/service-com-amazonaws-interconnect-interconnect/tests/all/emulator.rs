// SPDX-License-Identifier: CC0-1.0

//! The ONE wire harness this crate's attach-point and refusal suites drive the
//! emulator through.
//!
//! Every suite calls the service the way a client does — `POST /` with
//! `X-Amz-Target: Interconnect.<Operation>` on `awsJson1_0`, through the same
//! `MultiServiceRouter::dispatch` the socket front door uses — so the mounting rules
//! live here once rather than in each suite.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The signing name the model gives this service; also its ARN namespace.
pub(crate) const SIGNING_NAME: &str = "interconnect";

/// The ONLY value `AttachPointType` models
/// (`smithy_query(query="shape",shape="AttachPointType")` -> values:
/// ["DirectConnectGateway"]), so it is the only `type` a descriptor can report.
pub(crate) const ATTACH_POINT_TYPE: &str = "DirectConnectGateway";

/// One emulator: a router with this service mounted over one shared engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pinned off so the suites do not depend on the process-wide default; the
        // Authorization header below is still what carries the credential scope.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_interconnect_interconnect::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// `POST /` for one operation of `signing_name`, with that service's target and
    /// JSON protocol version.
    async fn post(
        &self,
        signing_name: &str,
        target: &str,
        json_version: &str,
        body: Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-target, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.localhost"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", target)
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decoded = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    /// One Interconnect operation (`awsJson1_0`), as a `(status, body)` pair.
    pub(crate) async fn ic(&self, operation: &str, body: Value) -> (u16, Value) {
        self.post(
            SIGNING_NAME,
            &format!("Interconnect.{operation}"),
            "1.0",
            body,
        )
        .await
    }

    /// One Interconnect operation, required to succeed.
    pub(crate) async fn ic_ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.ic(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// Every seeded Environment, as `ListEnvironments` reports it.
    pub(crate) async fn environments(&self) -> Vec<Value> {
        let listed = self.ic_ok("ListEnvironments", json!({})).await;
        listed["environments"]
            .as_array()
            .expect("environments is a list")
            .clone()
    }

    /// Every attach point `ListAttachPoints` reports for `environment_id`, walking the
    /// pages so the answer is the whole listing however it is windowed.
    pub(crate) async fn attach_points(&self, environment_id: &str) -> Vec<Value> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = Map::new();
            request.insert("environmentId".to_string(), json!(environment_id));
            if let Some(token) = &token {
                request.insert("nextToken".to_string(), json!(token));
            }
            let listed = self.ic_ok("ListAttachPoints", Value::Object(request)).await;
            all.extend(
                listed["attachPoints"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            token = listed["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                return all;
            }
        }
    }

    /// Create a connection on `environment_id` with `bandwidth`, returning the stored
    /// `Connection`.
    pub(crate) async fn create_connection(
        &self,
        environment_id: &str,
        bandwidth: &str,
        attach_point: &str,
        description: &str,
    ) -> Value {
        let created = self
            .ic_ok(
                "CreateConnection",
                json!({
                    "environmentId": environment_id,
                    "bandwidth": bandwidth,
                    "attachPoint": { "directConnectGateway": attach_point },
                    "description": description,
                    "remoteAccount": { "identifier": engine::EMULATOR_ACCOUNT_ID },
                }),
            )
            .await;
        created["connection"].clone()
    }

    /// `GetConnection`, as a `(status, body)` pair so the not-found case is readable.
    pub(crate) async fn get_connection(&self, identifier: &str) -> (u16, Value) {
        self.ic("GetConnection", json!({ "identifier": identifier }))
            .await
    }
}

/// A string member, or `""` when absent — so a missing member fails an assertion
/// rather than panicking the harness.
pub(crate) fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The wire error code of an error body.
pub(crate) fn code(body: &Value) -> String {
    text(body, "__type")
}

/// The `message` an error body carries, under either casing the envelope may use.
pub(crate) fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `Environment$bandwidths.available` for one listed environment.
pub(crate) fn available_bandwidths(environment: &Value) -> Vec<String> {
    environment["bandwidths"]["available"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|bandwidth| bandwidth.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The environments a connection can be created on: one offering no available
/// bandwidth (the `unavailable` state) takes none.
pub(crate) fn creatable_environments(catalog: &[Value]) -> Vec<Value> {
    catalog
        .iter()
        .filter(|environment| !available_bandwidths(environment).is_empty())
        .cloned()
        .collect()
}
