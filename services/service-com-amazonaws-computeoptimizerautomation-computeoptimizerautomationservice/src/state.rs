// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice service.
//!
//! This module owns everything the handlers share: the stored form of an
//! automation rule, the enrollment configuration, the client-token (idempotency)
//! ledger, and the pure helpers that mint a rule id, build an opaque page token
//! and decide whether a rule matches a `ListAutomationRules` filter. Handlers
//! live in `src/ops/`, one file per operation, and build their own output shapes
//! from what they read back out of here.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AutomationRule, Criteria, Filter, OrganizationConfiguration, Schedule, Tag,
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

/// The page size `ListAutomationRules` uses when the caller sends no
/// `maxResults`. The model bounds `maxResults` at `range(min: 1, max: 100)`, so
/// the largest page a caller may ask for is also the default.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The number of `ruleId` characters, from the `RuleId` pattern
/// `^[0-9A-Za-z]{16}$`.
const RULE_ID_LEN: usize = 16;

/// How many automation rules one account may hold before `CreateAutomationRule`
/// answers the modelled `ServiceQuotaExceededException` ("The request would
/// exceed service quotas", HTTP 402).
///
/// CHOSEN bound: the service's real per-account rule quota is in neither the
/// model nor the operation docs, and this store lives in memory, so the count is
/// capped rather than left to grow with every request. It is the same order as
/// the largest page the model's `maxResults` allows.
pub const MAX_RULES_PER_ACCOUNT: usize = 100;

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
    pub description: Option<String>,
    pub rule_type: String,
    pub rule_revision: i64,
    pub organization_configuration: Option<OrganizationConfiguration>,
    pub priority: Option<String>,
    pub recommended_action_types: Vec<String>,
    pub criteria: Option<Criteria>,
    pub schedule: Schedule,
    pub status: String,
    pub tags: Vec<Tag>,
    pub created_timestamp: ::aws_smithy_types::DateTime,
    pub last_updated_timestamp: Option<::aws_smithy_types::DateTime>,
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
            description: self.description.clone(),
            rule_type: Some(self.rule_type.clone()),
            rule_revision: Some(self.rule_revision),
            account_id: Some(self.account_id.clone()),
            organization_configuration: self.organization_configuration.clone(),
            priority: self.priority.clone(),
            recommended_action_types: Some(self.recommended_action_types.clone()),
            schedule: Some(self.schedule.clone()),
            status: Some(self.status.clone()),
            created_timestamp: Some(self.created_timestamp),
            last_updated_timestamp: self.last_updated_timestamp,
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

/// One entry of the client-token ledger. It records WHICH resource the call
/// acted on rather than a serialized body, so a replay is recomputed from the
/// store and can never report a resource that is no longer there.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub operation: &'static str,
    pub request_fingerprint: String,
    pub rule_arn: Option<String>,
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
    /// `clientToken` -> what the call that first used it did. Bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`] and [`IDEMPOTENCY_TTL`].
    pub idempotency: HashMap<String, IdempotencyRecord>,
    /// The tokens in `idempotency`, oldest first, so the ledger can evict.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter mixed into a minted `ruleId`, so two rules created in
    /// the same nanosecond still get different ids.
    rule_id_sequence: u64,
}

impl ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState {
    /// The enrollment configuration of `account_id`, or the unenrolled default
    /// when that account has never configured one. A READ: it stores nothing.
    pub fn enrollment(&self, account_id: &str) -> EnrollmentConfig {
        self.enrollments.get(account_id).cloned().unwrap_or_default()
    }

    /// A fresh `ruleId` matching the model's `^[0-9A-Za-z]{16}$`: the leading
    /// hex of a digest over this store's counter and the current clock, retried
    /// in the (astronomically unlikely) event of a collision with a live rule.
    pub fn mint_rule_id(&mut self) -> String {
        loop {
            self.rule_id_sequence += 1;
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let digest = Sha256::digest(format!("{}:{nanos}", self.rule_id_sequence).as_bytes());
            let candidate: String = hex::encode(digest).chars().take(RULE_ID_LEN).collect();
            if !self.rules.contains_rule_id(&candidate) {
                return candidate;
            }
        }
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
        rule_arn: Option<String>,
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
                    rule_arn,
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

/// Where a page of `ListAutomationRules` resumed from: how far into the filtered
/// sequence the previous page stopped, and a fingerprint of the filters it was
/// taken with, so resuming with different filters is refused instead of
/// returning a page of a different result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub offset: usize,
    pub filter_fingerprint: String,
}

/// Encode a cursor as the opaque `nextToken` the model describes
/// (`^[A-Za-z0-9+/=]+$`). Hex of `<offset>:<fingerprint>` satisfies that
/// pattern and carries the cursor itself, so a page token needs no server-side
/// entry and a read never writes.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    hex::encode(format!("{}:{}", cursor.offset, cursor.filter_fingerprint))
}

/// Decode a `nextToken` produced by [`encode_page_token`]. `None` when the token
/// was not produced by this service.
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
