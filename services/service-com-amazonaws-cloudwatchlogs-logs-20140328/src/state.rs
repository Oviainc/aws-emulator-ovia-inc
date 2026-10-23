// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchlogs-logs-20140328 service.
//!
//! The log-group / log-stream / log-event core loop is stored here, together with the
//! small helpers every handler of that loop shares: ARN construction, the wall clock,
//! log-group identifier resolution and the opaque pagination tokens
//! `GetLogEvents` / `DescribeLogStreams` / `DescribeLogGroups` hand back.
//!
//! A log group's subscription filters live on the group they are "associated with"
//! ([`LogGroupRecord::subscription_filters`]), so deleting the group removes them too.
//!
//! The account's resource policies live here too: an account-scoped one in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::account_resource_policies`], a
//! resource-scoped one on the log group it is attached to
//! ([`LogGroupRecord::resource_policy`]).

use std::collections::BTreeMap;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Generated emulators default to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The log group class `CreateLogGroup` applies when the request omits
/// `logGroupClass` ("If you omit this parameter, the default of STANDARD is used").
pub const DEFAULT_LOG_GROUP_CLASS: &str = "STANDARD";

/// Per-event storage overhead CloudWatch Logs charges a log group's `storedBytes`.
/// From the PutLogEvents quota: a batch's size "is calculated as the sum of all event
/// messages in UTF-8, plus 26 bytes for each log event".
pub const EVENT_OVERHEAD_BYTES: i64 = 26;

/// The number of items `DescribeLogGroups` / `DescribeLogStreams` return when the
/// request omits `limit` ("If you don't specify a value, the default is up to 50 items").
pub const DEFAULT_DESCRIBE_LIMIT: i32 = 50;

/// The number of events `GetLogEvents` returns when the request omits `limit`: the
/// documented ceiling of "up to 10,000 log events" per page.
pub const DEFAULT_EVENTS_LIMIT: i32 = 10_000;

/// The number of items the name-pattern list operations return when the request omits
/// `limit`: `ListLogGroups` ("If you omit this parameter, the default is up to 50 log
/// groups") and `ListAggregateLogGroupSummaries` ("If you omit this parameter, the
/// default is up to 50 aggregated summaries").
pub const DEFAULT_LIST_LIMIT: i32 = 50;

/// The shortest `logGroupNamePattern` the model admits: `LogGroupNameRegexPattern`
/// carries `@length(min: 3, max: 129)`.
pub const LOG_GROUP_NAME_PATTERN_MIN_LEN: usize = 3;

/// The longest `logGroupNamePattern` the model admits. See
/// [`LOG_GROUP_NAME_PATTERN_MIN_LEN`].
pub const LOG_GROUP_NAME_PATTERN_MAX_LEN: usize = 129;

/// How many `|`-separated alternatives one `logGroupNamePattern` carries, from the
/// member's own documentation: "You can specify as many as five different regular
/// expression patterns in this field".
pub const MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES: usize = 5;

/// The shortest one alternative of a `logGroupNamePattern` can be, from the same
/// sentence: "each of which must be between 3 and 24 characters".
///
/// The MAXIMUM half of that sentence — and of the `@pattern`'s `{3,24}` segments — is
/// deliberately NOT a constant here: the service accepts an alternative longer than 24
/// characters (log group names routinely exceed it), so refusing one would refuse a
/// request the service answers. See [`check_log_group_name_pattern`].
pub const MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN: usize = 3;

/// The prefix anchor a `logGroupNamePattern` alternative may carry: "Use ^ to match log
/// group names by prefix."
pub const LOG_GROUP_NAME_PATTERN_ANCHOR: char = '^';

/// The separator between a `logGroupNamePattern`'s alternatives: "To match multiple
/// patterns, separate them with a | as in the example ^/aws/lambda|discovery".
pub const LOG_GROUP_NAME_PATTERN_SEPARATOR: char = '|';

/// AWS's documented log group quota, from `CreateLogGroup`: "You can create up to
/// 1,000,000 log groups per Region per account."
pub const MAX_LOG_GROUPS: usize = 1_000_000;

/// AWS's documented resource-policy quota, from `PutResourcePolicy`: "An account can
/// have a maximum of 10 policies without resourceARN and one per LogGroup resourceARN".
/// The per-resource half of that quota is structural here: a resource-scoped policy is
/// stored ON its log group ([`LogGroupRecord::resource_policy`]), so a group can hold
/// exactly one.
pub const MAX_ACCOUNT_RESOURCE_POLICIES: usize = 10;

/// The scope of a policy attached to no resource, from the model's `PolicyScope` enum
/// (`ACCOUNT` | `RESOURCE`). This is also the scope `DescribeResourcePolicies` reports
/// on when the request names no `policyScope` ("When not specified, defaults to
/// ACCOUNT").
pub const POLICY_SCOPE_ACCOUNT: &str = "ACCOUNT";

/// The scope of a policy attached to one CloudWatch Logs resource, from the model's
/// `PolicyScope` enum (`ACCOUNT` | `RESOURCE`).
pub const POLICY_SCOPE_RESOURCE: &str = "RESOURCE";

/// The retentions a log group can be set to, from the model's `Days` shape: "The number
/// of days to retain the log events in the specified log group. Possible values are: 1,
/// 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557,
/// 2922, 3288, and 3653." `Days` is a plain integer in the model, so nothing refuses a
/// value outside this list before a handler runs. Ascending, so the list renders in the
/// documented order in a refusal message.
pub const RETENTION_IN_DAYS_VALUES: [i32; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

/// AWS's documented subscription-filter quota, from `PutSubscriptionFilter`: "Each log
/// group can have up to two subscription filters associated with it." A put naming a
/// filter the group already holds is an update of that filter, so it does not count
/// against this.
pub const MAX_SUBSCRIPTION_FILTERS_PER_GROUP: usize = 2;

/// The destinations `PutSubscriptionFilter` documents as supported, as the service
/// segment of the `destinationArn` naming each: "An Amazon Kinesis stream", "A logical
/// destination (specified using an ARN)" created with `PutDestination` (a CloudWatch
/// Logs `destination` resource), "A Kinesis Data Firehose delivery stream" and "A Lambda
/// function". The resource prefix is the one each of those services' ARNs carries.
pub const SUBSCRIPTION_DESTINATIONS: [(&str, &str); 4] = [
    ("kinesis", "stream/"),
    ("firehose", "deliverystream/"),
    ("lambda", "function:"),
    ("logs", "destination:"),
];

/// The system fields a subscription filter can emit, from
/// `PutSubscriptionFilter$emitSystemFields`: "Valid values are @aws.account,
/// @aws.region, and @source.log".
pub const SUBSCRIPTION_SYSTEM_FIELDS: [&str; 3] = ["@aws.account", "@aws.region", "@source.log"];

/// The `distribution` a subscription filter carries when the put naming it specified
/// none, from `PutSubscriptionFilter$distribution`: "By default, log data is grouped by
/// log stream, but the grouping can be set to random for a more even distribution." It is
/// one of the model's `Distribution` enum values (`ByLogStream` | `Random`), so a filter
/// always has a distribution to report.
pub const DEFAULT_DISTRIBUTION: &str = "ByLogStream";

/// The bracketed forms a filter pattern can open, as `(open, close)` pairs: a JSON
/// selector pattern is wrapped in `{ … }` and a space-delimited pattern in `[ … ]`
/// ("Filter pattern syntax for metric filters, subscription filters, filter log events,
/// and Live Tail", referenced from `PutSubscriptionFilter`). A pattern that opens one of
/// these forms must close it, which is the rule
/// [`validate_filter_pattern`] holds a pattern to.
pub const FILTER_PATTERN_DELIMITERS: [(char, char); 2] = [('{', '}'), ('[', ']')];

/// THE EMULATOR'S OWN CEILING on the log streams one group holds, not an AWS quota:
/// CloudWatch Logs documents no limit on log streams, but this emulator keeps every
/// stream in memory, so it refuses to create more than this many in one group rather
/// than growing without bound on request.
pub const MAX_STREAMS_PER_GROUP: usize = 10_000;

/// THE EMULATOR'S OWN STORAGE CEILING for one log stream, not an AWS quota: the events
/// a stream holds live in memory, so it keeps at most this many of them, and at most
/// [`MAX_STREAM_BYTES`] of message bytes, discarding the OLDEST first when a write would
/// exceed either. That is the effect a caller already observes from the real service
/// when events pass a log group's retention period and are deleted.
pub const MAX_STREAM_EVENTS: usize = 20_000;

/// The byte half of the per-stream storage ceiling. See [`MAX_STREAM_EVENTS`].
pub const MAX_STREAM_BYTES: i64 = 8 * 1024 * 1024;

/// Per-service state for com-amazonaws-cloudwatchlogs-logs-20140328. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Every created log group, keyed by `logGroupName`. A `BTreeMap` so
    /// `DescribeLogGroups` enumerates in log-group-name order without re-sorting.
    pub log_groups: BTreeMap<String, LogGroupRecord>,
    /// Monotonic ingestion counter. It totally orders events that share a timestamp
    /// and makes the pagination cursors and sequence tokens stable.
    pub next_ingestion_seq: u64,
    /// The account-scoped resource policies — the ones `PutResourcePolicy` creates
    /// without a `resourceArn` — keyed by `policyName`. A `BTreeMap` so
    /// `DescribeResourcePolicies` enumerates them in policy-name order. A
    /// resource-SCOPED policy is not here: it is attached to the log group its ARN
    /// names ([`LogGroupRecord::resource_policy`]), so deleting that group revokes its
    /// policy with it.
    pub account_resource_policies: BTreeMap<String, ResourcePolicyRecord>,
    /// Monotonic counter behind the resource-policy revision ids, so every update of a
    /// resource-scoped policy mints a revision no earlier update used.
    pub next_policy_revision: u64,
    /// The KMS key associated with the account's stored query-insights results — the
    /// `AssociateKmsKey` target named by the `query-result` ARN rather than by a log
    /// group ("use that key to encrypt the stored results of all future StartQuery
    /// operations in the account"). `None` until one is associated, and `None` again
    /// after `DisassociateKmsKey`.
    pub query_results_kms_key_id: Option<String>,
}

impl ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Take the next ingestion sequence number, advancing the counter.
    pub fn take_ingestion_seq(&mut self) -> u64 {
        let seq = self.next_ingestion_seq;
        self.next_ingestion_seq = self.next_ingestion_seq.saturating_add(1);
        seq
    }

    /// Take the next resource-policy revision number, advancing the counter.
    pub fn take_policy_revision(&mut self) -> u64 {
        let seq = self.next_policy_revision;
        self.next_policy_revision = self.next_policy_revision.saturating_add(1);
        seq
    }
}

/// One resource policy — "a policy enabling one or more entities to put logs to a log
/// group in this account" — as it is stored.
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    pub policy_name: String,
    pub policy_document: String,
    pub last_updated_time_ms: i64,
    /// The ARN this policy is attached to, for a resource-scoped policy; `None` for an
    /// account-scoped one (`ResourcePolicy$resourceArn` is "only populated for
    /// resource-scoped policies").
    pub resource_arn: Option<String>,
    /// The revision this policy is at, for a resource-scoped policy; `None` for an
    /// account-scoped one (`ResourcePolicy$revisionId` is "only populated for
    /// resource-scoped policies").
    pub revision_id: Option<String>,
}

impl ResourcePolicyRecord {
    /// This policy's `policyScope`: `RESOURCE` when it is attached to a resource,
    /// `ACCOUNT` when it is attached to none.
    pub fn policy_scope(&self) -> &'static str {
        if self.resource_arn.is_some() {
            POLICY_SCOPE_RESOURCE
        } else {
            POLICY_SCOPE_ACCOUNT
        }
    }
}

/// One created log group and everything stored under it.
#[derive(Debug, Clone)]
pub struct LogGroupRecord {
    pub name: String,
    /// The ARN version that carries a trailing `:*` (the `LogGroup$arn` member).
    pub arn: String,
    /// The ARN version without the trailing `:*` (the `LogGroup$logGroupArn` member).
    pub log_group_arn: String,
    pub creation_time_ms: i64,
    pub retention_in_days: Option<i32>,
    pub kms_key_id: Option<String>,
    pub log_group_class: String,
    pub deletion_protection_enabled: bool,
    /// Whether bearer token authentication is enabled for this group
    /// (`LogGroup$bearerTokenAuthenticationEnabled`: "Indicates whether bearer token
    /// authentication is enabled for this log group. When enabled, bearer token
    /// authentication is allowed on operations until it is explicitly disabled.").
    /// `PutBearerTokenAuthentication` is what moves it; a new group is not enabled.
    pub bearer_token_authentication_enabled: bool,
    pub stored_bytes: i64,
    pub metric_filter_count: i32,
    pub tags: BTreeMap<String, String>,
    /// The group's log streams, keyed by `logStreamName`.
    pub streams: BTreeMap<String, LogStreamRecord>,
    /// The group's subscription filters, keyed by `filterName`. A `BTreeMap` so
    /// `DescribeSubscriptionFilters` enumerates them ASCII-sorted by filter name
    /// without re-sorting, and so a put under a name already stored updates that
    /// filter in place.
    pub subscription_filters: BTreeMap<String, SubscriptionFilterRecord>,
    /// The resource-scoped policy attached to this group, if one has been put: AWS
    /// allows "one per LogGroup resourceARN", so at most one.
    pub resource_policy: Option<ResourcePolicyRecord>,
    /// The data sources that ingest into this group — what `ListLogGroups`'s
    /// `dataSources` filter selects on and what `ListAggregateLogGroupSummaries`
    /// counts log groups by. See [`DataSourceRecord`] for why a group created through
    /// this emulator's API carries none.
    pub data_sources: Vec<DataSourceRecord>,
}

impl LogGroupRecord {
    /// A brand-new log group: the class default, no retention, no stored bytes.
    pub fn new(region: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_group_arn(region, name, true),
            log_group_arn: log_group_arn(region, name, false),
            creation_time_ms,
            retention_in_days: None,
            kms_key_id: None,
            log_group_class: DEFAULT_LOG_GROUP_CLASS.to_string(),
            deletion_protection_enabled: false,
            bearer_token_authentication_enabled: false,
            stored_bytes: 0,
            metric_filter_count: 0,
            tags: BTreeMap::new(),
            streams: BTreeMap::new(),
            subscription_filters: BTreeMap::new(),
            resource_policy: None,
            data_sources: Vec::new(),
        }
    }
}

/// One data source that ingests into a log group: the model's `DataSource` — "a data
/// source that categorizes logs by originating service and log type" — plus the format
/// the aggregate summaries can additionally group by (`dataSource.Format`).
///
/// A log group created through this emulator's API carries none, and that is a property
/// of the API surface rather than a shortcut: the model's `LogGroup` has no data-source
/// member, no operation associates a data source WITH a log group (the one operation that
/// names a `DataSource`, `AssociateSourceToS3TableIntegration`, attaches it to an S3
/// Tables integration, and is a 501 stub), and the data sources AWS reports are the ones
/// its own vended-log ingestion created. So the aggregate grouped by data-source
/// characteristics counts nothing here — which is a 200 with an empty list, not a
/// refusal — and both readers go through this field, so they report whatever a later
/// operation stores in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DataSourceRecord {
    /// `DataSource$name` — "The name of the data source."
    pub name: String,
    /// `DataSource$type` — "The type of the data source."
    pub source_type: Option<String>,
    /// The data source's format, which `groupBy=DATA_SOURCE_NAME_TYPE_AND_FORMAT`
    /// groups by (`GroupingIdentifier`: "Examples are, dataSource.Name,
    /// dataSource.Type, and dataSource.Format").
    pub format: Option<String>,
}

/// Whether one data source satisfies a `dataSources` filter list.
///
/// From `ListLogGroups$dataSources`: "You can filter by data source name, type, or both.
/// Multiple filters within the same dimension are combined with OR logic, while filters
/// across different dimensions are combined with AND logic" — so every filter's `name`
/// forms one dimension and every filter's `type` the other, and each is matched as the
/// "name pattern" / "type pattern" the members document: a case-sensitive substring.
pub fn data_source_matches_filters(
    source: &DataSourceRecord,
    filters: &[crate::ops::types::DataSourceFilter],
) -> bool {
    // `DataSourceFilter$name` is `@required`, so the name dimension is never empty.
    let name_matches = filters
        .iter()
        .any(|filter| source.name.contains(&filter.name));
    let wanted_types: Vec<&str> = filters
        .iter()
        .filter_map(|filter| filter.r#type.as_deref())
        .collect();
    let type_matches = wanted_types.is_empty()
        || source.source_type.as_deref().is_some_and(|source_type| {
            wanted_types
                .iter()
                .any(|wanted| source_type.contains(wanted))
        });
    name_matches && type_matches
}

/// Whether one log group satisfies a `dataSources` filter list: one of its data sources
/// does ([`data_source_matches_filters`]).
pub fn log_group_matches_data_source_filters(
    group: &LogGroupRecord,
    filters: &[crate::ops::types::DataSourceFilter],
) -> bool {
    group
        .data_sources
        .iter()
        .any(|source| data_source_matches_filters(source, filters))
}

/// Whether one character may appear in a `logGroupNamePattern` alternative, from the
/// `LogGroupNameRegexPattern` `@pattern`'s character class `[\.\-_\/#A-Za-z0-9]` — which
/// is exactly the class a `LogGroupName` is built from, so an alternative can only ever
/// be literal text.
fn is_log_group_name_pattern_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '/' | '#')
}

/// The `LogGroupNameRegexPattern` `@pattern` as the service states it in a refusal: the
/// modelled expression itself, anchors included — the one pattern-rendering rule this
/// service words every constraint refusal with, stated on `requirement` in
/// `crate::service`. It includes the per-alternative 24-character maximum the service
/// accepts values past, which the emulator therefore does not enforce (see
/// [`check_log_group_name_pattern`]).
pub const LOG_GROUP_NAME_REGEX_PATTERN: &str =
    r"^(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}$";

/// One refused member, worded on the template CloudWatch Logs words a constraint
/// violation with: "N validation error detected: Value at '<member>' failed to satisfy
/// constraint: <constraint>". The template is built from the request — the member that
/// broke a constraint, and that member's modelled constraint — and carries NOTHING of the
/// submitted value.
///
/// Delegates to `crate::service`, which owns that template for every refusal of this
/// service, so a violation a handler finds and one the boundary walk finds read alike.
pub fn validation_message(member: &str, constraint: &str) -> String {
    crate::service::one_violation_message(member, constraint)
}

/// Judge a `logGroupNamePattern` — the member `ListLogGroups` and
/// `ListAggregateLogGroupSummaries` share — answering the message to refuse it with.
///
/// The member targets `LogGroupNameRegexPattern`, whose `@pattern` decomposes the value
/// into up to five `|`-separated alternatives of 3..24 characters. The 24-character
/// MAXIMUM is the one part of that the service does not apply: it answers 200 for a
/// single-alternative pattern far longer than 24 characters, which is what a pattern
/// naming a real log group looks like. Everything else the model and the documentation
/// state is enforced here, because the member carries `skip_validation` and the boundary
/// walk no longer sees it:
///
/// * the `@length(min: 3, max: 129)` on the whole value;
/// * "as many as five different regular expression patterns in this field";
/// * the minimum of "each of which must be between 3 and 24 characters";
/// * the character class, plus the optional leading `^` anchor on each alternative.
pub fn check_log_group_name_pattern(pattern: &str) -> Result<(), String> {
    const MEMBER: &str = "logGroupNamePattern";
    let refuse = |constraint: String| Err(validation_message(MEMBER, &constraint));

    let length = pattern.chars().count();
    if length < LOG_GROUP_NAME_PATTERN_MIN_LEN {
        return refuse(format!(
            "Member must have length greater than or equal to {LOG_GROUP_NAME_PATTERN_MIN_LEN}"
        ));
    }
    if length > LOG_GROUP_NAME_PATTERN_MAX_LEN {
        return refuse(format!(
            "Member must have length less than or equal to {LOG_GROUP_NAME_PATTERN_MAX_LEN}"
        ));
    }
    let alternatives: Vec<&str> = pattern.split(LOG_GROUP_NAME_PATTERN_SEPARATOR).collect();
    if alternatives.len() > MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES {
        return refuse(format!(
            "Member must specify at most {MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES} \
             '{LOG_GROUP_NAME_PATTERN_SEPARATOR}'-separated patterns"
        ));
    }
    for alternative in alternatives {
        let literal = alternative
            .strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR)
            .unwrap_or(alternative);
        let shaped = literal.chars().count() >= MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN
            && literal.chars().all(is_log_group_name_pattern_char);
        if !shaped {
            return refuse(format!(
                "Member must satisfy regular expression pattern: \
                 {LOG_GROUP_NAME_REGEX_PATTERN}"
            ));
        }
    }
    Ok(())
}

/// Whether a log group name matches a `logGroupNamePattern`.
///
/// The member documents the whole match rule: "Use ^ to match log group names by prefix.
/// For a substring match, specify the string to match. All matches are case sensitive To
/// match multiple patterns, separate them with a | as in the example ^/aws/lambda|
/// discovery". The pattern's character class admits no regular-expression metacharacter
/// beyond that `^` and `|`, so each alternative is literal text: anchored, a prefix;
/// otherwise, a substring. A name matches when ANY alternative matches it.
///
/// Caller-side the value has already been through [`check_log_group_name_pattern`].
pub fn log_group_name_matches_pattern(name: &str, pattern: &str) -> bool {
    pattern
        .split(LOG_GROUP_NAME_PATTERN_SEPARATOR)
        .any(|alternative| {
            match alternative.strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR) {
                Some(prefix) => name.starts_with(prefix),
                None => name.contains(alternative),
            }
        })
}

/// One subscription filter — "a real-time stream of log events ingested through
/// PutLogEvents" delivered to one destination — as it is stored. It lives on the log
/// group it is "associated with", so deleting that group removes its filters with it.
#[derive(Debug, Clone)]
pub struct SubscriptionFilterRecord {
    pub filter_name: String,
    pub filter_pattern: String,
    pub destination_arn: String,
    pub role_arn: Option<String>,
    /// How log data is spread over the destination. Never absent: a put that names no
    /// `distribution` gets the documented default ([`DEFAULT_DISTRIBUTION`]), so a
    /// client reads back the grouping the filter is actually configured with.
    pub distribution: String,
    pub apply_on_transformed_logs: bool,
    pub field_selection_criteria: Option<String>,
    pub emit_system_fields: Option<Vec<String>>,
    pub creation_time_ms: i64,
}

/// One created log stream and its events.
#[derive(Debug, Clone)]
pub struct LogStreamRecord {
    pub name: String,
    pub arn: String,
    pub creation_time_ms: i64,
    pub first_event_timestamp: Option<i64>,
    pub last_event_timestamp: Option<i64>,
    pub last_ingestion_time: Option<i64>,
    /// `None` until the first `PutLogEvents`: AWS reports no `uploadSequenceToken`
    /// for a stream nothing has been written to.
    pub upload_sequence_token: Option<String>,
    /// Events kept sorted by `(timestamp, ingestion_seq)`.
    pub events: Vec<StoredEvent>,
}

impl LogStreamRecord {
    /// A brand-new, empty log stream.
    pub fn new(region: &str, group_name: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_stream_arn(region, group_name, name),
            creation_time_ms,
            first_event_timestamp: None,
            last_event_timestamp: None,
            last_ingestion_time: None,
            upload_sequence_token: None,
            events: Vec::new(),
        }
    }

    /// Re-derive the event-time summary members from the stored events.
    pub fn refresh_event_times(&mut self) {
        self.first_event_timestamp = self.events.first().map(|e| e.timestamp);
        self.last_event_timestamp = self.events.last().map(|e| e.timestamp);
    }

    /// The bytes the stream's events occupy, counted the way a log group's
    /// `storedBytes` counts them.
    pub fn stored_bytes(&self) -> i64 {
        self.events
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add)
    }

    /// Hold the stream inside the emulator's storage ceiling ([`MAX_STREAM_EVENTS`] /
    /// [`MAX_STREAM_BYTES`]) by discarding the oldest events, and answer how many bytes
    /// went, so the owning log group's `storedBytes` can be reduced by the same amount.
    ///
    /// Events are stored oldest-first, so the discarded run is a prefix. A single event
    /// can never exceed the byte ceiling (a log event is capped at 1 MB on the way in),
    /// so the newest event always survives.
    pub fn enforce_storage_ceiling(&mut self) -> i64 {
        let mut kept_bytes = self.stored_bytes();
        let mut discard = 0usize;
        while discard < self.events.len()
            && (self.events.len() - discard > MAX_STREAM_EVENTS || kept_bytes > MAX_STREAM_BYTES)
        {
            kept_bytes = kept_bytes.saturating_sub(self.events[discard].stored_bytes());
            discard += 1;
        }
        let discarded_bytes = self.events[..discard]
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add);
        self.events.drain(..discard);
        discarded_bytes
    }
}

/// One ingested log event.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub timestamp: i64,
    pub message: String,
    pub ingestion_time: i64,
    pub ingestion_seq: u64,
}

impl StoredEvent {
    /// The total order events are stored and paginated in.
    pub fn key(&self) -> EventKey {
        EventKey {
            timestamp: self.timestamp,
            seq: self.ingestion_seq,
        }
    }

    /// What this event adds to its log group's `storedBytes`: the UTF-8 message plus
    /// the documented 26-byte per-event overhead.
    pub fn stored_bytes(&self) -> i64 {
        i64::try_from(self.message.len()).unwrap_or(i64::MAX) + EVENT_OVERHEAD_BYTES
    }
}

/// A position in a stream's event order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKey {
    pub timestamp: i64,
    pub seq: u64,
}

impl EventKey {
    /// The position before every event, so a forward scan from it starts at the head.
    pub const BEFORE_ALL: Self = Self {
        timestamp: i64::MIN,
        seq: 0,
    };
    /// The position after every event, so a backward scan from it starts at the tail.
    pub const AFTER_ALL: Self = Self {
        timestamp: i64::MAX,
        seq: u64::MAX,
    };
}

/// The region to stamp into the ARNs this request's resources get: the SigV4
/// credential scope's region, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Wall-clock milliseconds since the Unix epoch — what every CloudWatch Logs
/// timestamp member is expressed in.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// A log group's ARN, with the trailing `:*` when `wildcard_suffix` is set (the two
/// versions the model's `arn` and `logGroupArn` members document).
pub fn log_group_arn(region: &str, name: &str, wildcard_suffix: bool) -> String {
    let resource = if wildcard_suffix {
        format!("log-group:{name}:*")
    } else {
        format!("log-group:{name}")
    };
    ::engine::emulator_arn("logs", region, &resource)
}

/// A log stream's ARN.
pub fn log_stream_arn(region: &str, group_name: &str, stream_name: &str) -> String {
    ::engine::emulator_arn(
        "logs",
        region,
        &format!("log-group:{group_name}:log-stream:{stream_name}"),
    )
}

/// The service and resource segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN. The resource segment is whatever follows the account, colons and
/// slashes included, because a resource id carries its own separators.
pub fn arn_service_and_resource(arn: &str) -> Option<(&str, &str)> {
    arn_parts(arn).map(|(service, _account, resource)| (service, resource))
}

/// The `(service, account, resource)` segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN.
fn arn_parts(arn: &str) -> Option<(&str, &str, &str)> {
    let rest = arn.strip_prefix("arn:")?;
    // partition, service, region, account, then the whole resource segment.
    let mut segments = rest.splitn(5, ':');
    let _partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let account = segments.next()?;
    Some((service, account, segments.next().unwrap_or_default()))
}

/// The destination vendor CloudWatch Logs delivers to without assuming a role: a Lambda
/// function is invoked directly, which is why `PutSubscriptionFilter` documents the
/// `iam:PassRole` requirement for "any destination except a Lambda function".
pub const SELF_DELIVERED_DESTINATION_VENDOR: &str = "lambda";

/// The vendor of a `destinationArn` whose delivery needs the role `roleArn` names, or
/// `None` when the filter can be configured without one.
///
/// "To perform a PutSubscriptionFilter operation for any destination except a Lambda
/// function, you must also have the iam:PassRole permission": every vendor but Lambda is
/// delivered to through a role CloudWatch Logs assumes. The exception is a logical
/// destination owned by ANOTHER account, where the destination's own policy authorises
/// the delivery (`roleArn`: "You don't need to provide the ARN when you are working with
/// a logical destination for cross-account delivery").
///
/// A vendor this operation delivers to nothing in ([`SUBSCRIPTION_DESTINATIONS`]) needs
/// no role either: there is no delivery to authorise, and such a `destinationArn` is
/// refused for the resource it names instead.
pub fn subscription_role_required_vendor(arn: &str) -> Option<&str> {
    let (vendor, account, resource) = arn_parts(arn)?;
    if vendor == SELF_DELIVERED_DESTINATION_VENDOR
        || !SUBSCRIPTION_DESTINATIONS
            .iter()
            .any(|(name, _)| vendor == *name)
    {
        return None;
    }
    let cross_account_logical_destination = vendor == "logs"
        && resource.starts_with("destination:")
        && account != ::engine::emulator_account_id();
    if cross_account_logical_destination {
        return None;
    }
    Some(vendor)
}

/// Whether an ARN names a CloudWatch Logs log group — the only resource type a resource
/// policy can be attached to (`PutResourcePolicy$resourceArn`: "Currently only supports
/// LogGroup ARN").
///
/// A log group's ARN is `arn:<partition>:logs:<region>:<account>:log-group:<name>`, with
/// or without the trailing `:*` the `LogGroup$arn` member carries. A log STREAM's ARN
/// also names a log group, so it is excluded here: it names a stream, which cannot carry
/// a resource policy.
pub fn is_log_group_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == "logs"
            && resource.starts_with("log-group:")
            && !resource.contains(":log-stream:")
    })
}

/// Whether an ARN names the account's stored query-insights results — the non-log-group
/// target `AssociateKmsKey` / `DisassociateKmsKey` document: "Specify the following ARN
/// to have future GetQueryResults operations in this account encrypt the results with
/// the specified KMS key [...] arn:aws:logs:REGION:ACCOUNT_ID:query-result:*".
pub fn is_query_result_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == "logs" && resource == "query-result:*")
}

/// Whether a `kmsKeyId` names a KMS key: the member is documented as "The Amazon
/// Resource Name (ARN) of the KMS key to use when encrypting log data", so a value that
/// is an ARN must be one of KMS's. A bare key id or alias carries no service segment and
/// is left to KMS to resolve, but an empty value names nothing at all.
pub fn is_kms_key_id(kms_key_id: &str) -> bool {
    if kms_key_id.trim().is_empty() {
        return false;
    }
    match arn_service_and_resource(kms_key_id) {
        Some((service, resource)) => service == "kms" && !resource.is_empty(),
        None => !kms_key_id.starts_with("arn:"),
    }
}

/// Whether an ARN names a destination `PutSubscriptionFilter` supports delivering to:
/// one of the services in [`SUBSCRIPTION_DESTINATIONS`], naming a resource of the kind
/// that service's entry there gives.
///
/// A CloudWatch Logs ARN qualifies only when it names a logical `destination` (the
/// resource `PutDestination` creates), never a log group or a log stream.
pub fn is_subscription_destination_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        SUBSCRIPTION_DESTINATIONS
            .iter()
            .any(|(name, prefix)| service == *name && resource.starts_with(prefix))
    })
}

/// AWS's request-derived refusal message for a member the service parses as an ARN and
/// was handed something that is not one: the validation-error template naming the member
/// and the value it could not parse. `None` when the value parses as an ARN, whatever
/// resource it names — a well-formed ARN of the wrong kind is a different refusal, with
/// its own message.
pub fn unparsable_arn_message(member: &str, value: &str) -> Option<String> {
    arn_service_and_resource(value)
        .is_none()
        .then(|| format!("1 validation error detected: Unable to parse {member} {value}"))
}

/// Hold a `filterPattern` to the filter-pattern syntax's bracket rule: a pattern that
/// opens one of the bracketed forms in [`FILTER_PATTERN_DELIMITERS`] must close it with
/// the matching delimiter, and the brackets it nests inside that form must balance. The
/// `Err` carries the message to refuse with, as the service states the rule.
///
/// A pattern that opens no bracketed form — a term, a text or a regular-expression
/// pattern — carries no bracket structure to check, and the empty pattern (the modelled
/// `FilterPattern` is `length 0..1024`) matches every event.
pub fn validate_filter_pattern(pattern: &str) -> Result<(), String> {
    let pattern = pattern.trim();
    let Some(first) = pattern.chars().next() else {
        return Ok(());
    };
    let Some((open, close)) = FILTER_PATTERN_DELIMITERS
        .iter()
        .copied()
        .find(|(open, _)| *open == first)
    else {
        return Ok(());
    };
    if !pattern.ends_with(close) {
        return Err(format!(
            "If a filter pattern starts with '{open}' it must end with '{close}'"
        ));
    }
    if let Err(defect) = filter_pattern_structure(pattern, open) {
        return Err(match defect {
            PatternDefect::UnbalancedBrackets => format!(
                "Invalid filter pattern: the brackets in a '{open}' … '{close}' filter pattern must be balanced"
            ),
            PatternDefect::UnterminatedQuotedString => {
                "Invalid filter pattern: a quoted string is not terminated".to_string()
            }
            PatternDefect::UnterminatedRegex => {
                "Invalid filter pattern: a regular expression is not terminated".to_string()
            }
        });
    }
    Ok(())
}

/// What [`filter_pattern_structure`] found wrong with a pattern, so the refusal names the
/// defect it detected rather than one of them for all three.
enum PatternDefect {
    UnbalancedBrackets,
    UnterminatedQuotedString,
    UnterminatedRegex,
}

/// Whether a filter pattern's structure closes: its brackets open and close in order —
/// the bracketed forms in [`FILTER_PATTERN_DELIMITERS`], plus the `()` that group a JSON
/// pattern's terms, only in a JSON pattern, because a space-delimited pattern's terms can
/// carry a bare parenthesis — and the literals it opens are terminated.
///
/// A quoted string (`"…"`) and a regular-expression literal (`%…%`) are opaque: the
/// pattern syntax lets them carry any character, so a bracket inside one is data rather
/// than structure. A backslash escapes the character after it.
fn filter_pattern_structure(pattern: &str, opened: char) -> Result<(), PatternDefect> {
    let groups_terms_in_parentheses = opened == '{';
    let mut open_brackets: Vec<char> = Vec::new();
    let mut quoted = false;
    let mut regex = false;
    let mut chars = pattern.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' if !regex => quoted = !quoted,
            '%' if !quoted => regex = !regex,
            _ if quoted || regex => {}
            '(' | ')' if !groups_terms_in_parentheses => {}
            '{' | '[' | '(' => open_brackets.push(character),
            '}' | ']' | ')' => {
                let opened = match character {
                    '}' => '{',
                    ']' => '[',
                    _ => '(',
                };
                if open_brackets.pop() != Some(opened) {
                    return Err(PatternDefect::UnbalancedBrackets);
                }
            }
            _ => {}
        }
    }
    if quoted {
        return Err(PatternDefect::UnterminatedQuotedString);
    }
    if regex {
        return Err(PatternDefect::UnterminatedRegex);
    }
    if open_brackets.is_empty() {
        Ok(())
    } else {
        Err(PatternDefect::UnbalancedBrackets)
    }
}

/// The log group name a `logGroupIdentifier` names: the identifier itself when it is a
/// name, or the name segment of a log-group ARN (either ARN version).
pub fn log_group_name_from_identifier(identifier: &str) -> &str {
    const MARKER: &str = ":log-group:";
    match identifier.find(MARKER) {
        Some(at) => {
            let rest = &identifier[at + MARKER.len()..];
            let rest = rest.strip_suffix(":*").unwrap_or(rest);
            // A log-stream ARN carries the group name before `:log-stream:`.
            match rest.find(":log-stream:") {
                Some(end) => &rest[..end],
                None => rest,
            }
        }
        None => identifier,
    }
}

/// The refusal for a request that names BOTH log-group selectors, and the one that names
/// NEITHER ([`resolve_log_group_target`]).
///
/// This pair's refusal is the one CloudWatch Logs answers with `ValidationException`
/// (HTTP 400) rather than with the operation's own `InvalidParameterException`: it is
/// checked across two members, which no single member's constraint can express, and the
/// recorded refusal for a request naming both selectors is `400 ValidationException:
/// LogGroup name and LogGroup ARN are mutually exclusive parameters.` — which is why each
/// caller widens that shape in through `extra_errors`. The sentences live here, beside the
/// check that decides them, and name the two selectors the way the service names them: the
/// log group's NAME and its ARN.
pub const LOG_GROUP_SELECTORS_EXCLUSIVE: &str =
    "LogGroup name and LogGroup ARN are mutually exclusive parameters.";

/// The refusal for a request that names neither selector; see
/// [`LOG_GROUP_SELECTORS_EXCLUSIVE`]. Both sentences are the service's own fixed wording
/// for a request-independent refusal, and both name the selectors as the service names
/// them rather than as the members spell them.
pub const LOG_GROUP_SELECTOR_REQUIRED: &str = "Either LogGroupName or LogGroupArn must be provided.";

/// Resolve the log group an operation's `logGroupName` / `logGroupIdentifier` pair
/// names. The model documents them as mutually exclusive with one of them required, so
/// neither-or-both is a client error; the `Err` carries the message to refuse with, and
/// [`LOG_GROUP_SELECTORS_EXCLUSIVE`] records which error shape a caller refuses with.
pub fn resolve_log_group_target(
    log_group_name: Option<&str>,
    log_group_identifier: Option<&str>,
) -> Result<String, String> {
    match (log_group_name, log_group_identifier) {
        (Some(name), None) => Ok(name.to_string()),
        (None, Some(identifier)) => Ok(log_group_name_from_identifier(identifier).to_string()),
        (Some(_), Some(_)) => Err(LOG_GROUP_SELECTORS_EXCLUSIVE.to_string()),
        (None, None) => Err(LOG_GROUP_SELECTOR_REQUIRED.to_string()),
    }
}

/// What an `AssociateKmsKey` / `DisassociateKmsKey` request names: "you specify either
/// the logGroupName parameter or the resourceIdentifier parameter", and a
/// `resourceIdentifier` is either a log group's ARN or the account's query-result ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KmsKeyTarget {
    /// One log group, by the name the store is keyed by.
    LogGroup(String),
    /// The account's stored query-insights results.
    QueryResults,
}

/// Resolve the target of an `AssociateKmsKey` / `DisassociateKmsKey` request. The two
/// parameters are documented as mutually exclusive with one of them required, and a
/// `resourceIdentifier` naming neither a log group nor the query-result resource names
/// no target at all; the `Err` carries the message to refuse with.
pub fn resolve_kms_key_target(
    log_group_name: Option<&str>,
    resource_identifier: Option<&str>,
) -> Result<KmsKeyTarget, String> {
    match (log_group_name, resource_identifier) {
        (Some(name), None) => Ok(KmsKeyTarget::LogGroup(name.to_string())),
        (None, Some(identifier)) => {
            if is_query_result_arn(identifier) {
                Ok(KmsKeyTarget::QueryResults)
            } else if is_log_group_arn(identifier) {
                Ok(KmsKeyTarget::LogGroup(
                    log_group_name_from_identifier(identifier).to_string(),
                ))
            } else {
                Err(format!(
                    "resourceIdentifier must be a log group ARN or a query-result ARN, got '{identifier}'"
                ))
            }
        }
        (Some(_), Some(_)) => Err(
            "logGroupName and resourceIdentifier are mutually exclusive; specify only one"
                .to_string(),
        ),
        (None, None) => {
            Err("either logGroupName or resourceIdentifier must be specified".to_string())
        }
    }
}

/// The revision id a resource-scoped policy carries, minted from the monotonic policy
/// revision counter so it is unique and derived from stored state. AWS documents no
/// format for it — only that a put must present the revision the policy is currently
/// at — so this renders the counter as a fixed-width hex string.
pub fn mint_revision_id(seq: u64) -> String {
    format!("{seq:032x}")
}

/// The sequence token a `PutLogEvents` call hands back, minted from the monotonic
/// ingestion counter so it is unique and derived from stored state. Rendered the way
/// CloudWatch Logs renders one: a fixed-width decimal string.
pub fn mint_sequence_token(seq: u64) -> String {
    format!("{seq:056}")
}

/// The refusal CloudWatch Logs answers when the resource an operation names is not
/// there. The service states the resource KIND that is missing, in the sentence form the
/// model documents for `ResourceNotFoundException` — "The specified resource does not
/// exist." — terminated like that sentence, so every operation of this service refuses a
/// missing resource the same way.
pub fn resource_not_found_message(resource_kind: &str) -> String {
    format!("The specified {resource_kind} does not exist.")
}

/// The refusal CloudWatch Logs builds from the request when the resource policy the
/// caller named is not there. The message interpolates the requested `policyName`, so a
/// caller deleting policies in a loop can attribute the refusal to one policy name
/// instead of guessing which of its requests failed.
pub fn policy_not_found_message(policy_name: &str) -> String {
    format!("Policy with name [{policy_name}] does not exist.")
}

/// The refusal for a value outside the set a member's own documentation enumerates: the
/// value is declared invalid and the permitted set is then listed, so the refusal itself
/// tells a caller which values it may use.
///
/// `subject` names what was invalid (the "retention" of `retentionInDays`, say) and
/// `valid` is the catalogue the model states — [`RETENTION_IN_DAYS_VALUES`] or
/// [`SUBSCRIPTION_SYSTEM_FIELDS`] — rendered in the documented order, never a list typed
/// into a message.
pub fn invalid_value_message<T: std::fmt::Display>(
    subject: &str,
    valid: impl IntoIterator<Item = T>,
) -> String {
    let listed = valid
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("Invalid {subject} value. Valid values are: [{listed}]")
}

/// Encode a pagination cursor as an opaque token.
///
/// The payload is this service's own structure; hex-encoding it keeps the token
/// opaque to a client, which must only round-trip it back to the next call.
pub fn encode_token(payload: &serde_json::Value) -> String {
    hex::encode(serde_json::to_vec(payload).unwrap_or_default())
}

/// Decode a token [`encode_token`] produced. `None` for anything else, which a
/// handler refuses as an invalid `nextToken`.
pub fn decode_token(token: &str) -> Option<serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchlogsLogs20140328State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchlogsLogs20140328State>)
}
