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
//!
//! It owns the **resource-based policy** registry too — the policy document attached
//! to one CloudTrail resource — plus the rules every resource-policy operation
//! shares: how a `ResourceArn` is read, which resource types carry a policy at all,
//! which stored resource an ARN resolves to, and what makes a policy document valid.
//!
//! It owns the CloudTrail Lake **channel** registry on the same terms again: a channel
//! identifier is a channel ARN or the bare id the service minted for it, and the refusal
//! wording for an `arn:` value that is not a channel ARN, or for an identifier that names
//! no channel, lives here so every channel operation refuses alike.
//!
//! The **tagging** rules live here for the same reason: a tagging call addresses its
//! resource by ARN, and how that ARN resolves to a stored resource's tag set — plus
//! how a malformed one and one naming nothing are refused — is shared by every
//! tagging operation.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AdvancedEventSelector, AggregationConfiguration, ContextKeySelector, EventSelector,
    ImportSource, ImportStatistics, ImportsListItem, InsightSelector, PartitionKey,
    RefreshSchedule, RequestWidget, Tag, Trail, TrailInfo, Widget, INSIGHT_TYPE_VALUES,
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

/// How many advanced event selectors ONE event data store may carry, from the model
/// doc for `CreateEventDataStoreRequest$AdvancedEventSelectors` (and, in the same
/// words, `UpdateEventDataStoreRequest$AdvancedEventSelectors`): "You can configure up
/// to five advanced event selectors for each event data store."
pub const ADVANCED_SELECTORS_PER_EVENT_DATA_STORE_LIMIT: usize = 5;

/// How many trails one `ListTrails` page carries. `ListTrails` models no
/// `MaxResults`, so the page size is the service's to choose; the remainder is
/// reached through `NextToken`.
pub const LIST_TRAILS_PAGE_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Insights selectors
//
// The rules the Insights-selector pair (`PutInsightSelectors` /
// `GetInsightSelectors`) shares: which event category an Insights selector
// defaults to, how the `InsightType` value set is worded in a refusal, and how
// an event data store states which category of events it collects.
// ---------------------------------------------------------------------------

/// The event category an Insights selector that names none applies to, from the
/// model doc for `InsightSelector$EventCategories`: "If EventCategories is not
/// provided, the specified Insights types are enabled on management API calls by
/// default." It is one of [`crate::ops::types::SOURCE_EVENT_CATEGORY_VALUES`].
pub const DEFAULT_INSIGHTS_EVENT_CATEGORY: &str = "Management";

/// The event category of the data events Insights can be enabled on, the other
/// value of the model's `SourceEventCategory` enum
/// ([`crate::ops::types::SOURCE_EVENT_CATEGORY_VALUES`]).
pub const DATA_INSIGHTS_EVENT_CATEGORY: &str = "Data";

/// The `AdvancedFieldSelector$Field` an event data store selects a category of
/// events on. "You can call GetEventDataStore on an event data store to check
/// whether the event data store logs management events" (model doc for
/// `PutInsightSelectors`) — which categories a store collects is what its
/// advanced event selectors state, and they state it on this field.
pub const EVENT_CATEGORY_FIELD: &str = "eventCategory";

/// The `eventCategory` an event data store that logs Insights events selects on —
/// the destination store of `PutInsightSelectors$InsightsDestination`, which "must
/// log Insights events" (model doc for `InvalidInsightSelectorsException`).
pub const INSIGHT_EVENT_CATEGORY: &str = "Insight";

/// The Insight types the model's `InsightType` enum carries, worded as a refusal
/// lists them.
///
/// The enum carries exactly two values
/// ([`crate::ops::types::INSIGHT_TYPE_VALUES`]), so the set reads as "A and B";
/// read off the model rather than written out, so it cannot drift from it.
pub fn insight_type_value_set() -> String {
    INSIGHT_TYPE_VALUES.join(" and ")
}

/// The `InvalidInsightSelectorsException` text for an `InsightType` the model's
/// enum does not carry: "this exception is thrown when … the specified
/// `InsightType` in the `InsightSelectors` statement is not valid. Valid values
/// for InsightType are ApiCallRateInsight and ApiErrorRateInsight."
///
/// Built from the request — it names the value the caller sent and the set it is
/// not in — so a caller can fix the call from the refusal alone. The value set is
/// off the generated schema (`skip_validation` on `InsightSelector`), so this is
/// the ONE place that refusal is worded, for every operation that carries an
/// `InsightSelectors` statement.
pub fn invalid_insight_type_message(value: &str) -> String {
    format!(
        "The provided value is not a valid Insights type: {value}. Valid Insight types are: {}. \
         Verify your input and then try again.",
        insight_type_value_set()
    )
}

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
    /// The Insights selectors `PutInsightSelectors` configured on this trail and
    /// `GetInsightSelectors` reports.
    ///
    /// EMPTY means Insights event logging is OFF, which is also how a caller turns
    /// it off: "You also use PutInsightSelectors to turn off Insights event logging,
    /// by passing an empty list of Insights types" (model doc for
    /// `PutInsightSelectors`). A trail is created with Insights off.
    pub insight_selectors: Vec<InsightSelector>,
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
            // "Specifies whether a trail has Insight types specified in an
            // InsightSelector list" — true exactly when a selector list is
            // configured, so it is derived from the stored selectors rather than
            // tracked beside them.
            has_insight_selectors: Some(!self.insight_selectors.is_empty()),
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
/// * between [`TRAIL_NAME_MIN_LEN`] and [`TRAIL_NAME_MAX_LEN`] characters,
/// * starts and ends with a letter or digit,
/// * no two adjacent periods, underscores or dashes,
/// * only ASCII letters, digits, periods, underscores or dashes,
/// * not in IP-address format.
///
/// A refusal names the ONE rule the name broke — and, where that rule is a length
/// bound, the bound and the name's own length — rather than restating every rule, so
/// the caller can fix the name from the refusal alone. The rules are checked in the
/// order they are listed above, so a name that breaks several is refused for the
/// first.
///
/// That order is the order the real service applies them in, not an arbitrary one:
/// a name whose last character is neither a letter nor a digit is refused for the
/// terminal-character rule even when that character is also outside the charset
/// (recorded: `bad name!` answers "Trail name must end with a letter or number.",
/// not the charset sentence), and a name shorter than the minimum is refused for
/// its length whatever else it breaks. So the charset rule — which every
/// ill-formed name also breaks — is checked LAST of the character rules, and only
/// a name that satisfies the edge and adjacency rules can be refused for it.
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

/// The Region, account and resource segments of a CloudTrail resource ARN —
/// `arn:<partition>:cloudtrail:<region>:<account>:<resource>` — or `None` when `value`
/// is not one.
///
/// The ARN itself is read through [`::service_common::arn`], this tree's ONE parse of a
/// caller-supplied resource ARN — the same one [`parse_taggable_arn`] uses — so no
/// parser in this module can disagree with another, or with another service, about what
/// an ARN is. What this adds is CloudTrail's own part: every taggable or addressable
/// CloudTrail resource lives in a Region and belongs to an account, so an ARN leaving
/// either field empty names none of them, unlike the partition-global ARNs the shared
/// parse deliberately admits. What the RESOURCE segment must look like stays the
/// caller's rule, because it differs per resource type.
pub fn cloudtrail_arn_fields(value: &str) -> Option<(&str, &str, &str)> {
    let arn = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
    (!arn.region.is_empty() && !arn.account.is_empty())
        .then_some((arn.region, arn.account, arn.resource))
}

/// Read `arn:<partition>:cloudtrail:<region>:<account>:trail/<name>`.
fn parse_trail_arn(value: &str) -> Result<TrailIdentifier, IdentifierRejection> {
    // The refusal names the ARN the caller supplied, which is the one thing the
    // caller has to change — the way the service words it: "The ARN <arn> is
    // invalid."
    let reject = || {
        Err(IdentifierRejection::InvalidArn(format!(
            "The ARN {value} is invalid."
        )))
    };

    let Some((region, _account, resource)) = cloudtrail_arn_fields(value) else {
        return reject();
    };
    let Some(name) = resource.strip_prefix(TRAIL_RESOURCE_PREFIX) else {
        return reject();
    };
    if validate_trail_name(name).is_err() {
        return reject();
    }
    Ok(TrailIdentifier::Arn {
        region: region.to_string(),
        name: name.to_string(),
    })
}

/// The HTTP status every trail not-found refusal answers with.
///
/// The model annotates `TrailNotFoundException` `@httpError(404)`, but CloudTrail
/// answers 400 for it on the wire: the recorded responses of `GetTrail`,
/// `GetTrailStatus`, `DeleteTrail`, `StartLogging` and `StopLogging` on a trail that
/// was never created are HTTP 400 with `__type` `TrailNotFoundException`. The error
/// CODE and the body stay the model's; only the status is stated here, through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`]. It is the same deviation
/// `EventDataStoreNotFoundException` and `QueryIdNotFoundException` already carry in
/// `ops::describe_query`.
pub const TRAIL_NOT_FOUND_STATUS: u16 = 400;

/// The refusal text that names the trail by the ARN the identifier RESOLVED to —
/// the subject every trail operation's `TrailNotFoundException` carries unless its
/// own recorded refusal echoes the caller's identifier instead.
///
/// A caller who passed a bare name is told which Region's trail was looked for,
/// which is what the recorded refusals of `GetTrailStatus`, `DeleteTrail`,
/// `StartLogging`, `StopLogging`, `UpdateTrail` and `PutEventSelectors` carry. The
/// three exceptions, each pinned by its own recording, echo the identifier as the
/// request sent it ([`unknown_trail_message`]): `GetTrail`, `GetInsightSelectors`
/// and the tag writes.
pub fn trail_not_found_arn_message(region: &str, name: &str) -> String {
    unknown_trail_message(&trail_arn(region, name))
}

/// The refusal text for `TrailAlreadyExistsException`: `CreateTrail`'s duplicate-name
/// refusal, which names the trail the request asked for and the account that already
/// holds one of that name.
///
/// Request-derived (the `Name` the caller sent) plus the caller's account, the way
/// the recorded response words it — the Region is NOT in it, even though a trail name
/// is scoped to one Region, because "for customer: <account>" is the subject the
/// service states.
pub fn trail_already_exists_message(name: &str) -> String {
    format!(
        "Trail {name} already exists for customer: {}",
        ::engine::emulator_account_id()
    )
}

/// The refusal text for `InsightNotEnabledException`, shared by every operation that
/// reports or reads Insights events on a resource that has none enabled
/// (`GetInsightSelectors` and the Insights data operations).
///
/// Request-derived (the ARN of the resource the call addressed) plus the fixed tail
/// the recorded response carries, which tells the caller what to change. `kind` is the
/// noun the subject opens with: `Trail` for a trail, which is the form the recorded
/// refusal carries, and [`EVENT_DATA_STORE_SUBJECT`] for an event data store, whose
/// own wording no recording pins — so it states the same rule about the resource it
/// was actually given.
pub fn insight_not_enabled_message(kind: &str, resource_arn: &str) -> String {
    format!(
        "{kind} {resource_arn} does not have Insights enabled. Edit the trail settings to \
         enable Insights, and then try the operation again."
    )
}

/// The subject noun of a refusal about a trail: `Trail <arn>`.
pub const TRAIL_SUBJECT: &str = "Trail";

/// The subject noun of a refusal about an event data store.
pub const EVENT_DATA_STORE_SUBJECT: &str = "Event data store";

/// Render an instant the way the modelled string timestamp members carry it
/// (ISO 8601, e.g. `2026-10-21T21:38:12Z`).
pub fn iso8601(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

// ---------------------------------------------------------------------------
// Event data store lifecycle (CloudTrail Lake)
// ---------------------------------------------------------------------------

/// The shortest retention period an event data store may carry, from the model's
/// `@range` on `com.amazonaws.cloudtrail#RetentionPeriod` (7..3653).
pub const RETENTION_PERIOD_MIN_DAYS: i32 = 7;

/// From the model doc for `CreateEventDataStoreRequest$AdvancedEventSelectors`:
/// "You can configure up to five advanced event selectors for each event data
/// store." Exceeding it is the modelled `InvalidEventSelectorsException`.
pub const ADVANCED_EVENT_SELECTOR_LIMIT: usize = 5;

/// The default and maximum retention period of `billing_mode`, in days.
///
/// From the model doc for `CreateEventDataStoreRequest$BillingMode`:
/// "`EXTENDABLE_RETENTION_PRICING` - ... a flexible retention period of up to 3653
/// days (about 10 years). The default retention period for this billing mode is 366
/// days." / "`FIXED_RETENTION_PRICING` - ... need a retention period of up to 2557
/// days (about 7 years). The default retention period for this billing mode is 2557
/// days."
pub fn retention_bounds(billing_mode: EventDataStoreBillingMode) -> (i32, i32) {
    match billing_mode {
        EventDataStoreBillingMode::ExtendableRetentionPricing => (366, 3653),
        EventDataStoreBillingMode::FixedRetentionPricing => (2557, 2557),
    }
}

/// How long a newly created event data store stays in `CREATED` before the clock owes
/// it the status it was created for.
///
/// AWS publishes no span for it, so this is the emulator's own: short enough that a
/// workflow is not slowed, long enough that a caller reading the store straight back
/// still sees the transient `CREATED` the real service reports.
pub const EVENT_DATA_STORE_PROVISIONING: Duration = Duration::from_millis(500);

/// How long a store stays in `PENDING_DELETION` before it is really gone, from the
/// model doc for `DeleteEventDataStore`: "the event data store enters a
/// PENDING_DELETION state, and is automatically deleted after a wait period of seven
/// days."
pub const EVENT_DATA_STORE_DELETION_WAIT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many event data stores the registry holds before `CreateEventDataStore`
/// refuses with the modelled `EventDataStoreMaxLimitExceededException`.
///
/// CloudTrail publishes an adjustable per-Region quota, and this emulator models
/// neither Service Quotas nor an account's adjustments — so this is the emulator's
/// OWN bound on the store rather than a claim about any account's limit. It bounds
/// the WHOLE registry, because the Region a store is created in comes from the
/// request: a per-Region bound would still let a caller grow the store without end
/// by varying the Region it signs for. It sits far above any workflow an emulator
/// serves.
pub const EVENT_DATA_STORE_QUOTA: usize = 1_000;

/// The fully specified KMS key ARN a `KmsKeyId` names.
///
/// The request takes "an alias name prefixed by alias/, a fully specified ARN to an
/// alias, a fully specified ARN to a key, or a globally unique identifier" — the forms
/// differ only in how they SPELL one key, so two spellings of the same key name the
/// same key. Completing a bare value into the ARN it names is what lets a caller that
/// re-states a store's key in another accepted form be told it changed nothing.
pub fn kms_key_arn(region: &str, key: &str) -> String {
    if key.starts_with("arn:") {
        key.to_string()
    } else if key.starts_with("alias/") {
        ::engine::emulator_arn("kms", region, key)
    } else {
        ::engine::emulator_arn("kms", region, &format!("key/{key}"))
    }
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

    /// Project onto the modelled `ImportsListItem` shape, as `ListImports` reports
    /// it: the five members that shape carries, each from the stored record.
    pub fn to_imports_list_item(&self) -> ImportsListItem {
        ImportsListItem {
            import_id: Some(self.import_id.clone()),
            import_status: Some(self.status.clone()),
            destinations: Some(self.destinations.clone()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_timestamp)),
            updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.updated_timestamp)),
        }
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

    /// The status an in-flight ingestion transition settles into once the clock
    /// has run, or `None` for a status the clock owes nothing on.
    ///
    /// The model's `EventDataStoreStatus` enum carries `STARTING_INGESTION` and
    /// `STOPPING_INGESTION` precisely because the Start/Stop ingestion pair is
    /// asynchronous: a store told to stop reads `STOPPING_INGESTION` until
    /// ingestion has stopped, and `STOPPED_INGESTION` after. Every other value
    /// is terminal — the clock moves none of them on its own.
    pub fn settled(self) -> Option<Self> {
        match self {
            Self::StartingIngestion => Some(Self::Enabled),
            Self::StoppingIngestion => Some(Self::StoppedIngestion),
            Self::Created | Self::Enabled | Self::PendingDeletion | Self::StoppedIngestion => None,
        }
    }
}

/// The event categories whose ingestion can be stopped and started.
///
/// From the model doc for `StopEventDataStoreIngestion` (and, in the same words,
/// `StartEventDataStoreIngestion`): "To stop ingestion, the event data store
/// `Status` must be `ENABLED` and the `eventCategory` must be `Management`,
/// `Data`, `NetworkActivity`, or `ConfigurationItem`." A store collecting any
/// other category (CloudTrail Insights events, Audit Manager evidence, events
/// from outside Amazon Web Services) ingests nothing a caller can pause, which is
/// what the modelled `InvalidEventDataStoreCategoryException` reports.
pub const INGESTION_CONTROLLABLE_EVENT_CATEGORIES: &[&str] =
    &["Management", "Data", "NetworkActivity", "ConfigurationItem"];

/// How long an event data store reads a non-terminal status — `CREATED` after
/// `CreateEventDataStore`, `STARTING_INGESTION` / `STOPPING_INGESTION` after the
/// Start/Stop ingestion pair — before the clock settles it into the status it is
/// heading for (`ENABLED` / `STOPPED_INGESTION`).
///
/// The model's `EventDataStoreStatus` enum carries every one of those in-flight
/// values, so both transitions ARE asynchronous. CloudTrail publishes no duration
/// for either, so this span is the emulator's own: short enough not to stall a
/// workflow, non-zero so a caller that reads the store straight after the call
/// still sees the transient status — and so the state errors that transient
/// status carries are reachable.
pub const INGESTION_SETTLE_AFTER: Duration = Duration::from_millis(500);

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

    /// Read a billing mode a request supplied, or `None` for a value the model's
    /// `BillingMode` enum does not carry.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "EXTENDABLE_RETENTION_PRICING" => Some(Self::ExtendableRetentionPricing),
            "FIXED_RETENTION_PRICING" => Some(Self::FixedRetentionPricing),
            _ => None,
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
    /// The status the CLOCK owes this store, and the instant it comes due: a store
    /// AWS reports in a non-terminal status reaches its next one with no further
    /// call. Materialised by [`EventDataStoreRecord::settle`].
    pub pending_status: Option<(SystemTime, EventDataStoreState)>,
    /// When an in-flight ingestion `status` (`STARTING_INGESTION` /
    /// `STOPPING_INGESTION`) is due to settle into the status it is heading for;
    /// `None` once it has, and for a store whose ingestion is not moving.
    pub ingestion_settles_at: Option<SystemTime>,
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
    /// The store's tag set, as `CreateEventDataStore` seeds it, `AddTags` merges into
    /// it, `RemoveTags` drops keys from it and `ListTags` reports it.
    pub tags: Vec<Tag>,
    /// When the `PENDING_DELETION` wait period elapses and the store is gone, set
    /// by `DeleteEventDataStore`.
    pub deleted_at: Option<SystemTime>,
    /// The Insights selectors `PutInsightSelectors` configured on this store and
    /// `GetInsightSelectors` reports. EMPTY means Insights event logging is off,
    /// on the same terms as [`TrailRecord::insight_selectors`].
    pub insight_selectors: Vec<InsightSelector>,
    /// The ARN of the store the Insights events this store generates are logged
    /// to (`PutInsightSelectors$InsightsDestination`); `None` while Insights are
    /// off, which is the only state in which no destination is configured.
    pub insights_destination: Option<String>,
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

    /// Take the status the clock already owes this store, if its instant has passed.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some((due, next)) = self.pending_status {
            if now >= due {
                self.status = next;
                self.pending_status = None;
            }
        }
    }

    /// Whether this store's seven-day `PENDING_DELETION` wait has elapsed, so a
    /// caller would no longer find it at all.
    pub fn deletion_due(&self, now: SystemTime) -> bool {
        self.deleted_at.is_some_and(|at| now >= at)
    }

    /// Whether this store collects events of `category`: one of its advanced event
    /// selectors selects that value on the [`EVENT_CATEGORY_FIELD`].
    ///
    /// That is what "the event data store logs management events" and "the
    /// destination event data store … must log Insights events" (model doc for
    /// `InvalidInsightSelectorsException`) are read against, so both are decided
    /// from the store's own selectors rather than from the request asking about it.
    pub fn logs_event_category(&self, category: &str) -> bool {
        self.advanced_event_selectors.iter().any(|selector| {
            selector.field_selectors.iter().any(|field_selector| {
                field_selector.field == EVENT_CATEGORY_FIELD
                    && field_selector
                        .equals
                        .iter()
                        .flatten()
                        .any(|value| value == category)
            })
        })
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

    /// Materialise the ingestion transition the clock already owes:
    /// `STARTING_INGESTION` into `ENABLED`, `STOPPING_INGESTION` into
    /// `STOPPED_INGESTION`.
    ///
    /// A reporting operation may call this: the transition was due at
    /// `ingestion_settles_at` whether or not anybody read the store, so applying
    /// it changes nothing a later caller would otherwise have seen.
    pub fn settle_ingestion(&mut self, now: SystemTime) {
        let Some(due) = self.ingestion_settles_at else {
            return;
        };
        if now < due {
            return;
        }
        self.ingestion_settles_at = None;
        if let Some(settled) = self.status.settled() {
            self.status = settled;
        }
    }

    /// Whether ingestion can be stopped or started on this store at all: it
    /// collects one of the [`INGESTION_CONTROLLABLE_EVENT_CATEGORIES`].
    ///
    /// Decided from the store's OWN advanced event selectors — the only place an
    /// event data store states which category of events it collects — so the
    /// answer is the same for every caller that addresses it.
    pub fn ingestion_is_controllable(&self) -> bool {
        INGESTION_CONTROLLABLE_EVENT_CATEGORIES
            .iter()
            .any(|category| self.logs_event_category(category))
    }

    /// The event categories this store collects, in the order its selectors state
    /// them — what a refusal about the store's category names.
    pub fn event_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = Vec::new();
        for selector in &self.advanced_event_selectors {
            for field_selector in &selector.field_selectors {
                if field_selector.field != EVENT_CATEGORY_FIELD {
                    continue;
                }
                for value in field_selector.equals.iter().flatten() {
                    if !categories.contains(value) {
                        categories.push(value.clone());
                    }
                }
            }
        }
        categories
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
/// rather than taken for an id — and anything else as a bare id. Either way the id
/// has to BE one this service could have minted ([`is_event_data_store_id`]) when it
/// is written WITHOUT an ARN: the recorded answer to `UpdateEventDataStore` naming
/// `not-an-arn` is `EventDataStoreARNInvalidException` over the ARN that value
/// completes to, not a report that the store is absent. An ARN's resource suffix is
/// the store the ARN names, so for that form whether a store of that id exists is
/// the next question and not a question about the ARN's form.
pub fn parse_event_data_store_identifier(
    value: &str,
) -> Result<EventDataStoreIdentifier, EventDataStoreRejection> {
    let reject = || Err(EventDataStoreRejection::InvalidArn(value.to_string()));
    if !value.contains(':') {
        // The id suffix form, with or without the `eventdatastore/` resource
        // prefix the full ARN carries.
        let id = value
            .strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX)
            .unwrap_or(value);
        if !is_event_data_store_id(id) {
            return reject();
        }
        return Ok(EventDataStoreIdentifier {
            id: id.to_string(),
            region: None,
        });
    }

    let Some((region, _account, resource)) = cloudtrail_arn_fields(value) else {
        return reject();
    };
    // The shared parse keeps every colon the resource field carries, because an ARN of
    // the `<type>:<id>` form is as well formed as one of the `<type>/<id>` form. An
    // event data store ARN is only ever the slash form ("the ARN (or the ID suffix of
    // the ARN) of an event data store", whose published format is
    // `…:eventdatastore/<id>`), so a further colon here is a seventh field this
    // identifier cannot carry.
    if resource.contains(':') {
        return reject();
    }
    match resource.strip_prefix(EVENT_DATA_STORE_RESOURCE_PREFIX) {
        Some(id) if !id.is_empty() => Ok(EventDataStoreIdentifier {
            id: id.to_string(),
            region: Some(region.to_string()),
        }),
        _ => reject(),
    }
}

/// Whether `value` is an id this service could have minted for an event data
/// store: the UUID that is the resource suffix of its ARN
/// (`eventdatastore/1a0abbc2-dfa0-46e6-885a-5ce09f2ee3ee`).
///
/// Only the lower-case hyphenated form counts. `Uuid::try_parse` also accepts the
/// simple, braced and URN forms and upper-case hex, and none of those is an id
/// `CreateEventDataStore` ever handed out: the modelled `UUID` shape every store
/// id member targets is `len 36..36`, `pattern ^[a-f0-9\-]+$`
/// (`smithy_query(query="shape", shape="UUID")`).
pub fn is_event_data_store_id(value: &str) -> bool {
    value.len() == 36
        && !value.contains(|c: char| c.is_ascii_uppercase())
        && ::uuid::Uuid::try_parse(value).is_ok()
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
///
/// The Insights-selector pair (`PutInsightSelectors` / `GetInsightSelectors`)
/// carries the same pair under the same rule — "You cannot use this parameter
/// with the TrailName parameter" (model doc for
/// `PutInsightSelectorsRequest$EventDataStore`) — and reads it through here too,
/// so one statement of the rule serves every operation that addresses either a
/// trail or an event data store.
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

/// The CloudTrail Lake **channel** registry's own rules: how a channel identifier is
/// read and how a channel operation refuses one. They live in their own module beside
/// the registry they resolve against, the way `src/service/rpc_target.rs` holds the
/// target reading `src/service.rs` applies.
pub mod channels;

/// How a dashboard widget's `QueryStatement` is read, how many widgets one request may
/// carry, and the sentence a statement CloudTrail Lake cannot run is refused with. In
/// its own module beside the registry it guards, for the same reason [`channels`] is:
/// both dashboard operations apply it.
pub mod widget_query;

/// Per-service state for com-amazonaws-cloudtrail-cloudtrail-20131101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudtrailCloudtrail20131101State {
    /// How many dashboard refreshes this process has minted an id for;
    /// `StartDashboardRefresh` counts refresh ids off this.
    pub refreshes_started: u64,
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
    /// Every CloudTrail Lake channel in the account, keyed by [`channels::ChannelKey`].
    pub channels: BTreeMap<channels::ChannelKey, channels::ChannelRecord>,
    /// The resource-based policy attached to each resource that has one, keyed by
    /// the canonical ARN of that resource ([`PolicyResourceArn::canonical`]).
    ///
    /// Bounded by the resource registries above rather than by a quota of its own: a
    /// policy is only ever written for a resource that exists
    /// ([`ComAmazonawsCloudtrailCloudtrail20131101State::resolve_policy_resource`]),
    /// and the modelled `ResourcePolicy` length caps each document at 8192
    /// characters.
    ///
    /// An entry outlives nothing: a policy belongs to the resource it is attached to,
    /// so an operation that DELETES one of those resources drops that resource's entry
    /// here too — otherwise a resource later created with the same name in the same
    /// Region would inherit a policy nobody attached to it. `DeleteDashboard` does;
    /// `DeleteEventDataStore` and `DeleteChannel` own that for their registries.
    pub resource_policies: BTreeMap<String, String>,
}

/// What a reporting operation found for the identifier it was given: the registry
/// key the identifier addressed, and the trail stored under it, if any.
#[derive(Debug)]
pub struct AddressedTrail<'a> {
    /// The name and Region the identifier addressed — the ARN's own Region for a
    /// trail ARN, the request's Region for a bare name.
    pub key: TrailKey,
    /// The trail stored under [`Self::key`]; `None` is a well-formed identifier
    /// naming no trail, which is `TrailNotFoundException`.
    pub record: Option<&'a TrailRecord>,
}

impl AddressedTrail<'_> {
    /// The refusal text for an identifier that names no trail: the ARN the
    /// identifier resolved to, plus the account the lookup ran in
    /// ([`trail_not_found_arn_message`]).
    pub fn not_found_message(&self) -> String {
        let (name, region) = &self.key;
        trail_not_found_arn_message(region, name)
    }
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
    /// No trail of this name in any Region. Carries the name the identifier
    /// resolved to, which the not-found refusal names as that trail's ARN (see
    /// [`trail_not_found_arn_message`]).
    Absent { name: String },
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

    /// Resolve a caller-supplied identifier the way [`Self::resolve`] does, keeping
    /// the key it ADDRESSED beside the trail stored under it.
    ///
    /// A reporting operation whose not-found refusal names the trail by its resolved
    /// ARN needs both: the record when there is one, and the ARN the identifier
    /// resolved to when there is not
    /// ([`AddressedTrail::not_found_message`]).
    pub fn resolve_addressed(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Result<AddressedTrail<'_>, IdentifierRejection> {
        let key = self.addressed_key(identifier, request_region)?;
        let record = self.trails.get(&key);
        Ok(AddressedTrail { key, record })
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
                None => TrailForMutation::Absent { name },
            }),
        }
    }

    /// Materialise every change the CLOCK already owes the event-data-store
    /// registry: a store whose provisioning span has elapsed takes the status it was
    /// created for, one whose seven-day `PENDING_DELETION` wait has elapsed is gone,
    /// and the Lake queries that ran on a store THIS pass removed go with it — a
    /// query is reachable only through the store it ran on, so nothing can address
    /// those records again. Every event-data-store lifecycle operation calls this
    /// before it reads, so what it reports is what a caller would see even if nobody
    /// had called.
    pub fn settle_event_data_stores(&mut self, now: SystemTime) {
        // The stores this pass removes, and with them the queries that ran on them:
        // only those, so a query whose store the registry never held (a seeded one,
        // say) is left alone — the clock owes nothing about it.
        let removed: ::std::collections::HashSet<String> = self
            .event_data_stores
            .iter()
            .filter(|(_, record)| record.deletion_due(now))
            .map(|(id, _)| id.clone())
            .collect();
        self.event_data_stores
            .retain(|id, _| !removed.contains(id));
        for record in self.event_data_stores.values_mut() {
            record.settle(now);
            // Every transition the clock owes a store is materialised in ONE place,
            // so a precondition cannot read a stale federation or ingestion status.
            record.settle_federation(now);
            record.settle_ingestion(now);
        }
        // This is also the ONLY eviction path the query registry has: no operation
        // inserts into it yet (`StartQuery` is still a scaffold stub, with its own
        // filed issue), so a per-registry bound belongs with the handler that will.
        if !removed.is_empty() {
            self.queries
                .retain(|_, query| !removed.contains(&query.event_data_store_id));
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
    /// or that materialises a transition the clock already owed it: its status move
    /// (see [`EventDataStoreRecord::settle`], which covers the create's step and
    /// the ingestion step alike) and its federation transition (see
    /// [`EventDataStoreRecord::settle_federation`]), each of which was due at the
    /// stored instant whether or not anybody called.
    ///
    /// The addressing rule is the same one: a store is reachable only from the
    /// Region it lives in, whether it is named by a bare id or by an ARN.
    pub fn addressed_event_data_store_mut(
        &mut self,
        identifier: &EventDataStoreIdentifier,
        request_region: &str,
        now: SystemTime,
    ) -> Option<&mut EventDataStoreRecord> {
        let addressed = identifier.addressed_region(request_region).to_string();
        let record = self
            .event_data_stores
            .get_mut(&identifier.id)
            .filter(|record| record.region == addressed)?;
        record.settle(now);
        record.settle_federation(now);
        Some(record)
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
/// dashboard." The check both dashboard writers apply is
/// [`widget_query::widgets_beyond_dashboard_limit_message`].
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

/// The HTTP status this service answers a dashboard `ConflictException` with.
///
/// The model annotates the shape `@httpError(409)`
/// (`smithy_query(query="shape", shape="ConflictException")`), but `@httpError` is an
/// HTTP-BINDING trait and the recorded response carrying `__type: ConflictException` —
/// a second create of a dashboard name already taken,
/// `cloudtrail-dashboard-error-paths#task#2` — carries HTTP **400**: the status a
/// client's retry classifier sees is the service's, not the model's, and a service
/// renders one error shape one way. It is answered through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`], which keeps the modelled
/// shape's body and changes only the status line — the same deviation
/// [`TRAIL_NOT_FOUND_STATUS`] and [`IMPORT_NOT_FOUND_HTTP_STATUS`] already carry for
/// this service. Stated here rather than in one handler so every dashboard operation
/// that refuses with `ConflictException` answers at the same status.
pub const DASHBOARD_CONFLICT_HTTP_STATUS: u16 = 400;

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
    /// The id of the refresh `StartDashboardRefresh` last began for this dashboard,
    /// and `None` until one has been started.
    pub last_refresh_id: Option<String>,
    /// Why the dashboard's last refresh failed. Nothing sets it: a refresh is started
    /// and recorded, but no Lake query runs for it (`StartQuery` is still a scaffold),
    /// so no refresh of this emulator's own fails.
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
    if value.starts_with("arn:") {
        let (region, _account, resource) = cloudtrail_arn_fields(value)?;
        let name = resource.strip_prefix(DASHBOARD_RESOURCE_PREFIX)?;
        if !is_valid_dashboard_name(name) {
            return None;
        }
        return Some(DashboardIdentifier::Arn {
            region: region.to_string(),
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

    /// Whether a store in `region` other than `except` already carries `name`.
    ///
    /// An event data store name is unique per Region, which is what
    /// `EventDataStoreAlreadyExistsException` reports; `except` is the id of the
    /// store being renamed, which never conflicts with itself.
    pub fn event_data_store_name_taken(
        &self,
        name: &str,
        region: &str,
        except: Option<&str>,
    ) -> bool {
        self.event_data_stores.values().any(|record| {
            record.name == name
                && record.region == region
                && except.is_none_or(|id| record.id != id)
        })
    }
}

// ---------------------------------------------------------------------------
// Resource-based policies
// ---------------------------------------------------------------------------

/// The resource prefix of a channel ARN:
/// `arn:aws:cloudtrail:<region>:<account>:channel/<channel-id>`, as the model doc for
/// `ResourceArn` and for `AddTagsRequest$ResourceId` both give it ("Example channel
/// ARN format: arn:aws:cloudtrail:us-east-2:123456789012:channel/01234567890"). The
/// event data store and dashboard prefixes the same docs give are already defined
/// above, with the registries they address.
const CHANNEL_RESOURCE_PREFIX: &str = "channel/";

/// What kind of CloudTrail resource an ARN names.
///
/// The three kinds a resource-based policy attaches to are the ones the model doc
/// for `ResourceArn` names: "the CloudTrail event data store, dashboard, or channel
/// attached to the resource-based policy". [`ResourceKind::Trail`] is here because a
/// trail ARN is a well-formed CloudTrail resource ARN that carries no resource-based
/// policy, and that is a different refusal from an ARN that is not well formed at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    EventDataStore,
    Dashboard,
    Channel,
    Trail,
}

impl ResourceKind {
    /// Every kind, so a caller can walk them rather than restate the list.
    pub const ALL: [Self; 4] = [
        Self::EventDataStore,
        Self::Dashboard,
        Self::Channel,
        Self::Trail,
    ];

    /// The `<type>/` prefix this kind's ARN carries in its resource segment.
    pub fn resource_prefix(self) -> &'static str {
        match self {
            Self::EventDataStore => EVENT_DATA_STORE_RESOURCE_PREFIX,
            Self::Dashboard => DASHBOARD_RESOURCE_PREFIX,
            Self::Channel => CHANNEL_RESOURCE_PREFIX,
            Self::Trail => TRAIL_RESOURCE_PREFIX,
        }
    }

    /// The resource type as an ARN spells it, without the trailing separator.
    pub fn resource_type(self) -> &'static str {
        self.resource_prefix().trim_end_matches('/')
    }

    /// How a refusal names this kind of resource in prose.
    ///
    /// CloudTrail's resource-policy surface words a resource it could not find per
    /// resource kind, and the recorded answers word two of them differently: an event
    /// data store's and a channel's sentence name the ARN they were given ("The event
    /// data store <arn> was not found."), a dashboard's names no identifier at all
    /// ("The dashboard was not found."). [`ResourceKind::not_found_message`] is where
    /// that split lives. It is NOT
    /// [`EVENT_DATA_STORE_NOT_FOUND_MESSAGE`]: that is the text of a different
    /// exception on a different surface (`EventDataStoreNotFoundException`, which the
    /// Lake query operations answer), which states no identifier.
    pub fn described(self) -> &'static str {
        match self {
            Self::EventDataStore => "event data store",
            Self::Dashboard => "dashboard",
            Self::Channel => "channel",
            Self::Trail => "trail",
        }
    }

    /// The refusal text for a resource of this kind that `value` named and that does
    /// not exist.
    ///
    /// See [`ResourceKind::described`]: a dashboard's sentence carries no identifier,
    /// every other kind's names the ARN the request supplied. An event data store's
    /// and a channel's are the ones this crate already states for those kinds
    /// ([`event_data_store_not_found_message`],
    /// [`channels::channel_not_found_message`]), so the surfaces cannot drift.
    /// The dashboard's is NOT [`DASHBOARD_NOT_FOUND_MESSAGE`]: the dashboard
    /// operations were recorded answering "No matching dashboard found." and this
    /// surface "The dashboard was not found.", so the two are deliberately not one
    /// sentence.
    pub fn not_found_message(self, value: &str) -> String {
        match self {
            Self::EventDataStore => event_data_store_not_found_message(value),
            Self::Channel => channels::channel_not_found_message(value),
            Self::Dashboard => format!("The {} was not found.", self.described()),
            // Spelled out rather than `_`, so a resource kind added later is a
            // compile error here rather than silently taking this wording.
            Self::Trail => format!("The {} {value} was not found.", self.described()),
        }
    }

    /// Whether a resource-based policy can be attached to this kind of resource:
    /// true for the three kinds the `ResourceArn` doc names, false for every other
    /// CloudTrail resource — which is what `ResourceTypeNotSupportedException`
    /// ("the specified resource type is not supported by CloudTrail") refuses.
    pub fn takes_resource_policy(self) -> bool {
        match self {
            Self::EventDataStore | Self::Dashboard | Self::Channel => true,
            Self::Trail => false,
        }
    }
}

/// A CloudTrail resource ARN, as a resource-policy operation reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResourceArn {
    pub kind: ResourceKind,
    /// The Region the ARN pins the resource to.
    pub region: String,
    /// The resource segment after the `<type>/` prefix: a dashboard's name, an event
    /// data store's or a channel's id.
    pub id: String,
}

impl PolicyResourceArn {
    /// This resource's canonical ARN — the one the emulator mints for it, which is
    /// the key its policy is stored under.
    ///
    /// Reading an ARN back out canonically is what keeps two spellings of one
    /// resource (another partition, or a caller quoting a real account id) from
    /// addressing two different policies, exactly as a dashboard lookup keys on the
    /// name and the Region rather than on the ARN's account segment.
    pub fn canonical(&self) -> String {
        ::engine::emulator_arn(
            SERVICE_NAMESPACE,
            &self.region,
            &format!("{}{}", self.kind.resource_prefix(), self.id),
        )
    }
}

/// Read `arn:<partition>:cloudtrail:<region>:<account>:<type>/<id>`, the ARN form the
/// model documents for every resource a resource-based policy can name.
///
/// The six-field split is NOT re-derived here: it is
/// [`service_common::arn::parse_of`], the one caller-supplied-ARN parse, narrowed to
/// this service's vendor. What this layers on top of it is CloudTrail's own: a
/// resource is homed in a Region and an account (the shared parse admits the
/// partition-global and account-less forms AWS writes for other services, which no
/// CloudTrail resource has), and the resource field names one of this service's
/// resource kinds with a non-empty id.
///
/// `None` is a value that is not a CloudTrail resource ARN at all: a bare name, an
/// ARN of another service, one missing a field, or one whose resource segment names
/// no CloudTrail resource type.
///
/// SHARES ITS SUBJECT WITH [`parse_taggable_arn`], which classifies the same four
/// CloudTrail resource kinds for the TAGGING surface. The two are kept apart because
/// what each surface does with a kind differs beyond the wording: a tagging ARN has to
/// satisfy the addressed resource's own naming rules before it can be refused as the
/// right kind of invalid ARN (`ChannelARNInvalidException` versus
/// `CloudTrailARNInvalidException`), while a policy ARN is classified only to pick
/// between `ResourceARNNotValidException`, `ResourceTypeNotSupportedException` and
/// `ResourceNotFoundException` and is then resolved through the registries. Both read
/// the ARN itself through [`::service_common::arn`], so neither re-derives the split;
/// a change to WHICH resource types this service has belongs in both, and the fix that
/// merges them states the shared classification once and layers each surface's
/// rejection mapping on top.
pub fn parse_resource_arn(value: &str) -> Option<PolicyResourceArn> {
    let parsed = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
    if parsed.region.is_empty() || parsed.account.is_empty() {
        return None;
    }
    let kind = ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.resource_type() == parsed.resource_type())?;
    // The shared reader takes the id out of either form AWS writes a resource in
    // (`<type>/<id>` and `<type>:<id>`); CloudTrail writes the slash form, which is
    // the one the canonical ARN this resolves to is built in.
    let id = ::service_common::arn::resource_id_of_type(value, kind.resource_type())?;
    if id.is_empty() {
        return None;
    }
    Some(PolicyResourceArn {
        kind,
        region: parsed.region.to_string(),
        id: id.to_string(),
    })
}

/// Why a resource-policy operation could not reach the resource its `ResourceArn`
/// names.
///
/// Each variant names the modelled error every resource-policy operation answers it
/// with, so the three operations cannot drift in how they refuse one ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyResourceRejection {
    /// Not a well-formed CloudTrail resource ARN ⇒ `ResourceARNNotValidException`
    /// (HTTP 400).
    ArnNotValid(String),
    /// A well-formed CloudTrail ARN naming a resource type that carries no
    /// resource-based policy ⇒ `ResourceTypeNotSupportedException` (HTTP 400).
    TypeNotSupported(String),
    /// A well-formed ARN of a supported type that names no stored resource ⇒
    /// `ResourceNotFoundException` (HTTP 400).
    ResourceNotFound(String),
}

impl PolicyResourceRejection {
    /// The refusal text: built from the request, naming the value and the rule it
    /// broke, so a caller can fix the request from it alone.
    ///
    /// Borrowed, so a handler reads the text and matches on the variant without
    /// cloning the rejection.
    pub fn message(&self) -> &str {
        match self {
            Self::ArnNotValid(message)
            | Self::TypeNotSupported(message)
            | Self::ResourceNotFound(message) => message,
        }
    }
}

/// The resource a resource-policy operation addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResource {
    /// The canonical ARN of the resource, which is the registry key its policy is
    /// stored under and the ARN the operation reports back.
    pub arn: String,
    /// Whether the resource has finished loading. A resource CloudTrail is still
    /// loading is what `ConflictException` ("the specified resource is not ready for
    /// an operation … before CloudTrail has time to fully load the resource")
    /// refuses a WRITE to; a read reports its policy regardless.
    pub ready: bool,
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// Resolve a caller-supplied `ResourceArn` to the resource whose policy it
    /// addresses, materialising any transition that resource's clock already owes.
    ///
    /// The three refusals are ordered the way the ARN is read: an ARN that is not a
    /// CloudTrail resource ARN cannot name a resource type, and a resource type that
    /// carries no policy cannot be looked up.
    pub fn resolve_policy_resource(
        &mut self,
        value: &str,
        now: SystemTime,
    ) -> Result<PolicyResource, PolicyResourceRejection> {
        let parsed = parse_resource_arn(value).ok_or_else(|| {
            // The recorded refusal names the value and nothing else: "The format of
            // the resource ARN <value> is not valid." — built from the request, so a
            // caller sees which value it was.
            PolicyResourceRejection::ArnNotValid(format!(
                "The format of the resource ARN {value} is not valid."
            ))
        })?;
        if !parsed.kind.takes_resource_policy() {
            return Err(PolicyResourceRejection::TypeNotSupported(format!(
                "Resource type '{}' does not support resource-based policies: a CloudTrail \
                 resource-based policy attaches to an event data store, a dashboard or a \
                 channel",
                parsed.kind.resource_type()
            )));
        }

        let ready = match parsed.kind {
            // Through the dashboard registry's own reader, so the key shape and the
            // transition the clock owes (`CREATING -> CREATED`, due at `settles_at`
            // whether or not anybody addressed the dashboard) are stated once. The
            // ARN's own Region is the Region the identifier is read in, and its
            // resource id is the dashboard's name.
            ResourceKind::Dashboard => self
                .resolve_dashboard_mut(&parsed.id, &parsed.region, now)
                .map(|record| record.status != DASHBOARD_STATUS_CREATING),
            // An event data store id is unique across Regions, and a store is
            // reachable only from the Region it lives in — so an ARN pinning
            // another Region names no store.
            ResourceKind::EventDataStore => self
                .event_data_stores
                .get(&parsed.id)
                .filter(|record| record.region == parsed.region)
                // `EventDataStoreStatus` carries no "still loading" state, so a
                // stored store is ready for a write unless it is on its way out:
                // a store awaiting deletion is the resource `ConflictException`
                // ("another operation is modifying the resource") covers.
                .map(|record| record.status.is_queryable()),
            // A channel lives in the Region its ARN carries, keyed by the id
            // CloudTrail minted for it ([`channels::ChannelKey`]). A channel record
            // carries no transient status, so a stored channel is ready to take a
            // policy.
            ResourceKind::Channel => self
                .channels
                .get(&(parsed.id.clone(), parsed.region.clone()))
                .map(|_| true),
            // Refused above by `takes_resource_policy`; answered rather than
            // panicked so the two rules cannot disagree at runtime.
            ResourceKind::Trail => None,
        };
        let Some(ready) = ready else {
            return Err(PolicyResourceRejection::ResourceNotFound(
                parsed.kind.not_found_message(value),
            ));
        };

        Ok(PolicyResource {
            arn: parsed.canonical(),
            ready,
        })
    }
}

/// The policy elements a resource-based policy statement is checked against, from
/// the IAM JSON policy grammar: a statement states an `Effect` of `Allow` or `Deny`,
/// an `Action` (or `NotAction`), and — because this is a policy attached TO a
/// resource rather than to an identity — a `Principal` (or `NotPrincipal`).
const POLICY_EFFECTS: [&str; 2] = ["Allow", "Deny"];

/// Check a caller-supplied resource-based policy document.
///
/// `Err` carries the refusal text for `ResourcePolicyNotValidException`, which the
/// model documents for a policy that "has syntax errors, or contains a principal
/// that is not valid" — so the syntax of the JSON policy document and the presence
/// of a usable principal are what this checks, each named in the message so a caller
/// can fix the request from it alone.
///
/// MIGRATION NOTE: the GRAMMAR this walks is one IAM rule, and three other services
/// already walk it against their own recorded wording —
/// `service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`,
/// `service-com-amazonaws-s3files-s3files` and `service-com-amazonaws-rum-rum`, each
/// in its own `src/state.rs`. `services/common` carries no shared grammar check yet
/// (its `iam_authorization` / `iam_trust_policy` EVALUATE a policy rather than
/// validate one, and the latter's parse is `pub(crate)`), and each copy's messages
/// are its service's dialect, so the structural fix is a shared
/// `services/common` policy-grammar module that takes the per-service wording from
/// its caller and that every copy — this one included — then calls. Until that lands
/// the document is still parsed by `serde_json` rather than by a hand-rolled parser,
/// and only the element checks live here.
pub fn validate_resource_policy(document: &str) -> Result<(), String> {
    let parsed: ::serde_json::Value = ::serde_json::from_str(document)
        .map_err(|error| format!("The resource policy is not valid JSON: {error}"))?;
    let Some(members) = parsed.as_object() else {
        return Err(
            "The resource policy must be a JSON object carrying a Statement element".to_string(),
        );
    };

    let statement = members.get("Statement").ok_or_else(|| {
        "The resource policy carries no Statement element: a policy document states at least \
         one statement"
            .to_string()
    })?;
    let statements: Vec<&::serde_json::Value> = match statement {
        // The Statement element is a statement or an array of statements.
        ::serde_json::Value::Array(items) => items.iter().collect(),
        ::serde_json::Value::Object(_) => vec![statement],
        _ => {
            return Err(
                "The resource policy's Statement element must be a statement or an array of \
                 statements"
                    .to_string(),
            )
        }
    };
    if statements.is_empty() {
        return Err(
            "The resource policy's Statement element is empty: a policy document states at \
             least one statement"
                .to_string(),
        );
    }

    for (index, statement) in statements.iter().enumerate() {
        // 1-based, so the message counts statements the way a caller reads them.
        let position = index + 1;
        let Some(elements) = statement.as_object() else {
            return Err(format!(
                "Statement {position} of the resource policy is not a JSON object"
            ));
        };

        let effect = elements.get("Effect").and_then(::serde_json::Value::as_str);
        if !effect.is_some_and(|effect| POLICY_EFFECTS.contains(&effect)) {
            return Err(format!(
                "Statement {position} of the resource policy must state an Effect of {} or {}",
                POLICY_EFFECTS[0], POLICY_EFFECTS[1]
            ));
        }
        if !elements.contains_key("Action") && !elements.contains_key("NotAction") {
            return Err(format!(
                "Statement {position} of the resource policy states no Action or NotAction"
            ));
        }
        let principal = elements
            .get("Principal")
            .or_else(|| elements.get("NotPrincipal"));
        let Some(principal) = principal else {
            return Err(format!(
                "Statement {position} of the resource policy states no Principal: a \
                 resource-based policy names the principal it grants access to"
            ));
        };
        if !is_valid_principal(principal) {
            return Err(format!(
                "Statement {position} of the resource policy contains a principal that is not \
                 valid: a Principal is `*`, or a map of principal type to one or more \
                 non-empty identifiers"
            ));
        }
    }

    Ok(())
}

/// Whether a statement's `Principal` element names a principal at all: the `*`
/// wildcard, or a map of principal type (`AWS`, `Service`, …) to one identifier or a
/// non-empty list of them, as the IAM JSON policy `Principal` element documents.
fn is_valid_principal(principal: &::serde_json::Value) -> bool {
    let named = |value: &::serde_json::Value| {
        value
            .as_str()
            .is_some_and(|identifier| !identifier.trim().is_empty())
    };
    match principal {
        ::serde_json::Value::String(_) => named(principal),
        ::serde_json::Value::Object(types) => {
            !types.is_empty()
                && types.values().all(|identifiers| match identifiers {
                    ::serde_json::Value::Array(items) => {
                        !items.is_empty() && items.iter().all(named)
                    }
                    other => named(other),
                })
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tagging: the resource a tagging call addresses by ARN
// ---------------------------------------------------------------------------

/// The HTTP status CloudTrail answers a tagging refusal with.
///
/// Every tagging refusal in the recorded corpus is HTTP 400, including the
/// `*NotFoundException` shapes the model marks `@httpError(404)`:
/// `cloudtrail-channel-notfound-error-paths#task#5` records `RemoveTags` answering a
/// channel ARN that names no channel with HTTP 400 and the error code
/// `ChannelNotFoundException`. A client branches on the status it is SENT, so the
/// emulator sends the one the service sends; the `*NotFoundException` shapes of the
/// tagging operations are the only place the model's status and the wire's disagree.
pub const TAGGING_REFUSAL_STATUS: u16 = 400;

/// How many ARNs one `ListTags` call may name, from the model doc for
/// `ListTagsRequest$ResourceIdList`: "The list has a limit of 20 ARNs."
pub const LIST_TAGS_RESOURCE_LIMIT: usize = 20;

/// A well-formed tagging ARN: which kind of resource it names, the Region that
/// resource lives in, and its name or id.
///
/// The four kinds are the ones the model doc for `AddTagsRequest$ResourceId` /
/// `RemoveTagsRequest$ResourceId` lists — "the ARN of the trail, event data store,
/// dashboard, or channel" — each with its own example ARN format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableArn {
    Trail { region: String, name: String },
    Dashboard { region: String, name: String },
    Channel { region: String, id: String },
    EventDataStore { region: String, id: String },
}

impl TaggableArn {
    /// The noun CloudTrail names this kind of resource by in a refusal, as the
    /// recorded `ChannelNotFoundException` message words it ("The channel `<arn>` was
    /// not found.").
    pub fn noun(&self) -> &'static str {
        match self {
            Self::Trail { .. } => "trail",
            Self::Dashboard { .. } => "dashboard",
            Self::Channel { .. } => "channel",
            Self::EventDataStore { .. } => "event data store",
        }
    }

    /// The refusal text for an ARN that is well formed but names no stored resource,
    /// as a tag WRITE (`AddTags` / `RemoveTags`) words it.
    ///
    /// CloudTrail words each kind its own way, and the recorded corpus pins three of
    /// the four: a trail by its NAME and the account the call is made for ("Unknown
    /// trail: `<name>` for the user: `<account>`"), a channel by the ARN the caller
    /// passed ("The channel `<arn>` was not found."), and a dashboard by one sentence
    /// naming nothing at all ("No matching dashboard found."). An event data store,
    /// which the corpus does not pin, follows the channel's wording — the kind it
    /// shares its ARN-addressed not-found shape with.
    pub fn not_found_message(&self, resource_id: &str) -> String {
        match self {
            Self::Trail { name, .. } => self.not_found_message_naming(name),
            _ => self.not_found_message_naming(resource_id),
        }
    }

    /// The same refusal as [`Self::not_found_message`], as `ListTags` words it.
    ///
    /// The ONE difference the corpus records between the two paths is the subject of
    /// a trail's wording: a tag write names the trail's name, `ListTags` the whole
    /// ARN the request passed.
    pub fn listed_not_found_message(&self, resource_id: &str) -> String {
        self.not_found_message_naming(resource_id)
    }

    /// The per-kind wording, naming `subject`.
    fn not_found_message_naming(&self, subject: &str) -> String {
        match self {
            // The one wording every operation that cannot find a trail answers with,
            // and likewise for a dashboard and an event data store.
            Self::Trail { .. } => unknown_trail_message(subject),
            Self::Dashboard { .. } => DASHBOARD_NOT_FOUND_MESSAGE.to_string(),
            Self::EventDataStore { .. } => event_data_store_not_found_message(subject),
            // A channel has no shared helper yet: `RemoveTags` is the first operation
            // to answer `ChannelNotFoundException`, and the recorded text is this one.
            Self::Channel { .. } => format!("The {} {subject} was not found.", self.noun()),
        }
    }
}

/// Why a tagging `ResourceId` can name no taggable resource.
///
/// Each variant names the modelled error the handler answers with, so every tagging
/// operation refuses the same malformed ARN the same way. The kind-specific ARN
/// errors exist because the model declares them: a channel ARN is refused as a
/// channel ARN, an event data store ARN as an event data store ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableArnRejection {
    /// Written as a channel ARN but not a well-formed one ⇒
    /// `ChannelARNInvalidException`.
    ChannelArn(String),
    /// Written as an event data store ARN but not a well-formed one ⇒
    /// `EventDataStoreARNInvalidException`.
    EventDataStoreArn(String),
    /// Not a CloudTrail resource ARN at all ⇒ `CloudTrailARNInvalidException`.
    CloudTrailArn(String),
    /// A CloudTrail ARN naming a resource type that carries no tags ⇒
    /// `ResourceTypeNotSupportedException`.
    UnsupportedResourceType { resource_id: String, kind: String },
}

impl TaggableArnRejection {
    /// The refusal text: built from the request, naming the ARN it refused, as the
    /// recorded corpus words it — `RemoveTags` answers an ARN whose resource segment
    /// names no taggable resource with "The ARN `<value>` is invalid." The
    /// kind-specific shapes word it the same way, naming their kind, and an
    /// unsupported resource type names the type it read.
    pub fn message(&self) -> String {
        match self {
            Self::ChannelArn(resource_id) => format!("The channel ARN {resource_id} is invalid."),
            Self::EventDataStoreArn(resource_id) => {
                format!("The event data store ARN {resource_id} is invalid.")
            }
            Self::CloudTrailArn(resource_id) => format!("The ARN {resource_id} is invalid."),
            Self::UnsupportedResourceType { resource_id, kind } => format!(
                "The resource type {kind} in the ARN {resource_id} is not supported by \
                 CloudTrail: tags are carried by a trail, an event data store, a dashboard \
                 or a channel."
            ),
        }
    }
}

/// The character set a CloudTrail resource ARN is written from.
///
/// The model constrains the ARN of three of the four taggable kinds — `ChannelArn`,
/// `EventDataStoreArn` and `DashboardArn`, each `^[a-zA-Z0-9._/\-:]+$`
/// (`smithy_query(query="shape", shape="ChannelArn,EventDataStoreArn,DashboardArn")`) —
/// so that set is what a CloudTrail ARN is spelled with, whatever resource type is
/// written inside it. A tagging `ResourceId` carries no `@pattern` of its own (it
/// targets a plain `String`), so nothing has applied this before
/// [`parse_taggable_arn`] does.
const ARN_PUNCTUATION: [char; 5] = ['.', '_', '/', '-', ':'];

/// Whether `value` is written only from the characters a CloudTrail resource ARN is
/// spelled with ([`ARN_PUNCTUATION`] plus ASCII letters and digits).
fn is_arn_spelling(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ARN_PUNCTUATION.contains(&c))
}

/// Read a tagging `ResourceId` as the ARN of one of the four taggable resources.
///
/// The member is documented as "the ARN of the trail, event data store, dashboard, or
/// channel", so a value that is not such an ARN names no resource and is refused
/// rather than read as a bare name. The refusal names the kind of ARN the caller
/// MEANT — the resource type written in the value — which stays readable even when
/// the rest of the ARN is not.
///
/// The ARN itself is read through [`::service_common::arn`], the one parse of a
/// caller-supplied resource ARN in this tree, so the resource field keeps the colons
/// the model's `ChannelArn` / `EventDataStoreArn` pattern (`^[a-zA-Z0-9._/\-:]+$`)
/// admits and the `<type>/<id>` and `<type>:<id>` forms are read the same way. What
/// this function adds is this service's own part: that the value is spelled like an ARN
/// at all ([`is_arn_spelling`]), WHICH of the four kinds the ARN names, and that kind's
/// naming rules.
///
/// It is also why a tagging call does not read its identifier through
/// [`parse_trail_identifier`] or [`parse_event_data_store_identifier`]: those accept
/// the bare name / bare id form their OWN operations document, and each already knows
/// which kind of resource it is looking for, while a tagging `ResourceId` has to be
/// classified by kind before it can be read at all.
///
/// [`parse_resource_arn`] classifies the same four kinds for the resource-POLICY
/// surface; see the note there for what keeps the two apart and what merging them
/// would take.
pub fn parse_taggable_arn(value: &str) -> Result<TaggableArn, TaggableArnRejection> {
    // A value carrying a character no CloudTrail resource ARN is spelled with is not
    // one, whatever resource type is written inside it, so it is refused as an ARN
    // before its kind is read: the kind-specific errors below answer for a value that
    // IS written as that kind's ARN, and reading a kind out of a value that is no ARN
    // would let a resource id the model's ARN pattern cannot hold be looked up as if
    // it named something.
    if !is_arn_spelling(value) {
        return Err(TaggableArnRejection::CloudTrailArn(value.to_string()));
    }

    let reject = || {
        if value.contains(CHANNEL_RESOURCE_PREFIX) {
            TaggableArnRejection::ChannelArn(value.to_string())
        } else if value.contains(EVENT_DATA_STORE_RESOURCE_PREFIX) {
            TaggableArnRejection::EventDataStoreArn(value.to_string())
        } else {
            TaggableArnRejection::CloudTrailArn(value.to_string())
        }
    };

    let Some(arn) = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE) else {
        return Err(reject());
    };
    // Every taggable CloudTrail resource lives in a Region and belongs to an account,
    // so an ARN leaving either field empty names none of them — unlike the
    // partition-global ARNs the shared parse deliberately admits.
    if arn.region.is_empty() || arn.account.is_empty() {
        return Err(reject());
    }
    let resource_type = arn.resource_type();
    let Some(id) = ::service_common::arn::resource_id_of_type(value, resource_type) else {
        return Err(reject());
    };
    if id.is_empty() {
        return Err(reject());
    }

    let region = arn.region.to_string();
    // The resource type is the segment the four example ARN formats differ in; each
    // one's id has to satisfy that resource's own naming rules.
    match resource_type {
        "trail" => {
            validate_trail_name(id).map_err(|_| reject())?;
            Ok(TaggableArn::Trail {
                region,
                name: id.to_string(),
            })
        }
        "dashboard" => {
            if !is_valid_dashboard_name(id) {
                return Err(reject());
            }
            Ok(TaggableArn::Dashboard {
                region,
                name: id.to_string(),
            })
        }
        // A channel id and an event data store id are opaque to this service: the
        // model constrains the WHOLE ARN (`ChannelArn` / `EventDataStoreArn`, length
        // 3..256 of `^[a-zA-Z0-9._/\-:]+$`), whose character set every value reaching
        // here already satisfies (see `is_arn_spelling` above), and says nothing
        // further about the id itself.
        "channel" => Ok(TaggableArn::Channel {
            region,
            id: id.to_string(),
        }),
        "eventdatastore" => Ok(TaggableArn::EventDataStore {
            region,
            id: id.to_string(),
        }),
        kind => Err(TaggableArnRejection::UnsupportedResourceType {
            resource_id: value.to_string(),
            kind: kind.to_string(),
        }),
    }
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// The tag set stored for the resource `arn` names, or `None` when no stored
    /// resource answers to it.
    ///
    /// An event data store and a channel are reachable only from the Region they live
    /// in, which is the Region their ARN pins — the same rule [`Self::event_data_store`]
    /// and [`Self::addressed_channel`] apply. A channel the registry does not hold (no
    /// channel carries that id in that Region) answers `None`, which its caller reports
    /// as `ChannelNotFoundException`.
    pub fn resource_tags(&self, arn: &TaggableArn) -> Option<&[Tag]> {
        match arn {
            TaggableArn::Trail { region, name } => self
                .trails
                .get(&(name.clone(), region.clone()))
                .map(|record| record.tags.as_slice()),
            TaggableArn::Dashboard { region, name } => self
                .dashboards
                .get(&(name.clone(), region.clone()))
                .map(|record| record.tags.as_slice()),
            TaggableArn::EventDataStore { region, id } => self
                .event_data_stores
                .get(id)
                .filter(|record| record.region == *region)
                .map(|record| record.tags.as_slice()),
            // The channel registry is keyed by the id and Region a channel ARN pins
            // ([`channels::ChannelKey`]), which is exactly what a tagging ARN
            // carries — the same two fields a trail's or a store's ARN is looked up
            // by here.
            TaggableArn::Channel { region, id } => self
                .channels
                .get(&(id.clone(), region.clone()))
                .map(|record| record.tags.as_slice()),
        }
    }

    /// The tag set of the resource `arn` names, to change in place. See
    /// [`Self::resource_tags`] for what `None` means.
    pub fn resource_tags_mut(&mut self, arn: &TaggableArn) -> Option<&mut Vec<Tag>> {
        match arn {
            TaggableArn::Trail { region, name } => self
                .trails
                .get_mut(&(name.clone(), region.clone()))
                .map(|record| &mut record.tags),
            TaggableArn::Dashboard { region, name } => self
                .dashboards
                .get_mut(&(name.clone(), region.clone()))
                .map(|record| &mut record.tags),
            TaggableArn::EventDataStore { region, id } => self
                .event_data_stores
                .get_mut(id)
                .filter(|record| record.region == *region)
                .map(|record| &mut record.tags),
            TaggableArn::Channel { region, id } => self
                .channels
                .get_mut(&(id.clone(), region.clone()))
                .map(|record| &mut record.tags),
        }
    }
}

/// Merge `added` into `existing` the way `AddTags` documents it: "Overwrites an
/// existing tag's value when a new value is specified for an existing tag key", and a
/// key not already on the resource is appended.
///
/// The merged set is handed back rather than written in place, so a caller can hold it
/// against the per-resource limit before it becomes the resource's tag set.
pub fn tags_merged(existing: &[Tag], added: Vec<Tag>) -> Vec<Tag> {
    let mut merged = existing.to_vec();
    for tag in added {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => merged.push(tag),
        }
    }
    merged
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
