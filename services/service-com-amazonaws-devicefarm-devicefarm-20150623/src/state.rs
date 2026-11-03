// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devicefarm-devicefarm-20150623 service.
//!
//! The persisted records live here, together with the small helpers every Project
//! handler shares (ARN minting and classification, the request's signing region, and
//! the record -> modelled-shape conversion). `src/ops/mod.rs` is generated from the
//! model and cannot carry a hand-written module, so this editable file is the service's
//! one home for code more than one handler needs.

use std::collections::BTreeMap;

use crate::ops::types::{EnvironmentVariable, NetworkProfile, Project, VpcConfig};

/// The execution timeout a project gets when `CreateProject` is called without
/// `defaultJobTimeoutMinutes`.
///
/// The value is the model's own documented default for `Project$defaultJobTimeoutMinutes`
/// ("The default number of minutes (at the project level) a test run executes before it
/// times out. The default value is 150 minutes."), so it is a documented duration rather
/// than a chosen one.
pub const DEFAULT_JOB_TIMEOUT_MINUTES: i32 = 150;

/// The resource type a project ARN carries: `arn:aws:devicefarm:<region>:<account>:project:<id>`.
pub const PROJECT_RESOURCE_TYPE: &str = "project";

/// The resource type a network-profile ARN carries:
/// `arn:aws:devicefarm:<region>:<account>:networkprofile:<project-id>/<profile-id>`.
pub const NETWORK_PROFILE_RESOURCE_TYPE: &str = "networkprofile";

/// The only type a network profile created through the API can have.
///
/// `NetworkProfileType` models two values: a `CURATED` profile is one the service itself
/// publishes, and a profile a caller creates inside its own project is its own — so the
/// created profile is reported as `PRIVATE` whatever type the request asked for.
pub const NETWORK_PROFILE_TYPE_PRIVATE: &str = "PRIVATE";

/// The throughput a network profile gets for a direction the request does not shape.
///
/// `CreateNetworkProfile`'s bandwidth members document the range "an integer from 0 to
/// 104857600", and the top of that range is an unshaped link: a profile created without a
/// bandwidth imposes no throughput limit. The value is therefore the modelled maximum
/// rather than a figure of this emulator's own choosing, while which end of the range an
/// omitted member lands on is the emulator's reading of "unshaped".
pub const DEFAULT_NETWORK_PROFILE_BANDWIDTH_BITS: i64 = 104_857_600;

/// The delay and jitter a network profile gets for a direction the request does not shape:
/// none. Both members document milliseconds "from 0 to 2000", and an unshaped link adds
/// neither — the same reading as [`DEFAULT_NETWORK_PROFILE_BANDWIDTH_BITS`], at the other
/// end of the range because delay and jitter are impairments rather than capacity.
pub const DEFAULT_NETWORK_PROFILE_DELAY_MS: i64 = 0;

/// How many projects one `ListProjects` page carries.
///
/// Device Farm's `ListProjects` models no `maxResults`, and AWS documents no page size
/// for it, so this bound is CHOSEN: it only has to be a fixed positive number so a
/// truncated page hands back a `nextToken` and a resumed call continues from it.
pub const PROJECTS_PAGE_SIZE: usize = 50;

/// How many projects the store holds before `CreateProject` refuses with the modelled
/// `LimitExceededException`.
///
/// Device Farm publishes an adjustable per-account quota on projects, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the emulator's
/// OWN bound on the store rather than a claim about any account's limit. It is set far
/// above any workflow an emulator serves, and it is what stops the state growing without
/// end: every record it holds is released by `DeleteProject`.
pub const PROJECT_QUOTA: usize = 1_000;

/// How many network profiles the store holds before `CreateNetworkProfile` refuses with
/// the modelled `LimitExceededException`.
///
/// Like [`PROJECT_QUOTA`], this is the emulator's OWN bound on the store rather than a
/// claim about any account's published quota, set far above any workflow an emulator
/// serves: it is what stops repeated calls growing the state without end.
pub const NETWORK_PROFILE_QUOTA: usize = 1_000;

/// One stored project: the single source of truth every Project response is built from.
///
/// The ARN and `created` instant are minted once, at create time, and never recomputed —
/// so every later `GetProject`/`ListProjects` reports byte-identical values.
#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub arn: String,
    pub name: String,
    pub default_job_timeout_minutes: i32,
    pub created: ::aws_smithy_types::DateTime,
    pub vpc_config: Option<VpcConfig>,
    pub environment_variables: Vec<EnvironmentVariable>,
    pub execution_role_arn: Option<String>,
}

impl ProjectRecord {
    /// The modelled `Project` for this record. Every member is read off the record, so a
    /// response can never carry a value the store does not hold.
    pub fn to_project(&self) -> Project {
        Project {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            default_job_timeout_minutes: Some(self.default_job_timeout_minutes),
            created: Some(self.created),
            vpc_config: self.vpc_config.clone(),
            // An absent list and an empty list are different answers on the wire: a
            // project created without environment variables reports none at all.
            environment_variables: if self.environment_variables.is_empty() {
                None
            } else {
                Some(self.environment_variables.clone())
            },
            execution_role_arn: self.execution_role_arn.clone(),
        }
    }
}

/// One stored network profile: the single source of truth every `NetworkProfile` response
/// is built from.
///
/// Every shaping member is stored RESOLVED — the value the request asked for, or the
/// default the service applies for a direction it did not shape — so no response has to
/// re-derive one, and `type` is stored as the service classified the profile rather than
/// as the request named it.
#[derive(Debug, Clone)]
pub struct NetworkProfileRecord {
    pub arn: String,
    /// The project the profile belongs to. Kept so `ListNetworkProfiles` can select the
    /// profiles of one project without parsing the ARN back apart.
    pub project_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub r#type: String,
    pub uplink_bandwidth_bits: i64,
    pub downlink_bandwidth_bits: i64,
    pub uplink_delay_ms: i64,
    pub downlink_delay_ms: i64,
    pub uplink_jitter_ms: i64,
    pub downlink_jitter_ms: i64,
    pub uplink_loss_percent: i32,
    pub downlink_loss_percent: i32,
}

impl NetworkProfileRecord {
    /// The modelled `NetworkProfile` for this record. Every member is read off the
    /// record, so a response can never carry a value the store does not hold.
    pub fn to_network_profile(&self) -> NetworkProfile {
        NetworkProfile {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            r#type: Some(self.r#type.clone()),
            uplink_bandwidth_bits: Some(self.uplink_bandwidth_bits),
            downlink_bandwidth_bits: Some(self.downlink_bandwidth_bits),
            uplink_delay_ms: Some(self.uplink_delay_ms),
            downlink_delay_ms: Some(self.downlink_delay_ms),
            uplink_jitter_ms: Some(self.uplink_jitter_ms),
            downlink_jitter_ms: Some(self.downlink_jitter_ms),
            uplink_loss_percent: self.uplink_loss_percent,
            downlink_loss_percent: self.downlink_loss_percent,
        }
    }
}

/// Per-service state for com-amazonaws-devicefarm-devicefarm-20150623. A handler reaches it through [`service_state`].
///
/// `projects` is keyed by project ARN, so Get/Update/Delete are O(log n) and
/// `ListProjects` has one deterministic (ARN-ordered) page sequence. `network_profiles`
/// is keyed by network-profile ARN for the same reasons.
#[derive(Debug, Default)]
pub struct ComAmazonawsDevicefarmDevicefarm20150623State {
    pub projects: BTreeMap<String, ProjectRecord>,
    pub network_profiles: BTreeMap<String, NetworkProfileRecord>,
    /// The sequence the next minted project id comes from. Private, and only ever
    /// advanced by [`Self::mint_project_arn`], so no two projects can share an id and a
    /// released id is never handed out again.
    next_project_id: u64,
    /// The sequence the next minted network-profile id comes from, with the same
    /// guarantee as `next_project_id`.
    next_network_profile_id: u64,
}

impl ComAmazonawsDevicefarmDevicefarm20150623State {
    /// Mint a fresh, unique project ARN in `region`, advancing the store's sequence.
    ///
    /// The resource portion is `project:<id>`, the form a Device Farm project ARN takes;
    /// the account segment and the ARN frame come from the core's `emulator_arn`. The id
    /// itself is opaque to a client — the model constrains a project ARN only to
    /// `^arn:aws:devicefarm:.+$` (length 32..1011) — so the store's own never-reused
    /// sequence supplies it, which is what makes a minted ARN unique without any outside
    /// source of identity. The ARN is stored on the record and never recomputed.
    pub fn mint_project_arn(&mut self, region: &str) -> String {
        self.next_project_id = self.next_project_id.wrapping_add(1);
        ::emulator_server::emulator_arn(
            "devicefarm",
            region,
            &format!(
                "{PROJECT_RESOURCE_TYPE}:{:016x}-{}",
                self.next_project_id, RESOURCE_ID_SUFFIX
            ),
        )
    }

    /// Mint a fresh, unique network-profile ARN in `region` under `project_arn`,
    /// advancing the store's sequence.
    ///
    /// A Device Farm resource that lives inside a project carries both ids:
    /// `networkprofile:<project-id>/<profile-id>`, so the owning project is readable off
    /// the ARN exactly as it is in the service's own ARNs. The project id is the one the
    /// project ARN already holds, and the profile id comes from the store's never-reused
    /// sequence. The ARN is stored on the record and never recomputed.
    pub fn mint_network_profile_arn(&mut self, region: &str, project_arn: &str) -> String {
        self.next_network_profile_id = self.next_network_profile_id.wrapping_add(1);
        let project_id = devicefarm_resource_id(project_arn, PROJECT_RESOURCE_TYPE).unwrap_or("");
        ::emulator_server::emulator_arn(
            "devicefarm",
            region,
            &format!(
                "{NETWORK_PROFILE_RESOURCE_TYPE}:{project_id}/{:016x}-{}",
                self.next_network_profile_id, RESOURCE_ID_SUFFIX
            ),
        )
    }
}

/// The fixed tail every minted resource id carries, so an ARN is recognisable as one this
/// emulator minted and comfortably clears the modelled minimum ARN length of 32.
const RESOURCE_ID_SUFFIX: &str = "emulated";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDevicefarmDevicefarm20150623State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDevicefarmDevicefarm20150623State>)
}

/// The region this request was signed for, which is the region the resources it creates
/// live in.
///
/// Read from the SigV4 credential scope (`Credential=<akid>/<date>/<region>/<service>/aws4_request`)
/// — the only region a request carries. An unsigned request falls back to the emulator's
/// default region, as `emulator_arn` documents.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's resources are placed in.
const DEFAULT_REGION: &str = "us-east-1";

/// The resource id of `arn`, if `arn` names a Device Farm resource of type
/// `resource_type`.
///
/// Applies the ARN grammar: `arn:<partition>:<service>:<region>:<account>:<type>:<id>`,
/// with a non-empty resource id. A resource that lives inside a project carries both ids
/// in that one field (`<project-id>/<profile-id>`), which is why the id is returned whole
/// rather than split further.
pub fn devicefarm_resource_id<'a>(arn: &'a str, resource_type: &str) -> Option<&'a str> {
    let mut fields = arn.split(':');
    match (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) {
        (
            Some("arn"),
            Some(_partition),
            Some("devicefarm"),
            Some(_region),
            Some(_account),
            Some(found_type),
            Some(id),
            None,
        ) if found_type == resource_type && !id.is_empty() => Some(id),
        _ => None,
    }
}

/// Whether `arn` names a Device Farm resource of type `resource_type`.
///
/// Device Farm's ARN members are modelled only as `^arn:aws:devicefarm:.+$`, so an ARN
/// naming some OTHER Device Farm resource type (an upload, a run, a device pool) reaches a
/// handler and must be refused as an invalid argument rather than reported as a missing
/// resource of the type the operation expects.
pub fn is_devicefarm_arn_of_type(arn: &str, resource_type: &str) -> bool {
    devicefarm_resource_id(arn, resource_type).is_some()
}

/// How a project is named in the prose of a message about one.
pub const PROJECT_ENTITY: &str = "project";

/// How a network profile is named in the prose of a message about one.
pub const NETWORK_PROFILE_ENTITY: &str = "network profile";

/// The message a Device Farm `NotFoundException` carries when the ARN a request supplied
/// resolves to no resource of the type that member names.
///
/// Device Farm words that absence `No <entity> was found for arn <arn>`: the text around
/// the two variable parts is fixed, `entity` is the resource named in words (see
/// [`PROJECT_ENTITY`] and [`NETWORK_PROFILE_ENTITY`]), and `arn` is echoed verbatim from
/// the request's own ARN member rather than re-derived from the store. Every handler that
/// reports a missing resource goes through here, so the whole service words it one way.
pub fn arn_not_found_message(entity: &str, arn: &str) -> String {
    format!("No {entity} was found for arn {arn}")
}

/// Whether `arn` names a project.
pub fn is_project_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, PROJECT_RESOURCE_TYPE)
}

/// Whether `arn` names a network profile.
pub fn is_network_profile_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, NETWORK_PROFILE_RESOURCE_TYPE)
}

/// The current wall-clock instant as a modelled timestamp, for the `created`-style
/// members a handler mints once and then stores.
///
/// Lives here, beside the other helpers more than one handler needs, because
/// `src/ops/mod.rs` is generated and cannot carry a hand-written module.
///
/// A clock before the Unix epoch yields the epoch rather than a panic: a handler must
/// never panic, and the emulator's own creation times are what this feeds.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}
