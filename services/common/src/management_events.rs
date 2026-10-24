// SPDX-License-Identifier: CC0-1.0

//! The account's MANAGEMENT-EVENT log: the audit record of the API calls this emulator
//! served, which CloudTrail's `LookupEvents` reads.
//!
//! CloudTrail does not record its OWN calls only — it records the management (control
//! plane) API calls of every service in the account, and `LookupEvents` serves them
//! back. So the log cannot live in one service's state struct: the service that SERVES
//! a call is the one that knows the call happened, and the service that REPORTS it is
//! CloudTrail. This is that one store, held once in the shared engine, written by the
//! service that served the call ([`ManagementEventLog::record`]) and read by CloudTrail.
//!
//! Nothing here names a wire shape or a Smithy type: a reader projects a
//! [`ManagementEvent`] onto the shapes ITS model declares (CloudTrail's `Event` /
//! `Resource`). What lives here is the record, the JSON form of a CloudTrail event
//! ([`ManagementEvent::to_cloudtrail_event_json`]) and the bounds that keep the log from
//! growing without end.
//!
//! The log is BOUNDED twice over, because every call may add to it:
//!
//! * by TIME — a record older than [`EVENT_RETENTION`] is dropped, which is also the
//!   window CloudTrail serves ("You can look up events that occurred in a Region within
//!   the last 90 days", `LookupEvents`); and
//! * by COUNT — [`MAX_RECORDED_EVENTS`] records at most, oldest dropped first, so the
//!   events a live workflow just made stay resolvable.
//!
//! Both are materialised on the WRITE path ([`ManagementEventLog::record`] prunes what
//! the clock has already made due), never on a read: a lookup must not change what a
//! later lookup sees.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a recorded event stays retrievable.
///
/// From the CloudTrail `LookupEvents` documentation: "You can look up events that
/// occurred in a Region within the last 90 days."
pub const EVENT_RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// How many events the log holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: the log lives in memory for the life of the
/// process and every served call appends to it, so without a ceiling a caller looping on
/// any write would grow it without end. The oldest record is dropped first, so the
/// events a live workflow just made are the ones that survive.
pub const MAX_RECORDED_EVENTS: usize = 20_000;

/// The `eventCategory` of a management (control-plane) event, as CloudTrail stamps it —
/// the category `LookupEvents` serves when the request names no `EventCategory`.
pub const EVENT_CATEGORY_MANAGEMENT: &str = "Management";

/// The `eventCategory` of a CloudTrail Insights event. Insights events are DERIVED by
/// CloudTrail from a trail's unusual call volume rather than recorded per call, so
/// nothing in this emulator writes one; the constant is here because a reader filtering
/// by category needs the value it is filtering for.
pub const EVENT_CATEGORY_INSIGHT: &str = "Insight";

/// The `eventType` of a record made for an AWS API call (as against a service event or
/// a console sign-in).
pub const EVENT_TYPE_AWS_API_CALL: &str = "AwsApiCall";

/// The `eventVersion` stamped on every record this emulator writes: one version for one
/// record format, so a reader parsing `CloudTrailEvent` knows which fields to expect.
pub const EVENT_VERSION: &str = "1.09";

/// `userIdentity.type` for a call signed with an IAM user's long-term access key.
pub const PRINCIPAL_TYPE_IAM_USER: &str = "IAMUser";

/// `userIdentity.type` for a call signed with the temporary credentials of an assumed
/// role.
pub const PRINCIPAL_TYPE_ASSUMED_ROLE: &str = "AssumedRole";

/// `userIdentity.type` for a call that carried no credential this emulator can read an
/// identity from.
pub const PRINCIPAL_TYPE_UNKNOWN: &str = "Unknown";

/// One resource an event refers to: the pair CloudTrail reports in an event's
/// `Resources`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditedResource {
    /// E.g. `AWS::CloudTrail::Trail`.
    pub resource_type: String,
    /// The name (or ARN) of the resource the call acted on.
    pub resource_name: String,
}

impl AuditedResource {
    /// The resource of `resource_type` named `resource_name`.
    pub fn new(resource_type: impl Into<String>, resource_name: impl Into<String>) -> Self {
        Self {
            resource_type: resource_type.into(),
            resource_name: resource_name.into(),
        }
    }
}

/// What the service that SERVED a call knows about it — everything in a record except
/// the identifiers and the instant the log itself stamps.
#[derive(Debug, Clone)]
pub struct ServedApiCall {
    /// The API operation's name, as the model spells it (`CreateTrail`).
    pub event_name: String,
    /// The service the call was made to, in CloudTrail's `<endpoint-prefix>.amazonaws.com`
    /// form.
    pub event_source: String,
    /// The Region the call was served in.
    pub aws_region: String,
    /// Whether the call only READ state. CloudTrail records this as the event's
    /// `readOnly`, and `LookupEvents` can filter on it.
    pub read_only: bool,
    /// The access key id the request was signed with, when it carried one.
    pub access_key_id: Option<String>,
    /// The user (or role) name the caller speaks for, when the emulator can resolve one.
    pub user_name: Option<String>,
    /// `userIdentity.type` — one of the `PRINCIPAL_TYPE_*` constants.
    pub principal_type: String,
    /// The account the call was served for.
    pub account_id: String,
    /// The request's `User-Agent`, when it sent one.
    pub user_agent: Option<String>,
    /// The resources the call acted on.
    pub resources: Vec<AuditedResource>,
    /// The request's parameters, as the event's `requestParameters`.
    pub request_parameters: serde_json::Value,
    /// The response's values, as the event's `responseElements`; `None` for a call that
    /// failed or that returns nothing.
    pub response_elements: Option<serde_json::Value>,
    /// The wire error code the call answered with, for a call that failed.
    pub error_code: Option<String>,
    /// The message that went with [`ServedApiCall::error_code`].
    pub error_message: Option<String>,
}

/// One recorded event: a [`ServedApiCall`] plus the identifiers and the instant the log
/// stamped it with.
#[derive(Debug, Clone)]
pub struct ManagementEvent {
    /// The log's own ordinal for this record. Strictly increasing, so two events
    /// recorded in the same millisecond still have one definite order.
    pub seq: u64,
    /// The event's `eventID` / CloudTrail `EventId`.
    pub event_id: String,
    /// The `requestID` of the call the event records.
    pub request_id: String,
    /// When the call was served.
    pub event_time: SystemTime,
    /// One of [`EVENT_CATEGORY_MANAGEMENT`] / [`EVENT_CATEGORY_INSIGHT`].
    pub event_category: String,
    /// What the serving service reported.
    pub call: ServedApiCall,
}

impl ManagementEvent {
    /// The record's time as whole milliseconds since the epoch — the resolution a
    /// caller-supplied lookup window is compared at.
    pub fn event_time_millis(&self) -> i64 {
        self.event_time
            .duration_since(UNIX_EPOCH)
            .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }

    /// The record as the JSON document CloudTrail puts in an event's `CloudTrailEvent`.
    ///
    /// Only what this emulator actually knows is written: a field it cannot determine
    /// (the caller's source IP, a principal id for an unknown credential) is left out
    /// rather than filled with a made-up value.
    pub fn to_cloudtrail_event_json(&self) -> serde_json::Value {
        let call = &self.call;
        let mut identity = serde_json::Map::new();
        identity.insert(
            "type".to_string(),
            serde_json::Value::String(call.principal_type.clone()),
        );
        identity.insert(
            "accountId".to_string(),
            serde_json::Value::String(call.account_id.clone()),
        );
        if let Some(access_key_id) = &call.access_key_id {
            identity.insert(
                "accessKeyId".to_string(),
                serde_json::Value::String(access_key_id.clone()),
            );
        }
        if let Some(user_name) = &call.user_name {
            identity.insert(
                "userName".to_string(),
                serde_json::Value::String(user_name.clone()),
            );
        }

        let mut event = serde_json::Map::new();
        event.insert(
            "eventVersion".to_string(),
            serde_json::Value::String(EVENT_VERSION.to_string()),
        );
        event.insert("userIdentity".to_string(), identity.into());
        event.insert(
            "eventTime".to_string(),
            serde_json::Value::String(iso8601(self.event_time)),
        );
        event.insert(
            "eventSource".to_string(),
            serde_json::Value::String(call.event_source.clone()),
        );
        event.insert(
            "eventName".to_string(),
            serde_json::Value::String(call.event_name.clone()),
        );
        event.insert(
            "awsRegion".to_string(),
            serde_json::Value::String(call.aws_region.clone()),
        );
        if let Some(user_agent) = &call.user_agent {
            event.insert(
                "userAgent".to_string(),
                serde_json::Value::String(user_agent.clone()),
            );
        }
        if let Some(error_code) = &call.error_code {
            event.insert(
                "errorCode".to_string(),
                serde_json::Value::String(error_code.clone()),
            );
        }
        if let Some(error_message) = &call.error_message {
            event.insert(
                "errorMessage".to_string(),
                serde_json::Value::String(error_message.clone()),
            );
        }
        event.insert(
            "requestParameters".to_string(),
            call.request_parameters.clone(),
        );
        event.insert(
            "responseElements".to_string(),
            call.response_elements
                .clone()
                .unwrap_or(serde_json::Value::Null),
        );
        event.insert(
            "requestID".to_string(),
            serde_json::Value::String(self.request_id.clone()),
        );
        event.insert(
            "eventID".to_string(),
            serde_json::Value::String(self.event_id.clone()),
        );
        event.insert(
            "readOnly".to_string(),
            serde_json::Value::Bool(call.read_only),
        );
        event.insert(
            "eventType".to_string(),
            serde_json::Value::String(EVENT_TYPE_AWS_API_CALL.to_string()),
        );
        event.insert(
            "managementEvent".to_string(),
            serde_json::Value::Bool(self.event_category == EVENT_CATEGORY_MANAGEMENT),
        );
        event.insert(
            "recipientAccountId".to_string(),
            serde_json::Value::String(call.account_id.clone()),
        );
        event.insert(
            "eventCategory".to_string(),
            serde_json::Value::String(self.event_category.clone()),
        );
        event.into()
    }
}

/// The log: every recorded event, oldest first.
///
/// Reached through the shared engine (`engine.state::<ManagementEventLog>()`) so the
/// service that records a call and the service that reports it hold ONE log and not two
/// copies of it.
#[derive(Debug, Default)]
pub struct ManagementEventLog {
    /// Records in the order they were made, which is the order they were served in.
    events: VecDeque<ManagementEvent>,
    /// The ordinal the next record takes.
    next_seq: u64,
}

impl ManagementEventLog {
    /// Record `call`, served at `at`, as a management event; answer the event's id.
    ///
    /// Prunes first: a record the retention window has run out on, and the oldest
    /// records past [`MAX_RECORDED_EVENTS`], are dropped. Both are owed at this instant
    /// whether or not anybody recorded anything, so the write path is where they are
    /// materialised.
    pub fn record(&mut self, call: ServedApiCall, at: SystemTime) -> String {
        self.prune(at);
        self.next_seq = self.next_seq.saturating_add(1);
        let event = ManagementEvent {
            seq: self.next_seq,
            event_id: mint_uuid(),
            request_id: mint_uuid(),
            event_time: at,
            event_category: EVENT_CATEGORY_MANAGEMENT.to_string(),
            call,
        };
        let event_id = event.event_id.clone();
        self.events.push_back(event);
        event_id
    }

    /// Every record, oldest first. The iterator is double-ended, so a reader that
    /// reports the most recent event first walks it in reverse.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &ManagementEvent> {
        self.events.iter()
    }

    /// How many records the log holds.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the log holds no record at all.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Drop what the clock has already made unretrievable: records older than
    /// [`EVENT_RETENTION`], then the oldest records above [`MAX_RECORDED_EVENTS`].
    ///
    /// Called BEFORE a record is appended, which is why the count test is `>=`: it
    /// leaves room for the record about to be pushed, so the log settles at exactly
    /// [`MAX_RECORDED_EVENTS`] rather than one above it.
    fn prune(&mut self, now: SystemTime) {
        while self
            .events
            .front()
            .is_some_and(|event| event.event_time + EVENT_RETENTION <= now)
        {
            self.events.pop_front();
        }
        while self.events.len() >= MAX_RECORDED_EVENTS {
            self.events.pop_front();
        }
    }
}

/// Render `at` the way a CloudTrail event's `eventTime` carries it (ISO 8601, e.g.
/// `2026-10-22T22:04:46Z`).
fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// A fresh identifier in the canonical 8-4-4-4-12 UUID text form, the form CloudTrail's
/// `eventID` and `requestID` carry.
///
/// A version-4 UUID from the maintained `uuid` crate: an event id names one recorded
/// call and nothing else, so it is drawn fresh (never derived from the request or from
/// the log's position), which is also what makes two records' ids distinct.
fn mint_uuid() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str) -> ServedApiCall {
        ServedApiCall {
            event_name: name.to_string(),
            event_source: "cloudtrail.amazonaws.com".to_string(),
            aws_region: "us-east-1".to_string(),
            read_only: false,
            access_key_id: Some("AKIDEMULATOR".to_string()),
            user_name: Some("emulator".to_string()),
            principal_type: PRINCIPAL_TYPE_IAM_USER.to_string(),
            account_id: "000000000000".to_string(),
            user_agent: None,
            resources: vec![AuditedResource::new("AWS::CloudTrail::Trail", "demo")],
            request_parameters: serde_json::json!({"name": "demo"}),
            response_elements: None,
            error_code: None,
            error_message: None,
        }
    }

    /// Every record gets its own ordinal and its own ids, and the log keeps them in the
    /// order they were served.
    #[test]
    fn records_are_ordered_and_uniquely_identified() {
        let mut log = ManagementEventLog::default();
        let now = SystemTime::now();
        let first = log.record(call("CreateTrail"), now);
        let second = log.record(call("DeleteTrail"), now);
        assert_ne!(
            first, second,
            "case two records: expected distinct event ids, got {first:?} twice"
        );
        let seqs: Vec<u64> = log.iter().map(|event| event.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2],
            "case two records: expected increasing ordinals, got {seqs:?}"
        );
        let names: Vec<&str> = log.iter().map(|e| e.call.event_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["CreateTrail", "DeleteTrail"],
            "case two records: expected the served order, got {names:?}"
        );
    }

    /// A minted id has the canonical UUID form: 8-4-4-4-12 lowercase hex, version 4,
    /// variant 10xx.
    #[test]
    fn minted_ids_have_the_uuid_form() {
        let mut log = ManagementEventLog::default();
        let id = log.record(call("CreateTrail"), SystemTime::now());
        let groups: Vec<&str> = id.split('-').collect();
        let widths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case {id:?}: expected the 8-4-4-4-12 grouping, got widths {widths:?}"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()
                    && !c.is_ascii_uppercase())),
            "case {id:?}: expected lowercase hex digits"
        );
        assert_eq!(
            groups[2].chars().next(),
            Some('4'),
            "case {id:?}: expected the version nibble 4"
        );
        assert!(
            groups[3].starts_with(['8', '9', 'a', 'b']),
            "case {id:?}: expected the variant nibble in [89ab]"
        );
    }

    /// A record the retention window has run out on is dropped by the next write.
    #[test]
    fn retention_window_prunes_old_records() {
        let mut log = ManagementEventLog::default();
        let now = SystemTime::now();
        // Served just over the retention window ago, so `now` owes its removal.
        log.record(
            call("CreateTrail"),
            now - EVENT_RETENTION - Duration::from_secs(1),
        );
        // Served inside the window, so it stays.
        log.record(
            call("DeleteTrail"),
            now - EVENT_RETENTION + Duration::from_secs(60),
        );
        assert_eq!(
            log.len(),
            2,
            "case nothing due yet: expected both records, got {:?}",
            log.iter()
                .map(|e| e.call.event_name.clone())
                .collect::<Vec<_>>()
        );
        log.record(call("StartLogging"), now);
        let names: Vec<String> = log.iter().map(|e| e.call.event_name.clone()).collect();
        assert_eq!(
            names,
            vec!["DeleteTrail".to_string(), "StartLogging".to_string()],
            "case within retention: expected both kept, got {names:?}"
        );
    }

    /// The JSON form carries the fields a client reads an event by, and leaves out what
    /// the emulator does not know.
    #[test]
    fn cloudtrail_event_json_reports_what_is_known() {
        let mut log = ManagementEventLog::default();
        let mut failed = call("CreateTrail");
        failed.error_code = Some("TrailAlreadyExistsException".to_string());
        failed.error_message = Some("Trail demo already exists in us-east-1".to_string());
        log.record(failed, SystemTime::now());
        let event = log.iter().next().expect("the recorded event");
        let json = event.to_cloudtrail_event_json();
        for (field, expected) in [
            ("eventVersion", serde_json::json!(EVENT_VERSION)),
            ("eventName", serde_json::json!("CreateTrail")),
            ("eventSource", serde_json::json!("cloudtrail.amazonaws.com")),
            ("awsRegion", serde_json::json!("us-east-1")),
            ("readOnly", serde_json::json!(false)),
            ("eventType", serde_json::json!(EVENT_TYPE_AWS_API_CALL)),
            ("managementEvent", serde_json::json!(true)),
            ("eventCategory", serde_json::json!(EVENT_CATEGORY_MANAGEMENT)),
            (
                "errorCode",
                serde_json::json!("TrailAlreadyExistsException"),
            ),
        ] {
            assert_eq!(
                json.get(field),
                Some(&expected),
                "case {field:?}: expected {expected}, got {json}"
            );
        }
        assert_eq!(
            json.get("userAgent"),
            None,
            "case userAgent: an unknown field must be left out, got {json}"
        );
        assert_eq!(
            json.pointer("/userIdentity/userName"),
            Some(&serde_json::json!("emulator")),
            "case userIdentity: expected the caller's user name, got {json}"
        );
    }
}
