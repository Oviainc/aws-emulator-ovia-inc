// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-healthlake-healthlake service.
//!
//! The FHIR data store is this service's primary resource: `CreateFHIRDatastore`
//! mints one here and `DescribeFHIRDatastore` / `ListFHIRDatastores` /
//! `TagResource` / `ListTagsForResource` / `DeleteFHIRDatastore` all read and
//! write the SAME record, so a create is observable through every one of them.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AnalyticsConfiguration, BackupConfiguration, DatastoreBackupStatus, DatastoreProperties,
    ErrorCause, IdentityProviderConfiguration, NlpConfiguration, PreloadDataConfig,
    ProfileConfiguration, SseConfiguration, Tag,
};

/// How long after the call that starts it a data store's pending lifecycle
/// transition (`CREATING` -> `ACTIVE`, `DELETING` -> `DELETED`) falls due.
///
/// AWS documents no duration for either transition, so this span is CHOSEN: it is
/// non-zero, so a caller that describes the store immediately after creating it
/// still observes the transient status (and the state errors that go with it),
/// and short enough that nothing has to wait long for the settled one. It is
/// never slept on — the due instant is stored on the record and materialised by
/// the next call that reads it.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// How long a `DELETED` data store stays readable before it is reaped.
///
/// AWS documents no retention for a deleted data store, so this span is CHOSEN.
/// It is the TTL on a terminal record: long enough that a caller which polls
/// after its delete still sees `DELETED` rather than a bare 404, and short
/// enough that terminal records cannot accumulate. Like every other due instant
/// here it is stored, never slept on.
pub const DELETED_RETENTION: Duration = Duration::from_secs(60);

/// The region stamped into an ARN and an endpoint when the request carries no
/// SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names a data store no record holds.
///
/// It is a FIXED service text: the same sentence whatever `DatastoreId` or ARN
/// was asked about, so a not-found answer never quotes the request back at the
/// caller. The model documents the error itself only as "The requested data
/// store was not found."; this is the wording the service answers with. Every
/// data-store not-found path in this crate states it through this one constant,
/// so none of them can drift into echoing the identifier.
pub const DATASTORE_NOT_FOUND_MESSAGE: &str = "Could not find specified datastore.";

/// The sentence this service refuses a mutation with when the target data store
/// exists but its `DatastoreStatus` is not `ACTIVE` (`CREATING`, `DELETING` or
/// the terminal `DELETED`), beside `AccessDeniedException` (HTTP 403).
///
/// Like [`DATASTORE_NOT_FOUND_MESSAGE`] it names no identifier and no status: it
/// is one fixed text for "the data store cannot be written to right now".
pub const DATASTORE_NOT_ACTIVE_MESSAGE: &str = "Specified datastore is not active.";

/// The most data stores this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a `CreateFHIRDatastore`
/// loop cannot grow the emulator's heap without bound, and it also bounds the
/// per-request [`ComAmazonawsHealthlakeHealthlakeState::settle_lifecycle`] walk.
/// `CreateFHIRDatastore` refuses past it with the `ThrottlingException` it models
/// — the only quota-shaped refusal that operation declares.
pub const MAX_DATASTORES: usize = 100;

/// The most `ClientToken` entries kept for `CreateFHIRDatastore` idempotency.
///
/// At most one token is recorded per stored data store and a token is dropped
/// with the record it minted, so this cap is never the binding one in practice;
/// it is enforced anyway, by evicting the oldest token, so a stream of fresh
/// caller-chosen tokens cannot grow the map independently of the data stores
/// they name.
pub const MAX_CLIENT_TOKENS: usize = MAX_DATASTORES;

/// The most tags one data store carries.
///
/// The modelled `TagList` bound (0..200) caps ONE request; this caps the
/// accumulation across repeated `TagResource` calls with distinct keys. Both
/// `CreateFHIRDatastore` and `TagResource` refuse past it with the
/// `ValidationException` they model.
pub const MAX_TAGS_PER_DATASTORE: usize = 50;

/// The `datastore` resource prefix of a HealthLake data store ARN, as the
/// modelled `AmazonResourceName` pattern spells it
/// (`...:(datastore\/fhir\/.{32}|dataTransformationProfile\/[a-f0-9]{32})$`).
const DATASTORE_ARN_RESOURCE_PREFIX: &str = "datastore/fhir/";

/// The states a data store moves through in this slice, as the modelled
/// `DatastoreStatus` enum names them on the wire. The failure states the enum
/// also declares (`CREATE_FAILED`, `UPDATING`, `UPDATE_FAILED`) belong to
/// operations that are still scaffolded, so nothing here produces them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatastoreStatus {
    Creating,
    Active,
    Deleting,
    Deleted,
}

impl DatastoreStatus {
    /// The modelled enum value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }
}

/// One persisted FHIR data store: everything `CreateFHIRDatastore` was given,
/// plus the identifiers and the lifecycle state the service owns.
#[derive(Debug)]
pub struct Datastore {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub status: DatastoreStatus,
    /// The transition the clock owes this record, as `(due instant, next status)`.
    /// `None` once the status is settled.
    pub pending: Option<(Instant, DatastoreStatus)>,
    /// When this record is reaped, set once it reaches the terminal `DELETED`
    /// status. `None` while the data store is still live.
    pub reap_at: Option<Instant>,
    pub type_version: String,
    pub endpoint: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub sse_configuration: Option<SseConfiguration>,
    pub preload_data_config: Option<PreloadDataConfig>,
    pub identity_provider_configuration: Option<IdentityProviderConfiguration>,
    pub analytics_configuration: Option<AnalyticsConfiguration>,
    pub nlp_configuration: Option<NlpConfiguration>,
    pub profile_configuration: Option<ProfileConfiguration>,
    pub backup_configuration: Option<BackupConfiguration>,
    pub error_cause: Option<ErrorCause>,
    /// Tags by key, so `TagResource` merges by key and the reported order is
    /// stable across reads. Seeded from `CreateFHIRDatastore`'s `Tags`.
    pub tags: BTreeMap<String, String>,
}

impl Datastore {
    /// This record as the modelled `DatastoreProperties`, the one shape
    /// `DescribeFHIRDatastore` and `ListFHIRDatastores` both report.
    pub fn properties(&self) -> DatastoreProperties {
        DatastoreProperties {
            datastore_id: self.id.clone(),
            datastore_arn: self.arn.clone(),
            datastore_name: self.name.clone(),
            datastore_status: self.status.as_wire().to_string(),
            created_at: Some(self.created_at),
            datastore_type_version: self.type_version.clone(),
            datastore_endpoint: self.endpoint.clone(),
            sse_configuration: self.sse_configuration.clone(),
            preload_data_config: self.preload_data_config.clone(),
            identity_provider_configuration: self.identity_provider_configuration.clone(),
            error_cause: self.error_cause.clone(),
            nlp_configuration: self.nlp_configuration.clone(),
            analytics_configuration: self.analytics_configuration.clone(),
            profile_configuration: self.profile_configuration.clone(),
            // Only the configuration half of the backup status is known here:
            // it took effect when the data store was created. The restore-point
            // and scheduled-deletion instants describe backups that have run,
            // and nothing in this slice runs one, so they stay unset rather
            // than being invented.
            backup_status_info: self
                .backup_configuration
                .clone()
                .map(|configuration| DatastoreBackupStatus {
                    configuration: Some(configuration),
                    backup_enabled_at: Some(self.created_at),
                    earliest_restore_point: None,
                    latest_restore_point: None,
                    scheduled_permanent_deletion_time: None,
                }),
        }
    }

    /// The stored tags as the modelled `TagList`, ordered by key.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// What one `CreateFHIRDatastore` `ClientToken` recorded: the data store it
/// minted, and a fingerprint of the request that minted it, so a token reused
/// with a DIFFERENT request is refused instead of silently answering with the
/// earlier data store.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub datastore_id: String,
    pub fingerprint: String,
}

/// Per-service state for com-amazonaws-healthlake-healthlake. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsHealthlakeHealthlakeState {
    /// Every live data store, by `DatastoreId`. A `BTreeMap` so
    /// `ListFHIRDatastores` pages in a stable, resumable order. Bounded by
    /// [`MAX_DATASTORES`]; a `DELETED` record is reaped after
    /// [`DELETED_RETENTION`].
    pub datastores: BTreeMap<String, Datastore>,
    /// `CreateFHIRDatastore` idempotency: `ClientToken` -> what the first call
    /// with that token minted. Bounded by [`MAX_CLIENT_TOKENS`]; write it through
    /// [`Self::record_client_token`], never directly.
    pub client_tokens: BTreeMap<String, ClientTokenRecord>,
    /// The order [`Self::client_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    client_token_order: VecDeque<String>,
    /// How many data stores have been minted, mixed into the next identifier so
    /// two creates in the same clock tick still get distinct ones.
    pub datastores_created: u64,
}

impl ComAmazonawsHealthlakeHealthlakeState {
    /// Materialise every lifecycle transition the clock has ALREADY made due.
    ///
    /// Called by the operations that report a data store's status as well as by
    /// the ones that change it: the transition was owed at this instant whether
    /// or not anybody called, so applying it on read is not a read with a side
    /// effect — it is the record catching up with the clock.
    /// The walk is bounded by [`MAX_DATASTORES`], so it costs the same whatever
    /// a caller does.
    pub fn settle_lifecycle(&mut self) {
        let now = Instant::now();
        for datastore in self.datastores.values_mut() {
            if let Some((due, next)) = datastore.pending {
                if now >= due {
                    datastore.status = next;
                    datastore.pending = None;
                    // A record that has reached its terminal status starts its
                    // retention clock; everything else stays live.
                    if next == DatastoreStatus::Deleted {
                        datastore.reap_at = Some(now + DELETED_RETENTION);
                    }
                }
            }
        }
        self.reap_expired_records(now);
    }

    /// Drop every terminal record whose retention has run out, and the
    /// idempotency tokens that named it, so a deleted data store does not
    /// outlive its own retention.
    fn reap_expired_records(&mut self, now: Instant) {
        let reaped: Vec<String> = self
            .datastores
            .iter()
            .filter(|(_, datastore)| datastore.reap_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in reaped {
            self.datastores.remove(&id);
            self.forget_client_tokens_for(&id);
        }
    }

    /// Reap every terminal record NOW, whatever its retention: the answer to a
    /// full store, taken before a create is refused, so a caller that deletes
    /// what it created can always create again.
    pub fn reap_terminal_records(&mut self) {
        let reaped: Vec<String> = self
            .datastores
            .iter()
            .filter(|(_, datastore)| datastore.status == DatastoreStatus::Deleted)
            .map(|(id, _)| id.clone())
            .collect();
        for id in reaped {
            self.datastores.remove(&id);
            self.forget_client_tokens_for(&id);
        }
    }

    /// Record `token` as having minted `datastore_id` from a request with
    /// `fingerprint`, evicting the oldest token first if that would push the map
    /// past [`MAX_CLIENT_TOKENS`].
    pub fn record_client_token(&mut self, token: String, datastore_id: &str, fingerprint: String) {
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the
                // cap an invariant even if they ever drift.
                None => {
                    self.client_tokens.clear();
                    break;
                }
            }
        }
        if self
            .client_tokens
            .insert(
                token.clone(),
                ClientTokenRecord {
                    datastore_id: datastore_id.to_string(),
                    fingerprint,
                },
            )
            .is_none()
        {
            self.client_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that minted `datastore_id`, so a token never
    /// outlives the data store it names.
    fn forget_client_tokens_for(&mut self, datastore_id: &str) {
        self.client_tokens
            .retain(|_, record| record.datastore_id != datastore_id);
        let live = &self.client_tokens;
        self.client_token_order
            .retain(|token| live.contains_key(token));
    }

    /// Mint an unused `DatastoreId`: 32 lowercase hex characters, which is what
    /// the modelled data store ARN requires (`datastore/fhir/.{32}`) and what
    /// `DatastoreId`'s own length bound (1..32) allows.
    pub fn next_datastore_id(&mut self, created_at: SystemTime) -> String {
        use sha2::Digest;

        let nanos = created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        loop {
            self.datastores_created += 1;
            let mut hasher = sha2::Sha256::new();
            hasher.update(self.datastores_created.to_be_bytes());
            hasher.update(nanos.to_be_bytes());
            let id = hex::encode(&hasher.finalize()[..16]);
            if !self.datastores.contains_key(&id) {
                return id;
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsHealthlakeHealthlakeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsHealthlakeHealthlakeState>)
}

/// The region to stamp into a new resource's ARN and endpoint: the region of the
/// request's SigV4 credential scope, else [`DEFAULT_REGION`].
///
/// Lowercased and filtered to the characters the modelled `AmazonResourceName`
/// pattern allows in that segment (`[a-z0-9-]+`), so a scope carrying anything
/// else falls back rather than producing an ARN the model would refuse.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region.to_ascii_lowercase())
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the data store `id` in `region`, in the form the modelled
/// `DatastoreArn` and `AmazonResourceName` patterns both require. The account
/// segment comes from the emulator's canonical account id.
pub fn datastore_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "healthlake",
        region,
        &format!("{DATASTORE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The FHIR endpoint of the data store `id` in `region`.
pub fn datastore_endpoint(region: &str, id: &str) -> String {
    format!("https://healthlake.{region}.amazonaws.com/datastore/{id}/r4/")
}

/// The `DatastoreId` named by a HealthLake data store ARN, or `None` when the
/// ARN is not a HealthLake data store ARN (a malformed one, another service's,
/// or a `dataTransformationProfile` ARN, which names a different resource).
///
/// This reads the ARN's SHAPE only. It says nothing about whether a data store
/// with that id exists, nor about the ARN's partition, region or account — the
/// caller must still require the stored record's own ARN to equal the one it
/// was handed, so an ARN naming another region or account resolves to nothing.
pub fn datastore_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("healthlake"),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if !partition.starts_with("aws") {
        return None;
    }
    resource.strip_prefix(DATASTORE_ARN_RESOURCE_PREFIX)
}

/// Epoch nanoseconds for a modelled timestamp, so two `HealthLakeTimestamp`
/// values compare on one scale whatever precision each carries.
pub fn epoch_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}
