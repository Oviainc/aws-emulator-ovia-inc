// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rbin-amazonrecyclebin service.
//!
//! Recycle Bin's control plane is a collection of retention rules addressed by an
//! 11-character identifier, so the store is one ordered map of rules plus an ARN
//! index. The ordering is load-bearing: `ListRules` pages through the map in key
//! order, and its `NextToken` is a cursor naming the last identifier it returned, so a
//! page boundary stays stable while rules are added or removed. A cursor is honoured
//! only by the endpoint that issued it and only for a position it really handed out
//! ([`ComAmazonawsRbinAmazonrecyclebinState::page_cursor`]).
//!
//! A retention rule is a REGIONAL resource — it exists only in the Region it was
//! created in — so every rule carries that Region ([`Rule::region`]) and every
//! resolver, pager and quota counter below takes the Region the request was signed
//! for ([`request_region`]) and sees that Region's rules alone. One endpoint
//! therefore neither reports nor resolves nor counts a rule another Region holds.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};

use crate::ops::types::{
    ConflictException, LockConfiguration, ResourceNotFoundException, ResourceTag, RetentionPeriod,
    RuleSummary, ServiceQuotaExceededException, Tag, UnlockDelay, ValidationException,
};

/// The region an ARN carries when the request names none — it is unsigned, so no SigV4
/// credential scope states a region, or the scope's region is no Region code at all
/// ([`is_region_code`]). Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The longest Region an ARN this service reports can carry: the Region segment of the
/// modelled `RuleArn` is `[a-z\-0-9]{0,63}`
/// (smithy_query(query="shape", shape="RuleArn")).
pub const REGION_CODE_MAX_LEN: usize = 63;

/// Whether `region` is shaped like an Amazon Web Services Region code (`us-east-1`,
/// `ap-southeast-4`, `us-gov-east-1`).
///
/// The shape is the one the Region segment of the modelled `RuleArn` admits — lowercase
/// ASCII letters, digits and hyphens, non-empty and at most [`REGION_CODE_MAX_LEN`] of
/// them — so a Region this answers for is a Region every ARN member of this service can
/// hold, and one it rejects is a value no `RuleArn` could carry.
pub fn is_region_code(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= REGION_CODE_MAX_LEN
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// `RuleIdentifier` is `^[0-9a-zA-Z]{11}$` — exactly 11 characters from the base62
/// alphabet, per the Smithy model.
pub const IDENTIFIER_LEN: usize = 11;

/// The alphabet `^[0-9a-zA-Z]{11}$` allows, in the digit order used to render a
/// minted identifier. 62^11 exceeds `u64::MAX`, so every seed renders in 11 digits.
///
/// The digits are listed in ASCENDING BYTE order — `'0'..='9'` (0x30..=0x39), then
/// `'A'..='Z'` (0x41..=0x5a), then `'a'..='z'` (0x61..=0x7a) — so a higher digit value
/// always renders a higher byte. That is what makes [`base62_identifier`] order
/// preserving: [`ComAmazonawsRbinAmazonrecyclebinState::rules`] compares its keys by
/// bytes, so any alphabet that interleaved the two letter ranges would render a higher
/// seed as a key that sorts BEFORE a lower one.
const BASE62_DIGITS: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `ListRules` page size when the caller names none. The model bounds `MaxResults`
/// at 1000, so an unbounded page can never be larger than this.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The number of retention rules this Region holds, after which `CreateRule` is
/// refused with the modelled `ServiceQuotaExceededException`.
///
/// Recycle Bin's published Service Quotas entry "Retention rules per Region" is 1000
/// (Amazon EBS User Guide, "Recycle Bin quotas"), so a Region of the store is bounded by
/// the same number the real service bounds an account's Region by, rather than by an
/// invented cap.
pub const MAX_RULES: usize = 1000;

/// The most retention rules this emulator stores, across every Region.
///
/// Not an AWS quota: [`MAX_RULES`] is Recycle Bin's own per-Region one, and the Region of
/// a request is read off the caller's SigV4 credential scope ([`request_region`]), so a
/// per-Region bound is a bound the CALLER can partition by signing each request for
/// another Region string. This cap is keyed on nothing in the request, so it bounds
/// [`ComAmazonawsRbinAmazonrecyclebinState::rules`] — and with it every per-request pass
/// over those records ([`ComAmazonawsRbinAmazonrecyclebinState::rule_count_in_region`],
/// [`ComAmazonawsRbinAmazonrecyclebinState::exceeds_resource_tag_quota`] and
/// [`ComAmazonawsRbinAmazonrecyclebinState::page_from`]) — however the request varies,
/// and keeps the store from growing for the process's whole lifetime on nothing but
/// repeated `CreateRule` calls. `CreateRule` checks it before minting anything and
/// answers the modelled quota error past it, so no sequence of calls can leave the store
/// larger than this.
pub const RULE_STORE_LIMIT: usize = 2 * MAX_RULES;

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

/// The number of retention rules one resource tag key and value pair may select, after
/// which selecting on it again is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// The model states the bound on the member that carries the pairs: `ResourceTags` —
/// "You can add the same tag key and value pair to a maximum or five retention rules"
/// (smithy_query(query="doc", shape="CreateRuleRequest"/"UpdateRuleRequest",
/// member="ResourceTags")).
///
/// BOTH operations that can put a pair over it are held to it — `CreateRule` and
/// `UpdateRule`, the two that accept `ResourceTags` — so no sequence of calls can leave
/// one pair selecting more rules than the quota allows.
pub const MAX_RULES_PER_RESOURCE_TAG: usize = 5;

/// `ServiceQuotaExceededExceptionReason`'s sole value.
/// Source: smithy_query(query="shape", shape="ServiceQuotaExceededExceptionReason").
pub const SERVICE_QUOTA_REASON_EXCEEDED: &str = "SERVICE_QUOTA_EXCEEDED";

/// A resource tag as the five-rules-per-pair quota identifies it: the key together with
/// the value, where a tag that carries no value is the empty value (`ResourceTagValue`
/// is optional and bounded at `{0,256}`, so "no value" and "the empty value" select the
/// same resources).
fn resource_tag_pair(tag: &ResourceTag) -> (&str, &str) {
    (
        tag.resource_tag_key.as_str(),
        tag.resource_tag_value.as_deref().unwrap_or_default(),
    )
}

/// The modelled 402 EVERY quota refusal of this service answers with: the reason token
/// the model declares for the error, carried as the `Message` as well as the `Reason`.
///
/// Recycle Bin states a quota refusal by the token alone — the body names the fault
/// class and nothing else, so no part of the request (a count, a key, a Region) and no
/// bound of the service is rendered into it. Every bound below therefore reaches the
/// wire through this one body, which is also why a caller classifies a quota refusal by
/// the error code and reason rather than by prose.
pub fn quota_exceeded() -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(SERVICE_QUOTA_REASON_EXCEEDED.to_string()),
        reason: Some(SERVICE_QUOTA_REASON_EXCEEDED.to_string()),
    }
}

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

/// The tag prefix AWS reserves for its own tags: a caller may not create, change or
/// remove a tag whose KEY or whose VALUE claims it, and a tagging API refuses such a
/// request with its parameter-fault error.
///
/// Source: AWS General Reference, "Tag naming limits and requirements" — the `aws:`
/// prefix is reserved for AWS use, and tag keys and values carrying it can be neither
/// created nor edited nor deleted through a tagging API.
///
/// The model cannot express the rule — `TagKey`'s and `TagValue`'s patterns both admit a
/// colon anywhere — so the boundary validator passes such a key or value through and
/// every door a caller names a tag of its OWN through is held to it here:
/// `CreateRule$Tags` and `TagResource$Tags`, which carry both halves of a tag, and
/// `UntagResource$TagKeys`, which carries keys alone. A rule's resource selection
/// (`ResourceTags`, `ExcludeResourceTags`) is NOT held to it: those members select the
/// resources a rule retains by tags those resources already carry, which creates, changes
/// and deletes nothing.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Whether `text` — one half of a tag, its key or its value — claims the reserved
/// namespace.
///
/// Compared case-INSENSITIVELY: what AWS reserves is the namespace, not one spelling of
/// it, so neither half may claim it in any casing. The comparison is over BYTES, so a
/// half whose leading characters are multi-byte answers `false` rather than slicing a
/// string inside a character — correct, since a half whose leading bytes are not the
/// prefix does not claim the namespace.
pub fn claims_reserved_tag_namespace(text: &str) -> bool {
    text.as_bytes()
        .get(..RESERVED_TAG_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_PREFIX.as_bytes()))
}

/// Whether any of `halves` claims the reserved namespace — what a door carrying tag
/// KEYS alone (`UntagResource$TagKeys`) checks before it writes anything, so a refused
/// call leaves the rule's tags untouched however many well-formed keys travelled beside
/// the reserved one.
pub fn names_reserved_tag_namespace<'a>(halves: impl IntoIterator<Item = &'a str>) -> bool {
    halves.into_iter().any(claims_reserved_tag_namespace)
}

/// The half of a tag a refusal names: the reserved namespace covers both, and a caller
/// acts on which one of them its request claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagHalf {
    Key,
    Value,
}

impl TagHalf {
    /// The word a validation clause names this half by — the modelled member names of
    /// `Tag` are `Key` and `Value` (smithy_query(query="shape", shape="Tag")), which a
    /// clause states in prose as "tag key" and "tag value".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Value => "value",
        }
    }
}

/// The half of the first tag of `tags` that claims the reserved namespace, or `None` when
/// no tag in the list claims it on either side.
///
/// A tag is checked key first, then value, so a tag claiming the namespace on both halves
/// is reported by its key — the half a caller names a tag by. Checked over the WHOLE list
/// before anything is written, so a refused call leaves the rule's tags untouched however
/// many well-formed tags travelled beside the reserved one.
pub fn reserved_tag_half<'a>(tags: impl IntoIterator<Item = &'a Tag>) -> Option<TagHalf> {
    tags.into_iter().find_map(|tag| {
        if claims_reserved_tag_namespace(&tag.key) {
            Some(TagHalf::Key)
        } else if claims_reserved_tag_namespace(&tag.value) {
            Some(TagHalf::Value)
        } else {
            None
        }
    })
}

/// The member path a refused `Tags` is named by.
pub const TAGS_MEMBER: &str = "tags";

/// The member path a refused `TagKeys` is named by.
pub const TAG_KEYS_MEMBER: &str = "tagKeys";

/// The modelled 400 for a request naming a tag key or a tag value in the reserved
/// namespace, in the envelope every member-level refusal of this service takes.
///
/// `member` is the member that carried it — the tag list a write assigns, or the key list
/// a removal names — and `half` is the half of the tag that claimed the namespace, so the
/// clause states which side of the tag the caller must change. The offending key or value
/// is NOT echoed back, as no other validation clause of this service echoes the value it
/// refused; the clause states the prefix a tag may not claim, which is the fact the
/// caller can act on.
pub fn reserved_tag_not_allowed(member: &str, half: TagHalf) -> ValidationException {
    member_validation_failed(
        member,
        &format!(
            "Member must not contain a tag {} beginning with {RESERVED_TAG_PREFIX}, \
             which is reserved for AWS use",
            half.noun()
        ),
    )
}

/// The two states `RuleStatus` models, kept here so every status-aware operation
/// names one spelling. "The state of the retention rule. Only retention rules that
/// are in the `available` state retain resources."
///
/// These are the same two spellings the generated validation list
/// [`crate::ops::types::RULE_STATUS_VALUES`] enumerates; `tests/all/rule_status_transitions.rs`
/// pins each of them to a member of that list, so the two cannot drift apart.
pub const RULE_STATUS_PENDING: &str = "pending";
pub const RULE_STATUS_AVAILABLE: &str = "available";

/// How long a rule stays `pending` before the clock makes `available` due.
///
/// A rule that has just been created or modified is reported `pending` and reaches
/// `available` on its own. Recycle Bin publishes no figure for how long that takes, so
/// this span is CHOSEN rather than cited: long enough that a caller acting straight
/// after a create or an update still observes the `pending` the real service reports,
/// short enough that a wait loop finishes promptly.
pub const STATUS_TRANSITION: Duration = Duration::from_millis(300);

/// The instant a rule that entered `pending` at `now` becomes `available`.
///
/// `checked_add` rather than `+`: a clock far enough in the future to overflow must
/// leave the rule immediately settled, not panic in a handler.
pub fn status_transition_end(now: SystemTime) -> SystemTime {
    now.checked_add(STATUS_TRANSITION).unwrap_or(now)
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
    /// The Region this rule exists in: the one the create was signed for, and the
    /// Region segment of [`Self::rule_arn`]. Every lookup is scoped by it, so no other
    /// Region's endpoint reports, resolves or counts this rule.
    pub region: String,
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
    /// When a `pending` rule's transition to `available` falls due. See
    /// [`STATUS_TRANSITION`].
    pub available_at: SystemTime,
    pub lock_configuration: Option<LockConfiguration>,
    /// `locked` | `pending_unlock` | `unlocked`; `None` means never locked, a state
    /// a rule can never return to once locked.
    pub lock_state: Option<String>,
    /// When a `pending_unlock` rule's unlock delay expires. `None` means no unlock
    /// delay is running, so the rule is inside no delay period: every reader below
    /// treats an absent end time as a delay that is already over, never as one without
    /// an end.
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
            // The protection lasts exactly as long as the delay period the end time
            // bounds. A rule carrying NO end time is within no delay period at all —
            // the lock end time is "only returned for retention rules that have been
            // unlocked and that are still within the unlock delay period" — so it is
            // not protected, rather than protected by a period that never ends.
            Some(LOCK_STATE_PENDING_UNLOCK) => self.lock_end_time.is_some_and(|end| now < end),
            _ => false,
        }
    }

    /// Whether the rule's own resource selection makes it one Recycle Bin cannot lock,
    /// as [`unlockable_rule`] decides.
    pub fn unlockable(&self) -> Option<UnlockableRule> {
        unlockable_rule(&self.resource_tags, &self.exclude_resource_tags)
    }

    /// Whether the rule holds a lock — a lock configuration, and with it a lock state.
    ///
    /// True for every rule that has ever been locked, whatever lock state the clock has
    /// since brought it to: "Once a retention rule has been locked, it can transition
    /// between the locked and unlocked states only; it can never transition back to
    /// null". So it is still true of an `unlocked` rule whose unlock delay has run out —
    /// the one window in which a rule that holds a lock can be modified at all.
    pub fn holds_lock(&self) -> bool {
        self.lock_configuration.is_some() || self.lock_state.is_some()
    }

    /// Whether the rule is `locked` under exactly the lock `requested` asks for — the
    /// unlock delay already in force.
    ///
    /// This is what tells a lock request that asks for the state the rule is already in
    /// from one that would CHANGE the lock in force: the first is satisfied by the
    /// stored rule, the second is the request a lock conflict refuses.
    pub fn locked_with(&self, requested: &LockConfiguration) -> bool {
        self.lock_state.as_deref() == Some(LOCK_STATE_LOCKED)
            && self
                .lock_configuration
                .as_ref()
                .is_some_and(|held| same_unlock_delay(&held.unlock_delay, &requested.unlock_delay))
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
    /// A `pending_unlock` rule carrying NO end time is inside no delay period, so the
    /// transition is due at once: the rule leaves `pending_unlock` on the next settle
    /// rather than staying in it for ever, which no later call could release.
    ///
    /// Nothing else about a lock moves on its own, so a rule in any other state is
    /// left exactly as it is.
    pub fn settle_lock_state(&mut self, now: SystemTime) {
        if self.lock_state.as_deref() == Some(LOCK_STATE_PENDING_UNLOCK)
            && self.lock_end_time.is_none_or(|end| now >= end)
        {
            self.lock_state = Some(LOCK_STATE_UNLOCKED.to_string());
            self.lock_end_time = None;
        }
    }

    /// Enter `pending` now and owe `available` one [`STATUS_TRANSITION`] later.
    ///
    /// Every call that mints or modifies a rule goes through this: "Only retention
    /// rules that are in the `available` state retain resources", and a rule that has
    /// just been created or changed is not retaining yet.
    pub fn begin_pending(&mut self, now: SystemTime) {
        self.status = RULE_STATUS_PENDING.to_string();
        self.available_at = status_transition_end(now);
    }

    /// Materialise the status transition the CLOCK has already made due: a `pending`
    /// rule whose transition span has elapsed is `available`, whether or not anybody
    /// called. Nothing else moves a rule's status, so a rule already `available` is
    /// left exactly as it is — the status never runs backwards.
    pub fn settle_status(&mut self, now: SystemTime) {
        if self.status == RULE_STATUS_PENDING && now >= self.available_at {
            self.status = RULE_STATUS_AVAILABLE.to_string();
        }
    }

    /// Materialise every transition the clock owes this rule — [`Self::settle_status`]
    /// and [`Self::settle_lock_state`] — so a caller is answered with the state the
    /// clock has actually reached rather than one it has already left.
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_status(now);
        self.settle_lock_state(now);
    }

    /// Whether every pair in `filter` appears among the rule's resource tags, a pair
    /// being the key together with its value and a tag carrying no value being the
    /// empty value. An empty filter selects every rule.
    pub fn has_resource_tags(&self, filter: &[ResourceTag]) -> bool {
        filter
            .iter()
            .all(|wanted| contains_resource_tag(&self.resource_tags, wanted))
    }

    /// Whether every pair in `filter` appears among the rule's exclusion tags, a pair
    /// being the key together with its value and a tag carrying no value being the
    /// empty value. An empty filter selects every rule.
    pub fn has_exclude_resource_tags(&self, filter: &[ResourceTag]) -> bool {
        filter
            .iter()
            .all(|wanted| contains_resource_tag(&self.exclude_resource_tags, wanted))
    }
}

/// Whether two unlock delays are the SAME period: the same value, measured in the same
/// unit.
///
/// `UnlockDelay` carries exactly those two members, both required
/// (smithy_query(query="shape", shape="UnlockDelay")), so they are the whole of a delay
/// and comparing them compares the periods. The unit is compared as it was sent: the
/// model declares one unit, and the boundary validator refuses any other before a
/// handler runs, so no two spellings of one period reach here.
pub fn same_unlock_delay(held: &UnlockDelay, requested: &UnlockDelay) -> bool {
    held.unlock_delay_value == requested.unlock_delay_value
        && held.unlock_delay_unit == requested.unlock_delay_unit
}

/// The instant an unlock requested at `now` leaves its unlock delay.
///
/// The delay is the one the rule's lock configuration carries, so it keeps the value
/// the lock request asked for: `UnlockDelayValue` units of `UnlockDelayUnit`, which
/// the model bounds at 7..=30 and measures only in `DAYS`.
///
/// Every input no delay can be measured from answers `now` — a delay that is over at
/// the instant it starts — the way [`status_transition_end`] answers the same
/// impossibility: a rule with no lock configuration, a unit the model does not define,
/// and a sum too far in the future to represent. Serving an END a caller can never
/// reach would instead leave the rule inside a delay period for ever, and a rule that
/// never leaves `pending_unlock` can never be modified or deleted again.
pub fn unlock_delay_end(
    now: SystemTime,
    lock_configuration: Option<&LockConfiguration>,
) -> SystemTime {
    let Some(configuration) = lock_configuration else {
        // No lock configuration carries no delay to serve.
        return now;
    };
    let delay = &configuration.unlock_delay;
    // The model bounds the value at 7..=30, so a value outside `u64` cannot reach a
    // handler; treat one as no delay rather than panicking if it ever does.
    let value = u64::try_from(delay.unlock_delay_value).unwrap_or(0);
    let seconds = match delay.unlock_delay_unit.as_str() {
        "DAYS" => value.saturating_mul(SECONDS_PER_DAY),
        // `UnlockDelayUnit` models only `DAYS`, and the boundary validator refuses an
        // unmodelled value before a handler runs, so this arm serves no delay rather
        // than guessing a scale for a unit the model does not define.
        _ => 0,
    };
    // `checked_add` rather than `+`: a clock far enough in the future to overflow
    // leaves the delay immediately over, not a panic in a handler.
    now.checked_add(Duration::from_secs(seconds)).unwrap_or(now)
}

/// A tag matches when it is the SAME resource tag pair as the filter names, as
/// [`resource_tag_pair`] identifies a pair: equal keys and equal values, where a tag
/// carrying no value IS the empty value — on the STORED side and on the FILTER side
/// alike.
///
/// Selecting and counting therefore agree about what one pair is: the pairs the
/// five-rules-per-pair quota refuses a request for as already taken are exactly the
/// pairs a filter naming them selects the holders of, whichever of the two forms of a
/// valueless pair either call writes.
fn contains_resource_tag(tags: &[ResourceTag], wanted: &ResourceTag) -> bool {
    let wanted = resource_tag_pair(wanted);
    tags.iter().any(|tag| resource_tag_pair(tag) == wanted)
}

// ---------------------------------------------------------------------------
// The modelled error BODIES this service answers with.
//
// Every one of them is composed here, from the request and from the rule's own state,
// so one refusal class cannot be worded two ways by two handlers — and so the gate in
// `crate::service`, which answers a constraint refusal before any handler runs, states
// it exactly as a handler-raised refusal of the same class does.
// ---------------------------------------------------------------------------

/// `ValidationExceptionReason` for a parameter value the service refuses: a value
/// outside a constraint the model declares, or a combination of members that cannot
/// hold together.
///
/// Source: smithy_query(query="shape", shape="ValidationExceptionReason") — values
/// `[INVALID_PAGE_TOKEN, INVALID_PARAMETER_VALUE]`.
pub const VALIDATION_REASON_INVALID_PARAMETER_VALUE: &str = "INVALID_PARAMETER_VALUE";

/// `ValidationExceptionReason` for a `NextToken` whose SYNTAX is fine but which this
/// service never issued — the other fault class that enum names. A token that fails
/// `NextToken`'s own `@pattern` is a parameter value fault, not this one.
pub const VALIDATION_REASON_INVALID_PAGE_TOKEN: &str = "INVALID_PAGE_TOKEN";

/// The modelled 400 for a `NextToken` this endpoint did not issue.
///
/// Composed here, beside the cursor codec it is the refusal of, so a pager cannot word
/// this fault class its own way. The message states the member and nothing of the
/// token: a page cursor is opaque, so there is nothing about the submitted value a
/// caller can act on.
pub fn invalid_page_token() -> ValidationException {
    ValidationException {
        message: Some("NextToken is not a valid page token".to_string()),
        reason: Some(VALIDATION_REASON_INVALID_PAGE_TOKEN.to_string()),
    }
}

/// `ConflictExceptionReason`'s sole value.
/// Source: smithy_query(query="shape", shape="ConflictExceptionReason").
pub const CONFLICT_REASON_INVALID_RULE_STATE: &str = "INVALID_RULE_STATE";

/// The modelled 404 for a retention rule the request addressed and this Region does
/// not hold — one that was never created, or one already deleted.
///
/// `identifier` is the rule's own `RuleIdentifier`, the single variable part of this
/// message: the `Identifier` an identifier-addressed operation binds, or — for an
/// operation that addresses the rule by ARN — the identifier that ARN names
/// ([`rule_identifier_in_arn`]), never the ARN itself. So a rule that does not exist
/// reads the same way whichever operation reports it, and the message states nothing of
/// the partition, Region or account the caller happened to address it through.
///
/// `Reason` is left UNSET. It is an optional member of `ResourceNotFoundException`
/// (smithy_query(query="shape", shape="ResourceNotFoundException") — only `Message` and
/// `Reason`, neither required), and the service does not populate it when an addressed
/// rule does not exist: the body carries `Message` alone. A declared enum value is a
/// statement about what the member may hold, not that the member is sent, so nothing is
/// synthesized here — a caller that branches on `Reason` being present must see it
/// absent on this path. The other reason members this service DOES send
/// (`ConflictException`, `ValidationException`, `ServiceQuotaExceededException`) are
/// each populated by their own constructor below.
pub fn rule_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Recycle Bin Rule '{identifier}' does not exist.")),
        reason: None,
    }
}

/// The resource segment a retention rule's ARN carries: `rule/` then the rule's
/// `Identifier`, as the modelled `RuleArn` pattern spells it
/// (`^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}$`, from
/// smithy_query(query="shape", shape="RuleArn")). Stated once, so the ARN this service
/// MINTS and the identifier it reads back out of one cannot disagree about the spelling.
pub const RULE_RESOURCE_PREFIX: &str = "rule/";

/// The `Identifier` a rule ARN names: the identifier in its `rule/<Identifier>` resource
/// segment.
///
/// The resource segment is what follows the ARN's last `:` — an ARN's six fields are
/// `:`-separated and a rule's resource segment holds none — and the identifier is that
/// segment without its [`RULE_RESOURCE_PREFIX`]. An argument carrying neither (a value
/// the modelled `RuleArn` pattern would not admit, so not one a handler is reached with)
/// yields what it holds, rather than an empty identifier.
///
/// MIGRATION NOTE: `services/common/src/arn.rs` owns this tree's ARN reading
/// (`arn::resource_id_of_type(value, "rule")` is exactly this read), and a crate that
/// already depends on that module should call it rather than split an ARN again. This
/// crate depends on it for nothing else, so the two lines stay here until it does; they
/// are not to be copied further.
pub fn rule_identifier_in_arn(rule_arn: &str) -> &str {
    let resource = rule_arn.rsplit(':').next().unwrap_or(rule_arn);
    resource.strip_prefix(RULE_RESOURCE_PREFIX).unwrap_or(resource)
}

/// What a refused request asked to do to the rule, as the conflict message names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedAction {
    /// `LockRule`.
    Lock,
    /// `UnlockRule`.
    Unlock,
    /// `UpdateRule`: every change to a rule is "modify" in the lock documentation
    /// ("The retention rule is locked and can't be modified or deleted").
    Modify,
    /// `DeleteRule`.
    Delete,
}

impl BlockedAction {
    /// The verb the message states this action with.
    fn verb(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Unlock => "unlock",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }
}

/// The rule's own condition that blocks the request — the second half of a conflict
/// message, and the only thing a caller can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingCondition {
    /// The rule's lock still forbids modifying or deleting it.
    ///
    /// Reported for `locked` AND for `pending_unlock` while the unlock delay is still
    /// running — the model says such a rule "can be modified or deleted only after the
    /// unlock delay period has expired", so to the caller it is still locked.
    /// `pending_unlock` is an internal step of releasing that lock, not a different
    /// answer to "why was this refused", so the stored lock-state token stays out of the
    /// message.
    Locked,
    /// The rule is already `locked` under a DIFFERENT unlock delay than the request
    /// asked for, so completing the request would change the lock in force.
    DifferentUnlockDelay,
    /// The rule has no lock to release: it has never been locked, is `unlocked`, or is
    /// already serving its unlock delay.
    NotLocked,
    /// A tag-level rule (one carrying resource tags): "You can't lock tag-level
    /// retention rules".
    TagLevelRule,
    /// A Region-level rule carrying exclusion tags, which cannot be locked either:
    /// "or Region-level retention rules that have exclusion tags".
    RuleWithExclusionTags,
}

impl BlockingCondition {
    /// How the message describes the condition — the whole predicate, after "because
    /// it", so a condition that is something the rule HAS reads as naturally as one it
    /// IS.
    fn describe(self) -> &'static str {
        match self {
            Self::Locked => "is locked",
            Self::DifferentUnlockDelay => "has a different unlock delay already set",
            Self::NotLocked => "is not locked",
            Self::TagLevelRule => "is a tag-level rule",
            Self::RuleWithExclusionTags => "is a Region-level rule with exclusion tags",
        }
    }
}

/// Why a retention rule's resource selection makes it one Recycle Bin cannot hold a
/// lock on.
///
/// "You can't lock tag-level retention rules, or Region-level retention rules that have
/// exclusion tags" (smithy_query(query="doc", shape="LockRule")): a rule carrying
/// resource tags is tag-level, and one carrying exclusion tags is a Region-level rule
/// with exclusions. The invariant lives HERE, in one place, because every door the
/// combination can arrive through is held to it — `LockRule`, which refuses the rule the
/// request addressed, `CreateRule`, which refuses a request that would mint the same
/// combination, and `UpdateRule`, which refuses a request that would change a
/// lock-holding rule's selection into one — so no two of them can disagree about which
/// rules a lock can hold on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockableRule {
    /// The rule selects resources by resource tags, which makes it tag-level.
    TagLevel,
    /// The rule is Region-level and carries exclusion tags.
    ExclusionTags,
}

impl UnlockableRule {
    /// The blocking condition a 409 names when the rule a lock request ADDRESSED is one
    /// of these.
    pub fn blocking_condition(self) -> BlockingCondition {
        match self {
            Self::TagLevel => BlockingCondition::TagLevelRule,
            Self::ExclusionTags => BlockingCondition::RuleWithExclusionTags,
        }
    }

    /// The requirement the lock configuration failed, for the validation clause a 400
    /// names when the request would CREATE one of these. A tag-level rule's sentence is
    /// the one [`TAG_LEVEL_MEMBER_REQUIREMENT`] states for every member that cannot be
    /// specified for such a rule.
    fn lock_configuration_requirement(self) -> &'static str {
        match self {
            Self::TagLevel => TAG_LEVEL_MEMBER_REQUIREMENT,
            Self::ExclusionTags => {
                "Member must not be specified for a retention rule with exclusion tags"
            }
        }
    }

    /// The member that carries the selection making a rule one of these — the member a
    /// validation clause names when the request CHANGES the selection of a rule that
    /// already holds a lock, rather than asking for the lock itself.
    pub fn selection_member(self) -> &'static str {
        match self {
            Self::TagLevel => RESOURCE_TAGS_MEMBER,
            Self::ExclusionTags => EXCLUDE_RESOURCE_TAGS_MEMBER,
        }
    }
}

/// Whether a rule that retains what `resource_tags` select, less what
/// `exclude_resource_tags` exclude, is one Recycle Bin cannot lock — and why.
///
/// `None` only for a Region-level rule with no exclusion tags, the one kind a lock can
/// hold on.
pub fn unlockable_rule(
    resource_tags: &[ResourceTag],
    exclude_resource_tags: &[ResourceTag],
) -> Option<UnlockableRule> {
    if !resource_tags.is_empty() {
        Some(UnlockableRule::TagLevel)
    } else if !exclude_resource_tags.is_empty() {
        Some(UnlockableRule::ExclusionTags)
    } else {
        None
    }
}

/// The member path a refused `LockConfiguration` is named by, as a validation clause
/// spells a member: the modelled member name, lower camel case.
pub const LOCK_CONFIGURATION_MEMBER: &str = "lockConfiguration";

/// The member path a refused `ExcludeResourceTags` is named by.
pub const EXCLUDE_RESOURCE_TAGS_MEMBER: &str = "excludeResourceTags";

/// The member path a refused `ResourceTags` is named by.
pub const RESOURCE_TAGS_MEMBER: &str = "resourceTags";

/// The requirement ANY member fails by being specified for a tag-level retention rule —
/// a rule that selects what it retains by resource tags. Two members fail it, for
/// reasons the service states separately, so the sentence is written once here and both
/// name it: `ExcludeResourceTags` ("[Region-level retention rules only] ... You can't
/// specify exclusion tags for tag-level retention rules") through
/// [`EXCLUDE_RESOURCE_TAGS_REQUIREMENT`], and `LockConfiguration` ("You can't lock
/// tag-level retention rules") through [`UnlockableRule::lock_configuration_requirement`].
const TAG_LEVEL_MEMBER_REQUIREMENT: &str =
    "Member must not be specified for a tag-level retention rule";

/// The requirement an `ExcludeResourceTags` fails on a tag-level rule: the two members
/// select resources in ways that cannot hold together.
pub const EXCLUDE_RESOURCE_TAGS_REQUIREMENT: &str = TAG_LEVEL_MEMBER_REQUIREMENT;

/// The modelled 400 for a resource selection that names BOTH resource tags and exclusion
/// tags, or `None` when the selection is one a retention rule can hold.
///
/// The model cannot express the combination: resource tags make a rule tag-level, and
/// exclusion tags are "[Region-level retention rules only] ... You can't specify exclusion
/// tags for tag-level retention rules" (smithy_query(query="doc", shape="CreateRuleRequest",
/// member="ExcludeResourceTags")). The refusal is stated in the same envelope a modelled
/// constraint takes: the member that cannot hold, and what it must satisfy.
///
/// Every door onto a rule's resource selection goes through this: `CreateRule`, where both
/// lists are the ones the request names, and `UpdateRule`, where each list is the one the
/// request names or, absent, the stored one, so no sequence of calls leaves a rule
/// selecting resources both ways at once.
pub fn selection_combination_unsupported(
    resource_tags: &[ResourceTag],
    exclude_resource_tags: &[ResourceTag],
) -> Option<ValidationException> {
    if resource_tags.is_empty() || exclude_resource_tags.is_empty() {
        return None;
    }
    Some(member_validation_failed(
        EXCLUDE_RESOURCE_TAGS_MEMBER,
        EXCLUDE_RESOURCE_TAGS_REQUIREMENT,
    ))
}

/// The modelled 400 for a request asking to lock a rule Recycle Bin cannot lock.
///
/// The lock configuration is the member that cannot hold: the rule itself is one the
/// service creates happily without a lock, so the request is refused for asking to lock
/// it rather than for the way it selects resources.
pub fn lock_configuration_not_allowed(blocker: UnlockableRule) -> ValidationException {
    member_validation_failed(
        LOCK_CONFIGURATION_MEMBER,
        blocker.lock_configuration_requirement(),
    )
}

/// The requirement a selection member fails by being specified for a retention rule that
/// already holds a lock: a lock holds only on a Region-level rule with no exclusion tags,
/// so neither selection member can be given a value on such a rule.
const LOCK_HOLDING_MEMBER_REQUIREMENT: &str =
    "Member must not be specified for a retention rule with a lock configuration";

/// The modelled 400 for a request that would change the resource selection of a rule
/// HOLDING a lock into one a lock cannot hold on.
///
/// The mirror image of [`lock_configuration_not_allowed`], over the same invariant read
/// from the other side: there the selection is settled and the lock is the member that
/// cannot hold, here the lock is settled and the selection the request names is the
/// member that cannot hold, so the clause names that member.
pub fn lock_holding_selection_not_allowed(blocker: UnlockableRule) -> ValidationException {
    member_validation_failed(blocker.selection_member(), LOCK_HOLDING_MEMBER_REQUIREMENT)
}

/// The member path a refused `RetentionPeriodValue` is named by — the nested member, as a
/// validation clause spells a path: each modelled member name in lower camel case, joined
/// by dots.
pub const RETENTION_PERIOD_VALUE_MEMBER: &str = "retentionPeriod.retentionPeriodValue";

/// `ResourceType`'s values, so every rule read off a rule's type names one spelling.
/// Source: smithy_query(query="shape", shape="ResourceType") — the same value set
/// [`crate::ops::types::RESOURCE_TYPE_VALUES`] enumerates for the boundary validator.
pub const RESOURCE_TYPE_EBS_SNAPSHOT: &str = "EBS_SNAPSHOT";
pub const RESOURCE_TYPE_EBS_VOLUME: &str = "EBS_VOLUME";
pub const RESOURCE_TYPE_EC2_IMAGE: &str = "EC2_IMAGE";

/// The longest retention period, in days, Recycle Bin retains a deleted EBS VOLUME for.
///
/// Source: smithy_query(query="doc", shape="RetentionPeriod",
/// member="RetentionPeriodValue") — "The supported retention periods are: EBS volumes:
/// 1 - 7 days".
pub const MAX_RETENTION_DAYS_EBS_VOLUME: i32 = 7;

/// The longest retention period, in days, Recycle Bin retains a deleted EBS SNAPSHOT or
/// EBS-backed AMI for.
///
/// Source: the same member documentation — "EBS snapshots and EBS-backed AMIs: 1 - 365
/// days".
pub const MAX_RETENTION_DAYS_SNAPSHOT_OR_IMAGE: i32 = 365;

/// The longest retention period, in days, the service retains `resource_type` for — the
/// per-type bound the member's own documentation publishes, which is tighter than the
/// `@range` the model declares (that range is the union over every type, so it admits a
/// period no single type supports).
///
/// `None` for a resource type this catalog does not name, which leaves such a type held
/// to the modelled range alone rather than to a bound nothing publishes.
pub fn max_retention_period_days(resource_type: &str) -> Option<i32> {
    match resource_type {
        RESOURCE_TYPE_EBS_VOLUME => Some(MAX_RETENTION_DAYS_EBS_VOLUME),
        RESOURCE_TYPE_EBS_SNAPSHOT | RESOURCE_TYPE_EC2_IMAGE => {
            Some(MAX_RETENTION_DAYS_SNAPSHOT_OR_IMAGE)
        }
        _ => None,
    }
}

/// The modelled 400 for a retention period longer than the resource type it would be
/// retained for supports, or `None` when the period is one that type supports.
///
/// `RetentionPeriodUnit` models `DAYS` alone — "Currently, only `DAYS` is supported" — so
/// the period's value IS its length in days and is read against the per-type maximum
/// directly. The clause is worded the way a range the boundary validator refuses is
/// worded, so a caller reads one form whichever bound a period failed, and it names the
/// bound rather than echoing the value the request sent.
///
/// Every door onto a rule's retention period goes through this: `CreateRule`, where the
/// type is the one the request names, and `UpdateRule`, where it is the stored one (the
/// type cannot be updated), so no sequence of calls leaves a rule holding a period the
/// service could not retain.
pub fn retention_period_unsupported(
    resource_type: &str,
    retention_period: &RetentionPeriod,
) -> Option<ValidationException> {
    let max = max_retention_period_days(resource_type)?;
    if retention_period.retention_period_value <= max {
        return None;
    }
    Some(member_validation_failed(
        RETENTION_PERIOD_VALUE_MEMBER,
        &format!("Member must have value less than or equal to {max}"),
    ))
}

/// The modelled 409 for a lock, modify or delete the rule's own state forbids.
///
/// Composed from the identifier the request addressed and the condition that blocks it,
/// which is the form Recycle Bin states a refused lock/modify/delete with. The rule's
/// stored lock-state token is never interpolated: it is an internal spelling, and a
/// caller is told the condition instead.
pub fn rule_state_conflict(
    action: BlockedAction,
    identifier: &str,
    condition: BlockingCondition,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Cannot {} Recycle Bin rule {identifier} because it {}.",
            action.verb(),
            condition.describe()
        )),
        reason: Some(CONFLICT_REASON_INVALID_RULE_STATE.to_string()),
    }
}

/// The modelled 400 for the constraints a request violated, as the framework's own
/// constraint walk reports them. `None` when nothing was violated.
///
/// `Reason` is the fault class, not the member: every value that fails a bound the
/// model declares is an `INVALID_PARAMETER_VALUE`, including a `NextToken` that fails
/// its `@pattern` (an unusable page token whose syntax is fine is the service's own
/// [`VALIDATION_REASON_INVALID_PAGE_TOKEN`] fault, raised by the handler that decodes
/// it).
pub fn constraint_violations(
    violations: &[Violation],
    uri_path_labels: &[String],
) -> Option<ValidationException> {
    Some(ValidationException {
        message: Some(validation_envelope(violations, uri_path_labels)?),
        reason: Some(VALIDATION_REASON_INVALID_PARAMETER_VALUE.to_string()),
    })
}

/// The modelled 400 for a rule the MODEL cannot express — a combination of members that
/// cannot hold together — stated in the same envelope a modelled constraint takes, so a
/// caller reads one form for both.
pub fn member_validation_failed(member_path: &str, requirement: &str) -> ValidationException {
    ValidationException {
        message: Some(envelope_of(&[clause(member_path, requirement)])),
        reason: Some(VALIDATION_REASON_INVALID_PARAMETER_VALUE.to_string()),
    }
}

/// Every violated constraint as one `ValidationException` message. `None` when
/// `violations` is empty, so a caller cannot render "0 validation errors detected".
pub fn validation_envelope(violations: &[Violation], uri_path_labels: &[String]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(|violation| constraint_clause(violation, uri_path_labels))
        .collect();
    Some(envelope_of(&clauses))
}

/// `1 validation error detected: <clause>` / `N validation errors detected: <clause>;
/// <clause>` — the envelope AWS counts and joins its validation clauses in.
fn envelope_of(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// One clause: the member path the request named, then the requirement it failed.
fn clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The marker a clause splits at: what precedes it names the member, what follows states
/// the requirement.
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// One violated constraint as a clause.
///
/// The REQUIREMENT half is the framework's own wording for that constraint class
/// ([`Violation::message`]), which is where the model's declared bound — a range
/// endpoint, a `@pattern`, an enum's value set — is rendered, so no bound text is
/// restated here. Two things change: Recycle Bin names the member that failed without
/// quoting the submitted value back, and it quotes a `@pattern` as the expression the
/// service PUBLISHES as that member's requirement — the model's expression for a member
/// the request carries in its body or query string, the spliced one for a member bound
/// into the URI path (see [`quoted_pattern`]).
///
/// The requirement is read off the wording for the SAME violation with no value
/// attached, so the submitted value is nowhere in the text this splits and cannot leak
/// into the clause whatever it holds; the split takes the LAST marker, so neither a
/// value nor a caller-chosen map key inside the member path can move it.
///
/// An absent required member states no value either — the framework's
/// `Value null at '<path>' …` quotes nothing of the request — so that sentence stands.
fn constraint_clause(violation: &Violation, uri_path_labels: &[String]) -> String {
    let unechoed = Violation {
        path: violation.path.clone(),
        kind: match &violation.kind {
            ViolationKind::Pattern(pattern) => {
                ViolationKind::Pattern(quoted_pattern(pattern, &violation.path, uri_path_labels))
            }
            other => other.clone(),
        },
        value: None,
        sensitive: violation.sensitive,
    };
    let stated = unechoed.message();
    if violation.value.is_none() {
        return stated;
    }
    match stated.rsplit_once(CONSTRAINT_MARKER) {
        Some((_subject, requirement)) => clause(&violation.path, requirement),
        // The framework no longer words a clause with that marker: keep its sentence,
        // which (built with no value attached) still echoes nothing of the request.
        None => stated,
    }
}

/// A `@pattern` as Recycle Bin quotes it in a validation message: the expression the
/// service publishes as the requirement for the member that failed.
///
/// Which expression that is follows from WHERE the request carried the member, because
/// the two places are matched against two expressions:
///
/// * a member bound into the URI PATH (`@httpLabel`) is matched as one path segment of
///   the route, so the expression its value is held to is the member's spliced into the
///   route's — the modelled one without its whole-value anchors, which is the expression
///   quoted back ([`unanchored`]);
/// * a member the request carries in its BODY or its query string is matched against the
///   member's own expression, so the modelled `@pattern` is quoted exactly as declared,
///   anchors included.
///
/// `uri_path_labels` are the lower-camel member paths the operation binds into the URI
/// path, read off the model's own `@httpLabel` traits by `crate::service`, so which
/// members those are is nowhere restated here. A nested path
/// (`retentionPeriod.retentionPeriodValue`, `tags.1.member`) is placed by its FIRST
/// segment, the top-level member the whole subtree hangs off — the only segment an HTTP
/// binding can be declared on.
fn quoted_pattern(pattern: &str, member_path: &str, uri_path_labels: &[String]) -> String {
    let top_level = member_path.split('.').next().unwrap_or(member_path);
    if uri_path_labels.iter().any(|label| label == top_level) {
        unanchored(pattern).to_string()
    } else {
        pattern.to_string()
    }
}

/// `pattern` without the `^` and `$` anchors a model spells a whole-value match with —
/// the expression a URI path segment is matched against once the member's own expression
/// is spliced into the route's.
///
/// A `$` escaped by a backslash is a literal dollar sign inside the expression, not the
/// end anchor, so it stays.
///
/// MIGRATION NOTE, as `services/common/src/validation.rs` ("Still to migrate here") asks
/// of anything that inlines this again: the DROP itself is the same for every service and
/// belongs in that shared module, which already owns it as `validation::unanchored`; this
/// copy is to be deleted by the change that owns that migration rather than copied
/// further. What stays with this service is WHICH members are quoted this way
/// ([`quoted_pattern`]), which that module's header lists as the service's own half.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// Per-service state for com-amazonaws-rbin-amazonrecyclebin. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRbinAmazonrecyclebinState {
    /// Every retention rule of every Region, keyed by `Identifier` and ordered so
    /// paging is stable. A rule belongs to exactly one Region ([`Rule::region`]), and
    /// the resolvers below are the only way a handler reaches one, so the key order is
    /// shared while the rules themselves stay partitioned.
    pub rules: BTreeMap<String, Rule>,
    /// `RuleArn` -> `Identifier`, so the ARN-addressed tag operations resolve a rule
    /// without scanning the map. An ARN carries its rule's Region, so an entry is
    /// still reached through a Region-scoped resolver. Every key is an ARN minted from a
    /// Region the modelled `RuleArn` pattern admits ([`request_region`]), so it is a
    /// value those operations can be asked for: a caller can always hand back what a
    /// create reported.
    pub arn_index: HashMap<String, String>,
    /// Monotonic counter behind [`Self::mint_identifier`]. A fresh store starts it at
    /// [`INITIAL_IDENTIFIER_SEED`], the one seed whose rendering is never handed out.
    pub next_identifier_seed: u64,
}

impl ComAmazonawsRbinAmazonrecyclebinState {
    /// Mint an identifier no stored rule holds, as 11 base62 digits.
    ///
    /// The seed only ever rises and [`base62_identifier`] renders it order preserving
    /// under the byte comparison [`Self::rules`] orders its keys by, so the map's key
    /// order is also creation order and a freshly minted identifier sorts after every
    /// stored one; the loop skips a candidate that is somehow already taken rather than
    /// overwriting a rule.
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

    /// The rule `identifier` addresses in `region`, for a reporting operation.
    ///
    /// `None` for an identifier no rule holds AND for one held by a rule that belongs
    /// to another Region: a retention rule exists only in the Region it was created in,
    /// so the Region the request was signed for holds no such rule either way and the
    /// caller reports the modelled 404.
    pub fn rule(&self, region: &str, identifier: &str) -> Option<&Rule> {
        self.rules
            .get(identifier)
            .filter(|rule| rule.region == region)
    }

    /// The rule `identifier` addresses in `region`, for an operation that mutates it —
    /// the write-side twin of [`Self::rule`], scoped the same way.
    pub fn rule_mut(&mut self, region: &str, identifier: &str) -> Option<&mut Rule> {
        self.rules
            .get_mut(identifier)
            .filter(|rule| rule.region == region)
    }

    /// Remove the rule `identifier` addresses in `region` and drop its ARN index entry.
    /// `None` when that Region holds no such rule — including one another Region holds,
    /// which this Region's endpoint may not reach, let alone delete.
    pub fn remove_rule(&mut self, region: &str, identifier: &str) -> Option<Rule> {
        self.rule(region, identifier)?;
        let removed = self.rules.remove(identifier)?;
        self.arn_index.remove(&removed.rule_arn);
        Some(removed)
    }

    /// The rule an ARN addresses in `region`, resolved through [`Self::arn_index`] and
    /// scoped as [`Self::rule`] scopes an identifier.
    pub fn rule_by_arn(&self, region: &str, rule_arn: &str) -> Option<&Rule> {
        let identifier = self.arn_index.get(rule_arn)?;
        self.rule(region, identifier)
    }

    /// The rule an ARN addresses in `region`, for modification — the write-side twin of
    /// [`Self::rule_by_arn`], used by the ARN-addressed tag operations.
    pub fn rule_by_arn_mut(&mut self, region: &str, rule_arn: &str) -> Option<&mut Rule> {
        let identifier = self.arn_index.get(rule_arn)?.clone();
        self.rule_mut(region, &identifier)
    }

    /// Every rule `region` holds, in the store's stable identifier order.
    ///
    /// What a regional reader walks and what a per-Region quota counts: a rule belongs
    /// to exactly one Region, the one its ARN names.
    pub fn rules_in_region<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Rule> + 'a {
        self.rules.values().filter(move |rule| rule.region == region)
    }

    /// How many retention rules `region` already holds — the count [`MAX_RULES`] bounds.
    pub fn rule_count_in_region(&self, region: &str) -> usize {
        self.rules_in_region(region).count()
    }

    /// Whether assigning `tags` as a rule's resource tags would make some tag key and
    /// value pair the selector of more retention rules than
    /// [`MAX_RULES_PER_RESOURCE_TAG`] allows.
    ///
    /// A pair is counted over the rules OTHER than `excluding`, the rule the request is
    /// assigning to: a rule already selecting on a pair does not count against itself,
    /// so re-sending what a rule already holds is never refused. A create names no rule
    /// and passes `None`. The pairs the request repeats are counted once, since a rule
    /// selects on a pair whether it names it once or twice.
    ///
    /// Only `region`'s rules are counted: a retention rule exists in one Region, so a
    /// pair selecting rules elsewhere selects nothing here.
    pub fn exceeds_resource_tag_quota(
        &self,
        region: &str,
        tags: &[ResourceTag],
        excluding: Option<&str>,
    ) -> bool {
        let mut counted: BTreeSet<(&str, &str)> = BTreeSet::new();
        tags.iter().any(|tag| {
            let pair = resource_tag_pair(tag);
            counted.insert(pair)
                && self
                    .rules_in_region(region)
                    .filter(|rule| excluding.is_none_or(|identifier| rule.identifier != identifier))
                    .filter(|rule| {
                        rule.resource_tags
                            .iter()
                            .any(|held| resource_tag_pair(held) == pair)
                    })
                    .count()
                    >= MAX_RULES_PER_RESOURCE_TAG
        })
    }

    /// Materialise every transition the CLOCK has already made due for each rule
    /// `region` holds — [`Rule::settle`] applied across the Region a reader walks.
    ///
    /// The write-side twin of [`Self::rules_in_region`], for an operation that reads
    /// MANY rules at once: each of those transitions was owed at this instant whether
    /// or not anybody called, so a pager that settles first reports — and filters on —
    /// the state the clock reached rather than the one the last writer left behind, and
    /// agrees with what a single-rule reader answers for the same record.
    pub fn settle_rules_in_region(&mut self, region: &str, now: SystemTime) {
        for rule in self.rules.values_mut().filter(|rule| rule.region == region) {
            rule.settle(now);
        }
    }

    /// Whether `identifier` is one this store has already MINTED.
    ///
    /// [`Self::mint_identifier`] ADVANCES the seed before rendering it, and the seed only
    /// ever rises, so the seeds it has rendered are exactly those after
    /// [`INITIAL_IDENTIFIER_SEED`] up to and including the current one.
    /// [`base62_identifier`] renders every seed as the same number of digits in
    /// ascending-byte order and so maps that span of seeds onto exactly the span of
    /// renderings between the two bounds below. An identifier-shaped value ABOVE the
    /// upper bound names a position this store has not reached yet, and one AT OR BELOW
    /// the lower bound names a seed it never rendered at all: nothing could have been
    /// minted at either, whether or not a rule still holds it today.
    pub fn has_minted_identifier(&self, identifier: &str) -> bool {
        is_rule_identifier(identifier)
            && identifier > base62_identifier(INITIAL_IDENTIFIER_SEED).as_str()
            && identifier <= base62_identifier(self.next_identifier_seed).as_str()
    }

    /// The position `token` resumes a page of `region` at, or `None` when this endpoint
    /// did not issue it.
    ///
    /// A cursor is honoured only when it is one of THIS endpoint's own: it has to decode
    /// to the cursor form [`encode_page_token`] renders, to have been issued by the
    /// Region this request is signed for, and to name a position this store has actually
    /// minted ([`Self::has_minted_identifier`]). Anything else is no cursor at all — a
    /// value a client invented that merely satisfies `NextToken`'s `@pattern`, or a
    /// cursor another Region's endpoint issued — and resuming "after" a position never
    /// handed out would answer a page that silently drops every rule sorting at or
    /// before it (an empty one, for a value sorting above them all), so the caller
    /// refuses it with [`invalid_page_token`] instead.
    pub fn page_cursor(&self, region: &str, token: &str) -> Option<String> {
        let cursor = parse_page_token(token)?;
        (cursor.region == region && self.has_minted_identifier(&cursor.identifier))
            .then_some(cursor.identifier)
    }

    /// The rules of `region` to walk for a page, starting after `after` when a
    /// `NextToken` named one.
    ///
    /// Region-scoped like every other resolver: a regional reader pages through the
    /// rules its own Region holds, and `after` is a cursor [`Self::page_cursor`] has
    /// resolved, so it names a position that Region's endpoint itself handed out.
    pub fn page_from<'a>(
        &'a self,
        region: &'a str,
        after: Option<&str>,
    ) -> impl Iterator<Item = (&'a String, &'a Rule)> + 'a {
        let lower = match after {
            Some(identifier) => Bound::Excluded(identifier.to_string()),
            None => Bound::Unbounded,
        };
        self.rules
            .range((lower, Bound::Unbounded))
            .filter(move |(_, rule)| rule.region == region)
    }
}

/// The seed a fresh store's counter starts at — what `#[derive(Default)]` leaves in
/// [`ComAmazonawsRbinAmazonrecyclebinState::next_identifier_seed`].
///
/// [`ComAmazonawsRbinAmazonrecyclebinState::mint_identifier`] advances the counter
/// BEFORE it renders a candidate, so this seed's own rendering is the one
/// identifier-shaped value no store state can ever have handed out: the first identifier
/// minted is the rendering of the seed after it.
const INITIAL_IDENTIFIER_SEED: u64 = 0;

/// Render `seed` as exactly [`IDENTIFIER_LEN`] base62 digits, most significant
/// first and left-padded with `0`, so the result satisfies `^[0-9a-zA-Z]{11}$` and
/// sorts in the same order as the seed.
///
/// Order preserving under BYTE comparison, which is how the rule map compares its keys:
/// every rendering is the same length, the digits are most significant first, and
/// [`BASE62_DIGITS`] maps a higher digit value to a higher byte. So `seed_a < seed_b`
/// implies `base62_identifier(seed_a) < base62_identifier(seed_b)` as strings, and a
/// freshly minted identifier sorts after every identifier minted before it.
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
///
/// The scope's region is caller-written text that the scope parser checks only for being
/// present, so it is held to the Region shape ([`is_region_code`]) before this service
/// serves a request in it: the Region reaches the wire as the Region segment of every
/// ARN this service mints and reports, and that segment can only hold a Region the
/// modelled `RuleArn` pattern admits. A scope naming anything else names no Region of
/// this service, so the request is served in [`DEFAULT_REGION`], exactly as an unsigned
/// one is — rather than minting an ARN no member of this service may carry and no
/// ARN-addressed operation could be asked for again.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_code(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// What a page cursor carries: the Region whose endpoint issued it, and the identifier
/// of the last rule that page reported.
///
/// The Region is part of the cursor because a retention rule — and so a page of rules —
/// exists in one Region: a position one endpoint handed out is no position of another's
/// walk, so a cursor states which endpoint's walk it belongs to and any other endpoint
/// refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageCursor {
    region: String,
    identifier: String,
}

/// What separates a cursor's two parts. A Region is read off the SigV4 credential
/// scope, whose fields are themselves `/`-separated ([`request_region`]), so a Region
/// string holds no `/` and the LAST one in the payload opens the identifier.
const PAGE_CURSOR_SEPARATOR: char = '/';

/// Encode a page cursor: the base64 of the issuing Region and the last identifier the
/// page returned.
///
/// Base64 is what makes the token opaque to a client, and its alphabet is exactly
/// the `NextToken` pattern `^[A-Za-z0-9+/=]{1,2048}$`.
pub fn encode_page_token(region: &str, identifier: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{region}{PAGE_CURSOR_SEPARATOR}{identifier}"
    ))
}

/// Decode a token back to the cursor it carries, or `None` when it is not base64 of
/// anything [`encode_page_token`] renders.
///
/// A payload that parses is still only a well-formed cursor: whether this endpoint
/// issued it is [`ComAmazonawsRbinAmazonrecyclebinState::page_cursor`]'s question,
/// since answering it needs the Region the request is signed for and the store's own
/// minting watermark.
fn parse_page_token(token: &str) -> Option<PageCursor> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    let payload = String::from_utf8(bytes).ok()?;
    let (region, identifier) = payload.rsplit_once(PAGE_CURSOR_SEPARATOR)?;
    (!region.is_empty() && is_rule_identifier(identifier)).then(|| PageCursor {
        region: region.to_string(),
        identifier: identifier.to_string(),
    })
}

/// The identifier a page cursor names, for a reader that already knows which Region's
/// walk the cursor belongs to. `None` when `token` carries no cursor at all.
pub fn decode_page_token(token: &str) -> Option<String> {
    Some(parse_page_token(token)?.identifier)
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
