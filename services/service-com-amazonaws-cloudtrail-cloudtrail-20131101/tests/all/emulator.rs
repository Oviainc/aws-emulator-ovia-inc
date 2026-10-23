// SPDX-License-Identifier: CC0-1.0

//! The emulator under test, shared by this crate's suites.
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.<Operation>` through the same
//! `MultiServiceRouter` the production router serves, so what a suite asserts is
//! what a client sees — including the HTTP status and the error code.

use serde_json::Value;
use std::sync::Arc;

/// The `X-Amz-Target` prefix CloudTrail publishes for its `awsJson1_1` API, as
/// every AWS SDK sends it: the prefix is itself dotted, and the operation name
/// follows the last dot.
pub const TARGET_PREFIX: &str = "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.";

/// The Region most cases call in.
pub const HOME_REGION: &str = "us-east-1";

/// This service's state, as a suite reaches it for a resource no implemented
/// operation can create yet.
pub type ServiceState = engine::ServiceState<
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::ComAmazonawsCloudtrailCloudtrail20131101State,
>;

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
pub struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success.
    pub fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            // A `__type` may be namespaced (`com.amazonaws.cloudtrail#Foo`); the
            // code clients branch on is the shape name.
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// The refusal text a reply carries: the modelled error shapes spell the member
    /// `Message`, and the framework's pre-dispatch refusals spell it `message`, so a
    /// suite that asserts on wording reads both through here.
    pub fn message(&self) -> &str {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Assert this reply is the modelled failure `code` with HTTP `status`.
    pub fn expect_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got {self:?}"
        );
    }

    /// Assert this reply is a success, and hand back its body.
    pub fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's;
        // pinning it off keeps these cases independent of how the process was
        // started.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// This service's store in the shared engine — the SAME store every handler
    /// reads, which is how a suite seeds a resource whose creating operation is
    /// still a scaffold stub.
    pub fn state(&self) -> ServiceState {
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::service_state(&*self.engine)
            .expect("the in-memory engine carries this service's state")
    }

    /// Call `operation` from `region` with `body`, under the target prefix every
    /// SDK sends ([`TARGET_PREFIX`]).
    pub async fn call_in(&self, operation: &str, region: &str, body: &Value) -> Reply {
        self.call_with_target(&format!("{TARGET_PREFIX}{operation}"), region, body)
            .await
    }

    /// Call with a verbatim `X-Amz-Target`, for pinning how a target header is
    /// read.
    pub async fn call_with_target(&self, target: &str, region: &str, body: &Value) -> Reply {
        // The Region a request is served in comes from its SigV4 credential scope,
        // which is also how the router resolves the service.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/cloudtrail/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", target)
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply { status, body }
    }

    /// Call `operation` from [`HOME_REGION`].
    pub async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.call_in(operation, HOME_REGION, body).await
    }
}
