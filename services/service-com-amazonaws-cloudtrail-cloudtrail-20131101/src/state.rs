// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudtrail-cloudtrail-20131101 service.
//!
//! This module owns the trail registry — the service's primary resource — plus the
//! rules every trail operation shares: how a caller-supplied identifier (a bare name
//! or a trail ARN) resolves to a stored trail, which trails a request's Region can
//! see, and how a stored trail projects onto the modelled `Trail` / `TrailInfo`
//! shapes. It lives here, not in one handler, because seven operations apply the same
//! rules and two copies of them would drift.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::SystemTime;

use crate::ops::types::{Tag, Trail, TrailInfo};

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
    /// Set by `PutEventSelectors` / `PutInsightSelectors`, which are not
    /// implemented yet, so a trail is created without either.
    pub has_custom_event_selectors: bool,
    /// See [`TrailRecord::has_custom_event_selectors`].
    pub has_insight_selectors: bool,
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
            has_custom_event_selectors: Some(self.has_custom_event_selectors),
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

/// Check a bare trail name against the rules the model documents for
/// `CreateTrailRequest$Name`:
///
/// * only ASCII letters, digits, periods, underscores or dashes,
/// * starts and ends with a letter or digit,
/// * between [`TRAIL_NAME_MIN_LEN`] and [`TRAIL_NAME_MAX_LEN`] characters,
/// * no two adjacent periods, underscores or dashes,
/// * not in IP-address format.
pub fn validate_trail_name(name: &str) -> Result<(), IdentifierRejection> {
    let reject = || {
        Err(IdentifierRejection::InvalidName(format!(
            "Trail name '{name}' is not valid: a trail name must contain only ASCII \
             letters, numbers, periods (.), underscores (_) or dashes (-), start and end \
             with a letter or number, be between {TRAIL_NAME_MIN_LEN} and \
             {TRAIL_NAME_MAX_LEN} characters, have no adjacent periods, underscores or \
             dashes, and not be in IP address format"
        )))
    };

    let length = name.chars().count();
    if !(TRAIL_NAME_MIN_LEN..=TRAIL_NAME_MAX_LEN).contains(&length) {
        return reject();
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || TRAIL_NAME_PUNCTUATION.contains(&c))
    {
        return reject();
    }
    let edges_are_alphanumeric = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if !edges_are_alphanumeric {
        return reject();
    }
    let punctuation = |c: &char| TRAIL_NAME_PUNCTUATION.contains(c);
    if name
        .chars()
        .zip(name.chars().skip(1))
        .any(|(a, b)| punctuation(&a) && punctuation(&b))
    {
        return reject();
    }
    // "Not be in IP address format (for example, 192.168.5.4)" — a dotted quad is
    // exactly what `Ipv4Addr`'s parser accepts, and a name cannot hold the colons an
    // IPv6 literal needs.
    if name.parse::<Ipv4Addr>().is_ok() {
        return reject();
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

/// The registry key: a trail name and the Region the trail is homed in.
///
/// AWS scopes a trail name to the Region it was created in, so the name alone is
/// not a key: two Regions may each hold a trail of the same name, and each is
/// reachable only from its own Region (or by its own ARN). The name comes FIRST so
/// the `BTreeMap` iterates name-ordered, which is the order `DescribeTrails` and
/// `ListTrails` report and the order `ListTrails` pages through.
pub type TrailKey = (String, String);

/// Per-service state for com-amazonaws-cloudtrail-cloudtrail-20131101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudtrailCloudtrail20131101State {
    /// Every trail in the account, keyed by [`TrailKey`].
    pub trails: BTreeMap<TrailKey, TrailRecord>,
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
