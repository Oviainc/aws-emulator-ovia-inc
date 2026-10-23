// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime service.
//!
//! The service is the OnlineStore data plane of SageMaker Feature Store: every
//! operation reads or writes the one store modelled here. The runtime API carries no
//! feature-group definition (that lives in the SageMaker control plane), so the two
//! names a feature group's definition would supply — its record-identifier feature and
//! its event-time feature — are learned from the first `PutRecord` written to the group
//! and remembered for every later call, as documented on
//! [`FeatureGroupStore::record_identifier_feature_name`] and
//! [`FeatureGroupStore::event_time_feature_name`].
//!
//! The store is held in memory for the life of the process, so its three growth sites
//! are bounded by a maximum count: [`MAX_FEATURE_GROUPS`] groups, each holding at most
//! [`MAX_RECORDS_PER_GROUP`] records, each holding at most [`MAX_FEATURES_PER_RECORD`]
//! feature values. A write past any of the three is refused (the handler answers the
//! modelled `ServiceUnavailable`) rather than growing without limit, and a record
//! carrying a `TtlDuration` is additionally evicted once its expiry is due.

use std::collections::BTreeMap;
use std::collections::HashMap;

use aws_smithy_types::date_time::{DateTime, Format};

/// The most feature groups one emulator keeps an OnlineStore for.
///
/// A capacity bound, not an AWS quota: the store is per-process memory that grows with
/// each distinct caller-supplied `FeatureGroupName`, so it needs a maximum count.
pub const MAX_FEATURE_GROUPS: usize = 1_000;

/// The most records one feature group's OnlineStore keeps.
///
/// Same reason as [`MAX_FEATURE_GROUPS`]: `PutRecord` adds an entry per distinct record
/// identifier, and a caller that passes no `TtlDuration` would otherwise grow the group
/// without limit.
pub const MAX_RECORDS_PER_GROUP: usize = 10_000;

/// The most feature values one record of a feature group's OnlineStore keeps.
///
/// Same reason as [`MAX_RECORDS_PER_GROUP`], for the one dimension `PutRecord` cannot
/// grow: a `PutRecord` replaces a record's feature values wholesale, so its residency is
/// bounded by one request, but `UpdateRecord` is a PARTIAL update — a feature name the
/// stored record does not carry yet is appended to it — so successive calls naming fresh
/// names would otherwise grow one record without limit, and each `ValueAsString` is
/// modelled `len 0..358400`. A merge past this bound is refused rather than taken.
pub const MAX_FEATURES_PER_RECORD: usize = 1_000;

/// The page size `ListRecords` uses when the caller names no `MaxResults`.
///
/// The model constrains `MaxResults` to `range 1..100`, so 100 is the largest page the
/// API lets a caller ask for; using it as the default keeps a response from being linear
/// in the whole store.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// Per-service state for com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState {
    /// OnlineStore per feature group, keyed by [`group_key`] (so an ARN and the bare
    /// name of one group reach one store). At most [`MAX_FEATURE_GROUPS`] entries.
    groups: HashMap<String, FeatureGroupStore>,
}

impl ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState {
    /// The OnlineStore of `name_or_arn`, or `None` when no record was ever ingested
    /// into that feature group. Expired records are dropped first, so a caller never
    /// sees a record whose `ExpiresAt` has passed.
    pub fn group_mut(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroupStore> {
        let group = self.groups.get_mut(group_key(name_or_arn))?;
        group.purge_expired(now_secs());
        Some(group)
    }

    /// The names a feature group has already learned, without creating it.
    pub fn learned_feature_names(&self, name_or_arn: &str) -> (Option<String>, Option<String>) {
        match self.groups.get(group_key(name_or_arn)) {
            Some(group) => (
                group.record_identifier_feature_name.clone(),
                group.event_time_feature_name.clone(),
            ),
            None => (None, None),
        }
    }

    /// The OnlineStore of `name_or_arn`, created empty when this is the first record
    /// ingested into that feature group. The runtime API has no "create feature group"
    /// call, so a group's online store comes into existence with its first `PutRecord`.
    ///
    /// `None` when the group is not known yet and the emulator already holds
    /// [`MAX_FEATURE_GROUPS`] of them: the caller refuses the write rather than growing
    /// the store without limit.
    pub fn group_or_create(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroupStore> {
        let key = group_key(name_or_arn);
        if !self.groups.contains_key(key) && self.groups.len() >= MAX_FEATURE_GROUPS {
            return None;
        }
        let group = self.groups.entry(key.to_owned()).or_default();
        group.purge_expired(now_secs());
        Some(group)
    }
}

/// One feature group's OnlineStore.
#[derive(Debug, Default)]
pub struct FeatureGroupStore {
    /// The feature whose value identifies a record. Set from the FIRST `FeatureValue`
    /// of the first record written to this group and then fixed, because the runtime
    /// API never carries the group's `RecordIdentifierFeatureName`.
    pub record_identifier_feature_name: Option<String>,
    /// The feature carrying the record's event time, learned the first time a written
    /// record carries a feature named `EventTime` (in any case, with `-`/`_`
    /// separators). Absent when no record has carried one, in which case the group's
    /// records have no event time and neither the historic-record rule nor
    /// `TtlDuration` has anything to compute from.
    pub event_time_feature_name: Option<String>,
    /// Records by `RecordIdentifierValueAsString`, at most [`MAX_RECORDS_PER_GROUP`] of
    /// them. A `BTreeMap` so `ListRecords` enumerates in one deterministic ASCENDING
    /// identifier order — which is what lets a page token name the identifier to resume
    /// from rather than a positional index that insertions would shift.
    records: BTreeMap<String, StoredRecord>,
    /// A LOWER BOUND on the earliest `expires_at_secs` any record holds: no record
    /// expires before it. A read path with nothing due therefore skips the expiry scan
    /// entirely, and a bound left behind by a removed record only costs one scan, which
    /// recomputes it.
    next_expiry_secs: Option<f64>,
}

impl FeatureGroupStore {
    /// Hard-delete every record whose `ExpiresAt` is at or before `now_secs`.
    ///
    /// This is the TTL the model documents — "Time to live duration, where the record
    /// is hard deleted after the expiration time is reached" — so the removal is owed
    /// by the clock and is materialised lazily on the next call that reads the group,
    /// rather than by a timer. Nothing is scanned while no expiry is due.
    pub fn purge_expired(&mut self, now_secs: f64) {
        if self.next_expiry_secs.is_none_or(|due| due > now_secs) {
            return;
        }
        self.records
            .retain(|_, record| record.expires_at_secs.is_none_or(|at| at > now_secs));
        self.recompute_next_expiry();
    }

    /// The records of this group, in ascending identifier order.
    pub fn records(&self) -> impl Iterator<Item = &StoredRecord> {
        self.records.values()
    }

    /// The record `identifier` names, whether or not it is soft-deleted.
    pub fn record(&self, identifier: &str) -> Option<&StoredRecord> {
        self.records.get(identifier)
    }

    /// The record `identifier` names, if it is present and not soft-deleted.
    pub fn live_record(&self, identifier: &str) -> Option<&StoredRecord> {
        self.records
            .get(identifier)
            .filter(|record| !record.soft_deleted)
    }

    /// Store `record`, over-writing any record with the same identifier.
    ///
    /// `false` — nothing stored — when the record is new and the group already holds
    /// [`MAX_RECORDS_PER_GROUP`].
    pub fn insert_record(&mut self, record: StoredRecord) -> bool {
        let is_new = !self.records.contains_key(&record.identifier);
        if is_new && self.records.len() >= MAX_RECORDS_PER_GROUP {
            return false;
        }
        if let Some(at) = record.expires_at_secs {
            self.next_expiry_secs = Some(self.next_expiry_secs.map_or(at, |earliest| earliest.min(at)));
        }
        self.records.insert(record.identifier.clone(), record);
        true
    }

    /// Remove the record entirely (`HardDelete`).
    pub fn remove_record(&mut self, identifier: &str) {
        self.records.remove(identifier);
    }

    /// Leave the `SoftDelete` tombstone the model describes: "a record with the same
    /// RecordIdentifer as the original, but with `is_deleted` value set to True,
    /// `EventTime` set to the delete input EventTime, and other feature values set to
    /// null."
    pub fn soft_delete_record(&mut self, identifier: &str, event_time: &str) {
        let identifier_feature = self.record_identifier_feature_name.clone();
        let event_time_feature = self.event_time_feature_name.clone();
        if let Some(record) = self.records.get_mut(identifier) {
            record.soft_deleted = true;
            record.event_time = Some(event_time.to_owned());
            record.expires_at = None;
            record.expires_at_secs = None;
            record.features.retain(|feature| {
                identifier_feature.as_deref() == Some(feature.feature_name.as_str())
                    || event_time_feature.as_deref() == Some(feature.feature_name.as_str())
            });
            for feature in &mut record.features {
                if event_time_feature.as_deref() == Some(feature.feature_name.as_str()) {
                    feature.value_as_string = Some(event_time.to_owned());
                    feature.value_as_string_list = None;
                }
            }
        }
    }

    /// Re-derive the expiry bound from the records that remain.
    fn recompute_next_expiry(&mut self) {
        self.next_expiry_secs = self
            .records
            .values()
            .filter_map(|record| record.expires_at_secs)
            .fold(None, |earliest: Option<f64>, at| {
                Some(earliest.map_or(at, |e| e.min(at)))
            });
    }
}

/// One record of a feature group's OnlineStore.
#[derive(Debug, Clone, Default)]
pub struct StoredRecord {
    /// `RecordIdentifierValueAsString` — the value of the group's record-identifier
    /// feature in this record.
    pub identifier: String,
    /// Every feature value of the record, in the order it was written. A `PutRecord`
    /// is a full over-write, so this is replaced wholesale.
    pub features: Vec<StoredFeatureValue>,
    /// The record's event time, exactly as the event-time feature carried it.
    pub event_time: Option<String>,
    /// `EventTime + TtlDuration`, as an ISO-8601 string; absent when the record was
    /// written with no `TtlDuration` or with no parsable event time.
    pub expires_at: Option<String>,
    /// The same instant as [`Self::expires_at`] in epoch seconds, for the expiry
    /// comparison, so no handler re-parses the string.
    pub expires_at_secs: Option<f64>,
    /// Set by `DeleteRecord` in `SoftDelete` mode: the record keeps its place in the
    /// store but is no longer retrievable by `GetRecord` or `BatchGetRecord`.
    pub soft_deleted: bool,
}

impl StoredRecord {
    /// The record's event time in epoch seconds, or `None` when it has none or it is
    /// in neither of the two formats a Feature Store event time can take.
    pub fn event_time_secs(&self) -> Option<f64> {
        self.event_time.as_deref().and_then(parse_event_time_secs)
    }

    /// This record's features, restricted to `feature_names` when the caller named
    /// any, in the order the record stores them.
    ///
    /// `FeatureNames` is documented as "List of names of Features to be retrieved. If
    /// not specified, the latest value for all the Features are returned", so an
    /// absent list projects everything and a present one projects exactly what it
    /// names. A name the record does not carry contributes nothing and is not an
    /// error: the runtime API carries no feature-group definition, so a feature the
    /// group declares but this record never set is indistinguishable here from a name
    /// the group does not declare at all.
    pub fn project(&self, feature_names: Option<&[String]>) -> Vec<crate::ops::types::FeatureValue> {
        self.features
            .iter()
            .filter(|feature| {
                feature_names.is_none_or(|names| names.contains(&feature.feature_name))
            })
            .map(StoredFeatureValue::to_feature_value)
            .collect()
    }
}

/// One feature value of a stored record.
#[derive(Debug, Clone, Default)]
pub struct StoredFeatureValue {
    pub feature_name: String,
    pub value_as_string: Option<String>,
    pub value_as_string_list: Option<Vec<String>>,
}

impl StoredFeatureValue {
    /// The stored value as the modelled `FeatureValue` the wire carries.
    pub fn to_feature_value(&self) -> crate::ops::types::FeatureValue {
        crate::ops::types::FeatureValue {
            feature_name: self.feature_name.clone(),
            value_as_string: self.value_as_string.clone(),
            value_as_string_list: self.value_as_string_list.clone(),
        }
    }
}

impl From<&crate::ops::types::FeatureValue> for StoredFeatureValue {
    fn from(value: &crate::ops::types::FeatureValue) -> Self {
        Self {
            feature_name: value.feature_name.clone(),
            value_as_string: value.value_as_string.clone(),
            value_as_string_list: value.value_as_string_list.clone(),
        }
    }
}

/// The store key for a `FeatureGroupNameOrArn`.
///
/// The member's pattern is `^(arn:aws…:feature-group/)?(<name>)$`: the ARN prefix is
/// OPTIONAL, so the ARN and the bare name denote the same feature group and must reach
/// the same store. The key is the name, i.e. the part after the last `/`. Two ARNs that
/// differ only in region or account therefore collapse onto one store, which is correct
/// for an emulator serving a single account and region.
pub fn group_key(name_or_arn: &str) -> &str {
    match name_or_arn.rsplit_once('/') {
        Some((_, name)) => name,
        None => name_or_arn,
    }
}

/// Whether `feature_name` is the conventional name of a group's event-time feature.
///
/// The runtime API never states which feature is the event time, so the emulator
/// recognises the name SageMaker's own feature-group examples use, independent of case
/// and of the `-`/`_` separators the `FeatureName` pattern allows.
pub fn is_event_time_feature_name(feature_name: &str) -> bool {
    feature_name
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .eq("eventtime".chars())
}

/// An event time in epoch seconds.
///
/// A Feature Store event-time feature is either a `String` holding an ISO-8601
/// timestamp or a `Fractional`/`Integral` holding seconds since the epoch, so both
/// forms are accepted; anything else is not a timestamp and has no instant to compare.
pub fn parse_event_time_secs(event_time: &str) -> Option<f64> {
    if let Ok(parsed) = DateTime::from_str(event_time, Format::DateTime) {
        return Some(parsed.as_secs_f64());
    }
    event_time
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite())
}

/// `ExpiresAt = EventTime + TtlDuration`, as the ISO-8601 string the `ExpiresAt`
/// member documents, plus the same instant in epoch seconds.
///
/// Returns `None` when the event time is in neither accepted format, or when the unit
/// is not one the model declares.
pub fn expires_at(
    event_time: &str,
    ttl: &crate::ops::types::TtlDuration,
) -> Option<(String, f64)> {
    let seconds = ttl_seconds(&ttl.unit, ttl.value)?;
    let at = parse_event_time_secs(event_time)? + seconds;
    let formatted = DateTime::from_secs_f64(at).fmt(Format::DateTime).ok()?;
    Some((formatted, at))
}

/// `value` of the `TtlDurationUnit` `unit`, in seconds.
///
/// The units are exactly the model's `TtlDurationUnit` values (see
/// `TTL_DURATION_UNIT_VALUES` in `ops::types`): Seconds, Minutes, Hours, Days, Weeks.
fn ttl_seconds(unit: &str, value: i32) -> Option<f64> {
    let unit_secs = match unit {
        "Seconds" => 1.0,
        "Minutes" => 60.0,
        "Hours" => 3600.0,
        "Days" => 86_400.0,
        "Weeks" => 604_800.0,
        _ => return None,
    };
    Some(f64::from(value) * unit_secs)
}

/// The first value of an enum-typed member the model's value list does not declare.
///
/// The wire members of this service that carry an enum (`TargetStores`,
/// `DeletionMode`, `ExpirationTimeResponse`) arrive as plain strings, and a value
/// outside the enum changes what the operation would do — so it is refused with the
/// modelled `ValidationError` rather than read as "not the value I compare against".
pub fn undeclared_value<'a>(values: &'a [String], declared: &[&str]) -> Option<&'a str> {
    values
        .iter()
        .map(String::as_str)
        .find(|value| !declared.contains(value))
}

/// Whether a caller asked for `ExpiresAt` in the response.
///
/// "Parameter to request ExpiresAt in response. If Enabled, GetRecord will return the
/// value of ExpiresAt, if it is not null." A value outside the model's
/// `ExpirationTimeResponse` enum would otherwise be read as "not Enabled" and silently
/// withhold the member, so it is reported as an error for the caller to refuse with its
/// own modelled `ValidationError`. Shared by GetRecord and BatchGetRecord, which both
/// carry the member, so the two cannot drift.
pub fn expiration_time_enabled(value: Option<&str>) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(value) if crate::ops::types::EXPIRATION_TIME_RESPONSE_VALUES.contains(&value) => {
            Ok(value == "Enabled")
        }
        Some(value) => Err(format!(
            "ExpirationTimeResponse: '{value}' is not an ExpirationTimeResponse; expected one \
             of {}",
            crate::ops::types::EXPIRATION_TIME_RESPONSE_VALUES.join(", ")
        )),
    }
}

/// Whether a `TargetStores` list includes the OnlineStore this service serves.
///
/// An absent list means every store the feature group uses — "By default, Feature Store
/// adds the record to all of the stores that you're using for the FeatureGroup."
pub fn targets_online_store(target_stores: Option<&Vec<String>>) -> bool {
    target_stores.is_none_or(|stores| stores.iter().any(|store| store == "OnlineStore"))
}

/// Wall-clock now, in epoch seconds, for the TTL comparison.
pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState>)
}
