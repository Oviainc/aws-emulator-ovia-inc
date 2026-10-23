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
//! An update to a function is owed the same way. `UpdateFunctionCode` and
//! `UpdateFunctionConfiguration` report `LastUpdateStatus` `InProgress` on the call that
//! accepted the change, and [`FunctionRecord::begin_update`] stores the instant the move
//! to `Successful` becomes due, which `materialize` applies exactly as it applies the
//! move to `Active`.
//!
//! The resource-policy slice hangs off the same record. A function's resource-based
//! policy is attached to the ARN the permission names, so [`FunctionRecord::policies`]
//! holds one [`PolicyRecord`] per qualifier a statement was added to — the
//! function-level policy under [`VERSION_LATEST`], and one per published version or
//! alias. `AddPermission` appends exactly the statement its request described and
//! `GetPolicy` reports the document those statements make up, so no policy text is ever
//! echoed back out of a request body.
//!
//! The asynchronous-invocation slice hangs off the same record. `InvokeAsync` answers
//! "202 Accepted ... the request is queued for invocation", so what it persists is the
//! queued event itself — on [`FunctionRecord::async_invocations`], one
//! [`AsyncInvocationRecord`] per accepted event, against the version it was addressed
//! to. The event is never executed here (this emulator has no in-process runtime for a
//! deployment package), so the entry's outcome settles into
//! [`AsyncInvocationOutcome::Failed`] once its delivery attempt is due, carrying the
//! reason; nothing invents a result, a log tail or a duration for a function that did
//! not run.
//!
//! The capacity-provider slice is the other resource this store holds, in
//! [`ComAmazonawsLambdaAwsgirapiserviceState::capacity_providers`]: one
//! [`CapacityProviderRecord`] per provider `CreateCapacityProvider` persisted, keyed by
//! its name so `ListCapacityProviders` pages through them deterministically. A provider
//! has a lifetime of its own too — it is created `Pending` and the move to `Active` is
//! owed by the CLOCK (see [`CapacityProviderRecord::materialize`]) — and every member
//! reported for it comes out of the record.
//!
//! `GetAccountSettings` reports on the same records at the account level:
//! "Retrieves details about your account's limits and usage in an Amazon Web Services
//! Region", so its usage is what the store holds and its unreserved concurrency is what
//! those records reserve (see
//! [`ComAmazonawsLambdaAwsgirapiserviceState::account_usage`] and
//! [`ComAmazonawsLambdaAwsgirapiserviceState::account_limit`]).
//!
//! The capacity-provider slice is the account's other top-level resource:
//! [`ComAmazonawsLambdaAwsgirapiserviceState::capacity_providers`] holds one
//! [`CapacityProviderRecord`] per provider, keyed by its bare name, so every operation
//! that reports a provider answers from the single record `CreateCapacityProvider`
//! persisted. A provider has a lifetime of its own in exactly the way a function does:
//! the model's `CapacityProviderState` enum carries `Pending` beside `Active`, so a
//! freshly created provider is `Pending` and the move to `Active` is owed by the CLOCK
//! rather than by anybody calling — the record stores the instant that move becomes due
//! and [`CapacityProviderRecord::materialize`] brings it up to whatever the clock has
//! already made due.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use sha2::Digest as _;

use crate::ops::types::{
    AccountLimit, AccountUsage, AliasRoutingConfiguration, CapacityProvider,
    CapacityProviderConfig, CapacityProviderPermissionsConfig, CapacityProviderScalingConfig,
    CapacityProviderTelemetryConfig, CapacityProviderVpcConfig, Cors, CreateAliasOutput,
    CreateFunctionOutput, DeadLetterConfig, DurableConfig, Environment, EnvironmentResponse,
    EphemeralStorage, FileSystemConfig, FunctionCodeLocation, GetLayerVersionOutput, ImageConfig,
    ImageConfigResponse, InstanceRequirements, Layer, LayerVersionContentOutput, LoggingConfig,
    PropagateTags, PutFunctionConcurrencyOutput, ResolvedS3Object, SnapStartResponse,
    TenancyConfig, TracingConfigResponse, VpcConfig, VpcConfigResponse,
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

/// The `LastUpdateStatus` a function carries while an update is still being applied,
/// from the model's `LastUpdateStatus` enum. `UpdateFunctionConfiguration`'s own
/// documentation describes the window it covers: "When you update a function, Lambda
/// provisions an instance of the function and its supporting resources. [...] During
/// this time, you can't modify the function, but you can still invoke it. The
/// LastUpdateStatus [...] fields in the response from GetFunctionConfiguration indicate
/// when the update is complete". A code update is a change to the same function, so
/// `UpdateFunctionCode` reports it too; see [`FunctionRecord::begin_update`].
pub const LAST_UPDATE_STATUS_IN_PROGRESS: &str = "InProgress";

/// How long after `CreateFunction` the move from `Pending` to `Active` becomes due.
///
/// An EMULATOR-CHOSEN span (the real provisioning time depends on the package and on
/// whether the function joins a VPC, and is not in the model). It is deliberately
/// non-zero, so a caller that acts immediately after `CreateFunction` still observes
/// the `Pending` state the model documents and the state errors that go with it.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after an accepted update (of a function's code or of its configuration) the
/// move from [`LAST_UPDATE_STATUS_IN_PROGRESS`] to [`LAST_UPDATE_STATUS_SUCCESSFUL`]
/// becomes due.
///
/// An EMULATOR-CHOSEN span, for the same reason as [`ACTIVATION_DELAY`]: the real time
/// to provision a new instance of the function is not in the model ("If your function
/// connects to a VPC, this process can take a minute"). It is deliberately non-zero, so
/// a caller that modifies the function again immediately still meets the refusal the
/// same documentation describes ("During this time, you can't modify the function").
pub const UPDATE_DELAY: Duration = Duration::from_millis(500);

/// The `SnapStart$ApplyOn` value that asks Lambda to snapshot an initialized execution
/// environment, from the model's `SnapStartApplyOn` enum (`PublishedVersions | None`).
pub const SNAP_START_APPLY_ON_PUBLISHED_VERSIONS: &str = "PublishedVersions";

/// `SnapStartResponse$OptimizationStatus` values, from the model's
/// `SnapStartOptimizationStatus` enum (`On | Off`). The member reports, "When you
/// provide a qualified Amazon Resource Name (ARN), this response element indicates
/// whether SnapStart is activated for the specified function version", so a version
/// whose configuration applies SnapStart reports `On` and everything else reports `Off`.
pub const SNAP_START_OPTIMIZATION_STATUS_ON: &str = "On";
/// See [`SNAP_START_OPTIMIZATION_STATUS_ON`].
pub const SNAP_START_OPTIMIZATION_STATUS_OFF: &str = "Off";

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

/// The `RecursiveLoop` configuration a function has before anyone sets one, which the
/// model's own `RecursiveLoop` member documentation states twice — on
/// `GetFunctionRecursionConfig`'s output and on `PutFunctionRecursionConfig`'s input:
/// "By default, Lambda sets your function's configuration to Terminate."
pub const DEFAULT_RECURSIVE_LOOP: &str = "Terminate";

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

/// The most published versions this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`], and bounding
/// the same way: every `PublishVersion` keeps a whole configuration snapshot in memory,
/// including for a package whose bytes this emulator holds none of (so one that counts
/// zero against the account's code-storage quota), and past this point the slice answers
/// the modelled `CodeStorageExceededException` rather than growing without bound. AWS
/// bounds a function's versions by that code-storage quota instead, and answers the same
/// modelled error.
pub const MAX_FUNCTION_VERSIONS: usize = 500;

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

/// The IAM policy language version a resource-based policy document carries.
/// `2012-10-17` is the current version of the language every AWS policy document is
/// written in.
pub const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The `Id` of the resource-based policy Lambda maintains for a function.
pub const POLICY_ID: &str = "default";

/// The `Effect` of every statement `AddPermission` writes: the operation "Grants a
/// principal permission to use a function", so the statement it adds is a grant and
/// never a denial.
pub const STATEMENT_EFFECT_ALLOW: &str = "Allow";

/// The largest a function's resource-based policy may get, in bytes.
///
/// AWS publishes the quota as 20 KB (Lambda quotas, "Function resource-based policy"),
/// and answers `PolicyLengthExceededException` ("The permissions policy for the resource
/// is too large") for a call that would push the policy past it. Not adjustable.
pub const MAX_POLICY_SIZE_BYTES: usize = 20_480;

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
    /// How the deployment package is stored, when the caller said: `COPY` (the default,
    /// "Uploads a copy of your deployment package to Lambda") or `REFERENCE` ("Lambda
    /// references the deployment package from the specified Amazon S3 bucket"). See
    /// [`S3_OBJECT_STORAGE_MODE_COPY`] and [`S3_OBJECT_STORAGE_MODE_REFERENCE`].
    pub s3_object_storage_mode: Option<String>,
    /// The ARN of the customer managed KMS key the .zip deployment package is encrypted
    /// with, when the caller named one; `FunctionCodeLocation$SourceKMSKeyArn` reports
    /// it back.
    pub source_kms_key_arn: Option<String>,
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

/// The members of a request that name a deployment package.
///
/// `CreateFunction` carries them on its `Code` member and `UpdateFunctionCode` carries
/// them on the request itself, but they name the same package in the same forms, so both
/// resolve them through [`resolve_package_code`] rather than each deciding for itself
/// what a package may be.
#[derive(Debug, Default, Clone, Copy)]
pub struct PackageMembers<'a> {
    /// `ZipFile` — the deployment package bytes, uploaded inline.
    pub zip_file: Option<&'a [u8]>,
    /// `S3Bucket` — the bucket holding the deployment package.
    pub s3_bucket: Option<&'a str>,
    /// `S3Key` — the key of the deployment package.
    pub s3_key: Option<&'a str>,
    /// `S3ObjectVersion` — "For versioned objects, the version of the deployment package
    /// object to use."
    pub s3_object_version: Option<&'a str>,
    /// `S3ObjectStorageMode` — `COPY` or `REFERENCE`.
    pub s3_object_storage_mode: Option<&'a str>,
    /// `ImageUri` — the container image in the Amazon ECR registry.
    pub image_uri: Option<&'a str>,
    /// `SourceKMSKeyArn` — the customer managed key the .zip package is encrypted with.
    pub source_kms_key_arn: Option<&'a str>,
}

impl PackageMembers<'_> {
    /// Whether any member that only an S3-sourced package has was named.
    fn names_s3_object(&self) -> bool {
        self.s3_bucket.is_some()
            || self.s3_key.is_some()
            || self.s3_object_version.is_some()
            || self.s3_object_storage_mode.is_some()
    }
}

/// The [`CodeRecord`] `package_type` admits for `members`, or the message of the refusal.
///
/// The model states the rule on both operations: "If the function's package type is
/// Image, then you must specify the code package in ImageUri as the URI of a container
/// image in the Amazon ECR registry. If the function's package type is Zip, then you must
/// specify the deployment package as a .zip file archive. Enter the Amazon S3 bucket and
/// key of the code .zip file location. You can also provide the function code inline
/// using the ZipFile field."
pub fn resolve_package_code(
    package_type: &str,
    members: PackageMembers<'_>,
) -> Result<CodeRecord, String> {
    if package_type == PACKAGE_TYPE_IMAGE {
        let Some(image_uri) = members.image_uri else {
            return Err(
                "ImageUri is required for a function whose PackageType is Image".to_string(),
            );
        };
        if members.zip_file.is_some() || members.names_s3_object() {
            return Err(
                "Only ImageUri may name the code of a function whose PackageType is Image"
                    .to_string(),
            );
        }
        return Ok(CodeRecord {
            image_uri: Some(image_uri.to_string()),
            ..CodeRecord::default()
        });
    }

    if members.image_uri.is_some() {
        return Err(
            "ImageUri may only name the code of a function whose PackageType is Image"
                .to_string(),
        );
    }
    match (members.zip_file, members.s3_bucket, members.s3_key) {
        (Some(zip_file), None, None) if !members.names_s3_object() => {
            // `AccountLimit$CodeSizeZipped` is "The maximum size of a deployment package
            // when it's uploaded directly to Lambda", which is the ceiling on these
            // bytes — and, since they are held in memory here, on the store too.
            if i64::try_from(zip_file.len()).unwrap_or(i64::MAX) > CODE_SIZE_ZIPPED_LIMIT {
                return Err(format!(
                    "ZipFile must be smaller than {CODE_SIZE_ZIPPED_LIMIT} bytes for a direct \
                     upload; use S3Bucket and S3Key for a larger deployment package"
                ));
            }
            Ok(CodeRecord {
                zip_bytes: Some(zip_file.to_vec()),
                source_kms_key_arn: members.source_kms_key_arn.map(str::to_string),
                ..CodeRecord::default()
            })
        }
        (None, Some(bucket), Some(key)) => Ok(CodeRecord {
            s3_bucket: Some(bucket.to_string()),
            s3_key: Some(key.to_string()),
            s3_object_version: members.s3_object_version.map(str::to_string),
            s3_object_storage_mode: members.s3_object_storage_mode.map(str::to_string),
            source_kms_key_arn: members.source_kms_key_arn.map(str::to_string),
            ..CodeRecord::default()
        }),
        // Either neither form was named, or both were: the inline bytes and the S3
        // object each name a whole package, so a request carrying members of both names
        // no single one.
        _ => Err(
            "The code must name either ZipFile or both S3Bucket and S3Key for a function \
             whose PackageType is Zip"
                .to_string(),
        ),
    }
}

/// One layer a function's execution environment holds.
///
/// `FunctionConfiguration$Layers` reports "The function's layers" as the layer version's
/// ARN and "The size of the layer archive in bytes", so the resolution of a `Layers`
/// entry against the published layer version is what is stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionLayer {
    /// The ARN of the layer version, including its version number.
    pub arn: String,
    /// The size of that version's archive, in bytes.
    pub code_size: i64,
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
    /// The layer versions this configuration holds, in the order the caller named
    /// them, each resolved against the layer the store holds.
    pub layers: Vec<FunctionLayer>,
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
    /// The VPC the function is attached to, when the caller named one.
    pub vpc_config: Option<VpcConfig>,
    /// The dead-letter queue for failed asynchronous invocations, when the caller named
    /// one.
    pub dead_letter_config: Option<DeadLetterConfig>,
    /// The file systems the function mounts, in the order the caller named them (the
    /// model's `FileSystemConfigList` holds at most one).
    pub file_system_configs: Vec<FileSystemConfig>,
    /// The container image overrides, when the caller named any.
    pub image_config: Option<ImageConfig>,
    /// `SnapStart$ApplyOn`, when the caller set it.
    pub snap_start_apply_on: Option<String>,
    /// The CloudWatch Logs configuration, when the caller named one.
    pub logging_config: Option<LoggingConfig>,
    /// The capacity provider that manages this function's compute, when the caller
    /// named one.
    pub capacity_provider_config: Option<CapacityProviderConfig>,
    /// The durable-execution settings, when the caller named them.
    pub durable_config: Option<DurableConfig>,
    /// The tenant isolation settings, when the caller named them. `CreateFunction` is
    /// the only operation that sets this one: "Tenant isolation configuration cannot be
    /// modified after function creation", and `UpdateFunctionConfiguration` models no
    /// member for it.
    pub tenancy_config: Option<TenancyConfig>,
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

/// `value` as a JSON string literal, escaped by the JSON serializer rather than by
/// hand.
fn json_string(value: &str) -> String {
    ::serde_json::Value::String(value.to_string()).to_string()
}

/// A JSON object whose members keep the order they are given in.
///
/// A `serde_json::Value` map would sort them by key, and a policy document reads in a
/// fixed order (`Sid`, `Effect`, `Principal`, `Action`, `Resource`, `Condition`), so the
/// members are rendered in order while every VALUE is still serialized by `serde_json`.
fn json_object(members: &[(&str, String)]) -> String {
    let rendered: Vec<String> = members
        .iter()
        .map(|(name, value)| format!("{}:{value}", json_string(name)))
        .collect();
    format!("{{{}}}", rendered.join(","))
}

/// The `Principal` of a policy statement for the principal `AddPermission` was given.
///
/// The model documents the forms the member takes — "The Amazon Web Services service,
/// Amazon Web Services account, IAM user, or IAM role that invokes the function", where
/// "For Amazon Web Services services, the principal is a domain-style identifier that
/// the service defines, such as s3.amazonaws.com" — and the policy language spells each
/// of them differently:
///
/// * an account id is that account's root principal, written as its ARN;
/// * an IAM user or role is already an ARN, and is used as given;
/// * anything else is the domain-style identifier of a service principal;
/// * `*` is every principal, which the language writes as the bare string.
#[must_use]
pub fn principal_json(principal: &str) -> String {
    if principal == "*" {
        return json_string(principal);
    }
    let is_account_id = principal.len() == 12 && principal.bytes().all(|byte| byte.is_ascii_digit());
    if is_account_id {
        return json_object(&[(
            "AWS",
            json_string(&format!("arn:aws:iam::{principal}:root")),
        )]);
    }
    if principal.starts_with("arn:") {
        return json_object(&[("AWS", json_string(principal))]);
    }
    json_object(&[("Service", json_string(principal))])
}

/// One statement of a function's resource-based policy: exactly the grant one
/// `AddPermission` call described, plus the ARN it was granted on.
#[derive(Debug, Clone)]
pub struct PolicyStatement {
    /// The `StatementId`, which "differentiates the statement from others in the same
    /// policy" and is the statement's `Sid`.
    pub sid: String,
    /// The action the principal may use on the function.
    pub action: String,
    /// The principal, as supplied; [`principal_json`] renders it.
    pub principal: String,
    /// The ARN the permission is granted on: the function's unqualified ARN for a
    /// function-level statement, and the qualified one for a version or alias.
    pub resource: String,
    /// `SourceArn`, when the caller limited the grant to one invoking resource.
    pub source_arn: Option<String>,
    /// `SourceAccount`, when the caller limited the grant to one owning account.
    pub source_account: Option<String>,
    /// `PrincipalOrgID`, when the caller limited the grant to one organization.
    pub principal_org_id: Option<String>,
    /// `EventSourceToken`, when the caller required one.
    pub event_source_token: Option<String>,
    /// `FunctionUrlAuthType`, when the grant is about a function URL.
    pub function_url_auth_type: Option<String>,
    /// `InvokedViaFunctionUrl`, when the caller restricted the grant to invocations
    /// that come through a function URL.
    pub invoked_via_function_url: Option<bool>,
}

impl PolicyStatement {
    /// The `Condition` block of this statement, or `None` when the grant carries no
    /// restriction at all.
    ///
    /// Each optional member of the request restricts the grant, and each is written as
    /// the IAM condition key that expresses it. `SourceArn` is compared as an ARN — the
    /// model's note "Lambda configures the comparison using the `StringLike` operator"
    /// is why wildcards in the ARN match, which is what the ARN-typed `ArnLike` operator
    /// does — the remaining string-valued keys are compared for equality, and the
    /// boolean one with `Bool`.
    fn condition_json(&self) -> Option<String> {
        let mut operators: Vec<(&str, String)> = Vec::new();
        if let Some(source_arn) = &self.source_arn {
            operators.push((
                "ArnLike",
                json_object(&[("AWS:SourceArn", json_string(source_arn))]),
            ));
        }

        let mut equals: Vec<(&str, String)> = Vec::new();
        if let Some(source_account) = &self.source_account {
            equals.push(("AWS:SourceAccount", json_string(source_account)));
        }
        if let Some(org_id) = &self.principal_org_id {
            equals.push(("aws:PrincipalOrgID", json_string(org_id)));
        }
        if let Some(token) = &self.event_source_token {
            equals.push(("lambda:EventSourceToken", json_string(token)));
        }
        if let Some(auth_type) = &self.function_url_auth_type {
            equals.push(("lambda:FunctionUrlAuthType", json_string(auth_type)));
        }
        if !equals.is_empty() {
            operators.push(("StringEquals", json_object(&equals)));
        }

        if let Some(via_function_url) = self.invoked_via_function_url {
            operators.push((
                "Bool",
                json_object(&[(
                    "lambda:InvokedViaFunctionUrl",
                    json_string(&via_function_url.to_string()),
                )]),
            ));
        }

        (!operators.is_empty()).then(|| json_object(&operators))
    }

    /// This statement as the JSON `AddPermission` answers with and `GetPolicy` embeds:
    /// "The permission statement that's added to the function policy".
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut members = vec![
            ("Sid", json_string(&self.sid)),
            ("Effect", json_string(STATEMENT_EFFECT_ALLOW)),
            ("Principal", principal_json(&self.principal)),
            ("Action", json_string(&self.action)),
            ("Resource", json_string(&self.resource)),
        ];
        if let Some(condition) = self.condition_json() {
            members.push(("Condition", condition));
        }
        json_object(&members)
    }
}

/// The resource-based policy of one function, version or alias: the statements
/// `AddPermission` has added to it, and the revision they are at.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    /// The statements, in the order they were added, so the document reports them
    /// deterministically.
    pub statements: Vec<PolicyStatement>,
    /// The revision this policy is at. `GetPolicy` reports it, and `AddPermission`'s
    /// `RevisionId` precondition is checked against it ("For AddPermission and
    /// RemovePermission API operations: Call GetPolicy to retrieve the latest RevisionId
    /// for your resource"); a fresh one is minted on every change.
    pub revision_id: String,
}

impl Default for PolicyRecord {
    fn default() -> Self {
        Self {
            statements: Vec::new(),
            revision_id: mint_revision_id(),
        }
    }
}

impl PolicyRecord {
    /// Whether this policy already holds a statement with `sid`.
    #[must_use]
    pub fn holds(&self, sid: &str) -> bool {
        self.statements.iter().any(|statement| statement.sid == sid)
    }

    /// The whole policy document, which is what `GetPolicy`'s `Policy` member carries.
    #[must_use]
    pub fn to_json(&self) -> String {
        let statements: Vec<String> = self
            .statements
            .iter()
            .map(PolicyStatement::to_json)
            .collect();
        json_object(&[
            ("Version", json_string(POLICY_LANGUAGE_VERSION)),
            ("Id", json_string(POLICY_ID)),
            ("Statement", format!("[{}]", statements.join(","))),
        ])
    }
}

/// The `InvokeMode` a function URL gets when the caller names none, which the model's
/// own member documentation states: "BUFFERED – This is the default option. Lambda
/// invokes your function using the Invoke API operation."
pub const DEFAULT_INVOKE_MODE: &str = "BUFFERED";

/// The host suffix every Lambda function URL carries. AWS documents the form in
/// `CreateFunctionUrlConfig`'s guide ("Function URL endpoints have the following
/// format: `https://<url-id>.lambda-url.<region>.on.aws`"), and it is what keeps the
/// minted URL inside the model's 40..100 `FunctionUrl` length constraint.
pub const FUNCTION_URL_HOST_SUFFIX: &str = "lambda-url";

/// The URL of the function URL whose id is `url_id`, in the region it was created in.
///
/// The trailing `/` is part of the endpoint AWS vends, so the value a caller gets back
/// is the URL they can address as-is.
#[must_use]
pub fn function_url(region: &str, url_id: &str) -> String {
    format!("https://{url_id}.{FUNCTION_URL_HOST_SUFFIX}.{region}.on.aws/")
}

/// A fresh function-URL id: the opaque, per-URL subdomain label AWS mints for a
/// function URL. AWS's are 32 lowercase alphanumeric characters, which the 32 hex
/// digits of a random UUID satisfy. Minted once, at creation, and stored — so every
/// later read of the same function URL reports the same URL.
#[must_use]
pub fn mint_function_url_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// The model's `FunctionUrlQualifier` pattern, which is the one every function-URL
/// operation puts on the qualifier it accepts.
pub const FUNCTION_URL_QUALIFIER_PATTERN: &str = r"^((?!^\d+$)^[0-9a-zA-Z-_]+$)$";

/// Whether `qualifier` is one a function URL may be addressed by.
///
/// A function URL belongs to a function's unpublished version — addressed by naming no
/// qualifier at all — or to one of its aliases, and the model says exactly that through
/// [`FUNCTION_URL_QUALIFIER_PATTERN`]: an all-digit value (a published version number)
/// does not match it, and neither does [`VERSION_LATEST`], whose `$` the pattern's
/// character class does not admit.
#[must_use]
pub fn is_function_url_qualifier(qualifier: &str) -> bool {
    !qualifier.is_empty()
        && qualifier != VERSION_LATEST
        && !qualifier.bytes().all(|byte| byte.is_ascii_digit())
}

/// The refusal for a qualifier a function URL cannot be addressed by: a validation
/// message built from the request, naming the member and the constraint it failed.
#[must_use]
pub fn function_url_qualifier_message(qualifier: &str) -> String {
    format!(
        "1 validation error detected: Value '{qualifier}' at 'qualifier' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: \
         {FUNCTION_URL_QUALIFIER_PATTERN}"
    )
}

/// The qualifier a `FunctionName`/`Qualifier` pair NAMES, as opposed to the
/// [`VERSION_LATEST`] an unqualified request resolves to. It may come from either, and
/// [`resolve_function`] refuses a pair that names two different ones.
#[must_use]
pub fn named_qualifier(function_name: &str, qualifier: Option<&str>) -> Option<String> {
    qualifier
        .map(str::to_string)
        .or_else(|| parse_function_reference(function_name).and_then(|reference| reference.qualifier))
        .filter(|named| !named.is_empty())
}

/// One Lambda function URL: "a dedicated HTTP(S) endpoint that you can use to invoke
/// your function", with the configuration the caller gave it.
///
/// Held per qualifier on [`FunctionRecord::url_configs`], because a function URL is
/// attached to the function's unpublished version or to one of its aliases, and each of
/// those has at most one.
#[derive(Debug, Clone)]
pub struct FunctionUrlConfigRecord {
    /// The subdomain label the URL is addressed by, minted once by
    /// [`mint_function_url_id`].
    pub url_id: String,
    /// The `FunctionUrlAuthType` the URL enforces (`AWS_IAM` or `NONE`), as supplied.
    pub auth_type: String,
    /// The CORS settings, exactly as configured; absent when the caller configured
    /// none, because nothing here invents settings the caller never asked for.
    pub cors: Option<Cors>,
    /// The `InvokeMode`, defaulted to [`DEFAULT_INVOKE_MODE`] when the caller named
    /// none.
    pub invoke_mode: String,
    /// ISO-8601 instant the URL was created, "in ISO-8601 format
    /// (YYYY-MM-DDThh:mm:ss.sTZD)" as the model's `CreationTime` member documents.
    pub creation_time: String,
    /// ISO-8601 instant of the last change to this configuration; equal to
    /// [`Self::creation_time`] until one is made.
    pub last_modified_time: String,
}

impl FunctionUrlConfigRecord {
    /// The HTTP(S) endpoint of this function URL in `region`; see [`function_url`].
    #[must_use]
    pub fn url(&self, region: &str) -> String {
        function_url(region, &self.url_id)
    }
}

/// What has become of one queued asynchronous invocation.
///
/// A queued event is NOT a run: the queue is the only thing an asynchronous
/// invocation's accepting call ("A success response (202 Accepted) indicates that the
/// request is queued for invocation") promises, and this emulator has no in-process
/// runtime that can run a deployment package. So an entry starts [`Self::Queued`] and,
/// once its first delivery attempt is due, settles into [`Self::Failed`] carrying the
/// reason it never ran. Nothing here ever reports a result, a log tail or a duration
/// for a function that did not execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncInvocationOutcome {
    /// Accepted and waiting for its delivery attempt, which is due at
    /// [`AsyncInvocationRecord::attempt_due`].
    Queued,
    /// The delivery attempt came due and the event was never executed; the string is
    /// why.
    Failed(String),
}

/// One asynchronous invocation this emulator accepted for a function.
///
/// Held per function on [`FunctionRecord::async_invocations`], because Lambda queues an
/// asynchronous event against the function version it was addressed to.
#[derive(Debug, Clone)]
pub struct AsyncInvocationRecord {
    /// The version label the event was addressed to (`$LATEST` or a published version).
    pub version: String,
    /// The event payload, exactly as the caller supplied it.
    pub payload: Vec<u8>,
    /// ISO-8601 instant the event was accepted for invocation.
    pub received: String,
    /// When the first delivery attempt becomes due; see [`ASYNC_DELIVERY_DELAY`].
    pub attempt_due: SystemTime,
    /// What has become of the event.
    pub outcome: AsyncInvocationOutcome,
}

/// How long after an asynchronous invocation is accepted its first delivery attempt
/// becomes due.
///
/// An EMULATOR-CHOSEN span, like [`ACTIVATION_DELAY`]: Lambda publishes no latency for
/// the asynchronous event queue. It is deliberately non-zero, so a caller that looks
/// straight after the accepting call still sees the event [`AsyncInvocationOutcome::Queued`].
pub const ASYNC_DELIVERY_DELAY: Duration = Duration::from_millis(250);

/// Why a queued asynchronous invocation never ran, recorded on the entry when its
/// delivery attempt comes due.
pub const ASYNC_NOT_EXECUTED_REASON: &str =
    "the event was accepted and queued, but no in-process runtime in this emulator can \
     run the function's deployment package, so it was never executed";

/// The most queued asynchronous invocations this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`]: the queue is in
/// memory and every accepted event adds its payload to it, so past this point the
/// oldest entry is dropped rather than the store growing without bound. Dropping the
/// oldest does not change what any call answers — the accepting call reports only that
/// the event was queued, and no operation reads the queue back out.
pub const MAX_QUEUED_ASYNC_INVOCATIONS: usize = 100;

/// The most aliases this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling, in the sense [`MAX_FUNCTIONS`] is: the store is in memory
/// and every `CreateAlias` adds to it. AWS bounds the aliases of a function too and
/// answers the modelled `AliasLimitExceededException` for the create that would pass the
/// bound ("Lambda couldn't create the alias because your Amazon Web Services account has
/// exceeded the maximum number of aliases allowed per Lambda function"), but neither the
/// model nor the published quotas carry the number, so this slice picks one and answers
/// that same modelled error past it.
pub const MAX_FUNCTION_ALIASES: usize = 100;

/// The most versions one alias may route to BESIDE the version it invokes.
///
/// `CreateAlias`'s own documentation describes traffic shifting as a split between two
/// versions — "You can also map an alias to split invocation requests between two
/// versions. Use the RoutingConfig parameter to specify a second version and the
/// percentage of invocation requests that it receives" — so `AdditionalVersionWeights`
/// names at most one.
pub const MAX_ALIAS_ADDITIONAL_VERSIONS: usize = 1;

/// The model's `Alias` pattern, which every operation naming an alias puts on it.
pub const ALIAS_NAME_PATTERN: &str = r"^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$";

/// Whether `name` is a name an alias may carry.
///
/// An alias shares the qualifier namespace with a function's published version numbers,
/// and [`ALIAS_NAME_PATTERN`] is what keeps the two apart: its negative lookahead
/// refuses an all-digit name, so no alias can ever be confused with a version.
#[must_use]
pub fn is_alias_name(name: &str) -> bool {
    !name.is_empty()
        && !name.bytes().all(|byte| byte.is_ascii_digit())
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
        })
}

/// The refusal for a name no alias may carry: a validation message built from the
/// request, naming the member and the constraint it failed.
#[must_use]
pub fn alias_name_message(name: &str) -> String {
    format!(
        "1 validation error detected: Value '{name}' at 'name' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {ALIAS_NAME_PATTERN}"
    )
}

/// One alias of a function: "a function identifier that you can update to invoke a
/// different version", as `CreateAlias` documents it.
///
/// Held per name on [`FunctionRecord::aliases`], because an alias belongs to the
/// function rather than to one of its versions, and a function has at most one alias of
/// a given name.
#[derive(Debug, Clone)]
pub struct AliasRecord {
    /// The name the alias is addressed by, which is also the qualifier of its ARN.
    pub name: String,
    /// "The function version that the alias invokes": [`VERSION_LATEST`] or a published
    /// version number.
    pub function_version: String,
    /// The description, exactly as supplied; absent when the caller gave none.
    pub description: Option<String>,
    /// The second version this alias routes part of its invocations to, and the share it
    /// receives, keyed by version number so the configuration reports in a stable order.
    /// Empty when the alias does no traffic shifting.
    pub additional_version_weights: BTreeMap<String, f64>,
    /// "A unique identifier that changes when you update the alias", minted on every
    /// write to it.
    pub revision_id: String,
}

impl AliasRecord {
    /// The `AliasRoutingConfiguration` the wire reports for this alias, present only
    /// once the alias routes to a second version: the model gives the member no
    /// default, so an alias that shifts no traffic carries no routing configuration at
    /// all.
    #[must_use]
    pub fn routing_config(&self) -> Option<AliasRoutingConfiguration> {
        (!self.additional_version_weights.is_empty()).then(|| AliasRoutingConfiguration {
            additional_version_weights: Some(
                self.additional_version_weights
                    .iter()
                    .map(|(version, weight)| (version.clone(), *weight))
                    .collect(),
            ),
        })
    }
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
    /// When the move from [`LAST_UPDATE_STATUS_IN_PROGRESS`] to
    /// [`LAST_UPDATE_STATUS_SUCCESSFUL`] becomes due, while an update is in flight. Set
    /// by [`FunctionRecord::begin_update`].
    pub last_update_due: Option<SystemTime>,
    /// When the move to [`STATE_ACTIVE`] becomes due.
    pub active_due: SystemTime,
    /// Function-level tags, as supplied.
    pub tags: BTreeMap<String, String>,
    /// The reserved concurrency, when `PutFunctionConcurrency` has set one.
    pub reserved_concurrent_executions: Option<i32>,
    /// The function's recursive loop detection configuration, one of the model's
    /// `RecursiveLoop` values. It is a function-level setting rather than a
    /// version-specific one — `GetFunctionRecursionConfig` and
    /// `PutFunctionRecursionConfig` both take an `UnqualifiedFunctionName`, whose
    /// pattern admits no qualifier — so it lives on the record beside the function's
    /// other function-level settings and starts at [`DEFAULT_RECURSIVE_LOOP`].
    pub recursive_loop: String,
    /// The highest version published so far; 0 when none has been.
    pub latest_version: u64,
    /// The published versions, ordered by version number.
    pub versions: BTreeMap<u64, FunctionVersionRecord>,
    /// The resource-based policy of each qualifier that has one, keyed by the version
    /// label the permission was granted on: the function-level policy under
    /// [`VERSION_LATEST`], and one per published version or alias. A policy is attached
    /// to the ARN the permission names, so two qualifiers never share one.
    pub policies: BTreeMap<String, PolicyRecord>,
    /// The function URL of each qualifier that has one, keyed by the version label the
    /// URL was created on: the unpublished version under [`VERSION_LATEST`], and one
    /// per alias. A function URL is attached to the qualifier it was created on, and
    /// each qualifier has at most one, so a second create on the same qualifier
    /// conflicts rather than replacing it.
    pub url_configs: BTreeMap<String, FunctionUrlConfigRecord>,
    /// The aliases of this function, keyed by alias name so `ListAliases` pages through
    /// them in a stable order. An alias belongs to the function, not to one of its
    /// versions, and names the version it invokes.
    pub aliases: BTreeMap<String, AliasRecord>,
    /// The asynchronous invocations accepted for this function, oldest first, bounded
    /// by [`MAX_QUEUED_ASYNC_INVOCATIONS`].
    pub async_invocations: VecDeque<AsyncInvocationRecord>,
}

impl FunctionRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transitions it applies are the ones the clock owed at `now`: a function
    /// that has finished provisioning becomes `Active`, and an update that has finished
    /// being applied becomes `Successful`. Both would have been owed at the same instant
    /// even if nobody had called, so applying them on a read reports the truth rather
    /// than inventing a change.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.state == STATE_PENDING && now >= self.active_due {
            self.state = STATE_ACTIVE.to_string();
            self.state_reason = None;
            self.state_reason_code = None;
            // "This is first set to Successful after function creation completes" — an
            // update accepted while the function was still being provisioned is settled
            // by its own due instant below, so creation completing does not settle it.
            if self.last_update_status.is_none() {
                self.last_update_status = Some(LAST_UPDATE_STATUS_SUCCESSFUL.to_string());
            }
        }
        if self.last_update_due.is_some_and(|due| now >= due) {
            self.last_update_status = Some(LAST_UPDATE_STATUS_SUCCESSFUL.to_string());
            self.last_update_due = None;
        }
        // The same rule for the asynchronous event queue: a queued event whose delivery
        // attempt the clock has already made due has had that attempt, and this emulator
        // runs no deployment package, so the attempt ended without the function ever
        // executing. The outcome would have been owed at the same instant even if nobody
        // had called.
        for invocation in &mut self.async_invocations {
            if invocation.outcome == AsyncInvocationOutcome::Queued && now >= invocation.attempt_due
            {
                invocation.outcome =
                    AsyncInvocationOutcome::Failed(ASYNC_NOT_EXECUTED_REASON.to_string());
            }
        }
    }

    /// Record that an update to this function's code or configuration was accepted at
    /// `now`: the model's `LastUpdateStatus` is [`LAST_UPDATE_STATUS_IN_PROGRESS`] until
    /// the clock makes the move to [`LAST_UPDATE_STATUS_SUCCESSFUL`] due,
    /// [`UPDATE_DELAY`] later.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_status = Some(LAST_UPDATE_STATUS_IN_PROGRESS.to_string());
        self.last_update_due = Some(now + UPDATE_DELAY);
    }

    /// Freeze `config` as this function's next published version, answering the number
    /// it got.
    ///
    /// Versions are numbered from 1 upwards and a number is never reused, so the next
    /// one follows the highest ever published rather than the count held. `None` when no
    /// further number can be assigned.
    pub fn publish_next_version(
        &mut self,
        config: FunctionConfig,
        now: SystemTime,
    ) -> Option<u64> {
        let version = self.latest_version.checked_add(1)?;
        self.versions.insert(
            version,
            FunctionVersionRecord {
                version,
                config,
                created: iso8601(now),
            },
        );
        self.latest_version = version;
        Some(version)
    }

    /// Whether an update of this function is still being applied — the window
    /// `UpdateFunctionConfiguration` documents as "During this time, you can't modify
    /// the function, but you can still invoke it".
    ///
    /// Read it only after [`FunctionRecord::materialize`], so an update the clock has
    /// already settled is not reported as still in flight.
    #[must_use]
    pub fn update_in_progress(&self) -> bool {
        self.last_update_due.is_some()
    }

    /// Queue one asynchronous invocation of `version` carrying `payload`, accepted at
    /// `now`.
    ///
    /// The queue is bounded by [`MAX_QUEUED_ASYNC_INVOCATIONS`]: once it is full the
    /// oldest entry makes room for the newest, so accepting an event never fails for
    /// want of memory.
    pub fn queue_async_invocation(&mut self, version: &str, payload: Vec<u8>, now: SystemTime) {
        self.async_invocations.push_back(AsyncInvocationRecord {
            version: version.to_string(),
            payload,
            received: iso8601(now),
            attempt_due: now + ASYNC_DELIVERY_DELAY,
            outcome: AsyncInvocationOutcome::Queued,
        });
        while self.async_invocations.len() > MAX_QUEUED_ASYNC_INVOCATIONS {
            self.async_invocations.pop_front();
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

    /// The version label `qualifier` names, or `None` when this function has no such
    /// version or alias.
    ///
    /// [`VERSION_LATEST`] and a published version number name themselves; every other
    /// qualifier names an alias, and an alias names the version it invokes — "use
    /// aliases to provide clients with a function identifier that you can update to
    /// invoke a different version" (`CreateAlias`).
    #[must_use]
    pub fn resolved_version(&self, qualifier: &str) -> Option<String> {
        if qualifier == VERSION_LATEST {
            return Some(VERSION_LATEST.to_string());
        }
        if let Ok(number) = qualifier.parse::<u64>() {
            return self
                .versions
                .contains_key(&number)
                .then(|| number.to_string());
        }
        self.aliases
            .get(qualifier)
            .map(|alias| alias.function_version.clone())
    }

    /// The configuration of `version`, or `None` when this function has no such
    /// version. An alias resolves to the configuration of the version it invokes; see
    /// [`Self::resolved_version`].
    #[must_use]
    pub fn config_of(&self, version: &str) -> Option<&FunctionConfig> {
        let resolved = self.resolved_version(version)?;
        if resolved == VERSION_LATEST {
            return Some(&self.config);
        }
        resolved
            .parse::<u64>()
            .ok()
            .and_then(|number| self.versions.get(&number))
            .map(|published| &published.config)
    }

    /// The `AliasConfiguration` the wire reports for one of this function's aliases,
    /// built entirely out of what is stored.
    ///
    /// The model declares ONE `AliasConfiguration`, generated here as
    /// [`CreateAliasOutput`]; an operation whose output is generated as its own struct
    /// over that shape carries these members across one for one rather than rebuilding
    /// them from state a second time.
    #[must_use]
    pub fn alias_configuration(&self, alias: &AliasRecord) -> CreateAliasOutput {
        CreateAliasOutput {
            alias_arn: Some(self.versioned_arn(&alias.name)),
            name: Some(alias.name.clone()),
            function_version: Some(alias.function_version.clone()),
            description: alias.description.clone(),
            routing_config: alias.routing_config(),
            revision_id: Some(alias.revision_id.clone()),
        }
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
            // The `Version` member's own pattern (`^(\$LATEST|[0-9]+)$`) admits no alias
            // name, so a request addressed by an alias reports the version that alias
            // invokes, while the ARN above keeps the qualifier the caller named.
            version: Some(
                self.resolved_version(version)
                    .unwrap_or_else(|| version.to_string()),
            ),
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
            // A function the caller named no layer for carries no `Layers` member at
            // all: the model gives it no default, so an empty list is not a value this
            // configuration holds.
            layers: (!config.layers.is_empty()).then(|| {
                config
                    .layers
                    .iter()
                    .map(|layer| Layer {
                        arn: Some(layer.arn.clone()),
                        code_size: layer.code_size,
                        ..Layer::default()
                    })
                    .collect()
            }),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            state_reason_code: self.state_reason_code.clone(),
            last_update_status: if version == VERSION_LATEST {
                self.last_update_status.clone()
            } else {
                // `LastUpdateStatus` is "The status of the last update that was
                // performed on the function", and the update
                // `UpdateFunctionConfiguration` describes is applied to the UNPUBLISHED
                // version only ("You can't modify the configuration of a published
                // version, only the unpublished version"). A published version froze its
                // configuration before any such update began, so it is not the thing
                // being updated and reports the settled status rather than the in-flight
                // one.
                self.last_update_status.as_ref().map(|status| {
                    if status == LAST_UPDATE_STATUS_IN_PROGRESS {
                        LAST_UPDATE_STATUS_SUCCESSFUL.to_string()
                    } else {
                        status.clone()
                    }
                })
            },
            package_type: Some(config.package_type.clone()),
            architectures: Some(config.architectures.clone()),
            ephemeral_storage: Some(EphemeralStorage {
                size: config.ephemeral_storage_mb,
            }),
            // `VpcConfig` goes in and `VpcConfigResponse` comes out: the two differ only
            // by the `VpcId` of the VPC the subnets belong to, which lives in EC2. This
            // emulator resolves no EC2 state, so it reports no id it did not look up.
            vpc_config: config.vpc_config.as_ref().map(|vpc| VpcConfigResponse {
                subnet_ids: vpc.subnet_ids.clone(),
                security_group_ids: vpc.security_group_ids.clone(),
                vpc_id: None,
                ipv6_allowed_for_dual_stack: vpc.ipv6_allowed_for_dual_stack,
            }),
            dead_letter_config: config.dead_letter_config.clone(),
            // A function the caller mounted no file system on carries no
            // `FileSystemConfigs` member at all, for the same reason as `Layers` above.
            file_system_configs: (!config.file_system_configs.is_empty())
                .then(|| config.file_system_configs.clone()),
            // `ImageConfigResponse` wraps the stored overrides together with the error
            // of resolving them; nothing here resolves an image, so there is no error
            // to report.
            image_config_response: config.image_config.clone().map(|image_config| {
                ImageConfigResponse {
                    image_config: Some(image_config),
                    error: None,
                }
            }),
            snap_start: config
                .snap_start_apply_on
                .clone()
                .map(|apply_on| SnapStartResponse {
                    optimization_status: Some(
                        if version != VERSION_LATEST
                            && apply_on == SNAP_START_APPLY_ON_PUBLISHED_VERSIONS
                        {
                            SNAP_START_OPTIMIZATION_STATUS_ON
                        } else {
                            SNAP_START_OPTIMIZATION_STATUS_OFF
                        }
                        .to_string(),
                    ),
                    apply_on: Some(apply_on),
                }),
            logging_config: config.logging_config.clone(),
            capacity_provider_config: config.capacity_provider_config.clone(),
            durable_config: config.durable_config.clone(),
            tenancy_config: config.tenancy_config.clone(),
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
            // "The resolved Amazon S3 object that contains the deployment package" — in
            // `REFERENCE` mode that object is the caller's, because "Lambda references
            // the deployment package from the specified Amazon S3 bucket". In the default
            // `COPY` mode Lambda "uploads a copy of your deployment package" and serves it
            // from storage of its own, so the caller's object is no longer where the
            // package is served from and none is reported — the same rule
            // [`LayerVersionRecord::content`] applies to a layer archive.
            resolved_s3_object: self
                .code
                .s3_bucket
                .clone()
                .filter(|_| {
                    self.code.s3_object_storage_mode.as_deref()
                        == Some(S3_OBJECT_STORAGE_MODE_REFERENCE)
                })
                .map(|bucket| ResolvedS3Object {
                    s3_bucket: Some(bucket),
                    s3_key: self.code.s3_key.clone(),
                    s3_object_version: self.code.s3_object_version.clone(),
                }),
            source_kms_key_arn: self.code.source_kms_key_arn.clone(),
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

/// The `S3ObjectStorageMode` a layer archive is stored with when the caller names
/// none, from the member's own documentation: "`COPY` (default) – Uploads a copy of
/// your layer archive to Lambda."
pub const S3_OBJECT_STORAGE_MODE_COPY: &str = "COPY";

/// The `S3ObjectStorageMode` that leaves the archive where the caller put it:
/// "`REFERENCE` – Lambda references the layer archive from the specified Amazon S3
/// bucket."
pub const S3_OBJECT_STORAGE_MODE_REFERENCE: &str = "REFERENCE";

/// The most layers this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`]: the store is in
/// memory and every `PublishLayerVersion` adds to it, so past this point the slice
/// answers the modelled `CodeStorageExceededException` rather than growing without
/// bound. AWS bounds layers by the account's code-storage quota instead, and answers the
/// same modelled error.
pub const MAX_LAYERS: usize = 1_000;

/// The most versions this emulator keeps for one layer, bounding the store exactly as
/// [`MAX_FUNCTION_VERSIONS`] bounds a function's versions.
pub const MAX_LAYER_VERSIONS: usize = 500;

/// Where the archive of one layer version came from.
///
/// `zip_bytes` is the only variant whose bytes this emulator holds, so it is the only
/// one a size and a digest can be computed from; an S3 object is a reference this
/// emulator does not resolve, and nothing is reported for it that was not computed.
#[derive(Debug, Clone, Default)]
pub struct LayerArchiveRecord {
    /// The layer archive bytes, when the caller uploaded them inline.
    pub zip_bytes: Option<Vec<u8>>,
    /// The S3 bucket holding the layer archive, when the caller named one.
    pub s3_bucket: Option<String>,
    /// The S3 key of the layer archive.
    pub s3_key: Option<String>,
    /// The S3 object version of the layer archive.
    pub s3_object_version: Option<String>,
    /// How the archive is stored: [`S3_OBJECT_STORAGE_MODE_COPY`] or
    /// [`S3_OBJECT_STORAGE_MODE_REFERENCE`].
    pub s3_object_storage_mode: String,
}

/// One version of a layer: the archive it was published from, the metadata the request
/// carried and when it was published.
///
/// "Each time you call PublishLayerVersion with the same layer name, a new version is
/// created" (the model's own `PublishLayerVersion` documentation), so a version is
/// immutable once stored and keeps its own copy of everything a read reports.
#[derive(Debug, Clone)]
pub struct LayerVersionRecord {
    /// The version number, counting from 1 upwards.
    pub version: i64,
    /// The description of the version, when the caller gave one.
    pub description: Option<String>,
    /// The layer's software license, when the caller gave one.
    pub license_info: Option<String>,
    /// The runtimes the version declares itself compatible with, in the order given.
    pub compatible_runtimes: Vec<String>,
    /// The architectures the version declares itself compatible with, in the order
    /// given.
    pub compatible_architectures: Vec<String>,
    /// Where the archive came from.
    pub archive: LayerArchiveRecord,
    /// The size of the stored archive, in bytes; 0 when this emulator holds no bytes
    /// for it (an S3 object it does not resolve).
    pub code_size: i64,
    /// The base64 SHA-256 of the stored archive bytes; absent when this emulator holds
    /// no bytes to hash.
    pub code_sha256: Option<String>,
    /// ISO-8601 instant the version was published.
    pub created: String,
}

impl LayerVersionRecord {
    /// The `LayerVersionContentOutput` a read reports for this version's archive,
    /// `location` being the link into the Lambda-held copy of it.
    #[must_use]
    pub fn content(&self, location: String) -> LayerVersionContentOutput {
        LayerVersionContentOutput {
            location: Some(location),
            code_sha256: self.code_sha256.clone(),
            code_size: self.code_size,
            // "The resolved Amazon S3 object that contains the layer archive" — in
            // `REFERENCE` mode that object is the caller's, because "Lambda references
            // the layer archive from the specified Amazon S3 bucket". In the default
            // `COPY` mode Lambda holds a copy of its own and the caller's object is no
            // longer where the archive is served from, so none is reported.
            resolved_s3_object: self
                .archive
                .s3_bucket
                .clone()
                .filter(|_| self.archive.s3_object_storage_mode == S3_OBJECT_STORAGE_MODE_REFERENCE)
                .map(|bucket| ResolvedS3Object {
                    s3_bucket: Some(bucket),
                    s3_key: self.archive.s3_key.clone(),
                    s3_object_version: self.archive.s3_object_version.clone(),
                }),
            ..LayerVersionContentOutput::default()
        }
    }
}

/// One layer and every version published under its name.
#[derive(Debug, Clone)]
pub struct LayerRecord {
    /// The layer's own ARN, which carries no version.
    pub arn: String,
    /// The region the layer was published in, which its ARNs carry.
    pub region: String,
    /// The bare layer name (never an ARN).
    pub name: String,
    /// The highest version number ever published under this name; a number is never
    /// reused, so the next version follows this one rather than the count held.
    pub latest_version: i64,
    /// Every version this emulator holds, keyed by version number so a read of a
    /// layer's versions is ordered.
    pub versions: BTreeMap<i64, LayerVersionRecord>,
}

impl LayerRecord {
    /// The ARN of `version` of this layer: the layer ARN with the version appended, as
    /// the model's `LayerVersionArn` pattern requires.
    #[must_use]
    pub fn version_arn(&self, version: i64) -> String {
        format!("{}:{version}", self.arn)
    }

    /// The `GetLayerVersionResponse` the wire reports for one of this layer's versions,
    /// built entirely out of what is stored.
    #[must_use]
    pub fn version_detail(&self, version: &LayerVersionRecord) -> GetLayerVersionOutput {
        GetLayerVersionOutput {
            content: Some(version.content(layer_archive_location(
                &self.region,
                &self.name,
                version.version,
            ))),
            layer_arn: Some(self.arn.clone()),
            layer_version_arn: Some(self.version_arn(version.version)),
            description: version.description.clone(),
            created_date: Some(version.created.clone()),
            version: version.version,
            // A list the caller named nothing in is absent from the answer rather than
            // reported empty: the model gives neither member a default, so an unnamed
            // one is a member this version carries no value for.
            compatible_architectures: (!version.compatible_architectures.is_empty())
                .then(|| version.compatible_architectures.clone()),
            compatible_runtimes: (!version.compatible_runtimes.is_empty())
                .then(|| version.compatible_runtimes.clone()),
            license_info: version.license_info.clone(),
        }
    }
}

/// `CapacityProviderState` values this slice moves a capacity provider through, from
/// the model's `CapacityProviderState` enum (`Pending | Active | Failed | Deleting`).
pub const CAPACITY_PROVIDER_STATE_PENDING: &str = "Pending";
/// See [`CAPACITY_PROVIDER_STATE_PENDING`].
pub const CAPACITY_PROVIDER_STATE_ACTIVE: &str = "Active";

/// How long after `CreateCapacityProvider` the move from `Pending` to `Active` becomes
/// due.
///
/// An EMULATOR-CHOSEN span (how long Lambda takes to stand the compute resources up is
/// not in the model), deliberately non-zero for the same reason as
/// [`ACTIVATION_DELAY`]: a caller that reads the provider straight after creating it
/// sees the `Pending` state the create answered with. The instant it becomes due is
/// stored on the record and nothing sleeps for it.
pub const CAPACITY_PROVIDER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The most capacity providers this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `CreateCapacityProvider` adds to it, so past this point the slice answers the
/// modelled `CapacityProviderLimitExceededException` ("The maximum number of capacity
/// providers for your account has been exceeded") rather than growing without bound.
pub const MAX_CAPACITY_PROVIDERS: usize = 100;

/// One capacity provider this emulator holds.
///
/// `CreateCapacityProvider` "creates a capacity provider that manages compute resources
/// for Lambda Event Functions", so what it persists is the configuration the request
/// described — the VPC it launches instances in, the role it manages them with, the
/// instance requirements, the scaling configuration, the encryption key, the tag
/// propagation and the telemetry configuration — beside the lifecycle this emulator
/// owns. Every member `GetCapacityProvider` reports comes back out of this record (see
/// [`CapacityProviderRecord::reported`]), so nothing of a request is echoed straight
/// back to its caller.
#[derive(Debug, Clone)]
pub struct CapacityProviderRecord {
    /// The provider's bare name, which is also the key it is stored under.
    pub name: String,
    /// The ARN minted for it once, at creation; see [`capacity_provider_arn`].
    pub arn: String,
    /// The region that ARN carries.
    pub region: String,
    /// `VpcConfig`: the subnets and security groups instances are launched in.
    pub vpc_config: CapacityProviderVpcConfig,
    /// `PermissionsConfig`: the operator role the provider manages compute with.
    pub permissions_config: CapacityProviderPermissionsConfig,
    /// `InstanceRequirements`, when the request stated any.
    pub instance_requirements: Option<InstanceRequirements>,
    /// `CapacityProviderScalingConfig`, when the request stated one.
    pub scaling_config: Option<CapacityProviderScalingConfig>,
    /// `KmsKeyArn`, when the request named a key.
    pub kms_key_arn: Option<String>,
    /// The tags associated with the provider, kept in key order.
    pub tags: BTreeMap<String, String>,
    /// `PropagateTags`, when the request stated it.
    pub propagate_tags: Option<PropagateTags>,
    /// `TelemetryConfig`, when the request stated one.
    pub telemetry_config: Option<CapacityProviderTelemetryConfig>,
    /// The provider's `State`, one of the model's `CapacityProviderState` values.
    pub state: String,
    /// When the move from `Pending` to `Active` becomes due; see
    /// [`CapacityProviderRecord::materialize`].
    pub active_due: SystemTime,
    /// `LastModified`: "The date and time when the capacity provider was last
    /// modified", in the same ISO-8601 form as every other timestamp this service
    /// reports.
    pub last_modified: String,
}

impl CapacityProviderRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transition it applies is the one the clock owed at `now`: a provider
    /// whose compute resources have finished being stood up becomes `Active`. It would
    /// have been owed at the same instant even if nobody had called, so applying it on a
    /// read reports the truth rather than inventing a change.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.state == CAPACITY_PROVIDER_STATE_PENDING && now >= self.active_due {
            self.state = CAPACITY_PROVIDER_STATE_ACTIVE.to_string();
        }
    }

    /// The `CapacityProvider` the wire reports for this record, built entirely out of
    /// what is stored.
    #[must_use]
    pub fn reported(&self) -> CapacityProvider {
        CapacityProvider {
            capacity_provider_arn: self.arn.clone(),
            state: self.state.clone(),
            vpc_config: self.vpc_config.clone(),
            permissions_config: self.permissions_config.clone(),
            instance_requirements: self.instance_requirements.clone(),
            capacity_provider_scaling_config: self.scaling_config.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            last_modified: Some(self.last_modified.clone()),
            propagate_tags: self.propagate_tags.clone(),
            telemetry_config: self.telemetry_config.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-lambda-awsgirapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdaAwsgirapiserviceState {
    /// Every function this emulator holds, keyed by function name so `ListFunctions`
    /// pages through them in a stable order.
    pub functions: BTreeMap<String, FunctionRecord>,
    /// Every layer this emulator holds, keyed by layer name so `ListLayers` pages
    /// through them in a stable order.
    pub layers: BTreeMap<String, LayerRecord>,
    /// Every capacity provider this emulator holds, keyed by provider name so
    /// `ListCapacityProviders` pages through them in a stable order.
    pub capacity_providers: BTreeMap<String, CapacityProviderRecord>,
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
    /// Bring every function and every capacity provider up to whatever the clock has
    /// already made due; see [`FunctionRecord::materialize`] and
    /// [`CapacityProviderRecord::materialize`].
    pub fn materialize_all(&mut self, now: SystemTime) {
        for function in self.functions.values_mut() {
            function.materialize(now);
        }
        for provider in self.capacity_providers.values_mut() {
            provider.materialize(now);
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
    /// storage quota, so the sum covers `$LATEST` and each published version. Every
    /// published layer version's archive is the other term that sentence names.
    #[must_use]
    pub fn account_usage(&self) -> AccountUsage {
        let package_code_size = self
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
        let archive_code_size = self
            .layers
            .values()
            .flat_map(|layer| layer.versions.values().map(|version| version.code_size))
            .fold(0i64, i64::saturating_add);
        let total_code_size = package_code_size.saturating_add(archive_code_size);
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

/// The unqualified ARN of a layer: `arn:aws:lambda:<region>:<account>:layer:<name>`,
/// as the model's `LayerArn` pattern requires.
#[must_use]
pub fn layer_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("layer:{name}"))
}

/// The ARN of a capacity provider:
/// `arn:aws:lambda:<region>:<account>:capacity-provider:<name>`, as the model's
/// `CapacityProviderArn` pattern requires.
#[must_use]
pub fn capacity_provider_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("capacity-provider:{name}"),
    )
}

/// The bare provider name a `CapacityProviderName` names.
///
/// The model's `CapacityProviderName` pattern admits two forms — a capacity-provider ARN
/// and a bare name — and both name the same record, so an ARN resolves here to the name
/// it carries. `None` when the value is an `arn:`-prefixed string that is not a
/// capacity-provider ARN of this service, which names no provider at all.
#[must_use]
pub fn parse_capacity_provider_name(value: &str) -> Option<String> {
    if !value.starts_with("arn:") {
        return (!value.is_empty()).then(|| value.to_string());
    }
    // arn:<partition>:lambda:<region>:<account>:capacity-provider:<name>. The resource
    // type is required, so an ARN of another Lambda resource is not read as a provider
    // named after its resource type.
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 7 || parts[2] != ARN_NAMESPACE || parts[5] != "capacity-provider" {
        return None;
    }
    (!parts[6].is_empty()).then(|| parts[6].to_string())
}

/// The message a `CapacityProviderName` that names no provider is refused with.
#[must_use]
pub fn capacity_provider_name_message(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at 'capacityProviderName' failed to \
         satisfy constraint: Member must satisfy one of the documented name formats (name \
         or capacity provider ARN)"
    )
}

/// Resolve a `CapacityProviderName` (in either of its documented forms) to one stored
/// capacity provider.
///
/// The record is brought up to whatever the clock already made due before it is handed
/// back, so every caller reports the same lifecycle state at the same instant.
pub fn resolve_capacity_provider<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    capacity_provider_name: &str,
    region: &str,
    now: SystemTime,
) -> Result<&'a mut CapacityProviderRecord, ResolveFailure> {
    let name = parse_capacity_provider_name(capacity_provider_name).ok_or_else(|| {
        ResolveFailure::Malformed(capacity_provider_name_message(capacity_provider_name))
    })?;
    let Some(record) = store.capacity_providers.get_mut(&name) else {
        return Err(ResolveFailure::NotFound(format!(
            "Capacity provider not found: {}",
            capacity_provider_arn(region, &name)
        )));
    };
    record.materialize(now);
    Ok(record)
}

/// The download location a layer version's `Content$Location` reports ("A link to the
/// layer archive in Amazon S3 that is valid for 10 minutes").
///
/// Lambda keeps the archive in its own per-region bucket and vends a URL into it. Every
/// segment here comes from stored state — the layer's region, its name and the version
/// number — so two reads of one published version report the same location, and no read
/// changes what a later one sees.
#[must_use]
pub fn layer_archive_location(region: &str, name: &str, version: i64) -> String {
    format!(
        "https://awslambda-{region}-layers.s3.{region}.amazonaws.com/snapshots/{account}/{name}-{version}",
        account = ::engine::emulator_account_id(),
    )
}

/// The bare layer name a `LayerName` names.
///
/// The model documents the member as "The name or Amazon Resource Name (ARN) of the
/// layer", and both forms name the same record, so a layer ARN resolves here to the name
/// it carries. `None` when the value is neither form.
#[must_use]
pub fn parse_layer_name(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.split(':').collect();
    let name = if parts.first() == Some(&"arn") {
        // arn:<partition>:lambda:<region>:<account>:layer:<name>. The resource type is
        // required, so an ARN of another Lambda resource is not read as a layer named
        // after its resource type.
        if parts.len() != 7 || parts[2] != ARN_NAMESPACE || parts[5] != "layer" {
            return None;
        }
        parts[6]
    } else if parts.len() == 1 {
        parts[0]
    } else {
        return None;
    };
    is_layer_name(name).then(|| name.to_string())
}

/// Whether `name` is a layer name, which the model's `LayerName` pattern spells
/// `[a-zA-Z0-9-_]+`.
#[must_use]
pub fn is_layer_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// What a `LayerVersionArn` names.
///
/// The model's pattern admits exactly two forms, and they name resources in two
/// different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerVersionReference {
    /// `arn:<partition>:lambda:<region>:<account>:layer:<name>:<version>` — a version
    /// published into an account, which this store holds.
    Published {
        /// The layer's bare name.
        name: String,
        /// The version number the ARN qualifies it with.
        version: i64,
    },
    /// `arn:<partition>:lambda:::awslayer:<name>` — a layer Lambda itself provides,
    /// which belongs to no account and is held in no store.
    AwsManaged,
}

/// Split a `LayerVersionArn` into the parts that identify a layer version, or `None`
/// when the value is neither documented form.
#[must_use]
pub fn parse_layer_version_arn(value: &str) -> Option<LayerVersionReference> {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.first() != Some(&"arn") || parts.get(2) != Some(&ARN_NAMESPACE) {
        return None;
    }
    let resource_type = parts.get(5).copied().unwrap_or_default();
    match (parts.len(), resource_type) {
        (8, "layer") if is_layer_name(parts[6]) => {
            let version = parts[7].parse::<i64>().ok().filter(|version| *version > 0)?;
            Some(LayerVersionReference::Published {
                name: parts[6].to_string(),
                version,
            })
        }
        (7, "awslayer") if is_layer_name(parts[6]) => Some(LayerVersionReference::AwsManaged),
        _ => None,
    }
}

/// Resolve a function's `Layers` — "A list of function layers to add to the function's
/// execution environment. Specify each layer by its ARN, including the version" —
/// against the layer versions this store holds.
///
/// The error carries the message of the refusal: an entry that is not a layer version
/// ARN, or one naming a version this account has published no layer for. A layer
/// Lambda itself provides belongs to no account, so it is taken as named: this emulator
/// holds no archive for it and therefore reports the `CodeSize` member's own default for
/// it, exactly as it does for a deployment package whose bytes it does not hold.
pub fn resolve_function_layers(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    layers: &[String],
) -> Result<Vec<FunctionLayer>, String> {
    layers
        .iter()
        .map(|arn| match parse_layer_version_arn(arn) {
            None => Err(format!(
                "1 validation error detected: Value '{arn}' at 'layers' failed to satisfy \
                 constraint: Member must be the ARN of a layer version, including the version"
            )),
            Some(LayerVersionReference::AwsManaged) => Ok(FunctionLayer {
                arn: arn.clone(),
                code_size: 0,
            }),
            Some(LayerVersionReference::Published { name, version }) => {
                let published = store
                    .layers
                    .get(&name)
                    .and_then(|layer| Some((layer, layer.versions.get(&version)?)))
                    .ok_or_else(|| format!("Layer version {arn} does not exist."))?;
                // The ARN and the size both come out of the stored version, so nothing
                // of the request is echoed back as a resolved layer.
                Ok(FunctionLayer {
                    arn: published.0.version_arn(version),
                    code_size: published.1.code_size,
                })
            }
        })
        .collect()
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

/// AWS's published Lambda quota for environment variables: 4 KB for all of a function's
/// environment variables combined.
pub const MAX_ENVIRONMENT_BYTES: usize = 4 * 1024;

/// The environment variables an `Environment` member asks a function to hold, ordered so
/// a read reports them deterministically.
///
/// `Err` carries the message of the refusal: the set is larger than the documented
/// quota. Every operation that accepts an `Environment` applies the same quota, so the
/// check lives here rather than once per operation.
pub fn environment_variables(
    environment: Option<&Environment>,
) -> Result<BTreeMap<String, String>, String> {
    let variables: BTreeMap<String, String> = environment
        .and_then(|environment| environment.variables.clone())
        .map(|variables| variables.into_iter().collect())
        .unwrap_or_default();
    let size: usize = variables
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum();
    if size > MAX_ENVIRONMENT_BYTES {
        return Err(format!(
            "Environment.Variables must be smaller than {MAX_ENVIRONMENT_BYTES} bytes for all \
             environment variables combined, and this request's are {size} bytes"
        ));
    }
    Ok(variables)
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

/// What a `TaggableResource` ARN — the `Resource` label of the
/// `/2017-03-31/tags/{Resource}` operations — names.
///
/// The member's own pattern admits exactly five Lambda resource types
/// (`smithy_query(query="operation", operation="ListTags")`): a function, optionally
/// qualified by a published version or an alias, a code-signing configuration, an event
/// source mapping, a capacity provider and a network connector. A function and a capacity
/// provider are the ones this store holds records of; the rest are reported by their
/// resource type and answered as what is true of them here — a resource of that type this
/// account does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableResourceRef {
    /// `…:function:<name>` — one function, whose tag set is
    /// [`FunctionRecord::tags`].
    Function(String),
    /// `…:function:<name>:<qualifier>` — a published version or an alias of a function.
    /// The `Resource` member's own documentation says "Lambda does not support adding
    /// tags to function aliases or versions", so a qualified ARN names no tag set.
    FunctionQualifier {
        /// The bare function name the ARN carries.
        name: String,
        /// The version label or alias name qualifying it.
        qualifier: String,
    },
    /// `…:capacity-provider:<name>` — one capacity provider, whose tag set is
    /// [`CapacityProviderRecord::tags`].
    CapacityProvider(String),
    /// Another Lambda resource type the pattern admits, by its ARN resource type.
    Other(String),
}

/// Split a `TaggableResource` into the parts that identify what it names, or `None`
/// when the value is not a `lambda` resource ARN at all.
///
/// The ARN is read through `service_common::arn` — the one parse this repository states
/// for a caller-supplied ARN — so the `<type>:<id>` and `<type>/<id>` conventions are
/// handled in one place, and the function form is then split by
/// [`parse_function_reference`] so that every tag operation resolves a function exactly
/// the way every other operation on this service does.
///
/// The ARN's region and account fields are read by that parse and deliberately NOT
/// reported: this store keys a function by its name alone, exactly as
/// [`resolve_function`] resolves every other `FunctionName`, so reporting them here
/// would invite a caller to refuse a cross-region or cross-account ARN that every
/// other operation on this service accepts. A tag operation that needs to refuse one
/// has to make that rule hold for the whole service, and should call
/// `service_common::arn::parse_of` for the fields when it does.
///
/// [`TaggableResourceRef::Other`] is only a resource TYPE because this store holds no
/// records of those types yet — whoever implements `CreateCodeSigningConfig`,
/// `CreateEventSourceMapping` or the network-connector operations has to carry the type's
/// id through here and turn the tag handlers' `Other` branch into a real lookup against
/// the new records, the way a capacity provider is resolved here.
#[must_use]
pub fn parse_taggable_resource(value: &str) -> Option<TaggableResourceRef> {
    let arn = service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    let resource_type = arn.resource_type();
    if resource_type.is_empty() {
        return None;
    }
    // A capacity provider is named by `…:capacity-provider:<name>`, and this store holds
    // one record per provider, so the ARN resolves to the name that record is keyed by.
    if resource_type == "capacity-provider" {
        return Some(TaggableResourceRef::CapacityProvider(
            parse_capacity_provider_name(value)?,
        ));
    }
    if resource_type != "function" {
        return Some(TaggableResourceRef::Other(resource_type.to_string()));
    }
    let reference = parse_function_reference(value)?;
    Some(match reference.qualifier {
        Some(qualifier) => TaggableResourceRef::FunctionQualifier {
            name: reference.name,
            qualifier,
        },
        None => TaggableResourceRef::Function(reference.name),
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
    fn a_taggable_resource_arn_names_the_resource_it_carries() {
        // One row per alternative the `TaggableResource` pattern admits
        // (`smithy_query(query="operation", operation="ListTags")`), plus a value that
        // is no `lambda` resource ARN at all.
        let cases: Vec<(&str, Option<TaggableResourceRef>)> = vec![
            (
                "arn:aws:lambda:us-east-1:000000000000:function:my-function",
                Some(TaggableResourceRef::Function("my-function".to_string())),
            ),
            (
                "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:my-function",
                Some(TaggableResourceRef::Function("my-function".to_string())),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:function:my-function:3",
                Some(TaggableResourceRef::FunctionQualifier {
                    name: "my-function".to_string(),
                    qualifier: "3".to_string(),
                }),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:function:my-function:$LATEST",
                Some(TaggableResourceRef::FunctionQualifier {
                    name: "my-function".to_string(),
                    qualifier: "$LATEST".to_string(),
                }),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:function:my-function:blue",
                Some(TaggableResourceRef::FunctionQualifier {
                    name: "my-function".to_string(),
                    qualifier: "blue".to_string(),
                }),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdefg",
                Some(TaggableResourceRef::Other(
                    "code-signing-config".to_string(),
                )),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:event-source-mapping:\
                 12345678-1234-1234-1234-123456789012",
                Some(TaggableResourceRef::Other(
                    "event-source-mapping".to_string(),
                )),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:capacity-provider:fleet-1",
                Some(TaggableResourceRef::CapacityProvider("fleet-1".to_string())),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:network-connector:link-1",
                Some(TaggableResourceRef::Other("network-connector".to_string())),
            ),
            ("arn:aws:sqs:us-east-1:000000000000:queue:q", None),
            ("my-function", None),
        ];
        for (input, expected) in cases {
            let actual = parse_taggable_resource(input);
            assert_eq!(
                actual, expected,
                "case {input:?}: expected {expected:?}, got {actual:?}"
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
