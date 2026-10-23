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
    AutomationEvent, AutomationEventFilter, AutomationRule, Criteria, DoubleCriteriaCondition,
    EbsVolumeConfiguration, EstimatedMonthlySavings, Filter, IntegerCriteriaCondition,
    OptInRequiredException, OrganizationConfiguration, OrganizationScope, PreviewResult,
    RecommendedAction, RecommendedActionFilter, ResourceDetails, ResourceTagsCriteriaCondition,
    Schedule, StringCriteriaCondition, Tag, AUTOMATION_EVENT_FILTER_NAME_VALUES,
    RECOMMENDED_ACTION_FILTER_NAME_VALUES,
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
pub fn validation_report(violations: &[Violation]) -> String {
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
        "AfterDiscount" => savings.after_discount_savings,
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
/// its terminal status due [`EVENT_SETTLEMENT_DELAY`] later.
pub fn event_for_action(
    event_id: String,
    action: &RecommendedAction,
    account_id: String,
    started_at: SystemTime,
) -> StoredEvent {
    StoredEvent {
        event_id,
        event_description: None,
        // `EventType` and `RecommendedActionType` carry the same value set, and
        // the event automates exactly this action
        // (`smithy_query(query="shape",shape="EventType,RecommendedActionType")`).
        event_type: action.recommended_action_type.clone().unwrap_or_default(),
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
        created_timestamp: ::aws_smithy_types::DateTime::from(started_at),
        completed_timestamp: None,
        estimated_monthly_savings: action.estimated_monthly_savings.clone(),
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
