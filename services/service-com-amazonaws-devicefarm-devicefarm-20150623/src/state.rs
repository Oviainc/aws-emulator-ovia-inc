// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devicefarm-devicefarm-20150623 service.
//!
//! The persisted records live here, together with the small helpers every Project
//! handler shares (ARN minting and classification, the request's signing region, and
//! the record -> modelled-shape conversion). `src/ops/mod.rs` is generated from the
//! model and cannot carry a hand-written module, so this editable file is the service's
//! one home for code more than one handler needs.

use std::collections::BTreeMap;

use crate::ops::types::{EnvironmentVariable, Project, VpcConfig};

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

/// Per-service state for com-amazonaws-devicefarm-devicefarm-20150623. A handler reaches it through [`service_state`].
///
/// `projects` is keyed by project ARN, so Get/Update/Delete are O(log n) and
/// `ListProjects` has one deterministic (ARN-ordered) page sequence.
#[derive(Debug, Default)]
pub struct ComAmazonawsDevicefarmDevicefarm20150623State {
    pub projects: BTreeMap<String, ProjectRecord>,
    /// The sequence the next minted project id comes from. Private, and only ever
    /// advanced by [`Self::mint_project_arn`], so no two projects can share an id and a
    /// released id is never handed out again.
    next_project_id: u64,
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
                self.next_project_id, PROJECT_ID_SUFFIX
            ),
        )
    }
}

/// The fixed tail every minted project id carries, so a project ARN is recognisable as
/// one this emulator minted and comfortably clears the modelled minimum ARN length of 32.
const PROJECT_ID_SUFFIX: &str = "emulated";

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

/// Whether `arn` names a Device Farm resource of type `resource_type`.
///
/// Applies the ARN grammar: `arn:<partition>:<service>:<region>:<account>:<type>:<id>`,
/// with a non-empty resource id. Device Farm's ARN members are modelled only as
/// `^arn:aws:devicefarm:.+$`, so an ARN naming some OTHER Device Farm resource type (an
/// upload, a run, a device pool) reaches a handler and must be refused as an invalid
/// argument rather than reported as a missing project.
pub fn is_devicefarm_arn_of_type(arn: &str, resource_type: &str) -> bool {
    let mut fields = arn.split(':');
    matches!(
        (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ),
        (
            Some("arn"),
            Some(_partition),
            Some("devicefarm"),
            Some(_region),
            Some(_account),
            Some(found_type),
            Some(id),
            None,
        ) if found_type == resource_type && !id.is_empty()
    )
}

/// Whether `arn` names a project.
pub fn is_project_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, PROJECT_RESOURCE_TYPE)
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
