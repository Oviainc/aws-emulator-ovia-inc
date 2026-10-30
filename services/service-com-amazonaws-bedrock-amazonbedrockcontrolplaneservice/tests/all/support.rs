// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the emulator through.
//!
//! Extracted out of `guardrails.rs` so the other suites exercise the SAME dispatch path
//! rather than a second copy of it: one shared engine, one registered service, requests
//! signed the way an SDK signs them and dispatched in-process through the
//! `MultiServiceRouter` the emulator boots.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

/// The region every request in these suites signs for; it is what the minted ARNs must
/// carry.
pub const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries.
pub const ACCOUNT: &str = "000000000000";

/// One emulator answer: the HTTP status, the `content-type` it was serialized under, the
/// parsed JSON body, and the wire error code when it is a failure.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: Option<String>,
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

/// An RFC 3339 `date-time` as whole milliseconds since the epoch.
///
/// Beside [`date_time_seconds`] because every suite whose rule is about a `lastUpdatedAt`
/// concurrency token reads a reported instant and sends one back at THIS resolution — the
/// finest a client that carries milliseconds can echo — and one rendering of the pair is
/// what keeps those suites reading and writing the same wire form.
pub fn epoch_millis(case: &str, text: &str) -> i64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {text:?} does not parse as an RFC 3339 date-time: {error}")
    });
    parsed.secs() * 1_000 + i64::from(parsed.subsec_nanos() / 1_000_000)
}

/// `millis` since the epoch as the `date-time` text a client sends.
pub fn at_millis(case: &str, millis: i64) -> String {
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {millis} does not render: {error}"))
}

/// The test's own wall clock in epoch seconds, beside [`date_time_seconds`] so a suite that
/// bounds a response's instant against the clock reads it the same way every other suite
/// does: a stamp the emulator reports for a write must fall between the clock before the
/// call and the clock after it.
pub fn wall_clock_seconds() -> f64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// An identifier as an SDK writes it into a path label: an ARN carries `:` and `/`, and
/// both are percent-encoded so the whole identifier stays ONE path segment.
pub fn label(identifier: &str) -> String {
    identifier
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The `FoundationModelArn` of the base model `model_id` in `region`: a foundation model is
/// published by AWS rather than owned by the account reading it, so the ARN carries an
/// EMPTY account field
/// (`smithy_query(query="shape",shape="FoundationModelDetails")`).
///
/// Here rather than in one suite because three of them compare a response's ARN against
/// it, and the spelling they compare against must be ONE spelling.
pub fn foundation_model_arn(region: &str, model_id: &str) -> String {
    format!("arn:aws:bedrock:{region}::foundation-model/{model_id}")
}

/// The bare error code out of an `__type` / `x-amzn-errortype` value, which may carry a
/// namespace or a URL beside it.
pub fn error_code(raw: &str) -> &str {
    let without_url = raw.split(':').next().unwrap_or(raw);
    without_url.rsplit('#').next().unwrap_or(without_url)
}

/// Store the build workflow an admitted `StartAutomatedReasoningPolicyBuildWorkflow` would
/// have written `created_ago` ago, and hand back its `buildWorkflowId`.
///
/// The record is built out of the state helpers the start itself uses
/// ([`bedrock::state::mint_build_workflow_id`] and the status constants), so a seeded
/// workflow is the workflow a start would have left — and seeding is what lets a row fix the
/// instant a workflow was started, and so the order a listing must walk, or the status a
/// request must find it in, without sleeping for either. `tweak` is where a row differs from
/// that baseline.
///
/// Here rather than in one suite because two of them seed this record, and ONE rendering of
/// it is what keeps them from drifting apart when
/// [`bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord`] gains a field.
pub fn seed_build_workflow(
    emu: &Emulator,
    policy_arn: &str,
    workflow_type: &str,
    created_ago: Duration,
    tweak: impl FnOnce(&mut bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord),
) -> String {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let now = SystemTime::now();
    let created_at = now - created_ago;
    let mut store = state.lock();
    let id = store.mint_build_workflow_id(now);
    let mut record = bedrock::state::AutomatedReasoningPolicyBuildWorkflowRecord {
        policy_arn: policy_arn.to_string(),
        id: id.clone(),
        workflow_type: workflow_type.to_string(),
        status: bedrock::state::BUILD_WORKFLOW_STATUS_SCHEDULED,
        completed_due_at: Some(created_at + bedrock::state::BUILD_WORKFLOW_COMPLETE_AFTER),
        cancelled_due_at: None,
        document_name: None,
        document_content_type: None,
        document_description: None,
        created_at,
        updated_at: created_at,
    };
    tweak(&mut record);
    store
        .automated_reasoning_policy_build_workflows
        .insert((policy_arn.to_string(), id.clone()), record);
    id
}

/// The emulator under test: one shared engine, one registered service, dispatched
/// in-process.
pub struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    /// The very engine the registered service holds, so a suite whose rule is about a span
    /// of WALL-CLOCK time can read the store the handlers write and age a record it holds
    /// instead of sleeping for that span. Requests still travel through
    /// [`Self::call_in_region`]; this is never a second store.
    pub engine: Arc<engine::InMemoryEngine>,
}

pub fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
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
        self.call_with_headers(region, method, path, body, &[])
            .await
    }

    /// One request signed for `region`, carrying `headers` beside the ones every request
    /// here carries.
    ///
    /// The form a suite drives for an input member bound to a HEADER rather than to the
    /// body — an `@idempotencyToken` the model binds to `x-amz-client-token`, say — so
    /// such a member travels where the wire puts it.
    pub async fn call_with_headers(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
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
            .header("content-type", "application/json");
        let request = headers
            .iter()
            .fold(request, |request, (name, value)| {
                request.header(*name, *value)
            })
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
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
            content_type,
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
