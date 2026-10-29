// SPDX-License-Identifier: CC0-1.0

//! The ONE test harness every suite of this crate shares.
//!
//! Two things every suite of this crate needs, stated here ONCE instead of once per
//! suite:
//!
//! * how a wire answer is REDUCED to what a suite asserts on — the status, the bare
//!   error code behind `x-amzn-errortype`, the `Message` member of a restJson1 error
//!   document, and the raw body ([`Reply`]) — and how a request is DISPATCHED, including
//!   the access key id it presents ([`send`], [`send_as`]), which is the one thing that
//!   names the principal it speaks for;
//! * how a job comes to EXIST for a test of this crate — this service's model binds no
//!   operation that creates, lists or deletes a job, so the shared job directory the
//!   SageMaker control plane writes is the only place an ARN can be resolved
//!   ([`mounted_service`], [`create_job`], [`create_job_at_arn`], [`register_job`],
//!   [`delete_job`]).
//!
//! It owns no assertion and no expectation: a suite states its own rule, its own rows
//! and its own messages, and reaches the emulator through the request and reduction
//! helpers below so that a change to how the emulator renders an error envelope is
//! chased through one body rather than one per suite.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService;
use service_common::agentic_rft_jobs::{JobDirectory, JobRecord};
use smithy_server_runtime::MultiServiceRouter;

/// This service's own endpoint host, built from the model's endpoint prefix
/// (`job-runtime.sagemaker`).
pub const HOST: &str = "job-runtime.sagemaker.us-east-1.amazonaws.com";

/// The header `JobArn` is bound to.
pub const JOB_ARN_HEADER: &str = "x-amzn-sagemaker-job-arn";

/// The header `TrajectoryId` is bound to on the sampling operations.
pub const TRAJECTORY_ID_HEADER: &str = "x-amzn-sagemaker-trajectory-id";

/// The four URIs this model binds, one per operation.
pub const SAMPLE_PATH: &str = "/sample";
pub const SAMPLE_STREAM_PATH: &str = "/sample-with-response-stream";
pub const COMPLETE_ROLLOUT_PATH: &str = "/complete-rollout";
pub const UPDATE_REWARD_PATH: &str = "/update-reward";

/// The one account every emulated service of this project presents, and the Region it
/// serves: the pair the SageMaker control plane mints a job ARN in.
pub const ACCOUNT: &str = "000000000000";
pub const REGION: &str = "us-east-1";

/// The access key id a call carries when a suite does not choose one: a key neither STS
/// nor IAM holds a record for, which is the position the account's own published
/// credential is in.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// A SigV4 `Authorization` header whose credential scope names `service` — the primary
/// routing signal `resolve_route_key` reads — presenting `access_key_id`, which is what
/// names the principal the request speaks for. Request authentication is off on the test
/// router, so the signature itself is never checked.
#[must_use]
pub fn authorization_as(access_key_id: &str, service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential={access_key_id}/20240101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host, Signature=\
         0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The same header, presenting [`DEFAULT_ACCESS_KEY_ID`].
#[must_use]
pub fn authorization(service: &str) -> String {
    authorization_as(DEFAULT_ACCESS_KEY_ID, service)
}

/// This service's own sigv4 signing name, which the credential scope of a request to it
/// carries (`smithy_query(query="service_info")`: `sigv4Name`).
pub const SIGNING_NAME: &str = "sagemaker";

/// One HTTP answer, reduced to what the suites of this crate assert on.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    /// `x-amzn-errortype` — how restJson1 names the error shape.
    pub error_type: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl Reply {
    /// Reduce one dispatched answer to the parts a suite asserts on.
    #[must_use]
    pub fn of(response: http::Response<bytes::Bytes>) -> Self {
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string)
        };
        let error_type = header("x-amzn-errortype");
        let content_type = header("content-type");
        Self {
            status,
            error_type,
            content_type,
            body: response.into_body().to_vec(),
        }
    }

    /// The bare error code out of `x-amzn-errortype`, which may carry a namespace or a
    /// Coral shape-id suffix alongside the name (`ValidationException:http://…`): an SDK
    /// reads the name before the colon, after the last `#`.
    #[must_use]
    pub fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_default();
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `Message` member of a restJson1 error document.
    ///
    /// Read under the name the MODEL declares (`Message`, on every error shape of this
    /// service) and under no other: the framework's own pre-dispatch refusal envelope
    /// writes a lowercase `message` instead, so accepting both would hide a refusal that
    /// never reached this service's modelled errors.
    #[must_use]
    pub fn message(&self) -> String {
        self.json()
            .get("Message")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    /// The body as a JSON document, or [`Value::Null`] when it is not JSON at all.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The body as text, for a failure message.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A router carrying exactly this service over one shared engine — the same assembly
/// `core/router` performs, minus the other services. The engine comes back too, so a test
/// can create a job the way the control plane does and read this service's store the way
/// the next request would.
#[must_use]
pub fn mounted_service() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Dispatch one request to `path` with `headers` and `body`, signed for this service with
/// [`DEFAULT_ACCESS_KEY_ID`], and reduce the answer.
pub async fn send(
    router: &MultiServiceRouter,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    send_as(router, DEFAULT_ACCESS_KEY_ID, path, headers, body).await
}

/// The same dispatch, presenting `access_key_id` — the one thing that names the principal
/// a request speaks for, so a suite about WHO may call an operation varies it and every
/// other suite takes the default.
pub async fn send_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{HOST}{path}"))
        .header("authorization", authorization_as(access_key_id, SIGNING_NAME))
        .header("host", HOST);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    Reply::of(router.dispatch(request).await)
}

/// A job ARN assembled from the parts the modelled
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
/// pattern lets vary, so a row can vary the partition, the Region, the account, the job
/// category and the job name independently.
#[must_use]
pub fn job_arn(partition: &str, region: &str, account: &str, category: &str, name: &str) -> String {
    format!("arn:{partition}:sagemaker:{region}:{account}:job/{category}/{name}")
}

/// The ARN the SageMaker control plane mints for the job `category` / `name` in the
/// account and Region this emulator serves.
#[must_use]
pub fn minted_job_arn(category: &str, name: &str) -> String {
    job_arn("aws", REGION, ACCOUNT, category, name)
}

/// Record the job `category` / `name`, addressable at `arn`, in the shared job directory —
/// the registration the SageMaker control plane's `CreateJob` writes, which is what makes a
/// job addressable in this data plane. Every other create helper here funnels through this
/// one.
pub fn register_job(engine: &engine::InMemoryEngine, category: &str, name: &str, arn: &str) {
    engine.state::<JobDirectory>().lock().register(JobRecord {
        name: name.to_string(),
        category: category.to_string(),
        arn: arn.to_string(),
    });
}

/// Create the job `category` / `name`, addressable at the ARN the control plane would mint
/// for it.
pub fn create_job(engine: &engine::InMemoryEngine, category: &str, name: &str) {
    register_job(engine, category, name, &minted_job_arn(category, name));
}

/// Create the job `job_arn` names, taking its category and name out of the ARN's own
/// resource path and recording it under the ARN as given — which is how a row addresses a
/// job through another partition, Region or account.
pub fn create_job_at_arn(engine: &engine::InMemoryEngine, job_arn: &str) {
    let resource = ::service_common::arn::resource_id_of_type(job_arn, "job")
        .expect("a job ARN naming a job resource");
    let (category, name) = resource
        .split_once('/')
        .expect("a job resource of the form <category>/<name>");
    register_job(engine, category, name, job_arn);
}

/// Drop the job `category` / `name` from the shared directory, as the control plane's
/// delete does.
pub fn delete_job(engine: &engine::InMemoryEngine, category: &str, name: &str) {
    engine.state::<JobDirectory>().lock().forget(category, name);
}
