// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-customerprofiles-customerprofiles-20200815 service.
//!
//! The service's one aggregate is the DOMAIN — "a container for all customer data, such
//! as customer profile attributes, object types, profile keys, and encryption keys"
//! (`CreateDomain`). Every profile, and the searchable key index profiles are found
//! through, hangs off its domain, so `DeleteDomain` ("Deletes a specific domain and all
//! of its customer data, such as customer profile attributes and their related objects")
//! drops the whole subtree by removing one map entry.
//!
//! Domains are held in a `BTreeMap` and a domain's profiles in a `BTreeMap` keyed by
//! profile id, so every listing and search walks state in one stable order — which is
//! what makes the `NextToken` cursor below resumable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AttributeDetails, Conditions, CreateCalculatedAttributeDefinitionInput, DataStoreResponse,
    Filter, MatchingResponse, ObjectTypeField, ObjectTypeKey, Profile, Readiness,
    RecommenderSchemaField, ResultsSummary, RuleBasedMatchingResponse,
};

/// How long after `CreateDomain` the asynchronous work a new domain kicks off is treated
/// as finished: the rule-based matching rule reaches `ACTIVE`, and an enabled data store
/// reaches 100% readiness.
///
/// AWS does not document a duration for either, so this is a CHOSEN span. It is
/// deliberately non-zero: a caller that reads the domain back immediately must still
/// observe the transient state the create call reported.
pub const ASYNC_WORK_SPAN: Duration = Duration::from_millis(500);

/// The page size applied when a caller sends no `MaxResults`. `SearchProfiles`
/// documents its `MaxResults` as "The maximum number of objects returned per page. The
/// default is 20 if this parameter is not included in the request." `ListDomains`
/// documents no default for its own `MaxResults` ("The maximum number of objects
/// returned per page"), so it serves the one this service states, which is this value.
pub const DEFAULT_PAGE_SIZE: usize = 20;

/// The largest number of search values one `SearchProfiles` request may carry in total,
/// across its required `Values` list and every `AdditionalSearchKeys[*].Values` list.
///
/// The model bounds the number of search KEYS (one required plus up to four additional)
/// but puts no length on a key's value list, so this is a CHOSEN protective cap. It
/// bounds the index probes, and with [`DEFAULT_PAGE_SIZE`]/`MaxResults` bounding the
/// page it bounds the PRODUCT of page size and value count that describing a page costs.
/// It is far above any real search, which carries a handful of values.
pub const MAX_SEARCH_VALUES: usize = 100;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete, so
// each growth site carries a cap checked BEFORE the insert and nothing caller-sized
// reaches the store unchecked.
//
// The product is therefore finite: at most MAX_DOMAINS domains, each holding at most
// MAX_PROFILES_PER_DOMAIN profiles, each profile indexed under at most the handful of
// standard keys `standard_search_keys` can derive from it, and each caller-sized map on
// a record (`Tags` 1..50, `Attributes` entries 1..255 characters) bounded by the length
// the model declares for it and refused at the boundary before a handler runs.
// ---------------------------------------------------------------------------

/// Customer Profiles' published quota "Domains per AWS account per Region: 100".
pub const MAX_DOMAINS: usize = 100;

/// Emulator resource guard, NOT an AWS quota (the service publishes no per-domain
/// profile limit): the most profiles this emulator stores for one domain. Set well above
/// any realistic emulator workload.
pub const MAX_PROFILES_PER_DOMAIN: usize = 10_000;

/// Emulator resource guard, NOT an AWS quota: the most profile object types this
/// emulator stores for one domain. Set well above any realistic emulator workload, and
/// checked before the insert so repeated `PutProfileObjectType` calls cannot grow the
/// store without bound.
pub const MAX_OBJECT_TYPES_PER_DOMAIN: usize = 1_000;

/// Emulator resource guard, NOT an AWS quota: the most upload jobs this emulator stores
/// for one domain. Set well above any realistic emulator workload, and checked before
/// the insert so repeated `CreateUploadJob` calls cannot grow the store without bound.
pub const MAX_UPLOAD_JOBS_PER_DOMAIN: usize = 1_000;

/// Emulator resource guard, NOT an AWS quota: the most calculated attribute definitions
/// this emulator stores for one domain. Set well above any realistic emulator workload,
/// and checked before the insert so repeated `CreateCalculatedAttributeDefinition` calls
/// cannot grow the store without bound.
pub const MAX_CALCULATED_ATTRIBUTES_PER_DOMAIN: usize = 1_000;

/// How long after `CreateCalculatedAttributeDefinition` the indexing of the historical
/// data the definition asked for is treated as finished.
///
/// AWS documents no duration for it, so this is a CHOSEN span. It is deliberately
/// non-zero: a caller that reads the definition back immediately must still observe the
/// transient status the create call reported.
pub const HISTORICAL_DATA_INDEXING_SPAN: Duration = Duration::from_secs(3);

/// Emulator resource guard, NOT an AWS quota: the most recommender schemas this emulator
/// stores for one domain. Set well above any realistic emulator workload, and checked
/// before the insert so repeated `CreateRecommenderSchema` calls cannot grow the store
/// without bound.
pub const MAX_RECOMMENDER_SCHEMAS_PER_DOMAIN: usize = 1_000;

/// The `RecommenderSchemaStatus` a stored recommender schema reports. The status has two
/// modelled values, `ACTIVE` and `DELETING`, and a schema is usable for training as soon
/// as it is defined, so a stored one is active and nothing but its removal moves it.
pub const RECOMMENDER_SCHEMA_ACTIVE: &str = "ACTIVE";

/// The dataset types a recommender schema may carry columns for, as
/// `CreateRecommenderSchemaRequest$Fields` states them: "The `_webAnalytics` and
/// `_catalogItem` keys are supported." Sorted, so a refusal lists them in one order.
pub const SUPPORTED_RECOMMENDER_DATASET_TYPES: [&str; 2] = ["_catalogItem", "_webAnalytics"];

/// The `ContentType` of a column holding text — the value
/// `RecommenderSchemaField$ContentType` documents as its default.
pub const CONTENT_TYPE_STRING: &str = "STRING";

/// The `ContentType` of a column holding numbers.
pub const CONTENT_TYPE_NUMBER: &str = "NUMBER";

/// The `BadRequestException` message for a dataset type the domain holds no data table
/// for, worded as this service words a resource of a kind it cannot find: the kind, the
/// name the request carried, and "not found" (the same form `ObjectType {name} not found`
/// takes).
pub fn data_table_not_found(dataset_type: &str) -> String {
    format!("DataTable {dataset_type} not found")
}

/// The fewest columns one dataset type of a recommender schema may list: the `@length`
/// minimum of `RecommenderSchemaFieldList`.
pub const MIN_RECOMMENDER_SCHEMA_COLUMNS: usize = 1;

/// The most columns one dataset type of a recommender schema may list: the `@length`
/// maximum of `RecommenderSchemaFieldList`.
pub const MAX_RECOMMENDER_SCHEMA_COLUMNS: usize = 9;

/// This service's ARN namespace — the `profile` every `Arn` on its wire carries
/// (`^arn:([a-z\d-]+):profile:.*:.*:.+$`), which is also its SigV4 signing name.
pub const ARN_SERVICE: &str = "profile";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from: the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ReadinessStatus` while the historical data a calculated attribute asked for is still
/// being indexed — the first of the modelled statuses.
pub const READINESS_PREPARING: &str = "PREPARING";

/// `ReadinessStatus` once every object the calculated attribute covers is indexed, so the
/// attribute is ready for use.
pub const READINESS_COMPLETED: &str = "COMPLETED";

/// The `DataExpiry` an upload job gets when the request carries none.
/// `CreateUploadJobRequest$DataExpiry` documents it: "The expiry duration for the
/// profiles ingested with the job. If not provided, the system default of 2 weeks is
/// used." Two weeks, in the days the member is measured in.
pub const DEFAULT_UPLOAD_JOB_DATA_EXPIRY_DAYS: i32 = 14;

/// The `UploadJobStatus` a job is created in: "CREATED: The upload job has been created,
/// but has not started processing yet." Nothing but a later call moves a job out of it,
/// so it is not a clock-owed transition.
pub const UPLOAD_JOB_CREATED: &str = "CREATED";

/// `RuleBasedMatchingStatus` while the rule is being built. Its documentation: "The
/// system is creating the rule-based matching rule." A domain created with rule-based
/// matching enabled starts here rather than at `PENDING`, because `PENDING` is described
/// as the wait an EXISTING domain serves — "If it is a new domain, the system will skip
/// the PENDING stage."
pub const RULE_BASED_MATCHING_IN_PROGRESS: &str = "IN_PROGRESS";

/// `RuleBasedMatchingStatus` once the rule is built: "The rule is ready to use."
pub const RULE_BASED_MATCHING_ACTIVE: &str = "ACTIVE";

/// Predefined `SearchProfiles` key naming a profile by its own id.
pub const KEY_PROFILE_ID: &str = "_profileId";
/// Predefined `SearchProfiles` key naming a profile by its account number.
pub const KEY_ACCOUNT: &str = "_account";
/// Predefined `SearchProfiles` key naming a profile by any of its email addresses.
pub const KEY_EMAIL: &str = "_email";
/// Predefined `SearchProfiles` key naming a profile by any of its phone numbers.
pub const KEY_PHONE: &str = "_phone";
/// Predefined `SearchProfiles` key naming a profile by its full name.
pub const KEY_FULL_NAME: &str = "_fullName";

/// Per-service state for com-amazonaws-customerprofiles-customerprofiles-20200815. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCustomerprofilesCustomerprofiles20200815State {
    /// Every domain this emulator holds, keyed by `DomainName` — the name is the
    /// domain's identity (`@httpLabel` on every domain-scoped operation).
    pub domains: BTreeMap<String, Domain>,
}

/// One stored domain: the configuration `CreateDomain` persisted, the timestamps it
/// generated, and the customer data nested under it.
#[derive(Debug, Clone)]
pub struct Domain {
    /// `DomainName`, as stored (also this domain's key in [`ComAmazonawsCustomerprofilesCustomerprofiles20200815State::domains`]).
    pub name: String,
    /// `DefaultExpirationDays`, as stored.
    pub default_expiration_days: i32,
    /// `DefaultEncryptionKey`, as stored.
    pub default_encryption_key: Option<String>,
    /// `DeadLetterQueueUrl`, as stored.
    pub dead_letter_queue_url: Option<String>,
    /// `Matching`, as the wire reports it. `MatchingResponse` has no status member, so
    /// the stored value is the whole answer.
    pub matching: Option<MatchingResponse>,
    /// `RuleBasedMatching`, as stored, with `Status` left unset: the status is a
    /// function of the clock, so it is computed on every read by
    /// [`Domain::rule_based_matching_response`] rather than stored.
    pub rule_based_matching: Option<RuleBasedMatchingResponse>,
    /// Whether `DataStore` was requested enabled, as stored. The readiness beside it on
    /// the wire is a function of the clock — see [`Domain::data_store_response`].
    pub data_store_enabled: Option<bool>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated once, here, and never recomputed.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`: the last time the DOMAIN's own configuration changed. Writing a
    /// profile is not an edit of the domain, so it does not move this.
    pub last_updated_at: SystemTime,
    /// When the work [`ASYNC_WORK_SPAN`] covers falls due. Stored as an instant so no
    /// handler ever sleeps for it.
    pub ready_at: SystemTime,
    /// The domain's profiles, keyed by `ProfileId`.
    pub profiles: BTreeMap<String, StoredProfile>,
    /// The domain's profile object types, keyed by `ObjectTypeName` — the name is the
    /// object type's identity (`@httpLabel` on every object-type operation).
    pub object_types: BTreeMap<String, StoredObjectType>,
    /// The domain's upload jobs, keyed by the `JobId` `CreateUploadJob` minted — the id
    /// is the job's identity (`@httpLabel` on every upload-job operation). A `BTreeMap`,
    /// so a listing walks them in one stable order.
    pub upload_jobs: BTreeMap<String, StoredUploadJob>,
    /// The domain's calculated attribute definitions, keyed by `CalculatedAttributeName`
    /// — the name is the definition's identity (`@httpLabel` on every
    /// calculated-attribute operation), so the (domain, name) pair identifies exactly
    /// one definition. A `BTreeMap`, so a listing walks them in one stable order.
    pub calculated_attributes: BTreeMap<String, StoredCalculatedAttribute>,
    /// The domain's recommender schemas, keyed by `RecommenderSchemaName` — the name is
    /// the schema's identity within the domain (`@httpLabel` on every recommender-schema
    /// operation), so the (domain, name) pair identifies exactly one schema. A
    /// `BTreeMap`, so a listing walks them in one stable order.
    pub recommender_schemas: BTreeMap<String, StoredRecommenderSchema>,
    /// `KeyName` -> `Value` -> the ids of the profiles indexed under that pair. This is
    /// what `SearchProfiles` reads; [`Domain::put_profile`] fills it and
    /// [`Domain::remove_profile`] empties it. Nested rather than keyed by a
    /// `(KeyName, Value)` tuple so a lookup probes it with borrowed strings and
    /// allocates nothing per probe.
    pub key_index: HashMap<String, HashMap<String, BTreeSet<String>>>,
}

/// One stored profile object type: the configuration `PutProfileObjectType` persisted
/// and the timestamps the service keeps for it.
///
/// `MaxAvailableProfileObjectCount` is NOT stored: it is the unused part of the count
/// assigned to the type, so it is computed on every read by
/// [`StoredObjectType::max_available_profile_object_count`].
#[derive(Debug, Clone)]
pub struct StoredObjectType {
    /// `ObjectTypeName`, as stored (also this record's key in
    /// [`Domain::object_types`]).
    pub name: String,
    /// `Description`, as stored.
    pub description: String,
    /// `TemplateId`, as stored. When it is set the template supplies the members
    /// `PutProfileObjectType` refuses beside it, so those stay unset here.
    pub template_id: Option<String>,
    /// `ExpirationDays`, as stored.
    pub expiration_days: Option<i32>,
    /// `EncryptionKey`, as stored.
    pub encryption_key: Option<String>,
    /// `AllowProfileCreation`, as stored. The member is modelled with a default of
    /// `false`, so an absent one is stored as `false`.
    pub allow_profile_creation: bool,
    /// `SourceLastUpdatedTimestampFormat`, as stored.
    pub source_last_updated_timestamp_format: Option<String>,
    /// `MaxProfileObjectCount`, as stored.
    pub max_profile_object_count: Option<i32>,
    /// `SourcePriority`, as stored.
    pub source_priority: Option<i32>,
    /// `Fields`, as stored.
    pub fields: Option<HashMap<String, ObjectTypeField>>,
    /// `Keys`, as stored.
    pub keys: Option<HashMap<String, Vec<ObjectTypeKey>>>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated when the object type was first defined, and kept when a
    /// later put redefines it.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`: the last time the definition was written.
    pub last_updated_at: SystemTime,
}

impl StoredObjectType {
    /// `MaxAvailableProfileObjectCount` — "the amount of provisioned profile object max
    /// count available": the count assigned to this type less the profile objects held
    /// under it. A type with no assigned count reports none.
    ///
    /// `objects_held` is the number of profile objects stored for this type, which the
    /// caller counts: no operation stores profile objects yet, so every caller passes 0
    /// today and the whole assignment is available.
    pub fn max_available_profile_object_count(&self, objects_held: i32) -> Option<i32> {
        self.max_profile_object_count
            .map(|assigned| assigned.saturating_sub(objects_held).max(0))
    }
}

/// One stored upload job: the metadata `CreateUploadJob` persisted ("The metadata is
/// created for the job with the provided field mapping and unique key") and the progress
/// the service keeps for it.
///
/// The progress members are stored rather than derived: a job leaves
/// [`UPLOAD_JOB_CREATED`] only when a caller starts or stops it, never because the clock
/// moved, so a read reports exactly what the last call wrote.
#[derive(Debug, Clone)]
pub struct StoredUploadJob {
    /// `JobId`, as minted (also this record's key in [`Domain::upload_jobs`]).
    pub job_id: String,
    /// `DisplayName`, as stored.
    pub display_name: String,
    /// `Fields`, as stored: "The mapping between CSV Columns and Profile Object
    /// attributes."
    pub fields: HashMap<String, ObjectTypeField>,
    /// `UniqueKey`, as stored.
    pub unique_key: String,
    /// `DataExpiry`, as stored: the request's value, or
    /// [`DEFAULT_UPLOAD_JOB_DATA_EXPIRY_DAYS`] when it carried none.
    pub data_expiry: i32,
    /// `Status`, one of the modelled `UploadJobStatus` values.
    pub status: String,
    /// `StatusReason`: why the job is in its current status, set only by a status that
    /// has one.
    pub status_reason: Option<String>,
    /// `CreatedAt`: generated once, when the job was created, and never recomputed.
    pub created_at: SystemTime,
    /// `CompletedAt`: when the job finished processing, so unset until it has.
    pub completed_at: Option<SystemTime>,
    /// `ResultsSummary`: the records the job processed, so unset until it has processed
    /// any.
    pub results_summary: Option<ResultsSummary>,
}

impl StoredUploadJob {
    /// A freshly created job: the metadata the request carried, in
    /// [`UPLOAD_JOB_CREATED`] with nothing processed yet.
    pub fn new(
        job_id: String,
        display_name: String,
        fields: HashMap<String, ObjectTypeField>,
        unique_key: String,
        data_expiry: i32,
        now: SystemTime,
    ) -> Self {
        Self {
            job_id,
            display_name,
            fields,
            unique_key,
            data_expiry,
            status: UPLOAD_JOB_CREATED.to_string(),
            status_reason: None,
            created_at: now,
            completed_at: None,
            results_summary: None,
        }
    }
}

/// One stored calculated attribute definition: the configuration
/// `CreateCalculatedAttributeDefinition` persisted and the timestamps the service keeps
/// for it.
///
/// `Status` and `Readiness` are NOT stored: they report how far the indexing of the
/// definition's historical data has got, which is a function of the clock, so both are
/// computed on every read from [`StoredCalculatedAttribute::indexed_at`].
#[derive(Debug, Clone)]
pub struct StoredCalculatedAttribute {
    /// `CalculatedAttributeName`, as stored (also this record's key in
    /// [`Domain::calculated_attributes`]).
    pub name: String,
    /// `DisplayName`, as stored.
    pub display_name: Option<String>,
    /// `Description`, as stored.
    pub description: Option<String>,
    /// `AttributeDetails`, as stored: the expression and the attributes it names.
    pub attribute_details: AttributeDetails,
    /// `Conditions`, as stored.
    pub conditions: Option<Conditions>,
    /// `Filter`, as stored.
    pub filter: Option<Filter>,
    /// `Statistic`, one of the modelled `Statistic` values.
    pub statistic: String,
    /// `UseHistoricalData`, as stored: whether data ingested before the definition was
    /// created is included, which is what gives the definition anything to index.
    pub use_historical_data: Option<bool>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated once, when the definition was created, and never
    /// recomputed.
    pub created_at: SystemTime,
    /// `LastUpdatedAt`: the last time the definition was written.
    pub last_updated_at: SystemTime,
    /// When the indexing [`HISTORICAL_DATA_INDEXING_SPAN`] covers falls due. Stored as an
    /// instant so no handler ever sleeps for it, and equal to `created_at` for a
    /// definition that asked for no historical data: there is nothing to index, so it is
    /// ready at once.
    pub indexed_at: SystemTime,
}

impl StoredCalculatedAttribute {
    /// A freshly created definition: the configuration the request carried, taken from the
    /// already-validated request itself so no member can be transposed on the way in, with
    /// the instant its historical-data indexing falls due. A definition that did not ask
    /// for historical data has nothing to index, so that instant is `now`.
    pub fn from_request(input: CreateCalculatedAttributeDefinitionInput, now: SystemTime) -> Self {
        let indexed_at = if input.use_historical_data == Some(true) {
            now + HISTORICAL_DATA_INDEXING_SPAN
        } else {
            now
        };
        Self {
            name: input.calculated_attribute_name,
            display_name: input.display_name,
            description: input.description,
            attribute_details: input.attribute_details,
            conditions: input.conditions,
            filter: input.filter,
            statistic: input.statistic,
            use_historical_data: input.use_historical_data,
            tags: input.tags,
            created_at: now,
            last_updated_at: now,
            indexed_at,
        }
    }

    /// Whether every object this definition covers is indexed as of `now`.
    pub fn is_indexed(&self, now: SystemTime) -> bool {
        now >= self.indexed_at
    }

    /// `Status` as the wire reports it: the indexing is either still under way or done.
    pub fn status(&self, now: SystemTime) -> String {
        if self.is_indexed(now) {
            READINESS_COMPLETED.to_string()
        } else {
            READINESS_PREPARING.to_string()
        }
    }

    /// `Readiness` as the wire reports it: how far the indexing has got, as a percentage
    /// in 0..=100 (the range `Readiness$ProgressPercentage` declares), and a sentence
    /// saying whether the attribute can be used yet.
    ///
    /// `Readiness$Message` is documented only as "Any customer messaging", so the two
    /// sentences below are CHOSEN wording, in the same form as the one
    /// [`Domain::data_store_response`] reports for a data store.
    pub fn readiness(&self, now: SystemTime) -> Readiness {
        Readiness {
            progress_percentage: Some(progress_percentage(self.created_at, self.indexed_at, now)),
            message: Some(
                if self.is_indexed(now) {
                    "All historical data has been processed."
                } else {
                    "Historical data is being processed."
                }
                .to_string(),
            ),
        }
    }
}

/// One stored recommender schema: "the set of data columns available for training
/// recommenders and filters under a domain" (`CreateRecommenderSchema`), as the create
/// persisted it, plus the ARN it was minted with and the instant it was created.
///
/// `Status` is NOT stored: a defined schema is usable, so it reports
/// [`RECOMMENDER_SCHEMA_ACTIVE`] for as long as the record exists.
#[derive(Debug, Clone)]
pub struct StoredRecommenderSchema {
    /// `RecommenderSchemaName`, as stored (also this record's key in
    /// [`Domain::recommender_schemas`]).
    pub name: String,
    /// `RecommenderSchemaArn`: minted once, when the schema was created, so a later read
    /// reports the ARN the schema was created with.
    pub arn: String,
    /// `Fields`, as stored: the dataset type -> column definitions map the request
    /// carried.
    pub fields: HashMap<String, Vec<RecommenderSchemaField>>,
    /// `Tags`, as stored.
    pub tags: Option<HashMap<String, String>>,
    /// `CreatedAt`: generated once, when the schema was created, and never recomputed.
    pub created_at: SystemTime,
}

impl StoredRecommenderSchema {
    /// `Status` as the wire reports it: a stored schema is active.
    pub fn status(&self) -> String {
        RECOMMENDER_SCHEMA_ACTIVE.to_string()
    }
}

/// One stored profile: the modelled shape plus the timestamps the service keeps for it.
#[derive(Debug, Clone)]
pub struct StoredProfile {
    /// The profile exactly as it is reported back, `ProfileId` included. `FoundByItems`
    /// is NOT stored: it describes which search keys matched one particular search, so
    /// `SearchProfiles` computes it per response.
    pub profile: Profile,
    /// When the profile was created.
    pub created_at: SystemTime,
    /// When the profile was last written.
    pub last_updated_at: SystemTime,
}

impl Domain {
    /// A new domain holding no customer data: the two members `CreateDomain` requires,
    /// the timestamps it generated, and the instant the asynchronous work its creation
    /// starts falls due. The optional configuration members are set by the caller of
    /// this constructor.
    pub fn new(name: String, default_expiration_days: i32, now: SystemTime) -> Self {
        Self {
            name,
            default_expiration_days,
            default_encryption_key: None,
            dead_letter_queue_url: None,
            matching: None,
            rule_based_matching: None,
            data_store_enabled: None,
            tags: None,
            created_at: now,
            last_updated_at: now,
            ready_at: now + ASYNC_WORK_SPAN,
            profiles: BTreeMap::new(),
            object_types: BTreeMap::new(),
            upload_jobs: BTreeMap::new(),
            calculated_attributes: BTreeMap::new(),
            recommender_schemas: BTreeMap::new(),
            key_index: HashMap::new(),
        }
    }

    /// Whether this domain holds a data table for `dataset_type`.
    ///
    /// A dataset type's data table is the domain's OBJECT TYPE of that name — the
    /// definition the domain ingests that data through — so a domain that defines none for
    /// a dataset type holds no columns a recommender schema could select from it.
    pub fn holds_data_table(&self, dataset_type: &str) -> bool {
        self.object_types.contains_key(dataset_type)
    }

    /// Whether the work [`ASYNC_WORK_SPAN`] covers is due as of `now`.
    pub fn is_ready(&self, now: SystemTime) -> bool {
        now >= self.ready_at
    }

    /// How far through [`ASYNC_WORK_SPAN`] this domain is, as a percentage in 0..=100
    /// (the range `Readiness$ProgressPercentage` declares).
    pub fn progress_percentage(&self, now: SystemTime) -> i32 {
        progress_percentage(self.created_at, self.ready_at, now)
    }

    /// `Matching` as the wire reports it.
    pub fn matching_response(&self) -> Option<MatchingResponse> {
        self.matching.clone()
    }

    /// `RuleBasedMatching` as the wire reports it: the stored configuration with
    /// `Status` resolved from the clock. A configuration that is not enabled builds no
    /// rule, so it reports no status.
    pub fn rule_based_matching_response(
        &self,
        now: SystemTime,
    ) -> Option<RuleBasedMatchingResponse> {
        let mut response = self.rule_based_matching.clone()?;
        response.status = (response.enabled == Some(true)).then(|| {
            if self.is_ready(now) {
                RULE_BASED_MATCHING_ACTIVE.to_string()
            } else {
                RULE_BASED_MATCHING_IN_PROGRESS.to_string()
            }
        });
        Some(response)
    }

    /// `DataStore` as the wire reports it: what was requested, plus the readiness of an
    /// enabled store resolved from the clock.
    pub fn data_store_response(&self, now: SystemTime) -> Option<DataStoreResponse> {
        let enabled = self.data_store_enabled?;
        let readiness = enabled.then(|| Readiness {
            progress_percentage: Some(self.progress_percentage(now)),
            message: Some(
                if self.is_ready(now) {
                    "The data store is ready."
                } else {
                    "The data store is being prepared."
                }
                .to_string(),
            ),
        });
        Some(DataStoreResponse {
            enabled: Some(enabled),
            readiness,
        })
    }

    /// Store `stored` and index it under every standard search key its attributes
    /// populate. A profile already held under the same id is replaced, index included.
    pub fn put_profile(&mut self, stored: StoredProfile) {
        let Some(id) = stored.profile.profile_id.clone() else {
            return;
        };
        self.remove_profile(&id);
        for (key_name, value) in standard_search_keys(&stored.profile) {
            self.key_index
                .entry(key_name)
                .or_default()
                .entry(value)
                .or_default()
                .insert(id.clone());
        }
        self.profiles.insert(id, stored);
    }

    /// Remove the profile and every index entry that pointed at it.
    pub fn remove_profile(&mut self, profile_id: &str) -> Option<StoredProfile> {
        let stored = self.profiles.remove(profile_id)?;
        for (key_name, value) in standard_search_keys(&stored.profile) {
            let Some(by_value) = self.key_index.get_mut(&key_name) else {
                continue;
            };
            if by_value.get_mut(&value).is_some_and(|ids| {
                ids.remove(profile_id);
                ids.is_empty()
            }) {
                by_value.remove(&value);
            }
            if by_value.is_empty() {
                self.key_index.remove(&key_name);
            }
        }
        Some(stored)
    }

    /// The ids of the profiles indexed under `key_name` for ANY of `values` — the
    /// "key-value(s) pair" one `SearchProfiles` search key describes. Ascending by id,
    /// so a page boundary is reproducible.
    pub fn lookup(&self, key_name: &str, values: &[String]) -> BTreeSet<String> {
        let Some(by_value) = self.key_index.get(key_name) else {
            return BTreeSet::new();
        };
        values
            .iter()
            .filter_map(|value| by_value.get(value.as_str()))
            .flatten()
            .cloned()
            .collect()
    }
}

/// The predefined `SearchProfiles` keys a profile's OWN attributes populate.
///
/// `SearchProfiles$KeyName` lists the predefined keys: `_account`, `_profileId`,
/// `_assetId`, `_caseId`, `_orderId`, `_fullName`, `_phone`, `_email`, `_ctrContactId`,
/// and the connector ids (`_marketoLeadId`, `_salesforce*`, `_zendesk*`,
/// `_serviceNow*`, `_segmentUserId`, `_shopify*`). Only the five returned here are
/// derivable from a profile itself; the rest are carried by ingested profile OBJECTS,
/// which no operation stores yet.
///
/// Every email member feeds `_email` and every phone member feeds `_phone`, because
/// those keys name a class of value rather than one member. The result is sorted and
/// deduplicated so indexing and un-indexing walk the same entries.
pub fn standard_search_keys(profile: &Profile) -> Vec<(String, String)> {
    let full = full_name(profile);
    let singles = [
        (KEY_PROFILE_ID, profile.profile_id.as_deref()),
        (KEY_ACCOUNT, profile.account_number.as_deref()),
        (KEY_EMAIL, profile.email_address.as_deref()),
        (KEY_EMAIL, profile.personal_email_address.as_deref()),
        (KEY_EMAIL, profile.business_email_address.as_deref()),
        (KEY_PHONE, profile.phone_number.as_deref()),
        (KEY_PHONE, profile.mobile_phone_number.as_deref()),
        (KEY_PHONE, profile.home_phone_number.as_deref()),
        (KEY_PHONE, profile.business_phone_number.as_deref()),
        (KEY_FULL_NAME, full.as_deref()),
    ];
    let mut keys: Vec<(String, String)> = singles
        .into_iter()
        .filter_map(|(name, value)| {
            let value = value?;
            (!value.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// A profile's full name: its given, middle and family names in that order, space
/// separated, skipping the ones it does not carry. `None` when it carries none.
fn full_name(profile: &Profile) -> Option<String> {
    let parts: Vec<&str> = [
        profile.first_name.as_deref(),
        profile.middle_name.as_deref(),
        profile.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Take one page out of `items`, already in ascending cursor order.
///
/// `next_token` is the cursor a previous page handed back: the page resumes at the first
/// item whose cursor is not before it, so an item deleted between two pages is skipped
/// rather than shifting the window. The returned token is the cursor of the first item
/// that did not fit, and is `None` when the page is the last one.
pub fn paginate<T>(
    items: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let limit = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max.max(1) as usize);
    let mut page = Vec::new();
    let mut token = None;
    for (cursor, item) in items {
        if next_token.is_some_and(|resume| cursor.as_str() < resume) {
            continue;
        }
        if page.len() >= limit {
            token = Some(cursor);
            break;
        }
        page.push(item);
    }
    (page, token)
}

/// How far through a span that started at `started_at` and falls due at `due_at` the
/// clock has got as of `now`, as a percentage in 0..=100 — the range every
/// `Readiness$ProgressPercentage` on this service's wire declares. One definition of the
/// clamp and the rounding, so two resources reporting progress cannot drift apart.
///
/// A span of no length is complete: there is nothing left to wait for.
pub fn progress_percentage(started_at: SystemTime, due_at: SystemTime, now: SystemTime) -> i32 {
    let span = due_at
        .duration_since(started_at)
        .unwrap_or_default()
        .as_secs_f64();
    if span <= 0.0 {
        return 100;
    }
    let elapsed = now
        .duration_since(started_at)
        .unwrap_or_default()
        .as_secs_f64();
    ((elapsed / span) * 100.0).clamp(0.0, 100.0).round() as i32
}

/// A stored instant as the modelled `timestamp` the wire carries.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The longest a tag key may be: the `@length` maximum of `TagMap`'s key shape `TagKey`.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The expression `TagKey` declares, as the model spells it. Its leading negative
/// lookahead is what reserves the `aws:` prefix.
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The prefix the `TagKey` pattern's lookahead excludes: AWS keeps it for its own tags.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why `key` is not a tag key `TagMap` admits at `member`, or `None` when it is one.
///
/// The constraints are the model's own, on the map's KEY shape: `@length(min 1, max 128)`
/// and [`TAG_KEY_PATTERN`]. They are enforced here because a map's key is not a leaf the
/// boundary's constraint walk validates — it captures the key as the path segment of that
/// entry's VALUE, so a key reaches a handler unchecked — and this is the one place a tag
/// key is held to them, shared by every operation that takes a tag map.
///
/// The sentence is NOT written here: it is built from the framework's own violation and
/// rendered by its one envelope, so a key refused by a handler reads exactly like a value
/// the boundary refused for the same constraint class.
pub fn tag_key_violation(member: &str, key: &str) -> Option<String> {
    let length = key.chars().count();
    let kind = if length < 1 {
        ::aws_smithy_schema_server::constraints::ViolationKind::MinLength(1)
    } else if length > MAX_TAG_KEY_LENGTH {
        ::aws_smithy_schema_server::constraints::ViolationKind::MaxLength(
            MAX_TAG_KEY_LENGTH as i64,
        )
    } else if !admits_tag_key(key) {
        ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(
            TAG_KEY_PATTERN.to_string(),
        )
    } else {
        return None;
    };
    let violation = ::aws_smithy_schema_server::constraints::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(member),
        kind,
        // Already held to the key's own modelled maximum, because `Violation::message`
        // interpolates the value verbatim and a key arrives as long as the caller chose.
        value: Some(echoed_tag_key(key)),
        sensitive: false,
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
}

/// The first tag key of `tags` that [`tag_key_violation`] refuses, in ascending key order
/// so one request earns one sentence however the map happened to iterate.
pub fn first_tag_key_violation(
    member: &str,
    tags: Option<&HashMap<String, String>>,
) -> Option<String> {
    let mut keys: Vec<&String> = tags?.keys().collect();
    keys.sort();
    keys.into_iter()
        .find_map(|key| tag_key_violation(member, key))
}

/// Whether every character of `key` is one [`TAG_KEY_PATTERN`] admits, and the key does
/// not carry the reserved prefix.
///
/// The character set is the pattern's own: ASCII letters, the `+`-to-`=` range it spells
/// (which covers the digits and `, - . /` and `: ; < =`), and `.`, `_`, `:`, `/`.
fn admits_tag_key(key: &str) -> bool {
    !key.starts_with(RESERVED_TAG_KEY_PREFIX)
        && key.chars().all(|character| {
            character.is_ascii_alphabetic()
                || ('+'..='=').contains(&character)
                || matches!(character, '.' | '_' | ':' | '/')
        })
}

/// `key` as a refusal quotes it: at most the key's own modelled maximum of characters, so
/// a message cannot be sized by the request.
fn echoed_tag_key(key: &str) -> String {
    key.chars().take(MAX_TAG_KEY_LENGTH).collect()
}

/// The server fault every handler answers with when the store cannot be reached. One
/// wording, so a caller sees the same failure whichever operation it called.
pub fn store_unavailable() -> crate::ops::types::InternalServerException {
    crate::ops::types::InternalServerException {
        message: Some("the Customer Profiles store is unavailable".to_string()),
    }
}

/// The `ResourceNotFoundException` message for a profile object type a domain does not
/// hold, as the service words that refusal: the resource kind, the name the request
/// carried, and "not found". Shared by every operation addressing one object type, so
/// the same absent name reads the same whichever of them was called.
pub fn object_type_not_found(object_type_name: &str) -> String {
    format!("ObjectType {object_type_name} not found")
}

/// The `ResourceNotFoundException` message for a domain this emulator does not hold, as
/// every domain-scoped operation words that refusal: the resource kind and the name the
/// request carried.
pub fn domain_not_found(domain_name: &str) -> String {
    format!("The domain {domain_name} does not exist")
}

/// The `ResourceNotFoundException` message for a recommender schema a domain does not
/// hold, worded like every other per-domain resource this service reports absent: the
/// resource kind, the name the request carried, and the domain it was looked for in.
pub fn recommender_schema_not_found(domain_name: &str, recommender_schema_name: &str) -> String {
    format!(
        "The recommender schema {recommender_schema_name} does not exist in domain {domain_name}"
    )
}

/// One recommender schema's `RecommenderSchemaArn`: this service's ARN namespace, the
/// region the schema was created in, and the domain-scoped resource path the schema is
/// addressed by — the same path its operations are bound to.
pub fn recommender_schema_arn(
    region: &str,
    domain_name: &str,
    recommender_schema_name: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domains/{domain_name}/recommender-schemas/{recommender_schema_name}"),
    )
}

/// The region to build a new resource's ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`]. An ARN is minted once and stored, so a later read from another
/// region still reports the ARN the resource was created with.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCustomerprofilesCustomerprofiles20200815State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCustomerprofilesCustomerprofiles20200815State>)
}
