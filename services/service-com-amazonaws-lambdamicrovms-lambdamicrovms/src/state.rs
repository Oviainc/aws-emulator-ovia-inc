// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambdamicrovms-lambdamicrovms service.
//!
//! Every operation computes its response from the records held here; nothing is
//! canned and nothing is echoed back from the request. The handlers reach this
//! store through [`service_state`] and mutate it through the guard it hands back.
//!
//! ## The clock-owed transitions
//!
//! Two of the service's documented lifecycles are asynchronous:
//!
//! * `CreateMicrovmImage` — "The build is asynchronous — the image transitions
//!   from CREATING to CREATED on success"; and
//! * `RunMicrovm` — "The MicroVM starts in PENDING state and transitions to
//!   RUNNING once provisioning completes."
//!
//! Each record therefore stores the INSTANT its next transition becomes due and
//! [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::materialize_due`] applies
//! every transition the clock has already made due. No handler sleeps, and a
//! caller that reads straight after the create still observes the transient
//! state.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{CodeArtifact, CpuConfiguration, Hooks, IdlePolicy, Logging, Resources};

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
                let Some(record) = image.versions.get_mut(&version) else {
                    continue;
                };
                record.build_due_at = None;
                record.state = VERSION_STATE_SUCCESSFUL.to_string();
                record.status = VERSION_STATUS_ACTIVE.to_string();
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
