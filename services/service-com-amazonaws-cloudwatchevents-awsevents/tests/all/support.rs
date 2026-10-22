// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares: the emulator (over one engine),
//! the request shape both of this API's models publish (`POST /` with `X-Amz-Target:
//! AWSEvents.<Operation>`), and the ARN forms a stored record is expected to carry.
//!
//! It lives here rather than in each suite so a second suite does not re-derive the
//! request shape, the route keys or the ARN forms. Both suites mount THIS crate's model
//! alone ([`emulator`]); the cross-model claim — that each route key is served by exactly
//! one of the models that publish this wire identity — needs every claimant mounted and
//! so lives in `core/router/tests/all/events_store_ownership.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_cloudwatchevents_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and therefore the region every ARN the
/// answers carry is built for.
pub const REGION: &str = "eu-west-1";

/// The authority the requests arrive on. Both models publish endpoint prefix `events`,
/// which is exactly why the route key has to tell them apart.
pub const HOST: &str = "events.eu-west-1.amazonaws.com";

/// A SigV4 credential scope naming THIS model's own route key.
///
/// The 4th field is `cloudwatchevents`, this model's sdkId-derived key, NOT the `events`
/// signing name its model declares: `CloudWatch Events` is the legacy sdkId of the
/// service now called EventBridge, and the two models publish one identical wire
/// identity, so the `events` key is owned by the EventBridge crate
/// (`services/service-com-amazonaws-eventbridge-awsevents`, the superset model) and this
/// service answers under its own key — see the `OWN_SDK_ID_KEY` note in `src/service.rs`.
/// Routing both to one key is what the router refuses as `500 AmbiguousService`.
pub const SIGNED_FOR_LEGACY: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/cloudwatchevents/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The same scope naming the `events` key, which the EventBridge crate owns.
pub const SIGNED_FOR_EVENTS: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/events/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The bus every account owns, used when a request names none.
pub const DEFAULT_BUS: &str = "default";

/// One emulator and the engine it writes to, so a suite can read the store its handlers
/// wrote.
pub struct Emulator {
    pub router: MultiServiceRouter,
    pub engine: Arc<engine::InMemoryEngine>,
}

/// A router carrying ONLY this crate's service.
pub fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// The wire error code (`__type`), without its namespace; the empty string on a
    /// success, which carries none. The FULL `__type` is pinned by [`Answer::error_type`].
    pub fn error_code(&self) -> &str {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
    }

    /// The `__type` verbatim, namespace included, so the envelope's own shape is pinned
    /// somewhere rather than only its last segment.
    pub fn error_type(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    pub fn string(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

impl Emulator {
    /// One awsJson1_1 call signed for THIS model's own route key: `POST /` with
    /// `X-Amz-Target: AWSEvents.<Operation>`.
    pub async fn call(&self, operation: &str, body: Value) -> Answer {
        self.call_signed(SIGNED_FOR_LEGACY, operation, body).await
    }

    /// The same call under `authorization`, which is the only thing that tells the two
    /// models' requests apart: the wire form below is the one BOTH publish.
    pub async fn call_signed(&self, authorization: &str, operation: &str, body: Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("authorization", authorization)
            .header("x-amz-target", format!("AWSEvents.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }
}

/// The account id every generated emulator presents — the account segment of every ARN
/// it mints.
pub fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The ARN a rule on the default bus carries, built here rather than read from the
/// implementation.
pub fn default_bus_rule_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{name}", account())
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`.
pub fn event_bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// A target ARN pointing at some other service's resource.
pub fn target_arn(resource: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{resource}", account())
}
