// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codestarnotifications-codestarnotifications-20191015 service.
//!
//! This is the notification-rule store every handler in `src/ops/` reads and writes:
//! the rules themselves, the per-region name index that makes rule names unique within
//! a region of the account, the account-wide target registry, the idempotency-token
//! ledger, and the small set of helpers (ARNs, ids, page tokens, request fingerprints)
//! the handlers share so no two operations keep their own copy of a resource.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::SystemTime;

/// Per-service state for com-amazonaws-codestarnotifications-codestarnotifications-20191015. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodestarnotificationsCodestarnotifications20191015State {
    /// The primary store, keyed by the rule's ARN.
    pub rules: HashMap<String, NotificationRule>,
    /// `Name` -> rule ARN, keyed per REGION: a notification rule is a regional
    /// resource, so the name it must be unique against is the one taken in the region
    /// the request was signed for. The index behind `ResourceAlreadyExistsException`;
    /// build its key with [`rule_name_key`].
    pub rule_names: HashMap<String, String>,
    /// `(operation, ClientRequestToken)` -> the request it first served, so a replay of
    /// the same token on the same operation with the same parameters answers with the
    /// original resource. Reach it through [`Self::idempotent_request`] /
    /// [`Self::remember_idempotent_request`], which build the key.
    pub idempotency: HashMap<String, IdempotentRequest>,
    /// The order tokens entered `idempotency`, so the ledger can be held to
    /// [`MAX_IDEMPOTENCY_TOKENS`] by forgetting the oldest.
    idempotency_order: VecDeque<String>,
    /// The account-wide target registry, keyed by `TargetAddress`. A target exists for
    /// the account (with its own status), independently of how many rules use it.
    pub targets: HashMap<String, TargetRegistryEntry>,
    /// Creation counter: the stable ordering `ListNotificationRules` pages over.
    pub next_sequence: u64,
}

/// A persisted notification rule.
#[derive(Debug, Clone)]
pub struct NotificationRule {
    pub id: String,
    pub arn: String,
    /// The region this rule lives in — the one its request was signed for, and the one
    /// its ARN carries. A regional endpoint serves only its own region's rules.
    pub region: String,
    pub name: String,
    pub resource: String,
    pub detail_type: String,
    pub status: String,
    pub event_type_ids: Vec<String>,
    pub targets: Vec<RuleTarget>,
    pub tags: BTreeMap<String, String>,
    pub created_by: String,
    pub created_timestamp: ::aws_smithy_types::DateTime,
    pub last_modified_timestamp: ::aws_smithy_types::DateTime,
    /// Position in the account-wide creation order; the key page tokens encode.
    pub sequence: u64,
}

/// A target subscribed to one rule. The target's STATUS is not stored here: it belongs
/// to the account-wide [`TargetRegistryEntry`], so a rule and `ListTargets` cannot
/// report two different statuses for one address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTarget {
    pub target_address: String,
    pub target_type: String,
}

/// One target of the account, and the rules subscribed to it. An entry exists exactly
/// while at least one rule subscribes to the address, which is what bounds the registry
/// at [`MAX_RULES_TOTAL`] x [`MAX_TARGETS_PER_RULE`] however many subscribe / delete
/// cycles a caller drives.
#[derive(Debug, Clone)]
pub struct TargetRegistryEntry {
    pub target_type: String,
    /// The target's `TargetStatus`. A subscription this service creates is reported
    /// [`TARGET_STATUS_ACTIVE`] from the moment it exists — see that constant.
    pub status: String,
    pub subscribed_rules: HashSet<String>,
}

/// What a `ClientRequestToken` already bought: the resource it produced and a
/// fingerprint of the parameters it produced it from.
#[derive(Debug, Clone)]
pub struct IdempotentRequest {
    pub arn: String,
    pub fingerprint: String,
}

/// The most notification rules this account holds in ONE region. The model declares
/// `LimitExceededException` ("Limits apply to accounts, notification rules,
/// notifications, resources, and targets") but states no number, so the figure is
/// CHOSEN: it exists to bound caller-driven growth of the store, not to reproduce an
/// AWS quota.
pub const MAX_RULES_PER_REGION: usize = 1_000;

/// The most notification rules the store holds across EVERY region at once.
///
/// [`MAX_RULES_PER_REGION`] is counted per REGION, and the region a rule lands in is
/// the one the request was SIGNED for — a value the caller chooses and this service
/// does not validate against a fixed list of regions, because a regional endpoint for
/// any region must keep working. On its own that figure therefore bounds nothing: a
/// client that varies its credential scope's region opens a fresh per-region budget
/// each time. This second figure is keyed on NOTHING the request controls, so it is the
/// one that actually bounds the store (and, with it, the target registry and the name
/// index); it is CHOSEN for the same reason as the per-region figure, at a multiple of
/// it so an ordinary multi-region caller never meets it first.
pub const MAX_RULES_TOTAL: usize = 4 * MAX_RULES_PER_REGION;

/// The most idempotency tokens the ledger remembers at once. Also CHOSEN, for the same
/// reason: a caller must not be able to grow the ledger without limit.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// The status a subscription this service creates reports.
///
/// A subscription created through `CreateNotificationRule` / `Subscribe` is already
/// usable when the call returns — the recorded AWS responses report `ACTIVE` on the
/// describe that immediately follows the create — so there is no transition for the
/// clock to owe and the status is terminal from the first read. The other
/// `TargetStatus` values (`PENDING`, `UNREACHABLE`, `INACTIVE`, `DEACTIVATED`) describe
/// a destination whose own confirmation state this emulator does not model, so nothing
/// here invents them.
pub const TARGET_STATUS_ACTIVE: &str = "ACTIVE";

/// `NotificationRuleStatus`'s documented default: "The status of the notification
/// rule. The default value is ENABLED."
pub const DEFAULT_RULE_STATUS: &str = "ENABLED";

impl ComAmazonawsCodestarnotificationsCodestarnotifications20191015State {
    /// Hand out the next position in the account-wide creation order.
    pub fn take_sequence(&mut self) -> u64 {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        seq
    }

    /// Subscribe `rule_arn` to `target`, creating the account-wide registry entry the
    /// first time this address is seen.
    pub fn register_target(&mut self, rule_arn: &str, target: &RuleTarget) {
        let entry = self
            .targets
            .entry(target.target_address.clone())
            .or_insert_with(|| TargetRegistryEntry {
                target_type: target.target_type.clone(),
                status: TARGET_STATUS_ACTIVE.to_string(),
                subscribed_rules: HashSet::new(),
            });
        entry.target_type = target.target_type.clone();
        entry.subscribed_rules.insert(rule_arn.to_string());
    }

    /// Drop `rule_arn`'s subscription to `address`, and the registry entry with it when
    /// no rule subscribes to that address any more.
    ///
    /// Dropping the empty entry is what keeps the registry bounded: without it, a
    /// caller could grow `targets` without limit by subscribing new addresses and
    /// deleting the rules again. `DeleteTarget`, when it is implemented, removes a
    /// target that rules DO still subscribe to (with `ForceUnsubscribeAll`).
    pub fn unregister_target(&mut self, rule_arn: &str, address: &str) {
        if let Some(entry) = self.targets.get_mut(address) {
            entry.subscribed_rules.remove(rule_arn);
            if entry.subscribed_rules.is_empty() {
                self.targets.remove(address);
            }
        }
    }

    /// The rule `arn` names, if this REGION holds it. A rule is addressed through a
    /// regional endpoint, so an ARN from another region is not found here.
    pub fn rule_in_region(&self, region: &str, arn: &str) -> Option<&NotificationRule> {
        self.rules.get(arn).filter(|rule| rule.region == region)
    }

    /// The same lookup for a handler that MUTATES the rule it found, so a write path
    /// resolves the ARN and the region once rather than looking the rule up twice.
    pub fn rule_in_region_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut NotificationRule> {
        self.rules.get_mut(arn).filter(|rule| rule.region == region)
    }

    /// How many rules this region already holds, for [`MAX_RULES_PER_REGION`].
    pub fn rule_count_in_region(&self, region: &str) -> usize {
        self.rules
            .values()
            .filter(|rule| rule.region == region)
            .count()
    }

    /// How many rules the store holds in TOTAL, for [`MAX_RULES_TOTAL`] — the bound
    /// that does not depend on a request-controlled key.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Remove the rule `arn` names from this region, with everything that only existed
    /// for it: its name index entry, its target subscriptions and the idempotency
    /// records that served it. `None` when this region does not hold it.
    pub fn remove_rule(&mut self, region: &str, arn: &str) -> Option<NotificationRule> {
        self.rule_in_region(region, arn)?;
        let removed = self.rules.remove(arn)?;
        self.rule_names
            .remove(&rule_name_key(&removed.region, &removed.name));
        for target in &removed.targets {
            let address = target.target_address.clone();
            self.unregister_target(&removed.arn, &address);
        }
        self.forget_idempotent_requests_for(&removed.arn);
        Some(removed)
    }

    /// What this `(operation, token)` pair already served, if the ledger still holds it.
    pub fn idempotent_request(&self, operation: &str, token: &str) -> Option<&IdempotentRequest> {
        self.idempotency.get(&idempotency_key(operation, token))
    }

    /// Record what this `(operation, token)` pair served, forgetting the oldest entries
    /// once the ledger is at [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// A forgotten token simply has no memory left: a replayed create then meets the
    /// name-uniqueness check, and a replayed subscribe upserts the same target again.
    pub fn remember_idempotent_request(
        &mut self,
        operation: &str,
        token: &str,
        record: IdempotentRequest,
    ) {
        let key = idempotency_key(operation, token);
        if self.idempotency.insert(key.clone(), record).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token that served `arn`: once the rule is gone, no token may report
    /// it as if it were still there.
    pub fn forget_idempotent_requests_for(&mut self, arn: &str) {
        let ledger = &mut self.idempotency;
        let order = &mut self.idempotency_order;
        ledger.retain(|_, record| record.arn != arn);
        order.retain(|key| ledger.contains_key(key));
    }

    /// The account-wide status of `address`: the registry's own value, and
    /// [`TARGET_STATUS_ACTIVE`] for an address the registry somehow does not hold (a
    /// rule always registers its targets, so this is the status every subscription this
    /// service creates carries rather than an invented one).
    pub fn target_status(&self, address: &str) -> String {
        self.targets
            .get(address)
            .map_or_else(|| TARGET_STATUS_ACTIVE.to_string(), |e| e.status.clone())
    }

    /// Every rule THIS REGION holds, in the stable creation order page tokens resume
    /// in. A listing is served by a regional endpoint, so it names only that region's
    /// rules.
    pub fn rules_in_region_order(&self, region: &str) -> Vec<&NotificationRule> {
        let mut ordered: Vec<&NotificationRule> = self
            .rules
            .values()
            .filter(|rule| rule.region == region)
            .collect();
        ordered.sort_by_key(|rule| rule.sequence);
        ordered
    }
}

/// The key [`ComAmazonawsCodestarnotificationsCodestarnotifications20191015State::rule_names`]
/// is indexed by: a rule name is unique within one region of the account, so the same
/// name may be taken once per region.
pub fn rule_name_key(region: &str, name: &str) -> String {
    // `\u{1f}` (unit separator) matches neither a region nor `NotificationRuleName`'s
    // `^[A-Za-z0-9\-_ ]+$`, so two different pairs cannot collide on one key.
    format!("{region}\u{1f}{name}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodestarnotificationsCodestarnotifications20191015State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodestarnotificationsCodestarnotifications20191015State>)
}

/// The region a request was signed for, defaulting to `us-east-1` for a request that
/// carries no SigV4 credential scope (an unsigned call still has to get an ARN).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// The principal a rule records as its `CreatedBy`: an IAM principal ARN built from the
/// access key id the request was signed with, so the value identifies the caller rather
/// than being a constant.
pub fn caller_principal_arn(parts: &::http::request::Parts) -> String {
    let akid = ::engine::extract_akid(&parts.headers);
    // IAM is a global service, so its ARNs carry an empty region segment.
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// The ARN of a notification rule: `arn:aws:codestar-notifications:<region>:<account>:notificationrule/<id>`,
/// the form `NotificationRuleArn`'s pattern requires.
pub fn notification_rule_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "codestar-notifications",
        region,
        &format!("notificationrule/{id}"),
    )
}

/// Mint a `NotificationRuleId`: a UUID-shaped, 36-character id (inside the modelled
/// 1..40) derived from the account-wide creation counter and the wall clock, so two
/// rules created from identical parameters still get distinct ids.
///
/// The id is OPAQUE. Nothing may be parsed back out of it — not the sequence, not the
/// time — and no handler may derive anything from one: it is only ever compared for
/// equality or used as the ARN's resource segment.
pub fn new_rule_id(sequence: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let digest = sha256_hex(&format!("notificationrule/{sequence}/{nanos}"));
    // Lay the digest out as a version-4 UUID: 8-4-4-4-12 hex digits with the version
    // nibble set to 4 and the variant nibble to 8, the shape AWS rule ids take.
    let d = digest.as_bytes();
    let hex = |range: std::ops::Range<usize>| String::from_utf8_lossy(&d[range]).to_string();
    format!(
        "{}-{}-4{}-8{}-{}",
        hex(0..8),
        hex(8..12),
        hex(13..16),
        hex(17..20),
        hex(20..32),
    )
}

/// A stable fingerprint of the parameters a request carried, used to tell a genuine
/// idempotent replay (same token, same parameters) from a token reused with a changed
/// parameter.
pub fn fingerprint(fields: &[String]) -> String {
    // `\u{1f}` (unit separator) cannot appear in any of the modelled patterns these
    // fields come from, so joining on it cannot make two different requests collide.
    sha256_hex(&fields.join("\u{1f}"))
}

fn sha256_hex(input: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Encode the position a page ended at into an opaque `NextToken`.
///
/// Hex keeps the token inside `NextToken`'s `^[\w/+=]+$` pattern, and the version
/// prefix is what makes a token from anywhere else fail to decode.
pub fn encode_page_token(next_sequence: u64) -> String {
    hex::encode(format!("v1:{next_sequence}"))
}

/// Decode a `NextToken` back into the position it marks. `None` for anything this
/// service did not mint — the caller gets `InvalidNextTokenException`.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let decoded = hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix("v1:")?.parse::<u64>().ok()
}

/// Now, as a modelled timestamp.
pub fn now_timestamp() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
}

/// The ledger key for an idempotency token: the OPERATION as well as the token, so one
/// token used on two operations cannot hand either of them the other's resource.
fn idempotency_key(operation: &str, token: &str) -> String {
    // `\u{1f}` (unit separator) cannot appear in `ClientRequestToken`'s `^[\w:/-]+$`.
    format!("{operation}\u{1f}{token}")
}

/// Refuse a value a modelled enum does not list, returning the reason each handler then
/// words in its own modelled error. `allowed` is the generated `*_VALUES` slice, which
/// is the enum's own member list.
pub fn enum_violation(member: &str, value: &str, allowed: &[&str]) -> Option<String> {
    (!allowed.contains(&value))
        .then(|| format!("{member} must be one of {allowed:?}, got {value:?}"))
}

/// The modelled maximum number of targets a notification rule carries
/// (`Targets`: `@length(min: 0, max: 10)`).
pub const MAX_TARGETS_PER_RULE: usize = 10;

/// Turn the wire `Target` list into the rule's stored targets: every target must carry
/// both of its members, and an address appears once (a repeated address is one
/// subscription, with the type last given for it).
///
/// `Err` carries the reason, which each handler words in its own modelled error.
pub fn rule_targets_from(
    targets: &[crate::ops::types::Target],
) -> Result<Vec<RuleTarget>, String> {
    let mut normalized: Vec<RuleTarget> = Vec::with_capacity(targets.len());
    for target in targets {
        let address = target
            .target_address
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Target.TargetAddress is required for every target".to_string())?;
        let target_type = target
            .target_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!("Target.TargetType is required for every target (address {address})")
            })?;
        match normalized
            .iter_mut()
            .find(|existing| existing.target_address == address)
        {
            Some(existing) => existing.target_type = target_type.to_string(),
            None => normalized.push(RuleTarget {
                target_address: address.to_string(),
                target_type: target_type.to_string(),
            }),
        }
    }
    Ok(normalized)
}

/// The tags a rule stores, refusing the key names the API documents as reserved.
///
/// `CreateNotificationRule$Tags`: "A list of tags to apply to this notification rule.
/// Key names cannot start with \"aws\"."
pub fn rule_tags_from(
    tags: Option<&HashMap<String, String>>,
) -> Result<BTreeMap<String, String>, String> {
    let mut stored = BTreeMap::new();
    for (key, value) in tags.into_iter().flatten() {
        if key.to_ascii_lowercase().starts_with("aws") {
            return Err(format!("Tags key names cannot start with \"aws\": {key}"));
        }
        stored.insert(key.clone(), value.clone());
    }
    Ok(stored)
}

/// One entry of the event-type catalog: an event type id, the service it belongs to,
/// the resource type it fires on and its display NAME.
///
/// Every member is catalog DATA, recorded as the service publishes it. In particular the
/// display name is not derivable from the id — AWS groups each event under a category
/// ("Pull request", "Branches and tags", "Build state") whose word boundary the id's
/// hyphenated slug does not mark — so it is stored here rather than computed.
#[derive(Debug, Clone, Copy)]
pub struct EventTypeCatalogEntry {
    pub event_type_id: &'static str,
    pub service_name: &'static str,
    pub resource_type: &'static str,
    pub event_type_name: &'static str,
}

/// The seeded event-type catalog `EventTypeIds` resolve through.
///
/// Source: the event types listed under "Notification concepts" in the Developer
/// Tools Console User Guide, the page `EventTypeSummary$EventTypeId`'s own
/// documentation links to
/// (<https://docs.aws.amazon.com/codestar-notifications/latest/userguide/concepts.html#concepts-api>),
/// whose "Event type" column is the `EventTypeName` the API reports and whose resource
/// types are the capitalised singular nouns (`Repository`, `Project`, `Application`,
/// `Pipeline`) `ListEventTypes` answers with. It is a subset of that list, which is why
/// an id the catalog does not hold is still accepted and stored (see
/// [`resolve_event_type`]).
pub const EVENT_TYPE_CATALOG: &[EventTypeCatalogEntry] = &[
    entry("codecommit-repository-comments-on-commits", "CodeCommit", "Repository", "Comments: On commits"),
    entry("codecommit-repository-comments-on-pull-requests", "CodeCommit", "Repository", "Comments: On pull requests"),
    entry("codecommit-repository-approvals-status-changed", "CodeCommit", "Repository", "Approvals: Status changed"),
    entry("codecommit-repository-approvals-rule-override", "CodeCommit", "Repository", "Approvals: Rule override"),
    entry("codecommit-repository-pull-request-created", "CodeCommit", "Repository", "Pull request: Created"),
    entry("codecommit-repository-pull-request-status-changed", "CodeCommit", "Repository", "Pull request: Status changed"),
    entry("codecommit-repository-pull-request-merged", "CodeCommit", "Repository", "Pull request: Merged"),
    entry("codecommit-repository-pull-request-source-updated", "CodeCommit", "Repository", "Pull request: Source updated"),
    entry("codecommit-repository-branches-and-tags-created", "CodeCommit", "Repository", "Branches and tags: Created"),
    entry("codecommit-repository-branches-and-tags-deleted", "CodeCommit", "Repository", "Branches and tags: Deleted"),
    entry("codecommit-repository-branches-and-tags-updated", "CodeCommit", "Repository", "Branches and tags: Updated"),
    entry("codebuild-project-build-state-failed", "CodeBuild", "Project", "Build state: Failed"),
    entry("codebuild-project-build-state-succeeded", "CodeBuild", "Project", "Build state: Succeeded"),
    entry("codebuild-project-build-state-in-progress", "CodeBuild", "Project", "Build state: In progress"),
    entry("codebuild-project-build-state-stopped", "CodeBuild", "Project", "Build state: Stopped"),
    entry("codebuild-project-build-phase-failure", "CodeBuild", "Project", "Build phase: Failure"),
    entry("codebuild-project-build-phase-success", "CodeBuild", "Project", "Build phase: Success"),
    entry("codedeploy-application-deployment-failed", "CodeDeploy", "Application", "Deployment: Failed"),
    entry("codedeploy-application-deployment-succeeded", "CodeDeploy", "Application", "Deployment: Succeeded"),
    entry("codedeploy-application-deployment-started", "CodeDeploy", "Application", "Deployment: Started"),
    entry("codepipeline-pipeline-pipeline-execution-failed", "CodePipeline", "Pipeline", "Pipeline execution: Failed"),
    entry("codepipeline-pipeline-pipeline-execution-canceled", "CodePipeline", "Pipeline", "Pipeline execution: Canceled"),
    entry("codepipeline-pipeline-pipeline-execution-started", "CodePipeline", "Pipeline", "Pipeline execution: Started"),
    entry("codepipeline-pipeline-pipeline-execution-resumed", "CodePipeline", "Pipeline", "Pipeline execution: Resumed"),
    entry("codepipeline-pipeline-pipeline-execution-succeeded", "CodePipeline", "Pipeline", "Pipeline execution: Succeeded"),
    entry("codepipeline-pipeline-pipeline-execution-superseded", "CodePipeline", "Pipeline", "Pipeline execution: Superseded"),
    entry("codepipeline-pipeline-manual-approval-failed", "CodePipeline", "Pipeline", "Manual approval: Failed"),
    entry("codepipeline-pipeline-manual-approval-needed", "CodePipeline", "Pipeline", "Manual approval: Needed"),
    entry("codepipeline-pipeline-manual-approval-succeeded", "CodePipeline", "Pipeline", "Manual approval: Succeeded"),
    entry("codepipeline-pipeline-action-execution-succeeded", "CodePipeline", "Pipeline", "Action execution: Succeeded"),
    entry("codepipeline-pipeline-action-execution-failed", "CodePipeline", "Pipeline", "Action execution: Failed"),
    entry("codepipeline-pipeline-action-execution-canceled", "CodePipeline", "Pipeline", "Action execution: Canceled"),
    entry("codepipeline-pipeline-action-execution-started", "CodePipeline", "Pipeline", "Action execution: Started"),
    entry("codepipeline-pipeline-stage-execution-started", "CodePipeline", "Pipeline", "Stage execution: Started"),
    entry("codepipeline-pipeline-stage-execution-succeeded", "CodePipeline", "Pipeline", "Stage execution: Succeeded"),
    entry("codepipeline-pipeline-stage-execution-resumed", "CodePipeline", "Pipeline", "Stage execution: Resumed"),
    entry("codepipeline-pipeline-stage-execution-canceled", "CodePipeline", "Pipeline", "Stage execution: Canceled"),
    entry("codepipeline-pipeline-stage-execution-failed", "CodePipeline", "Pipeline", "Stage execution: Failed"),
];

const fn entry(
    event_type_id: &'static str,
    service_name: &'static str,
    resource_type: &'static str,
    event_type_name: &'static str,
) -> EventTypeCatalogEntry {
    EventTypeCatalogEntry {
        event_type_id,
        service_name,
        resource_type,
        event_type_name,
    }
}

/// Resolve one stored `EventTypeId` into the summary the API reports.
///
/// For a catalogued id every member is reported from the catalog entry, verbatim: the
/// name and resource type are the service's own published values, never rendered from
/// the id. For an id the catalog does not hold, the id — the one fact the rule stores —
/// is returned and nothing else is invented.
pub fn resolve_event_type(event_type_id: &str) -> ResolvedEventType {
    let Some(found) = EVENT_TYPE_CATALOG
        .iter()
        .find(|entry| entry.event_type_id == event_type_id)
    else {
        return ResolvedEventType {
            event_type_id: event_type_id.to_string(),
            service_name: None,
            event_type_name: None,
            resource_type: None,
        };
    };
    ResolvedEventType {
        event_type_id: event_type_id.to_string(),
        service_name: Some(found.service_name.to_string()),
        event_type_name: Some(found.event_type_name.to_string()),
        resource_type: Some(found.resource_type.to_string()),
    }
}

/// A resolved event type, in the members `EventTypeSummary` declares.
#[derive(Debug, Clone)]
pub struct ResolvedEventType {
    pub event_type_id: String,
    pub service_name: Option<String>,
    pub event_type_name: Option<String>,
    pub resource_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_token_round_trips_and_a_foreign_one_does_not_decode() {
        assert_eq!(decode_page_token(&encode_page_token(7)), Some(7));
        assert_eq!(decode_page_token("not-a-token"), None);
        // Valid hex, but not a token this service minted.
        assert_eq!(decode_page_token(&hex::encode("v9:3")), None);
    }

    #[test]
    fn a_rule_id_fits_the_modelled_length() {
        let id = new_rule_id(1);
        assert!(
            (1..=40).contains(&id.len()),
            "NotificationRuleId is len 1..40, got {} for {id}",
            id.len()
        );
    }

    #[test]
    fn a_catalogued_event_type_reports_the_catalogs_own_values() {
        let resolved = resolve_event_type("codecommit-repository-comments-on-commits");
        assert_eq!(
            resolved.event_type_name.as_deref(),
            Some("Comments: On commits"),
            "got {resolved:?}"
        );
        assert_eq!(resolved.resource_type.as_deref(), Some("Repository"));
        assert_eq!(resolved.service_name.as_deref(), Some("CodeCommit"));

        // An id the catalog does not hold reports nothing but the id.
        let unknown = resolve_event_type("codecommit-repository-not-in-the-catalog");
        assert!(
            unknown.event_type_name.is_none()
                && unknown.resource_type.is_none()
                && unknown.service_name.is_none(),
            "an uncatalogued id resolves nothing but its id, got {unknown:?}"
        );
    }

    #[test]
    fn the_idempotency_ledger_is_bounded_and_keeps_the_newest_tokens() {
        let mut state = ComAmazonawsCodestarnotificationsCodestarnotifications20191015State::default();
        let total = MAX_IDEMPOTENCY_TOKENS + 25;
        for index in 0..total {
            state.remember_idempotent_request(
                "CreateNotificationRule",
                &format!("token-{index}"),
                IdempotentRequest {
                    arn: format!("arn-{index}"),
                    fingerprint: String::new(),
                },
            );
        }
        assert!(
            state.idempotency.len() <= MAX_IDEMPOTENCY_TOKENS,
            "case {total} tokens: the ledger must stay within {MAX_IDEMPOTENCY_TOKENS}, got {}",
            state.idempotency.len()
        );
        assert!(
            state
                .idempotent_request("CreateNotificationRule", &format!("token-{}", total - 1))
                .is_some(),
            "case {total} tokens: the newest token must still be remembered"
        );
        assert!(
            state
                .idempotent_request("CreateNotificationRule", "token-0")
                .is_none(),
            "case {total} tokens: the oldest token must have been forgotten"
        );
    }

    #[test]
    fn one_token_on_two_operations_keeps_two_records() {
        let mut state = ComAmazonawsCodestarnotificationsCodestarnotifications20191015State::default();
        for operation in ["CreateNotificationRule", "Subscribe"] {
            state.remember_idempotent_request(
                operation,
                "shared-token",
                IdempotentRequest {
                    arn: format!("arn-for-{operation}"),
                    fingerprint: operation.to_string(),
                },
            );
        }
        for operation in ["CreateNotificationRule", "Subscribe"] {
            let record = state
                .idempotent_request(operation, "shared-token")
                .expect("each operation keeps its own record");
            assert_eq!(
                record.arn,
                format!("arn-for-{operation}"),
                "case {operation:?}: one token on two operations must not alias, got {record:?}"
            );
        }
    }

    #[test]
    fn a_target_entry_is_released_when_its_last_rule_unsubscribes() {
        let mut state = ComAmazonawsCodestarnotificationsCodestarnotifications20191015State::default();
        let target = RuleTarget {
            target_address: "arn:aws:sns:us-east-1:123456789012:topic".to_string(),
            target_type: "SNS".to_string(),
        };
        state.register_target("rule-a", &target);
        state.register_target("rule-b", &target);

        state.unregister_target("rule-a", &target.target_address);
        assert!(
            state.targets.contains_key(&target.target_address),
            "a target another rule still subscribes to stays registered, got {:?}",
            state.targets
        );
        state.unregister_target("rule-b", &target.target_address);
        assert!(
            state.targets.is_empty(),
            "the last unsubscribe releases the entry, got {:?}",
            state.targets
        );
    }
}
