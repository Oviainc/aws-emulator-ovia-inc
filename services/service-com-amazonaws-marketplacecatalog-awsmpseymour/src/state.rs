// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacecatalog-awsmpseymour service.
//!
//! The change set is the primary resource of the AWS Marketplace Catalog:
//! `StartChangeSet` mints one, `DescribeChangeSet` / `ListChangeSets` report it
//! and `CancelChangeSet` stops one that is still running. A change set that
//! succeeds with `Intent=APPLY` is what makes a catalog ENTITY exist, and
//! `DescribeEntity` / `ListEntities` report those entities. The two records
//! below are the one copy of each that every handler of this crate reads and
//! writes, plus the projections the model asks for (`ChangeSummary`,
//! `ChangeSetSummaryListItem`, `EntitySummary`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::Document;
use serde_json::{Map, Value};

use crate::ops::types::{
    AccessDeniedException, AmiProductSummary, ChangeSetSummaryListItem, ChangeSummary,
    ContainerProductSummary, DataProductSummary, Entity, EntitySummary,
    MachineLearningProductSummary, ResourceInUseException, ResourceNotFoundException,
    SaaSProductSummary, Tag, ValidationException, ValidationExceptionField,
};

/// The only catalog this service serves.
///
/// Every operation's `Catalog` member documents "Fixed value: AWSMarketplace",
/// so a request naming any other catalog is refused rather than opening a
/// second store.
pub const AWS_MARKETPLACE_CATALOG: &str = "AWSMarketplace";

/// The region an ARN names when the request carries no SigV4 credential scope
/// to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` of this service, and so the service segment of every ARN
/// it mints (`OPERATIONS.md`'s wire-protocol section).
pub const ARN_NAMESPACE: &str = "aws-marketplace";

/// How long a change set stays `PREPARING` before the clock owes it `APPLYING`.
///
/// A change set is asynchronous: `StartChangeSet` returns before the changes
/// have run. AWS takes minutes; the emulator chooses a span, and it is
/// deliberately NOT zero, so a caller acting straight after the start still
/// observes the running change set — and can still cancel it. The due instant
/// is stored on the record and materialised by whichever later call reads it;
/// nothing sleeps.
pub const CHANGE_SET_PREPARING: Duration = Duration::from_millis(250);

/// How long a change set then stays `APPLYING` before the clock owes it
/// `SUCCEEDED` — and, for `Intent=APPLY`, owes the catalog the entities its
/// changes describe. Chosen for the same reason as [`CHANGE_SET_PREPARING`].
pub const CHANGE_SET_APPLYING: Duration = Duration::from_millis(250);

/// How many change sets the store holds before `StartChangeSet` refuses with
/// the modelled `ServiceQuotaExceededException` ("The maximum number of open
/// requests per account has been exceeded").
///
/// AWS publishes an adjustable quota on change sets per account, which this
/// emulator models neither Service Quotas nor an account's adjustments for — so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is far above any workflow an emulator serves, and it is
/// what stops the state growing without end.
pub const CHANGE_SET_QUOTA: usize = 1_000;

/// The largest page `ListChangeSets` serves, and the page it serves when the
/// request names no `MaxResults`: `ListChangeSetsRequest$MaxResults` is
/// `@range(min: 1, max: 20)`.
pub const LIST_CHANGE_SETS_PAGE_LIMIT: i32 = 20;

/// The same bound for `ListEntities`: `ListEntitiesRequest$MaxResults` is
/// `@range(min: 1, max: 50)`.
pub const LIST_ENTITIES_PAGE_LIMIT: i32 = 50;

/// The `Filter$Name` values `ListChangeSets` accepts, from `Filter$ValueList`'s
/// own documentation ("The supported filter names and associated `ValueList`s is
/// as follows: ChangeSetName, Status, EntityId, BeforeStartTime, AfterStartTime,
/// BeforeEndTime, AfterEndTime").
pub const CHANGE_SET_FILTER_NAMES: &[&str] = &[
    "AfterEndTime",
    "AfterStartTime",
    "BeforeEndTime",
    "BeforeStartTime",
    "ChangeSetName",
    "EntityId",
    "Status",
];

/// The `Filter$Name` values `ListEntities` accepts: `Filter$Name` documents
/// "For `ListEntities`, the supported value for this is an `EntityId`".
pub const ENTITY_FILTER_NAMES: &[&str] = &["EntityId"];

/// The `ListChangeSets` filters whose `ValueList` is a single instant rather
/// than a set of values to match: each one is documented as "all change sets
/// that started/ended before/after THE filter value".
pub const CHANGE_SET_TIME_FILTER_NAMES: &[&str] = &[
    "AfterEndTime",
    "AfterStartTime",
    "BeforeEndTime",
    "BeforeStartTime",
];

/// The `Sort$SortBy` values `ListChangeSets` accepts, from `Sort$SortBy`'s doc
/// ("For `ListChangeSets`, supported attributes include `StartTime` and
/// `EndTime`"). The first entry is the default.
pub const CHANGE_SET_SORT_BY: &[&str] = &["StartTime", "EndTime"];

/// The `Sort$SortBy` values `ListEntities` accepts, from the same doc ("For
/// `ListEntities`, supported attributes include `LastModifiedDate` (default) and
/// `EntityId`"). The first entry is the default.
pub const ENTITY_SORT_BY: &[&str] = &["LastModifiedDate", "EntityId"];

/// The `SortOrder` a list uses when the request names none: `Sort$SortOrder`
/// documents "The default value is `DESCENDING`".
pub const DEFAULT_SORT_ORDER: &str = "DESCENDING";

/// The details members an entity's `Name` is read from.
///
/// `EntitySummary$Name` is "defined by the seller", and the per-type summaries
/// the model carries name it two ways: the five product summaries
/// (`AmiProductSummary`, `ContainerProductSummary`, `DataProductSummary`,
/// `SaaSProductSummary`, `MachineLearningProductSummary`) call it
/// `ProductTitle`, while `OfferSummary` / `OfferSetSummary` /
/// `ResaleAuthorizationSummary` call it `Name`.
pub const ENTITY_NAME_DETAIL_MEMBERS: &[&str] = &["ProductTitle", "Name"];

/// The details member an entity's `Visibility` is read from — the name every
/// per-type summary and `*VisibilityFilter` in the model gives it.
pub const ENTITY_VISIBILITY_DETAIL_MEMBER: &str = "Visibility";

/// The `Visibility` values an entity may carry: the union of the model's five
/// `*VisibilityString` enums (`AmiProductVisibilityString`,
/// `ContainerProductVisibilityString`, `DataProductVisibilityString`,
/// `MachineLearningProductVisibilityString`, `SaaSProductVisibilityString`).
pub const ENTITY_VISIBILITY_VALUES: &[&str] =
    &["Draft", "Limited", "Public", "Restricted", "Unavailable"];

/// The `Visibility` an entity carries when the change that created it named
/// none.
///
/// `EntitySummary$Visibility` documents `Limited` as "the entity is visible to
/// limited accounts only" — which is what an entity a change set has just
/// created is, since publishing it to buyers takes a further change.
pub const DEFAULT_ENTITY_VISIBILITY: &str = "Limited";

/// The entity types whose `EntitySummary` projection this emulator fills, and
/// the per-type summary member each one fills.
///
/// Taken from `EntitySummary`'s own members: the five `*ProductSummary` shapes
/// carry exactly `{ProductTitle, Visibility}`, both of which an entity record
/// holds. The remaining types (`Offer`, `OfferSet`, `ResaleAuthorization`,
/// `ExpressPrivateOfferConfiguration`) have summaries whose members — buyer
/// accounts, release dates, targeting — this slice's change-set application
/// does not model, so no per-type summary is reported for them.
pub const PRODUCT_ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "MachineLearningProduct",
    "SaaSProduct",
];

/// The resource type segment a CHANGE SET's ARN carries, as
/// [`change_set_arn`] writes it. An entity's ARN carries its `EntityType`
/// there instead, which is how one resource ARN form names both.
pub const CHANGE_SET_RESOURCE_TYPE: &str = "ChangeSet";

/// The resource-based policy a resource that has none carries.
///
/// `GetResourcePolicy` reports "a resource-based policy of an entity", and
/// `ResourcePolicyJson` is `@length 1..10240`, so the absence of a policy is
/// reported as the EMPTY JSON policy document rather than as an absent member
/// or an empty string, neither of which that bound admits.
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// How many tags one resource holds.
///
/// `TagList` is `@length(min: 1, max: 200)`, which is the bound on the tags a
/// resource carries as much as on the tags one request names, so a
/// `TagResource` whose merge would pass it is refused rather than storing a set
/// no `ListTagsForResource` could report.
pub const MAX_RESOURCE_TAGS: usize = 200;

/// The `NextToken` prefix `ListChangeSets` pages with.
pub const CHANGE_SET_PAGE_PREFIX: &str = "CS";

/// The `NextToken` prefix `ListEntities` pages with.
pub const ENTITY_PAGE_PREFIX: &str = "ENT";

/// A `ChangeStatus` value.
pub const STATUS_PREPARING: &str = "PREPARING";
/// A `ChangeStatus` value.
pub const STATUS_APPLYING: &str = "APPLYING";
/// A `ChangeStatus` value.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// A `ChangeStatus` value.
pub const STATUS_CANCELLED: &str = "CANCELLED";

/// An `Intent` value — `StartChangeSetRequest$Intent` documents "The default is
/// APPLY".
pub const INTENT_APPLY: &str = "APPLY";
/// An `Intent` value: "To test your request before applying changes to your
/// entities, use VALIDATE".
pub const INTENT_VALIDATE: &str = "VALIDATE";

/// A status change the clock owes a change set: the instant it falls due, and
/// the status the change set carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// One `Change` of a change set, as submitted and then as resolved.
#[derive(Debug, Clone)]
pub struct ChangeRecord {
    pub change_type: String,
    pub entity_type: String,
    /// `Entity$Identifier` exactly as submitted, which is absent for a change
    /// that creates an entity.
    pub entity_identifier: Option<String>,
    /// `Change$Details` as submitted.
    pub details: Option<String>,
    /// `Change$DetailsDocument` as submitted.
    pub details_document: Option<Value>,
    pub change_name: Option<String>,
    pub entity_tags: Vec<Tag>,
    /// The entity this change acts on: the id `Entity$Identifier` resolved to,
    /// or the id minted for the entity the change creates. Absent for a change
    /// of a `VALIDATE` change set that names no existing entity, since a
    /// validation never creates one.
    pub entity_id: Option<String>,
}

impl ChangeRecord {
    /// The details this change carries, as one JSON value — whichever of
    /// `Details` and `DetailsDocument` it was submitted with.
    ///
    /// `Change$Details` is a JSON string and `Change$DetailsDocument` the same
    /// value as a document ("You can use either `Details` or
    /// `DetailsDocument`, but not both"), so both land here as one value.
    pub fn details_value(&self) -> Option<Value> {
        if let Some(text) = self.details.as_deref() {
            return serde_json::from_str(text).ok();
        }
        self.details_document.clone()
    }

    /// The `ChangeSummary` projection `DescribeChangeSet` reports.
    pub fn to_summary(&self) -> ChangeSummary {
        ChangeSummary {
            change_type: Some(self.change_type.clone()),
            entity: Some(Entity {
                r#type: self.entity_type.clone(),
                identifier: self.entity_identifier.clone(),
            }),
            details: self.details.clone(),
            details_document: self
                .details_document
                .as_ref()
                // `None` is unreachable: a document deeper than
                // `MAX_DETAILS_DEPTH` is refused before it is stored.
                .and_then(json_to_document),
            // No change this emulator applies fails, so no change carries an
            // error detail; `FailureCode`/`FailureDescription` are reported for
            // the same reason only on a `FAILED` change set.
            error_detail_list: None,
            change_name: self.change_name.clone(),
        }
    }
}

/// One change set, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ChangeSetRecord {
    /// `ResourceId`'s `^[\w\-]+$`, minted by
    /// [`ComAmazonawsMarketplacecatalogAwsmpseymourState::mint_change_set_id`].
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// One of `Intent`'s values.
    pub intent: String,
    /// One of `ChangeStatus`'s values.
    pub status: String,
    /// `DateTimeISO8601`: `....-..-..T..:..:..Z`.
    pub start_time: String,
    pub end_time: Option<String>,
    pub failure_code: Option<String>,
    pub failure_description: Option<String>,
    pub changes: Vec<ChangeRecord>,
    pub tags: Vec<Tag>,
    /// The region this change set was started in — what the ARNs of the
    /// entities it applies are built for.
    pub region: String,
    /// The status changes the clock still owes this change set, oldest first.
    pub schedule: Vec<PendingTransition>,
}

impl ChangeSetRecord {
    /// True while the change set is still running, so its entities are locked
    /// and it can still be cancelled.
    pub fn is_running(&self) -> bool {
        self.status == STATUS_PREPARING || self.status == STATUS_APPLYING
    }

    /// The entities this change set acts on, in submission order and without
    /// repeats — `ChangeSetSummaryListItem$EntityIdList`.
    pub fn entity_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::with_capacity(self.changes.len());
        for id in self.changes.iter().filter_map(|c| c.entity_id.as_ref()) {
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.clone());
            }
        }
        ids
    }

    /// The `ChangeSetSummaryListItem` projection `ListChangeSets` reports.
    pub fn to_summary(&self) -> ChangeSetSummaryListItem {
        ChangeSetSummaryListItem {
            change_set_id: Some(self.id.clone()),
            change_set_arn: Some(self.arn.clone()),
            change_set_name: self.name.clone(),
            start_time: Some(self.start_time.clone()),
            end_time: self.end_time.clone(),
            status: Some(self.status.clone()),
            entity_id_list: Some(self.entity_ids()),
            failure_code: self.failure_code.clone(),
        }
    }

    /// Whether this change set satisfies the `ListChangeSets` filter `name`
    /// with `values`, as `Filter$ValueList`'s documentation defines each one.
    ///
    /// A name outside [`CHANGE_SET_FILTER_NAMES`] never reaches here: the
    /// handler refuses the request first.
    pub fn matches_filter(&self, name: &str, values: &[String]) -> bool {
        let bound = values.first().map(String::as_str).unwrap_or_default();
        match name {
            "ChangeSetName" => self
                .name
                .as_deref()
                .is_some_and(|held| values.iter().any(|value| value == held)),
            "Status" => values.iter().any(|value| value == &self.status),
            "EntityId" => self
                .entity_ids()
                .iter()
                .any(|id| values.iter().any(|value| value == id)),
            // The four time filters compare the `DateTimeISO8601` strings the
            // record holds, which are fixed-width and UTC, so their byte order
            // is their chronological order.
            "BeforeStartTime" => self.start_time.as_str() < bound,
            "AfterStartTime" => self.start_time.as_str() > bound,
            "BeforeEndTime" => self
                .end_time
                .as_deref()
                .is_some_and(|held| held < bound),
            "AfterEndTime" => self.end_time.as_deref().is_some_and(|held| held > bound),
            _ => false,
        }
    }

    /// The value this change set sorts on under `Sort$SortBy` value `sort_by`.
    ///
    /// A change set that is still running has no `EndTime`, and the empty
    /// string it sorts as puts it first ascending and last descending.
    pub fn sort_key(&self, sort_by: &str) -> &str {
        match sort_by {
            "EndTime" => self.end_time.as_deref().unwrap_or_default(),
            _ => self.start_time.as_str(),
        }
    }
}

/// One catalog entity, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct EntityRecord {
    /// `ResourceId`'s `^[\w\-]+$`.
    pub id: String,
    /// `EntityType`'s `^[a-zA-Z]+$`.
    pub entity_type: String,
    pub arn: String,
    /// How many change sets have applied a change to this entity. It is what
    /// makes `EntityIdentifier` (`<id>@<revision>`) move when the entity does.
    pub revision: u32,
    pub name: Option<String>,
    /// One of [`ENTITY_VISIBILITY_VALUES`].
    pub visibility: String,
    /// `DateTimeISO8601`: `....-..-..T..:..:..Z`.
    pub last_modified_date: String,
    /// The details the applied changes have accumulated, as one JSON object.
    pub details: Value,
    pub tags: Vec<Tag>,
}

impl EntityRecord {
    /// `EntityIdentifier`: the entity id and the revision the reader is seeing,
    /// which is the form `Entity$Identifier` accepts back (`^[\w\-@]+$`).
    pub fn identifier(&self) -> String {
        format!("{}@{}", self.id, self.revision)
    }

    /// The accumulated details as the `Json` string `Details` is modelled as.
    pub fn details_json(&self) -> String {
        self.details.to_string()
    }

    /// `{ProductTitle, Visibility}` for the one per-type summary member
    /// `entity_type` names, and `None` for every other entity's.
    ///
    /// The [`PRODUCT_ENTITY_TYPES`] test is what keeps the member and the type
    /// it belongs to in step: a type absent from that catalog reports no
    /// per-type summary at all.
    fn product_summary(&self, entity_type: &str) -> Option<(Option<String>, Option<String>)> {
        (self.entity_type == entity_type && PRODUCT_ENTITY_TYPES.contains(&entity_type))
            .then(|| (self.name.clone(), Some(self.visibility.clone())))
    }

    /// The `EntitySummary` projection `ListEntities` reports.
    pub fn to_summary(&self) -> EntitySummary {
        EntitySummary {
            name: self.name.clone(),
            entity_type: Some(self.entity_type.clone()),
            entity_id: Some(self.id.clone()),
            entity_arn: Some(self.arn.clone()),
            last_modified_date: Some(self.last_modified_date.clone()),
            visibility: Some(self.visibility.clone()),
            ami_product_summary: self.product_summary("AmiProduct").map(
                |(product_title, visibility)| AmiProductSummary {
                    product_title,
                    visibility,
                },
            ),
            container_product_summary: self.product_summary("ContainerProduct").map(
                |(product_title, visibility)| ContainerProductSummary {
                    product_title,
                    visibility,
                },
            ),
            data_product_summary: self.product_summary("DataProduct").map(
                |(product_title, visibility)| DataProductSummary {
                    product_title,
                    visibility,
                },
            ),
            saa_s_product_summary: self.product_summary("SaaSProduct").map(
                |(product_title, visibility)| SaaSProductSummary {
                    product_title,
                    visibility,
                },
            ),
            machine_learning_product_summary: self.product_summary("MachineLearningProduct").map(
                |(product_title, visibility)| MachineLearningProductSummary {
                    product_title,
                    visibility,
                },
            ),
            // The summaries of the remaining entity types carry members this
            // slice does not model; see `PRODUCT_ENTITY_TYPES`.
            offer_summary: None,
            resale_authorization_summary: None,
            offer_set_summary: None,
            express_private_offer_configuration_summary: None,
        }
    }

    /// Whether this entity satisfies the `ListEntities` filter `name` with
    /// `values`: `Filter$Name` documents `EntityId` as the one it supports, and
    /// `Filter$ValueList` as "a list of unique `EntityId`s".
    pub fn matches_filter(&self, name: &str, values: &[String]) -> bool {
        match name {
            "EntityId" => values.iter().any(|value| value == &self.id),
            _ => false,
        }
    }

    /// The value this entity sorts on under `Sort$SortBy` value `sort_by`.
    pub fn sort_key(&self, sort_by: &str) -> &str {
        match sort_by {
            "EntityId" => self.id.as_str(),
            _ => self.last_modified_date.as_str(),
        }
    }
}

/// One page of a list operation's answer, and the token the next page resumes
/// at.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The page of `all` that starts at `offset` and holds at most `page_size`
/// items, with a `NextToken` only when items are left over — `@paginated`'s
/// contract, and the reason the last page carries none.
pub fn paginate<T>(all: Vec<T>, offset: usize, page_size: usize, prefix: &str) -> Page<T> {
    let total = all.len();
    let items: Vec<T> = all.into_iter().skip(offset).take(page_size).collect();
    let end = offset.saturating_add(items.len());
    Page {
        next_token: (end < total).then(|| encode_page_token(prefix, end)),
        items,
    }
}

/// Per-service state for com-amazonaws-marketplacecatalog-awsmpseymour. A handler reaches it through [`service_state`].
///
/// There is one store rather than one per catalog because every operation's
/// `Catalog` member documents a fixed value ([`AWS_MARKETPLACE_CATALOG`]), which
/// the handlers refuse anything else for.
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplacecatalogAwsmpseymourState {
    /// Every change set, keyed by its id. A `BTreeMap` is what gives the lists
    /// a stable total order to sort and page within — ids sort in the order
    /// they were minted.
    pub change_sets: BTreeMap<String, ChangeSetRecord>,
    /// Every entity a succeeded `APPLY` change set has created, keyed by id.
    pub entities: BTreeMap<String, EntityRecord>,
    /// A `StartChangeSet` `ClientRequestToken` to the change set that token
    /// started, so a retried start reports the first change set instead of
    /// minting a second.
    pub idempotency_tokens: HashMap<String, String>,
    /// The counter change set ids are minted from.
    pub change_set_seq: u64,
    /// The counter entity ids are minted from.
    pub entity_seq: u64,
    /// The resource-based policy attached to each resource that has one, keyed
    /// by [`CatalogResource::policy_key`] — the resource's type and id, which
    /// is what identifies it whatever region or partition the ARN naming it was
    /// written with. A resource absent from the map has no policy attached.
    pub resource_policies: BTreeMap<String, String>,
}

impl ComAmazonawsMarketplacecatalogAwsmpseymourState {
    /// Mint an unused change set id matching `ResourceId`'s `^[\w\-]+$`.
    pub fn mint_change_set_id(&mut self) -> String {
        loop {
            self.change_set_seq = self.change_set_seq.wrapping_add(1);
            let id = format!("cs-{:012x}", self.change_set_seq & 0xffff_ffff_ffff);
            if !self.change_sets.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused entity id matching `ResourceId`'s `^[\w\-]+$`.
    ///
    /// The loop also skips an id a running change set has already reserved, so
    /// two change sets in flight never name the same new entity.
    pub fn mint_entity_id(&mut self) -> String {
        loop {
            self.entity_seq = self.entity_seq.wrapping_add(1);
            let id = format!("ent-{:012x}", self.entity_seq & 0xffff_ffff_ffff);
            if !self.entities.contains_key(&id) && self.locking_change_set(&id).is_none() {
                return id;
            }
        }
    }

    /// The id of the still-running change set that has `entity_id` locked, if
    /// any.
    ///
    /// `StartChangeSet` documents the lock: "when a ChangeSet is running, all
    /// the entities targeted by the different changes are locked until the
    /// change set has completed (either succeeded, cancelled, or failed)".
    pub fn locking_change_set(&self, entity_id: &str) -> Option<&str> {
        self.change_sets
            .values()
            .find(|record| {
                record.is_running()
                    && record
                        .changes
                        .iter()
                        .any(|change| change.entity_id.as_deref() == Some(entity_id))
            })
            .map(|record| record.id.as_str())
    }

    /// The entity `identifier` names, where an identifier is an entity id with
    /// an optional `@<revision>` suffix (`Identifier`'s `^[\w\-@]+$`).
    pub fn resolve_identifier(&self, identifier: &str) -> Option<&EntityRecord> {
        let id = identifier.split('@').next().unwrap_or(identifier);
        self.entities.get(id)
    }

    /// Whether this store holds the resource a resource ARN named.
    ///
    /// An entity's ARN names its `EntityType` as well as its id, so a record
    /// whose type is not the one the ARN carries is NOT the resource that ARN
    /// names.
    pub fn holds_resource(&self, resource: &CatalogResource) -> bool {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .is_some_and(|held| &held.entity_type == entity_type),
            CatalogResource::ChangeSet { id } => self.change_sets.contains_key(id),
        }
    }

    /// The ARN the stored record carries for the resource a resource ARN named,
    /// which is what an operation reporting the resource reports — never the
    /// ARN the request was written with.
    pub fn stored_resource_arn(&self, resource: &CatalogResource) -> Option<&str> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| held.arn.as_str()),
            CatalogResource::ChangeSet { id } => {
                self.change_sets.get(id).map(|held| held.arn.as_str())
            }
        }
    }

    /// The tags the stored record carries for the resource a resource ARN
    /// named: the one copy `StartChangeSet` wrote and `TagResource` updates.
    pub fn resource_tags(&self, resource: &CatalogResource) -> Option<&[Tag]> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| held.tags.as_slice()),
            CatalogResource::ChangeSet { id } => {
                self.change_sets.get(id).map(|held| held.tags.as_slice())
            }
        }
    }

    /// Those same tags, for writing.
    pub fn resource_tags_mut(&mut self, resource: &CatalogResource) -> Option<&mut Vec<Tag>> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get_mut(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| &mut held.tags),
            CatalogResource::ChangeSet { id } => {
                self.change_sets.get_mut(id).map(|held| &mut held.tags)
            }
        }
    }

    /// Materialise every change the clock owes: a change set past its due
    /// instant moves on, and one that reaches `SUCCEEDED` with `Intent=APPLY`
    /// writes the entities its changes describe.
    ///
    /// Each change is stamped with the instant it fell DUE, never with `now`,
    /// so what a caller reads does not depend on when it read.
    pub fn settle(&mut self, now: SystemTime) {
        let ids: Vec<String> = self.change_sets.keys().cloned().collect();
        for id in ids {
            while let Some(record) = self.change_sets.get_mut(&id) {
                let due = match record.schedule.first() {
                    Some(next) if now >= next.due => next.due,
                    _ => break,
                };
                let transition = record.schedule.remove(0);
                record.status = transition.status.clone();
                if transition.status != STATUS_SUCCEEDED {
                    continue;
                }
                record.end_time = Some(iso8601(due));
                let apply = record.intent == INTENT_APPLY;
                if apply {
                    self.apply_changes(&id, due);
                }
            }
        }
    }

    /// Write the entities the changes of change set `id` describe, as of `at`.
    ///
    /// A change naming an existing entity mutates that record and bumps its
    /// revision; a change naming none creates the record under the id
    /// `StartChangeSet` reserved for it.
    fn apply_changes(&mut self, id: &str, at: SystemTime) {
        let Some(record) = self.change_sets.get(id) else {
            return;
        };
        let changes = record.changes.clone();
        let region = record.region.clone();
        let stamp = iso8601(at);
        for change in changes {
            let Some(entity_id) = change.entity_id.clone() else {
                continue;
            };
            let details = change.details_value();
            let entry = self.entities.entry(entity_id.clone()).or_insert_with(|| {
                EntityRecord {
                    arn: entity_arn(&region, &change.entity_type, &entity_id),
                    id: entity_id.clone(),
                    entity_type: change.entity_type.clone(),
                    revision: 0,
                    name: None,
                    visibility: DEFAULT_ENTITY_VISIBILITY.to_string(),
                    last_modified_date: stamp.clone(),
                    details: Value::Object(Map::new()),
                    tags: Vec::new(),
                }
            });
            entry.revision = entry.revision.saturating_add(1);
            entry.last_modified_date = stamp.clone();
            for tag in change.entity_tags {
                entry.tags.retain(|held| held.key != tag.key);
                entry.tags.push(tag);
            }
            if let Some(Value::Object(members)) = details {
                if let Some(name) = detail_name(&members) {
                    entry.name = Some(name);
                }
                if let Some(visibility) = detail_visibility(&members) {
                    entry.visibility = visibility;
                }
                let Value::Object(held) = &mut entry.details else {
                    continue;
                };
                for (key, value) in members {
                    held.insert(key, value);
                }
            }
        }
    }
}

/// The `Name` the details of an applied change give an entity, if any.
fn detail_name(members: &Map<String, Value>) -> Option<String> {
    ENTITY_NAME_DETAIL_MEMBERS
        .iter()
        .filter_map(|member| members.get(*member))
        .find_map(|value| value.as_str().map(str::to_string))
}

/// The `Visibility` the details of an applied change give an entity, if any.
///
/// A value outside [`ENTITY_VISIBILITY_VALUES`] is left to
/// [`invalid_visibility`], which refuses the request before it is applied, so
/// nothing outside the modelled set reaches a record.
fn detail_visibility(members: &Map<String, Value>) -> Option<String> {
    let value = members.get(ENTITY_VISIBILITY_DETAIL_MEMBER)?.as_str()?;
    ENTITY_VISIBILITY_VALUES
        .contains(&value)
        .then(|| value.to_string())
}

/// The `Visibility` in these details if the details name one the model has no
/// value for — what a handler refuses a change for before storing it.
pub fn invalid_visibility(details: &Value) -> Option<String> {
    let value = details
        .as_object()?
        .get(ENTITY_VISIBILITY_DETAIL_MEMBER)?
        .as_str()?;
    (!ENTITY_VISIBILITY_VALUES.contains(&value)).then(|| value.to_string())
}

/// True when `value` is a `DateTimeISO8601`: the 20-character
/// `....-..-..T..:..:..Z` form that shape's pattern pins, which is what makes a
/// byte comparison of two of them a comparison of two instants.
///
/// The boundary validator refuses a modelled `DateTimeISO8601` member, so this
/// is for a value the model does not constrain — a `Filter$ValueList` entry a
/// before/after filter compares against.
pub fn is_iso8601(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 {
        return false;
    }
    let digits = |range: ::std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    let number = |range: ::std::ops::Range<usize>| value[range].parse::<u32>().unwrap_or(u32::MAX);
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z'
        && digits(0..4)
        && digits(5..7)
        && digits(8..10)
        && digits(11..13)
        && digits(14..16)
        && digits(17..19)
        && (1..=12).contains(&number(5..7))
        && (1..=31).contains(&number(8..10))
        && number(11..13) <= 23
        && number(14..16) <= 59
        && number(17..19) <= 59
}

/// `at` in the `DateTimeISO8601` form every timestamp of this service is
/// modelled with: exactly 20 characters, `....-..-..T..:..:..Z`.
pub fn iso8601(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let text = ::aws_smithy_types::DateTime::from_secs(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `Format::DateTime` renders a fractional part only for a non-zero
    // subsecond and `from_secs` carries none, so this keeps the 20-character
    // form the pattern requires even if that ever changes.
    match text.split_once('.') {
        Some((whole, _)) => format!("{whole}Z"),
        None => text,
    }
}

/// The ARN of a change set.
pub fn change_set_arn(region: &str, change_set_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/ChangeSet/{change_set_id}"),
    )
}

/// The ARN of a catalog entity.
pub fn entity_arn(region: &str, entity_type: &str, entity_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/{entity_type}/{entity_id}"),
    )
}

/// A resource of this catalog, as one of its resource ARNs names it.
///
/// `ListTagsForResource` is documented for "a resource (either an entity or
/// change set)", and [`entity_arn`] / [`change_set_arn`] write both in the one
/// form `<catalog>/<type>/<id>`, so one parse covers both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogResource {
    /// A catalog entity: its `EntityType` and its `ResourceId`.
    Entity { entity_type: String, id: String },
    /// A change set, by its `ResourceId`.
    ChangeSet { id: String },
}

impl CatalogResource {
    /// The id the ARN named — what a `ResourceNotFoundException` for it names.
    pub fn id(&self) -> &str {
        match self {
            Self::Entity { id, .. } | Self::ChangeSet { id } => id.as_str(),
        }
    }

    /// The key [`ComAmazonawsMarketplacecatalogAwsmpseymourState::resource_policies`]
    /// holds this resource's policy under: its type and id, so a policy
    /// attached through an ARN written for one region is the policy read back
    /// through an ARN written for another — the resource is the same resource.
    pub fn policy_key(&self) -> String {
        match self {
            Self::Entity { entity_type, id } => format!("{entity_type}/{id}"),
            Self::ChangeSet { id } => format!("{CHANGE_SET_RESOURCE_TYPE}/{id}"),
        }
    }
}

/// The resource portion of `arn` — everything after the account field — or the
/// whole value when it has no such field.
pub fn arn_resource(arn: &str) -> &str {
    let mut fields = arn.splitn(6, ':');
    match (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) {
        (Some("arn"), Some(_), Some(_), Some(_), Some(_), Some(resource)) => resource,
        _ => arn,
    }
}

/// The resource of THIS service `arn` names, or `None` when `arn` names none.
///
/// A resource ARN this service can act on is one it mints: this account, this
/// `arnNamespace`, and a resource portion of the `<catalog>/<type>/<id>` form
/// [`entity_arn`] and [`change_set_arn`] write. Every other well-formed ARN
/// names a resource outside this catalog, which no identity or resource policy
/// of this catalog can grant an action on — so the caller is not authorized to
/// act on it rather than it being absent.
///
/// The partition and region fields are not constrained: they name where the
/// resource lives, not what it is, and the modelled `ResourceARN` pattern
/// already bounds their characters.
pub fn parse_resource_arn(arn: &str) -> Option<CatalogResource> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(_partition),
        Some(ARN_NAMESPACE),
        Some(_region),
        Some(account),
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
    if account != ::engine::emulator_account_id() {
        return None;
    }
    let mut segments = resource.split('/');
    let (Some(AWS_MARKETPLACE_CATALOG), Some(resource_type), Some(id), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if id.is_empty() {
        return None;
    }
    Some(if resource_type == CHANGE_SET_RESOURCE_TYPE {
        CatalogResource::ChangeSet { id: id.to_string() }
    } else {
        CatalogResource::Entity {
            entity_type: resource_type.to_string(),
            id: id.to_string(),
        }
    })
}

/// The region this request was signed for — what the ARNs it mints name.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The page `offset` as a `NextToken`: `^[\w+=.:@\-\/]+$`, with a prefix naming
/// the list it pages, so a token one list returned is not accepted by another.
pub fn encode_page_token(prefix: &str, offset: usize) -> String {
    format!("{prefix}-{offset:x}")
}

/// The offset a `NextToken` this service issued for `prefix` resumes at, or
/// `None` for a token it never issued.
pub fn decode_page_token(prefix: &str, token: &str) -> Option<usize> {
    let rest = token.strip_prefix(prefix)?.strip_prefix('-')?;
    usize::from_str_radix(rest, 16).ok()
}

/// The deepest `Details` / `DetailsDocument` this service accepts.
///
/// Neither `Json` nor `JsonDocumentType` bounds nesting, but converting, storing
/// and reporting a details value walks it once per level, so a value nested
/// without limit would exhaust the stack — which Rust cannot catch. 64 levels is
/// far deeper than any entity description and below `serde_json`'s own
/// 128-level recursion guard, so a value this service accepts can always be
/// parsed, converted and read back.
pub const MAX_DETAILS_DEPTH: usize = 64;

/// True when `value` nests deeper than [`MAX_DETAILS_DEPTH`].
///
/// Counted with an explicit stack rather than by recursing, so measuring an
/// arbitrarily deep value is itself bounded.
pub fn exceeds_details_depth(value: &Value) -> bool {
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_DETAILS_DEPTH {
            return true;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => {
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// `serde_json`'s value as the `Document` the model's `JsonDocumentType`
/// members are deserialized into, or `None` when it nests deeper than
/// [`MAX_DETAILS_DEPTH`].
pub fn json_to_document(value: &Value) -> Option<Document> {
    to_document(value, MAX_DETAILS_DEPTH)
}

/// [`json_to_document`] with the levels it may still descend, so the recursion
/// can never outrun [`MAX_DETAILS_DEPTH`] frames.
fn to_document(value: &Value, budget: usize) -> Option<Document> {
    let next = budget.checked_sub(1)?;
    Some(match value {
        Value::Null => Document::Null,
        Value::Bool(flag) => Document::Bool(*flag),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                Document::Number(::aws_smithy_types::Number::PosInt(unsigned))
            } else if let Some(signed) = number.as_i64() {
                Document::Number(::aws_smithy_types::Number::NegInt(signed))
            } else {
                Document::Number(::aws_smithy_types::Number::Float(
                    number.as_f64().unwrap_or_default(),
                ))
            }
        }
        Value::String(text) => Document::String(text.clone()),
        Value::Array(items) => Document::Array(
            items
                .iter()
                .map(|item| to_document(item, next))
                .collect::<Option<Vec<Document>>>()?,
        ),
        Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(key, value)| to_document(value, next).map(|value| (key.clone(), value)))
                .collect::<Option<_>>()?,
        ),
    })
}

/// The inverse of [`json_to_document`], so a submitted `DetailsDocument` is
/// stored, merged and compared as one JSON value whichever member carried it.
/// `None` for a document nested deeper than [`MAX_DETAILS_DEPTH`], which the
/// handler refuses rather than storing.
pub fn document_to_json(document: &Document) -> Option<Value> {
    to_json(document, MAX_DETAILS_DEPTH)
}

/// [`document_to_json`] with the levels it may still descend.
fn to_json(document: &Document, budget: usize) -> Option<Value> {
    let next = budget.checked_sub(1)?;
    Some(match document {
        Document::Null => Value::Null,
        Document::Bool(flag) => Value::Bool(*flag),
        Document::Number(number) => match number {
            ::aws_smithy_types::Number::PosInt(unsigned) => Value::from(*unsigned),
            ::aws_smithy_types::Number::NegInt(signed) => Value::from(*signed),
            ::aws_smithy_types::Number::Float(float) => {
                serde_json::Number::from_f64(*float).map_or(Value::Null, Value::Number)
            }
        },
        Document::String(text) => Value::String(text.clone()),
        Document::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| to_json(item, next))
                .collect::<Option<Vec<Value>>>()?,
        ),
        Document::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, value)| to_json(value, next).map(|value| (key.clone(), value)))
                .collect::<Option<_>>()?,
        ),
    })
}

/// The modelled `ValidationException` payload for a request this service
/// refuses, naming the member that was wrong.
pub fn validation(field: &str, message: impl Into<String>) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: Some(message.clone()),
        validation_exception_field_list: Some(vec![ValidationExceptionField {
            // `ValidationExceptionReason`'s value for a member whose value the
            // service rejects.
            reason: Some("FieldValidationFailed".to_string()),
            field: Some(field.to_string()),
            message: Some(message),
            entity_type: None,
            entity_id: None,
            change_type: None,
        }]),
    }
}

/// The modelled `ValidationException` payload for a member this service could
/// not read at all — `ValidationExceptionReason`'s `CannotParse`.
pub fn cannot_parse(field: &str, message: impl Into<String>) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: Some(message.clone()),
        validation_exception_field_list: Some(vec![ValidationExceptionField {
            reason: Some("CannotParse".to_string()),
            field: Some(field.to_string()),
            message: Some(message),
            entity_type: None,
            entity_id: None,
            change_type: None,
        }]),
    }
}

/// The modelled `ValidationException` payload for a `Catalog` other than the
/// fixed value every operation documents.
pub fn wrong_catalog(catalog: &str) -> ValidationException {
    validation(
        "Catalog",
        format!("Catalog must be {AWS_MARKETPLACE_CATALOG}, not {catalog:?}"),
    )
}

/// The modelled `ResourceNotFoundException` payload for a change set id that
/// names no change set.
pub fn change_set_not_found(change_set_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Change set {change_set_id} was not found")),
    }
}

/// The modelled `ResourceNotFoundException` payload for an entity id that names
/// no entity.
///
/// The wording is this service's own for an absent resource, as its recorded
/// answers show it ("Requested entity '<id>' does not exist."): it names the id
/// the request asked for, so the caller can see which one was missing.
pub fn entity_not_found(entity_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Requested entity '{entity_id}' does not exist.")),
    }
}

/// The modelled `ResourceNotFoundException` payload for a resource ARN that
/// names a resource of this catalog the store does not hold.
pub fn resource_not_found(resource: &CatalogResource) -> ResourceNotFoundException {
    match resource {
        CatalogResource::Entity { id, .. } => entity_not_found(id),
        CatalogResource::ChangeSet { id } => change_set_not_found(id),
    }
}

/// The modelled `AccessDeniedException` payload for an action on a resource ARN
/// this catalog cannot authorize: it names the IAM action (this service's
/// `arnNamespace` is its IAM service prefix) and the resource the request named.
pub fn access_denied(operation: &str, resource_arn: &str) -> AccessDeniedException {
    let resource = arn_resource(resource_arn);
    AccessDeniedException {
        message: Some(format!(
            "Access is denied. The caller is not authorized to perform: \
             {ARN_NAMESPACE}:{operation} on resource: {resource} because no \
             resource-based policy allows the {ARN_NAMESPACE}:{operation} action"
        )),
    }
}

/// How this service states a resource ARN it will not accept: the value it
/// rejected and the syntax it required, which is the modelled `ResourceARN`
/// `@pattern` the request failed.
pub fn invalid_resource_arn_message(resource_arn: &str, pattern: &str) -> String {
    format!("[Requested resource arn '{resource_arn}' is invalid. It should match with {pattern}.]")
}

/// The modelled `ResourceInUseException` payload for a resource another call
/// holds.
pub fn in_use(message: impl Into<String>) -> ResourceInUseException {
    ResourceInUseException {
        message: Some(message.into()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacecatalogAwsmpseymourState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacecatalogAwsmpseymourState>)
}
