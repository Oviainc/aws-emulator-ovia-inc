// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-opensearchserverless-opensearchserverless service.
//!
//! This holds the resources this slice serves — the OpenSearch Serverless
//! **collection**, the **collection group**, the **security policy** and the **data access
//! policy** — plus everything more than one handler of those resources needs: the
//! server-derived fields (the minted ids, the ARNs, the endpoints derived from a collection
//! id, the epoch-millisecond timestamps), the clock-owed `CREATING -> ACTIVE` transition a
//! just-created collection settles through, the opaque page token `ListCollections` hands
//! out, and the `clientToken` replay index the `@idempotent` creates read.
//!
//! `src/ops/mod.rs` is generated, so a helper more than one operation reaches lives here
//! rather than in a second module under `src/ops/`.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessPolicyDetail, CapacityDetails, CollectionDetail, CollectionErrorDetail,
    CollectionGroupCapacityLimits, CollectionGroupDetail, CollectionGroupErrorDetail,
    CollectionSummary, CreateCollectionDetail, CreateCollectionGroupDetail, CurrentCapacity,
    DeleteCollectionDetail, FipsEndpoints, SecurityPolicyDetail, Tag, VectorOptions,
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

/// The account's data-access-policy quota in one Region, from the same "Quotas for Amazon
/// OpenSearch Serverless" page: 100 data access policies per account (adjustable on
/// request, so this is the DEFAULT quota). `AccessPolicyType` has the single value `data`,
/// so this bounds the whole map.
pub const MAX_ACCESS_POLICIES: usize = 100;

/// The most collection groups this emulator stores for one account in one Region.
///
/// Emulator resource guard, NOT a quota AWS publishes: the OpenSearch Serverless quota
/// page lists no collection-group limit, so this bound exists only to keep the store
/// finite. It is answered with the modelled `ServiceQuotaExceededException` because that
/// is the error `CreateCollectionGroup` declares for "more resources than the service
/// allows", and it is set at the collection quota above so a group per collection still
/// fits.
pub const MAX_COLLECTION_GROUPS: usize = MAX_COLLECTIONS;

/// The page bound `ListCollections` applies when the caller names none, from
/// `ListCollectionsRequest$maxResults`' own documentation: "The maximum number of results
/// to return. Default is 20."
pub const DEFAULT_PAGE_SIZE: i32 = 20;

/// The largest page `ListCollections` serves: the maximum of `maxResults`' own
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
    /// The limits the create (or a later update) stored; `None` while the group names no
    /// limit of its own.
    pub capacity_limits: Option<CollectionGroupCapacityLimits>,
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
            capacity_limits: self.capacity_limits.clone(),
            generation: Some(self.generation.clone()),
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
            capacity_limits: self.capacity_limits.clone(),
            current_capacity: self.current_capacity(),
            number_of_collections: Some(number_of_collections),
            generation: Some(self.generation.clone()),
        }
    }

    /// The group's current search and indexing capacity.
    ///
    /// Nothing in this emulator runs OCUs, so the only capacity a group demonstrably
    /// holds is the FLOOR its own stored limits name — `minSearchCapacityInOCU` and
    /// `minIndexingCapacityInOCU` — and it holds exactly that while no autoscaling action
    /// is in progress ([`AUTOSCALING_NO_ACTION`]). A group whose limits name no floor, or
    /// which carries no limits at all, reports no current capacity rather than a number
    /// nothing measured.
    fn current_capacity(&self) -> Option<CurrentCapacity> {
        let limits = self.capacity_limits.as_ref()?;
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
    /// Data access policies by `(AccessPolicyType, PolicyName)`, the pair that identifies
    /// one, exactly as `security_policies` is keyed.
    pub access_policies: BTreeMap<(String, String), AccessPolicyRecord>,
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

    /// The record a replayed `clientToken` on `operation` already created, if any.
    ///
    /// `None` both when the token is new and when the record it created has since been
    /// deleted — a replay cannot resurrect a deleted resource, so a stale token simply
    /// falls through to the operation's normal path.
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
    pub fn remember_token(&mut self, operation: &str, token: &str, key: &str) {
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
        let entry = idempotency_key(operation, token);
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

    /// The number of stored data access policies, which [`MAX_ACCESS_POLICIES`] bounds.
    pub fn access_policy_count(&self) -> usize {
        self.access_policies.len()
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

/// One page of a listing: the records it carries, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a> {
    /// The records of this page, in collection-id order.
    pub items: Vec<&'a CollectionRecord>,
    /// Set only when at least one more record remains after this page.
    pub next_token: Option<String>,
}

/// A page token is the collection id the NEXT page resumes after, hex-encoded.
///
/// Hex makes the token opaque in the sense the API promises — it is not an id a client
/// should read or construct — but it is NOT authenticated: hex is reversible, and a
/// hand-made token that decodes to UTF-8 is accepted and simply resumes after that id.
/// That is the same answer a token whose collection has since been deleted gets, which is
/// what lets a listing resume correctly across creates and deletes rather than failing on
/// them. Nothing a token can say reaches past this listing's own id order, so a forged one
/// grants no access: it can only start the same listing at a different place.
fn encode_page_token(id: &str) -> String {
    hex::encode(id)
}

/// The id a token resumes after, or the reason the token cannot be one this service
/// issued (its bytes are not hex, or not UTF-8).
fn decode_page_token(token: &str) -> Result<String, String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| "nextToken is not a pagination token this service issued".to_string())
}

/// The page of `ordered` that `next_token` and `max_results` select.
///
/// `ordered` is the already-filtered record sequence in collection-id order — the stable
/// sequence a token resumes inside. `max_results` is a page BOUND, not a count: a page
/// holds at most that many records, and `next_token` is returned only while more remain.
pub fn page<'a>(
    ordered: Vec<&'a CollectionRecord>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a>, String> {
    let bound = max_results.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let bound = usize::try_from(bound).unwrap_or(1);
    let after = match next_token {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };

    let mut remaining = ordered
        .into_iter()
        .filter(|record| after.as_deref().is_none_or(|from| record.id.as_str() > from));
    let items: Vec<&'a CollectionRecord> = remaining.by_ref().take(bound).collect();
    // A token only when a record is left over: the last page of a listing carries none.
    let next_token = remaining
        .next()
        .and_then(|_| items.last().map(|record| encode_page_token(&record.id)));

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
    /// A replayed `clientToken` answered the record its first call created; nothing was
    /// written.
    IdempotentReplay,
    /// A stored policy was reported to a reader.
    PolicyRead,
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
            Self::IdempotentReplay => "idempotent-replay",
            Self::PolicyRead => "policy-read",
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
