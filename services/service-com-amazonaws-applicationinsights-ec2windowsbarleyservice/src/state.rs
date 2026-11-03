// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationinsights-ec2windowsbarleyservice service.
//!
//! Holds the application + component registry the handlers in `src/ops/` read and
//! write, and the pure helpers that turn a stored record into the shapes those
//! handlers return. Keeping the record -> shape mapping here is what makes
//! `CreateApplication`, `DescribeApplication` and `ListApplications` report the
//! SAME `ApplicationInfo` for one stored application instead of three
//! hand-assembled ones that can drift.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{ApplicationComponent, ApplicationInfo, Tag};

pub mod resource_groups;

/// The region a request is attributed to when it carries no SigV4 credential
/// scope. `emulator_server::emulator_arn`'s own documentation names `us-east-1`
/// as the emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, which is the region every resource it
/// resolves in another service belongs to.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// What Application Insights answers when the resource group a request names is
/// not a group of the account: the group name from the request, and the account
/// the caller is in.
pub fn resource_group_missing_message(resource_group_name: &str) -> String {
    format!(
        "Resource Group with name {resource_group_name} does not exist in {}",
        ::engine::emulator_account_id()
    )
}

/// What Application Insights answers when no application it monitors answers to
/// the resource group a request named — the whole sentence, with no variable part:
/// the request already names the group, and the answer is only that nothing here
/// monitors it.
pub const APPLICATION_MISSING_MESSAGE: &str = "Application does not exist";

/// What Application Insights answers when the application exists and holds no
/// custom component of the name a request asked for: the component name and the
/// resource group, both from the request.
pub fn component_missing_message(component_name: &str, resource_group_name: &str) -> String {
    format!("Custom component {component_name} does not exist for {resource_group_name}")
}

/// What Application Insights answers a request that would group anything other
/// than EC2 instances into a custom component.
pub const ONLY_EC2_INSTANCES_MESSAGE: &str = "Only EC2 Instance can be grouped.";

/// What Application Insights answers a request whose `AccountId` names an account
/// other than the caller's: the caller's account and the one it asked for.
pub fn foreign_account_message(requested_account_id: &str) -> String {
    format!(
        "Account {} is not able to access applications that belong to account {requested_account_id}",
        ::engine::emulator_account_id()
    )
}

/// The account whose applications a request asks for, when that is NOT the
/// caller's own account.
///
/// `AccountId` selects the owner of the resource groups an operation reads, and
/// this emulator serves exactly one account: the caller can only read its own
/// applications, so any other account it names is refused rather than answered
/// from local state. A request that names none asks for the caller's own.
pub fn foreign_account(account_id: Option<&str>) -> Option<&str> {
    account_id.filter(|asked| *asked != ::engine::emulator_account_id())
}

/// `LifeCycle` an application carries while Application Insights is still
/// onboarding the resources it was created over.
pub const LIFE_CYCLE_CREATING: &str = "CREATING";

/// `LifeCycle` an application carries once onboarding has finished. `LifeCycle`
/// is an unconstrained string in the model (`smithy_query(query="shape",
/// shape="ApplicationInfo")`), so these two are the emulator's own vocabulary.
pub const LIFE_CYCLE_ACTIVE: &str = "ACTIVE";

/// `Tier` a component carries until a configuration gives it a stack tier.
/// `DEFAULT` is a member of the model's `Tier` enum
/// (`smithy_query(query="shape",shape="Tier")`).
pub const TIER_DEFAULT: &str = "DEFAULT";

/// `GroupingType` value that asks for an application covering the whole account.
/// The only member of the model's `GroupingType` enum.
pub const GROUPING_TYPE_ACCOUNT_BASED: &str = "ACCOUNT_BASED";

/// `DiscoveryType` of an application onboarded from one resource group.
pub const DISCOVERY_TYPE_RESOURCE_GROUP_BASED: &str = "RESOURCE_GROUP_BASED";

/// `DiscoveryType` of an application onboarded from every resource in the
/// account. Both values are members of the model's `DiscoveryType` enum.
pub const DISCOVERY_TYPE_ACCOUNT_BASED: &str = "ACCOUNT_BASED";

/// How long after `CreateApplication` the onboarding that call started is due to
/// finish.
///
/// CHOSEN, not documented: AWS publishes no onboarding duration, so this is the
/// emulator's own span. It is deliberately non-zero, so a caller that reads the
/// application straight back still observes `CREATING`, and it is stored as a
/// due instant on the application rather than waited for.
pub const ONBOARDING_SPAN: Duration = Duration::from_millis(500);

/// Page size `ListApplications` / `ListComponents` use when the caller names no
/// `MaxResults`, and the largest either accepts: the modelled range of the
/// `MaxResults` member (`MaxEntities`) is 1..40.
pub const MAX_PAGE_SIZE: i32 = 40;

/// What Application Insights answers a `NextToken` it cannot read.
///
/// A pagination token carries a WRAPPED PAYLOAD, and the first field of that
/// payload says which wrapping schema the rest of it is written in. A token this
/// service did not issue declares a schema it does not write, so the refusal
/// names the operation that was called and the schema id read off the token —
/// both taken from the request, nothing stored.
pub fn invalid_next_token_message(operation: &str, token: &str) -> String {
    format!(
        "Pagination token exception in operation '{operation}': invalid payload wrapping schema {}",
        token_wrapping_schema(token)
    )
}

/// The wrapping schema a pagination token's payload declares: the 16-bit signed
/// big-endian integer its first two bytes carry.
///
/// A token is base64 text, so the first FOUR characters of that text are the one
/// group the two bytes come from. Characters outside the alphabet are not part of
/// the payload, and the two alphabets (standard and URL-safe) encode the same
/// bits, so the text is reduced to the standard alphabet before it is decoded. A
/// payload shorter than one group declares the schema that group does, with the
/// bits it does not carry unset — so every token this service refuses reports the
/// schema its payload begins with.
fn token_wrapping_schema(token: &str) -> i16 {
    /// The character a zero sextet is written as: what a group the token does not
    /// fill out is padded with.
    const ZERO: char = 'A';

    let group: String = token
        .chars()
        .filter_map(|character| match character {
            // The URL-safe alphabet's last two characters, as the standard
            // alphabet writes the same two values.
            '-' => Some('+'),
            '_' => Some('/'),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => Some(character),
            _ => None,
        })
        .take(4)
        .collect();
    let group: String = group
        .chars()
        .chain(::std::iter::repeat(ZERO))
        .take(4)
        .collect();
    let payload = ::base64::Engine::decode(&::base64::engine::general_purpose::STANDARD, group)
        .unwrap_or_default();
    i16::from_be_bytes([
        payload.first().copied().unwrap_or_default(),
        payload.get(1).copied().unwrap_or_default(),
    ])
}

/// How many applications the store holds before `CreateApplication` refuses.
///
/// Amazon CloudWatch Application Insights publishes an adjustable quota on
/// applications per account per Region, and this emulator models neither Service
/// Quotas nor an account's adjustments — so this is the emulator's OWN bound on
/// the store rather than a claim about any account's limit. It is set far above
/// any workflow an emulator serves, and it is what stops the state growing
/// without end: every record it holds is released by `DeleteApplication`, which
/// drops that application's components with it.
pub const APPLICATION_QUOTA: usize = 1_000;

/// How many components one application holds before `CreateComponent` refuses.
/// The emulator's own bound, for the same reason as [`APPLICATION_QUOTA`], and
/// released with the application that holds them.
pub const COMPONENT_QUOTA: usize = 500;

/// How many resource ARNs one component stores. `ResourceList` carries no
/// modelled length bound and is kept verbatim, so this is the emulator's own
/// bound on what a single request may add to the store.
pub const COMPONENT_RESOURCE_QUOTA: usize = 1_000;

/// Marker every `NextToken` this service issues carries, so a token the service
/// did not mint is refused rather than read as a key. Without it any even-length
/// hex string would decode to some key and silently resume paging there.
const TOKEN_MARKER: &str = "applicationinsights:v1:";

/// One application, keyed in [`ComAmazonawsApplicationinsightsEc2windowsbarleyserviceState::applications`]
/// by its `ResourceGroupName`.
#[derive(Debug, Clone)]
pub struct Application {
    /// The resource group the application was created over; also its key.
    pub resource_group_name: String,
    /// The account that owns the application — this emulator's single account.
    pub account_id: String,
    pub ops_center_enabled: Option<bool>,
    pub cwe_monitor_enabled: Option<bool>,
    pub auto_config_enabled: Option<bool>,
    pub ops_item_sns_topic_arn: Option<String>,
    pub sns_notification_arn: Option<String>,
    pub attach_missing_permission: Option<bool>,
    /// How the application was onboarded; see [`discovery_type_for`].
    pub discovery_type: Option<String>,
    /// User-side issues blocking monitoring. Nothing blocks an emulated
    /// application, so it stays absent until a configuration slice sets it.
    pub remarks: Option<String>,
    /// The tags `CreateApplication` was given. Stored so the write is not
    /// discarded; no implemented operation reports them yet, because the tagging
    /// operations (`TagResource` / `UntagResource` / `ListTagsForResource`) are a
    /// later slice and still answer 501.
    pub tags: Vec<Tag>,
    /// Components keyed by `ComponentName`; a `BTreeMap` so a list is in a
    /// stable order and a pagination token resolves against it.
    pub components: BTreeMap<String, Component>,
    /// When onboarding is due to finish: the instant the create call made
    /// `LifeCycle` move off `CREATING`. Stored, so nothing sleeps and every
    /// reader computes the same answer from the clock.
    pub onboarded_at: SystemTime,
}

impl Application {
    /// The `LifeCycle` at `now`. The clock moves it, not the act of reading.
    pub fn life_cycle(&self, now: SystemTime) -> &'static str {
        if now < self.onboarded_at {
            LIFE_CYCLE_CREATING
        } else {
            LIFE_CYCLE_ACTIVE
        }
    }

    /// The `ApplicationInfo` every operation that reports this application
    /// returns, built only from what is stored plus the clock.
    pub fn info(&self, now: SystemTime) -> ApplicationInfo {
        ApplicationInfo {
            account_id: Some(self.account_id.clone()),
            resource_group_name: Some(self.resource_group_name.clone()),
            life_cycle: Some(self.life_cycle(now).to_string()),
            ops_item_sns_topic_arn: self.ops_item_sns_topic_arn.clone(),
            sns_notification_arn: self.sns_notification_arn.clone(),
            ops_center_enabled: self.ops_center_enabled,
            cwe_monitor_enabled: self.cwe_monitor_enabled,
            remarks: self.remarks.clone(),
            auto_config_enabled: self.auto_config_enabled,
            discovery_type: self.discovery_type.clone(),
            attach_missing_permission: self.attach_missing_permission,
        }
    }
}

/// One component of an application, keyed in [`Application::components`] by its
/// `ComponentName`.
#[derive(Debug, Clone)]
pub struct Component {
    pub component_name: String,
    /// The ARNs the component groups, exactly as `CreateComponent` supplied them.
    pub resource_list: Vec<String>,
    /// The single resource type the grouped ARNs name; see [`resource_type_of`].
    pub resource_type: Option<String>,
    /// The operating system of the component. Nothing in this emulator's state
    /// says what a grouped resource runs, so it stays absent rather than being
    /// guessed.
    pub os_type: Option<String>,
    /// The stack tier; [`TIER_DEFAULT`] until a configuration slice sets one.
    pub tier: String,
    /// Whether the component is monitored. A component is monitored once its
    /// configuration says so, which no implemented operation sets yet.
    pub monitor: bool,
    /// Whether the component has logs configured for monitoring; absent until a
    /// configuration slice sets it.
    pub component_remarks: Option<String>,
}

impl Component {
    /// The `ApplicationComponent` every operation that reports this component
    /// returns, built only from what is stored.
    pub fn describe(&self) -> ApplicationComponent {
        ApplicationComponent {
            component_name: Some(self.component_name.clone()),
            component_remarks: self.component_remarks.clone(),
            resource_type: self.resource_type.clone(),
            os_type: self.os_type.clone(),
            tier: Some(self.tier.clone()),
            monitor: Some(self.monitor),
            detected_workload: None,
        }
    }
}

/// The `DiscoveryType` an application gets from the `GroupingType` its create
/// call named: `ACCOUNT_BASED` onboards every resource in the account, anything
/// else onboards the named resource group.
pub fn discovery_type_for(grouping_type: Option<&str>) -> &'static str {
    if grouping_type == Some(GROUPING_TYPE_ACCOUNT_BASED) {
        DISCOVERY_TYPE_ACCOUNT_BASED
    } else {
        DISCOVERY_TYPE_RESOURCE_GROUP_BASED
    }
}

/// The `ResourceType` an EC2 instance carries, as the resource-type vocabulary
/// `ApplicationComponent$ResourceType` documents names it.
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// Whether `arn` names an EC2 instance.
///
/// A CUSTOM component groups standalone instances to monitor
/// (`smithy_query(query="operation",operation="CreateComponent")`: "Creates a
/// custom component by grouping similar standalone instances to monitor"), so
/// this is what decides whether a resource a request asks to group may be
/// grouped at all.
pub fn is_ec2_instance(arn: &str) -> bool {
    resource_type_of_arn(arn) == Some(RESOURCE_TYPE_EC2_INSTANCE)
}

/// The resource type a component reports, derived from the ARNs it groups.
///
/// The supported set is the one the `ApplicationComponent$ResourceType` member
/// documents — "Supported resource types include EC2 instances, Auto Scaling
/// group, Classic ELB, Application ELB, and SQS Queue"
/// (`smithy_query(query="shape",shape="ApplicationComponent")`). A component
/// whose resources are not all of one supported type has no single type to
/// report, so the member stays absent rather than naming one of them.
///
/// A CUSTOM component groups EC2 instances only ([`is_ec2_instance`]), so one
/// reports their type; the rest of that set is what a component Application
/// Insights discovers from the resource group itself can carry.
pub fn resource_type_of(resources: &[String]) -> Option<String> {
    let mut kinds = resources.iter().map(|arn| resource_type_of_arn(arn));
    let first = kinds.next()??;
    kinds
        .all(|other| other == Some(first))
        .then(|| first.to_string())
}

/// The AWS resource type one ARN names, for the types Application Insights
/// supports; `None` for an ARN of any other type, or one that is not an ARN.
///
/// Every returned value satisfies the `ResourceType` member's modelled
/// constraints (1..50 characters matching `^[0-9a-zA-Z:_]*$`).
fn resource_type_of_arn(arn: &str) -> Option<&'static str> {
    // arn:<partition>:<service>:<region>:<account>:<resource>
    let mut fields = arn.splitn(6, ':');
    let (
        "arn",
        Some(_partition),
        Some(service),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        fields.next()?,
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    // The resource portion is either `<kind>/<id>`, `<kind>:<id>`, or a bare
    // name (SQS queues), so the kind is its first segment under either separator.
    let kind = resource.split(['/', ':']).next().unwrap_or(resource);
    match (service, kind) {
        ("ec2", "instance") => Some(RESOURCE_TYPE_EC2_INSTANCE),
        ("autoscaling", "autoScalingGroup") => Some("AWS::AutoScaling::AutoScalingGroup"),
        // One ARN kind covers both load balancers; only the v2 (Application and
        // Network) ones carry a type segment after `loadbalancer/`.
        ("elasticloadbalancing", "loadbalancer") => {
            let detail = resource.strip_prefix("loadbalancer/").unwrap_or("");
            if detail.starts_with("app/") || detail.starts_with("net/") {
                Some("AWS::ElasticLoadBalancingV2::LoadBalancer")
            } else {
                Some("AWS::ElasticLoadBalancing::LoadBalancer")
            }
        }
        ("sqs", _) => Some("AWS::SQS::Queue"),
        _ => None,
    }
}

/// One page of a keyed collection, plus the token that continues it.
#[derive(Debug)]
pub struct Page<'a, V> {
    /// The values on this page, in key order.
    pub values: Vec<&'a V>,
    /// The token a caller passes as `NextToken` to continue; `None` on the last
    /// page.
    pub next_token: Option<String>,
}

/// Take one page of `items`, continuing after the key `next_token` encodes.
///
/// `max_results` is the page size the caller asked for — already inside the
/// modelled 1..40 range by the time a handler runs, and clamped here so a
/// future caller of this helper cannot ask for an unbounded page. Absent means
/// [`MAX_PAGE_SIZE`]. A token this service did not issue is refused with
/// [`invalid_next_token_message`], which names `operation` — the operation the
/// caller asked for, as its `Operation::NAME` spells it.
pub fn paginate<'a, V>(
    items: &'a BTreeMap<String, V>,
    next_token: Option<&str>,
    max_results: Option<i32>,
    operation: &str,
) -> Result<Page<'a, V>, String> {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let limit = usize::try_from(limit).unwrap_or(1);
    let after = match next_token {
        Some(token) => Bound::Excluded(
            decode_token(token)
                .ok_or_else(|| invalid_next_token_message(operation, token))?,
        ),
        None => Bound::Unbounded,
    };
    let mut rest = items.range((after, Bound::Unbounded));
    let mut values = Vec::new();
    let mut last: Option<&str> = None;
    for (key, value) in rest.by_ref().take(limit) {
        values.push(value);
        last = Some(key.as_str());
    }
    // A token is emitted only when another entry actually follows this page, so
    // a caller that follows tokens never makes a call that returns nothing.
    let next_token = match (rest.next(), last) {
        (Some(_), Some(key)) => Some(encode_token(key)),
        _ => None,
    };
    Ok(Page { values, next_token })
}

/// Encode the key a page stopped after as an opaque `NextToken`.
///
/// Hex round-trips exactly and is always non-empty for a non-empty key, so the
/// token satisfies the `PaginationToken` constraints (1..1024 characters
/// matching `^.+$`) and the next call resolves it against the same map — no
/// server-side cursor table.
pub fn encode_token(key: &str) -> String {
    hex::encode(format!("{TOKEN_MARKER}{key}").as_bytes())
}

/// The key an [`encode_token`] token names; `None` for anything this service did
/// not mint, including a well-formed hex string that carries no [`TOKEN_MARKER`].
pub fn decode_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(TOKEN_MARKER).map(str::to_string)
}

/// Per-service state for com-amazonaws-applicationinsights-ec2windowsbarleyservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationinsightsEc2windowsbarleyserviceState {
    /// Applications keyed by `ResourceGroupName`. A `BTreeMap` so `ListApplications`
    /// has a stable order and a pagination token resolves against it.
    pub applications: BTreeMap<String, Application>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationinsightsEc2windowsbarleyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationinsightsEc2windowsbarleyserviceState>)
}
