// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambda-awsgirapiservice service.
//!
//! The function lifecycle slice keeps every Lambda function this emulator holds in one
//! ordered map, so `ListFunctions` paginates deterministically and every operation
//! answers from the single record `CreateFunction` persisted.
//!
//! A function has one lifetime of its own. The model's own `CreateFunction`
//! documentation says: "When you create a function, Lambda provisions an instance of
//! the function and its supporting resources. [...] During this time, you can't invoke
//! or modify the function. The State, StateReason, and StateReasonCode fields in the
//! response from GetFunctionConfiguration indicate when the function is ready to
//! invoke." So a freshly created function is `Pending`, and the move to `Active` is
//! owed by the CLOCK rather than by anybody calling: the record stores the instant the
//! activation becomes due and [`FunctionRecord::materialize`] brings it up to whatever
//! the clock already made due. Nothing sleeps, and no read invents a transition that
//! was not owed.
//!
//! `GetAccountSettings` reports on the same records at the account level:
//! "Retrieves details about your account's limits and usage in an Amazon Web Services
//! Region", so its usage is what the store holds and its unreserved concurrency is what
//! those records reserve (see
//! [`ComAmazonawsLambdaAwsgirapiserviceState::account_usage`] and
//! [`ComAmazonawsLambdaAwsgirapiserviceState::account_limit`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use sha2::Digest as _;

use crate::ops::types::{
    AccountLimit, AccountUsage, CreateFunctionOutput, EnvironmentResponse, EphemeralStorage,
    FunctionCodeLocation, PutFunctionConcurrencyOutput, ResolvedS3Object, TracingConfigResponse,
};

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `lambda`").
const ARN_NAMESPACE: &str = "lambda";

/// The region an ARN carries when the request is unsigned and therefore names no
/// SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The unpublished version label every function always has, as the model's `Version`
/// pattern (`^(\$LATEST|[0-9]+)$`) and `CreateFunction`'s documentation ("A function
/// has an unpublished version") name it.
pub const VERSION_LATEST: &str = "$LATEST";

/// `PackageType` values, from the model's `PackageType` enum (`Zip | Image`).
pub const PACKAGE_TYPE_ZIP: &str = "Zip";
/// See [`PACKAGE_TYPE_ZIP`].
pub const PACKAGE_TYPE_IMAGE: &str = "Image";

/// `State` values this slice moves a function through, from the model's `State` enum.
pub const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
pub const STATE_ACTIVE: &str = "Active";

/// The `StateReasonCode` a function carries while it is being provisioned, from the
/// model's `StateReasonCode` enum, whose documentation reads: "When the code is
/// Creating, you can't invoke or modify the function."
pub const STATE_REASON_CODE_CREATING: &str = "Creating";

/// The `StateReason` that accompanies [`STATE_REASON_CODE_CREATING`].
pub const STATE_REASON_CREATING: &str = "The function is being created.";

/// The `LastUpdateStatus` a settled function carries. The model documents the member
/// as "The status of the last update that was performed on the function. This is first
/// set to Successful after function creation completes", so it is absent while the
/// function is still `Pending`.
pub const LAST_UPDATE_STATUS_SUCCESSFUL: &str = "Successful";

/// How long after `CreateFunction` the move from `Pending` to `Active` becomes due.
///
/// An EMULATOR-CHOSEN span (the real provisioning time depends on the package and on
/// whether the function joins a VPC, and is not in the model). It is deliberately
/// non-zero, so a caller that acts immediately after `CreateFunction` still observes
/// the `Pending` state the model documents and the state errors that go with it.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The default `MemorySize`, in MB: "The default value is 128 MB", as AWS documents
/// for the member (the model carries only its 128..32768 range).
pub const DEFAULT_MEMORY_SIZE_MB: i32 = 128;

/// The default `Timeout`, in seconds: "The default is 3 seconds", as AWS documents for
/// the member (the model carries only its 1..5400 range).
pub const DEFAULT_TIMEOUT_SECONDS: i32 = 3;

/// The default `EphemeralStorage` size, in MB, which the model's own member
/// documentation states: "The default value is 512, but can be any whole number
/// between 512 and 10,240 MB."
pub const DEFAULT_EPHEMERAL_STORAGE_MB: i32 = 512;

/// The default `Architectures` entry, which the model's own member documentation
/// states: "Enter a string array with one of the valid values (arm64 or x86_64). The
/// default value is x86_64."
pub const DEFAULT_ARCHITECTURE: &str = "x86_64";

/// The default `TracingConfig` mode, from the model's `TracingMode` enum
/// (`Active | PassThrough`); `PassThrough` is the mode a function gets when the caller
/// names no tracing configuration, because X-Ray then samples nothing of its own.
pub const DEFAULT_TRACING_MODE: &str = "PassThrough";

/// The `FunctionCodeLocation` `RepositoryType` of a .zip file archive package: Lambda
/// keeps the archive in its own S3 bucket and vends a download location for it.
pub const REPOSITORY_TYPE_S3: &str = "S3";
/// The `FunctionCodeLocation` `RepositoryType` of a container image package: the image
/// lives in the Amazon ECR registry.
pub const REPOSITORY_TYPE_ECR: &str = "ECR";

/// The longest a bare function name may be: "The length constraint applies only to the
/// full ARN. If you specify only the function name, it is limited to 64 characters in
/// length", as the model documents on `FunctionName`.
pub const MAX_BARE_FUNCTION_NAME_LEN: usize = 64;

/// The most entries `ListFunctions` puts in one page, which the model documents twice:
/// on the operation ("Lambda returns up to 50 functions per call") and on its
/// `MaxItems` member ("Note that ListFunctions returns a maximum of 50 items in each
/// response, even if you set the number higher"). It is therefore both the page size a
/// caller that names no `MaxItems` gets and the ceiling on the one who does.
pub const MAX_PAGE_SIZE: usize = 50;

/// The most functions this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `CreateFunction` adds to it, so past this point the slice answers the modelled
/// `CodeStorageExceededException` rather than growing without bound. AWS caps the
/// total code storage of an account and answers the same modelled error there.
pub const MAX_FUNCTIONS: usize = 1_000;

/// `AccountLimit$TotalCodeSize` — "The amount of storage space that you can use for all
/// deployment packages and layer archives", in bytes.
///
/// AWS publishes this quota as 75 GB per account per Region ("Function and layer
/// storage", Lambda quotas), which is 75 * 1024^3 bytes. It is adjustable on a real
/// account; this emulator models neither Service Quotas nor an account's adjustments, so
/// it reports the default.
pub const TOTAL_CODE_SIZE_LIMIT: i64 = 80_530_636_800;

/// `AccountLimit$CodeSizeUnzipped` — "The maximum size of a function's deployment
/// package and layers when they're extracted", in bytes.
///
/// AWS publishes 250 MB (Lambda quotas, "Deployment package (.zip file archive) size …
/// 250 MB (unzipped)"), which is 250 * 1024^2 bytes. Not adjustable.
pub const CODE_SIZE_UNZIPPED_LIMIT: i64 = 262_144_000;

/// `AccountLimit$CodeSizeZipped` — "The maximum size of a deployment package when it's
/// uploaded directly to Lambda", in bytes.
///
/// AWS publishes 50 MB for a direct upload (Lambda quotas, "50 MB (zipped, for direct
/// upload)"), which is 50 * 1024^2 bytes. Not adjustable.
pub const CODE_SIZE_ZIPPED_LIMIT: i64 = 52_428_800;

/// `AccountLimit$ConcurrentExecutions` — "The maximum number of simultaneous function
/// executions".
///
/// AWS publishes a default of 1,000 per Region (Lambda quotas, "Concurrent executions"),
/// adjustable on a real account; this emulator reports the default for the same reason
/// as [`TOTAL_CODE_SIZE_LIMIT`].
pub const CONCURRENT_EXECUTIONS_LIMIT: i32 = 1_000;

/// Where the code of a function came from.
///
/// `zip_bytes` is the only variant whose bytes this emulator holds, so it is the only
/// one `code_size` and `code_sha256` can be computed from; an S3 object or a container
/// image is a reference this emulator does not resolve, and nothing is reported for it
/// that was not computed.
#[derive(Debug, Clone, Default)]
pub struct CodeRecord {
    /// The deployment package bytes, when the caller uploaded them inline.
    pub zip_bytes: Option<Vec<u8>>,
    /// The S3 bucket holding the deployment package, when the caller named one.
    pub s3_bucket: Option<String>,
    /// The S3 key of the deployment package.
    pub s3_key: Option<String>,
    /// The S3 object version of the deployment package.
    pub s3_object_version: Option<String>,
    /// The container image URI, for an `Image` package.
    pub image_uri: Option<String>,
}

impl CodeRecord {
    /// Whether this package is a container image.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.image_uri.is_some()
    }
}

/// The configuration of one function version — the members `FunctionConfiguration`
/// reports that a published version freezes.
///
/// Held apart from [`FunctionRecord`] so publishing a version is a snapshot of exactly
/// these fields and nothing else.
#[derive(Debug, Clone)]
pub struct FunctionConfig {
    /// The function's name (never an ARN or a qualified name).
    pub name: String,
    /// The execution role ARN, as supplied.
    pub role: String,
    /// The runtime identifier, absent for an `Image` package.
    pub runtime: Option<String>,
    /// The handler, absent for an `Image` package.
    pub handler: Option<String>,
    /// The description, when the caller gave one.
    pub description: Option<String>,
    /// The timeout, in seconds.
    pub timeout: i32,
    /// The memory size, in MB.
    pub memory_size: i32,
    /// `Zip` or `Image`.
    pub package_type: String,
    /// Exactly one architecture, per the model's `@length(min: 1, max: 1)`.
    pub architectures: Vec<String>,
    /// The environment variables, ordered so a read reports them deterministically.
    pub environment: BTreeMap<String, String>,
    /// The `/tmp` size, in MB.
    pub ephemeral_storage_mb: i32,
    /// The X-Ray tracing mode.
    pub tracing_mode: String,
    /// The KMS key ARN, when the caller named one.
    pub kms_key_arn: Option<String>,
    /// The size of the stored deployment package, in bytes; 0 when this emulator holds
    /// no bytes for it (an S3 object or a container image it does not resolve).
    pub code_size: i64,
    /// The base64 SHA-256 of the stored deployment package bytes; absent when this
    /// emulator holds no bytes to hash.
    pub code_sha256: Option<String>,
    /// ISO-8601 instant of the last change to this configuration.
    pub last_modified: String,
    /// The revision this configuration is at.
    pub revision_id: String,
}

/// One published version of a function: its frozen configuration plus when it was
/// published.
#[derive(Debug, Clone)]
pub struct FunctionVersionRecord {
    /// The numeric version label ("1", "2", ...).
    pub version: u64,
    /// The configuration this version froze.
    pub config: FunctionConfig,
    /// ISO-8601 instant the version was published.
    pub created: String,
}

/// One Lambda function: its unpublished (`$LATEST`) configuration, its code, its
/// lifecycle state, its tags and its published versions.
#[derive(Debug, Clone)]
pub struct FunctionRecord {
    /// The unqualified function ARN, minted once at creation.
    pub arn: String,
    /// The region the function was created in, which its ARN and its code location
    /// carry.
    pub region: String,
    /// The `$LATEST` configuration.
    pub config: FunctionConfig,
    /// Where the code came from.
    pub code: CodeRecord,
    /// The model's `State` of the function.
    pub state: String,
    /// The model's `StateReason`, while there is one.
    pub state_reason: Option<String>,
    /// The model's `StateReasonCode`, while there is one.
    pub state_reason_code: Option<String>,
    /// The model's `LastUpdateStatus`, first set once creation completes.
    pub last_update_status: Option<String>,
    /// When the move to [`STATE_ACTIVE`] becomes due.
    pub active_due: SystemTime,
    /// Function-level tags, as supplied.
    pub tags: BTreeMap<String, String>,
    /// The reserved concurrency, when `PutFunctionConcurrency` has set one.
    pub reserved_concurrent_executions: Option<i32>,
    /// The highest version published so far; 0 when none has been.
    pub latest_version: u64,
    /// The published versions, ordered by version number.
    pub versions: BTreeMap<u64, FunctionVersionRecord>,
}

impl FunctionRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transition it applies is the one the clock owed at `now`: a function
    /// that has finished provisioning becomes `Active`. It would have been owed at the
    /// same instant even if nobody had called, so applying it on a read reports the
    /// truth rather than inventing a change.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.state == STATE_PENDING && now >= self.active_due {
            self.state = STATE_ACTIVE.to_string();
            self.state_reason = None;
            self.state_reason_code = None;
            self.last_update_status = Some(LAST_UPDATE_STATUS_SUCCESSFUL.to_string());
        }
    }

    /// The ARN of `version` of this function: the unqualified ARN for `$LATEST`, and
    /// the `:<version>`-suffixed one for a published version, exactly as the model's
    /// `NameSpacedFunctionArn` pattern allows.
    #[must_use]
    pub fn versioned_arn(&self, version: &str) -> String {
        if version == VERSION_LATEST {
            self.arn.clone()
        } else {
            format!("{}:{}", self.arn, version)
        }
    }

    /// The configuration of `version`, or `None` when this function has no such
    /// version.
    #[must_use]
    pub fn config_of(&self, version: &str) -> Option<&FunctionConfig> {
        if version == VERSION_LATEST {
            return Some(&self.config);
        }
        version
            .parse::<u64>()
            .ok()
            .and_then(|number| self.versions.get(&number))
            .map(|published| &published.config)
    }

    /// The `FunctionConfiguration` the wire reports for `version`, built entirely out
    /// of what is stored.
    #[must_use]
    pub fn configuration(&self, version: &str, config: &FunctionConfig) -> CreateFunctionOutput {
        CreateFunctionOutput {
            function_name: Some(config.name.clone()),
            function_arn: Some(self.versioned_arn(version)),
            runtime: config.runtime.clone(),
            role: Some(config.role.clone()),
            handler: config.handler.clone(),
            code_size: config.code_size,
            description: config.description.clone(),
            timeout: Some(config.timeout),
            memory_size: Some(config.memory_size),
            last_modified: Some(config.last_modified.clone()),
            code_sha256: config.code_sha256.clone(),
            version: Some(version.to_string()),
            environment: (!config.environment.is_empty()).then(|| EnvironmentResponse {
                variables: Some(
                    config
                        .environment
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect(),
                ),
                error: None,
            }),
            kms_key_arn: config.kms_key_arn.clone(),
            tracing_config: Some(TracingConfigResponse {
                mode: Some(config.tracing_mode.clone()),
            }),
            revision_id: Some(config.revision_id.clone()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            state_reason_code: self.state_reason_code.clone(),
            last_update_status: self.last_update_status.clone(),
            package_type: Some(config.package_type.clone()),
            architectures: Some(config.architectures.clone()),
            ephemeral_storage: Some(EphemeralStorage {
                size: config.ephemeral_storage_mb,
            }),
            ..CreateFunctionOutput::default()
        }
    }

    /// The `FunctionCodeLocation` `GetFunction` reports for this function's package.
    #[must_use]
    pub fn code_location(&self) -> FunctionCodeLocation {
        if let Some(image_uri) = &self.code.image_uri {
            // A container image is a reference into the Amazon ECR registry; the
            // resolved URI is the one the function was created with, because nothing
            // here re-tags it.
            return FunctionCodeLocation {
                repository_type: Some(REPOSITORY_TYPE_ECR.to_string()),
                image_uri: Some(image_uri.clone()),
                resolved_image_uri: Some(image_uri.clone()),
                ..FunctionCodeLocation::default()
            };
        }
        FunctionCodeLocation {
            repository_type: Some(REPOSITORY_TYPE_S3.to_string()),
            location: Some(code_location_url(
                &self.region,
                &self.config.name,
                &self.config.revision_id,
            )),
            resolved_s3_object: self.code.s3_bucket.as_ref().map(|bucket| ResolvedS3Object {
                s3_bucket: Some(bucket.clone()),
                s3_key: self.code.s3_key.clone(),
                s3_object_version: self.code.s3_object_version.clone(),
            }),
            ..FunctionCodeLocation::default()
        }
    }

    /// The `Concurrency` `GetFunction` reports, present only once reserved concurrency
    /// has been set on this function.
    #[must_use]
    pub fn concurrency(&self) -> Option<PutFunctionConcurrencyOutput> {
        self.reserved_concurrent_executions
            .map(|reserved| PutFunctionConcurrencyOutput {
                reserved_concurrent_executions: Some(reserved),
            })
    }
}

/// Per-service state for com-amazonaws-lambda-awsgirapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdaAwsgirapiserviceState {
    /// Every function this emulator holds, keyed by function name so `ListFunctions`
    /// pages through them in a stable order.
    pub functions: BTreeMap<String, FunctionRecord>,
}

/// One entry of a `ListFunctions` page: the key it is addressed by in the flattened
/// ordering, and the configuration to report.
pub struct ListedFunction {
    /// The pagination key: the function name for `$LATEST`, `name:version` for a
    /// published version.
    pub key: String,
    /// The configuration to report for the entry.
    pub configuration: CreateFunctionOutput,
}

impl ComAmazonawsLambdaAwsgirapiserviceState {
    /// Bring every function up to whatever the clock has already made due; see
    /// [`FunctionRecord::materialize`].
    pub fn materialize_all(&mut self, now: SystemTime) {
        for function in self.functions.values_mut() {
            function.materialize(now);
        }
    }

    /// Every entry `ListFunctions` can report, in pagination order.
    ///
    /// With `include_all_versions` each function's published versions are listed
    /// beside its `$LATEST` entry, which is what `FunctionVersion=ALL` asks for ("Set
    /// to ALL to include entries for all published versions of each function").
    ///
    /// The result is ordered BY ITS PAGINATION KEY, not by the order the entries are
    /// generated in. `ListFunctions` continues a page by taking the first entry whose
    /// key sorts after the caller's `Marker`, so the sequence has to be sorted the same
    /// way or an entry could be skipped: a version key (`name:1`) does not sort
    /// directly after its function's own key when another function's name extends that
    /// name (`fn-1`, `fn-1:1`, `fn-10`), because `:` sorts after a digit.
    #[must_use]
    pub fn listed_functions(&self, include_all_versions: bool) -> Vec<ListedFunction> {
        let mut entries = Vec::new();
        for function in self.functions.values() {
            entries.push(ListedFunction {
                key: function.config.name.clone(),
                configuration: listed_configuration(
                    function.configuration(VERSION_LATEST, &function.config),
                ),
            });
            if include_all_versions {
                for published in function.versions.values() {
                    let label = published.version.to_string();
                    entries.push(ListedFunction {
                        key: format!("{}:{label}", function.config.name),
                        configuration: listed_configuration(
                            function.configuration(&label, &published.config),
                        ),
                    });
                }
            }
        }
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        entries
    }

    /// `AccountUsage`: "The number of functions and amount of storage in use", computed
    /// from the functions this store holds.
    ///
    /// `FunctionCount` is "The number of Lambda functions", so it counts functions rather
    /// than their versions. `TotalCodeSize` is "The amount of storage space, in bytes,
    /// that's being used by deployment packages and layer archives": each published
    /// version keeps its own package, and AWS counts every one of them against the code
    /// storage quota, so the sum covers `$LATEST` and each published version. Layer
    /// archives add their own term here once `PublishLayerVersion` persists one.
    #[must_use]
    pub fn account_usage(&self) -> AccountUsage {
        let total_code_size = self
            .functions
            .values()
            .flat_map(|function| {
                std::iter::once(function.config.code_size).chain(
                    function
                        .versions
                        .values()
                        .map(|published| published.config.code_size),
                )
            })
            .fold(0i64, i64::saturating_add);
        AccountUsage {
            total_code_size,
            function_count: i64::try_from(self.functions.len()).unwrap_or(i64::MAX),
        }
    }

    /// `AccountLimit`: the account's quotas, with the one limit that is not a constant
    /// computed from the store.
    ///
    /// `UnreservedConcurrentExecutions` is modelled as "The maximum number of
    /// simultaneous function executions, minus the capacity that's reserved for
    /// individual functions with PutFunctionConcurrency", so it is the account's
    /// concurrency limit less the concurrency the stored functions reserve. The member is
    /// `@range(min: 0)`, so the subtraction is floored at zero.
    #[must_use]
    pub fn account_limit(&self) -> AccountLimit {
        let reserved = self
            .functions
            .values()
            .filter_map(|function| function.reserved_concurrent_executions)
            .fold(0i32, i32::saturating_add);
        AccountLimit {
            total_code_size: TOTAL_CODE_SIZE_LIMIT,
            code_size_unzipped: CODE_SIZE_UNZIPPED_LIMIT,
            code_size_zipped: CODE_SIZE_ZIPPED_LIMIT,
            concurrent_executions: CONCURRENT_EXECUTIONS_LIMIT,
            unreserved_concurrent_executions: Some(
                CONCURRENT_EXECUTIONS_LIMIT.saturating_sub(reserved).max(0),
            ),
        }
    }
}

/// Drop the members `ListFunctions` does not report.
///
/// The model's own `ListFunctions` documentation states which they are: "The
/// ListFunctions operation returns a subset of the FunctionConfiguration fields. To get
/// the additional fields (State, StateReasonCode, StateReason, LastUpdateStatus,
/// LastUpdateStatusReason, LastUpdateStatusReasonCode, RuntimeVersionConfig) for a
/// function or version, use GetFunction."
fn listed_configuration(mut configuration: CreateFunctionOutput) -> CreateFunctionOutput {
    configuration.state = None;
    configuration.state_reason = None;
    configuration.state_reason_code = None;
    configuration.last_update_status = None;
    configuration.last_update_status_reason = None;
    configuration.last_update_status_reason_code = None;
    configuration.runtime_version_config = None;
    configuration
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLambdaAwsgirapiserviceState>)
}

/// The unqualified ARN of a function:
/// `arn:aws:lambda:<region>:<account>:function:<name>`.
#[must_use]
pub fn function_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("function:{name}"))
}

/// The region this request's ARNs carry: the one the caller signed for, from the SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for an unsigned request.
#[must_use]
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The download location `GetFunction` reports for a .zip file archive package.
///
/// Lambda keeps the archive in its own per-region task bucket and vends a URL into it.
/// Every segment here comes from stored state — the function's region, its name and
/// the revision the stored code belongs to — so two reads of an unchanged function
/// report the same location, and a new revision reports a new one.
#[must_use]
pub fn code_location_url(region: &str, name: &str, revision_id: &str) -> String {
    format!(
        "https://awslambda-{region}-tasks.s3.{region}.amazonaws.com/snapshots/{account}/{name}-{revision_id}",
        account = ::engine::emulator_account_id(),
    )
}

/// `at` as the ISO-8601 instant the model's `LastModified` member describes
/// ("in ISO-8601 format (YYYY-MM-DDThh:mm:ss.sTZD)").
#[must_use]
pub fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The base64-encoded SHA-256 of `bytes`, which is what the model's `CodeSha256`
/// member reports ("The SHA256 hash of the function's deployment package").
#[must_use]
pub fn code_sha256(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// A fresh revision id. Lambda's `RevisionId` is a UUID, and a new one is minted on
/// every change to a function's code or configuration.
#[must_use]
pub fn mint_revision_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// A `FunctionName` the wire accepted, split into the parts that identify a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionReference {
    /// The bare function name.
    pub name: String,
    /// The qualifier the name carried, when it carried one.
    pub qualifier: Option<String>,
    /// Whether the reference was given as a full or partial ARN rather than a name.
    pub was_arn: bool,
}

/// Split a `FunctionName` into its name and its qualifier.
///
/// The model documents the four forms a `FunctionName` may take — "Function name –
/// my-function[:qualifier]", "Function ARN –
/// arn:aws:lambda:us-west-2:123456789012:function:my-function[:qualifier]", "Partial
/// ARN – 123456789012:function:my-function[:qualifier]" — and its pattern also admits
/// the bare `function:my-function` prefix. All of them name the same record, so they
/// all resolve here to the same [`FunctionReference`].
///
/// `None` when the value is not any of those forms.
#[must_use]
pub fn parse_function_reference(value: &str) -> Option<FunctionReference> {
    let mut parts: Vec<&str> = value.split(':').collect();
    let mut was_arn = false;

    // Full ARN: arn:<partition>:lambda:<region>:<account>:function:<name>[:<qualifier>].
    // The resource type is required, so an ARN of another Lambda resource (a layer, a
    // code-signing config) is not read as a function named after its resource type.
    if parts.first() == Some(&"arn") {
        if parts.len() < 7 || parts[2] != ARN_NAMESPACE || parts[5] != "function" {
            return None;
        }
        parts.drain(..5);
        was_arn = true;
    }

    // Partial ARN: <account>:function:<name>[:<qualifier>]. The `function` literal is
    // required for the leading 12-digit segment to be read as an account id, because a
    // function name may itself be twelve digits: `123456789012:2` names version 2 of
    // the function `123456789012`, not the function `2`.
    if parts.len() > 2
        && parts[1] == "function"
        && parts[0].len() == 12
        && parts[0].bytes().all(|byte| byte.is_ascii_digit())
    {
        parts.remove(0);
        was_arn = true;
    }

    // Both ARN forms, and the pattern's bare `function:` prefix, carry the resource
    // type before the name. The bare prefix is not an ARN, so it does not lift the
    // documented 64-character limit on a name given without one.
    if parts.len() > 1 && parts[0] == "function" {
        parts.remove(0);
    }

    let name = parts.first().copied().filter(|name| !name.is_empty())?;
    let qualifier = match parts.len() {
        1 => None,
        2 => Some(parts[1]).filter(|qualifier| !qualifier.is_empty()),
        _ => return None,
    };
    if parts.len() == 2 && qualifier.is_none() {
        return None;
    }

    Some(FunctionReference {
        name: name.to_string(),
        qualifier: qualifier.map(str::to_string),
        was_arn,
    })
}

/// Why a `FunctionName`/`Qualifier` pair names no record.
#[derive(Debug, Clone)]
pub enum ResolveFailure {
    /// The pair is not a resource this service can name: an unparseable
    /// `FunctionName`, or a `FunctionName` and a `Qualifier` naming two different
    /// versions. The caller has to change the request.
    Malformed(String),
    /// The pair is well formed but names no function, or no such version of one.
    NotFound(String),
}

/// Resolve a `FunctionName` (in any of its documented forms, optionally qualified) and
/// an optional `Qualifier` query parameter to one stored record and one version label.
///
/// The record is brought up to whatever the clock already made due before it is
/// handed back, so every caller reports the same lifecycle state at the same instant.
pub fn resolve_function<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    function_name: &str,
    qualifier: Option<&str>,
    region: &str,
    now: SystemTime,
) -> Result<(&'a mut FunctionRecord, String), ResolveFailure> {
    let reference = parse_function_reference(function_name).ok_or_else(|| {
        ResolveFailure::Malformed(format!(
            "1 validation error detected: Value '{function_name}' at 'functionName' failed to \
             satisfy constraint: Member must satisfy one of the documented name formats (name, \
             name:qualifier, partial ARN or function ARN)"
        ))
    })?;

    // Both the name's own qualifier and the `Qualifier` parameter name a version, so a
    // request carrying two different ones names no single resource.
    let version = match (reference.qualifier.as_deref(), qualifier) {
        (Some(from_name), Some(from_query)) if from_name != from_query => {
            return Err(ResolveFailure::Malformed(format!(
                "The function name '{function_name}' qualifies version '{from_name}', which \
                 does not match the Qualifier parameter '{from_query}'"
            )))
        }
        (Some(from_name), _) => from_name.to_string(),
        (None, Some(from_query)) => from_query.to_string(),
        (None, None) => VERSION_LATEST.to_string(),
    };

    let Some(record) = store.functions.get_mut(&reference.name) else {
        return Err(ResolveFailure::NotFound(format!(
            "Function not found: {}",
            function_arn(region, &reference.name)
        )));
    };
    record.materialize(now);
    if record.config_of(&version).is_none() {
        let arn = record.versioned_arn(&version);
        return Err(ResolveFailure::NotFound(format!("Function not found: {arn}")));
    }
    Ok((record, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_function_name_form_resolves_to_one_record() {
        let cases = [
            ("my-function", "my-function", None),
            ("my-function:2", "my-function", Some("2")),
            ("function:my-function", "my-function", None),
            ("000000000000:function:my-function", "my-function", None),
            (
                "000000000000:function:my-function:4",
                "my-function",
                Some("4"),
            ),
            // A function name may itself be twelve digits, and the documented partial
            // ARN always carries the `function:` literal, so this is a qualified name
            // rather than an account id.
            ("123456789012:2", "123456789012", Some("2")),
            (
                "arn:aws:lambda:us-east-1:000000000000:function:my-function",
                "my-function",
                None,
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:function:my-function:$LATEST",
                "my-function",
                Some("$LATEST"),
            ),
        ];
        for (input, name, qualifier) in cases {
            let parsed = parse_function_reference(input)
                .unwrap_or_else(|| panic!("case {input:?}: expected a parse, got None"));
            assert_eq!(
                parsed.name, name,
                "case {input:?}: expected name {name:?}, got {:?}",
                parsed.name
            );
            assert_eq!(
                parsed.qualifier.as_deref(),
                qualifier,
                "case {input:?}: expected qualifier {qualifier:?}, got {:?}",
                parsed.qualifier
            );
        }
    }

    #[test]
    fn a_value_that_is_no_documented_form_does_not_resolve() {
        for input in [
            "",
            ":",
            "my-function:",
            "arn:aws:s3:us-east-1:000000000000:function:my-function",
            "arn:aws:lambda:us-east-1:000000000000:layer:my-layer",
            "arn:aws:lambda:us-east-1:000000000000:my-function",
            "my-function:1:2:3",
        ] {
            assert!(
                parse_function_reference(input).is_none(),
                "case {input:?}: expected None, got {:?}",
                parse_function_reference(input)
            );
        }
    }
}
