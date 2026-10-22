// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloud9-awscloud9workspacemanagementservice service.
//!
//! The Cloud9 control plane is a store of **environments**. Everything the
//! handlers in `src/ops/` answer with is reconstructed from the records below;
//! nothing is echoed from a request and nothing is canned.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// How long after `CreateEnvironmentEC2` an environment's `creating` status
/// becomes `ready`.
///
/// CHOSEN, not documented: neither the Smithy model nor the Cloud9 API reference
/// states how long environment creation takes, so this is a span inside the
/// emulator's 250–750 ms window. It is deliberately NOT zero — a caller that
/// polls `DescribeEnvironmentStatus` straight after creating must still observe
/// the transient `creating` status and the `CREATING` lifecycle state the real
/// service reports.
pub const CREATION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// The page size `ListEnvironments` uses when the caller names none: the modeled
/// maximum of its own `maxResults` member (`@range(min: 0, max: 25)`).
pub const MAX_ENVIRONMENTS_PER_PAGE: usize = 25;

/// The most environments this emulator keeps resident for one account.
///
/// CHOSEN, not an AWS-published figure: Cloud9's environment quota is an
/// adjustable account limit, so the emulator does not claim a number AWS
/// guarantees. What matters is that the store is BOUNDED — a caller cannot grow
/// the process's memory without limit by creating environments it never deletes
/// — and that the refusal is the one AWS gives when an account's environment
/// quota is reached: the modeled `LimitExceededException` ("A service limit was
/// exceeded", HTTP 400).
///
/// It bounds `idempotency` too: a `clientRequestToken` is recorded only by a
/// create that actually minted an environment, and `DeleteEnvironment` drops the
/// tokens of the environment it removes, so there is never more than one token
/// per resident environment.
pub const ENVIRONMENTS_PER_ACCOUNT: usize = 200;

/// The region an ARN carries when the request is unsigned, so no SigV4
/// credential scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EnvironmentType` — the `type` member of `Environment`, which the model makes
/// `@required`. Values from `com.amazonaws.cloud9#EnvironmentType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentType {
    /// An Amazon EC2 instance connects to the environment.
    Ec2,
    /// The caller's own server connects to the environment.
    Ssh,
}

impl EnvironmentType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Ec2, Self::Ssh];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ec2 => "ec2",
            Self::Ssh => "ssh",
        }
    }
}

/// `ConnectionType` — how a client reaches an EC2 environment. Values from
/// `com.amazonaws.cloud9#ConnectionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionType {
    /// Direct SSH. The model documents this as the default.
    ConnectSsh,
    /// Through Amazon EC2 Systems Manager.
    ConnectSsm,
}

impl ConnectionType {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::ConnectSsh, Self::ConnectSsm];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ConnectSsh => "CONNECT_SSH",
            Self::ConnectSsm => "CONNECT_SSM",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// `EnvironmentStatus` — what `DescribeEnvironmentStatus` reports. Values from
/// `com.amazonaws.cloud9#EnvironmentStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentStatus {
    Connecting,
    Creating,
    Deleting,
    Error,
    Ready,
    Stopped,
    Stopping,
}

impl EnvironmentStatus {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[
        Self::Connecting,
        Self::Creating,
        Self::Deleting,
        Self::Error,
        Self::Ready,
        Self::Stopped,
        Self::Stopping,
    ];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Creating => "creating",
            Self::Deleting => "deleting",
            Self::Error => "error",
            Self::Ready => "ready",
            Self::Stopped => "stopped",
            Self::Stopping => "stopping",
        }
    }
}

/// `EnvironmentLifecycleStatus` — the `lifecycle.status` member of
/// `Environment`. Values from `com.amazonaws.cloud9#EnvironmentLifecycleStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentLifecycleStatus {
    Created,
    CreateFailed,
    Creating,
    DeleteFailed,
    Deleting,
}

impl EnvironmentLifecycleStatus {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[
        Self::Created,
        Self::CreateFailed,
        Self::Creating,
        Self::DeleteFailed,
        Self::Deleting,
    ];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::CreateFailed => "CREATE_FAILED",
            Self::Creating => "CREATING",
            Self::DeleteFailed => "DELETE_FAILED",
            Self::Deleting => "DELETING",
        }
    }
}

/// `ManagedCredentialsStatus` — the state of the Amazon Web Services managed
/// temporary credentials an environment carries. Values from
/// `com.amazonaws.cloud9#ManagedCredentialsStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedCredentialsStatus {
    EnabledOnCreate,
    EnabledByOwner,
    DisabledByDefault,
    DisabledByOwner,
    DisabledByCollaborator,
    PendingRemovalByCollaborator,
    PendingStartRemovalByCollaborator,
    PendingRemovalByOwner,
    PendingStartRemovalByOwner,
    FailedRemovalByCollaborator,
    FailedRemovalByOwner,
}

impl ManagedCredentialsStatus {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[
        Self::EnabledOnCreate,
        Self::EnabledByOwner,
        Self::DisabledByDefault,
        Self::DisabledByOwner,
        Self::DisabledByCollaborator,
        Self::PendingRemovalByCollaborator,
        Self::PendingStartRemovalByCollaborator,
        Self::PendingRemovalByOwner,
        Self::PendingStartRemovalByOwner,
        Self::FailedRemovalByCollaborator,
        Self::FailedRemovalByOwner,
    ];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnabledOnCreate => "ENABLED_ON_CREATE",
            Self::EnabledByOwner => "ENABLED_BY_OWNER",
            Self::DisabledByDefault => "DISABLED_BY_DEFAULT",
            Self::DisabledByOwner => "DISABLED_BY_OWNER",
            Self::DisabledByCollaborator => "DISABLED_BY_COLLABORATOR",
            Self::PendingRemovalByCollaborator => "PENDING_REMOVAL_BY_COLLABORATOR",
            Self::PendingStartRemovalByCollaborator => "PENDING_START_REMOVAL_BY_COLLABORATOR",
            Self::PendingRemovalByOwner => "PENDING_REMOVAL_BY_OWNER",
            Self::PendingStartRemovalByOwner => "PENDING_START_REMOVAL_BY_OWNER",
            Self::FailedRemovalByCollaborator => "FAILED_REMOVAL_BY_COLLABORATOR",
            Self::FailedRemovalByOwner => "FAILED_REMOVAL_BY_OWNER",
        }
    }
}

/// `ManagedCredentialsAction` — `UpdateEnvironment`'s request to turn managed
/// credentials on or off. Values from
/// `com.amazonaws.cloud9#ManagedCredentialsAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedCredentialsAction {
    Disable,
    Enable,
}

impl ManagedCredentialsAction {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Disable, Self::Enable];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Disable => "DISABLE",
            Self::Enable => "ENABLE",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }

    /// The `managedCredentialsStatus` an owner's action leaves the environment
    /// in: the model's two owner-attributed values.
    pub fn resulting_status(self) -> ManagedCredentialsStatus {
        match self {
            Self::Disable => ManagedCredentialsStatus::DisabledByOwner,
            Self::Enable => ManagedCredentialsStatus::EnabledByOwner,
        }
    }
}

/// `Permissions` — a member's permissions on an environment. Values from
/// `com.amazonaws.cloud9#Permissions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permissions {
    Owner,
    ReadOnly,
    ReadWrite,
}

impl Permissions {
    /// Every modeled value, for exhaustiveness checks.
    pub const ALL: &'static [Self] = &[Self::Owner, Self::ReadOnly, Self::ReadWrite];

    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }

    /// Parse a wire value, or `None` when it names no modeled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_wire() == value)
    }
}

/// The stored form of the `EnvironmentLifecycle` structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentLifecycleState {
    pub status: EnvironmentLifecycleStatus,
    pub reason: Option<String>,
    pub failure_resource: Option<String>,
}

/// One member of an environment's access list.
///
/// The creator is seeded as the single `owner` member at create time, so the
/// membership operations have a base to build on when they are implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentMember {
    pub environment_id: String,
    pub user_arn: String,
    pub user_id: String,
    pub permissions: Permissions,
    pub last_access: Option<SystemTime>,
}

/// A Cloud9 development environment, as this emulator stores it.
#[derive(Debug, Clone)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub env_type: EnvironmentType,
    pub connection_type: ConnectionType,
    pub arn: String,
    pub owner_arn: String,
    pub instance_type: String,
    pub image_id: String,
    pub subnet_id: Option<String>,
    pub automatic_stop_time_minutes: Option<i32>,
    pub status: EnvironmentStatus,
    pub status_message: String,
    pub lifecycle: EnvironmentLifecycleState,
    pub managed_credentials_status: Option<ManagedCredentialsStatus>,
    pub created_at: SystemTime,
    /// The instant the `creating` -> `ready` transition falls due:
    /// `created_at + CREATION_SETTLES_AFTER`. Stored, never slept on.
    pub settles_at: SystemTime,
    pub members: Vec<EnvironmentMember>,
    pub tags: BTreeMap<String, String>,
}

impl Environment {
    /// Materialise the `creating` -> `ready` transition the clock has ALREADY
    /// made due.
    ///
    /// This is the one write a reporting operation may make: the transition was
    /// owed at `settles_at` whether or not anybody called, so applying it on
    /// read changes nothing a later caller would otherwise have seen. Any status
    /// other than `creating` is left exactly as it is.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == EnvironmentStatus::Creating && now >= self.settles_at {
            self.status = EnvironmentStatus::Ready;
            self.status_message = "Environment is ready to use".to_string();
            self.lifecycle = EnvironmentLifecycleState {
                status: EnvironmentLifecycleStatus::Created,
                reason: None,
                failure_resource: None,
            };
        }
    }
}

/// Per-service state for com-amazonaws-cloud9-awscloud9workspacemanagementservice. A handler reaches it through [`service_state`].
///
/// `environments` is a `BTreeMap` so `ListEnvironments` has one stable order and
/// its `nextToken` can name the key to resume from.
#[derive(Debug, Default)]
pub struct ComAmazonawsCloud9Awscloud9workspacemanagementserviceState {
    /// Every live environment, keyed by its `environmentId`.
    pub environments: BTreeMap<String, Environment>,
    /// `clientRequestToken` -> the `environmentId` that token created, so a
    /// repeated `CreateEnvironmentEC2` (the operation is `@idempotent`) answers
    /// with the same id instead of creating a second environment.
    pub idempotency: BTreeMap<String, String>,
    /// Counts the environments this store has minted, so two ids minted inside
    /// one clock tick still differ. Never exposed on the wire.
    pub minted_environments: u64,
}

impl ComAmazonawsCloud9Awscloud9workspacemanagementserviceState {
    /// A fresh `environmentId`: 32 lowercase hex characters, which satisfies the
    /// modeled `^[a-zA-Z0-9]{8,32}$`.
    ///
    /// The id is a digest of the instant the environment is being created and
    /// this store's mint counter, so it is server-generated and opaque: it
    /// carries nothing from the request, two creates never collide (the counter
    /// separates them inside one clock tick), and two emulator runs do not
    /// repeat each other's ids. Retried against the store so an id is never
    /// reused.
    pub fn mint_environment_id(&mut self) -> String {
        use ::sha2::{Digest, Sha256};
        loop {
            self.minted_environments = self.minted_environments.wrapping_add(1);
            let since_epoch = SystemTime::now()
                .duration_since(::std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let digest = Sha256::digest(
                format!(
                    "cloud9-environment:{since_epoch}:{}",
                    self.minted_environments
                )
                .as_bytes(),
            );
            // 16 digest bytes render as 32 lowercase hex characters, the
            // modeled maximum length of an `EnvironmentId`.
            let candidate = ::hex::encode(&digest[..16]);
            if !self.environments.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the store is already holding [`ENVIRONMENTS_PER_ACCOUNT`]
    /// environments, so a further create must be refused.
    pub fn is_at_environment_quota(&self) -> bool {
        self.environments.len() >= ENVIRONMENTS_PER_ACCOUNT
    }
}

/// The region this request is for: the one its SigV4 credential scope names,
/// else [`DEFAULT_REGION`] when the request carries no parseable scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an environment: `arn:aws:cloud9:<region>:<account>:environment:<id>`.
pub fn environment_arn(region: &str, environment_id: &str) -> String {
    ::engine::emulator_arn("cloud9", region, &format!("environment:{environment_id}"))
}

/// The ARN of the principal that made this request, used where
/// `CreateEnvironmentEC2` documents that `ownerArn` "defaults to this
/// environment's creator".
///
/// Built from the request's SigV4 access-key id, reduced to the characters the
/// modeled `UserArn` pattern lets a user name carry; an unsigned or unusable
/// id falls back to the account root, which that pattern also accepts.
pub fn caller_user_arn(parts: &::http::request::Parts) -> String {
    let akid = ::engine::extract_akid(&parts.headers);
    let user_name: String = akid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '=' | ',' | '.' | '@' | '-' | '_'))
        .take(64)
        .collect();
    if user_name.is_empty() {
        ::engine::emulator_arn("iam", "", "root")
    } else {
        ::engine::emulator_arn("iam", "", &format!("user/{user_name}"))
    }
}

/// Whether `arn` names an IAM or STS principal, which is what the modeled
/// `UserArn` accepts: `arn:<partition>:(iam|sts)::<account>:` followed by
/// `root`, `user/<name>`, `federated-user/<name>` or
/// `assumed-role/<role>/<session>`.
///
/// Restated structurally from the model's `@pattern` rather than copied as a
/// second regex, so there is one grammar and this is a check against it.
pub fn is_principal_arn(arn: &str) -> bool {
    /// The partitions the modeled `UserArn` pattern lists.
    const PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov", "aws-iso", "aws-iso-b"];

    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields[..] else {
        return false;
    };
    if prefix != "arn"
        || !PARTITIONS.contains(&partition)
        || !matches!(service, "iam" | "sts")
        // An IAM/STS principal ARN carries no region.
        || !region.is_empty()
        || account.is_empty()
        || !account.chars().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    let within = |value: &str, min: usize, max: usize| {
        let len = value.chars().count();
        len >= min && len <= max
    };
    if resource == "root" {
        return true;
    }
    if let Some(user) = resource.strip_prefix("user/") {
        return within(user, 1, 64);
    }
    if let Some(user) = resource.strip_prefix("federated-user/") {
        return within(user, 2, 32);
    }
    if let Some(assumed) = resource.strip_prefix("assumed-role/") {
        let Some((role, session)) = assumed.split_once('/') else {
            return false;
        };
        return within(role, 1, 64) && within(session, 1, 64);
    }
    false
}

/// The IAM unique id of a principal, in the `AIDA…` shape IAM uses for a user.
///
/// Derived from the principal's ARN by digest, so it is STABLE for a principal
/// (two environments owned by the same ARN report the same `userId`) and
/// distinct between principals. It is computed from content, never randomised
/// per call.
pub fn principal_unique_id(user_arn: &str) -> String {
    use ::sha2::{Digest, Sha256};
    let digest = Sha256::digest(user_arn.as_bytes());
    let encoded = ::hex::encode_upper(digest);
    format!("AIDA{}", &encoded[..17])
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloud9Awscloud9workspacemanagementserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloud9Awscloud9workspacemanagementserviceState>)
}
