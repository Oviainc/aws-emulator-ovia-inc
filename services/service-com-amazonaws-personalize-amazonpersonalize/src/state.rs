// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-personalize-amazonpersonalize service.
//!
//! Personalize addresses every resource by ARN, and an ARN is minted from the
//! resource's own identity (a schema's and a dataset group's name, a dataset's group and
//! type), so each map below is keyed by the ARN and no separate name index is needed: a
//! duplicate-name create mints an ARN that is already present.
//!
//! The maps are `BTreeMap`s, so a `List*` enumerates in a stable order and an opaque
//! `nextToken` only has to carry the key the previous page ended on.
//!
//! Besides the state itself this module holds the helpers more than one operation needs —
//! ARN minting, the create-status clock, the page/token codec and the shared value
//! checks — so the handlers of the dataset-group lifecycle cannot drift from each other.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Dataset, DatasetGroup, DatasetGroupSummary, DatasetSchema, DatasetSchemaSummary,
    DatasetSummary, Tag,
};

/// What `CreateSchema` persists, and what `DescribeSchema` reconstructs a
/// [`DatasetSchema`] from.
#[derive(Debug, Clone)]
pub struct SchemaRecord {
    pub name: String,
    pub schema_arn: String,
    /// The Avro schema document exactly as the caller supplied it.
    pub schema: String,
    pub domain: Option<String>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
}

/// What `CreateDatasetGroup` persists, and what `DescribeDatasetGroup` /
/// `ListDatasetGroups` reconstruct a [`DatasetGroup`] / [`DatasetGroupSummary`] from.
///
/// `status` is NOT stored: the model documents the progression "CREATE PENDING > CREATE
/// IN_PROGRESS > ACTIVE", so the status is a function of the clock and is derived from
/// `active_at` on every read (see [`creation_status`]).
#[derive(Debug, Clone)]
pub struct DatasetGroupRecord {
    pub name: String,
    pub dataset_group_arn: String,
    /// The region this group's ARN is scoped to, so a dataset created in the group is
    /// minted in the SAME region as its group whichever region the create is signed for.
    pub region: String,
    pub role_arn: Option<String>,
    pub kms_key_arn: Option<String>,
    pub domain: Option<String>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// The instant the clock owes this group its `ACTIVE` status.
    pub active_at: SystemTime,
    pub tags: Vec<Tag>,
}

/// What `CreateDataset` persists, and what `DescribeDataset` / `ListDatasets` reconstruct
/// a [`Dataset`] / [`DatasetSummary`] from. `status` is derived exactly as it is for a
/// dataset group.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub name: String,
    pub dataset_arn: String,
    pub dataset_group_arn: String,
    /// The canonical spelling of the dataset type (see [`canonical_dataset_type`]).
    pub dataset_type: String,
    pub schema_arn: String,
    /// The tracking id Amazon Personalize provides for an ACTION INTERACTIONS dataset and
    /// for no other type: "The ID of the event tracker for an Action interactions
    /// dataset. You specify the tracker's ID in the `PutActionInteractions` API
    /// operation" (`smithy_query(query="doc", shape="Dataset", member="trackingId")`).
    /// Minted once by the create and kept, so every read of the dataset reports the same
    /// id.
    pub tracking_id: Option<String>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// The instant the clock owes this dataset its `ACTIVE` status.
    pub active_at: SystemTime,
    pub tags: Vec<Tag>,
}

/// Per-service state for com-amazonaws-personalize-amazonpersonalize. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPersonalizeAmazonpersonalizeState {
    /// Schemas, keyed by `schemaArn`.
    pub schemas: BTreeMap<String, SchemaRecord>,
    /// Dataset groups, keyed by `datasetGroupArn`.
    pub dataset_groups: BTreeMap<String, DatasetGroupRecord>,
    /// Datasets, keyed by `datasetArn`.
    pub datasets: BTreeMap<String, DatasetRecord>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPersonalizeAmazonpersonalizeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPersonalizeAmazonpersonalizeState>)
}

/// The INGESTION registry in the shared engine: call it with a handler's `&*self.engine`.
///
/// A `datasetArn` is minted HERE (`CreateDataset`) and used by Amazon Personalize Events,
/// whose own API carries no dataset definition: a `PutItems` names an ARN and nothing
/// else. So that data plane can resolve the ARN only because this service recorded it, in
/// the registry that crate's store already reads
/// ([`::service_com_amazonaws_personalizeevents_amazonpersonalizeevents::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState`]).
/// The engine keys state by TYPE, so a copy of its own inside
/// [`ComAmazonawsPersonalizeAmazonpersonalizeState`] would be invisible to that reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this registry second, and the
/// ingestion handlers take only their own store, so the order cannot invert.
///
/// Answers `None` only for an engine that is not the shared in-memory one; the
/// registration is an aid to another service, so leave it unwritten then.
pub fn ingestion_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_com_amazonaws_personalizeevents_amazonpersonalizeevents::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_com_amazonaws_personalizeevents_amazonpersonalizeevents::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState,
            >,
        )
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------

/// How many schemas this store holds before a NEW `CreateSchema` is refused with the
/// modelled `LimitExceededException`.
///
/// This is the EMULATOR's own bound, not a claim about any AWS account quota: a
/// long-lived emulator serves many callers and `name` is a free-form string, so without a
/// ceiling N requests leave N resident records for ever. It is set far above any realistic
/// workflow, so nothing a test or a client does legitimately reaches it.
pub const MAX_SCHEMAS: usize = 10_000;

/// The same bound for dataset groups. See [`MAX_SCHEMAS`].
pub const MAX_DATASET_GROUPS: usize = 10_000;

/// The same bound for datasets. See [`MAX_SCHEMAS`].
pub const MAX_DATASETS: usize = 50_000;

// ---------------------------------------------------------------------------
// Request-derived values
// ---------------------------------------------------------------------------

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to stamp into an ARN for this request.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// A schema's ARN: `arn:aws:personalize:<region>:<account>:schema/<name>`.
///
/// Derived from the name, so a schema deleted and created again under the same name is
/// never handed two ARNs that are both "current", and the value is reproducible rather
/// than per-run.
pub fn schema_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("personalize", region, &format!("schema/{name}"))
}

/// A dataset group's ARN: `arn:aws:personalize:<region>:<account>:dataset-group/<name>`.
pub fn dataset_group_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("personalize", region, &format!("dataset-group/{name}"))
}

/// A dataset's ARN:
/// `arn:aws:personalize:<region>:<account>:dataset/<dataset group name>/<DATASET_TYPE>`.
///
/// A dataset's identity inside its group is its TYPE, not its name — the model documents
/// that a group holds at most one dataset of each type — so the type, upper-cased the way
/// Personalize renders it in an ARN, is the last segment.
pub fn dataset_arn(region: &str, dataset_group_name: &str, dataset_type: &str) -> String {
    ::emulator_server::emulator_arn(
        "personalize",
        region,
        &format!(
            "dataset/{dataset_group_name}/{}",
            dataset_type.to_ascii_uppercase()
        ),
    )
}

// ---------------------------------------------------------------------------
// Refusal wording
// ---------------------------------------------------------------------------
//
// Personalize words each refusal class itself, and the same class reads the same
// whichever operation raised it, so the text lives here instead of being composed at the
// rejection site. Only the parts the service itself varies are arguments.

/// What a lookup that resolves to nothing answers, for the ARN the caller asked about:
/// the one variable part of the sentence.
///
/// The sentence names the ARN and NOT the kind of resource it addresses, so one wording
/// serves every resource this service holds — a schema, a dataset group, a dataset — and
/// every operation that resolves an ARN, whether it reads, deletes or references it.
pub fn resource_not_found_message(arn: &str) -> String {
    format!("Resource Arn {arn} does not exist.")
}

/// What a schema document that does not declare every field its dataset type requires
/// answers, for the names it left out: the sentence's fixed text is followed by the
/// bracketed, comma-separated list of those names.
///
/// `missing` is expected in the order the dataset type requires its fields (see
/// [`required_schema_fields`]), so two documents omitting the same names read the same
/// however their own field lists are ordered.
pub fn missing_schema_fields_message(missing: &[&str]) -> String {
    format!("Schema is missing required fields: [{}]", missing.join(", "))
}

/// What a schema delete still referenced by a dataset answers. The whole sentence is
/// fixed: it names neither the schema nor the datasets holding it, however many there
/// are, so a caller never learns the referencing ARNs from the refusal.
pub const SCHEMA_IN_USE_MESSAGE: &str =
    "The provided schema is being referenced by one or more datasets";

/// What a dataset-group delete that still holds datasets answers — fixed in the same way,
/// naming neither the group nor its datasets.
pub const DATASET_GROUP_IN_USE_MESSAGE: &str =
    "The provided dataset-group is being referenced by one or more datasets";

// ---------------------------------------------------------------------------
// The create-status clock
// ---------------------------------------------------------------------------

/// `CREATE PENDING`: the first status a freshly created dataset group or dataset reports.
pub const STATUS_CREATE_PENDING: &str = "CREATE PENDING";
/// `CREATE IN_PROGRESS`: the second status of the documented progression.
pub const STATUS_CREATE_IN_PROGRESS: &str = "CREATE IN_PROGRESS";
/// `ACTIVE`: the terminal status of a successful create.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long after a create this emulator takes to report the new resource `ACTIVE`.
///
/// Personalize does NOT return a new dataset group or dataset ready: the model documents
/// the progression "CREATE PENDING > CREATE IN_PROGRESS > ACTIVE" and that "you must wait
/// until the status of the dataset group is ACTIVE before adding a dataset to the group",
/// so a caller has to poll `Describe*`. This span is the EMULATOR's own choice — AWS
/// documents no duration for it — short enough not to slow a client down and long enough
/// that a caller acting straight after the create still observes the transient states.
pub const CREATE_SPAN: Duration = Duration::from_millis(500);

/// The instant a resource created now becomes `ACTIVE`.
pub fn active_at(created_at: SystemTime) -> SystemTime {
    created_at + CREATE_SPAN
}

/// The status of a resource the clock owes `ACTIVE` at `active_at`.
///
/// A pure function of the stored due instant and the current time: a read reports the
/// transition the clock has already made due and writes nothing, so two callers reading at
/// the same instant see the same status.
pub fn creation_status(active_at: SystemTime) -> &'static str {
    let now = SystemTime::now();
    if now >= active_at {
        return STATUS_ACTIVE;
    }
    // The progression has two transient states; the second half of the span is
    // `CREATE IN_PROGRESS`.
    match active_at.checked_sub(CREATE_SPAN / 2) {
        Some(in_progress_at) if now >= in_progress_at => STATUS_CREATE_IN_PROGRESS,
        _ => STATUS_CREATE_PENDING,
    }
}

/// A `SystemTime` as the modelled timestamp type.
pub fn timestamp_of(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

// ---------------------------------------------------------------------------
// Output shapes, rebuilt from the records
// ---------------------------------------------------------------------------

/// The `DatasetSchema` `DescribeSchema` returns for a stored record.
pub fn dataset_schema_of(record: &SchemaRecord) -> DatasetSchema {
    DatasetSchema {
        name: Some(record.name.clone()),
        schema_arn: Some(record.schema_arn.clone()),
        schema: Some(record.schema.clone()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        domain: record.domain.clone(),
    }
}

/// The `DatasetSchemaSummary` `ListSchemas` returns for a stored record.
///
/// A summary is the describe answer without the schema document itself: "Provides a
/// summary of the properties of a dataset schema. For a complete listing, call the
/// DescribeSchema API" (`smithy_query(query="doc", shape="DatasetSchemaSummary")`), so a
/// listing never carries the Avro document a describe serves.
pub fn schema_summary_of(record: &SchemaRecord) -> DatasetSchemaSummary {
    DatasetSchemaSummary {
        name: Some(record.name.clone()),
        schema_arn: Some(record.schema_arn.clone()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        domain: record.domain.clone(),
    }
}

/// The `DatasetGroup` `DescribeDatasetGroup` returns for a stored record.
pub fn dataset_group_of(record: &DatasetGroupRecord) -> DatasetGroup {
    DatasetGroup {
        name: Some(record.name.clone()),
        dataset_group_arn: Some(record.dataset_group_arn.clone()),
        status: Some(creation_status(record.active_at).to_string()),
        role_arn: record.role_arn.clone(),
        kms_key_arn: record.kms_key_arn.clone(),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        // Only a failed create has a reason, and nothing in this emulator fails a
        // create asynchronously, so the member stays absent rather than invented.
        failure_reason: None,
        domain: record.domain.clone(),
    }
}

/// The `DatasetGroupSummary` `ListDatasetGroups` returns for a stored record.
pub fn dataset_group_summary_of(record: &DatasetGroupRecord) -> DatasetGroupSummary {
    DatasetGroupSummary {
        name: Some(record.name.clone()),
        dataset_group_arn: Some(record.dataset_group_arn.clone()),
        status: Some(creation_status(record.active_at).to_string()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        failure_reason: None,
        domain: record.domain.clone(),
    }
}

/// The `Dataset` `DescribeDataset` returns for a stored record.
pub fn dataset_of(record: &DatasetRecord) -> Dataset {
    Dataset {
        name: Some(record.name.clone()),
        dataset_arn: Some(record.dataset_arn.clone()),
        dataset_group_arn: Some(record.dataset_group_arn.clone()),
        dataset_type: Some(record.dataset_type.clone()),
        schema_arn: Some(record.schema_arn.clone()),
        status: Some(creation_status(record.active_at).to_string()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        // `UpdateDataset` is not implemented, so no dataset has been updated and there
        // is no latest update to describe.
        latest_dataset_update: None,
        // Only an Action interactions dataset has a tracking id, minted with it.
        tracking_id: record.tracking_id.clone(),
    }
}

/// The `DatasetSummary` `ListDatasets` returns for a stored record.
pub fn dataset_summary_of(record: &DatasetRecord) -> DatasetSummary {
    DatasetSummary {
        name: Some(record.name.clone()),
        dataset_arn: Some(record.dataset_arn.clone()),
        dataset_type: Some(record.dataset_type.clone()),
        status: Some(creation_status(record.active_at).to_string()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The page size a `List*` serves when the caller names no `maxResults`.
///
/// The model bounds `maxResults` at 1..100 and the returned list at 0..100, so 100 is the
/// largest page the model allows and therefore the default. Values outside the range are
/// refused by the boundary validator before a handler runs.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The page size this request asks for.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(DEFAULT_MAX_RESULTS).max(1) as usize
}

/// The separator between a token's kind tag and its payload. A unit separator cannot
/// appear in an ARN, so splitting on it recovers exactly what was encoded.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque `nextToken` carrying the key a page of `kind` ended on.
///
/// Hex keeps the token inside the model's `^\p{ASCII}{0,1500}$` pattern and makes it
/// opaque, and the `kind` tag is what lets [`decode_next_token`] refuse a token this
/// operation did not issue.
pub fn encode_next_token(kind: &str, key: &str) -> String {
    hex::encode(format!("{kind}{TOKEN_SEPARATOR}{key}"))
}

/// The key a `nextToken` of `kind` carries, or `None` when the token is not one this
/// operation issued — the caller answers that with `InvalidNextTokenException`.
pub fn decode_next_token(kind: &str, token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (tag, key) = text.split_once(TOKEN_SEPARATOR)?;
    (tag == kind).then(|| key.to_string())
}

/// Cut `items` (already filtered and in key order) down to one page.
///
/// Returns the page and, only when items remain beyond it, the `nextToken` that resumes
/// after the last item emitted — so the final page carries no token and a client's loop
/// terminates.
pub fn take_page<T>(
    mut items: Vec<T>,
    page_size: usize,
    token_of: impl Fn(&T) -> String,
) -> (Vec<T>, Option<String>) {
    if items.len() <= page_size {
        return (items, None);
    }
    items.truncate(page_size);
    let token = items.last().map(token_of);
    (items, token)
}

// ---------------------------------------------------------------------------
// Shared value checks
// ---------------------------------------------------------------------------

/// The dataset types Personalize accepts, in the spelling it reports them back in.
///
/// Source: the model's own documentation for `CreateDatasetRequest$datasetType` — "One of
/// the following (case insensitive) values: Interactions, Items, Users, Actions,
/// Action_Interactions". `DatasetType` is a plain string shape (pattern `^[A-Za-z_]+$`),
/// not an enum, so the set is not in the generated value lists.
pub const DATASET_TYPE_VALUES: &[&str] = &[
    "Interactions",
    "Items",
    "Users",
    "Actions",
    "Action_Interactions",
];

/// The canonical spelling of `value`, matched case-insensitively, or `None` when it names
/// no dataset type at all.
pub fn canonical_dataset_type(value: &str) -> Option<&'static str> {
    DATASET_TYPE_VALUES
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(value))
        .copied()
}

/// The fields a dataset schema of each dataset type must declare, in the order a refusal
/// lists the ones a document left out.
///
/// A Personalize schema "is associated with a dataset type and has a set of required field
/// and keywords" (`smithy_query(query="doc", shape="CreateSchema")`), and which dataset
/// type a document is written for is the Avro record's own `name`. The model does not
/// enumerate the sets, so these are Amazon Personalize's own per-dataset-type required
/// fields, spelled as the service words them when it reports one missing: an interactions
/// record is keyed by user, item and time; an items or users record by the entity it
/// describes; an actions record by the action; an action-interactions record by user,
/// action, time and the event that happened.
pub const REQUIRED_SCHEMA_FIELDS: &[(&str, &[&str])] = &[
    ("Interactions", &["user_id", "item_id", "timestamp"]),
    ("Items", &["item_id"]),
    ("Users", &["user_id"]),
    ("Actions", &["action_id"]),
    (
        "Action_Interactions",
        &["user_id", "action_id", "timestamp", "event_type"],
    ),
];

/// The fields a schema written for `dataset_type` must declare, or `None` when the value
/// names no dataset type at all — a document whose record names no dataset type has no
/// per-type field set to be checked against, and the type a dataset uses it as is settled
/// by `CreateDataset`.
///
/// The type is matched exactly as `CreateDataset` matches it
/// ([`canonical_dataset_type`]), so one spelling of a type cannot be required fields the
/// other is not.
pub fn required_schema_fields(dataset_type: &str) -> Option<&'static [&'static str]> {
    let canonical = canonical_dataset_type(dataset_type)?;
    REQUIRED_SCHEMA_FIELDS
        .iter()
        .find(|(dataset_type, _)| *dataset_type == canonical)
        .map(|(_, fields)| *fields)
}

/// The one dataset type Amazon Personalize provides a tracking id with: an Action
/// interactions dataset, whose `trackingId` a caller "specif[ies] in the
/// `PutActionInteractions` API operation"
/// (`smithy_query(query="doc", shape="Dataset", member="trackingId")`). Every other type
/// is addressed by its ARN alone.
pub const DATASET_TYPE_WITH_TRACKING_ID: &str = "Action_Interactions";

/// The tracking id a new Action interactions dataset is given.
///
/// Amazon Personalize hands out a fresh identifier per tracker and documents no format
/// for it beyond `TrackingId`'s length (0..256), so it is a version-4 UUID from the
/// maintained `uuid` crate — the form the service's own ids take. Minted ONCE, at the
/// create, and stored on the record: the value is per-run, never derived from the
/// request, and stable for every later read of that dataset.
pub fn mint_tracking_id() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}
