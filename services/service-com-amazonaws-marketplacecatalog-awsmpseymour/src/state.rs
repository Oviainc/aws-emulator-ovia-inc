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
    AccessDeniedException, AMISecuritySummary, AmiProductSummary, AssessmentSummary,
    AssessmentTargetFilter, AssessmentTargetSummary, ChangeSetSummaryListItem, ChangeSummary,
    ContainerProductSummary, ContainerSecuritySummary, ControlAssessment, DataProductSummary,
    Entity, EntitySummary, FrameworkFilters, FrameworkSummary, MachineLearningProductSummary,
    ResourceInUseException, ResourceNotFoundException, SaaSProductSummary, Tag,
    ValidationException,
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

/// The entity types this catalog has at all — what an `EntityType` member is
/// checked against, beyond the characters `EntityType`'s `@pattern` admits.
///
/// `EntityType` is `^[a-zA-Z]+$`, which admits any letters-only word; the
/// catalog itself serves a FIXED set of types, and the model names it twice over
/// — once as `EntitySummary`'s per-type summary members
/// (`AmiProductSummary`, `ContainerProductSummary`, `DataProductSummary`,
/// `SaaSProductSummary`, `MachineLearningProductSummary`, `OfferSummary`,
/// `ResaleAuthorizationSummary`, `OfferSetSummary`,
/// `ExpressPrivateOfferConfigurationSummary`) and once as the `EntityTypeFilters`
/// / `EntityTypeSort` union members (`AmiProductFilters`, …), each the type's
/// name with the suffix dropped. A letters-only value naming none of them names
/// no entity type, so it is refused rather than listed as empty.
pub const ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "ExpressPrivateOfferConfiguration",
    "MachineLearningProduct",
    "Offer",
    "OfferSet",
    "ResaleAuthorization",
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

/// The largest page `ListAssessments` serves:
/// `ListAssessmentsMaxResultInteger` is `@range(min: 1, max: 100)`. Its
/// `MaxResults` is `@default(20)` ("If a value isn't provided, the default value
/// is 20"), which the boundary applies before a handler runs.
pub const LIST_ASSESSMENTS_PAGE_LIMIT: i32 = 100;

/// The largest page `DescribeAssessment` serves of one assessment's
/// `ControlAssessments`: `DescribeAssessmentMaxResultInteger` is
/// `@range(min: 1, max: 100)`, `@default(50)`.
pub const DESCRIBE_ASSESSMENT_PAGE_LIMIT: i32 = 100;

/// The `NextToken` prefix `ListAssessments` pages with.
pub const ASSESSMENT_PAGE_PREFIX: &str = "ASMT";

/// The `NextToken` prefix `DescribeAssessment` pages its `ControlAssessments`
/// with.
pub const CONTROL_PAGE_PREFIX: &str = "CTRL";

/// The prefix of every assessment id, from `AssessmentIdentifier`'s own pattern
/// (`^(assessment-[a-zA-Z0-9]+|arn:…/Assessment/assessment-[a-zA-Z0-9]+)$`) and
/// from `DescribeAssessmentRequest$AssessmentIdentifier`'s doc ("for example,
/// assessment-12345").
pub const ASSESSMENT_ID_PREFIX: &str = "assessment-";

/// The resource type an assessment ARN names, from the same two sources
/// ("arn:aws:aws-marketplace:us-east-1::AWSMarketplace/Assessment/assessment-12345").
pub const ASSESSMENT_ARN_RESOURCE_TYPE: &str = "Assessment";

/// The frameworks this catalog assesses against, named by the `FrameworkSummary`
/// and `FrameworkFilters` union members the model declares (`AMISecuritySummary`
/// / `AMISecurityFilters` -> `AMISecurity`, `ContainerSecuritySummary` /
/// `ContainerSecurityFilters` -> `ContainerSecurity`); `FrameworkId`'s doc gives
/// `AMISecurity` as the unversioned form and `DescribeAssessmentResponse$FrameworkId`'s
/// the versioned one ("frameworkId@version (for example, AMISecurity@1.0)").
pub const ASSESSMENT_FRAMEWORKS: &[&str] = &["AMISecurity", "ContainerSecurity"];

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

/// One assessment, as this emulator persists it: the record
/// `DescribeAssessment` reports and `ListAssessments` summarises.
///
/// An assessment is the RESULT of evaluating an entity or a change set against a
/// framework (`ListAssessments`: "An assessment is the result of evaluating a
/// product or change set against a framework, such as AMI Security or Container
/// Security"). This service models no operation that performs such an
/// evaluation, and this emulator runs none — so no call creates a record here,
/// and both operations answer from a store that is consequently empty
/// (`ListAssessments` an empty list, `DescribeAssessment` the modelled
/// `ResourceNotFoundException`). Reporting a `PASS`/`FAIL` for a framework
/// evaluation nothing ran would be a fabricated result, which is worse than
/// reporting that the account holds no assessments.
#[derive(Debug, Clone)]
pub struct AssessmentRecord {
    /// `ResourceId`'s `^[\w\-]+$`, in the `assessment-…` form
    /// `AssessmentIdentifier` accepts back.
    pub id: String,
    pub arn: String,
    /// `VersionedFrameworkId`: `<framework>@<version>`, whose framework is one of
    /// [`ASSESSMENT_FRAMEWORKS`].
    pub framework_id: String,
    /// The framework evaluated, one of [`ASSESSMENT_FRAMEWORKS`] — which
    /// `FrameworkSummary` member the report carries.
    pub framework: String,
    /// The delivery option the framework evaluated, carried by both
    /// `FrameworkSummary` members.
    pub delivery_option_id: Option<String>,
    /// `AssessmentTargetSummary$EntityId`: the entity assessed, if it was one.
    pub entity_id: Option<String>,
    /// `AssessmentTargetSummary$ChangeSetId`: the change set assessed, if it was
    /// one.
    pub change_set_id: Option<String>,
    /// `AssessmentResult`: `PASS` or `FAIL`.
    pub result: String,
    /// `DateTimeISO8601`: `....-..-..T..:..:..Z`.
    pub created_at: String,
    /// `DateTimeISO8601`, the instant the assessment expires.
    pub expires_at: String,
    /// One entry per control the framework evaluated.
    pub controls: Vec<ControlAssessment>,
}

impl AssessmentRecord {
    /// `AssessmentTargetSummary`: which entity or change set was assessed.
    pub fn target_summary(&self) -> AssessmentTargetSummary {
        AssessmentTargetSummary {
            entity_id: self.entity_id.clone(),
            change_set_id: self.change_set_id.clone(),
        }
    }

    /// `FrameworkSummary`: "The set member corresponds to the framework
    /// identified by `FrameworkId`".
    pub fn framework_summary(&self) -> Option<FrameworkSummary> {
        match self.framework.as_str() {
            "AMISecurity" => Some(FrameworkSummary::AMISecuritySummary(AMISecuritySummary {
                delivery_option_id: self.delivery_option_id.clone(),
            })),
            "ContainerSecurity" => Some(FrameworkSummary::ContainerSecuritySummary(
                ContainerSecuritySummary {
                    delivery_option_id: self.delivery_option_id.clone(),
                },
            )),
            _ => None,
        }
    }

    /// The `AssessmentSummary` projection `ListAssessments` reports.
    pub fn to_summary(&self) -> AssessmentSummary {
        AssessmentSummary {
            assessment_arn: Some(self.arn.clone()),
            assessment_id: Some(self.id.clone()),
            framework_id: Some(self.framework_id.clone()),
            assessment_target_summary: Some(self.target_summary()),
            framework_summary: self.framework_summary(),
            assessment_result: Some(self.result.clone()),
            created_at: Some(self.created_at.clone()),
            expires_at: Some(self.expires_at.clone()),
        }
    }

    /// Whether this assessment was performed against the framework
    /// `framework_id` names — `ListAssessmentsRequest$FrameworkId` is the
    /// UNVERSIONED id ("For example, AMISecurity"), so a request naming one
    /// selects every version of it, and one naming a version selects that
    /// version alone.
    pub fn matches_framework_id(&self, framework_id: &str) -> bool {
        if self.framework_id == framework_id {
            return true;
        }
        self.framework == framework_id
    }

    /// Whether this assessment satisfies an `AssessmentTargetFilter`: "Filters
    /// the list of assessments to those performed against a specific entity or
    /// change set". A member the filter leaves absent constrains nothing.
    pub fn matches_target(&self, filter: &AssessmentTargetFilter) -> bool {
        let entity = filter
            .entity_id
            .as_deref()
            .is_none_or(|wanted| self.entity_id.as_deref() == Some(wanted));
        let change_set = filter
            .change_set_id
            .as_deref()
            .is_none_or(|wanted| self.change_set_id.as_deref() == Some(wanted));
        entity && change_set
    }

    /// Whether this assessment satisfies `FrameworkFilters`: "Set exactly one
    /// member to filter results to assessments performed against that
    /// framework" — so the member chosen selects the framework, and the
    /// `DeliveryOptionId` it carries narrows further.
    pub fn matches_framework_filters(&self, filters: &FrameworkFilters) -> bool {
        let (framework, delivery_option) = match filters {
            FrameworkFilters::AMISecurityFilters(filter) => {
                ("AMISecurity", filter.delivery_option_id.as_deref())
            }
            FrameworkFilters::ContainerSecurityFilters(filter) => {
                ("ContainerSecurity", filter.delivery_option_id.as_deref())
            }
        };
        self.framework == framework
            && delivery_option.is_none_or(|wanted| self.delivery_option_id.as_deref() == Some(wanted))
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
    /// Every assessment, keyed by its id. Nothing creates one: an assessment is
    /// the result of a framework evaluation, which this service models no
    /// operation for and this emulator runs none of — see [`AssessmentRecord`].
    pub assessments: BTreeMap<String, AssessmentRecord>,
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
        self.entities.get(entity_id_of(identifier))
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
            // `Change$Entity$Type` is a REFERENCE to an entity type, optionally
            // at a schema version (`DataProduct@1.0`); the entity itself is of
            // the type that reference names, so the record and its ARN carry
            // the type, never the version the request addressed it through.
            let entity_type = entity_type_of(&change.entity_type).to_string();
            let entry = self.entities.entry(entity_id.clone()).or_insert_with(|| {
                EntityRecord {
                    arn: entity_arn(&region, &entity_type, &entity_id),
                    id: entity_id.clone(),
                    entity_type,
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

/// The ARN of an assessment, in the form
/// `DescribeAssessmentRequest$AssessmentIdentifier` documents
/// ("arn:aws:aws-marketplace:us-east-1::AWSMarketplace/Assessment/assessment-12345")
/// and `ARN`'s `^[a-zA-Z0-9:*/-]+$` allows.
pub fn assessment_arn(region: &str, assessment_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/{ASSESSMENT_ARN_RESOURCE_TYPE}/{assessment_id}"),
    )
}

/// True when `value` is an assessment id: [`ASSESSMENT_ID_PREFIX`] followed by at
/// least one alphanumeric character, which is `AssessmentIdentifier`'s
/// `assessment-[a-zA-Z0-9]+` alternative.
pub fn is_assessment_id(value: &str) -> bool {
    match value.strip_prefix(ASSESSMENT_ID_PREFIX) {
        Some(tail) => !tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}

/// The assessment `identifier` names, for an identifier of either form
/// `AssessmentIdentifier` documents: the id itself, or the id at the end of an
/// assessment ARN of `catalog` in this service's own ARN namespace. `None` for
/// anything else, which is what the caller refuses the request for.
///
/// The ARN alternative of the modelled pattern constrains only the SHAPE of an
/// ARN (`arn:<partition>:<service>:<region>:<account>:<type>/Assessment/assessment-…`),
/// so an ARN naming another service's namespace or another catalog satisfies it
/// while naming no assessment of this catalog. The partition, region and account
/// are the caller's: this service serves one catalog, whatever region a caller
/// signs for, so they do not select the record.
pub fn assessment_id_of<'a>(identifier: &'a str, catalog: &str) -> Option<&'a str> {
    if is_assessment_id(identifier) {
        return Some(identifier);
    }
    let mut fields = identifier.splitn(6, ':');
    let (
        Some("arn"),
        Some(_partition),
        Some(ARN_NAMESPACE),
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
    let id = resource.strip_prefix(&format!("{catalog}/{ASSESSMENT_ARN_RESOURCE_TYPE}/"))?;
    is_assessment_id(id).then_some(id)
}

/// The syntax the service publishes for `AssessmentIdentifier`, which it reports
/// in the fault for one outside it: the shape's own modelled `@pattern`
/// (`smithy_query(query="shape", shape="AssessmentIdentifier")`), which already
/// states both forms the member's doc documents ("You can provide either the
/// assessment ID … or the full assessment ARN").
///
/// [`assessment_id_of`] is STRICTER than this string on the ARN alternative: the
/// pattern constrains only the SHAPE of an ARN, so one naming another service's
/// namespace or another catalog satisfies it while naming no assessment of this
/// catalog, and such an identifier is refused too.
pub const ASSESSMENT_IDENTIFIER_PATTERN: &str = concat!(
    r"^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:",
    r"[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$"
);

/// The modelled `ValidationException` payload for an `AssessmentIdentifier` that
/// is neither of the forms [`assessment_id_of`] resolves — built from the
/// request, naming the identifier it rejected and the syntax it accepts.
pub fn invalid_assessment_identifier(identifier: &str) -> ValidationException {
    validation_message(format!(
        "Requested assessment identifier '{identifier}' is invalid. \
         It should match with {ASSESSMENT_IDENTIFIER_PATTERN}."
    ))
}

/// The modelled `ResourceNotFoundException` payload for an assessment id that
/// names no assessment.
pub fn assessment_not_found(assessment_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Requested assessment '{assessment_id}' does not exist."
        )),
    }
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

/// The resource ID `arn` names: the last `/`-separated segment of its resource
/// portion, or the whole portion when it carries none.
///
/// A resource ARN of this catalog carries `<catalog>/<type>/<id>` there
/// ([`entity_arn`], [`change_set_arn`]), of which the ID is what names the
/// resource — the catalog and the type say WHERE it lives — so it is the ID that
/// an authorization refusal reports as the resource it refused.
pub fn arn_resource_id(arn: &str) -> &str {
    let resource = arn_resource(arn);
    resource.rsplit('/').next().unwrap_or(resource)
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

/// The modelled `ValidationException` payload stated the way this service states
/// one: everything it has to say about the request is in `Message`, as the
/// bracketed list of per-member sentences, and `ValidationExceptionFieldList` is
/// left absent.
///
/// The form is the service's own, read off its recorded refusals: two different
/// operations answer a rejected member with a `Message` of exactly that shape
/// (`[Requested entity id '…' is invalid. It should match with …]`,
/// `[Requested sort by '…' is invalid. It should be one of '…'.]`) and with no
/// `ValidationExceptionFieldList` beside it. So the message is what names the
/// member, and every refusal built here passes one that does.
pub fn validation_message(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(format!("[{}]", message.into())),
        validation_exception_field_list: None,
    }
}

/// The modelled `ValidationException` payload for a request this service
/// refuses, naming the member that was wrong — [`validation_message`] with the
/// member named ahead of what was wrong with it.
pub fn validation(field: &str, message: impl Into<String>) -> ValidationException {
    validation_message(format!("{field}: {}", message.into()))
}

/// The modelled `ValidationException` payload for a member this service could
/// not read at all.
///
/// Stated like any other refusal: the only thing that distinguished it was
/// `ValidationExceptionField$Reason`, and this service reports no field list to
/// carry a reason in, so the message says what could not be read.
pub fn cannot_parse(field: &str, message: impl Into<String>) -> ValidationException {
    validation(field, message)
}

/// The modelled `ValidationException` payload for a change set that contradicts
/// itself: two of its changes carry the same change type against the same
/// entity.
///
/// `StartChangeSet` documents the rule ("Within a single `ChangeSet`, you can't
/// start the same change type against the same entity multiple times"), and it
/// is a rule about the REQUEST — so it is judged on the entity the request names,
/// before that entity is resolved, and the fault is built from the request: the
/// change type that was repeated and the entity it was repeated on.
pub fn duplicate_change_pair(change_type: &str, entity_id: &str) -> ValidationException {
    validation_message(format!(
        "Requested changes contain multiple changes for the same change type \
         '{change_type}' on entity '{entity_id}'. Entity - change type pairs \
         should be unique."
    ))
}

/// The modelled `ValidationException` payload for a `Catalog` other than the
/// fixed value every operation documents.
pub fn wrong_catalog(catalog: &str) -> ValidationException {
    validation(
        "Catalog",
        format!("Catalog must be {AWS_MARKETPLACE_CATALOG}, not {catalog:?}"),
    )
}

/// The syntax the service documents for a catalog NAME, which it reports in the
/// fault for a `Catalog` outside it.
///
/// It is WIDER than the modelled `Catalog` `@pattern` (`^[a-zA-Z]+$`) — digits,
/// dots and hyphens are admitted — and narrower in its length (47, not 64), so
/// the model's pattern cannot be what a caller is told to satisfy. This is the
/// pattern the service's own recorded refusals quote, and
/// [`is_catalog_name`] is this string spelled out as a predicate.
pub const CATALOG_PATTERN: &str =
    r"^(?![.-])(?!.*[.-]{2})(?![0-9]+\.[0-9.]+$)[a-zA-Z0-9.-]{1,47}(?<![.-])$";

/// How long a catalog name may be: the `{1,47}` of [`CATALOG_PATTERN`].
const CATALOG_NAME_MAX: usize = 47;

/// Whether `value` is a catalog NAME — [`CATALOG_PATTERN`] as a predicate: one
/// to [`CATALOG_NAME_MAX`] characters of `[a-zA-Z0-9.-]`, neither starting nor
/// ending with `.` or `-`, with no two of those adjacent, and not a bare version
/// number (`[0-9]+\.[0-9.]+`).
pub fn is_catalog_name(value: &str) -> bool {
    if value.is_empty() || value.chars().count() > CATALOG_NAME_MAX {
        return false;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return false;
    }
    let separator = |c: char| matches!(c, '.' | '-');
    if value.starts_with(separator) || value.ends_with(separator) {
        return false;
    }
    if value
        .as_bytes()
        .windows(2)
        .any(|pair| pair.iter().all(|byte| separator(char::from(*byte))))
    {
        return false;
    }
    !is_version_number(value)
}

/// Whether `value` is the `[0-9]+\.[0-9.]+` form [`CATALOG_PATTERN`] excludes:
/// digits, a dot, then digits and dots only.
fn is_version_number(value: &str) -> bool {
    let Some((head, tail)) = value.split_once('.') else {
        return false;
    };
    !head.is_empty()
        && head.bytes().all(|byte| byte.is_ascii_digit())
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The modelled `ValidationException` payload for a `Catalog` outside
/// [`CATALOG_PATTERN`] — built from the request, naming the value it rejected and
/// the syntax a catalog name has to satisfy.
pub fn invalid_catalog(catalog: &str) -> ValidationException {
    validation_message(format!(
        "Requested catalog '{catalog}' is invalid. It should match with {CATALOG_PATTERN}."
    ))
}

/// The `Catalog` check every operation of this service runs before it does
/// anything else: a value that is not a catalog NAME at all is refused for its
/// syntax, and a well-formed name other than the one catalog this service serves
/// is refused as naming nothing it has.
///
/// The two are different faults: the first says the request is malformed, the
/// second that the catalog it named is not this one.
pub fn check_catalog(catalog: &str) -> Result<(), ValidationException> {
    if !is_catalog_name(catalog) {
        return Err(invalid_catalog(catalog));
    }
    if catalog != AWS_MARKETPLACE_CATALOG {
        return Err(wrong_catalog(catalog));
    }
    Ok(())
}

/// Whether `value` names one of [`ENTITY_TYPES`].
pub fn is_entity_type(value: &str) -> bool {
    ENTITY_TYPES.contains(&value)
}

/// The modelled `ValidationException` payload for an `EntityType` this catalog
/// has no such type for.
///
/// The fault names the value and NOT a pattern: the value may well satisfy
/// `EntityType`'s `@pattern` and still name no type, so there is no syntax to
/// tell the caller to satisfy — the set in [`ENTITY_TYPES`] is what it failed.
pub fn invalid_entity_type(entity_type: &str) -> ValidationException {
    validation_message(format!("Requested entity type '{entity_type}' is invalid."))
}

/// The entity type a `Change$Entity$Type` names: everything before the optional
/// `@<version>` suffix.
///
/// `Entity$Type` addresses an entity type at a SCHEMA VERSION
/// (`DataProduct@1.0`) — the form every real caller sends and the form the
/// service accepts — while the entity itself is of the type the reference names.
pub fn entity_type_of(reference: &str) -> &str {
    reference.split('@').next().unwrap_or(reference)
}

/// Whether `reference` is an `Entity$Type`: one of [`ENTITY_TYPES`], on its own
/// or at a version (`<EntityType>@<version>`).
pub fn is_entity_type_reference(reference: &str) -> bool {
    match reference.split_once('@') {
        Some((entity_type, version)) => {
            is_entity_type(entity_type) && is_entity_type_version(version)
        }
        None => is_entity_type(reference),
    }
}

/// Whether `version` is the `@<version>` part of an `Entity$Type`: a non-empty
/// run of alphanumerics separated by dots, which is the form the model's own
/// versioned ids take (`VersionedFrameworkId`'s doc: "frameworkId@version (for
/// example, AMISecurity@1.0)").
fn is_entity_type_version(version: &str) -> bool {
    !version.is_empty()
        && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        && !version.starts_with('.')
        && !version.ends_with('.')
        && !version.contains("..")
}

/// The syntax the service documents for `Entity$Identifier`, which it reports in
/// the fault for one outside it.
///
/// Two alternatives: an entity id optionally at a revision, and the
/// `$<ChangeName>.Entity.Identifier` reference by which one change of a change
/// set names the entity another change of the SAME change set creates. Both are
/// wider than the modelled `Identifier` `@pattern` (`^[\w\-@]+$`) in the
/// characters they admit (`.` and `/`) and narrower in their shape, so the
/// model's pattern is not what a caller is told to satisfy.
pub const ENTITY_IDENTIFIER_PATTERN: &str =
    r"(^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9](@\d+)?$)|(^\$[a-zA-Z]{1,72}\.Entity\.Identifier$)";

/// The suffix of the change-reference alternative of [`ENTITY_IDENTIFIER_PATTERN`].
const CHANGE_REFERENCE_SUFFIX: &str = ".Entity.Identifier";

/// The longest change name the change-reference alternative of
/// [`ENTITY_IDENTIFIER_PATTERN`] admits (its `{1,72}`).
const CHANGE_REFERENCE_NAME_MAX: usize = 72;

/// Whether `value` is an `Entity$Identifier`: [`ENTITY_IDENTIFIER_PATTERN`]
/// spelled out as a predicate.
pub fn is_entity_identifier(value: &str) -> bool {
    if let Some(change_name) = value
        .strip_prefix('$')
        .and_then(|rest| rest.strip_suffix(CHANGE_REFERENCE_SUFFIX))
    {
        return (1..=CHANGE_REFERENCE_NAME_MAX).contains(&change_name.chars().count())
            && change_name.chars().all(|c| c.is_ascii_alphabetic());
    }
    match value.split_once('@') {
        Some((id, revision)) => {
            !revision.is_empty()
                && revision.bytes().all(|byte| byte.is_ascii_digit())
                && is_entity_id(id)
        }
        None => is_entity_id(value),
    }
}

/// The entity id an `Entity$Identifier` names: the id without the `@<revision>`
/// an identifier may address it at.
pub fn entity_id_of(identifier: &str) -> &str {
    identifier.split('@').next().unwrap_or(identifier)
}

/// The modelled `ValidationException` payload for an `Entity$Identifier` outside
/// [`ENTITY_IDENTIFIER_PATTERN`] — built from the request, naming the value and
/// the syntax it had to satisfy.
pub fn invalid_entity_identifier(identifier: &str) -> ValidationException {
    validation_message(format!(
        "Requested entity identifier '{identifier}' is invalid. \
         It should match with {ENTITY_IDENTIFIER_PATTERN}."
    ))
}

/// The syntax the service documents for `ChangeSetName`: the modelled
/// `@pattern` with the modelled `@length(min: 1, max: 100)` folded into it,
/// which is how the service quotes it back to a caller.
pub const CHANGE_SET_NAME_PATTERN: &str = r"^[\w\s+=.:@-]{1,100}$";

/// The `{1,100}` of [`CHANGE_SET_NAME_PATTERN`].
const CHANGE_SET_NAME_MAX: usize = 100;

/// Whether `value` is a `ChangeSetName`: [`CHANGE_SET_NAME_PATTERN`] as a
/// predicate — one to [`CHANGE_SET_NAME_MAX`] characters of `\w` (an ASCII word
/// character), `\s` (whitespace) or `+=.:@-`.
///
/// `\w` is read as ASCII, not Unicode: the pattern is the one the service
/// publishes and its `\w` is the ASCII class, so a name of letters outside ASCII
/// is refused rather than quietly accepted against a wider class than the
/// pattern the caller was told to satisfy.
pub fn is_change_set_name(value: &str) -> bool {
    (1..=CHANGE_SET_NAME_MAX).contains(&value.chars().count())
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c.is_whitespace()
                || c == '_'
                || matches!(c, '+' | '=' | '.' | ':' | '@' | '-')
        })
}

/// The modelled `ValidationException` payload for a `ChangeSetName` outside
/// [`CHANGE_SET_NAME_PATTERN`].
pub fn invalid_change_set_name(name: &str) -> ValidationException {
    validation_message(format!(
        "Requested change set name '{name}' is invalid. \
         It should match with {CHANGE_SET_NAME_PATTERN}."
    ))
}

/// The syntax the service documents for `ClientRequestToken`: the modelled
/// `@pattern` with the modelled `@length(min: 1, max: 64)` folded into it.
pub const CLIENT_REQUEST_TOKEN_PATTERN: &str = r"^[!-~]{1,64}$";

/// The `{1,64}` of [`CLIENT_REQUEST_TOKEN_PATTERN`].
const CLIENT_REQUEST_TOKEN_MAX: usize = 64;

/// Whether `value` is a `ClientRequestToken`: [`CLIENT_REQUEST_TOKEN_PATTERN`]
/// as a predicate — one to [`CLIENT_REQUEST_TOKEN_MAX`] printable ASCII
/// characters, space excluded (`!` to `~`).
pub fn is_client_request_token(value: &str) -> bool {
    (1..=CLIENT_REQUEST_TOKEN_MAX).contains(&value.chars().count())
        && value.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
}

/// The modelled `ValidationException` payload for a `ClientRequestToken` outside
/// [`CLIENT_REQUEST_TOKEN_PATTERN`].
pub fn invalid_client_request_token(token: &str) -> ValidationException {
    validation_message(format!(
        "Requested client request token '{token}' is invalid. \
         It should match with {CLIENT_REQUEST_TOKEN_PATTERN}."
    ))
}

/// The modelled `ValidationException` payload for a `NextToken` that names no
/// page of the list that was called.
///
/// A page token is OPAQUE: it is a value the service itself issued, so the fault
/// quotes neither the value nor a syntax — there is nothing a caller can correct
/// it to, only a page it has to ask for again. The same fault answers a token
/// outside the modelled `NextToken` syntax and one that is well formed but names
/// no page, because neither names a page.
pub fn invalid_page_token() -> ValidationException {
    validation_message("Requested page token is invalid.")
}

/// The form an entity id takes, as the model's own `EntityId` shape states it
/// (`@pattern("^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$")`).
///
/// `EntityId` is the shape every member that NAMES an entity targets —
/// `EntityRequest$EntityId`, the `EntityDetails` / `Errors` map keys of
/// `BatchDescribeEntities`, `ValidationExceptionField$EntityId` — so it is this
/// service's statement of which strings are entity ids at all. A handful of
/// members, `DescribeEntityRequest$EntityId` among them, target the wider
/// `ResourceId` (`^[\w\-]+$`) instead, which admits strings no entity id can be
/// (an underscore, or a single character); the service enforces the `EntityId`
/// form on those members too, and the pattern is reported verbatim in the
/// refusal so a caller can see what was required.
pub const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";

/// Whether `value` is an entity id: [`ENTITY_ID_PATTERN`], spelled out as a
/// predicate — an alphanumeric first and last character with one or more
/// alphanumeric, `.`, `/` or `-` characters between them (so the shortest entity
/// id is three characters long).
pub fn is_entity_id(value: &str) -> bool {
    let mut middle = value.chars();
    let (Some(first), Some(last)) = (middle.next(), middle.next_back()) else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && !middle.as_str().is_empty()
        && middle.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
}

/// The modelled `ValidationException` payload for an entity id outside
/// [`ENTITY_ID_PATTERN`] — what a read refuses BEFORE it looks the entity up, so
/// a malformed id is a 422 and only a well-formed one can reach the 404.
///
/// The message is built from the request: it names the rejected id and the
/// pattern the service requires, which is what lets a caller correct the call.
pub fn invalid_entity_id(entity_id: &str) -> ValidationException {
    validation_message(invalid_entity_id_message(entity_id))
}

/// The sentence that states `entity_id` is not an entity id, shared by the
/// top-level `ValidationException` a single entity read answers and the
/// `BatchDescribeErrorDetail$ErrorMessage` the batch read reports for the same
/// entry — so one read and a batch of them state a malformed id identically.
pub fn invalid_entity_id_message(entity_id: &str) -> String {
    format!(
        "Requested entity id '{entity_id}' is invalid. \
         It should match with {ENTITY_ID_PATTERN}."
    )
}

/// The modelled `ValidationException` payload for a `Sort$SortBy` outside the
/// attributes the list being called sorts on (`supported`).
///
/// The message is built from the request — the attribute it asked to sort by and
/// the set this list accepts — in the form the service states such a refusal in.
pub fn invalid_sort_by(sort_by: &str, supported: &[&str]) -> ValidationException {
    validation_message(format!(
        "Requested sort by '{sort_by}' is invalid. It should be one of '{}'.",
        supported.join(", ")
    ))
}

/// The modelled `ResourceNotFoundException` payload for a change set id that
/// names no change set.
///
/// Worded as this service words every "does not exist" refusal — naming the
/// resource the request asked for, which is the one rule a message built from the
/// request follows ("Requested change set '…' does not exist.", "Requested entity
/// '…' does not exist.", "Requested assessment '…' does not exist."). The error
/// code and the 404 are the contract; the prose follows that one form so a client
/// reading any of them reads the same sentence.
pub fn change_set_not_found(change_set_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Requested change set '{change_set_id}' does not exist."
        )),
    }
}

/// The modelled `ResourceNotFoundException` payload for an entity id that names
/// no entity.
///
/// The message is built from the request, naming the entity that was asked for,
/// in the form this service's recorded answers show it states a missing entity
/// in — the same sentence the `Errors` map of `BatchDescribeEntities` carries per
/// missing entity, so one entity read and a batch of them report a missing
/// entity identically.
pub fn entity_not_found(entity_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(entity_missing_message(entity_id)),
    }
}

/// The sentence that states `entity_id` names no entity, shared by the
/// `ResourceNotFoundException` a single entity read answers and the
/// `BatchDescribeErrorDetail$ErrorMessage` the batch read reports for the same
/// entity.
pub fn entity_missing_message(entity_id: &str) -> String {
    format!("Requested entity '{entity_id}' does not exist.")
}

/// The `BatchDescribeErrorDetail$ErrorCode` for an entity the catalog does not
/// hold.
///
/// `BatchDescribeEntities` models no top-level error for a missing entity (its
/// modelled errors are AccessDenied / InternalService / Throttling / Validation
/// only) and answers one in the `Errors` map instead, keyed by the entity id,
/// carrying the name of the error a single read of that entity would have
/// raised.
pub const BATCH_ERROR_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `BatchDescribeErrorDetail$ErrorCode` for an entry whose `EntityId` is not
/// an entity id at all.
///
/// A batch read reports a malformed entry the same way it reports a missing one
/// — per entry, under the id that was asked for, carrying the name of the error a
/// single read of that entry would have raised — so one bad entry never costs the
/// caller the results for the good ones in the same batch.
pub const BATCH_ERROR_VALIDATION: &str = "ValidationException";

/// The modelled `ResourceNotFoundException` payload for a resource ARN that
/// names a resource of this catalog the store does not hold.
pub fn resource_not_found(resource: &CatalogResource) -> ResourceNotFoundException {
    match resource {
        CatalogResource::Entity { id, .. } => entity_not_found(id),
        CatalogResource::ChangeSet { id } => change_set_not_found(id),
    }
}

/// The modelled `AccessDeniedException` payload for an action on a resource ARN
/// this catalog cannot authorize.
///
/// The message follows the authorization-failure form AWS states every such
/// refusal in — `User: <principal> is not authorized to perform: <action> on
/// resource: <resource>` — assembled from the authenticated principal
/// ([`caller_identity_arn`]), the IAM action (this service's `arnNamespace` is
/// its IAM service prefix, and `operation` the action name) and the resource the
/// request named ([`arn_resource_id`]). A client, a log scraper or an IAM
/// troubleshooter reads the denied principal out of that sentence.
pub fn access_denied(operation: &str, resource_arn: &str) -> AccessDeniedException {
    let resource = arn_resource_id(resource_arn);
    AccessDeniedException {
        message: Some(format!(
            "User: {} is not authorized to perform: {ARN_NAMESPACE}:{operation} on \
             resource: {resource} because no resource-based policy allows the \
             {ARN_NAMESPACE}:{operation} action",
            caller_identity_arn()
        )),
    }
}

/// The ARN of the identity every caller of this emulator authenticates as.
///
/// Every request is signed with the emulator's own credentials, so the caller is
/// the one IAM user this emulator publishes — the very principal its
/// `sts:GetCallerIdentity` answers, built the same way
/// (`::engine::emulator_arn("iam", "", "user/emulator")`), so the principal an
/// authorization refusal names is the principal the caller can look up.
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// How this service states a member it refused for its SYNTAX: one bracketed
/// fault naming the member, the value it rejected and the syntax it required.
///
/// `member` is the input member's name as a violation path spells it
/// (lowerCamel); the label is that name as words, which is how the service's own
/// recorded refusals address a member — `catalog`, `entity type`, `change set
/// name`, `resource arn` — rather than by the wire path.
pub fn invalid_member_message(member: &str, value: &str, pattern: &str) -> String {
    format!(
        "[Requested {} '{value}' is invalid. It should match with {pattern}.]",
        member_label(member)
    )
}

/// An input member's name as the words a fault addresses it by: its camelCase
/// humps split apart and lowercased (`resourceArn` -> `resource arn`).
fn member_label(member: &str) -> String {
    let mut label = String::with_capacity(member.len() + 2);
    for (index, c) in member.char_indices() {
        if c.is_uppercase() && index != 0 {
            label.push(' ');
        }
        label.extend(c.to_lowercase());
    }
    label
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
