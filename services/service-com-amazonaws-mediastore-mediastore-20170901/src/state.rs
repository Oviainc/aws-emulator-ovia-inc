// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediastore-mediastore-20170901 service.
//!
//! OUT OF SCOPE here: delivering container ACCESS LOGS. A container carries the
//! modelled `AccessLoggingEnabled` flag, which `StartAccessLogging` and
//! `StopAccessLogging` (both still scaffolded at 501) would move; real MediaStore
//! also delivers access-log records to CloudWatch Logs, and this emulator writes no
//! such record. The flag is persisted and reported faithfully; it enforces nothing.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{CorsRule, MetricPolicy, Tag};

/// The region an ARN or an endpoint carries when the request names none this service
/// can use.
///
/// The modelled `ContainerARN` pattern (`^arn:aws:mediastore:[a-z]+-[a-z]+-\d:...`)
/// admits only a canonical region token, so an unsigned request — which carries no
/// SigV4 credential scope to read a region from — still has to name a well-formed one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many hex characters of the keyed digest a pagination token carries. Four bytes
/// of digest: enough that a cursor cannot be guessed by trying tokens, and short enough
/// that the longest token stays far inside the modelled 1..1024 `PaginationToken`
/// length.
const TOKEN_DIGEST_BYTES: usize = 4;

/// How long after `CreateContainer` the clock owes a container its
/// `CREATING` -> `ACTIVE` move.
///
/// CHOSEN, not documented: the model states the transition
/// ("While the service is creating the container, the status is `CREATING`. When the
/// endpoint is available, the status changes to `ACTIVE`") but never how long it
/// takes. It is deliberately non-zero, so a caller that describes a container
/// immediately after creating it still observes `CREATING`.
pub const CONTAINER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The modelled `ContainerStatus` enum, as this service stores it.
///
/// The wire values are exactly the model's
/// [`CONTAINER_STATUS_VALUES`](crate::ops::types::CONTAINER_STATUS_VALUES).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Creating,
    Active,
    /// Modelled, but no operation here mints it: `DeleteContainer` removes the record
    /// in the same call, so this emulator has no delete-in-progress state to report.
    Deleting,
}

impl ContainerStatus {
    /// This status as the model spells it on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored MediaStore container: the modelled `Container` members plus the
/// policies and tags the other operations attach to the same record.
#[derive(Debug, Clone)]
pub struct Container {
    pub name: String,
    pub arn: String,
    /// The DNS endpoint. "The service assigns this value when the container is
    /// created. Once the value has been assigned, it does not change."
    pub endpoint: String,
    pub creation_time: SystemTime,
    pub status: ContainerStatus,
    pub access_logging_enabled: bool,
    pub policy: Option<String>,
    pub cors_policy: Option<Vec<CorsRule>>,
    pub lifecycle_policy: Option<String>,
    pub metric_policy: Option<MetricPolicy>,
    pub tags: Vec<Tag>,
    /// The instant the `CREATING` -> `ACTIVE` move falls due. Stored, never slept
    /// on: a later call materialises the transition the clock already owes.
    pub active_at: SystemTime,
}

impl Container {
    /// A container as `CreateContainer` mints it: `CREATING`, access logging off,
    /// no policy of any kind, and the tags the request carried.
    pub fn new(
        name: String,
        arn: String,
        endpoint: String,
        tags: Vec<Tag>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            name,
            arn,
            endpoint,
            creation_time: created_at,
            status: ContainerStatus::Creating,
            access_logging_enabled: false,
            policy: None,
            cors_policy: None,
            lifecycle_policy: None,
            metric_policy: None,
            tags,
            // Saturating rather than panicking: an instant the clock cannot represent
            // is one the transition is already past due at.
            active_at: created_at
                .checked_add(CONTAINER_ACTIVATION_DELAY)
                .unwrap_or(created_at),
        }
    }

    /// Materialise the `CREATING` -> `ACTIVE` move once `now` has reached the
    /// instant it fell due. This is the only write a reporting operation makes:
    /// the transition was owed at that instant whether or not anybody called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == ContainerStatus::Creating && now >= self.active_at {
            self.status = ContainerStatus::Active;
        }
    }

    /// This record as the modelled `Container` shape.
    pub fn to_wire(&self) -> crate::ops::types::Container {
        crate::ops::types::Container {
            endpoint: Some(self.endpoint.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.as_wire().to_string()),
            access_logging_enabled: Some(self.access_logging_enabled),
        }
    }
}

/// Per-service state for com-amazonaws-mediastore-mediastore-20170901. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMediastoreMediastore20170901State {
    /// The containers this account holds, keyed by name — a `BTreeMap` so
    /// `ListContainers` enumerates them in a stable order and a pagination token
    /// can resume the range after the name it names.
    pub containers: BTreeMap<String, Container>,
    /// Keys the pagination tokens this store issues; minted once, when the store
    /// comes up. A token is only a cursor this store handed out if its digest was
    /// computed under this key, so a forged one is refused instead of silently
    /// resuming at a cursor nobody was given.
    token_key: [u8; 32],
}

impl Default for ComAmazonawsMediastoreMediastore20170901State {
    fn default() -> Self {
        Self {
            containers: BTreeMap::new(),
            token_key: mint_token_key(),
        }
    }
}

impl ComAmazonawsMediastoreMediastore20170901State {
    /// Apply every transition the clock already owes, so each call observes the
    /// state as of `now` rather than as of the last call that happened to write.
    pub fn settle(&mut self, now: SystemTime) {
        for container in self.containers.values_mut() {
            container.settle(now);
        }
    }

    /// The opaque `NextToken` for a page that ended at `name`: that container's name,
    /// hex-encoded, and a digest of it keyed by this store.
    ///
    /// Both halves are hex with one `=` between them, which the modelled
    /// `^[0-9A-Za-z=/+]+$` pattern admits, and the longest name the 1..255
    /// `ContainerName` constraint allows yields 519 characters — inside the modelled
    /// 1..1024 token length. The name is what makes the cursor resumable: the next
    /// page continues the ordered map immediately after it, repeating and skipping
    /// nothing even if containers were added or removed in between.
    pub fn issue_page_token(&self, name: &str) -> String {
        format!("{}={}", hex::encode(name), self.token_digest(name))
    }

    /// The container name a `NextToken` resumes after, or `None` when the token is not
    /// one this store issued — a mangled cursor, or one minted under another key.
    pub fn resume_after(&self, token: &str) -> Option<String> {
        let (encoded_name, digest) = token.split_once('=')?;
        let name = String::from_utf8(hex::decode(encoded_name).ok()?).ok()?;
        (self.token_digest(&name) == digest).then_some(name)
    }

    /// This store's digest of `name`, as a pagination token carries it.
    fn token_digest(&self, name: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(self.token_key);
        digest.update(name.as_bytes());
        hex::encode(&digest.finalize()[..TOKEN_DIGEST_BYTES])
    }
}

/// A fresh key for the pagination tokens one store issues.
///
/// Derived from the instant the store comes up and a process-wide counter, so two
/// stores — and two runs of the emulator — key their cursors differently and a token
/// one issued is not a cursor another accepts.
fn mint_token_key() -> [u8; 32] {
    static STORES: AtomicU64 = AtomicU64::new(0);
    let store = STORES.fetch_add(1, Ordering::Relaxed);
    let started = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();
    Sha256::digest(format!("mediastore-pagination/{started}/{store}")).into()
}

/// The region this request was signed for, as an ARN or an endpoint may spell it.
///
/// Read from the SigV4 credential scope — the one place an inbound AWS request names
/// its region — and only when it is a canonical region token, since the modelled ARN
/// pattern accepts nothing else. Every operation of this service that mints a
/// region-bearing value reads it here rather than re-deriving it.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_canonical_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is a `<letters>-<letters>-<digit>` token, the shape the modelled
/// `ContainerARN` pattern `[a-z]+-[a-z]+-\d` admits.
fn is_canonical_region(region: &str) -> bool {
    let mut fields = region.split('-');
    let (Some(area), Some(direction), Some(index), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    let lowercase =
        |field: &str| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_lowercase());
    lowercase(area)
        && lowercase(direction)
        && index.len() == 1
        && index.starts_with(|c: char| c.is_ascii_digit())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediastoreMediastore20170901State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediastoreMediastore20170901State>)
}
