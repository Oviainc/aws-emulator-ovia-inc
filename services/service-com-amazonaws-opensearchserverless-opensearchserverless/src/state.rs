// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-opensearchserverless-opensearchserverless service.
//!
//! This holds the resources this slice serves — the OpenSearch Serverless
//! **collection**, the **collection group**, the **security policy**, the **data access
//! policy**, the **lifecycle policy** and the **security configuration** — plus everything
//! more than one handler of
//! those resources needs: the server-derived fields (the minted ids, the ARNs, the
//! endpoints derived from a collection id, the epoch-millisecond timestamps), the
//! clock-owed `CREATING -> ACTIVE` transition a just-created collection settles through,
//! the opaque page token a listing hands out, and the `clientToken` replay index the
//! `@idempotent` creates read. Beside the resources sits the ACCOUNT's own settings — the
//! OCU capacity limits `GetAccountSettings` reports ([`AccountSettingsRecord`]).
//!
//! `src/ops/mod.rs` is generated, so a helper more than one operation reaches lives here
//! rather than in a second module under `src/ops/`.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessPolicyDetail, AccountSettingsDetail, CapacityDetails, CapacityLimits, CollectionDetail,
    CollectionErrorDetail, CollectionGroupCapacityLimits, CollectionGroupDetail,
    CollectionGroupErrorDetail, CollectionGroupStorageConfiguration, CollectionSummary,
    CreateCollectionDetail, CreateCollectionGroupDetail, CurrentCapacity, DeleteCollectionDetail,
    FipsEndpoints, IamFederationConfigOptions, IamIdentityCenterConfigOptions,
    LifecyclePolicyDetail, LifecyclePolicyErrorDetail, SamlConfigOptions, SecurityConfigDetail,
    SecurityConfigSummary, SecurityPolicyDetail, Tag, UpdateCollectionGroupDetail, VectorOptions,
};

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`). Also its SigV4
/// signing name and the host label its collection endpoints carry.
pub const ARN_NAMESPACE: &str = "aoss";

/// The host label the FIPS endpoints carry, from the model's own endpoint rule set
/// (`smithy_query(query="service_info")` -> `endpointResolution.urlTemplates`, which lists
/// `https://aoss-fips.{Region}.{PartitionResult#dnsSuffix}` beside the standard one).
pub const FIPS_ARN_NAMESPACE: &str = "aoss-fips";

/// The DNS suffix an `aws`-partition endpoint carries.
pub const DNS_SUFFIX: &str = "amazonaws.com";

/// The path an OpenSearch Dashboards endpoint adds to the collection endpoint.
pub const DASHBOARDS_PATH: &str = "/_dashboards";

/// The region an ARN and an endpoint carry when the request is unsigned, so no SigV4
/// credential scope names one. `engine::emulator_arn`'s own doc fixes this default.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Modelled enum values this slice reports. The wire strings come from the model
// (`smithy_query(query="shape", shape="CollectionStatus")` and friends); the
// `*_VALUES` catalogs in `crate::ops::types` are what a handler validates a
// caller-supplied value against.
// ---------------------------------------------------------------------------

/// `CollectionStatus$CREATING` — "Creating collection resource".
pub const COLLECTION_CREATING: &str = "CREATING";
/// `CollectionStatus$ACTIVE` — "Collection resource is ready to use".
pub const COLLECTION_ACTIVE: &str = "ACTIVE";
/// `CollectionStatus$DELETING` — "Deleting collection resource".
pub const COLLECTION_DELETING: &str = "DELETING";
/// `CollectionStatus$FAILED` — "Collection resource create or delete failed".
pub const COLLECTION_FAILED: &str = "FAILED";
/// `CollectionStatus$UPDATE_FAILED` — "Collection resource update failed".
pub const COLLECTION_UPDATE_FAILED: &str = "UPDATE_FAILED";

/// `DeletionProtection$ENABLED`: "When deletion protection is `ENABLED`, the collection
/// cannot be deleted" (the shape's own documentation).
pub const DELETION_PROTECTION_ENABLED: &str = "ENABLED";
/// `DeletionProtection$DISABLED`; the value a collection carries when the create request
/// names none, so a collection is deletable unless the caller asked otherwise.
pub const DELETION_PROTECTION_DISABLED: &str = "DISABLED";

/// `StandbyReplicas$ENABLED`, the default for a collection whose create request names
/// none: AWS's "Creating and managing collections" documents standby replicas as enabled
/// by default.
pub const STANDBY_REPLICAS_ENABLED: &str = "ENABLED";

/// The `CollectionType` a create request defaults to. From `CreateCollection`'s own
/// documentation of `type`, AWS's default collection type is `TIMESERIES`.
pub const DEFAULT_COLLECTION_TYPE: &str = "TIMESERIES";

/// The `kmsKeyArn` a collection reports when it is encrypted with an AWS-owned key
/// rather than a customer managed key — the case `EncryptionConfig$aWSOwnedKey` selects.
/// OpenSearch Serverless reports the literal `auto` there, because no customer key ARN
/// exists to name.
pub const AWS_OWNED_KEY: &str = "auto";

/// The statuses from which a collection may be deleted, from `ConflictException`'s own
/// documentation: "When deleting a resource, thrown when the resource is not in the
/// ACTIVE, FAILED, or UPDATE_FAILED state."
pub const DELETABLE_STATUSES: [&str; 3] =
    [COLLECTION_ACTIVE, COLLECTION_FAILED, COLLECTION_UPDATE_FAILED];

/// `ServerlessGeneration$CLASSIC`, the generation a collection group carries when its
/// create request does not name one.
///
/// `generation` is OPTIONAL on `CreateCollectionGroupRequest` while every detail shape
/// reports one, so a stored group always carries a generation. `ServerlessGeneration`
/// models exactly `CLASSIC` and `NEXTGEN`, and `NEXTGEN` is the newer generation a caller
/// opts into by naming it, so a request that names none describes a `CLASSIC` group. The
/// VALUE is a choice this emulator makes; the fact that one is always reported is the
/// model's.
pub const SERVERLESS_GENERATION_CLASSIC: &str = "CLASSIC";

/// `ServerlessGeneration$NEXTGEN`, the generation a caller opts into by naming it. It is
/// the generation that runs on the next-generation (v2) storage stack, so it is what
/// [`CollectionGroupRecord::storage_configuration`] reports `v2Enabled` from.
pub const SERVERLESS_GENERATION_NEXTGEN: &str = "NEXTGEN";

// The single policy/configuration type values the account-wide statistics counters are
// keyed by. `GetPoliciesStatsResponse` reports ONE counter per type
// (`AccessPolicyStats$DataPolicyCount`, `SecurityPolicyStats$EncryptionPolicyCount` and
// `$NetworkPolicyCount`, `SecurityConfigStats$SamlConfigCount`,
// `LifecyclePolicyStats$RetentionPolicyCount`), so each counter names the type it counts
// rather than repeating the wire string.

/// `AccessPolicyType$data` — the only value that enum carries
/// (`smithy_query(query="shape", shape="AccessPolicyType")`).
pub const ACCESS_POLICY_DATA: &str = "data";

/// `SecurityPolicyType$encryption` — "Encryption at rest policy type".
pub const SECURITY_POLICY_ENCRYPTION: &str = "encryption";

/// `SecurityPolicyType$network` — "Network policy type".
pub const SECURITY_POLICY_NETWORK: &str = "network";

/// `SecurityConfigType$saml` — the SAML federation configuration type.
pub const SECURITY_CONFIG_SAML: &str = "saml";

// `LifecyclePolicyStats$RetentionPolicyCount`'s own type value is
// [`LIFECYCLE_POLICY_RETENTION`], declared with the retention-rule validation it keys.

/// `AutoscalingStatus$NO_ACTION`, the status a capacity report carries while no scaling
/// action is in progress — the other two members are `ACTION_SCALING_UP` and
/// `ACTION_SCALING_DOWN` (`smithy_query(query="shape", shape="AutoscalingStatus")`).
pub const AUTOSCALING_NO_ACTION: &str = "NO_ACTION";

/// The `errorCode` a `CollectionErrorDetail` carries for an id or name that names no
/// collection, from `CollectionErrorDetail$errorCode`'s own documentation: "The error
/// code for the request. For example, `NOT_FOUND`."
pub const ERROR_CODE_NOT_FOUND: &str = "NOT_FOUND";

/// The `errorMessage` that goes with [`ERROR_CODE_NOT_FOUND`], from
/// `CollectionErrorDetail$errorMessage`'s own documentation: "A description of the error.
/// For example, `The specified Collection is not found.`"
pub const ERROR_MESSAGE_NOT_FOUND: &str = "The specified Collection is not found.";

/// The `errorMessage` a `LifecyclePolicyErrorDetail` carries for an identifier that names
/// no lifecycle policy, from `LifecyclePolicyErrorDetail$errorMessage`'s own
/// documentation: "A description of the error. For example, `The specified Lifecycle Policy
/// is not found`."
pub const ERROR_MESSAGE_LIFECYCLE_POLICY_NOT_FOUND: &str =
    "The specified Lifecycle Policy is not found.";

/// How long after `CreateCollection` the `CREATING -> ACTIVE` transition falls due.
///
/// A span this emulator chooses, not a duration the service documents: a collection is
/// returned in the non-terminal `CREATING` status, so it reaches `ACTIVE` on its own a
/// moment later with nobody calling. It only has to be long enough that a caller acting
/// straight after the create still observes `CREATING` and the state errors that go with
/// it, and short enough to poll through.
const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The account's collection quota in one Region, from AWS's "Quotas for Amazon OpenSearch
/// Serverless": 50 collections per account (adjustable on request, so this is the DEFAULT
/// quota). It bounds the store as well as the API.
pub const MAX_COLLECTIONS: usize = 50;

/// The account's quota for security policies of ONE type in one Region, from the same
/// quota page: 20 encryption policies and 20 network policies per account.
pub const MAX_SECURITY_POLICIES_PER_TYPE: usize = 20;

/// The account's quota for security configurations of ONE type in one Region: 50
/// (adjustable on request, so this is the DEFAULT quota), which is what the real service's
/// own refusal names — "Your request exceeds the limit for saml policies in this AWS
/// account. The limit is 50." — and the same order as the collection quota above, not the
/// 20 the encryption/network policy rows carry. Counted per `SecurityConfigType`, as the
/// policy quota above is counted per `SecurityPolicyType`.
pub const MAX_SECURITY_CONFIGS_PER_TYPE: usize = 50;

/// The `sessionTimeout` a SAML security configuration carries when the create request
/// names none, from `SamlConfigOptions$sessionTimeout`'s own documentation: "The session
/// timeout, in minutes. Default is 60 minutes".
pub const DEFAULT_SAML_SESSION_TIMEOUT: i32 = 60;

/// The `userAttribute` an IAM Identity Center configuration carries when the create
/// request names none, from `CreateIamIdentityCenterConfigOptions$userAttribute`'s own
/// documentation: "Defaults to `UserId`".
pub const DEFAULT_IDENTITY_CENTER_USER_ATTRIBUTE: &str = "UserId";

/// The `groupAttribute` an IAM Identity Center configuration carries when the create
/// request names none, from `CreateIamIdentityCenterConfigOptions$groupAttribute`'s own
/// documentation: "Defaults to `GroupId`".
pub const DEFAULT_IDENTITY_CENTER_GROUP_ATTRIBUTE: &str = "GroupId";

/// The account's data-access-policy quota in one Region, from the same "Quotas for Amazon
/// OpenSearch Serverless" page: 100 data access policies per account (adjustable on
/// request, so this is the DEFAULT quota). `AccessPolicyType` has the single value `data`,
/// so this bounds the whole map.
pub const MAX_ACCESS_POLICIES: usize = 100;

/// The most lifecycle policies this emulator stores for one account in one Region.
///
/// Emulator resource guard, NOT a quota AWS publishes: the "Quotas for Amazon OpenSearch
/// Serverless" page lists rows for collections, security policies and data access policies
/// but none for data lifecycle policies, so this bound exists only to keep the store
/// finite. It is answered with the modelled `ServiceQuotaExceededException` because that is
/// the error `CreateLifecyclePolicy` declares for "more resources than the service allows",
/// and it is set at the data-access-policy figure above, the other per-account policy cap
/// this service carries. `LifecyclePolicyType` has the single value `retention`, so this
/// bounds the whole map.
pub const MAX_LIFECYCLE_POLICIES: usize = MAX_ACCESS_POLICIES;

/// The most collection groups this emulator stores for one account in one Region.
///
/// Emulator resource guard, NOT a quota AWS publishes: the OpenSearch Serverless quota
/// page lists no collection-group limit, so this bound exists only to keep the store
/// finite. It is answered with the modelled `ServiceQuotaExceededException` because that
/// is the error `CreateCollectionGroup` declares for "more resources than the service
/// allows", and it is set at the collection quota above so a group per collection still
/// fits.
pub const MAX_COLLECTION_GROUPS: usize = MAX_COLLECTIONS;

/// The `maxIndexingCapacityInOCU` an account reports before anything calls
/// `UpdateAccountSettings`, from AWS's "Managing capacity limits for Amazon OpenSearch
/// Serverless" — the page `CapacityLimits`' own documentation links — which gives the
/// default maximum capacity as 10 OCUs for indexing and 10 OCUs for search.
pub const DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU: i32 = 10;

/// The `maxSearchCapacityInOCU` default; see [`DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU`].
pub const DEFAULT_MAX_SEARCH_CAPACITY_IN_OCU: i32 = 10;

/// The `CollectionGroupCapacityLimits$maxIndexingCapacityInOCU` a COLLECTION GROUP
/// carries when its create request named no indexing ceiling of its own.
///
/// A group-level ceiling, distinct from the ACCOUNT's
/// [`DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU`] above: the model declares no default for
/// either `CollectionGroupCapacityLimits` member, and the wire recordings this emulator
/// replays show the live API answering a create that named no `capacityLimits` with a
/// defaulted ceiling of 96 OCUs for indexing and 96 for search. The VALUE comes from that
/// recorded default; what the model fixes is that the member is reported at all, since
/// every collection-group detail shape carries `capacityLimits`.
pub const DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU: f32 = 96.0;

/// The group-level `maxSearchCapacityInOCU` default; see
/// [`DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU`].
pub const DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU: f32 = 96.0;

/// The page bound a listing applies when the caller names none, from each listing's own
/// `maxResults` documentation: "The maximum number of results to return. Default is 20."
/// (`ListCollectionsRequest`), "The default is 20." (`ListSecurityConfigsRequest`).
pub const DEFAULT_PAGE_SIZE: i32 = 20;

/// The largest page a listing serves: the maximum of `maxResults`' own
/// `@range(min: 1, max: 100)`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most `clientToken` replay entries this emulator keeps.
///
/// Emulator resource guard, NOT an AWS quota: `ClientToken` is caller-supplied and up to
/// 512 characters, and a create/delete loop frees its resource slot while leaving one
/// token behind, so the resource caps above do not bound this map. It is held at this cap
/// by oldest-first eviction in
/// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::remember_token`], and an entry is dropped as soon as the record it
/// names is deleted. Evicting the oldest token only costs a retry of a create the caller
/// made long ago that very-much-later answer, which then conflicts on the name instead of
/// replaying — the same answer a token whose record was deleted already gets. Set well
/// above the combined resource caps so a realistic retry is always still replayable.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_024;

/// The number of characters a minted `CollectionId` carries. `CollectionId` is
/// `@length(min: 3, max: 40)` with `@pattern("^[a-z0-9]{3,40}$")`; 24 sits inside that and
/// keeps `36^24` inside a `u128` so [`ID_MULTIPLIER`]'s bijection can be computed.
const ID_LEN: u32 = 24;

/// The characters a `CollectionId` is built from: exactly the pattern's class.
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The number of distinct ids [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::mint_collection_id`] can render: `36^24`.
const ID_SPACE: u128 = 36u128.pow(ID_LEN);

/// Scrambles consecutive seeds across [`ID_SPACE`]. Prime and neither 2 nor 3, so it is
/// coprime with `36^24 = 2^48 * 3^48`, which makes
/// `seed -> seed * MULTIPLIER + OFFSET (mod ID_SPACE)` a BIJECTION: distinct seeds can
/// never mint the same collection id.
const ID_MULTIPLIER: u128 = 1_000_000_007;

/// The bijection's constant term, so the first id minted is not a run of leading `a`s.
const ID_OFFSET: u128 = 734_291;

/// The `errorMessage` a `CollectionGroupErrorDetail` carries for an id or name that names
/// no collection group, from `CollectionGroupErrorDetail$errorMessage`'s own
/// documentation: "A description of the error. For example, `The specified Collection
/// Group is not found.`"
pub const ERROR_MESSAGE_GROUP_NOT_FOUND: &str = "The specified Collection Group is not found.";

/// `arn:aws:aoss:<region>:<account>:collection/<id>`.
pub fn collection_arn(region: &str, collection_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("collection/{collection_id}"),
    )
}

/// `arn:aws:aoss:<region>:<account>:collection-group/<id>`.
///
/// The resource half is the `CollectionGroup` resource's own ARN template from the model
/// (`smithy_query(query="shape", shape="CollectionGroup")` -> `arn.template`:
/// `collection-group/{id}`).
pub fn collection_group_arn(region: &str, collection_group_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("collection-group/{collection_group_id}"),
    )
}

/// The collection-specific endpoint index, search and data-upload requests go to:
/// `https://<id>.<region>.aoss.amazonaws.com`. Derived from the collection id alone, so
/// every reader of a record reports the same endpoint.
pub fn collection_endpoint(collection_id: &str, region: &str) -> String {
    format!("https://{collection_id}.{region}.{ARN_NAMESPACE}.{DNS_SUFFIX}")
}

/// The OpenSearch Dashboards endpoint: [`collection_endpoint`] plus
/// [`DASHBOARDS_PATH`].
pub fn dashboard_endpoint(collection_id: &str, region: &str) -> String {
    format!("{}{DASHBOARDS_PATH}", collection_endpoint(collection_id, region))
}

/// The FIPS-compliant collection endpoint: [`collection_endpoint`] with the
/// [`FIPS_ARN_NAMESPACE`] host label the model's FIPS endpoint template uses.
pub fn fips_collection_endpoint(collection_id: &str, region: &str) -> String {
    format!("https://{collection_id}.{region}.{FIPS_ARN_NAMESPACE}.{DNS_SUFFIX}")
}

/// The FIPS-compliant Dashboards endpoint; see [`fips_collection_endpoint`].
pub fn fips_dashboard_endpoint(collection_id: &str, region: &str) -> String {
    format!(
        "{}{DASHBOARDS_PATH}",
        fips_collection_endpoint(collection_id, region)
    )
}

/// Now, as the epoch milliseconds `createdDate` / `lastModifiedDate` are modelled with
/// (`Long`, documented as "The Epoch time when the collection was created").
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The region this request is signed for, which is the region its collections' ARNs and
/// endpoints carry. Falls back to [`DEFAULT_REGION`] for an unsigned request, which
/// carries no credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A status change the clock owes a collection.
///
/// `CollectionStatus` models `CREATING` beside `ACTIVE`, so a collection is reported in
/// it while the resources it describes are brought up, and reaches `ACTIVE` on its own a
/// moment later with nobody calling. The due instant is STORED and applied when a later
/// call observes the record; no handler ever sleeps for it.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The instant from which [`status`](Self::status) is the collection's real status.
    pub due_at: Instant,
    /// The status the collection settles into.
    pub status: &'static str,
}

/// One stored collection. Every field a reader reports is held here; nothing is
/// recomputed from a request.
#[derive(Debug, Clone)]
pub struct CollectionRecord {
    /// The server-minted `CollectionId`, which is also this record's key.
    pub id: String,
    pub name: String,
    /// The `CollectionStatus` wire value, settled through [`PendingStatus`].
    pub status: String,
    /// `CollectionType`; named `kind` because `type` is a Rust keyword.
    pub kind: String,
    pub description: Option<String>,
    pub arn: String,
    pub kms_key_arn: String,
    pub standby_replicas: String,
    pub deletion_protection: String,
    pub vector_options: Option<VectorOptions>,
    pub collection_group_name: Option<String>,
    pub collection_endpoint: String,
    pub dashboard_endpoint: String,
    pub fips_collection_endpoint: String,
    pub fips_dashboard_endpoint: String,
    pub created_date: i64,
    pub last_modified_date: i64,
    /// The tags the create request carried. `CollectionDetail` models no `tags` member,
    /// so they are reached through the tagging operations (still stubs) rather than
    /// echoed on a read.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this collection, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
}

impl CollectionRecord {
    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.status = transient.to_string();
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + SETTLE_AFTER,
            status: settled,
        });
    }

    /// Materialise the transition the clock has already made due, if any.
    ///
    /// This is the one write a reporting operation may make: the change was owed at `now`
    /// whether or not anybody called, so applying it does not change what a later caller
    /// sees — it *is* what a later caller sees.
    pub fn settle(&mut self, now: Instant) {
        if let Some(pending) = self.pending_status.take_if(|pending| now >= pending.due_at) {
            self.status = pending.status.to_string();
        }
    }

    /// The `CreateCollectionDetail` body for this record — the one place this crate
    /// renders that shape.
    pub fn create_detail(&self) -> CreateCollectionDetail {
        CreateCollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.kind.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            vector_options: self.vector_options.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            collection_group_name: self.collection_group_name.clone(),
        }
    }

    /// The `CollectionDetail` body for this record — the one place this crate renders
    /// that shape, so `BatchGetCollection` cannot report a record differently from the
    /// create that stored it.
    pub fn detail(&self) -> CollectionDetail {
        CollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.kind.clone()),
            description: self.description.clone(),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            vector_options: self.vector_options.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            collection_endpoint: Some(self.collection_endpoint.clone()),
            dashboard_endpoint: Some(self.dashboard_endpoint.clone()),
            fips_endpoints: Some(FipsEndpoints {
                collection_endpoint: Some(self.fips_collection_endpoint.clone()),
                dashboard_endpoint: Some(self.fips_dashboard_endpoint.clone()),
            }),
            // `failureCode` / `failureMessage` describe a collection whose create or
            // update failed. Nothing in this slice fails a collection, so a stored record
            // carries neither.
            failure_code: None,
            failure_message: None,
            collection_group_name: self.collection_group_name.clone(),
        }
    }

    /// The `CollectionSummary` body for this record — the shape `ListCollections` pages
    /// over.
    pub fn summary(&self) -> CollectionSummary {
        CollectionSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            arn: Some(self.arn.clone()),
            kms_key_arn: Some(self.kms_key_arn.clone()),
            collection_group_name: self.collection_group_name.clone(),
        }
    }

    /// The `DeleteCollectionDetail` body for this record, as the delete that removed it
    /// reports it.
    pub fn delete_detail(&self) -> DeleteCollectionDetail {
        DeleteCollectionDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
        }
    }
}

/// One stored collection group. Every field a reader reports is held here, except the
/// two a reader DERIVES from the collections beside it (`numberOfCollections` and the
/// current capacity); see [`CollectionGroupRecord::detail`].
#[derive(Debug, Clone)]
pub struct CollectionGroupRecord {
    /// The server-minted `CollectionGroupId`, which is also this record's key.
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The `StandbyReplicas` wire value; `@required` on the create request, so every
    /// stored group carries one.
    pub standby_replicas: String,
    pub description: Option<String>,
    /// The `ServerlessGeneration` wire value, defaulted by
    /// [`SERVERLESS_GENERATION_CLASSIC`] when the create request named none.
    pub generation: String,
    /// The group's EFFECTIVE capacity limits: every member the create (or a later
    /// update) named, plus the service default for each ceiling it did not — see
    /// [`effective_capacity_limits`]. Not an `Option`, because a group always HAS
    /// effective limits: the ones it was asked for, or the defaults it starts at.
    pub capacity_limits: CollectionGroupCapacityLimits,
    /// The tags the create request carried. `CreateCollectionGroupDetail` and
    /// `CollectionGroupDetail` both model `tags`, so unlike a collection a group reports
    /// its own tags on a read.
    pub tags: Vec<Tag>,
    pub created_date: i64,
    /// `lastModifiedDate`, which `UpdateCollectionGroupDetail` reports; equal to
    /// [`created_date`](Self::created_date) until a write moves it.
    pub last_modified_date: i64,
}

impl CollectionGroupRecord {
    /// The `CreateCollectionGroupDetail` body for this record — the one place this crate
    /// renders that shape.
    pub fn create_detail(&self) -> CreateCollectionGroupDetail {
        CreateCollectionGroupDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            description: self.description.clone(),
            tags: Some(self.tags.clone()),
            created_date: Some(self.created_date),
            capacity_limits: Some(self.capacity_limits.clone()),
            generation: Some(self.generation.clone()),
            storage_configuration: Some(self.storage_configuration()),
        }
    }

    /// The `CollectionGroupDetail` body for this record, with the
    /// `numberOfCollections` its reader counted from the collections beside it, so a read
    /// cannot report a group differently from the create that stored it.
    pub fn detail(&self, number_of_collections: i32) -> CollectionGroupDetail {
        CollectionGroupDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            standby_replicas: Some(self.standby_replicas.clone()),
            description: self.description.clone(),
            tags: Some(self.tags.clone()),
            created_date: Some(self.created_date),
            capacity_limits: Some(self.capacity_limits.clone()),
            current_capacity: self.current_capacity(),
            number_of_collections: Some(number_of_collections),
            generation: Some(self.generation.clone()),
            storage_configuration: Some(self.storage_configuration()),
        }
    }

    /// The group's `storageConfiguration`, computed from the generation it stores.
    ///
    /// `v2Enabled` reports whether the group's data is held on the next-generation (v2)
    /// storage stack, which is what [`SERVERLESS_GENERATION_NEXTGEN`] selects: a group's
    /// `generation` is the only setting it carries that chooses a storage stack, so a
    /// `NEXTGEN` group is `v2Enabled: true` and a `CLASSIC` one `false`. Reported on every
    /// detail, so a client can read the group's storage mode off either the create or a
    /// later read.
    ///
    /// Only the `CLASSIC` -> `false` half is backed by a recording; the `NEXTGEN` -> `true`
    /// half is INFERRED from what the two generations are, and a later recording of a
    /// `NEXTGEN` group is what would confirm or correct it.
    fn storage_configuration(&self) -> CollectionGroupStorageConfiguration {
        CollectionGroupStorageConfiguration {
            v2_enabled: Some(self.generation == SERVERLESS_GENERATION_NEXTGEN),
        }
    }

    /// The `UpdateCollectionGroupDetail` body for this record — the one place this crate
    /// renders that shape, so an update cannot report a group differently from the read
    /// that follows it.
    pub fn update_detail(&self) -> UpdateCollectionGroupDetail {
        UpdateCollectionGroupDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            capacity_limits: Some(self.capacity_limits.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
            generation: Some(self.generation.clone()),
        }
    }

    /// Apply the members an `UpdateCollectionGroup` request named, leaving a member it did
    /// NOT name at its stored value: both are optional on a request whose `id` already
    /// identifies the group, and neither is documented as clearing what is stored — the
    /// same partial-write rule [`AccountSettingsRecord::apply`] follows.
    ///
    /// `capacityLimits` is replaced WHOLE rather than merged member by member: the request
    /// carries `CollectionGroupCapacityLimits` as one structure ("Updated capacity limits
    /// for the collection group"), so the limits the request states are the limits the
    /// group has — passed through [`effective_capacity_limits`], so an update that states
    /// no ceiling leaves the group with the service default rather than with none, exactly
    /// as a create does. Every write stamps `lastModifiedDate`.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        capacity_limits: Option<CollectionGroupCapacityLimits>,
        written_at: i64,
    ) {
        if description.is_some() {
            self.description = description;
        }
        if capacity_limits.is_some() {
            self.capacity_limits = effective_capacity_limits(capacity_limits.as_ref());
        }
        self.last_modified_date = written_at;
    }

    /// The group's current search and indexing capacity.
    ///
    /// Nothing in this emulator runs OCUs, so the only capacity a group demonstrably
    /// holds is the FLOOR its own stored limits name — `minSearchCapacityInOCU` and
    /// `minIndexingCapacityInOCU` — and it holds exactly that while no autoscaling action
    /// is in progress ([`AUTOSCALING_NO_ACTION`]). A group whose limits name no floor
    /// reports no current capacity rather than a number nothing measured — and the
    /// defaults [`effective_capacity_limits`] fills in are CEILINGS, so a group created
    /// without capacity limits names no floor.
    fn current_capacity(&self) -> Option<CurrentCapacity> {
        let limits = &self.capacity_limits;
        let at_floor = |floor: Option<f32>| {
            floor.map(|capacity_in_ocu| CapacityDetails {
                capacity_in_ocu: Some(capacity_in_ocu),
                autoscaling_status: Some(AUTOSCALING_NO_ACTION.to_string()),
            })
        };
        let search = at_floor(limits.min_search_capacity_in_ocu);
        let indexing = at_floor(limits.min_indexing_capacity_in_ocu);
        if search.is_none() && indexing.is_none() {
            return None;
        }
        Some(CurrentCapacity { search, indexing })
    }
}

/// The EFFECTIVE capacity limits of a collection group whose create (or update) request
/// named `requested`.
///
/// Every collection-group detail shape models `capacityLimits`, and the live API reports
/// it on a group whose create named none, so a group's limits are never absent: each
/// member the request named is kept as given, and each CEILING it did not name falls back
/// to the service default ([`DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU`],
/// [`DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU`]). A FLOOR stays absent when
/// the request named none: the recorded default names neither `min*` member, and a floor
/// nobody asked for would otherwise claim capacity this emulator never reserved.
///
/// The default is a CEILING the service fills in, not a bound a caller is held to, so a
/// floor named above it is stored as given rather than refused: only the halves one
/// request named itself are checked against each other
/// ([`validate_capacity_limits`]).
pub fn effective_capacity_limits(
    requested: Option<&CollectionGroupCapacityLimits>,
) -> CollectionGroupCapacityLimits {
    CollectionGroupCapacityLimits {
        max_indexing_capacity_in_ocu: requested
            .and_then(|limits| limits.max_indexing_capacity_in_ocu)
            .or(Some(DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU)),
        max_search_capacity_in_ocu: requested
            .and_then(|limits| limits.max_search_capacity_in_ocu)
            .or(Some(DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU)),
        min_indexing_capacity_in_ocu: requested
            .and_then(|limits| limits.min_indexing_capacity_in_ocu),
        min_search_capacity_in_ocu: requested.and_then(|limits| limits.min_search_capacity_in_ocu),
    }
}

/// Refuse `limits` whose floor exceeds its own ceiling, naming the two members and their
/// values.
///
/// `CollectionGroupCapacityLimits` bounds each member on its own (`max*` is
/// `@range(min: 1)`, `min*` is `@range(min: 0)`), which the boundary validator applies
/// before a handler runs; what it cannot express is the relation BETWEEN the pair, so
/// that is checked here. `Ok(())` for limits that name only one half of a pair: a floor
/// with no ceiling, or a ceiling with no floor, contradicts nothing.
pub fn validate_capacity_limits(limits: &CollectionGroupCapacityLimits) -> Result<(), String> {
    for (floor_member, floor, ceiling_member, ceiling) in [
        (
            "minIndexingCapacityInOCU",
            limits.min_indexing_capacity_in_ocu,
            "maxIndexingCapacityInOCU",
            limits.max_indexing_capacity_in_ocu,
        ),
        (
            "minSearchCapacityInOCU",
            limits.min_search_capacity_in_ocu,
            "maxSearchCapacityInOCU",
            limits.max_search_capacity_in_ocu,
        ),
    ] {
        if let (Some(floor), Some(ceiling)) = (floor, ceiling) {
            if floor > ceiling {
                return Err(format!(
                    "capacityLimits.{floor_member} ({floor}) must not exceed \
                     capacityLimits.{ceiling_member} ({ceiling})"
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The account's own settings
// ---------------------------------------------------------------------------

/// The account-level settings `GetAccountSettings` reports and `UpdateAccountSettings`
/// writes: the maximum capacity, in OCUs, this account's collections may scale to.
///
/// Held as plain `i32`s rather than as the modelled `CapacityLimits` (whose members are
/// both optional) because the account always HAS both limits: one it was set to, or the
/// default it starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSettingsRecord {
    /// `CapacityLimits$maxIndexingCapacityInOCU`.
    pub max_indexing_capacity_in_ocu: i32,
    /// `CapacityLimits$maxSearchCapacityInOCU`.
    pub max_search_capacity_in_ocu: i32,
}

impl Default for AccountSettingsRecord {
    /// The documented defaults, so a `GetAccountSettings` on an account nobody has
    /// configured reports the limits AWS starts one at instead of nothing.
    fn default() -> Self {
        Self {
            max_indexing_capacity_in_ocu: DEFAULT_MAX_INDEXING_CAPACITY_IN_OCU,
            max_search_capacity_in_ocu: DEFAULT_MAX_SEARCH_CAPACITY_IN_OCU,
        }
    }
}

impl AccountSettingsRecord {
    /// The `AccountSettingsDetail` body for these settings — the one place this crate
    /// renders that shape, so an update cannot report settings a later
    /// `GetAccountSettings` would not.
    pub fn detail(&self) -> AccountSettingsDetail {
        AccountSettingsDetail {
            capacity_limits: Some(CapacityLimits {
                max_indexing_capacity_in_ocu: Some(self.max_indexing_capacity_in_ocu),
                max_search_capacity_in_ocu: Some(self.max_search_capacity_in_ocu),
            }),
        }
    }

    /// Apply the limits an `UpdateAccountSettings` request named, leaving a limit the
    /// request did NOT name at its stored value: both `CapacityLimits` members are
    /// optional, so a request may raise or lower one limit without restating the other.
    pub fn apply(&mut self, limits: &CapacityLimits) {
        if let Some(value) = limits.max_indexing_capacity_in_ocu {
            self.max_indexing_capacity_in_ocu = value;
        }
        if let Some(value) = limits.max_search_capacity_in_ocu {
            self.max_search_capacity_in_ocu = value;
        }
    }
}

/// The members ONE write to a stored policy replaces, as `UpdateAccessPolicy` and
/// `UpdateSecurityPolicy` carry them.
///
/// Both operations model `description` and `policy` as OPTIONAL on a request whose
/// `type`/`name` already identify the policy, and neither is documented as clearing the
/// stored value, so a member the request did not name keeps what is stored — the same
/// partial-write rule [`AccountSettingsRecord::apply`] follows. The other two members are
/// server-derived: every write mints a fresh `policyVersion`
/// (see [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::next_policy_version`])
/// and stamps `lastModifiedDate`.
#[derive(Debug, Clone)]
pub struct PolicyWrite {
    /// The `description` the request named, or `None` when it named none.
    pub description: Option<String>,
    /// The PARSED `policy` document the request named, or `None` when it named none.
    pub policy: Option<::aws_smithy_types::Document>,
    /// The freshly minted `PolicyVersion` this write stores.
    pub policy_version: String,
    /// The epoch-millisecond instant of the write, which becomes `lastModifiedDate`.
    pub written_at: i64,
}

impl PolicyWrite {
    /// Apply this write to one stored policy's mutable members: the ONE place the merge
    /// rule lives, so a data access policy and a security policy cannot drift apart.
    fn apply(
        self,
        description: &mut Option<String>,
        policy: &mut ::aws_smithy_types::Document,
        policy_version: &mut String,
        last_modified_date: &mut i64,
    ) {
        if self.description.is_some() {
            *description = self.description;
        }
        if let Some(document) = self.policy {
            *policy = document;
        }
        *policy_version = self.policy_version;
        *last_modified_date = self.written_at;
    }
}

/// One stored security policy, keyed by its `(type, name)` pair.
#[derive(Debug, Clone)]
pub struct SecurityPolicyRecord {
    /// The `SecurityPolicyType` wire value; named `kind` because `type` is a keyword.
    pub kind: String,
    pub name: String,
    /// The `PolicyVersion` token, re-minted on every write; see
    /// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::next_policy_version`].
    pub policy_version: String,
    pub description: Option<String>,
    /// The policy document, held PARSED so a read answers "the JSON policy document
    /// without any whitespaces" (`SecurityPolicyDetail$policy`) rather than the caller's
    /// own formatting.
    pub policy: ::aws_smithy_types::Document,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl SecurityPolicyRecord {
    /// The `SecurityPolicyDetail` body for this record — the one place this crate renders
    /// that shape.
    pub fn detail(&self) -> SecurityPolicyDetail {
        SecurityPolicyDetail {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            policy: Some(self.policy.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// Apply `write` to this record; see [`PolicyWrite`] for the rule it follows.
    pub fn apply(&mut self, write: PolicyWrite) {
        write.apply(
            &mut self.description,
            &mut self.policy,
            &mut self.policy_version,
            &mut self.last_modified_date,
        );
    }
}

/// The `SecurityConfigId` the configuration `name` carries under `kind`:
/// `<type>/<account-id>/<name>`, the form AWS's own security-configuration ids take (the
/// `GetSecurityConfig` CLI example addresses one as `saml/123456789012/myprovider`). It is
/// derived from the type, the account and the name alone, so every reader of a record
/// reports the same id, and it stays inside `SecurityConfigId`'s `@length(max: 100)`
/// because `SecurityConfigType` is at most 17 characters and `ConfigName` at most 32.
pub fn security_config_id(kind: &str, name: &str) -> String {
    format!("{kind}/{}/{name}", ::engine::emulator_account_id())
}

/// One stored security configuration, keyed by its [`security_config_id`].
///
/// The identity-provider options are held as the shapes a READ reports
/// (`IamIdentityCenterConfigOptions` rather than the create request's
/// `CreateIamIdentityCenterConfigOptions`), with every documented default already applied,
/// so nothing is recomputed from a request when the record is reported.
#[derive(Debug, Clone)]
pub struct SecurityConfigRecord {
    /// The `SecurityConfigId`, which is also this record's key.
    pub id: String,
    /// The `ConfigName` the create request carried. Not modelled on either reported shape,
    /// but it is half of this record's identity: one name may be used once per type.
    pub name: String,
    /// The `SecurityConfigType` wire value; named `kind` because `type` is a Rust keyword.
    pub kind: String,
    /// The `PolicyVersion` token `configVersion` carries, re-minted on every write; see
    /// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::next_policy_version`].
    pub config_version: String,
    pub description: Option<String>,
    /// Set exactly for a `saml` configuration, which is the type whose options are
    /// required.
    pub saml_options: Option<SamlConfigOptions>,
    /// Set exactly for an `iamidentitycenter` configuration.
    pub iam_identity_center_options: Option<IamIdentityCenterConfigOptions>,
    /// Set exactly for an `iamfederation` configuration.
    pub iam_federation_options: Option<IamFederationConfigOptions>,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl SecurityConfigRecord {
    /// The `SecurityConfigDetail` body for this record — the one place this crate renders
    /// that shape, so a create cannot report a record differently from a later read.
    pub fn detail(&self) -> SecurityConfigDetail {
        SecurityConfigDetail {
            id: Some(self.id.clone()),
            r#type: Some(self.kind.clone()),
            config_version: Some(self.config_version.clone()),
            description: self.description.clone(),
            saml_options: self.saml_options.clone(),
            iam_identity_center_options: self.iam_identity_center_options.clone(),
            iam_federation_options: self.iam_federation_options.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// The `SecurityConfigSummary` body for this record — the shape `ListSecurityConfigs`
    /// pages over. It is `SecurityConfigDetail` without the identity-provider options.
    pub fn summary(&self) -> SecurityConfigSummary {
        SecurityConfigSummary {
            id: Some(self.id.clone()),
            r#type: Some(self.kind.clone()),
            config_version: Some(self.config_version.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }
}

/// One stored data access policy, keyed by its `(type, name)` pair.
///
/// Held beside [`SecurityPolicyRecord`] rather than sharing it: the two are different
/// resources with their own modelled detail shape, their own `type` value list and their
/// own quota, and `GetAccessPolicy` must not be able to read a security policy (or the
/// other way round) just because the names coincide.
#[derive(Debug, Clone)]
pub struct AccessPolicyRecord {
    /// The `AccessPolicyType` wire value; named `kind` because `type` is a keyword.
    pub kind: String,
    pub name: String,
    /// The `PolicyVersion` token, re-minted on every write; see
    /// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::next_policy_version`].
    pub policy_version: String,
    pub description: Option<String>,
    /// The policy document, held PARSED so a read answers "the JSON policy document
    /// without any whitespaces" (`AccessPolicyDetail$policy`) rather than the caller's own
    /// formatting.
    pub policy: ::aws_smithy_types::Document,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl AccessPolicyRecord {
    /// The `AccessPolicyDetail` body for this record — the one place this crate renders
    /// that shape, so a create cannot report a record differently from a later read.
    pub fn detail(&self) -> AccessPolicyDetail {
        AccessPolicyDetail {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            policy: Some(self.policy.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }

    /// Apply `write` to this record; see [`PolicyWrite`] for the rule it follows.
    pub fn apply(&mut self, write: PolicyWrite) {
        write.apply(
            &mut self.description,
            &mut self.policy,
            &mut self.policy_version,
            &mut self.last_modified_date,
        );
    }
}

/// The `LifecyclePolicyType` whose documents carry retention rules: `retention`, the one
/// value the model's own `LifecyclePolicyType` lists
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")`). The document syntax
/// [`validate_retention_policy`] checks is this type's, so the check is keyed on it.
pub const LIFECYCLE_POLICY_RETENTION: &str = "retention";

/// The shortest `MinIndexRetention` a retention rule may name, in hours.
///
/// From AWS's "Amazon OpenSearch Serverless data lifecycle policies", which documents a
/// rule's minimum retention period as a number of hours or days with "a minimum of 24
/// hours" — so a rule asking for less names a window the service does not serve.
pub const MIN_INDEX_RETENTION_HOURS: u64 = 24;

/// The longest `MinIndexRetention` a retention rule may name, in hours: the same page's
/// maximum of 3650 days (ten years), expressed in the hours a window is compared in.
pub const MAX_INDEX_RETENTION_HOURS: u64 = 3650 * 24;

/// The `MinIndexRetention` window `value` names, in hours, or why it names none.
///
/// AWS's data-lifecycle-policy syntax writes the window as a count followed by a unit —
/// `24h` for hours or `7d` for days — so anything else is not a retention period at all.
fn retention_hours(value: &str) -> Result<u64, String> {
    let (count, unit_hours) = match value.as_bytes().last() {
        Some(b'h') => (&value[..value.len() - 1], 1),
        Some(b'd') => (&value[..value.len() - 1], 24),
        _ => {
            return Err(format!(
                "{value:?} must be a count of hours or days, as in \"24h\" or \"7d\""
            ))
        }
    };
    // `str::parse` accepts a leading `+`, which the documented syntax does not: the count
    // is a run of digits and nothing else.
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "{value:?} must be a count of hours or days, as in \"24h\" or \"7d\""
        ));
    }
    let count: u64 = count
        .parse()
        .map_err(|_| format!("{value:?} names more hours than a retention window can hold"))?;
    count
        .checked_mul(unit_hours)
        .ok_or_else(|| format!("{value:?} names more hours than a retention window can hold"))
}

/// Check a `retention` lifecycle policy document against the rule syntax AWS documents,
/// answering the `ValidationException` message for the first thing wrong with it.
///
/// What the model cannot express: `PolicyDocument` is text with a character class, so the
/// boundary validator says nothing about the document's CONTENT. AWS's "Amazon OpenSearch
/// Serverless data lifecycle policies" documents that content — a `Rules` list, each rule
/// naming `ResourceType`, the `Resource` entries it applies to, and either a
/// `MinIndexRetention` window or `NoMinIndexRetention` for unlimited retention, never both
/// — and this service's own model confirms the pair a read reports them as
/// (`EffectiveLifecyclePolicyDetail$retentionPeriod`, "the minimum number of index
/// retention in days or hours", beside `$noMinRetentionPeriod`, "`true` if the minimum
/// number of days or hours is not set") and the single `ResourceType` value, `index`.
pub fn validate_retention_policy(document: &serde_json::Value) -> Result<(), String> {
    let Some(rules) = document.get("Rules") else {
        return Err("policy must carry a Rules list".to_string());
    };
    let Some(rules) = rules.as_array() else {
        return Err("policy.Rules must be a list of rules".to_string());
    };
    if rules.is_empty() {
        return Err("policy.Rules must name at least one rule".to_string());
    }
    for (index, rule) in rules.iter().enumerate() {
        let at = format!("policy.Rules[{index}]");
        if !rule.is_object() {
            return Err(format!("{at} must be an object"));
        }

        // `ResourceType`'s own value list in this service's model is the single value
        // `index` (`smithy_query(query="shape", shape="ResourceType")`).
        let resource_type = rule.get("ResourceType").and_then(serde_json::Value::as_str);
        match resource_type {
            Some(resource_type)
                if crate::ops::types::RESOURCE_TYPE_VALUES.contains(&resource_type) => {}
            Some(resource_type) => {
                return Err(format!(
                    "{at}.ResourceType must be one of [{}], got {resource_type:?}",
                    crate::ops::types::RESOURCE_TYPE_VALUES.join(", ")
                ))
            }
            None => return Err(format!("{at}.ResourceType must name a resource type")),
        }

        let Some(resources) = rule.get("Resource").and_then(serde_json::Value::as_array) else {
            return Err(format!("{at}.Resource must be a list of resource names"));
        };
        if resources.is_empty() {
            return Err(format!("{at}.Resource must name at least one resource"));
        }
        for resource in resources {
            match resource.as_str() {
                Some(resource) if !resource.is_empty() => {}
                _ => {
                    return Err(format!(
                        "{at}.Resource entries must be non-empty resource names"
                    ))
                }
            }
        }

        let no_minimum = match rule.get("NoMinIndexRetention") {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| {
                format!("{at}.NoMinIndexRetention must be true or false")
            })?,
        };
        let minimum = rule.get("MinIndexRetention");
        match (minimum, no_minimum) {
            (Some(_), true) => {
                return Err(format!(
                    "{at} must name either MinIndexRetention or NoMinIndexRetention, not both"
                ))
            }
            (None, false) => {
                return Err(format!(
                    "{at} must name either MinIndexRetention or NoMinIndexRetention"
                ))
            }
            (None, true) => {}
            (Some(minimum), false) => {
                let Some(minimum) = minimum.as_str() else {
                    return Err(format!(
                        "{at}.MinIndexRetention must be a retention window, as in \"24h\""
                    ));
                };
                let hours = retention_hours(minimum)
                    .map_err(|why| format!("{at}.MinIndexRetention {why}"))?;
                if !(MIN_INDEX_RETENTION_HOURS..=MAX_INDEX_RETENTION_HOURS).contains(&hours) {
                    return Err(format!(
                        "{at}.MinIndexRetention must be between {MIN_INDEX_RETENTION_HOURS}h \
                         and {MAX_INDEX_RETENTION_HOURS}h, got {minimum:?}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// One stored lifecycle policy, keyed by its `(type, name)` pair.
///
/// Held beside [`AccessPolicyRecord`] and [`SecurityPolicyRecord`] rather than sharing
/// either: a lifecycle policy is its own resource with its own detail shape, its own `type`
/// value list and its own reader, so `BatchGetLifecyclePolicy` must not be able to read a
/// security or data access policy just because the names coincide.
#[derive(Debug, Clone)]
pub struct LifecyclePolicyRecord {
    /// The `LifecyclePolicyType` wire value; named `kind` because `type` is a keyword.
    pub kind: String,
    pub name: String,
    /// The `PolicyVersion` token, re-minted on every write; see
    /// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::next_policy_version`].
    pub policy_version: String,
    pub description: Option<String>,
    /// The policy document, held PARSED so a read answers "the JSON policy document
    /// without any whitespaces" (`LifecyclePolicyDetail$policy`) rather than the caller's
    /// own formatting.
    pub policy: ::aws_smithy_types::Document,
    pub created_date: i64,
    pub last_modified_date: i64,
}

impl LifecyclePolicyRecord {
    /// The `LifecyclePolicyDetail` body for this record — the one place this crate renders
    /// that shape, so a create cannot report a record differently from a later read.
    pub fn detail(&self) -> LifecyclePolicyDetail {
        LifecyclePolicyDetail {
            r#type: Some(self.kind.clone()),
            name: Some(self.name.clone()),
            policy_version: Some(self.policy_version.clone()),
            description: self.description.clone(),
            policy: Some(self.policy.clone()),
            created_date: Some(self.created_date),
            last_modified_date: Some(self.last_modified_date),
        }
    }
}

/// A `LifecyclePolicyErrorDetail` for an `identifiers` entry that names no stored lifecycle
/// policy.
pub fn lifecycle_policy_error_for(kind: &str, name: &str) -> LifecyclePolicyErrorDetail {
    LifecyclePolicyErrorDetail {
        r#type: Some(kind.to_string()),
        name: Some(name.to_string()),
        error_message: Some(ERROR_MESSAGE_LIFECYCLE_POLICY_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionErrorDetail` for an `ids` entry that names no stored collection.
pub fn collection_error_for_id(id: &str) -> CollectionErrorDetail {
    CollectionErrorDetail {
        id: Some(id.to_string()),
        name: None,
        error_message: Some(ERROR_MESSAGE_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionErrorDetail` for a `names` entry that names no stored collection.
pub fn collection_error_for_name(name: &str) -> CollectionErrorDetail {
    CollectionErrorDetail {
        id: None,
        name: Some(name.to_string()),
        error_message: Some(ERROR_MESSAGE_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionGroupErrorDetail` for an `ids` entry that names no stored collection
/// group.
pub fn collection_group_error_for_id(id: &str) -> CollectionGroupErrorDetail {
    CollectionGroupErrorDetail {
        id: Some(id.to_string()),
        name: None,
        error_message: Some(ERROR_MESSAGE_GROUP_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// A `CollectionGroupErrorDetail` for a `names` entry that names no stored collection
/// group.
pub fn collection_group_error_for_name(name: &str) -> CollectionGroupErrorDetail {
    CollectionGroupErrorDetail {
        id: None,
        name: Some(name.to_string()),
        error_message: Some(ERROR_MESSAGE_GROUP_NOT_FOUND.to_string()),
        error_code: Some(ERROR_CODE_NOT_FOUND.to_string()),
    }
}

/// Per-service state for com-amazonaws-opensearchserverless-opensearchserverless. A handler reaches it through [`service_state`].
///
/// Collections are keyed by the server-minted id, which is their identity in this API
/// (`BatchGetCollection`/`DeleteCollection` address them by it), with a name index beside
/// them so a name lookup and the duplicate-name check are one map probe. Both maps are
/// `BTreeMap`s so `ListCollections` walks one deterministic order a page token can resume.
/// Collection groups are held the same way, and a collection names the group it belongs to
/// by NAME (`CreateCollectionRequest$collectionGroupName`), so a group's
/// `numberOfCollections` is counted over the collections map by that name.
#[derive(Debug, Default)]
pub struct ComAmazonawsOpensearchserverlessOpensearchserverlessState {
    /// Collections by `CollectionId`.
    pub collections: BTreeMap<String, CollectionRecord>,
    /// `CollectionName` -> `CollectionId`, for name-addressed reads and the
    /// duplicate-name conflict check.
    pub collection_name_index: BTreeMap<String, String>,
    /// Collection groups by `CollectionGroupId`, which is their identity in this API
    /// (`BatchGetCollectionGroup`/`UpdateCollectionGroup`/`DeleteCollectionGroup` address
    /// them by it).
    pub collection_groups: BTreeMap<String, CollectionGroupRecord>,
    /// `CollectionGroupName` -> `CollectionGroupId`, for the name-addressed reads
    /// `BatchGetCollectionGroup` serves and the duplicate-name conflict check.
    pub collection_group_name_index: BTreeMap<String, String>,
    /// Security policies by `(SecurityPolicyType, PolicyName)`: the pair that identifies
    /// one, since the same name may be used once per type.
    pub security_policies: BTreeMap<(String, String), SecurityPolicyRecord>,
    /// Security configurations by [`security_config_id`], which is the identity this API
    /// addresses them by (`GetSecurityConfig`/`DeleteSecurityConfig` take `id`). The id
    /// starts with the type and ends with the name, so this one `BTreeMap` already walks
    /// each type's configurations in name order — the stable order
    /// `ListSecurityConfigs`' page token resumes inside.
    pub security_configs: BTreeMap<String, SecurityConfigRecord>,
    /// Data access policies by `(AccessPolicyType, PolicyName)`, the pair that identifies
    /// one, exactly as `security_policies` is keyed.
    pub access_policies: BTreeMap<(String, String), AccessPolicyRecord>,
    /// Lifecycle policies by `(LifecyclePolicyType, PolicyName)`, the pair
    /// `BatchGetLifecyclePolicy`'s own `LifecyclePolicyIdentifier` addresses one by — and
    /// a `BTreeMap`, so `ListLifecyclePolicies` walks one deterministic order a page token
    /// can resume.
    pub lifecycle_policies: BTreeMap<(String, String), LifecyclePolicyRecord>,
    /// `<operation>:<clientToken>` -> the key of the record that token's first call
    /// created, so a replayed `clientToken` on an `@idempotent` create answers the
    /// original record instead of creating a second one.
    ///
    /// PRIVATE, with `idempotency_token_order` beside it, because the two carry one
    /// invariant [`MAX_IDEMPOTENCY_TOKENS`] depends on: they are written only together,
    /// by [`Self::remember_token`] and [`Self::forget_tokens_for`], so the map can never
    /// grow past the cap and no token outlives the record it names.
    idempotency_tokens: BTreeMap<String, String>,
    /// The order [`Self::idempotency_tokens`] was written in, so the cap evicts the
    /// oldest token first.
    idempotency_token_order: VecDeque<String>,
    /// Drives [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::mint_collection_id`] and
    /// [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::mint_collection_group_id`]; see [`ID_MULTIPLIER`] for how an
    /// id is derived from it. ONE seed serves both, so a collection and a collection group
    /// can never be minted the same id.
    pub id_seed: u64,
    /// Drives [`ComAmazonawsOpensearchserverlessOpensearchserverlessState::next_policy_version`], so two writes in the same
    /// millisecond still mint different versions.
    pub policy_version_seed: u32,
    /// The account's own OCU capacity settings: `GetAccountSettings` reports them and
    /// `UpdateAccountSettings` writes them. Defaulted rather than optional, because an
    /// account always has limits — see [`AccountSettingsRecord`].
    pub account_settings: AccountSettingsRecord,
}

impl ComAmazonawsOpensearchserverlessOpensearchserverlessState {
    /// Apply every collection status transition the clock has already made due. Call it
    /// before reading or writing a collection, so no caller observes a status the service
    /// has already moved on from. See [`CollectionRecord::settle`].
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for record in self.collections.values_mut() {
            record.settle(now);
        }
    }

    /// Mint a fresh `CollectionId`: [`ID_LEN`] lowercase alphanumeric characters, the
    /// shape `^[a-z0-9]{3,40}$` admits.
    ///
    /// The id is derived from a counter through the bijection described on
    /// [`ID_MULTIPLIER`], so it is unpredictable-looking yet never repeats.
    pub fn mint_collection_id(&mut self) -> String {
        self.mint_id()
    }

    /// Mint a fresh `CollectionGroupId`. `CollectionGroupId` carries the same
    /// `@length(min: 3, max: 40)` and `^[a-z0-9]{3,40}$` as `CollectionId`, so it is
    /// minted the same way and from the same seed; see [`Self::mint_collection_id`].
    pub fn mint_collection_group_id(&mut self) -> String {
        self.mint_id()
    }

    /// The next id out of the bijection on [`ID_MULTIPLIER`], shared by every resource of
    /// this service whose id shape is `^[a-z0-9]{3,40}$`.
    fn mint_id(&mut self) -> String {
        self.id_seed = self.id_seed.wrapping_add(1);
        let mut scrambled = (u128::from(self.id_seed) * ID_MULTIPLIER + ID_OFFSET) % ID_SPACE;
        let mut id = Vec::with_capacity(ID_LEN as usize);
        for _ in 0..ID_LEN {
            let digit = usize::try_from(scrambled % 36).unwrap_or(0);
            id.push(ID_ALPHABET[digit]);
            scrambled /= 36;
        }
        String::from_utf8(id).unwrap_or_default()
    }

    /// Mint the next `PolicyVersion` for a policy written at `written_at`.
    ///
    /// `PolicyVersion` is `@length(min: 20, max: 36)` with
    /// `@pattern("^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$")`, so
    /// the token is the hex of the write instant followed by a per-process counter: 24
    /// characters out of `[0-9a-f]`, which is six groups of four from the pattern's class
    /// and no padding. The counter is what makes two writes in the same millisecond mint
    /// different versions, so the version changes on EVERY write.
    pub fn next_policy_version(&mut self, written_at: i64) -> String {
        self.policy_version_seed = self.policy_version_seed.wrapping_add(1);
        let mut bytes = [0u8; 12];
        bytes[..8].copy_from_slice(&written_at.to_be_bytes());
        bytes[8..].copy_from_slice(&self.policy_version_seed.to_be_bytes());
        hex::encode(bytes)
    }

    /// The collection `name` resolves to, if one is stored under it.
    pub fn collection_by_name(&self, name: &str) -> Option<&CollectionRecord> {
        let id = self.collection_name_index.get(name)?;
        self.collections.get(id)
    }

    /// Store `record` under its id, indexing its name.
    pub fn insert_collection(&mut self, record: CollectionRecord) {
        self.collection_name_index
            .insert(record.name.clone(), record.id.clone());
        self.collections.insert(record.id.clone(), record);
    }

    /// The collection group `name` resolves to, if one is stored under it.
    pub fn collection_group_by_name(&self, name: &str) -> Option<&CollectionGroupRecord> {
        let id = self.collection_group_name_index.get(name)?;
        self.collection_groups.get(id)
    }

    /// Store `record` under its id, indexing its name.
    pub fn insert_collection_group(&mut self, record: CollectionGroupRecord) {
        self.collection_group_name_index
            .insert(record.name.clone(), record.id.clone());
        self.collection_groups.insert(record.id.clone(), record);
    }

    /// How many stored collections name the collection group `group_name` — the
    /// `numberOfCollections` a collection-group read reports, counted from the collections
    /// themselves rather than from a counter a create bumped.
    pub fn collections_in_group(&self, group_name: &str) -> usize {
        self.collections
            .values()
            .filter(|record| record.collection_group_name.as_deref() == Some(group_name))
            .count()
    }

    /// Remove the collection `id` names, dropping its name index entry and every
    /// idempotency token that named it with it.
    pub fn remove_collection(&mut self, id: &str) -> Option<CollectionRecord> {
        let record = self.collections.remove(id)?;
        self.collection_name_index.remove(&record.name);
        self.forget_tokens_for(id);
        Some(record)
    }

    /// Remove the security policy `(kind, name)` identifies, dropping every idempotency
    /// token that named it with it.
    pub fn remove_security_policy(
        &mut self,
        kind: &str,
        name: &str,
    ) -> Option<SecurityPolicyRecord> {
        let record = self
            .security_policies
            .remove(&(kind.to_string(), name.to_string()))?;
        self.forget_tokens_for(&policy_token_value(kind, name));
        Some(record)
    }

    /// Remove the data access policy `(kind, name)` identifies, dropping every idempotency
    /// token that named it with it — so a later read of that pair raises
    /// `ResourceNotFoundException`, exactly as [`Self::remove_security_policy`] does for a
    /// security policy.
    pub fn remove_access_policy(&mut self, kind: &str, name: &str) -> Option<AccessPolicyRecord> {
        let record = self
            .access_policies
            .remove(&(kind.to_string(), name.to_string()))?;
        self.forget_tokens_for(&policy_token_value(kind, name));
        Some(record)
    }

    /// Remove the security configuration `id` names, dropping every idempotency token
    /// that named it with it.
    pub fn remove_security_config(&mut self, id: &str) -> Option<SecurityConfigRecord> {
        let record = self.security_configs.remove(id)?;
        self.forget_tokens_for(id);
        Some(record)
    }

    /// The record a replayed `clientToken` on `operation` already created, if any.
    ///
    /// `None` both when the token is new and when the record it created has since been
    /// deleted — a replay cannot resurrect a deleted resource, so a stale token simply
    /// falls through to the operation's normal path.
    ///
    /// The handler SCOPES what this answers to the resource its request addresses: a token
    /// spent again on another resource is a request for that other resource, which is still
    /// served on its own terms rather than answered with the record the token first
    /// created. Every caller in `src/ops/` therefore filters on the addressed identity.
    pub fn replayed_collection(&self, operation: &str, token: &str) -> Option<&CollectionRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.collections.get(id)
    }

    /// The collection group a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_collection_group(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&CollectionGroupRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.collection_groups.get(id)
    }

    /// The security policy a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_security_policy(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&SecurityPolicyRecord> {
        let key = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        let (kind, name) = key.split_once('/')?;
        self.security_policies
            .get(&(kind.to_string(), name.to_string()))
    }

    /// The data access policy a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_access_policy(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&AccessPolicyRecord> {
        let key = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        let (kind, name) = key.split_once('/')?;
        self.access_policies
            .get(&(kind.to_string(), name.to_string()))
    }

    /// Remember that `operation` with `token` created the record keyed `key`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// FIRST write wins: a token names the request it was first spent on, so spending it
    /// again on another resource leaves the entry alone — the other resource is created on
    /// its own terms, and a later retry of the FIRST request still replays that one. The
    /// entry is dropped only with the record it names ([`Self::forget_tokens_for`]), after
    /// which the same token may name a new record.
    pub fn remember_token(&mut self, operation: &str, token: &str, key: &str) {
        let entry = idempotency_key(operation, token);
        if self.idempotency_tokens.contains_key(&entry) {
            return;
        }
        while self.idempotency_tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
            match self.idempotency_token_order.pop_front() {
                Some(oldest) => {
                    self.idempotency_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap an
                // invariant even if they ever drift.
                None => {
                    self.idempotency_tokens.clear();
                    break;
                }
            }
        }
        if self
            .idempotency_tokens
            .insert(entry.clone(), key.to_string())
            .is_none()
        {
            self.idempotency_token_order.push_back(entry);
        }
    }

    /// Drop every idempotency token that named the record keyed `key`: a token never
    /// outlives the resource it created, so a create/delete loop cannot accumulate
    /// entries for resources that no longer exist.
    fn forget_tokens_for(&mut self, key: &str) {
        self.idempotency_tokens.retain(|_, created| created != key);
        let live = &self.idempotency_tokens;
        self.idempotency_token_order
            .retain(|token| live.contains_key(token));
    }

    /// How many `clientToken` replay entries are held; [`MAX_IDEMPOTENCY_TOKENS`] bounds
    /// it. Exposed so a test can pin the cap and the eviction.
    pub fn idempotency_token_count(&self) -> usize {
        self.idempotency_tokens.len()
    }

    /// The number of stored security policies of `kind`, which its quota bounds.
    pub fn security_policy_count(&self, kind: &str) -> usize {
        self.security_policies
            .keys()
            .filter(|(policy_kind, _)| policy_kind == kind)
            .count()
    }

    /// The security configuration a replayed `clientToken` on `operation` already created,
    /// if any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_security_config(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&SecurityConfigRecord> {
        let id = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        self.security_configs.get(id)
    }

    /// The number of stored security configurations of `kind`, which its quota bounds.
    pub fn security_config_count(&self, kind: &str) -> usize {
        self.security_configs
            .values()
            .filter(|record| record.kind == kind)
            .count()
    }

    /// The number of stored data access policies, which [`MAX_ACCESS_POLICIES`] bounds.
    pub fn access_policy_count(&self) -> usize {
        self.access_policies.len()
    }

    /// The number of stored access policies of `kind`, which is the per-type counter
    /// `AccessPolicyStats` reports. `AccessPolicyType` carries the single value `data`, so
    /// it equals [`Self::access_policy_count`] — the account-wide total the quota bounds —
    /// for [`ACCESS_POLICY_DATA`] today, and stays per-type if the enum ever grows.
    pub fn access_policy_count_of(&self, kind: &str) -> usize {
        self.access_policies
            .keys()
            .filter(|(policy_kind, _)| policy_kind == kind)
            .count()
    }

    /// The lifecycle policy a replayed `clientToken` on `operation` already created, if
    /// any; see [`Self::replayed_collection`] for why a stale token answers `None`.
    pub fn replayed_lifecycle_policy(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&LifecyclePolicyRecord> {
        let key = self
            .idempotency_tokens
            .get(&idempotency_key(operation, token))?;
        let (kind, name) = key.split_once('/')?;
        self.lifecycle_policies
            .get(&(kind.to_string(), name.to_string()))
    }

    /// The number of stored lifecycle policies, which [`MAX_LIFECYCLE_POLICIES`] bounds.
    pub fn lifecycle_policy_count(&self) -> usize {
        self.lifecycle_policies.len()
    }

    /// The number of stored lifecycle policies of `kind`, which is the per-type counter
    /// `LifecyclePolicyStats` reports. `LifecyclePolicyType` carries the single value
    /// `retention`, so it equals [`Self::lifecycle_policy_count`] — the account-wide total
    /// the quota bounds — for [`LIFECYCLE_POLICY_RETENTION`] today, and stays per-type if
    /// the enum ever grows.
    pub fn lifecycle_policy_count_of(&self, kind: &str) -> usize {
        self.lifecycle_policies
            .keys()
            .filter(|(policy_kind, _)| policy_kind == kind)
            .count()
    }
}

/// The `idempotency_tokens` key for `token` replayed against `operation`. The operation
/// name is part of the key so one `clientToken` reused across two different creates is
/// two independent records, not one.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// The `idempotency_tokens` VALUE that names one stored policy by its `(type, name)`
/// pair. Every policy type value (`encryption`, `network`, `data`) and every `PolicyName`
/// (`^[a-z][a-z0-9-]+$`) is free of the separator, so the pair round-trips unambiguously;
/// the map is keyed by operation name as well, so a token is only ever read back by the
/// create that wrote it.
pub fn policy_token_value(kind: &str, name: &str) -> String {
    format!("{kind}/{name}")
}

/// A record kind a listing pages over.
///
/// The cursor is the key the listing's order is BY, which is also what a page token
/// resumes after — so one paging implementation serves every listing in this service
/// rather than one per resource.
pub trait Paged {
    /// This record's place in its listing's order.
    fn page_cursor(&self) -> &str;
}

impl Paged for CollectionRecord {
    fn page_cursor(&self) -> &str {
        &self.id
    }
}

impl Paged for SecurityConfigRecord {
    fn page_cursor(&self) -> &str {
        &self.id
    }
}

/// One page of a listing: the records it carries, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a, T> {
    /// The records of this page, in the listing's own cursor order.
    pub items: Vec<&'a T>,
    /// Set only when at least one more record remains after this page.
    pub next_token: Option<String>,
}

/// A page token is the cursor the NEXT page resumes after, hex-encoded.
///
/// Hex makes the token opaque in the sense the API promises — it is not an id a client
/// should read or construct — but it is NOT authenticated: hex is reversible, and a
/// hand-made token that decodes to UTF-8 is accepted and simply resumes after that cursor.
/// That is the same answer a token whose record has since been deleted gets, which is
/// what lets a listing resume correctly across creates and deletes rather than failing on
/// them. Nothing a token can say reaches past this listing's own order, so a forged one
/// grants no access: it can only start the same listing at a different place.
fn encode_page_token(cursor: &str) -> String {
    hex::encode(cursor)
}

/// The cursor a token resumes after, or the reason the token cannot be one this service
/// issued (its bytes are not hex, or not UTF-8).
fn decode_page_token(token: &str) -> Result<String, String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| "nextToken is not a pagination token this service issued".to_string())
}

/// The page of `ordered` that `next_token` and `max_results` select.
///
/// `ordered` is the already-filtered record sequence in cursor order — the stable
/// sequence a token resumes inside. `max_results` is a page BOUND, not a count: a page
/// holds at most that many records, and `next_token` is returned only while more remain.
pub fn page<'a, T: Paged>(
    ordered: Vec<&'a T>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, T>, String> {
    let bound = max_results.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let bound = usize::try_from(bound).unwrap_or(1);
    let after = match next_token {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };

    let mut remaining = ordered.into_iter().filter(|record| {
        after
            .as_deref()
            .is_none_or(|from| record.page_cursor() > from)
    });
    let items: Vec<&'a T> = remaining.by_ref().take(bound).collect();
    // A token only when a record is left over: the last page of a listing carries none.
    let next_token = remaining
        .next()
        .and_then(|_| items.last().map(|record| encode_page_token(record.page_cursor())));

    Ok(Page { items, next_token })
}

/// The deepest JSON nesting this emulator converts into an `aws_smithy_types::Document`.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: [`document_from_json`] recurses once per
/// nesting level over a CALLER-SUPPLIED document, and so does the resulting `Document`
/// tree's own drop, while a Rust stack overflow aborts the whole process instead of
/// failing one request. 64 levels is far past any real policy document — a data access
/// policy nests about four — so a document past it is refused with the modelled
/// `ValidationException` rather than converted. The bound is enforced HERE and does not
/// rely on `serde_json`'s own parse-time recursion limit.
pub const MAX_DOCUMENT_DEPTH: usize = 64;

/// The `aws_smithy_types::Document` a parsed JSON value is, so a policy document read
/// back is the document the caller supplied rather than its text.
///
/// `None` when the value nests deeper than [`MAX_DOCUMENT_DEPTH`]: the caller answers that
/// with the modelled `ValidationException`.
pub fn document_from_json(value: &serde_json::Value) -> Option<::aws_smithy_types::Document> {
    document_from_json_at(value, 1)
}

/// [`document_from_json`] at `depth`, where the top-level value is depth 1 and a
/// container's members are one deeper. The depth check is what bounds the recursion.
fn document_from_json_at(
    value: &serde_json::Value,
    depth: usize,
) -> Option<::aws_smithy_types::Document> {
    use ::aws_smithy_types::{Document, Number};
    if depth > MAX_DOCUMENT_DEPTH {
        return None;
    }
    Some(match value {
        serde_json::Value::Null => Document::Null,
        serde_json::Value::Bool(value) => Document::Bool(*value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_u64() {
                Document::Number(Number::PosInt(value))
            } else if let Some(value) = value.as_i64() {
                Document::Number(Number::NegInt(value))
            } else {
                Document::Number(Number::Float(value.as_f64().unwrap_or_default()))
            }
        }
        serde_json::Value::String(value) => Document::String(value.clone()),
        serde_json::Value::Array(items) => Document::Array(
            items
                .iter()
                .map(|item| document_from_json_at(item, depth + 1))
                .collect::<Option<Vec<_>>>()?,
        ),
        serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| {
                    document_from_json_at(member, depth + 1).map(|member| (name.clone(), member))
                })
                .collect::<Option<_>>()?,
        ),
    })
}

// ---------------------------------------------------------------------------
// The audit record a call emits
// ---------------------------------------------------------------------------

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a policy name are all caller-controlled, so
/// an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 256;

/// What the `resource` field reads as when the call resolved no resource.
pub const AUDIT_NO_RESOURCE: &str = "-";

/// A request-derived value as it may appear in an audit record: control characters (above
/// all CR and LF, which would forge a second record) replaced, and the whole value bounded
/// by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|character| if character.is_control() { '.' } else { character })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A policy was minted and stored.
    PolicyCreated,
    /// A stored policy was replaced by a write (`UpdateAccessPolicy`).
    PolicyUpdated,
    /// A stored policy was removed (`DeleteAccessPolicy`).
    PolicyDeleted,
    /// The `policyVersion` the write carried is not the stored one, so the write was
    /// refused (409 `ConflictException`).
    VersionMismatch,
    /// A replayed `clientToken` answered the record its first call created; nothing was
    /// written.
    IdempotentReplay,
    /// A stored policy was reported to a reader.
    PolicyRead,
    /// The account's own settings were written (`UpdateAccountSettings`).
    SettingsUpdated,
    /// The request was refused as invalid (400 `ValidationException`).
    ValidationRefused,
    /// A policy with that `(type, name)` pair already exists (409 `ConflictException`).
    AlreadyExists,
    /// An account quota was met (402 `ServiceQuotaExceededException`).
    QuotaExceeded,
    /// The `(type, name)` pair names no stored policy (404 `ResourceNotFoundException`).
    NotFound,
    /// This service's store could not be reached (500 `InternalServerException`).
    StoreUnreachable,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PolicyCreated => "policy-created",
            Self::PolicyUpdated => "policy-updated",
            Self::PolicyDeleted => "policy-deleted",
            Self::VersionMismatch => "version-mismatch",
            Self::IdempotentReplay => "idempotent-replay",
            Self::PolicyRead => "policy-read",
            Self::SettingsUpdated => "settings-updated",
            Self::ValidationRefused => "validation-refused",
            Self::AlreadyExists => "already-exists",
            Self::QuotaExceeded => "quota-exceeded",
            Self::NotFound => "not-found",
            Self::StoreUnreachable => "store-unreachable",
        }
    }
}

/// One structured audit record for a call: which operation, the identity the request
/// presented, what it was addressed to, the resource it resolved and the outcome. The
/// `tracing` subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one: the
/// request's parts carry its method, URI, headers and extensions, and nothing in this
/// framework puts a peer address in them, so the presented credential is what identifies
/// the caller — the access key id only, never the credential itself. Deliberately metadata
/// only: no policy document or description is logged, so a caller's own data never reaches
/// the log. Every interpolated value is request-derived, so [`audit_field`] bounds it and
/// replaces the control characters that would otherwise let one request forge a second
/// record.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
    resource: Option<&str>,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ARN_NAMESPACE,
        operation = %audit_field(operation),
        access_key_id = %audit_field(&::emulator_server::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource.unwrap_or(AUDIT_NO_RESOURCE)),
        outcome = outcome.as_str(),
        "opensearchserverless api call"
    );
}

/// The `resource` field naming one data access policy: its type and name, which is its
/// identity in this API. Named for the access policy because the prefix is fixed: another
/// resource's records name their own.
pub fn audit_access_policy_resource(kind: &str, name: &str) -> String {
    format!("access-policy/{kind}/{name}")
}

/// The `resource` field naming one lifecycle policy: its type and name, which is its
/// identity in this API (`LifecyclePolicyIdentifier` is exactly that pair).
pub fn audit_lifecycle_policy_resource(kind: &str, name: &str) -> String {
    format!("lifecycle-policy/{kind}/{name}")
}

/// The `resource` field naming the account's own settings, which `UpdateAccountSettings`
/// writes. A single resource per account, so it is a constant rather than a function:
/// nothing request-derived identifies it.
pub const AUDIT_ACCOUNT_SETTINGS_RESOURCE: &str = "account-settings";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOpensearchserverlessOpensearchserverlessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOpensearchserverlessOpensearchserverlessState>)
}

/// What a handler answers when [`service_state`] cannot reach this service's store: the
/// modelled `InternalServerException`, which every operation in this slice declares.
pub const STORE_UNREACHABLE: &str =
    "the OpenSearch Serverless emulator's state store is not reachable from this engine";
