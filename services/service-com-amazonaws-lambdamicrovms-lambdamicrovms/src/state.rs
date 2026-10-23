// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambdamicrovms-lambdamicrovms service.
//!
//! Every operation computes its response from the records held here; nothing is
//! canned and nothing is echoed back from the request. The handlers reach this
//! store through [`service_state`] and mutate it through the guard it hands back.
//!
//! ## The clock-owed transitions
//!
//! Several of the service's lifecycles are asynchronous:
//!
//! * `CreateMicrovmImage` — "The build is asynchronous — the image transitions
//!   from CREATING to CREATED on success";
//! * `RunMicrovm` — "The MicroVM starts in PENDING state and transitions to
//!   RUNNING once provisioning completes."; and
//! * `DeleteMicrovmImage` — the image is reported `DELETING` and the removal that
//!   makes every later read miss is owed by the clock.
//!
//! Each record therefore stores the INSTANT its next transition becomes due and
//! [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::materialize_due`] applies
//! every transition the clock has already made due. No handler sleeps, and a
//! caller that reads straight after the create still observes the transient
//! state.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    CloudWatchLogging, CodeArtifact, CpuConfiguration, Hooks, IdlePolicy, Logging, Resources,
};

/// The region an ARN carries when the request is unsigned, matching the region
/// the emulator's generated client config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateMicrovmImage` the asynchronous image build becomes due.
///
/// The model documents the build as asynchronous but gives it no duration, so
/// this span is chosen (not derived from AWS): long enough that a caller acting
/// straight after the create observes `CREATING`, short enough to poll through.
pub const IMAGE_BUILD_SPAN: Duration = Duration::from_millis(500);

/// How long after `RunMicrovm` the `PENDING` -> `RUNNING` provisioning becomes
/// due. Chosen for the same reason as [`IMAGE_BUILD_SPAN`].
pub const MICROVM_PROVISION_SPAN: Duration = Duration::from_millis(500);

/// How long after `TerminateMicrovm` the `TERMINATING` -> `TERMINATED`
/// transition becomes due. Chosen for the same reason as [`IMAGE_BUILD_SPAN`].
pub const MICROVM_TERMINATE_SPAN: Duration = Duration::from_millis(500);

/// How long after `DeleteMicrovmImage` the image's removal becomes due.
///
/// The deletion is asynchronous — the call reports the image `DELETING` and a client
/// polls `GetMicrovmImage` until it answers 404 — but AWS publishes no duration for
/// it, so this span is chosen for the same reason as [`IMAGE_BUILD_SPAN`]: a caller
/// acting straight after the delete still observes `DELETING`, and a poll gets
/// through it quickly.
pub const IMAGE_DELETE_SPAN: Duration = Duration::from_millis(500);

/// The AWS-managed network connector a MicroVM image gets when the request names no
/// `egressNetworkConnectors`: outbound internet access.
///
/// This service names its managed connectors (`CreateMicrovmShellAuthToken` documents
/// "the SHELL_INGRESS network connector"); which one is the egress default is recorded
/// AWS behaviour rather than modelled.
pub const DEFAULT_EGRESS_NETWORK_CONNECTOR: &str = "INTERNET_EGRESS";

/// The `maximumDurationInSeconds` a MicroVM gets when the caller names none.
///
/// `RunMicrovmRequest$maximumDurationInSeconds` is documented as "The maximum
/// duration in seconds that the MicroVM can exist before being terminated by the
/// platform. Valid range: 1–28,800 (8 hours)" and declares no `@default`, so a
/// caller that asks for no limit gets the platform's own maximum.
pub const DEFAULT_MAXIMUM_DURATION_IN_SECONDS: i32 = MAX_MAXIMUM_DURATION_IN_SECONDS;

/// The smallest `maximumDurationInSeconds` the member's own `@range` allows.
///
/// `smithy_query(query="operation",operation="RunMicrovm")` reports
/// `maximumDurationInSeconds` as `{"target": "Integer", "range": {"min": 1,
/// "max": 28800}}`, matching the member's documented "Valid range: 1–28,800 (8 hours)".
pub const MIN_MAXIMUM_DURATION_IN_SECONDS: i32 = 1;

/// The largest `maximumDurationInSeconds` the member's own `@range` allows — see
/// [`MIN_MAXIMUM_DURATION_IN_SECONDS`] for the `smithy_query` output it comes from.
pub const MAX_MAXIMUM_DURATION_IN_SECONDS: i32 = 28_800;

/// The largest page `ListMicrovms` serves when the caller names no `maxResults`.
///
/// This is the maximum the member's own `@range` allows:
/// `smithy_query(query="operation",operation="ListMicrovms")` reports
/// `maxResults` as `{"target": "Integer", "httpQuery": "maxResults",
/// "range": {"min": 1, "max": 50}}`.
pub const MAX_LIST_PAGE: usize = 50;

/// How many MicroVM images one account may hold.
///
/// The model declares `ServiceQuotaExceededException` — "You have exceeded a service
/// quota for Lambda MicroVMs" — on `CreateMicrovmImage`, so a per-account ceiling is
/// the service's own modelled behaviour. AWS publishes no figure, so the figure is
/// chosen: high enough that no realistic use meets it, finite so the store cannot grow
/// without bound.
pub const MAX_MICROVM_IMAGES: usize = 500;

/// How many MicroVMs one account may have in a non-terminal state at once. Modelled
/// and chosen for the same reasons as [`MAX_MICROVM_IMAGES`], on `RunMicrovm`.
pub const MAX_ACTIVE_MICROVMS: usize = 500;

/// How long a `TERMINATED` MicroVM stays readable before its record is reclaimed, so
/// terminated MicroVMs do not accumulate for the process's lifetime. AWS publishes no
/// figure, so this span is chosen.
pub const TERMINATED_MICROVM_RETENTION: Duration = Duration::from_secs(60 * 60);

/// How long a `clientToken` keeps replaying the resource it minted. AWS publishes no
/// figure for this service, so the span is chosen; it bounds the idempotency store by
/// time as [`MAX_IDEMPOTENCY_RECORDS`] bounds it by count.
pub const IDEMPOTENCY_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The most `clientToken` records the store keeps. Reaching it evicts the record
/// closest to expiry, so a caller sending unbounded distinct tokens cannot grow the
/// store without bound.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 10_000;

/// `stateReason` the service records for a caller-requested termination.
///
/// `NonBlankString` has pattern `^[^\s]+$`, so the reason is a single token.
pub const TERMINATION_REASON: &str = "Client.UserInitiatedTermination";

/// `stateReason` the service records when a MicroVM outlives its
/// `maximumDurationInSeconds` — documented as "The maximum duration in seconds that
/// the MicroVM can exist before being terminated by the platform."
pub const EXPIRY_REASON: &str = "Platform.MaximumDurationExceeded";

/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`).
pub const IMAGE_STATE_CREATING: &str = "CREATING";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`).
pub const IMAGE_STATE_CREATED: &str = "CREATED";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the state an image
/// carries while `DeleteMicrovmImage`'s asynchronous deletion is in flight.
pub const IMAGE_STATE_DELETING: &str = "DELETING";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the state
/// `DeleteMicrovmImage` reports for an image this account no longer holds.
pub const IMAGE_STATE_DELETED: &str = "DELETED";

/// The version the FIRST build of a MicroVM image is minted as.
///
/// Versions are named in this service's `major.minor` form, and a client feeds the
/// string straight back into the version-addressed calls
/// (`GetMicrovmImageVersion`, `RunMicrovm$imageVersion`,
/// `DeleteMicrovmImageVersion`), so the form matters as much as the ordering.
pub const FIRST_IMAGE_VERSION: &str = "1.0";

/// The `message` of the 404 a MicroVM-image lookup answers, built from the
/// `imageIdentifier` the request carried.
pub fn image_not_found_message(identifier: &str) -> String {
    format!("MicroVMImage not found for MicroVMImageID: {identifier}")
}

/// The `message` of the 400 a MicroVM-image lookup answers for an `imageIdentifier`
/// that is neither an id this account holds nor a well-formed ARN, built from the
/// identifier the request carried.
///
/// `MicrovmImageIdentifier` is "The ARN or ID of the MicroVmImage", so a value that
/// resolves as neither names nothing this API can ever address — a bad REQUEST, not a
/// resource that happens to be missing.
pub fn invalid_arn_format_message(identifier: &str) -> String {
    format!("Invalid ARN format: {identifier}")
}

/// The `message` of the 400 `CreateMicrovmImage` answers when the `name` the request
/// asks for is already taken, built from that name.
///
/// `name` "Must be unique within the AWS account", and the refusal is a VALIDATION of
/// the request's own `name` member — recorded AWS behaviour: the service answers a
/// duplicate name with `400 ValidationException` carrying this template rather than
/// with the `ConflictException` its model also declares.
pub fn image_name_conflict_message(name: &str) -> String {
    format!("A MicroVM image with the name '{name}' already exists in this account")
}

/// The `message` of the 400 `CreateMicrovmImage` answers when the `baseImageArn` the
/// request names cannot be resolved to a base image, built from that ARN.
///
/// `baseImageArn` is "The ARN of the Lambda-managed base MicroVM image to build upon",
/// so the one refusal for an unresolvable base-image reference is worded for a managed
/// image whatever namespace the ARN names.
pub fn base_image_not_found_message(base_image_arn: &str) -> String {
    format!("Managed VMI not found: {base_image_arn}")
}

/// The `message` of the 400 `CreateMicrovmImage` answers when `codeArtifact` carries a
/// `uri` that is not an S3 object URI: it names the form the member requires.
///
/// `CodeArtifact$uri` is "The URI of the code artifact in Amazon S3", and the build
/// resolves that location before anything is created, so a URI that addresses no S3
/// object is refused rather than stored.
pub const CODE_ARTIFACT_URI_REQUIREMENT: &str =
    "CodeArtifact URI must be a valid S3 URI in the format: s3://<bucket>/<key>";

/// The `(bucket, key)` the S3 object URI `uri` addresses, or `None` when `uri` is not
/// one.
///
/// An S3 object URI is `s3://<bucket>/<key>`: the scheme is `s3`, and both the bucket
/// and the key are non-empty — `s3://bucket` names a bucket and no object, and
/// `s3:///key` names no bucket.
pub fn s3_object_uri(uri: &str) -> Option<(&str, &str)> {
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}

/// Whether `arn` names a MicroVM image in THIS account's own `microvm-image`
/// namespace — the one namespace this emulator holds, and can therefore resolve a
/// reference against.
///
/// Read through the one shared ARN parse (`service_common::arn`): the vendor is this
/// service's `arnNamespace` (`lambda`), the account is the emulator's own, and the
/// resource type is the one `MicrovmImageArn`'s pattern
/// (`^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`) names. An ARN of
/// any other account — an AWS-MANAGED base image carries `aws` there — belongs to
/// another namespace; see [`managed_microvm_image_id`].
pub fn is_own_microvm_image_arn(arn: &str) -> bool {
    ::service_common::arn::parse(arn).is_some_and(|parsed| {
        parsed.vendor == "lambda"
            && parsed.account == ::engine::emulator_account_id()
            && parsed.resource_type() == "microvm-image"
    })
}

/// The ids of the AWS-MANAGED MicroVM base images this service publishes — the
/// catalogue `baseImageArn` is resolved against.
///
/// `CreateMicrovmImage$baseImageArn` is "The ARN of the Lambda-managed base MicroVM
/// image to build upon. Use ListManagedMicrovmImages to discover available base
/// images", so the set of base images is AWS's own published catalogue rather than
/// anything this account creates: `al2023-1` is the Amazon Linux 2023 base image the
/// service publishes (recorded as
/// `arn:aws:lambda:<region>:aws:microvm-image:al2023-1`). `ListManagedMicrovmImages`
/// is the operation that serves this catalogue; it is still a stub (its own gap
/// issue, deferred by the bootstrap issue), so today the catalogue is reached only
/// through the resolution below.
pub const MANAGED_BASE_IMAGE_IDS: &[&str] = &["al2023-1"];

/// The id an AWS-MANAGED MicroVM image ARN names, or `None` when `arn` names no
/// managed MicroVM image.
///
/// An AWS-owned resource of this service carries `aws` in the ARN's account field
/// (the same form [`managed_network_connector_arn`] builds), so that field is what
/// says a reference is into AWS's catalogue rather than into an account's own images.
pub fn managed_microvm_image_id(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != "lambda" || parsed.account != MANAGED_RESOURCE_ACCOUNT {
        return None;
    }
    parsed.resource.strip_prefix("microvm-image:")
}

/// Whether the AWS-managed catalogue publishes a base image with id `image_id`.
pub fn is_published_managed_base_image(image_id: &str) -> bool {
    MANAGED_BASE_IMAGE_IDS.contains(&image_id)
}

/// The ARN of the AWS-managed MicroVM base image `image_id` in `region`.
pub fn managed_base_image_arn(region: &str, image_id: &str) -> String {
    format!("arn:aws:lambda:{region}:{MANAGED_RESOURCE_ACCOUNT}:microvm-image:{image_id}")
}

/// The ARN account field an AWS-OWNED resource of this service carries, as against an
/// account's own resources, which carry that account's id.
pub const MANAGED_RESOURCE_ACCOUNT: &str = "aws";

/// What a `MicrovmImageIdentifier` resolves to.
///
/// The three outcomes are client-visible and mean different things: a resolved
/// identifier is served, a well-formed ARN this account holds nothing under is a
/// not-found (which a poll may legitimately retry), and anything else is a malformed
/// request that can never succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageReference {
    /// The `name` key of the image the identifier names.
    Resolved(String),
    /// A well-formed ARN (or a bare id) that this account holds no image under.
    Absent,
    /// Neither an id this account holds nor a well-formed ARN.
    Malformed,
}

/// The `DEFAULT_CPU_ARCHITECTURE` a MicroVM image is built for when the request names
/// no `cpuConfigurations`.
///
/// `Architecture` is an enum whose only value is `ARM_64`
/// (`smithy_query(query="shape",shape="Architecture")`), so the configuration the
/// service normalizes an unspecified request to is the only one it can name.
pub const DEFAULT_CPU_ARCHITECTURE: &str = "ARM_64";

/// The `minimumMemoryInMiB` a MicroVM image gets when the request names no
/// `resources`.
///
/// `Resources$minimumMemoryInMiB` is `@required` within the structure and
/// `ResourcesList` is `@length(max: 1)`, so an accepted image carries exactly one
/// resource floor; AWS publishes no figure for the default, so this one is recorded
/// AWS behaviour rather than modelled.
pub const DEFAULT_MINIMUM_MEMORY_IN_MIB: i32 = 2048;

/// The prefix of the CloudWatch log group a MicroVM image's logs go to when the
/// request names no `logging`.
///
/// `Logging` is a union of `cloudWatch` and `disabled` and declares no `@default`, so
/// a request that names neither gets the service's own target: a log group under this
/// service's own namespace, named for the image. The prefix is recorded AWS behaviour.
pub const DEFAULT_LOG_GROUP_PREFIX: &str = "/aws/lambda-microvms";

/// The `cpuConfigurations` an image gets when the request names none.
pub fn default_cpu_configurations() -> Vec<CpuConfiguration> {
    vec![CpuConfiguration {
        architecture: DEFAULT_CPU_ARCHITECTURE.to_string(),
    }]
}

/// The `resources` an image gets when the request names none.
pub fn default_resources() -> Vec<Resources> {
    vec![Resources {
        minimum_memory_in_mi_b: DEFAULT_MINIMUM_MEMORY_IN_MIB,
    }]
}

/// The `logging` an image named `name` gets when the request names none: its logs go
/// to this service's own CloudWatch log group for that image.
pub fn default_logging(name: &str) -> Logging {
    Logging::cloudWatch(CloudWatchLogging {
        log_group: Some(default_log_group(name)),
        log_stream: None,
    })
}

/// The CloudWatch log group an image named `name` logs to by default.
///
/// `CloudWatchLogging$logGroup` is `@length(1..512)` with pattern
/// `^[a-zA-Z0-9_\-/.#]+$`, which `ImageName` (`^[a-zA-Z0-9-_]+$`, 1..64 characters)
/// under this prefix always satisfies.
pub fn default_log_group(name: &str) -> String {
    format!("{DEFAULT_LOG_GROUP_PREFIX}/{name}")
}

/// The `message` of the 404 a MicroVM lookup answers, built from the
/// `microvmIdentifier` the request carried.
pub fn microvm_not_found_message(identifier: &str) -> String {
    format!("MicroVM not found: {identifier}")
}

/// The `message` of the 404 `RunMicrovm` answers when it can resolve no ACTIVE
/// version for the image the request names — a different condition from a missing
/// image, and the one whose remediation is to publish or wait for a version.
pub fn no_active_version_message(image: &str) -> String {
    format!("No active version found for MicroVM image {image}")
}

/// The version the NEXT build of an image is minted as, given the versions it
/// already holds.
///
/// Versions are `major.minor` (see [`FIRST_IMAGE_VERSION`]): the first build is
/// `1.0` and every later build of the same image takes the next minor of the
/// highest `major.minor` already minted. A stored version that is not in that form
/// (nothing this service mints is) is ignored rather than crashing the mint.
pub fn next_image_version<'a>(existing: impl IntoIterator<Item = &'a str>) -> String {
    let highest = existing
        .into_iter()
        .filter_map(parse_image_version)
        .max_by_key(|(major, minor)| (*major, *minor));
    match highest {
        Some((major, minor)) => format!("{major}.{}", minor + 1),
        None => FIRST_IMAGE_VERSION.to_string(),
    }
}

/// `version` as its `(major, minor)` pair, or `None` when it is not in that form.
fn parse_image_version(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Whether a MicroVM in `state` can still be connected to, which is what an
/// authentication token is for (`CreateMicrovmAuthToken`: "an authentication token
/// for accessing a running MicroVM").
///
/// A MicroVM on its way out, or already gone, never accepts another connection, so a
/// token for it would grant access to nothing.
pub fn is_connectable_microvm_state(state: &str) -> bool {
    state != MICROVM_STATE_TERMINATING && state != MICROVM_STATE_TERMINATED
}

/// A `MicrovmImageVersionState` value (`MICROVM_IMAGE_VERSION_STATE_VALUES`).
pub const VERSION_STATE_IN_PROGRESS: &str = "IN_PROGRESS";
/// A `MicrovmImageVersionState` value (`MICROVM_IMAGE_VERSION_STATE_VALUES`).
pub const VERSION_STATE_SUCCESSFUL: &str = "SUCCESSFUL";

/// A `MicrovmImageVersionStatus` value (`MICROVM_IMAGE_VERSION_STATUS_VALUES`).
pub const VERSION_STATUS_ACTIVE: &str = "ACTIVE";
/// A `MicrovmImageVersionStatus` value (`MICROVM_IMAGE_VERSION_STATUS_VALUES`).
pub const VERSION_STATUS_INACTIVE: &str = "INACTIVE";

/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_PENDING: &str = "PENDING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_RUNNING: &str = "RUNNING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_TERMINATING: &str = "TERMINATING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_TERMINATED: &str = "TERMINATED";

/// One version of a MicroVM image: the build inputs the create (or an update)
/// carried, plus the state of that build.
#[derive(Debug, Clone)]
pub struct MicrovmImageVersion {
    pub image_version: String,
    /// A `MicrovmImageVersionState`.
    pub state: String,
    /// A `MicrovmImageVersionStatus`.
    pub status: String,
    pub base_image_arn: String,
    pub base_image_version: Option<String>,
    pub build_role_arn: String,
    pub description: Option<String>,
    pub code_artifact: CodeArtifact,
    pub logging: Option<Logging>,
    pub egress_network_connectors: Vec<String>,
    pub cpu_configurations: Vec<CpuConfiguration>,
    pub resources: Vec<Resources>,
    pub additional_os_capabilities: Vec<String>,
    pub hooks: Option<Hooks>,
    pub environment_variables: BTreeMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: Option<DateTime>,
    pub tags: BTreeMap<String, String>,
    /// When the asynchronous build completes; `None` once it has.
    pub build_due_at: Option<Instant>,
}

/// A MicroVM image, keyed in [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::images`] by its name.
#[derive(Debug)]
pub struct MicrovmImage {
    pub image_arn: String,
    pub name: String,
    /// A `MicrovmImageState`.
    pub state: String,
    pub created_at: DateTime,
    pub updated_at: Option<DateTime>,
    pub latest_active_image_version: Option<String>,
    pub latest_failed_image_version: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub versions: BTreeMap<String, MicrovmImageVersion>,
    /// When this image's asynchronous deletion removes it; `None` while no delete is
    /// in flight.
    pub delete_due_at: Option<Instant>,
}

/// A running (or terminated) MicroVM, keyed in
/// [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::microvms`] by its `microvmId`.
#[derive(Debug, Clone)]
pub struct Microvm {
    pub microvm_id: String,
    /// This MicroVM's own ARN, so a `microvmIdentifier` given as an ARN resolves
    /// against the ARN this service minted rather than against any ARN whose last
    /// segment happens to match a local id.
    pub microvm_arn: String,
    /// A `MicrovmState`.
    pub state: String,
    pub endpoint: String,
    pub image_arn: String,
    pub image_version: String,
    pub execution_role_arn: Option<String>,
    pub idle_policy: Option<IdlePolicy>,
    pub logging: Option<Logging>,
    pub maximum_duration_in_seconds: i32,
    pub started_at: DateTime,
    pub terminated_at: Option<DateTime>,
    pub state_reason: Option<String>,
    pub ingress_network_connectors: Vec<String>,
    pub egress_network_connectors: Vec<String>,
    /// When `PENDING` -> `RUNNING` becomes due; `None` once it has.
    pub provision_due_at: Option<Instant>,
    /// When `TERMINATING` -> `TERMINATED` becomes due; `None` once it has.
    pub terminate_due_at: Option<Instant>,
    /// When `maximumDurationInSeconds` runs out and the platform terminates this
    /// MicroVM; `None` once it has, or once the MicroVM is already ending.
    pub expiry_due_at: Option<Instant>,
    /// When this `TERMINATED` record is reclaimed, so terminated MicroVMs do not
    /// accumulate for the process's lifetime; set from the termination instant, and
    /// `None` while the MicroVM has not terminated.
    pub reclaim_due_at: Option<Instant>,
}

/// What an `@idempotencyToken` replay must return instead of minting a second
/// resource.
#[derive(Debug, Clone)]
pub enum MintedResource {
    /// `CreateMicrovmImage`: the image ARN and the version that call minted.
    Image {
        image_arn: String,
        image_version: String,
    },
    /// `RunMicrovm`: the MicroVM that call minted.
    Microvm { microvm_id: String },
}

/// A `clientToken` record: what that token minted, and when it stops replaying.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub minted: MintedResource,
    pub expires_at: Instant,
}

/// Per-service state for com-amazonaws-lambdamicrovms-lambdamicrovms. A handler reaches it through [`service_state`].
///
/// Every map here is bounded: `images` by [`MAX_MICROVM_IMAGES`], `microvms` by
/// [`MAX_ACTIVE_MICROVMS`] plus the [`TERMINATED_MICROVM_RETENTION`] reclaim, and
/// `idempotency` by [`MAX_IDEMPOTENCY_RECORDS`] and [`IDEMPOTENCY_RETENTION`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdamicrovmsLambdamicrovmsState {
    /// Images by `name` (`ImageName` is unique within the account).
    pub images: BTreeMap<String, MicrovmImage>,
    /// `imageArn` -> image `name`, so an identifier resolves by ARN or by name.
    pub arn_index: BTreeMap<String, String>,
    /// MicroVMs by `microvmId`. The ids sort in creation order, which is the
    /// order `ListMicrovms` pages through.
    pub microvms: BTreeMap<String, Microvm>,
    /// MicroVM ARN -> `microvmId`, so an identifier resolves by ARN or by id.
    pub microvm_arn_index: BTreeMap<String, String>,
    /// `clientToken` -> the resource that token already minted.
    pub idempotency: BTreeMap<String, IdempotencyRecord>,
    /// The sequence the next minted `microvmId` is formatted from.
    pub next_microvm_seq: u64,
}

impl ComAmazonawsLambdamicrovmsLambdamicrovmsState {
    /// Apply every lifecycle transition the clock has already made due.
    ///
    /// This is the only write a reporting operation performs: each transition was
    /// owed at this same instant whether or not anybody called.
    pub fn materialize_due(&mut self, now: Instant) {
        for image in self.images.values_mut() {
            let due: Vec<String> = image
                .versions
                .iter()
                .filter(|(_, v)| v.build_due_at.is_some_and(|at| at <= now))
                .map(|(k, _)| k.clone())
                .collect();
            for version in due {
                // The build completing CHANGES the version and the image, so both
                // carry a new `updatedAt` — "the timestamp when the MicroVM image
                // was last updated" is what a client orders change by.
                let updated_at = now_datetime();
                let Some(record) = image.versions.get_mut(&version) else {
                    continue;
                };
                record.build_due_at = None;
                record.state = VERSION_STATE_SUCCESSFUL.to_string();
                record.status = VERSION_STATUS_ACTIVE.to_string();
                record.updated_at = Some(updated_at);
                image.updated_at = Some(updated_at);
                // A newly successful build becomes the image's active version,
                // and the previous one steps down: `MicrovmImageVersionStatus`
                // marks exactly one version ACTIVE.
                if let Some(previous) = image.latest_active_image_version.replace(version.clone()) {
                    if previous != version {
                        if let Some(old) = image.versions.get_mut(&previous) {
                            old.status = VERSION_STATUS_INACTIVE.to_string();
                        }
                    }
                }
                if image.state == IMAGE_STATE_CREATING {
                    image.state = IMAGE_STATE_CREATED.to_string();
                }
            }
        }

        // `DeleteMicrovmImage` is asynchronous: an image whose removal the clock has
        // made due goes, with every version it holds, and both indexes go with it.
        let removed: Vec<String> = self
            .images
            .values()
            .filter(|image| image.delete_due_at.is_some_and(|at| at <= now))
            .map(|image| image.name.clone())
            .collect();
        for name in removed {
            if let Some(image) = self.images.remove(&name) {
                self.arn_index.remove(&image.image_arn);
            }
        }

        for vm in self.microvms.values_mut() {
            if vm.provision_due_at.is_some_and(|at| at <= now) {
                vm.provision_due_at = None;
                if vm.state == MICROVM_STATE_PENDING {
                    vm.state = MICROVM_STATE_RUNNING.to_string();
                }
            }
            if let Some(due) = vm.terminate_due_at.filter(|at| *at <= now) {
                vm.terminate_due_at = None;
                if vm.state == MICROVM_STATE_TERMINATING {
                    vm.state = MICROVM_STATE_TERMINATED.to_string();
                    // `terminatedAt` is "the timestamp when the MicroVM was
                    // terminated", so it is stamped when the MicroVM REACHES
                    // TERMINATED, not when it started terminating.
                    vm.terminated_at = Some(now_datetime());
                    // The retention runs from the TERMINATION instant, not from the
                    // request that happened to observe it.
                    vm.reclaim_due_at = Some(due + TERMINATED_MICROVM_RETENTION);
                }
            }
            // "The maximum duration in seconds that the MicroVM can exist before
            // being terminated by the platform."
            if let Some(due) = vm.expiry_due_at.filter(|at| *at <= now) {
                vm.expiry_due_at = None;
                if vm.state != MICROVM_STATE_TERMINATING && vm.state != MICROVM_STATE_TERMINATED {
                    vm.state = MICROVM_STATE_TERMINATED.to_string();
                    vm.provision_due_at = None;
                    vm.terminated_at = Some(now_datetime());
                    vm.state_reason = Some(EXPIRY_REASON.to_string());
                    vm.reclaim_due_at = Some(due + TERMINATED_MICROVM_RETENTION);
                }
            }
        }
        let mut reclaimed: Vec<String> = Vec::new();
        self.microvms.retain(|_, vm| {
            let keep = vm.reclaim_due_at.is_none_or(|at| at > now);
            if !keep {
                reclaimed.push(vm.microvm_arn.clone());
            }
            keep
        });
        for arn in reclaimed {
            self.microvm_arn_index.remove(&arn);
        }
        self.idempotency.retain(|_, record| record.expires_at > now);
    }

    /// How many MicroVMs are not terminated — what [`MAX_ACTIVE_MICROVMS`] caps.
    pub fn active_microvm_count(&self) -> usize {
        self.microvms
            .values()
            .filter(|vm| vm.state != MICROVM_STATE_TERMINATED)
            .count()
    }

    /// What `client_token` already minted on `operation`, if anything.
    pub fn minted_by(&self, operation: &str, client_token: Option<&str>) -> Option<&MintedResource> {
        let key = idempotency_key(operation, client_token?);
        Some(&self.idempotency.get(&key)?.minted)
    }

    /// Record what `client_token` minted on `operation`, evicting the record closest
    /// to expiry when the store is at [`MAX_IDEMPOTENCY_RECORDS`].
    pub fn record_minted(
        &mut self,
        operation: &str,
        client_token: &str,
        minted: MintedResource,
        now: Instant,
    ) {
        let key = idempotency_key(operation, client_token);
        if !self.idempotency.contains_key(&key) && self.idempotency.len() >= MAX_IDEMPOTENCY_RECORDS
        {
            let evict = self
                .idempotency
                .iter()
                .min_by_key(|(_, record)| record.expires_at)
                .map(|(token, _)| token.clone());
            if let Some(token) = evict {
                self.idempotency.remove(&token);
            }
        }
        self.idempotency.insert(
            key,
            IdempotencyRecord {
                minted,
                expires_at: now + IDEMPOTENCY_RETENTION,
            },
        );
    }

    /// The name of the image `identifier` names, resolving either form
    /// `MicrovmImageIdentifier` documents ("The ARN or ID of the MicroVmImage").
    ///
    /// An ARN resolves ONLY through [`Self::arn_index`], so it names the image whose
    /// stored `imageArn` it IS. Matching just its resource segment would resolve
    /// another partition's, region's or account's ARN to a local image of the same
    /// name.
    pub fn resolve_image_name(&self, identifier: &str) -> Option<String> {
        if identifier.starts_with("arn:") {
            return self.arn_index.get(identifier).cloned();
        }
        self.images
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }

    /// What `identifier` resolves to, which is what says whether the caller gets the
    /// image, a not-found or a refusal.
    ///
    /// `MicrovmImageIdentifier` is "The ARN or ID of the MicroVmImage", so the
    /// identifier is resolved as an id this account holds FIRST and read as an ARN
    /// only when it names no held image: a value that is neither — a bare string that
    /// is no id of this account and no ARN — addresses nothing this API can name, so it
    /// is [`ImageReference::Malformed`] rather than absent.
    pub fn resolve_image_reference(&self, identifier: &str) -> ImageReference {
        if let Some(name) = self.resolve_image_name(identifier) {
            return ImageReference::Resolved(name);
        }
        if ::service_common::arn::parse(identifier).is_some() {
            return ImageReference::Absent;
        }
        ImageReference::Malformed
    }

    /// The image `identifier` names, by ARN or by name.
    pub fn image(&self, identifier: &str) -> Option<&MicrovmImage> {
        let name = self.resolve_image_name(identifier)?;
        self.images.get(&name)
    }

    /// The `microvmId` the `identifier` names, resolving either form
    /// `MicrovmIdentifier` documents ("The ARN or ID of the MicroVm").
    ///
    /// An ARN resolves ONLY through [`Self::microvm_arn_index`], so it names the
    /// MicroVM whose stored ARN it IS. Matching just its resource segment would
    /// resolve another partition's, region's or account's ARN to a local MicroVM of
    /// the same id.
    pub fn resolve_microvm_id(&self, identifier: &str) -> Option<String> {
        if identifier.starts_with("arn:") {
            return self.microvm_arn_index.get(identifier).cloned();
        }
        self.microvms
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }

    /// The MicroVM `identifier` names, by ARN or by id.
    pub fn microvm(&self, identifier: &str) -> Option<&Microvm> {
        let id = self.resolve_microvm_id(identifier)?;
        self.microvms.get(&id)
    }

    /// The image `identifier` names, for mutation.
    pub fn image_mut(&mut self, identifier: &str) -> Option<&mut MicrovmImage> {
        let name = self.resolve_image_name(identifier)?;
        self.images.get_mut(&name)
    }

    /// The version of the base image `base_image_arn` names, for a request that named
    /// none.
    ///
    /// `baseImageVersion` is "the specific version of the base MicroVM image to use",
    /// so the service resolves one rather than recording none: the base image's own
    /// active version when this account holds it, and otherwise — an AWS-managed base
    /// image, whose catalogue this emulator does not hold — that image's initial
    /// version, [`FIRST_IMAGE_VERSION`].
    pub fn resolve_base_image_version(&self, base_image_arn: &str) -> String {
        self.image(base_image_arn)
            .and_then(|image| image.latest_active_image_version.clone())
            .unwrap_or_else(|| FIRST_IMAGE_VERSION.to_string())
    }

    /// The MicroVM `identifier` names, for mutation.
    pub fn microvm_mut(&mut self, identifier: &str) -> Option<&mut Microvm> {
        let id = self.resolve_microvm_id(identifier)?;
        self.microvms.get_mut(&id)
    }

    /// Mint the next `microvmId`. Zero-padded so the ids sort in creation
    /// order, which is the order `ListMicrovms` must page through.
    pub fn next_microvm_id(&mut self) -> String {
        let seq = self.next_microvm_seq;
        self.next_microvm_seq += 1;
        format!("mvm-{seq:016x}")
    }
}

/// The key a `clientToken` is recorded under. An `@idempotencyToken` is scoped to the
/// operation that minted with it, so the same token on two operations is two keys.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}\u{1f}{client_token}")
}

/// `now`, as the wire timestamp type every output member uses.
pub fn now_datetime() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// The region this request was signed for, for the ARNs it mints.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the MicroVM image named `name` in `region`.
///
/// `MicrovmImageArn` has pattern
/// `^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`, and the
/// service's `arnNamespace` is `lambda`.
pub fn microvm_image_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("microvm-image:{name}"))
}

/// The ARN of an AWS-MANAGED network connector named `name`, in `region`.
///
/// The connector is an AWS-owned resource, so its ARN carries `aws` in the account
/// field rather than this account's id — which is why it is not built through
/// `engine::emulator_arn` (that mints an ARN owned by the emulator's account).
/// The form is recorded AWS behaviour:
/// `arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:<NAME>`.
pub fn managed_network_connector_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:aws:network-connector:aws-network-connector:{name}")
}

/// The ARN of the MicroVM `microvm_id` in `region`.
///
/// `MicrovmIdentifier` is documented as "The ARN or ID of the MicroVm", so each
/// MicroVM carries an ARN an identifier can be resolved against; the service's
/// `arnNamespace` is `lambda`.
pub fn microvm_arn(region: &str, microvm_id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("microvm:{microvm_id}"))
}

/// The HTTPS endpoint a MicroVM is reached on. `RunMicrovmResponse$endpoint` is
/// documented as "The HTTPS endpoint URL for communicating with the MicroVM".
pub fn microvm_endpoint(region: &str, microvm_id: &str) -> String {
    format!("https://{microvm_id}.microvm.lambda.{region}.amazonaws.com")
}

/// A fresh authentication token for connecting a client to one MicroVM's endpoint,
/// for `CreateMicrovmAuthToken`.
///
/// The token authenticates to the MICROVM endpoint, not to this emulator, so it must
/// never be accepted as a SigV4 credential — but it is still a secret handed to a
/// caller, and nothing in a service may construct a secret itself. It is therefore
/// minted by the one component that generates secrets, the emulator's credential
/// store, and then removed from that store again: the caller gets the secret, the
/// emulator keeps no credential it would accept, and nothing accumulates per call.
///
/// `None` when the store minted no token secret, which leaves the operation to refuse
/// rather than report a token it made up.
pub fn mint_microvm_auth_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// A sorted copy of a wire map, so a stored record has one deterministic order.
pub fn sorted_map(map: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    map.unwrap_or_default().into_iter().collect()
}

/// A stored map as the wire shape, omitted when empty rather than serialized as
/// an empty object.
pub fn wire_map(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    (!map.is_empty()).then(|| {
        map.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>()
    })
}

/// A stored map as the wire shape, PRESENT even when empty.
///
/// `GetMicrovmImage` projects the image's whole stored record, and the `tags` of an
/// image carrying none is an empty map rather than an absent member (recorded AWS
/// behaviour): a client reading `tags` gets a map to iterate either way.
pub fn wire_map_present(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        map.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

/// A stored list as the wire shape, omitted when empty.
pub fn wire_list<T: Clone>(list: &[T]) -> Option<Vec<T>> {
    (!list.is_empty()).then(|| list.to_vec())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdamicrovmsLambdamicrovmsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLambdamicrovmsLambdamicrovmsState>)
}
