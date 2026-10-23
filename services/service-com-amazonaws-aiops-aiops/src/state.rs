// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-aiops-aiops service.
//!
//! The service's primary resource is the **investigation group**. Every group is
//! stored exactly once, keyed by its 16-character id — the last segment of its ARN
//! — so each operation reads and writes the one record.
//!
//! A group is addressed by its ARN alone. The modelled `InvestigationGroupIdentifier`
//! pattern also admits a bare label, but the service resolves no group from one:
//! every `{identifier}` that is not this service's investigation-group ARN is a
//! modelled 404 ([`ComAmazonawsAiopsAiopsState::resolve_group_id`]).
//!
//! A group belongs to the Region its ARN names, and an account holds at most
//! [`MAX_INVESTIGATION_GROUPS_PER_REGION`] of them there — so the store is keyed by
//! id and read per region through
//! [`ComAmazonawsAiopsAiopsState::groups_in_region`].

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    CrossAccountConfiguration, EncryptionConfiguration, ResourceNotFoundException,
    ServiceQuotaExceededException,
};

/// Characters an investigation-group id is built from.
///
/// Taken from the modelled `InvestigationGroupArn` pattern, whose last segment is
/// `[A-Za-z0-9]{16}`:
/// `smithy_query(query="shape",shape="InvestigationGroupArn")`.
const GROUP_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Length of the id segment of an investigation-group ARN, from the same pattern.
pub const GROUP_ID_LEN: usize = 16;

/// The resource portion an investigation-group ARN carries before its id, from the
/// same modelled pattern.
pub const GROUP_ARN_RESOURCE_PREFIX: &str = "investigation-group/";

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name `aiops`).
pub const ARN_SERVICE: &str = "aiops";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Retention applied when `CreateInvestigationGroup` omits `retentionInDays`.
///
/// From that member's own documentation: "If you omit this parameter, the default
/// of 90 days is used."
pub const DEFAULT_RETENTION_IN_DAYS: i64 = 90;

/// Applied when `CreateInvestigationGroup` omits `isCloudTrailEventHistoryEnabled`.
///
/// From that member's own documentation: "The default is true."
pub const DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED: bool = true;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteInvestigationGroup`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. A cap is this service's own published
// quota where it publishes one; where it publishes none, the constant says so and
// the value is this emulator's resource guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_INVESTIGATION_GROUPS groups, each
// carrying at most MAX_TAGS tags, MAX_COLLECTION_ENTRIES tag key boundaries and
// MAX_COLLECTION_ENTRIES notification channels. Every remaining caller-sized member
// is already bounded by the model, so the centralized walk refuses it before a
// handler runs: `crossAccountConfigurations` (0..25), each channel's
// `ChatConfigurationArns` list (1..5) and its SNS topic key (20..2048), each tag key
// (1..128) and value (1..256), each `tagKeyBoundaries` entry (1..128), `roleArn`
// (20..2048), `name` (1..512) and the policy document (1..32768).
// ---------------------------------------------------------------------------

/// The most investigation groups one account may hold in one Region.
///
/// An AWS quota, from `CreateInvestigationGroup`'s own documentation: "Currently, you
/// can have one investigation group in each Region in your account." A create in a
/// Region that already holds this many is refused with the modelled
/// `ServiceQuotaExceededException` ([`region_quota_exceeded`]).
pub const MAX_INVESTIGATION_GROUPS_PER_REGION: usize = 1;

/// The most investigation groups this emulator will hold ACROSS every Region.
///
/// Emulator resource guard, NOT an AWS quota: the AWS quota is the per-region
/// [`MAX_INVESTIGATION_GROUPS_PER_REGION`] above, but the region a request is signed
/// for is caller-supplied text, so an unbounded number of regions could each hold
/// their one group. This cap is what keeps the store finite in that direction; a
/// create that would exceed it is refused with the modelled
/// `ServiceQuotaExceededException` too ([`store_capacity_exceeded`]).
pub const MAX_INVESTIGATION_GROUPS: usize = 500;

/// The most tags one group may carry.
///
/// From the `tags` member's own documentation: "You can associate as many as 50 tags
/// with an investigation group."
pub const MAX_TAGS: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will
/// store for a caller-sized list or map whose size the model publishes no length
/// for — `tagKeyBoundaries` and `chatbotNotificationChannel`.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// One stored investigation group: every member `GetInvestigationGroup` reports,
/// plus the resource policy `Put/GetInvestigationGroupPolicy` attach to it and the
/// tags the tagging operations will key off its ARN.
#[derive(Debug, Clone, Default)]
pub struct InvestigationGroup {
    /// The 16-character id: the ARN's last segment and this record's store key.
    pub id: String,
    pub arn: String,
    /// The Region this group belongs to — the region field of its own ARN, which is
    /// also what the per-region quota and `ListInvestigationGroups` are scoped by.
    pub region: String,
    pub name: String,
    pub role_arn: String,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub retention_in_days: Option<i64>,
    pub tag_key_boundaries: Vec<String>,
    pub chatbot_notification_channel: BTreeMap<String, Vec<String>>,
    pub is_cloud_trail_event_history_enabled: Option<bool>,
    pub cross_account_configurations: Vec<CrossAccountConfiguration>,
    /// The IAM resource policy document, present only once a Put stored one.
    pub policy: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub created_by: String,
    pub created_at: i64,
    pub last_modified_by: String,
    pub last_modified_at: i64,
}

impl InvestigationGroup {
    /// `chatbotNotificationChannel` as the wire shape wants it: omitted when the
    /// group carries no channel at all.
    pub fn chatbot_notification_channel_output(&self) -> Option<HashMap<String, Vec<String>>> {
        (!self.chatbot_notification_channel.is_empty()).then(|| {
            self.chatbot_notification_channel
                .iter()
                .map(|(topic, configs)| (topic.clone(), configs.clone()))
                .collect()
        })
    }

    /// `tagKeyBoundaries`, omitted when none were ever set.
    pub fn tag_key_boundaries_output(&self) -> Option<Vec<String>> {
        (!self.tag_key_boundaries.is_empty()).then(|| self.tag_key_boundaries.clone())
    }

    /// `crossAccountConfigurations`, omitted when none were ever set.
    pub fn cross_account_configurations_output(&self) -> Option<Vec<CrossAccountConfiguration>> {
        (!self.cross_account_configurations.is_empty())
            .then(|| self.cross_account_configurations.clone())
    }

    /// Record that `principal` just changed this group.
    ///
    /// A successful mutation ALWAYS advances `lastModifiedAt`, which is what lets a
    /// caller notice a configuration change and order two writers by it. The stamp
    /// is whole seconds (the unit the modelled long carries), so a second write
    /// inside the same second would otherwise report the instant the first one did:
    /// `at` is therefore taken as a floor and the stamp still moves by its smallest
    /// representable step when the clock has not.
    pub fn touch(&mut self, principal: &str, at: i64) {
        self.last_modified_by = principal.to_string();
        self.last_modified_at = at.max(self.last_modified_at.saturating_add(1));
    }
}

/// Per-service state for com-amazonaws-aiops-aiops. A handler reaches it through
/// [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAiopsAiopsState {
    /// Every investigation group, keyed by its 16-character id. A `BTreeMap` so
    /// `ListInvestigationGroups` walks a stable, total order and a page token can
    /// name the next id.
    pub investigation_groups: BTreeMap<String, InvestigationGroup>,
    /// Mixed into each generated id so two creates in the same nanosecond differ.
    id_counter: u64,
}

impl ComAmazonawsAiopsAiopsState {
    /// The id of the group `identifier` names — resolved from its ARN and from
    /// nothing else.
    ///
    /// The modelled `InvestigationGroupIdentifier` pattern admits a bare
    /// `[\-_A-Za-z0-9]{1,512}` label beside the ARN, so a name or a bare id reaches a
    /// handler as a well-formed identifier; the service still resolves no group from
    /// one, and every operation reports the group as absent instead. A name is
    /// therefore not an address: only the ARN the create handed back is.
    ///
    /// An ARN must match the stored ARN in EVERY field, so an ARN naming another
    /// partition, region or account resolves to nothing rather than to the group that
    /// happens to share its id segment.
    pub fn resolve_group_id(&self, identifier: &str) -> Option<String> {
        let id = group_id_from_arn(identifier)?;
        self.investigation_groups
            .get(id)
            .filter(|group| group.arn == identifier)
            .map(|group| group.id.clone())
    }

    /// The group `identifier` names, for a reporting operation.
    pub fn group(&self, identifier: &str) -> Option<&InvestigationGroup> {
        let id = self.resolve_group_id(identifier)?;
        self.investigation_groups.get(&id)
    }

    /// The group `identifier` names, for an operation the spec says mutates it.
    ///
    /// `id` is part of this record's identity — the store is keyed on it — so it may
    /// not be reassigned through this handle. No operation needs to; the model binds
    /// no `name` member on `UpdateInvestigationGroup` either, so a group keeps the
    /// name it was created with.
    pub fn group_mut(&mut self, identifier: &str) -> Option<&mut InvestigationGroup> {
        let id = self.resolve_group_id(identifier)?;
        self.investigation_groups.get_mut(&id)
    }

    /// Every group this account holds in `region`, in the store's stable id order.
    ///
    /// A group belongs to exactly one Region — the one its ARN names — so this is
    /// what a regional reader (`ListInvestigationGroups`) reports and what the
    /// per-region quota counts.
    pub fn groups_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a InvestigationGroup> + 'a {
        self.investigation_groups
            .values()
            .filter(move |group| group.region == region)
    }

    /// How many groups this account already holds in `region`, for the quota check.
    pub fn group_count_in_region(&self, region: &str) -> usize {
        self.groups_in_region(region).count()
    }

    /// Store a newly created group.
    pub fn insert_group(&mut self, group: InvestigationGroup) {
        self.investigation_groups.insert(group.id.clone(), group);
    }

    /// Remove the group `identifier` names, with its policy and tags. `None` when
    /// nothing matched.
    pub fn remove_group(&mut self, identifier: &str) -> Option<InvestigationGroup> {
        let id = self.resolve_group_id(identifier)?;
        self.investigation_groups.remove(&id)
    }

    /// A 16-character alphanumeric id no stored group is using.
    pub fn fresh_group_id(&mut self) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let seed = now_nanos() ^ self.id_counter.rotate_left(17);
            let candidate = group_id_from_seed(seed);
            if !self.investigation_groups.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[A-Za-z0-9]{16}`, and there are always
        // more of them than there are stored groups.
        (0u64..=self.investigation_groups.len() as u64)
            .map(|n| format!("{n:016}"))
            .find(|id| !self.investigation_groups.contains_key(id))
            .unwrap_or_else(|| format!("{:016}", self.investigation_groups.len()))
    }
}

// ---------------------------------------------------------------------------
// How this service words a refusal
//
// Two wordings, each single-homed here so every operation states the same failure
// the same way: the `ValidationException` message a bad input is refused with
// ([`validation_exception_message`], [`validation_message`]) and the
// `ResourceNotFoundException` message an absent resource is reported with
// ([`investigation_group_not_found`], [`investigation_group_policy_not_found`]).
//
// Neither carries anything the request sent. A `ValidationException` names the
// member and the requirement it violated and withholds the value that violated it;
// a `ResourceNotFoundException` names the RESOURCE TYPE that is absent and withholds
// the identifier that named it. Recorded 4xx responses from this service are built
// that way across every operation, and both messages are the whole machine-readable
// detail a client gets, so a caller's own bytes must not be copied into them.
//
// The TAGGING operations are served by the tagging frontend and have their own two
// wordings, single-homed below for the same reason: [`tag_capacity_rejection`] (400
// `BadRequestException`) and [`TAGGING_RESOURCE_NOT_FOUND`] (404
// `NotFoundException`). Neither carries anything the request sent either.
// ---------------------------------------------------------------------------

/// The modelled name of this service's primary resource
/// (`com.amazonaws.aiops#InvestigationGroup`, the resource every `{identifier}`
/// member carries a `@references` entry for), which is how a not-found answer names
/// it.
pub const INVESTIGATION_GROUP_RESOURCE: &str = "InvestigationGroup";

/// The modelled name of the resource policy attached to a group
/// (`com.amazonaws.aiops#InvestigationGroupPolicy`, the second `@references` entry on
/// the policy operations' inputs).
pub const INVESTIGATION_GROUP_POLICY_RESOURCE: &str = "InvestigationGroupPolicy";

/// The `ResourceNotFoundException` for an absent `resource`.
///
/// The message states the resource TYPE that does not exist and nothing else: the
/// recorded 404 for a group that was never created reads
/// `InvestigationGroup does not exist.` in every operation and every fixture, with
/// nothing drawn from the request in it. So the message varies only with WHICH
/// modelled resource is missing, never with the identifier that named it.
fn resource_not_found(resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{resource} does not exist.")),
    }
}

/// The 404 every operation answers when the investigation group `{identifier}` names
/// does not exist — including the policy operations, where the GROUP is what is
/// missing and so is what gets reported.
pub fn investigation_group_not_found() -> ResourceNotFoundException {
    resource_not_found(INVESTIGATION_GROUP_RESOURCE)
}

/// The 404 `GetInvestigationGroupPolicy` answers when the group EXISTS and carries no
/// resource policy: a different condition from an absent group, and so a different
/// resource named, which is what lets a client tell the two apart.
pub fn investigation_group_policy_not_found() -> ResourceNotFoundException {
    resource_not_found(INVESTIGATION_GROUP_POLICY_RESOURCE)
}

/// The text separating a clause's subject from the requirement it states.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// The prefix the requirement half carries when the constraint violated is a
/// `@pattern`; the expression follows it.
const PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: ";

/// The `@pattern` expressions this service APPLIES, as
/// `(modelled expression, applied expression)`, for the members whose live constraint
/// differs from the one the model publishes.
///
/// `name` is the one such member: recorded 400 `ValidationException` responses quote
/// `[\-_A-Za-z0-9\[\]\(\)\{\}\.:!?@; ]+`, which is the modelled class
/// (`smithy_query(query="shape",shape="StringWithPatternAndLengthLimits")`:
/// `^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$`) plus `!`, `?`, `@` and `;`, and without the
/// anchors.
///
/// REPORTED, not ENFORCED: the constraint walk at the request boundary applies the
/// member's own modelled trait, read off the operation's generated input schema, which
/// no service crate can widen — so for `name` the expression reported here is the
/// WIDER of the two and the emulator still refuses a name this expression admits and
/// the model's does not. The reported form is what a caller is told the service
/// enforces; do not read it as what was checked.
///
/// The lookup is keyed on the model's EXACT expression, so a model that changes one
/// misses here and is reported through its own expression rather than through a stale
/// override.
const APPLIED_PATTERNS: &[(&str, &str)] = &[(
    r"^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$",
    r"[\-_A-Za-z0-9\[\]\(\)\{\}\.:!?@; ]+",
)];

/// The expression this service reports for the modelled `@pattern` `modelled`: its
/// [`APPLIED_PATTERNS`] entry when it has one, else the modelled expression with its
/// anchors dropped.
pub fn applied_pattern(modelled: &str) -> &str {
    APPLIED_PATTERNS
        .iter()
        .find(|(declared, _)| *declared == modelled)
        .map_or_else(|| unanchored(modelled), |(_, applied)| *applied)
}

/// A `@pattern` expression without the anchors a model spells it with: Smithy's
/// `@pattern` is an unanchored find, so `^`/`$` are how a model demands a whole-string
/// match rather than part of the expression a value has to satisfy.
///
/// A trailing `$` that is ESCAPED is a literal dollar sign inside the expression, not
/// the end anchor, so it stays: the run of backslashes before it decides.
///
/// (Sibling service crates carry their own copies of this helper — `grep -rln 'fn
/// unanchored' services/*/src` — because none is reachable from another;
/// `services/common/src/validation.rs` is the staged home for the one shared copy.)
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) if trimmed.chars().rev().take_while(|c| *c == '\\').count() % 2 == 0 => {
            trimmed
        }
        _ => body,
    }
}

/// The requirement half of a clause: what the value had to satisfy.
///
/// Read off the FRAMEWORK's own rendering of `kind` — with an empty path and no value,
/// so every word of the sentence is the framework's or the model's and none of it is
/// anything a caller sent — and then cut at the clause separator. A constraint class
/// the framework learns to check therefore cannot drift from this wording, and
/// [`ViolationKind`] being `#[non_exhaustive]` costs nothing here.
///
/// A `@pattern` requirement is restated through [`applied_pattern`].
pub fn requirement_text(kind: ViolationKind) -> String {
    let rendered = Violation {
        path: String::new(),
        kind,
        value: None,
    }
    .message();
    let requirement = rendered
        .split_once(CONSTRAINT_SEPARATOR)
        .map_or(rendered.as_str(), |(_, requirement)| requirement);
    match requirement.strip_prefix(PATTERN_REQUIREMENT) {
        Some(pattern) => format!("{PATTERN_REQUIREMENT}{}", applied_pattern(pattern)),
        None => requirement.to_string(),
    }
}

/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated — and NOTHING of the value itself.
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}'{CONSTRAINT_SEPARATOR}{requirement}")
}

/// [`constraint_clause`] for a member that was ABSENT from the request.
///
/// The subject is `Value null`, because there was no value: the request carried
/// nothing at that path, which is a fact about the request's SHAPE rather than a value
/// echoed back from it.
pub fn absent_member_clause(member_path: &str, requirement: &str) -> String {
    format!("Value null at '{member_path}'{CONSTRAINT_SEPARATOR}{requirement}")
}

/// The AWS `ValidationException` envelope over one or more clauses: how many
/// constraints were violated, then the clauses.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [clause] => format!("1 validation error detected: {clause}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The whole `ValidationException` message for ONE member that failed ONE
/// requirement, for a rule a handler enforces itself (one the model does not express,
/// so the boundary walk passed it through). Worded through the same builders as a
/// boundary refusal, so the two cannot drift.
pub fn validation_message(member_path: &str, requirement: &str) -> String {
    validation_envelope(&[constraint_clause(member_path, requirement)])
}

/// The `ValidationException` message for a refusal the constraint walk made at the
/// request boundary: one clause per violation, in the AWS envelope.
///
/// `None` when there is nothing to state, which keeps the framework's own wording.
pub fn validation_exception_message(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(|violation| {
            let requirement = requirement_text(violation.kind.clone());
            match violation.value {
                // No value at that path: the member was absent from the request.
                None => absent_member_clause(&violation.path, &requirement),
                Some(_) => constraint_clause(&violation.path, &requirement),
            }
        })
        .collect();
    Some(validation_envelope(&clauses))
}

/// Why an `encryptionConfiguration` is unacceptable, or `None` when it is fine.
///
/// `EncryptionConfiguration.type` targets the modelled `EncryptionConfigurationType`
/// enum, so only that shape's own value set is accepted. The catalog is the
/// generated `ENCRYPTION_CONFIGURATION_TYPE_VALUES`, which carries exactly the
/// values `smithy_query(query="shape",shape="EncryptionConfigurationType")` reports.
/// Every operation that accepts the structure applies this one rule, each rendering
/// the refusal as its own modelled `ValidationException`.
///
/// The refusal is the service's own `ValidationException` MESSAGE, worded through
/// [`validation_message`]: the member path and the value set it had to satisfy, with
/// the rejected value withheld, exactly as a boundary refusal for the same class of
/// violation reads.
///
/// `type` is NOT `@required` on the structure, so a configuration that omits it is a
/// request the model admits and this rule has nothing to say about — hence the early
/// `?` on the member rather than a refusal.
pub fn encryption_configuration_rejection(
    configuration: Option<&EncryptionConfiguration>,
) -> Option<String> {
    let supplied = configuration?.r#type.as_deref()?;
    if crate::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&supplied) {
        return None;
    }
    let allowed = crate::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    Some(validation_message(
        "encryptionConfiguration.type",
        &requirement_text(ViolationKind::EnumValue(allowed)),
    ))
}

/// Why the caller-sized collections on a create or update request are unacceptable,
/// or `None` when every one of them is within the store's bounds.
///
/// The model publishes no length for the `chatbotNotificationChannel` map or the
/// `tagKeyBoundaries` list, so the centralized walk cannot refuse an oversized one
/// and this is where the bound lives. Every operation that stores any of them applies
/// this one rule, each rendering the refusal as its own modelled `ValidationException`.
/// Pass `None` for a member the operation does not accept. The `tags` map is bounded
/// by the service's published tag maximum instead, which is a different refusal:
/// [`tag_capacity_rejection`].
///
/// The refusal is worded through [`validation_message`] too: the member path and the
/// bound it exceeded, in the same clause a modelled `@length` maximum would be refused
/// with at the boundary — so a client reads one grammar for both, and the entry count
/// the request carried is not echoed back.
pub fn collection_bounds_rejection(
    tag_key_boundaries: Option<&[String]>,
    chatbot_notification_channel: Option<&std::collections::HashMap<String, Vec<String>>>,
) -> Option<String> {
    let too_many = |member: &str, actual: usize, cap: usize| {
        (actual > cap).then(|| {
            validation_message(
                member,
                &requirement_text(ViolationKind::MaxLength(cap as i64)),
            )
        })
    };
    too_many(
        "tagKeyBoundaries",
        tag_key_boundaries.map_or(0, <[String]>::len),
        MAX_COLLECTION_ENTRIES,
    )
    .or_else(|| {
        too_many(
            "chatbotNotificationChannel",
            chatbot_notification_channel.map_or(0, std::collections::HashMap::len),
            MAX_COLLECTION_ENTRIES,
        )
    })
}

/// Why a resource may not end up carrying `resulting_tags` tags, or `None` when that
/// many fit.
///
/// The cap applies to the tag set the resource ENDS UP with, so `TagResource` counts
/// the merge of the request's map with the tags the resource already carries, and a
/// create counts the map it was given. The message states that rule and carries
/// nothing of the request.
///
/// This refusal is NOT the service's own `ValidationException`: a tag write is served
/// by the tagging frontend, whose refusal is HTTP 400 with the error code
/// `BadRequestException` — the code recorded for a 51-tag call on an investigation
/// group, and the one a sibling service's tag map is refused with as well
/// (`services/service-com-amazonaws-batch-awsbatchv20160810/src/service.rs`). The
/// shape is not in this service's model, so it is answered through
/// [`crate::scaffolded::Scaffolded::bad_request`].
pub fn tag_capacity_rejection(resulting_tags: usize) -> Option<String> {
    (resulting_tags > MAX_TAGS)
        .then(|| format!("The resultant tag set must not have more than {MAX_TAGS} user tags."))
}

/// What the TAGGING operations answer when `resourceArn` names no resource.
///
/// The same split as [`tag_capacity_rejection`], one status up: a tagging call is
/// served by the tagging frontend rather than by this service's own resource surface,
/// and the frontend reports an absent resource as HTTP 404 with the error code
/// `NotFoundException` — the code recorded for `ListTagsForResource`, `TagResource`
/// and `UntagResource` against an investigation group that was never created, where
/// the resource operations are recorded answering this service's own modelled
/// `ResourceNotFoundException` ([`investigation_group_not_found`]). That shape is not
/// in this service's model, so the answer goes out through
/// [`crate::scaffolded::Scaffolded::not_found`].
///
/// The frontend addresses every taggable resource type through one `resourceArn`, so
/// its wording names no resource type; like every other 4xx this service words, it
/// carries nothing the request sent. Only the status and the error code are
/// requirements — no rule published by the model or the recording produces this
/// sentence — so this is the frontend's recorded generic text, single-homed here.
pub const TAGGING_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The `EncryptionConfigurationType` value naming the Amazon Web Services owned key,
/// taken from the modelled value set
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`).
pub const AWS_OWNED_KEY_TYPE: &str = "AWS_OWNED_KEY";

/// The encryption configuration a group carries when its create omitted one.
///
/// From `encryptionConfiguration`'s own documentation: "If you omit this parameter,
/// CloudWatch investigations will use an Amazon Web Services key to encrypt the
/// data." That is the [`AWS_OWNED_KEY_TYPE`] member of the modelled value set, and it
/// carries no `kmsKeyId` because no customer managed key is involved — so the
/// default-key state is a stored configuration like any other and every later read
/// reports it, rather than the member going missing.
pub fn default_encryption_configuration() -> EncryptionConfiguration {
    EncryptionConfiguration {
        r#type: Some(AWS_OWNED_KEY_TYPE.to_string()),
        kms_key_id: None,
    }
}

/// Why a `crossAccountConfigurations` list is unacceptable, or `None` when every
/// entry names an account other than this one.
///
/// A cross-account configuration exists to let the investigating account reach
/// ANOTHER account's resources, so a `sourceRoleArn` whose account id is the
/// investigation account's own is refused: it could never do anything. The modelled
/// constraints on the member (`@length 20..2048`, `^arn:.*$`) cannot express that, so
/// the boundary walk admits it and this is where the rule lives — and every operation
/// that stores the list applies it, each rendering the refusal as its own modelled
/// `ValidationException`.
///
/// Worded through [`validation_message`]: the member path and the requirement, with
/// the offending ARN withheld. The account named is the service's own, not anything
/// the request carried.
pub fn cross_account_configurations_rejection(
    configurations: Option<&[CrossAccountConfiguration]>,
) -> Option<String> {
    let own_account = ::engine::EMULATOR_ACCOUNT_ID;
    configurations?
        .iter()
        .filter_map(|configuration| configuration.source_role_arn.as_deref())
        .any(|arn| arn_account_id(arn) == Some(own_account))
        .then(|| {
            validation_message(
                "crossAccountConfigurations.sourceRoleArn",
                &format!(
                    "Member must name an account other than the investigation account {own_account}"
                ),
            )
        })
}

/// Why a resource policy document is unacceptable, or `None` when it is one this
/// service will attach.
///
/// The modelled constraints on the document bound only its length and its character
/// set (`@length 1..32768`, `^[\u0009\u000A\u000D\u0020-\u00FF]+$`), so the boundary
/// walk admits any printable text; the operation's own semantic check is that the
/// document IS a policy document — the `policy` member is documented as "The policy,
/// in JSON format", and a policy document is a JSON object. Parsed with `serde_json`,
/// so the judgement is a real parser's and never a match on the document's text.
///
/// A document that fails this is not stored at all, so a later
/// `GetInvestigationGroupPolicy` cannot hand back something unparseable.
pub fn policy_document_rejection(policy: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(policy).ok();
    parsed.filter(serde_json::Value::is_object).is_none().then(|| {
        validation_message(
            "policy",
            "Member must be a resource policy document in JSON format",
        )
    })
}

/// The account id field of `arn`, or `None` when `arn` is not an ARN carrying one.
///
/// An ARN is `arn:partition:service:region:account:resource`, and the resource
/// portion may itself hold `:`, so only the first five fields are split off.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let (_partition, _service, _region, account) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    (!account.is_empty()).then_some(account)
}

/// The `ServiceQuotaExceededException` every quota refusal of this service is built
/// through: the quota that was exceeded, stated in `message`, and the resource type
/// and service it belongs to.
///
/// `resourceId` and `quotaCode` stay unset: neither is a value this emulator knows,
/// and the resource the refused create would have minted has no id.
fn quota_exceeded(message: String) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(message),
        resource_type: Some(INVESTIGATION_GROUP_RESOURCE.to_string()),
        service_code: Some(ARN_SERVICE.to_string()),
        ..Default::default()
    }
}

/// The 402 a `CreateInvestigationGroup` is refused with when the Region it is signed
/// for already holds this account's one investigation group.
///
/// The quota is published on the operation itself: "Currently, you can have one
/// investigation group in each Region in your account."
pub fn region_quota_exceeded() -> ServiceQuotaExceededException {
    quota_exceeded(format!(
        "An account can have only {MAX_INVESTIGATION_GROUPS_PER_REGION} investigation group \
         in each Region."
    ))
}

/// The 402 a create is refused with when the store has reached its cross-region
/// resource guard, [`MAX_INVESTIGATION_GROUPS`].
pub fn store_capacity_exceeded() -> ServiceQuotaExceededException {
    quota_exceeded(format!(
        "this account already holds the maximum of {MAX_INVESTIGATION_GROUPS} investigation groups"
    ))
}

/// The id segment of an investigation-group ARN, or `None` when `identifier` is not
/// an ARN of this service's investigation-group resource.
pub fn group_id_from_arn(identifier: &str) -> Option<&str> {
    if !identifier.starts_with("arn:") {
        return None;
    }
    let mut fields = identifier.splitn(6, ':');
    let (_arn, _partition, service, _region, _account, resource) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if service != ARN_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(GROUP_ARN_RESOURCE_PREFIX)?;
    (id.len() == GROUP_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
}

/// The ARN of the investigation group `id` in `region`.
pub fn group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{GROUP_ARN_RESOURCE_PREFIX}{id}"))
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only `[a-zA-Z0-9-]*` in that field, so anything else is
/// refused here and the default is used instead: a generated ARN can never carry
/// text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The principal that made this request, for `createdBy`/`lastModifiedBy`.
///
/// Derived from the request's own SigV4 access key id, so the two members report
/// who called rather than a fabricated identity. That id is caller-controlled text,
/// so it is first reduced to the characters an access key id can hold; the rendering
/// then satisfies `IdentifierStringWithPatternAndLengthLimits`
/// (`^[\-_\/A-Za-z0-9:\.]+$`, 1..512) by construction, whatever the header said.
pub fn caller_principal(parts: &::http::request::Parts) -> String {
    let raw = ::engine::extract_akid(&parts.headers);
    let filtered: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// Now, in whole seconds since the Unix epoch — the unit the modelled `createdAt` /
/// `lastModifiedAt` longs carry.
pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `[A-Za-z0-9]{16}` id from `seed`.
fn group_id_from_seed(seed: u64) -> String {
    let mut state = seed;
    (0..GROUP_ID_LEN)
        .map(|_| {
            let draw = splitmix64(&mut state);
            GROUP_ID_ALPHABET[(draw % GROUP_ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAiopsAiopsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAiopsAiopsState>)
}
