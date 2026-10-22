// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rbin-amazonrecyclebin service.
//!
//! Recycle Bin's control plane is a flat collection of retention rules addressed
//! by an 11-character identifier, so the store is one ordered map of rules plus an
//! ARN index. The ordering is load-bearing: `ListRules` pages through the map in
//! key order, and its `NextToken` is the last identifier it returned, so a page
//! boundary stays stable while rules are added or removed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{LockConfiguration, ResourceTag, RetentionPeriod, RuleSummary, Tag};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `RuleIdentifier` is `^[0-9a-zA-Z]{11}$` — exactly 11 characters from the base62
/// alphabet, per the Smithy model.
pub const IDENTIFIER_LEN: usize = 11;

/// The alphabet `^[0-9a-zA-Z]{11}$` allows, in the digit order used to render a
/// minted identifier. 62^11 exceeds `u64::MAX`, so every seed renders in 11 digits.
const BASE62_DIGITS: &[u8; 62] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// `ListRules` page size when the caller names none. The model bounds `MaxResults`
/// at 1000, so an unbounded page can never be larger than this.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The number of retention rules this Region holds, after which `CreateRule` is
/// refused with the modelled `ServiceQuotaExceededException`.
///
/// Recycle Bin's published Service Quotas entry "Retention rules per Region" is 1000
/// (Amazon EBS User Guide, "Recycle Bin quotas"), so the store is bounded by the same
/// number the real service bounds an account's Region by, rather than by an invented
/// cap. It also keeps the store from growing for the process's whole lifetime on
/// nothing but repeated `CreateRule` calls.
pub const MAX_RULES: usize = 1000;

/// The number of tags one retention rule may carry, after which assigning more is
/// refused with the modelled `ServiceQuotaExceededException` ("The request would
/// cause a service quota for the number of tags per resource to be exceeded").
///
/// AWS General Reference, "Tag naming limits and requirements": "Maximum number of
/// tags per resource — 50". Recycle Bin publishes no higher figure of its own, and the
/// model's `TagList` bound of 200 limits one REQUEST, not the rule's total.
///
/// BOTH operations that can put a rule over it are held to it — `CreateRule`, which
/// models `ServiceQuotaExceededException` too, and `TagResource` — so no sequence of
/// calls can leave a rule carrying more tags than the quota allows.
pub const MAX_TAGS_PER_RULE: usize = 50;

/// Assign `tags` to the tag list `held`, keyed by tag key.
///
/// A key already present takes the new value IN PLACE, and a key that is not present is
/// appended, so a rule holds at most one tag per key and the order it reports them in
/// is assignment order. A request naming the same key twice therefore ends with the
/// last value it gave for that key. Shared by `CreateRule` and `TagResource`, which
/// assign a rule's tags the same way.
pub fn upsert_tags(held: &mut Vec<Tag>, tags: Vec<Tag>) {
    for tag in tags {
        match held.iter_mut().find(|present| present.key == tag.key) {
            Some(present) => present.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// Seconds in one day. `UnlockDelayUnit` models exactly one unit — "The unit of
/// time in which to measure the unlock delay. Currently, the unlock delay can be
/// measured only in days" — so an unlock delay is this many seconds per unit.
pub const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The lock states `LockState` models, plus the absent state a never-locked rule
/// reports. Kept here so every lock-aware operation names one spelling.
pub const LOCK_STATE_LOCKED: &str = "locked";
pub const LOCK_STATE_PENDING_UNLOCK: &str = "pending_unlock";
pub const LOCK_STATE_UNLOCKED: &str = "unlocked";

/// One persisted Recycle Bin retention rule — the single record every operation of
/// this service reads its response out of.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The rule's unique id, `^[0-9a-zA-Z]{11}$`, and this map's key.
    pub identifier: String,
    /// `arn:aws:rbin:{region}:{account}:rule/{identifier}`.
    pub rule_arn: String,
    pub description: Option<String>,
    /// `EBS_SNAPSHOT` | `EBS_VOLUME` | `EC2_IMAGE`. Fixed at creation: the model
    /// says a rule's resource type cannot be updated.
    pub resource_type: String,
    pub retention_period: RetentionPeriod,
    /// Tag-level rules: the resource tags that select what the rule retains.
    pub resource_tags: Vec<ResourceTag>,
    /// Region-level rules: the tags that exclude a resource from the rule.
    pub exclude_resource_tags: Vec<ResourceTag>,
    /// The rule's own AWS tags (`TagResource`/`ListTagsForResource`).
    pub tags: Vec<Tag>,
    /// `available` | `pending`.
    pub status: String,
    pub lock_configuration: Option<LockConfiguration>,
    /// `locked` | `pending_unlock` | `unlocked`; `None` means never locked, a state
    /// a rule can never return to once locked.
    pub lock_state: Option<String>,
    /// When a `pending_unlock` rule's unlock delay expires.
    pub lock_end_time: Option<SystemTime>,
    pub created_at: SystemTime,
}

impl Rule {
    /// The `RuleSummary` projection `ListRules` reports: identifier, description,
    /// retention period, lock state and ARN.
    pub fn to_summary(&self) -> RuleSummary {
        RuleSummary {
            identifier: Some(self.identifier.clone()),
            description: self.description.clone(),
            retention_period: Some(self.retention_period.clone()),
            lock_state: self.lock_state.clone(),
            rule_arn: Some(self.rule_arn.clone()),
        }
    }

    /// Whether the rule's lock still forbids modifying or deleting it.
    ///
    /// Per the model: `locked` cannot be modified or deleted; `pending_unlock` has
    /// been unlocked but is still inside the unlock delay, so it can be modified or
    /// deleted "only after the unlock delay period has expired" — which the clock,
    /// not a caller, makes due.
    pub fn is_lock_protected(&self, now: SystemTime) -> bool {
        match self.lock_state.as_deref() {
            Some(LOCK_STATE_LOCKED) => true,
            Some(LOCK_STATE_PENDING_UNLOCK) => self.lock_end_time.is_none_or(|end| now < end),
            _ => false,
        }
    }

    /// Assign `tags` to the rule, as [`upsert_tags`] defines assigning.
    pub fn apply_tags(&mut self, tags: Vec<Tag>) {
        upsert_tags(&mut self.tags, tags);
    }

    /// How many tags the rule would carry once `tags` were assigned — the size of the
    /// union of the keys it holds and the keys `tags` names, since assignment replaces
    /// a key's value rather than adding a second tag for it.
    pub fn tag_count_after(&self, tags: &[Tag]) -> usize {
        let mut keys: BTreeSet<&str> = self.tags.iter().map(|tag| tag.key.as_str()).collect();
        keys.extend(tags.iter().map(|tag| tag.key.as_str()));
        keys.len()
    }

    /// Unassign every tag whose key appears in `keys`. A key the rule does not carry is
    /// ignored, so unassigning is idempotent.
    pub fn remove_tags(&mut self, keys: &[String]) {
        self.tags.retain(|tag| !keys.contains(&tag.key));
    }

    /// Materialise the lock transition the CLOCK has already made due: a
    /// `pending_unlock` rule whose unlock delay has expired is `unlocked`, and it
    /// no longer reports a lock end time ("Only returned for retention rules that
    /// have been unlocked and that are still within the unlock delay period").
    ///
    /// Nothing else about a lock moves on its own, so a rule in any other state is
    /// left exactly as it is.
    pub fn settle_lock_state(&mut self, now: SystemTime) {
        if self.lock_state.as_deref() == Some(LOCK_STATE_PENDING_UNLOCK)
            && self.lock_end_time.is_some_and(|end| now >= end)
        {
            self.lock_state = Some(LOCK_STATE_UNLOCKED.to_string());
            self.lock_end_time = None;
        }
    }

    /// Whether every tag in `filter` appears among the rule's resource tags. An
    /// empty filter selects every rule.
    pub fn has_resource_tags(&self, filter: &[ResourceTag]) -> bool {
        filter
            .iter()
            .all(|wanted| contains_resource_tag(&self.resource_tags, wanted))
    }

    /// Whether every tag in `filter` appears among the rule's exclusion tags. An
    /// empty filter selects every rule.
    pub fn has_exclude_resource_tags(&self, filter: &[ResourceTag]) -> bool {
        filter
            .iter()
            .all(|wanted| contains_resource_tag(&self.exclude_resource_tags, wanted))
    }
}

/// The instant an unlock requested at `now` leaves its unlock delay.
///
/// The delay is the one the rule's lock configuration carries, so it keeps the value
/// the lock request asked for: `UnlockDelayValue` units of `UnlockDelayUnit`, which
/// the model bounds at 7..=30 and measures only in `DAYS`. A rule with no lock
/// configuration has no delay to serve.
pub fn unlock_delay_end(
    now: SystemTime,
    lock_configuration: Option<&LockConfiguration>,
) -> Option<SystemTime> {
    let delay = &lock_configuration?.unlock_delay;
    // The model bounds the value at 7..=30, so a value outside `u64` cannot reach a
    // handler; treat one as no delay rather than panicking if it ever does.
    let value = u64::try_from(delay.unlock_delay_value).unwrap_or(0);
    let seconds = match delay.unlock_delay_unit.as_str() {
        "DAYS" => value.saturating_mul(SECONDS_PER_DAY),
        // `UnlockDelayUnit` models only `DAYS`, and the boundary validator refuses an
        // unmodelled value before a handler runs, so this arm serves no delay rather
        // than guessing a scale for a unit the model does not define.
        _ => return None,
    };
    now.checked_add(Duration::from_secs(seconds))
}

/// A tag matches when the keys are equal and, where the filter names a value, the
/// values are equal too; a filter that names no value matches any value for the key.
fn contains_resource_tag(tags: &[ResourceTag], wanted: &ResourceTag) -> bool {
    tags.iter().any(|tag| {
        tag.resource_tag_key == wanted.resource_tag_key
            && wanted
                .resource_tag_value
                .as_ref()
                .is_none_or(|value| tag.resource_tag_value.as_ref() == Some(value))
    })
}

/// Per-service state for com-amazonaws-rbin-amazonrecyclebin. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRbinAmazonrecyclebinState {
    /// Every retention rule, keyed by `Identifier` and ordered so paging is stable.
    pub rules: BTreeMap<String, Rule>,
    /// `RuleArn` -> `Identifier`, so the ARN-addressed tag operations resolve a rule
    /// without scanning the map.
    pub arn_index: HashMap<String, String>,
    /// Monotonic counter behind [`Self::mint_identifier`].
    pub next_identifier_seed: u64,
}

impl ComAmazonawsRbinAmazonrecyclebinState {
    /// Mint an identifier no stored rule holds, as 11 base62 digits.
    ///
    /// Monotonic, so the map's key order is also creation order; the loop skips a
    /// candidate that is somehow already taken rather than overwriting a rule.
    pub fn mint_identifier(&mut self) -> String {
        loop {
            self.next_identifier_seed = self.next_identifier_seed.wrapping_add(1);
            let candidate = base62_identifier(self.next_identifier_seed);
            if !self.rules.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Insert a rule and index it by ARN.
    pub fn insert_rule(&mut self, rule: Rule) {
        self.arn_index
            .insert(rule.rule_arn.clone(), rule.identifier.clone());
        self.rules.insert(rule.identifier.clone(), rule);
    }

    /// Remove a rule and drop its ARN index entry. `None` when no such rule exists.
    pub fn remove_rule(&mut self, identifier: &str) -> Option<Rule> {
        let removed = self.rules.remove(identifier)?;
        self.arn_index.remove(&removed.rule_arn);
        Some(removed)
    }

    /// The rule an ARN addresses, resolved through [`Self::arn_index`].
    pub fn rule_by_arn(&self, rule_arn: &str) -> Option<&Rule> {
        let identifier = self.arn_index.get(rule_arn)?;
        self.rules.get(identifier)
    }

    /// The rule an ARN addresses, for modification — the write-side twin of
    /// [`Self::rule_by_arn`], used by the ARN-addressed tag operations.
    pub fn rule_by_arn_mut(&mut self, rule_arn: &str) -> Option<&mut Rule> {
        let identifier = self.arn_index.get(rule_arn)?.clone();
        self.rules.get_mut(&identifier)
    }

    /// The rules to walk for a page, starting after `after` when a `NextToken`
    /// named one.
    pub fn page_from<'a>(
        &'a self,
        after: Option<&str>,
    ) -> impl Iterator<Item = (&'a String, &'a Rule)> + 'a {
        let lower = match after {
            Some(identifier) => Bound::Excluded(identifier.to_string()),
            None => Bound::Unbounded,
        };
        self.rules.range((lower, Bound::Unbounded))
    }
}

/// Render `seed` as exactly [`IDENTIFIER_LEN`] base62 digits, most significant
/// first and left-padded with `0`, so the result satisfies `^[0-9a-zA-Z]{11}$` and
/// sorts in the same order as the seed.
fn base62_identifier(seed: u64) -> String {
    // Built as `char`s so the result needs no UTF-8 validation step — and so a future
    // change to the alphabet cannot turn into replacement characters.
    let mut digits = ['0'; IDENTIFIER_LEN];
    let mut remaining = seed;
    let mut index = IDENTIFIER_LEN;
    while remaining > 0 && index > 0 {
        index -= 1;
        digits[index] = BASE62_DIGITS[(remaining % 62) as usize] as char;
        remaining /= 62;
    }
    digits.iter().collect()
}

/// Whether `candidate` is shaped like a `RuleIdentifier` (`^[0-9a-zA-Z]{11}$`).
pub fn is_rule_identifier(candidate: &str) -> bool {
    candidate.len() == IDENTIFIER_LEN && candidate.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The region this request is for: the SigV4 credential scope's region, which is
/// what the SDK signs an endpoint's region into, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode a page cursor: the base64 of the last identifier the page returned.
///
/// Base64 is what makes the token opaque to a client, and its alphabet is exactly
/// the `NextToken` pattern `^[A-Za-z0-9+/=]{1,2048}$`.
pub fn encode_page_token(identifier: &str) -> String {
    ::aws_smithy_types::base64::encode(identifier)
}

/// Decode a page cursor back to the identifier the previous page stopped at.
///
/// `None` for any token this service did not emit — it is base64 of something that
/// is not a rule identifier — so the caller can refuse it as an invalid page token
/// rather than silently returning the first page.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    let identifier = String::from_utf8(bytes).ok()?;
    is_rule_identifier(&identifier).then_some(identifier)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRbinAmazonrecyclebinState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRbinAmazonrecyclebinState>)
}
