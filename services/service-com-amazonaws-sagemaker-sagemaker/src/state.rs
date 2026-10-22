// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemaker-sagemaker service.
//!
//! The inference-deployment lifecycle (model -> endpoint config -> endpoint) is kept
//! here, in `BTreeMap`s keyed by the resource name AWS keys them by, so
//! `ListModels` has one stable order to sort and paginate over and every
//! `Describe*` replays exactly what the matching `Create*` stored.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AsyncInferenceConfig, ContainerDefinition, DataCaptureConfig, ExplainerConfig, IamIdentity,
    InferenceExecutionConfig, ManagedConfiguration, MetricsConfig, ProductionVariant,
    ProductionVariantSummary, Tag, UserContext, VpcConfig,
};

/// The ARN service namespace (`arnNamespace` in the model's `aws.api#service` trait,
/// also SageMaker's sigv4 signing name).
pub const ARN_SERVICE: &str = "sagemaker";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. It matches the region the emulator's generated shared config
/// hands the AWS CLI.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateEndpoint` the clock owes the `Creating` -> `InService`
/// transition. A caller that describes the endpoint straight after creating it still
/// observes `Creating`; one that waits (as `aws sagemaker wait endpoint-in-service`
/// does) observes `InService`, so a waiter terminates. Real SageMaker takes minutes;
/// the span is compressed, never zero.
pub const ENDPOINT_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// The most records each of this service's stores holds, after which a create is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// SageMaker publishes adjustable per-account quotas on models, endpoint
/// configurations and endpoints, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on each store rather than
/// a claim about any account's limit. It sits far above any workflow an emulator serves,
/// and it is what stops the state growing without end: every record it holds is released
/// by the matching `Delete*`.
pub const MODEL_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const MODEL_PACKAGE_GROUP_QUOTA: usize = 1_000;

/// The most models `ListModels` returns in one page, including when `MaxResults` is
/// absent or larger.
///
/// `MaxResults` is modelled `range 1..100` but has no documented default, so this is the
/// service's own server-side ceiling: one call can never serialise the whole store, and
/// a caller that wants the rest follows `NextToken`. Matches the ceiling the other
/// paginated services in this workspace apply.
pub const MAX_PAGE_SIZE: i32 = 50;

/// `EndpointStatus`'s members this emulator moves an endpoint through (the modelled
/// enum also carries the update, rollback and delete states, which this slice does not
/// reach). Named once here so no handler compares a bare string literal.
pub const STATUS_CREATING: &str = "Creating";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_SERVICE: &str = "InService";

/// The `ModelPackageGroupStatus` a model package group carries once `CreateModelPackageGroup`
/// has returned.
///
/// A model group is a registry container, not a provisioned resource: the call that creates
/// it is synchronous, and the group is immediately usable (a model package can be registered
/// into it on the very next call). So it is born in this TERMINAL status and the clock owes
/// it no later transition — the modelled enum's `Pending`/`InProgress` describe a group
/// whose creation is still in flight, which never happens here.
pub const STATUS_COMPLETED: &str = "Completed";

/// A model created by `CreateModel`, as `DescribeModel` and `ListModels` replay it.
#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub arn: String,
    pub primary_container: Option<ContainerDefinition>,
    pub containers: Option<Vec<ContainerDefinition>>,
    pub inference_execution_config: Option<InferenceExecutionConfig>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// An endpoint configuration created by `CreateEndpointConfig`, as
/// `DescribeEndpointConfig` replays it and `CreateEndpoint` reads it.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub name: String,
    pub arn: String,
    pub production_variants: Vec<ProductionVariant>,
    pub shadow_production_variants: Option<Vec<ProductionVariant>>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub kms_key_id: Option<String>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// An endpoint created by `CreateEndpoint`, as `DescribeEndpoint` replays it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub arn: String,
    pub config_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub production_variants: Vec<ProductionVariantSummary>,
    pub shadow_production_variants: Vec<ProductionVariantSummary>,
    pub data_capture_config: Option<DataCaptureConfig>,
    pub async_inference_config: Option<AsyncInferenceConfig>,
    pub explainer_config: Option<ExplainerConfig>,
    pub metrics_config: Option<MetricsConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl Endpoint {
    /// Materialise the `Creating` -> `InService` transition the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `in_service_at` whether or not anybody called, so the observer makes no
    /// difference to the result. `LastModifiedTime` moves to the instant the
    /// transition was due, not to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.last_modified_time = self.in_service_at;
            return true;
        }
        false
    }
}

/// A model package group created by `CreateModelPackageGroup`, as
/// `DescribeModelPackageGroup` replays it.
///
/// `created_by` is stored at creation rather than rebuilt per read, so every later read
/// reports the identity that actually created the group.
#[derive(Debug, Clone)]
pub struct ModelPackageGroup {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub managed_configuration: Option<ManagedConfiguration>,
    pub status: String,
    pub created_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// Per-service state for com-amazonaws-sagemaker-sagemaker. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerSagemakerState {
    /// Keyed by `ModelName`.
    pub models: BTreeMap<String, Model>,
    /// Keyed by `EndpointConfigName`.
    pub endpoint_configs: BTreeMap<String, EndpointConfig>,
    /// Keyed by `EndpointName`.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Keyed by `ModelPackageGroupName`.
    pub model_package_groups: BTreeMap<String, ModelPackageGroup>,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another model fits, i.e. the store is below [`MODEL_QUOTA`].
    pub fn models_below_quota(&self) -> bool {
        self.models.len() < MODEL_QUOTA
    }

    /// Whether another endpoint configuration fits, i.e. the store is below
    /// [`ENDPOINT_CONFIG_QUOTA`].
    pub fn endpoint_configs_below_quota(&self) -> bool {
        self.endpoint_configs.len() < ENDPOINT_CONFIG_QUOTA
    }

    /// Whether another endpoint fits, i.e. the store is below [`ENDPOINT_QUOTA`].
    pub fn endpoints_below_quota(&self) -> bool {
        self.endpoints.len() < ENDPOINT_QUOTA
    }

    /// Whether another model package group fits, i.e. the store is below
    /// [`MODEL_PACKAGE_GROUP_QUOTA`].
    pub fn model_package_groups_below_quota(&self) -> bool {
        self.model_package_groups.len() < MODEL_PACKAGE_GROUP_QUOTA
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerSagemakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerSagemakerState>)
}

/// The message a handler refuses with when the service is mounted on an engine that
/// holds no store for it. Unreachable in the assembled emulator (the router mounts
/// every service on the one shared `InMemoryEngine`); stated once so no handler
/// invents its own wording — and never by building a private engine, which would
/// drop the request's writes.
pub const NO_STATE: &str =
    "SageMaker state is unavailable: this service is not mounted on the shared in-memory engine";

/// Whether `value` is a well-formed AWS region token: lowercase alphanumerics and
/// hyphens, at most `emulator_telemetry::policy::REGION_MAX_LEN` (32) bytes.
///
/// An ARN's region component is positional, so a scope carrying `:` or `/` would mint a
/// structurally malformed ARN. Anything outside the grammar is not accepted as a region.
fn is_region_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region this request's SigV4 credential scope names, or [`DEFAULT_REGION`] when
/// the request is unsigned or its scope region is not a region token. Every ARN this
/// service mints takes its region from here, so a client signing for one region never
/// reads back an ARN naming another.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Record a call that changed this service's state, or a status transition it
/// materialised, so an emulator run is auditable the way CloudTrail makes a real one.
///
/// Carries only request metadata: the operation, the resource ARN it changed and the
/// access-key id the REQUEST presented (`emulator_server::extract_akid`). The
/// emulator's own credentials and every secret stay out of it.
pub fn record_state_change(
    operation: &str,
    resource_arn: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    tracing::info!(
        operation,
        resource = resource_arn,
        access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
        "sagemaker state change"
    );
}

/// `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` — the one ARN form this
/// service mints, for the three resource kinds it owns.
pub fn resource_arn(region: &str, kind: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("{kind}/{name}"))
}

/// The resource NAME an `ArnOrName`-typed member addresses.
///
/// Several SageMaker readers accept either a bare name or the resource's full ARN for the
/// same resource (the `ArnOrName` shape), and the store is keyed by the name — so an ARN is
/// reduced to the resource portion after its final `/`. A value that is not an ARN is
/// already a name and is returned unchanged.
pub fn name_from_arn_or_name(value: &str) -> &str {
    match value.strip_prefix("arn:") {
        Some(_) => value.split('/').next_back().unwrap_or(value),
        None => value,
    }
}

/// The IAM user name the request's SigV4 credential names, or a fixed `emulator`
/// principal when the request carries no usable access key id.
///
/// The access key id is request text and the value is spliced into an ARN, whose segments
/// are positional, so anything not shaped like an access key id is replaced rather than
/// interpolated. `extract_akid` performs no verification — the emulator verifies signatures
/// at its boundary.
fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
    let shaped_like_an_access_key_id = !akid.is_empty()
        && akid.len() <= 128
        && akid.chars().all(|character| character.is_ascii_alphanumeric());
    if shaped_like_an_access_key_id {
        akid
    } else {
        "emulator".to_string()
    }
}

/// The `UserContext` SageMaker records as the creator of a resource.
///
/// The emulator serves no Studio domain, so the Studio members (`UserProfileArn`,
/// `UserProfileName`, `DomainId`) are ABSENT rather than empty: a caller cannot tell an
/// empty profile name from a missing one. The IAM identity is the one the request signed
/// with. `PrincipalId` and `SourceIdentity` stay absent — this emulator models no IAM
/// principal ids, and inventing one would report an identity that does not exist.
pub fn caller_user_context(ctx: &smithy_server_runtime::OperationContext<'_>) -> UserContext {
    let principal = caller_principal(ctx);
    UserContext {
        user_profile_arn: None,
        user_profile_name: None,
        domain_id: None,
        iam_identity: Some(IamIdentity {
            // IAM is global, so the region segment is empty.
            arn: Some(::emulator_server::emulator_arn(
                "iam",
                "",
                &format!("user/{principal}"),
            )),
            principal_id: None,
            source_identity: None,
        }),
    }
}

/// A stored `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_and_nanos(
            i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
            since_epoch.subsec_nanos(),
        ),
        // A host clock set before the epoch: keep the sign rather than clamping the
        // instant to zero and reporting a time the resource was not created at. Built
        // from whole seconds plus a non-negative nanos remainder, so this branch is as
        // lossless as the one above.
        Err(before_epoch) => {
            let magnitude = before_epoch.duration();
            let secs = i64::try_from(magnitude.as_secs()).unwrap_or(i64::MAX);
            match magnitude.subsec_nanos() {
                0 => ::aws_smithy_types::DateTime::from_secs(-secs),
                nanos => ::aws_smithy_types::DateTime::from_secs_and_nanos(
                    -secs - 1,
                    1_000_000_000 - nanos,
                ),
            }
        }
    }
}

/// A modelled `Timestamp` as a comparable key, so a stored time and a filter bound
/// order against each other without going through a lossy float.
pub fn timestamp_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}
