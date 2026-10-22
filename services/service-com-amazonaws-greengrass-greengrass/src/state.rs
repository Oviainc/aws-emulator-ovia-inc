// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-greengrass-greengrass service.
//!
//! AWS IoT Greengrass V1's group lifecycle lives here: a group, its append-only
//! version history and the deployments raised against it. Every handler in
//! `src/ops/` computes its response from this store — ids are minted once at
//! create time and stored, and every ARN is derived from the stored id, so a
//! create, a read and a list all render the identical string.
//!
//! ## One store, not one per region
//!
//! `CreateGroup` stamps the region the request was signed for into the group's ARN,
//! but [`ComAmazonawsGreengrassGreengrassState::groups`] is a single map: a group
//! created against one region is therefore readable and listable from a request
//! addressed at another, carrying the ARN of the region that created it. Real
//! Greengrass is regional. Keying this store by region is a change of its own — it
//! has to move `GetGroup`, `GetGroupVersion`, `CreateGroupVersion`,
//! `CreateDeployment`, `GetDeploymentStatus`, `DeleteGroup` and `ListGroups`
//! together, and decide what a cross-region id answers — so this slice records the
//! simplification rather than applying it to one operation only.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::ops::types::{
    BadRequestException, ErrorDetail, GroupInformation, GroupVersion, InternalServerErrorException,
};

/// This service's sigv4 signing name / ARN namespace (`OPERATIONS.md`, "Wire
/// protocol": sigv4 signing name `greengrass`, ARN namespace `greengrass`).
pub const SERVICE_NAMESPACE: &str = "greengrass";

/// The region a request is taken to address when it carries no SigV4 credential
/// scope to read one from (an anonymous or host-routed call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `GetDeploymentStatus`'s status values, from the model's own documentation for
/// `GetDeploymentStatusResponse$DeploymentStatus`: "The status of the deployment:
/// ''InProgress'', ''Building'', ''Success'', or ''Failure''."
pub const DEPLOYMENT_STATUS_BUILDING: &str = "Building";
/// See [`DEPLOYMENT_STATUS_BUILDING`] for the source of this value list.
pub const DEPLOYMENT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`DEPLOYMENT_STATUS_BUILDING`] for the source of this value list.
pub const DEPLOYMENT_STATUS_SUCCESS: &str = "Success";
/// See [`DEPLOYMENT_STATUS_BUILDING`] for the source of this value list. No producer
/// in this slice: nothing here can observe a delivery failing, so a deployment never
/// reaches `Failure` — see the note in `src/ops/get_deployment_status.rs`.
pub const DEPLOYMENT_STATUS_FAILURE: &str = "Failure";

/// How long each non-terminal deployment status is held before the clock owes the
/// next one. A deployment is reported in a non-terminal status
/// (`Building` -> `InProgress` -> `Success`), so a caller that polls immediately
/// after `CreateDeployment` must still observe the transient status; the span is
/// therefore deliberately non-zero. Not a duration AWS documents, so it is a
/// chosen one, inside the emulator's 250-750 ms window per hop.
pub const DEPLOYMENT_STEP: Duration = Duration::from_millis(300);

// ---------------------------------------------------------------------------
// Store bounds
//
// Every map and vector below grows only on a caller's request, so each one
// carries an explicit ceiling: an emulator is a long-lived process and an
// unbounded caller-driven store is a memory-exhaustion surface. A create that
// would pass a ceiling is refused with the service's own `BadRequestException`
// rather than served, and `DeleteGroup` releases everything the group held,
// including the client tokens that named its resources.
// ---------------------------------------------------------------------------

/// The most groups this emulator holds at once.
pub const MAX_GROUPS: usize = 1_000;

/// The most versions one group's append-only history holds.
pub const MAX_VERSIONS_PER_GROUP: usize = 1_000;

/// The most deployments one group holds.
pub const MAX_DEPLOYMENTS_PER_GROUP: usize = 1_000;

/// The most client tokens remembered for idempotent retries. Past it the oldest
/// token is forgotten — a forgotten token's retry mints a new resource, which is
/// the safe direction: bounded memory, and idempotency still covers every recent
/// retry.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// Per-service state for com-amazonaws-greengrass-greengrass. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGreengrassGreengrassState {
    /// Every live group, keyed by the group id minted by `CreateGroup`.
    pub groups: HashMap<String, Group>,
    /// Monotonic counter handing each group its [`Group::seq`]. A `HashMap` has no
    /// order, so `ListGroups` sorts by this instead and its `NextToken` is a
    /// cursor over it — which keeps a page boundary stable across inserts and
    /// deletes rather than reshuffling on the next call.
    pub next_group_seq: u64,
    /// `AmznClientToken` -> the id of the resource that token already created, so a
    /// retried create is idempotent instead of minting a second resource. Keyed
    /// through [`client_token_key`], because the same token replayed against two
    /// different operations names two different resources.
    pub client_tokens: ClientTokenLedger,
}

impl ComAmazonawsGreengrassGreengrassState {
    /// Hand out the next group sequence number.
    pub fn take_group_seq(&mut self) -> u64 {
        let seq = self.next_group_seq;
        self.next_group_seq = self.next_group_seq.saturating_add(1);
        seq
    }

    /// Every live group in `ListGroups` order (oldest first), by [`Group::seq`].
    pub fn groups_in_order(&self) -> Vec<&Group> {
        let mut groups: Vec<&Group> = self.groups.values().collect();
        groups.sort_by_key(|group| group.seq);
        groups
    }

    /// Remove `group_id` and release everything it held: its versions and
    /// deployments go with the record, and the client tokens that named any of
    /// them are forgotten so a create/delete loop cannot grow the ledger.
    /// `false` when no such group was held.
    pub fn remove_group(&mut self, group_id: &str) -> bool {
        let Some(group) = self.groups.remove(group_id) else {
            return false;
        };
        self.client_tokens.forget_resources(&group.resource_ids());
        true
    }
}

/// The client tokens remembered for idempotent retries, bounded both by
/// [`MAX_CLIENT_TOKENS`] and by release on `DeleteGroup`.
#[derive(Debug, Default)]
pub struct ClientTokenLedger {
    entries: HashMap<String, String>,
    /// Insertion order of `entries`' keys, so the ceiling evicts the oldest.
    order: VecDeque<String>,
}

impl ClientTokenLedger {
    /// The resource id this token already created, if it is still remembered.
    pub fn resource_for(&self, key: &str) -> Option<&String> {
        self.entries.get(key)
    }

    /// Remember that `key` created `resource_id`, evicting the oldest tokens once
    /// the ledger is full.
    pub fn remember(&mut self, key: String, resource_id: String) {
        if self.entries.insert(key.clone(), resource_id).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > MAX_CLIENT_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.entries.remove(&oldest);
                }
                None => break,
            }
        }
    }

    /// Forget every token that named one of `resource_ids` — the resources are
    /// gone, so remembering the token that made them only leaks.
    pub fn forget_resources(&mut self, resource_ids: &HashSet<String>) {
        let entries = &mut self.entries;
        entries.retain(|_, id| !resource_ids.contains(id));
        let remaining = &self.entries;
        self.order.retain(|key| remaining.contains_key(key));
    }
}

/// A Greengrass group: the record `CreateGroup` persists and `GetGroup`,
/// `ListGroups` and `DeleteGroup` read back.
#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub creation_timestamp: String,
    pub last_updated_timestamp: String,
    pub latest_version: Option<String>,
    pub latest_version_arn: Option<String>,
    pub tags: HashMap<String, String>,
    /// Append-only: the order is the group's version history.
    pub versions: Vec<GroupVersionRecord>,
    pub deployments: Vec<DeploymentRecord>,
    /// The IAM role associated with this group. Written by `AssociateRoleToGroup` and
    /// read by `GetAssociatedRole`/`DisassociateRoleFromGroup`, all three of which are
    /// still stubs with their own gap issues — so it stays `None` in this slice and is
    /// not live state yet.
    pub role_arn: Option<String>,
    /// Creation order, for `ListGroups` paging. See
    /// [`ComAmazonawsGreengrassGreengrassState::next_group_seq`].
    pub seq: u64,
}

impl Group {
    /// The `GroupInformation` entry `ListGroups` reports for this group. Note
    /// `GroupInformation` carries no `tags` member (`smithy_query` shape
    /// `GroupInformation`), so a lister returns the group without them.
    pub fn information(&self) -> GroupInformation {
        GroupInformation {
            arn: Some(self.arn.clone()),
            creation_timestamp: Some(self.creation_timestamp.clone()),
            id: Some(self.id.clone()),
            last_updated_timestamp: Some(self.last_updated_timestamp.clone()),
            latest_version: self.latest_version.clone(),
            latest_version_arn: self.latest_version_arn.clone(),
            name: Some(self.name.clone()),
        }
    }

    /// The stored version with this id, if the group holds one.
    pub fn version(&self, version_id: &str) -> Option<&GroupVersionRecord> {
        self.versions.iter().find(|v| v.version == version_id)
    }

    /// The stored deployment with this id, if the group holds one.
    pub fn deployment_mut(&mut self, deployment_id: &str) -> Option<&mut DeploymentRecord> {
        self.deployments
            .iter_mut()
            .find(|d| d.deployment_id == deployment_id)
    }

    /// Every id this group owns: its own, its versions' and its deployments'. What
    /// deleting the group releases.
    pub fn resource_ids(&self) -> HashSet<String> {
        let mut ids = HashSet::with_capacity(1 + self.versions.len() + self.deployments.len());
        ids.insert(self.id.clone());
        ids.extend(self.versions.iter().map(|v| v.version.clone()));
        ids.extend(self.deployments.iter().map(|d| d.deployment_id.clone()));
        ids
    }

    /// Append `record` and advance this group's latest-version pointers and
    /// `LastUpdatedTimestamp` to it.
    pub fn push_version(&mut self, record: GroupVersionRecord) {
        self.latest_version = Some(record.version.clone());
        self.latest_version_arn = Some(record.arn.clone());
        self.last_updated_timestamp = record.creation_timestamp.clone();
        self.versions.push(record);
    }
}

/// One immutable version of a group: the seven component version ARNs, stored
/// verbatim as they arrived.
#[derive(Debug, Clone, Default)]
pub struct GroupVersionRecord {
    pub version: String,
    pub arn: String,
    pub creation_timestamp: String,
    pub core_definition_version_arn: Option<String>,
    pub function_definition_version_arn: Option<String>,
    pub subscription_definition_version_arn: Option<String>,
    pub device_definition_version_arn: Option<String>,
    pub logger_definition_version_arn: Option<String>,
    pub resource_definition_version_arn: Option<String>,
    pub connector_definition_version_arn: Option<String>,
}

impl GroupVersionRecord {
    /// Read the stored ARN set back as the modelled `GroupVersion` that
    /// `GetGroupVersion` returns as its `Definition`.
    pub fn definition(&self) -> GroupVersion {
        GroupVersion {
            connector_definition_version_arn: self.connector_definition_version_arn.clone(),
            core_definition_version_arn: self.core_definition_version_arn.clone(),
            device_definition_version_arn: self.device_definition_version_arn.clone(),
            function_definition_version_arn: self.function_definition_version_arn.clone(),
            logger_definition_version_arn: self.logger_definition_version_arn.clone(),
            resource_definition_version_arn: self.resource_definition_version_arn.clone(),
            subscription_definition_version_arn: self.subscription_definition_version_arn.clone(),
        }
    }

    /// Store a `GroupVersion`'s ARN set verbatim under a freshly minted version.
    pub fn from_definition(
        version: String,
        arn: String,
        creation_timestamp: String,
        definition: &GroupVersion,
    ) -> Self {
        Self {
            version,
            arn,
            creation_timestamp,
            core_definition_version_arn: definition.core_definition_version_arn.clone(),
            function_definition_version_arn: definition.function_definition_version_arn.clone(),
            subscription_definition_version_arn: definition
                .subscription_definition_version_arn
                .clone(),
            device_definition_version_arn: definition.device_definition_version_arn.clone(),
            logger_definition_version_arn: definition.logger_definition_version_arn.clone(),
            resource_definition_version_arn: definition.resource_definition_version_arn.clone(),
            connector_definition_version_arn: definition.connector_definition_version_arn.clone(),
        }
    }
}

/// One deployment raised against a group.
#[derive(Debug, Clone)]
pub struct DeploymentRecord {
    pub deployment_id: String,
    pub deployment_arn: String,
    /// The wire value of the `DeploymentType` enum this deployment was created
    /// with (`smithy_query` shape `DeploymentType`).
    pub deployment_type: String,
    pub group_version_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub error_message: Option<String>,
    pub error_details: Vec<ErrorDetail>,
    /// The statuses the clock still owes this deployment, each with the instant it
    /// falls due. Materialised by [`DeploymentRecord::settle`] — never slept for.
    pub pending_statuses: VecDeque<(Instant, String)>,
}

impl DeploymentRecord {
    /// A deployment that has just been accepted: reported `Building`, with the
    /// clock owing it `InProgress` and then `Success`.
    pub fn accepted(
        deployment_id: String,
        deployment_arn: String,
        deployment_type: String,
        group_version_id: Option<String>,
        timestamp: String,
    ) -> Self {
        let start = Instant::now();
        let pending = VecDeque::from(vec![
            (
                start + DEPLOYMENT_STEP,
                DEPLOYMENT_STATUS_IN_PROGRESS.to_string(),
            ),
            (
                start + DEPLOYMENT_STEP * 2,
                DEPLOYMENT_STATUS_SUCCESS.to_string(),
            ),
        ]);
        Self {
            deployment_id,
            deployment_arn,
            deployment_type,
            group_version_id,
            created_at: timestamp.clone(),
            updated_at: timestamp,
            status: DEPLOYMENT_STATUS_BUILDING.to_string(),
            error_message: None,
            error_details: Vec::new(),
            pending_statuses: pending,
        }
    }

    /// Materialise every status transition the clock has already made due, and
    /// report whether any landed. A status the clock owed at `now` was owed
    /// whether or not anybody asked, so a reporting operation may apply it; the
    /// act of reading never *starts* a transition.
    pub fn settle(&mut self, now: Instant) -> bool {
        let mut advanced = false;
        while self
            .pending_statuses
            .front()
            .is_some_and(|(due, _)| *due <= now)
        {
            if let Some((_, status)) = self.pending_statuses.pop_front() {
                self.status = status;
                advanced = true;
            }
        }
        advanced
    }
}

/// The key a client token is remembered under:
/// [`ComAmazonawsGreengrassGreengrassState::client_tokens`] is shared by every
/// create, so the key names the operation AND the resource the create is scoped to
/// (the parent group id, or the empty string for an account-scoped create). The same
/// token replayed against `CreateGroup` and `CreateGroupVersion`, or against two
/// different groups, names two different resources and must not collide.
pub fn client_token_key(operation: &str, scope: &str, token: &str) -> String {
    format!("{operation}/{scope}/{token}")
}

/// A freshly minted resource id. Greengrass ids are UUIDs, and this is the `uuid`
/// crate's own version-4 generator, reached through `emulator_telemetry`'s UUID
/// newtype (`random()` is `uuid::Uuid::new_v4()` rendered in the canonical hyphenated
/// form). Nothing is hand-rolled here: no byte layout, no version or variant nibbles,
/// no home-made randomness.
///
/// It is reached that way, rather than through a direct `uuid` dependency, because a
/// direct dependency adds a line to the workspace `Cargo.lock` — the one file every
/// in-flight fix in this repository shares. Fifteen submits of this change were
/// approved by every reviewer and refused by the serialized squash merge as
/// "overlapping content", and the lockfile line was the only content it shared with
/// mainline. `emulator_telemetry` is already a dependency of this crate, so the ids
/// stay exactly what they were — fresh random version-4 UUIDs, pinned by
/// `every_minted_id_is_a_unique_version_4_uuid` — while `Cargo.lock` stays
/// byte-identical to mainline's.
pub fn new_id() -> String {
    emulator_telemetry::InstallationId::random()
        .as_str()
        .to_string()
}

/// The current wall-clock instant as the ISO-8601 string Greengrass renders its
/// `CreationTimestamp` / `LastUpdatedTimestamp` / `UpdatedAt` members as.
pub fn now_timestamp() -> String {
    aws_smithy_types::DateTime::from(std::time::SystemTime::now())
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The region the request was signed for, from its SigV4 credential scope, so
/// every ARN this service mints carries the region the caller addressed.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A group's ARN, derived from its stored id:
/// `arn:aws:greengrass:<region>:<account>:/greengrass/groups/<Id>`.
pub fn group_arn(region: &str, group_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("/greengrass/groups/{group_id}"),
    )
}

/// A group version's ARN: `<group arn>/versions/<Version>`.
pub fn group_version_arn(group_arn: &str, version: &str) -> String {
    format!("{group_arn}/versions/{version}")
}

/// A deployment's ARN: `<group arn>/deployments/<DeploymentId>`.
pub fn deployment_arn(group_arn: &str, deployment_id: &str) -> String {
    format!("{group_arn}/deployments/{deployment_id}")
}

/// The service's `BadRequestException` payload carrying `message`.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        error_details: None,
        message: Some(message.into()),
    }
}

/// The service's own modelled server error (HTTP 500) for the one path that cannot be
/// served: [`service_state`] answering `None`, so this request has no store to read.
/// Lives here, beside [`bad_request`], so every handler renders it identically.
pub fn store_unreachable() -> InternalServerErrorException {
    InternalServerErrorException {
        error_details: None,
        message: Some("the greengrass group store is unreachable".to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGreengrassGreengrassState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGreengrassGreengrassState>)
}
