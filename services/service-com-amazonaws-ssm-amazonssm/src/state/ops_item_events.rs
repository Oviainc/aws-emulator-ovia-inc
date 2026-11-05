// SPDX-License-Identifier: CC0-1.0

//! The event trail OpsCenter keeps for an OpsItem, and how a write to an OpsItem lands in
//! it.
//!
//! `ListOpsItemEvents` "returns a list of all OpsItem events in the current Amazon Web
//! Services Region and Amazon Web Services account", so the trail is a record of the
//! WRITES this service made to its OpsItems: every call that creates or edits an OpsItem
//! appends one event here ([`ComAmazonawsSsmAmazonssmState::record_ops_item_event`]), and
//! the listing projects what those writes persisted ([`OpsItemEventRecord::to_summary`]).
//! Nothing is invented at read time and a read appends nothing, so an account that has
//! written no OpsItem has no events to list.
//!
//! An event is identified by a server-minted event id of the hyphenated UUID form
//! ([`ComAmazonawsSsmAmazonssmState::mint_ops_item_event_id`]) and belongs to the Region
//! the OpsItem it is about lives in, so the store is keyed by `(region, event id)` the way
//! the OpsItems themselves are.

use std::time::SystemTime;

use super::ComAmazonawsSsmAmazonssmState;
use crate::ops::types::{OpsItemEventSummary, OpsItemIdentity};

/// The `Source` an event of this service carries: Systems Manager is the source of the
/// events it records about its own OpsItems, which is the name it publishes OpsItem change
/// events to Amazon EventBridge under.
pub const OPS_ITEM_EVENT_SOURCE: &str = "aws.ssm";

/// The `DetailType` of the event a create appends: the detail type Systems Manager names
/// an OpsItem creation by.
pub const DETAIL_TYPE_CREATE: &str = "OpsItem Create";

/// The `DetailType` of the event an edit appends: the detail type Systems Manager names an
/// OpsItem change by. Held here beside [`DETAIL_TYPE_CREATE`] so the one writer of the
/// trail names both.
pub const DETAIL_TYPE_UPDATE: &str = "OpsItem Update";

/// The bounds the modelled `OpsItemEventMaxResults` carries (range 1..50).
pub const MIN_EVENT_PAGE_SIZE: i32 = 1;

/// See [`MIN_EVENT_PAGE_SIZE`]. It is also the page size a request that names no
/// `MaxResults` gets, the way every other read of this service serves one.
pub const MAX_EVENT_PAGE_SIZE: i32 = 50;

/// How many events of ONE OpsItem this store retains, oldest dropped first.
///
/// Emulator resource guard, NOT a published AWS retention: an OpsItem can be edited
/// without end and each edit appends an event, so without a bound the trail of one
/// OpsItem would grow without end. The OpsItems themselves are already bounded
/// ([`super::ops_items::MAX_OPS_ITEMS`]), so bounding the trail per OpsItem bounds the
/// whole store.
pub const MAX_EVENTS_PER_OPS_ITEM: usize = 100;

/// The page size an event listing serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already refuses,
/// so this is a backstop rather than the first line of defence.
pub fn event_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(max_results, MIN_EVENT_PAGE_SIZE, MAX_EVENT_PAGE_SIZE)
}

/// Whether `id` is shaped like an event id of this trail.
///
/// An event id is the hyphenated UUID form this store mints
/// ([`ComAmazonawsSsmAmazonssmState::mint_ops_item_event_id`]), so the shape is read by the
/// one reader of that form this crate has ([`super::is_activation_id`], which is that
/// shape test and nothing more).
pub fn is_event_id(id: &str) -> bool {
    super::is_activation_id(id)
}

/// How many OpsItems ONE event listing may be narrowed to.
///
/// Nothing in the model bounds `OpsItemEventFilterValues` — the length it carries is each
/// value's, not the list's — nor the `Filters` list itself, so this bound is the emulator's
/// own, and the refusal past it is the one OpsCenter words ([`FILTER_VALUES_EXCEEDED`]). It
/// is also what keeps the work of a listing finite: every filter value is a criterion held
/// against every event of the Region, so the work grows with the product of the two.
pub const MAX_FILTER_VALUES: usize = 100;

/// What a listing whose filters name more OpsItem ids than it accepts is refused with: the
/// sentence OpsCenter's own validation words for the class, carrying no number of its own.
/// The status and code the envelope repeats are the modelled ones of
/// `OpsItemInvalidParameterException` (a client error, HTTP 400).
pub const FILTER_VALUES_EXCEEDED: &str = "Maximum number of OpsItemEvent filter values exceeded";

/// See [`FILTER_VALUES_EXCEEDED`]: the error code that refusal is reported under, as the
/// model names it.
const FILTER_VALUES_EXCEEDED_CODE: &str = "OpsItemInvalidParameterException";

/// See [`FILTER_VALUES_EXCEEDED`]: the HTTP status that refusal is answered with, from the
/// modelled `OpsItemInvalidParameterException`.
const FILTER_VALUES_EXCEEDED_STATUS: u16 = 400;

/// How many OpsItem ids `filters` names: the values of every filter, counting a filter that
/// names none as naming one, because a filter without values is still a criterion the
/// listing holds against every event.
pub fn filtered_ops_items(values_per_filter: impl Iterator<Item = usize>) -> usize {
    values_per_filter.map(|values| values.max(1)).sum()
}

/// Why the `Filters` of an event listing cannot be served, or `None` when they can.
///
/// A listing is narrowed to the OpsItems its filter values name
/// (`OpsItemEventFilter$Values`: "one or more OpsItem IDs"), and to at most
/// [`MAX_FILTER_VALUES`] of them, however the request spread those values over its
/// filters ([`filtered_ops_items`]).
pub fn filters_rejection(values: usize) -> Option<String> {
    (values > MAX_FILTER_VALUES).then(|| {
        super::with_sdk_error_envelope(
            FILTER_VALUES_EXCEEDED,
            FILTER_VALUES_EXCEEDED_STATUS,
            FILTER_VALUES_EXCEEDED_CODE,
        )
    })
}

/// One event of an OpsItem's trail as the store holds it.
///
/// Every member `ListOpsItemEvents` reports is here, so the listing projects this record
/// ([`OpsItemEventRecord::to_summary`]) rather than deriving anything at read time.
#[derive(Debug, Clone)]
pub struct OpsItemEventRecord {
    /// The server-minted event id, unique within the store.
    pub event_id: String,
    /// The Region of the OpsItem this event is about, and so the one whose endpoint
    /// reports it.
    pub region: String,
    /// The OpsItem the event is about.
    pub ops_item_id: String,
    /// "The source of the OpsItem event" ([`OPS_ITEM_EVENT_SOURCE`]).
    pub source: String,
    /// "The type of information provided as a detail" — which write this event records
    /// ([`DETAIL_TYPE_CREATE`], [`DETAIL_TYPE_UPDATE`]).
    pub detail_type: String,
    /// "Specific information about the OpsItem event": the JSON document the writer built
    /// out of what it persisted ([`written_detail`]).
    pub detail: String,
    /// "The Amazon Resource Name (ARN) of the IAM entity that created the OpsItem event".
    pub created_by: String,
    /// "The date and time the OpsItem event was created" — the instant of the write it
    /// records.
    pub created_time: SystemTime,
    /// How many events this store had minted when this one was appended, which totally
    /// orders two events of one instant ([`ComAmazonawsSsmAmazonssmState::ops_item_events_in_region`]).
    pub sequence: u64,
}

impl OpsItemEventRecord {
    /// The modelled `OpsItemEventSummary` for this record — every member projected from
    /// what the write persisted.
    pub fn to_summary(&self) -> OpsItemEventSummary {
        OpsItemEventSummary {
            ops_item_id: Some(self.ops_item_id.clone()),
            event_id: Some(self.event_id.clone()),
            source: Some(self.source.clone()),
            detail_type: Some(self.detail_type.clone()),
            detail: Some(self.detail.clone()),
            created_by: Some(OpsItemIdentity {
                arn: Some(self.created_by.clone()),
            }),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
        }
    }
}

/// The `Detail` of the event an OpsItem write appends: a JSON document of what the write
/// left persisted on the OpsItem — the version it is now at and the status it is now in —
/// so the detail is read off the stored record rather than composed of the request's text.
pub fn written_detail(version: u64, status: &str) -> String {
    ::serde_json::json!({ "Version": version.to_string(), "Status": status }).to_string()
}

impl ComAmazonawsSsmAmazonssmState {
    /// Mint the event id of an event being appended.
    ///
    /// The hyphenated UUID form, minted through [`super::minted_uuid`] from this store's
    /// own creation instant and its mint counter, so no two events of one run collide and
    /// the value is not a count a caller can read off the wire.
    pub fn mint_ops_item_event_id(&mut self) -> String {
        self.ops_item_event_sequence += 1;
        let since_epoch = self
            .created_at
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default();
        super::minted_uuid(since_epoch.as_nanos(), self.ops_item_event_sequence)
    }

    /// Append one event to `ops_item_id`'s trail in `region`, returning its event id.
    ///
    /// This is the store's ONE write path for the trail, so the per-OpsItem bound
    /// ([`MAX_EVENTS_PER_OPS_ITEM`]) holds however a caller reached it: past it, the oldest
    /// event of that OpsItem's trail is dropped.
    pub fn record_ops_item_event(
        &mut self,
        region: &str,
        ops_item_id: &str,
        detail_type: &str,
        detail: String,
        created_by: &str,
        at: SystemTime,
    ) -> String {
        let event_id = self.mint_ops_item_event_id();
        let record = OpsItemEventRecord {
            event_id: event_id.clone(),
            region: region.to_string(),
            ops_item_id: ops_item_id.to_string(),
            source: OPS_ITEM_EVENT_SOURCE.to_string(),
            detail_type: detail_type.to_string(),
            detail,
            created_by: created_by.to_string(),
            created_time: at,
            sequence: self.ops_item_event_sequence,
        };
        self.ops_item_events
            .insert((region.to_string(), event_id.clone()), record);
        while self
            .ops_item_events
            .values()
            .filter(|held| held.region == region && held.ops_item_id == ops_item_id)
            .count()
            > MAX_EVENTS_PER_OPS_ITEM
        {
            let Some(oldest) = self
                .ops_item_events
                .values()
                .filter(|held| held.region == region && held.ops_item_id == ops_item_id)
                .min_by_key(|held| (held.created_time, held.sequence))
                .map(|held| (held.region.clone(), held.event_id.clone()))
            else {
                break;
            };
            self.ops_item_events.remove(&oldest);
        }
        event_id
    }

    /// Every event `region` holds, most recent first: an event trail is read newest-first,
    /// and two events of one instant are ordered by the sequence they were appended in,
    /// which is what makes the order total and a resume cursor unambiguous.
    pub fn ops_item_events_in_region(&self, region: &str) -> Vec<&OpsItemEventRecord> {
        let mut events: Vec<&OpsItemEventRecord> = self
            .ops_item_events
            .values()
            .filter(|held| held.region == region)
            .collect();
        events.sort_by(|left, right| {
            right
                .created_time
                .cmp(&left.created_time)
                .then_with(|| right.sequence.cmp(&left.sequence))
        });
        events
    }

    /// Every event of ONE OpsItem's trail in `region`, most recent first (see
    /// [`Self::ops_item_events_in_region`]).
    pub fn ops_item_events_of(&self, region: &str, ops_item_id: &str) -> Vec<&OpsItemEventRecord> {
        let mut events = self.ops_item_events_in_region(region);
        events.retain(|held| held.ops_item_id == ops_item_id);
        events
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    /// Two events appended at the one instant are still totally ordered, newest first, and
    /// an event id is never reused.
    #[test]
    fn events_of_one_instant_are_ordered_by_append_sequence() {
        let mut state = ComAmazonawsSsmAmazonssmState::default();
        let at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let first = state.record_ops_item_event(
            "us-east-1",
            "oi-0123456789ab",
            DETAIL_TYPE_CREATE,
            written_detail(1, "Open"),
            "arn:aws:sts::123456789012:assumed-role/tests/tests",
            at,
        );
        let second = state.record_ops_item_event(
            "us-east-1",
            "oi-0123456789ab",
            DETAIL_TYPE_UPDATE,
            written_detail(2, "InProgress"),
            "arn:aws:sts::123456789012:assumed-role/tests/tests",
            at,
        );
        assert_ne!(first, second, "two appends mint two event ids");
        let ids: Vec<&str> = state
            .ops_item_events_of("us-east-1", "oi-0123456789ab")
            .iter()
            .map(|event| event.event_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![second.as_str(), first.as_str()],
            "the trail reports the most recent event first, got {ids:?}"
        );
    }

    /// The trail of one OpsItem is bounded: past the bound the oldest event is dropped and
    /// another OpsItem's trail is untouched.
    #[test]
    fn one_ops_items_trail_is_bounded() {
        let mut state = ComAmazonawsSsmAmazonssmState::default();
        let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut first_id = String::new();
        for step in 0..=MAX_EVENTS_PER_OPS_ITEM {
            let id = state.record_ops_item_event(
                "us-east-1",
                "oi-0123456789ab",
                DETAIL_TYPE_UPDATE,
                written_detail(step as u64 + 1, "Open"),
                "arn:aws:sts::123456789012:assumed-role/tests/tests",
                base + Duration::from_secs(step as u64),
            );
            if step == 0 {
                first_id = id;
            }
        }
        state.record_ops_item_event(
            "us-east-1",
            "oi-ba9876543210",
            DETAIL_TYPE_CREATE,
            written_detail(1, "Open"),
            "arn:aws:sts::123456789012:assumed-role/tests/tests",
            base,
        );
        let trail = state.ops_item_events_of("us-east-1", "oi-0123456789ab");
        assert_eq!(
            trail.len(),
            MAX_EVENTS_PER_OPS_ITEM,
            "one trail holds at most {MAX_EVENTS_PER_OPS_ITEM} events, got {}",
            trail.len()
        );
        assert!(
            !trail.iter().any(|event| event.event_id == first_id),
            "the oldest event of a full trail is the one dropped"
        );
        assert_eq!(
            state.ops_item_events_of("us-east-1", "oi-ba9876543210").len(),
            1,
            "another OpsItem's trail is untouched by the bound"
        );
    }
}
