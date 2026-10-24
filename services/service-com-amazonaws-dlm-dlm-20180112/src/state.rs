// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dlm-dlm-20180112 service.
//!
//! The lifecycle policy is the primary resource of the Amazon Data Lifecycle
//! Manager control plane: `CreateLifecyclePolicy` mints one,
//! `GetLifecyclePolicy` / `GetLifecyclePolicies` report it,
//! `UpdateLifecyclePolicy` mutates it, `DeleteLifecyclePolicy` removes it and the
//! three tag operations read and write its tags. [`LifecyclePolicyRecord`] below
//! is the ONE copy of a policy every one of those handlers reads and writes, plus
//! the two projections the model asks for (`LifecyclePolicy` and
//! `LifecyclePolicySummary`).

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    CreateRule, CrossRegionCopyTarget, Exclusions, InvalidRequestException, LifecyclePolicy,
    LifecyclePolicySummary, PolicyDetails, ResourceNotFoundException, Tag,
};

/// The region a policy ARN is built for when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / sigv4 signing name this service's ARNs carry
/// (`smithy_query(query="service_info")`: `arnNamespace: dlm`).
pub const ARN_NAMESPACE: &str = "dlm";

/// The `ResourceType` a `ResourceNotFoundException` / `LimitExceededException`
/// reports for a lifecycle policy — "the type of resource that was not found".
///
/// `ResourceNotFoundException$ResourceType` is a WIRE token naming a kind of
/// Amazon Data Lifecycle Manager resource, which is what lets a client tell a
/// missing policy from another missing DLM resource; it is not prose and not an
/// internal name. The lifecycle policy's token is `LIFECYCLE_POLICY`.
pub const POLICY_RESOURCE_TYPE: &str = "LIFECYCLE_POLICY";

/// The `Code` every `InvalidRequestException` this service answers carries.
///
/// `InvalidRequestException$Code` is the service's own error code for the
/// exception class — the member a client branches on beside the shape name —
/// and Amazon Data Lifecycle Manager answers `InvalidParameter` for a request
/// whose parameters it refuses.
pub const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The `Code` every `ResourceNotFoundException` this service answers carries,
/// by the same rule as [`INVALID_PARAMETER_CODE`].
pub const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// The tag-key prefix AWS reserves for itself: a caller may not SET a tag whose
/// key begins with it, in any case.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// How Amazon Data Lifecycle Manager states the refusal of a reserved tag key.
pub const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// The policy type a policy carries when `PolicyDetails$PolicyType` names none:
/// "The default is `EBS_SNAPSHOT_MANAGEMENT`."
pub const DEFAULT_POLICY_TYPE: &str = "EBS_SNAPSHOT_MANAGEMENT";

/// The policy type of a policy whose target resource type is `INSTANCE`: a
/// default policy for `INSTANCE` "creates EBS-backed AMIs from all instances in
/// the Region", and `IMAGE_MANAGEMENT` is the `PolicyTypeValues` member that
/// "manages the lifecycle of EBS-backed AMIs".
pub const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The `PolicyLanguage` value that marks a policy as a DEFAULT policy:
/// `PolicyDetails$PolicyLanguage` documents `SIMPLIFIED` as "To create a default
/// policy" and `STANDARD` as "To create a custom policy".
pub const SIMPLIFIED_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The `PolicyLanguage` value that marks a policy as a CUSTOM one:
/// `PolicyDetails$PolicyLanguage` documents `STANDARD` as "To create a custom
/// policy".
pub const STANDARD_POLICY_LANGUAGE: &str = "STANDARD";

/// The policy type of an event-based policy: `PolicyDetails$PolicyType`
/// documents `EVENT_BASED_POLICY` as the type that "performs specific actions
/// when a defined event occurs". A policy of this type has no schedules and no
/// resource locations, so none of the custom snapshot/AMI defaults below belong
/// to it.
pub const EVENT_BASED_POLICY_TYPE: &str = "EVENT_BASED_POLICY";

/// Where a custom snapshot or AMI policy's targets live when the request states
/// no `PolicyDetails$ResourceLocations`, and where a schedule creates its
/// snapshots when it states no `CreateRule$Location`: both members name the
/// in-Region location `CLOUD` ("If the source resources are located in a Region,
/// specify `CLOUD`", and `CreateRule$Location` documents "Default: `CLOUD`").
pub const CLOUD_LOCATION: &str = "CLOUD";

/// Whether a schedule copies the source resource's tags when the request states
/// no `Schedule$CopyTags`: the member targets the model's non-nullable `CopyTags`
/// boolean (as against the `CopyTagsNullable` the optional copy-tags members
/// target), and `CopyTags` is documented "If you do not specify a value, the
/// default is false."
pub const DEFAULT_SCHEDULE_COPY_TAGS: bool = false;

/// Whether a cross-Region snapshot or AMI copy carries the source's user-defined
/// tags when the copy rule states no `CrossRegionCopyRule$CopyTags`.
///
/// The member has no documented default, but a read of a policy whose copy rule
/// stated none reports the flag ON: a cross-Region copy carries the source's tags
/// unless the rule turns that off. Like the members above, it is materialized at
/// create, so a rule that states `false` keeps `false`.
pub const DEFAULT_COPY_TAGS_ON_COPY: bool = true;

/// The value a schedule rule's interval carries when the request states none.
///
/// `CreateRule$Interval` and `RetainRule$Interval` come back as `0` on a read of
/// a policy whose schedule stated neither — a cron-driven creation rule and a
/// count-based retention rule respectively — so the service materializes the
/// integer zero for them rather than leaving them out. Only the two interval
/// members are stamped: nothing is invented for a member no recorded read shows
/// the service filling in.
pub const UNSET_RULE_INTERVAL: i32 = 0;

/// The creation frequency a default policy runs at when the request names none:
/// `CreateInterval` documents "If you do not specify a value, the default is 1."
pub const DEFAULT_CREATE_INTERVAL: i32 = 1;

/// How long a default policy retains snapshots or AMIs when the request names
/// no value: `RetainInterval` documents "If you do not specify a value, the
/// default is 7."
pub const DEFAULT_RETAIN_INTERVAL: i32 = 7;

/// Whether a default policy copies tags from the source resource when the
/// request names no value: `CopyTags` documents "If you do not specify a value,
/// the default is false."
pub const DEFAULT_COPY_TAGS: bool = false;

/// Whether a default policy extends deletion when the request names no value:
/// `ExtendDeletion` documents "If you do not specify a value, the default is
/// false."
pub const DEFAULT_EXTEND_DELETION: bool = false;

/// The error code Amazon Data Lifecycle Manager answers when the caller may not
/// pass the `ExecutionRoleArn` it named.
///
/// It is not one of the errors the model lists for the operations that take an
/// `ExecutionRoleArn` (those are `InternalServerException`,
/// `InvalidRequestException`, `LimitExceededException` and, for the update,
/// `ResourceNotFoundException`): the `iam:PassRole` authorization runs in the
/// front door, ahead of the operation, so its denial is the generic AWS
/// authorization failure rather than a modelled operation error.
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The HTTP status an AWS `AccessDeniedException` carries: the request was
/// authenticated and then refused by authorization.
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// The denial for an `ExecutionRoleArn` that names another account's IAM
/// principal: Amazon Data Lifecycle Manager can only pass a role of the account
/// the request is made in.
pub const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";

/// Why passing `execution_role_arn` is denied, or `None` when the caller may
/// pass it.
///
/// Amazon Data Lifecycle Manager assumes the role the request names, so every
/// operation that takes an `ExecutionRoleArn` first authorizes `iam:PassRole`
/// for it. That authorization is about the role the ARN DENOTES, so it can only
/// be asked of a value that denotes an IAM principal at all: an
/// `ExecutionRoleArn` that is not a well-formed IAM ARN is left to the member's
/// own `@pattern`, which refuses it as a malformed request instead.
///
/// Of the IAM ARNs, two cannot be passed: one in another account (Amazon Data
/// Lifecycle Manager has no cross-account trust), and one that does not denote
/// a role (a user, a group, an instance profile — `iam:PassRole` has no role to
/// grant). The account is checked first, which is the order AWS answers in for
/// an ARN that is both.
pub fn pass_role_denial(execution_role_arn: &str) -> Option<String> {
    let (account, resource) = iam_arn_parts(execution_role_arn)?;
    if account != ::engine::emulator_account_id() {
        return Some(CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string());
    }
    let names_a_role = resource
        .strip_prefix("role/")
        .is_some_and(|name| !name.is_empty());
    if names_a_role {
        return None;
    }
    Some(format!(
        "The role {execution_role_arn} cannot be passed: it does not identify an IAM role."
    ))
}

/// The account id and the resource portion of `arn`, when `arn` is a well-formed
/// IAM ARN — `arn:<partition>:iam::<account>:<resource>`.
///
/// IAM is a global service, so an IAM ARN names an empty region; the partition
/// is the `aws(-[a-z]{1,3}){0,2}` family the `ExecutionRoleArn` `@pattern` also
/// spells, and the account is digits. Anything else is not an ARN this service
/// can resolve a principal from.
fn iam_arn_parts(arn: &str) -> Option<(&str, &str)> {
    // The resource portion may itself contain `:`, so it takes the remainder.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if partition != "aws" && !partition.starts_with("aws-") {
        return None;
    }
    if fields.next()? != "iam" {
        return None;
    }
    if !fields.next()?.is_empty() {
        return None;
    }
    let account = fields.next()?;
    if account.is_empty() || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let resource = fields.next()?;
    Some((account, resource))
}

/// How many policies the store holds before `CreateLifecyclePolicy` refuses with
/// the modelled `LimitExceededException`.
///
/// This is the emulator's OWN bound on the store, not a claim about any
/// account's Amazon Data Lifecycle Manager quota (which is adjustable, and which
/// this emulator does not model). It is set far above any workflow an emulator
/// serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteLifecyclePolicy`.
pub const POLICY_QUOTA: usize = 1_000;

/// The `PolicyDetails` this service STORES for a policy whose configuration was
/// stated as a `PolicyDetails` structure: the submitted structure with every
/// member the service itself materializes stamped onto it.
///
/// Amazon Data Lifecycle Manager fills a custom policy's omitted configuration
/// members in when it accepts the policy, and a read then reports the stored
/// configuration — the submitted members unchanged plus the materialized ones.
/// So the stamping happens here, where a create or an update writes the record,
/// and never in a read projection.
///
/// What is stamped, and why:
///
/// * `PolicyLanguage` — `STANDARD`, which `PolicyDetails$PolicyLanguage`
///   documents as the language of a custom policy.
/// * `ResourceLocations` — `[CLOUD]`, the in-Region location. The member is
///   `@length(min: 1, max: 1)`, so a read of a custom snapshot or AMI policy
///   reports exactly one location.
/// * per schedule — `CopyTags` false, the two rule intervals' zero,
///   `CreateRule$Location` `CLOUD` (its documented default) for a SNAPSHOT policy
///   only, since that member is `[Custom snapshot policies only]`, and a
///   cross-Region copy rule's `CopyTags`. This applies to EVERY element of
///   `Schedules` (`@length(min: 1, max: 4)`) and of `CrossRegionCopyRules`, not
///   just the first.
///
/// What is NOT stamped: a member for which neither the model, the documentation
/// nor a recorded read states a value the service fills in — a count, an
/// interval unit, a nested deprecate/archive/fast-restore rule's members. They
/// keep exactly what the request stated, because inventing one would be
/// fabrication.
pub fn materialized_policy_details(mut details: PolicyDetails) -> PolicyDetails {
    if details.policy_language.as_deref() == Some(SIMPLIFIED_POLICY_LANGUAGE) {
        // A DEFAULT policy's configuration. Its `[Default policies only]`
        // defaults are the documented ones, which
        // [`LifecyclePolicyRecord::effective_policy_details`] reports from the
        // record's own body parameters.
        return details;
    }
    details
        .policy_language
        .get_or_insert_with(|| STANDARD_POLICY_LANGUAGE.to_string());

    // Resource locations and schedules are `[Custom snapshot and AMI policies
    // only]`; an event-based policy configures an `EventSource` and `Actions`
    // instead, so it carries neither.
    let event_based = details.policy_type.as_deref() == Some(EVENT_BASED_POLICY_TYPE)
        || details.event_source.is_some();
    if event_based {
        return details;
    }
    // `CreateRule$Location` is `[Custom snapshot policies only]`, so an AMI
    // policy's creation rule has no location to report. `PolicyDetails$PolicyType`
    // is what states the type, and "the default is `EBS_SNAPSHOT_MANAGEMENT`".
    let creates_snapshots = details.policy_type.as_deref() != Some(IMAGE_POLICY_TYPE);

    details
        .resource_locations
        .get_or_insert_with(|| vec![CLOUD_LOCATION.to_string()]);
    for schedule in details.schedules.iter_mut().flatten() {
        schedule.copy_tags.get_or_insert(DEFAULT_SCHEDULE_COPY_TAGS);
        if let Some(create_rule) = schedule.create_rule.as_mut() {
            if creates_snapshots {
                create_rule
                    .location
                    .get_or_insert_with(|| CLOUD_LOCATION.to_string());
            }
            create_rule.interval.get_or_insert(UNSET_RULE_INTERVAL);
        }
        if let Some(retain_rule) = schedule.retain_rule.as_mut() {
            retain_rule.interval.get_or_insert(UNSET_RULE_INTERVAL);
        }
        for copy_rule in schedule.cross_region_copy_rules.iter_mut().flatten() {
            copy_rule.copy_tags.get_or_insert(DEFAULT_COPY_TAGS_ON_COPY);
        }
    }
    details
}

/// One Amazon Data Lifecycle Manager lifecycle policy, as this emulator persists
/// it. Every member a read operation reports comes from here, never from the
/// request being served.
#[derive(Debug, Clone)]
pub struct LifecyclePolicyRecord {
    /// `^policy-[a-f0-9]+$`, minted by
    /// [`ComAmazonawsDlmDlm20180112State::mint_policy_id`].
    pub policy_id: String,
    /// `arn:aws:dlm:<region>:<account>:policy/<policy id>`, derived at create.
    pub policy_arn: String,
    pub description: String,
    /// One of `GettablePolicyStateValues` (`ENABLED` | `DISABLED` | `ERROR`).
    pub state: String,
    /// "The description of the status", when some failure path has recorded one.
    /// Nothing in this emulator moves a policy into `ERROR`, so no policy
    /// carries one and [`LifecyclePolicyRecord::wire_status_message`] reports the
    /// policy's activation state instead.
    pub status_message: Option<String>,
    pub execution_role_arn: String,
    pub date_created: SystemTime,
    pub date_modified: SystemTime,
    pub policy_details: Option<PolicyDetails>,
    /// True when this is a default policy rather than a custom one.
    pub default_policy: bool,
    /// `DefaultPolicyTypeValues` (`INSTANCE` | `VOLUME`) for a default policy.
    pub default_policy_type: Option<String>,
    pub create_interval: Option<i32>,
    pub retain_interval: Option<i32>,
    pub copy_tags: Option<bool>,
    pub extend_deletion: Option<bool>,
    pub cross_region_copy_targets: Vec<CrossRegionCopyTarget>,
    pub exclusions: Option<Exclusions>,
    pub tags: BTreeMap<String, String>,
}

impl LifecyclePolicyRecord {
    /// The `PolicyDetails` this policy reports: "The configuration of the
    /// lifecycle policy".
    ///
    /// A CUSTOM policy's configuration is exactly the structure the request
    /// carried. A DEFAULT policy's may instead have been stated in the request
    /// body — `CreateLifecyclePolicy` documents the two as alternatives ("either
    /// in the request body, or in the PolicyDetails request structure, but not
    /// both") — so for one of those the stored body parameters are reported
    /// here, each with the default its member documents when the request named
    /// no value.
    ///
    /// A CUSTOM policy therefore reports its stored structure and nothing else,
    /// and that loses nothing: every one of the `[Default policies only]` fields
    /// below is left unset on a custom record, because a create or an update
    /// that states one for a policy that is not a default policy is refused
    /// ([`default_policy_only_parameters_refused`]) instead of being stored
    /// where no read could reach it.
    pub fn effective_policy_details(&self) -> Option<PolicyDetails> {
        if !self.default_policy {
            return self.policy_details.clone();
        }
        let mut details = self.policy_details.clone().unwrap_or_default();
        details.policy_language = Some(SIMPLIFIED_POLICY_LANGUAGE.to_string());
        details.policy_type = Some(self.policy_type());
        details.resource_type = self
            .default_policy_type
            .clone()
            .or(details.resource_type.clone());
        details.create_interval = Some(
            self.create_interval
                .or(details.create_interval)
                .unwrap_or(DEFAULT_CREATE_INTERVAL),
        );
        details.retain_interval = Some(
            self.retain_interval
                .or(details.retain_interval)
                .unwrap_or(DEFAULT_RETAIN_INTERVAL),
        );
        details.copy_tags = Some(
            self.copy_tags
                .or(details.copy_tags)
                .unwrap_or(DEFAULT_COPY_TAGS),
        );
        details.extend_deletion = Some(
            self.extend_deletion
                .or(details.extend_deletion)
                .unwrap_or(DEFAULT_EXTEND_DELETION),
        );
        if !self.cross_region_copy_targets.is_empty() {
            details.cross_region_copy_targets = Some(self.cross_region_copy_targets.clone());
        }
        details.exclusions = self.exclusions.clone().or(details.exclusions.clone());
        Some(details)
    }

    /// This policy's `PolicyTypeValues`.
    ///
    /// `PolicyDetails$PolicyType` is what states it; absent that, a policy whose
    /// target resource type is `INSTANCE` manages EBS-backed AMIs
    /// ([`IMAGE_POLICY_TYPE`]) and anything else takes the documented default
    /// ([`DEFAULT_POLICY_TYPE`]).
    pub fn policy_type(&self) -> String {
        if let Some(declared) = self
            .policy_details
            .as_ref()
            .and_then(|details| details.policy_type.clone())
        {
            return declared;
        }
        match self.target_resource_type().as_deref() {
            Some("INSTANCE") => IMAGE_POLICY_TYPE.to_string(),
            _ => DEFAULT_POLICY_TYPE.to_string(),
        }
    }

    /// The single resource type this policy targets, however it was stated: a
    /// default policy names it in `DefaultPolicy` (or `PolicyDetails$ResourceType`),
    /// a custom snapshot policy in `PolicyDetails$ResourceTypes`.
    fn target_resource_type(&self) -> Option<String> {
        if let Some(default_type) = self.default_policy_type.clone() {
            return Some(default_type);
        }
        let details = self.policy_details.as_ref()?;
        if let Some(resource_type) = details.resource_type.clone() {
            return Some(resource_type);
        }
        details
            .resource_types
            .as_ref()
            .and_then(|types| types.first().cloned())
    }

    /// Every resource type this policy targets, for the `resourceTypes` filter.
    pub fn target_resource_types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        if let Some(default_type) = self.default_policy_type.as_ref() {
            types.push(default_type.clone());
        }
        if let Some(details) = self.policy_details.as_ref() {
            if let Some(resource_type) = details.resource_type.as_ref() {
                types.push(resource_type.clone());
            }
            if let Some(resource_types) = details.resource_types.as_ref() {
                types.extend(resource_types.iter().cloned());
            }
        }
        types
    }

    /// This policy's target tags as the `key=value` strings the `targetTags`
    /// filter is expressed in ("Tags are strings in the format key=value").
    pub fn target_tag_filters(&self) -> Vec<String> {
        self.policy_details
            .as_ref()
            .and_then(|details| details.target_tags.as_ref())
            .map(|tags| tags.iter().map(tag_filter).collect())
            .unwrap_or_default()
    }

    /// The tags this policy's schedules add to the objects they create, as
    /// `key=value` strings, for the `tagsToAdd` filter.
    pub fn tags_to_add_filters(&self) -> Vec<String> {
        let Some(schedules) = self
            .policy_details
            .as_ref()
            .and_then(|details| details.schedules.as_ref())
        else {
            return Vec::new();
        };
        schedules
            .iter()
            .filter_map(|schedule| schedule.tags_to_add.as_ref())
            .flat_map(|tags| tags.iter().map(tag_filter))
            .collect()
    }

    /// The policy's tags as the wire map.
    ///
    /// A policy with no tags reports the EMPTY map, not an absent member: the
    /// read path reports the tag set a policy holds, and a client must be able to
    /// tell "no tags" from "not reported" without a second call. `TagMap`'s
    /// `@length(min: 1, max: 200)` bounds what a request may SEND — the
    /// validating walk runs on operation inputs only — and says nothing about
    /// what a read reports.
    pub fn wire_tags(&self) -> Option<HashMap<String, String>> {
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    /// "The description of the status" this policy reports.
    ///
    /// It describes the policy's CURRENT status, so a policy that carries no
    /// failure description reports its activation state — the same
    /// `GettablePolicyStateValues` member `State` reports. Nothing in this
    /// emulator runs a policy, so [`LifecyclePolicyRecord::status_message`] is
    /// set only if some future failure path records one, and it wins when it is.
    pub fn wire_status_message(&self) -> String {
        self.status_message
            .clone()
            .unwrap_or_else(|| self.state.clone())
    }

    /// The `LifecyclePolicy` projection `GetLifecyclePolicy` returns.
    pub fn to_policy(&self) -> LifecyclePolicy {
        LifecyclePolicy {
            policy_id: Some(self.policy_id.clone()),
            description: Some(self.description.clone()),
            state: Some(self.state.clone()),
            status_message: Some(self.wire_status_message()),
            execution_role_arn: Some(self.execution_role_arn.clone()),
            date_created: Some(::aws_smithy_types::DateTime::from(self.date_created)),
            date_modified: Some(::aws_smithy_types::DateTime::from(self.date_modified)),
            policy_details: self.effective_policy_details(),
            tags: self.wire_tags(),
            policy_arn: Some(self.policy_arn.clone()),
            default_policy: Some(self.default_policy),
        }
    }

    /// The `LifecyclePolicySummary` projection `GetLifecyclePolicies` returns.
    ///
    /// `DefaultPolicy` is modelled `boolean` on this shape too (the prose
    /// describing it as a type is stale), so it carries the same flag the full
    /// shape does.
    pub fn to_summary(&self) -> LifecyclePolicySummary {
        LifecyclePolicySummary {
            policy_id: Some(self.policy_id.clone()),
            description: Some(self.description.clone()),
            state: Some(self.state.clone()),
            tags: self.wire_tags(),
            policy_type: Some(self.policy_type()),
            default_policy: Some(self.default_policy),
        }
    }
}

/// A `Tag` as the `key=value` string Amazon Data Lifecycle Manager's tag filters
/// are expressed in.
fn tag_filter(tag: &Tag) -> String {
    format!("{}={}", tag.key, tag.value)
}

/// Per-service state for com-amazonaws-dlm-dlm-20180112. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDlmDlm20180112State {
    /// Every lifecycle policy, keyed by its id — the single source of truth. A
    /// `BTreeMap` gives `GetLifecyclePolicies` a stable total order, and ids sort
    /// in the order they were minted.
    pub policies: BTreeMap<String, LifecyclePolicyRecord>,
    /// The counter policy ids are minted from.
    pub next_policy_suffix: u64,
}

impl ComAmazonawsDlmDlm20180112State {
    /// Mint an unused policy id matching `PolicyId`'s `^policy-[a-f0-9]+$`.
    ///
    /// Sixteen lower-case hex digits keep every id the same width, so the
    /// `BTreeMap`'s key order is the order the ids were minted in; the loop skips
    /// an id already taken, which is what makes the wrap safe.
    pub fn mint_policy_id(&mut self) -> String {
        loop {
            self.next_policy_suffix = self.next_policy_suffix.wrapping_add(1);
            let id = format!("policy-{:016x}", self.next_policy_suffix);
            if !self.policies.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the policy whose stored `PolicyArn` is `arn`.
    ///
    /// The tag operations address a policy by ARN; no index is kept for it,
    /// because the ARN is derived from the id and the store is small.
    pub fn policy_id_for_arn(&self, arn: &str) -> Option<String> {
        self.policies
            .values()
            .find(|policy| policy.policy_arn == arn)
            .map(|policy| policy.policy_id.clone())
    }
}

/// The region this request was signed for — what a policy ARN names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the policy `policy_id` names, in `region`.
pub fn policy_arn(policy_id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("policy/{policy_id}"))
}

/// The `ResourceNotFoundException` payload for policy ids that name no policy.
///
/// Every member the shape declares is reported: the `Code` for the exception
/// class, the `ResourceType` token of the resource that was not found, the ids
/// that named none, and a message naming the resource asked for.
pub fn policies_not_found(ids: &[String]) -> ResourceNotFoundException {
    let message = match ids {
        [one] => format!("The resource {{{one}}} was not found."),
        many => format!("The resources {{{}}} were not found.", many.join(", ")),
    };
    ResourceNotFoundException {
        message: Some(message),
        code: Some(RESOURCE_NOT_FOUND_CODE.to_string()),
        resource_type: Some(POLICY_RESOURCE_TYPE.to_string()),
        resource_ids: Some(ids.to_vec()),
    }
}

/// The `ResourceNotFoundException` payload for an ARN that names no policy.
pub fn policy_not_found_by_arn(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("The resource {{{arn}}} was not found.")),
        code: Some(RESOURCE_NOT_FOUND_CODE.to_string()),
        resource_type: Some(POLICY_RESOURCE_TYPE.to_string()),
        // `ResourceIds` is "a list of resource IDs that were not found", and the
        // request named an ARN rather than an id that could be echoed here.
        resource_ids: None,
    }
}

/// The `InvalidRequestException` payload for a request this service refuses as
/// invalid, carrying the `Code` of the exception class and `message`.
///
/// `RequiredParameters` ("the request omitted one or more required parameters")
/// and `MutuallyExclusiveParameters` ("the request included parameters that
/// cannot be provided together") are left unset: each names a class of refusal,
/// so only the refusal of that class fills it — see
/// [`parameters_cannot_be_combined`] and [`required_parameters_omitted`].
pub fn invalid_request(message: String) -> InvalidRequestException {
    InvalidRequestException {
        message: Some(message),
        code: Some(INVALID_PARAMETER_CODE.to_string()),
        required_parameters: None,
        mutually_exclusive_parameters: None,
    }
}

/// The `InvalidRequestException` for a request that stated `parameters` which
/// cannot be provided together.
pub fn parameters_cannot_be_combined(
    message: String,
    parameters: Vec<String>,
) -> InvalidRequestException {
    InvalidRequestException {
        mutually_exclusive_parameters: Some(parameters),
        ..invalid_request(message)
    }
}

/// The `InvalidRequestException` for a request that omitted `parameters` it is
/// required to state.
pub fn required_parameters_omitted(
    message: String,
    parameters: Vec<String>,
) -> InvalidRequestException {
    InvalidRequestException {
        required_parameters: Some(parameters),
        ..invalid_request(message)
    }
}

/// The prefix [`DefaultPolicyOnlyParameters::stated_names`] qualifies a member
/// stated inside the `PolicyDetails` configuration structure with, so a refusal
/// tells a caller WHERE it stated the parameter.
pub const POLICY_DETAILS_PREFIX: &str = "PolicyDetails.";

/// Which `[Default policies only]` configuration members a request states,
/// wherever it stated them.
///
/// `CreateLifecyclePolicyRequest`, `UpdateLifecyclePolicyRequest` and
/// `PolicyDetails` all declare the same configuration members, and the model
/// documents every one of them `[Default policies only]`. `ResourceType` is
/// declared by `PolicyDetails` alone — the request body states the same thing as
/// `CreateLifecyclePolicyRequest$DefaultPolicy`, which is the member that MAKES
/// a request a default-policy request — so a request-body set leaves it unset.
///
/// This is the ONE list of their wire names, so the refusal of a default-only
/// parameter and the "either in the request body, or in the PolicyDetails
/// request structure, but not both" rule name the same members.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPolicyOnlyParameters {
    pub resource_type: bool,
    pub create_interval: bool,
    pub retain_interval: bool,
    pub copy_tags: bool,
    pub cross_region_copy_targets: bool,
    pub extend_deletion: bool,
    pub exclusions: bool,
}

impl DefaultPolicyOnlyParameters {
    /// Which of them `details` states.
    pub fn in_policy_details(details: Option<&PolicyDetails>) -> Self {
        let Some(details) = details else {
            return Self::default();
        };
        Self {
            resource_type: details.resource_type.is_some(),
            create_interval: details.create_interval.is_some(),
            retain_interval: details.retain_interval.is_some(),
            copy_tags: details.copy_tags.is_some(),
            cross_region_copy_targets: details.cross_region_copy_targets.is_some(),
            extend_deletion: details.extend_deletion.is_some(),
            exclusions: details.exclusions.is_some(),
        }
    }

    /// The wire names of the members stated, in the order the model declares
    /// them, each qualified by `prefix` — [`POLICY_DETAILS_PREFIX`] for a member
    /// stated inside the configuration structure, `""` for a request-body one.
    pub fn stated_names(&self, prefix: &str) -> Vec<String> {
        [
            ("ResourceType", self.resource_type),
            ("CreateInterval", self.create_interval),
            ("RetainInterval", self.retain_interval),
            ("CopyTags", self.copy_tags),
            ("CrossRegionCopyTargets", self.cross_region_copy_targets),
            ("ExtendDeletion", self.extend_deletion),
            ("Exclusions", self.exclusions),
        ]
        .into_iter()
        .filter(|(_, stated)| *stated)
        .map(|(name, _)| format!("{prefix}{name}"))
        .collect()
    }
}

/// The `InvalidRequestException` for `[Default policies only]` parameters stated
/// by a request that is not creating or updating a DEFAULT policy.
///
/// Every member [`DefaultPolicyOnlyParameters`] lists is documented `[Default
/// policies only]`, so a custom policy has nowhere to carry one and no read of
/// one can report it: the request states a configuration the service cannot
/// apply, which is exactly the "invalid parameters" the operation models
/// `InvalidRequestException` for. The message is built from the request — it
/// names the parameters refused and the constraint they fall outside.
pub fn default_policy_only_parameters_refused(parameters: &[String]) -> InvalidRequestException {
    invalid_request(format!(
        "The {} {} {} supported for default policies only.",
        if parameters.len() == 1 {
            "parameter"
        } else {
            "parameters"
        },
        parameters.join(", "),
        if parameters.len() == 1 { "is" } else { "are" },
    ))
}

/// The first tag key in `keys` that begins with the reserved [`aws:`
/// prefix](RESERVED_TAG_KEY_PREFIX), or `None` when none does.
///
/// The prefix is reserved case-insensitively, so `AWS:`, `Aws:` and `aws:` are
/// one and the same reservation.
///
/// A tag key is arbitrary caller-supplied text, so the prefix is compared on
/// BYTES: a `&str` range index would panic on a key whose fourth byte falls
/// inside a multi-byte character (`aws\u{e9}`), and a request must never be able
/// to panic the handler. The reservation itself is ASCII, so a byte comparison
/// decides it exactly.
pub fn reserved_tag_key<'keys, I>(keys: I) -> Option<&'keys str>
where
    I: IntoIterator<Item = &'keys str>,
{
    keys.into_iter().find(|key| {
        key.as_bytes()
            .get(..RESERVED_TAG_KEY_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
    })
}

/// The `InvalidRequestException` for a tag key a caller may not set.
pub fn reserved_tag_key_refused() -> InvalidRequestException {
    invalid_request(RESERVED_TAG_KEY_MESSAGE.to_string())
}

/// How the first schedule in `details` whose `CreateRule` states when to create
/// BOTH ways is named, or `None` when no schedule does.
///
/// `CreateRule` documents "You must specify either CronExpression, or Interval,
/// IntervalUnit, and Times", so the cron form and the interval form are
/// alternatives: a rule carrying a `CronExpression` beside any interval member
/// states its schedule twice, and the two cannot be reconciled into one.
///
/// `Schedule$Name` is optional, so a schedule that carries none is named by its
/// position in `Schedules` instead — what matters is that the caller can tell
/// WHICH of its schedules to correct.
pub fn schedule_with_conflicting_create_rule(details: Option<&PolicyDetails>) -> Option<String> {
    details?
        .schedules
        .as_ref()?
        .iter()
        .enumerate()
        .find(|(_, schedule)| {
            schedule
                .create_rule
                .as_ref()
                .is_some_and(create_rule_states_both_forms)
        })
        .map(|(position, schedule)| match schedule.name.as_deref() {
            Some(name) if !name.is_empty() => name.to_string(),
            // 1-based, as a validation path numbers a list element.
            _ => format!("Schedules[{}]", position + 1),
        })
}

/// True when `rule` states its schedule as a cron expression AND as an interval.
///
/// `Interval`, `IntervalUnit` and `Times` are the three members of the interval
/// form, so ANY of them beside a `CronExpression` is the conflict — a request
/// does not have to carry all three to have stated the schedule twice.
fn create_rule_states_both_forms(rule: &CreateRule) -> bool {
    let interval_based = rule.interval.is_some()
        || rule.interval_unit.is_some()
        || rule.times.as_ref().is_some_and(|times| !times.is_empty());
    rule.cron_expression.is_some() && interval_based
}

/// The `InvalidRequestException` for a schedule whose `CreateRule` states when
/// to create both ways. The message names the schedule that carries both, which
/// is what tells a caller which of its schedules to correct.
pub fn conflicting_create_rule(schedule: &str) -> InvalidRequestException {
    invalid_request(format!(
        "Interval based CreateRule and Cron based CreateRule cannot both be present in \
         schedule {{{schedule}}}."
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDlmDlm20180112State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDlmDlm20180112State>)
}
