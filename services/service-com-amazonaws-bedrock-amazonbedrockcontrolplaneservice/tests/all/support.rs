// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the emulator through.
//!
//! Extracted out of `guardrails.rs` so the other suites exercise the SAME dispatch path
//! rather than a second copy of it: one shared engine, one registered service, requests
//! signed the way an SDK signs them and dispatched in-process through the
//! `MultiServiceRouter` the emulator boots.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

/// The region every request in these suites signs for; it is what the minted ARNs must
/// carry.
pub const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries.
pub const ACCOUNT: &str = "000000000000";

/// One emulator answer: the HTTP status, the parsed JSON body, and the wire error code
/// when it is a failure.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub error: Option<String>,
}

impl Reply {
    /// The body member `name`, or `Value::Null` when the response omits it.
    pub fn get(&self, name: &str) -> Value {
        self.body.get(name).cloned().unwrap_or(Value::Null)
    }

    /// The body member `name` as a string; panics naming the member when it is absent.
    pub fn text(&self, name: &str) -> String {
        self.get(name)
            .as_str()
            .unwrap_or_else(|| panic!("response has no string member {name}: {:?}", self.body))
            .to_string()
    }

    /// The instant the body member `name` carries, in epoch seconds.
    ///
    /// Every `Timestamp` member of this service's guardrail responses is modelled
    /// `@timestampFormat("date-time")`, so the wire form is an RFC 3339 STRING and this
    /// panics on anything else — including on the epoch NUMBER the protocol default would
    /// have produced.
    pub fn instant(&self, name: &str) -> f64 {
        let value = self.get(name);
        let text = value.as_str().unwrap_or_else(|| {
            panic!(
                "member {name} is modelled @timestampFormat(\"date-time\"), so it must be an \
                 RFC 3339 string, got {value}"
            )
        });
        date_time_seconds(name, text)
    }
}

/// An RFC 3339 date-time as epoch seconds, read with the same codec the wire uses
/// (`aws_smithy_types::date_time::Format::DateTime`).
pub fn date_time_seconds(case: &str, text: &str) -> f64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case}: {text:?} does not parse as an RFC 3339 date-time: {error}")
    });
    parsed.as_secs_f64()
}

/// An identifier as an SDK writes it into a path label: an ARN carries `:` and `/`, and
/// both are percent-encoded so the whole identifier stays ONE path segment.
pub fn label(identifier: &str) -> String {
    identifier
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The bare error code out of an `__type` / `x-amzn-errortype` value, which may carry a
/// namespace or a URL beside it.
pub fn error_code(raw: &str) -> &str {
    let without_url = raw.split(':').next().unwrap_or(raw);
    without_url.rsplit('#').next().unwrap_or(without_url)
}

/// The emulator under test: one shared engine, one registered service, dispatched
/// in-process.
pub struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
}

pub fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(engine));
    Emulator { router }
}

impl Emulator {
    /// One request signed for [`REGION`], the region these suites address.
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.call_in_region(REGION, method, path, body).await
    }

    /// One request signed for `region`.
    ///
    /// The region a request addresses travels in its SigV4 credential scope, and a
    /// service reads it from there (`crate::state::request_region`), so a suite whose
    /// rule is about that region — an ARN compared against it, an ARN minted with it —
    /// drives this form and the rest keep [`call`](Self::call).
    pub async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://bedrock.{region}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| error_code(value).to_string());
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|value| error_code(value).to_string())
        });
        Reply {
            status,
            body,
            error,
        }
    }

    pub async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/guardrails", Some(body)).await
    }

    pub async fn get(&self, identifier: &str, version: Option<&str>) -> Reply {
        let identifier = label(identifier);
        let path = match version {
            Some(version) => format!("/guardrails/{identifier}?guardrailVersion={version}"),
            None => format!("/guardrails/{identifier}"),
        };
        self.call("GET", &path, None).await
    }

    pub async fn list(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/guardrails".to_string()
        } else {
            format!("/guardrails?{query}")
        };
        self.call("GET", &path, None).await
    }

    pub async fn update(&self, identifier: &str, body: Value) -> Reply {
        let identifier = label(identifier);
        self.call("PUT", &format!("/guardrails/{identifier}"), Some(body))
            .await
    }

    pub async fn create_version(&self, identifier: &str, body: Value) -> Reply {
        let identifier = label(identifier);
        self.call("POST", &format!("/guardrails/{identifier}"), Some(body))
            .await
    }

    pub async fn delete(&self, identifier: &str, version: Option<&str>) -> Reply {
        let identifier = label(identifier);
        let path = match version {
            Some(version) => format!("/guardrails/{identifier}?guardrailVersion={version}"),
            None => format!("/guardrails/{identifier}"),
        };
        self.call("DELETE", &path, None).await
    }

    /// Create a guardrail with only the required members and hand back its id.
    pub async fn create_named(&self, name: &str) -> String {
        let reply = self
            .create(json!({
                "name": name,
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            }))
            .await;
        assert_eq!(
            reply.status, 202,
            "case {name}: CreateGuardrail must answer 202, got {} {:?}",
            reply.status, reply.body
        );
        reply.text("guardrailId")
    }
}
