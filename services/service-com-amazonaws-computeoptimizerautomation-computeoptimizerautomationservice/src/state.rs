// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice service.
//!
//! This module owns everything the handlers share: the stored form of an
//! automation rule, of an automation event and of a recommended action, the
//! enrollment configuration and the service-wide enrollment pre-check every
//! action goes through, the client-token (idempotency) ledger, and the pure
//! helpers that mint a rule id, build an opaque page token, bound a queried time
//! range and decide whether a rule, an event or a recommended action matches a
//! list filter. Handlers live in `src/ops/`, one file per operation, and build
//! their own output shapes from what they read back out of here.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    AutomationEvent, AutomationEventFilter, AutomationEventStep, AutomationRule, Criteria,
    DoubleCriteriaCondition, EbsVolumeConfiguration, EstimatedMonthlySavings, Filter,
    IntegerCriteriaCondition,
    OptInRequiredException, OrganizationConfiguration, OrganizationScope, PreviewResult,
    RecommendedAction, RecommendedActionFilter, ResourceDetails, ResourceTagsCriteriaCondition,
    Schedule, StringCriteriaCondition, Tag, AUTOMATION_EVENT_FILTER_NAME_VALUES,
    COMPARISON_OPERATOR_VALUES, ENROLLMENT_STATUS_VALUES,
    RECOMMENDED_ACTION_FILTER_NAME_VALUES, RECOMMENDED_ACTION_TYPE_VALUES, RULE_APPLY_ORDER_VALUES,
    RULE_STATUS_VALUES, RULE_TYPE_VALUES, SAVINGS_ESTIMATION_MODE_VALUES,
};

/// The ARN namespace this service stamps on a rule ARN, from the model's
/// `aws.api#service` trait (`arnNamespace: compute-optimizer`). The `RuleArn`
/// pattern (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`)
/// has an EMPTY region segment, so [`rule_arn`] passes an empty region.
const ARN_NAMESPACE: &str = "compute-optimizer";

/// The revision a freshly created rule carries. `ruleRevision` is the service's
/// optimistic-concurrency token and is documented as "incremented each time the
/// rule is updated", so the first stored revision is 1 and the first successful
/// update makes it 2.
pub const INITIAL_RULE_REVISION: i64 = 1;

/// The enrollment status an account that has never called
/// `UpdateEnrollmentConfiguration` reports: it is not enrolled.
pub const DEFAULT_ENROLLMENT_STATUS: &str = "Inactive";

/// The one `EnrollmentStatus` that means the account is OPTED IN to Compute
/// Optimizer Automation.
///
/// `EnrollmentStatus` is `Active | Inactive | Pending | Failed`
/// (`smithy_query(query="shape",shape="EnrollmentStatus")`) and the `status`
/// member of `UpdateEnrollmentConfigurationRequest` documents what the two
/// settable ones mean: "Active - Enables the Automation feature for your
/// account" and "Inactive - Disables the Automation feature for your account
/// and stops all of your automation rules". `Pending` (an opt-in the service is
/// still processing) and `Failed` (one that did not complete) are the
/// server-only statuses, and neither has the feature enabled either — so
/// `Active` is the only status under which the account may perform this
/// service's actions.
pub const ENROLLED_STATUS: &str = "Active";

/// The `OrganizationRuleMode` an ASSOCIATED member account carries.
///
/// `OrganizationRuleMode` is `AnyAllowed | NoneAllowed`
/// (`smithy_query(query="shape",shape="OrganizationRuleMode")`) and it
/// "Specifies whether the management account can create Automation rules that
/// implement optimization actions for this account"
/// (`smithy_query(query="doc",shape="AccountInfo",member="organizationRuleMode")`).
/// `AssociateAccounts` documents which of the two an association leaves the
/// account in: "When you associate a member account, its organization rule mode
/// is automatically set to \"Any allowed,\" which permits the management account
/// to create Automation rules that automatically apply actions to that account."
pub const ORGANIZATION_RULE_MODE_ANY_ALLOWED: &str = "AnyAllowed";

/// How many account ids one organization-association request may name.
///
/// DOCUMENTED, not chosen: `accountIds` reads "The IDs of the member accounts to
/// associate. You can specify up to 50 account IDs."
/// (`smithy_query(query="doc",shape="AssociateAccountsRequest",member="accountIds")`).
/// The `AccountIdList` shape carries no `@length`, so the protocol boundary
/// admits a longer list and the bound is enforced by the handler.
pub const MAX_ACCOUNT_IDS_PER_REQUEST: usize = 50;

/// How many accounts the enrollment store holds before an association that would
/// bring in a new one is refused.
///
/// CHOSEN bound, for the same reason [`MAX_RULES_PER_ACCOUNT`] and
/// [`MAX_STORED_EVENTS`] are: the organization's real account quota is in neither
/// the model nor the operation docs, and this store lives in memory while
/// `AssociateAccounts` takes the account ids it stores FROM THE REQUEST (any id
/// matching `AccountId`'s `^[0-9]{12}$`). So the number of accounts the store
/// holds is capped rather than left to grow with every distinct id a caller
/// names: a request that would push it past the cap is refused and stores
/// NOTHING. [`MAX_ACCOUNT_IDS_PER_REQUEST`] does not bound it on its own — it
/// only caps ONE request — and an entry already in the store is re-associated
/// freely, because that adds nothing.
pub const MAX_ENROLLED_ACCOUNTS: usize = 1_000;

/// The `description` a rule created without one carries.
///
/// A rule's `description` is reported for every rule that exists — a create that
/// supplied none leaves the rule described by nothing, which is the empty
/// string, not an absent member. Stored at create time so every read path
/// (`GetAutomationRule`, `ListAutomationRules`, `UpdateAutomationRule`) projects
/// the same value out of the store.
pub const DEFAULT_RULE_DESCRIPTION: &str = "";

/// The `priority` a rule created without one carries.
///
/// `priority` is "a string representation of a decimal number between 0 and 1
/// ... When multiple rules match the same recommended action, Compute Optimizer
/// assigns the action to the rule with the lowest priority value (highest
/// priority)" (`smithy_query(query="doc",shape="AutomationRule",member="priority")`),
/// so every rule has a precedence whether or not the create named one.
///
/// CHOSEN value: neither the model nor the operation docs give the default. It is
/// the TOP of the documented range, which is the LOWEST precedence, so a rule
/// whose create named a priority always wins the tie against one that did not.
pub const DEFAULT_RULE_PRIORITY: &str = "1";

/// The page size a paginated list uses when the caller sends no `maxResults`.
/// The model bounds every `maxResults` member at `range(min: 1, max: 100)`
/// (`ListAutomationRules`, `ListAutomationRulePreview`, …), so the largest page
/// a caller may ask for is also the default.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The number of characters in a minted id. `RuleId`, `EventId` and
/// `RecommendedActionId` all share the pattern `^[0-9A-Za-z]{16}$`, so one
/// length serves all three
/// (`smithy_query(query="shape",shape="RuleId,EventId,RecommendedActionId")`).
const MINTED_ID_LEN: usize = 16;

/// The status an automation event carries while its automation is running, one
/// of the model's `EventStatus` values
/// (`smithy_query(query="shape",shape="EventStatus")`). `StartAutomationEvent`
/// accepted the request, so the event it mints is not yet terminal.
pub const EVENT_STATUS_IN_PROGRESS: &str = "InProgress";

/// The modelled `EventStatus` an automation event that did not complete carries.
pub const EVENT_STATUS_FAILED: &str = "Failed";

/// The `StepStatus` of a step that has not started: it is queued behind the
/// steps ahead of it in its event's sequence
/// (`smithy_query(query="shape",shape="StepStatus")`).
pub const STEP_STATUS_READY: &str = "Ready";

/// The `StepStatus` of the step its event is currently on.
pub const STEP_STATUS_IN_PROGRESS: &str = "InProgress";

/// The `StepStatus` of a step that did not complete.
pub const STEP_STATUS_FAILED: &str = "Failed";

/// The `EventType` (and `RecommendedActionType`) that snapshots an unattached
/// EBS volume and then deletes it.
pub const EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME: &str =
    "SnapshotAndDeleteUnattachedEbsVolume";

/// The `EventType` (and `RecommendedActionType`) that changes an EBS volume's
/// type.
pub const EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE: &str = "UpgradeEbsVolumeType";

/// The `StepType` that takes the snapshot of a volume.
pub const STEP_TYPE_CREATE_EBS_SNAPSHOT: &str = "CreateEbsSnapshot";

/// The `StepType` that deletes a volume.
pub const STEP_TYPE_DELETE_EBS_VOLUME: &str = "DeleteEbsVolume";

/// The `StepType` that changes a volume in place (its type, for an upgrade).
pub const STEP_TYPE_MODIFY_EBS_VOLUME: &str = "ModifyEbsVolume";

/// Why an automation event this emulator started never completes.
///
/// Automating a recommended action means changing the referenced resource (take
/// a snapshot and delete an EBS volume, change a volume's type). This emulator
/// performs no such work, so the event it starts is not reported as `Complete`:
/// it reaches the modelled failure status carrying this reason, and never claims
/// an automation that nothing ran.
pub const EVENT_UNPERFORMED_REASON: &str =
    "The automation was not performed: this emulator does not change the resources a \
     recommended action applies to.";

/// How long after the start call an automation event's terminal status is due.
///
/// CHOSEN span: neither the model nor the operation docs give how long an
/// automation takes. It is non-zero so a caller that reads the event straight
/// after starting it still sees the transient `InProgress` status, and short
/// enough that a test need not wait long for the terminal one.
pub const EVENT_SETTLEMENT_DELAY: Duration = Duration::from_millis(500);

/// How far back an automation event stays reportable. `ListAutomationEvents`
/// documents "You can retrieve events that were created within the past year"
/// (`smithy_query(query="doc",shape="ListAutomationEvents")`), so an event
/// created before that window is outside every query's reach however far back
/// the caller's `startTimeInclusive` points.
pub const EVENT_RETRIEVAL_WINDOW: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// How many automation events the store remembers.
///
/// CHOSEN bound, for the same reason [`MAX_RULES_PER_ACCOUNT`] is one: neither
/// the model nor the operation docs give a stored-event limit, and this store
/// lives in memory, so the oldest event is evicted to make room instead of
/// letting the process grow with every started event. [`EVENT_RETRIEVAL_WINDOW`]
/// does not bound it on its own: it only decides what a query may report.
pub const MAX_STORED_EVENTS: usize = 1_000;

/// How many automation rules one account may hold before `CreateAutomationRule`
/// answers the modelled `ServiceQuotaExceededException` ("The request would
/// exceed service quotas", HTTP 402).
///
/// CHOSEN bound: the service's real per-account rule quota is in neither the
/// model nor the operation docs, and this store lives in memory, so the count is
/// capped rather than left to grow with every request. It is the same order as
/// the largest page the model's `maxResults` allows.
pub const MAX_RULES_PER_ACCOUNT: usize = 100;

/// How many tags one resource of this service may carry.
///
/// DOCUMENTED, not chosen: the `tags` member of a rule create reads "A list of
/// key-value pairs used to categorize and organize the automation rule. Maximum
/// of 200 tags allowed."
/// (`smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="tags")`),
/// and `TagList` — the shape every tag list of this service is, request and
/// response alike — carries `@length(max: 200)`
/// (`smithy_query(query="shape",shape="TagList")`). So the list a resource
/// HOLDS is bounded exactly as the list a request carries is, and an addition
/// that would push it past the bound is refused instead of applied.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// How many client tokens the idempotency ledger remembers. Reached only by
/// distinct caller-supplied tokens; the oldest entry is evicted to make room, so
/// the ledger cannot grow without bound. A replay window that has been evicted
/// simply behaves as a fresh token.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 1024;

/// How long a client token stays replayable. CHOSEN bound: the model gives no
/// lifetime for `ClientToken`, and an entry that no retry will ever ask about
/// must not be kept for the life of the process.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The account id every emulated caller presents. Built through the engine's
/// canonical accessor rather than a literal, so every `accountId` and ARN this
/// service emits carries the one placeholder account.
pub fn caller_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The ARN of the rule identified by `rule_id`, in the form the model's
/// `RuleArn` pattern requires.
pub fn rule_arn(rule_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, "", &format!("automation-rule/{rule_id}"))
}

/// Does `candidate` name an automation rule of this service at all?
///
/// The form is the model's `RuleArn`
/// (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`,
/// from `smithy_query(query="shape",shape="RuleArn")`): the service's own ARN
/// namespace, an EMPTY region segment, a 12-digit account and the
/// `automation-rule/<ruleId>` resource. A string that does not have it names no
/// resource of this service, whether or not a rule with that ARN exists.
///
/// Checked by hand rather than with a regex engine: the pattern is a fixed
/// prefix plus two character classes, and this runs on the dispatch path.
pub fn names_automation_rule(candidate: &str) -> bool {
    let Some(rest) = candidate
        .strip_prefix("arn:aws:")
        .and_then(|rest| rest.strip_prefix(ARN_NAMESPACE))
        .and_then(|rest| rest.strip_prefix("::"))
    else {
        return false;
    };
    let Some((account_id, resource)) = rest.split_once(':') else {
        return false;
    };
    if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(rule_id) = resource.strip_prefix("automation-rule/") else {
        return false;
    };
    !rule_id.is_empty()
        && rule_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Is `key` the form the model's `TagKey` declares?
///
/// `TagKeyList`'s member targets a plain `String`
/// (`smithy_query(query="shape",shape="TagKeyList,TagKey")`), so the boundary
/// validator checks nothing for a key sent to `UntagResource` — but a tag key of
/// this service is a `TagKey`, which carries `@length(min: 1, max: 128)` and
/// `@pattern ^[\w\s\.\-\:\/\=\+\@]+$`, so that is the form checked here. `\w` is
/// `[A-Za-z0-9_]` plus the Unicode word characters `char::is_alphanumeric`
/// covers.
pub fn tag_key_is_valid(key: &str) -> bool {
    let length = key.chars().count();
    if !(1..=128).contains(&length) {
        return false;
    }
    key.chars().all(|c| {
        c.is_alphanumeric()
            || c.is_whitespace()
            || matches!(c, '_' | '.' | '-' | ':' | '/' | '=' | '+' | '@')
    })
}

/// Now, as the wire timestamp type. The ONE clock the handlers read, so a stored
/// timestamp is always the real store time.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
}

/// A stable digest of `value`'s debug rendering, used both as a request
/// fingerprint for the client-token ledger and as the filter fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its
/// members in declaration order, so the same request always digests to the same
/// string and a different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    let digest = Sha256::digest(format!("{value:?}").as_bytes());
    hex::encode(digest)
}

// ---------------------------------------------------------------------------
// Error wording
//
// Every client error this service raises is worded HERE, so the body a caller
// parses is built in one place and two handlers raising the same error cannot
// word it differently. Two shapes of rule live here:
//
// * a FIXED message, which the error carries whatever the request named — it
//   must not echo a resource identifier or any other request value;
// * a message BUILT from the request or from the stored resource, which states
//   the fact the caller needs to recover (the member and the constraint it
//   failed, the current revision, the values that are settable).
// ---------------------------------------------------------------------------

/// The message every `ResourceNotFoundException` of this service carries.
///
/// FIXED and resource-independent: the error reports that the resource the
/// request named is not there, and names neither the ARN nor the id, so a
/// not-found body never echoes a resource identifier back to its caller.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The specified resource could not be found";

/// The message every `OptInRequiredException` of this service carries.
///
/// FIXED and request-independent: the refusal states that the ACCOUNT is not
/// registered, and names neither the operation, the account nor anything else
/// the request carried. The requirement this error has to meet is its modelled
/// code and its HTTP 403; this is the service's own wording for it.
pub const OPT_IN_REQUIRED_MESSAGE: &str = "Aws account is not registered for recommendation.";

/// The message every `IdempotentParameterMismatchException` of this service
/// carries — the service's own wire wording for "this client token was first
/// used with a different request", FIXED and derived from nothing in the
/// request.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "The request tied to this token is different!";

/// The `InvalidParameterValueException` message for a request whose
/// `ruleRevision` is not the rule's current one.
///
/// `ruleRevision` is the rule's optimistic-concurrency token, so the rejection
/// states the rule's CURRENT revision — the one fact a caller that lost the race
/// needs to read-modify-write again — and not the rejected input or the rule's
/// ARN.
pub fn revision_mismatch_message(current_revision: i64) -> String {
    format!("Rule revision does not match the current revision number {current_revision}")
}

/// The `InvalidParameterValueException` message for a tag addition that would
/// leave the resource holding more than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// States the BOUND, which is the one fact a caller past it needs, and echoes
/// neither a key, a value nor the resource.
pub fn tag_limit_exceeded_message() -> String {
    format!("The request would exceed the limit of {MAX_TAGS_PER_RESOURCE} tags per resource")
}

/// The `InvalidParameterValueException` message for an association that would
/// leave the enrollment store holding more than [`MAX_ENROLLED_ACCOUNTS`]
/// accounts.
///
/// States the BOUND, which is the one fact a caller past it needs, and echoes no
/// account id the request carried.
pub fn account_limit_exceeded_message() -> String {
    format!("The request would exceed the limit of {MAX_ENROLLED_ACCOUNTS} accounts")
}

/// The `EnrollmentStatus` values `UpdateEnrollmentConfiguration` accepts from a
/// caller.
///
/// `EnrollmentStatus` is `Active | Inactive | Pending | Failed`
/// (`smithy_query(query="shape",shape="EnrollmentStatus")`) and the request
/// member documents that "The Pending and Failed options cannot be used to
/// update the enrollment status of an account. They are returned in the
/// response", so those two are server-only and the rest are settable.
pub const SETTABLE_ENROLLMENT_STATUSES: &[&str] = &["Active", "Inactive"];

/// The `InvalidParameterValueException` message for an enrollment status a
/// caller may not set: it names the rejected value AND enumerates the settable
/// set, so the caller is told what to send instead.
pub fn invalid_enrollment_status_message(status: &str) -> String {
    format!(
        "Invalid status: {status}. Only {} are allowed.",
        settable_enrollment_statuses_phrase()
    )
}

/// The settable statuses as the rejection lists them: the model's value set,
/// upper-cased, joined so the last one reads "… and X".
fn settable_enrollment_statuses_phrase() -> String {
    let values: Vec<String> = SETTABLE_ENROLLMENT_STATUSES
        .iter()
        .map(|status| status.to_uppercase())
        .collect();
    match values.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} and {last}", leading.join(", ")),
    }
}

/// The marker that separates the member clause of a validation clause from the
/// requirement it failed, in the framework's own wording.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// AWS's standard validation report for the modelled constraints a request
/// failed at the protocol boundary, as `src/service.rs` words a refusal with.
///
/// The report opens with HOW MANY members failed, then states one clause per
/// failing member: the member's dotted path and the requirement it failed. It
/// does NOT echo the rejected value — a request value (a name, or a secret on
/// some other member) must not be replayed into an error body — and reports a
/// `@pattern` as the regular expression itself, without the anchors the model
/// spells it with.
///
/// ONE refusal is worded differently, because the service words it itself rather
/// than leaving it to the generic constraint report: a value outside the value
/// set of an enum PARAMETER TYPE ([`REQUEST_ENUM_TYPES`]) is refused by naming
/// that type, the value as sent and the type's modelled values — see
/// [`enum_parameter_rejection`].
pub fn validation_report(violations: &[Violation]) -> String {
    // A value outside one of this service's enum PARAMETER TYPES is reported by
    // the service's own parameter validation, which names the type and the
    // value it was sent, and answers AHEAD of the generic constraint report —
    // see [`enum_parameter_rejection`].
    if let Some(report) = enum_parameter_rejection(violations) {
        return report;
    }
    let count = violations.len();
    let noun = if count == 1 { "error" } else { "errors" };
    let clauses: Vec<String> = violations.iter().map(validation_clause).collect();
    format!("{count} validation {noun} detected: {}", clauses.join("; "))
}

/// One clause of [`validation_report`]: the member and what it failed, with no
/// value echoed.
fn validation_clause(violation: &Violation) -> String {
    format!(
        "Value at '{}' {CONSTRAINT_MARKER}{}",
        violation.path,
        violation_requirement(violation)
    )
}

/// The validation report for ONE member a HANDLER refused — a rule the model's
/// constraints cannot carry, so the protocol boundary admitted the value and
/// the check lives in the handler.
///
/// Same format as [`validation_report`], because a client counts and attributes
/// a constraint failure the same way wherever the service found it, and the
/// rejected value is echoed in neither.
pub fn constraint_report(member: &str, requirement: &str) -> String {
    format!("1 validation error detected: Value at '{member}' {CONSTRAINT_MARKER}{requirement}")
}

/// The enum parameter TYPES a REQUEST of this service can carry a value for,
/// each under the name the model gives the shape, with the value set that shape
/// declares.
///
/// Every entry is a typed Smithy `enum` SHAPE that some input member targets:
/// `ruleType` (`RuleType`), an element of `recommendedActionTypes`
/// (`RecommendedActionType`), `status` (`RuleStatus` on the rule operations,
/// `EnrollmentStatus` on `UpdateEnrollmentConfiguration`),
/// `organizationConfiguration.ruleApplyOrder` (`RuleApplyOrder`) and a criteria
/// condition's `comparison` (`ComparisonOperator`) — the enum-typed input
/// members of this service
/// (`smithy_query(query="operation",operation="CreateAutomationRule,UpdateAutomationRule,UpdateEnrollmentConfiguration,ListAutomationRulePreview,ListAutomationRulePreviewSummaries")`).
/// The model's remaining enum shapes — `EventStatus`, `EventType`, `StepStatus`,
/// `StepType`, `ResourceType`, `OrganizationRuleMode`, `SavingsEstimationMode` —
/// appear in RESPONSES only, so no request can be refused against one, and
/// naming a type here cannot be ambiguous (`EventType` happens to share
/// `RecommendedActionType`'s value set, and is not a request parameter).
///
/// Each value set is the GENERATED, model-derived constant for that shape, so no
/// value list is spelled here: the set a request is refused against and the set
/// the refusal enumerates move together with the model.
///
/// Membership of this table is also what separates a named parameter type from
/// the model's legacy `@enum`-constrained STRINGS (`AutomationRuleFilterName`,
/// `AutomationEventFilterName`, `RecommendedActionFilterName`,
/// `SummaryDimensionKey`), which name no parameter type of their own and so keep
/// the generic validation report.
const REQUEST_ENUM_TYPES: &[(&str, &[&str])] = &[
    ("ComparisonOperator", COMPARISON_OPERATOR_VALUES),
    ("EnrollmentStatus", ENROLLMENT_STATUS_VALUES),
    ("RecommendedActionType", RECOMMENDED_ACTION_TYPE_VALUES),
    ("RuleApplyOrder", RULE_APPLY_ORDER_VALUES),
    ("RuleStatus", RULE_STATUS_VALUES),
    ("RuleType", RULE_TYPE_VALUES),
];

/// The rejection for the FIRST value these violations carry that fell outside
/// one of this service's enum parameter types, or `None` when none did.
///
/// A request is judged parameter by parameter, so the refusal states the one
/// parameter type it could not accept a value for rather than a count of
/// constraint clauses; a refusal carrying no such parameter is left to the
/// generic report.
fn enum_parameter_rejection(violations: &[Violation]) -> Option<String> {
    violations.iter().find_map(|violation| {
        let ViolationKind::EnumValue(declared) = &violation.kind else {
            return None;
        };
        // An absent member carries no value to name, so it is not this refusal.
        let value = violation.value.as_deref()?;
        let enum_type = request_enum_type(declared)?;
        Some(invalid_enum_value_message(enum_type, value, declared))
    })
}

/// The enum parameter type whose declared value set is `declared`.
///
/// A violation states the value set it refused a value against but not the shape
/// that set came from, so the type is recovered by matching the set against
/// [`REQUEST_ENUM_TYPES`]. Compared as a SET: a violation lists the model's
/// declaration order, a generated value constant is sorted.
fn request_enum_type(declared: &[String]) -> Option<&'static str> {
    REQUEST_ENUM_TYPES
        .iter()
        .find(|(_, values)| same_value_set(values, declared))
        .map(|(enum_type, _)| *enum_type)
}

/// Do these two value sets hold the same values? An enum's values are distinct,
/// so equal lengths plus containment is set equality.
fn same_value_set(declared: &[&str], refused: &[String]) -> bool {
    declared.len() == refused.len()
        && refused
            .iter()
            .all(|value| declared.contains(&value.as_str()))
}

/// The `InvalidParameterValueException` message for a value outside an enum
/// parameter type's modelled value set.
///
/// Built from the request and the model and from nothing else: the TYPE whose
/// set the value missed, the value exactly as the request sent it (bounded by
/// the framework's echo cap on a violation's value), and that type's modelled
/// values in the order the model declares them — which is the order the
/// violation carries them in. The rejected value IS named here, unlike in the
/// generic report: it is the token the caller has to replace, and a caller that
/// sent a list of them needs to know which one was refused.
fn invalid_enum_value_message(enum_type: &str, value: &str, valid: &[String]) -> String {
    format!(
        "Invalid {enum_type} value: {value}. Valid values: {}",
        valid.join(", ")
    )
}

/// The requirement a violation states.
///
/// A `@pattern` is reported unanchored, because the requirement is the
/// expression a value must match and the `^`/`$` are how the model spells
/// "whole value". Every other constraint keeps the framework's own requirement
/// wording, read off the clause it builds, so a constraint kind added to the
/// framework later is reported rather than dropped.
fn violation_requirement(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => {
            format!(
                "Member must satisfy regular expression pattern: {}",
                unanchored(pattern)
            )
        }
        _ => {
            // Read the requirement off a clause the framework builds for this
            // SAME violation with no value attached. A clause that carried the
            // value would put caller-controlled text before the marker, so a
            // value containing the marker itself would split inside the value
            // and copy the rest of it into the report.
            let valueless = Violation {
                value: None,
                ..violation.clone()
            };
            let clause = valueless.message();
            match clause.split_once(CONSTRAINT_MARKER) {
                Some((_, requirement)) => requirement.to_string(),
                None => clause,
            }
        }
    }
}

/// `pattern` without its `^` prefix and `$` suffix.
fn unanchored(pattern: &str) -> &str {
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret.strip_suffix('$').unwrap_or(without_caret)
}

/// The enrollment configuration of one account.
#[derive(Debug, Clone)]
pub struct EnrollmentConfig {
    pub status: String,
    pub status_reason: Option<String>,
    pub organization_rule_mode: Option<String>,
    pub last_updated_timestamp: Option<::aws_smithy_types::DateTime>,
}

impl Default for EnrollmentConfig {
    fn default() -> Self {
        Self {
            status: DEFAULT_ENROLLMENT_STATUS.to_string(),
            status_reason: None,
            organization_rule_mode: None,
            last_updated_timestamp: None,
        }
    }
}

/// An automation rule as stored. Every `Get`/`List`/`Update` response is built
/// from one of these, never from the request that created it.
#[derive(Debug, Clone)]
pub struct StoredRule {
    pub rule_arn: String,
    pub rule_id: String,
    pub account_id: String,
    pub name: String,
    /// Always a value: a create that supplied none stores
    /// [`DEFAULT_RULE_DESCRIPTION`].
    pub description: String,
    pub rule_type: String,
    pub rule_revision: i64,
    pub organization_configuration: Option<OrganizationConfiguration>,
    /// Always a value: a create that supplied none stores
    /// [`DEFAULT_RULE_PRIORITY`].
    pub priority: String,
    pub recommended_action_types: Vec<String>,
    pub criteria: Option<Criteria>,
    pub schedule: Schedule,
    pub status: String,
    pub tags: Vec<Tag>,
    pub created_timestamp: ::aws_smithy_types::DateTime,
    /// When this rule was last changed. Always a value: a rule that has never
    /// been updated was last changed when it was created, so a create stores its
    /// own creation instant here.
    pub last_updated_timestamp: ::aws_smithy_types::DateTime,
}

impl StoredRule {
    /// This rule as the `AutomationRule` summary `ListAutomationRules` returns.
    /// The summary shape carries neither `criteria` nor `tags` — the model does
    /// not declare them on it — so those are reachable only through
    /// `GetAutomationRule`.
    pub fn to_automation_rule(&self) -> AutomationRule {
        AutomationRule {
            rule_arn: Some(self.rule_arn.clone()),
            rule_id: Some(self.rule_id.clone()),
            name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            rule_type: Some(self.rule_type.clone()),
            rule_revision: Some(self.rule_revision),
            account_id: Some(self.account_id.clone()),
            organization_configuration: self.organization_configuration.clone(),
            priority: Some(self.priority.clone()),
            recommended_action_types: Some(self.recommended_action_types.clone()),
            schedule: Some(self.schedule.clone()),
            status: Some(self.status.clone()),
            created_timestamp: Some(self.created_timestamp),
            last_updated_timestamp: Some(self.last_updated_timestamp),
        }
    }
}

/// The rules of this service, in insertion order, with the two lookups the API
/// needs: by `ruleArn` (every operation takes one) and by `ruleId` (which events
/// reference). Keeping all three in one type is what keeps them consistent:
/// `insert` and `remove` are the only writers.
#[derive(Debug, Default)]
pub struct RuleStore {
    /// `ruleArn`s in creation order, so `ListAutomationRules` pages over a
    /// stable sequence.
    order: Vec<String>,
    by_arn: HashMap<String, StoredRule>,
    /// Secondary index: `ruleId` -> `ruleArn`.
    by_id: HashMap<String, String>,
}

impl RuleStore {
    /// Store a newly created rule, appending it to the insertion order.
    pub fn insert(&mut self, rule: StoredRule) {
        self.by_id
            .insert(rule.rule_id.clone(), rule.rule_arn.clone());
        self.order.push(rule.rule_arn.clone());
        self.by_arn.insert(rule.rule_arn.clone(), rule);
    }

    pub fn get(&self, rule_arn: &str) -> Option<&StoredRule> {
        self.by_arn.get(rule_arn)
    }

    pub fn get_mut(&mut self, rule_arn: &str) -> Option<&mut StoredRule> {
        self.by_arn.get_mut(rule_arn)
    }

    /// The rule a `ruleId` names, resolved through the secondary index rather
    /// than by scanning.
    pub fn get_by_rule_id(&self, rule_id: &str) -> Option<&StoredRule> {
        let arn = self.by_id.get(rule_id)?;
        self.by_arn.get(arn)
    }

    pub fn contains_rule_id(&self, rule_id: &str) -> bool {
        self.by_id.contains_key(rule_id)
    }

    /// Remove a rule, dropping it from the order and both indexes. `None` when
    /// no rule has that ARN.
    pub fn remove(&mut self, rule_arn: &str) -> Option<StoredRule> {
        let rule = self.by_arn.remove(rule_arn)?;
        self.by_id.remove(&rule.rule_id);
        self.order.retain(|arn| arn != rule_arn);
        Some(rule)
    }

    /// Every stored rule, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &StoredRule> {
        self.order.iter().filter_map(|arn| self.by_arn.get(arn))
    }

    /// How many rules `account_id` owns, for the [`MAX_RULES_PER_ACCOUNT`] quota.
    pub fn count_for_account(&self, account_id: &str) -> usize {
        self.by_arn
            .values()
            .filter(|rule| rule.account_id == account_id)
            .count()
    }
}

/// The recommended actions this service holds, in the order they were recorded,
/// with the by-id lookup `StartAutomationEvent` needs.
///
/// Compute Optimizer's own analysis is what produces a recommended action, so no
/// operation of THIS API creates one: the store holds the modelled
/// `RecommendedAction` as recorded, and `ListRecommendedActions` reports entries
/// straight out of it, never from the request. It reads like the map it replaces
/// — `insert(id, action)` / `get(id)` — while also keeping the record order a
/// paginated list has to page over. `insert` is the only writer, so the order and
/// the index cannot drift apart.
#[derive(Debug, Default)]
pub struct RecommendedActionStore {
    /// `recommendedActionId`s in record order, so a list pages over a stable
    /// sequence.
    order: Vec<String>,
    by_id: HashMap<String, RecommendedAction>,
}

impl RecommendedActionStore {
    /// Record `action` under `recommended_action_id`. Re-recording the same id
    /// replaces the stored action in place, keeping its position in the order.
    pub fn insert(&mut self, recommended_action_id: String, action: RecommendedAction) {
        if !self.by_id.contains_key(&recommended_action_id) {
            self.order.push(recommended_action_id.clone());
        }
        self.by_id.insert(recommended_action_id, action);
    }

    /// The action a `recommendedActionId` names.
    pub fn get(&self, recommended_action_id: &str) -> Option<&RecommendedAction> {
        self.by_id.get(recommended_action_id)
    }

    /// Every recorded action, in record order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &RecommendedAction> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// How many actions are recorded.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// This action as the `PreviewResult` `ListAutomationRulePreview` reports. The
/// model's `PreviewResult` repeats every member of `RecommendedAction`, so the
/// preview is the stored action itself, projected member for member.
pub fn preview_result_for(action: &RecommendedAction) -> PreviewResult {
    PreviewResult {
        recommended_action_id: action.recommended_action_id.clone(),
        resource_arn: action.resource_arn.clone(),
        resource_id: action.resource_id.clone(),
        account_id: action.account_id.clone(),
        region: action.region.clone(),
        resource_type: action.resource_type.clone(),
        look_back_period_in_days: action.look_back_period_in_days,
        recommended_action_type: action.recommended_action_type.clone(),
        current_resource_summary: action.current_resource_summary.clone(),
        current_resource_details: action.current_resource_details.clone(),
        recommended_resource_summary: action.recommended_resource_summary.clone(),
        recommended_resource_details: action.recommended_resource_details.clone(),
        restart_needed: action.restart_needed,
        estimated_monthly_savings: action.estimated_monthly_savings.clone(),
        resource_tags: action.resource_tags.clone(),
    }
}

/// The recommended actions ONE rule-preview request selects, in the order the
/// store holds them: every stored action that belongs to an account `scope`
/// covers, whose `recommendedActionType` is one of `recommended_action_types`,
/// and that satisfies every condition of `criteria`.
///
/// A preview request names its selection the same way whichever preview
/// operation reads it — `ListAutomationRulePreview` reports the selected actions
/// one by one and `ListAutomationRulePreviewSummaries` aggregates the very same
/// set — so the selection lives here once. An action that records no
/// `accountId` belongs to no account, so no scope covers it.
pub fn previewed_actions<'store>(
    actions: &'store RecommendedActionStore,
    scope: Option<&'store [String]>,
    recommended_action_types: &'store [String],
    criteria: &'store Criteria,
) -> impl Iterator<Item = &'store RecommendedAction> + 'store {
    actions
        .iter_in_order()
        .filter(move |action| {
            action
                .account_id
                .as_deref()
                .is_some_and(|account_id| account_in_scope(account_id, scope))
        })
        .filter(move |action| {
            action
                .recommended_action_type
                .as_ref()
                .is_some_and(|action_type| recommended_action_types.contains(action_type))
        })
        .filter(move |action| action_matches_criteria(action, criteria))
}

/// The `estimatedMonthlySavings` of a GROUP of recommended actions: the sums of
/// the before- and after-discount figures of the estimates the group carries.
///
/// An action Compute Optimizer has not estimated savings for carries no
/// estimate, which adds nothing to either sum (and is still one action of the
/// group, so the group's count is not this function's business).
///
/// * `currency` is the currency the group's estimates AGREE on. The model types
///   it as a free-form `String`, so a group holding no estimate — or estimates
///   in different currencies, whose amounts are not one currency's total —
///   names no currency: the empty string.
/// * `savingsEstimationMode` is typed as the `SavingsEstimationMode` enum
///   (`BeforeDiscount | AfterDiscount`), so a total must name one of the two
///   even when the group's estimates disagree or carry none. It reports the mode
///   the group agrees on, else `BeforeDiscount`, the conservative reading this
///   module already takes for an estimate whose mode it cannot place (see
///   `savings_for_criteria`).
pub fn total_savings<'a>(
    estimates: impl Iterator<Item = &'a EstimatedMonthlySavings>,
) -> EstimatedMonthlySavings {
    let mut total = EstimatedMonthlySavings {
        currency: String::new(),
        before_discount_savings: 0.0,
        after_discount_savings: 0.0,
        savings_estimation_mode: SAVINGS_MODE_BEFORE_DISCOUNT.to_string(),
    };
    let mut currency: Option<&str> = None;
    let mut mode: Option<&str> = None;
    let mut agreed_currency = true;
    let mut agreed_mode = true;
    for estimate in estimates {
        total.before_discount_savings += estimate.before_discount_savings;
        total.after_discount_savings += estimate.after_discount_savings;
        match currency {
            None => currency = Some(estimate.currency.as_str()),
            Some(seen) if seen != estimate.currency => agreed_currency = false,
            Some(_) => {}
        }
        match mode {
            None => mode = Some(estimate.savings_estimation_mode.as_str()),
            Some(seen) if seen != estimate.savings_estimation_mode => agreed_mode = false,
            Some(_) => {}
        }
    }
    if agreed_currency {
        if let Some(currency) = currency {
            total.currency = currency.to_string();
        }
    }
    if agreed_mode {
        // A stored mode outside the model's `SavingsEstimationMode` value set
        // cannot be put on the wire as that enum, so it reads as the
        // conservative before-discount mode the default already names.
        if let Some(mode) = mode.filter(|mode| SAVINGS_ESTIMATION_MODE_VALUES.contains(mode)) {
            total.savings_estimation_mode = mode.to_string();
        }
    }
    total
}

/// The `SavingsEstimationMode` a total names when the group's estimates do not
/// agree on one, carry none, or name one the model does not declare: the
/// before-discount reading, which is the conservative one (see
/// [`total_savings`]).
pub const SAVINGS_MODE_BEFORE_DISCOUNT: &str = "BeforeDiscount";

/// The other `SavingsEstimationMode` the model declares: the figure AFTER any
/// discounts.
pub const SAVINGS_MODE_AFTER_DISCOUNT: &str = "AfterDiscount";

/// The EBS volume configuration carried by `details`, which is what the
/// `ebsVolumeType` and `ebsVolumeSizeInGib` criteria read off the CURRENT
/// resource: the criteria choose which resources a rule acts on, so they read
/// the resource as it is now, never the recommended one.
fn ebs_configuration(details: Option<&ResourceDetails>) -> Option<&EbsVolumeConfiguration> {
    match details? {
        ResourceDetails::ebsVolume(volume) => volume.configuration.as_ref(),
    }
}

/// The value an `estimatedMonthlySavings` criterion compares against: the figure
/// for the estimate's OWN `savingsEstimationMode`, so a `BeforeDiscount`
/// estimate is compared before discounts and an `AfterDiscount` one after them.
fn savings_for_criteria(savings: &EstimatedMonthlySavings) -> f64 {
    match savings.savings_estimation_mode.as_str() {
        SAVINGS_MODE_AFTER_DISCOUNT => savings.after_discount_savings,
        // `BeforeDiscount` is the enum's only other member, and the
        // before-discount figure is the conservative reading of a mode this
        // model does not yet declare.
        _ => savings.before_discount_savings,
    }
}

/// The EBS volume type of `details`, for the
/// `CurrentResourceDetailsEbsVolumeType` filter. `None` when the action carries
/// no such configuration.
fn ebs_volume_type(details: Option<&ResourceDetails>) -> Option<String> {
    ebs_configuration(details)?.r#type.clone()
}

/// The steps an automation event of `event_type` performs, in the order it
/// performs them.
///
/// `EventType` is `SnapshotAndDeleteUnattachedEbsVolume | UpgradeEbsVolumeType`
/// and `StepType` is `CreateEbsSnapshot | CreateEbsVolume | DeleteEbsVolume |
/// ModifyEbsVolume` (`smithy_query(query="shape",shape="EventType,StepType")`).
/// An event type names the work its automation does, and its steps are exactly
/// that work in the order the name states it: a snapshot-and-delete takes the
/// snapshot ([`STEP_TYPE_CREATE_EBS_SNAPSHOT`]) and then deletes the volume
/// ([`STEP_TYPE_DELETE_EBS_VOLUME`]), and a volume-type upgrade modifies the
/// volume in place ([`STEP_TYPE_MODIFY_EBS_VOLUME`]). `CreateEbsVolume` belongs
/// to the ROLLBACK of a delete (restoring the volume from the snapshot the
/// first step took), which no forward run performs.
///
/// An event type outside the modelled set has no known step sequence, so it
/// gets none rather than a guessed one.
pub fn step_types_for_event_type(event_type: &str) -> &'static [&'static str] {
    match event_type {
        EVENT_TYPE_SNAPSHOT_AND_DELETE_UNATTACHED_EBS_VOLUME => {
            &[STEP_TYPE_CREATE_EBS_SNAPSHOT, STEP_TYPE_DELETE_EBS_VOLUME]
        }
        EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE => &[STEP_TYPE_MODIFY_EBS_VOLUME],
        _ => &[],
    }
}

/// One step of an automation event as stored, which is what
/// `ListAutomationEventSteps` reports. A step belongs to exactly one event and
/// is stored with it, so an evicted event takes its steps with it and no reader
/// sees a step whose event is gone.
#[derive(Debug, Clone)]
pub struct StoredStep {
    pub step_id: String,
    pub step_type: String,
    pub step_status: String,
    pub resource_id: Option<String>,
    pub start_timestamp: Option<::aws_smithy_types::DateTime>,
    pub completed_timestamp: Option<::aws_smithy_types::DateTime>,
    pub estimated_monthly_savings: Option<EstimatedMonthlySavings>,
}

impl StoredStep {
    /// This step as the `AutomationEventStep` the list operation returns.
    /// `eventId` is the owning event's, because a step is only ever reached
    /// through it.
    pub fn to_automation_event_step(&self, event_id: &str) -> AutomationEventStep {
        AutomationEventStep {
            event_id: Some(event_id.to_string()),
            step_id: Some(self.step_id.clone()),
            step_type: Some(self.step_type.clone()),
            step_status: Some(self.step_status.clone()),
            resource_id: self.resource_id.clone(),
            start_timestamp: self.start_timestamp,
            completed_timestamp: self.completed_timestamp,
            estimated_monthly_savings: self.estimated_monthly_savings.clone(),
        }
    }
}

/// The `stepId` of step `index` of the event `event_id`: the leading characters
/// of a digest over the pair. Derived rather than drawn from a counter so a
/// step's id is fixed when its event is created and no read ever mints one, and
/// it satisfies the model's `StepId` pattern `^[0-9A-Za-z]{16}$` by
/// construction.
fn step_id(event_id: &str, index: usize) -> String {
    let digest = Sha256::digest(format!("{event_id}:step:{index}").as_bytes());
    hex::encode(digest).chars().take(MINTED_ID_LEN).collect()
}

/// The steps of a freshly started event: the sequence its event type performs,
/// with the FIRST one running — the event is [`EVENT_STATUS_IN_PROGRESS`] and
/// that is the step it is on — and every later one queued
/// [`STEP_STATUS_READY`]. A step that has not started carries no timestamp.
pub fn steps_for_started_event(
    event_id: &str,
    event_type: &str,
    resource_id: Option<&str>,
    estimated_monthly_savings: Option<&EstimatedMonthlySavings>,
    started_at: ::aws_smithy_types::DateTime,
) -> Vec<StoredStep> {
    let step_types = step_types_for_event_type(event_type);
    step_types
        .iter()
        .enumerate()
        .map(|(index, step_type)| StoredStep {
            step_id: step_id(event_id, index),
            step_type: (*step_type).to_string(),
            step_status: if index == 0 {
                STEP_STATUS_IN_PROGRESS.to_string()
            } else {
                STEP_STATUS_READY.to_string()
            },
            // Every step of an event acts on the resource the event automates.
            resource_id: resource_id.map(str::to_string),
            start_timestamp: (index == 0).then_some(started_at),
            completed_timestamp: None,
            // The event's estimate is realised by the step that changes what
            // the resource costs — the LAST of the sequence, which deletes or
            // modifies the volume — so only that step carries it and summing
            // the steps of an event never double-counts its estimate.
            estimated_monthly_savings: (index + 1 == step_types.len())
                .then(|| estimated_monthly_savings.cloned())
                .flatten(),
        })
        .collect()
}

/// An automation event as stored: the run of ONE recommended action, which is
/// what `ListAutomationEvents` and the event readers report. Every response
/// member is built from one of these, never from the request that asked for it.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: String,
    pub event_description: Option<String>,
    pub event_type: String,
    pub event_status: String,
    pub event_status_reason: Option<String>,
    pub resource_arn: Option<String>,
    pub resource_id: Option<String>,
    pub recommended_action_id: Option<String>,
    pub account_id: String,
    pub region: Option<String>,
    pub rule_id: Option<String>,
    pub resource_type: String,
    pub created_timestamp: ::aws_smithy_types::DateTime,
    pub completed_timestamp: Option<::aws_smithy_types::DateTime>,
    pub estimated_monthly_savings: Option<EstimatedMonthlySavings>,
    /// The steps this event's automation performs, in the order it performs
    /// them (see [`steps_for_started_event`]). Stored WITH the event because a
    /// step is only ever reached through its event.
    pub steps: Vec<StoredStep>,
    /// When this event's terminal status falls due, or `None` when it owes no
    /// transition (it is already terminal). Stored as an instant rather than
    /// slept for, so no handler blocks on it.
    pub settles_at: Option<SystemTime>,
}

impl StoredEvent {
    /// Materialise the status the clock has already made due.
    ///
    /// The transition was owed at `settles_at` whether or not anybody called, so
    /// applying it on a read reports what the event already is rather than
    /// advancing it because it was read. A no-op before the due instant and
    /// after the event has settled.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else { return };
        if now < due {
            return;
        }
        self.event_status = EVENT_STATUS_FAILED.to_string();
        self.event_status_reason = Some(EVENT_UNPERFORMED_REASON.to_string());
        self.completed_timestamp = Some(::aws_smithy_types::DateTime::from(due));
        // The step the event was on took the event's outcome at the same
        // instant: it was running and never completed, so it failed. A step
        // still queued behind it never started, so it keeps its `Ready` status
        // and its absent timestamps rather than claiming work nothing ran.
        for step in &mut self.steps {
            if step.step_status == STEP_STATUS_IN_PROGRESS {
                step.step_status = STEP_STATUS_FAILED.to_string();
                step.completed_timestamp = Some(::aws_smithy_types::DateTime::from(due));
            }
        }
        self.settles_at = None;
    }

    /// This event as the `AutomationEvent` the list and get operations return.
    pub fn to_automation_event(&self) -> AutomationEvent {
        AutomationEvent {
            event_id: Some(self.event_id.clone()),
            event_description: self.event_description.clone(),
            event_type: Some(self.event_type.clone()),
            event_status: Some(self.event_status.clone()),
            event_status_reason: self.event_status_reason.clone(),
            resource_arn: self.resource_arn.clone(),
            resource_id: self.resource_id.clone(),
            recommended_action_id: self.recommended_action_id.clone(),
            account_id: Some(self.account_id.clone()),
            region: self.region.clone(),
            rule_id: self.rule_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            created_timestamp: Some(self.created_timestamp),
            completed_timestamp: self.completed_timestamp,
            estimated_monthly_savings: self.estimated_monthly_savings.clone(),
        }
    }
}

/// The automation events of this service, in creation order, keyed by the
/// `eventId` every event-scoped operation takes. Keeping the order beside the
/// index is what lets a list page over a stable sequence; `insert` is the only
/// writer, and it is what keeps the store bounded by [`MAX_STORED_EVENTS`].
#[derive(Debug, Default)]
pub struct EventStore {
    /// `eventId`s in creation order, oldest first, so the store can evict.
    order: VecDeque<String>,
    by_id: HashMap<String, StoredEvent>,
}

impl EventStore {
    /// Store an event, appending it to the creation order the first time its id
    /// is seen so a re-stored event keeps its place in the sequence. Evicts the
    /// oldest events once the store is full: an evicted event is the one no
    /// query can still reach first.
    pub fn insert(&mut self, event: StoredEvent) {
        let event_id = event.event_id.clone();
        if self.by_id.insert(event_id.clone(), event).is_none() {
            self.order.push_back(event_id);
        }
        while self.order.len() > MAX_STORED_EVENTS {
            if let Some(oldest) = self.order.pop_front() {
                self.by_id.remove(&oldest);
            }
        }
    }

    /// Every stored event, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &StoredEvent> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// The event an `eventId` names, which is how every event-scoped operation
    /// reaches one.
    pub fn get(&self, event_id: &str) -> Option<&StoredEvent> {
        self.by_id.get(event_id)
    }

    /// As [`get`](Self::get), for a caller that has to materialise the status
    /// the clock owes the event (see [`StoredEvent::settle`]).
    pub fn get_mut(&mut self, event_id: &str) -> Option<&mut StoredEvent> {
        self.by_id.get_mut(event_id)
    }

    /// Materialise every status transition the clock has already made due, so
    /// each reader of this store reports the same status for an event (see
    /// [`StoredEvent::settle`]). A no-op for an event that owes none.
    pub fn settle_due(&mut self, now: SystemTime) {
        for event in self.by_id.values_mut() {
            event.settle(now);
        }
    }

    pub fn contains(&self, event_id: &str) -> bool {
        self.by_id.contains_key(event_id)
    }
}

/// A newly started automation event for `action`, owned by `account_id`.
///
/// The event is built from the stored recommended action — never from the
/// request — and starts in the non-terminal [`EVENT_STATUS_IN_PROGRESS`] with
/// its terminal status due [`EVENT_SETTLEMENT_DELAY`] later. Its steps are the
/// sequence its event type performs ([`steps_for_started_event`]), stored at
/// start so every later read reports the same steps with the same ids.
pub fn event_for_action(
    event_id: String,
    action: &RecommendedAction,
    account_id: String,
    started_at: SystemTime,
) -> StoredEvent {
    let event_type = action.recommended_action_type.clone().unwrap_or_default();
    let created_timestamp = ::aws_smithy_types::DateTime::from(started_at);
    let steps = steps_for_started_event(
        &event_id,
        &event_type,
        action.resource_id.as_deref(),
        action.estimated_monthly_savings.as_ref(),
        created_timestamp,
    );
    StoredEvent {
        event_id,
        event_description: None,
        // `EventType` and `RecommendedActionType` carry the same value set, and
        // the event automates exactly this action
        // (`smithy_query(query="shape",shape="EventType,RecommendedActionType")`).
        event_type,
        event_status: EVENT_STATUS_IN_PROGRESS.to_string(),
        event_status_reason: None,
        resource_arn: action.resource_arn.clone(),
        resource_id: action.resource_id.clone(),
        recommended_action_id: action.recommended_action_id.clone(),
        account_id,
        region: action.region.clone(),
        // The rule whose schedule started the event; an on-demand
        // `StartAutomationEvent` has none.
        rule_id: None,
        resource_type: action.resource_type.clone().unwrap_or_default(),
        created_timestamp,
        completed_timestamp: None,
        estimated_monthly_savings: action.estimated_monthly_savings.clone(),
        steps,
        settles_at: Some(started_at + EVENT_SETTLEMENT_DELAY),
    }
}

/// Which accounts a preview covers.
///
/// `None` means every account the store holds actions for: an `OrganizationRule`
/// preview that names no `organizationScope` spans the organization. An
/// `AccountRule` covers only the calling account, because a rule of that type is
/// scoped to the account it lives in.
///
/// An EMPTY account list cannot reach here: the model constrains
/// `OrganizationConfigurationAccountIds` at `length(min: 1, max: 50)`, so the
/// protocol boundary refuses `accountIds: []` before the handler runs. A scope
/// present with no `accountIds` member is the same as no scope at all.
pub fn preview_account_scope(
    rule_type: &str,
    organization_scope: Option<&OrganizationScope>,
) -> Option<Vec<String>> {
    if rule_type == "OrganizationRule" {
        return organization_scope
            .and_then(|scope| scope.account_ids.clone())
            .filter(|accounts| !accounts.is_empty());
    }
    Some(vec![caller_account_id()])
}

/// Is `account_id` one of the accounts `scope` covers? An absent scope covers
/// every account.
pub fn account_in_scope(account_id: &str, scope: Option<&[String]>) -> bool {
    scope.is_none_or(|accounts| accounts.iter().any(|candidate| candidate == account_id))
}

/// Does `action` satisfy every condition of `criteria`?
///
/// A criterion holds a LIST of conditions and `criteria` holds several
/// criterions: all of them must hold (AND), while the values inside one
/// condition are alternatives (OR). A criterion the caller left out constrains
/// nothing, so empty criteria match every action.
pub fn action_matches_criteria(action: &RecommendedAction, criteria: &Criteria) -> bool {
    let configuration = ebs_configuration(action.current_resource_details.as_ref());
    // `restartNeeded` is modelled as a STRING criterion over a boolean member,
    // so it is compared against the member's wire rendering.
    let restart_needed = action
        .restart_needed
        .map(|needed| if needed { "true" } else { "false" });

    criteria
        .region
        .iter()
        .flatten()
        .all(|condition| string_condition_holds(condition, action.region.as_deref()))
        && criteria
            .resource_arn
            .iter()
            .flatten()
            .all(|condition| string_condition_holds(condition, action.resource_arn.as_deref()))
        && criteria.ebs_volume_type.iter().flatten().all(|condition| {
            string_condition_holds(
                condition,
                configuration.and_then(|configuration| configuration.r#type.as_deref()),
            )
        })
        && criteria
            .restart_needed
            .iter()
            .flatten()
            .all(|condition| string_condition_holds(condition, restart_needed))
        && criteria
            .ebs_volume_size_in_gib
            .iter()
            .flatten()
            .all(|condition| {
                integer_condition_holds(
                    condition,
                    configuration
                        .and_then(|configuration| configuration.size_in_gib)
                        .map(f64::from),
                )
            })
        && criteria
            .look_back_period_in_days
            .iter()
            .flatten()
            .all(|condition| {
                integer_condition_holds(condition, action.look_back_period_in_days.map(f64::from))
            })
        && criteria
            .estimated_monthly_savings
            .iter()
            .flatten()
            .all(|condition| {
                double_condition_holds(
                    condition,
                    action
                        .estimated_monthly_savings
                        .as_ref()
                        .map(savings_for_criteria),
                )
            })
        && criteria
            .resource_tag
            .iter()
            .flatten()
            .all(|condition| tag_condition_holds(condition, action))
}

/// The `String*` comparison operators, i.e. the members of the model's
/// `ComparisonOperator` enum that compare strings, with the `IfExists` suffix
/// stripped (`smithy_query(query="shape",shape="ComparisonOperator")`).
const STRING_COMPARISONS: &[&str] = &[
    "StringEquals",
    "StringNotEquals",
    "StringEqualsIgnoreCase",
    "StringNotEqualsIgnoreCase",
    "StringLike",
    "StringNotLike",
];

/// The `Numeric*` comparison operators of the same enum, `IfExists` stripped.
const NUMERIC_COMPARISONS: &[&str] = &[
    "NumericEquals",
    "NumericNotEquals",
    "NumericLessThan",
    "NumericLessThanEquals",
    "NumericGreaterThan",
    "NumericGreaterThanEquals",
];

/// A comparison operator split into the base operator and whether it carried the
/// `IfExists` suffix. `IfExists` is what decides a condition over an attribute
/// the resource does not have: it holds, while every other operator does not.
fn split_comparison(comparison: &str) -> (&str, bool) {
    match comparison.strip_suffix("IfExists") {
        Some(base) => (base, true),
        None => (comparison, false),
    }
}

/// Does a string condition hold for `attribute`?
fn string_comparison_holds(comparison: &str, attribute: Option<&str>, values: &[String]) -> bool {
    let (base, if_exists) = split_comparison(comparison);
    let Some(attribute) = attribute else {
        return if_exists;
    };
    match base {
        "StringEquals" => values.iter().any(|value| value == attribute),
        "StringNotEquals" => !values.iter().any(|value| value == attribute),
        "StringEqualsIgnoreCase" => values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(attribute)),
        "StringNotEqualsIgnoreCase" => !values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(attribute)),
        "StringLike" => values
            .iter()
            .any(|value| wildcard_matches(value, attribute)),
        "StringNotLike" => !values
            .iter()
            .any(|value| wildcard_matches(value, attribute)),
        // Unreachable: a comparison outside the operator's family is refused
        // before any action is matched.
        _ => false,
    }
}

/// Does `candidate` match the `StringLike` wildcard `pattern`?
///
/// The wildcard language of a `String*Like` condition value is the two
/// metacharacters an AWS condition uses: `*` stands for any run of characters
/// (including none) and `?` for exactly one; every other character is literal,
/// compared by Unicode scalar. Two metacharacters are not a pattern language, so
/// this is the textbook greedy walk rather than a dependency: `star` remembers
/// the last `*` seen and `resume` the input position it matched from, so a later
/// mismatch backtracks by giving that `*` one more character. It visits each
/// input position at most once per `*`, so it always terminates.
fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let candidate: Vec<char> = candidate.chars().collect();
    let mut pattern_at = 0usize;
    let mut candidate_at = 0usize;
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while candidate_at < candidate.len() {
        match pattern.get(pattern_at) {
            Some('*') => {
                star = Some(pattern_at);
                resume = candidate_at;
                pattern_at += 1;
            }
            Some('?') => {
                pattern_at += 1;
                candidate_at += 1;
            }
            Some(literal) if *literal == candidate[candidate_at] => {
                pattern_at += 1;
                candidate_at += 1;
            }
            // No literal match here: hand one more character to the last `*`, or
            // report the mismatch when there was none.
            _ => {
                let Some(star_at) = star else {
                    return false;
                };
                pattern_at = star_at + 1;
                resume += 1;
                candidate_at = resume;
            }
        }
    }

    // The input is consumed: what is left of the pattern may only be `*`s, each
    // of which can match the empty rest.
    pattern[pattern_at..].iter().all(|left| *left == '*')
}

/// Does a numeric condition hold for `attribute`? Several values are
/// alternatives, as they are for a string condition.
fn numeric_comparison_holds(comparison: &str, attribute: Option<f64>, values: &[f64]) -> bool {
    let (base, if_exists) = split_comparison(comparison);
    let Some(attribute) = attribute else {
        return if_exists;
    };
    match base {
        "NumericEquals" => values.contains(&attribute),
        "NumericNotEquals" => !values.contains(&attribute),
        "NumericLessThan" => values.iter().any(|value| attribute < *value),
        "NumericLessThanEquals" => values.iter().any(|value| attribute <= *value),
        "NumericGreaterThan" => values.iter().any(|value| attribute > *value),
        "NumericGreaterThanEquals" => values.iter().any(|value| attribute >= *value),
        _ => false,
    }
}

fn string_condition_holds(condition: &StringCriteriaCondition, attribute: Option<&str>) -> bool {
    string_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        condition.values.as_deref().unwrap_or_default(),
    )
}

fn integer_condition_holds(condition: &IntegerCriteriaCondition, attribute: Option<f64>) -> bool {
    let values: Vec<f64> = condition
        .values
        .iter()
        .flatten()
        .copied()
        .map(f64::from)
        .collect();
    numeric_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        &values,
    )
}

fn double_condition_holds(condition: &DoubleCriteriaCondition, attribute: Option<f64>) -> bool {
    numeric_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        condition.values.as_deref().unwrap_or_default(),
    )
}

/// Does a `resourceTag` condition hold? It names the tag KEY it is about, and
/// compares its values against the value of that one tag; a resource without the
/// tag has no attribute, so only an `IfExists` comparison holds for it.
fn tag_condition_holds(
    condition: &ResourceTagsCriteriaCondition,
    action: &RecommendedAction,
) -> bool {
    let key = condition.key.as_deref().unwrap_or_default();
    let attribute = action
        .resource_tags
        .iter()
        .flatten()
        .find(|tag| tag.key == key)
        .map(|tag| tag.value.as_str());
    string_comparison_holds(
        condition.comparison.as_deref().unwrap_or_default(),
        attribute,
        condition.values.as_deref().unwrap_or_default(),
    )
}

/// Whether a criterion compares strings or numbers, which is what decides the
/// family of comparison operators it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CriterionKind {
    Text,
    Numeric,
}

/// Why this `criteria` cannot be evaluated, if it cannot, as the service's own
/// validation report ([`constraint_report`], which echoes no request value).
///
/// The model types `comparison` and `values` as OPTIONAL members of every
/// condition and types `comparison` as the whole `ComparisonOperator` enum, so
/// the protocol boundary admits a condition with no operator, with no values, or
/// with an operator from the wrong family (a `Numeric*` operator on a string
/// criterion). None of those can select resources, so each is refused rather
/// than quietly matching nothing.
pub fn criteria_violation(criteria: &Criteria) -> Option<String> {
    let mut conditions: Vec<(&str, CriterionKind, Option<&str>, usize)> = Vec::new();
    let text_criteria: [(&str, &Option<Vec<StringCriteriaCondition>>); 4] = [
        ("criteria.region", &criteria.region),
        ("criteria.resourceArn", &criteria.resource_arn),
        ("criteria.ebsVolumeType", &criteria.ebs_volume_type),
        ("criteria.restartNeeded", &criteria.restart_needed),
    ];
    for (path, criterion) in text_criteria {
        for condition in criterion.iter().flatten() {
            conditions.push((
                path,
                CriterionKind::Text,
                condition.comparison.as_deref(),
                condition.values.as_ref().map_or(0, Vec::len),
            ));
        }
    }
    for condition in criteria.resource_tag.iter().flatten() {
        conditions.push((
            "criteria.resourceTag",
            CriterionKind::Text,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    for condition in criteria.ebs_volume_size_in_gib.iter().flatten() {
        conditions.push((
            "criteria.ebsVolumeSizeInGib",
            CriterionKind::Numeric,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    for condition in criteria.look_back_period_in_days.iter().flatten() {
        conditions.push((
            "criteria.lookBackPeriodInDays",
            CriterionKind::Numeric,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    for condition in criteria.estimated_monthly_savings.iter().flatten() {
        conditions.push((
            "criteria.estimatedMonthlySavings",
            CriterionKind::Numeric,
            condition.comparison.as_deref(),
            condition.values.as_ref().map_or(0, Vec::len),
        ));
    }
    // A `resourceTag` condition is also about one tag KEY, so a condition
    // without one names no attribute to compare.
    let missing_tag_key = criteria
        .resource_tag
        .iter()
        .flatten()
        .any(|condition| condition.key.as_deref().unwrap_or_default().is_empty());
    if missing_tag_key {
        return Some(constraint_report(
            "criteria.resourceTag.member.key",
            "Member is required",
        ));
    }

    conditions
        .into_iter()
        .find_map(|(path, kind, comparison, value_count)| {
            condition_violation(path, kind, comparison, value_count)
        })
}

/// Why one criteria condition cannot be evaluated, if it cannot.
fn condition_violation(
    path: &str,
    kind: CriterionKind,
    comparison: Option<&str>,
    value_count: usize,
) -> Option<String> {
    let Some(comparison) = comparison else {
        return Some(constraint_report(
            &format!("{path}.member.comparison"),
            "Member is required",
        ));
    };
    let (base, _) = split_comparison(comparison);
    let (accepted, family) = match kind {
        CriterionKind::Text => (STRING_COMPARISONS, "String"),
        CriterionKind::Numeric => (NUMERIC_COMPARISONS, "Numeric"),
    };
    if !accepted.contains(&base) {
        return Some(constraint_report(
            &format!("{path}.member.comparison"),
            &format!("Member must be a {family} comparison operator for this criterion"),
        ));
    }
    if value_count == 0 {
        return Some(constraint_report(
            &format!("{path}.member.values"),
            "Member must have length greater than or equal to 1",
        ));
    }
    None
}

/// A total order key for a wire timestamp: `DateTime` carries seconds and
/// sub-second nanos, so comparing the pair orders two instants exactly.
pub fn instant_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// Is `at` inside the half-open window `[start, end)`? An absent bound leaves
/// that side unbounded, and the member names fix which side is which:
/// `startTimeInclusive` is included, `endTimeExclusive` is not.
pub fn within_window(
    at: ::aws_smithy_types::DateTime,
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
) -> bool {
    let at = instant_key(at);
    start.is_none_or(|start| at >= instant_key(start))
        && end.is_none_or(|end| at < instant_key(end))
}

/// Is the queried time range a window that can hold anything?
///
/// `endTimeExclusive` is exclusive, so a range whose end does not come after
/// its start describes an empty — or inverted — window and selects nothing; the
/// service refuses such a request instead of reporting an empty page. A range
/// with only one bound is always valid: the other side is unbounded.
pub fn time_range_is_valid(
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
) -> bool {
    match (start, end) {
        (Some(start), Some(end)) => instant_key(start) < instant_key(end),
        _ => true,
    }
}

/// What the service says when [`time_range_is_valid`] refuses a range: one
/// constant sentence, carrying neither bound's value, so every refused range
/// reports the same text. Recorded from the service itself.
pub const INVALID_TIME_RANGE_MESSAGE: &str = "Start date must be before end date";

/// The effective start of the window an event query reports over: the LATER of
/// the caller's `startTimeInclusive` and the oldest retrievable instant
/// ([`EVENT_RETRIEVAL_WINDOW`] before `now`), so an event created outside the
/// retrieval window is never reported however far back the caller asks.
pub fn effective_window_start(
    requested: Option<::aws_smithy_types::DateTime>,
    now: SystemTime,
) -> ::aws_smithy_types::DateTime {
    let floor = ::aws_smithy_types::DateTime::from(
        now.checked_sub(EVENT_RETRIEVAL_WINDOW).unwrap_or(UNIX_EPOCH),
    );
    match requested {
        Some(start) if instant_key(start) > instant_key(floor) => start,
        _ => floor,
    }
}

/// Does `event` satisfy every `ListAutomationEvents` filter?
///
/// Same rule as [`rule_matches_filters`]: a filter matches when the event's
/// corresponding field equals any of that filter's values, and several filters
/// are ANDed. The names are the model's `AutomationEventFilterName` value set,
/// and an unrecognised one is reported by [`unknown_event_filter_name`] rather
/// than silently ignored.
pub fn event_matches_filters(event: &StoredEvent, filters: &[AutomationEventFilter]) -> bool {
    filters.iter().all(|filter| {
        let field: Option<&str> = match filter.name.as_str() {
            "AccountId" => Some(event.account_id.as_str()),
            "ResourceType" => Some(event.resource_type.as_str()),
            "EventType" => Some(event.event_type.as_str()),
            "EventStatus" => Some(event.event_status.as_str()),
            // An unknown name never reaches here: the caller refuses it first.
            _ => None,
        };
        field.is_some_and(|value| filter.values.iter().any(|candidate| candidate == value))
    })
}

/// The first filter name that is not one of the model's
/// `AutomationEventFilterName` values, if any.
pub fn unknown_event_filter_name(filters: &[AutomationEventFilter]) -> Option<&str> {
    filters
        .iter()
        .map(|filter| filter.name.as_str())
        .find(|name| !AUTOMATION_EVENT_FILTER_NAME_VALUES.contains(name))
}

/// What a client token already in the ledger says about the request carrying it.
#[derive(Debug)]
pub enum TokenOutcome {
    /// The token has not been seen: do the work.
    Fresh,
    /// The same token with the same parameters for the same operation: replay
    /// the earlier result, recomputed from the store.
    Replay(IdempotencyRecord),
    /// The same token and operation with DIFFERENT parameters.
    ParameterMismatch,
    /// The same token already used by a different operation.
    TokenInUse,
}

/// Which resource a client token's first use acted on.
///
/// A replay is recomputed from the store through this, so it can never report a
/// resource that is no longer there — and the two kinds this service mints are
/// named apart, because an `eventId` is not a rule ARN.
#[derive(Debug, Clone)]
pub enum TokenSubject {
    /// The `ruleArn` of the automation rule the call acted on.
    Rule(String),
    /// The `eventId` of the automation event the call started.
    Event(String),
}

impl TokenSubject {
    /// The rule ARN this subject names, or `None` when it names an event.
    pub fn rule_arn(&self) -> Option<&str> {
        match self {
            Self::Rule(rule_arn) => Some(rule_arn),
            Self::Event(_) => None,
        }
    }

    /// The event id this subject names, or `None` when it names a rule.
    pub fn event_id(&self) -> Option<&str> {
        match self {
            Self::Event(event_id) => Some(event_id),
            Self::Rule(_) => None,
        }
    }
}

/// One entry of the client-token ledger. It records WHICH resource the call
/// acted on rather than a serialized body, so a replay is recomputed from the
/// store and can never report a resource that is no longer there.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub operation: &'static str,
    pub request_fingerprint: String,
    pub subject: Option<TokenSubject>,
    /// When the token was first used, for the [`IDEMPOTENCY_TTL`] window.
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState {
    /// Enrollment configuration per account id.
    pub enrollments: HashMap<String, EnrollmentConfig>,
    /// Every automation rule, in creation order.
    pub rules: RuleStore,
    /// The recommended actions this service can automate, by
    /// `recommendedActionId` and in record order.
    ///
    /// A recommended action is produced by Compute Optimizer's analysis of the
    /// account's resources, which this emulator does not perform, so nothing in
    /// this API populates it: `StartAutomationEvent` resolves every id against
    /// this ONE store, `ListRecommendedActions` pages over it and
    /// `ListAutomationRulePreview` previews it, so no two of them read two
    /// copies of one resource.
    pub recommended_actions: RecommendedActionStore,
    /// Every automation event, in creation order. Written by the operations
    /// that start and roll back an event; read by the event readers.
    pub events: EventStore,
    /// `clientToken` -> what the call that first used it did. Bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`] and [`IDEMPOTENCY_TTL`].
    pub idempotency: HashMap<String, IdempotencyRecord>,
    /// The tokens in `idempotency`, oldest first, so the ledger can evict.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter mixed into a minted id, so two resources created in
    /// the same nanosecond still get different ids.
    id_sequence: u64,
}

impl ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState {
    /// The enrollment configuration of `account_id`, or the unenrolled default
    /// when that account has never configured one. A READ: it stores nothing.
    pub fn enrollment(&self, account_id: &str) -> EnrollmentConfig {
        self.enrollments.get(account_id).cloned().unwrap_or_default()
    }

    /// Is `account_id` opted in to Compute Optimizer Automation, i.e. does its
    /// stored enrollment report [`ENROLLED_STATUS`]? An account that never
    /// enrolled is not: [`EnrollmentConfig::default`] is the unenrolled one.
    pub fn is_enrolled(&self, account_id: &str) -> bool {
        self.enrollment(account_id).status == ENROLLED_STATUS
    }

    /// The service-wide enrollment pre-check, as the refusal it produces.
    ///
    /// `OptInRequiredException` is one of this service's COMMON errors
    /// (`smithy_query(query="service_info")`), so it is bound to every operation,
    /// and its shape documents "The account must be opted in to Compute Optimizer
    /// Automation before performing this action" with `httpError` 403. Every
    /// operation that performs one of this service's ACTIONS therefore refuses a
    /// caller whose account is not opted in, BEFORE it resolves a resource or
    /// evaluates a filter — so an unenrolled caller is never told "no such
    /// resource" or handed an empty listing.
    ///
    /// The two enrollment-configuration operations are the exception, and are the
    /// only ones that do not call this: `UpdateEnrollmentConfiguration` is the
    /// documented remedy for this very error (gating it would make the refusal
    /// unrecoverable) and `GetEnrollmentConfiguration` is how a caller reads the
    /// status it has to change, including [`DEFAULT_ENROLLMENT_STATUS`].
    ///
    /// `Some(..)` is the refusal to return; `None` means the gate is open. It
    /// reads the CALLER's own account, which is what the gate is about; an
    /// operation that has to judge some other, NAMED account (one listed in an
    /// organization request, say) asks [`Self::is_enrolled`] about that account.
    pub fn opt_in_required(&self) -> Option<OptInRequiredException> {
        (!self.is_enrolled(&caller_account_id())).then(|| OptInRequiredException {
            message: Some(OPT_IN_REQUIRED_MESSAGE.to_string()),
        })
    }

    /// Is `account_id` ASSOCIATED with the organization's management account,
    /// i.e. does its stored enrollment report
    /// [`ORGANIZATION_RULE_MODE_ANY_ALLOWED`]?
    ///
    /// Association is what that rule mode records: `AssociateAccounts` sets it
    /// to "Any allowed", which is what "permits the management account to create
    /// Automation rules that automatically apply actions to that account". An
    /// account that has never been associated has no rule mode at all, so it is
    /// not associated.
    pub fn is_associated(&self, account_id: &str) -> bool {
        self.enrollment(account_id)
            .organization_rule_mode
            .as_deref()
            == Some(ORGANIZATION_RULE_MODE_ANY_ALLOWED)
    }

    /// How many of `account_ids` the enrollment store does not hold an entry for
    /// yet — i.e. how much an association of exactly those accounts would GROW
    /// the store. An id already stored costs nothing, whatever its status.
    pub fn accounts_not_stored(&self, account_ids: &[String]) -> usize {
        account_ids
            .iter()
            .filter(|account_id| !self.enrollments.contains_key(*account_id))
            .count()
    }

    /// Would storing an entry for each of `account_ids` leave the enrollment
    /// store holding more than [`MAX_ENROLLED_ACCOUNTS`] accounts? The store is
    /// in memory and `AssociateAccounts` keys it by request-supplied ids, so a
    /// request past the cap is refused instead of applied.
    pub fn would_exceed_account_limit(&self, account_ids: &[String]) -> bool {
        self.enrollments
            .len()
            .saturating_add(self.accounts_not_stored(account_ids))
            > MAX_ENROLLED_ACCOUNTS
    }

    /// Associate `account_id` with the organization's management account, as
    /// `AssociateAccounts` documents the association: the account's organization
    /// rule mode becomes [`ORGANIZATION_RULE_MODE_ANY_ALLOWED`], and an account
    /// that "has not previously enabled the Automation feature" has it enabled —
    /// which is [`ENROLLED_STATUS`] — by the association itself.
    ///
    /// Answers whether the stored configuration CHANGED. `at` is stamped as the
    /// account's `lastUpdatedTimestamp` ("The timestamp when the account's
    /// Automation enrollment status was last updated") only when it did, so
    /// re-associating an account that is already associated and enrolled leaves
    /// its stored configuration, timestamp included, exactly where it was.
    pub fn associate_account(&mut self, account_id: &str, at: ::aws_smithy_types::DateTime) -> bool {
        let enrollment = self.enrollments.entry(account_id.to_string()).or_default();
        let mut changed = false;
        if enrollment.status != ENROLLED_STATUS {
            enrollment.status = ENROLLED_STATUS.to_string();
            // The reason belongs to the status it explains, and the status this
            // account now reports was not reached by a failure.
            enrollment.status_reason = None;
            changed = true;
        }
        if enrollment.organization_rule_mode.as_deref() != Some(ORGANIZATION_RULE_MODE_ANY_ALLOWED) {
            enrollment.organization_rule_mode =
                Some(ORGANIZATION_RULE_MODE_ANY_ALLOWED.to_string());
            changed = true;
        }
        if changed {
            enrollment.last_updated_timestamp = Some(at);
        }
        changed
    }

    /// A fresh `ruleId` matching the model's `^[0-9A-Za-z]{16}$`, retried in the
    /// (astronomically unlikely) event of a collision with a live rule.
    pub fn mint_rule_id(&mut self) -> String {
        loop {
            let candidate = self.next_id();
            if !self.rules.contains_rule_id(&candidate) {
                return candidate;
            }
        }
    }

    /// A fresh `eventId` matching the model's `^[0-9A-Za-z]{16}$`, retried in
    /// the (astronomically unlikely) event of a collision with a live event.
    pub fn mint_event_id(&mut self) -> String {
        loop {
            let candidate = self.next_id();
            if !self.events.contains(&candidate) {
                return candidate;
            }
        }
    }

    /// The next candidate id: the leading hex of a digest over this store's
    /// counter and the current clock. The ONE place an id's characters are
    /// chosen, so every minted id satisfies the same `^[0-9A-Za-z]{16}$`.
    fn next_id(&mut self) -> String {
        self.id_sequence += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let digest = Sha256::digest(format!("{}:{nanos}", self.id_sequence).as_bytes());
        hex::encode(digest).chars().take(MINTED_ID_LEN).collect()
    }

    /// What the ledger says about `token` for `operation` with `request_fingerprint`.
    /// An absent token is always [`TokenOutcome::Fresh`]: a caller that sends no
    /// token asks for no replay.
    pub fn classify_token(
        &self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
    ) -> TokenOutcome {
        let Some(token) = token else {
            return TokenOutcome::Fresh;
        };
        match self.idempotency.get(token) {
            None => TokenOutcome::Fresh,
            // Past its replay window: the token is as good as unseen.
            Some(record)
                if record
                    .recorded_at
                    .elapsed()
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL) =>
            {
                TokenOutcome::Fresh
            }
            Some(record) if record.operation != operation => TokenOutcome::TokenInUse,
            Some(record) if record.request_fingerprint != request_fingerprint => {
                TokenOutcome::ParameterMismatch
            }
            Some(record) => TokenOutcome::Replay(record.clone()),
        }
    }

    /// Record that `token` was used by `operation` for `request_fingerprint`.
    /// A no-op when the caller sent no token.
    pub fn record_token(
        &mut self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
        subject: Option<TokenSubject>,
    ) {
        let Some(token) = token else { return };
        let now = SystemTime::now();
        // Drop what no retry can still ask about, then make room for this entry.
        self.evict_expired_tokens(now);
        if self
            .idempotency
            .insert(
                token.to_string(),
                IdempotencyRecord {
                    operation,
                    request_fingerprint: request_fingerprint.to_string(),
                    subject,
                    recorded_at: now,
                },
            )
            .is_none()
        {
            self.idempotency_order.push_back(token.to_string());
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token past its [`IDEMPOTENCY_TTL`] window.
    fn evict_expired_tokens(&mut self, now: SystemTime) {
        while let Some(oldest) = self.idempotency_order.front() {
            let expired = self.idempotency.get(oldest).is_none_or(|record| {
                now.duration_since(record.recorded_at)
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL)
            });
            if !expired {
                return;
            }
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }
}

/// Does `rule` satisfy every `ListAutomationRules` filter?
///
/// A filter matches when the rule's corresponding field equals any of the
/// filter's values; several filters are ANDed. The filter names are the model's
/// `AutomationRuleFilterName` value set, and an unrecognised one is reported by
/// [`unknown_filter_name`] rather than silently ignored.
pub fn rule_matches_filters(rule: &StoredRule, filters: &[Filter]) -> bool {
    filters.iter().all(|filter| {
        let field: Option<String> = match filter.name.as_str() {
            "Name" => Some(rule.name.clone()),
            "RecommendedActionType" => {
                // The rule's action types are a LIST, so the filter matches when
                // any requested value is among them.
                return rule
                    .recommended_action_types
                    .iter()
                    .any(|t| filter.values.iter().any(|v| v == t));
            }
            "Status" => Some(rule.status.clone()),
            "RuleType" => Some(rule.rule_type.clone()),
            "OrganizationConfigurationRuleApplyOrder" => rule
                .organization_configuration
                .as_ref()
                .and_then(|c| c.rule_apply_order.clone()),
            "AccountId" => Some(rule.account_id.clone()),
            // An unknown name never reaches here: the caller refuses it first.
            _ => None,
        };
        field.is_some_and(|value| filter.values.contains(&value))
    })
}

/// The first filter name that is not one of the model's
/// `AutomationRuleFilterName` values, if any.
pub fn unknown_filter_name(filters: &[Filter]) -> Option<&str> {
    filters
        .iter()
        .map(|f| f.name.as_str())
        .find(|name| !AUTOMATION_RULE_FILTER_NAMES.contains(name))
}

/// Does `action` satisfy every `ListRecommendedActions` filter?
///
/// A filter matches when the action's corresponding member equals any of the
/// filter's values; several filters are ANDed, exactly as for
/// [`rule_matches_filters`]. A member the stored action does not carry matches
/// nothing. The names are the model's `RecommendedActionFilterName` value set
/// ([`RECOMMENDED_ACTION_FILTER_NAME_VALUES`]), and an unrecognised one is
/// reported by [`unknown_recommended_action_filter_name`] rather than silently
/// ignored.
pub fn recommended_action_matches_filters(
    action: &RecommendedAction,
    filters: &[RecommendedActionFilter],
) -> bool {
    filters.iter().all(|filter| {
        let field: Option<String> = match filter.name.as_str() {
            "ResourceType" => action.resource_type.clone(),
            "RecommendedActionType" => action.recommended_action_type.clone(),
            "ResourceId" => action.resource_id.clone(),
            "AccountId" => action.account_id.clone(),
            "CurrentResourceDetailsEbsVolumeType" => {
                ebs_volume_type(action.current_resource_details.as_ref())
            }
            // An integer member: the filter carries its decimal rendering, so
            // the comparison is numeric and a value that is not a number
            // matches nothing rather than matching by spelling.
            "LookBackPeriodInDays" => {
                let Some(days) = action.look_back_period_in_days else {
                    return false;
                };
                return filter
                    .values
                    .iter()
                    .filter_map(|value| value.trim().parse::<i32>().ok())
                    .any(|wanted| wanted == days);
            }
            // A boolean member: `true`/`false`, matched without regard to case
            // so neither JSON's nor the console's spelling is rejected.
            "RestartNeeded" => {
                let Some(restart_needed) = action.restart_needed else {
                    return false;
                };
                return filter
                    .values
                    .iter()
                    .filter_map(|value| value.trim().to_ascii_lowercase().parse::<bool>().ok())
                    .any(|wanted| wanted == restart_needed);
            }
            // The resource's tags are a LIST, so the filter matches when any
            // requested key (or value) occurs among them.
            "ResourceTagsKey" => {
                return action.resource_tags.iter().flatten().any(|tag| {
                    filter.values.contains(&tag.key)
                });
            }
            "ResourceTagsValue" => {
                return action.resource_tags.iter().flatten().any(|tag| {
                    filter.values.contains(&tag.value)
                });
            }
            // An unknown name never reaches here: the caller refuses it first.
            _ => None,
        };
        field.is_some_and(|value| filter.values.contains(&value))
    })
}

/// The first filter name that is not one of the model's
/// `RecommendedActionFilterName` values, if any.
pub fn unknown_recommended_action_filter_name(filters: &[RecommendedActionFilter]) -> Option<&str> {
    filters
        .iter()
        .map(|f| f.name.as_str())
        .find(|name| !RECOMMENDED_ACTION_FILTER_NAME_VALUES.contains(name))
}

/// The refusal for a filter name outside the model's value set. The set is
/// enumerated from [`AUTOMATION_RULE_FILTER_NAMES`], so the message cannot
/// drift from the names the service actually accepts.
pub fn unknown_filter_name_message() -> String {
    constraint_report(
        "filters.member.name",
        &format!(
            "Member must satisfy enum value set: [{}]",
            AUTOMATION_RULE_FILTER_NAMES.join(", ")
        ),
    )
}

/// The `AutomationRuleFilterName` value set, from the Smithy model
/// (`smithy_query(query="shape",shape="AutomationRuleFilterName")`).
const AUTOMATION_RULE_FILTER_NAMES: &[&str] = &[
    "Name",
    "RecommendedActionType",
    "Status",
    "RuleType",
    "OrganizationConfigurationRuleApplyOrder",
    "AccountId",
];

/// Is `priority` the form the member documents?
///
/// "A string representation of a decimal number between 0 and 1 (having up to
/// 30 digits after the decimal point)"
/// (`smithy_query(query="doc",shape="CreateAutomationRuleRequest",member="priority")`).
/// The model types the member as a plain `String`, so this rule is the service's
/// own and is checked here rather than at the protocol boundary.
pub fn priority_is_valid(priority: &str) -> bool {
    let (integral, fractional) = match priority.split_once('.') {
        Some((integral, fractional)) => (integral, Some(fractional)),
        None => (priority, None),
    };
    if integral.is_empty() || !integral.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if let Some(fractional) = fractional {
        if fractional.is_empty()
            || fractional.len() > 30
            || !fractional.bytes().all(|b| b.is_ascii_digit())
        {
            return false;
        }
    }
    // Between 0 and 1: any `0.<digits>`, or 1 with no non-zero fraction.
    match integral.parse::<u64>() {
        Ok(0) => true,
        Ok(1) => fractional.is_none_or(|f| f.bytes().all(|b| b == b'0')),
        _ => false,
    }
}

/// Where a page of a paginated list operation resumed from: how far into the
/// filtered sequence the previous page stopped, and a fingerprint of the filters
/// it was taken with, so resuming with different filters is refused instead of
/// returning a page of a different result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub offset: usize,
    pub filter_fingerprint: String,
}

/// Encode a cursor as the opaque `nextToken` every paginated operation of this
/// service describes (`^[A-Za-z0-9+/=]+$`). Hex of `<offset>:<fingerprint>` satisfies that
/// pattern and carries the cursor itself, so a page token needs no server-side
/// entry and a read never writes.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    hex::encode(format!("{}:{}", cursor.offset, cursor.filter_fingerprint))
}

/// Decode a `nextToken` produced by [`encode_page_token`]. `None` when the token
/// was not produced by this service. What a decoded cursor may resume is the
/// caller's own check: it compares the cursor's fingerprint against the one it
/// would have issued for this request.
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (offset, fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        filter_fingerprint: fingerprint.to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState>)
}
