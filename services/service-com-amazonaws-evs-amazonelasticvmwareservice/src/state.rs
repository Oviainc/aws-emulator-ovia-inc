// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-evs-amazonelasticvmwareservice service.
//!
//! One [`StoredEnvironment`] per Amazon EVS environment `CreateEnvironment` minted,
//! keyed by its `environmentId`, each carrying the ten VLAN subnets the service
//! materialised from the request's `initialVlans`. Every operation in this crate
//! computes its answer from this map; nothing is canned.
//!
//! The wire shapes (`Environment`, `EnvironmentSummary`, `Vlan`) are projections of
//! the stored record, built by the `to_*` methods below so that every operation
//! reports one resource identically.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    Check, ConnectivityInfo, Connector, ConnectorCheck, EipAssociation, Environment,
    EnvironmentSummary, Host, LicenseInfo, ResourceNotFoundException,
    ServiceAccessSecurityGroups, ValidationException, VcfHostnames, Vlan, VmEntitlement,
};

/// Whether IAM lets a request to this service perform the action it asks for — the
/// authorization step every implemented operation of this service runs first.
pub mod authorization;

/// What the account's own Amazon EC2 store says about the VPC, subnet and Elastic IP
/// resources an Amazon EVS request names.
pub mod ec2_resources;

/// The `EnvironmentState` / `VlanState` values this service moves a resource through.
/// Both enums carry the same five values (`smithy_query(query="shape",
/// shape="EnvironmentState,VlanState")`).
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_CREATING`].
pub const STATE_CREATED: &str = "CREATED";
/// See [`STATE_CREATING`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_CREATING`].
pub const STATE_DELETED: &str = "DELETED";

/// `CheckResult` values — the shape `environmentStatus` targets.
pub const CHECK_PASSED: &str = "PASSED";
/// See [`CHECK_PASSED`].
pub const CHECK_UNKNOWN: &str = "UNKNOWN";
/// See [`CHECK_PASSED`].
pub const CHECK_FAILED: &str = "FAILED";

/// The `HostState` values a stored host moves through.
///
/// The model declares `CREATING | CREATED | UPDATING | DELETING | DELETED |
/// CREATE_FAILED | UPDATE_FAILED` (`smithy_query(query="shape", shape="HostState")`). The
/// two named here are the ones `CreateEnvironmentHost` produces: a host is `CREATING`
/// while the ESX host is built and `CREATED` once it has joined the environment.
pub const HOST_CREATING: &str = "CREATING";
/// See [`HOST_CREATING`].
pub const HOST_CREATED: &str = "CREATED";

/// The `ValidationExceptionReason` values this service refuses with.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
pub const REASON_OTHER: &str = "other";

/// The resource type `ResourceNotFoundException` reports for an environment: the
/// `EnvironmentResource` the model binds these operations to.
pub const RESOURCE_TYPE_ENVIRONMENT: &str = "environment";

/// The `functionName` of the VLAN that carries ESX host management traffic — the
/// `initialVlans.vmkManagement` member's own name, which is the form the `Vlan` doc
/// gives ("a VLAN with a `functionName` of `hcx`"). It is the subnet a host's
/// management IP comes out of ([`StoredEnvironment::next_host_ip`]).
pub const VMK_MANAGEMENT_FUNCTION: &str = "vmkManagement";

/// The `functionName` of the VLAN that carries VMware HCX traffic, and the ONE value
/// `AssociateEipToVlan` / `DisassociateEipFromVlan` accept as a `vlanName`: "hcx is the
/// only accepted VLAN name at this time" (`vlanName` doc), and those operations are
/// "only allowed for public HCX VLANs at this time" (their own docs).
pub const HCX_FUNCTION: &str = "hcx";

/// The resource type `ResourceNotFoundException` reports for a connector — the other
/// resource these operations address by id (`Connector`, "An object that represents a
/// connector for an Amazon EVS environment").
pub const RESOURCE_TYPE_CONNECTOR: &str = "connector";

/// The `ConnectorState` values a stored connector moves through.
///
/// The model declares `CREATING | CREATE_FAILED | ACTIVE | UPDATING | UPDATE_FAILED |
/// DELETING | DELETED` (`smithy_query(query="shape", shape="ConnectorState")`). The
/// three named here are the ones the implemented connector operations produce: a
/// connector is `CREATING` while `CreateEnvironmentConnector`'s connection is being
/// established, `UPDATING` while `UpdateEnvironmentConnector`'s new metadata is being
/// applied, and `ACTIVE` once either has settled. The rest belong to the operations
/// that produce them (`DeleteEnvironmentConnector` still answers 501).
pub const CONNECTOR_CREATING: &str = "CREATING";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_UPDATING: &str = "UPDATING";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_ACTIVE: &str = "ACTIVE";
/// See [`CONNECTOR_CREATING`].
pub const CONNECTOR_DELETED: &str = "DELETED";

/// The `EntitlementStatus` values a stored entitlement moves through.
///
/// The model declares `CREATING | CREATED | DELETED | AT_RISK | ENTITLEMENT_REMOVED |
/// CREATE_FAILED` (`smithy_query(query="shape", shape="EntitlementStatus")`). The two
/// named here are the ones `CreateEntitlement` produces: an entitlement is `CREATING`
/// while the service validates the virtual machine ("This is an asynchronous
/// operation. Amazon EVS validates the specified virtual machines before starting
/// usage tracking", `CreateEntitlement` doc) and `CREATED` once usage tracking has
/// started. The other values belong to the operations that produce them
/// (`DeleteEntitlement` still answers 501).
pub const ENTITLEMENT_CREATING: &str = "CREATING";
/// See [`ENTITLEMENT_CREATING`].
pub const ENTITLEMENT_CREATED: &str = "CREATED";

/// How long after the call that starts it a lifecycle transition comes due
/// (`CREATING` -> `CREATED` after `CreateEnvironment`, `DELETING` -> `DELETED`
/// after `DeleteEnvironment`, an entitlement's `CREATING` -> `CREATED` after
/// `CreateEntitlement`).
///
/// CHOSEN, not documented: no Amazon EVS document states how long a real
/// environment takes to build, and a real one takes hours. The span is
/// deliberately non-zero so a caller that reads straight after the call still
/// observes the transient state; the due instant is stored and never slept on.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// Milliseconds since the Unix epoch, the form every stored timestamp takes.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// A stored millisecond timestamp as the modelled `Timestamp` type.
pub fn timestamp(ms: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(ms)
}

/// What the top-level `message` of an aggregated `ValidationException` opens with.
pub const VALIDATION_FAILURE_PREFIX: &str = "Request validation failure: ";

/// What joins the `fieldList` entries inside that top-level `message`.
pub const VALIDATION_FAILURE_SEPARATOR: &str = " | ";

/// One `fieldList` entry: the input member that failed a check, and the sentence
/// saying why. The sentence NAMES the member, because the aggregated top-level
/// `message` is built by joining these sentences and nothing else distinguishes them.
pub fn field_failure(member: &str, problem: &str) -> crate::ops::types::ValidationExceptionField {
    crate::ops::types::ValidationExceptionField {
        name: member.to_string(),
        message: format!("{member} {problem}"),
    }
}

/// A `ValidationException` reporting EVERY check a request failed, not just the
/// first: one `fieldList` entry per failure, and a top-level `message` that joins
/// those entries' sentences.
///
/// This is the shape AWS answers a request that breaks several of an operation's
/// preconditions with, so a caller that surfaces `fieldList` can correct all of them
/// in one round trip instead of discovering them one refusal at a time.
pub fn aggregate_validation_error(
    failures: Vec<crate::ops::types::ValidationExceptionField>,
) -> ValidationException {
    debug_assert!(
        !failures.is_empty(),
        "an aggregated ValidationException reports at least one failure"
    );
    let joined = failures
        .iter()
        .map(|failure| failure.message.as_str())
        .collect::<Vec<_>>()
        .join(VALIDATION_FAILURE_SEPARATOR);
    ValidationException {
        message: format!("{VALIDATION_FAILURE_PREFIX}{joined}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(failures),
    }
}

/// A `ValidationException` refusing one input member, naming the member and the
/// constraint it broke the way the boundary validator does. The single-failure case
/// of [`aggregate_validation_error`], so one refused member and ten of them are
/// reported in the same form.
pub fn field_validation_error(member: &str, problem: &str) -> ValidationException {
    aggregate_validation_error(vec![field_failure(member, problem)])
}

/// One `fieldList` entry whose sentence is given IN FULL rather than built by
/// prefixing the member's name: the entry for a check whose own wording already names
/// the member and the value it refuses (the VPC clause below names the `vpcId` it was
/// given). The `name` still carries the member, so a client that reads `fieldList`
/// groups the failure under the member whatever the sentence spells.
pub fn field_failure_stated(
    member: &str,
    message: impl Into<String>,
) -> crate::ops::types::ValidationExceptionField {
    crate::ops::types::ValidationExceptionField {
        name: member.to_string(),
        message: message.into(),
    }
}

/// The failure of the precondition that a host object names at most one placement
/// mechanism: "You cannot use the `dedicatedHostId` and `placementGroupId` parameters
/// together in the same `HostInfoForCreate` object. This results in a
/// `ValidationException` response." (`HostInfoForCreate` doc.)
///
/// The rule is the SHAPE's, not one operation's, so every operation that accepts a
/// `HostInfoForCreate` refuses the pair through this one helper: `CreateEnvironment`
/// (per `hosts` entry) and `CreateEnvironmentHost` (for its single `host`).
pub fn placement_conflict_failure(member: &str) -> crate::ops::types::ValidationExceptionField {
    field_failure(
        member,
        "cannot use dedicatedHostId and placementGroupId together in the same request",
    )
}

/// A `ValidationException` refusing a request that no single member explains.
pub fn request_validation_error(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: REASON_OTHER.to_string(),
        field_list: None,
    }
}

/// The `ResourceNotFoundException` every operation answers for an environment id
/// (or environment ARN) this service holds no record of.
///
/// The message is built from the request: the service names the environment the
/// caller asked for, in the form `Environment <requested id> was not found`, so the
/// text is a function of the lookup key and never of the store's contents.
pub fn environment_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Environment {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: RESOURCE_TYPE_ENVIRONMENT.to_string(),
    }
}

/// The refusal an operation that models NO `ResourceNotFoundException` answers for an
/// environment id this service holds no record of.
///
/// `CreateEnvironmentHost`'s error contract is `ThrottlingException` and
/// `ValidationException` only (`smithy_query(query="operation",
/// operation="CreateEnvironmentHost")`), so a well-formed id that names no environment
/// is carried by the only client-input error it declares. The sentence is built from the
/// request — it names the id the caller asked for and nothing from the store — and the
/// `fieldList` entry names the member that carried it, so a client learns WHICH member
/// was not resolvable.
pub fn environment_not_found_validation(environment_id: &str) -> ValidationException {
    let message = format!("{environment_id} was not found");
    ValidationException {
        message: message.clone(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![field_failure_stated("environmentId", message)]),
    }
}

/// The `ResourceNotFoundException` every operation answers for a connector id the
/// addressed environment holds no record of.
///
/// Built from the request exactly as [`environment_not_found`] is: the service names
/// the connector the caller asked for, in the form `Connector <requested id> was not
/// found`, so the text is a function of the lookup key and never of the store.
pub fn connector_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Connector {resource_id} was not found"),
        resource_id: resource_id.to_string(),
        resource_type: RESOURCE_TYPE_CONNECTOR.to_string(),
    }
}

/// A lifecycle transition the clock owes a stored environment: the instant it
/// becomes due, the wall-clock time to stamp `modifiedAt` with, and the states the
/// environment and its VLANs move to.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// When the transition becomes due. Compared against `Instant::now()`; never slept on.
    pub due_at: Instant,
    /// The wall-clock instant, in epoch milliseconds, the transition is owed at.
    pub due_at_ms: i64,
    /// The `EnvironmentState` the environment moves to.
    pub environment_state: &'static str,
    /// The `VlanState` the environment's VLAN subnets move to.
    pub vlan_state: &'static str,
}

impl PendingTransition {
    /// The transition `CreateEnvironment`/`DeleteEnvironment` starts, due
    /// [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(environment_state: &'static str, vlan_state: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
            environment_state,
            vlan_state,
        }
    }
}

/// The status change the clock owes a stored record of an environment — an
/// entitlement, a connector or a host: the instant it becomes due, the wall-clock time
/// the change is owed at, and the status the record moves to.
///
/// Separate from [`PendingTransition`] because each of those records runs on its own
/// clock: one created long after its environment settled still owes the change the call
/// that created it started.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// When the change becomes due. Compared against `Instant::now()`; never slept on.
    pub due_at: Instant,
    /// The wall-clock instant, in epoch milliseconds, the change is owed at.
    pub due_at_ms: i64,
    /// The status the record moves to — an `EntitlementStatus`, a `ConnectorState` or a
    /// `HostState`, according to the record that carries it.
    pub status: &'static str,
}

impl PendingStatus {
    /// The change a create starts, due [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(status: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
            status,
        }
    }
}

/// The state change the clock owes a stored connector: the instant it becomes due,
/// the wall-clock time the change is owed at, and the `ConnectorState` the connector
/// moves to.
///
/// Separate from [`PendingTransition`] because a connector runs on its own clock: one
/// created or updated long after its environment settled still owes the move to
/// `ACTIVE` that establishing the connection to the appliance takes.
#[derive(Debug, Clone)]
pub struct PendingConnectorState {
    /// When the change becomes due. Compared against `Instant::now()`; never slept on.
    pub due_at: Instant,
    /// The wall-clock instant, in epoch milliseconds, the change is owed at.
    pub due_at_ms: i64,
    /// The `ConnectorState` the connector moves to.
    pub state: &'static str,
}

impl PendingConnectorState {
    /// The change `CreateEnvironmentConnector`/`UpdateEnvironmentConnector` starts,
    /// due [`TRANSITION_DELAY`] after the call.
    pub fn starting_now(state: &'static str) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_DELAY,
            due_at_ms: now_ms() + TRANSITION_DELAY.as_millis() as i64,
            state,
        }
    }
}

/// One connector of an environment, as `CreateEnvironmentConnector` stored it: the
/// record of a connection to one VCF appliance type.
#[derive(Debug, Clone)]
pub struct StoredConnector {
    /// The minted `connectorId` (`cnctr-` + ten alphanumeric characters).
    pub connector_id: String,
    /// The `ConnectorType` the connector was created with. At most one live connector
    /// of each type per environment: "You can create only one connector of each type
    /// per environment" (`CreateEnvironmentConnector` doc).
    pub connector_type: String,
    /// The FQDN of the VCF appliance the connector connects to.
    pub appliance_fqdn: String,
    /// The Secrets Manager secret holding the appliance credentials, EXACTLY as the
    /// request supplied it.
    ///
    /// `secretIdentifier` accepts "The ARN or name of the Amazon Web Services Secrets
    /// Manager secret" and `secretArn` carries that same `SecretIdentifier` shape, so
    /// a name is reported as a name: this emulator runs no Secrets Manager and cannot
    /// resolve a name to the ARN of a secret it does not hold.
    pub secret_arn: String,
    /// The `ConnectorState` the connector is in.
    pub state: String,
    /// A detailed description of `state`, when there is one.
    pub state_details: Option<String>,
    /// The checks run on the connector, which [`Self::status`] rolls up.
    ///
    /// Always empty here: a connector check is a probe of the appliance named by
    /// `applianceFqdn`, and this emulator contacts no appliance, so no check has run.
    pub checks: Vec<ConnectorCheck>,
    /// Epoch milliseconds the connector was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the connector was last modified at.
    pub modified_at_ms: i64,
    /// The state change the clock owes this connector, if any.
    pub pending: Option<PendingConnectorState>,
}

impl StoredConnector {
    /// A new connector, as `CreateEnvironmentConnector` mints one: `CREATING` until
    /// the move to `ACTIVE` the clock owes it comes due.
    pub fn creating(
        connector_id: String,
        connector_type: &str,
        appliance_fqdn: &str,
        secret_identifier: &str,
        created_at_ms: i64,
        pending: PendingConnectorState,
    ) -> Self {
        Self {
            connector_id,
            connector_type: connector_type.to_string(),
            appliance_fqdn: appliance_fqdn.to_string(),
            secret_arn: secret_identifier.to_string(),
            state: CONNECTOR_CREATING.to_string(),
            state_details: None,
            checks: Vec::new(),
            created_at_ms,
            modified_at_ms: created_at_ms,
            pending: Some(pending),
        }
    }

    /// Apply the owed state change if `now` has reached its due instant.
    ///
    /// The change was owed at that instant whether or not anybody called, so a
    /// reporting operation may materialise it. The stamp it writes is the stored due
    /// instant and never the reading instant, so two reads report one `modifiedAt`.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.state = pending.state.to_string();
        self.modified_at_ms = pending.due_at_ms;
        self.pending = None;
    }

    /// `status`: the roll-up of the connector's `checks`, which identify "connectivity
    /// health". A failed check is `FAILED`; a connector no check has yet run on, or
    /// one that is not `ACTIVE` so its checks cannot have run, is `UNKNOWN`; anything
    /// else is `PASSED`.
    ///
    /// Nothing in this emulator probes an appliance, so `checks` is empty and this
    /// reports `UNKNOWN`: `PASSED` would claim a reachability check nothing performed.
    pub fn status(&self) -> &'static str {
        if self
            .checks
            .iter()
            .any(|check| check.result.as_deref() == Some(CHECK_FAILED))
        {
            CHECK_FAILED
        } else if self.checks.is_empty() || self.state != CONNECTOR_ACTIVE {
            CHECK_UNKNOWN
        } else {
            CHECK_PASSED
        }
    }

    /// This connector as the modelled `Connector` shape, reported for the environment
    /// that holds it.
    pub fn to_wire(&self, environment_id: &str) -> Connector {
        Connector {
            environment_id: Some(environment_id.to_string()),
            connector_id: Some(self.connector_id.clone()),
            r#type: Some(self.connector_type.clone()),
            appliance_fqdn: Some(self.appliance_fqdn.clone()),
            secret_arn: Some(self.secret_arn.clone()),
            state: Some(self.state.clone()),
            state_details: self.state_details.clone(),
            status: Some(self.status().to_string()),
            checks: (!self.checks.is_empty()).then(|| self.checks.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
        }
    }
}

/// The store key of one entitlement inside its environment: the connector it was
/// created through and the virtual machine it entitles.
///
/// That pair is what identifies an entitlement — `CreateEntitlement`,
/// `ListVmEntitlements` and `DeleteEntitlement` all address entitlements by
/// `connectorId` plus `vmIds`, and `CreateEntitlement` is `@idempotent`, so a second
/// create for the same pair must find the same record. `/` separates the two parts
/// because neither `^(cnctr-[a-zA-Z0-9]{10})$` nor `^vm-[0-9]+$` can contain one, so
/// no two distinct pairs share a key.
pub fn entitlement_key(connector_id: &str, vm_id: &str) -> String {
    format!("{connector_id}/{vm_id}")
}

/// One Windows Server License entitlement, as `CreateEntitlement` stored it: the
/// record of a virtual machine whose Windows Server usage Amazon EVS tracks.
#[derive(Debug, Clone)]
pub struct StoredEntitlement {
    /// The VMware vSphere virtual machine managed object ID the entitlement covers.
    pub vm_id: String,
    /// The connector the entitlement was created through.
    pub connector_id: String,
    /// The `EntitlementType` the entitlement was created with — the model declares
    /// only `WINDOWS_SERVER`.
    pub entitlement_type: String,
    /// The `EntitlementStatus` the entitlement is in.
    pub status: String,
    /// Epoch milliseconds the entitlement record was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds usage tracking started at ("The date and time that the
    /// entitlement started", `startedAt` doc): set when the entitlement reaches
    /// `CREATED`, and absent while the virtual machine is still being validated.
    pub started_at_ms: Option<i64>,
    /// The status change the clock owes this entitlement, if any.
    pub pending: Option<PendingStatus>,
}

impl StoredEntitlement {
    /// A new entitlement for `vm_id`, as `CreateEntitlement` mints one: `CREATING`
    /// until the validation the clock owes it comes due.
    pub fn creating(
        connector_id: &str,
        vm_id: &str,
        entitlement_type: &str,
        created_at_ms: i64,
        pending: PendingStatus,
    ) -> Self {
        Self {
            vm_id: vm_id.to_string(),
            connector_id: connector_id.to_string(),
            entitlement_type: entitlement_type.to_string(),
            status: ENTITLEMENT_CREATING.to_string(),
            created_at_ms,
            started_at_ms: None,
            pending: Some(pending),
        }
    }

    /// Apply the owed status change if `now` has reached its due instant.
    ///
    /// The change was owed at that instant whether or not anybody called, so a
    /// reporting operation may materialise it. The stamp it writes is the stored due
    /// instant and never the reading instant, so two reads report the same `startedAt`.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.status = pending.status.to_string();
        if pending.status == ENTITLEMENT_CREATED {
            // Usage tracking starts when the entitlement is created, which is the
            // instant the asynchronous validation was owed at.
            self.started_at_ms = Some(pending.due_at_ms);
        }
        self.pending = None;
    }

    /// This entitlement as the modelled `VmEntitlement` shape, reported for the
    /// environment that holds it.
    ///
    /// Three members are deliberately absent, because nothing in this emulator
    /// produces a value for them:
    ///
    /// * `vmName` — a virtual machine's name lives in the vCenter inventory, which
    ///   this emulator does not run, so there is nothing to read it from.
    /// * `lastSyncedAt` — no vCenter is contacted, so no sync has happened; a
    ///   timestamp here would claim work nothing performed.
    /// * `stoppedAt` and `errorDetail` — usage tracking has not stopped and no
    ///   entitlement has failed: `DeleteEntitlement`, the operation that stops it,
    ///   still answers 501.
    pub fn to_wire(&self, environment_id: &str) -> VmEntitlement {
        VmEntitlement {
            vm_id: Some(self.vm_id.clone()),
            environment_id: Some(environment_id.to_string()),
            connector_id: Some(self.connector_id.clone()),
            vm_name: None,
            r#type: Some(self.entitlement_type.clone()),
            status: Some(self.status.clone()),
            last_synced_at: None,
            started_at: self.started_at_ms.map(timestamp),
            stopped_at: None,
            error_detail: None,
        }
    }
}

/// One ESX host of an environment, as `CreateEnvironmentHost` stored it.
#[derive(Debug, Clone)]
pub struct StoredHost {
    /// The DNS hostname the request gave the host, unique across environments.
    pub host_name: String,
    /// The management IP address the host took out of the environment's
    /// `vmkManagement` VLAN subnet ([`StoredEnvironment::next_host_ip`]).
    pub ip_address: String,
    /// The SSH key the host is reached with.
    pub key_name: String,
    /// The EC2 bare metal instance type the host runs on.
    pub instance_type: String,
    /// The placement group the host was placed in, when the request named one.
    pub placement_group_id: Option<String>,
    /// The EC2 Dedicated Host the host was placed on, when the request named one.
    pub dedicated_host_id: Option<String>,
    /// The ESX version the request asked for, when it named one.
    ///
    /// Kept as supplied so caller input is not discarded, and reported by nothing: the
    /// modelled `Host` shape carries no ESX version member, and the catalogue that
    /// would resolve the default version for an environment's VCF version is
    /// `GetVersions`, which still answers 501.
    pub esx_version: Option<String>,
    /// The `HostState` the host is in.
    pub state: String,
    /// Epoch milliseconds the host was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the host was last modified at.
    pub modified_at_ms: i64,
    /// The state change the clock owes this host, if any.
    pub pending: Option<PendingStatus>,
}

impl StoredHost {
    /// A new host, as `CreateEnvironmentHost` mints one: `CREATING` until the build the
    /// clock owes it comes due.
    pub fn creating(
        host: &crate::ops::types::HostInfoForCreate,
        ip_address: String,
        esx_version: Option<String>,
        created_at_ms: i64,
        pending: PendingStatus,
    ) -> Self {
        Self {
            host_name: host.host_name.clone(),
            ip_address,
            key_name: host.key_name.clone(),
            instance_type: host.instance_type.clone(),
            placement_group_id: host.placement_group_id.clone(),
            dedicated_host_id: host.dedicated_host_id.clone(),
            esx_version,
            state: HOST_CREATING.to_string(),
            created_at_ms,
            modified_at_ms: created_at_ms,
            pending: Some(pending),
        }
    }

    /// Apply the owed state change if `now` has reached its due instant — the host
    /// counterpart of [`StoredConnector::materialise_due_transition`], and owed on the
    /// same terms.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) else {
            return;
        };
        self.state = pending.status.to_string();
        self.modified_at_ms = pending.due_at_ms;
        self.pending = None;
    }

    /// This host as the modelled `Host` shape.
    ///
    /// Two members are deliberately absent, because nothing in this emulator produces a
    /// value for them: `ec2InstanceId` and `networkInterfaces` name the EC2 instance and
    /// the elastic network interfaces Amazon EVS launches for the host, and this service
    /// launches none — reporting an instance id no EC2 store holds would name a
    /// resource that does not exist.
    pub fn to_wire(&self) -> Host {
        Host {
            host_name: Some(self.host_name.clone()),
            ip_address: Some(self.ip_address.clone()),
            key_name: Some(self.key_name.clone()),
            instance_type: Some(self.instance_type.clone()),
            placement_group_id: self.placement_group_id.clone(),
            dedicated_host_id: self.dedicated_host_id.clone(),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            host_state: Some(self.state.clone()),
            state_details: None,
            ec2_instance_id: None,
            network_interfaces: None,
        }
    }
}

/// One IPv4 CIDR block, reduced to the inclusive address range it covers.
///
/// Hand-written rather than taken from `ipnet`, which the workspace already resolves:
/// the three rules this service needs are a mask, a range comparison and an offset into
/// the block, and the offset — the addresses AWS leaves usable in a subnet — is not
/// something a general CIDR crate expresses. Switching the parse and the overlap onto
/// `ipnet::Ipv4Net` is a behaviour-neutral follow-up.
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Cidr {
    /// The first address of the block (the network address).
    pub first: u32,
    /// The last address of the block (the broadcast address).
    pub last: u32,
    /// The block's prefix length, in bits.
    pub prefix_len: u8,
}

impl Ipv4Cidr {
    /// Parse `a.b.c.d/<prefix length>`; `None` for anything that is not an IPv4 CIDR
    /// block.
    pub fn parse(cidr: &str) -> Option<Self> {
        let (address, prefix_len) = cidr.split_once('/')?;
        let address: ::std::net::Ipv4Addr = address.parse().ok()?;
        let prefix_len: u8 = prefix_len.parse().ok()?;
        if prefix_len > 32 {
            return None;
        }
        // The host bits below the prefix are what the block spans: clearing them gives
        // its first address, setting them its last.
        let host_bits = 32 - u32::from(prefix_len);
        let mask = if host_bits == 32 { 0 } else { u32::MAX << host_bits };
        let first = u32::from(address) & mask;
        Some(Self {
            first,
            last: first | !mask,
            prefix_len,
        })
    }

    /// Whether the two blocks share any address.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.first <= other.last && other.first <= self.last
    }

    /// Every address of the block a workload may take, in order.
    ///
    /// "The first four IP addresses and the last IP address in each subnet CIDR block
    /// are not available for you to use" (Amazon VPC User Guide, subnet sizing), so the
    /// usable range starts four addresses into the block and stops one short of its end.
    /// Empty for a block too small to hold any.
    pub fn usable_addresses(&self) -> impl Iterator<Item = ::std::net::Ipv4Addr> {
        const RESERVED_AT_START: u32 = 4;
        let first = self.first.saturating_add(RESERVED_AT_START);
        let last = self.last.saturating_sub(1);
        // A block too small to hold a usable address yields nothing: the count is taken
        // first, so the range is never walked backwards.
        let count = last.saturating_sub(first).saturating_add(1) as usize;
        let count = if first <= last { count } else { 0 };
        (first..)
            .take(count)
            .map(::std::net::Ipv4Addr::from)
    }
}

/// One VLAN subnet of an environment, materialised from an `initialVlans` entry.
#[derive(Debug, Clone)]
pub struct StoredVlan {
    /// The 802.1Q VLAN tag the environment carries this traffic type on.
    pub vlan_id: i32,
    /// The CIDR block the request supplied for this VLAN subnet.
    pub cidr: String,
    /// The VCF traffic type carried over the VLAN — the `initialVlans` member name.
    pub function_name: String,
    /// The subnet this service minted for the VLAN.
    pub subnet_id: String,
    /// The `VlanState` of the VLAN subnet.
    pub vlan_state: String,
    /// Whether the VLAN is public. Only the HCX VLAN can be, via `isHcxPublic`.
    pub is_public: bool,
    /// The network ACL the VLAN uses, when the request named one.
    pub network_acl_id: Option<String>,
    /// Epoch milliseconds the VLAN was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the VLAN was last modified at.
    pub modified_at_ms: i64,
    /// Elastic IP associations, which `AssociateEipToVlan` adds to a public VLAN.
    pub eip_associations: Vec<EipAssociation>,
}

impl StoredVlan {
    /// This VLAN as the modelled `Vlan` shape.
    ///
    /// `availabilityZone` is left absent: the AZ of a VLAN subnet is the AZ of the
    /// EC2 subnet the VPC provides, and this emulator holds no EC2 state to read it
    /// from — reporting a guess would be worse than reporting nothing.
    pub fn to_wire(&self) -> Vlan {
        Vlan {
            vlan_id: Some(self.vlan_id),
            cidr: Some(self.cidr.clone()),
            availability_zone: None,
            function_name: Some(self.function_name.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            vlan_state: Some(self.vlan_state.clone()),
            state_details: None,
            eip_associations: Some(self.eip_associations.clone()),
            is_public: Some(self.is_public),
            network_acl_id: self.network_acl_id.clone(),
        }
    }
}

/// One Amazon EVS environment, as `CreateEnvironment` stored it.
#[derive(Debug, Clone)]
pub struct StoredEnvironment {
    /// The minted `environmentId` (`env-` + ten alphanumeric characters).
    pub environment_id: String,
    /// The Region the environment was created in — the Region of the creating
    /// request's SigV4 credential scope, and the scope its name is unique within.
    pub region: String,
    /// The environment's ARN, the handle the tag operations address it by.
    pub environment_arn: String,
    /// The name the request gave the environment, unique per account and Region.
    pub environment_name: Option<String>,
    /// The `EnvironmentState` the environment is in.
    pub state: String,
    /// A detailed description of `state`, when there is one.
    pub state_details: Option<String>,
    /// Epoch milliseconds the environment was created at.
    pub created_at_ms: i64,
    /// Epoch milliseconds the environment was last modified at.
    pub modified_at_ms: i64,
    /// The VPC the environment is deployed inside.
    pub vpc_id: String,
    /// The subnet the Amazon EVS control plane reaches the environment through.
    pub service_access_subnet_id: String,
    /// The environment's `VcfVersion`.
    pub vcf_version: String,
    /// The caller's confirmation that it holds the required VCF licenses.
    pub terms_accepted: bool,
    /// The KMS key the VCF credential secrets are encrypted with, when given.
    pub kms_key_id: Option<String>,
    /// The Broadcom Site ID, when given.
    pub site_id: Option<String>,
    /// The VCF license keys, when given.
    pub license_info: Vec<LicenseInfo>,
    /// The route server peerings the environment's NSX uplink uses, when given.
    pub connectivity_info: Option<ConnectivityInfo>,
    /// The DNS hostnames of the VCF management appliances, when given.
    pub vcf_hostnames: Option<VcfHostnames>,
    /// The security groups guarding control-plane access, when given.
    pub service_access_security_groups: Option<ServiceAccessSecurityGroups>,
    /// The environment's tags, as the tag operations and `CreateEnvironment` set them.
    pub tags: BTreeMap<String, String>,
    /// The VLAN subnets materialised from `initialVlans`.
    pub vlans: Vec<StoredVlan>,
    /// The ESX hosts the create request asked for, kept as supplied.
    ///
    /// `CreateEnvironment` accepts them, so losing them would discard caller input;
    /// materialising them into `Host` records is the host slice's work
    /// (`CreateEnvironmentHost` / `ListEnvironmentHosts` still answer 501), so nothing
    /// reports them yet.
    pub requested_hosts: Vec<crate::ops::types::HostInfoForCreate>,
    /// Environment checks, the roll-up [`Self::environment_status`] reports.
    pub checks: Vec<Check>,
    /// The Windows Server License entitlements created in this environment, keyed by
    /// [`entitlement_key`]. A `BTreeMap` so `ListVmEntitlements` has one stable order
    /// to page over, and bounded by [`MAX_ENTITLEMENTS_PER_ENVIRONMENT`].
    pub entitlements: BTreeMap<String, StoredEntitlement>,
    /// The connectors created in this environment, keyed by `connectorId`. A
    /// `BTreeMap` so `ListEnvironmentConnectors` has one stable order to page over.
    ///
    /// Bounded by the connector rule itself rather than by a cap of its own: at most
    /// one LIVE connector of each `ConnectorType` exists per environment ("You can
    /// create only one connector of each type per environment",
    /// `CreateEnvironmentConnector` doc), so the map holds at most as many live
    /// records as the model declares types.
    pub connectors: BTreeMap<String, StoredConnector>,
    /// The ESX hosts added to this environment, keyed by `hostName` — the handle a host
    /// is addressed by, and unique across environments. A `BTreeMap` so
    /// `ListEnvironmentHosts` has one stable order to page over; bounded by the usable
    /// addresses of the environment's own `vmkManagement` VLAN subnet
    /// ([`Self::next_host_ip`]).
    pub hosts: BTreeMap<String, StoredHost>,
    /// The lifecycle transition the clock owes this environment, if any.
    pub pending: Option<PendingTransition>,
}

impl StoredEnvironment {
    /// Apply the owed lifecycle transition if `now` has reached its due instant, and
    /// the status change every entitlement of this environment is owed with it.
    ///
    /// The transition was owed at that instant whether or not anybody called, so a
    /// reporting operation may materialise it: it only makes a change the clock had
    /// already made due visible.
    pub fn materialise_due_transition(&mut self, now: Instant) {
        if let Some(pending) = self.pending.clone().filter(|pending| now >= pending.due_at) {
            self.state = pending.environment_state.to_string();
            self.modified_at_ms = pending.due_at_ms;
            for vlan in &mut self.vlans {
                vlan.vlan_state = pending.vlan_state.to_string();
                vlan.modified_at_ms = pending.due_at_ms;
            }
            self.pending = None;
        }
        // Each entitlement, connector and host runs on its own clock: one created after
        // the environment settled has no environment transition left to ride on.
        for entitlement in self.entitlements.values_mut() {
            entitlement.materialise_due_transition(now);
        }
        // A connector and a host each run on their own clock for the same reason.
        for connector in self.connectors.values_mut() {
            connector.materialise_due_transition(now);
        }
        for host in self.hosts.values_mut() {
            host.materialise_due_transition(now);
        }
    }

    /// The earliest instant at which this environment, or one of its entitlements,
    /// connectors or hosts, still owes a change — `None` when nothing is owed.
    pub fn next_transition_at(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|pending| pending.due_at)
            .chain(
                self.entitlements
                    .values()
                    .filter_map(|entitlement| entitlement.pending.as_ref())
                    .map(|pending| pending.due_at),
            )
            .chain(
                self.connectors
                    .values()
                    .filter_map(|connector| connector.pending.as_ref())
                    .map(|pending| pending.due_at),
            )
            .chain(
                self.hosts
                    .values()
                    .filter_map(|host| host.pending.as_ref())
                    .map(|pending| pending.due_at),
            )
            .min()
    }

    /// The management IP address the next host of this environment takes: the lowest
    /// usable address of the `vmkManagement` VLAN subnet that no host of this
    /// environment already holds.
    ///
    /// An ESX host's management interface lives on the `vmkManagement` VLAN — that is
    /// the traffic type the VLAN carries (`Vlan$functionName` doc) — so the address is
    /// COMPUTED from the environment's own VLAN plan rather than invented, and the
    /// subnet's usable range is therefore also what bounds how many hosts one
    /// environment can hold: `None` means the subnet is full and a further host cannot
    /// be addressed.
    pub fn next_host_ip(&self) -> Option<String> {
        let vmk = self
            .vlans
            .iter()
            .find(|vlan| vlan.function_name == VMK_MANAGEMENT_FUNCTION)?;
        let taken: Vec<&str> = self
            .hosts
            .values()
            .map(|host| host.ip_address.as_str())
            .collect();
        Ipv4Cidr::parse(&vmk.cidr)?
            .usable_addresses()
            .map(|address| address.to_string())
            .find(|address| !taken.contains(&address.as_str()))
    }

    /// `environmentStatus`: the roll-up of the environment's `checks`, which
    /// "reports impaired functionality that stems from issues internal to the
    /// environment". A failed check is `FAILED`; an environment whose checks cannot
    /// have run yet (it is not `CREATED`) is `UNKNOWN`; anything else is `PASSED`.
    pub fn environment_status(&self) -> &'static str {
        if self
            .checks
            .iter()
            .any(|check| check.result.as_deref() == Some(CHECK_FAILED))
        {
            CHECK_FAILED
        } else if self.state != STATE_CREATED {
            CHECK_UNKNOWN
        } else {
            CHECK_PASSED
        }
    }

    /// This environment as the modelled `Environment` shape.
    ///
    /// `credentials` is left absent: the VCF credential secrets are Secrets Manager
    /// secrets Amazon EVS creates during installation, and this emulator does not
    /// create them, so there is no secret ARN to report.
    pub fn to_wire(&self) -> Environment {
        Environment {
            environment_id: Some(self.environment_id.clone()),
            environment_state: Some(self.state.clone()),
            state_details: self.state_details.clone(),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            environment_arn: Some(self.environment_arn.clone()),
            environment_name: self.environment_name.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            service_access_subnet_id: Some(self.service_access_subnet_id.clone()),
            vcf_version: Some(self.vcf_version.clone()),
            terms_accepted: Some(self.terms_accepted),
            license_info: (!self.license_info.is_empty()).then(|| self.license_info.clone()),
            site_id: self.site_id.clone(),
            environment_status: Some(self.environment_status().to_string()),
            checks: (!self.checks.is_empty()).then(|| self.checks.clone()),
            connectivity_info: self.connectivity_info.clone(),
            vcf_hostnames: self.vcf_hostnames.clone(),
            kms_key_id: self.kms_key_id.clone(),
            service_access_security_groups: self.service_access_security_groups.clone(),
            credentials: None,
        }
    }

    /// This environment as the modelled `EnvironmentSummary` shape, the item
    /// `ListEnvironments` pages over.
    pub fn to_summary(&self) -> EnvironmentSummary {
        EnvironmentSummary {
            environment_id: Some(self.environment_id.clone()),
            environment_name: self.environment_name.clone(),
            vcf_version: Some(self.vcf_version.clone()),
            environment_status: Some(self.environment_status().to_string()),
            environment_state: Some(self.state.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            modified_at: Some(timestamp(self.modified_at_ms)),
            environment_arn: Some(self.environment_arn.clone()),
        }
    }
}

/// The most environments this emulator holds — the cap that bounds caller-driven
/// growth of the store.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// `CreateEnvironment` is the only thing that adds one, so the store needs a maximum
/// entry count: a TTL is not available to it, because `GetEnvironment` must still
/// answer what `CreateEnvironment` created. Past this cap a create is refused with the
/// service's own `ServiceQuotaExceededException` (HTTP 402), never by evicting a live
/// record. The cap also bounds the work
/// [`ComAmazonawsEvsAmazonelasticvmwareserviceState::materialise_due_transitions`] can
/// do while holding the state lock.
///
/// RETENTION RULE for the one record kind that is finished with: an environment that
/// has reached `DELETED` is kept so a later read still reports the deletion, but only
/// until the store needs the room — at the cap, the longest-deleted record is purged
/// to admit a new environment, and a read of a purged id answers
/// `ResourceNotFoundException`, which that error's own doc sanctions ("The resource
/// might not be specified correctly, or it may have a `state` of `DELETED`"). A
/// create/delete loop therefore cannot grow the map past the cap.
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes its
/// environments-per-Region quota as adjustable, so no single value is the published
/// one, and this is far above any ordinary workflow.
pub const MAX_ENVIRONMENTS: usize = 256;

/// The most entitlements one environment holds — the cap that bounds caller-driven
/// growth of an environment's entitlement map.
///
/// `vmIds` is capped at 100 per `CreateEntitlement` call by the model, but nothing
/// bounds the number of calls, so the store needs a maximum of its own: past this cap
/// a create is refused with the operation's modelled `ValidationException`, never by
/// evicting a live record (`DeleteEntitlement` is what removes one).
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes no
/// per-environment entitlement quota, and this is far above the VM count of any
/// ordinary workflow.
pub const MAX_ENTITLEMENTS_PER_ENVIRONMENT: usize = 1024;

/// The most Elastic IP associations one VLAN subnet holds — the cap that bounds
/// caller-driven growth of a VLAN's association list.
///
/// `AssociateEipToVlan` is the only thing that adds one and the CALLER chooses the
/// `allocationId`, so distinct ids would otherwise grow the list without limit, and
/// every response that reports the VLAN clones the whole list back out. Past this cap
/// an associate is refused with the operation's modelled `ValidationException`, never by
/// dropping an association the caller still holds — `DisassociateEipFromVlan` is what
/// removes one.
///
/// The number is the emulator's own bound, not a contract: Amazon EVS publishes no
/// per-VLAN Elastic IP quota, and an account's own Elastic IP quota (five per Region by
/// default, adjustable) is far below it.
pub const MAX_EIP_ASSOCIATIONS_PER_VLAN: usize = 64;

/// Per-service state for com-amazonaws-evs-amazonelasticvmwareservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEvsAmazonelasticvmwareserviceState {
    /// Every environment this service holds, keyed by `environmentId`. A `BTreeMap`
    /// so `ListEnvironments` has one stable order to page over. Bounded by
    /// [`MAX_ENVIRONMENTS`], which counts every record, `DELETED` ones included.
    pub environments: BTreeMap<String, StoredEnvironment>,
    /// The earliest instant at which any stored environment has a transition due, or
    /// `None` when none has. [`Self::materialise_due_transitions`] returns without
    /// looking at a single record until that instant arrives, so a request pays O(1)
    /// for the clock except on the calls where a transition really is due.
    next_transition_at: Option<Instant>,
    /// Counts the ids minted so far, so no two resources share one.
    next_id_seed: u64,
    /// Mixed into every minted id so ids are not guessable from a sequence. Seeded
    /// on first use (a zero salt means "not seeded yet").
    id_salt: u64,
}

impl ComAmazonawsEvsAmazonelasticvmwareserviceState {
    /// The next never-before-used id seed.
    fn next_seed(&mut self) -> u64 {
        if self.id_salt == 0 {
            // Seeded from the clock, with the golden-ratio odd constant mixed in so a
            // coarse clock still gives a salt with bits set across the word.
            self.id_salt = (now_ms() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        }
        self.next_id_seed += 1;
        self.next_id_seed
    }

    /// Mint an `environmentId`: `env-` + exactly ten lowercase hex characters, which
    /// satisfies the modelled `^(env-[a-zA-Z0-9]{10})$`. Distinct seeds XOR to
    /// distinct 40-bit values, so no two environments can collide.
    pub fn mint_environment_id(&mut self) -> String {
        let seed = self.next_seed();
        let value = (self.id_salt ^ seed) & 0xFF_FFFF_FFFF;
        format!("env-{value:010x}")
    }

    /// Mint a `connectorId`: `cnctr-` + exactly ten lowercase hex characters, which
    /// satisfies the modelled `^(cnctr-[a-zA-Z0-9]{10})$`. Distinct seeds XOR to
    /// distinct 40-bit values, so no two connectors can collide.
    pub fn mint_connector_id(&mut self) -> String {
        let seed = self.next_seed();
        let value = (self.id_salt ^ seed) & 0xFF_FFFF_FFFF;
        format!("cnctr-{value:010x}")
    }

    /// Mint a subnet id: `subnet-` + exactly 17 lowercase hex characters, the long
    /// form of the modelled `^subnet-[a-f0-9]{8}([a-f0-9]{9})?$`.
    pub fn mint_subnet_id(&mut self) -> String {
        let seed = u128::from(self.next_seed());
        let value = ((u128::from(self.id_salt) << 4) ^ seed) & ((1u128 << 68) - 1);
        format!("subnet-{value:017x}")
    }

    /// Mint an Elastic IP association id: `eipassoc-` + 17 lowercase hex characters,
    /// which satisfies the modelled `^eipassoc-[a-zA-Z0-9_-]+$` within its 9..26 length.
    pub fn mint_eip_association_id(&mut self) -> String {
        let seed = u128::from(self.next_seed());
        let value = ((u128::from(self.id_salt) << 4) ^ seed) & ((1u128 << 68) - 1);
        format!("eipassoc-{value:017x}")
    }

    /// Apply every lifecycle transition the clock has made due, so a reporting
    /// operation answers with the states that are owed at this instant.
    ///
    /// O(1) until the earliest due instant arrives: only then is the store walked, and
    /// the walk is bounded by [`MAX_ENVIRONMENTS`].
    pub fn materialise_due_transitions(&mut self) {
        let now = Instant::now();
        if self.next_transition_at.is_none_or(|due_at| now < due_at) {
            return;
        }
        let mut next_transition_at: Option<Instant> = None;
        for environment in self.environments.values_mut() {
            environment.materialise_due_transition(now);
            if let Some(due_at) = environment.next_transition_at() {
                next_transition_at = Some(match next_transition_at {
                    Some(earliest) => earliest.min(due_at),
                    None => due_at,
                });
            }
        }
        self.next_transition_at = next_transition_at;
    }

    /// Record that a transition is owed at `due_at`, so
    /// [`Self::materialise_due_transitions`] knows when to look. An operation that
    /// stores a [`PendingTransition`] must call this with its due instant.
    pub fn note_transition_due(&mut self, due_at: Instant) {
        self.next_transition_at = Some(match self.next_transition_at {
            Some(earliest) => earliest.min(due_at),
            None => due_at,
        });
    }

    /// Make room for one more environment, and report whether there is any.
    ///
    /// Below [`MAX_ENVIRONMENTS`] there is nothing to do. At the cap, the retention
    /// rule documented on that constant applies: the longest-deleted record is purged
    /// to admit the new environment, and only a store whose every record is still live
    /// answers `false` — a live record is never evicted, because a later read must
    /// still report it.
    ///
    /// Call it after [`Self::materialise_due_transitions`], so an environment whose
    /// deletion has come due counts as deleted here.
    pub fn reserve_environment_slot(&mut self) -> bool {
        if self.environments.len() < MAX_ENVIRONMENTS {
            return true;
        }
        let longest_deleted = self
            .environments
            .values()
            .filter(|environment| environment.state == STATE_DELETED)
            .min_by(|left, right| {
                (left.modified_at_ms, left.environment_id.as_str())
                    .cmp(&(right.modified_at_ms, right.environment_id.as_str()))
            })
            .map(|environment| environment.environment_id.clone());
        match longest_deleted {
            Some(environment_id) => {
                self.environments.remove(&environment_id);
                true
            }
            None => false,
        }
    }

    /// The environment `environment_id` names, if this service holds it IN `region`.
    ///
    /// An Amazon EVS environment is a regional resource: it exists at the endpoint of
    /// the Region it was created in and nowhere else, so a lookup from another Region
    /// is a miss, exactly as the ARN this service minted for it says.
    pub fn environment_in_region(
        &self,
        region: &str,
        environment_id: &str,
    ) -> Option<&StoredEnvironment> {
        self.environments
            .get(environment_id)
            .filter(|environment| environment.region == region)
    }

    /// Mutable form of [`Self::environment_in_region`].
    pub fn environment_in_region_mut(
        &mut self,
        region: &str,
        environment_id: &str,
    ) -> Option<&mut StoredEnvironment> {
        self.environments
            .get_mut(environment_id)
            .filter(|environment| environment.region == region)
    }

    /// The environment an ARN names, if this service holds it. The tag operations
    /// address an environment by ARN rather than by id; the ARN carries the Region, so
    /// matching it is already Region-scoped.
    pub fn environment_by_arn(&self, resource_arn: &str) -> Option<&StoredEnvironment> {
        self.environments
            .values()
            .find(|environment| environment.environment_arn == resource_arn)
    }

    /// Mutable form of [`Self::environment_by_arn`].
    pub fn environment_by_arn_mut(&mut self, resource_arn: &str) -> Option<&mut StoredEnvironment> {
        self.environments
            .values_mut()
            .find(|environment| environment.environment_arn == resource_arn)
    }

    /// Whether some environment IN `region` already carries `name`. `CreateEnvironment`
    /// refuses a duplicate: "The name must be unique within the Amazon Web Services
    /// Region and Amazon Web Services account that you're creating the environment in."
    /// One account is emulated, so the Region is the whole scope; a name freed by a
    /// deleted environment is available again.
    pub fn name_is_taken(&self, region: &str, name: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.region == region
                && environment.environment_name.as_deref() == Some(name)
                && environment.state != STATE_DELETED
        })
    }

    /// Whether some environment IN `region` is already deployed inside `vpc_id`.
    /// `CreateEnvironment` refuses a second one: "The VPC cannot be used with any
    /// other deployed Amazon EVS environment. Amazon EVS does not provide multi-VPC
    /// support for environments at this time." (`vpcId` doc). A VPC is a regional
    /// resource, so the Region is the scope, and a VPC freed by a deleted environment
    /// is usable again.
    pub fn vpc_is_in_use(&self, region: &str, vpc_id: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.region == region
                && environment.vpc_id == vpc_id
                && environment.state != STATE_DELETED
        })
    }

    /// Whether some host of some environment this service holds already carries
    /// `host_name`: "DNS hostnames for hosts must be unique across Amazon EVS
    /// environments and within VCF" (`HostInfoForCreate$hostName` doc), so the scope is
    /// every environment and not one — and not one Region either, since the uniqueness
    /// the doc states is within VCF rather than within an endpoint.
    pub fn host_name_is_taken(&self, host_name: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.state != STATE_DELETED && environment.hosts.contains_key(host_name)
        })
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEvsAmazonelasticvmwareserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEvsAmazonelasticvmwareserviceState>)
}

/// The Region an operation works in: the Region of the request's SigV4 credential
/// scope, which is the Region the caller addressed. It is what the ARNs this service
/// mints name, and what scopes an environment name's uniqueness.
///
/// Every AWS Region token is accepted, including the four-segment ones
/// (`us-gov-west-1`, `us-isob-east-1`): reporting the Region the caller addressed
/// matters more than fitting the narrower three-segment Region grammar the modelled
/// `Arn` pattern spells, which would mean relabelling the request as another Region.
/// Only a request that carries no SigV4 scope, or one whose scope names something
/// that is not a Region token at all, falls back to the default Region.
pub fn request_region(parts: &::http::request::Parts) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is an AWS Region token: a two-letter area, one or more lowercase
/// words, and a single-digit index (`us-east-1`, `ap-southeast-4`, `us-gov-west-1`).
fn is_region_token(region: &str) -> bool {
    let mut segments: Vec<&str> = region.split('-').collect();
    let Some(index) = segments.pop() else {
        return false;
    };
    let Some(area) = (!segments.is_empty()).then(|| segments.remove(0)) else {
        return false;
    };
    index.len() == 1
        && index.bytes().all(|b| b.is_ascii_digit())
        && area.len() == 2
        && area.bytes().all(|b| b.is_ascii_lowercase())
        && !segments.is_empty()
        && segments
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The ARN of an environment: `arn:aws:evs:<region>:<account>:environment/<id>`, as
/// the modelled `Arn` pattern spells it.
pub fn environment_arn(region: &str, environment_id: &str) -> String {
    ::engine::emulator_arn("evs", region, &format!("environment/{environment_id}"))
}

/// The live connector of `connector_type` this environment holds, if any: the record
/// "You can create only one connector of each type per environment"
/// (`CreateEnvironmentConnector` doc) allows at most one of.
///
/// A connector that has reached `DELETED` does not count, exactly as a deleted
/// environment does not hold its name: the type is free for a new connector again.
pub fn live_connector_of_type<'a>(
    environment: &'a StoredEnvironment,
    connector_type: &str,
) -> Option<&'a StoredConnector> {
    environment.connectors.values().find(|connector| {
        connector.connector_type == connector_type && connector.state != CONNECTOR_DELETED
    })
}

/// The largest page a `@paginated` operation of this service returns when the caller
/// names no `maxResults`: the top of the modelled `MaxResults` range (1..100).
pub const MAX_PAGE_SIZE: i32 = 100;

/// How long a `nextToken` stays usable: "Each pagination token expires after 24
/// hours" (`nextToken` doc on every `@paginated` operation of this service).
pub const PAGE_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Why a `nextToken` could not be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenError {
    /// The token was not issued by this service.
    Malformed,
    /// The token was issued by this service more than [`PAGE_TOKEN_TTL`] ago.
    Expired,
}

/// Encode a pagination cursor — the key of the first item of the NEXT page — as a
/// `nextToken`.
///
/// The token carries the cursor and the instant it was issued, hex-encoded so it is
/// opaque to the caller (AWS documents no structure for these tokens) while staying
/// exactly resumable.
pub fn encode_page_token(cursor: &str) -> String {
    ::hex::encode(format!("{}:{cursor}", now_ms()))
}

/// Decode a `nextToken` produced by [`encode_page_token`] back to its cursor.
pub fn decode_page_token(token: &str) -> Result<String, PageTokenError> {
    let decoded = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or(PageTokenError::Malformed)?;
    let (issued_at_ms, cursor) = decoded.split_once(':').ok_or(PageTokenError::Malformed)?;
    let issued_at_ms: i64 = issued_at_ms.parse().map_err(|_| PageTokenError::Malformed)?;
    if now_ms().saturating_sub(issued_at_ms) > PAGE_TOKEN_TTL.as_millis() as i64 {
        return Err(PageTokenError::Expired);
    }
    Ok(cursor.to_string())
}

/// The refusal a `@paginated` operation answers for a `nextToken` it cannot resume.
///
/// The documented answer to an expired token is "an HTTP 400 InvalidToken error", but
/// this service's model declares no `InvalidToken` shape, so the refusal is carried by
/// the only client-input error these operations do declare — at the documented status.
pub fn invalid_next_token(error: PageTokenError) -> ValidationException {
    field_validation_error(
        "nextToken",
        match error {
            PageTokenError::Malformed => "is not a pagination token this service issued",
            PageTokenError::Expired => "has expired; pagination tokens are valid for 24 hours",
        },
    )
}

// =============================================================================
// How this service STATES a request its modelled constraints refuse
// =============================================================================

/// The segment the framework appends for the step from a collection into one of its
/// elements (`constraints::path::element` / `path::entry` both end in it).
const COLLECTION_STEP: &str = "member";

/// The fixed text the framework puts between a clause's member half and the requirement
/// it failed, in [`Violation::message`].
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// What this service's envelope joins several clauses with: a bare comma, no space.
const CLAUSE_JOINER: &str = ",";

/// The `ValidationException` message this service states for a request the boundary
/// validator refused against the model's own constraints — the ONE renderer behind every
/// operation of this service, reached from `Service::override_refusal`.
///
/// The envelope: `1 validation error detected: <clause>` for a single violation and
/// `<n> validation error(s) detected: <clause>,<clause>` for several, with the clauses
/// ordered by the member path they are reported against (never by the order the members
/// appear in the request) and joined by [`CLAUSE_JOINER`]. A clause is
/// `Value at '<member path>' failed to satisfy constraint: <requirement>`, with the
/// rejected value never quoted back and only an ABSENT member keeping
/// `Value null at '<path>' …`, because absence is the fact that clause states.
///
/// Returns `None` when there is nothing to state, so the caller keeps the framework's
/// default wording.
///
/// This is deliberately not the framework's default envelope renderer
/// (`aws_smithy_schema_server::envelope`): that one echoes the value, prints a `@pattern`
/// with the anchors the model wrote it with, says `errors` rather than `error(s)`, joins
/// with `"; "`, keeps the request's own member order, and reports a collection element at
/// its indexed path. Other emulated services word these refusals differently again
/// (`artifact` joins with `"; "` and prints a `@pattern` WITH its anchors), so each
/// service states its own envelope, as the other service crates carrying this helper do.
pub fn constraint_violation_message(violations: &[Violation]) -> Option<String> {
    let mut clauses: Vec<(String, String)> = violations.iter().map(clause).collect();
    // Ordered by the member path the clause names, compared byte-wise. `sort_by` is
    // stable, so two violations of ONE member keep the order the walk found them in.
    clauses.sort_by(|(left, _), (right, _)| left.cmp(right));
    let rendered: Vec<String> = clauses.into_iter().map(|(_, clause)| clause).collect();
    match rendered.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", rendered[0])),
        n => Some(format!(
            "{n} validation error(s) detected: {}",
            rendered.join(CLAUSE_JOINER)
        )),
    }
}

/// One violation as this service states it: the member path it is reported AGAINST
/// (which is what the clauses are ordered by), and the clause itself.
fn clause(violation: &Violation) -> (String, String) {
    let requirement = requirement_of(violation);
    // A violation of a collection ELEMENT is reported against the collection member
    // that holds it, with the element's requirement bundled inside the outer
    // constraint — a caller is told which member of the request to fix, and that the
    // requirement applies to the member's elements rather than to the member itself.
    let (path, requirement) = match collection_holding(&violation.path) {
        Some(collection) => (
            collection.to_string(),
            format!("Member must satisfy constraint: [{requirement}]"),
        ),
        None => (violation.path.clone(), requirement),
    };
    // A present-but-invalid value is NOT quoted back at the caller; an absent member
    // keeps the framework's `Value null at …`, because absence IS what it states.
    let subject = if violation.value.is_some() {
        "Value at"
    } else {
        "Value null at"
    };
    let clause = format!("{subject} '{path}'{CONSTRAINT_MARKER}{requirement}");
    (path, clause)
}

/// The requirement half of a violation's clause: what the value had to satisfy.
///
/// Every constraint keeps the framework's own requirement wording, so a constraint kind
/// it learns to check is reported rather than dropped — but it is read off a clause the
/// framework builds for this SAME violation with NO value attached, at the LAST marker in
/// that clause. Both halves of the framework's clause can carry text the CALLER chose:
/// the value it rejected, and — for a map entry, whose path segment is the key as sent —
/// the member path. A requirement never contains the marker, so the last occurrence is
/// the real separator whatever either half spells, while the first could fall inside
/// caller text and copy the rest of it into the message.
///
/// A `@pattern` is the one requirement reworded here: its anchors are the model's way of
/// saying "the whole value", not part of the expression (see [`unanchored`]).
fn requirement_of(violation: &Violation) -> String {
    if let ViolationKind::Pattern(pattern) = &violation.kind {
        return format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        );
    }
    let valueless = Violation {
        value: None,
        ..violation.clone()
    };
    let rendered = valueless.message();
    match rendered.rsplit_once(CONSTRAINT_MARKER) {
        Some((_member_half, requirement)) => requirement.to_string(),
        // A wording with no requirement half: keep what the framework rendered.
        None => rendered,
    }
}

/// `pattern` without the anchors bracketing it: a leading `^` and a trailing `$` the
/// expression itself has not escaped.
///
/// A Smithy `@pattern` is an unanchored regular-expression SEARCH, so a model that wants
/// the whole value to match writes the anchors in; they say "all of it" and are not part
/// of the expression a caller has to satisfy.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        // An ODD number of trailing backslashes escapes the `$`, making it a literal
        // dollar the value must carry rather than an anchor.
        Some(head) if trailing_backslashes(head).is_multiple_of(2) => head,
        _ => body,
    }
}

/// How many `\` characters `text` ends with.
fn trailing_backslashes(text: &str) -> usize {
    text.chars().rev().take_while(|c| *c == '\\').count()
}

/// The collection member path a violation of a collection ELEMENT is reported against,
/// or `None` when the offending value is not an element of a collection.
///
/// The framework names an element by the step it took to reach it — `<1-based index>`
/// for a list element, the key for a map entry, each followed by [`COLLECTION_STEP`] —
/// so a path ENDING in that step is the element ITSELF and belongs to the collection
/// holding it. A path that merely passes THROUGH a collection to reach a member inside
/// an element (`settings.1.member.value`) keeps every segment: what failed there is that
/// member, not the element.
///
/// A map value that fails a constraint of its own shape is the one case this cannot
/// place: the framework names it by its key alone (`tags.someKey`), with no collection
/// step to tell it apart from a member of a structure, so it keeps the path the framework
/// built.
fn collection_holding(path: &str) -> Option<&str> {
    let through_step = path.strip_suffix(COLLECTION_STEP)?.strip_suffix('.')?;
    let (collection, _element) = through_step.rsplit_once('.')?;
    Some(collection)
}
