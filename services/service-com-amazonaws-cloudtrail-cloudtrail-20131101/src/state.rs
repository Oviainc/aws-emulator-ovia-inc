// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudtrail-cloudtrail-20131101 service.
//!
//! This module owns the trail registry — the service's primary resource — plus the
//! rules every trail operation shares: how a caller-supplied identifier (a bare name
//! or a trail ARN) resolves to a stored trail, which trails a request's Region can
//! see, and how a stored trail projects onto the modelled `Trail` / `TrailInfo`
//! shapes. It lives here, not in one handler, because seven operations apply the same
//! rules and two copies of them would drift.
//!
//! It owns the CloudTrail Lake registries on the same terms. An event data store
//! and the queries run against it are named by a dozen operations (`StartQuery`,
//! `DescribeQuery`, `CancelQuery`, `ListQueries`, `GetQueryResults`, the event
//! data store lifecycle), each of which has to resolve the same identifier forms
//! — an ARN or the bare id suffix of one — the same way. A dashboard identifier
//! is a bare name or a dashboard ARN, the refresh-schedule and widget rules are
//! the ones the model documents, and both are applied by every dashboard
//! operation rather than copied into each.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AdvancedEventSelector, AggregationConfiguration, ContextKeySelector, EventSelector,
    ImportSource, ImportStatistics, PartitionKey, RefreshSchedule, RequestWidget, Tag, Trail,
    TrailInfo, Widget,
};

/// The Region a request is served in when it carries no SigV4 credential scope to
/// read one from. It matches the Region the emulator writes into the AWS shared
/// config it hands clients, so an unsigned call and a default-configured SDK call
/// land in the same Region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`awsService.arnNamespace` in the Smithy model).
pub const SERVICE_NAMESPACE: &str = "cloudtrail";

/// The resource prefix of a trail ARN: `arn:aws:cloudtrail:<region>:<account>:trail/<name>`.
const TRAIL_RESOURCE_PREFIX: &str = "trail/";

/// Trail-name length bounds, from the model doc for `CreateTrailRequest$Name`
/// ("Be between 3 and 128 characters").
pub const TRAIL_NAME_MIN_LEN: usize = 3;
/// See [`TRAIL_NAME_MIN_LEN`].
pub const TRAIL_NAME_MAX_LEN: usize = 128;

/// From the model doc for `CreateTrailRequest$S3KeyPrefix`: "The maximum length is
/// 200 characters."
pub const S3_KEY_PREFIX_MAX_LEN: usize = 200;

/// From the model doc for `CreateTrailRequest$SnsTopicName`: "The maximum length is
/// 256 characters."
pub const SNS_TOPIC_NAME_MAX_LEN: usize = 256;

/// From the model doc for `TagsLimitExceededException`: "The number of tags per
/// trail, event data store, dashboard, or channel has exceeded the permitted amount.
/// Currently, the limit is 50."
pub const TAGS_PER_RESOURCE_LIMIT: usize = 50;

/// How many trails the registry holds before `CreateTrail` refuses with the
/// modelled `MaximumNumberOfTrailsExceededException`.
///
/// CloudTrail publishes an adjustable per-Region trail quota, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on the store rather than a claim about any account's limit.
/// It bounds the WHOLE registry, not one Region's share of it, because the Region a
/// trail is homed in comes from the request: a per-Region bound would still let a
/// caller grow the store without end by varying the Region it signs for. Every
/// record it holds is released by `DeleteTrail`, and it sits far above any workflow
/// an emulator serves.
pub const TRAIL_QUOTA: usize = 1_000;

/// How `ReadWriteType` reads when an event selector leaves it out, from the model doc
/// for `EventSelector$ReadWriteType`: "By default, the value is `All`." It is one of
/// [`crate::ops::types::READ_WRITE_TYPE_VALUES`].
pub const DEFAULT_READ_WRITE_TYPE: &str = "All";

/// How `IncludeManagementEvents` reads when an event selector leaves it out, from the
/// model doc for `EventSelector$IncludeManagementEvents`: "By default, the value is
/// `true`."
pub const DEFAULT_INCLUDE_MANAGEMENT_EVENTS: bool = true;

/// The resource types a BASIC event selector's `DataResource` may name, from the model
/// doc for `DataResource$Type`: "You can specify the following basic event selector
/// resource types: AWS::DynamoDB::Table, AWS::Lambda::Function, AWS::S3::Object".
/// Every other resource type is reached through advanced event selectors.
pub const BASIC_DATA_RESOURCE_TYPES: [&str; 3] = [
    "AWS::DynamoDB::Table",
    "AWS::Lambda::Function",
    "AWS::S3::Object",
];

/// The management event sources `ExcludeManagementEventSources` may name, from the
/// model doc for `EventSelector$ExcludeManagementEventSources`: "the list can be empty
/// (disables the filter), or it can filter out Key Management Service or Amazon RDS
/// Data API events by containing `kms.amazonaws.com` or `rdsdata.amazonaws.com`".
pub const EXCLUDABLE_MANAGEMENT_EVENT_SOURCES: [&str; 2] =
    ["kms.amazonaws.com", "rdsdata.amazonaws.com"];

/// How many basic event selectors one trail may carry, from the model doc for
/// `InvalidEventSelectorsException`: "A trail can have up to 5 event selectors."
pub const EVENT_SELECTORS_PER_TRAIL_LIMIT: usize = 5;

/// How many data resources one trail may carry across all of its basic event
/// selectors, from the model doc for `InvalidEventSelectorsException`: "A trail is
/// limited to 250 data resources. These data resources can be distributed across event
/// selectors, but the overall total cannot exceed 250."
pub const DATA_RESOURCES_PER_TRAIL_LIMIT: usize = 250;

/// How many condition values one trail may carry across all of its advanced event
/// selectors, from the model doc for `PutEventSelectors`: "You can add advanced event
/// selectors, and conditions for your advanced event selectors, up to a maximum of 500
/// values for all conditions and selectors on a trail."
pub const ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT: usize = 500;

/// How many trails one `ListTrails` page carries. `ListTrails` models no
/// `MaxResults`, so the page size is the service's to choose; the remainder is
/// reached through `NextToken`.
pub const LIST_TRAILS_PAGE_SIZE: usize = 100;

/// Characters a trail name may contain besides ASCII letters and digits, from the
/// model doc for `CreateTrailRequest$Name`: "periods (.), underscores (_), or
/// dashes (-)".
const TRAIL_NAME_PUNCTUATION: [char; 3] = ['.', '_', '-'];

/// The logging state of one trail.
///
/// `start_logging_time` / `stop_logging_time` are the instants the trail last
/// transitioned into and out of logging, which is what `GetTrailStatus` reports.
#[derive(Debug, Default, Clone)]
pub struct TrailStatus {
    pub is_logging: bool,
    pub start_logging_time: Option<SystemTime>,
    pub stop_logging_time: Option<SystemTime>,
}

/// The event selectors configured on one trail.
///
/// The two kinds are mutually exclusive — the model doc for `PutEventSelectors` says
/// "You can use either AdvancedEventSelectors or EventSelectors, but not both. If you
/// apply AdvancedEventSelectors to a trail, any existing EventSelectors are
/// overwritten" — so the store holds ONE of them rather than two fields that could
/// disagree.
#[derive(Debug, Clone)]
pub enum EventSelectorConfig {
    /// Basic event selectors, as `PutEventSelectors$EventSelectors` carries them.
    Basic(Vec<EventSelector>),
    /// Advanced event selectors, as `PutEventSelectors$AdvancedEventSelectors`
    /// carries them.
    Advanced(Vec<AdvancedEventSelector>),
}

/// The basic event selector a trail logs by, until something configures it.
///
/// From the model doc for `EventSelector`: "By default, trails created without
/// specific event selectors will be configured to log all read and write management
/// events, and no data events" — so `ReadWriteType` is `All`,
/// `IncludeManagementEvents` is true, and neither data resources nor excluded
/// management event sources are in effect.
pub fn default_event_selector() -> EventSelector {
    EventSelector {
        read_write_type: Some(DEFAULT_READ_WRITE_TYPE.to_string()),
        include_management_events: Some(DEFAULT_INCLUDE_MANAGEMENT_EVENTS),
        data_resources: Some(Vec::new()),
        exclude_management_event_sources: Some(Vec::new()),
    }
}

/// One persisted trail: everything `CreateTrail` was given, plus the values the
/// service mints (the trail ARN, the SNS topic ARN, the home Region) and the
/// logging state the Start/StopLogging pair moves.
#[derive(Debug, Default, Clone)]
pub struct TrailRecord {
    pub name: String,
    pub trail_arn: String,
    pub s3_bucket_name: String,
    pub s3_key_prefix: Option<String>,
    pub sns_topic_name: Option<String>,
    pub sns_topic_arn: Option<String>,
    pub include_global_service_events: bool,
    pub is_multi_region_trail: bool,
    pub log_file_validation_enabled: bool,
    pub cloud_watch_logs_log_group_arn: Option<String>,
    pub cloud_watch_logs_role_arn: Option<String>,
    pub kms_key_id: Option<String>,
    pub is_organization_trail: bool,
    pub recursive_logging: bool,
    pub home_region: String,
    /// What `PutEventSelectors` configured, or `None` for a trail nothing has
    /// configured — which logs by [`default_event_selector`] and reports
    /// `HasCustomEventSelectors` false.
    pub event_selectors: Option<EventSelectorConfig>,
    /// Set by `PutInsightSelectors`, which is not implemented yet, so a trail is
    /// created without Insights selectors.
    pub has_insight_selectors: bool,
    /// The event aggregation this trail is configured with, as
    /// `PutEventConfiguration` sets it and `GetEventConfiguration` reports it.
    /// Empty for a trail nobody has configured aggregation for; the model bounds
    /// `AggregationConfigurations` to 0..=1 entries.
    pub aggregation_configurations: Vec<AggregationConfiguration>,
    pub tags: Vec<Tag>,
    pub status: TrailStatus,
}

impl TrailRecord {
    /// Project onto the modelled `Trail` shape, as `GetTrail` and `DescribeTrails`
    /// report it.
    pub fn to_trail(&self) -> Trail {
        Trail {
            name: Some(self.name.clone()),
            s3_bucket_name: Some(self.s3_bucket_name.clone()),
            s3_key_prefix: self.s3_key_prefix.clone(),
            // The model deprecates this member — "This field is no longer in use.
            // Use SnsTopicARN." — so only the ARN is reported.
            sns_topic_name: None,
            sns_topic_arn: self.sns_topic_arn.clone(),
            include_global_service_events: Some(self.include_global_service_events),
            is_multi_region_trail: Some(self.is_multi_region_trail),
            home_region: Some(self.home_region.clone()),
            trail_arn: Some(self.trail_arn.clone()),
            log_file_validation_enabled: Some(self.log_file_validation_enabled),
            cloud_watch_logs_log_group_arn: self.cloud_watch_logs_log_group_arn.clone(),
            cloud_watch_logs_role_arn: self.cloud_watch_logs_role_arn.clone(),
            kms_key_id: self.kms_key_id.clone(),
            // "Specifies if the trail has custom event selectors" — true exactly
            // when something configured them, so it is derived from the stored
            // configuration rather than tracked beside it.
            has_custom_event_selectors: Some(self.event_selectors.is_some()),
            has_insight_selectors: Some(self.has_insight_selectors),
            is_organization_trail: Some(self.is_organization_trail),
            recursive_logging: Some(self.recursive_logging),
        }
    }

    /// Project onto the modelled `TrailInfo` shape, as `ListTrails` reports it.
    pub fn to_trail_info(&self) -> TrailInfo {
        TrailInfo {
            trail_arn: Some(self.trail_arn.clone()),
            name: Some(self.name.clone()),
            home_region: Some(self.home_region.clone()),
        }
    }

    /// The event selectors `GetEventSelectors` and `PutEventSelectors` report for this
    /// trail, as the `(EventSelectors, AdvancedEventSelectors)` pair of their output.
    ///
    /// Exactly one of the two is reported, because a trail carries one kind: the
    /// advanced selectors when advanced selectors are configured, otherwise the basic
    /// ones — which for a trail nothing has configured is the single
    /// [`default_event_selector`].
    pub fn reported_event_selectors(
        &self,
    ) -> (Option<Vec<EventSelector>>, Option<Vec<AdvancedEventSelector>>) {
        match &self.event_selectors {
            Some(EventSelectorConfig::Basic(selectors)) => (Some(selectors.clone()), None),
            Some(EventSelectorConfig::Advanced(selectors)) => (None, Some(selectors.clone())),
            None => (Some(vec![default_event_selector()]), None),
        }
    }

    /// Whether a request made in `region` sees this trail.
    ///
    /// A trail always shows in its home Region. A multi-Region trail also has a
    /// shadow copy in every other Region, which `DescribeTrails` includes unless the
    /// caller sets `includeShadowTrails` to false.
    pub fn visible_from(&self, region: &str, include_shadow_trails: bool) -> bool {
        self.home_region == region || (include_shadow_trails && self.is_multi_region_trail)
    }
}

/// Why a caller-supplied trail identifier could not be read at all.
///
/// Each variant names the modelled error the handler answers with, so every
/// operation refuses the same malformed identifier the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierRejection {
    /// Shaped like an ARN but not a CloudTrail trail ARN ⇒
    /// `CloudTrailARNInvalidException` (HTTP 400).
    InvalidArn(String),
    /// A bare name outside the documented trail-name rules ⇒
    /// `InvalidTrailNameException` (HTTP 400).
    InvalidName(String),
}

impl IdentifierRejection {
    /// The refusal text: built from the request, naming the value and the rule it
    /// broke, so a caller can fix the request from it alone.
    pub fn message(self) -> String {
        match self {
            Self::InvalidArn(message) | Self::InvalidName(message) => message,
        }
    }
}

/// A caller-supplied trail identifier, in either of the two forms every trail
/// operation accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrailIdentifier {
    /// A bare trail name, which names a trail in the caller's own Region.
    Name(String),
    /// A trail ARN, which also names the Region the trail is homed in.
    Arn { region: String, name: String },
}

impl TrailIdentifier {
    /// The trail name this identifier names — the registry's key.
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Arn { name, .. } => name,
        }
    }

    /// The home Region this identifier pins the trail to, if it pins one.
    pub fn region(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Arn { region, .. } => Some(region),
        }
    }
}

/// The Region this request is served in: the SigV4 credential scope's Region,
/// falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the trail called `name` homed in `region`.
pub fn trail_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{TRAIL_RESOURCE_PREFIX}{name}"),
    )
}

/// The ARN of the SNS topic `CreateTrail` was given.
///
/// `SnsTopicName` is documented as "the name or ARN of the Amazon SNS topic", so an
/// ARN is kept as given and a bare name is completed into one.
pub fn sns_topic_arn(region: &str, topic: &str) -> String {
    if topic.starts_with("arn:") {
        topic.to_string()
    } else {
        ::engine::emulator_arn("sns", region, topic)
    }
}

/// The refusal text for a well-formed identifier that names no trail, shared by
/// every operation that answers `TrailNotFoundException`.
///
/// It names the identifier EXACTLY as the caller wrote it — a bare name or a trail
/// ARN — and the account the lookup ran in, because a trail name is scoped to one
/// account and Region, so "no trail of this name here" is the whole refusal.
pub fn unknown_trail_message(identifier: &str) -> String {
    format!(
        "Unknown trail: {identifier} for the user: {}",
        ::engine::emulator_account_id()
    )
}

/// Check a bare trail name against the rules the model documents for
/// `CreateTrailRequest$Name`:
///
/// * only ASCII letters, digits, periods, underscores or dashes,
/// * starts and ends with a letter or digit,
/// * between [`TRAIL_NAME_MIN_LEN`] and [`TRAIL_NAME_MAX_LEN`] characters,
/// * no two adjacent periods, underscores or dashes,
/// * not in IP-address format.
///
/// A refusal names the ONE rule the name broke — and, where that rule is a length
/// bound, the bound and the name's own length — rather than restating every rule, so
/// the caller can fix the name from the refusal alone. The rules are checked in the
/// order they are listed above, so a name that breaks several is refused for the
/// first.
pub fn validate_trail_name(name: &str) -> Result<(), IdentifierRejection> {
    let reject = |message: String| Err(IdentifierRejection::InvalidName(message));

    let length = name.chars().count();
    if length < TRAIL_NAME_MIN_LEN {
        return reject(format!(
            "Trail name too short. Minimum allowed length: {TRAIL_NAME_MIN_LEN} characters. \
             Specified name length: {length} characters."
        ));
    }
    if length > TRAIL_NAME_MAX_LEN {
        return reject(format!(
            "Trail name too long. Maximum allowed length: {TRAIL_NAME_MAX_LEN} characters. \
             Specified name length: {length} characters."
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || TRAIL_NAME_PUNCTUATION.contains(&c))
    {
        return reject(
            "Trail name or ARN can only contain ASCII letters (a-z, A-Z), numbers (0-9), \
             periods (.), underscores (_), or dashes (-)."
                .to_string(),
        );
    }
    // The two edges are separate rules, so a refusal names the edge that broke.
    if name
        .chars()
        .next()
        .is_some_and(|c| !c.is_ascii_alphanumeric())
    {
        return reject("Trail name must start with a letter or number.".to_string());
    }
    if name
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_ascii_alphanumeric())
    {
        return reject("Trail name must end with a letter or number.".to_string());
    }
    let punctuation = |c: &char| TRAIL_NAME_PUNCTUATION.contains(c);
    if name
        .chars()
        .zip(name.chars().skip(1))
        .any(|(a, b)| punctuation(&a) && punctuation(&b))
    {
        return reject(
            "Trail name or ARN cannot have adjacent periods (.), hyphens (-), or \
             underscores (_)."
                .to_string(),
        );
    }
    // "Not be in IP address format (for example, 192.168.5.4)" — a dotted quad is
    // exactly what `Ipv4Addr`'s parser accepts, and a name cannot hold the colons an
    // IPv6 literal needs.
    if name.parse::<Ipv4Addr>().is_ok() {
        return reject("Trail names must not be formatted as an IP address.".to_string());
    }
    Ok(())
}

/// Read a caller-supplied identifier as either a trail ARN or a bare trail name.
///
/// A value that starts with `arn:` is read as an ARN — so a malformed ARN is
/// refused as one rather than as a bad name — and anything else as a bare name.
pub fn parse_trail_identifier(value: &str) -> Result<TrailIdentifier, IdentifierRejection> {
    if value.starts_with("arn:") {
        return parse_trail_arn(value);
    }
    validate_trail_name(value)?;
    Ok(TrailIdentifier::Name(value.to_string()))
}

/// Read `arn:<partition>:cloudtrail:<region>:<account>:trail/<name>`.
fn parse_trail_arn(value: &str) -> Result<TrailIdentifier, IdentifierRejection> {
    let reject = || {
        Err(IdentifierRejection::InvalidArn(format!(
            "'{value}' is not a valid trail ARN: the format of a trail ARN is \
             arn:aws:cloudtrail:<region>:<account-id>:trail/<trail-name>"
        )))
    };

    let fields: Vec<&str> = value.split(':').collect();
    let [arn, partition, service, region, account, resource] = fields.as_slice() else {
        return reject();
    };
    if *arn != "arn"
        || partition.is_empty()
        || *service != SERVICE_NAMESPACE
        || region.is_empty()
        || account.is_empty()
    {
        return reject();
    }
    let Some(name) = resource.strip_prefix(TRAIL_RESOURCE_PREFIX) else {
        return reject();
    };
    if validate_trail_name(name).is_err() {
        return reject();
    }
    Ok(TrailIdentifier::Arn {
        region: (*region).to_string(),
        name: name.to_string(),
    })
}

/// Render an instant the way the modelled string timestamp members carry it
/// (ISO 8601, e.g. `2026-10-21T21:38:12Z`).
pub fn iso8601(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The `ImportStatus` values the model's enum declares
/// (`smithy_query(query="shape",shape="ImportStatus")`): `INITIALIZING | IN_PROGRESS |
/// FAILED | STOPPED | COMPLETED`.
pub const IMPORT_STATUS_INITIALIZING: &str = "INITIALIZING";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_STOPPED: &str = "STOPPED";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_COMPLETED: &str = "COMPLETED";
/// See [`IMPORT_STATUS_INITIALIZING`].
pub const IMPORT_STATUS_FAILED: &str = "FAILED";

/// The statuses an import can still leave. The other three — `COMPLETED`, `FAILED`
/// and `STOPPED` — are where an import ends, so nothing moves them.
pub const RUNNING_IMPORT_STATUSES: [&str; 2] =
    [IMPORT_STATUS_INITIALIZING, IMPORT_STATUS_IN_PROGRESS];

/// The HTTP status this service answers `ImportNotFoundException` with.
///
/// The model annotates the shape `@httpError(404)`
/// (`smithy_query(query="shape",shape="ImportNotFoundException")`), but `@httpError`
/// is an HTTP-BINDING trait and the recorded CloudTrail responses to an import
/// operation addressing an unknown import id carry HTTP **400** with
/// `__type: ImportNotFoundException` — the status a client's retry classifier sees is
/// the service's, not the model's. A handler answers it through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`], which keeps the modelled
/// shape's body and changes only the status line.
pub const IMPORT_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// The refusal text for an import id that names no import.
///
/// Built from the request so a caller can tell WHICH id was not found, in the wording
/// the recorded CloudTrail responses use ("The import job <id> was not found.").
pub fn import_not_found_message(import_id: &str) -> String {
    format!("The import job {import_id} was not found.")
}

/// One persisted import job: the request `StartImport` was given, the status the job
/// is in, and the two instants the service stamps on it.
///
/// `statistics` is `None` while nothing has been imported: this emulator reads no S3
/// bucket, so an import reports no counts rather than counts nothing produced.
#[derive(Debug, Clone)]
pub struct ImportRecord {
    pub import_id: String,
    /// The destination event data store ARNs (`ImportDestinations`, length 1..1).
    pub destinations: Vec<String>,
    pub import_source: Option<ImportSource>,
    pub start_event_time: Option<SystemTime>,
    pub end_event_time: Option<SystemTime>,
    /// One of the [`IMPORT_STATUS_INITIALIZING`] family.
    pub status: String,
    pub created_timestamp: SystemTime,
    pub updated_timestamp: SystemTime,
    pub statistics: Option<ImportStatistics>,
}

impl ImportRecord {
    /// Whether this import is still running, so a stop has something to stop.
    pub fn is_running(&self) -> bool {
        RUNNING_IMPORT_STATUSES.contains(&self.status.as_str())
    }

    /// Stop this import.
    ///
    /// A running import moves to `STOPPED` and its `UpdatedTimestamp` becomes `at`.
    /// An import that already reached a terminal status is left exactly as it is —
    /// there is nothing left to stop — so a repeated stop reports the same record.
    pub fn stop(&mut self, at: SystemTime) {
        if self.is_running() {
            self.status = IMPORT_STATUS_STOPPED.to_string();
            self.updated_timestamp = at;
        }
    }
}

/// The import registry key: an import id and the Region the import was created in.
///
/// CloudTrail is a regional service — an import belongs to the Region its destination
/// event data store lives in — so an import is reachable only from that Region, the
/// same way [`TrailKey`] scopes a trail name. The id comes FIRST so the `BTreeMap`
/// iterates id-ordered, which is the order `ListImports` pages through.
pub type ImportKey = (String, String);

/// The first tag key that appears more than once in `tags`, if any.
///
/// `InvalidTagParameterException` covers duplicate tags, and a tag set is a map on
/// the wire even though the model carries it as a list — so the rule holds for every
/// taggable resource, not just one of them.
pub fn first_duplicate_tag_key(tags: &[Tag]) -> Option<&str> {
    let mut seen = ::std::collections::BTreeSet::new();
    tags.iter()
        .find(|tag| !seen.insert(tag.key.as_str()))
        .map(|tag| tag.key.as_str())
}

/// The registry key: a trail name and the Region the trail is homed in.
///
/// AWS scopes a trail name to the Region it was created in, so the name alone is
/// not a key: two Regions may each hold a trail of the same name, and each is
/// reachable only from its own Region (or by its own ARN). The name comes FIRST so
/// the `BTreeMap` iterates name-ordered, which is the order `DescribeTrails` and
/// `ListTrails` report and the order `ListTrails` pages through.
pub type TrailKey = (String, String);

/// The refusal text for `EventDataStoreNotFoundException`.
///
/// It names no value from the request because the service's own text does not: the
/// store the caller asked for is exactly what could not be found.
pub const EVENT_DATA_STORE_NOT_FOUND_MESSAGE: &str = "The event data store was not found.";

/// The refusal text for `EventDataStoreNotFoundException` on a call that names ONE
/// store, as the recorded response of the real service words it: it names the ARN
/// the request addressed, which for a bare id is the one that id completes to in
/// the caller's Region.
pub fn event_data_store_not_found_message(arn: &str) -> String {
    format!("The event data store {arn} was not found.")
}

/// The refusal text for `QueryIdNotFoundException`: it names the member the
/// request supplied and its value, the way the service words it — "The QueryId
/// value <id> does not exist or is not in the same region."
pub fn query_not_found_message(member: &str, value: &str) -> String {
    format!("The {member} value {value} does not exist or is not in the same region.")
}

/// The resource prefix of an event data store ARN:
/// `arn:aws:cloudtrail:<region>:<account>:eventdatastore/<id>`.
const EVENT_DATA_STORE_RESOURCE_PREFIX: &str = "eventdatastore/";

/// The status of an event data store: one variant per value of the model's
/// `EventDataStoreStatus` enum (`ops::types::EVENT_DATA_STORE_STATUS_VALUES`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum EventDataStoreState {
    Created,
    #[default]
    Enabled,
    PendingDeletion,
    StartingIngestion,
    StoppingIngestion,
    StoppedIngestion,
}

impl EventDataStoreState {
    /// The value this state carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Enabled => "ENABLED",
            Self::PendingDeletion => "PENDING_DELETION",
            Self::StartingIngestion => "STARTING_INGESTION",
            Self::StoppingIngestion => "STOPPING_INGESTION",
            Self::StoppedIngestion => "STOPPED_INGESTION",
        }
    }

    /// Whether a query operation may name this store.
    ///
    /// A store awaiting deletion holds no queryable lake any more, which is what
    /// the modelled `InactiveEventDataStoreException` ("The event data store is
    /// inactive") reports. A store that has stopped ingesting new events still
    /// serves queries over what it already holds, so only `PENDING_DELETION` is
    /// inactive for this purpose.
    pub fn is_queryable(self) -> bool {
        !matches!(self, Self::PendingDeletion)
    }
}

/// The billing mode of an event data store: one variant per value of the model's
/// `BillingMode` enum.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum EventDataStoreBillingMode {
    /// The model doc for `CreateEventDataStoreRequest$BillingMode`: "The default
    /// value is EXTENDABLE_RETENTION_PRICING."
    #[default]
    ExtendableRetentionPricing,
    FixedRetentionPricing,
}

impl EventDataStoreBillingMode {
    /// The value this billing mode carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ExtendableRetentionPricing => "EXTENDABLE_RETENTION_PRICING",
            Self::FixedRetentionPricing => "FIXED_RETENTION_PRICING",
        }
    }
}

/// The Lake query federation state of an event data store: one variant per value
/// of the model's `FederationStatus` enum.
///
/// `Disabled` is the default because a store federates only once
/// `EnableFederation` is called for it: "The status is ENABLED if Lake query
/// federation is enabled, or DISABLED if Lake query federation is disabled"
/// (model doc for `GetEventDataStoreResponse$FederationStatus`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FederationState {
    Enabling,
    Enabled,
    Disabling,
    #[default]
    Disabled,
}

impl FederationState {
    /// The value this federation state carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Enabling => "ENABLING",
            Self::Enabled => "ENABLED",
            Self::Disabling => "DISABLING",
            Self::Disabled => "DISABLED",
        }
    }

    /// Whether Lake query federation is on, or on its way on — the two states
    /// `DisableFederation` has something to turn off.
    pub fn is_on(self) -> bool {
        matches!(self, Self::Enabling | Self::Enabled)
    }

    /// The state this one settles into once the transition it names has run, or
    /// `None` for a state the clock owes nothing on.
    ///
    /// `ENABLING` and `DISABLING` exist in the model precisely because the
    /// Enable/Disable pair is asynchronous; `ENABLED` and `DISABLED` are terminal.
    pub fn settled(self) -> Option<Self> {
        match self {
            Self::Enabling => Some(Self::Enabled),
            Self::Disabling => Some(Self::Disabled),
            Self::Enabled | Self::Disabled => None,
        }
    }
}

/// The maximum size an event stored in an event data store may have: one variant
/// per value of the model's `MaxEventSize` enum
/// (`ops::types::MAX_EVENT_SIZE_VALUES`).
///
/// `Standard` is the default because `Large` is reachable only by asking for it:
/// "If you are using context key selectors, MaxEventSize must be set to Large"
/// (model doc for `PutEventConfigurationRequest$MaxEventSize`), so a store nobody
/// has configured carries the smaller limit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MaxEventSizeSetting {
    #[default]
    Standard,
    Large,
}

impl MaxEventSizeSetting {
    /// The value this setting carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Large => "Large",
        }
    }

    /// Read a wire value of the model's `MaxEventSize` enum.
    ///
    /// `None` is a value outside the enum, which the boundary validator refuses
    /// before a handler sees it.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Standard" => Some(Self::Standard),
            "Large" => Some(Self::Large),
            _ => None,
        }
    }
}

/// How long an event data store reads `ENABLING` / `DISABLING` before the clock
/// settles it into `ENABLED` / `DISABLED`.
///
/// The model's `FederationStatus` enum carries both in-flight values, so
/// `EnableFederation` and `DisableFederation` ARE asynchronous. CloudTrail
/// publishes no duration for either, so this span is the emulator's own: short
/// enough not to stall a workflow, non-zero so a caller that reads the store
/// straight after the call still sees the transient status.
pub const FEDERATION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// One persisted CloudTrail Lake event data store.
///
/// The ARN is not stored: it is a function of the Region and the id
/// ([`event_data_store_arn`]), so deriving it keeps one source of truth. Every
/// other member a read reports IS stored, so a read computes its whole answer from
/// this record rather than from the request that asked for it.
#[derive(Debug, Default, Clone)]
pub struct EventDataStoreRecord {
    /// The ARN's resource suffix — the identifier every operation accepts in
    /// place of the full ARN.
    pub id: String,
    pub name: String,
    /// The Region the store lives in; it is reachable only from there.
    pub region: String,
    pub status: EventDataStoreState,
    /// The account the store belongs to, or `None` for the emulator's own
    /// account. A cross-account query echoes it back as
    /// `EventDataStoreOwnerAccountId`.
    pub owner_account_id: Option<String>,
    pub created_at: Option<SystemTime>,
    /// When the store was last changed. "UpdatedTimestamp is always either the
    /// same or newer than the time shown in CreatedTimestamp" (model doc for
    /// `GetEventDataStoreResponse$UpdatedTimestamp`), and `None` for a store
    /// nothing has changed since it was created.
    pub updated_at: Option<SystemTime>,
    /// The selectors that choose which events the store collects.
    pub advanced_event_selectors: Vec<AdvancedEventSelector>,
    pub multi_region_enabled: bool,
    pub organization_enabled: bool,
    /// In days; the model bounds `RetentionPeriod` to 7..=3653. `None` for a store
    /// whose retention nothing has stated.
    pub retention_period: Option<i32>,
    pub termination_protection_enabled: bool,
    pub kms_key_id: Option<String>,
    pub billing_mode: EventDataStoreBillingMode,
    pub federation_status: FederationState,
    /// When an in-flight `federation_status` (`ENABLING` / `DISABLING`) is due to
    /// settle into the state it is heading for; `None` once it has, and for a
    /// store whose federation is not moving.
    pub federation_settles_at: Option<SystemTime>,
    /// The role Lake query federation accesses the store's resources through; set
    /// only while federation is on.
    pub federation_role_arn: Option<String>,
    /// The keys CloudTrail Lake partitions the store's event data by. The model
    /// bounds `PartitionKeyList` to 0..=2 entries.
    pub partition_keys: Vec<PartitionKey>,
    /// The largest event the store accepts, as `PutEventConfiguration` sets it
    /// and `GetEventConfiguration` reports it.
    pub max_event_size: MaxEventSizeSetting,
    /// The context keys CloudTrail includes in this store's enriched events, as
    /// `PutEventConfiguration` sets them and `GetEventConfiguration` reports
    /// them. Empty for a store nobody has configured; the model bounds
    /// `ContextKeySelectors` to 0..=2 entries.
    pub context_key_selectors: Vec<ContextKeySelector>,
    pub tags: Vec<Tag>,
}

impl EventDataStoreRecord {
    /// The account that owns this store: the one it was created for, defaulting
    /// to the emulator's own account.
    pub fn owner(&self) -> &str {
        match self.owner_account_id.as_deref() {
            Some(account_id) => account_id,
            None => ::engine::emulator_account_id(),
        }
    }

    /// This store's ARN, derived from the Region it lives in and its id.
    pub fn arn(&self) -> String {
        event_data_store_arn(&self.region, &self.id)
    }

    /// Materialise the federation transition the clock already owes: `ENABLING`
    /// into `ENABLED`, `DISABLING` into `DISABLED`.
    ///
    /// A reporting operation may call this: the transition was due at
    /// `federation_settles_at` whether or not anybody read the store, so applying
    /// it changes nothing a later caller would otherwise have seen. Federation
    /// reaching `DISABLED` releases the role it ran under — that role is held
    /// "only while federation is on".
    pub fn settle_federation(&mut self, now: SystemTime) {
        let Some(due) = self.federation_settles_at else {
            return;
        };
        if now < due {
            return;
        }
        self.federation_settles_at = None;
        if let Some(settled) = self.federation_status.settled() {
            self.federation_status = settled;
            if !settled.is_on() {
                self.federation_role_arn = None;
            }
        }
    }
}

/// A Lake query's lifecycle state: one variant per value of the model's
/// `QueryStatus` enum (`ops::types::QUERY_STATUS_VALUES`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    #[default]
    Queued,
    Running,
    Finished,
    Failed,
    Cancelled,
    TimedOut,
}

impl QueryState {
    /// The value this state carries on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Running => "RUNNING",
            Self::Finished => "FINISHED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::TimedOut => "TIMED_OUT",
        }
    }

    /// Whether the run has stopped moving: no later call changes this state.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

/// Where a query's results were delivered.
///
/// The URI and the delivery status are one value because the model reports them
/// as a pair — "If the query results were delivered to an S3 bucket, the response
/// also provides the S3 URI and the delivery status" — so neither can be stored
/// without the other.
#[derive(Debug, Clone)]
pub struct QueryDelivery {
    pub s3_uri: String,
    /// One of the model's `DeliveryStatus` values
    /// (`ops::types::DELIVERY_STATUS_VALUES`).
    pub status: String,
}

/// What a query run accumulated, as `DescribeQuery` reports it.
#[derive(Debug, Default, Clone)]
pub struct QueryStatisticsRecord {
    pub events_matched: i64,
    pub events_scanned: i64,
    pub bytes_scanned: i64,
    /// `None` until the run ends: a run time exists only once there is a run to
    /// have taken it.
    pub execution_time_in_millis: Option<i32>,
}

/// One persisted Lake query — everything `StartQuery` was given plus what the run
/// produced, which is what `DescribeQuery` reports.
#[derive(Debug, Default, Clone)]
pub struct QueryRecord {
    pub query_id: String,
    /// The id of the event data store the query ran on.
    pub event_data_store_id: String,
    /// The Region the query ran in; it is reachable only from there.
    pub region: String,
    pub query_string: String,
    /// The query template alias this run was started under, if any.
    pub query_alias: Option<String>,
    /// The dashboard refresh this run belongs to, if any.
    pub refresh_id: Option<String>,
    /// The natural-language prompt a generated query came from, if any.
    pub prompt: Option<String>,
    pub status: QueryState,
    pub statistics: QueryStatisticsRecord,
    pub delivery: Option<QueryDelivery>,
    /// Why the run failed; set only by a run that ended in failure.
    pub error_message: Option<String>,
    /// The account owning the event data store, for a cross-account query.
    pub event_data_store_owner_account_id: Option<String>,
    /// When `StartQuery` minted the query: the `CreationTime` of the statistics
    /// `DescribeQuery` reports.
    pub created_at: Option<SystemTime>,
}

impl QueryRecord {
    /// Whether a call made in `region` naming `event_data_store_id` (when it names
    /// one) addresses this query.
    pub fn addressed_by(&self, region: &str, event_data_store_id: Option<&str>) -> bool {
        self.region == region
            && event_data_store_id.is_none_or(|store| self.event_data_store_id == store)
    }
}

/// Why a caller-supplied event data store identifier could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventDataStoreRejection {
    /// Readable as neither an event data store ARN nor the bare id suffix of one
    /// ⇒ `EventDataStoreARNInvalidException` (HTTP 400). Carries the value as the
    /// request gave it, which is what the refusal names.
    InvalidArn(String),
}

impl EventDataStoreRejection {
    /// The refusal text: built from the request, naming the ARN it addressed, so a
    /// caller can fix the request from it alone. See
    /// [`event_data_store_arn_invalid_message`] for the wording and why a bare id
    /// needs the Region the call was made in.
    pub fn message(self, request_region: &str) -> String {
        match self {
            Self::InvalidArn(value) => {
                event_data_store_arn_invalid_message(&value, request_region)
            }
        }
    }
}

/// A caller-supplied event data store identifier, in either of the two forms the
/// model documents: "The ARN (or the ID suffix of the ARN) of an event data store".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDataStoreIdentifier {
    /// The store id — the ARN's resource suffix.
    pub id: String,
    /// The Region the ARN pins the store to; `None` for a bare id, which names a
    /// store in the caller's own Region.
    pub region: Option<String>,
}

impl EventDataStoreIdentifier {
    /// The Region this identifier addresses a store in: the ARN's own Region, or
    /// the Region the call was made in for a bare id.
    pub fn addressed_region<'a>(&'a self, request_region: &'a str) -> &'a str {
        self.region.as_deref().unwrap_or(request_region)
    }

    /// The whole ARN this identifier addresses — the ARN as given, or the one a
    /// bare id completes to in the caller's Region.
    pub fn arn(&self, request_region: &str) -> String {
        event_data_store_arn(self.addressed_region(request_region), &self.id)
    }
}

/// The ARN of the event data store `id` in `region`.
pub fn event_data_store_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{EVENT_DATA_STORE_RESOURCE_PREFIX}{id}"),
    )
}

/// Read a caller-supplied event data store identifier as either an event data
/// store ARN or the bare id suffix of one.
///
/// A value carrying a `:` is read as an ARN — so a malformed ARN is refused as one
/// rather than taken for an id — and anything else as a bare id.
pub fn parse_event_data_store_identifier(
    value: &str,
) -> Result<EventDataStoreIdentifier, EventDataStoreRejection> {
    if !value.contains(':') {
        // The id suffix form, with or without the `eventdatastore/` resource
        // prefix the full ARN carries.
        let id = value
            .strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX)
            .unwrap_or(value);
        if id.is_empty() {
            return Err(EventDataStoreRejection::InvalidArn(value.to_string()));
        }
        return Ok(EventDataStoreIdentifier {
            id: id.to_string(),
            region: None,
        });
    }

    let fields: Vec<&str> = value.split(':').collect();
    let [arn, partition, service, region, account, resource] = fields.as_slice() else {
        return Err(EventDataStoreRejection::InvalidArn(value.to_string()));
    };
    let id = resource.strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX);
    match id {
        Some(id)
            if *arn == "arn"
                && !partition.is_empty()
                && *service == SERVICE_NAMESPACE
                && !region.is_empty()
                && !account.is_empty()
                && !id.is_empty() =>
        {
            Ok(EventDataStoreIdentifier {
                id: id.to_string(),
                region: Some((*region).to_string()),
            })
        }
        _ => Err(EventDataStoreRejection::InvalidArn(value.to_string())),
    }
}

/// The text an unreadable event data store identifier is refused with, worded the
/// way the real service words it: "The event data store ARN <arn> format is not
/// valid."
///
/// It names the ARN the request addressed rather than the characters it typed:
/// a value that is ARN-shaped is named as given, and a bare id is completed into
/// the ARN it names in the Region the call was made in — which is why the Region
/// is a parameter. Recorded CloudTrail responses carry both forms: an identifier
/// that is not a CloudTrail ARN is echoed whole, and a bare id that cannot be read
/// comes back as `arn:aws:cloudtrail:<request-region>:<account>:eventdatastore/<id>`.
pub fn event_data_store_arn_invalid_message(value: &str, request_region: &str) -> String {
    let addressed = if value.contains(':') {
        value.to_string()
    } else {
        let id = value
            .strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX)
            .unwrap_or(value);
        event_data_store_arn(request_region, id)
    };
    format!("The event data store ARN {addressed} format is not valid.")
}

// ---------------------------------------------------------------------------
// Event configuration (`PutEventConfiguration` / `GetEventConfiguration`)
// ---------------------------------------------------------------------------

/// Which resource an event-configuration call addresses.
///
/// `TrailName` and `EventDataStore` are both optional in the model and name
/// different resources, so the two operations that carry them —
/// `PutEventConfiguration` and `GetEventConfiguration` — read the pair the same
/// way: exactly one of them addresses the call's resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventConfigurationTarget {
    /// The call addresses a trail, by name or by trail ARN.
    Trail(String),
    /// The call addresses an event data store, by ARN or by the id suffix of
    /// one.
    EventDataStore(String),
}

/// Why a request names no single event-configuration resource.
///
/// Both halves of the rule — naming neither resource and naming both — are the
/// modelled `InvalidParameterCombinationException` ("This exception is thrown
/// when the combination of parameters provided is not valid"), worded the way the
/// recorded response of the real service words it, so the rejection carries only
/// that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventConfigurationTargetRejection(pub String);

impl EventConfigurationTargetRejection {
    /// The refusal text: it states the rule the request broke.
    pub fn message(self) -> String {
        self.0
    }
}

/// The refusal text for a call that does not address exactly one resource, as the
/// recorded response of the real service words it.
pub const NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE: &str =
    "You must specify either a trail or an event data store.";

/// Read the resource an event-configuration call addresses from the `TrailName`
/// / `EventDataStore` pair.
pub fn event_configuration_target(
    trail_name: Option<&str>,
    event_data_store: Option<&str>,
) -> Result<EventConfigurationTarget, EventConfigurationTargetRejection> {
    match (trail_name, event_data_store) {
        (Some(name), None) => Ok(EventConfigurationTarget::Trail(name.to_string())),
        (None, Some(store)) => Ok(EventConfigurationTarget::EventDataStore(store.to_string())),
        // Neither resource leaves the call with nothing to address; both address
        // two resources of different kinds. Either way the pair cannot hold
        // together.
        (Some(_), Some(_)) | (None, None) => Err(EventConfigurationTargetRejection(
            NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE.to_string(),
        )),
    }
}

/// The refusal text for a `PutEventConfiguration` call that addresses an event
/// data store without stating the configuration to apply to it, as the recorded
/// response of the real service words it.
pub const MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE: &str =
    "You must specify the maximum event size and context key selectors.";

/// The refusal text for a `PutEventConfiguration` call that addresses a trail
/// without stating the configuration to apply to it. `AggregationConfigurations`
/// is the only trail-level setting this operation carries ("The list of
/// aggregation configurations that you want to configure for the trail").
pub const MISSING_TRAIL_CONFIGURATION_MESSAGE: &str =
    "You must specify the aggregation configurations.";

/// Per-service state for com-amazonaws-cloudtrail-cloudtrail-20131101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudtrailCloudtrail20131101State {
    /// Every trail in the account, keyed by [`TrailKey`].
    pub trails: BTreeMap<TrailKey, TrailRecord>,
    /// Every import job in the account, keyed by [`ImportKey`]. `StartImport` adds a
    /// record; `StopImport` moves a running one to `STOPPED`.
    ///
    /// Unbounded here on purpose: `StartImport` is the only operation that adds a
    /// record and it is still a scaffold stub, so the bound belongs with it — on the
    /// terms [`TRAIL_QUOTA`] and [`DASHBOARD_QUOTA`] state, a whole-registry cap
    /// refused with that operation's own modelled error.
    pub imports: BTreeMap<ImportKey, ImportRecord>,
    /// Every CloudTrail Lake event data store, keyed by its id (the resource
    /// suffix of its ARN), which is unique across Regions.
    pub event_data_stores: BTreeMap<String, EventDataStoreRecord>,
    /// Every Lake query run, keyed by its query id.
    pub queries: BTreeMap<String, QueryRecord>,
    /// Every CloudTrail Lake dashboard in the account, keyed by [`DashboardKey`].
    pub dashboards: BTreeMap<DashboardKey, DashboardRecord>,
}

/// What a mutating operation found for the identifier it was given.
///
/// The three failing variants are what keeps `TrailNotFoundException` (no such
/// trail anywhere) apart from `InvalidHomeRegionException` (the trail exists, but
/// not in the Region this call can drive it from).
pub enum TrailForMutation<'a> {
    /// The trail, homed in the caller's own Region.
    Found(&'a mut TrailRecord),
    /// The identifier pinned a Region other than the caller's — a trail ARN from
    /// another Region, which is how a shadow trail is addressed.
    AddressedInAnotherRegion { name: String, addressed: String },
    /// A trail of this name exists, but it is homed in another Region.
    HomedInAnotherRegion { name: String, home_region: String },
    /// No trail of this name in any Region.
    Absent,
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// The registry key a caller-supplied identifier addresses.
    ///
    /// An identifier names the trail of that name in the Region the identifier
    /// pins: the ARN's own Region, or — for a bare name — `request_region`, the
    /// Region the call was made in. That is why the status of a shadow trail (a
    /// multi-Region trail's replica in another Region) is reachable only by ARN.
    pub fn addressed_key(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<TrailKey, IdentifierRejection> {
        let parsed = parse_trail_identifier(identifier)?;
        Ok((
            parsed.name().to_string(),
            parsed.region().unwrap_or(request_region).to_string(),
        ))
    }

    /// Resolve a caller-supplied identifier to a stored trail.
    ///
    /// `Err` is a malformed identifier (refuse the request), `Ok(None)` a
    /// well-formed one naming no trail (`TrailNotFoundException`).
    pub fn resolve(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<Option<&TrailRecord>, IdentifierRejection> {
        let key = self.addressed_key(identifier, request_region)?;
        Ok(self.trails.get(&key))
    }

    /// Every Region holding a trail of this name, in Region order.
    pub fn home_regions_of(&self, name: &str) -> Vec<String> {
        self.trails
            .range((name.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == name)
            .map(|((_, region), _)| region.clone())
            .collect()
    }

    /// Resolve a caller-supplied identifier for an operation that CHANGES the
    /// trail, which runs only in the trail's home Region.
    pub fn resolve_for_mutation(
        &mut self,
        identifier: &str,
        request_region: &str,
    ) -> Result<TrailForMutation<'_>, IdentifierRejection> {
        let (name, addressed) = self.addressed_key(identifier, request_region)?;
        if addressed != request_region {
            return Ok(TrailForMutation::AddressedInAnotherRegion { name, addressed });
        }
        // Read where the name lives BEFORE taking the mutable borrow, so the
        // lookup below is the only one.
        let elsewhere = self.home_regions_of(&name);
        match self.trails.get_mut(&(name.clone(), addressed)) {
            Some(record) => Ok(TrailForMutation::Found(record)),
            None => Ok(match elsewhere.into_iter().next() {
                Some(home_region) => TrailForMutation::HomedInAnotherRegion { name, home_region },
                None => TrailForMutation::Absent,
            }),
        }
    }

    /// The import `import_id` names in `region`, for an operation that only reports
    /// it.
    pub fn import(&self, import_id: &str, region: &str) -> Option<&ImportRecord> {
        self.imports
            .get(&(import_id.to_string(), region.to_string()))
    }

    /// The import `import_id` names in `region`, for an operation that CHANGES it.
    ///
    /// `None` is a well-formed id naming no import the caller's Region holds, which
    /// every import operation answers with `ImportNotFoundException`.
    pub fn import_mut(&mut self, import_id: &str, region: &str) -> Option<&mut ImportRecord> {
        self.imports
            .get_mut(&(import_id.to_string(), region.to_string()))
    }

    /// Whether this Region holds any event data store at all — what a request
    /// that names none resolves against.
    pub fn has_event_data_store_in(&self, region: &str) -> bool {
        self.event_data_stores
            .values()
            .any(|record| record.region == region)
    }

    /// Whether this Region holds an event data store owned by `account_id` —
    /// what a request naming an `EventDataStoreOwnerAccountId` resolves against.
    pub fn has_event_data_store_owned_by(&self, region: &str, account_id: &str) -> bool {
        self.event_data_stores
            .values()
            .any(|record| record.region == region && record.owner() == account_id)
    }

    /// Resolve a caller-supplied event data store identifier (an ARN or the bare
    /// id suffix of one) to a stored event data store.
    ///
    /// `Err` is an unreadable identifier (refuse with
    /// `EventDataStoreARNInvalidException`), `Ok(None)` a readable one naming no
    /// store in the Region it addresses (`EventDataStoreNotFoundException`): a
    /// store is reachable only from the Region it lives in, whether it is named by
    /// a bare id (the caller's Region) or by an ARN (the ARN's Region).
    pub fn event_data_store(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<Option<&EventDataStoreRecord>, EventDataStoreRejection> {
        let parsed = parse_event_data_store_identifier(identifier)?;
        Ok(self.addressed_event_data_store(&parsed, request_region))
    }

    /// The event data store an already-read identifier addresses, if the registry
    /// holds it — the lookup half of [`Self::event_data_store`], for a caller that
    /// has read the identifier itself (to word a refusal with the ARN it
    /// addresses, say).
    pub fn addressed_event_data_store(
        &self,
        identifier: &EventDataStoreIdentifier,
        request_region: &str,
    ) -> Option<&EventDataStoreRecord> {
        let addressed = identifier.addressed_region(request_region);
        self.event_data_stores
            .get(&identifier.id)
            .filter(|record| record.region == addressed)
    }

    /// [`Self::addressed_event_data_store`] for a caller that CHANGES the store —
    /// or that materialises a transition the clock already owed it (see
    /// [`EventDataStoreRecord::settle_federation`]).
    ///
    /// The addressing rule is the same one: a store is reachable only from the
    /// Region it lives in, whether it is named by a bare id or by an ARN.
    pub fn addressed_event_data_store_mut(
        &mut self,
        identifier: &EventDataStoreIdentifier,
        request_region: &str,
    ) -> Option<&mut EventDataStoreRecord> {
        let addressed = identifier.addressed_region(request_region).to_string();
        self.event_data_stores
            .get_mut(&identifier.id)
            .filter(|record| record.region == addressed)
    }

    /// The query `query_id` names, as a call made in `request_region` and naming
    /// `event_data_store_id` (when it names one) addresses it.
    pub fn query(
        &self,
        query_id: &str,
        request_region: &str,
        event_data_store_id: Option<&str>,
    ) -> Option<&QueryRecord> {
        self.queries
            .get(query_id)
            .filter(|record| record.addressed_by(request_region, event_data_store_id))
    }

    /// The LAST query run for `alias` — what `DescribeQuery` reports when it is
    /// given a `QueryAlias` instead of a `QueryId` — narrowed to one dashboard
    /// refresh when `refresh_id` is given.
    ///
    /// "Last" is by creation time, so the newest run of the template wins.
    pub fn last_query_for_alias(
        &self,
        alias: &str,
        refresh_id: Option<&str>,
        request_region: &str,
        event_data_store_id: Option<&str>,
    ) -> Option<&QueryRecord> {
        self.queries
            .values()
            .filter(|record| record.addressed_by(request_region, event_data_store_id))
            .filter(|record| record.query_alias.as_deref() == Some(alias))
            .filter(|record| {
                refresh_id.is_none_or(|refresh| record.refresh_id.as_deref() == Some(refresh))
            })
            .max_by_key(|record| record.created_at)
    }
}

// ---------------------------------------------------------------------------
// CloudTrail Lake dashboards
// ---------------------------------------------------------------------------

/// The resource prefix of a dashboard ARN:
/// `arn:aws:cloudtrail:<region>:<account>:dashboard/<name>`.
const DASHBOARD_RESOURCE_PREFIX: &str = "dashboard/";

/// Dashboard-name length bounds, from the `DashboardName` length constraint in the
/// model (`smithy_query(query="operation", operation="CreateDashboard")` reports
/// `Name` as `len 3..128, pattern ^[a-zA-Z0-9_\-]+$`).
pub const DASHBOARD_NAME_MIN_LEN: usize = 3;
/// See [`DASHBOARD_NAME_MIN_LEN`].
pub const DASHBOARD_NAME_MAX_LEN: usize = 128;

/// The one name that names the CloudTrail-managed Highlights dashboard, from the
/// model doc for `CreateDashboardRequest$Name`: "To create the Highlights dashboard,
/// the name must be AWSCloudTrail-Highlights."
pub const HIGHLIGHTS_DASHBOARD_NAME: &str = "AWSCloudTrail-Highlights";

/// `DashboardType` as the model enumerates it (`MANAGED | CUSTOM`): the dashboard a
/// caller composes out of its own widgets.
pub const DASHBOARD_TYPE_CUSTOM: &str = "CUSTOM";
/// See [`DASHBOARD_TYPE_CUSTOM`]: the Highlights dashboard, which CloudTrail owns.
pub const DASHBOARD_TYPE_MANAGED: &str = "MANAGED";

/// `DashboardStatus` as the model enumerates it (`CREATING | CREATED | UPDATING |
/// UPDATED | DELETING`): a dashboard CloudTrail is still loading.
pub const DASHBOARD_STATUS_CREATING: &str = "CREATING";
/// See [`DASHBOARD_STATUS_CREATING`]: a dashboard that has finished loading.
pub const DASHBOARD_STATUS_CREATED: &str = "CREATED";

/// From the `CreateDashboard` model doc: "You can add up to 10 widgets to a custom
/// dashboard."
pub const CUSTOM_DASHBOARD_WIDGET_LIMIT: usize = 10;

/// How many dashboards the registry holds before `CreateDashboard` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// CloudTrail publishes an adjustable per-account dashboard quota, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on the store rather than a claim about any account's limit.
/// Every record it holds is released by `DeleteDashboard`, and it sits far above any
/// workflow an emulator serves.
pub const DASHBOARD_QUOTA: usize = 1_000;

/// The largest page `ListDashboards` reports, which is also the page size a request
/// that names no `MaxResults` takes.
///
/// It is the modelled maximum of `ListDashboardsMaxResultsCount`
/// (`smithy_query(query="operation", operation="ListDashboards")` reports `MaxResults`
/// as `range 1..1000`), so a caller asking for more than the model admits is refused at
/// the boundary rather than here, and a caller asking for nothing gets the whole
/// Region's dashboards in one page unless the registry holds more than the model lets
/// one page carry.
pub const LIST_DASHBOARDS_MAX_PAGE_SIZE: usize = 1_000;

/// How long a new dashboard reads [`DASHBOARD_STATUS_CREATING`] before the clock
/// settles it into [`DASHBOARD_STATUS_CREATED`].
///
/// The model carries a `CREATING` status and a `ConflictException` documented as
/// "the specified resource is not ready for an operation … before CloudTrail has
/// time to fully load the resource", so a new dashboard IS loaded asynchronously.
/// CloudTrail publishes no duration for that load, so this span is the emulator's
/// own: short enough not to stall a workflow, non-zero so a caller that reads the
/// dashboard straight after creating it still sees the transient status.
pub const DASHBOARD_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// `RefreshScheduleFrequencyUnit` as the model enumerates it (`HOURS | DAYS`).
pub const REFRESH_FREQUENCY_UNIT_HOURS: &str = "HOURS";
/// See [`REFRESH_FREQUENCY_UNIT_HOURS`].
pub const REFRESH_FREQUENCY_UNIT_DAYS: &str = "DAYS";

/// From the model doc for `RefreshScheduleFrequency$Value`: "For custom dashboards,
/// the following values are valid when the unit is HOURS: 1, 6, 12, 24".
pub const REFRESH_FREQUENCY_HOURS_VALUES: [i32; 4] = [1, 6, 12, 24];

/// From the same doc: "For custom dashboards, the only valid value when the unit is
/// DAYS is 1."
pub const REFRESH_FREQUENCY_DAYS_VALUE: i32 = 1;

/// From the model doc for `CreateDashboardRequest$RefreshSchedule`: "To create the
/// Highlights dashboard, you must set a refresh schedule and set the Status to
/// ENABLED. The Unit for the refresh schedule must be HOURS and the Value must be 6."
pub const HIGHLIGHTS_REFRESH_FREQUENCY_VALUE: i32 = 6;

/// `RefreshScheduleStatus` as the model enumerates it (`ENABLED | DISABLED`).
pub const REFRESH_SCHEDULE_ENABLED: &str = "ENABLED";

/// From the model doc for `RefreshSchedule$TimeOfDay`: "The time of day in UTC to
/// run the schedule; for hourly only refer to minutes; default is 00:00."
pub const DEFAULT_REFRESH_TIME_OF_DAY: &str = "00:00";

/// The refusal text a dashboard operation answers with when the identifier it was
/// given names no stored dashboard.
///
/// It is a FIXED service message — CloudTrail reports no part of the request in it,
/// and it is the text the recorded `GetDashboard` miss carries — so every dashboard
/// operation answers a miss with the same words rather than wording its own.
pub const DASHBOARD_NOT_FOUND_MESSAGE: &str = "No matching dashboard found.";

/// The registry key: a dashboard name and the Region the dashboard lives in.
///
/// A dashboard ARN carries a Region, so a dashboard is a regional resource and its
/// name alone is not a key. The name comes FIRST so the `BTreeMap` iterates
/// name-ordered, which is the order `ListDashboards` pages through.
pub type DashboardKey = (String, String);

/// One persisted CloudTrail Lake dashboard: everything the create/update request was
/// given, plus the values the service mints (the ARN, the type, the status and the
/// timestamps).
#[derive(Debug, Clone)]
pub struct DashboardRecord {
    pub name: String,
    pub dashboard_arn: String,
    /// [`DASHBOARD_TYPE_CUSTOM`] or [`DASHBOARD_TYPE_MANAGED`].
    pub dashboard_type: String,
    /// A `DashboardStatus` value; see [`DashboardRecord::settle`].
    pub status: String,
    /// When `CREATING` is due to settle into `CREATED`.
    pub settles_at: SystemTime,
    pub widgets: Vec<Widget>,
    pub refresh_schedule: Option<RefreshSchedule>,
    pub termination_protection_enabled: bool,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub home_region: String,
    /// Set by `StartDashboardRefresh`, which is not implemented yet, so a dashboard
    /// has never been refreshed.
    pub last_refresh_id: Option<String>,
    /// See [`DashboardRecord::last_refresh_id`].
    pub last_refresh_failure_reason: Option<String>,
}

impl DashboardRecord {
    /// Materialise the `CREATING` -> `CREATED` transition the clock already owes.
    ///
    /// A reporting operation may call this: the transition was due at `settles_at`
    /// whether or not anybody read the dashboard, so applying it changes nothing a
    /// later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == DASHBOARD_STATUS_CREATING && now >= self.settles_at {
            self.status = DASHBOARD_STATUS_CREATED.to_string();
        }
    }

    /// Whether this is the CloudTrail-managed Highlights dashboard.
    pub fn is_managed(&self) -> bool {
        self.dashboard_type == DASHBOARD_TYPE_MANAGED
    }
}

/// A caller-supplied dashboard identifier, in either of the two forms the model
/// documents for `GetDashboardRequest$DashboardId` ("The name or ARN for the
/// dashboard").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardIdentifier {
    /// A bare dashboard name, which names a dashboard in the caller's own Region.
    Name(String),
    /// A dashboard ARN, which also names the Region the dashboard lives in.
    Arn { region: String, name: String },
}

impl DashboardIdentifier {
    /// The dashboard name this identifier names — the registry's key.
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Arn { name, .. } => name,
        }
    }

    /// The Region this identifier pins the dashboard to, if it pins one.
    pub fn region(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Arn { region, .. } => Some(region),
        }
    }
}

/// The ARN of the dashboard called `name` in `region`.
pub fn dashboard_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{DASHBOARD_RESOURCE_PREFIX}{name}"),
    )
}

/// Whether `name` satisfies the modelled `DashboardName` constraints: between
/// [`DASHBOARD_NAME_MIN_LEN`] and [`DASHBOARD_NAME_MAX_LEN`] characters of
/// `^[a-zA-Z0-9_\-]+$`.
pub fn is_valid_dashboard_name(name: &str) -> bool {
    let length = name.chars().count();
    (DASHBOARD_NAME_MIN_LEN..=DASHBOARD_NAME_MAX_LEN).contains(&length)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Read a caller-supplied dashboard identifier as either a dashboard ARN or a bare
/// dashboard name. `None` is an identifier that can name no dashboard at all.
///
/// A value that starts with `arn:` is read as an ARN — the modelled `DashboardArn`
/// pattern admits both forms — and anything else as a bare name.
pub fn parse_dashboard_identifier(value: &str) -> Option<DashboardIdentifier> {
    if let Some(rest) = value.strip_prefix("arn:") {
        let fields: Vec<&str> = rest.split(':').collect();
        let [partition, service, region, account, resource] = fields.as_slice() else {
            return None;
        };
        if partition.is_empty()
            || *service != SERVICE_NAMESPACE
            || region.is_empty()
            || account.is_empty()
        {
            return None;
        }
        let name = resource.strip_prefix(DASHBOARD_RESOURCE_PREFIX)?;
        if !is_valid_dashboard_name(name) {
            return None;
        }
        return Some(DashboardIdentifier::Arn {
            region: (*region).to_string(),
            name: name.to_string(),
        });
    }
    if !is_valid_dashboard_name(value) {
        return None;
    }
    Some(DashboardIdentifier::Name(value.to_string()))
}

/// Check a caller-supplied refresh schedule against the frequencies the model
/// documents, and hand back the schedule as it will be STORED — which is the request's
/// own schedule with `TimeOfDay` defaulted to [`DEFAULT_REFRESH_TIME_OF_DAY`].
///
/// `Err` carries the refusal text, built from the request so a caller can fix it from
/// the message alone. Every dashboard operation that takes a `RefreshSchedule` applies
/// this, so the create and the update paths cannot drift.
pub fn normalize_refresh_schedule(schedule: &RefreshSchedule) -> Result<RefreshSchedule, String> {
    let time_of_day = schedule
        .time_of_day
        .clone()
        .unwrap_or_else(|| DEFAULT_REFRESH_TIME_OF_DAY.to_string());
    // The modelled pattern `^[0-9]{2}:[0-9]{2}$` fixes the shape; what it cannot say
    // is that the two fields are a UTC time of day.
    let (hours, minutes) = time_of_day
        .split_once(':')
        .ok_or_else(|| time_of_day_rejection(&time_of_day))?;
    let valid_time = match (hours.parse::<u32>(), minutes.parse::<u32>()) {
        (Ok(hours), Ok(minutes)) => hours < 24 && minutes < 60,
        _ => false,
    };
    if !valid_time {
        return Err(time_of_day_rejection(&time_of_day));
    }

    if let Some(frequency) = schedule.frequency.as_ref() {
        let (Some(unit), Some(value)) = (frequency.unit.as_deref(), frequency.value) else {
            return Err(
                "RefreshSchedule.Frequency must carry both Unit and Value: a refresh \
                 frequency is a unit and a value together"
                    .to_string(),
            );
        };
        let valid_value = match unit {
            REFRESH_FREQUENCY_UNIT_HOURS => REFRESH_FREQUENCY_HOURS_VALUES.contains(&value),
            REFRESH_FREQUENCY_UNIT_DAYS => value == REFRESH_FREQUENCY_DAYS_VALUE,
            // `RefreshScheduleFrequencyUnit` is a modelled enum, so the boundary
            // validator has already refused anything else.
            _ => false,
        };
        if !valid_value {
            return Err(format!(
                "RefreshSchedule.Frequency.Value {value} is not valid for Unit {unit}: the \
                 valid values are {:?} for {REFRESH_FREQUENCY_UNIT_HOURS} and \
                 {REFRESH_FREQUENCY_DAYS_VALUE} for {REFRESH_FREQUENCY_UNIT_DAYS}",
                REFRESH_FREQUENCY_HOURS_VALUES
            ));
        }
    }

    Ok(RefreshSchedule {
        frequency: schedule.frequency.clone(),
        status: schedule.status.clone(),
        time_of_day: Some(time_of_day),
    })
}

/// The refusal text for a `TimeOfDay` that is not a UTC time of day.
fn time_of_day_rejection(time_of_day: &str) -> String {
    format!(
        "RefreshSchedule.TimeOfDay '{time_of_day}' is not valid: the time of day is UTC \
         HH:MM, with HH between 00 and 23 and MM between 00 and 59"
    )
}

/// Whether `schedule` is the one the Highlights dashboard requires: enabled, and
/// refreshing every [`HIGHLIGHTS_REFRESH_FREQUENCY_VALUE`] hours.
pub fn is_highlights_refresh_schedule(schedule: &RefreshSchedule) -> bool {
    let enabled = schedule.status.as_deref() == Some(REFRESH_SCHEDULE_ENABLED);
    let frequency = schedule.frequency.as_ref().is_some_and(|frequency| {
        frequency.unit.as_deref() == Some(REFRESH_FREQUENCY_UNIT_HOURS)
            && frequency.value == Some(HIGHLIGHTS_REFRESH_FREQUENCY_VALUE)
    });
    enabled && frequency
}

/// Project a requested widget onto the modelled `Widget` shape, as a dashboard
/// reports it back.
///
/// `QueryAlias` names a CloudTrail-managed query; a widget the caller composed
/// carries its own `QueryStatement` instead, so no alias is minted for it.
pub fn request_widget_to_widget(widget: RequestWidget) -> Widget {
    Widget {
        query_alias: None,
        query_statement: Some(widget.query_statement),
        query_parameters: widget.query_parameters,
        view_properties: Some(widget.view_properties),
    }
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// The registry key a caller-supplied dashboard identifier addresses: the
    /// dashboard of that name in the Region the identifier pins — the ARN's own
    /// Region, or, for a bare name, the Region the call was made in.
    ///
    /// `None` is an identifier that can name no dashboard at all.
    pub fn addressed_dashboard_key(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Option<DashboardKey> {
        let parsed = parse_dashboard_identifier(identifier)?;
        Some((
            parsed.name().to_string(),
            parsed.region().unwrap_or(request_region).to_string(),
        ))
    }

    /// Resolve a caller-supplied dashboard identifier to the stored dashboard,
    /// materialising the transition its clock already owes.
    pub fn resolve_dashboard_mut(
        &mut self,
        identifier: &str,
        request_region: &str,
        now: SystemTime,
    ) -> Option<&mut DashboardRecord> {
        let key = self.addressed_dashboard_key(identifier, request_region)?;
        let record = self.dashboards.get_mut(&key)?;
        record.settle(now);
        Some(record)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudtrailCloudtrail20131101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudtrailCloudtrail20131101State>)
}
