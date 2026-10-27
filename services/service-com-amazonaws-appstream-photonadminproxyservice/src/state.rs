// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appstream-photonadminproxyservice service.
//!
//! The stack/fleet lifecycle lives here: `CreateStack`/`CreateFleet` write a record,
//! `UpdateStack` applies the members it is given to the stored record and clears the
//! fields its `AttributesToDelete` names, `DescribeStacks`/`DescribeFleets` render the
//! stored record back, `AssociateFleet` records the fleet/stack edge, and
//! `DeleteStack`/`DeleteFleet` refuse while an edge still names the resource.
//!
//! Records hold the MODELLED sub-shapes (`StorageConnector`, `UserSetting`,
//! `VpcConfig`, …) rather than copies of them, so what a Describe returns is literally
//! what the Create persisted.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AccessEndpoint, AgentAccessConfig, ApplicationSettingsResponse, ComputeCapacityStatus,
    ContentRedirection, DomainJoinInfo, Fleet, FleetError, S3Location, Stack, StackError,
    StorageConnector, StreamingExperienceSettings, UserSetting, VolumeConfig, VpcConfig,
};

/// The ARN namespace this service stamps on every resource ARN
/// (`OPERATIONS.md`: "ARN namespace: `appstream`").
pub const ARN_NAMESPACE: &str = "appstream";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own shared config declares.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `FleetType` a `CreateFleet` request that omits it gets. The model's doc for
/// `CreateFleet` states the service default is ON_DEMAND.
pub const DEFAULT_FLEET_TYPE: &str = "ON_DEMAND";

/// The `FleetState` a freshly created fleet carries: a fleet is created stopped and
/// starts streaming only once `StartFleet` runs, so this is a terminal status the clock
/// owes no transition out of.
pub const FLEET_STATE_CREATED: &str = "STOPPED";

/// How many records one `DescribeStacks`/`DescribeFleets` page carries.
///
/// The model declares `NextToken` on both operations but publishes no page limit, so
/// this is the emulator's own page size; what matters on the wire is that a caller who
/// follows `NextToken` sees every record exactly once, in a stable order.
pub const DESCRIBE_PAGE_SIZE: usize = 50;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores (stacks, fleets, fleet/stack associations).
///
/// This is NOT AWS's service quota — AppStream's published stack and fleet limits are
/// per-account and adjustable, and nothing here claims to reproduce them. It is the
/// emulator's own memory ceiling: every successful `CreateStack`, `CreateFleet` and
/// `AssociateFleet` inserts a record that lives until a caller deletes it, so without a
/// ceiling a caller could grow the process until the host runs out of memory. A request
/// that would cross it is refused with `LimitExceededException`, the modelled error AWS
/// itself uses for "the requested limit exceeds the permitted limit for an account", so
/// the refusal is one a real client already handles.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with; `store`
/// names the records it holds, e.g. `"stacks"`.
pub fn limit_exceeded_message(store: &str) -> String {
    format!(
        "The requested limit exceeds the permitted limit for an account: \
         this emulator holds at most {MAX_RECORDS_PER_STORE} {store}"
    )
}

/// The region to stamp into the ARNs a request mints: the SigV4 credential scope's
/// region, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:appstream:<region>:<account>:stack/<name>`.
pub fn stack_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("stack/{name}"))
}

/// `arn:aws:appstream:<region>:<account>:fleet/<name>`.
pub fn fleet_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("fleet/{name}"))
}

/// The instant a record is created, as the wire type the modelled `CreatedTime`
/// members carry.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A persisted AppStream stack.
#[derive(Debug, Clone)]
pub struct StackRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub storage_connectors: Vec<StorageConnector>,
    pub redirect_url: Option<String>,
    pub feedback_url: Option<String>,
    pub user_settings: Vec<UserSetting>,
    pub application_settings: Option<ApplicationSettingsResponse>,
    pub access_endpoints: Vec<AccessEndpoint>,
    pub embed_host_domains: Vec<String>,
    pub streaming_experience_settings: Option<StreamingExperienceSettings>,
    pub content_redirection: Option<ContentRedirection>,
    pub agent_access_config: Option<AgentAccessConfig>,
    pub storage_connector_authentication: Option<String>,
    pub stack_errors: Vec<StackError>,
    pub tags: BTreeMap<String, String>,
}

/// Render a stored configuration collection the way the resource reports it: a
/// collection with no entries is a configuration the stack does not have, so the member
/// is ABSENT from the rendered resource rather than present-and-empty.
///
/// This is the representation side of clearing a stack attribute: once an `UpdateStack`
/// request drops a stack's access endpoints, user settings, embedded host domains or
/// storage connectors, the corresponding key is gone from the returned `Stack` and from
/// every later `DescribeStacks`, exactly as it is for a stack that was never given one.
fn reported<T: Clone>(entries: &[T]) -> Option<Vec<T>> {
    (!entries.is_empty()).then(|| entries.to_vec())
}

impl StackRecord {
    /// Render the stored record as the modelled `Stack` every reader returns.
    pub fn to_stack(&self) -> Stack {
        Stack {
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            created_time: Some(self.created_time),
            storage_connectors: reported(&self.storage_connectors),
            redirect_url: self.redirect_url.clone(),
            feedback_url: self.feedback_url.clone(),
            // A stack always reports its error list; an empty one says "no errors",
            // which is a value, not a missing configuration.
            stack_errors: Some(self.stack_errors.clone()),
            user_settings: reported(&self.user_settings),
            application_settings: self.application_settings.clone(),
            access_endpoints: reported(&self.access_endpoints),
            embed_host_domains: reported(&self.embed_host_domains),
            streaming_experience_settings: self.streaming_experience_settings.clone(),
            content_redirection: self.content_redirection.clone(),
            agent_access_config: self.agent_access_config.clone(),
            storage_connector_authentication: self.storage_connector_authentication.clone(),
        }
    }
}

/// A persisted AppStream fleet.
#[derive(Debug, Clone)]
pub struct FleetRecord {
    pub arn: String,
    pub name: String,
    pub instance_type: String,
    pub fleet_type: String,
    pub image_name: Option<String>,
    pub image_arn: Option<String>,
    /// One of `FLEET_STATE_VALUES`; `STOPPED` for a freshly created fleet.
    pub state: String,
    pub desired_instances: i32,
    pub desired_user_sessions: Option<i32>,
    pub running: i32,
    pub in_use: i32,
    pub available: i32,
    pub vpc_config: Option<VpcConfig>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub max_user_duration_in_seconds: Option<i32>,
    pub disconnect_timeout_in_seconds: Option<i32>,
    pub idle_disconnect_timeout_in_seconds: Option<i32>,
    pub max_concurrent_sessions: Option<i32>,
    pub max_sessions_per_instance: Option<i32>,
    pub stream_view: Option<String>,
    pub platform: Option<String>,
    pub enable_default_internet_access: Option<bool>,
    pub domain_join_info: Option<DomainJoinInfo>,
    pub iam_role_arn: Option<String>,
    pub usb_device_filter_strings: Vec<String>,
    pub session_script_s3_location: Option<S3Location>,
    pub root_volume_config: Option<VolumeConfig>,
    pub disable_imdsv1: Option<bool>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub fleet_errors: Vec<FleetError>,
    pub tags: BTreeMap<String, String>,
}

impl FleetRecord {
    /// Render the stored record as the modelled `Fleet` every reader returns.
    pub fn to_fleet(&self) -> Fleet {
        Fleet {
            arn: self.arn.clone(),
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            image_name: self.image_name.clone(),
            image_arn: self.image_arn.clone(),
            instance_type: self.instance_type.clone(),
            fleet_type: Some(self.fleet_type.clone()),
            compute_capacity_status: ComputeCapacityStatus {
                desired: self.desired_instances,
                running: Some(self.running),
                in_use: Some(self.in_use),
                available: Some(self.available),
                desired_user_sessions: self.desired_user_sessions,
                ..ComputeCapacityStatus::default()
            },
            max_user_duration_in_seconds: self.max_user_duration_in_seconds,
            disconnect_timeout_in_seconds: self.disconnect_timeout_in_seconds,
            state: self.state.clone(),
            vpc_config: self.vpc_config.clone(),
            created_time: Some(self.created_time),
            fleet_errors: Some(self.fleet_errors.clone()),
            enable_default_internet_access: self.enable_default_internet_access,
            domain_join_info: self.domain_join_info.clone(),
            idle_disconnect_timeout_in_seconds: self.idle_disconnect_timeout_in_seconds,
            iam_role_arn: self.iam_role_arn.clone(),
            stream_view: self.stream_view.clone(),
            platform: self.platform.clone(),
            max_concurrent_sessions: self.max_concurrent_sessions,
            usb_device_filter_strings: Some(self.usb_device_filter_strings.clone()),
            session_script_s3_location: self.session_script_s3_location.clone(),
            max_sessions_per_instance: self.max_sessions_per_instance,
            root_volume_config: self.root_volume_config.clone(),
            disable_imdsv1: self.disable_imdsv1,
        }
    }
}

/// Per-service state for com-amazonaws-appstream-photonadminproxyservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// Stacks by `Name`. `BTreeMap` so every Describe page walks a stable order.
    pub stacks: BTreeMap<String, StackRecord>,
    /// Fleets by `Name`.
    pub fleets: BTreeMap<String, FleetRecord>,
    /// `(FleetName, StackName)` edges written by `AssociateFleet`.
    pub fleet_stack_associations: BTreeSet<(String, String)>,
}

impl ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// Whether any association still names this fleet. A predicate, so the common
    /// (deletable) case walks the edges without allocating; the names themselves are
    /// only materialised by [`stacks_for_fleet`](Self::stacks_for_fleet) on the
    /// refusal path.
    pub fn fleet_is_associated(&self, fleet_name: &str) -> bool {
        self.fleet_stack_associations
            .range((fleet_name.to_string(), String::new())..)
            .take_while(|(fleet, _)| fleet == fleet_name)
            .next()
            .is_some()
    }

    /// Whether any association still names this stack.
    pub fn stack_is_associated(&self, stack_name: &str) -> bool {
        self.fleet_stack_associations
            .iter()
            .any(|(_, stack)| stack == stack_name)
    }

    /// The stacks the named fleet is associated with.
    pub fn stacks_for_fleet(&self, fleet_name: &str) -> Vec<String> {
        self.fleet_stack_associations
            .range((fleet_name.to_string(), String::new())..)
            .take_while(|(fleet, _)| fleet == fleet_name)
            .map(|(_, stack)| stack.clone())
            .collect()
    }

    /// The fleets associated with the named stack.
    pub fn fleets_for_stack(&self, stack_name: &str) -> Vec<String> {
        self.fleet_stack_associations
            .iter()
            .filter(|(_, stack)| stack == stack_name)
            .map(|(fleet, _)| fleet.clone())
            .collect()
    }
}

/// One page of a Describe: the names to render, and the opaque token that resumes after
/// them.
///
/// `names` is sorted and deduplicated, so following `next_token` walks every record
/// exactly once even when records are added between calls.
#[derive(Debug)]
pub struct DescribePage {
    pub names: Vec<String>,
    pub next_token: Option<String>,
}

/// Encode a pagination cursor (the name of the first record of the next page).
///
/// A real AppStream `NextToken` is opaque, so the cursor is hex-encoded rather than
/// handed to the caller in the clear: a client cannot read structure into it or
/// hand-craft one. `hex` is the maintained crate for the codec.
fn encode_cursor(name: &str) -> String {
    hex::encode(name)
}

/// The inverse of [`encode_cursor`]; `None` for a token this service did not mint.
fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// Cut `all` (sorted, deduplicated record names) into the page that resumes at
/// `next_token`. `None` means the token is not one this service minted.
pub fn paginate(all: &BTreeSet<String>, next_token: Option<&str>) -> Option<DescribePage> {
    let cursor = match next_token {
        Some(token) => Some(decode_cursor(token)?),
        None => None,
    };
    let mut remaining = all
        .iter()
        .filter(|name| {
            cursor
                .as_deref()
                .is_none_or(|resume_at| name.as_str() >= resume_at)
        })
        .cloned();
    let names: Vec<String> = remaining.by_ref().take(DESCRIBE_PAGE_SIZE).collect();
    Some(DescribePage {
        next_token: remaining.next().map(|name| encode_cursor(&name)),
        names,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppstreamPhotonadminproxyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppstreamPhotonadminproxyserviceState>)
}
