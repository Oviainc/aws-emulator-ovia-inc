// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchevents-awsevents service.
//!
//! An EventBridge (CloudWatch Events) account owns event buses; a bus owns
//! rules; a rule owns targets. That containment is the shape of the store
//! below, and it is the ONE copy of a rule that `PutRule`, `DescribeRule`,
//! `ListRules`, `PutTargets`, `ListTargetsByRule` and `DeleteRule` all read and
//! write. `PutEvents` records what it accepted on the bus it was addressed to,
//! together with the rules whose event pattern the event matched.
//!
//! The default bus is the one every account owns: an operation that names no
//! `EventBusName` works on it ("If you omit this, the default event bus is
//! used"), so it is MATERIALISED on the first call that changes it and read as
//! an empty bus until then — a reporting operation never writes it into
//! existence.

pub mod event_pattern;

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{ResourceNotFoundException, Rule as RuleSummary, Tag, Target};

/// The bus every account owns, and the bus an operation works on when the
/// request names none: every `EventBusName` member documents "If you omit this,
/// the default event bus is used."
pub const DEFAULT_EVENT_BUS_NAME: &str = "default";

/// The region an ARN is built for when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page `ListRules` and `ListTargetsByRule` serve, and the page they
/// serve when the request names no `Limit`: both model `Limit` as `LimitMax100`,
/// `@range(min: 1, max: 100)`.
pub const PAGE_LIMIT: i32 = 100;

/// How many targets one rule holds. `TargetList` is modelled
/// `@length(min: 1, max: 100)` — it bounds both what a single `PutTargets`
/// carries and what `ListTargetsByRule` may report, so it is also the bound on
/// the stored list.
pub const MAX_TARGETS_PER_RULE: usize = 100;

/// The `State` of a rule that matches events. `RuleState` models exactly
/// `ENABLED` and `DISABLED`, and "You can disable a rule using DisableRule".
pub const RULE_STATE_ENABLED: &str = "ENABLED";

/// The `State` a rule carries when `PutRule` names none: "Rules are enabled by
/// default, or based on value of the state."
pub const DEFAULT_RULE_STATE: &str = RULE_STATE_ENABLED;

/// How many rules one bus holds before `PutRule` refuses to add another.
///
/// EventBridge publishes a quota of 300 rules per event bus per Region. This
/// emulator models neither Service Quotas nor an account's adjustments to that
/// quota, so this is its OWN bound on the store, set to the published figure.
/// It is also what bounds `PutEvents`'s matching work: an event is matched
/// against at most this many patterns per bus.
pub const MAX_RULES_PER_BUS: usize = 300;

/// How many events one bus's log retains before the oldest is evicted.
///
/// The log is written by `PutEvents` on every call, so without a bound a caller
/// could grow the store without limit. EventBridge itself does not retain
/// delivered events at all (retention is an Archive, whose operations are still
/// stubs), so there is no AWS figure to honour: this is the emulator's own
/// retention window, wide enough for any workflow that asserts on what a call
/// recorded and narrow enough to bound the store. Eviction is oldest-first, so
/// the most recent calls are always the ones observable.
pub const MAX_RECORDED_EVENTS_PER_BUS: usize = 1_000;

/// How many BYTES one bus's log retains before the oldest is evicted.
///
/// The count bound alone does not bound the memory: one entry may carry a
/// 256 KB `Detail`, so a thousand of them would be hundreds of megabytes. The
/// log is therefore bounded by both, and whichever bound binds first evicts.
/// Like the count, this is the emulator's own window — AWS retains delivered
/// events only in an Archive — and it is wide enough to hold every event any
/// workflow asserts on.
pub const MAX_RECORDED_BYTES_PER_BUS: usize = 4 * 1024 * 1024;

/// How many event buses the account holds.
///
/// EventBridge publishes a quota of 100 event buses per account per Region.
/// Each bus retains its own bounded log, so this is the OUTER bound that makes
/// the whole store finite: at most this many buses, each holding at most
/// [`MAX_RECORDED_EVENTS_PER_BUS`] events and [`MAX_RECORDED_BYTES_PER_BUS`]
/// bytes of them, and at most [`MAX_RULES_PER_BUS`] rules. Only `default` can
/// be materialised while `CreateEventBus` is a stub, so the bound is not
/// reachable yet — it is here so the bus map is bounded at the level the
/// per-bus bounds assume.
pub const MAX_EVENT_BUSES: usize = 100;

/// The largest `PutEvents` entry this service accepts.
///
/// EventBridge documents the limit and how to compute an entry's size
/// ("Calculating PutEvents event entry size"): 256 KB, counting 14 bytes for
/// `Time` when it is present plus the UTF-8 length of `Source`, `DetailType`,
/// every `Resources` ARN and `Detail`. `Detail` carries no modelled length
/// bound, so this is what stops one request storing an arbitrarily large
/// document.
pub const MAX_EVENT_ENTRY_BYTES: usize = 256 * 1024;

/// The units `rate(<value> <unit>)` accepts, from `ScheduleExpression`'s
/// documentation ("rate(5 minutes)") and the EventBridge rate-expression
/// reference: minute(s), hour(s) and day(s).
const RATE_UNITS: &[&str] = &["minute", "minutes", "hour", "hours", "day", "days"];

/// How many whitespace-separated fields a `cron(...)` schedule expression holds:
/// EventBridge cron expressions have six (minutes, hours, day-of-month, month,
/// day-of-week, year), as in `ScheduleExpression`'s own example
/// "cron(0 20 * * ? *)".
const CRON_FIELDS: usize = 6;

/// One event `PutEvents` accepted, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct RecordedEvent {
    /// The id `PutEvents` answered with for this entry.
    pub event_id: String,
    pub source: Option<String>,
    pub detail_type: Option<String>,
    /// The `Detail` JSON document, verbatim, when the entry carried one.
    pub detail: Option<String>,
    pub resources: Vec<String>,
    /// The entry's `Time`, or the instant `PutEvents` ran when it carried none:
    /// "If no time stamp is provided, the time stamp of the PutEvents call is
    /// used."
    pub time: SystemTime,
    /// The region the `PutEvents` call was signed for — what the event
    /// document's `region` field reports.
    pub region: String,
    /// The bus the entry was addressed to.
    pub bus_name: String,
    /// The names of the bus's enabled rules whose event pattern this event
    /// matched, in rule-name order.
    pub matched_rules: Vec<String>,
}

impl RecordedEvent {
    /// How many bytes of content this record retains — what
    /// [`MAX_RECORDED_BYTES_PER_BUS`] bounds.
    pub fn retained_bytes(&self) -> usize {
        self.event_id.len()
            + self.source.as_ref().map_or(0, String::len)
            + self.detail_type.as_ref().map_or(0, String::len)
            + self.detail.as_ref().map_or(0, String::len)
            + self.resources.iter().map(String::len).sum::<usize>()
            + self.bus_name.len()
            + self.region.len()
            + self.matched_rules.iter().map(String::len).sum::<usize>()
    }
}

impl RecordedEvent {
    /// The event document an event pattern is matched against.
    ///
    /// EventBridge matches a pattern against the whole event envelope, not
    /// against the `PutEvents` entry: the envelope carries `version`, `id`,
    /// `source`, `detail-type`, `account`, `time`, `region`, `resources` and
    /// `detail`. A field the entry did not carry is OMITTED rather than written
    /// as null, which is what makes `{"exists": false}` answer correctly for it.
    pub fn envelope(&self) -> ::serde_json::Value {
        let mut event = ::serde_json::Map::new();
        event.insert("version".to_string(), "0".into());
        event.insert("id".to_string(), self.event_id.clone().into());
        if let Some(source) = &self.source {
            event.insert("source".to_string(), source.clone().into());
        }
        if let Some(detail_type) = &self.detail_type {
            event.insert("detail-type".to_string(), detail_type.clone().into());
        }
        event.insert(
            "account".to_string(),
            ::engine::emulator_account_id().into(),
        );
        event.insert("time".to_string(), rfc3339(self.time).into());
        event.insert("region".to_string(), self.region.clone().into());
        event.insert(
            "resources".to_string(),
            ::serde_json::Value::from(self.resources.clone()),
        );
        // `PutEvents` accepts an entry only when its `Detail` parses as a JSON
        // object, so the parse here cannot fail for a stored event; an entry
        // that carried no detail reports the empty object.
        let detail = self
            .detail
            .as_deref()
            .and_then(|raw| ::serde_json::from_str::<::serde_json::Value>(raw).ok())
            .filter(::serde_json::Value::is_object)
            .unwrap_or_else(|| ::serde_json::Value::Object(::serde_json::Map::new()));
        event.insert("detail".to_string(), detail);
        ::serde_json::Value::Object(event)
    }
}

/// An event pattern as the store holds it: the document the caller submitted,
/// and the same document parsed ONCE.
///
/// Matching happens on every `PutEvents` entry against every rule of the bus,
/// so re-parsing the source there would multiply two caller-chosen factors.
/// `PutRule` already parses the pattern to validate it; this keeps that result.
#[derive(Debug, Clone)]
pub struct StoredPattern {
    /// What the caller submitted, which is what every report answers with.
    pub source: String,
    /// The parsed document [`event_pattern::matches_within`] reads. Shared, so
    /// a caller that needs to match OUTSIDE the state lock can snapshot the
    /// bus's patterns without copying them.
    pub parsed: ::std::sync::Arc<::serde_json::Value>,
}

/// One rule, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredRule {
    pub name: String,
    pub arn: String,
    /// The rule's event pattern, with the parse `PutRule` already performed.
    pub event_pattern: Option<StoredPattern>,
    pub schedule_expression: Option<String>,
    /// One of `RuleState`'s values (`ENABLED` | `DISABLED`).
    pub state: String,
    pub description: Option<String>,
    pub role_arn: Option<String>,
    /// The principal of the Amazon Web Services service that created the rule
    /// on the account's behalf. This emulator creates no managed rules, so it
    /// stays unset — which is also why `ManagedRuleException` is unreachable.
    pub managed_by: Option<String>,
    pub event_bus_name: String,
    /// The account that created the rule.
    pub created_by: Option<String>,
    pub tags: Vec<Tag>,
    /// The rule's targets, in the order `PutTargets` first added them and
    /// unique by `Target$Id`.
    pub targets: Vec<Target>,
}

impl StoredRule {
    /// Whether an event reaching this rule's bus can match it: "You can disable
    /// a rule using DisableRule", and a disabled rule matches nothing.
    pub fn is_enabled(&self) -> bool {
        self.state == RULE_STATE_ENABLED
    }

    /// The `EventPattern` member every report answers with: the document the
    /// caller submitted.
    pub fn event_pattern_source(&self) -> Option<String> {
        self.event_pattern
            .as_ref()
            .map(|pattern| pattern.source.clone())
    }

    /// The `Rule` projection `ListRules` returns.
    pub fn to_summary(&self) -> RuleSummary {
        RuleSummary {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            event_pattern: self.event_pattern_source(),
            state: Some(self.state.clone()),
            description: self.description.clone(),
            schedule_expression: self.schedule_expression.clone(),
            role_arn: self.role_arn.clone(),
            managed_by: self.managed_by.clone(),
            event_bus_name: Some(self.event_bus_name.clone()),
        }
    }
}

/// One event bus: the rules associated with it and the events it accepted.
#[derive(Debug, Clone)]
pub struct EventBus {
    pub name: String,
    pub arn: String,
    /// The resource policy `PutPermission` would set. That operation is still a
    /// stub, so nothing sets it yet.
    pub policy: Option<String>,
    /// The bus's rules, keyed by name. A `BTreeMap` is what gives `ListRules`
    /// the stable total order its pagination token indexes into.
    pub rules: BTreeMap<String, StoredRule>,
    /// The events `PutEvents` accepted on this bus, in arrival order, bounded
    /// by [`MAX_RECORDED_EVENTS_PER_BUS`] and [`MAX_RECORDED_BYTES_PER_BUS`]. A
    /// deque is what makes evicting the oldest cheap.
    pub events: VecDeque<RecordedEvent>,
    /// What the retained events add up to, so the byte bound costs nothing to
    /// enforce.
    pub retained_bytes: usize,
}

impl EventBus {
    /// A bus called `name`, with its ARN built for `region`.
    pub fn new(name: &str, region: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: event_bus_arn(name, region),
            policy: None,
            rules: BTreeMap::new(),
            events: VecDeque::new(),
            retained_bytes: 0,
        }
    }

    /// Record an accepted event, evicting the oldest until the log is inside
    /// BOTH its retention windows ([`MAX_RECORDED_EVENTS_PER_BUS`] events and
    /// [`MAX_RECORDED_BYTES_PER_BUS`] bytes).
    ///
    /// One entry is bounded at [`MAX_EVENT_ENTRY_BYTES`], well under the byte
    /// window, so the newest event always fits once the log is empty — which is
    /// what makes the loop terminate.
    pub fn record(&mut self, event: RecordedEvent) {
        let size = event.retained_bytes();
        while !self.events.is_empty()
            && (self.events.len() >= MAX_RECORDED_EVENTS_PER_BUS
                || self.retained_bytes.saturating_add(size) > MAX_RECORDED_BYTES_PER_BUS)
        {
            if let Some(evicted) = self.events.pop_front() {
                self.retained_bytes = self
                    .retained_bytes
                    .saturating_sub(evicted.retained_bytes());
            }
        }
        self.retained_bytes = self.retained_bytes.saturating_add(size);
        self.events.push_back(event);
    }
}

/// Per-service state for com-amazonaws-cloudwatchevents-awsevents. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatcheventsAwseventsState {
    /// Every event bus, keyed by name. The default bus is absent until the
    /// first call that changes it materialises it; [`Self::bus_exists`] still
    /// reports it as present, because every account owns it.
    pub buses: BTreeMap<String, EventBus>,
    /// The counter `EventId`s are minted from.
    pub event_id_seq: u64,
}

impl ComAmazonawsCloudwatcheventsAwseventsState {
    /// Mint the `EventId` `PutEvents` answers for one accepted entry.
    ///
    /// `PutEventsResultEntry$EventId` is a UUID, so the id carries that layout:
    /// 32 hex digits in 8-4-4-4-12 groups, with the version nibble `4` and the
    /// RFC 4122 variant nibble. The 16 bytes come from SHA-256 over this
    /// store's own counter, which is what makes the id unique — the counter
    /// never repeats a value the store has already used — and keeps any
    /// request content out of it.
    pub fn mint_event_id(&mut self) -> String {
        use ::sha2::Digest;

        self.event_id_seq = self.event_id_seq.wrapping_add(1);
        let digest = ::sha2::Sha256::digest(format!("event:{}", self.event_id_seq));
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = ::hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The bus `name` for a caller that is about to CHANGE it, materialising the
    /// default bus if this is the first such call.
    ///
    /// `None` means the account holds no bus of that name — the caller answers
    /// with [`bus_not_found`]. Only a mutating operation may use this, and only
    /// on the path that really writes: a call that is going to be REFUSED
    /// resolves existence through [`Self::bus`] / [`Self::bus_exists`] /
    /// [`Self::rule`] first, so it materialises nothing.
    pub fn bus_for_write(&mut self, name: &str, region: &str) -> Option<&mut EventBus> {
        if name == DEFAULT_EVENT_BUS_NAME {
            // The bus map is bounded like everything else the store holds: past
            // [`MAX_EVENT_BUSES`] no further bus is materialised. Only `default`
            // is ever materialised today, so this cannot bind yet; it is what
            // keeps the map finite once `CreateEventBus` routes through here.
            if !self.buses.contains_key(name) && self.buses.len() >= MAX_EVENT_BUSES {
                return None;
            }
            return Some(
                self.buses
                    .entry(name.to_string())
                    .or_insert_with(|| EventBus::new(name, region)),
            );
        }
        self.buses.get_mut(name)
    }

    /// The bus `name`, or `None` when nothing has been stored on it yet.
    ///
    /// An absent DEFAULT bus is not an error — it is an empty one. Pair this
    /// with [`Self::bus_exists`] to tell "no such bus" from "nothing on it yet".
    pub fn bus(&self, name: &str) -> Option<&EventBus> {
        self.buses.get(name)
    }

    /// Whether the account owns a bus called `name` at all. The default bus
    /// always exists, whether or not anything has been stored on it.
    pub fn bus_exists(&self, name: &str) -> bool {
        name == DEFAULT_EVENT_BUS_NAME || self.buses.contains_key(name)
    }

    /// The enabled rules of bus `name` that carry an event pattern, as the
    /// (name, pattern) pairs an event has to be matched against.
    ///
    /// This is a SNAPSHOT: the patterns are shared, not copied, so a caller can
    /// take it under the state lock, release the lock, and then do the matching
    /// work without holding up every other request to this service.
    pub fn match_candidates(
        &self,
        name: &str,
    ) -> Vec<(String, ::std::sync::Arc<::serde_json::Value>)> {
        let Some(bus) = self.bus(name) else {
            return Vec::new();
        };
        bus.rules
            .values()
            .filter(|rule| rule.is_enabled())
            .filter_map(|rule| {
                rule.event_pattern
                    .as_ref()
                    .map(|pattern| (rule.name.clone(), ::std::sync::Arc::clone(&pattern.parsed)))
            })
            .collect()
    }

    /// The rule `name` on bus `bus`, as a reporting operation reads it.
    pub fn rule(&self, bus: &str, name: &str) -> Option<&StoredRule> {
        self.bus(bus)?.rules.get(name)
    }

    /// The rule `name` on bus `bus`, for a caller that is about to CHANGE it.
    ///
    /// Existence is resolved through the reporting accessor FIRST, so a call
    /// that is going to be refused never materialises the default bus: it comes
    /// into being only when there really is a rule on it to change.
    pub fn rule_for_write(
        &mut self,
        bus: &str,
        name: &str,
        region: &str,
    ) -> Option<&mut StoredRule> {
        self.rule(bus, name)?;
        self.bus_for_write(bus, region)?.rules.get_mut(name)
    }
}

/// The bus name a request names: the member itself, the name segment of an
/// `arn:...:event-bus/<name>` ARN, or the default bus when the member is absent.
///
/// `EventBusNameOrArn` admits either form
/// (`^(arn:aws[\w-]*:events:...:event-bus\/)?[/\.\-_A-Za-z0-9]+$`), so every
/// operation that takes one resolves it the same way.
pub fn resolve_bus_name(named: Option<&str>) -> String {
    let Some(named) = named else {
        return DEFAULT_EVENT_BUS_NAME.to_string();
    };
    match named.split_once(":event-bus/") {
        Some((_, name)) if named.starts_with("arn:") => name.to_string(),
        _ => named.to_string(),
    }
}

/// The ARN of rule `rule` on bus `bus`.
///
/// A rule on the default bus is `rule/<name>`; a rule on any other bus carries
/// that bus in its resource path, `rule/<bus>/<name>`, because a rule name is
/// only unique within its bus.
pub fn rule_arn(bus: &str, rule: &str, region: &str) -> String {
    let resource = if bus == DEFAULT_EVENT_BUS_NAME {
        format!("rule/{rule}")
    } else {
        format!("rule/{bus}/{rule}")
    };
    ::engine::emulator_arn("events", region, &resource)
}

/// The ARN of the event bus `name`.
pub fn event_bus_arn(name: &str, region: &str) -> String {
    ::engine::emulator_arn("events", region, &format!("event-bus/{name}"))
}

/// The `ResourceNotFoundException` payload for a rule name that names no rule.
pub fn rule_not_found(bus: &str, rule: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Rule {rule} does not exist on EventBus {bus}.")),
    }
}

/// The `ResourceNotFoundException` payload for a bus name the account does not
/// own.
pub fn bus_not_found(bus: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Event bus {bus} does not exist.")),
    }
}

/// The size EventBridge computes for a `PutEvents` entry, in bytes.
///
/// From "Calculating PutEvents event entry size": `Time` counts as 14 bytes
/// when present, and `Source`, `DetailType`, each `Resources` ARN and `Detail`
/// count as their UTF-8 length. An entry over [`MAX_EVENT_ENTRY_BYTES`] is
/// refused.
pub fn event_entry_size(
    has_time: bool,
    source: Option<&str>,
    detail_type: Option<&str>,
    resources: &[String],
    detail: Option<&str>,
) -> usize {
    let mut size = if has_time { 14 } else { 0 };
    size += source.map_or(0, str::len);
    size += detail_type.map_or(0, str::len);
    size += resources.iter().map(String::len).sum::<usize>();
    size += detail.map_or(0, str::len);
    size
}

/// Whether `expression` is a schedule expression EventBridge accepts.
///
/// `ScheduleExpression` documents the two forms — "cron(0 20 * * ? *)" or
/// "rate(5 minutes)". A rate expression carries a positive value and one of
/// [`RATE_UNITS`]; a cron expression carries [`CRON_FIELDS`] fields. The field
/// contents themselves are not checked here: an unparseable schedule is refused,
/// a cron field whose syntax is wrong is accepted and never fires, which is the
/// same thing an emulator with no scheduler does with a correct one.
pub fn is_schedule_expression(expression: &str) -> bool {
    if let Some(body) = expression
        .strip_prefix("rate(")
        .and_then(|body| body.strip_suffix(')'))
    {
        let mut fields = body.split_whitespace();
        let (Some(value), Some(unit), None) = (fields.next(), fields.next(), fields.next()) else {
            return false;
        };
        // All-digits only: a sign or whitespace inside the value is not a rate
        // expression EventBridge accepts, and `u64::parse` would take `+5`.
        return !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && value.parse::<u64>().is_ok_and(|value| value >= 1)
            && RATE_UNITS.contains(&unit);
    }
    if let Some(body) = expression
        .strip_prefix("cron(")
        .and_then(|body| body.strip_suffix(')'))
    {
        return body.split_whitespace().count() == CRON_FIELDS;
    }
    false
}

/// `stamp` as a [`SystemTime`], for a `PutEvents` entry that carried a `Time`.
///
/// `SystemTime` is what the store keeps, so the two readers of an event (the
/// envelope's `time` field and any later report) agree on one instant.
/// `DateTime` carries its sub-second part as a FORWARD offset from `secs`, so a
/// pre-epoch stamp moves back by whole seconds and then forward by the
/// remainder, never back by both. A stamp so far from the epoch that the host's
/// clock type cannot hold it answers the epoch rather than panicking on the
/// request path.
pub fn to_system_time(stamp: ::aws_smithy_types::DateTime) -> SystemTime {
    let whole = Duration::from_secs(stamp.secs().unsigned_abs());
    let remainder = Duration::from_nanos(u64::from(stamp.subsec_nanos()));
    let shifted = if stamp.secs() >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(whole)
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(whole)
    };
    shifted
        .and_then(|instant| instant.checked_add(remainder))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// `time` as the RFC3339 stamp the event document's `time` field carries
/// (`EventTime` is "per RFC3339").
///
/// It is an instant at WHOLE-SECOND precision closed by `Z`
/// (`2026-01-01T00:00:00Z`), which is the one form every event AWS delivers carries
/// it in — so a server clock reading, which is finer, has its sub-second remainder
/// dropped before the instant is formatted rather than written into the field.
pub fn rfc3339(time: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from_secs(::aws_smithy_types::DateTime::from(time).secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The region this request was signed for — what every ARN this service mints is
/// built for.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatcheventsAwseventsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatcheventsAwseventsState>)
}
