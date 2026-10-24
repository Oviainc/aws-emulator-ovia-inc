// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-healthlake-healthlake service.
//!
//! The FHIR data store is this service's primary resource: `CreateFHIRDatastore`
//! mints one here and `DescribeFHIRDatastore` / `ListFHIRDatastores` /
//! `TagResource` / `ListTagsForResource` / `UpdateFHIRDatastore` /
//! `DeleteFHIRDatastore` all read and write the SAME record, so a create is
//! observable through every one of them.
//!
//! `RestoreFHIRDatastore` mints one too, from the record a backup-enabled data
//! store already holds here, so the restored data store is observable through
//! exactly the same operations.
//!
//! The data transformation profile is the second: `CreateDataTransformationProfile`
//! mints one in DRAFT here and `GetDataTransformationProfile` reads the SAME
//! record back, version by version.

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

/// The sentence this service refuses a point-in-time restore with when the
/// source data store exists but holds no enabled backup to restore from.
///
/// Fixed, like the two texts above: it names neither the requested
/// `DatastoreId` nor the restore point, so a refusal never quotes the request
/// back at the caller.
pub const DATASTORE_BACKUP_NOT_ENABLED_MESSAGE: &str =
    "Specified datastore does not have backup enabled.";

/// Seconds in a day, for turning a modelled `BackupRetentionPeriodInDays`
/// (range 1..30) into the span it names.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names a data transformation profile no record holds.
///
/// Fixed, for the reason [`DATASTORE_NOT_FOUND_MESSAGE`] is: the same sentence
/// whatever `ProfileId` was asked about, so a not-found answer never quotes the
/// request back at the caller. Every profile not-found path in this crate states
/// it through this one constant.
pub const PROFILE_NOT_FOUND_MESSAGE: &str = "Could not find specified profile.";

/// The sentence this service's `ResourceNotFoundException` carries when the
/// profile exists but not at the version the request asked for — a version that
/// was never published, or a latest published version on a profile that has only
/// its DRAFT. Fixed for the same reason as [`PROFILE_NOT_FOUND_MESSAGE`].
pub const PROFILE_VERSION_NOT_FOUND_MESSAGE: &str =
    "Could not find specified profile version.";

/// The most data stores this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a `CreateFHIRDatastore`
/// loop cannot grow the emulator's heap without bound, and it also bounds the
/// per-request [`ComAmazonawsHealthlakeHealthlakeState::settle_lifecycle`] walk.
/// `CreateFHIRDatastore` refuses past it with the `ThrottlingException` it models
/// — the only quota-shaped refusal that operation declares.
pub const MAX_DATASTORES: usize = 100;

/// The most `ClientToken` entries kept for data-store idempotency, across both
/// operations that mint a data store (`CreateFHIRDatastore` and
/// `RestoreFHIRDatastore`).
///
/// At most one token is recorded per stored data store and a token is dropped
/// with the record it minted, so this cap is never the binding one in practice;
/// it is enforced anyway, by evicting the oldest token, so a stream of fresh
/// caller-chosen tokens cannot grow the map independently of the data stores
/// they name.
pub const MAX_CLIENT_TOKENS: usize = MAX_DATASTORES;

/// The most FHIR resources one data store holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a loop of FHIR
/// creates against the data store's REST endpoint cannot grow the emulator's
/// heap without bound. The data plane refuses past it rather than evicting, so
/// a resource a caller created stays readable for as long as its data store
/// lives.
pub const MAX_FHIR_RESOURCES_PER_DATASTORE: usize = 500;

/// The largest single FHIR resource the data plane stores.
///
/// A count ceiling alone does not bound the retained HEAP: the bytes held would
/// be that count times whatever one request body the transport admits. So the
/// SIZE of one resource is bounded here, the bytes one data store retains by
/// [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`], and the bytes every data store
/// retains together by [`MAX_FHIR_RESOURCE_BYTES_TOTAL`] — so no product of
/// caller-chosen factors is the bound. Like the count, these are ceilings this
/// emulator chooses, not claims about any AWS quota.
pub const MAX_FHIR_RESOURCE_BYTES: usize = 1 << 20;

/// The most bytes of FHIR resources one data store retains.
pub const MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE: usize = 16 << 20;

/// The most bytes of FHIR resources EVERY data store retains together, so
/// [`MAX_DATASTORES`] data stores at their own budget cannot multiply into an
/// unbounded heap.
pub const MAX_FHIR_RESOURCE_BYTES_TOTAL: usize = 64 << 20;

/// The most tags one data store carries.
///
/// The modelled `TagList` bound (0..200) caps ONE request; this caps the
/// accumulation across repeated `TagResource` calls with distinct keys. Both
/// `CreateFHIRDatastore` and `TagResource` refuse past it with the
/// `ValidationException` they model.
pub const MAX_TAGS_PER_DATASTORE: usize = 50;

/// The most tags one data transformation profile carries.
///
/// Taken from the model rather than chosen: a profile's tags are also written
/// by `CreateDataTransformationProfile`'s `Tags` (`TagMap`, len 0..50), so the
/// set a repeated `TagResource` accumulates is held to the same bound the
/// create path is held to. `TagResource` refuses past it with the
/// `ValidationException` it models.
pub const MAX_TAGS_PER_PROFILE: usize = 50;

/// The release-version segment a data store's FHIR endpoint publishes, and so
/// the third segment of every path on that endpoint
/// (`/datastore/{DatastoreId}/r4/...`).
///
/// The modelled `FHIRVersion` enum declares exactly one value (`R4`), which is
/// what [`Datastore::type_version`] holds; the endpoint spells it lowercase.
pub const FHIR_ENDPOINT_VERSION_SEGMENT: &str = "r4";

/// The literal first segment of a data store FHIR endpoint path.
const DATASTORE_PATH_SEGMENT: &str = "datastore";

/// The `datastore` resource prefix of a HealthLake data store ARN, as the
/// modelled `AmazonResourceName` pattern spells it
/// (`...:(datastore\/fhir\/.{32}|dataTransformationProfile\/[a-f0-9]{32})$`).
const DATASTORE_ARN_RESOURCE_PREFIX: &str = "datastore/fhir/";

/// The enabled value of the modelled `BackupStatus` enum
/// (`BACKUP_STATUS_VALUES` = `["DISABLED", "ENABLED"]`).
const BACKUP_ENABLED: &str = "ENABLED";

/// The `dataTransformationProfile` resource prefix of a HealthLake data
/// transformation profile ARN, as the same modelled `AmazonResourceName`
/// pattern spells it, and as the `DataTransformationProfile` resource's
/// `aws.api#arn` template does (`dataTransformationProfile/{ProfileId}`).
const PROFILE_ARN_RESOURCE_PREFIX: &str = "dataTransformationProfile/";

/// The version number that names a data transformation profile's DRAFT.
///
/// `CreateDataTransformationProfile` documents it: "The starting version is
/// always 0, which indicates the profile is in DRAFT state", and
/// `GetDataTransformationProfile` reads it back the same way ("Specify 0 to
/// retrieve the DRAFT version").
pub const DRAFT_PROFILE_VERSION: i32 = 0;

/// The highest version number a data transformation profile reaches, from the
/// modelled `ProfileVersion` range (0..99): the DRAFT is 0, so at most 99
/// published versions exist.
pub const MAX_PROFILE_VERSION: i32 = 99;

/// The most mapping entries one profile version holds, from the modelled
/// `ProfileMapping` map's own length bound (1..500) — the shape
/// `GetDataTransformationProfile` reports the content through, so content past
/// it could be stored but never read back.
pub const MAX_PROFILE_MAPPING_ENTRIES: usize = 500;

/// The fewest mapping entries one profile version holds, from the same
/// modelled `ProfileMapping` length bound (1..500): a profile version always
/// carries content.
pub const MIN_PROFILE_MAPPING_ENTRIES: usize = 1;

/// The longest mapping key, from the modelled `ProfileMappingKey` length bound
/// (1..500).
pub const MAX_PROFILE_MAPPING_KEY_LEN: usize = 500;

/// The longest mapping value, from the modelled `ProfileMappingValue` length
/// bound (0..102400).
pub const MAX_PROFILE_MAPPING_VALUE_LEN: usize = 102_400;

/// The most BYTES one profile version's mapping holds TOGETHER.
///
/// The modelled `ProfileMapping` bounds are all per entry (at most 500 entries, keys
/// 1..500 characters, values 0..102400), so a mapping that satisfies every one of
/// them can still carry ~51 MB — and a count ceiling alone would leave the retained
/// heap a product of caller-chosen factors (the same reason the FHIR data plane
/// budgets [`MAX_FHIR_RESOURCE_BYTES`]). This ceiling is this emulator's, not an AWS
/// quota: it admits ten modelled-maximum files, or all 500 small ones, which is more
/// than a profile needs, and it is what bounds the retained profile content —
/// [`MAX_PROFILES`] profiles times this, because
/// `CreateDataTransformationProfile` is the ONLY operation that writes profile
/// content today (`UpdateDataTransformationProfile` and
/// `PublishDataTransformationProfile` are still 501 scaffolds, so no profile can
/// hold a second version).
///
/// WHOEVER IMPLEMENTS either of those must also charge a per-profile retained-byte
/// budget, the way the FHIR data plane charges
/// [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`] against
/// [`Datastore::fhir_resource_bytes`]: a profile may then hold
/// `1 + MAX_PROFILE_VERSION` versions, and this per-version ceiling alone would stop
/// bounding what the store retains.
///
/// Measured in BYTES (`str::len`) rather than characters, so multi-byte content
/// cannot pass the budget by a factor of four.
pub const MAX_PROFILE_MAPPING_BYTES: usize = 1 << 20;

/// The most data transformation profiles this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a
/// `CreateDataTransformationProfile` loop cannot grow the emulator's heap
/// without bound. `CreateDataTransformationProfile` refuses past it with the
/// `ServiceQuotaExceededException` it models.
pub const MAX_PROFILES: usize = 100;

/// The most `ClientToken` entries kept for `CreateDataTransformationProfile`
/// idempotency — one per profile that can be stored, for the same reason
/// [`MAX_CLIENT_TOKENS`] bounds the data store tokens.
pub const MAX_PROFILE_CLIENT_TOKENS: usize = MAX_PROFILES;

/// The states a data store moves through in this slice, as the modelled
/// `DatastoreStatus` enum names them on the wire. The states the enum also
/// declares (`CREATE_FAILED`, `UPDATING`, `UPDATE_FAILED`) describe a failure,
/// or work that outlives its own call, and nothing in this slice produces
/// either, so nothing here produces them.
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
    /// When this data store's backup configuration became `ENABLED`, which is
    /// what the modelled `DatastoreBackupStatus.BackupEnabledAt` reports ("the
    /// time backup was enabled on the data store"). `None` whenever backup is
    /// not enabled. Written only through [`Datastore::set_backup_configuration`].
    pub backup_enabled_at: Option<::aws_smithy_types::DateTime>,
    pub error_cause: Option<ErrorCause>,
    /// Tags by key, so `TagResource` merges by key and the reported order is
    /// stable across reads. Seeded from `CreateFHIRDatastore`'s `Tags`.
    pub tags: BTreeMap<String, String>,
    /// Every FHIR resource written through this data store's REST endpoint, by
    /// `(resourceType, logical id)`. A `BTreeMap` so the held order is stable
    /// across reads, and held HERE rather than in a map beside the registry so
    /// a data store's resources are dropped with the data store itself.
    /// Bounded by [`MAX_FHIR_RESOURCES_PER_DATASTORE`].
    pub fhir_resources: BTreeMap<(String, String), FhirResource>,
    /// How many BYTES the resources above hold together — the sum of their own
    /// [`FhirResource::bytes`], maintained as they are stored so the data plane
    /// can refuse past [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`] without
    /// re-measuring every stored resource.
    pub fhir_resource_bytes: usize,
}

/// One FHIR resource a data store holds, as the FHIR REST data plane stored it.
///
/// The resource is kept as the JSON a read answers VERBATIM, so two reads of one
/// resource cannot differ and no read re-derives a server-owned value.
#[derive(Debug, Clone)]
pub struct FhirResource {
    /// The `resourceType` the resource carries, which the FHIR create
    /// interaction requires to equal the type segment of the URL it was posted
    /// to.
    pub resource_type: String,
    /// The logical id the server assigned.
    pub id: String,
    /// `meta.versionId`: 1 for a resource that has only been created.
    pub version: u64,
    /// `meta.lastUpdated`, the instant the stored version was written.
    pub last_updated: ::aws_smithy_types::DateTime,
    /// The stored resource JSON: the submitted content with the server-owned
    /// `id` and `meta` written onto it.
    pub content: ::serde_json::Value,
    /// How many bytes that JSON serializes to — what this record costs the
    /// store, so a later delete can give the budget back exactly what the
    /// create took.
    pub bytes: usize,
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
            // Only the configuration half of the backup status is known here,
            // plus the instant that configuration became enabled. The
            // restore-point instants would each have to be derived from the
            // clock on every read — `RestoreFHIRDatastore` holds a requested
            // restore point against the window
            // [`Datastore::restore_window`] derives at the instant it runs —
            // and the scheduled-deletion instant describes retained backup
            // data of a deleted data store, which nothing in this slice keeps,
            // so all three stay unset rather than being invented here and
            // making two identical reads of one data store differ.
            backup_status_info: self
                .backup_configuration
                .clone()
                .map(|configuration| DatastoreBackupStatus {
                    configuration: Some(configuration),
                    backup_enabled_at: self.backup_enabled_at,
                    earliest_restore_point: None,
                    latest_restore_point: None,
                    scheduled_permanent_deletion_time: None,
                }),
        }
    }

    /// The FHIR resource of `resource_type` with logical id `id`, or `None` when
    /// this data store holds no such resource.
    pub fn fhir_resource(&self, resource_type: &str, id: &str) -> Option<&FhirResource> {
        self.fhir_resources
            .get(&(resource_type.to_string(), id.to_string()))
    }

    /// Record `configuration` as this data store's backup configuration, taken
    /// at `at`.
    ///
    /// `DatastoreBackupStatus.BackupEnabledAt` is "the time backup was enabled
    /// on the data store", so it is stamped when the configuration becomes
    /// enabled, left alone while it stays enabled (a later configuration change
    /// does not re-enable anything), and cleared when it is not enabled.
    pub fn set_backup_configuration(
        &mut self,
        configuration: BackupConfiguration,
        at: ::aws_smithy_types::DateTime,
    ) {
        let enabled = configuration.status.as_deref() == Some(BACKUP_ENABLED);
        self.backup_configuration = Some(configuration);
        self.backup_enabled_at = match (enabled, self.backup_enabled_at) {
            (true, already_enabled_at) => already_enabled_at.or(Some(at)),
            (false, _) => None,
        };
    }

    /// The window a point-in-time restore of this data store may name, or `None`
    /// when it holds no enabled backup and so cannot be restored at all.
    ///
    /// Continuous backup is a CONTINUOUS window rather than a set of snapshots,
    /// so it runs from the instant backup became enabled on this data store —
    /// nothing before that was ever backed up — no earlier than the retention
    /// period the configuration names (`BackupConfiguration.RetentionPeriodInDays`,
    /// "the number of days backup data is retained"), up to `now`, the latest
    /// instant a continuous backup covers.
    ///
    /// Derived on every call rather than stored: the latest restorable point
    /// advances with the clock, so there is nothing here to keep up to date and
    /// reading it writes nothing.
    pub fn restore_window(&self, now: SystemTime) -> Option<RestoreWindow> {
        let enabled_at = self.backup_enabled_at?;
        let retention_floor = self
            .backup_configuration
            .as_ref()
            .and_then(|configuration| configuration.retention_period_in_days)
            .filter(|days| *days > 0)
            .and_then(|days| {
                now.checked_sub(Duration::from_secs(
                    u64::from(days.unsigned_abs()).saturating_mul(SECONDS_PER_DAY),
                ))
            })
            .map(::aws_smithy_types::DateTime::from);
        // Whichever bound is LATER is the earliest restorable point: backup
        // covers nothing before it was enabled, and nothing older than the
        // retention period is retained.
        let earliest = match retention_floor {
            Some(floor) if epoch_nanos(&floor) > epoch_nanos(&enabled_at) => floor,
            _ => enabled_at,
        };
        let at = ::aws_smithy_types::DateTime::from(now);
        // A window never ends before it starts, whatever the clock did between
        // the instant backup was enabled and this one.
        let latest = if epoch_nanos(&earliest) > epoch_nanos(&at) {
            earliest
        } else {
            at
        };
        Some(RestoreWindow { earliest, latest })
    }

    /// This data store's FHIR resources as they stood at `at`, with the bytes
    /// they hold together.
    ///
    /// A point-in-time restore reproduces the data store as of one instant, so a
    /// resource whose last write came AFTER that instant was not in the data
    /// store in that form then and is not in the restored one. Nothing is
    /// reconstructed: a resource is copied exactly as this service holds it, or
    /// not at all.
    ///
    /// KNOWN LIMITATION, and the reason this is a `<=` filter rather than a
    /// version walk: [`FhirResource`] holds ONE version — the FHIR update
    /// interaction rewrites it in place and keeps no history — so a resource
    /// created before `at` and updated after it has no version this service can
    /// answer for `at`. It is left out rather than copied at the version it
    /// reached later, because that content did not exist at `at` and a restore
    /// must not invent it. Whoever gives [`FhirResource`] a version history
    /// (charging the retained bytes the way the create and update paths already
    /// charge [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`]) should make this pick
    /// the newest version at or before `at` instead, which is exactly what the
    /// real point-in-time restore does.
    pub fn fhir_resources_as_of(
        &self,
        at: &::aws_smithy_types::DateTime,
    ) -> (BTreeMap<(String, String), FhirResource>, usize) {
        let cutoff = epoch_nanos(at);
        let resources: BTreeMap<(String, String), FhirResource> = self
            .fhir_resources
            .iter()
            .filter(|(_, resource)| epoch_nanos(&resource.last_updated) <= cutoff)
            .map(|(key, resource)| (key.clone(), resource.clone()))
            .collect();
        let bytes = resources
            .values()
            .fold(0usize, |total, resource| total.saturating_add(resource.bytes));
        (resources, bytes)
    }

    /// The tags a restore of this data store inherits: its own tags when its
    /// backup configuration includes them, and none otherwise.
    ///
    /// `BackupConfiguration.BackupTagsEnabled` "specifies whether tags are
    /// included in backups", so a backup taken with it unset carries no tags for
    /// a restore to reproduce.
    pub fn backed_up_tags(&self) -> BTreeMap<String, String> {
        let included = self
            .backup_configuration
            .as_ref()
            .is_some_and(|configuration| configuration.backup_tags_enabled);
        if included {
            self.tags.clone()
        } else {
            BTreeMap::new()
        }
    }

    /// The stored tags as the modelled `TagList`, ordered by key.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// A stored tag map as the modelled `TagList`, ordered by key.
///
/// Every taggable resource in this service holds its tags the same way (a
/// `BTreeMap` keyed by tag key), so `ListTagsForResource` reports all of them
/// through this one function.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The window a point-in-time restore may name on a backup-enabled data store:
/// from the instant backup became enabled (no earlier than the retention period
/// the configuration names) up to now, which is the range
/// `RestoreFHIRDatastore` holds a requested `RestorePointTime` against. The
/// modelled `DatastoreBackupStatus.EarliestRestorePoint` / `LatestRestorePoint`
/// describe the same two instants; [`Datastore::properties`] says why it does
/// not report them.
#[derive(Debug, Clone, Copy)]
pub struct RestoreWindow {
    pub earliest: ::aws_smithy_types::DateTime,
    pub latest: ::aws_smithy_types::DateTime,
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

/// What one `CreateDataTransformationProfile` `ClientToken` recorded: the
/// profile it minted and a fingerprint of the request that minted it.
///
/// Kept apart from [`ClientTokenRecord`] because the two operations mint
/// different resources: one caller's token for a data store must never resolve
/// to a profile, or the other way round.
#[derive(Debug, Clone)]
pub struct ProfileClientTokenRecord {
    pub profile_id: String,
    pub fingerprint: String,
}

/// One version of a data transformation profile: its content, why it changed,
/// and when it last did.
///
/// The DRAFT (version [`DRAFT_PROFILE_VERSION`]) is the mutable one; a
/// published version is an immutable snapshot of the DRAFT taken when it was
/// published, which is why the two are the same record type.
#[derive(Debug, Clone)]
pub struct ProfileVersionRecord {
    pub version: i32,
    /// The profile content, keyed by file path. A `BTreeMap` so the content a
    /// caller reads back is ordered the same way on every read. Satisfies the
    /// modelled `ProfileMapping` bounds — see [`validate_profile_mapping`].
    pub mapping: BTreeMap<String, String>,
    pub change_description: Option<String>,
    pub last_updated_at: ::aws_smithy_types::DateTime,
}

impl ProfileVersionRecord {
    /// The content as the modelled `ProfileMapping`, the shape
    /// `GetDataTransformationProfile` reports it through.
    pub fn profile_mapping(&self) -> ::std::collections::HashMap<String, String> {
        self.mapping
            .iter()
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect()
    }
}

/// One persisted data transformation profile: the metadata
/// `CreateDataTransformationProfile` was given, plus its DRAFT and every
/// version published from it.
#[derive(Debug)]
pub struct DataTransformationProfile {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The format the profile converts FROM, as the modelled `SourceFormat`
    /// enum spells it on the wire.
    pub source_format: String,
    /// The format the profile converts TO. The modelled `TargetFormat` enum
    /// declares exactly one value, which `CreateDataTransformationProfile`
    /// documents as "Always FHIR_R4" — stored rather than re-derived so a
    /// later value of the enum does not silently change an existing profile.
    pub target_format: String,
    pub kms_key_id: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    /// Version [`DRAFT_PROFILE_VERSION`], the one `UpdateDataTransformationProfile`
    /// rewrites. A profile always has one: it is created in DRAFT state.
    pub draft: ProfileVersionRecord,
    /// The published versions, by version number (1..=[`MAX_PROFILE_VERSION`]).
    /// A `BTreeMap` so "the latest published version" is its last key and a
    /// listing is in a stable order.
    pub published: BTreeMap<i32, ProfileVersionRecord>,
    /// Tags by key, seeded from `CreateDataTransformationProfile`'s `Tags`, so
    /// the reported order is stable across reads.
    pub tags: BTreeMap<String, String>,
}

impl DataTransformationProfile {
    /// The version `version` names: the DRAFT at [`DRAFT_PROFILE_VERSION`], a
    /// published snapshot above it, or `None` when this profile has no such
    /// version.
    pub fn version(&self, version: i32) -> Option<&ProfileVersionRecord> {
        if version == DRAFT_PROFILE_VERSION {
            Some(&self.draft)
        } else {
            self.published.get(&version)
        }
    }

    /// The stored tags as the modelled `TagList`, ordered by key — the same
    /// shape `ListTagsForResource` reports a data store's tags through.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// The highest published version, or `None` while the profile has only its
    /// DRAFT — which is what `GetDataTransformationProfile` resolves when the
    /// caller omits the version ("the service returns the latest published
    /// version").
    pub fn latest_published(&self) -> Option<&ProfileVersionRecord> {
        self.published.values().next_back()
    }
}

/// Refuse profile content the modelled `ProfileMapping` could not carry, with
/// the member and the bound it violates.
///
/// The create/update SOURCE shapes take an unconstrained `StringMap`, but the
/// content is read back through `ProfileMapping` (1..500 entries, keys 1..500,
/// values 0..102400), so content past those bounds could be stored and never
/// read. `Ok(())` means every bound holds.
pub fn validate_profile_mapping(mapping: &BTreeMap<String, String>) -> Result<(), String> {
    if mapping.len() < MIN_PROFILE_MAPPING_ENTRIES {
        return Err(format!(
            "ProfileMapping must hold at least {MIN_PROFILE_MAPPING_ENTRIES} entry, got {}",
            mapping.len()
        ));
    }
    if mapping.len() > MAX_PROFILE_MAPPING_ENTRIES {
        return Err(format!(
            "ProfileMapping must hold at most {MAX_PROFILE_MAPPING_ENTRIES} entries, got {}",
            mapping.len()
        ));
    }
    for (path, content) in mapping {
        if path.is_empty() || path.chars().count() > MAX_PROFILE_MAPPING_KEY_LEN {
            return Err(format!(
                "every ProfileMapping key must be 1..{MAX_PROFILE_MAPPING_KEY_LEN} characters long, got one of {}",
                path.chars().count()
            ));
        }
        if content.chars().count() > MAX_PROFILE_MAPPING_VALUE_LEN {
            return Err(format!(
                "every ProfileMapping value must be at most {MAX_PROFILE_MAPPING_VALUE_LEN} characters long, got one of {}",
                content.chars().count()
            ));
        }
    }
    // Every entry can satisfy its own modelled bound and the mapping still be far
    // larger than this emulator retains, so the bytes they hold TOGETHER are bounded
    // too — see [`MAX_PROFILE_MAPPING_BYTES`].
    let bytes = mapping.iter().fold(0usize, |total, (path, content)| {
        total.saturating_add(path.len()).saturating_add(content.len())
    });
    if bytes > MAX_PROFILE_MAPPING_BYTES {
        return Err(format!(
            "ProfileMapping must hold at most {MAX_PROFILE_MAPPING_BYTES} bytes of keys and \
             values together, got {bytes}"
        ));
    }
    Ok(())
}

/// Per-service state for com-amazonaws-healthlake-healthlake. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsHealthlakeHealthlakeState {
    /// Every live data store, by `DatastoreId`. A `BTreeMap` so
    /// `ListFHIRDatastores` pages in a stable, resumable order. Bounded by
    /// [`MAX_DATASTORES`]; a `DELETED` record is reaped after
    /// [`DELETED_RETENTION`].
    pub datastores: BTreeMap<String, Datastore>,
    /// Data-store idempotency: `ClientToken` -> what the first call with that
    /// token minted. ONE map for both operations that mint a data store
    /// (`CreateFHIRDatastore` and `RestoreFHIRDatastore`), so one caller's token
    /// names one data store whichever of them minted it; each records a
    /// fingerprint of its own request, so a token reused with a different
    /// request — including the other operation's — is refused rather than
    /// answered with the earlier data store. Bounded by [`MAX_CLIENT_TOKENS`]; write it through
    /// [`Self::record_client_token`], never directly.
    pub client_tokens: BTreeMap<String, ClientTokenRecord>,
    /// The order [`Self::client_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    client_token_order: VecDeque<String>,
    /// How many data stores have been minted, mixed into the next identifier so
    /// two creates in the same clock tick still get distinct ones.
    pub datastores_created: u64,
    /// How many FHIR resources have been minted across every data store, mixed
    /// into the next logical id for the same reason.
    pub fhir_resources_created: u64,
    /// Every data transformation profile, by `ProfileId`. A `BTreeMap` so
    /// `ListDataTransformationProfiles` pages in a stable, resumable order.
    /// Bounded by [`MAX_PROFILES`].
    pub profiles: BTreeMap<String, DataTransformationProfile>,
    /// `CreateDataTransformationProfile` idempotency: `ClientToken` -> what the
    /// first call with that token minted. Bounded by
    /// [`MAX_PROFILE_CLIENT_TOKENS`]; write it through
    /// [`Self::record_profile_client_token`], never directly.
    pub profile_client_tokens: BTreeMap<String, ProfileClientTokenRecord>,
    /// The order [`Self::profile_client_tokens`] was written in, so the cap
    /// evicts the oldest token rather than an arbitrary one.
    profile_client_token_order: VecDeque<String>,
    /// How many profiles have been minted, mixed into the next identifier so two
    /// creates in the same clock tick still get distinct ones.
    pub profiles_created: u64,
}

impl ComAmazonawsHealthlakeHealthlakeState {
    /// How many BYTES of FHIR resources every data store retains together.
    ///
    /// Summed over the data stores' own counters rather than tracked as a second
    /// total, so a reaped data store gives its bytes back by being dropped and
    /// the two can never disagree. The walk is bounded by [`MAX_DATASTORES`].
    pub fn fhir_resource_bytes_total(&self) -> usize {
        self.datastores
            .values()
            .fold(0usize, |total, datastore| {
                total.saturating_add(datastore.fhir_resource_bytes)
            })
    }

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
        loop {
            self.datastores_created += 1;
            let id = mint_hex_id(
                DATASTORE_ARN_RESOURCE_PREFIX,
                self.datastores_created,
                created_at,
            );
            if !self.datastores.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint the next FHIR logical id a create interaction assigns: a
    /// UUID-shaped, 36-character identifier, which is the form HealthLake's FHIR
    /// endpoint assigns and which satisfies the FHIR `id` type's own alphabet
    /// (`[A-Za-z0-9\-\.]{1,64}`).
    ///
    /// Server-assigned, never derived from the submitted resource: two creates
    /// of identical content get two resources, as FHIR requires. The mint
    /// counter is ADVANCED on every call and mixed in by [`mint_hex_id`] beside
    /// the clock — under this resource kind's own domain, so a FHIR resource and
    /// a control-plane resource cannot collide on one hash — so two creates in
    /// the same tick, and two successive calls with the same instant, still get
    /// different ids. The CALLER checks the minted id against the data store it
    /// is writing to, so a collision cannot overwrite a stored resource.
    pub fn next_fhir_resource_id(&mut self, created_at: SystemTime) -> String {
        self.fhir_resources_created += 1;
        let mixed = mint_hex_id(
            FHIR_RESOURCE_MINT_DOMAIN,
            self.fhir_resources_created,
            created_at,
        );
        // The 8-4-4-4-12 grouping a UUID is written in.
        format!(
            "{}-{}-{}-{}-{}",
            &mixed[..8],
            &mixed[8..12],
            &mixed[12..16],
            &mixed[16..20],
            &mixed[20..32]
        )
    }

    /// Record `token` as having minted `profile_id` from a request with
    /// `fingerprint`, evicting the oldest token first if that would push the map
    /// past [`MAX_PROFILE_CLIENT_TOKENS`].
    pub fn record_profile_client_token(
        &mut self,
        token: String,
        profile_id: &str,
        fingerprint: String,
    ) {
        while self.profile_client_tokens.len() >= MAX_PROFILE_CLIENT_TOKENS {
            match self.profile_client_token_order.pop_front() {
                Some(oldest) => {
                    self.profile_client_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the
                // cap an invariant even if they ever drift.
                None => {
                    self.profile_client_tokens.clear();
                    break;
                }
            }
        }
        if self
            .profile_client_tokens
            .insert(
                token.clone(),
                ProfileClientTokenRecord {
                    profile_id: profile_id.to_string(),
                    fingerprint,
                },
            )
            .is_none()
        {
            self.profile_client_token_order.push_back(token);
        }
    }

    /// Mint an unused `ProfileId`: 32 lowercase hex characters, which is
    /// exactly what the modelled `ProfileIdString` pattern (`^[a-f0-9]{32}$`)
    /// and the profile ARN template both require.
    pub fn next_profile_id(&mut self, created_at: SystemTime) -> String {
        loop {
            self.profiles_created += 1;
            let id = mint_hex_id(PROFILE_ARN_RESOURCE_PREFIX, self.profiles_created, created_at);
            if !self.profiles.contains_key(&id) {
                return id;
            }
        }
    }
}

/// The mint domain a FHIR resource's logical id is derived under, so it cannot
/// collide with a control-plane resource's id for the same counter and instant.
/// Unlike the two ARN prefixes [`mint_hex_id`] is otherwise called with, a FHIR
/// resource has no ARN — it is addressed by its data store's endpoint — so the
/// domain is named here.
const FHIR_RESOURCE_MINT_DOMAIN: &str = "fhir-resource/";

/// One 32-lowercase-hex identifier, the form this service's resource ids and ARNs
/// take, minted from the resource's own `domain` prefix, the mint counter and the
/// creation instant — so an identifier leaks neither verbatim, two creates in the
/// same clock tick still differ, and the two resource kinds cannot collide on one
/// hash. Shared by every mint here; the caller re-mints while the id is taken.
fn mint_hex_id(domain: &str, counter: u64, created_at: SystemTime) -> String {
    use sha2::Digest;

    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = sha2::Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hex::encode(&hasher.finalize()[..16])
}

/// What a request path says about a data store's FHIR endpoint: the data store
/// it addresses, and the FHIR interaction path on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FhirEndpointPath {
    /// The `DatastoreId` the path names. Whether a data store with that id
    /// exists — and whether it is serving — is the handler's question, not this
    /// reader's.
    pub datastore_id: String,
    /// The interaction path relative to the endpoint: the non-empty segments
    /// after the release-version segment (`["Patient"]` for a type-level
    /// interaction, `["Patient", "<id>"]` for an instance-level one).
    pub interaction: Vec<String>,
}

/// Read the data store FHIR endpoint path a request addresses, or `None` when
/// the request is not on such a path.
///
/// The path space is the one [`datastore_endpoint`] publishes —
/// `/datastore/{DatastoreId}/r4/` — so the two are written from the same
/// constants and cannot drift. Empty segments are skipped, so a client that
/// joins an interaction onto an endpoint already ending in `/` (yielding `//`)
/// addresses the same resource as one that does not. Segments are read RAW:
/// every value this path space carries — a `DatastoreId`, a FHIR resource type,
/// a FHIR logical id — is drawn from an alphabet no client percent-encodes.
pub fn parse_datastore_fhir_path(path: &str) -> Option<FhirEndpointPath> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    if segments.next()? != DATASTORE_PATH_SEGMENT {
        return None;
    }
    let datastore_id = segments.next()?;
    if !segments
        .next()?
        .eq_ignore_ascii_case(FHIR_ENDPOINT_VERSION_SEGMENT)
    {
        return None;
    }
    Some(FhirEndpointPath {
        datastore_id: datastore_id.to_string(),
        interaction: segments.map(str::to_string).collect(),
    })
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
///
/// Written from the same two constants [`parse_datastore_fhir_path`] reads, so
/// the endpoint this service advertises is exactly the path space its FHIR REST
/// data plane serves.
pub fn datastore_endpoint(region: &str, id: &str) -> String {
    format!(
        "https://healthlake.{region}.amazonaws.com/{DATASTORE_PATH_SEGMENT}/{id}/{FHIR_ENDPOINT_VERSION_SEGMENT}/"
    )
}

/// The ARN of the data transformation profile `id` in `region`, in the form the
/// modelled `AmazonResourceName` pattern and the `DataTransformationProfile`
/// resource's `aws.api#arn` template both require.
pub fn profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "healthlake",
        region,
        &format!("{PROFILE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// A stable fingerprint of the fields that define one request, for `ClientToken`
/// idempotency: a token resolves to the resource it minted only for a repeat of
/// the SAME request, and a token reused with a different request is refused.
///
/// Fields are hashed LENGTH-PREFIXED, so two different splits of the same bytes
/// cannot fingerprint alike. Every field a caller passes must have a CANONICAL
/// form: order a map (a `BTreeMap`, never the `HashMap` a modelled map
/// deserializes into — its iteration order differs per instance) and render
/// presence explicitly, or two identical requests fingerprint differently and
/// idempotency breaks.
pub fn request_fingerprint<I: IntoIterator<Item = String>>(fields: I) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for field in fields {
        hasher.update(field.len().to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// One optional member as a fingerprint field, so an absent member and an empty
/// one are different requests rather than being collapsed by a default.
pub fn optional_field(member: &str, value: Option<&str>) -> String {
    match value {
        Some(value) => format!("{member}=present:{value}"),
        None => format!("{member}=absent"),
    }
}

/// Whether `id` has the shape of a `ProfileId`, i.e. satisfies the modelled
/// `ProfileIdString` pattern `^[a-f0-9]{32}$`.
///
/// The members that carry a `ProfileId` INSIDE a source union target a plain
/// `String`, so the boundary validator does not hold them to that pattern; a
/// handler reading one must.
pub fn is_profile_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    healthlake_arn_resource(arn)?.strip_prefix(DATASTORE_ARN_RESOURCE_PREFIX)
}

/// The `ProfileId` named by a HealthLake data transformation profile ARN, or
/// `None` when the ARN is not one (a malformed one, another service's, or a
/// `datastore/fhir` ARN, which names a different resource).
///
/// Like [`datastore_id_from_arn`] this reads the ARN's SHAPE only: the id has
/// to satisfy the modelled `ProfileIdString` alphabet the `AmazonResourceName`
/// pattern spells for this form (`dataTransformationProfile\/[a-f0-9]{32}`),
/// but nothing here says a profile with that id exists, and the caller must
/// still require the stored record's own ARN to equal the one it was handed so
/// an ARN naming another partition, region or account resolves to nothing.
pub fn profile_id_from_arn(arn: &str) -> Option<&str> {
    let id = healthlake_arn_resource(arn)?.strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?;
    is_profile_id(id).then_some(id)
}

/// Which HealthLake resource an `AmazonResourceName` names, as the modelled
/// pattern's two resource forms spell them.
///
/// The tagging operations take that one shape for EITHER resource, so they
/// resolve it through [`tagged_resource_from_arn`] rather than each knowing one
/// form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// A FHIR data store, by `DatastoreId`.
    Datastore(&'a str),
    /// A data transformation profile, by `ProfileId`.
    Profile(&'a str),
}

/// The resource a HealthLake `AmazonResourceName` names, or `None` when the ARN
/// is of neither modelled resource form.
///
/// The modelled pattern admits exactly two
/// (`datastore\/fhir\/.{32}` and `dataTransformationProfile\/[a-f0-9]{32}`),
/// and both name a resource this service holds and tags, so every tagging
/// operation resolves its `ResourceARN` here. SHAPE only, as
/// [`datastore_id_from_arn`] and [`profile_id_from_arn`] are.
pub fn tagged_resource_from_arn(arn: &str) -> Option<TaggedResource<'_>> {
    if let Some(id) = datastore_id_from_arn(arn) {
        return Some(TaggedResource::Datastore(id));
    }
    profile_id_from_arn(arn).map(TaggedResource::Profile)
}

/// The RESOURCE field of a HealthLake ARN (everything after the account
/// segment), or `None` when `arn` is not an ARN of this service in an `aws`
/// partition.
///
/// The one place the five leading ARN fields are parsed, so every resource form
/// this service resolves agrees about what an ARN of its own looks like.
fn healthlake_arn_resource(arn: &str) -> Option<&str> {
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
    Some(resource)
}

/// Epoch nanoseconds for a modelled timestamp, so two `HealthLakeTimestamp`
/// values compare on one scale whatever precision each carries.
pub fn epoch_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}
