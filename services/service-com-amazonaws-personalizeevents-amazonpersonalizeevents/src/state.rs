// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-personalizeevents-amazonpersonalizeevents service.
//!
//! Amazon Personalize Events is a pure ingestion API: every one of its five operations
//! answers an EMPTY 200 body, so the only things a call can be judged by are what it
//! PERSISTS here and the status it computes from what is already here. This module is
//! therefore the whole observable surface of the service, and it also holds the few
//! rules the five handlers share — the registry lookups, the `properties` JSON rule and
//! the server-generated event id — so the operation files agree on one implementation
//! of each instead of carrying five copies that can drift apart.
//!
//! ## What is "known"
//!
//! A `trackingId` and a `datasetArn` are both minted in the Amazon Personalize control
//! plane, which is a DIFFERENT service (`personalize`), not this one: `CreateDataset`
//! mints the dataset ARN and the tracking id of an Action interactions dataset, and
//! `CreateEventTracker` mints a tracker's. So this crate keeps an explicit registry
//! ([`ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState::register_event_tracker`]
//! / [`ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState::register_dataset`])
//! that the control plane — or a test standing in for it — seeds, rather than accepting
//! any string. EVERY operation refuses a target that is not in it.
//!
//! A registered dataset also carries its [`DatasetType`], derived from the ARN the
//! control plane minted (`dataset/<dataset group>/<DATASET_TYPE>`), because a dataset's
//! type is what decides which metadata put it serves: `PutItems` serves an Items
//! dataset, `PutUsers` a Users dataset and `PutActions` an Actions dataset.
//!
//! A metadata put asks two things of its `datasetArn` — does it resolve, and does it
//! resolve to a dataset of the type this operation serves? — and each failure is an INPUT
//! fault, `InvalidInputException` (400) and never a not-found: an ARN resolving to no
//! dataset is refused with [`INVALID_DATASET_ARN_MESSAGE`], one resolving to a dataset of
//! another type with [`not_a_valid_dataset_message`].
//!
//! An ingestion put asks the same of its `trackingId`, and a tracking id this service
//! cannot ingest the requested stream through is an INPUT fault too — never a not-found,
//! whatever each operation's modelled error set also allows: `PutEvents` refuses it with
//! [`invalid_tracking_id_message`] and `PutActionInteractions` with
//! [`ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE`]. Like the two dataset refusals, neither
//! echoes the value the caller submitted.
//!
//! ## What this store retains
//!
//! Ingestion never stops, so every place this store can grow is bounded and the bound
//! is enforced where the growth happens:
//!
//! * an event stream and an action-interaction stream retain their most recent
//!   [`MAX_EVENTS_PER_TRACKER`] / [`MAX_ACTION_INTERACTIONS_PER_TRACKER`] records; a
//!   push past that drops the OLDEST record and forgets its event id in step, so a
//!   de-duplication set can never outgrow the records it guards (an id that fell out of
//!   retention is a new event if it is sent again);
//! * a dataset collection holds at most [`MAX_RECORDS_PER_DATASET_COLLECTION`] records,
//!   and a record beyond that is REFUSED rather than evicted — a metadata record is
//!   addressable, so dropping one silently would make a later read wrong;
//! * at most [`MAX_EVENT_TRACKERS`] trackers and [`MAX_DATASETS`] datasets exist. No
//!   modelled operation creates either — only the registry seam does — so a caller
//!   cannot grow those maps at all; the caps bound the seam itself.
//!
//! The counts are this emulator's own capacity, not a documented AWS quota.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::SystemTime;

/// The most event trackers this store holds. Only the registry seam creates one (no
/// modelled operation does), but the cap keeps that seam bounded.
pub const MAX_EVENT_TRACKERS: usize = 256;

/// The most datasets this store holds. Only the registry seam creates one (no modelled
/// operation does), but the cap keeps that seam bounded too.
pub const MAX_DATASETS: usize = 256;

/// The most recent item-interaction events one tracker retains.
pub const MAX_EVENTS_PER_TRACKER: usize = 1_000;

/// The most recent action-interaction events one tracker retains.
pub const MAX_ACTION_INTERACTIONS_PER_TRACKER: usize = 1_000;

/// The most records one dataset collection (items, users or actions) holds.
pub const MAX_RECORDS_PER_DATASET_COLLECTION: usize = 1_000;

/// The message every handler answers with when this service's store cannot be reached.
///
/// Unreachable when the service is mounted on the shared `InMemoryEngine` the router
/// builds: [`service_state`] only answers `None` for some other engine type.
pub const STORE_UNAVAILABLE: &str = "the Personalize Events store is unavailable";

/// The keys Amazon Personalize reserves in an event's `properties` map, documented
/// case-insensitively on both `Event$properties` and `ActionInteraction$properties`
/// ("The following can't be included as a keyword for properties (case insensitive)":
/// `smithy_query(query="doc", shape="Event", member="properties")`).
///
/// Item, user and action metadata (`PutItems` / `PutUsers` / `PutActions`) document NO
/// reserved keys, so the rule is applied to the two event streams only.
pub const RESERVED_PROPERTY_KEYS: [&str; 6] = [
    "userId",
    "sessionId",
    "eventType",
    "timestamp",
    "recommendationId",
    "impression",
];

/// Parse a `properties` member: the modelled shape is a JSON string map
/// (`@mediaType("application/json")`, documented as "a string map of ... data" whose
/// elements are key-value pairs), so a value that is not a JSON object, or — when
/// `reserved_keys_apply` — one using a reserved key, is not a `properties` map at all.
///
/// Returns the parsed object on success, and on failure the message for the caller's
/// `InvalidInputException`: it names the member and what was wrong with it, never the
/// value itself (every `properties` member is `@sensitive`).
pub fn parse_properties(
    member: &str,
    raw: &str,
    reserved_keys_apply: bool,
) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| format!("{member} is not valid JSON: {err}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{member} must be a JSON object of key-value pairs"))?;
    if reserved_keys_apply {
        for key in object.keys() {
            if let Some(reserved) = RESERVED_PROPERTY_KEYS
                .iter()
                .find(|reserved| reserved.eq_ignore_ascii_case(key))
            {
                return Err(format!(
                    "{member} must not use the reserved key {reserved}; \
                     Personalize reserves it case insensitively"
                ));
            }
        }
    }
    Ok(value)
}

/// Mint the `eventId` for an event whose caller omitted one, advancing the store's
/// counter of generated ids (`minted`) as it does.
///
/// `Event$eventId` / `ActionInteraction$eventId`: "If an event ID is not provided,
/// Amazon Personalize generates a unique ID for the event." Personalize documents no
/// format for that id and the member's only constraint is length 1..256, so it is built
/// from the two things that make it unique without a source of randomness:
///
/// * the store's own counter, which is monotonic and never reset, so no value is minted
///   twice for the life of the store — an id that retention dropped is not minted again
///   either, because the counter does not go back;
/// * the wall clock at the call, which separates the ids one run of the emulator mints
///   from another run's.
///
/// The counter lives on the store rather than on a tracker, so an id is unique across
/// every tracker and both streams, not only within the stream that de-duplicates on it.
pub fn generate_event_id(minted: &mut u64) -> String {
    *minted += 1;
    let sequence = *minted;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos:x}-{sequence:x}")
}

/// The `MetricAttribution` carried by a stored event — its one member,
/// `eventAttributionSource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMetricAttribution {
    pub event_attribution_source: String,
}

/// One item-interaction event ingested by `PutEvents`, with the session and user of the
/// call that carried it stamped onto it: the stream is per tracker, while `sessionId`
/// and `userId` are per request, so an event that did not keep them could not be read
/// back as the interaction it was.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// The caller's `eventId`, or the one [`generate_event_id`] minted for it.
    pub event_id: String,
    pub event_type: String,
    /// The client-side instant the caller reported (`Event$sentAt`), kept as sent.
    pub sent_at: ::aws_smithy_types::DateTime,
    /// The server clock when this service ingested the event.
    pub received_at: SystemTime,
    pub session_id: String,
    pub user_id: Option<String>,
    pub item_id: Option<String>,
    pub event_value: Option<f32>,
    /// The parsed `properties` object, so what was stored is JSON and not a string that
    /// merely looked like it.
    pub properties: Option<serde_json::Value>,
    pub recommendation_id: Option<String>,
    pub impression: Vec<String>,
    pub metric_attribution: Option<StoredMetricAttribution>,
}

/// One action-interaction event ingested by `PutActionInteractions`.
#[derive(Debug, Clone)]
pub struct StoredActionInteraction {
    pub event_id: String,
    pub action_id: String,
    pub session_id: String,
    pub user_id: Option<String>,
    pub event_type: String,
    /// `ActionInteraction$timestamp`, the client-side instant of the interaction.
    pub timestamp: ::aws_smithy_types::DateTime,
    pub received_at: SystemTime,
    pub recommendation_id: Option<String>,
    pub impression: Vec<String>,
    pub properties: Option<serde_json::Value>,
}

/// A registered event tracker and everything ingested through it.
#[derive(Debug, Default, Clone)]
pub struct EventTracker {
    pub tracking_id: String,
    /// The dataset group the tracker belongs to, when whoever registered it knows:
    /// this service never mints it (`CreateEventTracker` does).
    pub dataset_group_arn: Option<String>,
    /// The item-interaction stream, most recent [`MAX_EVENTS_PER_TRACKER`] records in
    /// ingestion order. A `VecDeque` so dropping the oldest record is O(1).
    pub events: VecDeque<StoredEvent>,
    /// The action-interaction stream, most recent
    /// [`MAX_ACTION_INTERACTIONS_PER_TRACKER`] records in ingestion order.
    pub action_interactions: VecDeque<StoredActionInteraction>,
    /// Every event id the ITEM-interaction stream has seen, which is what makes
    /// ingestion de-duplicating: "Amazon Personalize uses the event ID to distinguish
    /// unique events. Any subsequent events after the first with the same event ID are
    /// not used" (`smithy_query(query="doc", shape="Event", member="eventId")`).
    pub event_ids: BTreeSet<String>,
    /// The same, for the ACTION-interaction stream. A separate set because the two
    /// streams feed two different datasets (Item interactions and Action interactions),
    /// so an id reused across them identifies two different events, not one.
    pub action_interaction_event_ids: BTreeSet<String>,
    /// Set by whoever registered the tracker while the resource is busy, to make the
    /// modelled `ResourceInUseException` (409) reachable from state.
    pub in_use: bool,
}

impl EventTracker {
    /// Claim `event_id` for this tracker's item-interaction stream: `true` when it is
    /// the first event with that id (so the record is stored), `false` when an event of
    /// that stream already used it (so this one is not stored a second time).
    pub fn claim_event_id(&mut self, event_id: &str) -> bool {
        self.event_ids.insert(event_id.to_string())
    }

    /// The same for the action-interaction stream, which de-duplicates on its own ids.
    pub fn claim_action_interaction_event_id(&mut self, event_id: &str) -> bool {
        self.action_interaction_event_ids.insert(event_id.to_string())
    }

    /// Append an item-interaction event, retaining the most recent
    /// [`MAX_EVENTS_PER_TRACKER`].
    ///
    /// A record that falls out of retention takes its event id with it: [`claim_event_id`]
    /// holds one record per id, so the id of the dropped record is held by nothing else
    /// and the de-duplication set stays the size of the stream it guards.
    ///
    /// [`claim_event_id`]: EventTracker::claim_event_id
    pub fn push_event(&mut self, event: StoredEvent) {
        // One push adds one record, and this is the only way the stream grows, so it is
        // at most AT the cap here: one drop makes room.
        if self.events.len() >= MAX_EVENTS_PER_TRACKER {
            if let Some(dropped) = self.events.pop_front() {
                self.event_ids.remove(&dropped.event_id);
            }
        }
        self.events.push_back(event);
    }

    /// Append an action-interaction event, retaining the most recent
    /// [`MAX_ACTION_INTERACTIONS_PER_TRACKER`]. Same retention rule as
    /// [`EventTracker::push_event`].
    pub fn push_action_interaction(&mut self, interaction: StoredActionInteraction) {
        if self.action_interactions.len() >= MAX_ACTION_INTERACTIONS_PER_TRACKER {
            if let Some(dropped) = self.action_interactions.pop_front() {
                self.action_interaction_event_ids.remove(&dropped.event_id);
            }
        }
        self.action_interactions.push_back(interaction);
    }
}

/// What an ingestion put answers for a `trackingId` whose item-interaction stream this
/// service cannot ingest into — the tracking id resolves to no usable event tracker.
///
/// Amazon Personalize words this refusal against the ACCOUNT the call is authenticated to
/// rather than against the value submitted: the fixed text is `Invalid trackingId in
/// account ` and the single variable part is that account's 12-digit id, which for this
/// emulator is the one account it serves ([`::engine::emulator_account_id`]). Like
/// [`INVALID_DATASET_ARN_MESSAGE`] it names the member that could not be resolved and
/// never the id itself, so the refusal reveals nothing about which ids exist.
pub fn invalid_tracking_id_message() -> String {
    format!(
        "Invalid trackingId in account {}",
        ::engine::emulator_account_id()
    )
}

/// What `PutActionInteractions` answers for a `trackingId` whose action-interaction
/// stream this service cannot ingest into.
///
/// Ingesting action interactions is a capability of what the tracking id names — a
/// tracking id serves them only where an Action interactions dataset stands behind it —
/// so a tracking id this service cannot ingest that stream through is reported as the
/// capability fault it is (`InvalidInputException`, 400) rather than as a missing
/// resource. The whole sentence is fixed: nothing from the request or from state is
/// interpolated into it, so it carries no tracking id and no account.
pub const ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE: &str = "ActionInteractions is not supported.";

/// One stored metadata record — an item, a user or an action — of a dataset.
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub properties: Option<serde_json::Value>,
    /// The server clock when the last `Put*` call for this id was ingested.
    pub updated_at: SystemTime,
}

/// The dataset types Amazon Personalize publishes, in the spelling its control plane
/// reports them back in.
///
/// Source: the documentation of `CreateDatasetRequest$datasetType` in the Amazon
/// Personalize control-plane model — "One of the following (case insensitive) values:
/// Interactions, Items, Users, Actions, Action_Interactions"
/// (`smithy_query(query="doc", shape="CreateDatasetRequest", member="datasetType",
/// model_path="services/service-com-amazonaws-personalize-amazonpersonalize/smithy-model/com-amazonaws-personalize-amazonpersonalize.json")`).
/// `CreateDataset` belongs to the `personalize` service, not this one, which is why the
/// catalog is restated here (the sibling crate keeps the same list in its own
/// `canonical_dataset_type`) rather than making one service crate depend on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetType {
    Interactions,
    Items,
    Users,
    Actions,
    ActionInteractions,
}

impl DatasetType {
    /// Every published dataset type, so a caller (or a test) enumerates the set from one
    /// place instead of restating it.
    pub const ALL: [DatasetType; 5] = [
        DatasetType::Interactions,
        DatasetType::Items,
        DatasetType::Users,
        DatasetType::Actions,
        DatasetType::ActionInteractions,
    ];

    /// The canonical spelling, as `CreateDataset` documents it.
    pub fn as_str(self) -> &'static str {
        match self {
            DatasetType::Interactions => "Interactions",
            DatasetType::Items => "Items",
            DatasetType::Users => "Users",
            DatasetType::Actions => "Actions",
            DatasetType::ActionInteractions => "Action_Interactions",
        }
    }

    /// The type `value` names, matched case-insensitively because the control plane
    /// documents the values that way (and renders them upper-cased inside an ARN), or
    /// `None` when it names no published type at all.
    pub fn parse(value: &str) -> Option<Self> {
        DatasetType::ALL
            .into_iter()
            .find(|candidate| candidate.as_str().eq_ignore_ascii_case(value))
    }

    /// The type a dataset ARN names: see [`dataset_type_segment`].
    pub fn from_arn(dataset_arn: &str) -> Option<Self> {
        Self::parse(dataset_type_segment(dataset_arn))
    }
}

/// The segment of a dataset ARN that names its TYPE: a Personalize dataset ARN's
/// resource is `dataset/<dataset group name>/<DATASET_TYPE>`, so the type is the last
/// `/`-delimited segment — a dataset's identity inside its group IS its type, which is
/// why the control plane puts it there instead of a name.
///
/// The one place this emulator decides a dataset's type from its ARN.
pub fn dataset_type_segment(dataset_arn: &str) -> &str {
    dataset_arn.split('/').next_back().unwrap_or(dataset_arn)
}

/// A registered dataset and the metadata records put into it. `BTreeMap` so a reader
/// walks the records in a deterministic order.
///
/// A dataset carries the three metadata collections AND its [`DatasetType`], derived
/// once from its ARN when it is registered: the type is what decides which metadata put
/// a dataset serves, so a `PutUsers` into an Items dataset is refused rather than
/// stored.
#[derive(Debug, Default, Clone)]
pub struct Dataset {
    pub arn: String,
    /// The type the ARN names, as [`DatasetType::from_arn`] derived it. `None` when that
    /// segment is not one of the published values — such a dataset serves no metadata put
    /// at all (every one of them is refused with `InvalidInputException`).
    pub dataset_type: Option<DatasetType>,
    pub items: BTreeMap<String, Record>,
    pub users: BTreeMap<String, Record>,
    pub actions: BTreeMap<String, Record>,
    /// Set by whoever registered the dataset while it is busy (an import job running,
    /// say), to make the modelled `ResourceInUseException` (409) reachable from state.
    pub in_use: bool,
}

/// Which metadata collection of a dataset a `Put{Items,Users,Actions}` call writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataKind {
    Items,
    Users,
    Actions,
}

impl MetadataKind {
    /// The dataset type whose records this kind IS — the only type of dataset the
    /// operation carrying these records serves (`PutItems` an Items dataset, `PutUsers` a
    /// Users dataset, `PutActions` an Actions dataset).
    pub fn dataset_type(self) -> DatasetType {
        match self {
            MetadataKind::Items => DatasetType::Items,
            MetadataKind::Users => DatasetType::Users,
            MetadataKind::Actions => DatasetType::Actions,
        }
    }
}

/// What a metadata put answers for a `datasetArn` that resolves to no dataset at all.
///
/// The whole sentence is fixed: Amazon Personalize names the member that cannot be
/// resolved and nothing else, so the refusal never echoes the ARN the caller submitted
/// (and never reveals whether some other account holds it).
pub const INVALID_DATASET_ARN_MESSAGE: &str = "Invalid datasetArn";

/// What a metadata put answers when its `datasetArn` resolves to a dataset that is NOT
/// of the type the operation serves.
///
/// The dataset type is named the way a dataset ARN's last segment spells it (upper case),
/// so the only variable parts are the ARN from the request and the type the operation
/// serves.
pub fn not_a_valid_dataset_message(dataset_arn: &str, required: DatasetType) -> String {
    format!(
        "{dataset_arn} is not a valid {} dataset.",
        required.as_str().to_ascii_uppercase()
    )
}

impl Dataset {
    /// The collection `kind` names, for reading.
    pub fn records(&self, kind: MetadataKind) -> &BTreeMap<String, Record> {
        match kind {
            MetadataKind::Items => &self.items,
            MetadataKind::Users => &self.users,
            MetadataKind::Actions => &self.actions,
        }
    }

    /// The collection `kind` names, for writing.
    pub fn records_mut(&mut self, kind: MetadataKind) -> &mut BTreeMap<String, Record> {
        match kind {
            MetadataKind::Items => &mut self.items,
            MetadataKind::Users => &mut self.users,
            MetadataKind::Actions => &mut self.actions,
        }
    }
}

/// Why a `Put{Items,Users,Actions}` batch was not ingested. Each handler renders it as
/// the modelled error of its own operation, which is the same triple for all three.
#[derive(Debug, Clone)]
pub enum MetadataPutError {
    /// The dataset is marked in use — `ResourceInUseException` (409).
    InUse,
    /// A record in the batch is not usable, or would take the collection past
    /// [`MAX_RECORDS_PER_DATASET_COLLECTION`] — `InvalidInputException` (400).
    InvalidInput(String),
}

/// Upsert one batch of metadata records (`id`, raw `properties`) into the `kind`
/// collection of the dataset `dataset_arn`.
///
/// This is the whole behaviour of `PutItems`, `PutUsers` and `PutActions`, which differ
/// only in the collection they write and the member they name: a repeated id REPLACES
/// the record stored for it (the latest call for an id is that record, not a second
/// one), and the batch is validated before any of it is stored, so a batch carrying one
/// unusable record stores none of it.
///
/// A batch that would take the collection past
/// [`MAX_RECORDS_PER_DATASET_COLLECTION`] records is refused whole: a metadata record is
/// addressable by its id, so evicting one silently would make a later read of it wrong.
/// Replacing a record already stored never grows the collection, so it is accepted at
/// the cap.
///
/// The `datasetArn` must resolve, and it must resolve to a dataset of the TYPE these
/// records are ([`MetadataKind::dataset_type`]) — two input faults, both
/// `InvalidInputException` (400) and neither a not-found: an ARN that resolves to no
/// dataset is [`INVALID_DATASET_ARN_MESSAGE`], and one that resolves to a dataset of
/// another type (or of no published type at all) is
/// [`not_a_valid_dataset_message`] — a batch of users is not a batch of items. Neither
/// stores anything.
///
/// The target is resolved FIRST, so a request that names an unusable (or busy) dataset
/// and also carries an unusable record answers for the target rather than for the
/// record: the stronger, more specific fault is the one the caller has to fix.
pub fn put_metadata_records(
    state: &mut ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState,
    dataset_arn: &str,
    kind: MetadataKind,
    member: &str,
    records: &[(&str, Option<&str>)],
) -> Result<(), MetadataPutError> {
    let records_type = kind.dataset_type();

    let dataset = state.dataset_mut(dataset_arn).ok_or_else(|| {
        MetadataPutError::InvalidInput(INVALID_DATASET_ARN_MESSAGE.to_string())
    })?;
    if dataset.in_use {
        return Err(MetadataPutError::InUse);
    }

    // The records' type against the dataset's: the operation decides the records' type
    // and the `datasetArn` the dataset's, and a dataset that is not of the records' type
    // is not a dataset this put can serve.
    if dataset.dataset_type != Some(records_type) {
        return Err(MetadataPutError::InvalidInput(not_a_valid_dataset_message(
            dataset_arn,
            records_type,
        )));
    }

    let mut parsed = Vec::with_capacity(records.len());
    for (id, raw) in records {
        let properties = match raw {
            Some(raw) => Some(
                parse_properties(member, raw, false).map_err(MetadataPutError::InvalidInput)?,
            ),
            None => None,
        };
        parsed.push(((*id).to_string(), properties));
    }

    // The batch is refused whole when it would take the collection past its cap. Only
    // the ids the collection does not already hold grow it, and an id the batch repeats
    // grows it once: counting through a set so a batch that upserts one new id twice is
    // charged once.
    let collection = dataset.records_mut(kind);
    let new_ids: BTreeSet<&str> = parsed
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| !collection.contains_key(*id))
        .collect();
    if collection.len() + new_ids.len() > MAX_RECORDS_PER_DATASET_COLLECTION {
        // Reported as the modelled `InvalidInputException` (400) because nothing better
        // is modelled: this service declares only 400/404/409, and the request IS the
        // thing that cannot be served — but the limit is this emulator's capacity, not
        // a rule of the real service, so the message says so.
        return Err(MetadataPutError::InvalidInput(format!(
            "this emulator stores at most {MAX_RECORDS_PER_DATASET_COLLECTION} records per \
             dataset collection, and this request would take it to {}",
            collection.len() + new_ids.len()
        )));
    }

    let updated_at = SystemTime::now();
    for (id, properties) in parsed {
        collection.insert(
            id.clone(),
            Record {
                id,
                properties,
                updated_at,
            },
        );
    }
    Ok(())
}

/// Per-service state for com-amazonaws-personalizeevents-amazonpersonalizeevents. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState {
    /// Every known event tracker, keyed by its `trackingId`.
    pub event_trackers: BTreeMap<String, EventTracker>,
    /// Every known dataset, keyed by its ARN.
    pub datasets: BTreeMap<String, Dataset>,
    /// How many `eventId`s this store has minted for events whose caller omitted one.
    /// Monotonic and never reset — the uniqueness of a generated id rests on it, so
    /// [`generate_event_id`] is the only thing that advances it.
    pub generated_event_ids: u64,
}

impl ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState {
    /// Register the event tracker `tracking_id`, or return the one already registered
    /// with its streams intact — registering is idempotent, so seeding twice never
    /// discards what was ingested.
    ///
    /// This is the ONLY way a tracker comes into existence: no modelled operation
    /// creates one, because a `trackingId` is minted by `CreateEventTracker` in the
    /// Personalize control plane. Answers `None` when the tracker is not known and the
    /// store already holds [`MAX_EVENT_TRACKERS`] of them.
    pub fn register_event_tracker(
        &mut self,
        tracking_id: &str,
        dataset_group_arn: Option<String>,
    ) -> Option<&mut EventTracker> {
        if !self.event_trackers.contains_key(tracking_id)
            && self.event_trackers.len() >= MAX_EVENT_TRACKERS
        {
            return None;
        }
        let tracker = self
            .event_trackers
            .entry(tracking_id.to_string())
            .or_insert_with(|| EventTracker {
                tracking_id: tracking_id.to_string(),
                ..EventTracker::default()
            });
        if dataset_group_arn.is_some() {
            tracker.dataset_group_arn = dataset_group_arn;
        }
        Some(tracker)
    }

    /// The tracker `tracking_id`, or `None` when it was never registered.
    pub fn event_tracker_mut(&mut self, tracking_id: &str) -> Option<&mut EventTracker> {
        self.event_trackers.get_mut(tracking_id)
    }

    /// Drop the tracker `tracking_id` and the streams ingested through it: whoever
    /// registered it reports it gone, so this service stops ingesting through it.
    ///
    /// Forgetting a tracking id that was never registered changes nothing, so a repeated
    /// delete is harmless.
    pub fn forget_event_tracker(&mut self, tracking_id: &str) {
        self.event_trackers.remove(tracking_id);
    }

    /// Register the dataset `dataset_arn`, or return the one already registered with
    /// its records intact.
    ///
    /// The dataset's [`DatasetType`] is DERIVED from the ARN here — the one place that
    /// decides it — so every dataset in the registry carries the type its ARN names
    /// without the seam having to be told it twice.
    ///
    /// Answers `None` once the store holds [`MAX_DATASETS`] of them, for the same
    /// reason as [`Self::register_event_tracker`].
    pub fn register_dataset(&mut self, dataset_arn: &str) -> Option<&mut Dataset> {
        if !self.datasets.contains_key(dataset_arn) && self.datasets.len() >= MAX_DATASETS {
            return None;
        }
        Some(
            self.datasets
                .entry(dataset_arn.to_string())
                .or_insert_with(|| Dataset {
                    arn: dataset_arn.to_string(),
                    dataset_type: DatasetType::from_arn(dataset_arn),
                    ..Dataset::default()
                }),
        )
    }

    /// The dataset `dataset_arn`, or `None` when it was never registered.
    pub fn dataset_mut(&mut self, dataset_arn: &str) -> Option<&mut Dataset> {
        self.datasets.get_mut(dataset_arn)
    }

    /// Drop the dataset `dataset_arn` and everything put into it: whoever registered it
    /// reports it gone, so this service stops ingesting into it and a metadata put naming
    /// it is refused as the unresolvable `datasetArn` it now is
    /// ([`INVALID_DATASET_ARN_MESSAGE`]).
    ///
    /// Forgetting a dataset that was never registered changes nothing, so a repeated
    /// delete is harmless.
    pub fn forget_dataset(&mut self, dataset_arn: &str) {
        self.datasets.remove(dataset_arn);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState>)
}
