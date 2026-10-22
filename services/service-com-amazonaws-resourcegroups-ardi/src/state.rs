// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resourcegroups-ardi service.
//!
//! Every resource group this emulator holds lives in ONE map, [`ComAmazonawsResourcegroupsArdiState::groups`],
//! so `CreateGroup`, `GetGroup`, `GetGroupConfiguration`, `ListGroups`, `GroupResources`,
//! `ListGroupResources`, `UpdateGroup` and `DeleteGroup` all read and write the same
//! records. The map is a
//! [`BTreeMap`] keyed by group name: the key order is the listing order, which is
//! what makes `ListGroups` paging deterministic and resumable from an opaque
//! token even after a group in the middle of the listing is deleted.

use std::collections::BTreeMap;

use crate::ops::types::{
    Group, GroupConfiguration, GroupConfigurationItem, GroupIdentifier, ListGroupResourcesItem,
    ResourceIdentifier, ResourceQuery, ResourceStatus,
};

/// Per-service state for com-amazonaws-resourcegroups-ardi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResourcegroupsArdiState {
    /// Every persisted resource group, keyed by `GroupRecord::name`. Ordered, so
    /// a listing and its continuation token are stable.
    pub groups: BTreeMap<String, GroupRecord>,
}

impl ComAmazonawsResourcegroupsArdiState {
    /// The name of the group `identifier` names, or `None` when nothing stored
    /// answers to it.
    ///
    /// An identifier is either a group ARN or a bare group name, and it is read by
    /// SHAPE, not by prefix:
    ///
    /// * a full `arn:<partition>:resource-groups:<region>:<account>:group/<name>`
    ///   names exactly one resource, so it resolves only when it equals that
    ///   group's STORED ARN — which keeps an ARN for another account or region from
    ///   resolving to a same-named local group;
    /// * anything else must be a bare `GroupName`, which the model constrains to
    ///   `[a-zA-Z0-9_\.-]{1,300}` or `<name>/<ulid>` — no colon — so an identifier
    ///   that is neither a well-formed ARN nor a well-formed name resolves to
    ///   nothing instead of slipping past the ARN check.
    ///
    /// The caller picks the identifier: an operation that models both members takes
    /// `Group` and falls back to the deprecated `GroupName`, so the group a request
    /// resolves to never depends on which of the two a handler happens to read.
    pub fn lookup(&self, identifier: &str) -> Option<String> {
        if let Some((service, resource)) = parse_arn(identifier) {
            if service != "resource-groups" {
                return None;
            }
            let record = self.groups.get(resource.strip_prefix("group/")?)?;
            if identifier != record.group_arn {
                return None;
            }
            return Some(record.name.clone());
        }
        if identifier.contains(':') {
            return None;
        }
        Some(self.groups.get(identifier)?.name.clone())
    }
}

/// The most groups this emulator holds at once.
///
/// NOT an AWS quota: real Resource Groups enforces its own, and answers its own
/// limit error. This is the emulator's bound on in-memory fixture state, so a
/// client looping on `CreateGroup` is refused rather than growing the store
/// without end. Reaching it is a `BadRequestException`, and deleting a group
/// makes room again.
pub const MAX_GROUPS: usize = 1_000;

/// The most static memberships one group holds at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal.
pub const MAX_MEMBERS_PER_GROUP: usize = 1_000;

/// The modelled `GroupConfigurationStatus` of a service configuration that is in
/// effect: nothing is being applied to the group.
///
/// `CreateGroup` is synchronous — the model gives it HTTP 200 and a
/// `GroupConfiguration` in its own response, unlike `PutGroupConfiguration`
/// (HTTP 202) — so the configuration a create stores is already the one in
/// effect, with no update in flight to report.
pub const CONFIGURATION_IN_EFFECT: &str = "UPDATE_COMPLETE";

/// One persisted resource group: its identity, its describable attributes, the
/// query or the service configuration it was created with, its tags, and its
/// static members.
#[derive(Debug, Clone)]
pub struct GroupRecord {
    /// The group's name, which is also its key in [`ComAmazonawsResourcegroupsArdiState::groups`].
    pub name: String,
    /// `arn:aws:resource-groups:<region>:<account>:group/<name>`, minted once at
    /// create time and never recomputed, so every later read returns the same ARN.
    pub group_arn: String,
    /// The id of the account that owns the group, captured once at create time
    /// from the emulator's canonical account id — the same account the stored
    /// ARN names — so every later read reports the same owner.
    pub owner_id: String,
    pub description: Option<String>,
    pub criticality: Option<i32>,
    pub owner: Option<String>,
    pub display_name: Option<String>,
    /// The resource query the group was created with, if any.
    pub resource_query: Option<ResourceQueryRecord>,
    /// The service configuration the group was created with, if any. A group
    /// contains either a configuration or a resource query, never both, and a
    /// group created with neither carries `None` here.
    pub configuration: Option<Vec<GroupConfigurationItem>>,
    /// The group's tags, kept in key order so the stored record is deterministic
    /// (the wire map itself is unordered).
    pub tags: BTreeMap<String, String>,
    /// Static members added by `GroupResources`, kept sorted by ARN: the sort is
    /// the listing order `ListGroupResources` pages through.
    pub members: Vec<MemberRecord>,
}

impl GroupRecord {
    /// The modelled `Group` structure for this record.
    pub fn to_group(&self) -> Group {
        Group {
            owner_id: Some(self.owner_id.clone()),
            group_arn: self.group_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            criticality: self.criticality,
            owner: self.owner.clone(),
            display_name: self.display_name.clone(),
            // Only an application group (created through the Service Catalog
            // AppRegistry integration) carries one, and this slice creates none.
            application_tag: None,
        }
    }

    /// The modelled `GroupIdentifier` for this record, as `ListGroups` reports it.
    pub fn to_identifier(&self) -> GroupIdentifier {
        GroupIdentifier {
            group_name: Some(self.name.clone()),
            group_arn: Some(self.group_arn.clone()),
            description: self.description.clone(),
            criticality: self.criticality,
            owner: self.owner.clone(),
            display_name: self.display_name.clone(),
        }
    }

    /// The modelled `GroupConfiguration` for this record, if it was created with
    /// a service configuration, and `None` for a group that has none.
    ///
    /// `ProposedConfiguration` is reported only while an update is in flight, and
    /// `FailureReason` only for one that failed; a stored configuration is in
    /// effect, so neither is reported here.
    pub fn to_group_configuration(&self) -> Option<GroupConfiguration> {
        self.configuration.as_ref().map(|items| GroupConfiguration {
            configuration: Some(items.clone()),
            proposed_configuration: None,
            status: Some(CONFIGURATION_IN_EFFECT.to_string()),
            failure_reason: None,
        })
    }

    /// The modelled `ResourceQuery` for this record, if it was created with one.
    pub fn to_resource_query(&self) -> Option<ResourceQuery> {
        self.resource_query.as_ref().map(|q| ResourceQuery {
            r#type: q.query_type.clone(),
            query: q.query.clone(),
        })
    }

    /// This record's tags in the shape the wire uses.
    pub fn tags_map(&self) -> std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Record `resource_arn` as a member, keeping [`GroupRecord::members`] sorted
    /// by ARN. Adding an ARN that is already a member changes nothing, so
    /// repeating a `GroupResources` call does not duplicate a membership.
    pub fn add_member(&mut self, resource_arn: &str) {
        let at = self
            .members
            .partition_point(|m| m.resource_arn.as_str() < resource_arn);
        if self
            .members
            .get(at)
            .is_some_and(|m| m.resource_arn == resource_arn)
        {
            return;
        }
        self.members.insert(
            at,
            MemberRecord {
                resource_arn: resource_arn.to_string(),
                resource_type: resource_type_of_arn(resource_arn).map(str::to_string),
                // The modelled `ResourceStatusValue` has one value, `PENDING`,
                // and the model documents it as "the process isn't completed
                // yet". A member this emulator reports in `Succeeded` is already
                // part of the group, so it carries no status.
                status: None,
            },
        );
    }
}

/// The resource query a group was created with.
#[derive(Debug, Clone)]
pub struct ResourceQueryRecord {
    /// A modelled `QueryType` value: `TAG_FILTERS_1_0` or `CLOUDFORMATION_STACK_1_0`.
    pub query_type: String,
    /// The query document, stored verbatim as the caller sent it.
    pub query: String,
}

/// One static membership: a resource ARN added to a group by `GroupResources`.
#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub resource_arn: String,
    /// The CloudFormation-style type name derived from the ARN, when
    /// [`resource_type_of_arn`] knows it; `None` otherwise.
    pub resource_type: Option<String>,
    /// A modelled `ResourceStatusValue`, set only while a membership is still
    /// being applied asynchronously.
    pub status: Option<String>,
}

impl MemberRecord {
    /// The modelled `ResourceIdentifier` for this membership.
    pub fn to_identifier(&self) -> ResourceIdentifier {
        ResourceIdentifier {
            resource_arn: Some(self.resource_arn.clone()),
            resource_type: self.resource_type.clone(),
        }
    }

    /// The modelled `ListGroupResourcesItem` for this membership.
    pub fn to_item(&self) -> ListGroupResourcesItem {
        ListGroupResourcesItem {
            identifier: Some(self.to_identifier()),
            status: self
                .status
                .as_ref()
                .map(|name| ResourceStatus {
                    name: Some(name.clone()),
                }),
        }
    }
}

/// The region every ARN this emulator mints for an unsigned request carries.
/// `emulator_server::emulator_arn`'s own documentation names `us-east-1` as the
/// emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to mint an ARN in for this request: the one in the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries no scope or
/// a scope region that is not shaped like an AWS region (the modelled `GroupArn`
/// pattern requires `[a-z]{2}(-[a-z]+)+-\d{1}`).
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `target` every audit record this service emits carries, so a consumer can
/// select the access trail without reading the rest of the emulator's traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the operation,
/// the caller's presented access key id, the region the request resolved to, the
/// resource it changed and the outcome. Metadata only — never a request body, a
/// tag value or a credential secret.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in its
/// SigV4 credential scope. `emulator_server::extract_akid` answers `"default"`
/// when the request carries no scope. An access key id NAMES a caller and is not
/// a secret — the signature and the secret key are never read here.
pub fn request_caller(parts: &http::request::Parts) -> String {
    emulator_server::extract_akid(&parts.headers)
}

/// Whether `partition` matches the partition production the modelled ARN patterns
/// use: `aws(-[a-z]+)*`.
fn is_arn_partition(partition: &str) -> bool {
    let mut segments = partition.split('-');
    segments.next() == Some("aws")
        && segments.all(|segment| {
            !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_lowercase())
        })
}

/// Whether `region` matches the region production the modelled ARN patterns use:
/// `[a-z]{2}(-[a-z]+)+-\d{1}`.
fn is_arn_region(region: &str) -> bool {
    let Some((head, digit)) = region.rsplit_once('-') else {
        return false;
    };
    if digit.len() != 1 || !digit.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    let mut segments = head.split('-');
    let lowercase = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    match segments.next() {
        Some(first) if first.len() == 2 && lowercase(first) => {}
        _ => return false,
    }
    let mut middle = 0usize;
    for segment in segments {
        if !lowercase(segment) {
            return false;
        }
        middle += 1;
    }
    middle >= 1
}

/// The message this service answers when a group identifier resolves to no
/// stored group.
///
/// It is the service's own FIXED wording, and it names nothing the caller sent:
/// the identifier is already in the request, so echoing it back tells the caller
/// nothing the request does not already say. Every operation that addresses one
/// group answers the same sentence, so a client cannot tell them apart by their
/// 404 text.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "The specified group does not exist";

/// The ARN of the group named `name` in `region`, in the one form every
/// operation of this service reports: `arn:aws:resource-groups:<region>:<account>:group/<name>`.
/// The account segment is the emulator's canonical account id.
pub fn group_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn("resource-groups", region, &format!("group/{name}"))
}

/// The id of the account that owns every group this emulator holds: the
/// emulator's canonical account id, which is also the account segment of every
/// ARN [`group_arn`] mints.
pub fn owning_account_id() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// The marker every pagination token this service mints carries, before its
/// listing name and resume key. It is what makes a token verifiable: a string
/// that merely happens to be base64 is not a resume point, and a token issued by
/// one listing is not accepted by another.
const PAGE_TOKEN_PREFIX: &str = "resourcegroups/v1/";

/// Encode a listing's resume point — the key of the last item returned — as the
/// opaque `NextToken` the model describes (`^[a-zA-Z0-9+/]*={0,2}$`, i.e.
/// base64). `listing` names the listing the token belongs to.
pub fn encode_page_token(listing: &str, last_key: &str) -> String {
    aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{listing}/{last_key}"))
}

/// The resume point a `NextToken` carries, or `None` when the token was not
/// minted by `listing`.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_PREFIX)?
        .strip_prefix(listing)?
        .strip_prefix('/')
        .map(str::to_string)
}

/// The CloudFormation-style resource type an ARN names, e.g.
/// `arn:aws:ec2:us-east-1:000000000000:instance/i-0abc` -> `AWS::EC2::Instance`.
///
/// Derived from the ARN's own service segment and resource-type segment through
/// the table below; a resource whose type this table does not name resolves to
/// `None`, and the membership is then reported with no `ResourceType` rather
/// than with a guessed one (the member is optional in `ResourceIdentifier`).
///
/// The table's keys are the ARN formats in the AWS "Actions, resources, and
/// condition keys" service reference; its values are the type names from the AWS
/// CloudFormation resource and property types reference
/// (<https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/aws-template-resource-type-ref.html>),
/// which is the vocabulary `ResourceIdentifier$ResourceType` is documented in
/// ("such as AWS::EC2::Instance") and whose shape its `^AWS::[a-zA-Z0-9]+::\w+$`
/// pattern requires. An empty resource-type key is an ARN whose resource part is
/// a bare id (`arn:aws:sqs:<region>:<account>:<queue-name>`).
const RESOURCE_TYPES: &[(&str, &str, &str)] = &[
    ("cloudformation", "stack", "AWS::CloudFormation::Stack"),
    ("cloudwatch", "alarm", "AWS::CloudWatch::Alarm"),
    ("dynamodb", "table", "AWS::DynamoDB::Table"),
    ("ec2", "instance", "AWS::EC2::Instance"),
    ("ec2", "security-group", "AWS::EC2::SecurityGroup"),
    ("ec2", "snapshot", "AWS::EC2::Snapshot"),
    ("ec2", "subnet", "AWS::EC2::Subnet"),
    ("ec2", "volume", "AWS::EC2::Volume"),
    ("ec2", "vpc", "AWS::EC2::VPC"),
    ("ecs", "cluster", "AWS::ECS::Cluster"),
    ("ecs", "service", "AWS::ECS::Service"),
    ("iam", "role", "AWS::IAM::Role"),
    ("iam", "user", "AWS::IAM::User"),
    ("kms", "key", "AWS::KMS::Key"),
    ("lambda", "function", "AWS::Lambda::Function"),
    ("logs", "log-group", "AWS::Logs::LogGroup"),
    ("rds", "cluster", "AWS::RDS::DBCluster"),
    ("rds", "db", "AWS::RDS::DBInstance"),
    ("resource-groups", "group", "AWS::ResourceGroups::Group"),
    ("s3", "", "AWS::S3::Bucket"),
    ("sns", "", "AWS::SNS::Topic"),
    ("sqs", "", "AWS::SQS::Queue"),
    ("ssm", "document", "AWS::SSM::Document"),
    ("states", "stateMachine", "AWS::StepFunctions::StateMachine"),
];

/// The CloudFormation-style type name for `arn`, per [`RESOURCE_TYPES`].
pub fn resource_type_of_arn(arn: &str) -> Option<&'static str> {
    let (service, resource) = parse_arn(arn)?;
    // `type/id`, `type:id` and a bare `id` are the three resource forms an ARN
    // uses; the bare form has no type segment at all.
    let resource_type = resource
        .find([':', '/'])
        .map_or("", |split| &resource[..split]);
    RESOURCE_TYPES
        .iter()
        .find(|(s, t, _)| *s == service && *t == resource_type)
        .map(|(_, _, cfn)| *cfn)
}

/// Whether `arn` is shaped like the modelled `ResourceArn`
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
pub fn is_resource_arn(arn: &str) -> bool {
    parse_arn(arn).is_some()
}

/// The service and resource portions of `arn`, or `None` when the string is not
/// an ARN.
///
/// The shape is the modelled `ResourceArn`'s own:
/// `^arn:aws(-[a-z]+)*:[a-z0-9\-]*:([a-z]{2}(-[a-z]+)+-\d{1})?:([0-9]{12})?:.+$`
/// — six fields; a partition matching `aws(-[a-z]+)*`; a lowercase service; a
/// region that is either empty (a global service, as in `arn:aws:s3:::bucket`) or
/// shaped like an AWS region; an account that is either empty or twelve digits;
/// and a non-empty resource.
fn parse_arn(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if !is_arn_partition(partition) {
        return None;
    }
    if service.is_empty()
        || !service
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    if !region.is_empty() && !is_arn_region(region) {
        return None;
    }
    if !account.is_empty() && (account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if resource.is_empty() {
        return None;
    }
    Some((service, resource))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResourcegroupsArdiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResourcegroupsArdiState>)
}
